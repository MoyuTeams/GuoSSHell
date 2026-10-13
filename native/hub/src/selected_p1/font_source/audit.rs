//! 仅独立 integration executable 通过 #[path] 链接的固定 allocator 审计器。

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::{Cell, RefCell};
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

struct Control {
    active: Cell<bool>,
    busy: Cell<bool>,
    incomplete: Cell<bool>,
}

// 无 Drop、常量初始化的 TLS 不依赖析构顺序；状态借用失败仍能登记审计缺口。
thread_local! {
    static CONTROL: Control = const { Control {
        active: Cell::new(false),
        busy: Cell::new(false),
        incomplete: Cell::new(false),
    } };
}

fn incomplete() {
    CONTROL
        .try_with(|control| control.incomplete.set(true))
        .unwrap_or_else(|_| std::process::abort());
}

struct AllocatorEntry;

fn enter_allocator() -> Option<AllocatorEntry> {
    CONTROL
        .try_with(|control| {
            if !control.active.get() {
                return None;
            }
            if control.busy.replace(true) {
                control.incomplete.set(true);
                return None;
            }
            Some(AllocatorEntry)
        })
        // 无法判断审计是否活动时不能声称完整；allocator 内不能 unwind。
        .unwrap_or_else(|_| std::process::abort())
}

impl Drop for AllocatorEntry {
    fn drop(&mut self) {
        CONTROL
            .try_with(|control| control.busy.set(false))
            .unwrap_or_else(|_| std::process::abort());
    }
}

fn with_state<T>(operation: impl FnOnce(&mut State) -> T) -> Option<T> {
    STATE
        .try_with(|cell| match cell.try_borrow_mut() {
            Ok(mut state) => Some(operation(&mut state)),
            Err(_) => {
                incomplete();
                None
            }
        })
        .unwrap_or_else(|_| {
            incomplete();
            None
        })
}

/// 直接回归在真实 allocator 操作期间持有审计状态借用。
#[cfg(test)]
pub fn with_state_borrowed_for_test(operation: impl FnOnce()) {
    STATE.with(|cell| {
        let _state = cell.borrow_mut();
        operation();
    });
}

#[cfg(test)]
thread_local! {
    static SNAPSHOT_HOOK: std::cell::Cell<Option<fn()>> = const { std::cell::Cell::new(None) };
}

/// 在真实 snapshot 的边界注入一次 allocator 重入。
#[cfg(test)]
pub fn before_snapshot_for_test(operation: fn()) {
    SNAPSHOT_HOOK.with(|hook| hook.set(Some(operation)));
}

fn should_fail(layout: Layout) -> bool {
    with_state(|state| {
        if state.fail_size == Some(layout.size()) {
            state.fail_size = None;
            true
        } else {
            false
        }
    })
    .unwrap_or(false)
}

fn observe_free(pointer: *mut u8, layout: Layout) -> bool {
    with_state(|state| {
        if !pointer.is_null()
            && state.secret_pointer == pointer as usize
            && state.secret_size == Some(layout.size())
        {
            // SAFETY: 只观察已登记 secret 的已初始化字节，仍在 System 释放/重分配前。
            unsafe { std::slice::from_raw_parts(pointer, layout.size()) }
                .iter()
                .all(|byte| *byte == 0)
        } else {
            false
        }
    })
    .unwrap_or(false)
}

fn record(kind: u32, pointer: *mut u8, layout: Layout, zero_before_free: bool) {
    let Some(root) = with_state(|state| state.root) else {
        return;
    };
    #[cfg(test)]
    SNAPSHOT_HOOK
        .try_with(|hook| {
            if let Some(operation) = hook.take() {
                operation();
            }
        })
        .unwrap_or_else(|_| incomplete());
    // SAFETY: scope 保证 ledger 存活且不可 Send；snapshot 不跨 STATE 借用。
    // AllocatorEntry 覆盖 snapshot 与 System，嵌套操作只标记缺口，不再次取账本锁。
    let snapshot = unsafe { &*root }.snapshot();
    with_state(|state| {
        if kind == 1 && state.secret_size == Some(layout.size()) && state.secret_pointer == 0 {
            state.secret_pointer = pointer as usize;
        }
        if kind == 1 {
            let Some(allocations) = state.allocations.checked_add(1) else {
                incomplete();
                return;
            };
            state.allocations = allocations;
            if state.allocations == state.cancel_alloc && !state.cancel.is_null() {
                unsafe { &*state.cancel }.store(true, Ordering::Release);
            }
        }
        let index = state.count;
        if index >= RECORDS {
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
        let entry = enter_allocator();
        if entry.is_some() && should_fail(layout) {
            record(4, std::ptr::null_mut(), layout, false);
            return std::ptr::null_mut();
        }
        let pointer = unsafe { System.alloc(layout) };
        if entry.is_some() {
            record(
                if pointer.is_null() { 4 } else { 1 },
                pointer,
                layout,
                false,
            );
        }
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let entry = enter_allocator();
        if entry.is_some() && should_fail(layout) {
            record(4, std::ptr::null_mut(), layout, false);
            return std::ptr::null_mut();
        }
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if entry.is_some() {
            record(
                if pointer.is_null() { 4 } else { 1 },
                pointer,
                layout,
                false,
            );
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        let entry = enter_allocator();
        if entry.is_some() {
            record(2, pointer, layout, observe_free(pointer, layout));
        }
        unsafe { System.dealloc(pointer, layout) };
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let entry = enter_allocator();
        let Ok(next) = Layout::from_size_align(size, layout.align()) else {
            return std::ptr::null_mut();
        };
        if entry.is_some() && should_fail(next) {
            record(4, pointer, next, false);
            return std::ptr::null_mut();
        }
        let zero_before_free = entry.is_some() && observe_free(pointer, layout);
        let next_pointer = unsafe { System.realloc(pointer, layout, size) };
        if entry.is_some() {
            if next_pointer.is_null() {
                record(4, pointer, next, false);
            } else {
                record(2, pointer, layout, zero_before_free);
                record(3, next_pointer, next, false);
            }
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
        CONTROL.with(|control| assert!(!control.active.get(), "审计 scope 不可嵌套"));
        STATE.with(|cell| {
            let mut state = cell.borrow_mut();
            *state = State::empty();
            state.root = root;
        });
        CONTROL.with(|control| {
            control.incomplete.set(false);
            control.active.set(true);
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
        let incomplete = CONTROL.with(|control| {
            control.active.set(false);
            control.incomplete.replace(false) || control.busy.get()
        });
        let report = with_state(|state| {
            let state = std::mem::replace(state, State::empty());
            Report {
                events: state.events,
                count: state.count,
                overflow: state.overflow,
            }
        });
        drop(self);
        // 缺口在 allocator 返回并停用审计后报告，所有调用者都不能拿部分事件作证明。
        assert!(!incomplete, "allocator 审计不完整：存在重入或状态不可访问");
        let report = match report {
            Some(report) => report,
            None => panic!("allocator 审计不完整：无法读取审计状态"),
        };
        assert!(!report.overflow, "allocator 审计不完整：事件容量不足");
        report
    }
}

impl Drop for AuditScope<'_> {
    fn drop(&mut self) {
        CONTROL
            .try_with(|control| control.active.set(false))
            .unwrap_or_else(|_| std::process::abort());
        with_state(|state| *state = State::empty());
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
