//! 真实系统 ssh 经本地 PTY 连到进程内 loopback SSH 对端：首见主机密钥与密码都由 ssh 在
//! 终端里提问，应用只转发终端字节；退出码原样成为会话的退出状态。
//!
//! 隔离只在测试夹具层面：`-F` 指向本轮临时配置，known_hosts、身份文件与 agent 都不碰
//! 用户自己的 ~/.ssh（产品路径不隔离，照常使用用户的 ssh 配置）。
#![allow(clippy::expect_used)]
#![cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]

use super::{context, profile};
use crate::connect::{self, Channels, PromptIds, RemoteTerminal, Target};
use rshell_m0::rshell_core::{AuthenticationKind, TerminalSize, TransportKind};
use rshell_m0::rshell_session::{SessionTransport, TransportEvent};
use rshell_m0::rshell_storage::MemoryCredentialVault;
use russh::keys::PrivateKey;
use russh::keys::ssh_key::private::Ed25519Keypair;
use russh::server::{self, Auth, Handler, Session};
use russh::{Channel, ChannelId, Pty};
use secrecy::SecretString;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::mpsc::unbounded_channel;

const USER: &str = "probe";
const PASSWORD: &str = "system-ssh-fixture";
const BANNER: &[u8] = b"SYSTEM-SSH-BANNER";
/// 对端收到这个输入时以 [`EXIT_CODE`] 结束 shell，ssh 以同一退出码退出。
const EXIT_TRIGGER: &[u8] = b"!exit";
const EXIT_CODE: u32 = 7;
// 仅测试墙钟 watchdog；不改变生产连接或关闭时限。
const WATCHDOG: Duration = Duration::from_secs(30);

/// 本轮独占临时目录；路径由 UUID 新建，只清理它自己。
struct Root(PathBuf);
impl Drop for Root {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).expect("清理本轮独占目录");
    }
}

#[derive(Default)]
struct Observed {
    authentication: Vec<(String, bool)>,
    pty_terms: Vec<String>,
    shells: usize,
    received: Vec<u8>,
}

struct PeerHandler(Arc<Mutex<Observed>>);
impl Handler for PeerHandler {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        let accepted = user == USER && password == PASSWORD;
        self.0
            .lock()
            .expect("对端记录")
            .authentication
            .push((user.into(), accepted));
        Ok(if accepted {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }

    async fn channel_open_session(
        &mut self,
        _: Channel<server::Msg>,
        reply: server::ChannelOpenHandle,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        term: &str,
        _: u32,
        _: u32,
        _: u32,
        _: u32,
        _: &[(Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.0.lock().expect("对端记录").pty_terms.push(term.into());
        session.channel_success(channel)?;
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.0.lock().expect("对端记录").shells += 1;
        session.channel_success(channel)?;
        let mut banner = BANNER.to_vec();
        banner.extend_from_slice(b"\r\n$ ");
        session.data(channel, banner)?;
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let received = {
            let mut observed = self.0.lock().expect("对端记录");
            observed.received.extend_from_slice(data);
            observed.received.clone()
        };
        let mut echo = b"[echo:".to_vec();
        echo.extend_from_slice(data);
        echo.push(b']');
        session.data(channel, echo)?;
        if contains(&received, EXIT_TRIGGER) {
            session.exit_status_request(channel, EXIT_CODE)?;
            session.eof(channel)?;
            session.close(channel)?;
        }
        Ok(())
    }
}

struct Peer {
    port: u16,
    observed: Arc<Mutex<Observed>>,
    task: tokio::task::JoinHandle<()>,
}

impl Peer {
    async fn new() -> Self {
        let socket = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("loopback 端口");
        let port = socket.local_addr().expect("绑定地址").port();
        let config = Arc::new(server::Config {
            keys: vec![PrivateKey::from(Ed25519Keypair::from_seed(&[91; 32]))],
            auth_rejection_time: Duration::from_millis(10),
            ..server::Config::default()
        });
        let observed = Arc::new(Mutex::new(Observed::default()));
        let records = observed.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = socket.accept().await else {
                    return;
                };
                let handler = PeerHandler(records.clone());
                let config = config.clone();
                tokio::spawn(async move {
                    if let Ok(running) = server::run_stream(config, stream, handler).await {
                        let _ = running.await;
                    }
                });
            }
        });
        Self {
            port,
            observed,
            task,
        }
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// 夹具配置：OpenSSH 配置里统一用正斜杠并加引号。
fn quoted(path: &std::path::Path) -> String {
    format!("\"{}\"", path.display().to_string().replace('\\', "/"))
}

/// 本轮 `-F` 配置：主机密钥记录、身份文件都在本轮目录，不连 agent、只用密码认证。
fn write_config(root: &std::path::Path) -> (PathBuf, PathBuf) {
    let known_hosts = root.join("known_hosts");
    let global = root.join("global_known_hosts");
    std::fs::write(&global, b"").expect("空的全局 known_hosts");
    let config = root.join("ssh_config");
    let text = format!(
        "Host *\n\
         \x20 UserKnownHostsFile {}\n\
         \x20 GlobalKnownHostsFile {}\n\
         \x20 IdentityFile {}\n\
         \x20 IdentitiesOnly yes\n\
         \x20 IdentityAgent none\n\
         \x20 PubkeyAuthentication no\n\
         \x20 PreferredAuthentications password\n\
         \x20 UpdateHostKeys no\n\
         \x20 NumberOfPasswordPrompts 1\n",
        quoted(&known_hosts),
        quoted(&global),
        quoted(&root.join("no_identity")),
    );
    std::fs::write(&config, text).expect("本轮 ssh 配置");
    (config, known_hosts)
}

async fn establish(target: Target) -> Box<dyn SessionTransport> {
    let context = context(Arc::new(MemoryCredentialVault::new()));
    let (_commands_tx, mut commands) = unbounded_channel();
    let (_replies_tx, mut replies) = unbounded_channel();
    let mut backlog = VecDeque::new();
    let mut prompts = PromptIds::default();
    let connected = tokio::time::timeout(
        WATCHDOG,
        connect::establish(
            &context,
            91,
            target,
            SecretString::from(String::new()),
            RemoteTerminal {
                // 宽终端，ssh 的提示不被折行打断。
                size: TerminalSize {
                    cols: 200,
                    rows: 50,
                    pixel_width: 0,
                    pixel_height: 0,
                    dpi: 96,
                },
                terminal_type: Some("xterm-256color"),
            },
            Channels {
                commands: &mut commands,
                replies: &mut replies,
                backlog: &mut backlog,
            },
            &mut prompts,
        ),
    )
    .await
    .expect("启动 watchdog");
    match connected {
        Ok(connected) => connected.transport,
        Err(_) => panic!("系统 ssh 启动失败"),
    }
}

/// 读到标记出现；进程提前退出则带着已见输出失败。
async fn read_until(transport: &mut dyn SessionTransport, seen: &mut Vec<u8>, marker: &[u8]) {
    let found = tokio::time::timeout(WATCHDOG, async {
        while !contains(seen, marker) {
            match transport.next_event().await.expect("PTY 事件") {
                TransportEvent::Output(bytes) => seen.extend_from_slice(&bytes),
                TransportEvent::Exit(status) => {
                    panic!("标记出现前 ssh 已退出：{status:?}")
                }
                _ => {}
            }
        }
    })
    .await;
    assert!(
        found.is_ok(),
        "未见 {}：{}",
        String::from_utf8_lossy(marker),
        String::from_utf8_lossy(seen)
    );
}

/// 读到 ssh 退出，返回退出码与期间输出。
async fn read_exit(transport: &mut dyn SessionTransport) -> (Option<i32>, Vec<u8>) {
    tokio::time::timeout(WATCHDOG, async {
        let mut output = Vec::new();
        loop {
            match transport.next_event().await.expect("PTY 事件") {
                TransportEvent::Output(bytes) => output.extend_from_slice(&bytes),
                TransportEvent::Exit(status) => return (status.code, output),
                _ => {}
            }
        }
    })
    .await
    .expect("ssh 退出 watchdog")
}

fn target(port: u16, config: PathBuf) -> Target {
    let mut profile = profile();
    profile.host = "127.0.0.1".into();
    profile.port = port;
    profile.username = USER.into();
    profile.transport = TransportKind::SystemOpenSsh;
    profile.authentication = AuthenticationKind::Agent;
    profile.remote_command = None;
    Target::system(profile, false, Some(config))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn system_ssh_asks_in_the_terminal_echoes_and_reports_exit_codes() {
    let root =
        Root(std::env::temp_dir().join(format!("guosh-system-ssh-{}", uuid::Uuid::new_v4())));
    std::fs::create_dir(&root.0).expect("本轮独占目录");
    let (config, known_hosts) = write_config(&root.0);
    let peer = Peer::new().await;

    // 首次：ssh 在终端里问主机密钥与密码，应用不弹任何提示。
    let mut transport = establish(target(peer.port, config.clone())).await;
    let mut seen = Vec::new();
    read_until(transport.as_mut(), &mut seen, b"(yes/no").await;
    assert!(!known_hosts.exists(), "确认前不写 known_hosts");
    transport.write(b"yes\r").await.expect("确认主机密钥");
    read_until(transport.as_mut(), &mut seen, b"password:").await;
    transport
        .write(format!("{PASSWORD}\r").as_bytes())
        .await
        .expect("输入密码");
    read_until(transport.as_mut(), &mut seen, BANNER).await;
    transport.write(b"hello-42").await.expect("输入");
    read_until(transport.as_mut(), &mut seen, b"[echo:").await;
    read_until(transport.as_mut(), &mut seen, b"42]").await;
    transport.write(EXIT_TRIGGER).await.expect("请求对端退出");
    let (code, _) = read_exit(transport.as_mut()).await;
    assert_eq!(
        code,
        Some(EXIT_CODE as i32),
        "ssh 以对端 shell 的退出码退出"
    );
    tokio::time::timeout(WATCHDOG, transport.shutdown())
        .await
        .expect("关闭 watchdog")
        .expect("退出后关闭");
    let recorded = std::fs::read_to_string(&known_hosts).expect("本轮 known_hosts 已写入");
    assert!(recorded.contains("ssh-ed25519"), "{recorded}");
    {
        let observed = peer.observed.lock().expect("对端记录");
        assert_eq!(observed.authentication, [(USER.to_owned(), true)]);
        assert_eq!(observed.pty_terms, ["xterm-256color"]);
        assert_eq!(observed.shells, 1);
        assert!(contains(&observed.received, b"hello-42"));
    }

    // 再连（等同重连：重新运行 ssh）：主机已知不再询问；密码错误时 ssh 以 255 退出。
    let mut transport = establish(target(peer.port, config)).await;
    let mut seen = Vec::new();
    read_until(transport.as_mut(), &mut seen, b"password:").await;
    assert!(!contains(&seen, b"(yes/no"), "已记录的主机不再询问");
    transport.write(b"wrong\r").await.expect("输入错误密码");
    let (code, _) = read_exit(transport.as_mut()).await;
    assert_eq!(code, Some(255), "认证失败");
    tokio::time::timeout(WATCHDOG, transport.shutdown())
        .await
        .expect("关闭 watchdog")
        .expect("退出后关闭");
    assert_eq!(
        peer.observed
            .lock()
            .expect("对端记录")
            .authentication
            .last(),
        Some(&(USER.to_owned(), false))
    );
    drop(peer);
}
