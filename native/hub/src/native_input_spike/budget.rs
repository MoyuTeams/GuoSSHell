//! B 自有资源的源码上界；不是全 engine allocator／生产 P1 证明。
use super::{
    core::{Core, EXHAUSTED, Owner, READ_BYTES, SLOT_BYTES, STREAM_BYTES},
    payload::{PAYLOAD_LIMIT, RETAINED_LIMIT},
    projection::Projection,
};
use rshell_m0::rshell_core::{RenderCell, RenderFrame, RenderRow};
use std::mem::size_of;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct ScopeBudget {
    pub fixed_owner: u64,
    pub retained_payload: u64,
    pub encoder_workspace: u64,
    pub chunk_window: u64,
    pub read_and_stream: u64,
    pub snapshot_and_pack: u64,
    pub existing_vte_sync: u64,
    pub per_owner_delta: u64,
    pub root_delta: u64,
}

pub(super) fn manifest() -> Result<ScopeBudget, u32> {
    // profile 捕获固定六条，engine 私有图继续消费 A 的条件资源类；不在 delta 重签。
    // encode_input 转移已授额 String；paste 一次容量 L+12；key 格式化至多32 bytes。
    // render.rs：最多3行×12 cell，ASCII域每cell一个字符，Vec→Arc同时存活也计入。
    // 当前frame、一次锁外projection lease、构造中新frame最多三份；每份另含Vec→Arc峰值。
    let frame = size_of::<RenderFrame>()
        + 32
        + 3 * (2 * size_of::<RenderRow>() + 16)
        + 36 * (2 * size_of::<RenderCell>() + 16);
    let mut budget = ScopeBudget {
        fixed_owner: (size_of::<Owner>()
            + size_of::<Core>()
            + size_of::<crate::desktop_input_policy::InputPolicyState>())
            as u64,
        retained_payload: RETAINED_LIMIT as u64,
        encoder_workspace: (PAYLOAD_LIMIT + 12 + 32) as u64,
        chunk_window: SLOT_BYTES as u64,
        read_and_stream: (READ_BYTES + 1 + STREAM_BYTES * 2) as u64,
        snapshot_and_pack: (frame * 3 + size_of::<Projection>() + 128) as u64,
        // vte 0.15.0 SyncState::default 在 engine 构造时已有2MiB预分配；不是32-byte outbox。
        existing_vte_sync: 0x20_0000,
        per_owner_delta: 0,
        root_delta: 0,
    };
    budget.per_owner_delta = [
        budget.fixed_owner,
        budget.retained_payload,
        budget.encoder_workspace,
        budget.chunk_window,
        budget.read_and_stream,
        budget.snapshot_and_pack,
    ]
    .into_iter()
    .try_fold(0u64, |sum, value| sum.checked_add(value).ok_or(EXHAUSTED))?;
    budget.root_delta = budget.per_owner_delta.checked_mul(8).ok_or(EXHAUSTED)?;
    Ok(budget)
}
