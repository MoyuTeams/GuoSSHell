use std::alloc::Layout;

use crate::selected_p1::model::layout::NumericReject;
use crate::selected_p1::resource::ledger::{
    Admission, Amount, Grant, ProcessLedger, ReservationReject, ResourceClass,
};
use crate::selected_p1::resource::owner::{OwnerLayout, SharedOwner};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Site {
    Binding,
    Registry,
    Socket,
    IoRegistration,
    Timer,
    Cooperative,
    Task,
    ThreadJob,
    Poller,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Prerequisite {
    UnixPollerSourceClosure,
    WindowsThreadSourceClosure,
    ReservedInlineInstaller,
    ProductionHolderGraph,
    DnsClosureAndResult,
    ExternalSignerClosureAndResult,
}

/// 准入类别来自同一次锁内判定；没有字符串恢复或全局 last-error。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeFault {
    Admission {
        site: Site,
        reason: ReservationReject,
    },
    Numeric {
        site: Site,
        reason: NumericReject,
    },
    RegistryFull {
        site: Site,
        capacity: usize,
    },
    Os {
        site: Site,
        code: i32,
    },
    Cancelled,
    Unsupported(Prerequisite),
}

impl RuntimeFault {
    pub(crate) fn numeric(site: Site, reason: NumericReject) -> Self {
        Self::Numeric { site, reason }
    }

    pub(crate) fn io(self) -> std::io::Error {
        match self {
            Self::Os { code, .. } => std::io::Error::from_raw_os_error(code),
            Self::Cancelled => std::io::ErrorKind::Interrupted.into(),
            Self::Unsupported(_) => std::io::ErrorKind::Unsupported.into(),
            _ => std::io::ErrorKind::OutOfMemory.into(),
        }
    }
}

/// 原进程账本及固定 attempt 准入；旧实例不随新 epoch/source 重绑定。
#[derive(Clone, Copy)]
pub struct AppScope {
    pub ledger: &'static ProcessLedger,
    pub admission: Admission,
}

impl AppScope {
    pub fn reserve(
        self,
        site: Site,
        layout: Layout,
        slots: u32,
        class: ResourceClass,
    ) -> Result<Grant<'static>, RuntimeFault> {
        self.ledger
            .try_reserve_detailed(
                Amount {
                    rust_bytes: layout.size(),
                    c_bytes: 0,
                    slots,
                },
                class,
                self.admission,
            )
            .map_err(|reason| RuntimeFault::Admission { site, reason })
    }

    pub(crate) fn own<T>(
        self,
        site: Site,
        slots: u32,
        init: impl FnOnce() -> Result<T, NumericReject>,
    ) -> Result<SharedOwner<'static, T>, RuntimeFault> {
        let model =
            OwnerLayout::actual::<T>(0).map_err(|reason| RuntimeFault::numeric(site, reason))?;
        let grant = self.reserve(site, model.backing, slots, ResourceClass::Runtime)?;
        SharedOwner::try_init(grant, 0, 0, |_| init())
            .map_err(|reason| RuntimeFault::numeric(site, reason))
    }
}
