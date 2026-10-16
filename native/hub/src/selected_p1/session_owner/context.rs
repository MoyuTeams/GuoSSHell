//! 借用 reactor 的入口不新建 driver；persistent root 必须覆盖原 runtime 和 GNU TLS。

use std::marker::PhantomData;
use std::rc::Rc;
use tokio::runtime::Handle;

use super::operation::{Opcode, OperationBound, OperationRoot};
use crate::selected_p1::model::layout::NumericReject;
use crate::selected_p1::resource::ledger::{Admission, ProcessLedger};

pub struct BorrowedReactor<'a> {
    handle: &'a Handle,
    persistent: OperationRoot<'a, ()>,
    same_thread: PhantomData<Rc<()>>,
}

impl<'a> BorrowedReactor<'a> {
    /// # Safety
    /// actual runtime 仍由已收费 persistent owner 驱动并存活；bytes 覆盖本线程所有
    /// 首次 TLS（GNU System allocation 也计入）、enter clone/context/通知。
    /// root 活到全部调用及 IO/waker holder 结束、线程 TLS dtor 完成；不得按 enter 退出退额。
    /// 新建 Runtime/线程/driver 不在此接缝中，须独立完整源式，不能填 0。
    pub unsafe fn from_persistent_proof(
        handle: &'a Handle,
        bytes: usize,
        ledger: &'a ProcessLedger,
        admission: Admission,
    ) -> Result<Self, NumericReject> {
        if bytes == 0 {
            return Err(NumericReject::UnsupportedEnvelope);
        }
        let persistent = unsafe {
            OperationRoot::before_call(
                OperationBound {
                    opcode: Opcode::ReactorContext,
                    rust_bytes: bytes,
                },
                ledger,
                admission,
                || (),
            )
        }?;
        Ok(Self {
            handle,
            persistent,
            same_thread: PhantomData,
        })
    }
    pub fn with_enter<R>(&self, action: impl FnOnce() -> R) -> R {
        let _root = &self.persistent;
        let _guard = self.handle.enter();
        action()
    }
}
