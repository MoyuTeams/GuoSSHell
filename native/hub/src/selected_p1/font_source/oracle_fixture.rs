//! 仅供 integration 链接原 fonts/font_resources/profile_fonts 的宿主 fixture。
//! 偏好只访问测试提供的路径；生产 Source 不调用这些偏好/全配置包装。

pub mod keys {
    use crate::fonts::ImportedFont;
    use serde::{Deserialize, Serialize};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default, Serialize, Deserialize)]
    pub struct Preferences {
        pub interface_font_file: Option<ImportedFont>,
        pub terminal_font_file: Option<ImportedFont>,
        pub show_key_bar: Option<bool>,
        pub windows_opacity: Option<f64>,
    }

    #[derive(Clone)]
    pub struct PreferenceFile {
        path: PathBuf,
        value: Arc<Mutex<Preferences>>,
    }

    impl PreferenceFile {
        pub fn open(path: PathBuf) -> Self {
            let value = std::fs::read(&path)
                .ok()
                .and_then(|data| serde_json::from_slice(&data).ok())
                .unwrap_or_default();
            Self {
                path,
                value: Arc::new(Mutex::new(value)),
            }
        }
        pub fn get(&self) -> Preferences {
            self.value
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
        pub fn update(&self, change: impl FnOnce(&mut Preferences)) -> Result<(), String> {
            let mut current = self
                .value
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut next = current.clone();
            change(&mut next);
            let encoded = serde_json::to_vec(&next).map_err(|error| error.to_string())?;
            std::fs::write(&self.path, encoded).map_err(|error| error.to_string())?;
            *current = next;
            Ok(())
        }
    }
}

pub mod settings {
    use rshell_m0::rshell_core::TerminalProfile;
    use rshell_m0::rshell_storage::{
        ConfigurationChange, ConfigurationCommitOutcome, SqliteRepository, StorageError,
    };

    pub const DEFAULT_TERMINAL_FONT: &str = "MesloLGS NF";
    pub fn desktop_available() -> bool {
        cfg!(any(
            target_os = "windows",
            target_os = "macos",
            target_os = "linux"
        ))
    }
    pub fn default_profile(repository: &SqliteRepository) -> Result<TerminalProfile, StorageError> {
        let snapshot = repository.load_configuration()?;
        Ok(snapshot
            .profiles
            .into_iter()
            .find(|p| p.id == snapshot.settings.default_terminal_profile)
            .unwrap_or_else(TerminalProfile::p0_default))
    }
    pub fn adopt_app_defaults(repository: &SqliteRepository, fresh: bool) -> Result<(), String> {
        if !fresh {
            return Ok(());
        }
        let snapshot = repository.load_configuration().map_err(|e| e.to_string())?;
        let mut profile = snapshot
            .profiles
            .into_iter()
            .find(|p| p.id == snapshot.settings.default_terminal_profile)
            .ok_or("默认配置缺失")?;
        crate::profile_fonts::set_binding(
            &mut profile.settings,
            DEFAULT_TERMINAL_FONT,
            &crate::profile_fonts::binding(
                crate::signals::settings::ProfileFontMode::InheritTerminalSlot,
                None,
            ),
        )
        .map_err(str::to_owned)?;
        profile.settings.font_size = 14.0;
        match repository
            .commit_configuration(ConfigurationChange {
                expected_revision: snapshot.revision,
                settings: snapshot.settings,
                upsert_profiles: vec![profile],
                delete_profiles: vec![],
            })
            .map_err(|e| e.to_string())?
        {
            ConfigurationCommitOutcome::Applied { .. } => Ok(()),
            ConfigurationCommitOutcome::Conflict { .. } => Err("配置冲突".into()),
        }
    }
}

/// oracle 的纯值宿主，不创建 Rinf queue/ingress；此类型集不提供 V1 ABI 证据。
pub mod settings_signals {
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Serialize, Deserialize)]
    pub struct FontFileRequest {
        pub request_id: u32,
        pub slot: String,
        pub action: String,
        pub file_name: String,
    }
    #[derive(Serialize)]
    pub struct FontFileState {
        pub request_id: u32,
        pub slot: String,
        pub family: String,
        pub label: String,
        pub error: String,
        pub applied: bool,
    }
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct DesktopKeyBinding {
        pub key: String,
        pub shift: bool,
        pub control: bool,
        pub alt: bool,
        pub super_key: bool,
        pub action: String,
    }
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct DesktopTerminalProfile {
        pub id: String,
        pub name: String,
        pub version: u32,
        pub terminal_type: String,
        pub initial_cols: u32,
        pub initial_rows: u32,
        pub scrollback_lines: u32,
        pub font_family: String,
        pub font_size: f64,
        pub color_scheme: String,
        pub key_bindings: Vec<DesktopKeyBinding>,
        pub left_alt_as_meta: bool,
        pub right_alt_as_meta: bool,
        pub enable_csi_u: bool,
        pub enable_kitty_keyboard: bool,
        pub mouse_reporting: bool,
        pub scroll_on_output: bool,
        pub scroll_on_keypress: bool,
        pub answerback: String,
    }
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    pub enum ProfileFontMode {
        InheritTerminalSlot,
        ExplicitFamily,
        ExplicitCachedTtf,
        LegacyUndetermined,
        Uneditable,
    }
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    pub enum ProfileFontMetadataError {
        UnsupportedVersion,
        InvalidMetadata,
    }
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    pub enum ProfileFontRuntimeSource {
        ProfileBinding,
        LegacyTerminalSlot,
        Uneditable,
    }
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    pub enum ProfileFontFailure {
        Missing,
        Corrupt,
        Unavailable,
        InvalidResourceReference,
        UneditableMetadata,
    }
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct ProfileFontBinding {
        pub version: u32,
        pub mode: ProfileFontMode,
        pub hash: Option<String>,
        pub uneditable_reason: Option<ProfileFontMetadataError>,
    }
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct ProfileFontResourceRef {
        pub version: u32,
        pub hash: String,
        pub family: String,
    }
    #[derive(Debug, Clone, PartialEq, Eq, Serialize)]
    pub struct ProfileFontSnapshot {
        pub version: u32,
        pub profile_id: String,
        pub configuration_revision: u64,
        pub saved: ProfileFontBinding,
        pub raw_family: String,
        pub runtime_source: ProfileFontRuntimeSource,
        pub captured_terminal_slot: Option<ProfileFontResourceRef>,
        pub resource: Option<ProfileFontResourceRef>,
        pub actual_family: String,
        pub fallback_reason: Option<ProfileFontFailure>,
    }
    #[derive(Clone, Serialize, Deserialize)]
    pub struct ProfileFontResourceRequest {
        pub request_id: String,
        pub resource: ProfileFontResourceRef,
    }
    #[derive(Debug, Serialize)]
    pub struct ProfileFontResourceState {
        pub request_id: String,
        pub resource: Option<ProfileFontResourceRef>,
        pub family: String,
        pub failure: Option<ProfileFontFailure>,
    }
}
