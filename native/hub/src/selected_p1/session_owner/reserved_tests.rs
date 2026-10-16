use std::alloc::Layout;
use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::task::{Context, Poll, Waker};

use super::{HolderBound, InlineLayout, InlineSessionOwner, PendingInstall, PollState};
use crate::audit::AuditScope;
use crate::selected_p1::model::layout::NumericReject;
use crate::selected_p1::resource::ledger::{
    Admission, Amount, BYTE_LIMIT, CLASS_COUNT, Grant, ProcessLedger, ResourceClass,
};
use crate::selected_p1::resource::owner::{OwnerLayout, SharedOwner};
use crate::selected_p1::resource::secret::{CopyValidity, LockedSecretString};

const fn slots() -> [u32; CLASS_COUNT] {
    let mut limits = [u32::MAX; CLASS_COUNT];
    limits[ResourceClass::Runtime as usize] = 1;
    limits
}
static ROOT: ProcessLedger = ProcessLedger::new(BYTE_LIMIT, slots());
static SERIAL: Mutex<()> = Mutex::new(());
type Expected = (ResourceClass, Amount);

struct Captured<'a> {
    secret: Option<LockedSecretString<'static>>,
    source: Option<SharedOwner<'static, String>>,
    external: &'a RefCell<Option<Waker>>,
    ready: &'a Cell<bool>,
    drops: &'a Cell<u32>,
    expected: &'a Cell<Option<Expected>>,
}

impl Future for Captured<'_> {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.external.borrow().is_none() {
            *self.external.borrow_mut() = Some(cx.waker().clone());
        }
        if self.ready.get() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

impl Drop for Captured<'_> {
    fn drop(&mut self) {
        if let Some((class, amount)) = self.expected.get() {
            let usage = ROOT.snapshot().classes[class as usize];
            assert_eq!(usage.rust_bytes, amount.rust_bytes);
            assert_eq!(usage.c_bytes, amount.c_bytes);
            assert_eq!(usage.slots, amount.slots);
        }
        self.drops.set(self.drops.get() + 1);
        drop(self.secret.take());
        drop(self.source.take());
    }
}

fn admission() -> Admission {
    Admission {
        epoch: ROOT.snapshot().epoch,
        source: None,
    }
}
fn bound() -> Result<HolderBound, NumericReject> {
    // SAFETY: 此 fixture 只有 owner、Context、cancel、一个外部槽及一次 clone，最多5。
    // 不作为正常网络 holder proof。
    unsafe { HolderBound::from_source_proof(5) }
}
fn capture<'a>(
    external: &'a RefCell<Option<Waker>>,
    ready: &'a Cell<bool>,
    drops: &'a Cell<u32>,
    expected: &'a Cell<Option<Expected>>,
) -> Result<Captured<'a>, NumericReject> {
    let secret = LockedSecretString::try_copy(
        "synthetic-secret!",
        ROOT.try_reserve(
            Amount {
                rust_bytes: 17,
                c_bytes: 0,
                slots: 1,
            },
            ResourceClass::Secret,
            admission(),
        )?,
        &CopyValidity::default(),
    )?;
    let model = OwnerLayout::actual::<String>(4)?;
    let permit = ROOT.try_reserve(
        Amount {
            rust_bytes: model.required_rust(6)?,
            c_bytes: 0,
            slots: 1,
        },
        ResourceClass::Source,
        admission(),
    )?;
    let source = SharedOwner::try_init(permit, 4, 6, |payload| {
        payload.fill(9);
        Ok("source".to_owned())
    })?;
    Ok(Captured {
        secret: Some(secret),
        source: Some(source),
        external,
        ready,
        drops,
        expected,
    })
}
fn reserve(amount: Amount, class: ResourceClass) -> Result<Grant<'static>, NumericReject> {
    ROOT.try_reserve_detailed(amount, class, admission())
        .map_err(|reject| reject.into_numeric())
}
fn exact_amount() -> Result<Amount, NumericReject> {
    Ok(Amount {
        rust_bytes: InlineLayout::actual::<Captured<'_>>()?.backing.size(),
        c_bytes: 0,
        slots: 1,
    })
}
fn audit_captures(report: &crate::audit::Report) -> Result<(), NumericReject> {
    super::super::tests::verify_each_allocation(report)?;
    assert!(
        report
            .events()
            .iter()
            .any(|event| event.kind == 2 && event.size == 17 && event.zero_before_free)
    );
    Ok(())
}

#[test]
fn reserved_same_grant_epoch_slots_and_late_waker_hold_real_captures() -> Result<(), NumericReject>
{
    let _serial = SERIAL
        .lock()
        .map_err(|_| NumericReject::ConstructionFailure)?;
    assert_eq!(ROOT.snapshot().rust_bytes, 0);
    let (external, ready, drops, expected) = (
        RefCell::new(None),
        Cell::new(false),
        Cell::new(0),
        Cell::new(None),
    );
    let audit = AuditScope::start(&ROOT);
    audit.inspect_secret_free(17);
    let future = capture(&external, &ready, &drops, &expected)?;
    let amount = exact_amount()?;
    let permit = reserve(amount, ResourceClass::Runtime)?;
    let epoch = permit.epoch();
    expected.set(Some((permit.class(), permit.amount())));
    let charged = ROOT.snapshot();
    // accepted Grant 失效仍可转移/清理；不重新准入或恢复业务 entry。
    ROOT.advance_epoch(epoch)?;
    let before = ROOT.snapshot();
    let mut owner = InlineSessionOwner::try_install_reserved(future, permit, bound()?)?;
    assert_eq!(ROOT.snapshot(), before);
    // SAFETY: 唯一 owner 保持 header；只有读取未被修改的原 Grant 元数据。
    let installed = unsafe { &owner.header.as_ref().permit };
    assert_eq!(installed.epoch(), epoch);
    assert_eq!(installed.amount(), amount);
    assert_eq!(installed.class(), ResourceClass::Runtime);
    assert_eq!(charged.classes[ResourceClass::Runtime as usize].slots, 1);
    assert_eq!(owner.poll_if_ready(), PollState::Pending);
    ready.set(true);
    external
        .borrow()
        .as_ref()
        .ok_or(NumericReject::ConstructionFailure)?
        .wake_by_ref();
    assert_eq!(owner.poll_if_ready(), PollState::Complete);
    assert_eq!(drops.get(), 1);
    drop(owner);
    assert_eq!(ROOT.snapshot().rust_bytes, amount.rust_bytes);
    let late = external
        .borrow_mut()
        .take()
        .ok_or(NumericReject::ConstructionFailure)?;
    let last = late.clone();
    late.wake();
    assert_eq!(
        ROOT.snapshot().classes[ResourceClass::Runtime as usize].slots,
        1
    );
    drop(last);
    assert_eq!(ROOT.snapshot().rust_bytes, 0);
    assert_eq!(
        ROOT.snapshot().classes[ResourceClass::Runtime as usize].slots,
        0
    );
    let report = audit.finish();
    audit_captures(&report)?;
    assert_eq!(
        report
            .events()
            .iter()
            .filter(|event| event.kind == 1 && event.size == amount.rust_bytes)
            .count(),
        1
    );
    let last_free = report
        .events()
        .last()
        .ok_or(NumericReject::ConstructionFailure)?;
    assert_eq!(
        (last_free.kind, last_free.size, last_free.held_rust),
        (2, amount.rust_bytes, amount.rust_bytes)
    );
    Ok(())
}

#[test]
fn reserved_insufficient_rust_wrong_class_or_missing_slot_never_allocates_owner()
-> Result<(), NumericReject> {
    let _serial = SERIAL
        .lock()
        .map_err(|_| NumericReject::ConstructionFailure)?;
    let exact = exact_amount()?;
    for (class, amount, reject) in [
        (
            ResourceClass::Runtime,
            Amount {
                rust_bytes: exact.rust_bytes - 1,
                ..exact
            },
            NumericReject::RustBudgetExceeded,
        ),
        (
            ResourceClass::Control,
            exact,
            NumericReject::UnsupportedEnvelope,
        ),
        (
            ResourceClass::Runtime,
            Amount { slots: 0, ..exact },
            NumericReject::RustBudgetExceeded,
        ),
        (
            ResourceClass::Runtime,
            Amount {
                rust_bytes: 0,
                c_bytes: exact.rust_bytes,
                slots: 1,
            },
            NumericReject::RustBudgetExceeded,
        ),
    ] {
        let (external, ready, drops, expected) = (
            RefCell::new(None),
            Cell::new(false),
            Cell::new(0),
            Cell::new(None),
        );
        let audit = AuditScope::start(&ROOT);
        audit.inspect_secret_free(17);
        let future = capture(&external, &ready, &drops, &expected)?;
        let permit = reserve(amount, class)?;
        expected.set(Some((class, amount)));
        assert_eq!(
            InlineSessionOwner::try_install_reserved(future, permit, bound()?).err(),
            Some(reject)
        );
        assert_eq!(drops.get(), 1);
        assert!(external.borrow().is_none());
        assert_eq!(
            (ROOT.snapshot().rust_bytes, ROOT.snapshot().c_bytes),
            (0, 0)
        );
        let report = audit.finish();
        audit_captures(&report)?;
        // 只有事先带额的 secret/source header/source string 三笔 allocation。
        assert_eq!(
            report
                .events()
                .iter()
                .filter(|event| event.kind == 1)
                .count(),
            3
        );
    }
    Ok(())
}

#[test]
fn reserved_null_allocation_keeps_original_grant_during_future_cleanup() -> Result<(), NumericReject>
{
    let _serial = SERIAL
        .lock()
        .map_err(|_| NumericReject::ConstructionFailure)?;
    let (external, ready, drops, expected) = (
        RefCell::new(None),
        Cell::new(false),
        Cell::new(0),
        Cell::new(None),
    );
    let audit = AuditScope::start(&ROOT);
    audit.inspect_secret_free(17);
    let future = capture(&external, &ready, &drops, &expected)?;
    let amount = exact_amount()?;
    let permit = reserve(amount, ResourceClass::Runtime)?;
    expected.set(Some((permit.class(), permit.amount())));
    audit.fail_next_size(amount.rust_bytes);
    assert_eq!(
        InlineSessionOwner::try_install_reserved(future, permit, bound()?).err(),
        Some(NumericReject::AllocationFailure)
    );
    assert_eq!(drops.get(), 1);
    assert!(external.borrow().is_none());
    assert_eq!(ROOT.snapshot().rust_bytes, 0);
    let report = audit.finish();
    audit_captures(&report)?;
    assert_eq!(
        report
            .events()
            .iter()
            .filter(|event| event.kind == 4 && event.size == amount.rust_bytes)
            .count(),
        1
    );
    assert_eq!(
        report
            .events()
            .iter()
            .filter(|event| event.kind == 1)
            .count(),
        3
    );
    Ok(())
}

#[test]
fn reserved_numeric_and_holder_construction_reject_before_allocator_with_guarded_future()
-> Result<(), NumericReject> {
    use crate::session_owner::operation::{Opcode, OperationBound};
    let _serial = SERIAL
        .lock()
        .map_err(|_| NumericReject::ConstructionFailure)?;
    for overflow_holders in [false, true] {
        let (external, ready, drops, expected) = (
            RefCell::new(None),
            Cell::new(false),
            Cell::new(0),
            Cell::new(None),
        );
        let audit = AuditScope::start(&ROOT);
        audit.inspect_secret_free(17);
        let future = capture(&external, &ready, &drops, &expected)?;
        let amount = exact_amount()?;
        let permit = reserve(amount, ResourceClass::Runtime)?;
        expected.set(Some((permit.class(), permit.amount())));
        let pending = PendingInstall::new(future, permit);
        let rejected = if overflow_holders {
            unsafe { HolderBound::from_source_proof(usize::MAX) }.err()
        } else {
            OperationBound::sum(
                Opcode::AttemptClone,
                &[InlineLayout::actual::<Captured<'_>>()?.backing],
                usize::MAX,
            )
            .err()
        };
        assert_eq!(
            rejected,
            Some(if overflow_holders {
                NumericReject::RefcountOverflow
            } else {
                NumericReject::Overflow
            })
        );
        drop(pending);
        assert_eq!(drops.get(), 1);
        assert_eq!(ROOT.snapshot().rust_bytes, 0);
        let report = audit.finish();
        audit_captures(&report)?;
        assert_eq!(
            report
                .events()
                .iter()
                .filter(|event| event.kind == 1)
                .count(),
            3
        );
    }
    // 真实 Header extend 对超出 isize 地址范围的 Layout typed 拒绝，不向 allocator 发送。
    let audit = AuditScope::start(&ROOT);
    let huge = Layout::from_size_align(isize::MAX as usize, 1)
        .map_err(|_| NumericReject::InvalidLayout)?;
    assert_eq!(
        crate::selected_p1::model::layout::extend(Layout::new::<super::Header>(), huge).err(),
        Some(NumericReject::InvalidLayout)
    );
    assert!(audit.finish().events().is_empty());
    Ok(())
}

#[test]
fn reserved_cancel_retains_grant_until_last_waker_and_cancel_root() -> Result<(), NumericReject> {
    let _serial = SERIAL
        .lock()
        .map_err(|_| NumericReject::ConstructionFailure)?;
    let (external, ready, drops, expected) = (
        RefCell::new(None),
        Cell::new(false),
        Cell::new(0),
        Cell::new(None),
    );
    let audit = AuditScope::start(&ROOT);
    audit.inspect_secret_free(17);
    let future = capture(&external, &ready, &drops, &expected)?;
    let amount = exact_amount()?;
    let permit = reserve(amount, ResourceClass::Runtime)?;
    expected.set(Some((permit.class(), permit.amount())));
    let mut owner = InlineSessionOwner::try_install_reserved(future, permit, bound()?)?;
    let cancel = owner.cancel_handle()?;
    assert_eq!(owner.poll_if_ready(), PollState::Pending);
    cancel.cancel();
    assert_eq!(owner.poll_if_ready(), PollState::Cancelled);
    assert_eq!(drops.get(), 1);
    drop(owner);
    let late = external
        .borrow_mut()
        .take()
        .ok_or(NumericReject::ConstructionFailure)?;
    late.wake();
    assert_eq!(ROOT.snapshot().rust_bytes, amount.rust_bytes);
    cancel.cancel();
    drop(cancel);
    assert_eq!(ROOT.snapshot().rust_bytes, 0);
    let report = audit.finish();
    audit_captures(&report)?;
    let last_free = report
        .events()
        .last()
        .ok_or(NumericReject::ConstructionFailure)?;
    assert_eq!(
        (last_free.kind, last_free.size, last_free.held_rust),
        (2, amount.rust_bytes, amount.rust_bytes)
    );
    Ok(())
}
