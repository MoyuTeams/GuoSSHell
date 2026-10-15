//! 真实 loopback SSH 经准备、认证、PTY、引擎收发与离线重连；不替代 GUI 验收。
#![allow(clippy::expect_used)]
#![cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]

use super::super::{SessionCommand, run_prepared_session};
use super::{context_at, size};
use crate::app::AppContext;
use crate::connect::{self, Abort, Channels, PromptIds, RemoteTerminal};
use crate::desktop_prepare::{Loaded, METADATA_LIMIT, Preparations, Usage};
use crate::desktop_runtime::{SOURCE, Source};
use crate::signals::desktop_session::{
    DesktopGeometry, DesktopPreparationState, DesktopPrepare, DesktopSource, DesktopStart, VERSION,
};
use crate::signals::settings::{FontFileRequest, ProfileFontMode};
use crate::signals::{DesktopTargetKind, FailureKind};
use rshell_m0::rshell_core::{
    AuthenticationKind, CatalogMutation, ConnectionProfile, SecretUpdate, TerminalInput,
    TerminalProfile, TransportKind,
};
use rshell_m0::rshell_storage::{
    ConfigurationChange, ConfigurationCommitOutcome, MemoryCredentialVault,
};
use russh::keys::ssh_key::private::Ed25519Keypair;
use russh::keys::{PrivateKey, check_known_hosts_path, known_hosts::learn_known_hosts_path};
use russh::server::{self, Auth, Handler, Session};
use russh::{Channel, ChannelId, Pty};
use secrecy::SecretString;
use std::borrow::Cow;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::{Notify, mpsc::unbounded_channel, watch};
use tokio::time::Instant;

const USER: &str = "native-regression";
const PASSWORD: &str = "虚构的回归口令";
const COMMAND: &str = "test-session-command";
const ANSWERBACK: &str = "冻结应答-ENQ";
// 仅测试墙钟 watchdog；不改变生产连接或关闭时限。
const WATCHDOG: Duration = Duration::from_secs(15);

struct Root(PathBuf);
impl Drop for Root {
    fn drop(&mut self) {
        // 路径只由本轮 UUID 创建，不清理共享临时目录。
        std::fs::remove_dir_all(&self.0).expect("清理本轮独占目录");
    }
}

struct Fixture {
    context: Arc<AppContext>,
    root: Root,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("guosh-native-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).expect("本轮独占目录");
        Self {
            context: context_at(
                Arc::new(MemoryCredentialVault::new()),
                root.join("known_hosts"),
                root.join("preferences.json"),
            ),
            root: Root(root),
        }
    }

    fn font(&self, action: &str) {
        let (state, _) = crate::fonts::handle(
            &self.root.0,
            &self.context.preferences,
            FontFileRequest {
                request_id: 1,
                slot: "terminal".into(),
                action: action.into(),
                file_name: "native.ttf".into(),
            },
            if action == "apply" {
                include_bytes!("../../../assets/fonts/MesloLGS-NF-Regular.ttf").to_vec()
            } else {
                vec![]
            },
        );
        assert!(state.error.is_empty(), "真实字体操作");
    }

    fn save(&self, profile: TerminalProfile) {
        let snapshot = self
            .context
            .repository
            .load_configuration()
            .expect("配置快照");
        assert!(matches!(
            self.context
                .repository
                .commit_configuration(ConfigurationChange {
                    expected_revision: snapshot.revision,
                    settings: snapshot.settings,
                    upsert_profiles: vec![profile],
                    delete_profiles: vec![],
                })
                .expect("配置 CAS"),
            ConfigurationCommitOutcome::Applied { .. }
        ));
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PtyRequest {
    term: String,
    geometry: [u32; 4],
}

#[derive(Clone, Default, Debug)]
struct Connection {
    authentication: Vec<(String, bool)>,
    authenticated: bool,
    ptys: Vec<PtyRequest>,
    commands: Vec<Vec<u8>>,
    received: Vec<u8>,
    endpoint: Option<(server::Handle, ChannelId)>,
    ended: bool,
    failure: Option<String>,
}

#[derive(Default)]
struct Observed {
    connections: Mutex<Vec<Connection>>,
    changed: Notify,
}
impl Observed {
    fn update(&self, index: usize, change: impl FnOnce(&mut Connection)) {
        change(&mut self.connections.lock().expect("服务端记录")[index]);
        self.changed.notify_one();
    }

    async fn wait(&self, index: usize, ready: impl Fn(&Connection) -> bool) -> Connection {
        tokio::time::timeout(WATCHDOG, async {
            loop {
                let snapshot = self
                    .connections
                    .lock()
                    .expect("服务端快照")
                    .get(index)
                    .cloned();
                if let Some(snapshot) = snapshot {
                    if ready(&snapshot) {
                        assert!(snapshot.failure.is_none(), "SSH 服务端失败：{snapshot:?}");
                        return snapshot;
                    }
                    assert!(!snapshot.ended, "所需阶段前连接已结束：{snapshot:?}");
                }
                self.changed.notified().await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "服务端阶段未完成：{:?}",
                self.connections.lock().expect("服务端快照")
            )
        })
    }
}

struct PeerHandler {
    observed: Arc<Observed>,
    index: usize,
}
impl Handler for PeerHandler {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        let accepted = user == USER && password == PASSWORD;
        self.observed.update(self.index, |c| {
            c.authentication.push((user.into(), accepted))
        });
        Ok(if accepted {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }

    async fn auth_succeeded(&mut self, _: &mut Session) -> Result<(), Self::Error> {
        self.observed.update(self.index, |c| c.authenticated = true);
        Ok(())
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
        cols: u32,
        rows: u32,
        pixel_width: u32,
        pixel_height: u32,
        _: &[(Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.observed.update(self.index, |c| {
            c.ptys.push(PtyRequest {
                term: term.into(),
                geometry: [cols, rows, pixel_width, pixel_height],
            })
        });
        session.channel_success(channel)?;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        command: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        session.data(channel, b"\x05".as_slice())?;
        self.observed.update(self.index, |c| {
            c.commands.push(command.to_vec());
            c.endpoint = Some((session.handle(), channel));
        });
        Ok(())
    }

    async fn data(
        &mut self,
        _: ChannelId,
        data: &[u8],
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        self.observed
            .update(self.index, |c| c.received.extend_from_slice(data));
        Ok(())
    }
}

struct Peer {
    port: u16,
    key: PrivateKey,
    observed: Arc<Observed>,
    stop: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}
impl Peer {
    async fn new() -> Self {
        Self::with_compression(None).await
    }

    async fn with_compression(compression: Option<russh::compression::Name>) -> Self {
        let socket = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("loopback 端口");
        let port = socket.local_addr().expect("绑定地址").port();
        let key = PrivateKey::from(Ed25519Keypair::from_seed(&[73; 32]));
        let mut config = server::Config {
            keys: vec![key.clone()],
            ..server::Config::default()
        };
        if let Some(compression) = compression {
            // 对端只提供指定算法，成功协商不能退回 none。
            config.preferred.compression = Cow::Owned(vec![compression]);
        }
        let config = Arc::new(config);
        let observed = Arc::new(Observed::default());
        let records = observed.clone();
        let (stop, mut stopping) = watch::channel(false);
        let task = tokio::spawn(async move {
            loop {
                let (stream, _) = tokio::select! {
                    result = socket.accept() => result.expect("接受真实连接"),
                    _ = stopping.changed() => return,
                };
                let index = {
                    let mut connections = records.connections.lock().expect("新增连接");
                    connections.push(Connection::default());
                    connections.len() - 1
                };
                let handler = PeerHandler {
                    observed: records.clone(),
                    index,
                };
                let mut running = server::run_stream(config.clone(), stream, handler)
                    .await
                    .expect("运行 SSH 服务端");
                let handle = running.handle();
                tokio::select! {
                    result = &mut running => {
                        // 客户端正常关闭 socket 时，各宿主可能返回 EOF、reset 或 abort。
                        let expected = match &result {
                            Ok(()) | Err(russh::Error::Disconnect) => true,
                            Err(russh::Error::IO(error)) => matches!(error.kind(),
                                std::io::ErrorKind::ConnectionAborted | std::io::ErrorKind::ConnectionReset
                                | std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::UnexpectedEof),
                            _ => false,
                        };
                        records.update(index, |c| {
                            c.ended = true;
                            if !expected { c.failure = Some(format!("{result:?}")); }
                        });
                    }
                    _ = stopping.changed() => {
                        let _ = handle.disconnect(russh::Disconnect::ByApplication, "测试结束".into(), String::new()).await;
                        let _ = running.await;
                        return;
                    }
                }
            }
        });
        Self {
            port,
            key,
            observed,
            stop,
            task,
        }
    }

    fn trust(&self, fixture: &Fixture) {
        learn_known_hosts_path(
            "127.0.0.1",
            self.port,
            self.key.public_key(),
            &fixture.context.known_hosts,
        )
        .expect("本轮真实 known_hosts");
        assert!(
            check_known_hosts_path(
                "127.0.0.1",
                self.port,
                self.key.public_key(),
                &fixture.context.known_hosts
            )
            .expect("真实文件匹配")
        );
    }

    async fn finish(self) {
        self.stop.send_replace(true);
        tokio::time::timeout(WATCHDOG, self.task)
            .await
            .expect("服务端结束 watchdog")
            .expect("服务端任务");
    }
}

fn prepare(connection: &ConnectionProfile) -> DesktopPrepare {
    DesktopPrepare {
        version: VERSION,
        request_id: 10,
        source: DesktopSource {
            session_id: 71,
            generation: 1,
        },
        target: DesktopTargetKind::NativeSsh,
        profile_id: String::new(),
        connection_id: connection.id.0.to_string(),
        host: String::new(),
        port: 0,
        username: String::new(),
        command: String::new(),
    }
}

#[tokio::test]
async fn native_prepared_auth_pty_answerback_io_reconnect_and_last_font_owner() {
    prepared_session_io(None, false).await;
}

#[tokio::test]
async fn native_prepared_zlib_auth_pty_answerback_io_reconnect_and_last_font_owner() {
    prepared_session_io(Some(russh::compression::ZLIB), false).await;
}

#[tokio::test]
async fn native_prepared_delayed_zlib_auth_pty_answerback_io_reconnect_and_last_font_owner() {
    prepared_session_io(Some(russh::compression::ZLIB_LEGACY), false).await;
}

#[tokio::test]
async fn native_prepared_remembered_password_is_used_on_same_pane_reconnect() {
    prepared_session_io(None, true).await;
}

#[test]
fn russh_compression_preserves_each_channel_data_packet() {
    let mut encoder = russh::compression::Compress::None;
    let mut decoder = russh::compression::Decompress::None;
    russh::compression::Compression::Zlib.init_compress(&mut encoder);
    russh::compression::Compression::Zlib.init_decompress(&mut decoder);
    let mut compressed = Vec::new();
    let mut output = Vec::new();
    for text in [
        ANSWERBACK,
        "持续输入一",
        ANSWERBACK,
        "持续输入二",
        ANSWERBACK,
    ] {
        // RFC 4254 CHANNEL_DATA：类型、channel、SSH string 长度与内容。
        let mut packet = vec![94, 0, 0, 0, 2];
        packet.extend_from_slice(&(text.len() as u32).to_be_bytes());
        packet.extend_from_slice(text.as_bytes());
        let length = encoder
            .compress_into(&packet, &mut compressed, 0)
            .expect("连续压缩");
        let decoded = decoder
            .decompress(&compressed[..length], &mut output)
            .expect("连续解压");
        assert_eq!(decoded, packet, "每个压缩包须独立完整解出，不跨包遗留数据");
    }
}

async fn prepared_session_io(compression: Option<russh::compression::Name>, remember: bool) {
    let fixture = Fixture::new();
    let peer = Peer::with_compression(compression).await;
    peer.trust(&fixture);
    fixture.font("apply");
    let mut profile =
        crate::settings::default_profile(&fixture.context.repository).expect("默认配置");
    profile.settings.terminal_type = "vt220".into();
    profile.settings.answerback = ANSWERBACK.into();
    crate::profile_fonts::write_binding(
        &mut profile.settings,
        &crate::profile_fonts::binding(ProfileFontMode::InheritTerminalSlot, None),
    )
    .expect("继承本轮字体");
    fixture.save(profile.clone());
    let mut connection = ConnectionProfile::new("真实 Native 回归", "127.0.0.1");
    connection.port = peer.port;
    connection.username = USER.into();
    connection.authentication = AuthenticationKind::Password;
    connection.transport = TransportKind::NativeSsh;
    connection.terminal_profile_id = Some(profile.id);
    connection.remote_command = Some(COMMAND.into());
    fixture
        .context
        .credentials
        .apply_catalog(
            CatalogMutation::Create(connection.clone()),
            if remember {
                SecretUpdate::Unchanged
            } else {
                SecretUpdate::Set(SecretString::from(PASSWORD.to_owned()))
            },
        )
        .expect("虚构凭证目录");

    let request = prepare(&connection);
    let mut table = Preparations::native();
    let (identity, lease, _) = table
        .begin(request.clone(), Instant::now())
        .expect("真实准备预约");
    let (result, lease) =
        connect::resolve_prepared(&fixture.context, request.native_request(), lease).await;
    let (reply, binary) = table
        .complete(
            Loaded {
                identity: identity.clone(),
                result,
                lease,
            },
            Instant::now(),
        )
        .expect("准备完成");
    assert_eq!(reply.state, DesktopPreparationState::Prepared);
    let snapshot = reply.snapshot.expect("准备投影");
    assert_eq!(snapshot.profile.terminal_type, "vt220");
    assert_eq!(snapshot.profile.answerback, ANSWERBACK);
    let font_len = binary.len();
    assert!(font_len > 0);
    drop(binary);

    // Start 前编辑存储，实际网络仍须使用已准备的目标、命令和终端配置。
    profile.settings.terminal_type = "xterm".into();
    profile.settings.answerback = "不能用于旧会话".into();
    fixture.save(profile);
    if !remember {
        connection.host = "changed.invalid".into();
        connection.username = "changed-user".into();
        connection.remote_command = Some("changed-command".into());
        fixture
            .context
            .credentials
            .apply_catalog(CatalogMutation::Update(connection), SecretUpdate::Unchanged)
            .expect("编辑目录");
    }
    fixture.font("reset");

    let start = DesktopStart {
        version: VERSION,
        request_id: 11,
        prepare_request_id: request.request_id,
        source: request.source.clone(),
        identity: identity.clone(),
        geometry: DesktopGeometry {
            cols: 91,
            rows: 31,
            pixel_width: 910,
            pixel_height: 620,
            dpi: 144,
        },
        font_projection: snapshot.font_projection,
        password: String::new(),
    };
    let launch = table
        .start(&start, SecretString::from(String::new()), Instant::now())
        .expect("真实 Start");
    let retained = launch.target.font.as_ref().expect("运行字体").clone();
    let weak = Arc::downgrade(retained.resource.as_ref().expect("真实字体字节").bytes());
    let running_usage = Usage {
        count: 0,
        bytes: font_len + METADATA_LIMIT,
        inflight: 0,
    };
    assert_eq!(table.budget.usage(), running_usage);
    let (commands_tx, commands) = unbounded_channel();
    let (replies_tx, replies) = unbounded_channel();
    if remember {
        // 已预置信任；只回答首次密码问题，重连不得再次索取答案。
        assert!(
            replies_tx
                .send(crate::protected_reply::ProtectedReply::from(
                    crate::signals::interaction::InteractionReply {
                        session_id: request.source.session_id,
                        prompt_id: 1,
                        accept: true,
                        answers: vec![PASSWORD.to_owned()],
                        remember: true,
                    }
                ))
                .is_ok()
        );
    }
    let (close, closing) = watch::channel(false);
    let context = fixture.context.clone();
    let context_weak = Arc::downgrade(&context);
    let source = Source {
        source: launch.source,
        identity: launch.identity,
    };
    let task = tokio::spawn(SOURCE.scope(
        source,
        crate::transport_lifecycle::owner(
            closing,
            run_prepared_session(context, launch.target, commands, replies),
        ),
    ));

    for index in 0..2 {
        let first = peer
            .observed
            .wait(index, |c| c.received.len() >= ANSWERBACK.len())
            .await;
        assert_eq!(first.authentication, vec![(USER.into(), true)]);
        assert!(first.authenticated);
        assert_eq!(
            first.ptys,
            vec![PtyRequest {
                term: "vt220".into(),
                geometry: [91, 31, 910, 620]
            }]
        );
        assert_eq!(first.commands, vec![COMMAND.as_bytes().to_vec()]);
        assert_eq!(first.received, ANSWERBACK.as_bytes());
        let (handle, channel) = first.endpoint.expect("真实远端 channel");
        let mut expected = ANSWERBACK.as_bytes().to_vec();
        for text in ["持续输入一", "持续输入二"] {
            assert!(
                commands_tx
                    .send(SessionCommand::Input(TerminalInput::CommittedText(
                        text.into()
                    )))
                    .is_ok()
            );
            expected.extend_from_slice(text.as_bytes());
            let received = peer
                .observed
                .wait(index, |c| c.received.len() >= expected.len())
                .await;
            assert_eq!(received.received, expected);
            handle
                .data(channel, b"remote-output\r\n\x05".as_slice())
                .await
                .expect("持续真实输出");
            expected.extend_from_slice(ANSWERBACK.as_bytes());
            let received = peer
                .observed
                .wait(index, |c| c.received.len() >= expected.len())
                .await;
            assert_eq!(received.received, expected, "每次 ENQ 精确应答且仍可收发");
        }
        assert!(weak.upgrade().is_some());
        assert_eq!(
            weak.strong_count(),
            2,
            "运行任务与迟到 holder 均持有实际字体"
        );
        assert_eq!(table.budget.usage(), running_usage);
        if index == 0 {
            handle.eof(channel).await.expect("远端正常 EOF");
            handle.close(channel).await.expect("远端关闭 channel");
            peer.observed.wait(index, |c| c.ended).await;
            assert!(!task.is_finished(), "远端结束后仍保留离线会话");
            assert!(weak.upgrade().is_some());
            assert_eq!(weak.strong_count(), 2, "离线会话仍持有同一冻结字体");
            assert_eq!(table.budget.usage(), running_usage);
            assert!(commands_tx.send(SessionCommand::Reconnect).is_ok());
        }
    }
    // 清除测试的 context owner，文件目录仍保留到全部任务退出。
    let Fixture {
        context,
        root: _root,
    } = fixture;
    drop(context);
    close.send_replace(true);
    assert_eq!(
        tokio::time::timeout(WATCHDOG, task)
            .await
            .expect("关闭 watchdog")
            .expect("会话任务"),
        0,
        "正常 Native shutdown 不失败"
    );
    assert!(commands_tx.is_closed());
    assert!(replies_tx.is_closed());
    assert!(context_weak.upgrade().is_none(), "关闭释放会话 context");
    peer.observed.wait(1, |c| c.ended).await;
    assert!(weak.upgrade().is_some(), "关闭后迟到字体 owner 仍持有字节");
    assert_eq!(weak.strong_count(), 1, "会话不再持有字体");
    assert_eq!(table.budget.usage(), running_usage);
    drop(retained);
    assert!(weak.upgrade().is_none(), "最后一个字体 owner 释放真实字节");
    assert_eq!(table.budget.usage(), Usage::default());
    peer.finish().await;
}

#[tokio::test]
async fn native_wrong_password_rejects_before_pty_and_releases_real_transport() {
    let fixture = Fixture::new();
    let peer = Peer::new().await;
    peer.trust(&fixture);
    let mut profile = ConnectionProfile::new("拒绝虚构凭证", "127.0.0.1");
    profile.port = peer.port;
    profile.username = USER.into();
    profile.authentication = AuthenticationKind::Password;
    profile.transport = TransportKind::NativeSsh;
    let (_commands_tx, mut commands) = unbounded_channel();
    let (_replies_tx, mut replies) = unbounded_channel();
    let mut backlog = VecDeque::new();
    let mut prompts = PromptIds::default();
    let result = tokio::time::timeout(
        WATCHDOG,
        connect::establish(
            &fixture.context,
            72,
            connect::Target::ssh(profile, false),
            SecretString::from("错误的虚构口令".to_owned()),
            RemoteTerminal {
                size: size(),
                terminal_type: Some("vt220"),
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
    .expect("认证拒绝 watchdog");
    assert!(matches!(
        result,
        Err(Abort::Failed {
            failure: FailureKind::Authentication,
            ..
        })
    ));
    let observed = peer.observed.wait(0, |c| c.ended).await;
    assert_eq!(observed.authentication, vec![(USER.into(), false)]);
    assert!(!observed.authenticated);
    assert!(observed.ptys.is_empty());
    assert!(observed.commands.is_empty());
    assert!(observed.received.is_empty());
    peer.finish().await;
}
