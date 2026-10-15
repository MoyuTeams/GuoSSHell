//! 桌面完整配置事务。所有写入以固定上游 CAS 的 Applied 为成功依据。

#[cfg(test)]
#[path = "desktop_settings_tests.rs"]
mod tests;

use rshell_m0::rshell_core::{
    AppSettings, ColorScheme, KeyBinding, KeyCode, KeyModifiers, SettingsValidationError,
    TerminalOverrides, TerminalProfile, TerminalProfileId, TerminalSettingsV1,
    validate_app_settings, validate_terminal_profile,
};
use rshell_m0::rshell_storage::{
    ConfigurationChange, ConfigurationCommitOutcome, ConfigurationSnapshot, SqliteRepository,
    StorageError,
};

use crate::profile_fonts;
use crate::signals::settings::{
    DesktopAppSettings, DesktopConfiguration, DesktopKeyBinding, DesktopProfileEffective,
    DesktopProfileSettingsRequest, DesktopProfileTtfRequest, DesktopSettingsCapabilities,
    DesktopSettingsFieldError, DesktopSettingsRequest, DesktopSettingsResult,
    DesktopTerminalProfile, ProfileFontBinding, ProfileFontMode,
};
use std::path::Path;

pub const PROTOCOL_VERSION: u32 = 1;

pub fn available() -> bool {
    cfg!(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "linux"
    ))
}

const COLORS: &[(&str, ColorScheme)] = &[
    ("default", ColorScheme::Default),
    ("one_dark", ColorScheme::OneDark),
    ("solarized_dark", ColorScheme::SolarizedDark),
    ("solarized_light", ColorScheme::SolarizedLight),
    ("dracula", ColorScheme::Dracula),
    ("monokai", ColorScheme::Monokai),
    ("nord", ColorScheme::Nord),
    ("gruvbox_dark", ColorScheme::GruvboxDark),
    ("tokyo_night", ColorScheme::TokyoNight),
    ("campbell_powershell", ColorScheme::CampbellPowershell),
];

fn capabilities(desktop: bool) -> DesktopSettingsCapabilities {
    DesktopSettingsCapabilities {
        available: desktop,
        unavailable_reason: if desktop {
            ""
        } else {
            "移动端不支持桌面配置协议"
        }
        .into(),
        color_schemes: COLORS.iter().map(|(name, _)| (*name).into()).collect(),
        key_actions: [
            "send:\u{1b}[3~",
            "send:\u{7f}",
            "send:\u{8}",
            "clear_scrollback",
            "new_tab",
            "split_vertical",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        min_font_size: 6.0,
        max_font_size: 72.0,
        min_scrollback_lines: 100,
        max_scrollback_lines: 1_000_000,
        min_grid_size: 1,
        max_grid_size: 999,
        profile_changes_require_new_session: true,
    }
}

fn result(request_id: String, desktop: bool) -> DesktopSettingsResult {
    DesktopSettingsResult {
        protocol_version: PROTOCOL_VERSION,
        request_id,
        status: "storage_error".into(),
        revision: None,
        target_id: String::new(),
        detail: "配置存储不可用，请重试。".into(),
        field_errors: vec![],
        capabilities: capabilities(desktop),
        snapshot: None,
    }
}

pub fn task_failed(request_id: String) -> DesktopSettingsResult {
    result(request_id, available())
}

#[cfg(test)]
pub fn handle(
    repository: &SqliteRepository,
    request: DesktopSettingsRequest,
) -> DesktopSettingsResult {
    handle_for(repository, request, available())
}

/// 查询及成功提交的投影共享同一次 terminal 槽 capture，不声称偏好文件参与 CAS。
pub fn handle_with_resources(
    repository: &SqliteRepository,
    request: DesktopSettingsRequest,
    root: &Path,
    captured_slot: Option<crate::fonts::ImportedFont>,
) -> DesktopSettingsResult {
    let mut reply = handle_inner(repository, request, available(), Some(root), None);
    project_fonts(&mut reply, root, captured_slot);
    reply
}

pub fn handle_profile(
    repository: &SqliteRepository,
    request: DesktopProfileSettingsRequest,
    root: &Path,
    captured_slot: Option<crate::fonts::ImportedFont>,
) -> DesktopSettingsResult {
    let draft = DesktopSettingsRequest {
        protocol_version: request.protocol_version,
        request_id: request.request_id,
        base_revision: request.base_revision,
        operation: "save_profile".into(),
        target_id: request.target_id,
        name: String::new(),
        app: None,
        profile: Some(request.profile),
    };
    let mut reply = handle_inner(
        repository,
        draft,
        available(),
        Some(root),
        Some(&request.font_binding),
    );
    project_fonts(&mut reply, root, captured_slot);
    reply
}

/// 先使不可变资源就绪，再由普通整体配置 CAS 发布引用；冲突不删除 blob。
pub fn handle_ttf(
    repository: &SqliteRepository,
    request: DesktopProfileTtfRequest,
    bytes: Vec<u8>,
    root: &Path,
    captured_slot: Option<crate::fonts::ImportedFont>,
) -> DesktopSettingsResult {
    let mut draft_request = DesktopSettingsRequest {
        protocol_version: request.protocol_version,
        request_id: request.request_id,
        base_revision: request.base_revision,
        operation: "save_profile".into(),
        target_id: request.target_id,
        name: String::new(),
        app: None,
        profile: Some(request.profile),
    };
    let mut reply = result(draft_request.request_id.clone(), available());
    if !available()
        || draft_request.protocol_version != PROTOCOL_VERSION
        || draft_request.request_id.trim().is_empty()
    {
        return handle_inner(repository, draft_request, available(), Some(root), None);
    }
    let snapshot = match repository.load_configuration() {
        Ok(snapshot) => snapshot,
        Err(_) => return reply,
    };
    reply.revision = Some(snapshot.revision);
    if draft_request.base_revision != snapshot.revision {
        return conflict(reply, snapshot.revision);
    }
    let target = match id(&draft_request.target_id, "target_id") {
        Ok(target) => target,
        Err(error) => return invalid(reply, error),
    };
    let Some(stored) = snapshot
        .profiles
        .iter()
        .find(|profile| profile.id == target)
    else {
        return invalid(reply, field("target_id", "unknown_profile"));
    };
    if profile_fonts::saved_binding(&stored.settings).mode == ProfileFontMode::Uneditable {
        return invalid(reply, field("font_binding", "font_metadata_uneditable"));
    }
    let resource = match crate::font_resources::verify_ttf(bytes) {
        Ok(resource) => resource,
        Err(error) => return invalid(reply, field("font_resource", error.reason())),
    };
    let selected = profile_fonts::binding(
        ProfileFontMode::ExplicitCachedTtf,
        Some(resource.hash().into()),
    );
    if let Some(draft) = draft_request.profile.as_mut() {
        draft.font_family = resource.family();
    }
    // 校验全部 profile 字段后再发布资源，避免无效草稿留下无用缓存。
    let (change, target_id) = match prepare_with_binding(&snapshot, &draft_request, Some(&selected))
    {
        Ok(prepared) => prepared,
        Err(error) => return invalid(reply, error),
    };
    if let Err(error) = crate::font_resources::publish(root, &resource) {
        return invalid(reply, field("font_resource", error.reason()));
    }
    let mut reply = commit(repository, reply, snapshot, change, target_id);
    project_fonts(&mut reply, root, captured_slot);
    reply
}

fn project_fonts(
    reply: &mut DesktopSettingsResult,
    root: &Path,
    captured_slot: Option<crate::fonts::ImportedFont>,
) {
    let Some(projected) = reply.snapshot.as_mut() else {
        return;
    };
    // 只从成功查询/提交的投影构建字体快照，不再读取一个可能已推进的配置 revision。
    for effective in &mut projected.effective_profiles {
        if let Some(profile) = projected
            .profiles
            .iter()
            .find(|profile| profile.id == effective.id)
        {
            // 解析器的拥有字段来自精确投影；extensions 在此不能重建，保存模式直接投影。
            effective.font = Some(profile_fonts::resolve_projection(
                profile,
                effective.font_binding.clone(),
                projected.revision,
                captured_slot.clone(),
                root,
            ));
        }
    }
}

// 平台参数仅用于模块内部测试；生产入口只能取编译目标，Dart 无权启用桌面能力。
#[cfg(test)]
fn handle_for(
    repository: &SqliteRepository,
    request: DesktopSettingsRequest,
    desktop: bool,
) -> DesktopSettingsResult {
    handle_inner(repository, request, desktop, None, None)
}

fn handle_inner(
    repository: &SqliteRepository,
    request: DesktopSettingsRequest,
    desktop: bool,
    root: Option<&Path>,
    selected: Option<&ProfileFontBinding>,
) -> DesktopSettingsResult {
    let mut reply = result(request.request_id.clone(), desktop);
    if !desktop {
        reply.status = "unsupported_platform".into();
        reply.detail = "移动端不支持桌面配置协议。".into();
        return reply;
    }
    if request.protocol_version != PROTOCOL_VERSION {
        reply.status = "unsupported_version".into();
        reply.detail = "不支持的桌面配置协议版本。".into();
        return reply;
    }
    if request.request_id.trim().is_empty() {
        return invalid(reply, field("request_id", "blank"));
    }
    let snapshot = match repository.load_configuration() {
        Ok(snapshot) => snapshot,
        Err(_) => return reply,
    };
    reply.revision = Some(snapshot.revision);
    if request.operation == "query" {
        reply.status = "ok".into();
        reply.detail.clear();
        reply.snapshot = Some(project(&snapshot));
        return reply;
    }
    if request.base_revision != snapshot.revision {
        return conflict(reply, snapshot.revision);
    }
    let (change, target_id) = match prepare_with_binding(&snapshot, &request, selected) {
        Ok(prepared) => prepared,
        Err(error) => return invalid(reply, error),
    };
    // 已保存引用缺失时仍允许非字体编辑；只有新选择才要求资源已就绪。
    for profile in change
        .upsert_profiles
        .iter()
        .filter(|_| request.operation == "save_profile")
    {
        let selected = profile_fonts::saved_binding(&profile.settings);
        if selected.mode != ProfileFontMode::ExplicitCachedTtf {
            continue;
        }
        if snapshot.profiles.iter().any(|stored| {
            stored.id == profile.id
                && stored.settings.font_family == profile.settings.font_family
                && profile_fonts::saved_binding(&stored.settings) == selected
        }) {
            continue;
        }
        let Some(root) = root else {
            return invalid(reply, field("font_resource", "unavailable"));
        };
        if let Some(hash) = selected.hash.as_deref()
            && let Err(error) = crate::font_resources::load_by_hash(root, hash)
        {
            return invalid(reply, field("font_resource", error.reason()));
        }
    }
    commit(repository, reply, snapshot, change, target_id)
}

fn commit(
    repository: &SqliteRepository,
    mut reply: DesktopSettingsResult,
    snapshot: ConfigurationSnapshot,
    change: ConfigurationChange,
    target_id: String,
) -> DesktopSettingsResult {
    let deleted_id = change.delete_profiles.first().copied();
    // 使用已提交的精确快照构造回包，不在提交后再读一个可能更新的版本。
    let mut committed = snapshot;
    committed.settings = change.settings.clone();
    for profile in &change.upsert_profiles {
        committed.profiles.retain(|stored| stored.id != profile.id);
        committed.profiles.push(profile.clone());
    }
    committed
        .profiles
        .retain(|profile| !change.delete_profiles.contains(&profile.id));
    match repository.commit_configuration(change) {
        Ok(ConfigurationCommitOutcome::Applied { revision }) => {
            committed.revision = revision;
            reply.status = "applied".into();
            reply.revision = Some(revision);
            reply.target_id = target_id;
            reply.detail.clear();
            reply.snapshot = Some(project(&committed));
            reply
        }
        Ok(ConfigurationCommitOutcome::Conflict { actual_revision }) => {
            conflict(reply, actual_revision)
        }
        Err(StorageError::Constraint) if deleted_id.is_some() => {
            // Constraint 也可能来自版本溢出等存储约束；只有查到连接引用才报告引用保护。
            // 删除与引用检查仍以上游同一写事务为权威，这次读取仅细化错误说明。
            if repository.load_catalog().is_ok_and(|catalog| {
                catalog
                    .connections
                    .values()
                    .any(|connection| connection.terminal_profile_id == deleted_id)
            }) {
                invalid(reply, field("target_id", "profile_in_use"))
            } else {
                reply
            }
        }
        Err(_) => reply,
    }
}

fn conflict(mut reply: DesktopSettingsResult, revision: u64) -> DesktopSettingsResult {
    reply.status = "conflict".into();
    reply.revision = Some(revision);
    reply.detail = "配置已被其他操作修改，请重新读取后确认。".into();
    reply
}

fn invalid(
    mut reply: DesktopSettingsResult,
    error: DesktopSettingsFieldError,
) -> DesktopSettingsResult {
    reply.status = "validation_error".into();
    reply.detail = "配置未保存，请检查标记字段。".into();
    reply.field_errors.push(error);
    reply
}

fn field(name: &str, code: &str) -> DesktopSettingsFieldError {
    DesktopSettingsFieldError {
        field: name.into(),
        code: code.into(),
    }
}

fn validation(error: SettingsValidationError) -> DesktopSettingsFieldError {
    use rshell_m0::rshell_core::SettingsValidationCode::{
        Blank, DuplicateBinding, InvalidAction, InvalidChord, NonFinite, OutOfRange, UnknownProfile,
    };
    field(
        error.field,
        match error.code {
            Blank => "blank",
            OutOfRange => "out_of_range",
            NonFinite => "non_finite",
            InvalidChord => "invalid_chord",
            DuplicateBinding => "duplicate_binding",
            InvalidAction => "invalid_action",
            UnknownProfile => "unknown_profile",
        },
    )
}

fn id(value: &str, name: &str) -> Result<TerminalProfileId, DesktopSettingsFieldError> {
    uuid::Uuid::parse_str(value)
        .map(TerminalProfileId)
        .map_err(|_| field(name, "invalid_id"))
}

#[cfg(test)]
fn prepare(
    snapshot: &ConfigurationSnapshot,
    request: &DesktopSettingsRequest,
) -> Result<(ConfigurationChange, String), DesktopSettingsFieldError> {
    prepare_with_binding(snapshot, request, None)
}

fn prepare_with_binding(
    snapshot: &ConfigurationSnapshot,
    request: &DesktopSettingsRequest,
    selected: Option<&ProfileFontBinding>,
) -> Result<(ConfigurationChange, String), DesktopSettingsFieldError> {
    let mut change = ConfigurationChange {
        expected_revision: request.base_revision,
        settings: snapshot.settings.clone(),
        upsert_profiles: vec![],
        delete_profiles: vec![],
    };
    let target_id;
    match request.operation.as_str() {
        "save_app" => {
            let app = request
                .app
                .as_ref()
                .ok_or_else(|| field("app", "required"))?;
            change.settings = AppSettings {
                default_terminal_profile: id(
                    &app.default_terminal_profile,
                    "default_terminal_profile",
                )?,
                color_scheme: color(&app.color_scheme)?,
                key_bindings: bindings(&app.key_bindings)?,
            };
            validate_app_settings(&change.settings, &snapshot.profiles).map_err(validation)?;
            target_id = String::new();
        }
        "create_profile" => {
            let mut profile = TerminalProfile {
                id: TerminalProfileId(uuid::Uuid::new_v4()),
                name: request.name.clone(),
                settings: TerminalSettingsV1 {
                    font_family: super::FONT_FAMILIES[0].into(),
                    font_size: super::DEFAULT_FONT_SIZE,
                    ..TerminalSettingsV1::default()
                },
            };
            profile_fonts::write_binding(
                &mut profile.settings,
                &profile_fonts::binding(ProfileFontMode::InheritTerminalSlot, None),
            )
            .map_err(|code| field("font_binding", code))?;
            validate_terminal_profile(&profile).map_err(validation)?;
            target_id = profile.id.0.to_string();
            change.upsert_profiles.push(profile);
        }
        "save_profile" | "copy_profile" | "rename_profile" | "delete_profile" => {
            let target = id(&request.target_id, "target_id")?;
            let stored = snapshot
                .profiles
                .iter()
                .find(|profile| profile.id == target)
                .ok_or_else(|| field("target_id", "unknown_profile"))?;
            let mut profile = stored.clone();
            match request.operation.as_str() {
                "save_profile" => {
                    let draft = request
                        .profile
                        .as_ref()
                        .ok_or_else(|| field("profile", "required"))?;
                    profile = parse_profile(stored, draft, selected)?;
                    if profile.id != target {
                        return Err(field("profile.id", "target_mismatch"));
                    }
                }
                "copy_profile" => {
                    profile.id = TerminalProfileId(uuid::Uuid::new_v4());
                    profile.name = request.name.clone();
                }
                "rename_profile" => profile.name = request.name.clone(),
                "delete_profile" => {
                    if target == snapshot.settings.default_terminal_profile {
                        return Err(field("target_id", "default_profile"));
                    }
                    change.delete_profiles.push(target);
                }
                _ => unreachable!(),
            }
            target_id = profile.id.0.to_string();
            if change.delete_profiles.is_empty() {
                validate_terminal_profile(&profile).map_err(validation)?;
                change.upsert_profiles.push(profile);
            }
        }
        _ => return Err(field("operation", "unknown_operation")),
    }
    Ok((change, target_id))
}

fn color(value: &str) -> Result<ColorScheme, DesktopSettingsFieldError> {
    COLORS
        .iter()
        .find(|(name, _)| *name == value)
        .map(|(_, color)| *color)
        .ok_or_else(|| field("color_scheme", "unknown_color_scheme"))
}

fn color_name(value: ColorScheme) -> String {
    COLORS
        .iter()
        .find(|(_, color)| *color == value)
        .map(|(name, _)| (*name).into())
        .unwrap_or_default()
}

fn bindings(values: &[DesktopKeyBinding]) -> Result<Vec<KeyBinding>, DesktopSettingsFieldError> {
    values
        .iter()
        .map(|binding| {
            Ok(KeyBinding {
                code: match binding.key.as_str() {
                    "Enter" => KeyCode::Enter,
                    "Escape" => KeyCode::Escape,
                    "Tab" => KeyCode::Tab,
                    "Backspace" => KeyCode::Backspace,
                    "Delete" => KeyCode::Delete,
                    "Insert" => KeyCode::Insert,
                    "Home" => KeyCode::Home,
                    "End" => KeyCode::End,
                    "PageUp" => KeyCode::PageUp,
                    "PageDown" => KeyCode::PageDown,
                    "ArrowUp" => KeyCode::ArrowUp,
                    "ArrowDown" => KeyCode::ArrowDown,
                    "ArrowLeft" => KeyCode::ArrowLeft,
                    "ArrowRight" => KeyCode::ArrowRight,
                    key if key.chars().count() == 1 => KeyCode::Character(
                        key.chars()
                            .next()
                            .ok_or_else(|| field("key_bindings.code", "invalid_chord"))?,
                    ),
                    key => {
                        let number = key
                            .strip_prefix('F')
                            .and_then(|number| number.parse::<u8>().ok())
                            .filter(|number| {
                                (1..=24).contains(number) && key == format!("F{number}")
                            })
                            .ok_or_else(|| field("key_bindings.code", "invalid_chord"))?;
                        KeyCode::F(number)
                    }
                },
                modifiers: KeyModifiers {
                    shift: binding.shift,
                    control: binding.control,
                    alt: binding.alt,
                    super_key: binding.super_key,
                },
                action: binding.action.clone(),
            })
        })
        .collect()
}

fn project_bindings(values: &[KeyBinding]) -> Vec<DesktopKeyBinding> {
    values
        .iter()
        .map(|binding| DesktopKeyBinding {
            key: match binding.code {
                KeyCode::Character(ch) => ch.to_string(),
                KeyCode::F(number) => format!("F{number}"),
                _ => format!("{:?}", binding.code),
            },
            shift: binding.modifiers.shift,
            control: binding.modifiers.control,
            alt: binding.modifiers.alt,
            super_key: binding.modifiers.super_key,
            action: binding.action.clone(),
        })
        .collect()
}

fn parse_profile(
    stored: &TerminalProfile,
    value: &DesktopTerminalProfile,
    selected: Option<&ProfileFontBinding>,
) -> Result<TerminalProfile, DesktopSettingsFieldError> {
    if value.version != 1 {
        return Err(field("profile.version", "unsupported_version"));
    }
    // 必须在 f64→f32 转换前校验，避免略超界的值被浮点舍入接纳。
    if !value.font_size.is_finite() {
        return Err(field("font_size", "non_finite"));
    }
    if !(6.0..=72.0).contains(&value.font_size) {
        return Err(field("font_size", "out_of_range"));
    }
    let saved = profile_fonts::saved_binding(&stored.settings);
    let same_family = value.font_family == stored.settings.font_family;
    let preserve = selected.is_none_or(|selected| selected == &saved);
    let mut profile = stored.clone();
    profile.id = id(&value.id, "profile.id")?;
    profile.name = value.name.clone();
    if saved.mode == ProfileFontMode::Uneditable && (!same_family || !preserve) {
        return Err(field("font_binding", "font_metadata_uneditable"));
    }
    if !same_family || !preserve {
        let selected = selected.ok_or_else(|| field("font_binding", "font_binding_required"))?;
        profile_fonts::set_binding(&mut profile.settings, &value.font_family, selected)
            .map_err(|code| field("font_binding", code))?;
    }
    let settings = &mut profile.settings;
    settings.terminal_type = value.terminal_type.clone();
    settings.initial_cols =
        u16::try_from(value.initial_cols).map_err(|_| field("initial_cols", "out_of_range"))?;
    settings.initial_rows =
        u16::try_from(value.initial_rows).map_err(|_| field("initial_rows", "out_of_range"))?;
    settings.scrollback_lines = value.scrollback_lines as usize;
    settings.font_size = value.font_size as f32;
    settings.color_scheme = color(&value.color_scheme)?;
    settings.key_bindings = bindings(&value.key_bindings)?;
    settings.left_alt_as_meta = value.left_alt_as_meta;
    settings.right_alt_as_meta = value.right_alt_as_meta;
    settings.enable_csi_u = value.enable_csi_u;
    settings.enable_kitty_keyboard = value.enable_kitty_keyboard;
    settings.mouse_reporting = value.mouse_reporting;
    settings.scroll_on_output = value.scroll_on_output;
    settings.scroll_on_keypress = value.scroll_on_keypress;
    settings.answerback = value.answerback.clone();
    Ok(profile)
}

fn project_profile(profile: &TerminalProfile) -> DesktopTerminalProfile {
    let s = &profile.settings;
    DesktopTerminalProfile {
        id: profile.id.0.to_string(),
        name: profile.name.clone(),
        version: 1,
        terminal_type: s.terminal_type.clone(),
        initial_cols: s.initial_cols.into(),
        initial_rows: s.initial_rows.into(),
        scrollback_lines: s.scrollback_lines as u32,
        font_family: s.font_family.clone(),
        font_size: s.font_size.into(),
        color_scheme: color_name(s.color_scheme),
        key_bindings: project_bindings(&s.key_bindings),
        left_alt_as_meta: s.left_alt_as_meta,
        right_alt_as_meta: s.right_alt_as_meta,
        enable_csi_u: s.enable_csi_u,
        enable_kitty_keyboard: s.enable_kitty_keyboard,
        mouse_reporting: s.mouse_reporting,
        scroll_on_output: s.scroll_on_output,
        scroll_on_keypress: s.scroll_on_keypress,
        answerback: s.answerback.clone(),
    }
}

fn project(snapshot: &ConfigurationSnapshot) -> DesktopConfiguration {
    DesktopConfiguration {
        revision: snapshot.revision,
        app: DesktopAppSettings {
            default_terminal_profile: snapshot.settings.default_terminal_profile.0.to_string(),
            color_scheme: color_name(snapshot.settings.color_scheme),
            key_bindings: project_bindings(&snapshot.settings.key_bindings),
        },
        profiles: snapshot.profiles.iter().map(project_profile).collect(),
        effective_profiles: snapshot
            .profiles
            .iter()
            .map(|profile| {
                let resolved = profile
                    .settings
                    .resolve(&TerminalOverrides::default())
                    .with_app_key_bindings(&snapshot.settings.key_bindings);
                DesktopProfileEffective {
                    id: profile.id.0.to_string(),
                    key_bindings: project_bindings(&resolved.key_bindings),
                    scrollback_lines: resolved.scrollback_lines as u32,
                    scrollback_limit_reason: String::new(),
                    font_binding: profile_fonts::saved_binding(&profile.settings),
                    font: None,
                }
            })
            .collect(),
    }
}
