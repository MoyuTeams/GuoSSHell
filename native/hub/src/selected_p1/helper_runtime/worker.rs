//! 固定 4 process owner、4 stderr drainer、1 supervisor；结果槽与真实收割同步。
//! raw CreateThread 路径不调用 std::thread、Tokio、stdio globals、日志或动态队列。
//! Windows OS 栈/pipe kernel buffer/句柄单列，不计作 Rust allocator Layout。

#[cfg(windows)]
mod platform {
    use super::super::{
        process::{
            BorrowedField, BorrowedValue, Failure, Footer, GROUP_FACT_MASK, GROUP_FACT_SHIFT,
            GroupReferenceFact, HelperBuildRecord, LoadReservations, RawStorage, Request,
            SUCCESS_FLAGS, read_stream,
        },
        windows::{self, Child, Handle, SpawnBuffers, SpawnCostPlan, TrustedLaunch},
    };
    use crate::selected_p1::{
        model::layout::NumericReject,
        resource::{
            ledger::{Admission, Amount, Grant, ProcessLedger, ResourceClass},
            owner::{OwnerLayout, SharedOwner},
        },
    };
    use std::alloc::Layout;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Mutex, MutexGuard};
    use windows_sys::Win32::{
        Foundation::WAIT_OBJECT_0,
        System::{
            SystemInformation::GetTickCount64,
            Threading::{CreateThread, STACK_SIZE_PARAM_IS_A_RESERVATION},
        },
    };

    pub const PROCESS_OWNERS: usize = 4;
    pub const STDERR_DRAINERS: usize = 4;
    pub const WORKERS: usize = 9;
    pub const STACK_RESERVATION: usize = 1024 * 1024;

    fn admission_failure(error: NumericReject) -> Failure {
        match error {
            NumericReject::EpochMismatch
            | NumericReject::SourceMismatch
            | NumericReject::Cancelled => Failure::Cancelled,
            NumericReject::RustBudgetExceeded => Failure::Budget,
            NumericReject::AllocationFailure => Failure::Allocation,
            NumericReject::ConstructionFailure => Failure::Io,
            NumericReject::Overflow
            | NumericReject::InvalidLayout
            | NumericReject::InvalidSource
            | NumericReject::RefcountOverflow
            | NumericReject::UnsupportedEnvelope => Failure::Unsupported,
        }
    }

    /// exact GNU std 的 poison::Flag→thread::panicking→panic_count 慢路径。
    /// sys/thread_local/os.rs 的 repr(C) Value<Cell<(usize,bool)>> 后接 u32 Key。
    /// System.alloc 不经过测试 global allocator；不能从空日志把这一项算成零。
    pub fn worker_tls_upper() -> Result<usize, NumericReject> {
        if !cfg!(all(
            target_env = "gnu",
            target_pointer_width = "64",
            panic = "unwind"
        )) {
            return Err(NumericReject::UnsupportedEnvelope);
        }
        let value = Layout::new::<std::cell::Cell<(usize, bool)>>()
            .extend(Layout::new::<u32>())
            .map_err(|_| NumericReject::InvalidLayout)?
            .0
            .pad_to_align();
        value
            .size()
            .checked_mul(WORKERS)
            .ok_or(NumericReject::Overflow)
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    #[repr(C)]
    pub struct Ticket {
        pub slot: usize,
        pub generation: u64,
    }

    #[repr(C)]
    struct Job {
        request: Request,
        artifact_sha256: [u8; 32],
        spawn: Option<SpawnBuffers<'static>>,
        helper: Grant<'static>,
        raw: RawStorage<'static>,
        deadline: u64,
    }

    /// completion 的 helper grant 到结果交接才归还；raw grant 随真实 consumer 继续存活。
    pub struct Completion {
        pub ticket: Ticket,
        result: Result<Footer, Failure>,
        raw: RawStorage<'static>,
        stderr_bytes: u64,
        stderr_overflow: bool,
        exit_code: u32,
        request: Request,
        artifact_sha256: [u8; 32],
        helper: Grant<'static>,
    }

    impl Completion {
        pub fn result(&self) -> Result<Footer, Failure> {
            self.result
        }
        pub fn raw(&self) -> &RawStorage<'static> {
            &self.raw
        }
        pub fn exit_code(&self) -> u32 {
            self.exit_code
        }
        pub fn stderr_bytes(&self) -> u64 {
            self.stderr_bytes
        }
        pub fn stderr_overflow(&self) -> bool {
            self.stderr_overflow
        }
        pub fn helper_admitted(&self) -> Amount {
            self.helper.amount()
        }
        /// 实际 result consumer 接管 raw 后释放已收割 helper envelope；raw permit 不变。
        pub fn into_result(self) -> (Result<Footer, Failure>, RawStorage<'static>) {
            (self.result, self.raw)
        }
        /// EOF、真实 exit 0、所有本次 IO 活动结束及受证映像一致后才成为 LoadedRaw。
        pub fn into_loaded(self) -> Result<LoadedRaw, Failure> {
            let footer = self.result?;
            if footer.outcome != 0 {
                return Err(Failure::Helper);
            }
            if footer.flags & !(255 | GROUP_FACT_MASK) != 0
                || footer.flags & !GROUP_FACT_MASK != SUCCESS_FLAGS
                || self.exit_code != 0
                || self.stderr_bytes != 0
                || self.stderr_overflow
            {
                return Err(Failure::Protocol);
            }
            let group_reference = GroupReferenceFact::decode(
                ((footer.flags & GROUP_FACT_MASK) >> GROUP_FACT_SHIFT) as u16,
            )?;
            Ok(LoadedRaw {
                raw: self.raw,
                request: self.request,
                footer,
                artifact_sha256: self.artifact_sha256,
                group_reference,
            })
        }
    }

    /// 仅真实 process 收割路径可以构造；不认证调用方另供的 DomainRefs 事务事实。
    pub struct LoadedRaw {
        raw: RawStorage<'static>,
        request: Request,
        footer: Footer,
        artifact_sha256: [u8; 32],
        group_reference: GroupReferenceFact,
    }

    /// 完整成功 stream、EOF、实际 exit/wait/drain 与产物核验共同关联的数字事实。
    /// 不解析 UUID/JSON，也不代替 domain 的字段 codec 和错误顺序。
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct RequiredFacts {
        group_reference: GroupReferenceFact,
        transaction_nonce: [u8; 16],
        revision: u64,
        source: u32,
        selected_connection: Option<[u8; 16]>,
    }
    impl RequiredFacts {
        pub fn group_reference(self) -> GroupReferenceFact {
            self.group_reference
        }
        pub fn transaction_nonce(self) -> [u8; 16] {
            self.transaction_nonce
        }
        pub fn revision(self) -> u64 {
            self.revision
        }
        pub fn source(self) -> u32 {
            self.source
        }
        pub fn selected_connection(self) -> Option<[u8; 16]> {
            self.selected_connection
        }
    }
    impl LoadedRaw {
        pub fn required_facts(&self) -> RequiredFacts {
            RequiredFacts {
                group_reference: self.group_reference,
                transaction_nonce: self.nonce(),
                revision: self.revision(),
                source: self.source(),
                selected_connection: self.selected_connection(),
            }
        }
        pub fn nonce(&self) -> [u8; 16] {
            self.request.nonce
        }
        pub fn revision(&self) -> u64 {
            self.footer.revision
        }
        pub fn source(&self) -> u32 {
            self.footer.source
        }
        pub fn selected_connection(&self) -> Option<[u8; 16]> {
            if !self.request.connected {
                return None;
            }
            let mut uuid = [0; 16];
            uuid.copy_from_slice(&self.request.bytes[40..56]);
            Some(uuid)
        }
        pub fn artifact_sha256(&self) -> [u8; 32] {
            self.artifact_sha256
        }
        pub fn storage(&self) -> &RawStorage<'static> {
            &self.raw
        }
        pub fn field(&self, index: usize) -> Option<BorrowedField<'_>> {
            let descriptor = self.raw.descriptors().get(index)?;
            let end = descriptor.offset.checked_add(descriptor.len)?;
            let bytes = self.raw.bytes().get(descriptor.offset..end)?;
            let value = match descriptor.kind {
                0 => BorrowedValue::Null,
                1 => BorrowedValue::Text(std::str::from_utf8(bytes).ok()?),
                2 => BorrowedValue::Port(u16::from_le_bytes(bytes.try_into().ok()?)),
                3 => BorrowedValue::Position(i64::from_le_bytes(bytes.try_into().ok()?)),
                _ => return None,
            };
            Some(BorrowedField {
                tag: descriptor.tag,
                ordinal: descriptor.ordinal,
                value,
            })
        }
    }

    pub struct Submission<'a, R: HelperBuildRecord> {
        pub launch: &'a TrustedLaunch<'a>,
        pub bytes: [u8; 112],
        pub record: &'a R,
        pub admission: Admission,
        pub timeout_ms: u64,
    }

    #[repr(C)]
    struct Quarantine {
        job: Job,
        child: Child,
    }

    #[repr(C)]
    struct ProcSlot {
        generation: u64,
        reserved: bool,
        running: bool,
        cancel: Option<Failure>,
        process: usize,
        pending: Option<Job>,
        completion: Option<Completion>,
        stderr: Option<Handle>,
        draining: bool,
        stderr_bytes: u64,
        stderr_overflow: bool,
        stderr_failed: bool,
        deadline: u64,
        admission: Option<Admission>,
        quarantine: Option<Quarantine>,
    }

    impl ProcSlot {
        fn new() -> Self {
            Self {
                generation: 0,
                reserved: false,
                running: false,
                cancel: None,
                process: 0,
                pending: None,
                completion: None,
                stderr: None,
                draining: false,
                stderr_bytes: 0,
                stderr_overflow: false,
                stderr_failed: false,
                deadline: 0,
                admission: None,
                quarantine: None,
            }
        }
    }

    #[repr(C)]
    struct State {
        slots: [ProcSlot; 4],
        stopping: bool,
    }
    #[repr(C)]
    struct Boot {
        address: AtomicUsize,
        kind: u32,
        index: u32,
    }
    #[repr(C)]
    struct PoolBody {
        root: &'static ProcessLedger,
        state: Mutex<State>,
        events: [Handle; WORKERS],
        boots: [Boot; WORKERS],
        shutdown_failure: AtomicBool,
    }
    fn lock(body: &PoolBody) -> MutexGuard<'_, State> {
        body.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub struct HelperPool {
        root: &'static ProcessLedger,
        body: Option<SharedOwner<'static, PoolBody>>,
        threads: [Option<Handle>; WORKERS],
    }

    pub fn persistent_layout() -> Result<OwnerLayout, NumericReject> {
        OwnerLayout::actual::<PoolBody>(0)
    }

    /// inline 子布局仅用于核对总 backing，不作为重复加额项；TLS 是单独 out-of-line 根。
    #[derive(Clone, Copy, Debug)]
    pub struct PoolCostClasses {
        pub backing: OwnerLayout,
        pub process_slots_inline: Layout,
        pub boots_inline: Layout,
        pub events_inline: Layout,
        pub worker_tls_rust: usize,
        pub os_stack_reservation: usize,
    }
    pub fn cost_classes() -> Result<PoolCostClasses, NumericReject> {
        Ok(PoolCostClasses {
            backing: persistent_layout()?,
            process_slots_inline: Layout::array::<ProcSlot>(PROCESS_OWNERS)
                .map_err(|_| NumericReject::InvalidLayout)?,
            boots_inline: Layout::array::<Boot>(WORKERS)
                .map_err(|_| NumericReject::InvalidLayout)?,
            events_inline: Layout::array::<Handle>(WORKERS)
                .map_err(|_| NumericReject::InvalidLayout)?,
            worker_tls_rust: worker_tls_upper()?,
            os_stack_reservation: STACK_RESERVATION
                .checked_mul(WORKERS)
                .ok_or(NumericReject::Overflow)?,
        })
    }

    impl HelperPool {
        pub fn new(root: &'static ProcessLedger, epoch: u64) -> Result<Self, Failure> {
            let classes = cost_classes().map_err(|_| Failure::Unsupported)?;
            let layout = classes.backing;
            let tls = classes.worker_tls_rust;
            let permit = root
                .try_reserve(
                    Amount {
                        rust_bytes: layout
                            .required_rust(tls)
                            .map_err(|_| Failure::Unsupported)?,
                        c_bytes: 0,
                        slots: 0,
                    },
                    ResourceClass::Persistent,
                    Admission {
                        epoch,
                        source: None,
                    },
                )
                .map_err(|_| Failure::Budget)?;
            let body = SharedOwner::try_init(permit, 0, tls, |_| {
                let mut events: [Option<Handle>; WORKERS] = std::array::from_fn(|_| None);
                for event in &mut events {
                    *event = Some(Handle::event().map_err(|_| NumericReject::ConstructionFailure)?);
                }
                let events = events.map(|event| match event {
                    Some(event) => event,
                    None => std::process::abort(),
                });
                Ok(PoolBody {
                    root,
                    state: Mutex::new(State {
                        slots: std::array::from_fn(|_| ProcSlot::new()),
                        stopping: false,
                    }),
                    events,
                    boots: std::array::from_fn(|i| Boot {
                        address: AtomicUsize::new(0),
                        kind: if i < 4 {
                            0
                        } else if i < 8 {
                            1
                        } else {
                            2
                        },
                        index: (i % 4) as u32,
                    }),
                    shutdown_failure: AtomicBool::new(false),
                })
            })
            .map_err(|e| {
                if e == NumericReject::AllocationFailure {
                    Failure::Allocation
                } else {
                    Failure::Io
                }
            })?;
            let mut pool = Self {
                root,
                body: Some(body),
                threads: std::array::from_fn(|_| None),
            };
            let body = pool.body.as_ref().ok_or(Failure::Unsupported)?.value();
            let address = body as *const PoolBody as usize;
            for i in 0..WORKERS {
                body.boots[i].address.store(address, Ordering::Release);
                let thread = unsafe {
                    CreateThread(
                        std::ptr::null(),
                        STACK_RESERVATION,
                        Some(thread_entry),
                        (&body.boots[i] as *const Boot).cast(),
                        STACK_SIZE_PARAM_IS_A_RESERVATION,
                        std::ptr::null_mut(),
                    )
                };
                if thread.is_null() {
                    return Err(Failure::Io);
                }
                pool.threads[i] = Some(Handle(thread as usize));
            }
            Ok(pool)
        }
        pub fn submit(
            &self,
            launch: &TrustedLaunch<'_>,
            bytes: [u8; 112],
            record: &impl HelperBuildRecord,
            admission: Admission,
            timeout_ms: u64,
        ) -> Result<Ticket, Failure> {
            self.submit_inner(
                Submission {
                    launch,
                    bytes,
                    record,
                    admission,
                    timeout_ms,
                },
                None,
            )
        }
        pub fn submit_reserved(
            &self,
            submission: Submission<'_, impl HelperBuildRecord>,
            reservations: LoadReservations<'static>,
        ) -> Result<Ticket, Failure> {
            self.submit_inner(submission, Some(reservations))
        }
        fn submit_inner(
            &self,
            submission: Submission<'_, impl HelperBuildRecord>,
            reservations: Option<LoadReservations<'static>>,
        ) -> Result<Ticket, Failure> {
            let Submission {
                launch,
                bytes,
                record,
                admission,
                timeout_ms,
            } = submission;
            let body = self.body.as_ref().ok_or(Failure::Unsupported)?.value();
            let request = Request::parse(bytes, record.request_magic())?;
            let deadline = unsafe { GetTickCount64() }
                .checked_add(timeout_ms)
                .ok_or(Failure::Unsupported)?;
            let ticket = {
                let mut state = lock(body);
                if state.stopping {
                    return Err(Failure::Cancelled);
                }
                let index = state
                    .slots
                    .iter()
                    .position(|s| {
                        !s.reserved
                            && !s.running
                            && s.completion.is_none()
                            && s.quarantine.is_none()
                    })
                    .ok_or(Failure::Budget)?;
                let slot = &mut state.slots[index];
                slot.generation = slot.generation.checked_add(1).ok_or(Failure::Unsupported)?;
                slot.reserved = true;
                slot.cancel = None;
                slot.admission = Some(admission);
                Ticket {
                    slot: index,
                    generation: slot.generation,
                }
            };
            let prepared = (|| {
                let plan = SpawnCostPlan::bound(launch, &request, record)?;
                let reserved = match reservations {
                    Some(reserved) => reserved,
                    None => plan
                        .costs()
                        .reserve(self.root, admission)
                        .map_err(admission_failure)?,
                };
                if !reserved.belongs_to(self.root) || reserved.admission() != admission {
                    return Err(Failure::Unsupported);
                }
                let (helper, spawn, raw) = reserved.into_parts();
                if helper.amount() != plan.costs().helper
                    || helper.epoch() != admission.epoch
                    || spawn.epoch() != admission.epoch
                    || raw.epoch() != admission.epoch
                {
                    return Err(Failure::Unsupported);
                }
                // 原额度可跨异步存活，但失效 token 不可开启新的 owned copy/spawn。
                self.root
                    .try_reserve(Amount::default(), ResourceClass::Runtime, admission)
                    .map_err(admission_failure)?;
                let raw = RawStorage::from_grant(raw, &request).map_err(admission_failure)?;
                let spawn = SpawnBuffers::from_grant(spawn, plan)?;
                Ok(Job {
                    request,
                    artifact_sha256: plan.artifact_sha256(),
                    spawn: Some(spawn),
                    helper,
                    raw,
                    deadline,
                })
            })();
            let mut state = lock(body);
            let slot = &mut state.slots[ticket.slot];
            match prepared {
                Ok(job) => {
                    slot.deadline = job.deadline;
                    slot.pending = Some(job);
                    slot.stderr_bytes = 0;
                    slot.stderr_overflow = false;
                    slot.stderr_failed = false;
                }
                Err(error) => {
                    slot.reserved = false;
                    slot.admission = None;
                    slot.cancel = None;
                    return Err(error);
                }
            }
            drop(state);
            body.events[ticket.slot].notify();
            body.events[8].notify();
            Ok(ticket)
        }
        pub fn cancel(&self, ticket: Ticket) -> Result<(), Failure> {
            let body = self.body.as_ref().ok_or(Failure::Unsupported)?.value();
            let mut state = lock(body);
            let slot = state.slots.get_mut(ticket.slot).ok_or(Failure::Protocol)?;
            if slot.generation != ticket.generation
                || (!slot.reserved && !slot.running && slot.completion.is_none())
            {
                return Err(Failure::Protocol);
            }
            if slot.cancel.is_none() {
                slot.cancel = Some(Failure::Cancelled);
            }
            if slot.process != 0 && !windows::exited(slot.process) {
                let _ = windows::kill(slot.process);
            }
            drop(state);
            body.events[8].notify();
            Ok(())
        }
        pub fn cancellation(&self, ticket: Ticket) -> Result<Option<Failure>, Failure> {
            let state = lock(self.body.as_ref().ok_or(Failure::Unsupported)?.value());
            let slot = state.slots.get(ticket.slot).ok_or(Failure::Unsupported)?;
            if slot.generation != ticket.generation {
                return Err(Failure::Cancelled);
            }
            Ok(slot.cancel)
        }

        pub fn take_result(&self, ticket: Ticket) -> Result<Option<Completion>, Failure> {
            let body = self.body.as_ref().ok_or(Failure::Unsupported)?.value();
            let mut state = lock(body);
            let slot = state.slots.get_mut(ticket.slot).ok_or(Failure::Protocol)?;
            if slot.generation != ticket.generation {
                return Err(Failure::Protocol);
            }
            if slot.quarantine.is_some() {
                return Err(Failure::Wait);
            }
            // completion 只在实际 exit/wait、stdin/stdout、stderr idle 后发布。
            let mut result = slot.completion.take();
            if let Some(cancel) = slot.cancel
                && let Some(completion) = &mut result
            {
                completion.result = Err(cancel);
            }
            if slot.admission.is_some_and(|admission| {
                body.root
                    .try_reserve(Amount::default(), ResourceClass::Runtime, admission)
                    .is_err()
            }) && let Some(completion) = &mut result
                && completion.result.is_ok()
            {
                completion.result = Err(Failure::Cancelled);
            }
            if result.is_some() {
                slot.cancel = None;
                slot.reserved = false;
            }
            Ok(result)
        }
        /// 关闭只使准入失效；join 失败或 wait 未证明时保留整个 backing/charge。
        pub fn shutdown(&mut self) -> Result<(), Failure> {
            let Some(owner) = &self.body else {
                return Ok(());
            };
            let body = owner.value();
            {
                let mut state = lock(body);
                state.stopping = true;
                for slot in &mut state.slots {
                    if slot.reserved || slot.running {
                        slot.cancel.get_or_insert(Failure::Cancelled);
                    }
                    if slot.process != 0 && !windows::exited(slot.process) {
                        let _ = windows::kill(slot.process);
                    }
                }
            }
            for event in &body.events {
                event.notify();
            }
            for thread in &mut self.threads {
                if let Some(handle) = thread
                    && handle.wait(u32::MAX) != WAIT_OBJECT_0
                {
                    body.shutdown_failure.store(true, Ordering::Release);
                    return Err(Failure::Wait);
                }
                drop(thread.take());
            }
            if body.shutdown_failure.load(Ordering::Acquire) {
                return Err(Failure::Wait);
            }
            drop(self.body.take());
            Ok(())
        }
        pub fn snapshot(&self) -> [u32; 4] {
            let Some(body) = &self.body else {
                return [0; 4];
            };
            let state = lock(body.value());
            std::array::from_fn(|i| {
                let slot = &state.slots[i];
                u32::from(slot.reserved)
                    | (u32::from(slot.running) << 1)
                    | (u32::from(slot.draining) << 2)
                    | (u32::from(slot.completion.is_some()) << 3)
                    | (u32::from(slot.quarantine.is_some()) << 4)
            })
        }
        /// wait 观察失败不会解除 quarantine；只有新的真实 wait 和 stderr idle 可移交。
        pub fn retry_wait(&self, ticket: Ticket) -> Result<bool, Failure> {
            let body = self.body.as_ref().ok_or(Failure::Unsupported)?.value();
            let quarantine = {
                let mut state = lock(body);
                let slot = state.slots.get_mut(ticket.slot).ok_or(Failure::Protocol)?;
                if slot.generation != ticket.generation {
                    return Err(Failure::Protocol);
                }
                let Some(quarantine) = &slot.quarantine else {
                    return Ok(false);
                };
                if slot.draining || !windows::exited(quarantine.child.process.0) {
                    return Ok(false);
                }
                // 已观察实际 exit，非阻塞成功路径仍需 GetExitCode/wait 证明。
                let code = windows::wait(quarantine.child.process.0)?;
                let quarantine = slot.quarantine.take().ok_or(Failure::Wait)?;
                slot.process = 0;
                (quarantine, code)
            };
            let (quarantine, code) = quarantine;
            drop(quarantine.child);
            publish(body, ticket, quarantine.job, Err(Failure::Wait), code);
            let state = lock(body);
            let failed = state.slots.iter().any(|s| s.quarantine.is_some());
            drop(state);
            body.shutdown_failure.store(failed, Ordering::Release);
            Ok(true)
        }
    }

    impl Drop for HelperPool {
        fn drop(&mut self) {
            if self.shutdown().is_err() {
                if let Some(body) = self.body.take() {
                    std::mem::forget(body);
                }
                for handle in &mut self.threads {
                    if let Some(handle) = handle.take() {
                        std::mem::forget(handle);
                    }
                }
            }
        }
    }

    // C callback 的任何未预期 Rust panic 都 fail-closed 终止进程，绝不 unwind 越过 ABI。
    // 正常错误路径只返回数字；不以 panic/catch 构造预算或系统 OOM 结果。
    unsafe extern "system" fn thread_entry(context: *mut std::ffi::c_void) -> u32 {
        let run = std::panic::catch_unwind(|| {
            let boot = unsafe { &*context.cast::<Boot>() };
            let address = boot.address.load(Ordering::Acquire);
            let body = unsafe { &*(address as *const PoolBody) };
            match boot.kind {
                0 => process_owner(body, boot.index as usize),
                1 => drainer(body, boot.index as usize),
                _ => supervisor(body),
            }
        });
        if run.is_err() {
            std::process::abort();
        }
        0
    }

    fn process_owner(body: &PoolBody, index: usize) {
        loop {
            let job = {
                let mut state = lock(body);
                let slot = &mut state.slots[index];
                let job = slot.pending.take();
                if job.is_some() {
                    slot.running = true;
                }
                if job.is_none() && state.stopping {
                    return;
                }
                job
            };
            let Some(mut job) = job else {
                body.events[index].wait(20);
                continue;
            };
            let ticket = {
                let state = lock(body);
                Ticket {
                    slot: index,
                    generation: state.slots[index].generation,
                }
            };
            let cancelled = { lock(body).slots[index].cancel };
            if let Some(failure) = cancelled {
                publish(body, ticket, job, Err(failure), 0);
                continue;
            }
            let Some(spawn) = job.spawn.take() else {
                publish(body, ticket, job, Err(Failure::Unsupported), 0);
                continue;
            };
            let mut child = match spawn.spawn() {
                Ok(child) => child,
                Err(failure) => {
                    publish(body, ticket, job, Err(failure), 0);
                    continue;
                }
            };
            #[cfg(test)]
            test_hooks::pause(1);
            {
                let mut state = lock(body);
                let slot = &mut state.slots[index];
                slot.process = child.process.0;
                slot.stderr = Some(std::mem::replace(&mut child.stderr, Handle(0)));
                slot.draining = true;
                if slot.cancel.is_some() {
                    let _ = windows::kill(slot.process);
                }
            }
            body.events[index + 4].notify();
            #[cfg(test)]
            test_hooks::pause(2);
            let result = child.stdin.write_all(&job.request.bytes).and_then(|()| {
                drop(std::mem::replace(&mut child.stdin, Handle(0)));
                read_stream(&mut child.stdout, &job.request, &mut job.raw)
            });
            drop(std::mem::replace(&mut child.stdin, Handle(0)));
            drop(std::mem::replace(&mut child.stdout, Handle(0)));
            if let Err(error) = result {
                lock(body).slots[index].cancel.get_or_insert(error);
                let _ = windows::kill(child.process.0);
            }
            #[cfg(test)]
            let exit = if test_hooks::FAIL_WAIT.swap(false, Ordering::AcqRel) {
                Err(Failure::Wait)
            } else {
                windows::wait(child.process.0)
            };
            #[cfg(not(test))]
            let exit = windows::wait(child.process.0);
            if exit.is_err() {
                let mut state = lock(body);
                let slot = &mut state.slots[index];
                slot.quarantine = Some(Quarantine { job, child });
                slot.running = false;
                body.shutdown_failure.store(true, Ordering::Release);
                continue;
            }
            {
                let mut state = lock(body);
                state.slots[index].process = 0;
            }
            drop(child);
            loop {
                if !lock(body).slots[index].draining {
                    break;
                }
                body.events[index].wait(20);
            }
            let code = exit.unwrap_or(1);
            let result = {
                let state = lock(body);
                let slot = &state.slots[index];
                if let Some(cancel) = slot.cancel {
                    Err(cancel)
                } else if result.is_err() {
                    result
                } else if code != 0 {
                    Err(Failure::ChildCrashed)
                } else if slot.stderr_failed {
                    Err(Failure::Io)
                } else {
                    result
                }
            };
            publish(body, ticket, job, result, code);
        }
    }

    fn publish(
        body: &PoolBody,
        ticket: Ticket,
        mut job: Job,
        result: Result<Footer, Failure>,
        exit_code: u32,
    ) {
        drop(job.spawn.take());
        let mut state = lock(body);
        let slot = &mut state.slots[ticket.slot];
        slot.running = false;
        slot.completion = Some(Completion {
            ticket,
            result: if let Some(cancel) = slot.cancel {
                Err(cancel)
            } else {
                result
            },
            raw: job.raw,
            request: job.request,
            artifact_sha256: job.artifact_sha256,
            stderr_bytes: slot.stderr_bytes,
            stderr_overflow: slot.stderr_overflow,
            exit_code,
            helper: job.helper,
        });
    }

    fn drainer(body: &PoolBody, index: usize) {
        loop {
            let handle = {
                let mut state = lock(body);
                let handle = state.slots[index].stderr.take();
                if handle.is_none()
                    && state.stopping
                    && !state.slots[index].running
                    && state.slots[index].pending.is_none()
                {
                    return;
                }
                handle
            };
            let Some(handle) = handle else {
                body.events[index + 4].wait(20);
                continue;
            };
            let mut scratch = [0u8; 128];
            let mut bytes = 0u64;
            let mut overflow = false;
            let mut failed = false;
            loop {
                match handle.read(&mut scratch) {
                    Ok(0) => break,
                    Ok(n) => match bytes.checked_add(n as u64) {
                        Some(total) => bytes = total,
                        None => overflow = true,
                    },
                    Err(_) => {
                        failed = true;
                        break;
                    }
                }
            }
            drop(handle);
            {
                let mut state = lock(body);
                let slot = &mut state.slots[index];
                slot.stderr_bytes = bytes;
                slot.stderr_overflow = overflow;
                slot.stderr_failed = failed;
                if failed {
                    slot.cancel.get_or_insert(Failure::Io);
                }
                slot.draining = false;
            }
            body.events[index].notify();
        }
    }

    fn supervisor(body: &PoolBody) {
        loop {
            let now = unsafe { GetTickCount64() };
            {
                let mut state = lock(body);
                let stopping = state.stopping;
                for slot in &mut state.slots {
                    // active Job 的 deadline 同样存于 slot，运行期间不得消失。
                    if (slot.pending.is_some() || slot.running)
                        && now >= slot.deadline
                        && slot.cancel.is_none()
                    {
                        slot.cancel = Some(Failure::TimedOut);
                    }
                    if (slot.pending.is_some() || slot.running)
                        && slot.admission.is_some_and(|admission| {
                            body.root
                                .try_reserve(Amount::default(), ResourceClass::Runtime, admission)
                                .is_err()
                        })
                    {
                        slot.cancel.get_or_insert(Failure::Cancelled);
                    }
                    if (slot.cancel.is_some() || stopping)
                        && slot.process != 0
                        && !windows::exited(slot.process)
                    {
                        let _ = windows::kill(slot.process);
                    }
                }
                if stopping {
                    return;
                }
            }
            body.events[8].wait(10);
        }
    }

    /// 只在独立 test binary 编译；不增加生产参数、环境变量或路径发现旁路。
    #[cfg(test)]
    pub mod test_hooks {
        use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
        pub static POINT: AtomicU32 = AtomicU32::new(0);
        pub static REACHED: AtomicBool = AtomicBool::new(false);
        pub static RELEASE: AtomicBool = AtomicBool::new(false);
        pub static FAIL_WAIT: AtomicBool = AtomicBool::new(false);
        pub fn arm(point: u32) {
            REACHED.store(false, Ordering::Release);
            RELEASE.store(false, Ordering::Release);
            POINT.store(point, Ordering::Release);
        }
        pub(super) fn pause(point: u32) {
            if POINT
                .compare_exchange(point, 0, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                REACHED.store(true, Ordering::Release);
                while !RELEASE.load(Ordering::Acquire) {
                    unsafe {
                        windows_sys::Win32::System::Threading::Sleep(1);
                    }
                }
            }
        }
    }
}

#[cfg(windows)]
pub use platform::*;
