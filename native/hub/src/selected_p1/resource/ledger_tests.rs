use std::sync::Barrier;

use super::{
    Admission, Amount, BYTE_LIMIT, CLASS_COUNT, PROCESS_LEDGER, ProcessLedger, ReservationReject,
    ResourceClass, SOURCE_COUNT, SourceToken,
};
use crate::audit::AuditScope;
use crate::selected_p1::model::layout::NumericReject;

fn admission(epoch: u64) -> Admission {
    Admission {
        epoch,
        source: None,
    }
}
fn amount(rust_bytes: usize, c_bytes: usize, slots: u32) -> Amount {
    Amount {
        rust_bytes,
        c_bytes,
        slots,
    }
}

#[test]
fn real_byte_class_and_slot_consumption_are_atomic() -> Result<(), NumericReject> {
    assert_eq!(BYTE_LIMIT, 403_701_760);
    assert_eq!(PROCESS_LEDGER.snapshot().rust_bytes, 0);
    assert_eq!(
        ProcessLedger::new(usize::MAX, [u32::MAX; CLASS_COUNT]).byte_limit,
        BYTE_LIMIT
    );
    let root = ProcessLedger::new(96, [2; CLASS_COUNT]);
    let audit = AuditScope::start(&root);
    let first = root.try_reserve(amount(32, 64, 2), ResourceClass::Helper, admission(1))?;
    assert_eq!(first.epoch(), 1);
    assert_eq!(first.class(), ResourceClass::Helper);
    assert_eq!(root.snapshot().rust_bytes, 32);
    assert_eq!(root.snapshot().c_bytes, 64);
    assert_eq!(
        root.try_reserve(amount(1, 0, 0), ResourceClass::Raw, admission(1))
            .err(),
        Some(NumericReject::RustBudgetExceeded)
    );
    assert_eq!(
        root.try_reserve(amount(0, 0, 1), ResourceClass::Helper, admission(1))
            .err(),
        Some(NumericReject::RustBudgetExceeded)
    );
    let report = audit.finish();
    assert!(report.events().is_empty());
    drop(first);
    assert_eq!(root.snapshot().rust_bytes + root.snapshot().c_bytes, 0);
    Ok(())
}

#[test]
fn split_tighten_and_move_preserve_actual_total_and_class() -> Result<(), NumericReject> {
    let root = ProcessLedger::new(96, [2; CLASS_COUNT]);
    let mut grant = root.try_reserve(amount(32, 64, 2), ResourceClass::Helper, admission(1))?;
    let transferred = grant.split(amount(16, 32, 1))?;
    assert_eq!(root.snapshot().rust_bytes + root.snapshot().c_bytes, 96);
    grant.tighten_unused(amount(8, 16, 0))?;
    assert_eq!(root.snapshot().rust_bytes + root.snapshot().c_bytes, 72);
    assert_eq!(
        root.snapshot().classes[ResourceClass::Helper as usize].slots,
        1
    );
    assert!(grant.split(amount(9, 0, 0)).is_err());
    assert_eq!(grant.amount(), amount(8, 16, 0));
    drop(grant);
    assert_eq!(root.snapshot().rust_bytes + root.snapshot().c_bytes, 48);
    drop(transferred);
    assert_eq!(root.snapshot().rust_bytes + root.snapshot().c_bytes, 0);
    Ok(())
}

#[test]
fn epoch_invalidation_and_source_aba_never_clear_old_owners() -> Result<(), NumericReject> {
    let root = ProcessLedger::new(100, [2; CLASS_COUNT]);
    let old = root.open_source(1, 0)?;
    let resource = root.try_reserve(
        amount(70, 0, 1),
        ResourceClass::Reply,
        Admission {
            epoch: 1,
            source: Some(old),
        },
    )?;
    root.invalidate_source(old)?;
    let new = root.open_source(1, 0)?;
    assert_ne!(old.generation, new.generation);
    assert_eq!(
        root.try_reserve(
            amount(1, 0, 1),
            ResourceClass::Reply,
            Admission {
                epoch: 1,
                source: Some(old)
            }
        )
        .err(),
        Some(NumericReject::SourceMismatch)
    );
    let peer = root.try_reserve(
        amount(30, 0, 1),
        ResourceClass::Reply,
        Admission {
            epoch: 1,
            source: Some(new),
        },
    )?;
    root.advance_epoch(1)?;
    assert_eq!(root.snapshot().rust_bytes, 100);
    assert_eq!(
        root.try_reserve(amount(0, 0, 0), ResourceClass::Control, admission(1))
            .err(),
        Some(NumericReject::EpochMismatch)
    );
    assert_eq!(
        root.try_reserve(amount(1, 0, 0), ResourceClass::Control, admission(2))
            .err(),
        Some(NumericReject::RustBudgetExceeded)
    );
    drop(resource);
    assert_eq!(root.snapshot().rust_bytes, 30);
    drop(peer);
    assert_eq!(root.snapshot().rust_bytes, 0);
    let active = root.open_source(2, 0)?;
    assert!(active.generation > new.generation);
    assert_eq!(
        root.open_source(2, SOURCE_COUNT as u32),
        Err(NumericReject::InvalidSource)
    );
    Ok(())
}

#[test]
fn overflow_and_invalid_stamps_have_no_side_effects() -> Result<(), NumericReject> {
    let root = ProcessLedger::new(100, [u32::MAX; CLASS_COUNT]);
    assert_eq!(
        root.try_reserve(amount(usize::MAX, 1, 0), ResourceClass::Raw, admission(1))
            .err(),
        Some(NumericReject::Overflow)
    );
    let held = root.try_reserve(amount(1, 0, u32::MAX), ResourceClass::Raw, admission(1))?;
    assert_eq!(
        root.try_reserve(amount(0, 0, 1), ResourceClass::Raw, admission(1))
            .err(),
        Some(NumericReject::Overflow)
    );
    let before = root.snapshot();
    assert_eq!(root.advance_epoch(2), Err(NumericReject::EpochMismatch));
    assert_eq!(root.snapshot(), before);
    drop(held);
    for class in [
        ResourceClass::Persistent,
        ResourceClass::Helper,
        ResourceClass::Raw,
        ResourceClass::Decode,
        ResourceClass::Control,
        ResourceClass::Secret,
        ResourceClass::Source,
        ResourceClass::Runtime,
        ResourceClass::Reply,
    ] {
        let grant = root.try_reserve(amount(1, 0, 1), class, admission(1))?;
        assert_eq!(root.snapshot().classes[class as usize].rust_bytes, 1);
        drop(grant);
    }
    let mut state = root.lock();
    state.epoch = u64::MAX;
    drop(state);
    assert_eq!(root.advance_epoch(u64::MAX), Err(NumericReject::Overflow));
    let mut state = root.lock();
    state.sources[0].generation = u64::MAX;
    drop(state);
    assert_eq!(root.open_source(u64::MAX, 0), Err(NumericReject::Overflow));
    Ok(())
}

#[test]
fn simultaneous_admissions_cannot_overbook_bytes_or_slots() {
    let root = ProcessLedger::new(200, [2; CLASS_COUNT]);
    let barrier = Barrier::new(9);
    std::thread::scope(|scope| {
        let mut threads = Vec::new();
        for _ in 0..8 {
            let root = &root;
            let barrier = &barrier;
            threads.push(scope.spawn(move || {
                barrier.wait();
                let grant =
                    root.try_reserve(amount(100, 0, 1), ResourceClass::Control, admission(1));
                barrier.wait();
                barrier.wait();
                grant.is_ok()
            }));
        }
        barrier.wait();
        barrier.wait();
        assert_eq!(root.snapshot().rust_bytes, 200);
        assert_eq!(
            root.snapshot().classes[ResourceClass::Control as usize].slots,
            2
        );
        barrier.wait();
        let successes = threads
            .into_iter()
            .map(|thread| match thread.join() {
                Ok(admitted) => usize::from(admitted),
                Err(_) => panic!("1"),
            })
            .sum::<usize>();
        assert_eq!(successes, 2);
    });
    assert_eq!(root.snapshot().rust_bytes, 0);
}

#[test]
fn detailed_quota_rejects_preserve_atomic_values_and_byte_priority() -> Result<(), NumericReject> {
    let root = ProcessLedger::new(100, [2; CLASS_COUNT]);
    let held = root.try_reserve(amount(40, 60, 1), ResourceClass::Raw, admission(1))?;
    let before = root.snapshot();
    let audit = AuditScope::start(&root);
    for (requested, expected) in [
        (
            amount(1, 0, 0),
            ReservationReject::ByteLimit {
                requested: 1,
                current: 100,
                limit: 100,
            },
        ),
        (
            amount(0, 0, 2),
            ReservationReject::ClassSlots {
                class: ResourceClass::Raw,
                requested: 2,
                current: 1,
                limit: 2,
            },
        ),
        (
            amount(1, 0, 2),
            ReservationReject::ByteLimit {
                requested: 1,
                current: 100,
                limit: 100,
            },
        ),
    ] {
        assert_eq!(
            root.try_reserve_detailed(requested, ResourceClass::Raw, admission(1))
                .err(),
            Some(expected)
        );
        assert_eq!(root.snapshot(), before);
        assert_eq!(
            root.try_reserve(requested, ResourceClass::Raw, admission(1))
                .err(),
            Some(NumericReject::RustBudgetExceeded)
        );
        assert_eq!(root.snapshot(), before);
    }
    assert!(audit.finish().events().is_empty());
    drop(held);
    assert_eq!(root.snapshot().rust_bytes + root.snapshot().c_bytes, 0);
    Ok(())
}

#[test]
fn detailed_numeric_errors_remain_numeric_without_quota_classification() -> Result<(), NumericReject>
{
    let root = ProcessLedger::new(100, [u32::MAX; CLASS_COUNT]);
    let token = root.open_source(1, 0)?;
    let held = root.try_reserve(amount(100, 0, u32::MAX), ResourceClass::Raw, admission(1))?;
    let before = root.snapshot();
    let audit = AuditScope::start(&root);
    for (requested, admission, expected) in [
        (amount(1, 0, 0), admission(2), NumericReject::EpochMismatch),
        (
            amount(1, 0, 0),
            Admission {
                epoch: 1,
                source: Some(SourceToken {
                    generation: token.generation + 1,
                    ..token
                }),
            },
            NumericReject::SourceMismatch,
        ),
        (
            amount(1, 0, 0),
            Admission {
                epoch: 1,
                source: Some(SourceToken {
                    index: SOURCE_COUNT as u32,
                    ..token
                }),
            },
            NumericReject::InvalidSource,
        ),
        (
            amount(usize::MAX, 1, 0),
            admission(2),
            NumericReject::Overflow,
        ),
        (
            amount(usize::MAX, 0, 0),
            admission(1),
            NumericReject::Overflow,
        ),
        (amount(1, 0, 1), admission(1), NumericReject::Overflow),
    ] {
        assert_eq!(
            root.try_reserve_detailed(requested, ResourceClass::Raw, admission)
                .err(),
            Some(ReservationReject::Numeric(expected))
        );
        assert_eq!(
            root.try_reserve(requested, ResourceClass::Raw, admission)
                .err(),
            Some(expected)
        );
        assert_eq!(root.snapshot(), before);
    }
    assert!(audit.finish().events().is_empty());
    drop(held);
    assert_eq!(root.snapshot().rust_bytes, 0);
    Ok(())
}

#[test]
fn detailed_rejection_records_survive_concurrent_holder_release() -> Result<(), NumericReject> {
    let root = ProcessLedger::new(100, [1; CLASS_COUNT]);
    let held = root.try_reserve(amount(30, 50, 1), ResourceClass::Helper, admission(1))?;
    let release = Barrier::new(2);
    let (byte_rejection, slot_rejection) = std::thread::scope(|scope| {
        let release_ref = &release;
        let holder = scope.spawn(move || {
            release_ref.wait();
            drop(held);
        });
        let byte_rejection = root
            .try_reserve_detailed(amount(21, 0, 0), ResourceClass::Helper, admission(1))
            .err();
        let slot_rejection = root
            .try_reserve_detailed(amount(0, 0, 1), ResourceClass::Helper, admission(1))
            .err();
        release.wait();
        assert!(holder.join().is_ok());
        (byte_rejection, slot_rejection)
    });
    assert_eq!(root.snapshot().rust_bytes + root.snapshot().c_bytes, 0);
    assert_eq!(
        root.snapshot().classes[ResourceClass::Helper as usize].slots,
        0
    );
    assert_eq!(
        byte_rejection,
        Some(ReservationReject::ByteLimit {
            requested: 21,
            current: 80,
            limit: 100,
        })
    );
    assert_eq!(
        slot_rejection,
        Some(ReservationReject::ClassSlots {
            class: ResourceClass::Helper,
            requested: 1,
            current: 1,
            limit: 1,
        })
    );
    let granted = root
        .try_reserve_detailed(amount(21, 0, 1), ResourceClass::Helper, admission(1))
        .map_err(ReservationReject::into_numeric)?;
    assert_eq!(root.snapshot().rust_bytes, 21);
    assert_eq!(granted.amount(), amount(21, 0, 1));
    drop(granted);
    assert_eq!(root.snapshot().rust_bytes, 0);
    Ok(())
}
