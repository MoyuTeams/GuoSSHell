//! M1 演示服务器：给模拟器 / "Designed for iPad" 上的 GuoSSHell 一个无需凭证的环回对端。
//!
//! ```text
//! cargo run --example demo_server -- [port]   # 默认 2222
//! ```
//!
//! 登录 `probe` / `probe`。连上后先发一段彩色 + CJK + emoji 横幅，
//! 然后每 700 ms 追加一行日志（模拟 `tail -f`，用来压帧率）。
//! 输入回显为青色；window-change 回发 RESIZE-ACK（M1 验收要看的远端响应）。
//! 设置 `GUOSH_DEMO_ENQ=1` 时，横幅后额外发一个 ENQ（0x05），用来验证终端 answerback。

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use russh::server::{Auth, ChannelOpenHandle, Handler, Msg, Server, Session};
use russh::{Channel, ChannelId};

struct DemoServer;

impl Server for DemoServer {
    type Handler = DemoHandler;

    fn new_client(&mut self, _peer: Option<std::net::SocketAddr>) -> DemoHandler {
        DemoHandler {
            channel: None,
            channel_id: None,
        }
    }
}

struct DemoHandler {
    channel: Option<Channel<Msg>>,
    channel_id: Option<ChannelId>,
}

impl Handler for DemoHandler {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        eprintln!("[server] auth_password user={user}");
        if user == "probe" && password == "probe" {
            eprintln!("[server] password accepted");
            Ok(Auth::Accept)
        } else {
            Ok(Auth::reject())
        }
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        eprintln!("[server] channel_open_session");
        reply.accept().await;
        self.channel_id = Some(channel.id());
        self.channel = Some(channel);
        Ok(())
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        term: &str,
        col_width: u32,
        row_height: u32,
        pix_width: u32,
        pix_height: u32,
        _modes: &[(russh::Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        eprintln!("[server] pty_request term={term} cols={col_width} rows={row_height}");
        let _ = session.channel_success(channel);
        session.data(
            channel,
            format!("PTY-ACK term={term} cols={col_width} rows={row_height} px={pix_width}x{pix_height}\r\n")
                .into_bytes(),
        )?;
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        eprintln!("[server] shell_request -> starting demo stream");
        self.start_session(channel, session)
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let command = String::from_utf8_lossy(data).to_string();
        eprintln!("[server] exec_request cmd={command}");
        let _ = session.channel_success(channel);
        let Some(channel) = self.channel.take() else {
            return Ok(());
        };
        // 测试服务器才这么干：把命令当真在本机执行，stdout/stderr 直通频道。
        tokio::spawn(run_real_command(channel, command));
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        // 输入回显成青色（M1 终端只读，但服务器照常回显）。
        if let Ok(path) = std::env::var("GUOSH_DEMO_INPUT_LOG") {
            use std::io::Write;
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                for byte in data {
                    let _ = write!(file, "{byte:02x}");
                }
                let _ = writeln!(file);
            }
        }
        let mut echo = b"\x1b[36m".to_vec();
        echo.extend_from_slice(data);
        echo.extend_from_slice(b"\x1b[0m");
        session.data(channel, echo)?;
        Ok(())
    }

    async fn window_change_request(
        &mut self,
        channel: ChannelId,
        col_width: u32,
        row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.data(
            channel,
            format!("RESIZE-ACK cols={col_width} rows={row_height}\r\n").into_bytes(),
        )?;
        Ok(())
    }
}

impl DemoHandler {
    /// shell 与 exec 共用：确认频道 + 起演示流。
    fn start_session(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), russh::Error> {
        let _ = session.channel_success(channel);
        if let Some(channel) = self.channel.take() {
            tokio::spawn(demo_stream(channel));
        }
        Ok(())
    }
}

/// 测试服务器专用：真实执行命令，stdout/stderr 直通频道。
/// 进度条脚本的 `\r` 重画会原样进引擎——正好当 60Hz 帧源。
async fn run_real_command(channel: Channel<Msg>, command: String) {
    let mut child = match tokio::process::Command::new("bash")
        .arg("-c")
        .arg(&command)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            let _ = channel
                .data_bytes(format!("exec failed: {error}\r\n").into_bytes())
                .await;
            return;
        }
    };

    // 两路都读到 EOF 才等退出；错误或超时才终止子进程，单路 EOF 不能截断另一端。
    let completed = tokio::time::timeout(std::time::Duration::from_secs(3600), async {
        let out = child.stdout.take().ok_or(())?;
        let err = child.stderr.take().ok_or(())?;
        drain_command_output(out, err, |bytes| async {
            channel.data_bytes(bytes).await.map_err(|_| ())
        })
        .await?;
        child.wait().await.map_err(|_| ())
    })
    .await;
    let code = match completed {
        Ok(Ok(status)) => status.code().unwrap_or(1) as u32,
        failure => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            if failure.is_err() { 124 } else { 1 }
        }
    };
    // 真 sshd 的收尾序列：exit-status → EOF → close（App 据此弹「已断开」）。
    let _ = channel.exit_status(code).await;
    let _ = channel.eof().await;
    let _ = channel.close().await;
}

async fn drain_command_output<R, S, F, Fut>(out: R, err: S, send: F) -> Result<(), ()>
where
    R: tokio::io::AsyncRead + Unpin,
    S: tokio::io::AsyncRead + Unpin,
    F: Fn(Vec<u8>) -> Fut,
    Fut: std::future::Future<Output = Result<(), ()>>,
{
    async fn pipe<R, F, Fut>(mut stream: R, send: &F) -> Result<(), ()>
    where
        R: tokio::io::AsyncRead + Unpin,
        F: Fn(Vec<u8>) -> Fut,
        Fut: std::future::Future<Output = Result<(), ()>>,
    {
        use tokio::io::AsyncReadExt;
        let mut buffer = [0u8; 8192];
        loop {
            let count = stream.read(&mut buffer).await.map_err(|_| ())?;
            if count == 0 {
                return Ok(());
            }
            send(buffer[..count].to_vec()).await?;
        }
    }
    tokio::try_join!(pipe(out, &send), pipe(err, &send))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn one_pipe_eof_does_not_truncate_the_other_tail() {
        use tokio::io::AsyncWriteExt;
        for close_stdout in [false, true] {
            let (out, mut out_writer) = tokio::io::duplex(64);
            let (err, mut err_writer) = tokio::io::duplex(64);
            let bytes = std::sync::Mutex::new(Vec::new());
            let writer = async {
                let remaining = if close_stdout {
                    out_writer.shutdown().await.unwrap();
                    &mut err_writer
                } else {
                    err_writer.shutdown().await.unwrap();
                    &mut out_writer
                };
                tokio::task::yield_now().await;
                remaining.write_all(b"TAIL\n").await.unwrap();
                remaining.shutdown().await.unwrap();
            };
            let drain = super::drain_command_output(out, err, |chunk| {
                bytes.lock().unwrap().extend(chunk);
                std::future::ready(Ok(()))
            });
            let (_, result) = tokio::join!(writer, drain);
            assert_eq!(result, Ok(()));
            assert_eq!(*bytes.lock().unwrap(), b"TAIL\n");
        }
    }
}

/// 横幅 + 持续日志流。颜色形态故意覆盖 M1 验收的每一项：
/// 16 色 / truecolor / 反显 / 粗体 / CJK 宽字符 / emoji / 长空白 run。
async fn demo_stream(channel: Channel<Msg>) {
    let banner: Vec<&str> = vec![
        "\x1b[1;44m GuoSSHell M1 \x1b[0m 环回演示服务器\r\n",
        "\x1b[31m■ red\x1b[0m  \x1b[32m■ green\x1b[0m  \x1b[33m■ yellow\x1b[0m  \x1b[34m■ blue\x1b[0m\r\n",
        "\x1b[38;2;255;128;0m■ truecolor orange\x1b[0m  \x1b[7m■ reverse\x1b[0m  \x1b[1m■ bold\x1b[0m\r\n",
        "📦 emoji 宽度测试 ✅ 中文 2 列，ABC 紧随其后不错位\r\n",
        "$ ",
    ];
    for chunk in banner {
        if channel
            .data_bytes(Bytes::copy_from_slice(chunk.as_bytes()))
            .await
            .is_err()
        {
            return;
        }
    }

    // ENQ 探针（GUOSH_DEMO_ENQ=1）：横幅之后发 0x05，终端引擎的 answerback 回复
    // 会当普通输入回到 `data`，落进 GUOSH_DEMO_INPUT_LOG。
    if std::env::var("GUOSH_DEMO_ENQ").as_deref() == Ok("1")
        && channel
            .data_bytes(Bytes::from_static(b"\x05"))
            .await
            .is_err()
    {
        return;
    }

    let start = std::time::Instant::now();
    let mut seq = 0u64;
    loop {
        tokio::time::sleep(Duration::from_millis(700)).await;
        let elapsed = start.elapsed().as_secs_f32();
        let line = format!(
            "\x1b[32mOK\x1b[0m [{elapsed:8.3}] 日志行 {seq}：帧压缩与脏行增量实测中，这是一段较长的中文文本\r\n"
        );
        seq += 1;
        if channel
            .data_bytes(Bytes::copy_from_slice(line.as_bytes()))
            .await
            .is_err()
        {
            return;
        }
    }
}

#[tokio::main]
async fn main() {
    let port: u16 = std::env::args()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(2222);

    let config = Arc::new(russh::server::Config {
        keys: vec![demo_host_key()],
        inactivity_timeout: None,
        auth_rejection_time: Duration::from_millis(10),
        ..Default::default()
    });

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .expect("bind 127.0.0.1");
    println!("demo server on 127.0.0.1:{port}  (probe / probe)");

    loop {
        let Ok((socket, peer)) = listener.accept().await else {
            break;
        };
        let config = Arc::clone(&config);
        tokio::spawn(async move {
            let handler = DemoServer.new_client(Some(peer));
            if let Err(error) = russh::server::run_stream(config, socket, handler).await {
                eprintln!("[server] session ended: {error}");
            }
        });
    }
}

/// 一次性生成主机密钥（复用 m0_loopback 的做法：走 ssh-keygen）。
/// 密钥文件跨重启**保留**：App 沙箱里的 known_hosts 是 TOFU 落盘的，
/// 服务器每次换密钥会触发 HostKeyChanged（那是 M3 才处理的场景）。
fn demo_host_key() -> russh::keys::PrivateKey {
    // /tmp 会被 macOS 定期清理，密钥一换 App 里的 TOFU entry 就失配
    // （HostKeyChanged）。放到用户主目录下才能长期稳定。
    let path = dirs_demo_host_key();
    if path.exists() {
        return russh::keys::load_secret_key(&path, None).expect("load existing host key");
    }

    let status = std::process::Command::new("ssh-keygen")
        .args(["-t", "ed25519", "-N", "", "-q", "-f"])
        .arg(&path)
        .status()
        .expect("ssh-keygen must be available");
    assert!(status.success(), "ssh-keygen failed");

    russh::keys::load_secret_key(&path, None).expect("load generated host key")
}

/// 密钥的持久位置：`~/.guosh-demo/hostkey`（/tmp 会被系统清理）。
fn dirs_demo_host_key() -> std::path::PathBuf {
    if let Ok(path) = std::env::var("GUOSH_DEMO_DATA") {
        let dir = std::path::PathBuf::from(path);
        std::fs::create_dir_all(&dir).expect("测试数据目录");
        return dir.join("hostkey");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_owned());
    let dir = std::path::Path::new(&home).join(".guosh-demo");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("hostkey")
}
