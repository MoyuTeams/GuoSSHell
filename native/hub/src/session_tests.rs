//! 会话连接生命周期回归；测试凭证仅在内存中，不访问真实钥匙串或硬件。

#![allow(clippy::expect_used)]

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rshell_m0::rshell_core::{
    AuthenticationKind, CatalogMutation, ConnectionProfile, CredentialRef, KeyBinding, KeyCode,
    KeyModifiers, SecretUpdate, TerminalProfile, TerminalProfileId, TerminalSize, TransportKind,
};
use rshell_m0::rshell_storage::{
    ConfigurationChange, ConfigurationCommitOutcome, CredentialCoordinator, CredentialVault,
    MemoryCredentialVault, SqliteRepository, VaultError,
};
use secrecy::{ExposeSecret, SecretString};
use tokio::sync::Notify;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use super::{SessionCommand, run_session};
use crate::app::AppContext;
use crate::card::{CardContext, NoCards};
use crate::connect::{self, Abort, Channels, PromptIds, RemoteTerminal, Target};
use crate::keys::{MemoryKeyStore, PreferenceFile};
use crate::signals::interaction::InteractionReply;
use crate::signals::{
    ConnectRequest, DESKTOP_CONNECT_VERSION, DesktopConnectError, DesktopConnectRequest,
    DesktopConnectResult, DesktopTargetKind, FailureKind,
};

#[path = "session_native_tests.rs"]
mod native;

fn context(vault: Arc<dyn CredentialVault>) -> Arc<AppContext> {
    context_at(
        vault,
        PathBuf::new(),
        std::env::temp_dir().join(format!("guosh-session-tests-{}.json", uuid::Uuid::new_v4())),
    )
}

fn context_at(
    vault: Arc<dyn CredentialVault>,
    known_hosts: PathBuf,
    preferences: PathBuf,
) -> Arc<AppContext> {
    let repository = Arc::new(SqliteRepository::open_in_memory().expect("内存目录"));
    repository.migrate().expect("目录迁移");
    Arc::new(AppContext {
        credentials: CredentialCoordinator::new(repository.clone(), vault),
        repository,
        known_hosts,
        keys: Arc::new(MemoryKeyStore::new(false)),
        key_operations: Mutex::new(()),
        cards: Arc::new(CardContext::new(Arc::new(NoCards))),
        security_keys: Arc::new(crate::security_key::Unavailable),
        preferences: PreferenceFile::open(preferences),
        catalog_changed: Notify::new(),
        keys_changed: Notify::new(),
    })
}

fn profile() -> ConnectionProfile {
    let mut profile = ConnectionProfile::new("会话回归", "example.test");
    // 目录允许空用户名，NativeSshTransport 在建网前拒绝；已回答的连接只走到校验。
    profile.username.clear();
    profile.authentication = AuthenticationKind::Password;
    profile.transport = TransportKind::NativeSsh;
    profile
}

fn size() -> TerminalSize {
    TerminalSize {
        cols: 80,
        rows: 24,
        pixel_width: 0,
        pixel_height: 0,
        dpi: 96,
    }
}

fn reply(prompt_id: u32, accept: bool) -> crate::protected_reply::ProtectedReply {
    InteractionReply {
        session_id: 1,
        prompt_id,
        accept,
        answers: vec!["测试口令".into()],
        remember: false,
    }
    .into()
}

fn desktop(target: DesktopTargetKind) -> DesktopConnectRequest {
    DesktopConnectRequest {
        version: DESKTOP_CONNECT_VERSION,
        request_id: 23,
        session_id: 1,
        target,
        profile_id: String::new(),
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

fn binding(index: u8, action: &str) -> KeyBinding {
    KeyBinding {
        code: KeyCode::F(index),
        modifiers: KeyModifiers::default(),
        action: action.into(),
    }
}

#[tokio::test]
async fn desktop_targets_that_are_not_implemented_never_enter_the_transport() {
    let context = context(Arc::new(MemoryCredentialVault::new()));
    for target in [
        DesktopTargetKind::LocalShell,
        DesktopTargetKind::SystemOpenSsh,
    ] {
        let mut request = desktop(target);
        if target == DesktopTargetKind::SystemOpenSsh {
            request.host = "example.test".into();
            request.port = 22;
            request.username = "user".into();
        }
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            connect::resolve_desktop(&context, request),
        )
        .await
        .expect("拒绝不等待网络或子进程");
        assert!(matches!(
            result,
            Err((DesktopConnectError::TransportUnavailable, _))
        ));
    }
}

#[tokio::test]
async fn desktop_rejects_bad_fields_unknown_ids_and_unsupported_versions() {
    let context = context(Arc::new(MemoryCredentialVault::new()));
    let mut native = desktop(DesktopTargetKind::NativeSsh);
    native.host = "example.test".into();
    native.port = 22;
    native.username = "user".into();
    native.version += 1;
    assert!(matches!(
        connect::resolve_desktop(&context, native).await,
        Err((DesktopConnectError::UnsupportedVersion, _))
    ));

    let mut native = desktop(DesktopTargetKind::NativeSsh);
    native.host = "example.test".into();
    native.port = 22;
    native.username = "user".into();
    native.profile_id = "bad-id".into();
    assert!(matches!(
        connect::resolve_desktop(&context, native).await,
        Err((DesktopConnectError::InvalidProfileId, _))
    ));

    let mut native = desktop(DesktopTargetKind::NativeSsh);
    native.host = "example.test".into();
    native.port = 22;
    native.username = "user".into();
    native.profile_id = uuid::Uuid::new_v4().to_string();
    assert!(matches!(
        connect::resolve_desktop(&context, native).await,
        Err((DesktopConnectError::ProfileNotFound, _))
    ));

    let mut native = desktop(DesktopTargetKind::NativeSsh);
    native.connection_id = "invalid".into();
    assert!(matches!(
        connect::resolve_desktop(&context, native).await,
        Err((DesktopConnectError::InvalidConnectionId, _))
    ));

    let mut native = desktop(DesktopTargetKind::NativeSsh);
    native.connection_id = uuid::Uuid::new_v4().to_string();
    assert!(matches!(
        connect::resolve_desktop(&context, native).await,
        Err((DesktopConnectError::ConnectionNotFound, _))
    ));

    let mut local = desktop(DesktopTargetKind::LocalShell);
    local.host = "example.test".into();
    assert!(matches!(
        connect::resolve_desktop(&context, local).await,
        Err((DesktopConnectError::InvalidField, _))
    ));

    let mut native = desktop(DesktopTargetKind::NativeSsh);
    native.cols = 0;
    assert!(matches!(
        connect::resolve_desktop(&context, native).await,
        Err((DesktopConnectError::InvalidField, _))
    ));
}

#[tokio::test]
async fn native_desktop_uses_the_original_native_target_and_does_not_echo_secrets() {
    let context = context(Arc::new(MemoryCredentialVault::new()));
    let mut native = desktop(DesktopTargetKind::NativeSsh);
    native.host = "example.test".into();
    native.port = 22;
    native.username = "user".into();
    native.password = "测试秘密不回显".into();
    let prepared = connect::resolve_desktop(&context, native)
        .await
        .expect("桌面 Native 预校验");
    assert_eq!(prepared.target.profile.transport, TransportKind::NativeSsh);
    assert!(prepared.request.password.is_empty());
    assert_eq!(prepared.quick_password.expose_secret(), "测试秘密不回显");
    assert!(!prepared.target.saved);

    // 没有运行时不启动连接：非法请求仅发出关联结果，且结果不带秘密。
    let mut rejected = desktop(DesktopTargetKind::SystemOpenSsh);
    rejected.password = "测试秘密不回显".into();
    let (error, detail) = connect::resolve_desktop(&context, rejected)
        .await
        .err()
        .expect("拒绝不支持的字段");
    let result = DesktopConnectResult {
        request_id: 23,
        session_id: 1,
        error,
        detail,
    };
    assert_eq!(result.request_id, 23);
    assert_eq!(result.session_id, 1);
    assert_eq!(result.error, DesktopConnectError::InvalidField);
    assert!(
        !serde_json::to_string(&result)
            .expect("序列化结果")
            .contains("测试秘密不回显")
    );
}

#[tokio::test]
async fn saved_native_target_uses_its_profile_unless_the_request_selects_another() {
    let context = context(Arc::new(MemoryCredentialVault::new()));
    let mut selected = TerminalProfile::p0_default();
    selected.id = TerminalProfileId::from(uuid::Uuid::new_v4());
    selected.name = "目录选择的配置".into();
    selected.settings.font_size = 23.0;
    selected.settings.terminal_type = "vt220".into();
    context
        .repository
        .save_terminal_profile(selected.clone())
        .expect("保存配置");
    let mut saved = profile();
    saved.username = "user".into();
    saved.authentication = AuthenticationKind::KeyboardInteractive;
    saved.terminal_profile_id = Some(selected.id);
    saved.terminal_overrides.terminal_type = Some("screen-256color".into());
    let id = saved.id.0.to_string();
    context
        .credentials
        .apply_catalog(CatalogMutation::Create(saved), SecretUpdate::Unchanged)
        .expect("保存 Native 目录连接");

    let mut request = desktop(DesktopTargetKind::NativeSsh);
    request.connection_id = id.clone();
    let prepared = connect::resolve_desktop(&context, request)
        .await
        .expect("连接配置引用");
    assert!(prepared.target.saved);
    assert_eq!(prepared.settings.font_size, 23.0);
    assert_eq!(prepared.settings.terminal_type, "screen-256color");

    let mut request = desktop(DesktopTargetKind::NativeSsh);
    request.connection_id = id;
    request.profile_id = context
        .repository
        .load_settings()
        .expect("应用设置")
        .default_terminal_profile
        .0
        .to_string();
    let prepared = connect::resolve_desktop(&context, request)
        .await
        .expect("显式配置优先");
    assert_ne!(prepared.settings.font_size, 23.0);
}

#[tokio::test]
async fn desktop_native_merges_app_bindings_with_profile_precedence_in_one_snapshot() {
    let context = context(Arc::new(MemoryCredentialVault::new()));
    let initial = context
        .repository
        .load_configuration()
        .expect("初始设置快照");
    let mut selected = initial.profiles[0].clone();
    selected.settings.terminal_type = "vt220".into();
    selected.settings.key_bindings = vec![binding(1, "clear_scrollback")];
    let mut app = initial.settings;
    app.key_bindings = vec![binding(1, "new_tab"), binding(2, "split_vertical")];
    assert!(matches!(
        context
            .repository
            .commit_configuration(ConfigurationChange {
                expected_revision: initial.revision,
                settings: app,
                upsert_profiles: vec![selected],
                delete_profiles: vec![],
            }),
        Ok(ConfigurationCommitOutcome::Applied { .. })
    ));

    let mut native = desktop(DesktopTargetKind::NativeSsh);
    native.host = "example.test".into();
    native.port = 22;
    native.username = "user".into();
    let prepared = connect::resolve_desktop(&context, native)
        .await
        .expect("桌面有效配置");
    assert_eq!(prepared.settings.terminal_type, "vt220");
    assert_eq!(
        prepared.settings.key_bindings,
        [binding(1, "clear_scrollback"), binding(2, "split_vertical")],
        "应用新增 F2 生效，profile 对 F1 的绑定优先"
    );
}

#[tokio::test]
async fn changing_default_then_deleting_old_profile_never_mixes_settings_snapshots() {
    let context = context(Arc::new(MemoryCredentialVault::new()));
    let initial = context.repository.load_configuration().expect("初始设置");
    let old_id = initial.settings.default_terminal_profile;
    let mut old_profile = initial.profiles[0].clone();
    old_profile.settings.terminal_type = "vt220".into();
    let mut next_profile = old_profile.clone();
    next_profile.id = TerminalProfileId::from(uuid::Uuid::new_v4());
    next_profile.name = "新默认配置".into();
    next_profile.settings.terminal_type = "screen-256color".into();
    let mut old_app = initial.settings;
    old_app.key_bindings = vec![binding(3, "new_tab")];
    assert!(matches!(
        context
            .repository
            .commit_configuration(ConfigurationChange {
                expected_revision: initial.revision,
                settings: old_app,
                upsert_profiles: vec![old_profile, next_profile.clone()],
                delete_profiles: vec![],
            }),
        Ok(ConfigurationCommitOutcome::Applied { .. })
    ));
    let request = || {
        let mut native = desktop(DesktopTargetKind::NativeSsh);
        native.host = "example.test".into();
        native.port = 22;
        native.username = "user".into();
        native
    };
    let before = connect::resolve_desktop(&context, request())
        .await
        .expect("旧默认快照");
    assert_eq!(before.settings.terminal_type, "vt220");

    let writer = context.repository.clone();
    let update = std::thread::spawn(move || {
        let current = writer.load_configuration().expect("更新前快照");
        let mut app = current.settings;
        app.default_terminal_profile = next_profile.id;
        app.key_bindings = vec![binding(3, "split_vertical")];
        assert!(matches!(
            writer.commit_configuration(ConfigurationChange {
                expected_revision: current.revision,
                settings: app.clone(),
                upsert_profiles: vec![],
                delete_profiles: vec![],
            }),
            Ok(ConfigurationCommitOutcome::Applied { .. })
        ));
        let current = writer.load_configuration().expect("切换后快照");
        assert!(matches!(
            writer.commit_configuration(ConfigurationChange {
                expected_revision: current.revision,
                settings: app,
                upsert_profiles: vec![],
                delete_profiles: vec![old_id],
            }),
            Ok(ConfigurationCommitOutcome::Applied { .. })
        ));
    });
    for _ in 0..32 {
        let prepared = connect::resolve_desktop(&context, request())
            .await
            .expect("切换/删除期间必须取得完整快照");
        let pair = (
            prepared.settings.terminal_type.as_str(),
            prepared.settings.key_bindings[0].action.as_str(),
        );
        assert!(
            pair == ("vt220", "new_tab") || pair == ("screen-256color", "split_vertical"),
            "默认配置与应用绑定必须来自同一快照: {pair:?}"
        );
    }
    update.join().expect("完成切换与删除");
    let after = connect::resolve_desktop(&context, request())
        .await
        .expect("新默认快照");
    assert_eq!(after.settings.terminal_type, "screen-256color");
    assert_eq!(after.settings.key_bindings, [binding(3, "split_vertical")]);
    assert_eq!(
        before.settings.terminal_type, "vt220",
        "已启动会话仍持有旧快照"
    );
    assert_eq!(before.settings.key_bindings, [binding(3, "new_tab")]);
}

#[tokio::test]
async fn old_connect_request_rejects_a_system_catalog_entry_instead_of_falling_back() {
    let context = context(Arc::new(MemoryCredentialVault::new()));
    let mut system = profile();
    system.transport = TransportKind::SystemOpenSsh;
    system.authentication = AuthenticationKind::Agent;
    system.username = "user".into();
    let id = system.id.0.to_string();
    context
        .credentials
        .apply_catalog(CatalogMutation::Create(system), SecretUpdate::Unchanged)
        .expect("保存系统目标");
    let request = ConnectRequest {
        session_id: 1,
        connection_id: id,
        host: String::new(),
        port: 22,
        username: String::new(),
        password: String::new(),
        command: String::new(),
        cols: 80,
        rows: 24,
        pixel_width: 0,
        pixel_height: 0,
        dpi: 96,
    };
    let result = connect::resolve_target(&context, &request).await;
    assert!(matches!(
        result,
        Err(Abort::Failed {
            failure: FailureKind::InvalidTarget,
            ..
        })
    ));
}

#[cfg(any(target_os = "ios", target_os = "android"))]
#[tokio::test]
async fn forged_mobile_desktop_request_is_rejected_before_storage_or_transport() {
    let context = context(Arc::new(MemoryCredentialVault::new()));
    let request = desktop(DesktopTargetKind::LocalShell);
    assert!(matches!(
        connect::resolve_desktop(&context, request).await,
        Err((DesktopConnectError::PlatformUnavailable, _))
    ));
}

#[tokio::test]
async fn stale_acceptance_and_rejection_cannot_answer_a_reconnect_prompt() {
    for stale_accept in [true, false] {
        let context = context(Arc::new(MemoryCredentialVault::new()));
        let (commands_tx, mut commands) = unbounded_channel();
        let (replies_tx, mut replies) = unbounded_channel();
        let mut backlog = VecDeque::new();
        let mut prompts = PromptIds::default();
        let target = || Target {
            profile: profile(),
            saved: false,
        };
        assert!(replies_tx.send(reply(1, false)).is_ok());
        let first = connect::establish(
            &context,
            1,
            target(),
            SecretString::from(String::new()),
            RemoteTerminal {
                size: size(),
                terminal_type: None,
            },
            Channels {
                commands: &mut commands,
                replies: &mut replies,
                backlog: &mut backlog,
            },
            &mut prompts,
        )
        .await;
        assert!(matches!(first, Err(Abort::Cancelled)));

        // 模拟上一轮关闭后到达的旧答案排在本轮答案前面。
        assert!(replies_tx.send(reply(1, stale_accept)).is_ok());
        assert!(replies_tx.send(reply(2, !stale_accept)).is_ok());
        let second = connect::establish(
            &context,
            1,
            target(),
            SecretString::from(String::new()),
            RemoteTerminal {
                size: size(),
                terminal_type: None,
            },
            Channels {
                commands: &mut commands,
                replies: &mut replies,
                backlog: &mut backlog,
            },
            &mut prompts,
        )
        .await;
        if stale_accept {
            assert!(matches!(second, Err(Abort::Cancelled)), "只能接受本轮拒绝");
        } else {
            assert!(
                matches!(second, Err(Abort::Failed { .. })),
                "本轮回答应进入 transport 校验"
            );
        }
        drop(commands_tx);
    }
}

/// 每次读密码都通知测试，便于精确进入重连中的交互等待。
struct NotifyingVault(UnboundedSender<()>);

impl CredentialVault for NotifyingVault {
    fn get(&self, _: &CredentialRef) -> Result<Option<SecretString>, VaultError> {
        let _ = self.0.send(());
        Ok(None)
    }

    fn put(&self, _: &CredentialRef, _: &SecretString) -> Result<(), VaultError> {
        // 测试建立目录引用后模拟凭证缺失，使每轮都走连接端的交互分支。
        Ok(())
    }

    fn delete(&self, _: &CredentialRef) -> Result<(), VaultError> {
        unreachable!("此测试不删除凭证")
    }
}

#[tokio::test]
async fn closing_a_pane_during_reconnect_releases_the_session_and_queues() {
    let (reads_tx, mut reads) = unbounded_channel();
    let context = context(Arc::new(NotifyingVault(reads_tx)));
    let profile = profile();
    let id = profile.id.0.to_string();
    context
        .credentials
        .apply_catalog(
            CatalogMutation::Create(profile),
            SecretUpdate::Set(SecretString::from("测试凭证".to_owned())),
        )
        .expect("测试连接");
    let request = ConnectRequest {
        session_id: 1,
        connection_id: id,
        host: String::new(),
        port: 22,
        username: String::new(),
        // 第一轮无需提问，并确定以 transport 配置失败结束。
        password: "首轮测试口令".into(),
        command: String::new(),
        cols: 80,
        rows: 24,
        pixel_width: 0,
        pixel_height: 0,
        dpi: 96,
    };
    let (commands_tx, commands) = unbounded_channel();
    let (replies_tx, replies) = unbounded_channel();
    let released = Arc::downgrade(&context);
    let task = tokio::spawn(run_session(context, request, commands, replies));
    tokio::time::timeout(Duration::from_secs(2), reads.recv())
        .await
        .expect("第一轮")
        .expect("读密码");
    assert!(commands_tx.send(SessionCommand::Reconnect).is_ok());
    tokio::time::timeout(Duration::from_secs(2), reads.recv())
        .await
        .expect("重连开始")
        .expect("读密码");
    assert!(commands_tx.send(SessionCommand::Disconnect).is_ok());
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect("关闭窗格后会话必须结束")
        .expect("会话任务");
    assert!(commands_tx.is_closed());
    assert!(replies_tx.is_closed());
    assert!(released.upgrade().is_none(), "会话不再持有共享缓存");
}
