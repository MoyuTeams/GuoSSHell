//! 真实 borrowed ingress 与最终 inbox 类型的独立回归；不注册 hub 入口。

#[path = "../src/selected_p1/control_runtime/admission.rs"]
pub mod admission;
#[path = "../src/selected_p1/control_runtime/frame.rs"]
pub mod frame;
#[path = "../src/selected_p1/control_runtime/inbox.rs"]
pub mod inbox;
#[path = "../src/selected_p1/model/layout.rs"]
pub mod layout;
#[path = "../src/selected_p1/resource/ledger.rs"]
pub mod ledger;
#[path = "../src/selected_p1/resource/owner.rs"]
pub mod owner;
#[path = "../src/selected_p1/control_runtime/persistent.rs"]
pub mod persistent;
#[path = "../src/selected_p1/helper_runtime/process.rs"]
pub mod process;
#[path = "../src/selected_p1/resource/secret.rs"]
pub mod secret;

pub mod selected_p1 {
    pub mod model {
        pub use crate::layout;
    }
    pub mod resource {
        pub use crate::{ledger, owner, secret};
    }
    pub mod control_runtime {
        pub use crate::{admission, frame, inbox, persistent};
    }
    pub mod helper_runtime {
        pub use crate::process;
    }
}

/// 固定 TLS 测试审计；逐 allocation 保存原 Layout，直到真实 dealloc。
pub mod audit {
    use crate::ledger::ProcessLedger;
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::{Cell, UnsafeCell};
    use std::marker::PhantomData;
    use std::sync::atomic::{AtomicBool, Ordering};
    const COUNT: usize = 1024;
    #[derive(Clone, Copy, Default)]
    pub struct Event {
        pub kind: u32,
        pub pointer: usize,
        pub size: usize,
        pub align: usize,
        pub old_size: usize,
        pub held_rust: usize,
        pub held_c: usize,
        pub zero_before_free: bool,
    }
    #[derive(Clone, Copy)]
    struct State {
        root: *const ProcessLedger,
        active: bool,
        fail: usize,
        secret: usize,
        secret_pointer: usize,
        events: [Event; COUNT],
        len: usize,
        overflow: bool,
    }
    impl State {
        const fn empty() -> Self {
            Self {
                root: std::ptr::null(),
                active: false,
                fail: 0,
                secret: 0,
                secret_pointer: 0,
                events: [Event {
                    kind: 0,
                    pointer: 0,
                    size: 0,
                    align: 0,
                    old_size: 0,
                    held_rust: 0,
                    held_c: 0,
                    zero_before_free: false,
                }; COUNT],
                len: 0,
                overflow: false,
            }
        }
    }
    thread_local! { static STATE: Cell<State> = const { Cell::new(State::empty()) }; }
    type AllocationHook = (usize, Option<fn()>);
    thread_local! { static ALLOCATION_HOOK: Cell<AllocationHook> = const { Cell::new((0, None)) }; }
    thread_local! { static DEALLOCATION_HOOK: Cell<AllocationHook> = const { Cell::new((0, None)) }; }
    fn before_deallocation(layout: Layout) {
        let hook = DEALLOCATION_HOOK
            .try_with(|cell| {
                let (size, hook) = cell.get();
                if size == layout.size() {
                    cell.set((0, None));
                    hook
                } else {
                    None
                }
            })
            .unwrap_or(None);
        if let Some(hook) = hook {
            hook();
        }
    }
    fn after_allocation(layout: Layout) {
        let hook = ALLOCATION_HOOK
            .try_with(|cell| {
                let (size, hook) = cell.get();
                if size == layout.size() {
                    cell.set((0, None));
                    hook
                } else {
                    None
                }
            })
            .unwrap_or(None);
        if let Some(hook) = hook {
            hook();
        }
    }
    struct GlobalState(UnsafeCell<State>);
    // SAFETY: 每次全局访问均受 AUDIT_LOCK 的 Acquire/Release 独占协议保护。
    unsafe impl Sync for GlobalState {}
    static GLOBAL_STATE: GlobalState = GlobalState(UnsafeCell::new(State::empty()));
    static AUDIT_LOCK: AtomicBool = AtomicBool::new(false);
    fn global(mut call: impl FnMut(&mut State)) {
        while AUDIT_LOCK
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            std::hint::spin_loop();
        }
        call(unsafe { &mut *GLOBAL_STATE.0.get() });
        AUDIT_LOCK.store(false, Ordering::Release);
    }
    fn append(state: &mut State, kind: u32, pointer: *mut u8, layout: Layout, old_size: usize) {
        if !state.active {
            return;
        }
        let held = unsafe { &*state.root }.snapshot();
        if kind == 1 && state.secret == layout.size() && state.secret_pointer == 0 {
            state.secret_pointer = pointer as usize;
        }
        let zero = kind == 2
            && state.secret != 0
            && state.secret == layout.size()
            && state.secret_pointer == pointer as usize
            && unsafe { std::slice::from_raw_parts(pointer, layout.size()) }
                .iter()
                .all(|b| *b == 0);
        if state.len < COUNT {
            state.events[state.len] = Event {
                kind,
                pointer: pointer as usize,
                size: layout.size(),
                align: layout.align(),
                old_size,
                held_rust: held.rust_bytes,
                held_c: held.c_bytes,
                zero_before_free: zero,
            };
            state.len += 1;
        } else {
            state.overflow = true;
        }
    }
    fn record(kind: u32, pointer: *mut u8, layout: Layout, old_size: usize) {
        let _ = STATE.try_with(|cell| {
            let mut state = cell.get();
            append(&mut state, kind, pointer, layout, old_size);
            cell.set(state);
        });
        global(|state| append(state, kind, pointer, layout, old_size));
    }
    fn fail(layout: Layout) -> bool {
        STATE
            .try_with(|cell| {
                let mut s = cell.get();
                if s.active && s.fail == layout.size() {
                    s.fail = 0;
                    cell.set(s);
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false)
    }
    pub struct Allocator;
    // SAFETY: 同一 System allocator；测试失败注入只返回 NULL。
    unsafe impl GlobalAlloc for Allocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            if fail(layout) {
                record(4, std::ptr::null_mut(), layout, 0);
                return std::ptr::null_mut();
            }
            let p = unsafe { System.alloc(layout) };
            record(1, p, layout, 0);
            after_allocation(layout);
            p
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            if fail(layout) {
                record(4, std::ptr::null_mut(), layout, 0);
                return std::ptr::null_mut();
            }
            let p = unsafe { System.alloc_zeroed(layout) };
            record(1, p, layout, 0);
            after_allocation(layout);
            p
        }
        unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
            record(2, p, layout, 0);
            before_deallocation(layout);
            unsafe {
                System.dealloc(p, layout);
            }
        }
        unsafe fn realloc(&self, p: *mut u8, layout: Layout, n: usize) -> *mut u8 {
            let Ok(new) = Layout::from_size_align(n, layout.align()) else {
                return std::ptr::null_mut();
            };
            if fail(new) {
                record(4, std::ptr::null_mut(), new, layout.size());
                return std::ptr::null_mut();
            }
            // 测试实现显式 old+new 共存，以两笔事件保留原 backing 到真 free。
            let next = unsafe { System.alloc(new) };
            if next.is_null() {
                return next;
            }
            record(1, next, new, layout.size());
            unsafe {
                std::ptr::copy_nonoverlapping(p, next, layout.size().min(n));
            }
            record(2, p, layout, 0);
            unsafe {
                System.dealloc(p, layout);
            }
            next
        }
    }
    pub struct AuditScope<'a> {
        root: &'a ProcessLedger,
        marker: PhantomData<*mut ()>,
    }
    /// 跨 raw worker 审计只用于隔离的单测试场景；直到全部 join 后才结束。
    pub struct PoolAuditScope<'a> {
        root: &'a ProcessLedger,
    }
    pub struct Report {
        events: [Event; COUNT],
        len: usize,
        pub overflow: bool,
    }
    impl Report {
        pub fn events(&self) -> &[Event] {
            &self.events[..self.len]
        }
        pub fn symmetric_single_backing(&self) -> bool {
            self.len == 2
                && self.events[0].kind == 1
                && self.events[1].kind == 2
                && self.events[0].pointer == self.events[1].pointer
                && self.events[0].size == self.events[1].size
                && self.events[0].align == self.events[1].align
        }
        pub fn every_live_layout_covered(&self) -> bool {
            let mut live = [(0usize, 0usize, 0usize); COUNT];
            let mut total = 0usize;
            for event in self.events() {
                if event.kind == 1 && event.pointer != 0 {
                    let Some(slot) = live.iter_mut().find(|s| s.0 == 0) else {
                        return false;
                    };
                    *slot = (event.pointer, event.size, event.align);
                    total += event.size;
                    if total > event.held_rust {
                        return false;
                    }
                } else if event.kind == 2 {
                    if total > event.held_rust {
                        return false;
                    }
                    let Some(slot) = live
                        .iter_mut()
                        .find(|s| **s == (event.pointer, event.size, event.align))
                    else {
                        return false;
                    };
                    total -= event.size;
                    *slot = (0, 0, 0);
                }
            }
            !self.overflow && total == 0
        }
    }
    impl<'a> AuditScope<'a> {
        pub fn start(root: &'a ProcessLedger) -> Self {
            STATE.with(|cell| {
                assert!(!cell.get().active);
                let mut state = State::empty();
                state.root = root;
                state.active = true;
                cell.set(state);
            });
            Self {
                root,
                marker: PhantomData,
            }
        }
        pub fn fail_next_size(&self, n: usize) {
            STATE.with(|cell| {
                let mut s = cell.get();
                s.fail = n;
                cell.set(s);
            });
        }
        pub fn inspect_secret_free(&self, n: usize) {
            STATE.with(|cell| {
                let mut s = cell.get();
                s.secret = n;
                cell.set(s);
            });
        }
        pub fn after_next_allocation(&self, size: usize, hook: fn()) {
            ALLOCATION_HOOK.with(|cell| cell.set((size, Some(hook))));
        }
        pub fn before_next_deallocation(&self, size: usize, hook: fn()) {
            DEALLOCATION_HOOK.with(|cell| cell.set((size, Some(hook))));
        }
        pub fn finish(self) -> Report {
            let r = STATE.with(|cell| {
                let s = cell.replace(State::empty());
                Report {
                    events: s.events,
                    len: s.len,
                    overflow: s.overflow,
                }
            });
            drop(self);
            r
        }
    }
    impl Drop for AuditScope<'_> {
        fn drop(&mut self) {
            let _ = self.root;
            let _ = STATE.try_with(|cell| cell.set(State::empty()));
            let _ = ALLOCATION_HOOK.try_with(|cell| cell.set((0, None)));
            let _ = DEALLOCATION_HOOK.try_with(|cell| cell.set((0, None)));
        }
    }
    impl<'a> PoolAuditScope<'a> {
        pub fn start(root: &'a ProcessLedger) -> Self {
            global(|state| {
                assert!(!state.active);
                *state = State::empty();
                state.root = root;
                state.active = true;
            });
            Self { root }
        }
        pub fn finish(self) -> Report {
            let mut result = Report {
                events: [Event::default(); COUNT],
                len: 0,
                overflow: false,
            };
            global(|state| {
                result = Report {
                    events: state.events,
                    len: state.len,
                    overflow: state.overflow,
                };
                *state = State::empty();
            });
            drop(self);
            result
        }
    }
    impl Drop for PoolAuditScope<'_> {
        fn drop(&mut self) {
            let _ = self.root;
            global(|state| *state = State::empty());
        }
    }
}
#[global_allocator]
static ALLOCATOR: audit::Allocator = audit::Allocator;

use admission::{ControlRuntime, PrepareCosts};
use frame::{Command, ControlFrame, Status};
use ledger::{Amount, BYTE_LIMIT, CLASS_COUNT, ProcessLedger};
const IDENTITY: &str = "00000000-0000-4000-8000-000000000001";

pub fn costs() -> PrepareCosts {
    PrepareCosts {
        loading: process::LoadCosts {
            raw: Amount {
                rust_bytes: 1024,
                c_bytes: 0,
                slots: 0,
            },
            ..Default::default()
        },
        reply: Amount {
            rust_bytes: 128,
            c_bytes: 0,
            slots: 0,
        },
        start_reply: Amount {
            rust_bytes: 128,
            c_bytes: 0,
            slots: 0,
        },
        terminal_reply: Amount {
            rust_bytes: 128,
            c_bytes: 0,
            slots: 0,
        },
    }
}

pub fn wire(command: Command, key: inbox::Key, request: u32, fields: &[&str]) -> Vec<u8> {
    let mut out = vec![0; frame::HEADER + fields.iter().map(|s| s.len()).sum::<usize>()];
    out[..8].copy_from_slice(frame::MAGIC);
    out[8..10].copy_from_slice(&2u16.to_le_bytes());
    out[10..12].copy_from_slice(&(command as u16).to_le_bytes());
    for (at, n) in [
        (16, key.epoch),
        (24, key.owner),
        (40, key.generation),
        (56, key.entry),
    ] {
        out[at..at + 8].copy_from_slice(&n.to_le_bytes());
    }
    for (at, n) in [
        (32, key.source_index),
        (36, key.session),
        (48, request),
        (52, key.prepare),
    ] {
        out[at..at + 4].copy_from_slice(&n.to_le_bytes());
    }
    if matches!(
        command,
        Command::RegisterReplyPort | Command::CloseReplyPort
    ) {
        out[64..72].copy_from_slice(&123i64.to_le_bytes());
    }
    if command == Command::Start {
        out[78..80].copy_from_slice(&80u16.to_le_bytes());
        out[80..82].copy_from_slice(&24u16.to_le_bytes());
        out[84..88].copy_from_slice(&800u32.to_le_bytes());
        out[88..92].copy_from_slice(&480u32.to_le_bytes());
        out[92..96].copy_from_slice(&96u32.to_le_bytes());
    }
    let mut at = frame::HEADER;
    for (i, text) in fields.iter().enumerate() {
        out[112 + i * 16..120 + i * 16].copy_from_slice(&(at as u64).to_le_bytes());
        out[120 + i * 16..128 + i * 16].copy_from_slice(&(text.len() as u64).to_le_bytes());
        out[at..at + text.len()].copy_from_slice(text.as_bytes());
        at += text.len();
    }
    out
}

fn submit(runtime: &ControlRuntime<'_>, wire: &[u8], secret: &[u8]) -> [u64; 8] {
    runtime.submit(
        ControlFrame::parse(wire, secret).unwrap_or_else(|error| panic!("非法测试帧: {error:?}")),
    )
}
fn registered(runtime: &ControlRuntime<'_>) -> inbox::Key {
    let result = submit(
        runtime,
        &wire(
            Command::RegisterReplyPort,
            inbox::Key {
                epoch: 1,
                ..Default::default()
            },
            0,
            &[],
        ),
        &[],
    );
    assert_eq!(result[0], 0);
    inbox::Key {
        epoch: result[1],
        owner: result[2],
        ..Default::default()
    }
}
fn prepare(
    runtime: &ControlRuntime<'_>,
    owner: inbox::Key,
    source: ledger::SourceToken,
    session: u32,
    request: u32,
) -> inbox::Key {
    let result = submit(
        runtime,
        &wire(
            Command::Prepare,
            inbox::Key {
                generation: source.generation,
                source_index: source.index,
                session,
                ..owner
            },
            request,
            &["", "", "localhost", "fixture", ""],
        ),
        &[],
    );
    assert_eq!(result[0], 0);
    inbox::Key {
        epoch: result[1],
        owner: result[2],
        entry: result[3],
        generation: result[4],
        source_index: result[5] as u32,
        session: result[6] as u32,
        prepare: result[7] as u32,
    }
}

#[test]
fn ingress_five_controls_start_once_and_receiver_drop_wipes() {
    let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
    let source = root
        .open_source(1, 0)
        .unwrap_or_else(|error| panic!("source: {error:?}"));
    let runtime =
        ControlRuntime::new(&root, 1, costs()).unwrap_or_else(|error| panic!("inbox: {error:?}"));
    let owner = registered(&runtime);
    let key = prepare(&runtime, owner, source, 1, 1);
    let work = runtime
        .next_batch()
        .normal
        .unwrap_or_else(|| panic!("缺 Prepare work"));
    assert!(matches!(&work, inbox::NormalWork::Prepare(_)));
    runtime
        .install_prepared(key, IDENTITY, "main", "fallback", false)
        .unwrap_or_else(|error| panic!("prepared: {error:?}"));
    drop(work);
    let start = wire(Command::Start, key, 2, &[IDENTITY, "main"]);
    let password = vec![b'x'; 1_048_577];
    let audit = audit::AuditScope::start(&root);
    audit.inspect_secret_free(password.len());
    assert_eq!(submit(&runtime, &start, &password)[0], 0);
    assert_eq!(
        submit(&runtime, &start, &password)[0],
        Status::AlreadyStarted as u64
    );
    let transfer = runtime
        .next_batch()
        .normal
        .unwrap_or_else(|| panic!("缺 Start transfer"));
    assert!(matches!(&transfer, inbox::NormalWork::Start(_)));
    assert_eq!(
        submit(&runtime, &wire(Command::Cancel, key, 3, &[IDENTITY]), &[])[0],
        Status::AlreadyStarted as u64
    );
    drop(transfer);
    let report = audit.finish();
    assert!(
        report
            .events()
            .iter()
            .any(|e| e.kind == 2 && e.size == password.len() && e.zero_before_free)
    );
    let close = wire(Command::CloseReplyPort, owner, 0, &[]);
    assert_eq!(submit(&runtime, &close, &[])[0], 0);
    assert_eq!(root.snapshot().epoch, 2);
    drop(runtime);
    assert_eq!(root.snapshot().rust_bytes, 0);
}

#[test]
fn ingress_copy_claim_cancel_close_and_fairness_do_not_reuse_copy_slot() {
    let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
    let source = root
        .open_source(1, 0)
        .unwrap_or_else(|error| panic!("source: {error:?}"));
    let runtime =
        ControlRuntime::new(&root, 1, costs()).unwrap_or_else(|error| panic!("inbox: {error:?}"));
    let owner = registered(&runtime);
    let prepare_wire = wire(
        Command::Prepare,
        inbox::Key {
            generation: source.generation,
            source_index: source.index,
            session: 1,
            ..owner
        },
        1,
        &["", "", "host", "user", ""],
    );
    let view =
        ControlFrame::parse(&prepare_wire, &[]).unwrap_or_else(|error| panic!("frame: {error:?}"));
    let claim = runtime
        .claim(view)
        .unwrap_or_else(|error| panic!("claim: {error:?}"));
    let key = claim.key;
    assert_eq!(
        submit(&runtime, &wire(Command::Cancel, key, 2, &[""]), &[])[0],
        0
    );
    let terminal = runtime
        .next_batch()
        .terminal
        .unwrap_or_else(|| panic!("缺独立 terminal"));
    let occupied = root.snapshot().rust_bytes;
    drop(terminal);
    assert!(root.snapshot().rust_bytes < occupied);
    assert_eq!(claim.copy_and_install(), Err(Status::Cancelled));
    drop(runtime.next_batch());
    for session in 1..=8 {
        prepare(&runtime, owner, source, session, session + 10);
    }
    let mut seen = [false; 8];
    for _ in 0..8 {
        let batch = runtime.next_batch();
        if let Some(inbox::NormalWork::Prepare(work)) = batch.normal {
            seen[work.key.session as usize - 1] = true;
            runtime.finish_work(work.key, Status::Cancelled);
        }
    }
    assert!(seen.iter().all(|s| *s));
    assert_eq!(
        submit(&runtime, &wire(Command::CloseReplyPort, owner, 0, &[]), &[])[0],
        0
    );
    for _ in 0..9 {
        drop(runtime.next_batch());
    }
    drop(runtime);
    assert_eq!(root.snapshot().rust_bytes, 0);
}

#[test]
fn ingress_invalid_duplicate_full_and_oversized_identity_have_no_owned_allocations() {
    let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
    let source = root
        .open_source(1, 0)
        .unwrap_or_else(|error| panic!("source: {error:?}"));
    let runtime =
        ControlRuntime::new(&root, 1, costs()).unwrap_or_else(|error| panic!("inbox: {error:?}"));
    let owner = registered(&runtime);
    let key = prepare(&runtime, owner, source, 1, 1);
    let work = runtime
        .next_batch()
        .normal
        .unwrap_or_else(|| panic!("缺 work"));
    runtime
        .install_prepared(key, IDENTITY, "main", "fallback", false)
        .unwrap_or_else(|error| panic!("binding: {error:?}"));
    drop(work);
    let identity = "a".repeat(1024 * 1024);
    let oversized = wire(Command::Start, key, 2, &[&identity, "main"]);
    let audit = audit::AuditScope::start(&root);
    for _ in 0..1000 {
        assert_eq!(
            submit(&runtime, &oversized, b"secret")[0],
            Status::Identity as u64
        );
    }
    assert!(audit.finish().events().is_empty());
    for session in 2..=8 {
        prepare(&runtime, owner, source, session, session);
    }
    let full = wire(
        Command::Prepare,
        inbox::Key {
            generation: source.generation,
            session: 9,
            ..owner
        },
        9,
        &["", "", "host", "user", ""],
    );
    let audit = audit::AuditScope::start(&root);
    for _ in 0..1000 {
        assert_eq!(submit(&runtime, &full, &[])[0], Status::Full as u64);
    }
    assert!(audit.finish().events().is_empty());
    assert_eq!(
        submit(&runtime, &wire(Command::Cancel, key, 10, &[IDENTITY]), &[])[0],
        0
    );
    assert!(runtime.next_batch().terminal.is_some());
    drop(runtime);
    assert_eq!(root.snapshot().rust_bytes, 0);
}

#[test]
fn ingress_every_layout_is_reserved_until_deallocation_and_zero_budget_rejects() {
    let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
    let source = root
        .open_source(1, 0)
        .unwrap_or_else(|error| panic!("source: {error:?}"));
    let register_wire = wire(
        Command::RegisterReplyPort,
        inbox::Key {
            epoch: 1,
            ..Default::default()
        },
        0,
        &[],
    );
    let prepare_wire = wire(
        Command::Prepare,
        inbox::Key {
            epoch: 1,
            owner: 1,
            generation: source.generation,
            session: 1,
            ..Default::default()
        },
        1,
        &["", "", "host", "user", ""],
    );
    let cancel_wire = wire(
        Command::Cancel,
        inbox::Key {
            epoch: 1,
            owner: 1,
            entry: 2,
            generation: source.generation,
            session: 1,
            prepare: 1,
            ..Default::default()
        },
        2,
        &[""],
    );
    let audit = audit::AuditScope::start(&root);
    let runtime =
        ControlRuntime::new(&root, 1, costs()).unwrap_or_else(|error| panic!("runtime: {error:?}"));
    assert_eq!(submit(&runtime, &register_wire, &[])[0], 0);
    assert_eq!(submit(&runtime, &prepare_wire, &[])[0], 0);
    assert_eq!(submit(&runtime, &cancel_wire, &[])[0], 0);
    drop(runtime.next_batch());
    drop(runtime);
    let report = audit.finish();
    assert!(report.every_live_layout_covered());
    let empty = ProcessLedger::new(0, [u32::MAX; CLASS_COUNT]);
    let audit = audit::AuditScope::start(&empty);
    assert_eq!(
        ControlRuntime::new(&empty, 1, costs()).err(),
        Some(layout::NumericReject::RustBudgetExceeded)
    );
    assert!(audit.finish().events().is_empty());
}

#[test]
fn ingress_ffi_result_validation_stale_copy_and_expiry_preserve_roots() {
    let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
    let source = root
        .open_source(1, 0)
        .unwrap_or_else(|error| panic!("source: {error:?}"));
    let runtime =
        ControlRuntime::new(&root, 1, costs()).unwrap_or_else(|error| panic!("inbox: {error:?}"));
    let owner = registered(&runtime);
    let prepare_wire = wire(
        Command::Prepare,
        inbox::Key {
            generation: source.generation,
            source_index: source.index,
            session: 1,
            ..owner
        },
        1,
        &["", "", "host", "user", ""],
    );
    let mut output = [0u64; 8];
    let audit = audit::AuditScope::start(&root);
    assert_eq!(
        unsafe {
            frame::with_ffi_frame(
                prepare_wire.as_ptr(),
                prepare_wire.len(),
                prepare_wire.as_ptr(),
                1,
                output.as_mut_ptr(),
                8,
                |_| panic!("重叠输入不得进入 owned copy"),
            )
        },
        Status::Invalid
    );
    assert_eq!(output, [0; 8]);
    assert_eq!(
        unsafe {
            frame::with_ffi_frame(
                prepare_wire.as_ptr(),
                prepare_wire.len(),
                std::ptr::null(),
                0,
                output.as_mut_ptr(),
                7,
                |_| panic!("结果区错误不得调用"),
            )
        },
        Status::Invalid
    );
    assert_eq!(
        unsafe {
            frame::with_ffi_frame(
                std::ptr::null(),
                1,
                std::ptr::null(),
                0,
                output.as_mut_ptr(),
                8,
                |_| panic!("NULL 输入不得调用"),
            )
        },
        Status::Invalid
    );
    assert!(audit.finish().events().is_empty());
    let claim = runtime
        .claim(
            ControlFrame::parse(&prepare_wire, &[])
                .unwrap_or_else(|error| panic!("frame: {error:?}")),
        )
        .unwrap_or_else(|error| panic!("claim: {error:?}"));
    let key = claim.key;
    let held = root.snapshot().rust_bytes;
    root.invalidate_source(source)
        .unwrap_or_else(|error| panic!("invalidate: {error:?}"));
    assert_eq!(root.snapshot().rust_bytes, held);
    assert_eq!(claim.copy_and_install(), Err(Status::Cancelled));
    assert_eq!(
        runtime
            .next_batch()
            .terminal
            .unwrap_or_else(|| panic!("缺 terminal"))
            .key,
        key
    );
    drop(runtime.next_batch());
    let source = root
        .open_source(1, 0)
        .unwrap_or_else(|error| panic!("新 source: {error:?}"));
    let key = prepare(&runtime, owner, source, 1, 2);
    runtime.expire_at(std::time::Instant::now() + std::time::Duration::from_secs(31));
    let batch = runtime.next_batch();
    let terminal = batch
        .terminal
        .unwrap_or_else(|| panic!("缺 expired terminal"));
    assert_eq!(terminal.key, key);
    assert_eq!(terminal.reason, Status::Expired);
    assert!(batch.normal.is_none());
    drop(terminal);
    drop(runtime);
    assert_eq!(root.snapshot().rust_bytes, 0);
}

static COPY_BARRIER: std::sync::Barrier = std::sync::Barrier::new(2);
fn pause_owned_allocation() {
    COPY_BARRIER.wait();
    COPY_BARRIER.wait();
}

#[test]
fn ingress_cancel_during_actual_secret_copy_keeps_claim_and_wipes_by_copy_owner() {
    let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
    let source = root
        .open_source(1, 0)
        .unwrap_or_else(|error| panic!("source: {error:?}"));
    let runtime =
        ControlRuntime::new(&root, 1, costs()).unwrap_or_else(|error| panic!("inbox: {error:?}"));
    let owner = registered(&runtime);
    let key = prepare(&runtime, owner, source, 1, 1);
    let work = runtime
        .next_batch()
        .normal
        .unwrap_or_else(|| panic!("缺 work"));
    runtime
        .install_prepared(key, IDENTITY, "main", "fallback", false)
        .unwrap_or_else(|error| panic!("prepared: {error:?}"));
    drop(work);
    let start = wire(Command::Start, key, 2, &[IDENTITY, "main"]);
    let cancel = wire(Command::Cancel, key, 3, &[IDENTITY]);
    let password = [b's'; 8193];
    std::thread::scope(|scope| {
        let canceller = scope.spawn(|| {
            COPY_BARRIER.wait();
            assert_eq!(submit(&runtime, &cancel, &[])[0], 0);
            assert_eq!(
                root.snapshot().classes[ledger::ResourceClass::Secret as usize].rust_bytes,
                password.len()
            );
            let terminal = runtime
                .next_batch()
                .terminal
                .unwrap_or_else(|| panic!("缺 terminal"));
            drop(terminal);
            assert_eq!(
                root.snapshot().classes[ledger::ResourceClass::Secret as usize].rust_bytes,
                password.len()
            );
            COPY_BARRIER.wait();
        });
        let audit = audit::AuditScope::start(&root);
        audit.inspect_secret_free(password.len());
        audit.after_next_allocation(password.len(), pause_owned_allocation);
        assert_eq!(
            submit(&runtime, &start, &password)[0],
            Status::Cancelled as u64
        );
        let report = audit.finish();
        assert!(report.events().iter().any(|event| event.kind == 2
            && event.size == password.len()
            && event.zero_before_free
            && event.held_rust >= password.len()));
        assert!(canceller.join().is_ok());
    });
    drop(runtime.next_batch());
    drop(runtime);
    assert_eq!(root.snapshot().rust_bytes, 0);
}

#[test]
fn ingress_terminal_works_with_no_remaining_global_grant_and_duplicates_allocate_nothing() {
    let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
    let source = root
        .open_source(1, 0)
        .unwrap_or_else(|error| panic!("source: {error:?}"));
    let runtime =
        ControlRuntime::new(&root, 1, costs()).unwrap_or_else(|error| panic!("inbox: {error:?}"));
    let owner = registered(&runtime);
    let key = prepare(&runtime, owner, source, 1, 1);
    let duplicate = wire(
        Command::Prepare,
        inbox::Key {
            entry: 0,
            prepare: 0,
            ..key
        },
        1,
        &["", "", "host", "user", ""],
    );
    let cancel = wire(Command::Cancel, key, 2, &[""]);
    let snapshot = root.snapshot();
    let occupied = root
        .try_reserve(
            Amount {
                rust_bytes: BYTE_LIMIT - snapshot.rust_bytes,
                c_bytes: 0,
                slots: 0,
            },
            ledger::ResourceClass::Runtime,
            ledger::Admission {
                epoch: 1,
                source: None,
            },
        )
        .unwrap_or_else(|error| panic!("占满预约: {error:?}"));
    let audit = audit::AuditScope::start(&root);
    for _ in 0..1000 {
        assert_eq!(
            submit(&runtime, &duplicate, &[])[0],
            Status::Duplicate as u64
        );
    }
    assert_eq!(submit(&runtime, &cancel, &[])[0], 0);
    assert_eq!(submit(&runtime, &cancel, &[])[0], 0);
    assert!(audit.finish().events().is_empty());
    let batch = runtime.next_batch();
    assert!(batch.terminal.is_some());
    assert!(batch.normal.is_none());
    drop(batch);
    drop(occupied);
    drop(runtime);
    assert_eq!(root.snapshot().rust_bytes, 0);
}

static INSTALLED_COPY_BARRIER: std::sync::Barrier = std::sync::Barrier::new(2);
static CLEANUP_BEGIN_BARRIER: std::sync::Barrier = std::sync::Barrier::new(2);
static SECRET_FREE_BARRIER: std::sync::Barrier = std::sync::Barrier::new(2);
static CLEANUP_UNWIND: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn pause_completed_copy_and_cleanup(point: u32) {
    let barrier = if point == 1 {
        &INSTALLED_COPY_BARRIER
    } else {
        &CLEANUP_BEGIN_BARRIER
    };
    barrier.wait();
    barrier.wait();
    if point == 2 && CLEANUP_UNWIND.load(std::sync::atomic::Ordering::Acquire) {
        panic!("测试 copy owner 的 unwind 清理");
    }
}

fn pause_secret_before_free() {
    SECRET_FREE_BARRIER.wait();
    SECRET_FREE_BARRIER.wait();
}

#[test]
fn ingress_cancelled_completed_copy_holds_slot_until_owned_cleanup_finishes() {
    for unwind in [false, true] {
        CLEANUP_UNWIND.store(unwind, std::sync::atomic::Ordering::Release);
        let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
        let source = root
            .open_source(1, 0)
            .unwrap_or_else(|e| panic!("source: {e:?}"));
        let runtime =
            ControlRuntime::new(&root, 1, costs()).unwrap_or_else(|e| panic!("inbox: {e:?}"));
        let owner = registered(&runtime);
        let key = prepare(&runtime, owner, source, 1, 1);
        let work = runtime
            .next_batch()
            .normal
            .unwrap_or_else(|| panic!("缺 work"));
        runtime
            .install_prepared(key, IDENTITY, "main", "fallback", false)
            .unwrap_or_else(|e| panic!("prepared: {e:?}"));
        drop(work);
        for session in 2..=8 {
            prepare(&runtime, owner, source, session, session);
        }
        let start = wire(Command::Start, key, 9, &[IDENTITY, "main"]);
        let cancel = wire(Command::Cancel, key, 10, &[IDENTITY]);
        let reuse = wire(
            Command::Prepare,
            inbox::Key {
                generation: source.generation,
                source_index: source.index,
                session: 9,
                ..owner
            },
            20,
            &["", "", "fixture", "fixture", ""],
        );
        let password = [b's'; 4103];
        let (cancelled, terminal_key, before_wipe, before_free, held, result, report) =
            std::thread::scope(|scope| {
                let copier = scope.spawn(|| {
                    let audit = audit::AuditScope::start(&root);
                    audit.inspect_secret_free(password.len());
                    audit.before_next_deallocation(password.len(), pause_secret_before_free);
                    admission::test_hooks::set(Some(pause_completed_copy_and_cleanup));
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        submit(&runtime, &start, &password)[0]
                    }));
                    admission::test_hooks::set(None);
                    (result, audit.finish())
                });
                INSTALLED_COPY_BARRIER.wait();
                let cancelled = submit(&runtime, &cancel, &[])[0];
                INSTALLED_COPY_BARRIER.wait();
                CLEANUP_BEGIN_BARRIER.wait();
                let first = runtime.next_batch();
                let terminal_key = first.terminal.as_ref().map(|terminal| terminal.key);
                drop(first.terminal);
                let before_wipe = submit(&runtime, &reuse, &[])[0];
                CLEANUP_BEGIN_BARRIER.wait();
                SECRET_FREE_BARRIER.wait();
                let second = runtime.next_batch();
                let before_free = submit(&runtime, &reuse, &[])[0];
                let held =
                    root.snapshot().classes[ledger::ResourceClass::Secret as usize].rust_bytes;
                SECRET_FREE_BARRIER.wait();
                // 全部放行析构并 join 后才作断言；allocator 回调中不 panic。
                let (result, report) = copier.join().unwrap_or_else(|_| panic!("copy 线程异常"));
                drop(first.normal);
                drop(second);
                (
                    cancelled,
                    terminal_key,
                    before_wipe,
                    before_free,
                    held,
                    result,
                    report,
                )
            });
        drop(runtime.next_batch());
        let after_cleanup = submit(&runtime, &reuse, &[])[0];
        let secret_after =
            root.snapshot().classes[ledger::ResourceClass::Secret as usize].rust_bytes;
        drop(runtime);
        assert_eq!(root.snapshot().rust_bytes, 0);
        assert_eq!(cancelled, Status::Admitted as u64);
        assert_eq!(terminal_key, Some(key));
        assert_eq!(
            (before_wipe, before_free),
            (Status::Full as u64, Status::Full as u64)
        );
        assert_eq!(after_cleanup, Status::Admitted as u64);
        assert_eq!(held, password.len());
        assert_eq!(secret_after, 0);
        if unwind {
            assert!(result.is_err());
        } else {
            assert_eq!(
                result.unwrap_or_else(|_| panic!("非 unwind 分支异常")),
                Status::Cancelled as u64
            );
        }
        assert!(!report.overflow);
        assert!(report.events().iter().any(|event| event.kind == 2
            && event.size == password.len()
            && event.zero_before_free
            && event.held_rust >= password.len()));
    }
    CLEANUP_UNWIND.store(false, std::sync::atomic::Ordering::Release);
}

#[test]
fn ingress_workspace_port_is_shared_and_nested_borrowed_entry_cannot_steal_registration() {
    let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
    let source = root
        .open_source(1, 0)
        .unwrap_or_else(|error| panic!("source: {error:?}"));
    let runtime =
        ControlRuntime::new(&root, 1, costs()).unwrap_or_else(|error| panic!("runtime: {error:?}"));
    let owner = registered(&runtime);
    let first = prepare(&runtime, owner, source, 1, 1);
    let second = prepare(&runtime, owner, source, 2, 2);
    let repeated = wire(Command::RegisterReplyPort, owner, 0, &[]);
    let mut stolen = repeated.clone();
    stolen[64..72].copy_from_slice(&124i64.to_le_bytes());
    let cancel_first = wire(Command::Cancel, first, 3, &[""]);
    let cancel_second = wire(Command::Cancel, second, 4, &[""]);
    let mut outer = [0u64; 8];
    let mut inner = [0u64; 8];
    let audit = audit::AuditScope::start(&root);
    assert_eq!(submit(&runtime, &repeated, &[])[0], 0);
    assert_eq!(submit(&runtime, &stolen, &[])[0], Status::Duplicate as u64);
    // 同一 FFI consume 内再次进入五控制；借用结束前不持有 inbox 锁或调用者 owned copy。
    let status = unsafe {
        frame::with_ffi_frame(
            cancel_first.as_ptr(),
            cancel_first.len(),
            std::ptr::null(),
            0,
            outer.as_mut_ptr(),
            8,
            |entry| {
                let result = runtime.submit(entry);
                assert_eq!(
                    frame::with_ffi_frame(
                        cancel_second.as_ptr(),
                        cancel_second.len(),
                        std::ptr::null(),
                        0,
                        inner.as_mut_ptr(),
                        8,
                        |entry| runtime.submit(entry),
                    ),
                    Status::Admitted
                );
                result
            },
        )
    };
    assert_eq!(status, Status::Admitted);
    assert_eq!(outer[3], first.entry);
    assert_eq!(inner[3], second.entry);
    assert!(audit.finish().events().is_empty());
    let terminal_first = runtime
        .next_batch()
        .terminal
        .unwrap_or_else(|| panic!("缺第一份终结"));
    let terminal_second = runtime
        .next_batch()
        .terminal
        .unwrap_or_else(|| panic!("缺第二份终结"));
    assert_ne!(terminal_first.key.session, terminal_second.key.session);
    let held = root.snapshot().rust_bytes;
    assert_eq!(
        submit(&runtime, &wire(Command::CloseReplyPort, owner, 0, &[]), &[])[0],
        0
    );
    assert_eq!(root.snapshot().rust_bytes, held);
    drop(runtime);
    assert!(root.snapshot().rust_bytes > 0);
    drop(terminal_first);
    drop(terminal_second);
    assert_eq!(root.snapshot().rust_bytes, 0);
}
