use std::collections::BTreeSet;
use std::path::PathBuf;

use rshell_m0::rshell_core::{
    AppSettings, AuthenticationKind, ColorScheme, ConnectionId, ConnectionProfile, CredentialRef,
    GroupId, HostKeyPolicy, KeyBinding, TerminalOverrides, TerminalProfile, TerminalProfileId,
    TerminalSettingsV1, TransportKind,
};
use serde::Deserialize;
use uuid::Uuid;

use super::{
    ExecutionCounts, Failure,
    raw::{FieldValue, RawFields},
    validation,
};

// 私有 storage schema 接缝；不复制序列化器或上游实现。
#[derive(Deserialize)]
struct OverridesDocument {
    version: u8,
    #[serde(flatten)]
    options: TerminalOverrides,
}

#[derive(Deserialize)]
struct KeyBindingsDocument {
    version: u8,
    key_bindings: Vec<KeyBinding>,
}

/// uuid 1.26.1 parser::try_parse/decode_hex32 与 InvalidUuid::into_err 仅固定数组及借用；
/// 不格式化诊断。simple/hyphenated/braced/URN、大小写由真实依赖决定。
pub fn uuid(text: &str, tag: u16) -> Result<Uuid, Failure> {
    Uuid::parse_str(text).map_err(|_| Failure::Corrupt { tag, json: None })
}

/// canonical formatter 使用 uuid::fmt 的固定字节写入闭包，不构造 String。
pub fn canonical_uuid(id: Uuid, buffer: &mut [u8; 36]) -> &str {
    id.hyphenated().encode_lower(buffer)
}

fn json<T: serde::de::DeserializeOwned>(
    text: &str,
    tag: u16,
    calls: &mut ExecutionCounts,
) -> Result<T, Failure> {
    calls.schema += 1;
    serde_json::from_str(text).map_err(|error| Failure::Corrupt {
        tag,
        json: Some(super::JsonFailure::from_error(&error)),
    })
}

pub(super) fn color(text: &str) -> Result<ColorScheme, Failure> {
    match text {
        "default" => Ok(ColorScheme::Default),
        "one_dark" => Ok(ColorScheme::OneDark),
        "solarized_dark" => Ok(ColorScheme::SolarizedDark),
        "solarized_light" => Ok(ColorScheme::SolarizedLight),
        "dracula" => Ok(ColorScheme::Dracula),
        "monokai" => Ok(ColorScheme::Monokai),
        "nord" => Ok(ColorScheme::Nord),
        "gruvbox_dark" => Ok(ColorScheme::GruvboxDark),
        "tokyo_night" => Ok(ColorScheme::TokyoNight),
        "campbell_powershell" => Ok(ColorScheme::CampbellPowershell),
        _ => Err(Failure::Corrupt {
            tag: 18,
            json: None,
        }),
    }
}

pub(super) fn saved(
    raw: &RawFields<'_>,
    calls: &mut ExecutionCounts,
) -> Result<Option<ConnectionProfile>, Failure> {
    if !raw.saved() {
        raw.validate_references(None, None)?;
        return Ok(None);
    }
    // catalog.rs 的构造/错误顺序：id/group/port/transport/auth/host-key/profile/overrides。
    let id = ConnectionId(uuid(raw.text(1), 1)?);
    let group_id = raw
        .nullable_text(2)
        .map(|text| uuid(text, 2).map(GroupId))
        .transpose()?;
    let FieldValue::Port(port) = raw.value(5) else {
        return Err(Failure::Protocol);
    };
    let transport = match raw.text(7) {
        "system_open_ssh" => TransportKind::SystemOpenSsh,
        "native_ssh" => TransportKind::NativeSsh,
        _ => return Err(Failure::Corrupt { tag: 7, json: None }),
    };
    let authentication = match raw.text(8) {
        "password" => AuthenticationKind::Password,
        "public_key" => AuthenticationKind::PublicKey,
        "agent" => AuthenticationKind::Agent,
        "keyboard_interactive" => AuthenticationKind::KeyboardInteractive,
        _ => return Err(Failure::Corrupt { tag: 8, json: None }),
    };
    if raw.text(11) != "strict" {
        return Err(Failure::Corrupt {
            tag: 11,
            json: None,
        });
    }
    let terminal_profile_id = raw
        .nullable_text(15)
        .map(|text| uuid(text, 15).map(TerminalProfileId))
        .transpose()?;
    let document: OverridesDocument = json(raw.text(16), 16, calls)?;
    if document.version != 1 {
        return Err(Failure::Corrupt {
            tag: 16,
            json: None,
        });
    }
    let FieldValue::Position(position) = raw.value(14) else {
        return Err(Failure::Protocol);
    };
    let mut tags = BTreeSet::new();
    for tag in raw.tags() {
        tags.insert(tag.to_owned());
    }
    let value = ConnectionProfile {
        id,
        group_id,
        name: raw.text(3).to_owned(),
        host: raw.text(4).to_owned(),
        port,
        username: raw.text(6).to_owned(),
        transport,
        authentication,
        credential_ref: raw
            .nullable_text(9)
            .map(|text| CredentialRef(text.to_owned())),
        identity_file: raw
            .nullable_text(10)
            .map(|text| PathBuf::from(text.to_owned())),
        host_key_policy: HostKeyPolicy::Strict,
        remote_command: raw.nullable_text(12).map(str::to_owned),
        note: raw.text(13).to_owned(),
        tags,
        position,
        terminal_profile_id,
        terminal_overrides: document.options,
    };
    raw.validate_references(Some(value.id), value.group_id)?;
    validation::connection(&value)?;
    Ok(Some(value))
}

pub(super) fn configuration(
    raw: &RawFields<'_>,
    calls: &mut ExecutionCounts,
) -> Result<(AppSettings, TerminalProfile), Failure> {
    // configuration::load 先 app_settings；mapping 先 document/version 再 UUID/color。
    let document: KeyBindingsDocument = json(raw.text(19), 19, calls)?;
    if document.version != 1 {
        return Err(Failure::Corrupt {
            tag: 19,
            json: None,
        });
    }
    let app = AppSettings {
        default_terminal_profile: TerminalProfileId(uuid(raw.text(17), 17)?),
        color_scheme: color(raw.text(18))?,
        key_bindings: document.key_bindings,
    };
    // profiles::load_profiles 先 settings，再 profile UUID；不调用写入用 validator。
    let settings: TerminalSettingsV1 = json(raw.text(22), 22, calls)?;
    let profile = TerminalProfile {
        id: TerminalProfileId(uuid(raw.text(20), 20)?),
        name: raw.text(21).to_owned(),
        settings,
    };
    Ok((app, profile))
}
