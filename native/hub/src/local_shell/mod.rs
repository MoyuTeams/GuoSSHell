//! 独立 Local 探针接缝；不注册生产模块、目标或 transport factory。

mod owner;

#[cfg(windows)]
pub(crate) use owner::{Fixture, LocalProcessOwner, ProbeRoot, pty_owner};

pub(crate) use owner::{
    DispatchGate, INPUT_LIMIT, LocalReject, OUTPUT_LIMIT, ProbeBoundary, app_root_shape,
    reserve_probe_root, validate_local_fields,
};
