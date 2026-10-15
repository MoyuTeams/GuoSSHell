use std::sync::Barrier;

use super::{CopyValidity, LockedSecretString};
use crate::audit::AuditScope;
use crate::selected_p1::model::layout::NumericReject;
use crate::selected_p1::resource::ledger::{
    Admission, Amount, BYTE_LIMIT, CLASS_COUNT, Grant, ProcessLedger, ResourceClass,
};

fn reserve(root: &ProcessLedger, n: usize) -> Result<Grant<'_>, NumericReject> {
    root.try_reserve(
        Amount {
            rust_bytes: n,
            c_bytes: 0,
            slots: 1,
        },
        ResourceClass::Secret,
        Admission {
            epoch: 1,
            source: None,
        },
    )
}

#[test]
fn actual_secrecy_accepts_empty_unicode_emoji_and_nul_without_shrink() -> Result<(), NumericReject>
{
    for input in ["", "a", "\u{4e2d}\u{1f642}", "a\0b", "\0"] {
        let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
        let validity = CopyValidity::default();
        let audit = AuditScope::start(&root);
        audit.inspect_secret_free(input.len());
        let secret = LockedSecretString::try_copy(input, reserve(&root, input.len())?, &validity)?;
        assert!(secret.expose_secret() == input);
        assert_eq!(secret.admitted().rust_bytes, input.len());
        assert_eq!(
            root.snapshot().classes[ResourceClass::Secret as usize].slots,
            1
        );
        drop(secret);
        let report = audit.finish();
        if input.is_empty() {
            assert!(report.events().is_empty());
        } else {
            assert!(report.symmetric_single_backing());
            assert!(report.events()[1].zero_before_free);
            assert_eq!(report.events()[1].held_rust, input.len());
        }
        assert_eq!(root.snapshot().rust_bytes, 0);
        assert_eq!(
            root.snapshot().classes[ResourceClass::Secret as usize].slots,
            0
        );
    }
    Ok(())
}

#[test]
fn rejection_and_null_alloc_make_no_owned_plaintext() -> Result<(), NumericReject> {
    let input = "\u{1f642}\0";
    let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
    let validity = CopyValidity::default();
    let audit = AuditScope::start(&root);
    assert_eq!(
        LockedSecretString::try_copy(input, reserve(&root, input.len() - 1)?, &validity).err(),
        Some(NumericReject::RustBudgetExceeded)
    );
    validity.invalidate();
    assert_eq!(
        LockedSecretString::try_copy(input, reserve(&root, input.len())?, &validity).err(),
        Some(NumericReject::Cancelled)
    );
    assert!(audit.finish().events().is_empty());
    let validity = CopyValidity::default();
    let audit = AuditScope::start(&root);
    audit.fail_next_size(input.len());
    assert_eq!(
        LockedSecretString::try_copy(input, reserve(&root, input.len())?, &validity).err(),
        Some(NumericReject::AllocationFailure)
    );
    let report = audit.finish();
    assert_eq!(report.events().len(), 1);
    assert_eq!(report.events()[0].kind, 4);
    assert_eq!(report.events()[0].held_rust, input.len());
    assert_eq!(root.snapshot().rust_bytes, 0);
    Ok(())
}

#[test]
fn concurrent_cancel_only_invalidates_and_copier_wipes_full_capacity() -> Result<(), NumericReject>
{
    let input = "x".repeat(3073);
    let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
    let validity = CopyValidity::default();
    let barrier = Barrier::new(2);
    std::thread::scope(|scope| -> Result<(), NumericReject> {
        let canceller = scope.spawn(|| {
            barrier.wait();
            validity.invalidate();
            assert_eq!(root.snapshot().rust_bytes, input.len());
            barrier.wait();
        });
        let audit = AuditScope::start(&root);
        audit.inspect_secret_free(input.len());
        let mut copied = 0;
        let result = LockedSecretString::copy_checked(
            &input,
            reserve(&root, input.len())?,
            &validity,
            |n| {
                copied = n;
                if n == 1024 {
                    barrier.wait();
                    barrier.wait();
                    assert!(!validity.is_valid());
                    assert_eq!(root.snapshot().rust_bytes, input.len());
                }
                Ok(())
            },
        );
        assert_eq!(result.err(), Some(NumericReject::Cancelled));
        assert_eq!(copied, input.len());
        let report = audit.finish();
        assert!(report.symmetric_single_backing());
        assert!(report.events()[1].zero_before_free);
        assert_eq!(report.events()[1].held_rust, input.len());
        assert!(canceller.join().is_ok());
        Ok(())
    })?;
    assert_eq!(root.snapshot().rust_bytes, 0);
    Ok(())
}

#[test]
fn construction_failure_wipes_written_and_unwritten_capacity_then_frees()
-> Result<(), NumericReject> {
    let input = "y".repeat(3073);
    let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
    let validity = CopyValidity::default();
    let audit = AuditScope::start(&root);
    audit.inspect_secret_free(input.len());
    let result =
        LockedSecretString::copy_checked(&input, reserve(&root, input.len())?, &validity, |_| {
            Err(NumericReject::ConstructionFailure)
        });
    assert_eq!(result.err(), Some(NumericReject::ConstructionFailure));
    let report = audit.finish();
    assert!(report.symmetric_single_backing());
    assert!(report.events()[1].zero_before_free);
    assert_eq!(report.events()[1].size, input.len());
    assert_eq!(report.events()[1].held_rust, input.len());
    assert_eq!(root.snapshot().rust_bytes, 0);
    Ok(())
}

#[test]
fn real_authplan_handoff_keeps_secret_grant_until_operation_drop() -> Result<(), NumericReject> {
    use rshell_m0::rshell_core::{AuthenticationKind, ConnectionProfile, TransportKind};
    use rshell_m0::rshell_session::AuthPlan;

    let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
    let validity = CopyValidity::default();
    let mut profile = ConnectionProfile::new("1", "1");
    profile.transport = TransportKind::NativeSsh;
    profile.authentication = AuthenticationKind::Password;
    let input = "\u{1f642}\0";
    let host_grant = root.try_reserve(
        Amount {
            rust_bytes: profile.host.len(),
            c_bytes: 0,
            slots: 0,
        },
        ResourceClass::Runtime,
        Admission {
            epoch: 1,
            source: None,
        },
    )?;
    let audit = AuditScope::start(&root);
    audit.inspect_secret_free(input.len());
    let secret = LockedSecretString::try_copy(input, reserve(&root, input.len())?, &validity)?;
    // 这里只证 Password 变体移动及实际 Drop。完整生产 auth/task/profile envelope 仍归 W1-S。
    // SAFETY: 本同步 operation 不连接、不 clone、不启动任务；真实 AuthPlan 不逃逸此 wrapper。
    let operation = unsafe {
        secret.into_operation(|secret| (AuthPlan::from_secret(&profile, Some(secret)), host_grant))
    };
    assert!(matches!(
        operation.operation().0,
        Ok(AuthPlan::Password { .. })
    ));
    assert_eq!(operation.admitted().rust_bytes, input.len());
    root.advance_epoch(1)?;
    assert_eq!(root.snapshot().rust_bytes, input.len() + profile.host.len());
    drop(operation);
    let report = audit.finish();
    let secret_free = report
        .events()
        .iter()
        .filter(|event| event.kind == 2 && event.size == input.len())
        .collect::<Vec<_>>();
    assert_eq!(secret_free.len(), 1);
    assert!(secret_free[0].zero_before_free);
    assert_eq!(secret_free[0].held_rust, input.len() + profile.host.len());
    assert_eq!(root.snapshot().rust_bytes, 0);
    Ok(())
}

#[test]
fn handoff_constructor_unwind_still_wipes_before_grant_returns() -> Result<(), NumericReject> {
    let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
    let validity = CopyValidity::default();
    let secret = LockedSecretString::try_copy("z", reserve(&root, 1)?, &validity)?;
    let result = std::panic::catch_unwind(|| {
        // SAFETY: consume 不产生任何逃逸 root；unwind 正常析构其 SecretString 参数。
        let _ = unsafe { secret.into_operation::<()>(|_secret| std::panic::panic_any(4u32)) };
    });
    assert!(result.is_err());
    assert_eq!(root.snapshot().rust_bytes, 0);
    Ok(())
}
