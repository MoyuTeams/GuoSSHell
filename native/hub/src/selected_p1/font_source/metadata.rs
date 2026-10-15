//! 原 profile 字体 namespace 的借用判定；不生成 UUID/中文错误/临时 family。

use rshell_m0::rshell_core::TerminalSettingsV1;

pub const NAMESPACE: &str = "guosshell.font";
pub const VERSION: u32 = 1;
pub const FAMILY_PREFIX: &str = "GuoshFont_";
pub const DEFAULT_FAMILY: &str = "MesloLGS NF";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum FontMode {
    LegacyUndetermined,
    InheritTerminalSlot,
    ExplicitFamily,
    ExplicitCachedTtf,
    Uneditable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum MetadataError {
    InvalidMetadata,
    UnsupportedVersion,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ResourceFailure {
    Missing,
    Corrupt,
    Unavailable,
    UneditableMetadata,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BindingRef<'a> {
    pub version: u32,
    pub mode: FontMode,
    pub hash: Option<&'a str>,
    pub uneditable_reason: Option<MetadataError>,
}

pub fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn family_matches(family: &str, hash: &str) -> bool {
    family.strip_prefix(FAMILY_PREFIX) == Some(hash)
}

pub fn saved_binding(settings: &TerminalSettingsV1) -> BindingRef<'_> {
    let binding = |mode, hash, uneditable_reason| BindingRef {
        version: VERSION,
        mode,
        hash,
        uneditable_reason,
    };
    let invalid = |reason| binding(FontMode::Uneditable, None, Some(reason));
    let Some(value) = settings.extensions.get(NAMESPACE) else {
        return binding(FontMode::LegacyUndetermined, None, None);
    };
    let Some(object) = value.as_object() else {
        return invalid(MetadataError::InvalidMetadata);
    };
    match object.get("version").and_then(serde_json::Value::as_u64) {
        Some(1) => {}
        Some(_) => return invalid(MetadataError::UnsupportedVersion),
        None => return invalid(MetadataError::InvalidMetadata),
    }
    match object.get("mode").and_then(serde_json::Value::as_str) {
        Some("inherit_terminal_slot") if object.len() == 2 => {
            binding(FontMode::InheritTerminalSlot, None, None)
        }
        Some("explicit_family") if object.len() == 2 && !settings.font_family.trim().is_empty() => {
            binding(FontMode::ExplicitFamily, None, None)
        }
        Some("explicit_cached_ttf") if object.len() == 3 => {
            let Some(hash) = object.get("hash").and_then(serde_json::Value::as_str) else {
                return invalid(MetadataError::InvalidMetadata);
            };
            if !valid_hash(hash) || !family_matches(&settings.font_family, hash) {
                return invalid(MetadataError::InvalidMetadata);
            }
            binding(FontMode::ExplicitCachedTtf, Some(hash), None)
        }
        _ => invalid(MetadataError::InvalidMetadata),
    }
}
