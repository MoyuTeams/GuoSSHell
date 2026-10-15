//! W1-D 完整 selected 领域解码；normal 接线仍须父组合所有 P1 gates。
//! 仅接已拥有、已有 Hraw grant 的 RawFields；不读取数据库、不加载整配置、不处理秘密。

pub mod budget;
pub mod codec;
pub mod error_bound;
pub mod raw;
mod resolve;
pub mod snapshot;
pub mod validation;

use crate::selected_p1::{
    decode_shape::{self, ScratchFrame},
    model::{certificate::TargetLayouts, layout::NumericReject},
    resource::{
        ledger::{Admission, Amount, Grant, ProcessLedger, ResourceClass},
        owner::{OwnerLayout, SharedOwner},
    },
};

pub use budget::{DecodePlan, bound_selected};
pub use snapshot::{CloneShape, SnapshotBody};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JsonFailure {
    pub category: u32,
    pub line: usize,
    pub column: usize,
}

impl JsonFailure {
    fn from_error(error: &serde_json::Error) -> Self {
        let category = match error.classify() {
            serde_json::error::Category::Io => 1,
            serde_json::error::Category::Syntax => 2,
            serde_json::error::Category::Data => 3,
            serde_json::error::Category::Eof => 4,
        };
        Self {
            category,
            line: error.line(),
            column: error.column(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Resource(NumericReject),
    Protocol,
    Corrupt { tag: u16, json: Option<JsonFailure> },
    Connection(validation::ConnectionValidation),
    Settings(rshell_m0::rshell_core::SettingsValidationError),
    Quick(validation::QuickValidation),
    MissingProfile,
    MissingGroup(rshell_m0::rshell_core::GroupId),
    ReferenceNotReady,
    TargetMismatch,
    TransportUnavailable,
}

impl From<NumericReject> for Failure {
    fn from(error: NumericReject) -> Self {
        Self::Resource(error)
    }
}

impl Failure {
    fn shape(error: decode_shape::NumericReject) -> Self {
        match error {
            decode_shape::NumericReject::ArithmeticOverflow => {
                Self::Resource(NumericReject::Overflow)
            }
            decode_shape::NumericReject::NeededScratch(_) => {
                Self::Resource(NumericReject::ConstructionFailure)
            }
        }
    }
}

/// 计数与真实 schema/core 调用紧邻；不足额度不进入这两类调用。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExecutionCounts {
    pub schema: u32,
    pub core: u32,
}

struct ShapeScratch<'root> {
    // 声明顺序保证 backing 先 drop，grant 后 return。
    frames: Vec<ScratchFrame>,
    grant: Grant<'root>,
}

impl ShapeScratch<'_> {
    fn bytes(&self) -> usize {
        self.grant.amount().rust_bytes
    }
}

pub struct FrozenBuild<'root> {
    pub snapshot: SharedOwner<'root, SnapshotBody>,
    pub clone_shape: CloneShape,
    pub plan: DecodePlan,
}

/// root 是进程全局账本（harness 可提供独立 root）；target 必须来自实际构建指纹核验。
pub fn decode_resolve<'root>(
    raw: &raw::RawFields<'_>,
    target: &TargetLayouts,
    root: &'root ProcessLedger,
    admission: Admission,
    calls: &mut ExecutionCounts,
) -> Result<FrozenBuild<'root>, Failure> {
    budget::validate_target(target)?;
    let requirement = budget::scratch_requirement(raw)?;
    let scratch_grant = root.try_reserve(
        Amount {
            rust_bytes: requirement.bytes,
            c_bytes: 0,
            slots: 0,
        },
        ResourceClass::Decode,
        admission,
    )?;
    let mut scratch = ShapeScratch {
        frames: Vec::new(),
        grant: scratch_grant,
    };
    scratch
        .frames
        .try_reserve_exact(requirement.slots)
        .map_err(|_| Failure::Resource(NumericReject::AllocationFailure))?;
    scratch
        .frames
        .resize(requirement.slots, ScratchFrame::default());
    debug_assert_eq!(scratch.bytes(), requirement.bytes);
    let plan = bound_selected(raw, &mut scratch.frames, target)?;
    let mut grant = root.try_reserve(
        Amount {
            rust_bytes: plan.rust_bytes,
            c_bytes: 0,
            slots: 1,
        },
        ResourceClass::Decode,
        admission,
    )?;
    // 所有 serde/Content/error/default 与 resolve clones 均在真实 grant 之后。
    let value = resolve::decode(raw, calls)?;
    let clone_shape = value.clone_shape(target)?;
    drop(scratch);
    let owner_layout = OwnerLayout::actual::<SnapshotBody>(0)?;
    let retained = owner_layout.required_rust(clone_shape.retained_bytes)?;
    grant.tighten_unused(Amount {
        rust_bytes: retained,
        c_bytes: 0,
        slots: 1,
    })?;
    let snapshot = SharedOwner::try_init(grant, 0, clone_shape.retained_bytes, |_| Ok(value))?;
    Ok(FrozenBuild {
        snapshot,
        clone_shape,
        plan,
    })
}

/// 每次真正复制完整图均先另取 grant；共享 holder 的 try_clone 不复制图。
pub fn clone_snapshot<'root>(
    value: &SnapshotBody,
    target: &TargetLayouts,
    root: &'root ProcessLedger,
    admission: Admission,
) -> Result<SharedOwner<'root, SnapshotBody>, Failure> {
    budget::validate_target(target)?;
    let shape = value.clone_shape(target)?;
    let layout = OwnerLayout::actual::<SnapshotBody>(0)?;
    let grant = root.try_reserve(
        Amount {
            rust_bytes: layout.required_rust(shape.clone_bytes)?,
            c_bytes: 0,
            slots: 1,
        },
        ResourceClass::Runtime,
        admission,
    )?;
    Ok(SharedOwner::try_init(grant, 0, shape.clone_bytes, |_| {
        Ok(value.clone())
    })?)
}
