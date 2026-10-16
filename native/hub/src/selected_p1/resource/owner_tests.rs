use std::alloc::Layout;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Barrier;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::{MAX_REFCOUNT, OwnerLayout, RcMemberLayout, SharedOwner};
use crate::audit::AuditScope;
use crate::selected_p1::model::layout::NumericReject;
use crate::selected_p1::resource::ledger::{
    Admission, Amount, BYTE_LIMIT, CLASS_COUNT, Grant, ProcessLedger, ResourceClass,
};

fn reserve(root: &ProcessLedger, bytes: usize) -> Result<Grant<'_>, NumericReject> {
    root.try_reserve(
        Amount {
            rust_bytes: bytes,
            c_bytes: 0,
            slots: 1,
        },
        ResourceClass::Source,
        Admission {
            epoch: 1,
            source: None,
        },
    )
}

#[test]
fn final_header_uses_original_alignment_and_retains_all_inner_backing() -> Result<(), NumericReject>
{
    #[repr(align(64))]
    struct Body(String);
    let model = OwnerLayout::actual::<Body>(19)?;
    assert_eq!(model.backing.align(), 64);
    let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
    let bytes = model.required_rust(13)?;
    let audit = AuditScope::start(&root);
    let owner = SharedOwner::try_init(reserve(&root, bytes)?, 19, 13, |payload| {
        payload.fill(3);
        Ok(Body(String::from("1234567890123")))
    })?;
    assert_eq!(owner.value().0.len(), 13);
    assert_eq!(owner.payload(), &[3; 19]);
    assert_eq!(owner.admitted().rust_bytes, bytes);
    let raw = owner.into_raw();
    assert_eq!(root.snapshot().rust_bytes, bytes);
    // SAFETY: 原同 Body strong reference 唯一接管一次。
    drop(unsafe { SharedOwner::<Body>::from_raw(raw) });
    let report = audit.finish();
    assert_eq!(report.events().len(), 4);
    assert_eq!(report.events()[2].size, 13);
    assert_eq!(report.events()[3].size, model.backing.size());
    assert_eq!(report.events()[3].align, 64);
    assert!(report.events().iter().all(|event| event.held_rust == bytes));
    assert_eq!(root.snapshot().rust_bytes, 0);
    Ok(())
}

#[test]
fn grant_shortfall_null_allocator_and_init_failure_do_not_publish_an_owner()
-> Result<(), NumericReject> {
    let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
    let model = OwnerLayout::actual::<u64>(8)?;
    let called = Cell::new(false);
    let audit = AuditScope::start(&root);
    let insufficient =
        SharedOwner::try_init(reserve(&root, model.backing.size() - 1)?, 8, 0, |_| {
            called.set(true);
            Ok(1u64)
        });
    assert_eq!(insufficient.err(), Some(NumericReject::RustBudgetExceeded));
    assert!(!called.get());
    assert!(audit.finish().events().is_empty());
    let audit = AuditScope::start(&root);
    audit.fail_next_size(model.backing.size());
    let failed = SharedOwner::try_init(reserve(&root, model.backing.size())?, 8, 0, |_| {
        called.set(true);
        Ok(1u64)
    });
    assert_eq!(failed.err(), Some(NumericReject::AllocationFailure));
    assert!(!called.get());
    let report = audit.finish();
    assert_eq!(report.events().len(), 1);
    assert_eq!(report.events()[0].kind, 4);
    assert_eq!(report.events()[0].held_rust, model.backing.size());
    assert_eq!(root.snapshot().rust_bytes, 0);
    let audit = AuditScope::start(&root);
    let failed = SharedOwner::<u64>::try_init(reserve(&root, model.backing.size())?, 8, 0, |_| {
        Err(NumericReject::ConstructionFailure)
    });
    assert_eq!(failed.err(), Some(NumericReject::ConstructionFailure));
    assert!(audit.finish().symmetric_single_backing());
    assert_eq!(root.snapshot().rust_bytes, 0);
    Ok(())
}

#[test]
fn construction_and_inner_drop_unwind_still_free_backing_before_return() -> Result<(), NumericReject>
{
    let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
    let model = OwnerLayout::actual::<u64>(0)?;
    let grant = reserve(&root, model.backing.size())?;
    let result = std::panic::catch_unwind(|| {
        let _ = SharedOwner::<u64>::try_init(grant, 0, 0, |_| std::panic::panic_any(2u32));
    });
    assert!(result.is_err());
    assert_eq!(root.snapshot().rust_bytes, 0);
    struct Panics;
    impl Drop for Panics {
        fn drop(&mut self) {
            std::panic::panic_any(3u32);
        }
    }
    let model = OwnerLayout::actual::<Panics>(0)?;
    let owner = SharedOwner::try_init(reserve(&root, model.backing.size())?, 0, 0, |_| Ok(Panics))?;
    assert!(std::panic::catch_unwind(|| drop(owner)).is_err());
    assert_eq!(root.snapshot().rust_bytes, 0);
    Ok(())
}

#[test]
fn refcount_overflow_is_fallible_without_wrap_leak_or_unwind() -> Result<(), NumericReject> {
    let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
    let model = OwnerLayout::actual::<u64>(0)?;
    let owner = SharedOwner::try_init(reserve(&root, model.backing.size())?, 0, 0, |_| Ok(1u64))?;
    // SAFETY: 本测试唯一 holder；只用于验证边界，drop 前恢复真实计数。
    let refs = unsafe { &owner.header.as_ref().refs };
    refs.store(MAX_REFCOUNT, Ordering::Relaxed);
    assert_eq!(
        owner.try_clone().err(),
        Some(NumericReject::RefcountOverflow)
    );
    assert_eq!(refs.load(Ordering::Relaxed), MAX_REFCOUNT);
    refs.store(1, Ordering::Relaxed);
    drop(owner);
    assert_eq!(root.snapshot().rust_bytes, 0);
    Ok(())
}

#[test]
fn actual_send_sync_bounds_and_last_two_ref_competition() -> Result<(), NumericReject> {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<SharedOwner<'static, AtomicUsize>>();
    // 编译期反证：非 Sync/非 Send 的真实 T 不会被 raw-pointer impl 藏掉。
    trait AmbiguousIfSend<A> {
        fn marker() {}
    }
    impl<T: ?Sized> AmbiguousIfSend<()> for T {}
    struct SendMarker;
    impl<T: ?Sized + Send> AmbiguousIfSend<SendMarker> for T {}
    let _ = <SharedOwner<'static, Cell<u32>> as AmbiguousIfSend<_>>::marker;
    let _ = <SharedOwner<'static, Rc<RefCell<u32>>> as AmbiguousIfSend<_>>::marker;
    let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
    let drops = AtomicUsize::new(0);
    struct Body<'a> {
        drops: &'a AtomicUsize,
        root: &'a ProcessLedger,
        bytes: usize,
    }
    impl Drop for Body<'_> {
        fn drop(&mut self) {
            assert_eq!(self.root.snapshot().rust_bytes, self.bytes);
            self.drops.fetch_add(1, Ordering::Relaxed);
        }
    }
    for round in 0..512 {
        let model = OwnerLayout::actual::<Body<'_>>(0)?;
        let owner = SharedOwner::try_init(reserve(&root, model.backing.size())?, 0, 0, |_| {
            Ok(Body {
                drops: &drops,
                root: &root,
                bytes: model.backing.size(),
            })
        })?;
        let other = owner.try_clone()?;
        let barrier = Barrier::new(2);
        std::thread::scope(|scope| {
            let barrier_ref = &barrier;
            let first = scope.spawn(move || {
                barrier_ref.wait();
                drop(owner);
            });
            let second = scope.spawn(move || {
                barrier_ref.wait();
                drop(other);
            });
            assert!(first.join().is_ok());
            assert!(second.join().is_ok());
        });
        assert_eq!(drops.load(Ordering::Relaxed), round + 1);
        assert_eq!(root.snapshot().rust_bytes, 0);
    }
    Ok(())
}

#[test]
fn header_layout_is_actual_not_a_research_prefix_constant() -> Result<(), NumericReject> {
    let permit = Layout::new::<Grant<'_>>();
    let header = Layout::new::<RcMemberLayout<Grant<'_>>>();
    assert!(header.size() >= permit.size() + Layout::new::<AtomicUsize>().size());
    println!("201:{}:{}", permit.size(), permit.align());
    println!("202:{}:{}", header.size(), header.align());
    let secret = Layout::new::<secrecy::SecretString>();
    let locked = Layout::new::<super::super::secret::LockedSecretString<'_>>();
    let model = OwnerLayout::actual::<u64>(17)?;
    println!("203:{}:{}", secret.size(), secret.align());
    println!("204:{}:{}", locked.size(), locked.align());
    println!(
        "205:{}:{}:{}:{}",
        model.backing.size(),
        model.backing.align(),
        model.body_offset,
        model.payload_offset
    );
    Ok(())
}
