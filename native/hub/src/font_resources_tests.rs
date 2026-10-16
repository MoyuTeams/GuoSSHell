#![allow(clippy::expect_used)]

use std::path::PathBuf;
use std::sync::{Arc, Barrier};

use rshell_m0::rshell_core::TerminalProfileId;
use rshell_m0::rshell_storage::{
    ConfigurationChange, ConfigurationCommitOutcome, SqliteRepository,
};
use sha2::{Digest, Sha256};

use super::{FontResource, FontResourceError, load_by_hash, publish, verify_ttf};
use crate::fonts::{self, MAX_FONT_BYTES};
use crate::keys::PreferenceFile;
use crate::signals::settings::FontFileRequest;

const FONT: &[u8] = include_bytes!("../../../assets/fonts/MesloLGS-NF-Regular.ttf");
const OTHER_FONT: &[u8] = include_bytes!("../../../assets/fonts/MesloLGS-NF-Bold.ttf");

fn font_without_table(tag: &[u8; 4]) -> Vec<u8> {
    let mut bytes = FONT.to_vec();
    let count = u16::from_be_bytes(bytes[4..6].try_into().expect("TTF 表数量"));
    let offset = (0..usize::from(count))
        .map(|index| 12 + index * 16)
        .find(|offset| &bytes[*offset..*offset + 4] == tag)
        .expect("真实 TTF 中的目标表");
    bytes[offset..offset + 4].copy_from_slice(b"test");
    bytes
}

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("guosh-font-resources-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).expect("资源测试目录");
        Self { root }
    }

    fn preferences(&self) -> PreferenceFile {
        PreferenceFile::open(self.root.join("preferences.json"))
    }

    fn repository(&self) -> SqliteRepository {
        let repository =
            SqliteRepository::open(self.root.join("catalog.sqlite3")).expect("打开资源引用数据库");
        repository.migrate().expect("迁移资源引用数据库");
        repository
    }

    fn path(&self, resource: &FontResource) -> PathBuf {
        fonts::font_path(&self.root, resource.hash()).expect("资源缓存路径")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.root) {
            if std::thread::panicking() {
                eprintln!("资源测试目录清理失败：{error}");
            } else {
                panic!("资源测试目录清理失败：{error}");
            }
        }
    }
}

fn request(slot: &str, action: &str) -> FontFileRequest {
    FontFileRequest {
        request_id: 1,
        slot: slot.to_owned(),
        action: action.to_owned(),
        file_name: "chosen.ttf".to_owned(),
    }
}

fn assert_resource(resource: &FontResource, bytes: &[u8]) {
    let hash: String = Sha256::digest(bytes)
        .iter()
        .map(|value| format!("{value:02x}"))
        .collect();
    assert_eq!(resource.hash(), hash);
    assert_eq!(resource.family(), format!("GuoshFont_{hash}"));
    assert!(!resource.label().is_empty());
    assert_eq!(resource.bytes().as_ref(), bytes);
}

fn assert_error(result: Result<FontResource, FontResourceError>, expected: FontResourceError) {
    assert_eq!(result.expect_err("必须拒绝不可用资源"), expected);
}

#[test]
fn real_ttf_validation_is_typed_deterministic_and_does_not_persist() {
    let fixture = Fixture::new();
    let regular = verify_ttf(FONT.to_vec()).expect("校验常规 TTF");
    let bold = verify_ttf(OTHER_FONT.to_vec()).expect("校验粗体 TTF");
    assert_resource(&regular, FONT);
    assert_resource(&bold, OTHER_FONT);
    assert_ne!(regular.hash(), bold.hash());
    let repeated = verify_ttf(FONT.to_vec()).expect("再次校验 TTF");
    assert_eq!(regular.label(), repeated.label());
    assert_eq!(regular.family(), repeated.family());
    assert!(!fixture.root.join("fonts").exists());
    assert!(!fixture.root.join("preferences.json").exists());
}

#[test]
fn true_type_structure_and_the_existing_32_mib_boundary_are_enforced() {
    for bytes in [Vec::new(), b"OTTO".to_vec(), vec![0, 1, 0, 0]] {
        assert_error(verify_ttf(bytes), FontResourceError::Corrupt);
    }
    for tag in [b"glyf", b"cmap"] {
        assert_error(
            verify_ttf(font_without_table(tag)),
            FontResourceError::Corrupt,
        );
    }
    let mut bytes = FONT.to_vec();
    bytes.resize(MAX_FONT_BYTES, 0);
    let resource = verify_ttf(bytes).expect("恰好 32 MiB 的真实 TTF");
    assert_eq!(resource.bytes().len(), MAX_FONT_BYTES);
    let mut oversized = FONT.to_vec();
    oversized.resize(MAX_FONT_BYTES + 1, 0);
    assert_error(verify_ttf(oversized), FontResourceError::Corrupt);
}

#[test]
fn unnamed_ttf_uses_a_deterministic_label_without_changing_legacy_filename_labels() {
    let fixture = Fixture::new();
    let bytes = font_without_table(b"name");
    assert_eq!(
        fonts::inspect(&bytes, "chosen.ttf")
            .expect("旧导入标签")
            .label,
        "chosen.ttf"
    );
    let resource = verify_ttf(bytes.clone()).expect("没有名称表的真实 TTF");
    assert_eq!(resource.label(), "cached.ttf");
    publish(&fixture.root, &resource).expect("发布无名称资源");
    let loaded = load_by_hash(&fixture.root, resource.hash()).expect("重载无名称资源");
    assert_eq!(loaded.label(), resource.label());
    assert_resource(&loaded, &bytes);
}

#[test]
fn standalone_publication_reuses_the_existing_cache_without_creating_a_slot() {
    let fixture = Fixture::new();
    let resource = verify_ttf(FONT.to_vec()).expect("校验直接上传的 TTF");
    publish(&fixture.root, &resource).expect("独立发布资源");
    assert_eq!(
        std::fs::read(fixture.path(&resource)).expect("缓存内容"),
        FONT
    );
    let loaded = load_by_hash(&fixture.root, resource.hash()).expect("独立按 hash 加载");
    assert_resource(&loaded, FONT);
    assert_eq!(loaded.label(), resource.label());
    assert!(!fixture.root.join("preferences.json").exists());
    assert!(!fixture.root.join("catalog.sqlite3").exists());
    let prefs = fixture.preferences();
    assert!(prefs.get().interface_font_file.is_none());
    assert!(prefs.get().terminal_font_file.is_none());
    publish(&fixture.root, &resource).expect("复用相同 blob");
    assert_eq!(
        std::fs::read_dir(fixture.root.join("fonts"))
            .expect("缓存目录")
            .count(),
        1
    );
}

#[test]
fn publication_never_changes_either_selected_slot_or_configuration_revision() {
    let fixture = Fixture::new();
    let prefs = fixture.preferences();
    for slot in ["interface", "terminal"] {
        let (state, _) =
            fonts::handle(&fixture.root, &prefs, request(slot, "apply"), FONT.to_vec());
        assert!(state.applied && state.error.is_empty(), "{}", state.error);
    }
    let preferences_before =
        std::fs::read(fixture.root.join("preferences.json")).expect("已有偏好");
    let repository = fixture.repository();
    let config_before = repository.load_configuration().expect("已有配置");
    let resource = verify_ttf(OTHER_FONT.to_vec()).expect("独立粗体资源");
    publish(&fixture.root, &resource).expect("为未来 profile 发布粗体");
    let current = prefs.get();
    for selected in [current.interface_font_file, current.terminal_font_file] {
        assert_ne!(selected.expect("原双槽选择").hash, resource.hash());
    }
    assert_eq!(
        std::fs::read(fixture.root.join("preferences.json")).expect("偏好未变"),
        preferences_before
    );
    assert_eq!(
        repository.load_configuration().expect("配置未变"),
        config_before
    );
    assert_resource(
        &load_by_hash(&fixture.root, resource.hash()).expect("未引用缓存保留"),
        OTHER_FONT,
    );
}

#[test]
fn missing_and_path_like_hashes_have_fixed_redacted_reasons() {
    let fixture = Fixture::new();
    let resource = verify_ttf(FONT.to_vec()).expect("字体身份");
    assert_error(
        load_by_hash(&fixture.root, resource.hash()),
        FontResourceError::Missing,
    );
    for hash in [
        String::new(),
        "../../outside".to_owned(),
        format!("{}../", "a".repeat(61)),
        format!("/{}", "a".repeat(63)),
        format!("C:\\{}", "a".repeat(61)),
        "a".repeat(63),
        "g".repeat(64),
        resource.family(),
        resource.hash().to_ascii_uppercase(),
    ] {
        assert_error(
            load_by_hash(&fixture.root, &hash),
            FontResourceError::Corrupt,
        );
    }
    for (error, reason, message) in [
        (FontResourceError::Missing, "missing", "字体缓存缺失"),
        (FontResourceError::Corrupt, "corrupt", "字体缓存损坏"),
        (
            FontResourceError::Unavailable,
            "unavailable",
            "字体缓存不可用",
        ),
    ] {
        assert_eq!(error.reason(), reason);
        assert_eq!(error.to_string(), message);
        assert!(
            !error
                .to_string()
                .contains(&fixture.root.to_string_lossy().to_string())
        );
    }
}

#[test]
fn cached_family_never_bypasses_structure_or_content_hash_validation_and_retries_work() {
    let fixture = Fixture::new();
    let prefs = fixture.preferences();
    let resource = verify_ttf(FONT.to_vec()).expect("字体资源");
    publish(&fixture.root, &resource).expect("原资源发布");
    let repository = fixture.repository();
    let config_before = repository.load_configuration().expect("配置快照");
    for bytes in [b"broken".as_slice(), OTHER_FONT] {
        std::fs::write(fixture.path(&resource), bytes).expect("替换缓存内容");
        assert_error(
            load_by_hash(&fixture.root, resource.hash()),
            FontResourceError::Corrupt,
        );
        assert_eq!(resource.family(), format!("GuoshFont_{}", resource.hash()));
    }
    std::fs::remove_file(fixture.path(&resource)).expect("模拟缓存丢失");
    assert_error(
        load_by_hash(&fixture.root, resource.hash()),
        FontResourceError::Missing,
    );
    assert_eq!(
        repository.load_configuration().expect("失败不改配置"),
        config_before
    );
    assert!(prefs.get().terminal_font_file.is_none());
    assert!(!fixture.root.join("preferences.json").exists());
    std::fs::write(fixture.path(&resource), FONT).expect("恢复缓存");
    assert_resource(
        &load_by_hash(&fixture.root, resource.hash()).expect("同 hash 可重试"),
        FONT,
    );
}

#[test]
fn oversized_cached_file_is_rejected_before_loading_its_contents() {
    let fixture = Fixture::new();
    let resource = verify_ttf(FONT.to_vec()).expect("字体资源");
    publish(&fixture.root, &resource).expect("原资源发布");
    std::fs::OpenOptions::new()
        .write(true)
        .open(fixture.path(&resource))
        .expect("打开缓存")
        .set_len(MAX_FONT_BYTES as u64 + 1)
        .expect("过大缓存");
    assert_error(
        load_by_hash(&fixture.root, resource.hash()),
        FontResourceError::Corrupt,
    );
    assert_eq!(
        publish(&fixture.root, &resource),
        Err(FontResourceError::Corrupt)
    );
}

#[test]
fn unavailable_cache_and_publication_failures_do_not_publish_a_selection() {
    let fixture = Fixture::new();
    let prefs = fixture.preferences();
    let resource = verify_ttf(FONT.to_vec()).expect("字体资源");
    std::fs::write(fixture.root.join("fonts"), b"not a directory").expect("不可用缓存目录");
    assert_eq!(
        publish(&fixture.root, &resource),
        Err(FontResourceError::Unavailable)
    );
    assert_error(
        load_by_hash(&fixture.root, resource.hash()),
        FontResourceError::Unavailable,
    );
    assert!(!fixture.root.join("preferences.json").exists());
    assert!(prefs.get().interface_font_file.is_none());
    assert!(prefs.get().terminal_font_file.is_none());
}

#[test]
fn directory_instead_of_cached_blob_is_unavailable() {
    let fixture = Fixture::new();
    let resource = verify_ttf(FONT.to_vec()).expect("字体资源");
    std::fs::create_dir_all(fixture.path(&resource)).expect("错误的缓存文件类型");
    assert_error(
        load_by_hash(&fixture.root, resource.hash()),
        FontResourceError::Unavailable,
    );
}

#[test]
fn concurrent_publication_reuses_one_complete_blob_and_cleans_temporary_files() {
    let fixture = Fixture::new();
    let resource = verify_ttf(FONT.to_vec()).expect("字体资源");
    let barrier = Barrier::new(8);
    std::thread::scope(|scope| {
        for _ in 0..8 {
            let fixture = &fixture;
            let resource = &resource;
            let barrier = &barrier;
            scope.spawn(move || {
                barrier.wait();
                publish(&fixture.root, resource).expect("并发安全发布");
                let loaded =
                    load_by_hash(&fixture.root, resource.hash()).expect("读取完整发布结果");
                assert_resource(&loaded, FONT);
            });
        }
    });
    assert_eq!(
        std::fs::read_dir(fixture.root.join("fonts"))
            .expect("缓存目录")
            .count(),
        1
    );
    assert_resource(
        &load_by_hash(&fixture.root, resource.hash()).expect("最终缓存"),
        FONT,
    );
}

#[test]
fn concurrent_publication_does_not_overwrite_an_incomplete_existing_blob() {
    let fixture = Fixture::new();
    let resource = verify_ttf(FONT.to_vec()).expect("字体资源");
    std::fs::create_dir(fixture.root.join("fonts")).expect("缓存目录");
    std::fs::write(fixture.path(&resource), b"partial").expect("不完整 blob");
    std::thread::scope(|scope| {
        for _ in 0..8 {
            let fixture = &fixture;
            let resource = &resource;
            scope.spawn(move || {
                assert_eq!(
                    publish(&fixture.root, resource),
                    Err(FontResourceError::Corrupt)
                );
            });
        }
    });
    assert_eq!(
        std::fs::read(fixture.path(&resource)).expect("没有覆写已有 blob"),
        b"partial"
    );
    assert_eq!(
        std::fs::read_dir(fixture.root.join("fonts"))
            .expect("没有遗留临时文件")
            .count(),
        1
    );
}

#[test]
fn failed_cache_publication_preserves_old_slot_selection_and_legacy_error() {
    let fixture = Fixture::new();
    let prefs = fixture.preferences();
    let (saved, _) = fonts::handle(
        &fixture.root,
        &prefs,
        request("terminal", "apply"),
        FONT.to_vec(),
    );
    assert!(saved.applied && saved.error.is_empty(), "{}", saved.error);
    let previous = std::fs::read(fixture.root.join("preferences.json")).expect("旧槽偏好");
    let resource = verify_ttf(OTHER_FONT.to_vec()).expect("新资源");
    std::fs::write(fixture.path(&resource), b"partial").expect("已有损坏 blob");
    assert_eq!(
        publish(&fixture.root, &resource),
        Err(FontResourceError::Corrupt)
    );
    let (failed, bytes) = fonts::handle(
        &fixture.root,
        &prefs,
        request("terminal", "apply"),
        OTHER_FONT.to_vec(),
    );
    assert!(!failed.applied);
    assert_eq!(failed.error, "字体缓存校验失败");
    assert!(bytes.is_empty());
    assert_eq!(
        std::fs::read(fixture.root.join("preferences.json")).expect("旧偏好不变"),
        previous
    );
    assert_resource(
        &load_by_hash(
            &fixture.root,
            &prefs.get().terminal_font_file.expect("旧选择").hash,
        )
        .expect("旧资源可用"),
        FONT,
    );
}

#[test]
fn loading_a_captured_imported_font_never_requeries_the_current_slot() {
    let fixture = Fixture::new();
    let prefs = fixture.preferences();
    let (saved, _) = fonts::handle(
        &fixture.root,
        &prefs,
        request("terminal", "apply"),
        FONT.to_vec(),
    );
    assert!(saved.applied && saved.error.is_empty(), "{}", saved.error);
    // 一次 get 取得完整旧槽资源；异步工作不得在加载时再次取当前槽。
    let captured = prefs.get().terminal_font_file.expect("捕获旧 ImportedFont");
    let (changed, _) = fonts::handle(
        &fixture.root,
        &prefs,
        request("terminal", "apply"),
        OTHER_FONT.to_vec(),
    );
    assert!(
        changed.applied && changed.error.is_empty(),
        "{}",
        changed.error
    );
    assert_ne!(
        prefs.get().terminal_font_file.expect("新槽").hash,
        captured.hash
    );
    let loaded = load_by_hash(&fixture.root, &captured.hash).expect("加载捕获身份");
    assert_resource(&loaded, FONT);
    assert_eq!(loaded.family(), saved.family);
    assert_eq!(loaded.label(), captured.label);
}

#[test]
fn cached_resource_survives_both_slots_reset_profile_delete_and_database_reopen() {
    let fixture = Fixture::new();
    let prefs = fixture.preferences();
    let resource = verify_ttf(FONT.to_vec()).expect("profile 独立资源");
    publish(&fixture.root, &resource).expect("不经 terminal apply 发布");
    let id = TerminalProfileId::from(uuid::Uuid::new_v4());
    {
        let repository = fixture.repository();
        let snapshot = repository.load_configuration().expect("配置快照");
        let mut profile = snapshot.profiles[0].clone();
        profile.id = id;
        profile.name = "缓存资源引用".to_owned();
        profile.settings.font_family = resource.family();
        // 此 fixture 仅建立缓存引用；模式/hash 的 CAS 接缝由另一 worker 实现。
        assert!(matches!(
            repository
                .commit_configuration(ConfigurationChange {
                    expected_revision: snapshot.revision,
                    settings: snapshot.settings,
                    upsert_profiles: vec![profile],
                    delete_profiles: vec![],
                })
                .expect("保存 profile 引用"),
            ConfigurationCommitOutcome::Applied { .. }
        ));
    }
    for slot in ["interface", "terminal"] {
        let (applied, _) =
            fonts::handle(&fixture.root, &prefs, request(slot, "apply"), FONT.to_vec());
        assert!(
            applied.applied && applied.error.is_empty(),
            "{}",
            applied.error
        );
        let (reset, _) = fonts::handle(&fixture.root, &prefs, request(slot, "reset"), vec![]);
        assert!(reset.applied && reset.error.is_empty(), "{}", reset.error);
    }
    let held = load_by_hash(&fixture.root, resource.hash()).expect("捕获运行资源");
    {
        let reopened = fixture.repository();
        let snapshot = reopened.load_configuration().expect("重开 profile 数据库");
        assert_eq!(
            snapshot
                .profiles
                .iter()
                .find(|profile| profile.id == id)
                .expect("重开 profile")
                .settings
                .font_family,
            resource.family()
        );
        assert_resource(
            &load_by_hash(&fixture.root, resource.hash()).expect("重开按引用加载"),
            FONT,
        );
        assert!(matches!(
            reopened
                .commit_configuration(ConfigurationChange {
                    expected_revision: snapshot.revision,
                    settings: snapshot.settings,
                    upsert_profiles: vec![],
                    delete_profiles: vec![id],
                })
                .expect("删除可删除的 profile"),
            ConfigurationCommitOutcome::Applied { .. }
        ));
    }
    let reopened_prefs = fixture.preferences();
    assert!(reopened_prefs.get().interface_font_file.is_none());
    assert!(reopened_prefs.get().terminal_font_file.is_none());
    assert!(
        fixture
            .repository()
            .load_configuration()
            .expect("删后重开配置")
            .profiles
            .iter()
            .all(|profile| profile.id != id)
    );
    assert_resource(
        &load_by_hash(&fixture.root, resource.hash()).expect("删除引用后仍保守留存"),
        FONT,
    );
    assert_resource(&held, FONT);
}

#[test]
fn held_bytes_are_immutable_when_the_cache_is_changed_or_removed() {
    let fixture = Fixture::new();
    let resource = verify_ttf(FONT.to_vec()).expect("资源校验");
    publish(&fixture.root, &resource).expect("资源发布");
    let held = load_by_hash(&fixture.root, resource.hash()).expect("保留资源句柄");
    let clone = held.clone();
    assert!(Arc::ptr_eq(held.bytes(), clone.bytes()));
    let held_bytes = Arc::clone(held.bytes());
    std::fs::write(fixture.path(&resource), OTHER_FONT).expect("磁盘缓存被修改");
    assert_error(
        load_by_hash(&fixture.root, resource.hash()),
        FontResourceError::Corrupt,
    );
    assert_resource(&held, FONT);
    std::fs::remove_file(fixture.path(&resource)).expect("磁盘缓存丢失");
    assert_error(
        load_by_hash(&fixture.root, resource.hash()),
        FontResourceError::Missing,
    );
    assert_resource(&clone, FONT);
    assert_eq!(held_bytes.as_ref(), FONT);
}
