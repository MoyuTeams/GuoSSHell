//! 用户字体的校验、持久化与缓存标识。预览请求不会写入偏好或磁盘。
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::keys::PreferenceFile;
use crate::signals::settings::{FontFileRequest, FontFileState};

pub(super) const MAX_FONT_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportedFont {
    pub hash: String,
    pub label: String,
}

fn default_family(slot: &str) -> &'static str {
    if slot == "terminal" {
        crate::settings::DEFAULT_TERMINAL_FONT
    } else {
        "MiSans"
    }
}

pub(super) fn font_path(root: &Path, hash: &str) -> Result<PathBuf, String> {
    if hash.len() != 64 || !hash.bytes().all(|value| value.is_ascii_hexdigit()) {
        return Err("字体缓存标识无效".to_owned());
    }
    Ok(root.join("fonts").join(format!("{hash}.ttf")))
}

pub(super) fn inspect(bytes: &[u8], file_name: &str) -> Result<ImportedFont, String> {
    if bytes.len() > MAX_FONT_BYTES {
        return Err("字体文件不能超过 32 MiB".to_owned());
    }
    if !file_name.to_ascii_lowercase().ends_with(".ttf") {
        return Err("请选择 .ttf 字体文件".to_owned());
    }
    if bytes.get(..4) != Some(&[0, 1, 0, 0]) && bytes.get(..4) != Some(b"true") {
        return Err("文件不是有效的 TrueType 字体".to_owned());
    }
    let face = ttf_parser::Face::parse(bytes, 0)
        .map_err(|_| "无法读取 TTF 字体，请选择其他文件".to_owned())?;
    if face.number_of_glyphs() == 0 || face.tables().glyf.is_none() || face.tables().cmap.is_none()
    {
        return Err("字体缺少有效的字形或字符映射".to_owned());
    }
    let label = face
        .names()
        .into_iter()
        .filter(|name| name.name_id == ttf_parser::name_id::FAMILY)
        .find_map(|name| name.to_string())
        .unwrap_or_else(|| {
            file_name
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or("自定义字体")
                .to_owned()
        })
        .chars()
        .filter(|value| !value.is_control())
        .take(128)
        .collect();
    Ok(ImportedFont {
        hash: Sha256::digest(bytes)
            .iter()
            .flat_map(|byte| {
                const HEX: &[u8; 16] = b"0123456789abcdef";
                [
                    HEX[(*byte >> 4) as usize] as char,
                    HEX[(*byte & 15) as usize] as char,
                ]
            })
            .collect(),
        label,
    })
}

fn selected(preferences: &PreferenceFile, slot: &str) -> Option<ImportedFont> {
    let value = preferences.get();
    if slot == "terminal" {
        value.terminal_font_file
    } else {
        value.interface_font_file
    }
}

fn replace(
    preferences: &PreferenceFile,
    slot: &str,
    font: Option<ImportedFont>,
) -> Result<(), String> {
    // profile 和运行中快照也可能引用缓存，双槽变更或偏好写入失败都不能据此回收。
    preferences.update(|value| {
        if slot == "terminal" {
            value.terminal_font_file = font;
        } else {
            value.interface_font_file = font;
        }
    })
}

#[derive(Debug)]
pub(super) enum CacheError {
    InvalidHash,
    Corrupt,
    Io(std::io::Error),
}

impl CacheError {
    fn legacy_message(self) -> String {
        match self {
            Self::InvalidHash => "字体缓存标识无效".to_owned(),
            Self::Corrupt => "字体缓存校验失败".to_owned(),
            Self::Io(error) => error.to_string(),
        }
    }
}

impl From<std::io::Error> for CacheError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// 先检查打开文件的大小，再限制实际读取量，避免读取期间增长绕过 32 MiB 上限。
pub(super) fn read_cached(path: &Path) -> Result<Vec<u8>, CacheError> {
    let file = std::fs::File::open(path)?;
    if file.metadata()?.len() > MAX_FONT_BYTES as u64 {
        return Err(CacheError::Corrupt);
    }
    let mut bytes = Vec::new();
    file.take(MAX_FONT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_FONT_BYTES {
        return Err(CacheError::Corrupt);
    }
    Ok(bytes)
}

fn reuse_cached(path: &Path, bytes: &[u8]) -> Result<(), CacheError> {
    if read_cached(path)? != bytes {
        return Err(CacheError::Corrupt);
    }
    Ok(())
}

/// 只发布已校验的内容，不改选择；旧双槽与独立资源共用同一缓存。
pub(super) fn publish_cached(
    root: &Path,
    font: &ImportedFont,
    bytes: &[u8],
) -> Result<(), CacheError> {
    let path = font_path(root, &font.hash).map_err(|_| CacheError::InvalidHash)?;
    match reuse_cached(&path, bytes) {
        Ok(()) => return Ok(()),
        Err(CacheError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let directory = root.join("fonts");
    std::fs::create_dir_all(&directory)?;
    let temporary = directory.join(format!("{}.tmp", uuid::Uuid::new_v4()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let written = (|| -> Result<(), CacheError> {
        file.write_all(bytes)?;
        file.sync_all()?;
        // rename 在部分平台会覆盖目标。硬链接原子发布完整文件，且绝不覆盖已有 blob。
        match std::fs::hard_link(&temporary, &path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                reuse_cached(&path, bytes)
            }
            Err(error) => Err(CacheError::Io(error)),
        }
    })();
    drop(file);
    finish_cache_publication(&temporary, written)
}

fn finish_cache_publication(
    temporary: &Path,
    written: Result<(), CacheError>,
) -> Result<(), CacheError> {
    // 清理失败优先返回；只清理本轮临时名，绝不回滚已就绪 blob。
    std::fs::remove_file(temporary)?;
    written
}

fn apply(
    root: &Path,
    preferences: &PreferenceFile,
    slot: &str,
    font: ImportedFont,
    bytes: &[u8],
) -> Result<(), String> {
    publish_cached(root, &font, bytes).map_err(CacheError::legacy_message)?;
    replace(preferences, slot, Some(font))
}

pub fn handle(
    root: &Path,
    preferences: &PreferenceFile,
    request: FontFileRequest,
    bytes: Vec<u8>,
) -> (FontFileState, Vec<u8>) {
    let mut state = FontFileState {
        request_id: request.request_id,
        slot: request.slot.clone(),
        family: default_family(&request.slot).to_owned(),
        label: default_family(&request.slot).to_owned(),
        error: String::new(),
        applied: false,
    };
    let result = (|| -> Result<Vec<u8>, String> {
        if !matches!(request.slot.as_str(), "interface" | "terminal") {
            return Err("未知的字体用途".to_owned());
        }
        match request.action.as_str() {
            "preview" | "apply" => {
                let font = inspect(&bytes, &request.file_name)?;
                if request.action == "apply" {
                    apply(root, preferences, &request.slot, font.clone(), &bytes)?;
                    state.applied = true;
                }
                state.family = format!("GuoshFont_{}", font.hash);
                state.label = font.label;
                Ok(bytes)
            }
            "reset" => {
                replace(preferences, &request.slot, None)?;
                state.applied = true;
                Ok(Vec::new())
            }
            "query" => {
                state.applied = true;
                if let Some(font) = selected(preferences, &request.slot) {
                    let path = font_path(root, &font.hash)?;
                    if std::fs::metadata(&path)
                        .map_err(|_| "已保存的字体不可用，已恢复内置字体")?
                        .len()
                        > MAX_FONT_BYTES as u64
                    {
                        return Err("已保存的字体过大".to_owned());
                    }
                    let data = std::fs::read(path).map_err(|e| e.to_string())?;
                    let checked = inspect(&data, "saved.ttf")?;
                    if checked.hash != font.hash {
                        return Err("已保存的字体校验失败，已恢复内置字体".to_owned());
                    }
                    state.family = format!("GuoshFont_{}", font.hash);
                    state.label = font.label;
                    Ok(data)
                } else {
                    Ok(Vec::new())
                }
            }
            _ => Err("未知字体操作".to_owned()),
        }
    })();
    match result {
        Ok(bytes) => (state, bytes),
        Err(error) => {
            state.error = error;
            (state, Vec::new())
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::*;
    use rshell_m0::rshell_storage::{
        ConfigurationChange, ConfigurationCommitOutcome, SqliteRepository,
    };
    const FONT: &[u8] = include_bytes!("../../../assets/fonts/MesloLGS-NF-Regular.ttf");
    const OTHER_FONT: &[u8] = include_bytes!("../../../assets/fonts/MesloLGS-NF-Bold.ttf");

    struct CacheFixture {
        root: PathBuf,
    }

    impl CacheFixture {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("guosh-font-cache-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&root).expect("缓存测试目录");
            Self { root }
        }

        fn preferences(&self) -> PreferenceFile {
            PreferenceFile::open(self.root.join("preferences.json"))
        }

        fn repository(&self) -> SqliteRepository {
            let repository = SqliteRepository::open(self.root.join("catalog.sqlite3"))
                .expect("打开字体引用数据库");
            repository.migrate().expect("迁移字体引用数据库");
            repository
        }
    }

    impl Drop for CacheFixture {
        fn drop(&mut self) {
            if let Err(error) = std::fs::remove_dir_all(&self.root) {
                if std::thread::panicking() {
                    eprintln!("缓存测试目录清理失败：{error}");
                } else {
                    panic!("缓存测试目录清理失败：{error}");
                }
            }
        }
    }

    fn save_profile_family(repository: &SqliteRepository, family: &str) {
        let snapshot = repository.load_configuration().expect("读取配置快照");
        let mut profile = snapshot.profiles[0].clone();
        profile.settings.font_family = family.to_owned();
        let committed = repository
            .commit_configuration(ConfigurationChange {
                expected_revision: snapshot.revision,
                settings: snapshot.settings,
                upsert_profiles: vec![profile],
                delete_profiles: vec![],
            })
            .expect("保存 profile 字体引用");
        assert!(matches!(
            committed,
            ConfigurationCommitOutcome::Applied { .. }
        ));
    }

    fn assert_cached_family(root: &Path, family: &str, expected: &[u8]) {
        let hash = family.strip_prefix("GuoshFont_").expect("导入字体标识");
        let path = font_path(root, hash).expect("字体缓存路径");
        assert!(path.is_file(), "双槽变更不能删除 profile 或快照引用的缓存");
        let bytes = std::fs::read(path).expect("读回字体缓存");
        assert_eq!(bytes, expected);
        assert_eq!(
            inspect(&bytes, "cached.ttf").expect("校验缓存字体").hash,
            hash
        );
    }

    fn fixture() -> (PathBuf, PreferenceFile) {
        let root = std::env::temp_dir().join(format!("guosh-font-file-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("测试目录");
        let preferences = PreferenceFile::open(root.join("preferences.json"));
        (root, preferences)
    }
    fn request(slot: &str, action: &str) -> FontFileRequest {
        FontFileRequest {
            request_id: 1,
            slot: slot.to_owned(),
            action: action.to_owned(),
            file_name: "chosen.ttf".to_owned(),
        }
    }

    #[test]
    fn preview_does_not_write_and_invalid_fonts_do_not_replace_preferences() {
        let (root, prefs) = fixture();
        let (state, data) = handle(
            &root,
            &prefs,
            request("interface", "preview"),
            FONT.to_vec(),
        );
        assert!(state.error.is_empty());
        assert!(!state.applied);
        assert_eq!(data, FONT);
        assert!(!root.join("fonts").exists());
        assert!(prefs.get().interface_font_file.is_none());
        let (bad, _) = handle(
            &root,
            &prefs,
            request("interface", "apply"),
            vec![0, 1, 0, 0],
        );
        assert!(!bad.error.is_empty());
        assert!(!bad.applied);
        assert!(prefs.get().interface_font_file.is_none());
        let mut otf = request("terminal", "apply");
        otf.file_name = "wrong.otf".to_owned();
        assert!(!handle(&root, &prefs, otf, FONT.to_vec()).0.error.is_empty());
        assert!(font_path(&root, "../../outside").is_err());
    }

    #[test]
    fn legacy_font_name_is_ignored_without_losing_other_preferences() {
        let (root, _) = fixture();
        let path = root.join("preferences.json");
        std::fs::write(
            &path,
            r#"{"interface_font":"misans","show_key_bar":false,"windows_opacity":0.6}"#,
        )
        .expect("旧偏好");
        let preferences = PreferenceFile::open(path);
        let (state, _) = handle(&root, &preferences, request("interface", "query"), vec![]);
        assert_eq!(state.family, "MiSans");
        assert!(state.error.is_empty());
        assert_eq!(preferences.get().show_key_bar, Some(false));
        assert_eq!(preferences.get().windows_opacity, Some(0.6));
    }

    #[test]
    fn files_survive_reopen_and_shared_font_cache_is_retained_after_both_slots_reset() {
        let (root, prefs) = fixture();
        let source = root.join("original.ttf");
        std::fs::write(&source, FONT).expect("原文件");
        let (ui, _) = handle(&root, &prefs, request("interface", "apply"), FONT.to_vec());
        let (terminal, _) = handle(&root, &prefs, request("terminal", "apply"), FONT.to_vec());
        assert!(ui.applied && terminal.applied);
        assert_eq!(ui.family, terminal.family);
        let hash = prefs.get().terminal_font_file.expect("终端偏好").hash;
        let path = font_path(&root, &hash).expect("缓存路径");
        let reopened = PreferenceFile::open(root.join("preferences.json"));
        let (loaded, bytes) = handle(&root, &reopened, request("terminal", "query"), vec![]);
        assert_eq!(loaded.family, terminal.family);
        assert_eq!(bytes, FONT);
        assert!(
            handle(&root, &reopened, request("interface", "reset"), vec![])
                .0
                .applied
        );
        assert!(path.exists());
        assert!(
            handle(&root, &reopened, request("terminal", "reset"), vec![])
                .0
                .applied
        );
        assert!(path.is_file());
        assert_eq!(std::fs::read(path).expect("重置后缓存保留"), FONT);
        assert_eq!(std::fs::read(source).expect("原文件保留"), FONT);
    }

    #[test]
    fn profile_font_cache_survives_both_slots_reset_and_database_reopen() {
        assert_profile_cache_survives_slot_changes("reset");
    }

    #[test]
    fn profile_font_cache_survives_both_slots_replaced_and_database_reopen() {
        assert_profile_cache_survives_slot_changes("apply");
    }

    fn assert_profile_cache_survives_slot_changes(action: &str) {
        let fixture = CacheFixture::new();
        let prefs = fixture.preferences();
        let source = fixture.root.join("original.ttf");
        std::fs::write(&source, FONT).expect("真实导入文件");
        let imported = std::fs::read(&source).expect("读取真实导入文件");
        let (saved, bytes) = handle(
            &fixture.root,
            &prefs,
            request("terminal", "apply"),
            imported.clone(),
        );
        assert!(saved.applied && saved.error.is_empty(), "{}", saved.error);
        assert_eq!(bytes, imported);
        let (shared, _) = handle(
            &fixture.root,
            &prefs,
            request("interface", "apply"),
            imported,
        );
        assert!(
            shared.applied && shared.error.is_empty(),
            "{}",
            shared.error
        );
        assert_eq!(shared.family, saved.family);
        assert_cached_family(&fixture.root, &saved.family, FONT);
        {
            let repository = fixture.repository();
            save_profile_family(&repository, &saved.family);
        }

        for slot in ["interface", "terminal"] {
            let (state, _) = handle(
                &fixture.root,
                &prefs,
                request(slot, action),
                OTHER_FONT.to_vec(),
            );
            assert!(state.applied && state.error.is_empty(), "{}", state.error);
            assert_ne!(state.family, saved.family);
        }
        let reopened_prefs = fixture.preferences();
        for slot in ["interface", "terminal"] {
            let (state, bytes) = handle(
                &fixture.root,
                &reopened_prefs,
                request(slot, "query"),
                vec![],
            );
            assert!(state.applied && state.error.is_empty(), "{}", state.error);
            assert_ne!(state.family, saved.family);
            if action == "reset" {
                assert_eq!(state.family, default_family(slot));
                assert!(bytes.is_empty());
            } else {
                assert_eq!(bytes, OTHER_FONT);
            }
        }
        let reopened = fixture.repository();
        let snapshot = reopened.load_configuration().expect("重开 profile 数据库");
        assert_eq!(snapshot.profiles[0].settings.font_family, saved.family);
        assert_cached_family(
            &fixture.root,
            &snapshot.profiles[0].settings.font_family,
            FONT,
        );
        assert_eq!(std::fs::read(source).expect("原文件保留"), FONT);
    }

    #[test]
    fn font_cache_survives_when_only_a_held_settings_snapshot_references_it() {
        let fixture = CacheFixture::new();
        let prefs = fixture.preferences();
        let (saved, _) = handle(
            &fixture.root,
            &prefs,
            request("terminal", "apply"),
            FONT.to_vec(),
        );
        assert!(saved.applied && saved.error.is_empty(), "{}", saved.error);
        let repository = fixture.repository();
        save_profile_family(&repository, &saved.family);
        let held = repository.load_configuration().expect("保留配置快照");
        save_profile_family(&repository, default_family("terminal"));
        let (reset, _) = handle(&fixture.root, &prefs, request("terminal", "reset"), vec![]);
        assert!(reset.applied && reset.error.is_empty(), "{}", reset.error);
        assert!(prefs.get().interface_font_file.is_none());
        assert!(prefs.get().terminal_font_file.is_none());
        let latest = repository.load_configuration().expect("最新配置");
        assert_eq!(
            latest.profiles[0].settings.font_family,
            default_family("terminal")
        );
        assert_eq!(held.profiles[0].settings.font_family, saved.family);
        assert_cached_family(&fixture.root, &held.profiles[0].settings.font_family, FONT);
    }

    #[test]
    fn failed_preference_write_preserves_an_imported_cache_for_profile_references() {
        let fixture = CacheFixture::new();
        let inspected = inspect(FONT, "chosen.ttf").expect("字体标识");
        let family = format!("GuoshFont_{}", inspected.hash);
        let repository = fixture.repository();
        save_profile_family(&repository, &family);
        let broken = PreferenceFile::open(fixture.root.join("missing").join("preferences.json"));
        let (state, _) = handle(
            &fixture.root,
            &broken,
            request("terminal", "apply"),
            FONT.to_vec(),
        );
        assert!(!state.applied);
        assert!(!state.error.is_empty());
        assert!(broken.get().terminal_font_file.is_none());
        let snapshot = repository
            .load_configuration()
            .expect("保存的 profile 引用");
        assert_eq!(snapshot.profiles[0].settings.font_family, family);
        assert_cached_family(&fixture.root, &family, FONT);
    }

    #[test]
    fn corrupt_cache_falls_back_and_failed_preference_write_does_not_publish_font() {
        let (root, prefs) = fixture();
        let (saved, _) = handle(&root, &prefs, request("interface", "apply"), FONT.to_vec());
        assert!(saved.applied);
        let path = font_path(&root, &prefs.get().interface_font_file.expect("偏好").hash)
            .expect("缓存路径");
        std::fs::write(path, b"broken").expect("损坏副本");
        let (state, _) = handle(&root, &prefs, request("interface", "query"), vec![]);
        assert_eq!(state.family, "MiSans");
        assert!(state.applied);
        assert!(!state.error.is_empty());
        let (other, _) = fixture();
        let broken = PreferenceFile::open(other.join("missing").join("preferences.json"));
        let (state, _) = handle(&other, &broken, request("terminal", "apply"), FONT.to_vec());
        assert!(!state.applied);
        assert!(!state.error.is_empty());
        assert!(broken.get().terminal_font_file.is_none());
    }

    fn staged_cache_fixture(fixture: &CacheFixture) -> (PathBuf, PathBuf) {
        let directory = fixture.root.join("fonts");
        std::fs::create_dir(&directory).expect("缓存目录");
        let font = inspect(FONT, "chosen.ttf").expect("真实 TTF 标识");
        let cached = font_path(&fixture.root, &font.hash).expect("缓存 blob 路径");
        let temporary = directory.join("publication.tmp");
        std::fs::write(&temporary, FONT).expect("完整临时文件");
        std::fs::hard_link(&temporary, &cached).expect("原子发布 blob");
        (temporary, cached)
    }

    #[test]
    fn cleanup_failure_is_typed_and_preserves_an_already_published_blob() {
        let fixture = CacheFixture::new();
        let (temporary, cached) = staged_cache_fixture(&fixture);
        std::fs::remove_file(&temporary).expect("模拟其他清理者先移除临时文件");
        let error = finish_cache_publication(&temporary, Ok(())).expect_err("清理失败不能返回成功");
        assert!(
            matches!(error, CacheError::Io(error) if error.kind() == std::io::ErrorKind::NotFound)
        );
        assert_eq!(std::fs::read(cached).expect("已发布 blob 不回滚"), FONT);
        assert!(!fixture.root.join("preferences.json").exists());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn cleanup_failure_from_a_delete_blocking_handle_preserves_both_files() {
        use std::os::windows::fs::OpenOptionsExt as _;

        let fixture = CacheFixture::new();
        let (temporary, cached) = staged_cache_fixture(&fixture);
        let blocker = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&temporary)
            .expect("不允许共享删除的测试句柄");
        let result = finish_cache_publication(&temporary, Ok(()));
        drop(blocker);
        let error = result.expect_err("删除受阻不能返回成功");
        assert!(matches!(error, CacheError::Io(error) if error.raw_os_error() == Some(32)));
        assert_eq!(std::fs::read(temporary).expect("受阻临时文件保留"), FONT);
        assert_eq!(std::fs::read(cached).expect("已发布 blob 保留"), FONT);
        assert!(!fixture.root.join("preferences.json").exists());
    }

    #[test]
    fn successful_cleanup_preserves_the_publication_error_and_never_removes_the_blob() {
        let fixture = CacheFixture::new();
        let (temporary, cached) = staged_cache_fixture(&fixture);
        assert!(matches!(
            finish_cache_publication(&temporary, Err(CacheError::Corrupt)),
            Err(CacheError::Corrupt)
        ));
        assert!(!temporary.exists());
        assert_eq!(std::fs::read(cached).expect("完成阶段不移除 blob"), FONT);
    }
}
