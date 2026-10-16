use rshell_m0::rshell_core::{
    AuthenticationKind, ConnectionProfile, KeyBinding, KeyCode, SettingsValidationCode as Code,
    SettingsValidationError, TerminalOverrides, TerminalSettingsV1, TransportKind,
    parse_terminal_key_action,
};

use super::raw::QuickTarget;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum ConnectionValidation {
    InvalidHost = 1,
    InvalidPort = 2,
    InvalidAuthentication = 3,
    MissingIdentityFile = 4,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum QuickValidation {
    HostRequired = 1,
    HostInvalid = 2,
    PortInvalid = 3,
    UsernameRequired = 4,
}

pub fn quick_target<'a>(target: QuickTarget<'a>) -> Result<(&'a str, &'a str), QuickValidation> {
    let host = target.host.trim();
    if host.is_empty() {
        return Err(QuickValidation::HostRequired);
    }
    if host.starts_with('-') || host.chars().any(char::is_whitespace) {
        return Err(QuickValidation::HostInvalid);
    }
    if target.port == 0 {
        return Err(QuickValidation::PortInvalid);
    }
    let username = target.username.trim();
    if username.is_empty() {
        return Err(QuickValidation::UsernameRequired);
    }
    Ok((host, username))
}

/// 对照 b72 connection::validation::validate_profile；不创建 InvalidHost 的 owned clone。
pub fn connection(profile: &ConnectionProfile) -> Result<(), super::Failure> {
    use ConnectionValidation as V;
    let host = profile.host.trim();
    if host.is_empty() || host.starts_with('-') {
        return Err(super::Failure::Connection(V::InvalidHost));
    }
    if profile.port == 0 {
        return Err(super::Failure::Connection(V::InvalidPort));
    }
    if !matches!(
        (profile.transport, profile.authentication),
        (
            TransportKind::SystemOpenSsh,
            AuthenticationKind::Agent | AuthenticationKind::PublicKey
        ) | (
            TransportKind::NativeSsh,
            AuthenticationKind::Password
                | AuthenticationKind::PublicKey
                | AuthenticationKind::KeyboardInteractive
        )
    ) {
        return Err(super::Failure::Connection(V::InvalidAuthentication));
    }
    if profile.authentication == AuthenticationKind::PublicKey
        && !profile.identity_file.as_ref().is_some_and(|path| {
            path.to_str()
                .map_or(!path.as_os_str().is_empty(), |text| !text.trim().is_empty())
        })
    {
        return Err(super::Failure::Connection(V::MissingIdentityFile));
    }
    overrides(&profile.terminal_overrides).map_err(super::Failure::Settings)
}

const fn error(field: &'static str, code: Code) -> SettingsValidationError {
    SettingsValidationError { field, code }
}

fn nonblank(field: &'static str, text: &str) -> Result<(), SettingsValidationError> {
    if text.trim().is_empty() {
        Err(error(field, Code::Blank))
    } else {
        Ok(())
    }
}

fn range(
    field: &'static str,
    value: usize,
    min: usize,
    max: usize,
) -> Result<(), SettingsValidationError> {
    if (min..=max).contains(&value) {
        Ok(())
    } else {
        Err(error(field, Code::OutOfRange))
    }
}

/// KeyCode 的 Debug 标识在此 schema 上是一一映射，直接比较同键及 modifiers 保留判重语义。
/// 检查顺序仍为 chord → action → duplicate；无 HashSet 或 chord format。
pub fn bindings(bindings: &[KeyBinding]) -> Result<(), SettingsValidationError> {
    for (index, binding) in bindings.iter().enumerate() {
        if matches!(binding.code, KeyCode::Character(c) if c.is_control())
            || matches!(binding.code, KeyCode::F(n) if !(1..=24).contains(&n))
        {
            return Err(error("key_bindings.code", Code::InvalidChord));
        }
        parse_terminal_key_action(&binding.action)?;
        if bindings[..index]
            .iter()
            .any(|prior| prior.code == binding.code && prior.modifiers == binding.modifiers)
        {
            return Err(error("key_bindings", Code::DuplicateBinding));
        }
    }
    Ok(())
}

pub fn settings(value: &TerminalSettingsV1) -> Result<(), SettingsValidationError> {
    nonblank("terminal_type", &value.terminal_type)?;
    range("initial_cols", usize::from(value.initial_cols), 1, 999)?;
    range("initial_rows", usize::from(value.initial_rows), 1, 999)?;
    range("scrollback_lines", value.scrollback_lines, 100, 1_000_000)?;
    nonblank("font_family", &value.font_family)?;
    if !value.font_size.is_finite() {
        return Err(error("font_size", Code::NonFinite));
    }
    if !(6.0..=72.0).contains(&value.font_size) {
        return Err(error("font_size", Code::OutOfRange));
    }
    nonblank("answerback", &value.answerback)?;
    bindings(&value.key_bindings)
}

pub fn overrides(value: &TerminalOverrides) -> Result<(), SettingsValidationError> {
    if let Some(v) = &value.terminal_type {
        nonblank("terminal_overrides.terminal_type", v)?;
    }
    if let Some(v) = value.initial_cols {
        range("terminal_overrides.initial_cols", usize::from(v), 1, 999)?;
    }
    if let Some(v) = value.initial_rows {
        range("terminal_overrides.initial_rows", usize::from(v), 1, 999)?;
    }
    if let Some(v) = value.scrollback_lines {
        range("terminal_overrides.scrollback_lines", v, 100, 1_000_000)?;
    }
    if let Some(v) = &value.font_family {
        nonblank("terminal_overrides.font_family", v)?;
    }
    if let Some(v) = value.font_size {
        if !v.is_finite() {
            return Err(error("terminal_overrides.font_size", Code::NonFinite));
        }
        if !(6.0..=72.0).contains(&v) {
            return Err(error("terminal_overrides.font_size", Code::OutOfRange));
        }
    }
    if let Some(v) = &value.key_bindings {
        bindings(v)?;
    }
    if let Some(v) = &value.answerback {
        nonblank("terminal_overrides.answerback", v)?;
    }
    Ok(())
}
