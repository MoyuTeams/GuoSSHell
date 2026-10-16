//! 桌面配置协议回归：仅使用本地 SQLite，不创建应用上下文或钥匙串。

#![allow(clippy::expect_used)]

use std::path::PathBuf;

use rshell_m0::rshell_core::{CatalogMutation, ConnectionProfile};

use super::*;

const REQUEST_ID: &str = "desktop-settings-test";

fn memory_repository() -> SqliteRepository {
    let repository = SqliteRepository::open_in_memory().expect("打开内存数据库");
    repository.migrate().expect("迁移内存数据库");
    repository
}

struct TempDatabase {
    directory: PathBuf,
    path: PathBuf,
}

impl TempDatabase {
    fn new() -> Self {
        let directory =
            std::env::temp_dir().join(format!("guosh-desktop-settings-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).expect("创建临时数据库目录");
        let path = directory.join("catalog.sqlite3");
        Self { directory, path }
    }

    fn open(&self) -> SqliteRepository {
        let repository = SqliteRepository::open(&self.path).expect("打开文件数据库");
        repository.migrate().expect("迁移文件数据库");
        repository
    }
}

impl Drop for TempDatabase {
    fn drop(&mut self) {
        // 即使断言失败，也尽可能清理本测试唯一的临时目录。
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

fn request(operation: &str, base_revision: u64) -> DesktopSettingsRequest {
    DesktopSettingsRequest {
        protocol_version: PROTOCOL_VERSION,
        request_id: REQUEST_ID.into(),
        base_revision,
        operation: operation.into(),
        target_id: String::new(),
        name: String::new(),
        app: None,
        profile: None,
    }
}

fn query(repository: &SqliteRepository) -> DesktopConfiguration {
    let reply = handle_for(repository, request("query", u64::MAX), true);
    assert_eq!(reply.status, "ok");
    assert_eq!(reply.request_id, REQUEST_ID);
    assert!(reply.field_errors.is_empty());
    let snapshot = reply.snapshot.expect("查询返回快照");
    assert_eq!(reply.revision, Some(snapshot.revision));
    snapshot
}

fn applied(
    repository: &SqliteRepository,
    request: DesktopSettingsRequest,
) -> DesktopSettingsResult {
    applied_font(repository, request, None)
}

fn applied_font(
    repository: &SqliteRepository,
    request: DesktopSettingsRequest,
    selected: Option<&ProfileFontBinding>,
) -> DesktopSettingsResult {
    let previous = request.base_revision;
    let reply = handle_inner(repository, request, true, None, selected);
    assert_eq!(reply.status, "applied", "写入必须确认提交");
    assert_eq!(reply.revision, Some(previous + 1));
    assert_eq!(
        reply.snapshot.as_ref().map(|snapshot| snapshot.revision),
        reply.revision
    );
    assert!(reply.field_errors.is_empty());
    assert!(reply.detail.is_empty());
    reply
}

fn binding(key: &str, action: &str) -> DesktopKeyBinding {
    DesktopKeyBinding {
        key: key.into(),
        shift: true,
        control: true,
        alt: false,
        super_key: false,
        action: action.into(),
    }
}

fn profile<'a>(snapshot: &'a DesktopConfiguration, id: &str) -> &'a DesktopTerminalProfile {
    snapshot
        .profiles
        .iter()
        .find(|profile| profile.id == id)
        .expect("配置存在")
}

fn assert_invalid(
    repository: &SqliteRepository,
    request: DesktopSettingsRequest,
    field_name: &str,
    code: &str,
) {
    let before = repository.load_configuration().expect("原始配置");
    let reply = handle_for(repository, request, true);
    assert_eq!(reply.status, "validation_error", "{field_name}");
    assert!(reply.snapshot.is_none());
    assert!(
        reply
            .field_errors
            .iter()
            .any(|error| error.field == field_name && error.code == code),
        "{field_name}: {code}"
    );
    assert_eq!(repository.load_configuration().expect("配置未变化"), before);
}

fn create(repository: &SqliteRepository) -> String {
    let mut create = request("create_profile", query(repository).revision);
    create.name = "桌面配置".into();
    let reply = applied(repository, create);
    assert!(uuid::Uuid::parse_str(&reply.target_id).is_ok());
    assert_eq!(
        profile(&query(repository), &reply.target_id).name,
        "桌面配置"
    );
    reply.target_id
}

// 对全部 16 个终端字段赋予非默认值；精确比较信号投影与磁盘回读。
fn complete_profile(mut draft: DesktopTerminalProfile) -> DesktopTerminalProfile {
    draft.name = "全字段配置".into();
    draft.terminal_type = "screen-256color".into();
    draft.initial_cols = 91;
    draft.initial_rows = 37;
    draft.scrollback_lines = 9_001;
    draft.font_family = "测试字体".into();
    draft.font_size = 21.5;
    draft.color_scheme = "tokyo_night".into();
    draft.key_bindings = [
        "send:\u{1b}[3~",
        "send:\u{7f}",
        "send:\u{8}",
        "clear_scrollback",
        "new_tab",
        "split_vertical",
    ]
    .iter()
    .enumerate()
    .map(|(index, action)| binding(&format!("F{}", index + 1), action))
    .collect();
    draft.left_alt_as_meta = false;
    draft.right_alt_as_meta = false;
    draft.enable_csi_u = true;
    draft.enable_kitty_keyboard = true;
    draft.mouse_reporting = false;
    draft.scroll_on_output = false;
    draft.scroll_on_keypress = true;
    draft.answerback = "桌面应答".into();
    draft
}

fn save_complete(
    repository: &SqliteRepository,
) -> (String, DesktopTerminalProfile, DesktopAppSettings) {
    let id = create(repository);
    let current = query(repository);
    let draft = complete_profile(profile(&current, &id).clone());
    let mut save = request("save_profile", current.revision);
    save.target_id = id.clone();
    save.profile = Some(draft.clone());
    let reply = applied_font(
        repository,
        save,
        Some(&profile_fonts::binding(
            ProfileFontMode::ExplicitFamily,
            None,
        )),
    );
    assert_eq!(reply.target_id, id);
    assert_eq!(
        profile(reply.snapshot.as_ref().expect("已提交快照"), &id),
        &draft
    );

    let current = query(repository);
    let app = DesktopAppSettings {
        default_terminal_profile: id.clone(),
        color_scheme: "gruvbox_dark".into(),
        key_bindings: vec![binding("F1", "split_vertical"), binding("F10", "new_tab")],
    };
    let mut save = request("save_app", current.revision);
    save.app = Some(app.clone());
    let reply = applied(repository, save);
    assert_eq!(reply.snapshot.as_ref().expect("已提交快照").app, app);
    (id, draft, app)
}

fn assert_complete(
    repository: &SqliteRepository,
    id: &str,
    draft: &DesktopTerminalProfile,
    app: &DesktopAppSettings,
) {
    let current = query(repository);
    assert_eq!(profile(&current, id), draft);
    assert_eq!(&current.app, app);
    let stored = repository.load_configuration().expect("读取 SQLite 快照");
    assert_eq!(
        project_profile(
            stored
                .profiles
                .iter()
                .find(|profile| profile.id.0.to_string() == id)
                .expect("SQLite 配置")
        ),
        *draft
    );
    assert_eq!(project(&stored).app, *app);
    let effective = current
        .effective_profiles
        .iter()
        .find(|effective| effective.id == id)
        .expect("有效绑定");
    assert_eq!(effective.key_bindings.len(), 7);
    assert_eq!(effective.key_bindings[0], draft.key_bindings[0]);
    assert_eq!(effective.key_bindings[6], app.key_bindings[1]);
    assert_eq!(effective.scrollback_lines, draft.scrollback_lines);
    assert!(effective.scrollback_limit_reason.is_empty());
    assert_eq!(
        handle_for(repository, request("query", 0), true)
            .capabilities
            .key_actions,
        draft
            .key_bindings
            .iter()
            .map(|binding| binding.action.clone())
            .collect::<Vec<_>>()
    );
}

#[test]
fn memory_sqlite_roundtrips_all_terminal_and_app_fields_and_merges_bindings() {
    let repository = memory_repository();
    let (id, draft, app) = save_complete(&repository);
    assert_complete(&repository, &id, &draft, &app);
}

#[test]
fn file_sqlite_preserves_all_fields_after_shutdown_and_reopen() {
    let database = TempDatabase::new();
    let repository = database.open();
    let (id, draft, app) = save_complete(&repository);
    assert_complete(&repository, &id, &draft, &app);
    repository.shutdown().expect("关闭数据库 worker");
    drop(repository);
    let reopened = database.open();
    assert_complete(&reopened, &id, &draft, &app);
    reopened.shutdown().expect("再次关闭数据库 worker");
}

#[test]
fn create_copy_rename_switch_default_and_delete_enforce_references() {
    let repository = memory_repository();
    let original_default = query(&repository).app.default_terminal_profile;
    let id = create(&repository);
    let current = query(&repository);
    let mut copy = request("copy_profile", current.revision);
    copy.target_id = id.clone();
    copy.name = "复制配置".into();
    let copied = applied(&repository, copy).target_id;
    assert_ne!(copied, id);
    let snapshot = query(&repository);
    let mut expected_copy = profile(&snapshot, &id).clone();
    expected_copy.id = copied.clone();
    expected_copy.name = "复制配置".into();
    assert_eq!(profile(&snapshot, &copied), &expected_copy);

    let mut rename = request("rename_profile", query(&repository).revision);
    rename.target_id = id.clone();
    rename.name = "重命名配置".into();
    assert_eq!(applied(&repository, rename).target_id, id);
    assert_eq!(profile(&query(&repository), &id).name, "重命名配置");

    let mut delete = request("delete_profile", query(&repository).revision);
    delete.target_id = original_default.clone();
    assert_invalid(&repository, delete, "target_id", "default_profile");

    let current = query(&repository);
    let mut save = request("save_app", current.revision);
    let mut app = current.app;
    app.default_terminal_profile = id.clone();
    save.app = Some(app);
    applied(&repository, save);
    let mut delete = request("delete_profile", query(&repository).revision);
    delete.target_id = original_default.clone();
    applied(&repository, delete);
    assert!(
        !query(&repository)
            .profiles
            .iter()
            .any(|profile| profile.id == original_default)
    );

    let mut connection = ConnectionProfile::new("引用配置", "example.test");
    connection.terminal_profile_id = Some(TerminalProfileId(
        uuid::Uuid::parse_str(&copied).expect("复制 ID"),
    ));
    let connection_id = connection.id;
    repository
        .apply(CatalogMutation::Create(connection))
        .expect("写入连接引用");
    let mut delete = request("delete_profile", query(&repository).revision);
    delete.target_id = copied.clone();
    assert_invalid(&repository, delete, "target_id", "profile_in_use");
    repository
        .apply(CatalogMutation::Delete(connection_id))
        .expect("解除连接引用");
    let mut delete = request("delete_profile", query(&repository).revision);
    delete.target_id = copied.clone();
    applied(&repository, delete);
    assert_eq!(query(&repository).profiles.len(), 1);
    assert_eq!(query(&repository).app.default_terminal_profile, id);
}

#[test]
fn rejects_invalid_actions_duplicate_chords_nonfinite_sizes_ranges_and_ids_without_writing() {
    let repository = memory_repository();
    let id = create(&repository);
    let current = query(&repository);
    let draft = profile(&current, &id).clone();
    let mut bad = request("save_profile", current.revision);
    bad.target_id = id.clone();
    bad.profile = Some(draft.clone());

    for (action, field_name, code) in [
        ("command:secret", "key_bindings.action", "invalid_action"),
        ("send:hello", "key_bindings.action", "invalid_action"),
    ] {
        let mut candidate = bad.clone();
        candidate.profile.as_mut().expect("草稿").key_bindings = vec![binding("F2", action)];
        assert_invalid(&repository, candidate, field_name, code);
    }
    let mut candidate = bad.clone();
    candidate.profile.as_mut().expect("草稿").key_bindings =
        vec![binding("F1", "new_tab"), binding("F1", "split_vertical")];
    assert_invalid(&repository, candidate, "key_bindings", "duplicate_binding");
    for key in ["F0", "F25", "F01", "NotAKey", "\n"] {
        let mut candidate = bad.clone();
        candidate.profile.as_mut().expect("草稿").key_bindings = vec![binding(key, "new_tab")];
        assert_invalid(&repository, candidate, "key_bindings.code", "invalid_chord");
    }
    for size in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut candidate = bad.clone();
        candidate.profile.as_mut().expect("草稿").font_size = size;
        assert_invalid(&repository, candidate, "font_size", "non_finite");
    }
    for size in [5.9, 72.0000001] {
        let mut candidate = bad.clone();
        candidate.profile.as_mut().expect("草稿").font_size = size;
        assert_invalid(&repository, candidate, "font_size", "out_of_range");
    }
    for (field_name, values) in [
        ("initial_cols", [0, 1000, 65536]),
        ("initial_rows", [0, 1000, 65536]),
        ("scrollback_lines", [0, 99, 1_000_001]),
    ] {
        for value in values {
            let mut candidate = bad.clone();
            let profile = candidate.profile.as_mut().expect("草稿");
            match field_name {
                "initial_cols" => profile.initial_cols = value,
                "initial_rows" => profile.initial_rows = value,
                _ => profile.scrollback_lines = value,
            }
            assert_invalid(&repository, candidate, field_name, "out_of_range");
        }
    }
    let mut candidate = bad.clone();
    candidate.profile.as_mut().expect("草稿").version = 2;
    assert_invalid(
        &repository,
        candidate,
        "profile.version",
        "unsupported_version",
    );
    let mut candidate = bad.clone();
    candidate.profile.as_mut().expect("草稿").id = uuid::Uuid::new_v4().to_string();
    assert_invalid(&repository, candidate, "profile.id", "target_mismatch");
    let mut candidate = bad.clone();
    candidate.profile.as_mut().expect("草稿").name = "  ".into();
    assert_invalid(&repository, candidate, "profile.name", "blank");
    let mut candidate = bad.clone();
    candidate.profile.as_mut().expect("草稿").answerback = " ".into();
    assert_invalid(&repository, candidate, "answerback", "blank");
    let mut candidate = bad.clone();
    candidate.profile.as_mut().expect("草稿").color_scheme = "missing".into();
    assert_invalid(
        &repository,
        candidate,
        "color_scheme",
        "unknown_color_scheme",
    );

    let mut save = request("save_app", current.revision);
    let mut app = current.app;
    app.key_bindings = vec![binding("F2", "new_tab"), binding("F2", "new_tab")];
    save.app = Some(app.clone());
    assert_invalid(
        &repository,
        save.clone(),
        "key_bindings",
        "duplicate_binding",
    );
    app.key_bindings = vec![binding("F1", "not_an_action")];
    save.app = Some(app.clone());
    assert_invalid(
        &repository,
        save.clone(),
        "key_bindings.action",
        "invalid_action",
    );
    app.key_bindings.clear();
    app.default_terminal_profile = uuid::Uuid::new_v4().to_string();
    save.app = Some(app.clone());
    assert_invalid(
        &repository,
        save.clone(),
        "default_terminal_profile",
        "unknown_profile",
    );
    app.default_terminal_profile = "invalid".into();
    save.app = Some(app.clone());
    assert_invalid(
        &repository,
        save.clone(),
        "default_terminal_profile",
        "invalid_id",
    );
    app.default_terminal_profile = id.clone();
    app.color_scheme = "missing".into();
    save.app = Some(app);
    assert_invalid(&repository, save, "color_scheme", "unknown_color_scheme");

    for (operation, target, field_name, code) in [
        ("save_profile", "bad-id", "target_id", "invalid_id"),
        ("rename_profile", "missing", "target_id", "invalid_id"),
        ("copy_profile", "", "target_id", "invalid_id"),
        ("delete_profile", "", "target_id", "invalid_id"),
        (
            "save_profile",
            "00000000-0000-0000-0000-000000000099",
            "target_id",
            "unknown_profile",
        ),
        (
            "delete_profile",
            "00000000-0000-0000-0000-000000000099",
            "target_id",
            "unknown_profile",
        ),
        ("other", "", "operation", "unknown_operation"),
    ] {
        let mut candidate = request(operation, current.revision);
        candidate.target_id = target.into();
        candidate.name = "有效名称".into();
        assert_invalid(&repository, candidate, field_name, code);
    }
    let mut missing_profile = request("save_profile", current.revision);
    missing_profile.target_id = id;
    assert_invalid(&repository, missing_profile, "profile", "required");
    assert_invalid(
        &repository,
        request("save_app", current.revision),
        "app",
        "required",
    );
    let mut blank_id = request("query", current.revision);
    blank_id.request_id = " ".into();
    assert_invalid(&repository, blank_id, "request_id", "blank");
    let mut wrong_version = request("create_profile", current.revision);
    wrong_version.protocol_version += 1;
    let reply = handle_for(&repository, wrong_version, true);
    assert_eq!(reply.status, "unsupported_version");
    assert!(reply.snapshot.is_none());
    assert_eq!(query(&repository).revision, current.revision);
}

#[test]
fn stale_revision_and_legacy_repository_writes_cannot_overwrite_newer_configuration() {
    let repository = memory_repository();
    let id = create(&repository);
    let original = repository.load_configuration().expect("初始版本");
    let mut legacy_profile = original
        .profiles
        .iter()
        .find(|profile| profile.id.0.to_string() == id)
        .expect("配置")
        .clone();
    legacy_profile.name = "旧接口写入".into();
    repository
        .save_terminal_profile(legacy_profile)
        .expect("旧接口保存 profile");
    assert_eq!(query(&repository).revision, original.revision + 1);
    let mut stale = request("rename_profile", original.revision);
    stale.target_id = id.clone();
    stale.name = "不应保存".into();
    let reply = handle_for(&repository, stale, true);
    assert_eq!(reply.status, "conflict");
    assert_eq!(reply.revision, Some(original.revision + 1));
    assert!(reply.snapshot.is_none());
    assert_eq!(profile(&query(&repository), &id).name, "旧接口写入");

    let next = repository.load_configuration().expect("最新版本");
    let mut settings = next.settings.clone();
    settings.color_scheme = ColorScheme::Nord;
    repository
        .save_settings(settings)
        .expect("旧接口保存 app 设置");
    assert_eq!(query(&repository).revision, next.revision + 1);
    let mut stale = request("save_app", next.revision);
    stale.app = Some(DesktopAppSettings {
        default_terminal_profile: id.clone(),
        color_scheme: "dracula".into(),
        key_bindings: vec![],
    });
    let reply = handle_for(&repository, stale, true);
    assert_eq!(reply.status, "conflict");
    assert_eq!(reply.revision, Some(next.revision + 1));
    assert_eq!(query(&repository).app.color_scheme, "nord");
    assert_eq!(
        repository
            .load_configuration()
            .expect("拒绝过期写入")
            .revision,
        next.revision + 1
    );
    let mut fresh = request("rename_profile", next.revision + 1);
    fresh.target_id = id.clone();
    fresh.name = "更新成功".into();
    applied(&repository, fresh);
    assert_eq!(profile(&query(&repository), &id).name, "更新成功");
}

#[test]
fn simulated_mobile_rejects_every_operation_before_storage_access() {
    let repository = memory_repository();
    let before = repository.load_configuration().expect("原始配置");
    for operation in [
        "query",
        "save_app",
        "save_profile",
        "create_profile",
        "copy_profile",
        "rename_profile",
        "delete_profile",
        "unknown",
    ] {
        let reply = handle_for(&repository, request(operation, before.revision), false);
        assert_eq!(reply.status, "unsupported_platform", "{operation}");
        assert_eq!(reply.request_id, REQUEST_ID);
        assert!(!reply.capabilities.available);
        assert!(reply.snapshot.is_none());
        assert!(reply.revision.is_none());
        assert!(reply.field_errors.is_empty());
    }
    let mut wrong_version = request("query", before.revision);
    wrong_version.protocol_version += 1;
    assert_eq!(
        handle_for(&repository, wrong_version, false).status,
        "unsupported_platform"
    );
    assert_eq!(
        repository.load_configuration().expect("移动端未写入"),
        before
    );
}

#[test]
fn rejected_action_never_echoes_canary_or_reaches_sqlite_or_wal() {
    let database = TempDatabase::new();
    let repository = database.open();
    let id = create(&repository);
    let before = query(&repository);
    let canary = "not-a-key-action-secret-canary-84f7c263";
    let mut draft = profile(&before, &id).clone();
    draft.key_bindings = vec![binding("F3", canary)];
    let mut save = request("save_profile", before.revision);
    save.target_id = id;
    save.profile = Some(draft);
    let reply = handle_for(&repository, save, true);
    assert_eq!(reply.status, "validation_error");
    assert!(
        reply
            .field_errors
            .iter()
            .any(|error| error.field == "key_bindings.action" && error.code == "invalid_action")
    );
    for value in [
        serde_json::to_string(&reply).expect("错误结果序列化"),
        serde_json::to_string(&handle_for(&repository, request("query", 0), true))
            .expect("查询结果序列化"),
    ] {
        assert!(!value.contains(canary), "协议结果不得回显非法动作内容");
    }
    assert_eq!(query(&repository).revision, before.revision);
    for path in [
        database.path.clone(),
        database.path.with_extension("sqlite3-wal"),
    ] {
        if path.exists() {
            let bytes = std::fs::read(path).expect("检查 SQLite 文件");
            assert!(
                !bytes
                    .windows(canary.len())
                    .any(|part| part == canary.as_bytes()),
                "非法动作不得进入 SQLite 或 WAL"
            );
        }
    }
    repository.shutdown().expect("关闭文件数据库");
}

#[test]
fn closed_repository_returns_storage_error_for_queries_and_writes() {
    let repository = memory_repository();
    let revision = query(&repository).revision;
    repository.shutdown().expect("真实关闭数据库 worker");
    for operation in ["query", "create_profile", "save_app", "delete_profile"] {
        let reply = handle_for(&repository, request(operation, revision), true);
        assert_eq!(reply.status, "storage_error", "{operation}");
        assert!(reply.revision.is_none());
        assert!(reply.snapshot.is_none());
        assert!(reply.field_errors.is_empty());
        assert_eq!(reply.request_id, REQUEST_ID);
    }
}

#[test]
fn commit_failure_does_not_publish_draft_and_file_reopen_allows_retry() {
    let database = TempDatabase::new();
    let repository = database.open();
    let before = repository.load_configuration().expect("读取事务前快照");
    let mut create = request("create_profile", before.revision);
    create.name = "失败后重试".into();
    let (change, target_id) = prepare(&before, &create).expect("提交前校验");
    repository.shutdown().expect("在提交前关闭 worker");
    let reply = commit(
        &repository,
        result(REQUEST_ID.into(), true),
        before.clone(),
        change,
        target_id,
    );
    assert_eq!(reply.status, "storage_error");
    assert!(reply.snapshot.is_none());
    assert!(reply.target_id.is_empty());
    drop(repository);
    let reopened = database.open();
    assert_eq!(
        reopened.load_configuration().expect("失败未持久化草稿"),
        before
    );
    let reply = applied(&reopened, create);
    assert_eq!(
        reply.snapshot.expect("成功才返回快照").profiles.len(),
        before.profiles.len() + 1
    );
}

#[test]
fn separate_file_workers_cannot_commit_the_same_revision_twice() {
    let database = TempDatabase::new();
    let first = database.open();
    let second = database.open();
    let before = first.load_configuration().expect("共享文件快照");
    let mut one = request("create_profile", before.revision);
    one.name = "第一个候选".into();
    let mut two = request("create_profile", before.revision);
    two.name = "第二个候选".into();
    // 两个 worker 都完成读取与校验后才提交，锁定事务内部 CAS 而不是 handler 的提前检查。
    let (one_change, one_id) = prepare(&before, &one).expect("第一份草稿");
    let (two_change, two_id) = prepare(&before, &two).expect("第二份草稿");
    let applied = commit(
        &first,
        result(REQUEST_ID.into(), true),
        before.clone(),
        one_change,
        one_id,
    );
    assert_eq!(applied.status, "applied");
    let rejected = commit(
        &second,
        result(REQUEST_ID.into(), true),
        before.clone(),
        two_change,
        two_id,
    );
    assert_eq!(rejected.status, "conflict");
    assert_eq!(rejected.revision, Some(before.revision + 1));
    assert!(rejected.snapshot.is_none());
    let after = second.load_configuration().expect("事务冲突后的快照");
    assert_eq!(after.revision, before.revision + 1);
    assert_eq!(after.profiles.len(), before.profiles.len() + 1);
    assert!(
        after
            .profiles
            .iter()
            .any(|profile| profile.name == "第一个候选")
    );
    assert!(
        !after
            .profiles
            .iter()
            .any(|profile| profile.name == "第二个候选")
    );
}

fn effective_binding(snapshot: &DesktopConfiguration, id: &str) -> ProfileFontBinding {
    snapshot
        .effective_profiles
        .iter()
        .find(|p| p.id == id)
        .expect("字体投影")
        .font_binding
        .clone()
}

#[test]
fn disk_roundtrip_preserves_absent_known_unknown_and_foreign_payloads_through_nonfont_edits() {
    for payload in [
        None,
        Some(serde_json::json!({"version": 1, "mode": "explicit_family"})),
        Some(serde_json::json!({"version": 71, "future": {"array": [null, true, 3]}})),
        Some(serde_json::json!({"version": 1, "mode": "broken", "keep": "raw"})),
    ] {
        let database = TempDatabase::new();
        let repository = database.open();
        let mut stored = super::super::default_profile(&repository).expect("存量 profile");
        stored.settings.font_family = r#"{"version":1,"mode":"explicit_family"}"#.into();
        let foreign = serde_json::json!({"nested": [1, {"owned": false}]});
        stored
            .settings
            .extensions
            .insert("other.app".into(), foreign);
        if let Some(payload) = payload {
            stored
                .settings
                .extensions
                .insert(profile_fonts::NAMESPACE.into(), payload);
        }
        repository
            .save_terminal_profile(stored.clone())
            .expect("受控存量 fixture");
        super::super::adopt_app_defaults(&repository, false).expect("不迁移存量");
        let current = query(&repository);
        let id = stored.id.0.to_string();
        assert_eq!(
            effective_binding(&current, &id),
            profile_fonts::saved_binding(&stored.settings)
        );
        let mut nonfont = request("save_profile", current.revision);
        let mut draft = profile(&current, &id).clone();
        draft.font_size = 26.0;
        draft.name = "非字体保存".into();
        nonfont.target_id = id.clone();
        nonfont.profile = Some(draft);
        applied(&repository, nonfont);
        let mut copy = request("copy_profile", query(&repository).revision);
        copy.target_id = id.clone();
        copy.name = "复制存量".into();
        let copied_id = applied(&repository, copy).target_id;
        let mut rename = request("rename_profile", query(&repository).revision);
        rename.target_id = copied_id.clone();
        rename.name = "重命名存量".into();
        applied(&repository, rename);
        let current = query(&repository);
        let mut switch = request("save_app", current.revision);
        let mut app = current.app;
        app.default_terminal_profile = copied_id.clone();
        switch.app = Some(app);
        applied(&repository, switch);
        repository.shutdown().expect("关闭");
        drop(repository);
        let reopened = database.open();
        let after = reopened.load_configuration().expect("磁盘重开");
        for id in [&id, &copied_id] {
            let saved = after
                .profiles
                .iter()
                .find(|p| p.id.0.to_string() == *id)
                .expect("保全 profile");
            assert_eq!(saved.settings.font_family, stored.settings.font_family);
            assert_eq!(saved.settings.extensions, stored.settings.extensions);
            assert_eq!(saved.settings.font_size, 26.0);
        }
    }
}

#[test]
fn old_full_profile_protocol_preserves_known_binding_and_rejects_unrepresentable_font_changes() {
    let repository = memory_repository();
    let id = create(&repository);
    let current = query(&repository);
    assert_eq!(
        effective_binding(&current, &id).mode,
        ProfileFontMode::InheritTerminalSlot
    );
    let mut draft = profile(&current, &id).clone();
    draft.name = "旧协议重命名".into();
    let mut save = request("save_profile", current.revision);
    save.target_id = id.clone();
    save.profile = Some(draft.clone());
    applied(&repository, save);
    assert_eq!(
        effective_binding(&query(&repository), &id).mode,
        ProfileFontMode::InheritTerminalSlot
    );
    let current = query(&repository);
    draft.font_family = "另一个 family".into();
    draft.font_size = 23.0;
    let mut save = request("save_profile", current.revision);
    save.target_id = id.clone();
    save.profile = Some(draft);
    assert_invalid(&repository, save, "font_binding", "font_binding_required");
}

#[test]
fn explicit_same_family_and_reset_are_configuration_cas_operations_and_unknown_reset_is_rejected() {
    let repository = memory_repository();
    let id = create(&repository);
    let current = query(&repository);
    let mut save = request("save_profile", current.revision);
    save.target_id = id.clone();
    save.profile = Some(profile(&current, &id).clone());
    let selected = profile_fonts::binding(ProfileFontMode::ExplicitFamily, None);
    applied_font(&repository, save, Some(&selected));
    assert_eq!(effective_binding(&query(&repository), &id), selected);
    let current = query(&repository);
    let mut save = request("save_profile", current.revision);
    save.target_id = id.clone();
    save.profile = Some(profile(&current, &id).clone());
    applied_font(
        &repository,
        save,
        Some(&profile_fonts::binding(
            ProfileFontMode::InheritTerminalSlot,
            None,
        )),
    );
    let mut stored = repository
        .load_configuration()
        .expect("快照")
        .profiles
        .into_iter()
        .find(|p| p.id.0.to_string() == id)
        .expect("profile");
    stored.settings.extensions.insert(
        profile_fonts::NAMESPACE.into(),
        serde_json::json!({"version": 99}),
    );
    repository.save_terminal_profile(stored).expect("未知版本");
    let before = repository.load_configuration().expect("原配置");
    let current = query(&repository);
    let mut reset = request("save_profile", current.revision);
    let mut draft = profile(&current, &id).clone();
    draft.font_size = 31.0;
    reset.target_id = id;
    reset.profile = Some(draft);
    let reply = handle_inner(
        &repository,
        reset,
        true,
        None,
        Some(&profile_fonts::binding(
            ProfileFontMode::InheritTerminalSlot,
            None,
        )),
    );
    assert_eq!(reply.status, "validation_error");
    assert_eq!(reply.field_errors[0].code, "font_metadata_uneditable");
    assert_eq!(
        repository.load_configuration().expect("拒绝整个配置"),
        before
    );
}

#[test]
fn font_selection_conflicts_after_another_profile_or_app_settings_advance_the_global_revision() {
    let repository = memory_repository();
    let id = create(&repository);
    let other_id = create(&repository);
    for app_change in [false, true] {
        let before = repository.load_configuration().expect("配置基准");
        let current = query(&repository);
        let mut selection = request("save_profile", current.revision);
        selection.target_id = id.clone();
        let mut draft = profile(&current, &id).clone();
        draft.font_family = "Menlo".into();
        draft.font_size = 19.0;
        selection.profile = Some(draft);
        let (change, target_id) = prepare_with_binding(
            &before,
            &selection,
            Some(&profile_fonts::binding(
                ProfileFontMode::ExplicitFamily,
                None,
            )),
        )
        .expect("已验证草稿");
        if app_change {
            let mut app = before.settings.clone();
            app.color_scheme = ColorScheme::Nord;
            repository
                .save_settings(app)
                .expect("其他 AppSettings 变更");
        } else {
            let mut other = before
                .profiles
                .iter()
                .find(|p| p.id.0.to_string() == other_id)
                .expect("其他 profile")
                .clone();
            other.name = "其他配置改名".into();
            repository
                .save_terminal_profile(other)
                .expect("其他 profile 变更");
        }
        let after_other_change = repository.load_configuration().expect("其他操作后");
        let reply = commit(
            &repository,
            result(REQUEST_ID.into(), true),
            before,
            change,
            target_id,
        );
        assert_eq!(reply.status, "conflict");
        assert!(reply.snapshot.is_none());
        assert_eq!(
            repository.load_configuration().expect("未发布字体"),
            after_other_change
        );
    }
}

#[test]
fn profile_ttf_signal_publishes_only_cas_reference_and_never_changes_slots() {
    use crate::signals::settings::DesktopProfileTtfRequest;
    let database = TempDatabase::new();
    let repository = database.open();
    let preferences =
        crate::keys::PreferenceFile::open(database.directory.join("preferences.json"));
    let id = create(&repository);
    let current = query(&repository);
    let mut draft = profile(&current, &id).clone();
    draft.font_size = 24.0;
    draft.name = "显式 TTF 配置".into();
    let ttf_request = DesktopProfileTtfRequest {
        protocol_version: PROTOCOL_VERSION,
        request_id: "TTF配置".into(),
        base_revision: current.revision,
        target_id: id.clone(),
        profile: draft,
    };
    let before_prefs = serde_json::to_value(preferences.get()).expect("偏好快照");
    let font = include_bytes!("../../../assets/fonts/MesloLGS-NF-Regular.ttf");
    let reply = handle_ttf(
        &repository,
        ttf_request.clone(),
        font.to_vec(),
        &database.directory,
        None,
    );
    assert_eq!(reply.status, "applied");
    let snapshot = reply.snapshot.expect("提交快照");
    let selected = effective_binding(&snapshot, &id);
    assert_eq!(selected.mode, ProfileFontMode::ExplicitCachedTtf);
    let hash = selected.hash.expect("资源身份");
    let effective = snapshot
        .effective_profiles
        .iter()
        .find(|p| p.id == id)
        .expect("字体投影")
        .font
        .as_ref()
        .expect("运行契约");
    assert_eq!(effective.actual_family, profile_fonts::cached_family(&hash));
    assert_eq!(profile(&snapshot, &id).font_size, 24.0);
    assert_eq!(
        serde_json::to_value(preferences.get()).expect("偏好未改"),
        before_prefs
    );
    let before = repository.load_configuration().expect("提交后");
    assert_eq!(
        handle_ttf(
            &repository,
            ttf_request,
            font.to_vec(),
            &database.directory,
            None
        )
        .status,
        "conflict"
    );
    assert_eq!(
        repository.load_configuration().expect("过期请求拒绝"),
        before
    );
    let path = database.directory.join("fonts").join(format!("{hash}.ttf"));
    std::fs::remove_file(&path).expect("缓存丢失");
    let current = query(&repository);
    let mut copy = request("copy_profile", current.revision);
    copy.target_id = id.clone();
    copy.name = "复制缺失缓存的绑定".into();
    applied(&repository, copy);
    let mut rename = request("save_profile", query(&repository).revision);
    let mut draft = profile(&query(&repository), &id).clone();
    draft.name = "缺失缓存仍可改名".into();
    rename.target_id = id.clone();
    rename.profile = Some(draft);
    applied(&repository, rename);
    let reply = handle_with_resources(&repository, request("query", 0), &database.directory, None);
    let snapshot = reply.snapshot.expect("缺失仍可读");
    let font = snapshot
        .effective_profiles
        .iter()
        .find(|p| p.id == id)
        .expect("投影")
        .font
        .as_ref()
        .expect("回退");
    assert_eq!(
        font.fallback_reason,
        Some(crate::signals::settings::ProfileFontFailure::Missing)
    );
    assert_eq!(font.saved.hash.as_deref(), Some(hash.as_str()));
    assert_eq!(font.actual_family, super::super::DEFAULT_TERMINAL_FONT);
}

#[test]
fn published_blob_survives_a_configuration_cas_failure_without_publishing_selection() {
    let database = TempDatabase::new();
    let repository = database.open();
    let id = create(&repository);
    let before = repository.load_configuration().expect("基准");
    let resource = crate::font_resources::verify_ttf(
        include_bytes!("../../../assets/fonts/MesloLGS-NF-Regular.ttf").to_vec(),
    )
    .expect("校验资源");
    let current = query(&repository);
    let mut draft = profile(&current, &id).clone();
    draft.font_family = resource.family();
    let mut selection = request("save_profile", before.revision);
    selection.target_id = id;
    selection.profile = Some(draft);
    let (change, target_id) = prepare_with_binding(
        &before,
        &selection,
        Some(&profile_fonts::binding(
            ProfileFontMode::ExplicitCachedTtf,
            Some(resource.hash().into()),
        )),
    )
    .expect("CAS草稿");
    crate::font_resources::publish(&database.directory, &resource).expect("资源先就绪");
    let mut settings = before.settings.clone();
    settings.color_scheme = ColorScheme::Nord;
    repository.save_settings(settings).expect("并发应用更新");
    let after_other_change = repository.load_configuration().expect("并发已保存");
    let reply = commit(
        &repository,
        result(REQUEST_ID.into(), true),
        before,
        change,
        target_id,
    );
    assert_eq!(reply.status, "conflict");
    assert!(reply.snapshot.is_none());
    assert_eq!(
        repository.load_configuration().expect("未发布选择"),
        after_other_change
    );
    assert_eq!(
        crate::font_resources::load_by_hash(&database.directory, resource.hash())
            .expect("不删可能共享的 blob")
            .bytes()
            .as_ref(),
        resource.bytes().as_ref()
    );
}

#[test]
fn profile_ttf_validation_and_publication_failures_do_not_commit_any_profile_fields() {
    let database = TempDatabase::new();
    let repository = database.open();
    let id = create(&repository);
    let current = query(&repository);
    let mut draft = profile(&current, &id).clone();
    draft.name = "不应提交的草稿".into();
    draft.font_size = 24.0;
    let mut selection = DesktopProfileTtfRequest {
        protocol_version: PROTOCOL_VERSION,
        request_id: "TTF故障".into(),
        base_revision: current.revision,
        target_id: id,
        profile: draft,
    };
    let before = repository.load_configuration().expect("配置原值");
    let font = include_bytes!("../../../assets/fonts/MesloLGS-NF-Regular.ttf");
    let reply = handle_ttf(
        &repository,
        selection.clone(),
        vec![1, 2, 3],
        &database.directory,
        None,
    );
    assert_eq!(reply.status, "validation_error");
    assert_eq!(reply.field_errors[0].code, "corrupt");
    assert_eq!(
        repository.load_configuration().expect("校验失败不提交"),
        before
    );
    selection.profile.name = " ".into();
    let reply = handle_ttf(
        &repository,
        selection.clone(),
        font.to_vec(),
        &database.directory,
        None,
    );
    assert_eq!(reply.status, "validation_error");
    assert_eq!(reply.field_errors[0].field, "profile.name");
    assert!(
        !database.directory.join("fonts").exists(),
        "无效配置不发布 blob"
    );
    selection.profile.name = "发布失败".into();
    std::fs::write(database.directory.join("fonts"), b"not a directory")
        .expect("模拟资源目录不可用");
    let reply = handle_ttf(
        &repository,
        selection,
        font.to_vec(),
        &database.directory,
        None,
    );
    assert_eq!(reply.status, "validation_error");
    assert_eq!(reply.field_errors[0].code, "unavailable");
    assert_eq!(
        repository.load_configuration().expect("发布失败不提交"),
        before
    );
}
