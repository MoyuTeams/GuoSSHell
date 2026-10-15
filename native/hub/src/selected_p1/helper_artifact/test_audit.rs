//! 只由独立 integration target 引入：固定跨线程事件，追踪 spawn worker 的真实 free。
//! 生产 mod.rs 不引入此文件；直接 System.alloc 的 worker TLS 仍单列源式证据。

use crate::ledger::ProcessLedger;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::UnsafeCell,
    marker::PhantomData,
    sync::atomic::{AtomicBool, Ordering},
};

const RECORDS: usize = 1024;
#[derive(Clone, Copy, Default)]
pub struct Event {
    pub kind: u32,
    pub size: usize,
    pub align: usize,
    pub old_size: usize,
    pub held_rust: usize,
    pub held_c: usize,
    pub zero_before_free: bool,
    pointer: usize,
}
impl Event {
    const EMPTY: Self = Self {
        kind: 0,
        size: 0,
        align: 0,
        old_size: 0,
        held_rust: 0,
        held_c: 0,
        zero_before_free: false,
        pointer: 0,
    };
}
struct State {
    root: *const ProcessLedger,
    events: [Event; RECORDS],
    pointers: [usize; RECORDS],
    count: usize,
    overflow: bool,
    fail_size: Option<usize>,
    secret_size: Option<usize>,
    secret_pointer: usize,
}
impl State {
    const fn empty() -> Self {
        Self {
            root: std::ptr::null(),
            events: [Event::EMPTY; RECORDS],
            pointers: [0; RECORDS],
            count: 0,
            overflow: false,
            fail_size: None,
            secret_size: None,
            secret_pointer: 0,
        }
    }
}
struct Shared(UnsafeCell<State>);
// SAFETY: 所有 state 访问都由 LOCK 独占，scope 关闭也等待正在记录的 callback。
unsafe impl Sync for Shared {}
static STATE: Shared = Shared(UnsafeCell::new(State::empty()));
static LOCK: AtomicBool = AtomicBool::new(false);
struct Lock;
impl Lock {
    fn acquire() -> Self {
        while LOCK
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            std::hint::spin_loop();
        }
        Self
    }
    fn state(&mut self) -> &mut State {
        // SAFETY: 当前 guard 独占 state，引用不逃出 guard。
        unsafe { &mut *STATE.0.get() }
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        LOCK.store(false, Ordering::Release);
    }
}

fn fail(layout: Layout) -> bool {
    let mut lock = Lock::acquire();
    let state = lock.state();
    if !state.root.is_null() && state.fail_size == Some(layout.size()) {
        state.fail_size = None;
        true
    } else {
        false
    }
}
fn event(kind: u32, pointer: *mut u8, old_pointer: *mut u8, layout: Layout, old_size: usize) {
    let mut lock = Lock::acquire();
    let state = lock.state();
    if state.root.is_null() {
        return;
    }
    let id = pointer as usize;
    match kind {
        1 => {
            if let Some(slot) = state.pointers.iter_mut().find(|slot| **slot == 0) {
                *slot = id;
            } else {
                state.overflow = true;
                return;
            }
            if state.secret_size == Some(layout.size()) {
                state.secret_pointer = id;
            }
        }
        2 => {
            let Some(slot) = state.pointers.iter_mut().find(|slot| **slot == id) else {
                return;
            };
            *slot = 0;
        }
        3 => {
            let Some(slot) = state
                .pointers
                .iter_mut()
                .find(|slot| **slot == old_pointer as usize)
            else {
                return;
            };
            *slot = id;
        }
        _ => {}
    }
    if state.count == RECORDS {
        state.overflow = true;
        return;
    }
    // SAFETY: scope 的 root 借用保活；ledger 数字临界区无 allocator 调用。
    let snapshot = unsafe { &*state.root }.snapshot();
    let zero_before_free =
        if kind == 2 && state.secret_pointer == id && state.secret_size == Some(layout.size()) {
            // SAFETY: 已登记 secret 此刻还未 System.dealloc，整个 backing 已初始化。
            unsafe { std::slice::from_raw_parts(pointer, layout.size()) }
                .iter()
                .all(|byte| *byte == 0)
        } else {
            false
        };
    state.events[state.count] = Event {
        kind,
        pointer: id,
        size: layout.size(),
        align: layout.align(),
        old_size,
        held_rust: snapshot.rust_bytes,
        held_c: snapshot.c_bytes,
        zero_before_free,
    };
    state.count += 1;
}
pub struct Allocator;
// SAFETY: 每笔都原样使用 System 和原 Layout；审计表本身不分配。
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if fail(layout) {
            event(4, std::ptr::null_mut(), std::ptr::null_mut(), layout, 0);
            return std::ptr::null_mut();
        }
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            event(1, pointer, std::ptr::null_mut(), layout, 0);
        }
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if fail(layout) {
            event(4, std::ptr::null_mut(), std::ptr::null_mut(), layout, 0);
            return std::ptr::null_mut();
        }
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            event(1, pointer, std::ptr::null_mut(), layout, 0);
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        event(2, pointer, std::ptr::null_mut(), layout, 0);
        unsafe { System.dealloc(pointer, layout) };
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let Ok(next) = Layout::from_size_align(size, layout.align()) else {
            return std::ptr::null_mut();
        };
        if fail(next) {
            event(4, pointer, pointer, next, layout.size());
            return std::ptr::null_mut();
        }
        let new = unsafe { System.realloc(pointer, layout, size) };
        if !new.is_null() {
            event(3, new, pointer, next, layout.size());
        }
        new
    }
}
pub struct AuditScope<'r> {
    marker: PhantomData<(&'r ProcessLedger, *mut ())>,
}
pub struct Report {
    events: [Event; RECORDS],
    count: usize,
    pub overflow: bool,
}
impl<'r> AuditScope<'r> {
    pub fn start(root: &'r ProcessLedger) -> Self {
        let mut lock = Lock::acquire();
        let state = lock.state();
        assert!(state.root.is_null(), "audit target 必须串行运行");
        *state = State::empty();
        state.root = root;
        Self {
            marker: PhantomData,
        }
    }
    pub fn fail_next_size(&self, size: usize) {
        Lock::acquire().state().fail_size = Some(size);
    }
    pub fn inspect_secret_free(&self, size: usize) {
        Lock::acquire().state().secret_size = Some(size);
    }
    pub fn finish(self) -> Report {
        let mut lock = Lock::acquire();
        let state = lock.state();
        let report = Report {
            events: state.events,
            count: state.count,
            overflow: state.overflow,
        };
        state.root = std::ptr::null();
        drop(lock);
        drop(self);
        report
    }
}
impl Drop for AuditScope<'_> {
    fn drop(&mut self) {
        Lock::acquire().state().root = std::ptr::null();
    }
}
impl Report {
    pub fn events(&self) -> &[Event] {
        &self.events[..self.count]
    }
    pub fn symmetric_single_backing(&self) -> bool {
        let events = self.events();
        events.len() == 2
            && events[0].kind == 1
            && events[1].kind == 2
            && events[0].pointer == events[1].pointer
            && events[0].size == events[1].size
            && events[0].align == events[1].align
    }
}
