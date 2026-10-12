//! System 独立基础；不注册 transport，不把配置查询当认证或终端成功。

mod evidence;
mod launch;
mod locator;
mod model;

pub use evidence::*;
pub use launch::*;
pub use locator::*;
pub use model::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScopeReject {
    Resource(crate::selected_p1::model::layout::NumericReject),
    EmptyField,
    ControlCharacter,
    LeadingOption,
    InvalidHost,
    InvalidPort,
    InvalidTerm,
    AbsolutePathRequired,
    InvalidPrivateLayout,
    PrivateAttemptUnavailable,
    ExactFileSelectionRequired,
    FilePermissionNotProven,
    InvalidOverride,
    NotFound,
    NotExecutable,
    ImageChanged,
    ImageNotLocked,
    UntrustedImage,
    Io,
    Encoding,
    VersionNotOpenSsh,
    QueryMismatch,
    QueryTooLarge,
    HookNotVerified,
    SourceClosureRequired,
    Quota {
        requested: usize,
        current: usize,
        limit: usize,
    },
    Slots,
    CleanupPending,
}

impl From<crate::selected_p1::model::layout::NumericReject> for ScopeReject {
    fn from(value: crate::selected_p1::model::layout::NumericReject) -> Self {
        Self::Resource(value)
    }
}
