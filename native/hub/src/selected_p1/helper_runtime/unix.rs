//! Unix 等价固定 pipe/启动接缝由 W1-T 按实际直接 API 和目标源码闭合。

use super::process::Failure;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PendingUnixProof {
    pub process_api: bool,
    pub inherited_environment: bool,
    pub worker_tls: bool,
}

pub const REQUIRED: PendingUnixProof = PendingUnixProof {
    process_api: true,
    inherited_environment: true,
    worker_tls: true,
};

pub fn require_target_proof() -> Result<(), Failure> {
    Err(Failure::Unsupported)
}
