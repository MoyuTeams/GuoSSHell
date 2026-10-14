//! 准备态真实存储/字体接缝及可控时钟竞态，不建立网络连接。
#![allow(clippy::expect_used)]

use super::{
    BYTE_LIMIT, Budget, COUNT_LIMIT, FONT_LIMIT, Failure, INFLIGHT_LIMIT, Lease, Loaded,
    METADATA_LIMIT, Preparations, RESERVATION, State, TTL, Usage,
};
use crate::app::AppContext;
use crate::signals::desktop_session::{
    DesktopCancel, DesktopGeometry, DesktopPrepare, DesktopSource, DesktopStart, VERSION,
};
use crate::signals::{
    DesktopTargetKind,
    settings::{FontFileRequest, ProfileFontMode},
};
use rshell_m0::rshell_core::{CatalogMutation, ConnectionProfile, SecretUpdate, TerminalProfile};
use rshell_m0::rshell_storage::{
    ConfigurationChange, CredentialCoordinator, MemoryCredentialVault, SqliteRepository,
};
use secrecy::{ExposeSecret, SecretString};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::time::Instant;

struct Cleanup(PathBuf);
impl std::ops::Deref for Cleanup {
    type Target = std::path::Path;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).expect("释放 SQLite 后清理本轮目录");
    }
}
// 字段按声明顺序释放，失败断言也先关库再清理。
struct Fixture {
    context: Arc<AppContext>,
    root: Cleanup,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("guosh-prepare-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).expect("独立临时目录");
        let repository =
            Arc::new(SqliteRepository::open(root.join("catalog.sqlite3")).expect("真实文件目录"));
        repository.migrate().expect("迁移");
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
        Self {
            context,
            root: Cleanup(root),
        }
    }
    fn save(&self, profile: TerminalProfile) {
        let snapshot = self.context.repository.load_configuration().expect("配置");
        assert!(matches!(
            self.context
                .repository
                .commit_configuration(ConfigurationChange {
                    expected_revision: snapshot.revision,
                    settings: snapshot.settings,
                    upsert_profiles: vec![profile],
                    delete_profiles: vec![]
                })
                .expect("CAS"),
            rshell_m0::rshell_storage::ConfigurationCommitOutcome::Applied { .. }
        ));
    }
    fn slot(&self, action: &str) {
        let (state, _) = crate::fonts::handle(
            &self.root,
            &self.context.preferences,
            FontFileRequest {
                request_id: 1,
                slot: "terminal".into(),
                action: action.into(),
                file_name: "chosen.ttf".into(),
            },
            if action == "apply" {
                include_bytes!("../../../assets/fonts/MesloLGS-NF-Regular.ttf").to_vec()
            } else {
                vec![]
            },
        );
        assert!(state.error.is_empty(), "字体操作");
    }
    async fn load(&self, request: &DesktopPrepare, identity: String, lease: Lease) -> Loaded {
        let (result, lease) =
            crate::connect::resolve_prepared(&self.context, request.native_request(), lease).await;
        Loaded {
            identity,
            result,
            lease,
        }
    }
    fn close(self) {
        drop(self);
    }
}

fn prepare(session: u32, generation: u64, request_id: u32) -> DesktopPrepare {
    DesktopPrepare {
        version: VERSION,
        request_id,
        source: DesktopSource {
            session_id: session,
            generation,
        },
        target: DesktopTargetKind::NativeSsh,
        profile_id: String::new(),
        connection_id: String::new(),
        host: "example.test".into(),
        port: 22,
        username: "fixture".into(),
        command: String::new(),
    }
}
fn start(request: &DesktopPrepare, identity: &str) -> DesktopStart {
    DesktopStart {
        version: VERSION,
        request_id: request.request_id + 1,
        prepare_request_id: request.request_id,
        source: request.source.clone(),
        identity: identity.into(),
        geometry: DesktopGeometry {
            cols: 91,
            rows: 31,
            pixel_width: 910,
            pixel_height: 620,
            dpi: 144,
        },
        font_projection: format!("{identity}:font"),
        password: String::new(),
    }
}
fn cancel(request: &DesktopPrepare, identity: &str) -> DesktopCancel {
    DesktopCancel {
        version: VERSION,
        request_id: request.request_id + 2,
        prepare_request_id: request.request_id,
        source: request.source.clone(),
        identity: identity.into(),
    }
}
fn secret() -> SecretString {
    SecretString::from("仅测试最终启动秘密".to_owned())
}

#[test]
fn production_table_enables_desktop_only_and_explicit_off_stays_policy_unavailable() {
    assert_eq!(
        Preparations::native().native_enabled,
        crate::connect::desktop_supported()
    );
    // 桌面目标：生产表直接进入预约；显式关闭表仍按策略拒绝且不占预算。
    let mut production = Preparations {
        native_enabled: true,
        ..Preparations::default()
    };
    let (identity, _lease, replaced) = production
        .begin_on(prepare(5, 1, 7), Instant::now(), true)
        .expect("桌面生产入口开启");
    assert!(!identity.is_empty() && replaced.is_none());
    let mut off = Preparations::default();
    assert_eq!(
        off.begin_on(prepare(5, 1, 7), Instant::now(), true).err(),
        Some(Failure::PolicyUnavailable)
    );
    assert_eq!(off.budget.usage(), Usage::default());
    // 移动目标：无论开关如何都在平台检查处拒绝，旧 ConnectRequest 路径不受影响。
    for mut table in [Preparations::native(), Preparations::default()] {
        assert_eq!(
            table
                .begin_on(prepare(6, 1, 8), Instant::now(), false)
                .err(),
            Some(Failure::PlatformUnavailable)
        );
        assert_eq!(table.budget.usage(), Usage::default());
    }
}

#[test]
fn explicit_off_preparations_reject_v2_before_reservation_and_preserve_result_correlation() {
    let mut table = Preparations::default();
    let request = prepare(17, 9, 11);
    let failure = table
        .begin(request.clone(), Instant::now())
        .err()
        .expect("显式关闭拒绝准备");
    assert_eq!(failure, Failure::PolicyUnavailable);
    let reply = super::result(
        request.source.clone(),
        request.request_id,
        request.request_id,
        String::new(),
        State::Failed,
        failure,
    );
    assert_eq!(reply.source, request.source);
    assert_eq!(reply.request_id, 11);
    assert_eq!(reply.prepare_request_id, 11);
    assert_eq!(reply.state, State::Failed);
    assert_eq!(reply.failure, Failure::PolicyUnavailable);
    assert!(reply.identity.is_empty());
    assert!(reply.snapshot.is_none());

    let request = start(&request, "客户端回传身份");
    let failure = table
        .start(&request, secret(), Instant::now())
        .err()
        .expect("显式关闭拒绝启动");
    assert_eq!(failure, Failure::PolicyUnavailable);
    let reply = super::result(
        request.source.clone(),
        request.request_id,
        request.prepare_request_id,
        request.identity.clone(),
        State::Failed,
        failure,
    );
    assert_eq!(reply.source, request.source);
    assert_eq!(reply.request_id, 12);
    assert_eq!(reply.prepare_request_id, 11);
    assert_eq!(reply.identity, request.identity);
    assert_eq!(reply.state, State::Failed);
    assert_eq!(reply.failure, Failure::PolicyUnavailable);
    assert!(reply.snapshot.is_none());
    assert_eq!(table.budget.usage(), Usage::default());
    assert!(table.entries.is_empty());
    assert!(table.history.is_empty());
    assert!(table.deadline().is_none());
}

#[tokio::test]
async fn explicit_off_guard_preserves_prepared_entry_font_and_budget_until_cancel() {
    let f = Fixture::new();
    f.slot("apply");
    let request = prepare(1, 1, 10);
    let received = Instant::now();
    let mut enabled = Preparations::enabled_for_test();
    let (identity, lease, _) = enabled
        .begin(request.clone(), received)
        .expect("显式测试预约");
    let loaded = f.load(&request, identity.clone(), lease).await;
    let weak = Arc::downgrade(
        loaded
            .result
            .as_ref()
            .expect("真实捕获")
            .font
            .resource
            .as_ref()
            .expect("实际字体")
            .bytes(),
    );
    let (_, bytes) = enabled.complete(loaded, received).expect("准备就绪");
    let mut table = Preparations {
        entries: enabled.entries,
        budget: enabled.budget,
        ..Preparations::default()
    };
    let reserved = Usage {
        count: 1,
        bytes: bytes.len() * 2 + METADATA_LIMIT,
        inflight: 0,
    };
    assert_eq!(table.budget.usage(), reserved);
    for next in [prepare(1, 2, 20), prepare(2, 1, 30)] {
        assert!(matches!(
            table.begin(next, received),
            Err(Failure::PolicyUnavailable)
        ));
    }
    for now in [received, received + TTL] {
        assert_eq!(
            table
                .start(&start(&request, &identity), secret(), now)
                .err(),
            Some(Failure::PolicyUnavailable)
        );
    }
    assert_eq!(table.entries.len(), 1);
    let entry = table.entries.get(&1).expect("未替换或消费准备");
    assert_eq!(entry.identity, identity);
    assert!(entry.ready.is_some());
    assert!(table.history.is_empty());
    assert_eq!(table.budget.usage(), reserved);
    assert!(weak.upgrade().is_some());
    assert_eq!(
        table
            .cancel(&cancel(&request, &identity), received)
            .expect("关闭 guard 仍允许取消清理")
            .failure,
        Failure::Cancelled
    );
    assert_eq!(
        table.budget.usage(),
        Usage {
            count: 0,
            bytes: bytes.len(),
            inflight: 0
        },
        "取消释放准备字体，但尚未交付的独立二进制仍持有额度"
    );
    assert!(weak.upgrade().is_none());
    drop(bytes);
    assert_eq!(table.budget.usage(), Usage::default());
    f.close();
}

#[tokio::test]
async fn prepared_configuration_catalog_and_actual_font_survive_edits_and_runtime_transfer() {
    let f = Fixture::new();
    f.slot("apply");
    let mut profile = crate::settings::default_profile(&f.context.repository).expect("默认配置");
    profile.settings.terminal_type = "vt220".into();
    crate::profile_fonts::write_binding(
        &mut profile.settings,
        &crate::profile_fonts::binding(ProfileFontMode::InheritTerminalSlot, None),
    )
    .expect("明确继承");
    f.save(profile.clone());
    let mut connection = ConnectionProfile::new("固定目录目标", "example.test");
    connection.transport = rshell_m0::rshell_core::TransportKind::NativeSsh;
    connection.authentication = rshell_m0::rshell_core::AuthenticationKind::Password;
    connection.username = "fixture".into();
    connection.terminal_profile_id = Some(profile.id);
    connection.terminal_overrides.initial_cols = Some(107);
    f.context
        .credentials
        .apply_catalog(
            CatalogMutation::Create(connection.clone()),
            SecretUpdate::Unchanged,
        )
        .expect("目录");
    let mut request = prepare(1, 1, 10);
    request.connection_id = connection.id.0.to_string();
    request.host.clear();
    request.port = 0;
    request.username.clear();
    let mut table = Preparations::enabled_for_test();
    let (identity, lease, _) = table
        .begin(request.clone(), Instant::now())
        .expect("准备预约");
    let loaded = f.load(&request, identity.clone(), lease).await;
    assert!(
        loaded.result.is_ok(),
        "目录准备失败：{:?}",
        loaded.result.as_ref().err()
    );
    let (reply, bytes) = table.complete(loaded, Instant::now()).expect("发布准备");
    assert_eq!(reply.state, State::Prepared);
    let snapshot = reply.snapshot.expect("完整配置投影");
    let revision = snapshot.font.configuration_revision;
    assert_eq!(snapshot.profile.terminal_type, "vt220");
    assert_eq!(snapshot.profile.color_scheme, "default");
    assert_eq!(snapshot.profile.initial_cols, 107);
    assert!(!bytes.is_empty());
    assert_eq!(table.budget.usage().bytes, bytes.len() * 2 + METADATA_LIMIT);
    profile.settings.terminal_type = "xterm".into();
    f.save(profile);
    connection.host = "changed.example.test".into();
    connection.terminal_overrides.initial_cols = Some(44);
    f.context
        .credentials
        .apply_catalog(CatalogMutation::Update(connection), SecretUpdate::Unchanged)
        .expect("更改目录");
    f.slot("reset");
    let launch = table
        .start(
            &start(&request, &identity),
            SecretString::from(String::new()),
            Instant::now(),
        )
        .expect("实测启动");
    assert_eq!(launch.target.target.profile.host, "example.test");
    assert_eq!(launch.target.settings.terminal_type, "vt220");
    assert_eq!(launch.target.settings.cols, 107);
    assert_eq!(launch.target.request.cols, 91);
    assert_eq!(launch.target.request.dpi, 144);
    assert_eq!(
        launch
            .target
            .font
            .as_ref()
            .expect("运行字体")
            .projection
            .configuration_revision,
        revision
    );
    let resource = launch
        .target
        .font
        .as_ref()
        .expect("运行字体")
        .resource
        .as_ref()
        .expect("真实引用");
    assert_eq!(resource.bytes().as_ref(), bytes.as_slice());
    assert_eq!(
        table.budget.usage(),
        Usage {
            count: 0,
            bytes: bytes.len() * 2 + METADATA_LIMIT,
            inflight: 0,
        },
        "Start 释放准备槽位，但运行字体和发送字节仍受计量"
    );
    assert_eq!(
        table
            .cancel(&cancel(&request, &identity), Instant::now())
            .err(),
        Some(Failure::AlreadyStarted)
    );
    table.expire(Instant::now() + TTL);
    assert_eq!(
        resource.bytes().as_ref(),
        bytes.as_slice(),
        "旧 cancel/timer 不释放运行快照"
    );
    let weak = Arc::downgrade(resource.bytes());
    drop(launch);
    assert!(weak.upgrade().is_none(), "退出释放实际资源");
    f.close();
}

#[tokio::test]
async fn repeated_binary_handoff_retains_running_font_without_accumulating_orphaned_quota() {
    let f = Fixture::new();
    f.slot("apply");
    let mut profile = crate::settings::default_profile(&f.context.repository).expect("默认配置");
    crate::profile_fonts::write_binding(
        &mut profile.settings,
        &crate::profile_fonts::binding(ProfileFontMode::InheritTerminalSlot, None),
    )
    .expect("继承字体");
    f.save(profile);
    let mut table = Preparations::enabled_for_test();
    for generation in 1..=3 {
        let request = prepare(1, generation, generation as u32 * 10);
        let (identity, lease, _) = table.begin(request.clone(), Instant::now()).expect("预约");
        let loaded = f.load(&request, identity.clone(), lease).await;
        let (reply, bytes) = table.complete(loaded, Instant::now()).expect("就绪");
        let length = bytes.len();
        assert!(length > 0);
        let launch = table
            .start(&start(&request, &identity), secret(), Instant::now())
            .expect("运行转移");
        let retained = launch.target.font.as_ref().expect("运行字体").clone();
        let weak = Arc::downgrade(retained.resource.as_ref().expect("实际字体").bytes());
        // Rust 测试未注册 Dart isolate，真实 sender 拒绝交付并销毁 binary。
        bytes.send(reply);
        let running = Usage {
            count: 0,
            bytes: length + METADATA_LIMIT,
            inflight: 0,
        };
        assert_eq!(
            table.budget.usage(),
            running,
            "发送失败不能留下无 owner 的字节额度"
        );
        drop(launch);
        assert_eq!(
            table.budget.usage(),
            running,
            "实际运行字体的迟到 holder 继续计量"
        );
        assert!(weak.upgrade().is_some());
        drop(retained);
        assert!(weak.upgrade().is_none());
        assert_eq!(table.budget.usage(), Usage::default());
    }
    f.close();
}

#[tokio::test]
async fn large_configuration_keeps_small_selected_projection_and_unknown_metadata() {
    let f = Fixture::new();
    let mut selected = crate::settings::default_profile(&f.context.repository).expect("默认配置");
    selected.settings.terminal_type = "vt220".into();
    selected.settings.answerback = "完整应答".into();
    selected.settings.font_family = "selected-family".into();
    crate::profile_fonts::write_binding(
        &mut selected.settings,
        &crate::profile_fonts::binding(ProfileFontMode::ExplicitFamily, None),
    )
    .expect("显式字体");
    let unknown = serde_json::json!({"version": 97, "payload": "未知数据".repeat(40000)});
    selected
        .settings
        .extensions
        .insert("future.selected".into(), unknown.clone());
    let snapshot = f.context.repository.load_configuration().expect("配置");
    let mut profiles = vec![selected.clone()];
    for index in 0..80 {
        let mut profile = selected.clone();
        profile.id = uuid::Uuid::new_v4().into();
        profile.name = format!("其他配置{index}");
        profiles.push(profile);
    }
    let outcome = f
        .context
        .repository
        .commit_configuration(ConfigurationChange {
            expected_revision: snapshot.revision,
            settings: snapshot.settings,
            upsert_profiles: profiles,
            delete_profiles: vec![],
        })
        .expect("大配置 CAS");
    assert!(matches!(
        outcome,
        rshell_m0::rshell_storage::ConfigurationCommitOutcome::Applied { .. }
    ));
    let revision = f
        .context
        .repository
        .load_configuration()
        .expect("快照")
        .revision;
    let mut table = Preparations::enabled_for_test();
    let mut request = prepare(1, 1, 10);
    request.profile_id = selected.id.0.to_string();
    let (identity, lease, _) = table.begin(request.clone(), Instant::now()).expect("预约");
    let loaded = f.load(&request, identity.clone(), lease).await;
    assert!(
        loaded.result.is_ok(),
        "正常 selected 不受整库序列化规模限制：{:?}",
        loaded.result.as_ref().err()
    );
    let (reply, bytes) = table.complete(loaded, Instant::now()).expect("就绪");
    let projection = reply.snapshot.expect("完整投影");
    assert_eq!(projection.profile.terminal_type, "vt220");
    assert_eq!(projection.profile.answerback, "完整应答");
    assert_eq!(projection.font.actual_family, "selected-family");
    assert_eq!(projection.font.configuration_revision, revision);
    assert_eq!(projection.font.profile_id, selected.id.0.to_string());
    assert!(bytes.is_empty());
    let after = f
        .context
        .repository
        .load_configuration()
        .expect("原业务数据");
    assert_eq!(after.profiles.len(), 81);
    assert!(
        after
            .profiles
            .iter()
            .all(|p| p.settings.extensions.get("future.selected") == Some(&unknown))
    );
    f.close();
}

#[tokio::test]
async fn retained_command_and_overridden_raw_font_cannot_bypass_metadata_admission() {
    for case in 0..3 {
        let oversized_family = case == 1;
        let f = Fixture::new();
        let mut profile = crate::settings::default_profile(&f.context.repository).expect("配置");
        if oversized_family {
            profile.settings.font_family = "f".repeat(METADATA_LIMIT + 1);
            crate::profile_fonts::write_binding(
                &mut profile.settings,
                &crate::profile_fonts::binding(ProfileFontMode::ExplicitFamily, None),
            )
            .expect("实际字体绑定");
        }
        f.save(profile.clone());
        let mut connection = ConnectionProfile::new("目录目标", "example.test");
        connection.username = "fixture".into();
        connection.transport = rshell_m0::rshell_core::TransportKind::NativeSsh;
        connection.authentication = rshell_m0::rshell_core::AuthenticationKind::Password;
        connection.terminal_profile_id = Some(profile.id);
        if oversized_family {
            connection.terminal_overrides.font_family = Some("small-family".into());
        } else {
            let command_bytes = if case == 2 {
                // 单份合法，但运行目标、attempt 与 Start 请求的共存数据超过预约。
                METADATA_LIMIT / 3 + 1024
            } else {
                METADATA_LIMIT + 1
            };
            connection.remote_command = Some("c".repeat(command_bytes));
        }
        f.context
            .credentials
            .apply_catalog(
                CatalogMutation::Create(connection.clone()),
                SecretUpdate::Unchanged,
            )
            .expect("保存目录");
        let catalog_before = f.context.repository.load_catalog().expect("已提交目录");
        let configuration_before = f
            .context
            .repository
            .load_configuration()
            .expect("已提交配置");
        let mut request = prepare(1, 1, 10);
        request.connection_id = connection.id.0.to_string();
        request.host.clear();
        request.username.clear();
        request.port = 0;
        let mut table = Preparations::enabled_for_test();
        let (identity, lease, _) = table.begin(request.clone(), Instant::now()).expect("预约");
        let loaded = f.load(&request, identity, lease).await;
        let (reply, bytes) = table.complete(loaded, Instant::now()).expect("准入结果");
        assert_eq!(reply.failure, Failure::ByteBudget);
        assert_eq!(reply.state, State::Failed);
        assert!(reply.snapshot.is_none());
        assert!(bytes.is_empty());
        assert_eq!(table.budget.usage(), Usage::default());
        let saved = f.context.repository.load_catalog().expect("原目录");
        assert_eq!(
            saved.connections.get(&connection.id),
            catalog_before.connections.get(&connection.id)
        );
        let configuration = f.context.repository.load_configuration().expect("原配置");
        assert_eq!(
            configuration.profiles.iter().find(|p| p.id == profile.id),
            configuration_before
                .profiles
                .iter()
                .find(|p| p.id == profile.id)
        );
        f.close();
    }
}

#[tokio::test(start_paused = true)]
async fn timer_reclaims_without_requests_and_late_load_never_revives_cancelled_or_expired_identity()
{
    let f = Fixture::new();
    f.slot("apply");
    for cancelled in [false, true] {
        let mut table = Preparations::enabled_for_test();
        let request = prepare(2, 1, 1);
        let (identity, lease, _) = table.begin(request.clone(), Instant::now()).expect("准备");
        let loaded = f.load(&request, identity.clone(), lease).await;
        let weak = Arc::downgrade(
            loaded
                .result
                .as_ref()
                .expect("真实捕获")
                .font
                .resource
                .as_ref()
                .expect("实际字节")
                .bytes(),
        );
        // 完成屏障之前租约仍在消息里；删 map 不能释放消息真实持有。
        if cancelled {
            assert_eq!(
                table
                    .cancel(&cancel(&request, ""), Instant::now())
                    .expect("无 identity 取消")
                    .failure,
                Failure::Cancelled
            );
        } else {
            tokio::time::sleep_until(table.deadline().expect("主动期限")).await;
            assert_eq!(table.expire(Instant::now())[0].failure, Failure::Expired);
        }
        assert_eq!(table.budget.usage().inflight, 1);
        assert_eq!(table.budget.usage().bytes, RESERVATION);
        assert!(weak.upgrade().is_some(), "后台完成消息仍持有真实字体");
        assert!(table.complete(loaded, Instant::now()).is_none());
        assert!(weak.upgrade().is_none(), "丢弃迟到完成才真正释放资源");
        assert_eq!(table.budget.usage(), Usage::default());
        assert!(
            table
                .start(&start(&request, &identity), secret(), Instant::now())
                .is_err()
        );
    }
    f.close();
}

#[tokio::test]
async fn replacement_completion_and_start_have_one_atomic_winner_in_both_orders() {
    let f = Fixture::new();
    for completed_first in [false, true] {
        let mut table = Preparations::enabled_for_test();
        let first = prepare(1, 1, 1);
        let (id1, lease, _) = table.begin(first.clone(), Instant::now()).expect("第一份");
        let loaded = f.load(&first, id1.clone(), lease).await;
        let late = if completed_first {
            table.complete(loaded, Instant::now());
            None
        } else {
            Some(loaded)
        };
        let second = prepare(1, 2, 1);
        let (id2, lease, replaced) = table.begin(second.clone(), Instant::now()).expect("替换");
        assert_eq!(replaced.expect("明确替换").failure, Failure::Replaced);
        if let Some(late) = late {
            assert!(table.complete(late, Instant::now()).is_none());
        }
        assert_eq!(
            table
                .start(&start(&first, &id1), secret(), Instant::now())
                .err(),
            Some(Failure::StaleGeneration)
        );
        let loaded = f.load(&second, id2.clone(), lease).await;
        table.complete(loaded, Instant::now()).expect("新准备");
        let launch = table
            .start(&start(&second, &id2), secret(), Instant::now())
            .expect("一次消费");
        assert_eq!(
            launch.target.quick_password.expose_secret(),
            "仅测试最终启动秘密"
        );
        assert_eq!(
            table
                .start(&start(&second, &id2), secret(), Instant::now())
                .err(),
            Some(Failure::AlreadyStarted)
        );
        drop(launch);
        assert_eq!(table.budget.usage(), Usage::default());
    }
    f.close();
}

#[tokio::test]
async fn cancelled_loading_task_keeps_its_real_resource_reservation_until_barrier_release() {
    let f = Fixture::new();
    f.slot("apply");
    let mut table = Preparations::enabled_for_test();
    let request = prepare(1, 1, 1);
    let (identity, lease, _) = table
        .begin(request.clone(), Instant::now())
        .expect("加载预约");
    let context = f.context.clone();
    let native = request.native_request();
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, barrier) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let (result, lease) = crate::connect::resolve_prepared(&context, native, lease).await;
        let weak = Arc::downgrade(
            result
                .as_ref()
                .expect("后台捕获")
                .font
                .resource
                .as_ref()
                .expect("资源")
                .bytes(),
        );
        entered.send(weak).expect("实际持有通知");
        barrier.await.expect("加载完成屏障");
        Loaded {
            identity,
            result,
            lease,
        }
    });
    let weak = ready.await.expect("真实加载已持有");
    table
        .cancel(&cancel(&request, ""), Instant::now())
        .expect("加载中取消");
    assert_eq!(table.budget.usage().inflight, 1);
    assert!(weak.upgrade().is_some());
    table.clear();
    assert_eq!(
        table.budget.usage().inflight,
        1,
        "退出清表不能假释放后台资源"
    );
    release.send(()).expect("真实任务释放");
    assert!(
        table
            .complete(task.await.expect("加载任务结束"), Instant::now())
            .is_none()
    );
    assert!(weak.upgrade().is_none());
    assert_eq!(table.budget.usage(), Usage::default());
    f.close();
}

#[tokio::test(start_paused = true)]
async fn completed_then_cancelled_or_expired_cannot_start_and_history_is_bounded() {
    let f = Fixture::new();
    for cancelled in [false, true] {
        let mut table = Preparations::enabled_for_test();
        let request = prepare(1, 1, 1);
        let (identity, lease, _) = table.begin(request.clone(), Instant::now()).expect("准备");
        table
            .complete(
                f.load(&request, identity.clone(), lease).await,
                Instant::now(),
            )
            .expect("先完成");
        if cancelled {
            table
                .cancel(&cancel(&request, &identity), Instant::now())
                .expect("再取消");
        } else {
            tokio::time::sleep_until(table.deadline().expect("准备截止")).await;
            table.expire(Instant::now());
        }
        assert!(
            table
                .start(&start(&request, &identity), secret(), Instant::now())
                .is_err()
        );
        assert_eq!(table.budget.usage(), Usage::default());
    }
    let mut table = Preparations::enabled_for_test();
    for id in 1..=200 {
        let request = prepare(1, id, 1);
        let (_, lease, _) = table
            .begin(request.clone(), Instant::now())
            .expect("有限历史");
        table
            .cancel(&cancel(&request, ""), Instant::now())
            .expect("终结");
        drop(lease);
    }
    assert_eq!(table.history.len(), super::HISTORY_LIMIT);
    f.close();
}

#[tokio::test]
async fn invalid_source_geometry_font_and_not_ready_never_consume_and_deadline_is_from_receipt() {
    let f = Fixture::new();
    let mut table = Preparations::enabled_for_test();
    let request = prepare(1, 1, 1);
    let received = Instant::now();
    let (identity, lease, _) = table.begin(request.clone(), received).expect("准备");
    assert_eq!(
        table
            .start(&start(&request, &identity), secret(), received)
            .err(),
        Some(Failure::NotPrepared)
    );
    assert!(matches!(
        table.begin(request.clone(), received),
        Err(Failure::Duplicate)
    ));
    table
        .complete(
            f.load(&request, identity.clone(), lease).await,
            received + TTL / 2,
        )
        .expect("加载");
    let mut invalid = start(&request, &identity);
    invalid.source.generation += 1;
    assert_eq!(
        table.start(&invalid, secret(), received).err(),
        Some(Failure::StaleGeneration)
    );
    invalid = start(&request, &identity);
    invalid.geometry.dpi = 0;
    assert_eq!(
        table.start(&invalid, secret(), received).err(),
        Some(Failure::InvalidGeometry)
    );
    invalid = start(&request, &identity);
    invalid.font_projection = "任意family不允许".into();
    assert_eq!(
        table.start(&invalid, secret(), received).err(),
        Some(Failure::InvalidFont)
    );
    assert_eq!(
        table
            .start(&start(&request, &identity), secret(), received + TTL)
            .err(),
        Some(Failure::Expired)
    );
    assert_eq!(table.budget.usage(), Usage::default());
    f.close();
}

#[test]
fn reservations_bound_actual_inflight_count_and_bytes_and_shutdown_releases_all() {
    let budget = Budget::default();
    let mut loads = Vec::new();
    for _ in 0..INFLIGHT_LIMIT {
        loads.push(budget.reserve().expect("预约"));
    }
    assert!(matches!(budget.reserve(), Err(Failure::InflightBudget)));
    assert_eq!(budget.usage().bytes, BYTE_LIMIT);
    drop(loads);
    let mut retained = Vec::new();
    for _ in 0..COUNT_LIMIT {
        let mut lease = budget.reserve().expect("无字体准备");
        lease.loaded(0);
        retained.push(lease);
    }
    assert!(matches!(budget.reserve(), Err(Failure::CountBudget)));
    drop(retained);
    let mut actual = Vec::new();
    loop {
        match budget.reserve() {
            Ok(mut lease) => {
                let bytes = vec![0u8; FONT_LIMIT];
                lease.loaded(bytes.len());
                actual.push((bytes, lease));
            }
            Err(error) => {
                assert_eq!(error, Failure::ByteBudget);
                break;
            }
        }
    }
    assert_eq!(
        budget.usage().bytes,
        actual.len() * (FONT_LIMIT * 2 + METADATA_LIMIT)
    );
    drop(actual);
    assert_eq!(budget.usage(), Usage::default());
}

#[test]
fn forged_mobile_and_non_native_preparations_fail_before_any_resource_reservation() {
    let mut table = Preparations::enabled_for_test();
    let mut request = prepare(1, 1, 1);
    assert!(matches!(
        table.begin_on(request.clone(), Instant::now(), false),
        Err(Failure::PlatformUnavailable)
    ));
    request.version = 1;
    assert!(matches!(
        table.begin_on(request.clone(), Instant::now(), true),
        Err(Failure::UnsupportedVersion)
    ));
    request.version = VERSION;
    for target in [
        DesktopTargetKind::SystemOpenSsh,
        DesktopTargetKind::LocalShell,
    ] {
        request.target = target;
        assert!(matches!(
            table.begin_on(request.clone(), Instant::now(), true),
            Err(Failure::TransportUnavailable)
        ));
    }
    assert_eq!(table.budget.usage(), Usage::default());
}
