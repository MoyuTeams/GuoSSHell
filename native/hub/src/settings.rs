//! 设置：上游默认 `TerminalProfile` 里的字体、字号与滚回行数（引擎也从这份配置取
//! 终端参数）。滚回行数另有按本机物理内存分档的上界。

use std::sync::Arc;

use rinf::{DartSignal, RustSignal, debug_print};
use rshell_m0::rshell_core::{TerminalProfile, TerminalSettingsV1};
use rshell_m0::rshell_storage::{SqliteRepository, StorageError};
use tokio::task::spawn_blocking;

use crate::app::AppContext;
use crate::signals::settings::{
    KeyBarLayoutResult, SaveKeyBarLayout, SaveSettings, SettingsQuery, SettingsState,
};

/// 可选字体：第一个随 App 内置（带 powerline / Nerd Font 字形），其余是系统自带。
pub const FONT_FAMILIES: [&str; 2] = ["MesloLGS NF", "Menlo"];
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

/// 默认配置里的字体不是本 App 提供的（上游迁移种下的默认值就是这样）时，
/// 换成本 App 的默认字体与字号。
pub fn adopt_app_defaults(repository: &SqliteRepository) -> Result<(), String> {
    let mut profile =
        default_profile(repository).map_err(|error| format!("load settings: {error:?}"))?;
    if FONT_FAMILIES.contains(&profile.settings.font_family.as_str()) {
        return Ok(());
    }
    profile.settings.font_family = FONT_FAMILIES[0].to_owned();
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
    loop {
        tokio::select! {
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

/// 存设置。字体必须是可选字体之一（否则不改），字号与滚回行数夹到允许范围。
fn save(repository: &SqliteRepository, request: &SaveSettings) -> Result<(), StorageError> {
    let mut profile = default_profile(repository)?;
    if let Some(family) = FONT_FAMILIES
        .iter()
        .find(|family| Some(**family) == request.font_family.as_deref())
    {
        profile.settings.font_family = (*family).to_owned();
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
    repository.save_terminal_profile(profile)
}

async fn publish(context: &Arc<AppContext>) {
    let settings = terminal_settings(context).await;
    SettingsState {
        font_family: settings.font_family,
        font_size: f64::from(settings.font_size),
        font_families: FONT_FAMILIES
            .iter()
            .map(|family| (*family).to_owned())
            .collect(),
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
    use super::{FONT_FAMILIES, adopt_app_defaults, default_profile, save, scrollback_cap_for};
    use crate::signals::settings::SaveSettings;
    use rshell_m0::rshell_storage::SqliteRepository;

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
        assert_eq!(profile.settings.font_family, FONT_FAMILIES[0]);
        assert_eq!(profile.settings.font_size, 14.0);

        save(
            &repository,
            &SaveSettings {
                font_family: Some("Menlo".to_owned()),
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
    fn unknown_fonts_are_ignored_and_sizes_clamped() {
        let repository = repository();
        adopt_app_defaults(&repository).expect("adopt defaults");
        save(
            &repository,
            &SaveSettings {
                font_family: Some("Comic Sans".to_owned()),
                font_size: Some(200.0),
                scrollback_lines: Some(10),
                show_key_bar: Some(false),
            },
        )
        .expect("save");
        let profile = default_profile(&repository).expect("profile");
        assert_eq!(profile.settings.font_family, FONT_FAMILIES[0]);
        assert_eq!(profile.settings.font_size, 32.0);
        assert_eq!(profile.settings.scrollback_lines, 1_000);
    }

    #[test]
    fn consecutive_partial_settings_preserve_preceding_edits() {
        let repository = repository();
        adopt_app_defaults(&repository).expect("应用默认设置");
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
