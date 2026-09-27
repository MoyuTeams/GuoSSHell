//! rsHell on iOS —— M0 / M1 编译-链接探针。
//!
//! 这不是产品代码。它存在的唯一目的，是把「Flutter 前端 + Rust 业务内核」路线上
//! **两件无法靠读源码确认的事**变成实跑结果：
//!
//! * **M0**：`NativeSshTransport`（russh + `tokio::net::TcpStream` + `request_pty` +
//!   `request_shell`）能否为 `aarch64-apple-ios` 编译并链接；并且在 iOS 上
//!   **不触碰** `fork` / `openpty` / `posix_spawn`。这是「业务仍然用 Rust 跑」的
//!   全部前提——SSH 通了，终端状态机和渲染帧才有意义。
//!
//! * **M1**：`DefaultTerminalEngine`（alacritty 网格 + `RenderFrame`）能否为 iOS
//!   编译，且 `RenderFrame` 能直接走 serde 序列化——这是 rinf 边界的先决条件。
//!
//! 两个函数都不依赖 `rshell-ui`（GTK/Relm4，22812 行），也不依赖 `rshell-storage`
//! 的 keyring 路径：M0 阶段凭证写死在 Rust 侧，`AuthPlan::from_secret` 不读 vault。

use std::sync::Arc;

// 对下游（`native/hub`，rinf 的信号层）再导出上游内核：
// 上游 git 依赖只在本 crate 的 Cargo.toml 里 pin 一次 rev，
// hub 通过这里拿类型，避免第二处 rev 需要同步升级。
pub use rshell_core;
pub use rshell_session;
pub use rshell_storage;
// 与 rshell-session 锁在同一版本（见 Cargo.toml）：hub 解析私钥用它，类型与上游一致。
pub use russh;

/// 把平台的钥匙串设为 keyring 的默认存储（`SystemCredentialVault` 经它读写）。
///
/// keyring v1 在 macOS / Windows / Linux 上会自己注册，iOS 上什么都不做——
/// 这里补上 iOS 的 protected data store 与 Android 的 Keystore 加密存储。
/// Android 宿主必须先初始化应用上下文；其他平台由 keyring 选择系统后端。
pub fn register_credential_store() -> Result<(), String> {
    #[cfg(target_os = "ios")]
    {
        let store = apple_native_keyring_store::protected::Store::new()
            .map_err(|error| format!("keychain store: {error}"))?;
        keyring_core::set_default_store(store);
    }
    #[cfg(target_os = "android")]
    {
        let store = android_native_keyring_store::Store::new()
            .map_err(|error| format!("Android credential store: {error}"))?;
        keyring_core::set_default_store(store);
    }
    Ok(())
}

use rshell_core::{
    AuthenticationKind, ConnectionProfile, HostKeyDecision, InteractionRequest,
    InteractionResponse, ResolvedTerminalProfile, TerminalOverrides, TerminalSettingsV1,
    TerminalSize, TransportKind, Viewport,
};
use rshell_session::{
    AuthPlan, DefaultTerminalEngine, KnownHostsVerifier, NativeSshTransport, SessionTransport,
    TerminalEngine, TransportEvent, TransportRequest, interaction_channel,
};
use secrecy::SecretString;

const TRANSCRIPT_LIMIT: usize = 256 * 1024;

/// M0 的读循环**必须自带截止时间**：真实远端 shell 永远不会 EOF，也没有 ExitStatus。
/// 一把「等 Eof 就退出」的循环会在真机上永久挂住——这是 M0 最容易踩的坑。
const READ_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

/// M0 顺带验证写路径：一条命令发过去，看它有没有被远端回显。
const PROBE_COMMAND: &[u8] = b"echo M0-ECHO\r";

// ─────────────────────────────────────────────────────────────────────────────
// M0：SSH 本身
// ─────────────────────────────────────────────────────────────────────────────

/// M0 的完整 SSH 路径。写死的地址与密码由调用方在 Rust 侧传入（真机上先不接 UI）。
///
/// 关键点：
/// * `AuthPlan::Password` 直接调 `russh` 的 `authenticate_password`，
///   **不经过 InteractionBroker**——所以 M0 不需要任何 UI 参与认证。
/// * 唯一需要交互的是主机密钥确认，这里用「一律接受并落盘」（TOFU）自动应答，
///   落盘路径指向 iOS 沙箱内可写文件。
/// * `TransportRequest` 通过 `configure_channel` 发出 `request_pty(terminal_type,
///   cols, rows, pixel_w, pixel_h)` + `request_shell`。**PTY 开在远端**，
///   本地不需要 fork。
pub async fn smoke(
    host: &str,
    port: u16,
    username: &str,
    password: &str,
    known_hosts_path: &str,
) -> Result<String, String> {
    let mut profile = ConnectionProfile::new("m0", host);
    profile.host = host.to_owned();
    profile.port = port;
    profile.username = username.to_owned();
    profile.transport = TransportKind::NativeSsh;
    profile.authentication = AuthenticationKind::Password;

    let auth = AuthPlan::from_secret(&profile, Some(SecretString::from(password.to_owned())))
        .map_err(|error| format!("AuthPlan: {error:?}"))?;

    let verifier = KnownHostsVerifier::new(known_hosts_path);
    let (broker, mut interactions) = interaction_channel();

    // M0 无 UI：主机密钥一律 TOFU 接受。M2 起这里换成 rinf 的 InteractionRequired 往返。
    let responder = {
        let broker = broker.clone();
        tokio::spawn(async move {
            while let Some((id, request)) = interactions.recv().await {
                let response = match request {
                    InteractionRequest::HostKey(_prompt) => {
                        InteractionResponse::HostKey(HostKeyDecision::AcceptAndStore)
                    }
                    // 其余分支 M0 不会走到：密码走 AuthPlan，键盘交互式未启用。
                    _ => InteractionResponse::Cancel,
                };
                let _ = broker.respond(id, response);
            }
        })
    };

    let request = TransportRequest::new(TerminalSize {
        cols: 80,
        rows: 24,
        pixel_width: 0,
        pixel_height: 0,
        dpi: 96,
    });

    let mut transport = NativeSshTransport::new(profile, auth, verifier)
        .map_err(|error| format!("NativeSshTransport: {error:?}"))?;

    transport
        .connect(&request, broker)
        .await
        .map_err(|error| format!("connect: {error:?}"))?;

    let mut transcript = String::new();

    // 写路径：这部分就是 M2 输入闭环的最小切片。
    if let Err(error) = transport.write(PROBE_COMMAND).await {
        transcript.push_str(&format!("[write failed: {error:?}]\n"));
    }

    let mut reads = 0usize;
    let read_loop = async {
        loop {
            match transport.next_event().await {
                Ok(TransportEvent::Output(bytes)) => {
                    reads += 1;
                    transcript.push_str(&String::from_utf8_lossy(&bytes));
                    if transcript.len() >= TRANSCRIPT_LIMIT {
                        return "transcript-limit".to_owned();
                    }
                }
                Ok(TransportEvent::Exit(status)) => {
                    return format!("exit code={:?} success={}", status.code, status.success);
                }
                Ok(TransportEvent::Eof) => return "eof".to_owned(),
                Ok(TransportEvent::Failure(failure)) => return format!("failure {failure:?}"),
                Ok(_) => {}
                Err(error) => return format!("error {error:?}"),
            }
        }
    };

    let outcome = match tokio::time::timeout(READ_DEADLINE, read_loop).await {
        Ok(outcome) => outcome,
        Err(_) => format!("read-deadline({READ_DEADLINE:?})"),
    };

    let _ = transport.shutdown().await;
    responder.abort();

    // M0 先验证「字节流回来了」；M1 之后同一批字节才会喂给 DefaultTerminalEngine。
    Ok(format!(
        "outcome={outcome} reads={reads} bytes={} \n--- transcript ---\n{transcript}",
        transcript.len()
    ))
}

/// 同步封装：iOS 侧（Swift / Flutter 的 Isolate）自己起线程调用，不要占主线程。
pub fn blocking_smoke(
    host: &str,
    port: u16,
    username: &str,
    password: &str,
    known_hosts_path: &str,
) -> Result<String, String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|error| format!("tokio: {error}"))?;
    runtime.block_on(smoke(host, port, username, password, known_hosts_path))
}

// ─────────────────────────────────────────────────────────────────────────────
// M1：终端状态机 + 渲染帧（rinf 边界的先决条件）
// ─────────────────────────────────────────────────────────────────────────────

/// 构造 `DefaultTerminalEngine`，喂一段带 SGR 颜色的字节，渲染一帧并序列化。
///
/// 这一步证明的是：**渲染帧可以从 Rust 直接序列化过边界**，Flutter 侧只负责画。
/// Rust 是唯一状态权威，Dart 不持有终端状态机。
pub fn engine_smoke() -> Result<String, String> {
    let profile: ResolvedTerminalProfile =
        TerminalSettingsV1::default().resolve(&TerminalOverrides::default());
    let size = TerminalSize {
        cols: 80,
        rows: 24,
        pixel_width: 0,
        pixel_height: 0,
        dpi: 96,
    };

    let mut engine =
        DefaultTerminalEngine::new(&profile, size).map_err(|error| format!("engine: {error:?}"))?;
    engine
        .advance(b"\x1b[31mred\x1b[0m plain\r\nsecond line \xe4\xb8\xad\xe6\x96\x87\r\n")
        .map_err(|error| format!("advance: {error:?}"))?;

    let frame = engine
        .render(
            Viewport {
                top_stable_row: i64::MAX,
                rows: 24,
            },
            None,
        )
        .map_err(|error| format!("render: {error:?}"))?;

    let encoded = serde_json::to_string(&*frame).map_err(|error| format!("serde: {error}"))?;
    Ok(format!(
        "rows={} encoded_bytes={} scrollback_top={}",
        frame.rows.len(),
        encoded.len(),
        frame.viewport_top
    ))
}

/// 同一个引擎连续渲染两帧，验证 `RenderFrame` 是纯数据、可跨线程搬运（rinf 在独立线程发信号）。
pub fn frame_is_send() -> Result<String, String> {
    let profile: ResolvedTerminalProfile =
        TerminalSettingsV1::default().resolve(&TerminalOverrides::default());
    let size = TerminalSize {
        cols: 80,
        rows: 24,
        pixel_width: 0,
        pixel_height: 0,
        dpi: 96,
    };
    let mut engine =
        DefaultTerminalEngine::new(&profile, size).map_err(|error| format!("engine: {error:?}"))?;
    engine
        .advance(b"hello\r\n")
        .map_err(|error| format!("advance: {error:?}"))?;
    let frame: Arc<rshell_core::RenderFrame> = engine
        .render(
            Viewport {
                top_stable_row: i64::MAX,
                rows: 24,
            },
            None,
        )
        .map_err(|error| format!("render: {error:?}"))?;

    let handle = std::thread::spawn(move || frame.rows.len());
    let rows = handle.join().map_err(|_| "join".to_owned())?;
    Ok(format!("frames-moved-across-thread rows={rows}"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn engine_frame_serializes() {
        let summary = super::engine_smoke().unwrap_or_else(|error| panic!("{error}"));
        assert!(summary.starts_with("rows=24 "), "{summary}");
    }

    #[test]
    fn frames_move_across_threads() {
        let summary = super::frame_is_send().unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(summary, "frames-moved-across-thread rows=24");
    }
}
