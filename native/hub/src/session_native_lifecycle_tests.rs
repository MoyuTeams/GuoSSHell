//! 原会话 actor 的认证前投影、认证失败与连接中关闭；不代表 GUI/原生输入验收。
#![allow(clippy::expect_used)]

use super::*;
use crate::desktop_input_port::{self, InputPort, Registration};
use crate::signals::desktop_native_input::{
    DesktopNativeCommand, DesktopNativeLifecycle, DesktopNativeLifecycleQuery, NativeCommand,
    NativeCommandFailure, NativeLifecycleQueryFailure, NativeLifecycleState, VERSION,
};
use rshell_m0::rshell_core::{AuthenticationKind, ConnectionProfile, TransportKind};
use rshell_m0::rshell_storage::{CredentialCoordinator, MemoryCredentialVault, SqliteRepository};
use russh::keys::ssh_key::private::Ed25519Keypair;
use russh::keys::{PrivateKey, known_hosts::learn_known_hosts_path};
use russh::server::{self, Auth, Handler, Session};
use std::path::PathBuf;
use std::sync::Mutex;
use tokio::sync::{mpsc::UnboundedReceiver, watch};

struct Fixture {
    root: PathBuf,
    context: Arc<AppContext>,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("guosh-lifecycle-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).expect("本轮独占测试目录");
        let repository = Arc::new(SqliteRepository::open_in_memory().expect("内存目录"));
        repository.migrate().expect("内存迁移");
        let context = Arc::new(AppContext {
            credentials: CredentialCoordinator::new(
                repository.clone(),
                Arc::new(MemoryCredentialVault::new()),
            ),
            repository,
            known_hosts: root.join("known_hosts"),
            keys: Arc::new(crate::keys::MemoryKeyStore::new(false)),
            key_operations: Mutex::new(()),
            cards: Arc::new(crate::card::CardContext::new(Arc::new(
                crate::card::NoCards,
            ))),
            security_keys: Arc::new(crate::security_key::Unavailable),
            preferences: crate::keys::PreferenceFile::open(root.join("preferences.json")),
            catalog_changed: tokio::sync::Notify::new(),
            keys_changed: tokio::sync::Notify::new(),
        });
        Self { root, context }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // 只删除本轮以 UUID 创建并持有的精确目录，不接触旧 storage。
        std::fs::remove_dir_all(&self.root).expect("清理本轮目录");
    }
}

fn registration(session_id: u32) -> Registration {
    crate::desktop_input_port::tests::registration(session_id)
}
fn query(port: &InputPort) -> DesktopNativeLifecycle {
    let result = desktop_input_port::query_lifecycle(DesktopNativeLifecycleQuery {
        version: VERSION,
        source: port.source.source.clone(),
        identity: port.source.identity.clone(),
        request_id: 1,
    });
    assert_eq!(result.failure, NativeLifecycleQueryFailure::None);
    result.lifecycle.expect("原来源只读投影")
}
async fn state(
    events: &mut UnboundedReceiver<DesktopNativeLifecycle>,
    expected: NativeLifecycleState,
) -> DesktopNativeLifecycle {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let value = events.recv().await.expect("生命周期出口");
            if value.state == expected {
                return value;
            }
        }
    })
    .await
    .expect("生命周期 watchdog")
}
fn request(session_id: u32) -> ConnectRequest {
    ConnectRequest {
        session_id,
        connection_id: String::new(),
        host: "127.0.0.1".into(),
        port: 22,
        username: "lifecycle-test".into(),
        password: String::new(),
        command: String::new(),
        cols: 80,
        rows: 24,
        pixel_width: 640,
        pixel_height: 384,
        dpi: 96,
    }
}
fn run_actor(
    fixture: &Fixture,
    root: Registration,
    profile: ConnectionProfile,
    password: &str,
    commands: UnboundedReceiver<SessionCommand>,
    replies: UnboundedReceiver<ProtectedReply>,
    closing: watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    let context = fixture.context.clone();
    let source = root.0.source.clone();
    let settings = root.0.profile.as_ref().clone();
    let password = SecretString::from(password.to_owned());
    tokio::spawn(async move {
        desktop_input_port::CURRENT
            .scope(
                root.0.clone(),
                desktop_runtime::SOURCE.scope(
                    source.clone(),
                    crate::transport_lifecycle::owner(
                        closing,
                        run_session_inner(
                            context,
                            request(source.source.session_id),
                            Some(connect::Target::ssh(profile, false)),
                            Some(settings),
                            Some(password),
                            commands,
                            replies,
                        ),
                    ),
                ),
            )
            .await;
        drop(root);
    })
}

fn close_original(port: &InputPort, close: &watch::Sender<bool>, request_id: u64) {
    let original = query(port);
    assert_ne!(original.attempt, 0);
    let result = native_input::route_command(DesktopNativeCommand {
        version: VERSION,
        source: original.source,
        identity: original.identity,
        attempt: original.attempt,
        focus_epoch: original.focus_epoch,
        request_id,
        command: NativeCommand::Event(DesktopEvent::Disconnect),
    });
    assert_eq!(result.failure, NativeCommandFailure::None);
    // 正常 supervisor 在该准入成功后发送同一 close 控制。
    close.send_replace(true);
}

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn legacy_system_ssh_reconnect_keeps_transport_for_quick_and_saved_targets() {
    for saved in [false, true] {
        let fixture = Fixture::new();
        // 解析本轮配置即退出，不读取用户 SSH 配置、不连接网络或 agent。
        let config = fixture.root.join("ssh_config");
        std::fs::write(&config, "GuoshRegressionInvalidOption yes\n").expect("隔离 ssh 配置");
        let session_id = if saved { 8511 } else { 8510 };
        let root = registration(session_id);
        let mut events = root.0.observe_lifecycle();
        let settings = root.0.profile.as_ref().clone();
        let mut profile = ConnectionProfile::new("System 重连", "127.0.0.1");
        profile.username = "fixture".into();
        profile.transport = TransportKind::SystemOpenSsh;
        profile.authentication = AuthenticationKind::Agent;
        let mut request = request(session_id);
        if saved {
            request.connection_id = profile.id.0.to_string();
            fixture
                .context
                .credentials
                .apply_catalog(
                    rshell_m0::rshell_core::CatalogMutation::Create(profile.clone()),
                    rshell_m0::rshell_core::SecretUpdate::Unchanged,
                )
                .expect("System 目录目标");
        }
        let target = connect::Target::system(profile, saved, Some(config));
        let (commands_tx, commands) = unbounded_channel();
        let (_replies_tx, replies) = unbounded_channel();
        let (close, closing) = watch::channel(false);
        let context = fixture.context.clone();
        let task = tokio::spawn(async move {
            // CURRENT 仅观察生命周期；不设置 V2 SOURCE，实际执行 V1 重连分支。
            desktop_input_port::CURRENT
                .scope(
                    root.0.clone(),
                    crate::transport_lifecycle::owner(
                        closing,
                        run_session_inner(
                            context,
                            request,
                            Some(target),
                            Some(settings),
                            None,
                            commands,
                            replies,
                        ),
                    ),
                )
                .await;
            drop(root);
        });
        for attempt in 0..2 {
            state(&mut events, NativeLifecycleState::Authenticated).await;
            state(&mut events, NativeLifecycleState::Disconnected).await;
            if attempt == 0 {
                assert!(commands_tx.send(SessionCommand::Reconnect).is_ok());
            }
        }
        close.send_replace(true);
        tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .expect("关闭 watchdog")
            .expect("System actor");
    }
}

#[tokio::test]
async fn lifecycle_attempt_is_queryable_before_password_wait_and_connecting_close() {
    let fixture = Fixture::new();
    let root = registration(8501);
    let port = root.0.clone();
    let mut events = port.observe_lifecycle();
    let (commands_tx, commands) = unbounded_channel();
    let (_replies_tx, replies) = unbounded_channel();
    let (close, closing) = watch::channel(false);
    let mut profile = ConnectionProfile::new("认证前生命周期", "127.0.0.1");
    profile.transport = TransportKind::NativeSsh;
    profile.username = "lifecycle-test".into();
    profile.authentication = AuthenticationKind::Password;
    let task = run_actor(&fixture, root, profile, "", commands, replies, closing);
    let connecting = state(&mut events, NativeLifecycleState::Connecting).await;
    assert_eq!(query(&port), connecting);
    assert!(port.projection().is_none());
    assert!(!task.is_finished());
    close_original(&port, &close, 1);
    tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .expect("连接中关闭")
        .expect("actor");
    assert!(commands_tx.is_closed());
    assert_eq!(query(&port).state, NativeLifecycleState::Ended);
}

#[tokio::test]
async fn lifecycle_accepted_then_immediate_closing_still_creates_original_attempt() {
    let fixture = Fixture::new();
    let root = registration(8502);
    let port = root.0.clone();
    let mut events = port.observe_lifecycle();
    assert_eq!(query(&port).state, NativeLifecycleState::Registered);
    let (_commands_tx, commands) = unbounded_channel();
    let (_replies_tx, replies) = unbounded_channel();
    let (_close, closing) = watch::channel(true);
    let mut profile = ConnectionProfile::new("立即关闭", "127.0.0.1");
    profile.transport = TransportKind::NativeSsh;
    profile.username = "lifecycle-test".into();
    profile.authentication = AuthenticationKind::Password;
    let task = run_actor(&fixture, root, profile, "", commands, replies, closing);
    tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .expect("立即关闭")
        .expect("actor");
    let connecting = state(&mut events, NativeLifecycleState::Connecting).await;
    let ended = query(&port);
    assert_eq!(ended.state, NativeLifecycleState::Ended);
    assert_eq!(ended.attempt, connecting.attempt);
    assert!(ended.lifecycle_revision > connecting.lifecycle_revision);
    assert!(port.projection().is_none());
}

struct RejectPassword;
impl Handler for RejectPassword {
    type Error = russh::Error;
    async fn auth_password(&mut self, _: &str, _: &str) -> Result<Auth, Self::Error> {
        Ok(Auth::reject())
    }
    async fn auth_succeeded(&mut self, _: &mut Session) -> Result<(), Self::Error> {
        panic!("拒绝凭证不能认证成功")
    }
}

#[tokio::test]
async fn lifecycle_real_authentication_failure_then_reconnect_uses_new_revision() {
    let fixture = Fixture::new();
    let socket = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("真实环回");
    let address = socket.local_addr().expect("环回地址");
    let key = PrivateKey::from(Ed25519Keypair::from_seed(&[84; 32]));
    learn_known_hosts_path(
        "127.0.0.1",
        address.port(),
        key.public_key(),
        &fixture.context.known_hosts,
    )
    .expect("本轮隔离信任记录");
    let config = Arc::new(server::Config {
        keys: vec![key],
        ..Default::default()
    });
    let server = tokio::spawn(async move {
        let (stream, _) = socket.accept().await.expect("真实 SSH 连接");
        let running = server::run_stream(config, stream, RejectPassword)
            .await
            .expect("真实 SSH 服务端");
        // 认证失败后的连接由客户端关闭；服务端结束结果与此次客户端生命周期无关。
        let _ = running.await;
    });
    let root = registration(8503);
    let port = root.0.clone();
    let mut events = port.observe_lifecycle();
    let (commands_tx, commands) = unbounded_channel();
    let (_replies_tx, replies) = unbounded_channel();
    let (close, closing) = watch::channel(false);
    let mut profile = ConnectionProfile::new("认证拒绝后重连", "127.0.0.1");
    profile.transport = TransportKind::NativeSsh;
    profile.port = address.port();
    profile.username = "lifecycle-test".into();
    profile.authentication = AuthenticationKind::Password;
    let task = run_actor(
        &fixture,
        root,
        profile,
        "错误的虚构口令",
        commands,
        replies,
        closing,
    );
    let first = state(&mut events, NativeLifecycleState::Connecting).await;
    let failed = state(&mut events, NativeLifecycleState::AuthenticationFailed).await;
    assert_eq!(failed.attempt, first.attempt);
    assert!(failed.lifecycle_revision > first.lifecycle_revision);
    assert_eq!(query(&port), failed);
    assert!(port.projection().is_none());
    let result = native_input::route_command(DesktopNativeCommand {
        version: VERSION,
        source: failed.source.clone(),
        identity: failed.identity.clone(),
        attempt: failed.attempt,
        focus_epoch: failed.focus_epoch,
        request_id: 1,
        command: NativeCommand::Event(DesktopEvent::Reconnect),
    });
    assert_eq!(result.failure, NativeCommandFailure::None);
    let next = state(&mut events, NativeLifecycleState::Connecting).await;
    assert_eq!(next.source, failed.source);
    assert_eq!(next.identity, failed.identity);
    assert_eq!(next.attempt, failed.attempt + 1);
    assert!(next.lifecycle_revision > failed.lifecycle_revision);
    assert!(next.focus_epoch > failed.focus_epoch);
    assert_eq!(query(&port), next);
    close_original(&port, &close, 2);
    tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .expect("重连认证中关闭")
        .expect("actor");
    tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .expect("服务端释放")
        .expect("服务端任务");
    assert!(commands_tx.is_closed());
    assert_eq!(query(&port).state, NativeLifecycleState::Ended);
}
