//! 独立真实消费 W1-L；不注册 hub 生产模块，不启动任何生产 P1 入口。

#[path = "../src/selected_p1/model/capacity.rs"]
pub mod capacity;
#[path = "../src/selected_p1/model/certificate.rs"]
pub mod certificate;
#[path = "../src/selected_p1/model/layout.rs"]
pub mod layout;
#[path = "../src/selected_p1/resource/ledger.rs"]
pub mod ledger;
#[path = "../src/selected_p1/resource/owner.rs"]
pub mod owner;
#[path = "../src/selected_p1/resource/secret.rs"]
pub mod secret;

pub mod selected_p1 {
    pub mod model {
        pub use crate::{capacity, certificate, layout};
    }
    pub mod resource {
        pub use crate::{ledger, owner, secret};
    }
}

/// 审计只存在于独立 test binary；固定记录，不安装到生产主进程。
pub mod audit {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;
    use std::marker::PhantomData;

    use crate::ledger::ProcessLedger;

    const RECORDS: usize = 512;

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

    #[derive(Clone, Copy)]
    struct State {
        active: bool,
        root: *const ProcessLedger,
        fail_size: Option<usize>,
        secret_size: Option<usize>,
        secret_pointer: usize,
        events: [Event; RECORDS],
        count: usize,
        overflow: bool,
    }

    impl State {
        const fn empty() -> Self {
            Self {
                active: false,
                root: std::ptr::null(),
                fail_size: None,
                secret_size: None,
                secret_pointer: 0,
                events: [Event {
                    kind: 0,
                    size: 0,
                    align: 0,
                    old_size: 0,
                    held_rust: 0,
                    held_c: 0,
                    zero_before_free: false,
                    pointer: 0,
                }; RECORDS],
                count: 0,
                overflow: false,
            }
        }
    }

    thread_local! { static STATE: Cell<State> = const { Cell::new(State::empty()) }; }

    fn fail(layout: Layout) -> bool {
        STATE
            .try_with(|cell| {
                let mut state = cell.get();
                if state.active && state.fail_size == Some(layout.size()) {
                    state.fail_size = None;
                    cell.set(state);
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false)
    }

    fn record(kind: u32, pointer: *mut u8, layout: Layout, old_size: usize) {
        let _ = STATE.try_with(|cell| {
            let mut state = cell.get();
            if !state.active {
                return;
            }
            // SAFETY: AuditScope 的借用保持 root 存活；scope 不可 Send，只服务当前线程。
            let snapshot = unsafe { &*state.root }.snapshot();
            if kind == 1 && state.secret_size == Some(layout.size()) && state.secret_pointer == 0 {
                state.secret_pointer = pointer as usize;
            }
            let zero_before_free = if kind == 2
                && state.secret_size == Some(layout.size())
                && state.secret_pointer == pointer as usize
            {
                // SAFETY: 只观察已登记的 exact secret allocation；此时 n bytes 全初始化。
                // 在 System.dealloc 前观察，绝不读取已释放指针。
                unsafe {
                    std::slice::from_raw_parts(pointer, layout.size())
                        .iter()
                        .all(|byte| *byte == 0)
                }
            } else {
                false
            };
            let event = Event {
                kind,
                size: layout.size(),
                align: layout.align(),
                old_size,
                held_rust: snapshot.rust_bytes,
                held_c: snapshot.c_bytes,
                zero_before_free,
                pointer: pointer as usize,
            };
            if state.count < RECORDS {
                state.events[state.count] = event;
                state.count += 1;
            } else {
                state.overflow = true;
            }
            cell.set(state);
        });
    }

    pub struct Allocator;

    // SAFETY: 所有正常请求原样交给 System；失败注入只返回合法 NULL，不伪造 allocation。
    // 固定 TLS 审计不分配；System 与 allocator 的 alloc/dealloc/realloc 成对使用。
    unsafe impl GlobalAlloc for Allocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            if fail(layout) {
                record(4, std::ptr::null_mut(), layout, 0);
                return std::ptr::null_mut();
            }
            // SAFETY: GlobalAlloc 调用者提供合法非零 layout。
            let pointer = unsafe { System.alloc(layout) };
            record(1, pointer, layout, 0);
            pointer
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            if fail(layout) {
                record(4, std::ptr::null_mut(), layout, 0);
                return std::ptr::null_mut();
            }
            let pointer = unsafe { System.alloc_zeroed(layout) };
            record(1, pointer, layout, 0);
            pointer
        }

        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            record(2, pointer, layout, 0);
            // SAFETY: 原样归还来自同 allocator 的 pointer/Layout。
            unsafe {
                System.dealloc(pointer, layout);
            }
        }

        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            let Ok(new_layout) = Layout::from_size_align(new_size, layout.align()) else {
                return std::ptr::null_mut();
            };
            if fail(new_layout) {
                record(4, pointer, new_layout, layout.size());
                return std::ptr::null_mut();
            }
            record(3, pointer, new_layout, layout.size());
            // SAFETY: 使用同 System allocator、原 layout 和合法 new_size。
            unsafe { System.realloc(pointer, layout, new_size) }
        }
    }

    pub struct AuditScope<'root> {
        root: &'root ProcessLedger,
        not_send: PhantomData<*mut ()>,
    }

    pub struct Report {
        events: [Event; RECORDS],
        count: usize,
        pub overflow: bool,
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

    impl<'root> AuditScope<'root> {
        pub fn start(root: &'root ProcessLedger) -> Self {
            STATE.with(|cell| {
                assert!(!cell.get().active);
                let mut state = State::empty();
                state.active = true;
                state.root = root;
                cell.set(state);
            });
            Self {
                root,
                not_send: PhantomData,
            }
        }

        pub fn fail_next_size(&self, size: usize) {
            STATE.with(|cell| {
                let mut state = cell.get();
                state.fail_size = Some(size);
                cell.set(state);
            });
        }

        pub fn inspect_secret_free(&self, size: usize) {
            STATE.with(|cell| {
                let mut state = cell.get();
                state.secret_size = Some(size);
                cell.set(state);
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
            let _ = self.root;
            let _ = STATE.try_with(|cell| cell.set(State::empty()));
        }
    }
}

#[global_allocator]
static ALLOCATOR: audit::Allocator = audit::Allocator;

#[test]
fn actual_small_resource_survives_runtime_epoch_and_returns_after_header_free()
-> Result<(), layout::NumericReject> {
    use ledger::{Admission, Amount, BYTE_LIMIT, CLASS_COUNT, ProcessLedger, ResourceClass};
    let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
    let model = owner::OwnerLayout::actual::<u64>(17)?;
    let audit = audit::AuditScope::start(&root);
    let grant = root.try_reserve(
        Amount {
            rust_bytes: model.backing.size(),
            c_bytes: 0,
            slots: 1,
        },
        ResourceClass::Reply,
        Admission {
            epoch: 1,
            source: None,
        },
    )?;
    let resource = owner::SharedOwner::try_init(grant, 17, 0, |payload| {
        payload.fill(5);
        Ok(12u64)
    })?;
    let old_peer = resource.try_clone()?;
    drop(resource);
    assert_eq!(root.advance_epoch(1)?, 2);
    assert_eq!(root.snapshot().rust_bytes, model.backing.size());
    assert_eq!(old_peer.payload(), &[5; 17]);
    assert_eq!(*old_peer.value(), 12);
    drop(old_peer);
    let report = audit.finish();
    assert!(!report.overflow);
    assert!(report.symmetric_single_backing());
    assert!(
        report
            .events()
            .iter()
            .all(|event| event.held_rust == model.backing.size())
    );
    assert_eq!(root.snapshot().rust_bytes, 0);
    Ok(())
}
