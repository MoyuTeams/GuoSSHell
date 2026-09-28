//! 用户字体的校验、持久化与缓存标识。预览请求不会写入偏好或磁盘。
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::keys::PreferenceFile;
use crate::signals::settings::{FontFileRequest, FontFileState};

const MAX_FONT_BYTES: usize = 32 * 1024 * 1024;

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

fn font_path(root: &Path, hash: &str) -> Result<PathBuf, String> {
    if hash.len() != 64 || !hash.bytes().all(|value| value.is_ascii_hexdigit()) {
        return Err("字体缓存标识无效".to_owned());
    }
    Ok(root.join("fonts").join(format!("{hash}.ttf")))
}

fn inspect(bytes: &[u8], file_name: &str) -> Result<ImportedFont, String> {
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
    root: &Path,
    preferences: &PreferenceFile,
    slot: &str,
    font: Option<ImportedFont>,
) -> Result<(), String> {
    let old = selected(preferences, slot);
    preferences.update(|value| {
        if slot == "terminal" {
            value.terminal_font_file = font;
        } else {
            value.interface_font_file = font;
        }
    })?;
    // 只清理本应用复制且不再被任一用途引用的字体，绝不触碰用户选择的原文件。
    if let Some(old) = old {
        let value = preferences.get();
        let referenced = [value.interface_font_file, value.terminal_font_file]
            .iter()
            .flatten()
            .any(|font| font.hash == old.hash);
        if !referenced && let Ok(path) = font_path(root, &old.hash) {
            let _ = std::fs::remove_file(path);
        }
    }
    Ok(())
}

fn apply(
    root: &Path,
    preferences: &PreferenceFile,
    slot: &str,
    font: ImportedFont,
    bytes: &[u8],
) -> Result<(), String> {
    let path = font_path(root, &font.hash)?;
    let exists = path.exists();
    if exists {
        if std::fs::read(&path).map_err(|e| e.to_string())? != bytes {
            return Err("字体缓存校验失败".to_owned());
        }
    } else {
        let directory = root.join("fonts");
        std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let temporary = directory.join(format!("{}.tmp", uuid::Uuid::new_v4()));
        let written = (|| -> std::io::Result<()> {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            std::fs::rename(&temporary, &path)
        })();
        if let Err(error) = written {
            let _ = std::fs::remove_file(&temporary);
            return Err(error.to_string());
        }
    }
    if let Err(error) = replace(root, preferences, slot, Some(font)) {
        if !exists {
            let _ = std::fs::remove_file(path);
        }
        return Err(error);
    }
    Ok(())
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
                replace(root, preferences, &request.slot, None)?;
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
    const FONT: &[u8] = include_bytes!("../../../assets/fonts/MesloLGS-NF-Regular.ttf");

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
    fn files_survive_reopen_and_shared_font_is_retained_until_both_slots_reset() {
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
        assert!(!path.exists());
        assert_eq!(std::fs::read(source).expect("原文件保留"), FONT);
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
}
