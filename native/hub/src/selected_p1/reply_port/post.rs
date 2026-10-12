//! 只使用锁定 allo-isolate 公共 API；Isolate::post 持有其唯一 SDK callback。

use std::ffi::c_void;

use allo_isolate::ffi::{
    DartCObject, DartCObjectType, DartCObjectValue, DartNativeExternalTypedData, DartTypedDataType,
};
use allo_isolate::{IntoDart, Isolate};

use crate::selected_p1::resource::ledger::Grant;
use crate::selected_p1::resource::owner::{RcMemberLayout, SharedOwner};

use super::owner::{NoUnwindRoot, OwnedReplyBody, UnpostedReply};
use super::registration::{PortOwner, ReplyPortReject, ReplyPortState};

/// 父同 SDK getter 的 process-lifetime library pin 证明，不是 UI owner 或卸载 guard。
pub struct LibraryPinProof {
    _private: (),
}

impl LibraryPinProof {
    /// # Safety
    /// 父已保持当前 hub 及其 allo callback 代码 loaded，直到所有 VM finalizers 结束或
    /// 进程结束；UI/port/runtime 关闭、join 或 peerGone/ACK 不能撤销该 pin。
    /// Isolate::post 使用 Rinf 初始化的同一 allo 0.1.27 实例，不能重设生产 callback。
    pub unsafe fn from_process_library_pin() -> Self {
        Self { _private: () }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PostOutcome {
    /// 只表示 post bool=true；不是 Dart 消费、ACK 或预算已释放。
    AcceptedByVm,
    /// callback 未注册或 VM 拒绝；都不是 Dart delivered 的证明。
    NotAccepted,
    RegistrationRejected(ReplyPortReject),
}

impl ReplyPortState {
    /// 父的同一 runtime actor 按注册/关闭/发送顺序调用；不把状态借给 pane 独立发送。
    /// 父另须校验 wire 中 source/session/generation/prepare_id/identity 与 admitted entry；
    /// 本方法的 port/Grant epoch 检查不代替该关联或父的生产证书 gate。
    pub fn post<R: NoUnwindRoot + 'static>(
        &self,
        owner: PortOwner,
        reply: UnpostedReply<'static, R>,
        _pin: &LibraryPinProof,
    ) -> PostOutcome {
        if reply.epoch() != owner.epoch {
            return PostOutcome::RegistrationRejected(ReplyPortReject::EpochMismatch);
        }
        let port = match self.authorize(owner) {
            Ok(port) => port,
            Err(error) => return PostOutcome::RegistrationRejected(error),
        };
        if Isolate::new(port.value()).post(VmMessage { reply }) {
            PostOutcome::AcceptedByVm
        } else {
            // false 若进入 IntoDart，allo 已恰一次调用 finalizer；不能再次接管 peer。
            PostOutcome::NotAccepted
        }
    }
}

// 只有 post 验证 registration/library pin 后才能构造；UnpostedReply 不公开 IntoDart 旁路。
struct VmMessage<R: NoUnwindRoot + 'static> {
    reply: UnpostedReply<'static, R>,
}

impl<R: NoUnwindRoot + 'static> IntoDart for VmMessage<R> {
    fn into_dart(self) -> DartCObject {
        let mut object = DartCObject {
            ty: DartCObjectType::DartExternalTypedData,
            value: DartCObjectValue {
                as_external_typed_data: DartNativeExternalTypedData {
                    ty: DartTypedDataType::Uint8,
                    length: 0,
                    data: std::ptr::null_mut(),
                    peer: std::ptr::null_mut(),
                    callback: finalize::<R>,
                },
            },
        };
        let (peer, data, length) = self.reply.into_raw_parts();
        // 转 raw 后只填栈上 CObject，不读 owner/tail，不再分配或调用用户代码。
        object.value.as_external_typed_data = DartNativeExternalTypedData {
            ty: DartTypedDataType::Uint8,
            length,
            data,
            peer: peer.cast::<c_void>(),
            callback: finalize::<R>,
        };
        object
    }
}

struct AbortOnUnwind;
impl Drop for AbortOnUnwind {
    fn drop(&mut self) {
        std::process::abort();
    }
}

unsafe extern "C" fn finalize<R: NoUnwindRoot + 'static>(
    _isolate_callback_data: *mut c_void,
    peer: *mut c_void,
) {
    // SAFETY: 唯一 peer 来自同 R/'static 的 into_raw；VM 或 allo false 分支只消费一次。
    // 首参数不是 owner。null/过期/越界/重复 peer 不属于此 API 的合法输入，不能假 drop。
    let abort = AbortOnUnwind;
    let owner = unsafe {
        SharedOwner::<OwnedReplyBody<R>>::from_raw(peer.cast::<RcMemberLayout<Grant<'static>>>())
    };
    drop(owner);
    // 正常路径不触发 abort；若违反 NoUnwindRoot 证明则立即终止，不吞 panic 继续 service。
    std::mem::forget(abort);
}
