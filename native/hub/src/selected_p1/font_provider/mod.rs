//! 从已收费 settings、槽版本与受信目录句柄同步捕获既有字体缓存。
//! 不调用旧 load_by_hash，不持有旧 FontResource/Arc，不发布设置或字体选择。

pub mod model;
pub mod root;
pub mod slot;
#[cfg(target_os = "windows")]
mod windows;

use std::sync::atomic::{AtomicBool, Ordering};

use rshell_m0::rshell_core::TerminalSettingsV1;

use crate::selected_p1::font_source::metadata::{FontMode, saved_binding, valid_hash};
use crate::selected_p1::font_source::{
    BorrowedCapture, FontAvailability, FontBuildProof, FontReject, ResourceFailure, SourceOwner,
    TrustedRoot, capture_bytes,
};
use crate::selected_p1::model::layout::NumericReject;
use crate::selected_p1::resource::ledger::{Admission, ProcessLedger};

pub use root::{BudgetedCacheRoot, TrustedCacheRoot};
pub use slot::BudgetedLegacySlot;

/// 完整 ResolvedSnapshot 的借用投影；调用期间父必须持有其原 decode/resolve owner。
/// 此值不复制 settings，不充当完整 snapshot 的证书或预算根。
#[derive(Clone, Copy)]
pub struct BorrowedProfile<'a> {
    pub settings: &'a TerminalSettingsV1,
    pub profile_id: [u8; 16],
    pub revision: u64,
}

/// 父仅在此返回 true 时进入偏好锁；槽身份和版本必须在同一临界区捕获一次。
pub fn uses_legacy_slot(settings: &TerminalSettingsV1) -> bool {
    matches!(
        saved_binding(settings).mode,
        FontMode::LegacyUndetermined | FontMode::InheritTerminalSlot
    )
}

/// 同步借用不能逃出调用；缺省槽也必须由含版本的 BudgetedLegacySlot 表示。
/// explicitCached/ExplicitFamily/Uneditable 不检查、不解引用 slot。
pub fn capture<'root>(
    proof: &FontBuildProof,
    ledger: &'root ProcessLedger,
    admission: Admission,
    profile: BorrowedProfile<'_>,
    slot: Option<&BudgetedLegacySlot<'_>>,
    root: TrustedCacheRoot<'_, '_>,
    cancel: &AtomicBool,
) -> Result<SourceOwner<'root>, FontReject> {
    proof.require()?;
    if cancel.load(Ordering::Acquire) {
        return Err(NumericReject::Cancelled.into());
    }
    let binding = saved_binding(profile.settings);
    let (legacy_slot_version, captured_slot) = if uses_legacy_slot(profile.settings) {
        let slot = slot.ok_or(NumericReject::InvalidSource)?;
        (slot.version(), slot.font())
    } else {
        (0, None)
    };
    let input = BorrowedCapture {
        settings: profile.settings,
        profile_id: profile.profile_id,
        revision: profile.revision,
        legacy_slot_version,
        captured_slot,
        trusted_root: TrustedRoot::from_root_caller(root.path()),
    };
    let hash = if matches!(
        binding.mode,
        FontMode::LegacyUndetermined | FontMode::InheritTerminalSlot
    ) {
        captured_slot.map(|font| font.hash.as_str())
    } else {
        binding.hash
    };
    let Some(hash) = hash.filter(|hash| valid_hash(hash)) else {
        return capture_bytes(
            proof,
            ledger,
            admission,
            input,
            FontAvailability::Failed(ResourceFailure::Missing),
            cancel,
        );
    };

    #[cfg(target_os = "windows")]
    {
        windows::capture_cached(proof, ledger, admission, input, root, hash, cancel)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = hash;
        Err(NumericReject::UnsupportedEnvelope.into())
    }
}

#[cfg(all(test, target_os = "windows"))]
pub use windows::probe;
