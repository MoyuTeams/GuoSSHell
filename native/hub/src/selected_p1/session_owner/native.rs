//! async_trait 的 Box 在 trait 方法调用时分配，所以 reserve 必须在调用闭包之前。
//! 已连接分支的 russh queue/task/handler 另需完整 last-root 证明。

use std::alloc::Layout;
use std::future::Future;
use std::mem::ManuallyDrop;
use std::pin::Pin;
use std::task::{Context, Poll};

use rshell_m0::rshell_core::TerminalSize;
use rshell_m0::rshell_session::{
    InteractionBroker, NativeSshTransport, SessionTransport, TransportError, TransportEvent,
    TransportRequest,
};

use super::operation::{Opcode, OperationBound};
use crate::selected_p1::model::layout::NumericReject;
use crate::selected_p1::resource::ledger::{Admission, Grant, ProcessLedger};

#[derive(Clone, Copy)]
pub struct NativeFutureLayouts([Layout; 5]);
impl NativeFutureLayouts {
    /// # Safety
    /// 必须是 actual locked Native impl 的五个 async_trait generated F 的 layout 上界；
    /// full compiler/target/cfg/features 一致，源式和私有 monomorph 诊断互核。
    /// 顺序 connect/next_event/write/resize/shutdown；Box/dyn 指针不满足前件。
    pub unsafe fn from_monomorph_proof(layouts: [Layout; 5]) -> Result<Self, NumericReject> {
        if layouts.iter().any(|layout| layout.size() == 0) {
            return Err(NumericReject::UnsupportedEnvelope);
        }
        Ok(Self(layouts))
    }
    pub fn bound(self, opcode: Opcode) -> Result<OperationBound, NumericReject> {
        let index = match opcode {
            Opcode::TransportConnect => 0,
            Opcode::TransportNextEvent => 1,
            Opcode::TransportWrite => 2,
            Opcode::TransportResize => 3,
            Opcode::TransportShutdown => 4,
            _ => return Err(NumericReject::UnsupportedEnvelope),
        };
        OperationBound::sum(opcode, &[self.0[index]], 0)
    }
}

type BoxedNativeFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Result 也留在 root，防止 Output(Vec)/error graph 比 boxed F 活得更久时早退额。
pub struct NativeCall<'a, T> {
    future: Option<BoxedNativeFuture<'a, T>>,
    result: Option<T>,
    grant: ManuallyDrop<Grant<'a>>,
}
impl<'a, T> NativeCall<'a, T> {
    /// # Safety
    /// 全部 library allocation 和输出由 bound 覆盖；内部任务/queue/callback/Weak 不可
    /// 逃出此 root，或另有真实外部 operation root 持有到最终 dealloc，不能用 task join。
    pub unsafe fn before_call(
        bound: OperationBound,
        ledger: &'a ProcessLedger,
        admission: Admission,
        call: impl FnOnce() -> BoxedNativeFuture<'a, T>,
    ) -> Result<Self, NumericReject> {
        let grant = bound.reserve(ledger, admission)?;
        let future = call();
        Ok(Self {
            future: Some(future),
            result: None,
            grant: ManuallyDrop::new(grant),
        })
    }
    pub fn result(&self) -> Option<&T> {
        self.result.as_ref()
    }
}
impl<T> Unpin for NativeCall<'_, T> {}
impl<T> Future for NativeCall<'_, T> {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        let Some(future) = &mut this.future else {
            return Poll::Ready(());
        };
        match future.as_mut().poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(result) => {
                this.result = Some(result);
                drop(this.future.take());
                Poll::Ready(())
            }
        }
    }
}
impl<T> Drop for NativeCall<'_, T> {
    fn drop(&mut self) {
        let grant = unsafe { ManuallyDrop::take(&mut self.grant) };
        drop(self.future.take());
        drop(self.result.take());
        drop(grant);
    }
}

/// 只支持从未进入 connect 的 transport，状态由此私有 owner 管理而非 caller bool。
/// 可执行这些正常公共方法的无 channel 分支，不签发网络运行证书。
pub struct UnconnectedNative {
    transport: NativeSshTransport,
    layouts: NativeFutureLayouts,
}
impl UnconnectedNative {
    /// # Safety
    /// transport 刚从 new 构造，未调用过 connect；其 profile/auth/verifier 图已有 root。
    /// 本类型 move 接管，不能借出 &mut transport 或让它进入网络 connected 状态。
    pub unsafe fn from_new(transport: NativeSshTransport, layouts: NativeFutureLayouts) -> Self {
        Self { transport, layouts }
    }
    pub fn connect_validation<'a>(
        &'a mut self,
        request: &'a TransportRequest,
        broker: InteractionBroker,
        ledger: &'a ProcessLedger,
        admission: Admission,
    ) -> Result<NativeCall<'a, Result<(), TransportError>>, NumericReject> {
        let size = request.initial_size();
        if size.cols != 0 && size.rows != 0 {
            return Err(NumericReject::UnsupportedEnvelope);
        }
        // SAFETY: connect_inner validate_request 在 TcpStream/new Handler/config/spawn 前退出；
        // broker clone 属 caller channel root，future 自身 drop 丢弃此 holder，无内部 spawn。
        unsafe {
            NativeCall::before_call(
                self.layouts.bound(Opcode::TransportConnect)?,
                ledger,
                admission,
                || self.transport.connect(request, broker),
            )
        }
    }
    pub fn next_event<'a>(
        &'a mut self,
        ledger: &'a ProcessLedger,
        admission: Admission,
    ) -> Result<NativeCall<'a, Result<TransportEvent, TransportError>>, NumericReject> {
        unsafe {
            NativeCall::before_call(
                self.layouts.bound(Opcode::TransportNextEvent)?,
                ledger,
                admission,
                || self.transport.next_event(),
            )
        }
    }
    pub fn write<'a>(
        &'a mut self,
        bytes: &'a [u8],
        ledger: &'a ProcessLedger,
        admission: Admission,
    ) -> Result<NativeCall<'a, Result<(), TransportError>>, NumericReject> {
        // channel_mut()? 在 bytes.to_vec() 前，空 write 直接成功；不会复制 input。
        unsafe {
            NativeCall::before_call(
                self.layouts.bound(Opcode::TransportWrite)?,
                ledger,
                admission,
                || self.transport.write(bytes),
            )
        }
    }
    pub fn resize<'a>(
        &'a mut self,
        size: TerminalSize,
        ledger: &'a ProcessLedger,
        admission: Admission,
    ) -> Result<NativeCall<'a, Result<(), TransportError>>, NumericReject> {
        unsafe {
            NativeCall::before_call(
                self.layouts.bound(Opcode::TransportResize)?,
                ledger,
                admission,
                || self.transport.resize(size),
            )
        }
    }
    pub fn shutdown<'a>(
        &'a mut self,
        ledger: &'a ProcessLedger,
        admission: Admission,
    ) -> Result<NativeCall<'a, Result<(), TransportError>>, NumericReject> {
        unsafe {
            NativeCall::before_call(
                self.layouts.bound(Opcode::TransportShutdown)?,
                ledger,
                admission,
                || self.transport.shutdown(),
            )
        }
    }
}
