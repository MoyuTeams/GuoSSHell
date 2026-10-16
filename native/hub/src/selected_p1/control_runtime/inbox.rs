//! 固定入口、终结预约和纯数字历史；复制根与槽位代次独立。

use super::frame::{Geometry, Status};
use crate::selected_p1::helper_runtime::process::LoadReservations;
use crate::selected_p1::resource::{
    ledger::{Grant, SourceToken},
    owner::SharedOwner,
    secret::{CopyValidity, LockedSecretString},
};
use std::sync::{Mutex, MutexGuard};

pub const CONTROL_SLOTS: usize = 8;
pub const HISTORY_SLOTS: usize = 128;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(C)]
pub struct Key {
    pub epoch: u64,
    pub owner: u64,
    pub entry: u64,
    pub generation: u64,
    pub source_index: u32,
    pub session: u32,
    pub prepare: u32,
}

impl Key {
    pub fn source(self) -> SourceToken {
        SourceToken {
            epoch: self.epoch,
            generation: self.generation,
            index: self.source_index,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct HistoryRecord {
    pub key: Key,
    pub identity: Option<u128>,
    pub status: u32,
}

#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct PortRecord {
    pub epoch: u64,
    pub token: u64,
    pub port: i64,
    pub closed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Phase {
    Free,
    Copying,
    Queued,
    Working,
    Prepared,
    StartCopying,
    StartQueued,
    Started,
    Terminal,
}

#[repr(C)]
pub struct PrepareMeta {
    pub key: Key,
    pub request: u32,
    pub target: u32,
    pub ssh_port: u16,
    pub lengths: [usize; 5],
}
pub type PreparePayload<'r> = SharedOwner<'r, PrepareMeta>;

#[repr(C)]
pub struct Binding {
    pub(super) lengths: [usize; 3],
    pub(super) saved: bool,
}
pub type PreparedBinding<'r> = SharedOwner<'r, Binding>;

pub fn binding_field<'a>(binding: &'a PreparedBinding<'_>, index: usize) -> &'a str {
    let lengths = binding.value().lengths;
    let start: usize = lengths[..index].iter().sum();
    // 已从合法 UTF-8 同步复制，长度表是 owner 构造时生成的，不接受 wire 表。
    unsafe { std::str::from_utf8_unchecked(&binding.payload()[start..start + lengths[index]]) }
}

/// 原 identity 是 new_v4 的规范小写 hyphenated UUID；历史只保留等价数字。
pub fn identity_code(text: &str) -> Option<u128> {
    if text.len() != 36 {
        return None;
    }
    let identity = uuid::Uuid::parse_str(text).ok()?;
    let mut canonical = [0u8; 36];
    if identity.hyphenated().encode_lower(&mut canonical) != text {
        return None;
    }
    Some(u128::from_le_bytes(*identity.as_bytes()))
}

#[repr(C)]
pub struct StartOwned<'r> {
    pub request: u32,
    pub geometry: Geometry,
    pub fallback: bool,
    pub secret: LockedSecretString<'r>,
    pub reply: Grant<'r>,
}

#[repr(C)]
pub struct Slot<'r> {
    pub key: Key,
    pub identity: Option<u128>,
    pub deadline: std::time::Instant,
    pub phase: Phase,
    pub copying: u32,
    pub working: bool,
    pub terminal_ready: bool,
    pub reason: Status,
    pub validity: Option<SharedOwner<'r, CopyValidity>>,
    pub payload: Option<PreparePayload<'r>>,
    pub binding: Option<PreparedBinding<'r>>,
    pub start: Option<StartOwned<'r>>,
    pub loading: Option<LoadReservations<'r>>,
    pub reply: Option<Grant<'r>>,
    pub terminal: Option<Grant<'r>>,
}

impl Slot<'_> {
    pub fn empty() -> Self {
        Self {
            key: Key::default(),
            identity: None,
            deadline: std::time::Instant::now(),
            phase: Phase::Free,
            copying: 0,
            working: false,
            terminal_ready: false,
            reason: Status::Admitted,
            validity: None,
            payload: None,
            binding: None,
            start: None,
            loading: None,
            reply: None,
            terminal: None,
        }
    }
    pub fn invalidate(&mut self, reason: Status) {
        if let Some(validity) = &self.validity {
            validity.value().invalidate();
        }
        self.reason = reason;
        self.terminal_ready = true;
        self.phase = Phase::Terminal;
    }
}

#[repr(C)]
pub struct InboxState<'r> {
    pub slots: [Slot<'r>; CONTROL_SLOTS],
    pub history: [HistoryRecord; HISTORY_SLOTS],
    pub history_next: usize,
    pub normal_next: usize,
    pub terminal_next: usize,
    pub port: PortRecord,
    pub next_token: u64,
}

impl InboxState<'_> {
    pub fn initial() -> Self {
        Self {
            slots: std::array::from_fn(|_| Slot::empty()),
            history: [HistoryRecord::default(); HISTORY_SLOTS],
            history_next: 0,
            normal_next: 0,
            terminal_next: 0,
            port: PortRecord::default(),
            next_token: 1,
        }
    }
    pub fn remember(&mut self, key: Key, identity: Option<u128>, status: Status) {
        self.history[self.history_next] = HistoryRecord {
            key,
            identity,
            status: status as u32,
        };
        self.history_next = (self.history_next + 1) % HISTORY_SLOTS;
    }
}

#[repr(C)]
pub struct InboxBody<'r> {
    pub state: Mutex<InboxState<'r>>,
}

pub fn lock<'a, 'r>(body: &'a InboxBody<'r>) -> MutexGuard<'a, InboxState<'r>> {
    body.state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub struct PrepareWork<'r> {
    pub key: Key,
    pub payload: PreparePayload<'r>,
    pub loading: LoadReservations<'r>,
    pub reply: Grant<'r>,
}
pub struct StartTransfer<'r> {
    pub key: Key,
    pub start: StartOwned<'r>,
    pub binding: PreparedBinding<'r>,
    pub terminal: Grant<'r>,
}
pub struct TerminalWork<'r> {
    pub key: Key,
    pub reason: Status,
    pub permit: Grant<'r>,
}
pub struct WorkBatch<'r> {
    pub terminal: Option<TerminalWork<'r>>,
    pub normal: Option<NormalWork<'r>>,
}
pub enum NormalWork<'r> {
    Prepare(PrepareWork<'r>),
    Start(StartTransfer<'r>),
}
