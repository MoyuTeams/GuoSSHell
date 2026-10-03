//! 阶段二契约证据：只使用 SQLite、偏好和真实 TTF，不代替阶段四窗格验收。
#![allow(clippy::expect_used)]

use std::path::PathBuf;

use rshell_m0::rshell_core::{TerminalProfile, TerminalSettingsV1};
use rshell_m0::rshell_storage::{
    ConfigurationChange, ConfigurationCommitOutcome, SqliteRepository,
};

use super::*;
use crate::font_resources::{publish, verify_ttf};
use crate::keys::PreferenceFile;
use crate::signals::settings::FontFileRequest;

const REGULAR: &[u8] = include_bytes!("../../../assets/fonts/MesloLGS-NF-Regular.ttf");
const BOLD: &[u8] = include_bytes!("../../../assets/fonts/MesloLGS-NF-Bold.ttf");

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("guosh-profile-font-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).expect("独立临时目录");
        Self(root)
    }

    fn repository(&self) -> SqliteRepository {
        let repository = SqliteRepository::open(self.0.join("catalog.sqlite3")).expect("文件配置");
        repository.migrate().expect("迁移");
        repository
    }

    fn preferences(&self) -> PreferenceFile {
        PreferenceFile::open(self.0.join("preferences.json"))
    }

    fn resolve(
        &self,
        repository: &SqliteRepository,
        prefs: &PreferenceFile,
    ) -> ResolvedProfileFont {
        let snapshot = repository.load_configuration().expect("整个配置快照");
        let captured = prefs.get().terminal_font_file;
        resolve_profile_font(
            &snapshot,
            snapshot.settings.default_terminal_profile,
            captured,
            &self.0,
        )
        .expect("默认配置存在")
    }

    fn slot(&self, prefs: &PreferenceFile, slot: &str, action: &str, bytes: &[u8]) {
        let (state, _) = crate::fonts::handle(
            &self.0,
            prefs,
            FontFileRequest {
                request_id: 1,
                slot: slot.into(),
                action: action.into(),
                file_name: "chosen.ttf".into(),
            },
            bytes.to_vec(),
        );
        assert!(state.error.is_empty(), "槽操作成功");
        assert!(state.applied);
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn default_profile(repository: &SqliteRepository) -> TerminalProfile {
    crate::settings::default_profile(repository).expect("默认 profile")
}

fn save(repository: &SqliteRepository, profile: TerminalProfile) {
    let snapshot = repository.load_configuration().expect("CAS 基准");
    assert!(matches!(
        repository
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

#[test]
fn strict_metadata_never_confuses_unknown_or_magic_raw_family_with_legacy() {
    let mut settings = TerminalSettingsV1::default();
    for family in [
        "未知历史自定义字体",
        "GuoshFont_012345",
        r#"{"version":1,"mode":"explicit_family"}"#,
    ] {
        settings.font_family = family.into();
        assert_eq!(
            saved_binding(&settings).mode,
            ProfileFontMode::LegacyUndetermined
        );
    }
    for payload in [
        json!({"version": 2, "mode": "explicit_family", "future": [1, 2]}),
        json!(null),
        json!("legacy"),
        json!({"version": 1, "mode": "unknown"}),
        json!({"version": 1, "mode": "inherit_terminal_slot", "extra": true}),
        json!({"version": 1, "mode": "explicit_cached_ttf", "hash": "../escape"}),
        json!({"version": 1, "mode": "explicit_cached_ttf", "hash": "a".repeat(64)}),
    ] {
        settings
            .extensions
            .insert(NAMESPACE.into(), payload.clone());
        let before = settings.clone();
        assert_eq!(saved_binding(&settings).mode, ProfileFontMode::Uneditable);
        assert!(
            set_binding(
                &mut settings,
                "Menlo",
                &binding(ProfileFontMode::ExplicitFamily, None)
            )
            .is_err()
        );
        assert!(
            write_binding(
                &mut settings,
                &binding(ProfileFontMode::InheritTerminalSlot, None)
            )
            .is_err()
        );
        assert_eq!(settings, before);
    }
}

#[test]
fn known_bindings_have_one_family_value_and_preserve_unowned_namespaces() {
    let mut settings = TerminalSettingsV1::default();
    let foreign = json!({"nested": [null, 1, {"preserve": true}]});
    settings
        .extensions
        .insert("other.app".into(), foreign.clone());
    for mode in [
        ProfileFontMode::InheritTerminalSlot,
        ProfileFontMode::ExplicitFamily,
    ] {
        set_binding(&mut settings, "Menlo", &binding(mode, None)).expect("明确字体模式");
        assert_eq!(saved_binding(&settings), binding(mode, None));
        assert!(!settings.extensions[NAMESPACE].to_string().contains("Menlo"));
        assert_eq!(settings.extensions["other.app"], foreign);
        assert_eq!(
            settings.resolve(&Default::default()).extensions,
            settings.extensions
        );
    }
    let hash = "a".repeat(64);
    let cached = binding(ProfileFontMode::ExplicitCachedTtf, Some(hash.clone()));
    let before = settings.clone();
    assert!(set_binding(&mut settings, "Menlo", &cached).is_err());
    assert_eq!(settings, before);
    set_binding(&mut settings, &cached_family(&hash), &cached).expect("缓存绑定");
    set_binding(
        &mut settings,
        "Menlo",
        &binding(ProfileFontMode::ExplicitFamily, None),
    )
    .expect("切出缓存模式");
    assert_eq!(
        saved_binding(&settings).mode,
        ProfileFontMode::ExplicitFamily
    );
}

#[test]
fn legacy_unknown_raw_families_survive_reopen_and_follow_terminal_slot_without_writes() {
    for family in [
        "自定义旧字体",
        "GuoshFont_012345",
        r#"{"version":1,"mode":"explicit_family"}"#,
    ] {
        let fixture = Fixture::new();
        let repository = fixture.repository();
        let prefs = fixture.preferences();
        let mut profile = default_profile(&repository);
        profile.settings.font_family = family.into();
        save(&repository, profile.clone());
        fixture.slot(&prefs, "terminal", "apply", REGULAR);
        let before = repository.load_configuration().expect("启动前配置");
        let first = fixture.resolve(&repository, &prefs);
        assert_eq!(
            first.projection.runtime_source,
            ProfileFontRuntimeSource::LegacyTerminalSlot
        );
        assert_eq!(first.projection.raw_family, family);
        assert_eq!(
            first.resource.as_ref().expect("旧槽资源").bytes().as_ref(),
            REGULAR
        );
        fixture.slot(&prefs, "terminal", "apply", BOLD);
        let second = fixture.resolve(&repository, &prefs);
        assert_ne!(
            first.projection.actual_family,
            second.projection.actual_family
        );
        assert_eq!(
            first.resource.expect("旧捕获仍有效").bytes().as_ref(),
            REGULAR
        );
        assert_eq!(
            second.resource.expect("新身份和字节配对").bytes().as_ref(),
            BOLD
        );
        assert_eq!(repository.load_configuration().expect("不回写配置"), before);
        repository.shutdown().expect("关闭");
        drop(repository);
        let reopened = fixture.repository();
        crate::settings::adopt_app_defaults(&reopened, false).expect("既存启动");
        assert_eq!(default_profile(&reopened), profile);
        assert!(!profile.settings.extensions.contains_key(NAMESPACE));
    }
}

#[test]
fn cached_binding_reopens_after_both_slots_reset_and_snapshot_outlives_profile_deletion() {
    let fixture = Fixture::new();
    let repository = fixture.repository();
    let prefs = fixture.preferences();
    let resource = verify_ttf(REGULAR.to_vec()).expect("真实 TTF");
    publish(&fixture.0, &resource).expect("先发布 blob");
    fixture.slot(&prefs, "interface", "apply", BOLD);
    fixture.slot(&prefs, "terminal", "apply", BOLD);
    let mut profile = default_profile(&repository);
    set_binding(
        &mut profile.settings,
        &resource.family(),
        &binding(
            ProfileFontMode::ExplicitCachedTtf,
            Some(resource.hash().into()),
        ),
    )
    .expect("明确引用");
    save(&repository, profile.clone());
    let snapshot = fixture.resolve(&repository, &prefs);
    assert!(snapshot.captured_terminal_slot.is_none());
    for slot in ["interface", "terminal"] {
        fixture.slot(&prefs, slot, "reset", &[]);
    }
    repository.shutdown().expect("退出");
    drop(repository);
    let reopened = fixture.repository();
    let resolved = fixture.resolve(&reopened, &fixture.preferences());
    assert_eq!(resolved.projection.actual_family, resource.family());
    assert_eq!(
        resolved
            .resource
            .expect("重开按 hash 加载")
            .bytes()
            .as_ref(),
        REGULAR
    );
    let mut replacement = TerminalProfile::p0_default();
    replacement.id = TerminalProfileId(uuid::Uuid::new_v4());
    replacement.name = "新的默认配置".into();
    let before = reopened.load_configuration().expect("删除前");
    assert!(
        reopened
            .load_catalog()
            .expect("引用证据")
            .connections
            .values()
            .all(|connection| connection.terminal_profile_id != Some(profile.id))
    );
    rshell_m0::rshell_core::validate_terminal_profile(&replacement).expect("替代 profile 合法");
    let mut settings = before.settings;
    settings.default_terminal_profile = replacement.id;
    // 上游保护当前默认引用；按桌面操作顺序先切默认，原 profile 才成为可删除对象。
    assert!(matches!(
        reopened
            .commit_configuration(ConfigurationChange {
                expected_revision: before.revision,
                settings,
                upsert_profiles: vec![replacement],
                delete_profiles: vec![],
            })
            .expect("先切默认"),
        ConfigurationCommitOutcome::Applied { .. }
    ));
    let switched = reopened.load_configuration().expect("已解除默认引用");
    assert!(matches!(
        reopened
            .commit_configuration(ConfigurationChange {
                expected_revision: switched.revision,
                settings: switched.settings,
                upsert_profiles: vec![],
                delete_profiles: vec![profile.id],
            })
            .expect("删除可删除的原 profile"),
        ConfigurationCommitOutcome::Applied { .. }
    ));
    assert_eq!(
        snapshot
            .resource
            .expect("不可变句柄独立于配置和槽")
            .bytes()
            .as_ref(),
        REGULAR
    );
}

#[test]
fn missing_and_corrupt_cache_fall_back_visibly_without_configuration_changes_and_retry() {
    let fixture = Fixture::new();
    let repository = fixture.repository();
    let prefs = fixture.preferences();
    let resource = verify_ttf(REGULAR.to_vec()).expect("真实字体");
    publish(&fixture.0, &resource).expect("资源");
    let mut profile = default_profile(&repository);
    set_binding(
        &mut profile.settings,
        &resource.family(),
        &binding(
            ProfileFontMode::ExplicitCachedTtf,
            Some(resource.hash().into()),
        ),
    )
    .expect("缓存绑定");
    save(&repository, profile);
    let before = repository.load_configuration().expect("原配置");
    let path = fixture
        .0
        .join("fonts")
        .join(format!("{}.ttf", resource.hash()));
    for failure in [ProfileFontFailure::Missing, ProfileFontFailure::Corrupt] {
        if failure == ProfileFontFailure::Missing {
            std::fs::remove_file(&path).expect("模拟丢失");
        } else {
            // 另一个真实 TTF，结构合法但摘要不同，也必须拒绝混配。
            std::fs::write(&path, BOLD).expect("模拟内容身份损坏");
        }
        let resolved = fixture.resolve(&repository, &prefs);
        assert_eq!(resolved.projection.fallback_reason, Some(failure));
        assert_eq!(
            resolved.projection.actual_family,
            crate::settings::DEFAULT_TERMINAL_FONT
        );
        assert_eq!(
            resolved.projection.saved.hash.as_deref(),
            Some(resource.hash())
        );
        let (state, bytes) = load_resource(
            &fixture.0,
            ProfileFontResourceRequest {
                request_id: "故障重试".into(),
                resource: resource_ref(resource.hash()),
            },
        );
        assert_eq!(state.failure, Some(failure));
        assert!(bytes.is_empty());
        assert_eq!(
            repository.load_configuration().expect("故障不改配置"),
            before
        );
        std::fs::write(&path, REGULAR).expect("恢复缓存");
        let (state, bytes) = load_resource(
            &fixture.0,
            ProfileFontResourceRequest {
                request_id: "故障重试".into(),
                resource: resource_ref(resource.hash()),
            },
        );
        assert_eq!(state.failure, None);
        assert_eq!(state.family, resource.family());
        assert_eq!(bytes, REGULAR);
        assert!(
            fixture
                .resolve(&repository, &prefs)
                .projection
                .fallback_reason
                .is_none()
        );
        assert_eq!(
            repository.load_configuration().expect("重试不改配置"),
            before
        );
    }
}

#[test]
fn inherited_snapshot_uses_captured_identity_and_explicit_family_ignores_slot() {
    let fixture = Fixture::new();
    let repository = fixture.repository();
    let prefs = fixture.preferences();
    fixture.slot(&prefs, "terminal", "apply", REGULAR);
    let mut profile = default_profile(&repository);
    set_binding(
        &mut profile.settings,
        "Menlo",
        &binding(ProfileFontMode::InheritTerminalSlot, None),
    )
    .expect("继承");
    save(&repository, profile.clone());
    let captured = fixture.resolve(&repository, &prefs);
    assert_eq!(
        captured.projection.runtime_source,
        ProfileFontRuntimeSource::ProfileBinding
    );
    assert!(captured.captured_terminal_slot.is_some());
    fixture.slot(&prefs, "terminal", "apply", BOLD);
    let (state, bytes) = load_resource(
        &fixture.0,
        ProfileFontResourceRequest {
            request_id: "捕获身份".into(),
            resource: captured.projection.resource.expect("捕获引用"),
        },
    );
    assert_eq!(state.failure, None);
    assert_eq!(bytes, REGULAR);
    assert_eq!(
        captured
            .resource
            .expect("继承快照仍持有旧字节")
            .bytes()
            .as_ref(),
        REGULAR
    );
    set_binding(
        &mut profile.settings,
        "Menlo",
        &binding(ProfileFontMode::ExplicitFamily, None),
    )
    .expect("显式 family");
    save(&repository, profile);
    let explicit = fixture.resolve(&repository, &prefs);
    assert_eq!(explicit.projection.actual_family, "Menlo");
    assert!(explicit.captured_terminal_slot.is_none());
    assert!(explicit.resource.is_none());
}

#[test]
fn independent_resource_loading_rejects_paths_bad_versions_and_family_mismatch() {
    let fixture = Fixture::new();
    for mut reference in [
        resource_ref("../escape"),
        resource_ref(&"A".repeat(64)),
        resource_ref(&"a".repeat(64)),
    ] {
        if valid_hash(&reference.hash) {
            reference.family = "same-family-wrong-identity".into();
        }
        let (state, bytes) = load_resource(
            &fixture.0,
            ProfileFontResourceRequest {
                request_id: "严格引用".into(),
                resource: reference,
            },
        );
        assert_eq!(
            state.failure,
            Some(ProfileFontFailure::InvalidResourceReference)
        );
        assert!(state.resource.is_none());
        assert!(bytes.is_empty());
    }
    let mut reference = resource_ref(&"a".repeat(64));
    reference.version = 2;
    let (state, _) = load_resource(
        &fixture.0,
        ProfileFontResourceRequest {
            request_id: "版本".into(),
            resource: reference,
        },
    );
    assert_eq!(
        state.failure,
        Some(ProfileFontFailure::InvalidResourceReference)
    );
}

#[test]
fn concurrent_slot_updates_never_mix_captured_hash_family_and_immutable_bytes() {
    use std::sync::{Arc, Barrier};
    let fixture = Fixture::new();
    let repository = fixture.repository();
    let preferences = Arc::new(fixture.preferences());
    let regular = verify_ttf(REGULAR.to_vec()).expect("常规字体");
    let bold = verify_ttf(BOLD.to_vec()).expect("粗体字体");
    for resource in [&regular, &bold] {
        publish(&fixture.0, resource).expect("资源就绪");
    }
    let imported = |resource: &FontResource| ImportedFont {
        hash: resource.hash().into(),
        label: resource.label().into(),
    };
    preferences
        .update(|prefs| prefs.terminal_font_file = Some(imported(&regular)))
        .expect("初始槽");
    let configuration = repository.load_configuration().expect("只读 configuration");
    let barrier = Arc::new(Barrier::new(2));
    std::thread::scope(|scope| {
        let updating = preferences.clone();
        let started = barrier.clone();
        let regular_slot = imported(&regular);
        let bold_slot = imported(&bold);
        scope.spawn(move || {
            started.wait();
            for index in 0..40 {
                let selected = if index % 2 == 0 {
                    &bold_slot
                } else {
                    &regular_slot
                };
                updating
                    .update(|prefs| prefs.terminal_font_file = Some(selected.clone()))
                    .expect("并发更新槽");
            }
        });
        barrier.wait();
        for _ in 0..40 {
            let resolved = resolve_profile_font(
                &configuration,
                configuration.settings.default_terminal_profile,
                preferences.get().terminal_font_file,
                &fixture.0,
            )
            .expect("legacy 解析");
            let resource = resolved.resource.expect("一次捕获身份的资源");
            let reference = resolved.projection.resource.expect("捕获引用");
            assert_eq!(reference.hash, resource.hash());
            assert_eq!(reference.family, resource.family());
            assert_eq!(resolved.projection.actual_family, resource.family());
            let expected = if resource.hash() == regular.hash() {
                REGULAR
            } else {
                assert_eq!(resource.hash(), bold.hash());
                BOLD
            };
            assert_eq!(resource.bytes().as_ref(), expected);
        }
    });
    assert_eq!(
        repository.load_configuration().expect("槽更新不反写配置"),
        configuration
    );
}

#[test]
fn unknown_metadata_is_not_a_legacy_runtime_source_even_with_a_valid_terminal_slot() {
    let fixture = Fixture::new();
    let repository = fixture.repository();
    let prefs = fixture.preferences();
    fixture.slot(&prefs, "terminal", "apply", REGULAR);
    let mut profile = default_profile(&repository);
    profile
        .settings
        .extensions
        .insert(NAMESPACE.into(), json!({"version": 9, "future": true}));
    save(&repository, profile);
    let before = repository.load_configuration().expect("已保存未知载荷");
    let resolved = fixture.resolve(&repository, &prefs);
    assert_eq!(resolved.projection.saved.mode, ProfileFontMode::Uneditable);
    assert_eq!(
        resolved.projection.saved.uneditable_reason,
        Some(ProfileFontMetadataError::UnsupportedVersion)
    );
    assert_eq!(
        resolved.projection.runtime_source,
        ProfileFontRuntimeSource::Uneditable
    );
    assert!(resolved.captured_terminal_slot.is_none());
    assert!(resolved.resource.is_none());
    assert_eq!(
        resolved.projection.fallback_reason,
        Some(ProfileFontFailure::UneditableMetadata)
    );
    assert_eq!(
        resolved.projection.actual_family,
        crate::settings::DEFAULT_TERMINAL_FONT
    );
    assert_eq!(
        repository.load_configuration().expect("不降级未知载荷"),
        before
    );
}

#[test]
fn forged_mobile_resource_request_is_rejected_even_with_valid_cache_without_changing_any_state() {
    let fixture = Fixture::new();
    let repository = fixture.repository();
    let preferences = fixture.preferences();
    fixture.slot(&preferences, "interface", "apply", BOLD);
    fixture.slot(&preferences, "terminal", "apply", REGULAR);
    let regular = verify_ttf(REGULAR.to_vec()).expect("合法常规 TTF");
    let bold = verify_ttf(BOLD.to_vec()).expect("合法粗体 TTF");
    let mut profile = default_profile(&repository);
    set_binding(
        &mut profile.settings,
        &regular.family(),
        &binding(
            ProfileFontMode::ExplicitCachedTtf,
            Some(regular.hash().into()),
        ),
    )
    .expect("明确缓存绑定");
    save(&repository, profile);
    let before_configuration = repository.load_configuration().expect("配置原值");
    let before_preferences = serde_json::to_value(preferences.get()).expect("双槽原值");
    let cache_paths = [regular.hash(), bold.hash()]
        .map(|hash| fixture.0.join("fonts").join(format!("{hash}.ttf")));
    let before_cache = cache_paths
        .each_ref()
        .map(|path| std::fs::read(path).expect("缓存原值"));
    let request = ProfileFontResourceRequest {
        request_id: "伪造移动资源请求".into(),
        resource: resource_ref(regular.hash()),
    };
    let (mobile, bytes) = load_resource_for(&fixture.0, request.clone(), false);
    assert_eq!(mobile.request_id, request.request_id);
    assert_eq!(mobile.failure, Some(ProfileFontFailure::Unavailable));
    assert!(mobile.resource.is_none());
    assert!(mobile.family.is_empty());
    assert!(bytes.is_empty(), "移动请求不能获得已有缓存字节");

    let (desktop, bytes) = load_resource_for(&fixture.0, request.clone(), true);
    assert_eq!(desktop.request_id, request.request_id);
    assert_eq!(desktop.failure, None);
    assert_eq!(desktop.resource.as_ref(), Some(&request.resource));
    assert_eq!(desktop.family, regular.family());
    assert_eq!(bytes, REGULAR);

    let (production, bytes) = load_resource(&fixture.0, request.clone());
    if crate::settings::desktop_available() {
        assert_eq!(production.failure, None);
        assert_eq!(production.resource.as_ref(), Some(&request.resource));
        assert_eq!(production.family, regular.family());
        assert_eq!(bytes, REGULAR);
    } else {
        assert_eq!(production.failure, Some(ProfileFontFailure::Unavailable));
        assert!(production.resource.is_none());
        assert!(production.family.is_empty());
        assert!(bytes.is_empty());
    }
    assert_eq!(
        repository.load_configuration().expect("配置没有变更"),
        before_configuration
    );
    assert_eq!(
        serde_json::to_value(preferences.get()).expect("双槽没有变更"),
        before_preferences
    );
    assert_eq!(
        cache_paths
            .each_ref()
            .map(|path| std::fs::read(path).expect("缓存没有变更")),
        before_cache
    );
}
