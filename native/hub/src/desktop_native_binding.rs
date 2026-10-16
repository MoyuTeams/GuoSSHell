//! 正常会话的独立原生绑定接缝。不创建终端、连接、运行时或 fixture owner。
//!
//! 平台宿主须先以真实 OS 来源登记；普通绑定只领取这个既有 surface。
//! C ABI 仅供受信原生宿主使用；不通过 Dart/Rinf 暴露原生登记能力。

mod ffi;
#[cfg(test)]
mod tests;

use crate::desktop_input_port::{self, Binding, Reject, Surface};
use crate::signals::desktop_session::DesktopSource;
use std::sync::Arc;

pub(crate) const VERSION: u16 = 1;

/// Busy 可将原请求稍后再投递，不得改取当前 pane；其他错误需按原来源处理。
pub(crate) fn bind(
    version: u16,
    source: &DesktopSource,
    identity: &str,
    binding: Binding,
) -> Result<Arc<Surface>, Reject> {
    if version != VERSION {
        return Err(Reject::UnsupportedVersion);
    }
    desktop_input_port::lookup(source, identity)?.attach(binding)
}

/// # Safety
/// 调用者是实际平台宿主：已证明 UI 线程、原 pane 的排他焦点域，以及真实原生
/// key/IME/raw/dual 事件来源。不得从 Dart/Rinf 数字、自报 bool 或 fixture 名称推导证明。
/// 必须保留返回的 Arc 至该 surface 的真实 callback 退休；先 revoke 停止准入，
/// 已 claim 的原目标动作结算，平台确认不再回调后才释放。SSH actor 的 join 属非 UI owner。
/// attempt 必须来自原绑定尝试，不得在迟到回调中改取 current pane/attempt。
pub(crate) unsafe fn register_verified_host(
    source: &DesktopSource,
    identity: &str,
    attempt: u64,
) -> Result<Arc<Surface>, Reject> {
    let port = desktop_input_port::lookup(source, identity)?;
    unsafe { port.attach_verified(attempt) }
}
