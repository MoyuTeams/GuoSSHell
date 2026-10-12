//! profile 字体的单一配置归属、严格元数据及阶段四可消费的不可变资源快照。

use std::path::Path;

use rshell_m0::rshell_core::TerminalProfileId;
use rshell_m0::rshell_core::TerminalSettingsV1;
use rshell_m0::rshell_storage::ConfigurationSnapshot;
use serde_json::json;

use crate::font_resources::FontResource;
use crate::font_resources::{FontResourceError, load_by_hash};
use crate::fonts::ImportedFont;
use crate::signals::settings::{
    DesktopTerminalProfile, ProfileFontBinding, ProfileFontFailure, ProfileFontMetadataError,
    ProfileFontMode, ProfileFontResourceRef, ProfileFontResourceRequest, ProfileFontResourceState,
    ProfileFontRuntimeSource, ProfileFontSnapshot,
};

pub(crate) const NAMESPACE: &str = "guosshell.font";
pub(crate) const VERSION: u32 = 1;

pub(crate) fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(crate) fn cached_family(hash: &str) -> String {
    format!("GuoshFont_{hash}")
}

pub(crate) fn binding(mode: ProfileFontMode, hash: Option<String>) -> ProfileFontBinding {
    ProfileFontBinding {
        version: VERSION,
        mode,
        hash,
        uneditable_reason: None,
    }
}

fn uneditable(reason: ProfileFontMetadataError) -> ProfileFontBinding {
    ProfileFontBinding {
        uneditable_reason: Some(reason),
        ..binding(ProfileFontMode::Uneditable, None)
    }
}

/// 缺省是唯一的 legacy 判据；未知、损坏和非规范已知载荷一律不可编辑。
pub(crate) fn saved_binding(settings: &TerminalSettingsV1) -> ProfileFontBinding {
    let Some(value) = settings.extensions.get(NAMESPACE) else {
        return binding(ProfileFontMode::LegacyUndetermined, None);
    };
    let Some(object) = value.as_object() else {
        return uneditable(ProfileFontMetadataError::InvalidMetadata);
    };
    match object.get("version").and_then(serde_json::Value::as_u64) {
        Some(1) => {}
        Some(_) => return uneditable(ProfileFontMetadataError::UnsupportedVersion),
        None => return uneditable(ProfileFontMetadataError::InvalidMetadata),
    }
    let mode = match object.get("mode").and_then(serde_json::Value::as_str) {
        Some("inherit_terminal_slot") if object.len() == 2 => ProfileFontMode::InheritTerminalSlot,
        Some("explicit_family") if object.len() == 2 && !settings.font_family.trim().is_empty() => {
            ProfileFontMode::ExplicitFamily
        }
        Some("explicit_cached_ttf") if object.len() == 3 => {
            let Some(hash) = object.get("hash").and_then(serde_json::Value::as_str) else {
                return uneditable(ProfileFontMetadataError::InvalidMetadata);
            };
            if !valid_hash(hash) || settings.font_family != cached_family(hash) {
                return uneditable(ProfileFontMetadataError::InvalidMetadata);
            }
            return binding(ProfileFontMode::ExplicitCachedTtf, Some(hash.into()));
        }
        _ => return uneditable(ProfileFontMetadataError::InvalidMetadata),
    };
    binding(mode, None)
}

/// 仅更新拥有的 namespace；ExplicitFamily 的值唯一保存在原 font_family 字段。
pub(crate) fn write_binding(
    settings: &mut TerminalSettingsV1,
    selected: &ProfileFontBinding,
) -> Result<(), &'static str> {
    let family = settings.font_family.clone();
    set_binding(settings, &family, selected)
}

pub(crate) fn set_binding(
    settings: &mut TerminalSettingsV1,
    family: &str,
    selected: &ProfileFontBinding,
) -> Result<(), &'static str> {
    if saved_binding(settings).mode == ProfileFontMode::Uneditable {
        return Err("font_metadata_uneditable");
    }
    if selected.version != VERSION || selected.uneditable_reason.is_some() {
        return Err("invalid_font_binding");
    }
    let value = match (selected.mode, selected.hash.as_deref()) {
        (ProfileFontMode::InheritTerminalSlot, None) => {
            json!({"version": VERSION, "mode": "inherit_terminal_slot"})
        }
        (ProfileFontMode::ExplicitFamily, None) if !family.trim().is_empty() => {
            json!({"version": VERSION, "mode": "explicit_family"})
        }
        (ProfileFontMode::ExplicitCachedTtf, Some(hash))
            if valid_hash(hash) && family == cached_family(hash) =>
        {
            json!({"version": VERSION, "mode": "explicit_cached_ttf", "hash": hash})
        }
        _ => return Err("invalid_font_binding"),
    };
    settings.font_family = family.into();
    settings.extensions.insert(NAMESPACE.into(), value);
    Ok(())
}

pub(crate) fn resource_ref(hash: &str) -> ProfileFontResourceRef {
    ProfileFontResourceRef {
        version: VERSION,
        hash: hash.into(),
        family: cached_family(hash),
    }
}

fn resource_failure(error: FontResourceError) -> ProfileFontFailure {
    match error {
        FontResourceError::Missing => ProfileFontFailure::Missing,
        FontResourceError::Corrupt => ProfileFontFailure::Corrupt,
        FontResourceError::Unavailable => ProfileFontFailure::Unavailable,
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedProfileFont {
    pub(crate) projection: ProfileFontSnapshot,
    /// 准备态和运行态实际持有已验证字节，槽变更不会替换此资源。
    pub(crate) resource: Option<FontResource>,
    /// 原始槽身份只 capture 一次，legacy 更新时重新解析而不回写配置。
    #[cfg(test)]
    pub(crate) captured_terminal_slot: Option<ImportedFont>,
}

/// 调用方从 PreferenceFile capture 一次 terminal 槽，再传入本函数。
/// 显式模式不读取槽，明确继承只捕获一次；legacy 随旧槽事件重新调用。
pub(crate) fn resolve_profile_font(
    configuration: &ConfigurationSnapshot,
    profile_id: TerminalProfileId,
    captured_slot: Option<ImportedFont>,
    root: &Path,
) -> Option<ResolvedProfileFont> {
    let profile = configuration.profiles.iter().find(|p| p.id == profile_id)?;
    let saved = saved_binding(&profile.settings);
    Some(resolve_parts(
        profile_id.0.to_string(),
        configuration.revision,
        profile.settings.font_family.clone(),
        saved,
        captured_slot,
        root,
    ))
}

pub(crate) fn resolve_projection(
    profile: &DesktopTerminalProfile,
    saved: ProfileFontBinding,
    configuration_revision: u64,
    captured_slot: Option<ImportedFont>,
    root: &Path,
) -> ProfileFontSnapshot {
    resolve_parts(
        profile.id.clone(),
        configuration_revision,
        profile.font_family.clone(),
        saved,
        captured_slot,
        root,
    )
    .projection
}

fn resolve_parts(
    profile_id: String,
    configuration_revision: u64,
    raw_family: String,
    saved: ProfileFontBinding,
    captured_slot: Option<ImportedFont>,
    root: &Path,
) -> ResolvedProfileFont {
    let uses_slot = matches!(
        saved.mode,
        ProfileFontMode::InheritTerminalSlot | ProfileFontMode::LegacyUndetermined
    );
    let captured_terminal_slot = if uses_slot { captured_slot } else { None };
    let runtime_source = match saved.mode {
        ProfileFontMode::LegacyUndetermined => ProfileFontRuntimeSource::LegacyTerminalSlot,
        ProfileFontMode::Uneditable => ProfileFontRuntimeSource::Uneditable,
        _ => ProfileFontRuntimeSource::ProfileBinding,
    };
    let hash = if uses_slot {
        captured_terminal_slot
            .as_ref()
            .map(|font| font.hash.as_str())
    } else {
        saved.hash.as_deref()
    };
    let mut projection = ProfileFontSnapshot {
        version: VERSION,
        profile_id,
        configuration_revision,
        saved: saved.clone(),
        raw_family: raw_family.clone(),
        runtime_source,
        captured_terminal_slot: captured_terminal_slot
            .as_ref()
            .filter(|font| valid_hash(&font.hash))
            .map(|font| resource_ref(&font.hash)),
        resource: hash.filter(|hash| valid_hash(hash)).map(resource_ref),
        actual_family: crate::settings::DEFAULT_TERMINAL_FONT.into(),
        fallback_reason: None,
    };
    let mut resource = None;
    if saved.mode == ProfileFontMode::Uneditable {
        projection.fallback_reason = Some(ProfileFontFailure::UneditableMetadata);
    } else if saved.mode == ProfileFontMode::ExplicitFamily {
        projection.actual_family = raw_family;
    } else if let Some(hash) = hash {
        match load_by_hash(root, hash) {
            Ok(loaded) => {
                projection.actual_family = loaded.family();
                resource = Some(loaded);
            }
            Err(error) => projection.fallback_reason = Some(resource_failure(error)),
        }
    }
    ResolvedProfileFont {
        projection,
        resource,
        #[cfg(test)]
        captured_terminal_slot,
    }
}

/// 与槽和配置完全独立，只按请求捕获的严格内容身份加载，失败可随时重试。
pub(crate) fn load_resource(
    root: &Path,
    request: ProfileFontResourceRequest,
) -> (ProfileFontResourceState, Vec<u8>) {
    load_resource_for(root, request, crate::settings::desktop_available())
}

// 平台参数只供模块内部测试注入；生产入口固定取编译目标能力。
fn load_resource_for(
    root: &Path,
    request: ProfileFontResourceRequest,
    desktop: bool,
) -> (ProfileFontResourceState, Vec<u8>) {
    let mut state = ProfileFontResourceState {
        request_id: request.request_id,
        resource: None,
        family: String::new(),
        failure: None,
    };
    if !desktop {
        state.failure = Some(ProfileFontFailure::Unavailable);
        return (state, vec![]);
    }
    let reference = request.resource;
    if state.request_id.trim().is_empty()
        || reference.version != VERSION
        || !valid_hash(&reference.hash)
        || reference.family != cached_family(&reference.hash)
    {
        state.failure = Some(ProfileFontFailure::InvalidResourceReference);
        return (state, vec![]);
    }
    state.resource = Some(reference.clone());
    match load_by_hash(root, &reference.hash) {
        Ok(resource) => {
            state.family = resource.family();
            (state, resource.bytes().to_vec())
        }
        Err(error) => {
            state.failure = Some(resource_failure(error));
            (state, vec![])
        }
    }
}

#[cfg(test)]
#[path = "profile_fonts_tests.rs"]
mod tests;
