//! exact global backing 的第一份 owned 明文先受保护；不经 String/Vec shrink。

use std::alloc::{Layout, alloc, dealloc};
use std::mem::ManuallyDrop;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};

use secrecy::{ExposeSecret, SecretString};
use zeroize::Zeroize;

use super::ledger::{Amount, Grant};
use crate::selected_p1::model::layout::NumericReject;

pub struct CopyValidity {
    valid: AtomicBool,
}

impl Default for CopyValidity {
    fn default() -> Self {
        Self {
            valid: AtomicBool::new(true),
        }
    }
}

impl CopyValidity {
    pub fn invalidate(&self) {
        self.valid.store(false, Ordering::Release);
    }
    pub fn is_valid(&self) -> bool {
        self.valid.load(Ordering::Acquire)
    }
}

struct ZeroizingBacking<'root> {
    pointer: NonNull<u8>,
    layout: Layout,
    permit: Option<Grant<'root>>,
}

impl Drop for ZeroizingBacking<'_> {
    fn drop(&mut self) {
        // SAFETY: 全部 n bytes 已初始化；guard 独占，Cancel 只置失效位、不访问此内存。
        unsafe {
            std::slice::from_raw_parts_mut(self.pointer.as_ptr(), self.layout.size()).zeroize();
            if self.layout.size() != 0 {
                dealloc(self.pointer.as_ptr(), self.layout);
            }
        }
        drop(self.permit.take());
    }
}

/// 可放在已收费外层 allocation 内；此类型自身不分配 header。
#[repr(C)]
pub struct LockedSecretString<'root> {
    secret: ManuallyDrop<SecretString>,
    permit: ManuallyDrop<Grant<'root>>,
}

impl<'root> LockedSecretString<'root> {
    pub fn try_copy(
        input: &str,
        permit: Grant<'root>,
        validity: &CopyValidity,
    ) -> Result<Self, NumericReject> {
        Self::copy_checked(input, permit, validity, |_| Ok(()))
    }

    fn copy_checked(
        input: &str,
        permit: Grant<'root>,
        validity: &CopyValidity,
        mut checkpoint: impl FnMut(usize) -> Result<(), NumericReject>,
    ) -> Result<Self, NumericReject> {
        let n = input.len();
        let layout = Layout::array::<u8>(n).map_err(|_| NumericReject::InvalidLayout)?;
        if n > permit.amount().rust_bytes {
            return Err(NumericReject::RustBudgetExceeded);
        }
        if !validity.is_valid() {
            return Err(NumericReject::Cancelled);
        }
        let pointer = if n == 0 {
            NonNull::dangling()
        } else {
            // SAFETY: 非零 checked Layout；后续 Box<str> 也使用同一个 global allocator。
            NonNull::new(unsafe { alloc(layout) }).ok_or(NumericReject::AllocationFailure)?
        };
        // guard 在写入第一份 owned 明文之前建立；初始化整个 exact capacity。
        let mut guard = ZeroizingBacking {
            pointer,
            layout,
            permit: Some(permit),
        };
        unsafe {
            pointer.as_ptr().write_bytes(0, n);
        }
        let mut copied = 0;
        for chunk in input.as_bytes().chunks(1024) {
            // SAFETY: input 与新 allocation 不重叠，范围已由 n 的 Layout 验证。
            unsafe {
                std::ptr::copy_nonoverlapping(
                    chunk.as_ptr(),
                    pointer.as_ptr().add(copied),
                    chunk.len(),
                );
            }
            copied += chunk.len();
            checkpoint(copied)?;
        }
        // install 到 inbox 时 C 所有者仍须在自己的线性化点复核 validity/token。
        if !validity.is_valid() {
            return Err(NumericReject::Cancelled);
        }
        let Some(permit) = guard.permit.take() else {
            std::process::abort();
        };
        // SAFETY: exact n-byte global allocation（n=0 为对齐 dangling）；全部 bytes 等于
        // 已验证的 input UTF-8。Box<str> 接管同一 allocation，不 shrink/copy/新分配。
        let boxed = unsafe {
            Box::from_raw(std::ptr::slice_from_raw_parts_mut(pointer.as_ptr(), n) as *mut str)
        };
        std::mem::forget(guard);
        Ok(Self {
            secret: ManuallyDrop::new(SecretString::new(boxed)),
            permit: ManuallyDrop::new(permit),
        })
    }

    pub fn expose_secret(&self) -> &str {
        self.secret.expose_secret()
    }
    pub fn admitted(&self) -> Amount {
        self.permit.amount()
    }

    /// 可信私有适配器使用；W1-S 必须证明实际 AuthPlan/transport/task operation root。
    /// # Safety
    /// consume 产生的 O 必须持有原秘密，所有副本/异步使用不能逃出此 operation 的寿命；
    /// O drop 返回前全部秘密 backing 必须已 wipe/free。不得 raw-take 后提早释放额度。
    /// 所有额外 clone/backing 必须在调用前另有 Grant，FFI 使用时 O::drop 不得 unwind。
    pub unsafe fn into_operation<O>(
        self,
        consume: impl FnOnce(SecretString) -> O,
    ) -> SecretOperation<'root, O> {
        let mut this = ManuallyDrop::new(self);
        // SAFETY: 唯一 owned self；两字段各移动一次，operation 或局部 unwind guard 持 permit。
        let permit = unsafe { ManuallyDrop::take(&mut this.permit) };
        let secret = unsafe { ManuallyDrop::take(&mut this.secret) };
        let operation = consume(secret);
        SecretOperation {
            operation: ManuallyDrop::new(operation),
            permit: ManuallyDrop::new(permit),
        }
    }
}

impl Drop for LockedSecretString<'_> {
    fn drop(&mut self) {
        // SAFETY: 两字段初始化且尚未取走；真实 SecretString Drop zeroize 后 Box free。
        unsafe {
            ManuallyDrop::drop(&mut self.secret);
            ManuallyDrop::drop(&mut self.permit);
        }
    }
}

#[repr(C)]
pub struct SecretOperation<'root, O> {
    operation: ManuallyDrop<O>,
    permit: ManuallyDrop<Grant<'root>>,
}

impl<O> SecretOperation<'_, O> {
    pub fn operation(&self) -> &O {
        &self.operation
    }
    pub fn admitted(&self) -> Amount {
        self.permit.amount()
    }
}

impl<O> Drop for SecretOperation<'_, O> {
    fn drop(&mut self) {
        // SAFETY: O 已由可信适配器建立，秘密及所有不可逃逸使用均在 O 内。
        // 把 permit 先取到栈，单次 O::drop unwind 也不会泄漏额度。
        let permit = unsafe { ManuallyDrop::take(&mut self.permit) };
        unsafe {
            ManuallyDrop::drop(&mut self.operation);
        }
        drop(permit);
    }
}

#[cfg(test)]
#[path = "secret_tests.rs"]
mod tests;
