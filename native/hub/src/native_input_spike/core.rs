use super::{effects::Effects, payload::TicketRef};
use crate::desktop_input_policy::{InputPolicyState, PolicyKeyInput};
use crate::signals::desktop_session::{DesktopAction, DesktopKeyPhase};
use rshell_m0::rshell_core::{
    KeyBinding, KeyCode, KeyModifiers, ResolvedTerminalProfile, TerminalOverrides,
    TerminalSettingsV1, TerminalSize,
};
use rshell_m0::rshell_session::DefaultTerminalEngine;
use std::{
    num::NonZeroU64,
    sync::{
        Arc, Mutex, TryLockError,
        atomic::{AtomicBool, Ordering},
    },
    thread::{JoinHandle, ThreadId},
};
use tokio::io::DuplexStream;

pub(super) const OK: u32 = 0;
pub(super) const DISABLED: u32 = 1;
pub(super) const BUSY: u32 = 2;
pub(super) const STALE_OWNER: u32 = 3;
pub(super) const INVALID: u32 = 4;
pub(super) const INVALID_SOURCE: u32 = 5;
pub(super) const UNSUPPORTED: u32 = 6;
pub(super) const STOPPED: u32 = 7;
pub(super) const EXHAUSTED: u32 = 8;
pub(super) const ENGINE_FAILURE: u32 = 9;
pub(super) const TRANSPORT_FAILURE: u32 = 10;
pub(super) const PRODUCTION_NOT_READY: u32 = 11;
pub(super) const SCOPED_TRANSPORT_UNAVAILABLE: u32 = 12;
pub(super) const TEXT_CAPACITY: u32 = 13;
pub(super) const ACTIVE_KEY_BUDGET: u32 = 14;

// fixture scope：四个活 cycle（含延期 Up）、八个普通 effect、两个远端槽。
// 每个 cycle 的 Release 与每个 ticket 的 completion/cancel 不占普通槽。
// Root 仍保留八个 lifetime／两个 worker；不将本预算作为生产 H 证书。
pub(super) const ACTIVE_KEYS: usize = 4;
pub(super) const UI_KEYS: usize = 8;
pub(super) const NORMAL_SLOTS: usize = 8;
pub(super) const REMOTE_START: usize = NORMAL_SLOTS + ACTIVE_KEYS;
pub(super) const SLOTS: usize = REMOTE_START + 2;
pub(super) const SLOT_BYTES: usize = 32;
pub(super) const STREAM_BYTES: usize = 128;
pub(super) const REMOTE_LIMIT: u64 = 32;
// 输入域是显式 fixture records；不是通用远端 parser 或 production stream。
// 字符内容仅 ASCII，禁任意 OSC/零宽串；32 次 lifetime 限制协商栈与同步暂存。
pub(super) const MODES: [&[u8]; 18] = [
    b"\x1b[?u",
    b"\x1b[>1u",
    b"\x1b[>2u",
    b"\x1b[>3u",
    b"\x1b[=1u",
    b"\x1b[=2u",
    b"\x1b[=3u",
    b"\x1b[<u",
    b"\x1b[=0u",
    b"\x1b[?2026hA",
    b"\x1b[?2026l\x1b[?2026hB",
    b"\x1b[?2026l",
    b"x",
    b"\x1b[?2004h",
    b"\x1b[?2004l",
    b"hello",
    b"\x1b[?1000h\x1b[?1006h",
    b"\x1b[?1000l",
];
pub(super) const READ_BYTES: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Origin {
    DeclaredSynthetic(u64),
    VerifiedNative { platform: u32, registration: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Binding {
    pub handle: u64,
    pub context: usize,
    pub session: u32,
    pub generation: u64,
    pub identity: [u8; 16],
    pub ui_thread: ThreadId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Admission {
    pub status: u32,
    pub disposition: u32,
    pub seq: u64,
    pub action: u32,
    pub bytes: u32,
    pub effect: u64,
    pub ticket: TicketRef,
}

impl Admission {
    pub fn error(status: u32) -> Self {
        Self {
            status,
            disposition: 0,
            seq: 0,
            action: 0,
            bytes: 0,
            effect: 0,
            ticket: TicketRef::default(),
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Packet {
    pub seq: u64,
    pub bytes: [u8; SLOT_BYTES],
    pub len: usize,
}

/// 已捕获 fixture 的 profile/font/viewport/source revision；无正常 Prepare 入口。
pub(super) struct TestProfileLease {
    profile: ResolvedTerminalProfile,
    pub revision: u64,
    pub size: TerminalSize,
    pub cell_width: u32,
    pub cell_height: u32,
}

impl TestProfileLease {
    fn captured() -> Self {
        let settings = TerminalSettingsV1 {
            scrollback_lines: 0,
            enable_kitty_keyboard: true,
            enable_csi_u: false,
            answerback: String::new(),
            key_bindings: [
                "clear_scrollback",
                "new_tab",
                "split_vertical",
                "send:\x1b[3~",
                "send:\x7f",
                "send:\x08",
            ]
            .into_iter()
            .enumerate()
            .map(|(i, action)| KeyBinding {
                code: KeyCode::F((i + 1) as u8),
                modifiers: KeyModifiers::default(),
                action: action.into(),
            })
            .collect(),
            ..TerminalSettingsV1::default()
        };
        Self {
            profile: settings.resolve(&TerminalOverrides::default()),
            revision: 1,
            size: TerminalSize {
                cols: 12,
                rows: 3,
                pixel_width: 120,
                pixel_height: 60,
                dpi: 96,
            },
            cell_width: 10,
            cell_height: 20,
        }
    }
}

pub(super) struct Core {
    pub engine: DefaultTerminalEngine,
    lease: TestProfileLease,
    policy: InputPolicyState,
    pub effects: Effects,
    pub origin: Origin,
    pub scope_budget: super::budget::ScopeBudget,
    pub next_seq: u64,
    pub remote_applied: u64,
    pub last_written_seq: u64,
    #[cfg(test)]
    pub panic_key: bool,
}

impl Core {
    fn new(binding: Binding) -> Result<Self, u32> {
        let scope_budget = super::budget::manifest()?;
        let lease = TestProfileLease::captured();
        let engine =
            DefaultTerminalEngine::new(&lease.profile, lease.size).map_err(|_| ENGINE_FAILURE)?;
        let effects = Effects::new(binding, lease.size);
        Ok(Self {
            engine,
            lease,
            policy: InputPolicyState::default(),
            effects,
            origin: Origin::DeclaredSynthetic(binding.handle),
            scope_budget,
            next_seq: 1,
            remote_applied: 0,
            last_written_seq: 0,
            #[cfg(test)]
            panic_key: false,
        })
    }
    fn key(&mut self, input: PolicyKeyInput) -> Result<Admission, u32> {
        #[cfg(test)]
        if self.panic_key {
            self.panic_key = false;
            panic!("固定 key 持锁区的单次 panic 注入");
        }
        self.effects.key(
            &mut self.policy,
            &self.lease.profile,
            &mut self.next_seq,
            input,
        )
    }
    pub fn remote(&mut self, bytes: &[u8]) -> Result<bool, u32> {
        self.effects.remote(&mut self.next_seq, bytes)
    }
    pub fn take_write(&mut self) -> Result<Option<(usize, Packet)>, u32> {
        let result = self.effects.take_write(&mut self.engine);
        self.remote_applied = self.effects.remote_applied;
        result
    }
    pub fn finish_write(&mut self, slot: usize, seq: u64) -> Result<(), u32> {
        self.effects
            .finish_write(&self.engine, &mut self.next_seq, slot, seq)?;
        self.last_written_seq = self.effects.last_written;
        Ok(())
    }
    pub fn charge(&self) -> (u32, u32) {
        self.effects.charge()
    }
    pub fn captured_geometry(&self) -> (u64, u32, u32) {
        (
            self.lease.revision,
            self.lease.cell_width,
            self.lease.cell_height,
        )
    }
    pub fn discard(&mut self) {
        self.effects.discard();
        self.policy = InputPolicyState::default();
    }
}

#[derive(Clone)]
enum Life {
    Rejected,
    Accepted,
    PendingUp(PolicyKeyInput),
}
struct UiKey {
    id: NonZeroU64,
    life: Life,
}
struct Ledger {
    keys: [Option<UiKey>; UI_KEYS],
}
impl Default for Ledger {
    fn default() -> Self {
        Self {
            keys: std::array::from_fn(|_| None),
        }
    }
}

pub(super) struct Owner {
    pub binding: Binding,
    pub core: Mutex<Core>,
    ui: Mutex<Ledger>,
    pub peer: Mutex<DuplexStream>,
    pub stopped: AtomicBool,
    pub worker_done: AtomicBool,
    pub worker_joined: AtomicBool,
    pub worker: Mutex<Option<JoinHandle<()>>>,
    pub projection: Mutex<()>,
    pub pending_cancel: Mutex<[Option<TicketRef>; super::payload::TICKETS]>,
    pub pending_revoke: AtomicBool,
    #[cfg(test)]
    pub key_entry: Mutex<Option<Arc<TestGate>>>,
    #[cfg(test)]
    pub thread_tail: Mutex<Option<Arc<TestGate>>>,
    #[cfg(test)]
    pub write_boundary: Mutex<Option<Arc<TestGate>>>,
    #[cfg(test)]
    pub write_pending: Mutex<Option<Arc<TestGate>>>,
    #[cfg(test)]
    pub write_complete: Mutex<Option<Arc<TestGate>>>,
}

#[cfg(test)]
pub(super) struct TestGate {
    pub entered: std::sync::Barrier,
    pub release: std::sync::Barrier,
}
#[cfg(test)]
pub(super) fn test_pause(gate: &Mutex<Option<Arc<TestGate>>>) {
    let gate = gate.lock().map(|g| g.clone()).ok().flatten();
    if let Some(gate) = gate {
        gate.entered.wait();
        gate.release.wait();
    }
}

impl Owner {
    pub fn new(binding: Binding, peer: DuplexStream) -> Result<Arc<Self>, u32> {
        assert_send::<DefaultTerminalEngine>();
        assert_send::<Core>();
        assert_send::<Owner>();
        let owner = Arc::new(Self {
            binding,
            core: Mutex::new(Core::new(binding)?),
            ui: Mutex::new(Ledger::default()),
            peer: Mutex::new(peer),
            stopped: AtomicBool::new(false),
            worker_done: AtomicBool::new(false),
            worker_joined: AtomicBool::new(false),
            worker: Mutex::new(None),
            projection: Mutex::new(()),
            pending_cancel: Mutex::new([None; super::payload::TICKETS]),
            pending_revoke: AtomicBool::new(false),
            #[cfg(test)]
            key_entry: Mutex::new(None),
            #[cfg(test)]
            thread_tail: Mutex::new(None),
            #[cfg(test)]
            write_boundary: Mutex::new(None),
            #[cfg(test)]
            write_pending: Mutex::new(None),
            #[cfg(test)]
            write_complete: Mutex::new(None),
        });
        {
            let mut core = owner.core.lock().map_err(|_| STOPPED)?;
            core.effects.sync = super::deadline::SyncTimer::new(
                (&core.engine as *const _) as usize as u64,
                binding.generation,
            );
        }
        Ok(owner)
    }
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
    }
    pub fn poisoned(&self) -> bool {
        self.ui.is_poisoned()
            || self.core.is_poisoned()
            || self.peer.is_poisoned()
            || self.worker.is_poisoned()
    }
    pub fn retire(&self) {
        self.stop();
        let mut ledger = self.ui.lock().unwrap_or_else(|e| e.into_inner());
        let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
        core.discard();
        *ledger = Ledger::default();
        *self
            .pending_cancel
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = [None; super::payload::TICKETS];
        drop(core);
        drop(ledger);
        self.worker_done.store(true, Ordering::Release);
    }
    pub fn join_worker(&self) -> u32 {
        if self.binding.ui_thread == std::thread::current().id() {
            return INVALID_SOURCE;
        }
        if !self.stopped.load(Ordering::Acquire) {
            return BUSY;
        }
        let mut worker = match self.worker.lock() {
            Ok(worker) => worker,
            Err(_) => {
                self.stop();
                return STOPPED;
            }
        };
        if self.worker_joined.load(Ordering::Acquire) {
            return OK;
        }
        let Some(handle) = worker.take() else {
            return INVALID;
        };
        // 仅非 UI 管理线程 join 实际线程，worker_done 不返还 Root worker 名额。
        let result = handle.join();
        self.worker_joined.store(true, Ordering::Release);
        if result.is_ok() { OK } else { STOPPED }
    }
    pub fn key(
        &self,
        id: NonZeroU64,
        phase: DesktopKeyPhase,
        input: Result<PolicyKeyInput, u32>,
    ) -> Admission {
        if self.stopped.load(Ordering::Acquire) {
            return Admission::error(STOPPED);
        }
        #[cfg(test)]
        test_pause(&self.key_entry);
        if self.binding.ui_thread != std::thread::current().id() {
            self.stop();
            return Admission::error(INVALID_SOURCE);
        }
        let mut ledger = match self.ui.try_lock() {
            Ok(ledger) => ledger,
            Err(_) => {
                self.stop();
                return Admission::error(STOPPED);
            }
        };
        if self.stopped.load(Ordering::Acquire) {
            return Admission::error(STOPPED);
        }
        let existing = ledger
            .keys
            .iter()
            .position(|k| k.as_ref().is_some_and(|k| k.id == id));
        let slot = match (phase, existing) {
            (DesktopKeyPhase::Down, None) => {
                let cancel_pending = match self.pending_cancel.try_lock() {
                    Ok(mailbox) => mailbox.iter().any(Option::is_some),
                    Err(_) => {
                        self.stop();
                        return Admission::error(STOPPED);
                    }
                };
                if cancel_pending
                    || ledger
                        .keys
                        .iter()
                        .flatten()
                        .any(|k| matches!(k.life, Life::PendingUp(_)))
                {
                    // 保留新 Down 的拒绝记录；不能因为 pending lifecycle 把它交 IME。
                    let Some(slot) = ledger.keys.iter().position(Option::is_none) else {
                        self.stop();
                        return Admission::error(STOPPED);
                    };
                    ledger.keys[slot] = Some(UiKey {
                        id,
                        life: Life::Rejected,
                    });
                    return Admission::error(BUSY);
                }
                let Some(slot) = ledger.keys.iter().position(Option::is_none) else {
                    self.stop();
                    return Admission::error(STOPPED);
                };
                ledger.keys[slot] = Some(UiKey {
                    id,
                    life: Life::Rejected,
                });
                slot
            }
            (DesktopKeyPhase::Down, Some(_)) | (_, None) => return Admission::error(INVALID),
            (_, Some(slot)) => slot,
        };
        if phase != DesktopKeyPhase::Down
            && ledger.keys[slot]
                .as_ref()
                .is_some_and(|k| matches!(k.life, Life::Rejected))
        {
            if phase == DesktopKeyPhase::Up {
                ledger.keys[slot] = None;
            }
            return Admission::error(INVALID);
        }
        if ledger.keys[slot]
            .as_ref()
            .is_some_and(|k| matches!(k.life, Life::PendingUp(_)))
        {
            return Admission::error(BUSY);
        }
        let input = match input {
            Ok(input) => input,
            Err(status) => {
                if phase == DesktopKeyPhase::Up {
                    self.stop();
                }
                return Admission::error(status);
            }
        };
        let mut core = match self.core.try_lock() {
            Ok(core) => core,
            Err(TryLockError::WouldBlock) => {
                if phase == DesktopKeyPhase::Up {
                    ledger.keys[slot] = Some(UiKey {
                        id,
                        life: Life::PendingUp(input),
                    });
                }
                return Admission::error(BUSY);
            }
            Err(TryLockError::Poisoned(_)) => {
                self.stop();
                return Admission::error(STOPPED);
            }
        };
        if self.stopped.load(Ordering::Acquire) {
            return Admission::error(STOPPED);
        }
        match core.key(input) {
            Ok(admission) => {
                ledger.keys[slot] = if phase == DesktopKeyPhase::Up {
                    None
                } else {
                    Some(UiKey {
                        id,
                        life: if admission.status == OK {
                            Life::Accepted
                        } else {
                            Life::Rejected
                        },
                    })
                };
                admission
            }
            Err(status) => {
                if !matches!(
                    status,
                    BUSY | INVALID | STALE_OWNER | ACTIVE_KEY_BUDGET | TEXT_CAPACITY
                ) {
                    self.stop();
                }
                Admission::error(status)
            }
        }
    }
    pub fn release_pending(&self) -> Result<bool, u32> {
        let mut ledger = self.ui.lock().map_err(|_| {
            self.stop();
            STOPPED
        })?;
        let mut core = self.core.lock().map_err(|_| {
            self.stop();
            STOPPED
        })?;
        if self.stopped.load(Ordering::Acquire) {
            return Err(STOPPED);
        }
        if self.pending_revoke.swap(false, Ordering::AcqRel) {
            core.effects.revoke();
            super::effects::sequence(&mut core.next_seq)?;
            *ledger = Ledger::default();
            self.stop();
            return Err(STOPPED);
        }
        for slot in &mut ledger.keys {
            if let Some(UiKey {
                life: Life::PendingUp(input),
                ..
            }) = slot
            {
                core.key(input.clone())?;
                *slot = None;
            }
        }
        let mut cancels = self.pending_cancel.lock().map_err(|_| STOPPED)?;
        for pending in cancels.iter_mut() {
            if let Some(ticket) = pending.take() {
                // 旧票据可能已被显式完成／撤销；仅结算原通知，不转贴新 txn。
                if core.effects.payloads.cancel(ticket).is_ok() {
                    core.effects.ticket_finished(ticket);
                }
                super::effects::sequence(&mut core.next_seq)?;
            }
        }
        Ok(true)
    }
    pub fn defer_cancel(&self, ticket: TicketRef) -> u32 {
        if ticket.owner != self.binding.handle || ticket.context != self.binding.context as u64 {
            return INVALID_SOURCE;
        }
        let mut mailbox = match self.pending_cancel.try_lock() {
            Ok(mailbox) => mailbox,
            Err(_) => {
                self.stop();
                return STOPPED;
            }
        };
        if mailbox.contains(&Some(ticket)) {
            return BUSY;
        }
        if let Some(cell) = mailbox.iter_mut().find(|cell| cell.is_none()) {
            *cell = Some(ticket);
            BUSY
        } else {
            self.stop();
            STOPPED
        }
    }
}

fn assert_send<T: Send>() {}

pub(super) fn action_number(action: DesktopAction) -> u32 {
    match action {
        DesktopAction::NewTab => 1,
        DesktopAction::SplitRight => 2,
        DesktopAction::SplitDown => 3,
        DesktopAction::ClosePane => 4,
        DesktopAction::NextTab => 5,
        DesktopAction::PreviousTab => 6,
        DesktopAction::NextPane => 7,
        DesktopAction::Search => 8,
        DesktopAction::Settings => 9,
        DesktopAction::ToggleSidebar => 10,
        DesktopAction::Copy => 11,
        DesktopAction::Paste => 12,
        DesktopAction::SelectAll => 13,
        DesktopAction::ZoomIn => 14,
        DesktopAction::ZoomOut => 15,
        DesktopAction::ZoomReset => 16,
    }
}
