//! supervisor 独占的准备态。加载任务仅回传结果，不能发布或复活 identity。

use crate::connect::{self, DesktopTarget, PreparedTarget};
use crate::signals::desktop_session::{
    DesktopCancel, DesktopFailure as Failure, DesktopGeometry, DesktopPreparationResult,
    DesktopPreparationState as State, DesktopPrepare, DesktopPreparedSnapshot, DesktopSource,
    DesktopStart, VERSION,
};
use crate::signals::settings::{DesktopKeyBinding, DesktopTerminalProfile};
use crate::signals::{DESKTOP_CONNECT_VERSION, DesktopConnectError, DesktopConnectRequest};
use rinf::RustSignalBinary;
use secrecy::{ExposeSecret, SecretString};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::Instant;

/// 单字体最多 32 MiB；额外预留转换为 Arc 时的短暂副本和边界二进制副本。
pub(crate) const FONT_LIMIT: usize = 32 * 1024 * 1024;
pub(crate) const METADATA_LIMIT: usize = 256 * 1024;
const RESERVATION: usize = FONT_LIMIT * 3 + METADATA_LIMIT;
pub(crate) const BYTE_LIMIT: usize = RESERVATION * 4;
pub(crate) const COUNT_LIMIT: usize = 8;
pub(crate) const INFLIGHT_LIMIT: usize = 4;
pub(crate) const TTL: Duration = Duration::from_secs(30);
const HISTORY_LIMIT: usize = 128;

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Usage {
    pub count: usize,
    pub bytes: usize,
    pub inflight: usize,
}

#[derive(Clone, Default)]
pub(crate) struct Budget(Arc<Mutex<Usage>>);

/// 同一预约可随不可取消的 I/O 一起持有，最后一个持有者释放时才归还预算。
#[derive(Clone)]
pub(crate) struct Lease(Arc<Mutex<Reservation>>);

struct Reservation {
    budget: Budget,
    bytes: usize,
    loading: bool,
}

impl Budget {
    #[cfg(test)]
    pub(crate) fn usage(&self) -> Usage {
        *self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn reserve(&self) -> Result<Lease, Failure> {
        let mut u = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if u.count >= COUNT_LIMIT {
            return Err(Failure::CountBudget);
        }
        if u.inflight >= INFLIGHT_LIMIT {
            return Err(Failure::InflightBudget);
        }
        if u.bytes + RESERVATION > BYTE_LIMIT {
            return Err(Failure::ByteBudget);
        }
        u.count += 1;
        u.inflight += 1;
        u.bytes += RESERVATION;
        Ok(Lease(Arc::new(Mutex::new(Reservation {
            budget: self.clone(),
            bytes: RESERVATION,
            loading: true,
        }))))
    }
}

impl Lease {
    fn loaded(&mut self, bytes: usize) {
        let mut reservation = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let Reservation {
            budget,
            bytes: reserved,
            loading,
        } = &mut *reservation;
        let mut u = budget.0.lock().unwrap_or_else(|e| e.into_inner());
        u.inflight -= usize::from(*loading);
        *loading = false;
        // 保留二进制发送副本的预算；共享资源重复预约是保守上界，不是少计。
        let retained = bytes * 2 + METADATA_LIMIT;
        u.bytes -= *reserved - retained;
        *reserved = retained;
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut u = self.budget.0.lock().unwrap_or_else(|e| e.into_inner());
        u.count -= 1;
        u.bytes -= self.bytes;
        u.inflight -= usize::from(self.loading);
    }
}

struct Entry {
    request: DesktopPrepare,
    identity: String,
    deadline: Instant,
    ready: Option<(PreparedTarget, Lease)>,
}

struct Ended {
    source: DesktopSource,
    request_id: u32,
    identity: String,
    failure: Failure,
}

pub(crate) struct Loaded {
    pub identity: String,
    pub result: Result<PreparedTarget, (DesktopConnectError, String)>,
    pub lease: Lease,
}

pub(crate) struct Launch {
    pub source: DesktopSource,
    pub identity: String,
    pub target: DesktopTarget,
}

#[derive(Default)]
pub(crate) struct Preparations {
    // 生产实例保持关闭；只有测试 fixture 可显式启用 V2 准备机制。
    prototype_enabled: bool,
    entries: HashMap<u32, Entry>,
    history: VecDeque<Ended>,
    pub budget: Budget,
}

impl DesktopPrepare {
    pub(crate) fn native_request(&self) -> DesktopConnectRequest {
        DesktopConnectRequest {
            version: DESKTOP_CONNECT_VERSION,
            request_id: self.request_id,
            session_id: self.source.session_id,
            target: self.target,
            profile_id: self.profile_id.clone(),
            connection_id: self.connection_id.clone(),
            host: self.host.clone(),
            port: self.port,
            username: self.username.clone(),
            password: String::new(),
            command: self.command.clone(),
            cols: 80,
            rows: 24,
            pixel_width: 0,
            pixel_height: 0,
            dpi: 96,
        }
    }
}

pub(crate) fn result(
    source: DesktopSource,
    request_id: u32,
    prepare_request_id: u32,
    identity: String,
    state: State,
    failure: Failure,
) -> DesktopPreparationResult {
    DesktopPreparationResult {
        source,
        request_id,
        prepare_request_id,
        identity,
        state,
        failure,
        remaining_ms: 0,
        snapshot: None,
    }
}

impl Preparations {
    #[cfg(test)]
    pub(crate) fn enabled_for_test() -> Self {
        Self {
            prototype_enabled: true,
            ..Self::default()
        }
    }

    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.entries.values().map(|e| e.deadline).min()
    }
    pub(crate) fn contains(&self, session: u32) -> bool {
        self.entries.contains_key(&session)
    }

    fn remember(&mut self, entry: &Entry, failure: Failure) {
        self.history.push_back(Ended {
            source: entry.request.source.clone(),
            request_id: entry.request.request_id,
            identity: entry.identity.clone(),
            failure,
        });
        if self.history.len() > HISTORY_LIMIT {
            self.history.pop_front();
        }
    }

    fn end(&mut self, session: u32, failure: Failure) -> Option<DesktopPreparationResult> {
        let entry = self.entries.remove(&session)?;
        self.remember(&entry, failure);
        Some(result(
            entry.request.source,
            entry.request.request_id,
            entry.request.request_id,
            entry.identity,
            if failure == Failure::Cancelled {
                State::Cancelled
            } else {
                State::Failed
            },
            failure,
        ))
    }

    pub(crate) fn expire(&mut self, now: Instant) -> Vec<DesktopPreparationResult> {
        let expired: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, e)| now >= e.deadline)
            .map(|(id, _)| *id)
            .collect();
        expired
            .into_iter()
            .filter_map(|id| self.end(id, Failure::Expired))
            .collect()
    }

    pub(crate) fn begin(
        &mut self,
        request: DesktopPrepare,
        now: Instant,
    ) -> Result<(String, Lease, Option<DesktopPreparationResult>), Failure> {
        self.begin_on(request, now, connect::desktop_supported())
    }

    fn begin_on(
        &mut self,
        request: DesktopPrepare,
        now: Instant,
        desktop: bool,
    ) -> Result<(String, Lease, Option<DesktopPreparationResult>), Failure> {
        if request.version != VERSION {
            return Err(Failure::UnsupportedVersion);
        }
        if !desktop {
            return Err(Failure::PlatformUnavailable);
        }
        if request.target != crate::signals::DesktopTargetKind::NativeSsh {
            return Err(Failure::TransportUnavailable);
        }
        if !self.prototype_enabled {
            return Err(Failure::PolicyUnavailable);
        }
        if request.request_id == 0 || request.source.generation == 0 {
            return Err(Failure::InvalidTarget);
        }
        if request.host.len()
            + request.username.len()
            + request.command.len()
            + request.profile_id.len()
            + request.connection_id.len()
            > 64 * 1024
        {
            return Err(Failure::ByteBudget);
        }
        for ended in &self.history {
            if ended.source.session_id == request.source.session_id {
                if ended.source.generation > request.source.generation {
                    return Err(Failure::StaleGeneration);
                }
                if ended.source == request.source && ended.request_id == request.request_id {
                    return Err(Failure::Duplicate);
                }
            }
        }
        if let Some(old) = self.entries.get(&request.source.session_id) {
            if old.request.source.generation > request.source.generation {
                return Err(Failure::StaleGeneration);
            }
            if old.request.source == request.source && old.request.request_id == request.request_id
            {
                return Err(Failure::Duplicate);
            }
        }
        // 先预约再替换，超限不破坏已有准备；被取消的加载仍持有自己的租约。
        if self.entries.len() >= COUNT_LIMIT
            && !self.entries.contains_key(&request.source.session_id)
        {
            return Err(Failure::CountBudget);
        }
        let lease = self.budget.reserve()?;
        let replaced = self.end(request.source.session_id, Failure::Replaced);
        let identity = uuid::Uuid::new_v4().to_string();
        self.entries.insert(
            request.source.session_id,
            Entry {
                request,
                identity: identity.clone(),
                deadline: now + TTL,
                ready: None,
            },
        );
        Ok((identity, lease, replaced))
    }

    pub(crate) fn complete(
        &mut self,
        mut loaded: Loaded,
        now: Instant,
    ) -> Option<(DesktopPreparationResult, Vec<u8>)> {
        let session = self
            .entries
            .iter()
            .find(|(_, e)| e.identity == loaded.identity)
            .map(|(id, _)| *id)?;
        if now >= self.entries.get(&session)?.deadline {
            return self
                .end(session, Failure::Expired)
                .map(|reply| (reply, vec![]));
        }
        let target = match loaded.result {
            Ok(target) => target,
            Err((error, detail)) => {
                let failure = if detail == "snapshot_budget" {
                    Failure::ByteBudget
                } else {
                    match error {
                        DesktopConnectError::PlatformUnavailable => Failure::PlatformUnavailable,
                        DesktopConnectError::TransportUnavailable => Failure::TransportUnavailable,
                        DesktopConnectError::ProfileNotFound
                        | DesktopConnectError::InvalidProfileId => Failure::ProfileNotFound,
                        DesktopConnectError::Storage => Failure::Storage,
                        _ => Failure::InvalidTarget,
                    }
                };
                return self.end(session, failure).map(|reply| (reply, vec![]));
            }
        };
        let entry = self.entries.get_mut(&session)?;
        let font = &target.font;
        let bytes = font
            .resource
            .as_ref()
            .map(|r| r.bytes().to_vec())
            .unwrap_or_default();
        loaded.lease.loaded(bytes.len());
        let profile = project(&target.settings, &font.projection.profile_id);
        let mut adjustments = Vec::new();
        if target.settings.font_family != font.projection.actual_family {
            adjustments.push("font_binding_projection".into());
        }
        if profile.initial_cols < 2 || profile.initial_rows < 2 {
            adjustments.push("pty_minimum_2x2".into());
        }
        let snapshot = DesktopPreparedSnapshot {
            profile,
            font: font.projection.clone(),
            font_projection: format!("{}:font", entry.identity),
            fallback_projection: format!("{}:fallback", entry.identity),
            fallback_family: crate::settings::DEFAULT_TERMINAL_FONT.into(),
            resource_bytes: bytes.len() as u32,
            adjustments,
        };
        let mut reply = result(
            entry.request.source.clone(),
            entry.request.request_id,
            entry.request.request_id,
            entry.identity.clone(),
            State::Prepared,
            Failure::None,
        );
        reply.remaining_ms = entry.deadline.saturating_duration_since(now).as_millis() as u32;
        reply.snapshot = Some(snapshot);
        entry.ready = Some((target, loaded.lease));
        Some((reply, bytes))
    }

    fn match_entry(
        &self,
        source: &DesktopSource,
        prepare: u32,
        identity: &str,
        allow_empty: bool,
    ) -> Result<&Entry, Failure> {
        if let Some(entry) = self.entries.get(&source.session_id) {
            if entry.request.source != *source {
                return Err(Failure::StaleGeneration);
            }
            if entry.request.request_id != prepare
                || !(entry.identity == identity || (allow_empty && identity.is_empty()))
            {
                return Err(Failure::IdentityMismatch);
            }
            return Ok(entry);
        }
        if self.history.iter().any(|e| {
            e.source.session_id == source.session_id && e.source.generation > source.generation
        }) {
            return Err(Failure::StaleGeneration);
        }
        if let Some(ended) = self.history.iter().rev().find(|e| {
            e.source == *source
                && e.request_id == prepare
                && (e.identity == identity || (allow_empty && identity.is_empty()))
        }) {
            return Err(ended.failure);
        }
        Err(Failure::IdentityMismatch)
    }

    pub(crate) fn cancel(
        &mut self,
        request: &DesktopCancel,
        now: Instant,
    ) -> Result<DesktopPreparationResult, Failure> {
        if !connect::desktop_supported() {
            return Err(Failure::PlatformUnavailable);
        }
        if request.version != VERSION {
            return Err(Failure::UnsupportedVersion);
        }
        let entry = self.match_entry(
            &request.source,
            request.prepare_request_id,
            &request.identity,
            true,
        )?;
        let failure = if now >= entry.deadline {
            Failure::Expired
        } else {
            Failure::Cancelled
        };
        let mut reply = self
            .end(request.source.session_id, failure)
            .ok_or(Failure::IdentityMismatch)?;
        reply.request_id = request.request_id;
        Ok(reply)
    }

    pub(crate) fn start(
        &mut self,
        request: &DesktopStart,
        password: SecretString,
        now: Instant,
    ) -> Result<Launch, Failure> {
        if !connect::desktop_supported() {
            return Err(Failure::PlatformUnavailable);
        }
        if request.version != VERSION {
            return Err(Failure::UnsupportedVersion);
        }
        if !self.prototype_enabled {
            return Err(Failure::PolicyUnavailable);
        }
        let entry = self.match_entry(
            &request.source,
            request.prepare_request_id,
            &request.identity,
            false,
        )?;
        if now >= entry.deadline {
            self.end(request.source.session_id, Failure::Expired);
            return Err(Failure::Expired);
        }
        if request.request_id == 0 || request.request_id == request.prepare_request_id {
            return Err(Failure::Duplicate);
        }
        let target = &entry.ready.as_ref().ok_or(Failure::NotPrepared)?.0;
        if target.target.saved && !password.expose_secret().is_empty() {
            return Err(Failure::InvalidTarget);
        }
        geometry(request.geometry)?;
        if request.font_projection != format!("{}:font", entry.identity)
            && request.font_projection != format!("{}:fallback", entry.identity)
        {
            return Err(Failure::InvalidFont);
        }
        let mut entry = self
            .entries
            .remove(&request.source.session_id)
            .ok_or(Failure::IdentityMismatch)?;
        self.remember(&entry, Failure::AlreadyStarted);
        let (mut target, lease) = entry.ready.take().ok_or(Failure::NotPrepared)?;
        target.settings.font_family = if request.font_projection.ends_with(":fallback") {
            crate::settings::DEFAULT_TERMINAL_FONT.into()
        } else {
            target.font.projection.actual_family.clone()
        };
        let g = request.geometry;
        let native = crate::signals::ConnectRequest {
            session_id: request.source.session_id,
            connection_id: if target.target.saved {
                target.target.profile.id.0.to_string()
            } else {
                String::new()
            },
            host: target.target.profile.host.clone(),
            port: target.target.profile.port,
            username: target.target.profile.username.clone(),
            password: String::new(),
            command: target
                .target
                .profile
                .remote_command
                .clone()
                .unwrap_or_default(),
            cols: g.cols,
            rows: g.rows,
            pixel_width: g.pixel_width,
            pixel_height: g.pixel_height,
            dpi: g.dpi,
        };
        let target = DesktopTarget {
            request: native,
            target: target.target,
            settings: target.settings,
            quick_password: password,
            font: Some(target.font),
        };
        // 字体实际引用随 target 进入运行态，不再占未消费准备预算。
        drop(lease);
        Ok(Launch {
            source: request.source.clone(),
            identity: entry.identity,
            target,
        })
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.history.clear();
    }
}

pub(crate) fn geometry(
    g: DesktopGeometry,
) -> Result<rshell_m0::rshell_core::TerminalSize, Failure> {
    if !(2..=999).contains(&g.cols)
        || !(2..=999).contains(&g.rows)
        || g.pixel_width < u32::from(g.cols)
        || g.pixel_height < u32::from(g.rows)
        || g.pixel_width > 65535
        || g.pixel_height > 65535
        || !(48..=960).contains(&g.dpi)
    {
        return Err(Failure::InvalidGeometry);
    }
    Ok(rshell_m0::rshell_core::TerminalSize {
        cols: g.cols,
        rows: g.rows,
        pixel_width: g.pixel_width,
        pixel_height: g.pixel_height,
        dpi: g.dpi,
    })
}

fn project(
    s: &rshell_m0::rshell_core::ResolvedTerminalProfile,
    id: &str,
) -> DesktopTerminalProfile {
    use rshell_m0::rshell_core::KeyCode;
    DesktopTerminalProfile {
        id: id.into(),
        name: String::new(),
        version: 1,
        terminal_type: s.terminal_type.clone(),
        initial_cols: s.cols.into(),
        initial_rows: s.rows.into(),
        scrollback_lines: s.scrollback_lines as u32,
        font_family: s.font_family.clone(),
        font_size: s.font_size.into(),
        color_scheme: serde_json::to_value(s.color_scheme)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default(),
        key_bindings: s
            .key_bindings
            .iter()
            .map(|b| DesktopKeyBinding {
                key: match b.code {
                    KeyCode::Character(c) => c.to_string(),
                    KeyCode::F(n) => format!("F{n}"),
                    _ => format!("{:?}", b.code),
                },
                shift: b.modifiers.shift,
                control: b.modifiers.control,
                alt: b.modifiers.alt,
                super_key: b.modifiers.super_key,
                action: b.action.clone(),
            })
            .collect(),
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

pub(crate) fn publish(reply: DesktopPreparationResult) {
    reply.send_signal_to_dart(Vec::new());
}

#[cfg(test)]
#[path = "desktop_prepare_tests.rs"]
mod tests;
