//! 桌面完整配置与 profile 事务、旧四字段补丁、窗口外观及独立字体文件请求。
//! 移动端保留字体白名单、字号与按物理内存分档的滚回限制。

use std::sync::Arc;

use rinf::{DartSignal, DartSignalBinary, RustSignal, RustSignalBinary, debug_print};
use rshell_m0::rshell_core::{TerminalProfile, TerminalSettingsV1};
use rshell_m0::rshell_storage::{
    ConfigurationChange, ConfigurationCommitOutcome, SqliteRepository, StorageError,
};
use tokio::task::spawn_blocking;

use crate::app::AppContext;
use crate::signals::settings::{
    DesktopProfileSettingsRequest, DesktopProfileTtfRequest, DesktopSettingsRequest,
    KeyBarLayoutResult, ProfileFontFailure, ProfileFontMode, ProfileFontResourceRequest,
    ProfileFontResourceState, SaveKeyBarLayout, SaveSettings, SaveWindowsAppearance, SettingsQuery,
    SettingsState, WindowsAppearanceQuery, WindowsAppearanceState,
};
use crate::signals::settings::{FontFileRequest, FontFileState};

#[path = "desktop_settings.rs"]
mod desktop;

/// 新桌面请求统一复用编译目标能力；Dart 无权启用。
pub(crate) use desktop::available as desktop_available;

pub const DEFAULT_TERMINAL_FONT: &str = "MesloLGS NF";
/// 可选字体：第一个随 App 内置（带 powerline / Nerd Font 字形），其余是系统自带。
pub const FONT_FAMILIES: [&str; 2] = [DEFAULT_TERMINAL_FONT, "Menlo"];
const DEFAULT_FONT_SIZE: f32 = 14.0;
const MIN_FONT_SIZE: f32 = 8.0;
const MAX_FONT_SIZE: f32 = 32.0;
/// 滚回行数的下限（上游配置校验也不接受更少）。
const MIN_SCROLLBACK_LINES: usize = 1_000;

/// 本机每个会话的滚回上界。桌面的配置上限是一百万行，手机照搬会被系统因内存直接杀掉：
/// alacritty 每格约 24 字节，200 列 × 1 万行约 48 MB，多开几个会话还要翻倍。
pub fn scrollback_cap() -> usize {
    scrollback_cap_for(physical_memory())
}

fn scrollback_cap_for(physical_memory: u64) -> usize {
    const GIB: u64 = 1 << 30;
    match physical_memory {
        // 取不到内存大小：按中档。
        0 => 5_000,
        bytes if bytes < 5 * GIB => 2_000,
        bytes if bytes < 9 * GIB => 5_000,
        bytes if bytes < 17 * GIB => 20_000,
        _ => 100_000,
    }
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
fn physical_memory() -> u64 {
    objc2_foundation::NSProcessInfo::processInfo().physicalMemory()
}

#[cfg(not(any(target_os = "ios", target_os = "macos")))]
fn physical_memory() -> u64 {
    0
}

/// 键位条默认显示与否：触屏设备（iOS / iPadOS）默认显示，桌面（有实体键盘）默认不显示。
pub fn default_show_key_bar() -> bool {
    cfg!(any(target_os = "ios", target_os = "android"))
}

/// 设置里的滚回行数收窄到本机上界。
fn effective_scrollback(lines: usize) -> usize {
    lines.clamp(
        MIN_SCROLLBACK_LINES,
        scrollback_cap().max(MIN_SCROLLBACK_LINES),
    )
}

/// 默认终端配置（设置里指定的那份；找不到就用上游内置默认）。
pub fn default_profile(repository: &SqliteRepository) -> Result<TerminalProfile, StorageError> {
    let snapshot = repository.load_configuration()?;
    Ok(snapshot
        .profiles
        .into_iter()
        .find(|profile| profile.id == snapshot.settings.default_terminal_profile)
        .unwrap_or_else(TerminalProfile::p0_default))
}

/// 仅对启动前不存在的数据库，替换迁移种入的默认字体与字号；已有配置不按字体猜测来源。
pub fn adopt_app_defaults(
    repository: &SqliteRepository,
    fresh_catalog: bool,
) -> Result<(), String> {
    if !fresh_catalog {
        return Ok(());
    }
    let snapshot = repository
        .load_configuration()
        .map_err(|error| format!("load settings: {error:?}"))?;
    let mut profile = snapshot
        .profiles
        .into_iter()
        .find(|profile| profile.id == snapshot.settings.default_terminal_profile)
        .ok_or_else(|| "默认终端配置不存在。".to_owned())?;
    crate::profile_fonts::set_binding(
        &mut profile.settings,
        DEFAULT_TERMINAL_FONT,
        &crate::profile_fonts::binding(ProfileFontMode::InheritTerminalSlot, None),
    )
    .map_err(|_| "首次默认字体元数据不可编辑。".to_owned())?;
    profile.settings.font_size = DEFAULT_FONT_SIZE;
    match repository
        .commit_configuration(ConfigurationChange {
            expected_revision: snapshot.revision,
            settings: snapshot.settings,
            upsert_profiles: vec![profile],
            delete_profiles: vec![],
        })
        .map_err(|error| format!("save settings: {error:?}"))?
    {
        ConfigurationCommitOutcome::Applied { .. } => Ok(()),
        ConfigurationCommitOutcome::Conflict { .. } => Err("首次配置已被其他操作修改。".to_owned()),
    }
}

/// 新会话用的终端配置（仅移动端按本机上界收窄）。读不到就用上游内置默认
/// （不让会话因此失败）。
pub async fn terminal_settings(context: &Arc<AppContext>) -> TerminalSettingsV1 {
    let context = context.clone();
    let mut settings = match spawn_blocking(move || session_settings(&context.repository)).await {
        Ok(Ok(settings)) => settings,
        Ok(Err(error)) => {
            debug_print!("[settings] load: {error:?}");
            TerminalSettingsV1::default()
        }
        Err(error) => {
            debug_print!("[settings] load task: {error}");
            TerminalSettingsV1::default()
        }
    };
    if !desktop::available() {
        settings.scrollback_lines = effective_scrollback(settings.scrollback_lines);
    }
    settings
}

fn session_settings(repository: &SqliteRepository) -> Result<TerminalSettingsV1, StorageError> {
    if !desktop::available() {
        return default_profile(repository).map(|profile| profile.settings);
    }
    let snapshot = repository.load_configuration()?;
    let mut settings = snapshot
        .profiles
        .into_iter()
        .find(|profile| profile.id == snapshot.settings.default_terminal_profile)
        .unwrap_or_else(TerminalProfile::p0_default)
        .settings;
    settings.key_bindings = settings
        .resolve(&Default::default())
        .with_app_key_bindings(&snapshot.settings.key_bindings)
        .key_bindings;
    Ok(settings)
}

pub async fn run(context: Arc<AppContext>) {
    let query_rx = SettingsQuery::get_dart_signal_receiver();
    let save_rx = SaveSettings::get_dart_signal_receiver();
    let layout_rx = SaveKeyBarLayout::get_dart_signal_receiver();
    let appearance_query_rx = WindowsAppearanceQuery::get_dart_signal_receiver();
    let appearance_save_rx = SaveWindowsAppearance::get_dart_signal_receiver();
    let font_file_rx = FontFileRequest::get_dart_signal_receiver();
    let desktop_rx = DesktopSettingsRequest::get_dart_signal_receiver();
    let profile_ttf_rx = DesktopProfileTtfRequest::get_dart_signal_receiver();
    let profile_settings_rx = DesktopProfileSettingsRequest::get_dart_signal_receiver();
    let resource_rx = ProfileFontResourceRequest::get_dart_signal_receiver();
    loop {
        tokio::select! {
            pack = profile_settings_rx.recv() => {
                let Some(pack) = pack else { break };
                let request_id = pack.message.request_id.clone();
                let task_context = context.clone();
                let reply = spawn_blocking(move || {
                    let root = task_context.known_hosts.parent().unwrap_or_else(|| std::path::Path::new("."));
                    let captured_slot = task_context.preferences.get().terminal_font_file;
                    desktop::handle_profile(&task_context.repository, pack.message, root, captured_slot)
                }).await.unwrap_or_else(|_| desktop::task_failed(request_id));
                let applied = reply.status == "applied";
                reply.send_signal_to_dart();
                if !applied { continue; }
            }
            pack = resource_rx.recv() => {
                let Some(pack) = pack else { break };
                let request_id = pack.message.request_id.clone();
                let task_context = context.clone();
                let loaded = spawn_blocking(move || {
                    let root = task_context.known_hosts.parent().unwrap_or_else(|| std::path::Path::new("."));
                    crate::profile_fonts::load_resource(root, pack.message)
                }).await;
                let (state, bytes) = loaded.unwrap_or_else(|_| (ProfileFontResourceState {
                    request_id, resource: None, family: String::new(),
                    failure: Some(ProfileFontFailure::Unavailable),
                }, Vec::new()));
                state.send_signal_to_dart(bytes);
                continue;
            }
            pack = profile_ttf_rx.recv() => {
                let Some(pack) = pack else { break };
                let request_id = pack.message.request_id.clone();
                let task_context = context.clone();
                let reply = spawn_blocking(move || {
                    let root = task_context.known_hosts.parent().unwrap_or_else(|| std::path::Path::new("."));
                    let captured_slot = task_context.preferences.get().terminal_font_file;
                    desktop::handle_ttf(&task_context.repository, pack.message, pack.binary, root, captured_slot)
                }).await.unwrap_or_else(|_| desktop::task_failed(request_id));
                let applied = reply.status == "applied";
                reply.send_signal_to_dart();
                if !applied { continue; }
            }
            pack = font_file_rx.recv() => {
                let Some(pack) = pack else { break };
                let task_context = context.clone();
                let request_id = pack.message.request_id;
                let slot = pack.message.slot.clone();
                let result = spawn_blocking(move || {
                    let root = task_context.known_hosts.parent().unwrap_or_else(|| std::path::Path::new("."));
                    crate::fonts::handle(root, &task_context.preferences, pack.message, pack.binary)
                }).await;
                match result {
                    Ok((state, binary)) => state.send_signal_to_dart(binary),
                    Err(error) => FontFileState { request_id, slot, family: String::new(), label: String::new(), error: error.to_string(), applied: false }.send_signal_to_dart(Vec::new()),
                }
                continue;
            }
            pack = appearance_query_rx.recv() => {
                if pack.is_none() { break; }
                publish_appearance(&context, String::new());
                continue;
            }
            pack = appearance_save_rx.recv() => {
                let Some(pack) = pack else { break };
                let request = pack.message;
                let result = save_appearance(&context.preferences, request.acrylic, request.opacity);
                publish_appearance(&context, result.err().unwrap_or_default());
                continue;
            }
            pack = desktop_rx.recv() => {
                let Some(pack) = pack else { break };
                let request_id = pack.message.request_id.clone();
                let task_context = context.clone();
                let reply = spawn_blocking(move || {
                    let root = task_context.known_hosts.parent().unwrap_or_else(|| std::path::Path::new("."));
                    let captured_slot = task_context.preferences.get().terminal_font_file;
                    desktop::handle_with_resources(&task_context.repository, pack.message, root, captured_slot)
                })
                    .await.unwrap_or_else(|_| desktop::task_failed(request_id));
                let applied = reply.status == "applied";
                reply.send_signal_to_dart();
                if !applied { continue; }
            }
            pack = query_rx.recv() => {
                if pack.is_none() {
                    break;
                }
            }
            pack = layout_rx.recv() => {
                let Some(pack) = pack else { break };
                let saved = validate_key_bar_rows(&pack.message.rows).and_then(|()| {
                    context.preferences.update(|preferences| {
                        preferences.key_bar_rows = Some(pack.message.rows);
                        preferences.key_bar_layout_version = 1;
                    })
                });
                KeyBarLayoutResult {
                    ok: saved.is_ok(),
                    detail: saved.err().unwrap_or_default(),
                }.send_signal_to_dart();
            }
            pack = save_rx.recv() => {
                let Some(pack) = pack else { break };
                let request = pack.message;
                if let Some(show_key_bar) = request.show_key_bar
                    && let Err(error) = context.preferences.update(|preferences| {
                        preferences.show_key_bar = Some(show_key_bar);
                    }) {
                    debug_print!("[settings] preferences: {error}");
                }
                let task_context = context.clone();
                let saved = spawn_blocking(move || save(&task_context.repository, &request)).await;
                match saved {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => debug_print!("[settings] save: {error:?}"),
                    Err(error) => debug_print!("[settings] save task: {error}"),
                }
            }
        }
        publish(&context).await;
    }
}

/// 透明度只改变背景材质；限制范围保证终端及界面文字的可读性。
fn appearance_opacity(value: f64) -> Result<f64, String> {
    if !value.is_finite() {
        return Err("透明度必须是有限数值".to_owned());
    }
    Ok(value.clamp(0.35, 1.0))
}

fn save_appearance(
    preferences: &crate::keys::PreferenceFile,
    acrylic: bool,
    opacity: f64,
) -> Result<(), String> {
    let opacity = appearance_opacity(opacity)?;
    preferences.update(|value| {
        value.windows_acrylic = Some(acrylic);
        value.windows_opacity = Some(opacity);
    })
}

fn publish_appearance(context: &AppContext, error: String) {
    let preferences = context.preferences.get();
    WindowsAppearanceState {
        acrylic: preferences.windows_acrylic.unwrap_or(true),
        opacity: appearance_opacity(preferences.windows_opacity.unwrap_or(0.78)).unwrap_or(0.78),
        error,
    }
    .send_signal_to_dart();
}

/// 保存旧终端字段补丁；字体限于白名单，字号与滚回行数限制在允许范围内。
fn save(repository: &SqliteRepository, request: &SaveSettings) -> Result<(), StorageError> {
    save_replaying(repository, request, || {})
}

fn save_replaying(
    repository: &SqliteRepository,
    request: &SaveSettings,
    mut before_commit: impl FnMut(),
) -> Result<(), StorageError> {
    if request.font_family.is_none()
        && request.font_size.is_none()
        && request.scrollback_lines.is_none()
    {
        return Ok(());
    }
    // 旧请求没有 revision，只能在最新快照上重放其拥有的字段；不得重放整份旧 profile。
    for _ in 0..8 {
        let snapshot = repository.load_configuration()?;
        let mut profile = snapshot
            .profiles
            .iter()
            .find(|profile| profile.id == snapshot.settings.default_terminal_profile)
            .cloned()
            .ok_or(StorageError::Corrupt)?;
        patch_profile(&mut profile, request)?;
        before_commit();
        match repository.commit_configuration(ConfigurationChange {
            expected_revision: snapshot.revision,
            settings: snapshot.settings,
            upsert_profiles: vec![profile],
            delete_profiles: vec![],
        })? {
            ConfigurationCommitOutcome::Applied { .. } => return Ok(()),
            ConfigurationCommitOutcome::Conflict { .. } => continue,
        }
    }
    Err(StorageError::Busy)
}

fn patch_profile(
    profile: &mut TerminalProfile,
    request: &SaveSettings,
) -> Result<(), StorageError> {
    if let Some(family) = FONT_FAMILIES
        .iter()
        .find(|family| Some(**family) == request.font_family.as_deref())
    {
        // 每一轮重放都先保护未知元数据；合法且实际提供的 family 是明确写入意图。
        crate::profile_fonts::set_binding(
            &mut profile.settings,
            family,
            &crate::profile_fonts::binding(ProfileFontMode::ExplicitFamily, None),
        )
        .map_err(|_| StorageError::Constraint)?;
    }
    if let Some(size) = request.font_size
        && size.is_finite()
    {
        profile.settings.font_size = (size as f32).clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
    }
    if let Some(lines) = request.scrollback_lines {
        profile.settings.scrollback_lines =
            effective_scrollback(usize::try_from(lines).unwrap_or(usize::MAX));
    }
    Ok(())
}

async fn publish(context: &Arc<AppContext>) {
    let settings = terminal_settings(context).await;
    SettingsState {
        font_size: f64::from(settings.font_size),
        min_font_size: f64::from(MIN_FONT_SIZE),
        max_font_size: f64::from(MAX_FONT_SIZE),
        scrollback_lines: u32::try_from(effective_scrollback(settings.scrollback_lines))
            .unwrap_or(u32::MAX),
        max_scrollback_lines: u32::try_from(scrollback_cap()).unwrap_or(u32::MAX),
        key_bar_rows: {
            let preferences = context.preferences.get();
            effective_key_bar_rows(preferences.key_bar_rows, preferences.key_bar_layout_version)
        },
        show_key_bar: context
            .preferences
            .get()
            .show_key_bar
            .unwrap_or_else(default_show_key_bar),
    }
    .send_signal_to_dart();
}

/// 稳定标识与 Flutter 的按钮目录对应；自定义文本不包含控制字符。
const KEY_BAR_BUTTONS: &[&str] = &[
    "spacer",
    "escape",
    "tab",
    "up",
    "down",
    "left",
    "right",
    "ctrl",
    "alt",
    "keyboard",
    "backspace",
    "disconnect",
    "copy",
    "paste",
    "pipe",
    "slash",
    "minus",
    "tilde",
    "period",
    "enter",
    "delete",
    "home",
    "end",
    "pageUp",
    "pageDown",
    "f1",
    "f2",
    "f3",
    "f4",
    "f5",
    "f6",
    "f7",
    "f8",
    "f9",
    "f10",
    "f11",
    "f12",
    "ctrlC",
    "ctrlD",
    "ctrlZ",
    "ctrlL",
    "zoomIn",
    "zoomOut",
    "zoomReset",
];

pub fn default_key_bar_rows() -> Vec<Vec<String>> {
    [
        vec![
            "escape",
            "slash",
            "minus",
            "home",
            "up",
            "end",
            "keyboard",
            "backspace",
        ],
        vec![
            "tab", "ctrl", "alt", "left", "down", "right", "copy", "paste",
        ],
    ]
    .into_iter()
    .map(|row| row.into_iter().map(str::to_owned).collect())
    .collect()
}

/// 仅升级旧版本的原始默认排布；自定义排布以及新保存的配置保持原样。
fn effective_key_bar_rows(rows: Option<Vec<Vec<String>>>, version: u8) -> Vec<Vec<String>> {
    let Some(rows) = rows.filter(|rows| validate_key_bar_rows(rows).is_ok()) else {
        return default_key_bar_rows();
    };
    const LEGACY: [&[&str]; 2] = [
        &[
            "escape",
            "tab",
            "up",
            "down",
            "left",
            "right",
            "ctrl",
            "alt",
            "keyboard",
            "backspace",
        ],
        &[
            "disconnect",
            "copy",
            "paste",
            "pipe",
            "slash",
            "minus",
            "tilde",
            "period",
        ],
    ];
    if version == 0
        && rows
            .iter()
            .zip(LEGACY)
            .all(|(row, expected)| row.iter().map(String::as_str).eq(expected.iter().copied()))
    {
        default_key_bar_rows()
    } else {
        rows
    }
}

fn validate_key_bar_rows(rows: &[Vec<String>]) -> Result<(), String> {
    if rows.len() != 2 || rows.iter().any(|row| row.len() > 24) {
        return Err("键位条需要两排，每排最多 24 个按钮。".to_owned());
    }
    for button in rows.iter().flatten() {
        let custom_valid = button.strip_prefix("text:").is_some_and(|text| {
            !text.trim().is_empty()
                && text.chars().count() <= 32
                && !text.chars().any(char::is_control)
        });
        if !KEY_BAR_BUTTONS.contains(&button.as_str()) && !custom_valid {
            return Err("键位条含有未知按钮或无效文本。".to_owned());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use std::path::Path;

    use super::{
        DEFAULT_TERMINAL_FONT, FONT_FAMILIES, adopt_app_defaults, default_profile, save,
        scrollback_cap_for,
    };
    use crate::signals::settings::SaveSettings;
    use rshell_m0::rshell_core::ColorScheme;
    use rshell_m0::rshell_storage::SqliteRepository;

    #[test]
    fn windows_appearance_compatible_with_old_preferences() {
        let old: crate::keys::Preferences =
            serde_json::from_str(r#"{"sync_keys":true,"show_key_bar":false}"#).expect("旧偏好");
        assert!(old.sync_keys);
        assert_eq!(old.show_key_bar, Some(false));
        assert!(old.windows_acrylic.is_none());
        assert!(old.windows_opacity.is_none());
        assert_eq!(super::appearance_opacity(0.0).expect("下界"), 0.35);
        assert_eq!(super::appearance_opacity(2.0).expect("上界"), 1.0);
        assert!(super::appearance_opacity(f64::NAN).is_err());
        assert!(super::appearance_opacity(f64::INFINITY).is_err());
    }

    #[test]
    fn windows_appearance_survives_reopen_without_changing_other_preferences() {
        let path =
            std::env::temp_dir().join(format!("guosh-appearance-{}.json", uuid::Uuid::new_v4()));
        let preferences = crate::keys::PreferenceFile::open(path.clone());
        preferences
            .update(|value| {
                value.show_key_bar = Some(true);
                value.sync_keys = true;
            })
            .expect("其他偏好");
        super::save_appearance(&preferences, false, 0.6).expect("保存外观");
        assert!(super::save_appearance(&preferences, true, f64::NAN).is_err());
        let reopened = crate::keys::PreferenceFile::open(path).get();
        assert_eq!(reopened.windows_acrylic, Some(false));
        assert_eq!(reopened.windows_opacity, Some(0.6));
        assert_eq!(reopened.show_key_bar, Some(true));
        assert!(reopened.sync_keys);
    }

    #[test]
    fn key_bar_layout_survives_reopen_and_other_preferences() {
        use crate::keys::PreferenceFile;
        let directory = std::env::temp_dir().join(format!("guosh-layout-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("临时目录");
        let path = directory.join("preferences.json");
        std::fs::write(&path, r#"{"sync_keys":true,"show_key_bar":false}"#).expect("旧偏好");
        let preferences = PreferenceFile::open(path.clone());
        assert!(preferences.get().key_bar_rows.is_none());
        let rows = vec![vec!["f1".to_owned(), "text:ls -la".to_owned()], vec![]];
        super::validate_key_bar_rows(&rows).expect("有效排布");
        preferences
            .update(|p| p.key_bar_rows = Some(rows.clone()))
            .expect("保存");
        preferences
            .update(|p| p.show_key_bar = Some(true))
            .expect("其他设置");
        let loaded = PreferenceFile::open(path).get();
        assert!(loaded.sync_keys);
        assert_eq!(loaded.show_key_bar, Some(true));
        assert_eq!(loaded.key_bar_rows, Some(rows));
        std::fs::remove_dir_all(directory).expect("清理");
    }

    #[test]
    fn key_bar_default_upgrade_preserves_custom_and_newly_saved_layouts() {
        let old = vec![
            vec![
                "escape",
                "tab",
                "up",
                "down",
                "left",
                "right",
                "ctrl",
                "alt",
                "keyboard",
                "backspace",
            ],
            vec![
                "disconnect",
                "copy",
                "paste",
                "pipe",
                "slash",
                "minus",
                "tilde",
                "period",
            ],
        ]
        .into_iter()
        .map(|row| row.into_iter().map(str::to_owned).collect())
        .collect::<Vec<Vec<String>>>();
        assert_eq!(
            super::effective_key_bar_rows(Some(old.clone()), 0),
            super::default_key_bar_rows()
        );
        assert_eq!(super::effective_key_bar_rows(Some(old.clone()), 1), old);
        let custom = vec![vec!["f1".to_owned()], vec![]];
        assert_eq!(
            super::effective_key_bar_rows(Some(custom.clone()), 0),
            custom
        );
        let default = super::default_key_bar_rows();
        let up = default[0].iter().position(|id| id == "up").expect("上");
        assert_eq!(&default[1][up - 1..=up + 1], &["left", "down", "right"]);
    }

    #[test]
    fn key_bar_rejects_unknown_and_control_text_but_allows_empty_rows() {
        assert!(super::validate_key_bar_rows(&super::default_key_bar_rows()).is_ok());
        assert!(super::validate_key_bar_rows(&[vec![], vec![]]).is_ok());
        for id in [
            "unknown",
            "text:",
            "text: ",
            "text:cmd\n",
            "text:\u{1b}[31m",
        ] {
            assert!(super::validate_key_bar_rows(&[vec![id.to_owned()], vec![]]).is_err());
        }
        assert!(super::validate_key_bar_rows(&[vec!["tab".to_owned(); 25], vec![]]).is_err());
    }

    fn repository() -> SqliteRepository {
        let repository = SqliteRepository::open_in_memory().expect("in-memory catalog");
        repository.migrate().expect("migrate");
        repository
    }

    fn open_catalog(path: &Path) -> (SqliteRepository, bool) {
        let fresh = !path.try_exists().expect("检查目录文件");
        let repository = SqliteRepository::open(path).expect("打开目录");
        repository.migrate().expect("迁移目录");
        adopt_app_defaults(&repository, fresh).expect("应用首次默认设置");
        (repository, fresh)
    }

    #[test]
    fn new_catalog_gets_app_defaults_and_reopen_preserves_custom_profile() {
        let directory =
            std::env::temp_dir().join(format!("guosh-settings-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).expect("建立临时目录");
        let path = directory.join("catalog.sqlite3");

        let (repository, fresh) = open_catalog(&path);
        assert!(fresh);
        let mut profile = default_profile(&repository).expect("首次配置");
        assert_eq!(profile.settings.font_family, FONT_FAMILIES[0]);
        assert_eq!(profile.settings.font_size, 14.0);

        profile.name = "用户配置".to_owned();
        profile.settings.font_family = "自定义字体".to_owned();
        profile.settings.font_size = 21.0;
        profile.settings.scrollback_lines = 4_000;
        profile.settings.color_scheme = ColorScheme::Dracula;
        repository
            .save_terminal_profile(profile)
            .expect("保存用户配置");
        drop(repository);

        for _ in 0..2 {
            let (repository, fresh) = open_catalog(&path);
            assert!(!fresh);
            let profile = default_profile(&repository).expect("重启后的配置");
            assert_eq!(profile.name, "用户配置");
            assert_eq!(profile.settings.font_family, "自定义字体");
            assert_eq!(profile.settings.font_size, 21.0);
            assert_eq!(profile.settings.scrollback_lines, 4_000);
            assert_eq!(profile.settings.color_scheme, ColorScheme::Dracula);
        }
        std::fs::remove_dir_all(directory).expect("清理临时目录");
    }

    #[test]
    fn existing_uninitialized_catalog_is_not_assumed_new() {
        let directory =
            std::env::temp_dir().join(format!("guosh-settings-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).expect("建立临时目录");
        let path = directory.join("catalog.sqlite3");
        std::fs::File::create(&path).expect("已有但未初始化的目录文件");

        let (repository, fresh) = open_catalog(&path);
        assert!(!fresh);
        let profile = default_profile(&repository).expect("迁移后的上游配置");
        let upstream = rshell_m0::rshell_core::TerminalProfile::p0_default();
        assert_eq!(profile.settings.font_family, upstream.settings.font_family);
        assert_eq!(profile.settings.font_size, upstream.settings.font_size);
        drop(repository);
        std::fs::remove_dir_all(directory).expect("清理临时目录");
    }

    #[test]
    fn upstream_seed_is_replaced_by_the_app_defaults_once() {
        let repository = repository();
        adopt_app_defaults(&repository, true).expect("adopt defaults");
        let profile = default_profile(&repository).expect("profile");
        assert_eq!(profile.settings.font_family, DEFAULT_TERMINAL_FONT);
        assert_eq!(profile.settings.font_size, 14.0);

        let mut legacy = profile;
        legacy.settings.font_family = "Menlo".to_owned();
        repository.save_terminal_profile(legacy).expect("历史配置");
        save(
            &repository,
            &SaveSettings {
                font_family: None,
                font_size: Some(17.0),
                scrollback_lines: Some(3_000),
                show_key_bar: Some(true),
            },
        )
        .expect("save");
        adopt_app_defaults(&repository, false).expect("adopt defaults again");
        let profile = default_profile(&repository).expect("profile");
        assert_eq!(profile.settings.font_family, "Menlo");
        assert_eq!(profile.settings.font_size, 17.0);
        assert_eq!(profile.settings.scrollback_lines, 3_000);
    }

    #[test]
    fn existing_custom_font_and_size_survive_startup_defaults() {
        let repository = repository();
        let mut profile = default_profile(&repository).expect("迁移种入的配置");
        profile.name = "自定义配置".to_owned();
        profile.settings.font_family = "自定义字体".to_owned();
        profile.settings.font_size = 19.0;
        profile.settings.color_scheme = ColorScheme::Dracula;
        repository
            .save_terminal_profile(profile)
            .expect("保存自定义配置");

        adopt_app_defaults(&repository, false).expect("再次启动");
        let saved = default_profile(&repository).expect("读取已有配置");
        assert_eq!(saved.name, "自定义配置");
        assert_eq!(saved.settings.font_family, "自定义字体");
        assert_eq!(saved.settings.font_size, 19.0);
        assert_eq!(saved.settings.color_scheme, ColorScheme::Dracula);
    }

    #[test]
    fn terminal_sizes_are_clamped_without_changing_profile_font() {
        let repository = repository();
        adopt_app_defaults(&repository, true).expect("adopt defaults");
        save(
            &repository,
            &SaveSettings {
                font_family: None,
                font_size: Some(200.0),
                scrollback_lines: Some(10),
                show_key_bar: Some(false),
            },
        )
        .expect("save");
        let profile = default_profile(&repository).expect("profile");
        assert_eq!(profile.settings.font_family, DEFAULT_TERMINAL_FONT);
        assert_eq!(profile.settings.font_size, 32.0);
        assert_eq!(profile.settings.scrollback_lines, 1_000);
    }

    #[test]
    fn unknown_fonts_are_ignored_and_sizes_clamped() {
        let repository = repository();
        adopt_app_defaults(&repository, true).expect("应用默认设置");
        save(
            &repository,
            &SaveSettings {
                font_family: Some("Comic Sans".to_owned()),
                font_size: Some(200.0),
                scrollback_lines: Some(10),
                show_key_bar: Some(false),
            },
        )
        .expect("旧四字段补丁");
        let profile = default_profile(&repository).expect("配置");
        assert_eq!(profile.settings.font_family, DEFAULT_TERMINAL_FONT);
        assert_eq!(profile.settings.font_size, 32.0);
        assert_eq!(profile.settings.scrollback_lines, 1_000);
    }

    #[test]
    fn consecutive_partial_settings_preserve_preceding_edits() {
        let repository = repository();
        adopt_app_defaults(&repository, true).expect("应用默认设置");
        for request in [
            SaveSettings {
                font_family: Some("Menlo".to_owned()),
                ..SaveSettings::default()
            },
            SaveSettings {
                font_size: Some(22.0),
                ..SaveSettings::default()
            },
            SaveSettings {
                scrollback_lines: Some(2_000),
                ..SaveSettings::default()
            },
            SaveSettings {
                show_key_bar: Some(false),
                ..SaveSettings::default()
            },
        ] {
            save(&repository, &request).expect("按字段保存设置");
        }
        let settings = default_profile(&repository).expect("最终设置").settings;
        assert_eq!(settings.font_family, "Menlo");
        assert_eq!(settings.font_size, 22.0);
        assert_eq!(settings.scrollback_lines, 2_000);
    }

    #[test]
    fn legacy_patch_preserves_all_advanced_fields_and_invalidates_stale_desktop_revision() {
        use rshell_m0::rshell_storage::{ConfigurationChange, ConfigurationCommitOutcome};
        let repository = repository();
        let mut profile = default_profile(&repository).expect("读取默认配置");
        profile.settings.terminal_type = "vt220".into();
        profile.settings.initial_cols = 180;
        profile.settings.initial_rows = 55;
        profile.settings.color_scheme = ColorScheme::Nord;
        profile.settings.answerback = "自定义回答".into();
        profile.settings.left_alt_as_meta = false;
        profile.settings.right_alt_as_meta = false;
        profile.settings.enable_csi_u = true;
        profile.settings.enable_kitty_keyboard = true;
        profile.settings.mouse_reporting = false;
        profile.settings.scroll_on_output = false;
        profile.settings.scroll_on_keypress = true;
        repository
            .save_terminal_profile(profile.clone())
            .expect("高级配置");
        let before = repository.load_configuration().expect("桌面基准快照");
        save(
            &repository,
            &SaveSettings {
                font_size: Some(20.0),
                ..SaveSettings::default()
            },
        )
        .expect("旧字号补丁");
        profile.settings.font_size = 20.0;
        assert_eq!(default_profile(&repository).expect("更新后的配置"), profile);
        assert_eq!(
            repository
                .commit_configuration(ConfigurationChange {
                    expected_revision: before.revision,
                    settings: before.settings,
                    upsert_profiles: before.profiles,
                    delete_profiles: vec![],
                })
                .expect("拒绝过期桌面提交"),
            ConfigurationCommitOutcome::Conflict {
                actual_revision: before.revision + 1
            }
        );
        let before_keybar = repository.load_configuration().expect("键位条前");
        save(
            &repository,
            &SaveSettings {
                show_key_bar: Some(true),
                ..SaveSettings::default()
            },
        )
        .expect("键位条不改数据库");
        assert_eq!(
            repository.load_configuration().expect("键位条后"),
            before_keybar
        );
    }

    #[test]
    fn concurrent_legacy_patches_rebase_only_owned_fields() {
        use std::sync::{Arc, Barrier};
        let repository = Arc::new(repository());
        let before = repository.load_configuration().expect("初始快照");
        let barrier = Arc::new(Barrier::new(2));
        std::thread::scope(|scope| {
            for request in [
                SaveSettings {
                    font_size: Some(23.0),
                    ..SaveSettings::default()
                },
                SaveSettings {
                    font_family: Some("Menlo".into()),
                    ..SaveSettings::default()
                },
            ] {
                let repository = repository.clone();
                let barrier = barrier.clone();
                scope.spawn(move || {
                    barrier.wait();
                    save(&repository, &request).expect("并发字段补丁");
                });
            }
        });
        let after = repository.load_configuration().expect("最终快照");
        assert_eq!(after.revision, before.revision + 2);
        let profile = default_profile(&repository).expect("合并后的配置");
        assert_eq!(profile.settings.font_size, 23.0);
        assert_eq!(profile.settings.font_family, "Menlo");
        assert_eq!(after.settings, before.settings);
    }

    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    #[test]
    fn desktop_profile_changes_and_legacy_patch_preserve_both_font_slots() {
        use crate::keys::PreferenceFile;
        use crate::signals::settings::{DesktopSettingsRequest, FontFileRequest};

        const FONT: &[u8] = include_bytes!("../../../assets/fonts/MesloLGS-NF-Regular.ttf");
        let root =
            std::env::temp_dir().join(format!("guosh-profile-fonts-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).expect("字体测试目录");
        let path = root.join("preferences.json");
        let preferences = PreferenceFile::open(path.clone());
        super::save_appearance(&preferences, false, 0.6).expect("保存外观");
        let font_request = |slot: &str, action: &str| FontFileRequest {
            request_id: 1,
            slot: slot.into(),
            action: action.into(),
            file_name: "chosen.ttf".into(),
        };
        let mut families = Vec::new();
        for slot in ["interface", "terminal"] {
            let (state, _) = crate::fonts::handle(
                &root,
                &preferences,
                font_request(slot, "apply"),
                FONT.to_vec(),
            );
            assert!(state.applied);
            assert!(state.error.is_empty());
            families.push((slot, state.family));
        }
        let before = serde_json::to_value(preferences.get()).expect("原始字体与外观偏好");

        let repository = repository();
        adopt_app_defaults(&repository, true).expect("首次默认配置");
        let request = |operation: &str| DesktopSettingsRequest {
            protocol_version: super::desktop::PROTOCOL_VERSION,
            request_id: "字体槽位回归".into(),
            base_revision: repository.load_configuration().expect("最新版本").revision,
            operation: operation.into(),
            target_id: String::new(),
            name: String::new(),
            app: None,
            profile: None,
        };
        let mut create = request("create_profile");
        create.name = "新桌面配置".into();
        let created = super::desktop::handle(&repository, create);
        assert_eq!(created.status, "applied");
        let snapshot = created.snapshot.expect("新配置快照");
        let mut draft = snapshot
            .profiles
            .into_iter()
            .find(|profile| profile.id == created.target_id)
            .expect("新配置");
        assert_eq!(draft.font_family, DEFAULT_TERMINAL_FONT);
        draft.font_family = "自定义 profile 字体".into();
        draft.font_size = 25.0;
        let mut update = request("save_profile");
        update.target_id = created.target_id.clone();
        update.profile = Some(draft);
        assert_eq!(
            super::desktop::handle_profile(
                &repository,
                crate::signals::settings::DesktopProfileSettingsRequest {
                    protocol_version: update.protocol_version,
                    request_id: update.request_id,
                    base_revision: update.base_revision,
                    target_id: update.target_id,
                    profile: update.profile.expect("完整草稿"),
                    font_binding: crate::profile_fonts::binding(
                        super::ProfileFontMode::ExplicitFamily,
                        None
                    ),
                },
                &root,
                preferences.get().terminal_font_file
            )
            .status,
            "applied"
        );
        let mut app = snapshot.app;
        app.default_terminal_profile = created.target_id;
        let mut switch = request("save_app");
        switch.app = Some(app);
        assert_eq!(
            super::desktop::handle(&repository, switch).status,
            "applied"
        );
        save(
            &repository,
            &SaveSettings {
                font_family: Some("Menlo".into()),
                font_size: Some(20.0),
                scrollback_lines: Some(2_000),
                show_key_bar: None,
            },
        )
        .expect("旧字体字段补丁");
        assert_eq!(
            default_profile(&repository)
                .expect("更新默认配置")
                .settings
                .font_family,
            "Menlo"
        );

        let reopened = PreferenceFile::open(path);
        assert_eq!(
            serde_json::to_value(reopened.get()).expect("重开偏好"),
            before,
            "profile 与旧补丁不能改动字体槽位或外观"
        );
        for (slot, family) in families {
            let (state, bytes) =
                crate::fonts::handle(&root, &reopened, font_request(slot, "query"), vec![]);
            assert!(state.applied);
            assert!(state.error.is_empty());
            assert_eq!(state.family, family);
            assert_eq!(bytes, FONT);
        }
        std::fs::remove_dir_all(root).expect("清理字体测试目录");
    }

    #[tokio::test]
    async fn desktop_request_uses_real_rinf_decoder_and_transaction_handler() {
        use crate::signals::settings::{DesktopSettingsRequest, inject_desktop_request};
        use rinf::DartSignal;
        let repository = repository();
        let initial = repository.load_configuration().expect("初始快照");
        let receiver = DesktopSettingsRequest::get_dart_signal_receiver();
        let request = DesktopSettingsRequest {
            protocol_version: super::desktop::PROTOCOL_VERSION,
            request_id: "配置桥接测试".into(),
            base_revision: initial.revision,
            operation: "create_profile".into(),
            target_id: String::new(),
            name: "桥接创建".into(),
            app: None,
            profile: None,
        };
        inject_desktop_request(&rinf::serialize(&request).expect("真实 rinf 编码"));
        let received = tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv())
            .await
            .expect("收到请求")
            .expect("信号通道未关闭");
        let result = super::desktop::handle(&repository, received.message);
        assert_eq!(result.request_id, request.request_id);
        if super::desktop::available() {
            assert_eq!(result.status, "applied");
            assert_eq!(result.revision, Some(initial.revision + 1));
            assert!(
                repository
                    .load_configuration()
                    .expect("实际落库")
                    .profiles
                    .iter()
                    .any(|profile| profile.id.0.to_string() == result.target_id
                        && profile.name == "桥接创建")
            );
        } else {
            assert_eq!(result.status, "unsupported_platform");
            assert_eq!(repository.load_configuration().expect("拒绝写入"), initial);
        }
        assert!(
            !rinf::serialize(&result)
                .expect("完整结果使用 rinf 编码")
                .is_empty()
        );
    }

    #[test]
    fn session_settings_merge_desktop_bindings_without_changing_stored_profile() {
        use rshell_m0::rshell_core::{KeyBinding, KeyCode, KeyModifiers};
        let repository = repository();
        let binding = |ch, action: &str| KeyBinding {
            code: KeyCode::Character(ch),
            modifiers: KeyModifiers {
                control: true,
                ..KeyModifiers::default()
            },
            action: action.into(),
        };
        let mut snapshot = repository.load_configuration().expect("读取配置");
        snapshot.settings.key_bindings =
            vec![binding('t', "new_tab"), binding('s', "split_vertical")];
        let mut profile = snapshot.profiles.remove(0);
        profile.settings.key_bindings = vec![binding('t', "clear_scrollback")];
        profile.settings.scrollback_lines = 900_000;
        repository
            .save_terminal_profile(profile.clone())
            .expect("配置绑定");
        repository
            .save_settings(snapshot.settings)
            .expect("应用绑定");
        let settings = super::session_settings(&repository).expect("新会话参数");
        let expected = if super::desktop::available() {
            vec![
                binding('t', "clear_scrollback"),
                binding('s', "split_vertical"),
            ]
        } else {
            profile.settings.key_bindings.clone()
        };
        assert_eq!(settings.key_bindings, expected);
        assert_eq!(settings.scrollback_lines, 900_000);
        assert_eq!(
            default_profile(&repository).expect("持久配置不写回合并结果"),
            profile
        );
    }

    #[test]
    fn scrollback_caps_follow_physical_memory() {
        const GIB: u64 = 1 << 30;
        assert_eq!(scrollback_cap_for(4 * GIB), 2_000);
        assert_eq!(scrollback_cap_for(6 * GIB), 5_000);
        assert_eq!(scrollback_cap_for(8 * GIB), 5_000);
        assert_eq!(scrollback_cap_for(16 * GIB), 20_000);
        assert_eq!(scrollback_cap_for(64 * GIB), 100_000);
        assert_eq!(scrollback_cap_for(0), 5_000);
    }

    #[test]
    fn fresh_defaults_record_inheritance_even_when_seed_family_is_already_whitelisted() {
        let repository = repository();
        let mut profile = default_profile(&repository).expect("上游种入");
        profile.settings.font_family = "Menlo".into();
        repository
            .save_terminal_profile(profile)
            .expect("受控白名单种入");
        let before = repository.load_configuration().expect("首次 CAS 前");
        adopt_app_defaults(&repository, true).expect("真正新库");
        let after = repository.load_configuration().expect("首次 CAS 后");
        assert_eq!(after.revision, before.revision + 1);
        let profile = default_profile(&repository).expect("首次 profile");
        assert_eq!(profile.settings.font_family, DEFAULT_TERMINAL_FONT);
        assert_eq!(
            crate::profile_fonts::saved_binding(&profile.settings).mode,
            super::ProfileFontMode::InheritTerminalSlot
        );
        adopt_app_defaults(&repository, false).expect("存量不补写");
        assert_eq!(repository.load_configuration().expect("无重复提交"), after);
    }

    #[test]
    fn legacy_family_none_preserves_absence_and_same_legal_family_is_explicit_intent() {
        let repository = repository();
        let mut profile = default_profile(&repository).expect("历史 profile");
        profile.settings.font_family = "Menlo".into();
        profile
            .settings
            .extensions
            .insert("other.app".into(), serde_json::json!({"keep": [1, null]}));
        repository
            .save_terminal_profile(profile)
            .expect("历史 raw family");
        save(
            &repository,
            &SaveSettings {
                font_size: Some(20.0),
                ..SaveSettings::default()
            },
        )
        .expect("非字体 patch");
        let after_nonfont = default_profile(&repository).expect("未补模式");
        assert_eq!(
            crate::profile_fonts::saved_binding(&after_nonfont.settings).mode,
            super::ProfileFontMode::LegacyUndetermined
        );
        save(
            &repository,
            &SaveSettings {
                font_family: Some("invalid".into()),
                ..SaveSettings::default()
            },
        )
        .expect("非法 family 仍忽略");
        assert_eq!(
            default_profile(&repository).expect("模式不变"),
            after_nonfont
        );
        save(
            &repository,
            &SaveSettings {
                font_family: Some("Menlo".into()),
                ..SaveSettings::default()
            },
        )
        .expect("相同字符串的明确选择");
        let explicit = default_profile(&repository).expect("显式 family");
        assert_eq!(
            crate::profile_fonts::saved_binding(&explicit.settings).mode,
            super::ProfileFontMode::ExplicitFamily
        );
        assert_eq!(
            explicit.settings.extensions["other.app"],
            after_nonfont.settings.extensions["other.app"]
        );
    }

    #[test]
    fn legacy_font_patch_rejects_unknown_payload_atomically_but_nonfont_preserves_it() {
        for payload in [
            serde_json::json!({"version": 9, "future": [1, null]}),
            serde_json::json!("broken"),
        ] {
            let repository = repository();
            let mut profile = default_profile(&repository).expect("旧配置");
            profile.settings.font_family = "Menlo".into();
            profile
                .settings
                .extensions
                .insert(crate::profile_fonts::NAMESPACE.into(), payload.clone());
            profile
                .settings
                .extensions
                .insert("other.app".into(), serde_json::json!({"keep": true}));
            repository
                .save_terminal_profile(profile)
                .expect("未知元数据");
            let before = repository.load_configuration().expect("配置原值");
            assert!(matches!(
                save(
                    &repository,
                    &SaveSettings {
                        font_family: Some("Menlo".into()),
                        font_size: Some(24.0),
                        scrollback_lines: Some(3_000),
                        show_key_bar: Some(true),
                    }
                ),
                Err(rshell_m0::rshell_storage::StorageError::Constraint)
            ));
            assert_eq!(repository.load_configuration().expect("不部分提交"), before);
            save(
                &repository,
                &SaveSettings {
                    font_size: Some(24.0),
                    ..SaveSettings::default()
                },
            )
            .expect("非字体可改");
            let profile = default_profile(&repository).expect("保全扩展");
            assert_eq!(
                profile.settings.extensions[crate::profile_fonts::NAMESPACE],
                payload
            );
            assert_eq!(profile.settings.font_size, 24.0);
            assert_eq!(profile.settings.font_family, "Menlo");
        }
    }

    #[test]
    fn legacy_cas_replay_rechecks_unknown_metadata_and_stops_after_eight_conflicts() {
        let repository = repository();
        let before = repository.load_configuration().expect("基准");
        let mut rounds = 0;
        let result = super::save_replaying(
            &repository,
            &SaveSettings {
                font_family: Some("Menlo".into()),
                font_size: Some(24.0),
                ..SaveSettings::default()
            },
            || {
                rounds += 1;
                let mut profile = default_profile(&repository).expect("并发读取");
                profile.settings.extensions.insert(
                    crate::profile_fonts::NAMESPACE.into(),
                    serde_json::json!({"version": 9}),
                );
                repository
                    .save_terminal_profile(profile)
                    .expect("并发未知载荷");
            },
        );
        assert!(matches!(
            result,
            Err(rshell_m0::rshell_storage::StorageError::Constraint)
        ));
        assert_eq!(rounds, 1, "第二轮在提交前重新保护未知版本");
        let after = repository.load_configuration().expect("仅并发操作提交");
        assert_eq!(after.revision, before.revision + 1);
        assert_eq!(
            default_profile(&repository)
                .expect("非字体字段没部分写入")
                .settings
                .font_size,
            before.profiles[0].settings.font_size
        );

        let repository = self::repository();
        let mut rounds = 0;
        let result = super::save_replaying(
            &repository,
            &SaveSettings {
                font_size: Some(24.0),
                ..SaveSettings::default()
            },
            || {
                rounds += 1;
                let mut settings = repository
                    .load_configuration()
                    .expect("最新应用设置")
                    .settings;
                settings.color_scheme = ColorScheme::Nord;
                repository
                    .save_settings(settings)
                    .expect("每轮制造真实 CAS 冲突");
            },
        );
        assert!(matches!(
            result,
            Err(rshell_m0::rshell_storage::StorageError::Busy)
        ));
        assert_eq!(rounds, 8);
        assert_ne!(
            default_profile(&repository)
                .expect("未保存字号")
                .settings
                .font_size,
            24.0
        );
    }

    #[tokio::test]
    async fn typed_profile_font_request_uses_real_rinf_decoder() {
        use crate::signals::settings::{DesktopProfileSettingsRequest, DesktopSettingsRequest};
        use rinf::DartSignal;
        let repository = repository();
        let snapshot = repository.load_configuration().expect("原配置");
        let queried = super::desktop::handle(
            &repository,
            DesktopSettingsRequest {
                protocol_version: super::desktop::PROTOCOL_VERSION,
                request_id: "查询字体协议".into(),
                base_revision: 0,
                operation: "query".into(),
                target_id: String::new(),
                name: String::new(),
                app: None,
                profile: None,
            },
        );
        if !super::desktop::available() {
            assert_eq!(queried.status, "unsupported_platform");
            return;
        }
        let mut profile = queried.snapshot.expect("profile 投影").profiles.remove(0);
        profile.font_family = "Menlo".into();
        profile.font_size = 24.0;
        let request = DesktopProfileSettingsRequest {
            protocol_version: super::desktop::PROTOCOL_VERSION,
            request_id: "明确绑定协议".into(),
            base_revision: snapshot.revision,
            target_id: profile.id.clone(),
            profile,
            font_binding: crate::profile_fonts::binding(
                super::ProfileFontMode::ExplicitFamily,
                None,
            ),
        };
        let receiver = DesktopProfileSettingsRequest::get_dart_signal_receiver();
        crate::signals::settings::inject_profile_settings_request(
            &rinf::serialize(&request).expect("新协议编码"),
        );
        let received = tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv())
            .await
            .expect("收到 typed 请求")
            .expect("信号通道")
            .message;
        assert_eq!(received.font_binding, request.font_binding);
        let reply = super::desktop::handle_profile(&repository, received, Path::new("."), None);
        assert_eq!(reply.status, "applied");
        let stored = default_profile(&repository).expect("已提交");
        assert_eq!(stored.settings.font_family, "Menlo");
        assert_eq!(stored.settings.font_size, 24.0);
        assert_eq!(
            crate::profile_fonts::saved_binding(&stored.settings),
            request.font_binding
        );
    }
}
