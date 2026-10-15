use rshell_m0::rshell_core::{
    AuthenticationKind, ConnectionProfile, HostKeyPolicy, TerminalOverrides, TransportKind,
};

use super::{
    ExecutionCounts, Failure, SnapshotBody, codec,
    raw::{ProfileSource, RawFields},
    validation,
};
use crate::selected_p1::model::layout::add;

pub(super) fn decode(
    raw: &RawFields<'_>,
    calls: &mut ExecutionCounts,
) -> Result<SnapshotBody, Failure> {
    let saved = codec::saved(raw, calls)?;
    let (app, profile) = codec::configuration(raw, calls)?;
    let (source, wanted) = if let Some(id) = raw.selection.explicit_profile {
        (ProfileSource::Explicit, id)
    } else if let Some(id) = saved.as_ref().and_then(|value| value.terminal_profile_id) {
        (ProfileSource::Connection, id)
    } else {
        (ProfileSource::Default, app.default_terminal_profile)
    };
    if source != raw.source {
        return Err(Failure::Protocol);
    }
    if profile.id != wanted {
        return Err(Failure::MissingProfile);
    }
    if saved
        .as_ref()
        .is_some_and(|value| Some(value.id) != raw.selection.connection)
    {
        return Err(Failure::Protocol);
    }
    if saved
        .as_ref()
        .is_some_and(|value| value.transport != raw.selection.expected_transport)
    {
        return Err(Failure::TargetMismatch);
    }
    if raw.selection.expected_transport != TransportKind::NativeSsh {
        return Err(Failure::TransportUnavailable);
    }
    let saved_flag = saved.is_some();
    let target = match saved {
        Some(value) => value,
        None => {
            let quick = raw.selection.quick.ok_or(Failure::Protocol)?;
            let (host, username) = validation::quick_target(quick).map_err(Failure::Quick)?;
            // 精确 capacity 消除 format! 的增长；结果与旧 username@host 相同。
            let mut name = String::with_capacity(add(add(username.len(), 1)?, host.len())?);
            name.push_str(username);
            name.push('@');
            name.push_str(host);
            let command = quick.command.trim();
            ConnectionProfile {
                id: quick.id,
                group_id: None,
                name,
                host: host.to_owned(),
                port: quick.port,
                username: username.to_owned(),
                transport: TransportKind::NativeSsh,
                authentication: AuthenticationKind::Password,
                credential_ref: None,
                identity_file: None,
                host_key_policy: HostKeyPolicy::Strict,
                remote_command: (!command.is_empty()).then(|| command.to_owned()),
                note: String::new(),
                tags: Default::default(),
                position: i64::MAX,
                terminal_profile_id: None,
                terminal_overrides: TerminalOverrides::default(),
            }
        }
    };
    calls.core += 1;
    let resolved = profile
        .settings
        .resolve(&target.terminal_overrides)
        .with_app_key_bindings(&app.key_bindings);
    Ok(SnapshotBody {
        target,
        saved: saved_flag,
        profile,
        app,
        resolved,
        revision: raw.revision,
        source,
        selected_references: raw.selected_references(),
    })
}
