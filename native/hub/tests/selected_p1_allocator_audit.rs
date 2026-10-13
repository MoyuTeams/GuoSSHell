//! allocator 的独立直接回归；受控 snapshot 重入仍使用真实账本与 GlobalAlloc。

#[path = "../src/selected_p1/model/layout.rs"]
pub mod layout;
#[path = "../src/selected_p1/resource/ledger.rs"]
pub mod ledger;
pub mod selected_p1 {
    pub mod model {
        pub use crate::layout;
    }
}

#[path = "../src/selected_p1/font_source/audit.rs"]
pub mod audit;

#[global_allocator]
static ALLOCATOR: audit::Allocator = audit::Allocator;

use std::alloc::{GlobalAlloc, Layout};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

fn succeeds<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("预期成功：{error:?}"),
    }
}

fn child_or_run(name: &str, operation: impl FnOnce()) {
    if std::env::var("GUOSHELL_ALLOCATOR_AUDIT_CASE").as_deref() == Ok(name) {
        operation();
        return;
    }
    let status = succeeds(
        Command::new(succeeds(std::env::current_exe()))
            .args(["--exact", name, "--nocapture"])
            .env("GUOSHELL_ALLOCATOR_AUDIT_CASE", name)
            .status(),
    );
    assert!(status.success(), "allocator 回归子进程失败：{status}");
}

fn allocate_and_free() {
    let layout = succeeds(Layout::from_size_align(37, 8));
    // SAFETY: 使用相同 Layout 释放真实 GlobalAlloc 返回的非空指针。
    unsafe {
        let pointer = ALLOCATOR.alloc(layout);
        assert!(!pointer.is_null());
        ALLOCATOR.dealloc(pointer, layout);
    }
}

fn root() -> ledger::ProcessLedger {
    ledger::ProcessLedger::new(ledger::BYTE_LIMIT, [u32::MAX; ledger::CLASS_COUNT])
}

fn grant(root: &ledger::ProcessLedger, bytes: usize) -> ledger::Grant<'_> {
    succeeds(root.try_reserve(
        ledger::Amount {
            rust_bytes: bytes,
            c_bytes: 0,
            slots: 0,
        },
        ledger::ResourceClass::Source,
        ledger::Admission {
            epoch: 1,
            source: None,
        },
    ))
}

fn all_allocator_entries() {
    allocate_and_free();
    let layout = succeeds(Layout::from_size_align(37, 8));
    let next = succeeds(Layout::from_size_align(73, 8));
    // SAFETY: 重分配只用真实非空 backing；成功后用新 Layout 释放。
    unsafe {
        let pointer = ALLOCATOR.alloc_zeroed(layout);
        assert!(!pointer.is_null());
        let resized = ALLOCATOR.realloc(pointer, layout, next.size());
        assert!(!resized.is_null());
        ALLOCATOR.dealloc(resized, next);
    }
}

#[test]
fn inactive_borrow_allows_real_allocator_without_unwind() {
    child_or_run(
        "inactive_borrow_allows_real_allocator_without_unwind",
        || audit::with_state_borrowed_for_test(allocate_and_free),
    );
}

#[test]
fn active_borrow_reentry_rejects_incomplete_audit_outside_allocator() {
    child_or_run(
        "active_borrow_reentry_rejects_incomplete_audit_outside_allocator",
        || {
            let root = root();
            let scope = audit::AuditScope::start(&root);
            audit::with_state_borrowed_for_test(allocate_and_free);
            assert!(
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| scope.finish())).is_err()
            );
            let scope = audit::AuditScope::start(&root);
            assert!(scope.finish().events().is_empty());
        },
    );
}

#[test]
fn snapshot_allocator_reentry_cannot_produce_passing_report() {
    child_or_run(
        "snapshot_allocator_reentry_cannot_produce_passing_report",
        || {
            let root = root();
            let scope = audit::AuditScope::start(&root);
            audit::before_snapshot_for_test(allocate_and_free);
            allocate_and_free();
            assert!(
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| scope.finish())).is_err()
            );
        },
    );
}

#[test]
fn every_allocator_entry_handles_borrow_conflict_without_unwind() {
    child_or_run(
        "every_allocator_entry_handles_borrow_conflict_without_unwind",
        || {
            audit::with_state_borrowed_for_test(all_allocator_entries);
            let root = root();
            let scope = audit::AuditScope::start(&root);
            audit::with_state_borrowed_for_test(all_allocator_entries);
            assert!(
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| scope.finish())).is_err()
            );
        },
    );
}

#[test]
fn nested_allocator_entries_at_snapshot_leave_sticky_gap() {
    child_or_run(
        "nested_allocator_entries_at_snapshot_leave_sticky_gap",
        || {
            let root = root();
            let backing_grant = grant(&root, 37);
            let scope = audit::AuditScope::start(&root);
            audit::before_snapshot_for_test(all_allocator_entries);
            allocate_and_free();
            drop(backing_grant);
            assert!(
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| scope.finish())).is_err()
            );
        },
    );
}

#[test]
fn scope_lifecycle_under_borrow_does_not_repanic_in_allocator() {
    child_or_run(
        "scope_lifecycle_under_borrow_does_not_repanic_in_allocator",
        || {
            let root = root();
            audit::with_state_borrowed_for_test(|| {
                assert!(
                    std::panic::catch_unwind(|| audit::AuditScope::start(&root)).is_err()
                );
            });
            let scope = audit::AuditScope::start(&root);
            audit::with_state_borrowed_for_test(|| {
                assert!(
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| scope.finish())).is_err()
                );
                all_allocator_entries();
            });
            let scope = audit::AuditScope::start(&root);
            audit::with_state_borrowed_for_test(|| {
                drop(scope);
                all_allocator_entries();
            });
            let scope = audit::AuditScope::start(&root);
            assert!(scope.finish().events().is_empty());
        },
    );
}

#[test]
fn real_tls_destructor_can_audit_and_release_backing() {
    child_or_run("real_tls_destructor_can_audit_and_release_backing", || {
        static CHECKED: AtomicBool = AtomicBool::new(false);
        struct ExitAudit;
        impl Drop for ExitAudit {
            fn drop(&mut self) {
                let root = root();
                let backing_grant = grant(&root, 37);
                let scope = audit::AuditScope::start(&root);
                allocate_and_free();
                drop(backing_grant);
                let report = scope.finish();
                report.assert_balanced_and_pregranted();
                assert!(report.symmetric_single_backing());
                all_allocator_entries();
                CHECKED.store(true, Ordering::Release);
            }
        }
        thread_local! {
            static AT_EXIT: ExitAudit = const { ExitAudit };
        }
        succeeds(std::thread::spawn(|| AT_EXIT.with(|_| {})).join());
        assert!(CHECKED.load(Ordering::Acquire));
    });
}

#[test]
fn null_injection_is_one_shot_and_realloc_keeps_original_on_failure() {
    let root = root();
    let layout = succeeds(Layout::from_size_align(37, 8));
    let next = succeeds(Layout::from_size_align(73, 8));
    let backing_grant = grant(&root, next.size());
    let scope = audit::AuditScope::start(&root);
    scope.inspect_secret_free(layout.size());
    scope.fail_next_size(layout.size());
    // SAFETY: NULL 不交给 dealloc/realloc；失败的 realloc 保留原 backing。
    unsafe {
        assert!(ALLOCATOR.alloc(layout).is_null());
        scope.fail_next_size(layout.size());
        assert!(ALLOCATOR.alloc_zeroed(layout).is_null());
        let pointer = ALLOCATOR.alloc_zeroed(layout);
        assert!(!pointer.is_null());
        assert!(
            std::slice::from_raw_parts(pointer, layout.size())
                .iter()
                .all(|byte| *byte == 0)
        );
        scope.fail_next_size(next.size());
        assert!(ALLOCATOR.realloc(pointer, layout, next.size()).is_null());
        assert!(
            std::slice::from_raw_parts(pointer, layout.size())
                .iter()
                .all(|byte| *byte == 0)
        );
        let resized = ALLOCATOR.realloc(pointer, layout, next.size());
        assert!(!resized.is_null());
        assert!(
            std::slice::from_raw_parts(resized, layout.size())
                .iter()
                .all(|byte| *byte == 0)
        );
        ALLOCATOR.dealloc(resized, next);
    }
    drop(backing_grant);
    let report = scope.finish();
    report.assert_balanced_and_pregranted();
    let kinds: Vec<_> = report.events().iter().map(|event| event.kind).collect();
    assert_eq!(kinds, [4, 4, 1, 4, 2, 3, 2]);
    assert!(report.events()[4].zero_before_free);
    assert_eq!(root.snapshot().rust_bytes, 0);
}

#[test]
fn free_observes_secret_and_last_free_grant_before_system_release() {
    for wipe in [false, true] {
        let root = root();
        let cancel = AtomicBool::new(false);
        let layout = succeeds(Layout::from_size_align(37, 8));
        let backing_grant = grant(&root, layout.size());
        let scope = audit::AuditScope::start(&root);
        scope.inspect_secret_free(layout.size());
        scope.cancel_on_allocation(&cancel, 1);
        // SAFETY: 全部字节初始化后才登记释放观察；相同 Layout 释放后不再读指针。
        unsafe {
            let pointer = ALLOCATOR.alloc_zeroed(layout);
            assert!(!pointer.is_null());
            assert!(cancel.load(Ordering::Acquire));
            pointer.write_bytes(0x7b, layout.size());
            if wipe {
                pointer.write_bytes(0, layout.size());
            }
            ALLOCATOR.dealloc(pointer, layout);
        }
        drop(backing_grant);
        let report = scope.finish();
        report.assert_balanced_and_pregranted();
        assert!(report.symmetric_single_backing());
        assert_eq!(report.events()[1].zero_before_free, wipe);
        assert_eq!(report.events()[1].held_rust, layout.size());
        assert_eq!(report.events()[1].source_bytes, layout.size());
        assert_eq!(root.snapshot().rust_bytes, 0);
    }
}

#[test]
fn real_quota_rejection_precedes_any_allocator_request() {
    let root = ledger::ProcessLedger::new(1, [u32::MAX; ledger::CLASS_COUNT]);
    let scope = audit::AuditScope::start(&root);
    let rejected = root.try_reserve(
        ledger::Amount {
            rust_bytes: 37,
            c_bytes: 0,
            slots: 0,
        },
        ledger::ResourceClass::Source,
        ledger::Admission {
            epoch: 1,
            source: None,
        },
    );
    assert!(rejected.is_err());
    let report = scope.finish();
    assert!(report.events().is_empty());
    report.assert_balanced_and_pregranted();
    assert_eq!(root.snapshot().rust_bytes, 0);
}

#[test]
fn grant_assertion_rejects_missing_or_early_released_grant() {
    for early_release in [false, true] {
        let root = root();
        let backing_grant = early_release.then(|| grant(&root, 37));
        let scope = audit::AuditScope::start(&root);
        let layout = succeeds(Layout::from_size_align(37, 8));
        // SAFETY: 真实 allocation 用原 Layout 释放；仅账本额度故意违反审计约束。
        unsafe {
            let pointer = ALLOCATOR.alloc(layout);
            assert!(!pointer.is_null());
            drop(backing_grant);
            ALLOCATOR.dealloc(pointer, layout);
        }
        let report = scope.finish();
        assert!(std::panic::catch_unwind(|| report.assert_balanced_and_pregranted()).is_err());
    }
}

#[test]
fn event_capacity_exhaustion_cannot_return_partial_report() {
    let root = root();
    let backing_grant = grant(&root, 37);
    let scope = audit::AuditScope::start(&root);
    for _ in 0..257 {
        allocate_and_free();
    }
    drop(backing_grant);
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| scope.finish())).is_err());
}
