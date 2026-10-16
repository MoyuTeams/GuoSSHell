//! 真实本地 PTY 经连接入口与会话 actor 收发、改尺寸、退出与重连；不替代 GUI 验收。
#![allow(clippy::expect_used)]
#![cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]

use super::super::{SessionCommand, run_session_inner};
use super::{context, size};
use crate::connect::{self, Channels, PromptIds, RemoteTerminal, Target};
use crate::signals::ConnectRequest;
use rshell_m0::rshell_core::{TerminalOverrides, TerminalSettingsV1, TerminalSize};
use rshell_m0::rshell_session::{LocalLaunch, SessionTransport, TransportEvent};
use rshell_m0::rshell_storage::MemoryCredentialVault;
use secrecy::SecretString;
use std::collections::{BTreeMap, VecDeque};
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tokio::sync::watch;

// 仅测试墙钟 watchdog；不改变生产连接或关闭时限。
const WATCHDOG: Duration = Duration::from_secs(20);

fn launch(program: &str, args: &[&str]) -> LocalLaunch {
    LocalLaunch::Command {
        program: PathBuf::from(program),
        args: args.iter().map(OsString::from).collect(),
        cwd: None,
        env: BTreeMap::new(),
    }
}

/// 交互 shell：Windows 用 cmd（不跑 AutoRun、关闭回显），其余用 /bin/sh。
fn shell() -> LocalLaunch {
    #[cfg(windows)]
    return launch("cmd.exe", &["/d", "/q"]);
    #[cfg(not(windows))]
    return launch("/bin/sh", &[]);
}

/// 不读 stdin 的长时间子进程。
fn idle() -> LocalLaunch {
    #[cfg(windows)]
    return launch("ping", &["-n", "60", "127.0.0.1"]);
    #[cfg(not(windows))]
    return launch("sleep", &["60"]);
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
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
            81,
            target,
            SecretString::from(String::new()),
            RemoteTerminal {
                size: size(),
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
        Err(_) => panic!("本地 shell 启动失败"),
    }
}

async fn read_until(transport: &mut dyn SessionTransport, seen: &mut Vec<u8>, marker: &[u8]) {
    let found = tokio::time::timeout(WATCHDOG, async {
        while !contains(seen, marker) {
            match transport.next_event().await.expect("本地 PTY 事件") {
                TransportEvent::Output(bytes) => seen.extend_from_slice(&bytes),
                TransportEvent::Exit(status) => panic!("标记出现前已退出：{status:?}"),
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_shell_echoes_resizes_and_reports_exit_code() {
    let mut transport = establish(Target::local(Some(shell()))).await;
    let mut seen = Vec::new();
    // 标记由 shell 展开得到，回显的命令行本身不含它。
    #[cfg(windows)]
    let (command, marker) = (b"echo GUO_%OS%\r".as_slice(), b"GUO_Windows_NT".as_slice());
    #[cfg(not(windows))]
    let (command, marker) = (b"echo GUO_$((40+2))\r".as_slice(), b"GUO_42".as_slice());
    transport.write(command).await.expect("写入命令");
    read_until(transport.as_mut(), &mut seen, marker).await;

    transport
        .resize(TerminalSize {
            cols: 117,
            rows: 33,
            pixel_width: 1170,
            pixel_height: 660,
            dpi: 96,
        })
        .await
        .expect("改尺寸");
    seen.clear();
    #[cfg(windows)]
    transport.write(b"mode con\r").await.expect("查询尺寸");
    #[cfg(not(windows))]
    transport.write(b"stty size\r").await.expect("查询尺寸");
    read_until(transport.as_mut(), &mut seen, b"117").await;

    transport.write(b"exit 3\r").await.expect("退出命令");
    let status = tokio::time::timeout(WATCHDOG, async {
        loop {
            if let TransportEvent::Exit(status) =
                transport.next_event().await.expect("本地 PTY 事件")
            {
                return status;
            }
        }
    })
    .await
    .expect("退出 watchdog");
    assert_eq!(status.code, Some(3));
    assert!(!status.success);
    tokio::time::timeout(WATCHDOG, transport.shutdown())
        .await
        .expect("关闭 watchdog")
        .expect("退出后关闭");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_local_shell_runs_input_and_exits_cleanly() {
    let mut transport = establish(Target::local(None)).await;
    transport.write(b"exit 0\r").await.expect("写入退出命令");
    let (output, status) = tokio::time::timeout(WATCHDOG, async {
        let mut output = Vec::new();
        loop {
            match transport.next_event().await.expect("默认 shell 事件") {
                TransportEvent::Output(bytes) => output.extend_from_slice(&bytes),
                TransportEvent::Exit(status) => return (output, status),
                _ => {}
            }
        }
    })
    .await
    .expect("默认 shell 退出 watchdog");
    assert!(!output.is_empty(), "默认 shell 有输出");
    assert!(status.success, "默认 shell 退出状态 {status:?}");
    tokio::time::timeout(WATCHDOG, transport.shutdown())
        .await
        .expect("关闭 watchdog")
        .expect("关闭默认 shell");
}

fn request(session_id: u32) -> ConnectRequest {
    ConnectRequest {
        session_id,
        connection_id: String::new(),
        host: String::new(),
        port: 0,
        username: String::new(),
        password: String::new(),
        command: String::new(),
        cols: 80,
        rows: 24,
        pixel_width: 0,
        pixel_height: 0,
        dpi: 96,
    }
}

struct Actor {
    commands: tokio::sync::mpsc::UnboundedSender<SessionCommand>,
    statuses: UnboundedReceiver<(bool, serde_json::Value)>,
    task: tokio::task::JoinHandle<u32>,
    _replies: tokio::sync::mpsc::UnboundedSender<crate::protected_reply::ProtectedReply>,
    _close: watch::Sender<bool>,
}

impl Actor {
    /// 不经 V2 来源运行：本地目标仍须在重连时沿用同一启动描述。
    fn spawn(session_id: u32, launch: LocalLaunch) -> Self {
        let context = context(Arc::new(MemoryCredentialVault::new()));
        let (commands, receiver) = unbounded_channel();
        let (replies_tx, replies) = unbounded_channel();
        let (events, statuses) = unbounded_channel();
        let (close, closing) = watch::channel(false);
        let settings = TerminalSettingsV1::default().resolve(&TerminalOverrides::default());
        let task = tokio::spawn(crate::status_sink::OBSERVER.scope(
            events,
            crate::transport_lifecycle::owner(
                closing,
                run_session_inner(
                    context,
                    request(session_id),
                    Some(Target::local(Some(launch))),
                    Some(settings),
                    Some(SecretString::from(String::new())),
                    receiver,
                    replies,
                ),
            ),
        ));
        Self {
            commands,
            statuses,
            task,
            _replies: replies_tx,
            _close: close,
        }
    }

    async fn status(&mut self, state: &str) -> serde_json::Value {
        tokio::time::timeout(WATCHDOG, async {
            loop {
                let (_, status) = self.statuses.recv().await.expect("会话状态");
                assert_ne!(status["state"], "Failed", "会话失败：{status}");
                if status["state"] == state {
                    return status;
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("未等到 {state}"))
    }

    fn send(&self, command: SessionCommand) {
        assert!(self.commands.send(command).is_ok(), "会话仍在运行");
    }

    async fn finish(self) {
        let Self { commands, task, .. } = self;
        assert!(commands.send(SessionCommand::Disconnect).is_ok());
        assert_eq!(
            tokio::time::timeout(WATCHDOG, task)
                .await
                .expect("关闭会话 watchdog")
                .expect("会话任务"),
            0,
            "本地 PTY 关闭不失败"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_session_maps_exit_code_to_close_and_reconnect_relaunches() {
    let mut actor = Actor::spawn(82, shell());
    let connected = actor.status("Connected").await;
    assert_eq!(connected["detail"], "本地 shell");
    actor.send(SessionCommand::Paste("exit 3\r".into()));
    let closed = actor.status("Closed").await;
    let detail = closed["detail"].as_str().expect("关闭原因");
    assert!(detail.contains("code=Some(3)"), "{detail}");

    // 重连即重新启动同一 shell，仍可再次退出。
    actor.send(SessionCommand::Reconnect);
    actor.status("Connected").await;
    actor.send(SessionCommand::Paste("exit 5\r".into()));
    let closed = actor.status("Closed").await;
    let detail = closed["detail"].as_str().expect("关闭原因");
    assert!(detail.contains("code=Some(5)"), "{detail}");
    actor.finish().await;
}

/// 子进程不读输入时，1 MiB 粘贴不得卡住会话；随后的关闭仍在时限内完成。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn large_paste_to_a_non_reading_child_keeps_the_session_responsive() {
    let mut actor = Actor::spawn(83, idle());
    actor.status("Connected").await;
    // 按行组织：规范模式的 tty 只会因完整行积压而阻塞写入。
    let mut line = "x".repeat(1023);
    line.push('\n');
    actor.send(SessionCommand::Paste(line.repeat(1024)));
    let started = std::time::Instant::now();
    actor.finish().await;
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "关闭耗时 {:?}",
        started.elapsed()
    );
}
