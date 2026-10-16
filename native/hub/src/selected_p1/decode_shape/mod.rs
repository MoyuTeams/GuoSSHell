//! 借用 JSON 的全 occurrence 与错误前缀计形；不执行领域解码或签发资源证书。
//!
//! 调用顺序为 `needed_scratch` → 调用者真实 grant/取得 scratch → `scan`。
//! `Complete` 仅表示结构词法完整：数值范围、owned Unicode、typed 递归限制、
//! derive/default/flatten 与业务 predicate 仍由已预约的旧 codec 判定。
//! `Invalid` 保留前缀供旧 codec/错误模板预约，不直接替换其错误类或检查顺序。
//! 所有容器（包括失败时未闭合容器）可经 `scan_with` 重放给 target 成本模型，
//! 不建立按节点分配的统计表。重复 key 不去重，所有数组都覆盖 struct-array 候选。
//!
//! 源上下文：serde_json 1.0.151 `read.rs::{parse_str_bytes,ignore_str,
//! push_wtf8_codepoint}`、`de.rs::{ignore_value,ignore_integer}`；serde 1.0.229。
//! 仅适用于 SliceRead、default/std，且无 preserve_order、arbitrary_precision、
//! raw_value、unbounded_depth、float_roundtrip 的既有分支。
//! `push_wtf8_codepoint` 的非 ASCII 输出先 reserve(4)，不是只 reserve 实际长度；
//! 因此 scratch 的 required 输入独立于 decoded len。
//!
//! 本模块不提供 Value/Content/FlattenEntry/KeyBinding/ErrorImpl 的布局、
//! Vec 增长或 BTree split 证书。父模型须对每个 ContainerPrefix 的 started_members
//! 计算这些类的旧新共存，另加 schema defaults、真实错误模板、resolve/clone、
//! raw owner 和目标私有 owner 布局；不能只以最终去重结果或观测峰值求界。

mod error;
mod scanner;

pub use error::{ErrorFormatInputs, ErrorFormatUpper};
pub use scanner::{needed_scratch, scan, scan_with};

use core::alloc::Layout;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumericReject {
    ArithmeticOverflow,
    NeededScratch(ScratchRequirement),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScratchRequirement {
    pub slots: usize,
    pub bytes: usize,
    pub alignment: usize,
}

impl ScratchRequirement {
    /// 每个活动容器消耗不同的 opening byte，故 input_len 个 slot 是无语法前提的界。
    pub fn for_input_len(input_len: usize) -> Result<Self, NumericReject> {
        Self::for_slots(input_len)
    }

    pub(super) fn for_slots(slots: usize) -> Result<Self, NumericReject> {
        let layout =
            Layout::array::<ScratchFrame>(slots).map_err(|_| NumericReject::ArithmeticOverflow)?;
        Ok(Self {
            slots,
            bytes: layout.size(),
            alignment: layout.align(),
        })
    }
}

/// 调用者在 grant 后初始化的层级状态；scanner 不创建动态集合或递归调用栈。
#[derive(Clone, Copy, Debug, Default)]
pub struct ScratchFrame {
    pub(super) start: usize,
    pub(super) started: usize,
    pub(super) completed: usize,
    pub(super) state: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContainerKind {
    Array,
    Object,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContainerPrefix {
    pub kind: ContainerKind,
    pub start: usize,
    pub end: usize,
    pub started_members: usize,
    pub completed_members: usize,
    pub closed: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ContainerInputs {
    pub occurrences: usize,
    pub started_members: usize,
    pub completed_members: usize,
    pub max_started_members: usize,
}

impl ContainerInputs {
    pub(super) fn include(&mut self, prefix: ContainerPrefix) -> Result<(), NumericReject> {
        self.occurrences = add(self.occurrences, 1)?;
        self.started_members = add(self.started_members, prefix.started_members)?;
        self.completed_members = add(self.completed_members, prefix.completed_members)?;
        self.max_started_members = self.max_started_members.max(prefix.started_members);
        Ok(())
    }
}

/// 首次 grant 的输入全为标量；逐容器非线性公式可用同一 raw/scratch 重放。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CostInputs {
    /// 值与 object key 各消耗一个不同的起始 byte；含不完整 token。
    pub tokens_started: usize,
    pub strings_started: usize,
    pub keys_started: usize,
    pub escaped_strings: usize,
    pub escape_sequences: usize,
    pub decoded_string_bytes_upper: usize,
    pub max_decoded_string_bytes_upper: usize,
    pub max_string_scalars_upper: usize,
    /// 含 reserve(4) 的 required（不是 capacity，也不是测量峰值）。
    pub max_string_scratch_required_upper: usize,
    pub arrays: ContainerInputs,
    pub objects: ContainerInputs,
    pub max_depth: usize,
    pub max_array_depth: usize,
    pub max_object_depth: usize,
    /// 不识别 schema 时，以所有容器深度覆盖任何 ignored 子树的 scratch。
    pub ignored_stack_required_upper: usize,
    pub numbers_started: usize,
    pub max_number_bytes: usize,
    /// 仅标记 owned String 的风险；IgnoredAny 可接受，不能据此拒绝整个输入。
    pub strings_with_invalid_utf8: usize,
    pub unpaired_surrogates: usize,
}

impl CostInputs {
    pub fn parser_scratch_required_upper(self) -> usize {
        self.max_string_scratch_required_upper
            .max(self.ignored_stack_required_upper)
    }

    pub fn debug_string_bytes_upper(self) -> Result<usize, NumericReject> {
        // Debug 的每 scalar 最多十个 ASCII byte（\u{10ffff}），另有两引号。
        add(mul(self.max_string_scalars_upper, 10)?, 2)
    }

    pub fn error_count_upper(self) -> usize {
        self.tokens_started
            .max(self.arrays.max_started_members)
            .max(self.objects.max_started_members)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrefixErrorKind {
    UnexpectedEnd,
    ExpectedValue,
    ExpectedKey,
    ExpectedColon,
    ExpectedCommaOrEnd,
    TrailingComma,
    TrailingCharacters,
    InvalidLiteral,
    InvalidNumber,
    InvalidEscape,
    ControlInString,
}

/// 本扫描器的首个语法失败位置，不冒充 serde 的错误类、line/column 或检查顺序。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrefixError {
    pub kind: PrefixErrorKind,
    pub offset: usize,
    pub active_depth: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeStatus {
    Complete,
    Invalid(PrefixError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shape {
    pub status: ShapeStatus,
    /// [0,prefix_bytes) 内的已消费前缀；即使失败也保留计数与未闭合容器。
    pub prefix_bytes: usize,
    pub input_bytes: usize,
    pub costs: CostInputs,
}

impl Shape {
    /// 覆盖同一输入的 SliceRead 解析位置：line≤input_bytes+1，column≤input_bytes。
    /// serde_json 1.0.151 的 position_of_index 以 1 为首行，peek 索引不超过输入长度。
    pub fn position_value_upper(&self) -> Result<usize, NumericReject> {
        self.input_bytes
            .checked_add(1)
            .ok_or(NumericReject::ArithmeticOverflow)
    }
}

pub(super) fn add(left: usize, right: usize) -> Result<usize, NumericReject> {
    left.checked_add(right)
        .ok_or(NumericReject::ArithmeticOverflow)
}

pub(super) fn mul(left: usize, right: usize) -> Result<usize, NumericReject> {
    left.checked_mul(right)
        .ok_or(NumericReject::ArithmeticOverflow)
}
