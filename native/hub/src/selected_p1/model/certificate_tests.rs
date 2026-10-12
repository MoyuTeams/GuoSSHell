use super::{
    ActualLayoutProvider, BuildFingerprint, CertificateState, GNU_1981, ProofGate, TargetLayouts,
};
use crate::selected_p1::model::layout::{NumericReject, ProductLayouts};

struct UninstantiatedProduct;
impl ActualLayoutProvider for UninstantiatedProduct {
    fn product_layouts(&self) -> Result<ProductLayouts, ProofGate> {
        Err(ProofGate::FinalSchemas)
    }
}

#[test]
fn fingerprint_mismatch_is_not_a_quota_or_a_ready_product() -> Result<(), NumericReject> {
    for build in [
        BuildFingerprint {
            compiler: "0",
            ..GNU_1981
        },
        BuildFingerprint {
            target: "0",
            ..GNU_1981
        },
        BuildFingerprint {
            json_feature_bits: 2,
            ..GNU_1981
        },
        BuildFingerprint {
            cfg_bits: 2,
            ..GNU_1981
        },
        BuildFingerprint {
            serde_core_feature_bits: 0,
            ..GNU_1981
        },
        BuildFingerprint {
            zeroize: "0",
            ..GNU_1981
        },
        BuildFingerprint {
            layout_flags: "0",
            ..GNU_1981
        },
    ] {
        let target = TargetLayouts::foundation(build)?;
        assert_eq!(
            target.state(),
            CertificateState::Pending(ProofGate::BuildFingerprint)
        );
        assert_eq!(
            target.private_bounds(),
            Err(NumericReject::UnsupportedEnvelope)
        );
    }
    let target = TargetLayouts::foundation(GNU_1981)?;
    assert_eq!(target.state(), CertificateState::FoundationReady);
    let private = target.private_bounds()?;
    assert_eq!((private.content.size(), private.content.align()), (32, 8));
    assert_eq!(
        (private.content_pair.size(), private.flatten_entry.size()),
        (64, 64)
    );
    assert_eq!(
        (
            private.error_code.size(),
            private.error_impl_actual.size(),
            private.error_impl_upper.size()
        ),
        (24, 40, 40)
    );
    for gate in [
        ProofGate::DecodeClosure,
        ProofGate::SourceClosure,
        ProofGate::ControlClosure,
        ProofGate::AuthOperationRoot,
        ProofGate::ReplyVm,
    ] {
        assert_eq!(
            target.product_state(&UninstantiatedProduct, gate),
            CertificateState::Pending(ProofGate::FinalSchemas)
        );
    }
    Ok(())
}

#[test]
fn actual_target_numeric_layouts() -> Result<(), NumericReject> {
    let target = TargetLayouts::foundation(GNU_1981)?;
    let public = target.public;
    for (id, layout) in [
        (101, public.string),
        (102, public.path),
        (103, public.value),
        (104, public.key_code),
        (105, public.key_modifiers),
        (106, public.key_binding),
    ] {
        // 仅合成类编号/size/align；没有业务正文、地址、路径或秘密。
        println!("{id}:{}:{}", layout.size(), layout.align());
    }
    Ok(())
}

#[test]
fn real_syntax_and_io_error_boxes_use_verified_private_layout() -> Result<(), NumericReject> {
    use crate::audit::AuditScope;
    use crate::selected_p1::resource::ledger::{
        Admission, Amount, CLASS_COUNT, ProcessLedger, ResourceClass,
    };
    let target = TargetLayouts::foundation(GNU_1981)?;
    let error_layout = target.private_bounds()?.error_impl_actual;
    // 此固定 syntax 前缀无字符串/元素图；两个 ErrorImpl 窗口先预约，不签 decode closure。
    let bytes = error_layout.size() * 2;
    let root = ProcessLedger::new(bytes, [u32::MAX; CLASS_COUNT]);
    let grant = root.try_reserve(
        Amount {
            rust_bytes: bytes,
            c_bytes: 0,
            slots: 1,
        },
        ResourceClass::Decode,
        Admission {
            epoch: 1,
            source: None,
        },
    )?;
    let audit = AuditScope::start(&root);
    let parsed = serde_json::from_slice::<serde_json::Value>(b"[");
    assert!(parsed.is_err());
    drop(parsed);
    let io_error = serde_json::Error::io(std::io::Error::from_raw_os_error(1));
    assert!(io_error.is_io());
    drop(io_error);
    let report = audit.finish();
    assert!(!report.events().is_empty());
    assert!(
        report
            .events()
            .iter()
            .all(|event| event.size == error_layout.size()
                && event.align == error_layout.align()
                && event.held_rust == bytes)
    );
    drop(grant);
    assert_eq!(root.snapshot().rust_bytes, 0);
    Ok(())
}
