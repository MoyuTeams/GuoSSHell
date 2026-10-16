//! no-Weak 的共享资源；最后 inner drop → original backing free → grant return。

use std::alloc::{Layout, alloc, dealloc};
use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicUsize, Ordering, fence};

use super::ledger::{Amount, Grant};
use crate::selected_p1::model::layout::{NumericReject, add, extend};

pub const MAX_REFCOUNT: usize = isize::MAX as usize;

#[repr(C)]
pub struct RcMemberLayout<P> {
    refs: AtomicUsize,
    permit: ManuallyDrop<P>,
}

#[repr(C)]
pub struct ResourceBody<T> {
    original_layout: Layout,
    payload_offset: usize,
    payload_len: usize,
    value: T,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OwnerLayout {
    pub backing: Layout,
    pub body_offset: usize,
    pub payload_offset: usize,
}

impl OwnerLayout {
    pub fn actual<T>(payload_len: usize) -> Result<Self, NumericReject> {
        let (prefix, body_offset) = extend(
            Layout::new::<RcMemberLayout<Grant<'_>>>(),
            Layout::new::<ResourceBody<T>>(),
        )?;
        let payload = Layout::array::<u8>(payload_len).map_err(|_| NumericReject::InvalidLayout)?;
        let (backing, payload_offset) = extend(prefix, payload)?;
        Ok(Self {
            backing,
            body_offset,
            payload_offset,
        })
    }

    pub fn required_rust(self, out_of_line_upper: usize) -> Result<usize, NumericReject> {
        add(self.backing.size(), out_of_line_upper)
    }
}

/// 未初始化 allocation 的析构根；初始化失败/unwind 也先 dealloc 再返额。
struct BackingGuard<'root> {
    pointer: NonNull<u8>,
    layout: Layout,
    permit: Option<Grant<'root>>,
}

impl Drop for BackingGuard<'_> {
    fn drop(&mut self) {
        // SAFETY: pointer 来自 global alloc(layout)，尚未移交/释放，且 layout 非零。
        unsafe {
            dealloc(self.pointer.as_ptr(), self.layout);
        }
        drop(self.permit.take());
    }
}

pub struct SharedOwner<'root, T> {
    header: NonNull<RcMemberLayout<Grant<'root>>>,
    body: NonNull<ResourceBody<T>>,
    marker: PhantomData<&'root T>,
}

// SAFETY: 只有不可变 T/初始化后 payload 借用；跨线程共享要求 T 同时 Send+Sync。
// 每个 holder 独占一个计数，clone CAS 与最后 Release/Acquire 保证只析构一次。
// Grant 引用的 ProcessLedger 为 Sync，且 'root 阻止账本先于所有 holder 销毁。
unsafe impl<T: Send + Sync> Send for SharedOwner<'_, T> {}
// SAFETY: 与 Send 相同；没有可变 T 访问或 Weak/复活入口。
unsafe impl<T: Send + Sync> Sync for SharedOwner<'_, T> {}

impl<'root, T> SharedOwner<'root, T> {
    /// init 中的 out-of-line 构造也须已由 out_of_line_upper 的源模型覆盖。
    pub fn try_init(
        permit: Grant<'root>,
        payload_len: usize,
        out_of_line_upper: usize,
        init: impl FnOnce(&mut [u8]) -> Result<T, NumericReject>,
    ) -> Result<Self, NumericReject> {
        let model = OwnerLayout::actual::<T>(payload_len)?;
        if model.required_rust(out_of_line_upper)? > permit.amount().rust_bytes {
            return Err(NumericReject::RustBudgetExceeded);
        }
        // header 含 AtomicUsize，所以 backing 永远非零；不向 allocator 发送零尺寸。
        // SAFETY: checked 非零 Layout；使用与 dealloc 一致的 global allocator。
        let pointer = NonNull::new(unsafe { alloc(model.backing) })
            .ok_or(NumericReject::AllocationFailure)?;
        let mut guard = BackingGuard {
            pointer,
            layout: model.backing,
            permit: Some(permit),
        };
        // SAFETY: tail 位于已取得 allocation 中；先初始化全部 bytes 再交安全 slice。
        let payload_pointer = unsafe { pointer.as_ptr().add(model.payload_offset) };
        unsafe {
            payload_pointer.write_bytes(0, payload_len);
        }
        let payload = unsafe { std::slice::from_raw_parts_mut(payload_pointer, payload_len) };
        let value = init(payload)?;
        let Some(permit) = guard.permit.take() else {
            std::process::abort();
        };
        let header = pointer.cast::<RcMemberLayout<Grant<'root>>>();
        // SAFETY: extend 给出 body 对齐/范围；所有 write 不调用用户代码/不分配。
        let body = unsafe {
            NonNull::new_unchecked(
                pointer
                    .as_ptr()
                    .add(model.body_offset)
                    .cast::<ResourceBody<T>>(),
            )
        };
        unsafe {
            header.as_ptr().write(RcMemberLayout {
                refs: AtomicUsize::new(1),
                permit: ManuallyDrop::new(permit),
            });
            body.as_ptr().write(ResourceBody {
                original_layout: model.backing,
                payload_offset: model.payload_offset,
                payload_len,
                value,
            });
        }
        std::mem::forget(guard);
        Ok(Self {
            header,
            body,
            marker: PhantomData,
        })
    }

    pub fn value(&self) -> &T {
        // SAFETY: 当前 holder 保持 initialized backing，只有不可变访问。
        unsafe { &self.body.as_ref().value }
    }

    pub fn payload(&self) -> &[u8] {
        // SAFETY: body 记录原 initialized tail 范围，当前 holder 阻止最后析构。
        unsafe {
            let body = self.body.as_ref();
            std::slice::from_raw_parts(
                self.header.as_ptr().cast::<u8>().add(body.payload_offset),
                body.payload_len,
            )
        }
    }

    pub fn admitted(&self) -> Amount {
        // SAFETY: header 活着；permit 只在最后 holder 独占析构时被取走。
        unsafe { self.header.as_ref().permit.amount() }
    }

    pub fn try_clone(&self) -> Result<Self, NumericReject> {
        // SAFETY: self 是存活 strong ref，故 refcount >= 1，不能被其他 holder 复活。
        let refs = unsafe { &self.header.as_ref().refs };
        let mut current = refs.load(Ordering::Relaxed);
        loop {
            if current >= MAX_REFCOUNT {
                return Err(NumericReject::RefcountOverflow);
            }
            match refs.compare_exchange_weak(
                current,
                current + 1,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    return Ok(Self {
                        header: self.header,
                        body: self.body,
                        marker: PhantomData,
                    });
                }
                Err(next) => current = next,
            }
        }
    }

    /// 把一个真实 strong reference 交给 FFI peer；不会改变 refcount 或分配。
    pub fn into_raw(self) -> *mut RcMemberLayout<Grant<'root>> {
        let pointer = self.header.as_ptr();
        std::mem::forget(self);
        pointer
    }

    /// # Safety
    /// pointer 必须来自同 T/'root 的 into_raw，仍独占一个未消费的 strong reference。
    /// 不允许借用 data pointer、重复接管、过期指针或在 hub 卸载后回调。
    pub unsafe fn from_raw(pointer: *mut RcMemberLayout<Grant<'root>>) -> Self {
        // T 的 body offset 与 payload 长度无关；合法 into_raw 保证构造成功过。
        let Ok(model) = OwnerLayout::actual::<T>(0) else {
            std::process::abort();
        };
        // SAFETY: 调用者提供原 strong reference，header/body 对齐并已初始化。
        unsafe {
            Self {
                header: NonNull::new_unchecked(pointer),
                body: NonNull::new_unchecked(pointer.cast::<u8>().add(model.body_offset).cast()),
                marker: PhantomData,
            }
        }
    }
}

impl<T> Drop for SharedOwner<'_, T> {
    fn drop(&mut self) {
        // SAFETY: 当前 holder 对应一个有效 strong reference。
        if unsafe { self.header.as_ref().refs.fetch_sub(1, Ordering::Release) } != 1 {
            return;
        }
        fence(Ordering::Acquire);
        // SAFETY: 最后 ref 独占；先把所有释放所需信息取到栈，之后不再读 header。
        let permit = unsafe { ManuallyDrop::take(&mut self.header.as_mut().permit) };
        let layout = unsafe { self.body.as_ref().original_layout };
        let guard = BackingGuard {
            pointer: self.header.cast(),
            layout,
            permit: Some(permit),
        };
        // SAFETY: 唯一 initialized T；guard 在 T 的正常/单次 unwind 后均 free outer 再返额。
        // FFI finalizer 的具体 T 仍须由调用者证明 Drop 不 unwind。
        unsafe {
            std::ptr::drop_in_place(std::ptr::addr_of_mut!((*self.body.as_ptr()).value));
        }
        drop(guard);
    }
}

#[cfg(test)]
#[path = "owner_tests.rs"]
mod tests;
