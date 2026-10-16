//! 不透明库值的额度只能由完整不可逃逸图持到析构；不是 future 外壳证书。

use std::alloc::Layout;
use std::mem::ManuallyDrop;

use crate::selected_p1::model::layout::{NumericReject, add};
use crate::selected_p1::resource::ledger::{
    Admission, Amount, Grant, ProcessLedger, ResourceClass,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Opcode {
    SessionChannels,
    TransportConnect,
    TransportNextEvent,
    TransportWrite,
    TransportResize,
    TransportShutdown,
    AuthPlan,
    InteractionChannel,
    InteractionRequest,
    ReactorContext,
    AttemptClone,
    EngineClone,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OperationBound {
    pub opcode: Opcode,
    pub rust_bytes: usize,
}

impl OperationBound {
    pub fn sum(opcode: Opcode, layouts: &[Layout], extra: usize) -> Result<Self, NumericReject> {
        let mut rust_bytes = extra;
        for layout in layouts {
            rust_bytes = add(rust_bytes, layout.size())?;
        }
        Ok(Self { opcode, rust_bytes })
    }
    pub fn reserve<'a>(
        &self,
        ledger: &'a ProcessLedger,
        admission: Admission,
    ) -> Result<Grant<'a>, NumericReject> {
        ledger.try_reserve(
            Amount {
                rust_bytes: self.rust_bytes,
                c_bytes: 0,
                slots: 1,
            },
            ResourceClass::Runtime,
            admission,
        )
    }
}

/// T 在栈／已收费 allocation 内；此 root 不另建 Box/Arc。
pub struct OperationRoot<'a, T> {
    value: ManuallyDrop<T>,
    grant: ManuallyDrop<Grant<'a>>,
}
impl<'a, T> OperationRoot<'a, T> {
    /// # Safety
    /// bound 必须覆盖实际 build 全调用闭包，build 产生的 holder/Weak/回调不得逃出 T。
    /// 包括失败/取消/迟到结果；T drop 返回前所有该 operation allocation 都已 free。
    /// 此函数先 reserve 再 build，不用 build 返回后的 size_of_val 补额度。
    pub unsafe fn before_call(
        bound: OperationBound,
        ledger: &'a ProcessLedger,
        admission: Admission,
        build: impl FnOnce() -> T,
    ) -> Result<Self, NumericReject> {
        let grant = bound.reserve(ledger, admission)?;
        let value = build();
        Ok(Self {
            value: ManuallyDrop::new(value),
            grant: ManuallyDrop::new(grant),
        })
    }
    pub fn value(&self) -> &T {
        &self.value
    }
    /// # Safety
    /// 不可把带 allocation 的字段 move/clone 到 root 外；只做已证原位操作。
    pub unsafe fn value_mut(&mut self) -> &mut T {
        &mut self.value
    }
}
impl<T> Drop for OperationRoot<'_, T> {
    fn drop(&mut self) {
        let grant = unsafe { ManuallyDrop::take(&mut self.grant) };
        unsafe { ManuallyDrop::drop(&mut self.value) };
        drop(grant);
    }
}

/// sibling 必须给真实完整 Snapshot/Source 的借用证明；本模块不构造这些类型。
/// # Safety
/// 实现者须覆盖真实 clone 的完整分配闭包，不能以投影字段/测量峰值代替来源证明。
pub unsafe trait FrozenCloneShape<T: ?Sized> {
    /// # Safety
    /// 实现必须来自真实 clone 路径，含全部 String/Path/Vec/tree/错误及共存 scratch。
    fn clone_upper(&self, borrowed: &T) -> Result<usize, NumericReject>;
}

pub fn reserve_before_attempt<'a, T: ?Sized>(
    borrowed: &T,
    shape: &impl FrozenCloneShape<T>,
    additional_layouts: &[Layout],
    ledger: &'a ProcessLedger,
    admission: Admission,
) -> Result<Grant<'a>, NumericReject> {
    OperationBound::sum(
        Opcode::AttemptClone,
        additional_layouts,
        shape.clone_upper(borrowed)?,
    )?
    .reserve(ledger, admission)
}

/// actual clone 本体依然由 caller 执行；拿到 Grant 不等于已取得 Source/TTF 证书。
pub fn reserve_before_engine<'a, T: ?Sized>(
    borrowed: &T,
    shape: &impl FrozenCloneShape<T>,
    ledger: &'a ProcessLedger,
    admission: Admission,
) -> Result<Grant<'a>, NumericReject> {
    OperationBound::sum(Opcode::EngineClone, &[], shape.clone_upper(borrowed)?)?
        .reserve(ledger, admission)
}
