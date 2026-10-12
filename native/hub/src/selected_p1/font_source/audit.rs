//! 仅独立 integration executable 通过 #[path] 链接的固定 allocator 审计器。

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::RefCell;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::ledger::ProcessLedger;

const RECORDS: usize = 256;

#[derive(Clone, Copy, Default)]
pub struct Event {
    pub kind: u32,
    pub size: usize,
    pub align: usize,
    pub held_rust: usize,
    pub source_bytes: usize,
    pub reply_bytes: usize,
    pub zero_before_free: bool,
    pointer: usize,
}

struct State {
    active: bool,
    root: *const ProcessLedger,
    events: [Event; RECORDS],
    count: usize,
    overflow: bool,
    fail_size: Option<usize>,
    cancel: *const AtomicBool,
    cancel_alloc: usize,
    allocations: usize,
    secret_size: Option<usize>,
    secret_pointer: usize,
}

impl State {
    const fn empty() -> Self {
        Self {
            active: false,
            root: std::ptr::null(),
            events: [Event {
                kind: 0,
                size: 0,
                align: 0,
                held_rust: 0,
                source_bytes: 0,
                reply_bytes: 0,
                zero_before_free: false,
                pointer: 0,
            }; RECORDS],
            count: 0,
            overflow: false,
            fail_size: None,
            cancel: std::ptr::null(),
            cancel_alloc: 0,
            allocations: 0,
            secret_size: None,
            secret_pointer: 0,
        }
    }
}

thread_local! { static STATE: RefCell<State> = const { RefCell::new(State::empty()) }; }

fn should_fail(layout: Layout) -> bool {
    STATE
        .try_with(|cell| {
            let mut state = cell.borrow_mut();
            if state.active && state.fail_size == Some(layout.size()) {
                state.fail_size = None;
                true
            } else {
                false
            }
        })
        .unwrap_or(false)
}

fn record(kind: u32, pointer: *mut u8, layout: Layout) {
    let _ = STATE.try_with(|cell| {
        let mut state = cell.borrow_mut();
        if !state.active {
            return;
        }
        // SAFETY: scope 的借用保证 ledger 与 cancel 活着；线程本地 scope 不可 Send。
        let snapshot = unsafe { &*state.root }.snapshot();
        if kind == 1 && state.secret_size == Some(layout.size()) && state.secret_pointer == 0 {
            state.secret_pointer = pointer as usize;
        }
        let zero_before_free = if kind == 2
            && state.secret_pointer == pointer as usize
            && state.secret_size == Some(layout.size())
        {
            // SAFETY: 只观察已登记 secret 的已初始化字节，仍在 System.dealloc 前。
            unsafe { std::slice::from_raw_parts(pointer, layout.size()) }
                .iter()
                .all(|byte| *byte == 0)
        } else {
            false
        };
        if kind == 1 {
            state.allocations += 1;
            if state.allocations == state.cancel_alloc && !state.cancel.is_null() {
                unsafe { &*state.cancel }.store(true, Ordering::Release);
            }
        }
        let index = state.count;
        if index == RECORDS {
            state.overflow = true;
            return;
        }
        state.events[index] = Event {
            kind,
            pointer: pointer as usize,
            size: layout.size(),
            align: layout.align(),
            held_rust: snapshot.rust_bytes,
            source_bytes: snapshot.classes[6].rust_bytes,
            reply_bytes: snapshot.classes[8].rust_bytes,
            zero_before_free,
        };
        state.count += 1;
    });
}

pub struct Allocator;

// SAFETY: 所有成功 allocation 与原 Layout 原样转发 System；故障注入只返回 null。
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if should_fail(layout) {
            record(4, std::ptr::null_mut(), layout);
            return std::ptr::null_mut();
        }
        let pointer = unsafe { System.alloc(layout) };
        record(1, pointer, layout);
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if should_fail(layout) {
            record(4, std::ptr::null_mut(), layout);
            return std::ptr::null_mut();
        }
        let pointer = unsafe { System.alloc_zeroed(layout) };
        record(1, pointer, layout);
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        record(2, pointer, layout);
        unsafe { System.dealloc(pointer, layout) };
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let Ok(next) = Layout::from_size_align(size, layout.align()) else {
            return std::ptr::null_mut();
        };
        if should_fail(next) {
            record(4, pointer, next);
            return std::ptr::null_mut();
        }
        let next_pointer = unsafe { System.realloc(pointer, layout, size) };
        if !next_pointer.is_null() {
            record(2, pointer, layout);
            record(3, next_pointer, next);
        }
        next_pointer
    }
}

pub struct AuditScope<'root> {
    marker: PhantomData<(&'root ProcessLedger, *mut ())>,
}

pub struct Report {
    events: [Event; RECORDS],
    count: usize,
    pub overflow: bool,
}

impl<'root> AuditScope<'root> {
    pub fn start(root: &'root ProcessLedger) -> Self {
        STATE.with(|cell| {
            let mut state = cell.borrow_mut();
            assert!(!state.active);
            *state = State::empty();
            state.active = true;
            state.root = root;
        });
        Self {
            marker: PhantomData,
        }
    }

    pub fn fail_next_size(&self, size: usize) {
        STATE.with(|cell| cell.borrow_mut().fail_size = Some(size));
    }

    pub fn inspect_secret_free(&self, size: usize) {
        STATE.with(|cell| cell.borrow_mut().secret_size = Some(size));
    }

    pub fn cancel_on_allocation(&self, cancel: &'root AtomicBool, number: usize) {
        STATE.with(|cell| {
            let mut state = cell.borrow_mut();
            state.cancel = cancel;
            state.cancel_alloc = number;
        });
    }

    pub fn finish(self) -> Report {
        let report = STATE.with(|cell| {
            let state = cell.replace(State::empty());
            Report {
                events: state.events,
                count: state.count,
                overflow: state.overflow,
            }
        });
        drop(self);
        report
    }
}

impl Drop for AuditScope<'_> {
    fn drop(&mut self) {
        let _ = STATE.try_with(|cell| *cell.borrow_mut() = State::empty());
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

    /// 逐指针配对原 Layout；dealloc 时额度仍持有，不能仅检查事后高水位。
    pub fn assert_balanced_and_pregranted(&self) {
        assert!(!self.overflow);
        let mut live = [Event::default(); RECORDS];
        let mut bytes = 0;
        for event in self.events() {
            match event.kind {
                1 | 3 => {
                    assert_ne!(event.pointer, 0);
                    let Some(slot) = live.iter_mut().find(|slot| slot.pointer == 0) else {
                        panic!("审计容量不足");
                    };
                    *slot = *event;
                    bytes += event.size;
                    assert!(bytes <= event.held_rust);
                    assert!(event.source_bytes + event.reply_bytes >= bytes);
                }
                2 => {
                    let Some(slot) = live.iter_mut().find(|slot| slot.pointer == event.pointer)
                    else {
                        panic!("释放未知 allocation");
                    };
                    assert_eq!((slot.size, slot.align), (event.size, event.align));
                    assert!(event.held_rust >= bytes);
                    bytes -= slot.size;
                    *slot = Event::default();
                }
                4 => assert!(event.held_rust >= event.size),
                _ => panic!("未知 allocator 事件"),
            }
        }
        assert_eq!(bytes, 0);
    }
}
