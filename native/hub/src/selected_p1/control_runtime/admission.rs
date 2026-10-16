//! 五操作完整准入；所有 owned 复制都在额度和可取消 claim 之后。

use super::{
    frame::{BorrowEntry, Command, ControlFrame, RESULT_WORDS, Status},
    inbox::{
        self, Binding, InboxBody, Key, NormalWork, Phase, PrepareMeta, PrepareWork, Slot,
        StartOwned, StartTransfer, TerminalWork, WorkBatch,
    },
    persistent,
};
use crate::selected_p1::{
    helper_runtime::process::LoadCosts,
    model::layout::{NumericReject, add},
    resource::{
        ledger::{Admission, Amount, Grant, ProcessLedger, ResourceClass},
        owner::{OwnerLayout, SharedOwner},
        secret::{CopyValidity, LockedSecretString},
    },
};

#[derive(Clone, Copy)]
pub struct PrepareCosts {
    pub loading: LoadCosts,
    pub reply: Amount,
    pub start_reply: Amount,
    pub terminal_reply: Amount,
}

pub struct ControlRuntime<'r> {
    ledger: &'r ProcessLedger,
    backing: SharedOwner<'r, InboxBody<'r>>,
    costs: PrepareCosts,
}

impl Drop for ControlRuntime<'_> {
    fn drop(&mut self) {
        let mut state = inbox::lock(self.backing.value());
        if state.port.port != 0 && !state.port.closed {
            let epoch = state.port.epoch;
            let _ = self.ledger.advance_epoch(epoch);
            state.port.closed = true;
            for slot in &mut state.slots {
                if !matches!(slot.phase, Phase::Free | Phase::Started) {
                    slot.invalidate(Status::Cancelled);
                }
            }
        }
    }
}

fn reject(error: NumericReject) -> Status {
    match error {
        NumericReject::AllocationFailure => Status::Allocation,
        NumericReject::EpochMismatch
        | NumericReject::SourceMismatch
        | NumericReject::InvalidSource => Status::Stale,
        NumericReject::Cancelled => Status::Cancelled,
        _ => Status::Budget,
    }
}

fn amount(bytes: usize, slots: u32) -> Amount {
    Amount {
        rust_bytes: bytes,
        c_bytes: 0,
        slots,
    }
}

impl<'r> ControlRuntime<'r> {
    pub fn new(
        ledger: &'r ProcessLedger,
        epoch: u64,
        costs: PrepareCosts,
    ) -> Result<Self, NumericReject> {
        Ok(Self {
            ledger,
            backing: persistent::allocate(ledger, epoch)?,
            costs,
        })
    }
    pub fn backing_layout(&self) -> Result<OwnerLayout, NumericReject> {
        persistent::inbox_layout()
    }
    pub fn submit(&self, frame: BorrowEntry<'_>) -> [u64; RESULT_WORDS] {
        let result = match frame.command {
            Command::Prepare | Command::Start => {
                self.claim(frame).and_then(|claim| claim.copy_and_install())
            }
            Command::Cancel => self.cancel(frame),
            Command::RegisterReplyPort => self.register(frame),
            Command::CloseReplyPort => self.close(frame),
        };
        match result {
            Ok(key) => [
                0,
                key.epoch,
                key.owner,
                key.entry,
                key.generation,
                u64::from(key.source_index),
                u64::from(key.session),
                u64::from(key.prepare),
            ],
            Err(status) => [status as u64, 0, 0, 0, 0, 0, 0, 0],
        }
    }
    fn register(&self, frame: ControlFrame<'_>) -> Result<Key, Status> {
        let mut state = inbox::lock(self.backing.value());
        if self.ledger.snapshot().epoch != frame.epoch {
            return Err(Status::Stale);
        }
        if state.port.port != 0 {
            if !state.port.closed
                && state.port.epoch == frame.epoch
                && state.port.port == frame.port
                && state.port.token == frame.owner
            {
                return Ok(Key {
                    epoch: frame.epoch,
                    owner: frame.owner,
                    ..Key::default()
                });
            }
            if !state.port.closed {
                return Err(Status::Duplicate);
            }
        }
        if frame.owner != 0 {
            return Err(Status::Stale);
        }
        let owner = state.next_token;
        state.next_token = owner.checked_add(1).ok_or(Status::Unavailable)?;
        state.port = inbox::PortRecord {
            epoch: frame.epoch,
            token: owner,
            port: frame.port,
            closed: false,
        };
        Ok(Key {
            epoch: frame.epoch,
            owner,
            ..Key::default()
        })
    }
    fn port_valid(state: &inbox::InboxState<'_>, frame: ControlFrame<'_>) -> bool {
        !state.port.closed
            && state.port.port != 0
            && state.port.epoch == frame.epoch
            && state.port.token == frame.owner
    }
    fn close(&self, frame: ControlFrame<'_>) -> Result<Key, Status> {
        let mut state = inbox::lock(self.backing.value());
        if state.port.epoch != frame.epoch
            || state.port.token != frame.owner
            || state.port.port != frame.port
        {
            return Err(Status::Stale);
        }
        if !state.port.closed {
            self.ledger.advance_epoch(frame.epoch).map_err(reject)?;
            state.port.closed = true;
            for slot in &mut state.slots {
                if slot.phase != Phase::Free && slot.phase != Phase::Started {
                    slot.invalidate(Status::Cancelled);
                }
            }
        }
        Ok(Key {
            epoch: frame.epoch,
            owner: frame.owner,
            ..Key::default()
        })
    }
    fn matching(state: &inbox::InboxState<'_>, frame: ControlFrame<'_>) -> Result<usize, Status> {
        state
            .slots
            .iter()
            .position(|s| {
                s.phase != Phase::Free
                    && s.key.epoch == frame.epoch
                    && s.key.owner == frame.owner
                    && s.key.entry == frame.entry
                    && s.key.generation == frame.source.generation
                    && s.key.source_index == frame.source.index
                    && s.key.session == frame.session
                    && s.key.prepare == frame.prepare
            })
            .ok_or_else(|| {
                if state.history.iter().any(|h| {
                    h.key.entry != 0
                        && h.key.epoch == frame.epoch
                        && h.key.entry == frame.entry
                        && h.key.owner == frame.owner
                        && h.key.source_index == frame.source.index
                        && h.key.generation == frame.source.generation
                        && h.key.session == frame.session
                        && h.key.prepare == frame.prepare
                        && h.status == Status::AlreadyStarted as u32
                        && h.identity.is_some()
                        && h.identity == inbox::identity_code(frame.fields[0])
                }) {
                    Status::AlreadyStarted
                } else {
                    Status::Stale
                }
            })
    }
    fn cancel(&self, frame: ControlFrame<'_>) -> Result<Key, Status> {
        let mut state = inbox::lock(self.backing.value());
        let index = match Self::matching(&state, frame) {
            Ok(index) => index,
            Err(status) => {
                if let Some(history) = state.history.iter().find(|h| {
                    h.key.entry == frame.entry
                        && h.key.entry != 0
                        && h.key.epoch == frame.epoch
                        && h.key.owner == frame.owner
                        && h.key.session == frame.session
                        && h.key.source_index == frame.source.index
                        && h.key.generation == frame.source.generation
                        && h.key.prepare == frame.prepare
                }) {
                    if !frame.fields[0].is_empty()
                        && (history.identity.is_none()
                            || history.identity != inbox::identity_code(frame.fields[0]))
                    {
                        return Err(Status::Identity);
                    }
                    return if history.status == Status::AlreadyStarted as u32 {
                        Err(Status::AlreadyStarted)
                    } else {
                        Ok(history.key)
                    };
                }
                return Err(status);
            }
        };
        let slot = &mut state.slots[index];
        if slot.phase == Phase::Started {
            return Err(Status::AlreadyStarted);
        }
        if !frame.fields[0].is_empty()
            && slot
                .binding
                .as_ref()
                .is_none_or(|b| inbox::binding_field(b, 0) != frame.fields[0])
        {
            return Err(Status::Identity);
        }
        if slot.phase != Phase::Terminal {
            slot.invalidate(Status::Cancelled);
        }
        Ok(slot.key)
    }
    /// claim 不持有借用 payload；可由专用测试在 copy 与 Cancel 之间设置屏障。
    pub fn claim<'a, 'f>(
        &'a self,
        frame: ControlFrame<'f>,
    ) -> Result<CopyClaim<'a, 'r, 'f>, Status> {
        let mut state = inbox::lock(self.backing.value());
        if !Self::port_valid(&state, frame) {
            return Err(Status::Stale);
        }
        let admission = Admission {
            epoch: frame.epoch,
            source: Some(frame.source),
        };
        match frame.command {
            Command::Prepare => {
                if state.history.iter().any(|h| {
                    h.key.epoch == frame.epoch
                        && h.key.session == frame.session
                        && h.key.generation == frame.source.generation
                        && h.key.prepare == frame.request
                        && h.key.entry != 0
                }) || state.slots.iter().any(|s| {
                    s.phase != Phase::Free
                        && s.key.epoch == frame.epoch
                        && s.key.session == frame.session
                        && s.key.generation == frame.source.generation
                        && s.key.prepare == frame.request
                }) {
                    return Err(Status::Duplicate);
                }
                if state.slots.iter().any(|s| {
                    s.phase != Phase::Free
                        && s.key.epoch == frame.epoch
                        && s.key.session == frame.session
                        && s.key.generation > frame.source.generation
                }) {
                    return Err(Status::Stale);
                }
                let index = state
                    .slots
                    .iter()
                    .position(|s| s.phase == Phase::Free)
                    .ok_or(Status::Full)?;
                let payload_len = frame.fields[..5]
                    .iter()
                    .try_fold(0, |n, s| add(n, s.len()))
                    .map_err(reject)?;
                let payload_layout =
                    OwnerLayout::actual::<PrepareMeta>(payload_len).map_err(reject)?;
                let validity_layout = OwnerLayout::actual::<CopyValidity>(0).map_err(reject)?;
                // 所有预约先完成；中途拒绝时局部 Grant 自动退还，尚未发生 allocation/copy。
                let payload = self
                    .ledger
                    .try_reserve(
                        amount(payload_layout.backing.size(), 0),
                        ResourceClass::Control,
                        admission,
                    )
                    .map_err(reject)?;
                let validity = self
                    .ledger
                    .try_reserve(
                        amount(validity_layout.backing.size(), 1),
                        ResourceClass::Control,
                        admission,
                    )
                    .map_err(reject)?;
                let loading = self
                    .costs
                    .loading
                    .reserve(self.ledger, admission)
                    .map_err(reject)?;
                let reply = self
                    .ledger
                    .try_reserve(self.costs.reply, ResourceClass::Reply, admission)
                    .map_err(reject)?;
                let terminal = self
                    .ledger
                    .try_reserve(self.costs.terminal_reply, ResourceClass::Reply, admission)
                    .map_err(reject)?;
                let validity =
                    SharedOwner::try_init(validity, 0, 0, |_| Ok(CopyValidity::default()))
                        .map_err(reject)?;
                let copy_validity = validity.try_clone().map_err(reject)?;
                let entry = state.next_token;
                state.next_token = entry.checked_add(1).ok_or(Status::Unavailable)?;
                let key = Key {
                    epoch: frame.epoch,
                    owner: frame.owner,
                    entry,
                    generation: frame.source.generation,
                    source_index: frame.source.index,
                    session: frame.session,
                    prepare: frame.request,
                };
                let deadline = std::time::Instant::now()
                    .checked_add(std::time::Duration::from_secs(30))
                    .ok_or(Status::Unavailable)?;
                for old in &mut state.slots {
                    if old.phase != Phase::Free
                        && old.key.session == frame.session
                        && old.phase != Phase::Started
                    {
                        old.invalidate(Status::Cancelled);
                    }
                }
                state.slots[index] = Slot {
                    key,
                    deadline,
                    phase: Phase::Copying,
                    copying: 1,
                    validity: Some(validity),
                    loading: Some(loading),
                    reply: Some(reply),
                    terminal: Some(terminal),
                    ..Slot::empty()
                };
                Ok(CopyClaim {
                    runtime: self,
                    frame,
                    index,
                    key,
                    validity: copy_validity,
                    kind: Some(ClaimKind::Prepare {
                        payload,
                        lengths: std::array::from_fn(|i| frame.fields[i].len()),
                        target: frame.target,
                        ssh_port: frame.ssh_port,
                    }),
                    finished: false,
                })
            }
            Command::Start => {
                let index = Self::matching(&state, frame)?;
                let slot = &mut state.slots[index];
                if slot.deadline <= std::time::Instant::now() {
                    slot.invalidate(Status::Expired);
                    return Err(Status::Expired);
                }
                let binding = slot.binding.as_ref().ok_or(Status::NotPrepared)?;
                if frame.fields[0] != inbox::binding_field(binding, 0) {
                    return Err(Status::Identity);
                }
                if matches!(
                    slot.phase,
                    Phase::StartCopying | Phase::StartQueued | Phase::Started
                ) {
                    return Err(Status::AlreadyStarted);
                }
                if slot.phase != Phase::Prepared {
                    return Err(Status::NotPrepared);
                }
                let fallback = if frame.fields[1] == inbox::binding_field(binding, 1) {
                    false
                } else if frame.fields[1] == inbox::binding_field(binding, 2) {
                    true
                } else {
                    return Err(Status::Identity);
                };
                if !frame.geometry.valid() {
                    return Err(Status::Geometry);
                }
                if binding.value().saved && !frame.secret.is_empty() {
                    return Err(Status::Invalid);
                }
                let secret = self
                    .ledger
                    .try_reserve(
                        amount(frame.secret.len(), 1),
                        ResourceClass::Secret,
                        admission,
                    )
                    .map_err(reject)?;
                let reply = self
                    .ledger
                    .try_reserve(self.costs.start_reply, ResourceClass::Reply, admission)
                    .map_err(reject)?;
                let validity = slot
                    .validity
                    .as_ref()
                    .ok_or(Status::Stale)?
                    .try_clone()
                    .map_err(reject)?;
                slot.phase = Phase::StartCopying;
                slot.copying = 1;
                Ok(CopyClaim {
                    runtime: self,
                    frame,
                    index,
                    key: slot.key,
                    validity,
                    kind: Some(ClaimKind::Start {
                        secret,
                        reply,
                        request: frame.request,
                        geometry: frame.geometry,
                        fallback,
                    }),
                    finished: false,
                })
            }
            _ => Err(Status::Invalid),
        }
    }
    /// 父以真实准备结果传入 identity 和两份投影；此入口自行先预约后复制。
    pub fn install_prepared(
        &self,
        key: Key,
        identity: &str,
        primary: &str,
        fallback: &str,
        saved: bool,
    ) -> Result<(), Status> {
        let identity = (
            identity,
            inbox::identity_code(identity).ok_or(Status::Identity)?,
        );
        let len = [identity.0, primary, fallback]
            .iter()
            .try_fold(0, |n, s| add(n, s.len()))
            .map_err(reject)?;
        let layout = OwnerLayout::actual::<Binding>(len).map_err(reject)?;
        let permit = self
            .ledger
            .try_reserve(
                amount(layout.backing.size(), 0),
                ResourceClass::Control,
                Admission {
                    epoch: key.epoch,
                    source: Some(key.source()),
                },
            )
            .map_err(reject)?;
        let binding = SharedOwner::try_init(permit, len, 0, |payload| {
            let mut at = 0;
            for text in [identity.0, primary, fallback] {
                payload[at..at + text.len()].copy_from_slice(text.as_bytes());
                at += text.len();
            }
            Ok(Binding {
                lengths: [identity.0.len(), primary.len(), fallback.len()],
                saved,
            })
        })
        .map_err(reject)?;
        let mut state = inbox::lock(self.backing.value());
        let slot = state
            .slots
            .iter_mut()
            .find(|s| s.key == key && s.phase != Phase::Free)
            .ok_or(Status::Stale)?;
        slot.working = false;
        if slot.phase != Phase::Working
            || !slot.validity.as_ref().is_some_and(|v| v.value().is_valid())
        {
            return Err(Status::Cancelled);
        }
        slot.identity = Some(identity.1);
        slot.binding = Some(binding);
        slot.phase = Phase::Prepared;
        Ok(())
    }
    /// loading owner 完成 kill/wait/drain 后调用；不能由外层 future drop 提前调用。
    pub fn finish_work(&self, key: Key, reason: Status) {
        let mut state = inbox::lock(self.backing.value());
        if let Some(slot) = state
            .slots
            .iter_mut()
            .find(|s| s.key == key && s.phase != Phase::Free)
        {
            slot.working = false;
            if slot.phase != Phase::Terminal {
                slot.invalidate(reason);
            }
        }
    }
    /// 每轮最多一个 terminal 和一个 normal；两条 lane 分别轮转。
    pub fn next_batch(&self) -> WorkBatch<'r> {
        self.expire_at(std::time::Instant::now());
        let mut retired: [Option<Slot<'r>>; 8] = std::array::from_fn(|_| None);
        let mut state = inbox::lock(self.backing.value());
        let mut terminal = None;
        for offset in 0..8 {
            let i = (state.terminal_next + offset) % 8;
            if state.slots[i].terminal_ready {
                let slot = &mut state.slots[i];
                slot.terminal_ready = false;
                if let Some(permit) = slot.terminal.take() {
                    terminal = Some(TerminalWork {
                        key: slot.key,
                        reason: slot.reason,
                        permit,
                    });
                }
                state.terminal_next = (i + 1) % 8;
                break;
            }
        }
        let mut normal = None;
        for offset in 0..8 {
            let i = (state.normal_next + offset) % 8;
            let slot = &mut state.slots[i];
            if slot.phase == Phase::Queued {
                if let (Some(payload), Some(loading), Some(reply)) =
                    (slot.payload.take(), slot.loading.take(), slot.reply.take())
                {
                    slot.phase = Phase::Working;
                    slot.working = true;
                    normal = Some(NormalWork::Prepare(PrepareWork {
                        key: slot.key,
                        payload,
                        loading,
                        reply,
                    }));
                }
            } else if slot.phase == Phase::StartQueued {
                if !slot.validity.as_ref().is_some_and(|v| v.value().is_valid())
                    || self
                        .ledger
                        .try_reserve(
                            Amount::default(),
                            ResourceClass::Control,
                            Admission {
                                epoch: slot.key.epoch,
                                source: Some(slot.key.source()),
                            },
                        )
                        .is_err()
                {
                    slot.invalidate(Status::Cancelled);
                    continue;
                }
                if let (Some(start), Some(binding), Some(terminal)) =
                    (slot.start.take(), slot.binding.take(), slot.terminal.take())
                {
                    slot.phase = Phase::Started;
                    normal = Some(NormalWork::Start(StartTransfer {
                        key: slot.key,
                        start,
                        binding,
                        terminal,
                    }));
                }
            }
            if normal.is_some() {
                state.normal_next = (i + 1) % 8;
                break;
            }
        }
        for (i, retired_slot) in retired.iter_mut().enumerate() {
            if (state.slots[i].phase == Phase::Terminal
                && !state.slots[i].terminal_ready
                && state.slots[i].copying == 0
                && !state.slots[i].working)
                || state.slots[i].phase == Phase::Started
            {
                let old = std::mem::replace(&mut state.slots[i], Slot::empty());
                state.remember(
                    old.key,
                    old.identity,
                    if old.phase == Phase::Started {
                        Status::AlreadyStarted
                    } else {
                        old.reason
                    },
                );
                *retired_slot = Some(old);
            }
        }
        drop(state);
        drop(retired);
        WorkBatch { terminal, normal }
    }
    pub fn expire_at(&self, now: std::time::Instant) {
        let mut state = inbox::lock(self.backing.value());
        for slot in &mut state.slots {
            if !matches!(slot.phase, Phase::Free | Phase::Terminal | Phase::Started)
                && now >= slot.deadline
            {
                slot.invalidate(Status::Expired);
            }
        }
    }
}

enum ClaimKind<'r> {
    Prepare {
        payload: Grant<'r>,
        lengths: [usize; 5],
        target: u32,
        ssh_port: u16,
    },
    Start {
        secret: Grant<'r>,
        reply: Grant<'r>,
        request: u32,
        geometry: super::frame::Geometry,
        fallback: bool,
    },
}

pub struct CopyClaim<'a, 'r, 'f> {
    runtime: &'a ControlRuntime<'r>,
    frame: ControlFrame<'f>,
    index: usize,
    pub key: Key,
    validity: SharedOwner<'r, CopyValidity>,
    kind: Option<ClaimKind<'r>>,
    finished: bool,
}

impl<'r> CopyClaim<'_, 'r, '_> {
    pub fn copy_and_install(mut self) -> Result<Key, Status> {
        let frame = self.frame;
        // claim 只可复制原来验证的字段长度及同一请求，不能替换为另一 DTO。
        if frame.epoch != self.key.epoch
            || frame.owner != self.key.owner
            || frame.source != self.key.source()
            || frame.session != self.key.session
        {
            return Err(Status::Invalid);
        }
        let kind = self.kind.take().ok_or(Status::Invalid)?;
        // self 是参数，owned 局部变量后声明；unwind 先清理局部 owner，再由 claim 退 copy。
        let mut payload = None;
        let mut start = None;
        match kind {
            ClaimKind::Prepare {
                payload: permit,
                lengths,
                target,
                ssh_port,
            } => {
                if frame.command != Command::Prepare
                    || frame.request != self.key.prepare
                    || lengths != std::array::from_fn(|i| frame.fields[i].len())
                    || target != frame.target
                    || ssh_port != frame.ssh_port
                {
                    return Err(Status::Invalid);
                }
                let len = lengths.iter().sum();
                payload = Some(
                    SharedOwner::try_init(permit, len, 0, |out| {
                        let mut at = 0;
                        for text in &frame.fields[..5] {
                            out[at..at + text.len()].copy_from_slice(text.as_bytes());
                            at += text.len();
                        }
                        Ok(PrepareMeta {
                            key: self.key,
                            request: frame.request,
                            target,
                            ssh_port,
                            lengths,
                        })
                    })
                    .map_err(reject)?,
                );
            }
            ClaimKind::Start {
                secret: permit,
                reply,
                request,
                geometry,
                fallback,
            } => {
                if frame.command != Command::Start
                    || frame.prepare != self.key.prepare
                    || frame.entry != self.key.entry
                    || frame.request != request
                    || frame.geometry != geometry
                    || frame.secret.len() != permit.amount().rust_bytes
                {
                    return Err(Status::Invalid);
                }
                let secret =
                    LockedSecretString::try_copy(frame.secret, permit, self.validity.value())
                        .map_err(reject)?;
                start = Some(StartOwned {
                    request,
                    geometry,
                    fallback,
                    secret,
                    reply,
                });
            }
        }
        #[cfg(test)]
        test_hooks::checkpoint(1);
        let mut state = inbox::lock(self.runtime.backing.value());
        let slot = &mut state.slots[self.index];
        if slot.key != self.key
            || !self.validity.value().is_valid()
            || self
                .runtime
                .ledger
                .try_reserve(
                    Amount::default(),
                    ResourceClass::Control,
                    Admission {
                        epoch: self.key.epoch,
                        source: Some(self.key.source()),
                    },
                )
                .is_err()
        {
            if slot.key == self.key {
                slot.invalidate(Status::Cancelled);
            }
            drop(state);
            #[cfg(test)]
            test_hooks::checkpoint(2);
            drop(start);
            drop(payload);
            // 锁外 wipe/free 全部完成后，CopyClaim::drop 才允许 terminal retire。
            return Err(Status::Cancelled);
        }
        if let Some(payload) = payload {
            slot.payload = Some(payload);
            slot.phase = Phase::Queued;
        }
        if let Some(start) = start {
            slot.start = Some(start);
            slot.phase = Phase::StartQueued;
        }
        slot.copying -= 1;
        self.finished = true;
        Ok(self.key)
    }
}

impl Drop for CopyClaim<'_, '_, '_> {
    fn drop(&mut self) {
        if !self.finished {
            let mut state = inbox::lock(self.runtime.backing.value());
            let slot = &mut state.slots[self.index];
            if slot.key == self.key && slot.phase != Phase::Free {
                slot.copying -= 1;
                slot.invalidate(Status::Cancelled);
            }
        }
    }
}

/// 只用于独立 harness 的 owned-copy/析构边界，不进入生产控制入口。
#[cfg(test)]
pub mod test_hooks {
    use std::cell::Cell;
    thread_local! { static HOOK: Cell<Option<fn(u32)>> = const { Cell::new(None) }; }
    pub fn set(hook: Option<fn(u32)>) {
        HOOK.with(|cell| cell.set(hook));
    }
    pub(super) fn checkpoint(point: u32) {
        if let Some(hook) = HOOK.with(Cell::get) {
            hook(point);
        }
    }
}
