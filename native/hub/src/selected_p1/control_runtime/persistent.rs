//! 常驻账本绑定到最后一份 inbox backing；public Layout 由最终类型求值。

use super::inbox::{InboxBody, InboxState};
use crate::selected_p1::{
    model::layout::NumericReject,
    resource::{
        ledger::{Admission, Amount, ProcessLedger, ResourceClass},
        owner::{OwnerLayout, SharedOwner},
    },
};
use std::sync::Mutex;

/// 调用五控制/账本的外部线程可能走 std poison 的慢路径。
/// 父需在首次调用之前持有该 TLS 的 process/thread-lifetime root，不能绑到 inbox Drop。
/// worker 的 9 份同类费用由 helper pool 保到实际 join/TLS dtor 完成。
pub fn external_caller_tls_layout() -> Result<std::alloc::Layout, NumericReject> {
    if !cfg!(all(
        target_os = "windows",
        target_env = "gnu",
        target_pointer_width = "64",
        panic = "unwind"
    )) {
        return Err(NumericReject::UnsupportedEnvelope);
    }
    std::alloc::Layout::new::<std::cell::Cell<(usize, bool)>>()
        .extend(std::alloc::Layout::new::<u32>())
        .map(|pair| pair.0.pad_to_align())
        .map_err(|_| NumericReject::InvalidLayout)
}

/// 最终布局与源成本分开；调用者 TLS/可信路径原图由父真实 root 持有，不虚记为零。
#[derive(Clone, Copy, Debug)]
pub struct ControlCostClasses {
    pub inbox_backing: std::alloc::Layout,
    pub external_caller_tls: std::alloc::Layout,
}
pub fn cost_classes() -> Result<ControlCostClasses, NumericReject> {
    Ok(ControlCostClasses {
        inbox_backing: inbox_layout()?.backing,
        external_caller_tls: external_caller_tls_layout()?,
    })
}

pub fn inbox_layout() -> Result<OwnerLayout, NumericReject> {
    OwnerLayout::actual::<InboxBody<'_>>(0)
}

pub fn allocate<'r>(
    ledger: &'r ProcessLedger,
    epoch: u64,
) -> Result<SharedOwner<'r, InboxBody<'r>>, NumericReject> {
    let layout = inbox_layout()?;
    let grant = ledger.try_reserve(
        Amount {
            rust_bytes: layout.backing.size(),
            c_bytes: 0,
            slots: 1,
        },
        ResourceClass::Persistent,
        Admission {
            epoch,
            source: None,
        },
    )?;
    SharedOwner::try_init(grant, 0, 0, |_| {
        Ok(InboxBody {
            state: Mutex::new(InboxState::initial()),
        })
    })
}
