//! 终端字号与滚回设置、窗口外观，以及字体文件请求的分发。

use std::sync::Arc;

use rinf::{DartSignal, DartSignalBinary, RustSignal, RustSignalBinary, debug_print};
use rshell_m0::rshell_core::{TerminalProfile, TerminalSettingsV1};
use rshell_m0::rshell_storage::{SqliteRepository, StorageError};
use tokio::task::spawn_blocking;

use crate::app::AppContext;
use crate::signals::settings::{FontFileRequest, FontFileState};
use crate::signals::settings::{
    KeyBarLayoutResult, SaveKeyBarLayout, SaveSettings, SaveWindowsAppearance, SettingsQuery,
    SettingsState, WindowsAppearanceQuery, WindowsAppearanceState,
};

pub const DEFAULT_TERMINAL_FONT: &str = "MesloLGS NF";
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
    let settings = repository.load_settings()?;
    Ok(repository
        .load_terminal_profiles()?
        .into_iter()
        .find(|profile| profile.id == settings.default_terminal_profile)
        .unwrap_or_else(TerminalProfile::p0_default))
}

/// 识别本应用的历史配置，避免启动时重置已有字号；字库选择由 fonts 模块管理。
pub fn adopt_app_defaults(repository: &SqliteRepository) -> Result<(), String> {
    let mut profile =
        default_profile(repository).map_err(|error| format!("load settings: {error:?}"))?;
    if matches!(
        profile.settings.font_family.as_str(),
        DEFAULT_TERMINAL_FONT | "Menlo"
    ) {
        return Ok(());
    }
    profile.settings.font_family = DEFAULT_TERMINAL_FONT.to_owned();
    profile.settings.font_size = DEFAULT_FONT_SIZE;
    repository
        .save_terminal_profile(profile)
        .map_err(|error| format!("save settings: {error:?}"))
}

/// 新会话用的终端配置（滚回行数已按本机上界收窄）。读不到就用上游内置默认
/// （不让会话因此失败）。
pub async fn terminal_settings(context: &Arc<AppContext>) -> TerminalSettingsV1 {
    let context = context.clone();
    let mut settings = match spawn_blocking(move || default_profile(&context.repository)).await {
        Ok(Ok(profile)) => profile.settings,
        Ok(Err(error)) => {
            debug_print!("[settings] load: {error:?}");
            TerminalSettingsV1::default()
        }
        Err(error) => {
            debug_print!("[settings] load task: {error}");
            TerminalSettingsV1::default()
        }
    };
    settings.scrollback_lines = effective_scrollback(settings.scrollback_lines);
    settings
}

pub async fn run(context: Arc<AppContext>) {
    let query_rx = SettingsQuery::get_dart_signal_receiver();
    let save_rx = SaveSettings::get_dart_signal_receiver();
    let layout_rx = SaveKeyBarLayout::get_dart_signal_receiver();
    let appearance_query_rx = WindowsAppearanceQuery::get_dart_signal_receiver();
    let appearance_save_rx = SaveWindowsAppearance::get_dart_signal_receiver();
    let font_file_rx = FontFileRequest::get_dart_signal_receiver();
    loop {
        tokio::select! {
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

/// 保存终端参数，字号与滚回行数限制在允许范围内。
fn save(repository: &SqliteRepository, request: &SaveSettings) -> Result<(), StorageError> {
    let mut profile = default_profile(repository)?;
    if let Some(size) = request.font_size
        && size.is_finite()
    {
        profile.settings.font_size = (size as f32).clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
    }
    if let Some(lines) = request.scrollback_lines {
        profile.settings.scrollback_lines =
            effective_scrollback(usize::try_from(lines).unwrap_or(usize::MAX));
    }
    repository.save_terminal_profile(profile)
}

async fn publish(context: &Arc<AppContext>) {
    let settings = terminal_settings(context).await;
    SettingsState {
        font_size: f64::from(settings.font_size),
        min_font_size: f64::from(MIN_FONT_SIZE),
        max_font_size: f64::from(MAX_FONT_SIZE),
        scrollback_lines: u32::try_from(settings.scrollback_lines).unwrap_or(u32::MAX),
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
    use super::{
        DEFAULT_TERMINAL_FONT, adopt_app_defaults, default_profile, save, scrollback_cap_for,
    };
    use crate::signals::settings::SaveSettings;
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

    #[test]
    fn upstream_seed_is_replaced_by_the_app_defaults_once() {
        let repository = repository();
        adopt_app_defaults(&repository).expect("adopt defaults");
        let profile = default_profile(&repository).expect("profile");
        assert_eq!(profile.settings.font_family, DEFAULT_TERMINAL_FONT);
        assert_eq!(profile.settings.font_size, 14.0);

        let mut legacy = profile;
        legacy.settings.font_family = "Menlo".to_owned();
        repository.save_terminal_profile(legacy).expect("历史配置");
        save(
            &repository,
            &SaveSettings {
                font_size: Some(17.0),
                scrollback_lines: Some(3_000),
                show_key_bar: Some(true),
            },
        )
        .expect("save");
        adopt_app_defaults(&repository).expect("adopt defaults again");
        let profile = default_profile(&repository).expect("profile");
        assert_eq!(profile.settings.font_family, "Menlo");
        assert_eq!(profile.settings.font_size, 17.0);
        assert_eq!(profile.settings.scrollback_lines, 3_000);
    }

    #[test]
    fn terminal_sizes_are_clamped_without_changing_profile_font() {
        let repository = repository();
        adopt_app_defaults(&repository).expect("adopt defaults");
        save(
            &repository,
            &SaveSettings {
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
    fn consecutive_partial_settings_preserve_preceding_edits() {
        let repository = repository();
        adopt_app_defaults(&repository).expect("应用默认设置");
        for request in [
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
        assert_eq!(settings.font_family, DEFAULT_TERMINAL_FONT);
        assert_eq!(settings.font_size, 22.0);
        assert_eq!(settings.scrollback_lines, 2_000);
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
}
