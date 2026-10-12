//! future 仅由唯一 poll owner 访问；跨线程 waker/cancel 只访问固定 header。

use std::alloc::{Layout, alloc, dealloc};
use std::cell::Cell;
use std::future::Future;
use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering, fence};
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

use crate::selected_p1::model::layout::{NumericReject, extend};
use crate::selected_p1::resource::ledger::{
    Admission, Amount, Grant, ProcessLedger, ResourceClass,
};

/// 包含 poll owner、每轮 Context、cancel 和所有库/回调保存的 waker。
/// 此值不是由实测峰值签发的生产证明。
#[derive(Clone, Copy)]
pub struct HolderBound(usize);

impl HolderBound {
    /// # Safety
    /// 调用闭包的完整源式须证明同时存活引用（含内部 clone/跨线程回调）不超过 max。
    /// 不允许 forget holder；RawWaker 没有 fallible clone，违反前件将直接 abort。
    pub unsafe fn from_source_proof(max: usize) -> Result<Self, NumericReject> {
        if !(2..=isize::MAX as usize).contains(&max) {
            return Err(NumericReject::RefcountOverflow);
        }
        Ok(Self(max))
    }
}

#[repr(C)]
struct Header {
    refs: AtomicUsize,
    max_refs: usize,
    ready: AtomicBool,
    cancelled: AtomicBool,
    complete: AtomicBool,
    next: Cell<*mut Header>,
    future_offset: usize,
    original_layout: Layout,
    permit: ManuallyDrop<Grant<'static>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InlineLayout {
    pub backing: Layout,
    pub future_offset: usize,
    pub future: Layout,
}

impl InlineLayout {
    pub fn actual<F>() -> Result<Self, NumericReject> {
        let future = Layout::new::<F>();
        let (backing, future_offset) = extend(Layout::new::<Header>(), future)?;
        Ok(Self {
            backing,
            future_offset,
            future,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PollState {
    Idle,
    Pending,
    Complete,
    Cancelled,
}

pub struct InlineSessionOwner<F: Future> {
    header: NonNull<Header>,
    future: NonNull<F>,
    future_live: bool,
    not_sync: PhantomData<Cell<F>>,
}

/// 未发布 F 的析构根；失败时先销毁捕获图，再退还已持有的原 Grant。
struct PendingInstall<F> {
    future: Option<F>,
    permit: Option<Grant<'static>>,
}

impl<F> PendingInstall<F> {
    fn new(future: F, permit: Grant<'static>) -> Self {
        Self {
            future: Some(future),
            permit: Some(permit),
        }
    }
}

impl<F> Drop for PendingInstall<F> {
    fn drop(&mut self) {
        // 单次 F::drop unwind 后，字段中的 permit 仍在捕获图析构完后自动释放。
        drop(self.future.take());
        drop(self.permit.take());
    }
}

// SAFETY: future 只经 &mut owner poll/drop；Header 原子访问，静态账本跨 waker 存活。
unsafe impl<F: Future + Send> Send for InlineSessionOwner<F> {}

impl<F: Future> InlineSessionOwner<F> {
    /// F 已在栈上构造；此函数在任何 heap allocation 前取得实际 F+header 额度。
    /// F 构造前已有的 owned captures 必须各自带 root，不能事后归入本 grant。
    pub fn try_install(
        future: F,
        ledger: &'static ProcessLedger,
        admission: Admission,
        holders: HolderBound,
    ) -> Result<Self, NumericReject> {
        let model = InlineLayout::actual::<F>()?;
        let permit = ledger.try_reserve(
            Amount {
                rust_bytes: model.backing.size(),
                c_bytes: 0,
                slots: 1,
            },
            ResourceClass::Runtime,
            admission,
        )?;
        Self::install(PendingInstall::new(future, permit), model, holders)
    }

    /// 消费 caller 已在同一账本准入的 move-only Runtime Grant，不退额/拆分/重新预约。
    /// permit 须覆盖 actual Header+F 的 Rust bytes，并至少持有一个 Runtime slot；
    /// C bytes 不能替代 Rust bytes。所有额外已批准额度原样持到最后 header dealloc。
    ///
    /// F 构造前的 out-of-line captures 仍须各自带真实 root 和完整 holder 证明。
    /// caller 负责 data-before-owned、取消及 entry 最后准入线性化；本接口不读取
    /// 当前 epoch/source，不凭旧 Grant 恢复新 entry。失效 epoch 的 accepted Grant
    /// 仍可被转移和正常清理。失败先销毁未发布 F/秘密，再退还原额度。
    pub fn try_install_reserved(
        future: F,
        permit: Grant<'static>,
        holders: HolderBound,
    ) -> Result<Self, NumericReject> {
        let pending = PendingInstall::new(future, permit);
        let model = InlineLayout::actual::<F>()?;
        Self::install(pending, model, holders)
    }

    fn install(
        mut pending: PendingInstall<F>,
        model: InlineLayout,
        holders: HolderBound,
    ) -> Result<Self, NumericReject> {
        let Some(permit) = pending.permit.as_ref() else {
            std::process::abort();
        };
        if permit.class() != ResourceClass::Runtime {
            return Err(NumericReject::UnsupportedEnvelope);
        }
        let amount = permit.amount();
        if amount.rust_bytes < model.backing.size() || amount.slots < 1 {
            return Err(NumericReject::RustBudgetExceeded);
        }
        // SAFETY: Header 使 checked layout 非零；alloc/dealloc 使用同 global allocator。
        let pointer = NonNull::new(unsafe { alloc(model.backing) })
            .ok_or(NumericReject::AllocationFailure)?;
        let Some(future) = pending.future.take() else {
            std::process::abort();
        };
        let Some(permit) = pending.permit.take() else {
            std::process::abort();
        };
        let header = pointer.cast::<Header>();
        // SAFETY: checked extend 保证 F 的对齐/范围；移动 F 后首次 pin，不再移动。
        let future_pointer = unsafe {
            NonNull::new_unchecked(pointer.as_ptr().add(model.future_offset).cast::<F>())
        };
        unsafe {
            header.as_ptr().write(Header {
                refs: AtomicUsize::new(1),
                max_refs: holders.0,
                ready: AtomicBool::new(true),
                cancelled: AtomicBool::new(false),
                complete: AtomicBool::new(false),
                original_layout: model.backing,
                next: Cell::new(std::ptr::null_mut()),
                future_offset: model.future_offset,
                permit: ManuallyDrop::new(permit),
            });
            future_pointer.as_ptr().write(future);
        }
        Ok(Self {
            header,
            future: future_pointer,
            future_live: true,
            not_sync: PhantomData,
        })
    }

    pub fn layout(&self) -> Layout {
        // SAFETY: owner 保留一个真实引用。
        unsafe { self.header.as_ref().original_layout }
    }

    pub fn is_complete(&self) -> bool {
        unsafe { self.header.as_ref().complete.load(Ordering::Acquire) }
    }

    pub fn cancel_handle(&self) -> Result<CancelHandle, NumericReject> {
        retain(self.header)?;
        Ok(CancelHandle {
            header: self.header,
        })
    }

    pub fn poll_if_ready(&mut self) -> PollState {
        if !self.future_live {
            return PollState::Complete;
        }
        let header = unsafe { self.header.as_ref() };
        if header.cancelled.load(Ordering::Acquire) {
            self.finish();
            return PollState::Cancelled;
        }
        if !header.ready.swap(false, Ordering::AcqRel) {
            return PollState::Idle;
        }
        if retain(self.header).is_err() {
            // 不构造一个少计数的 Waker，也不 wrap/泄漏；有效源式保证不可达。
            std::process::abort();
        }
        // SAFETY: retain 的一个引用精确移交 RawWaker，vtable 只访问 Header。
        let waker = unsafe { Waker::from_raw(raw(self.header)) };
        let mut context = Context::from_waker(&waker);
        // SAFETY: F 已在其最终 allocation pin；&mut owner 排除并发 poll/drop。
        match unsafe { Pin::new_unchecked(self.future.as_mut()) }.poll(&mut context) {
            Poll::Ready(output) => {
                // Complete 不等于 header free；F 和 output 都在本线程销毁。
                self.finish();
                drop(output);
                PollState::Complete
            }
            Poll::Pending => {
                if unsafe { self.header.as_ref().cancelled.load(Ordering::Acquire) } {
                    self.finish();
                    PollState::Cancelled
                } else {
                    PollState::Pending
                }
            }
        }
    }

    fn finish(&mut self) {
        if self.future_live {
            unsafe { self.header.as_ref().complete.store(true, Ordering::Release) };
            self.future_live = false;
            // SAFETY: 只调用一次，唯一 owner，cancel handle 从不访问 F。
            unsafe { std::ptr::drop_in_place(self.future.as_ptr()) };
        }
    }
}

impl<F: Future> Drop for InlineSessionOwner<F> {
    fn drop(&mut self) {
        // 即使 F drop unwind，也由 guard 释放 owner 引用；RawWaker 无 unwind 路径。
        let reference = HeaderReference(self.header);
        self.finish();
        drop(reference);
    }
}

pub struct CancelHandle {
    header: NonNull<Header>,
}
// SAFETY: 此 handle 仅操作 atomics 和 refcount，不访问 future。
unsafe impl Send for CancelHandle {}
// SAFETY: 同上，cancel 无可变非原子访问。
unsafe impl Sync for CancelHandle {}
impl CancelHandle {
    pub fn cancel(&self) {
        unsafe {
            self.header
                .as_ref()
                .cancelled
                .store(true, Ordering::Release);
            self.header.as_ref().ready.store(true, Ordering::Release);
        }
    }
}
impl Drop for CancelHandle {
    fn drop(&mut self) {
        release(self.header);
    }
}
struct HeaderReference(NonNull<Header>);
impl Drop for HeaderReference {
    fn drop(&mut self) {
        release(self.0);
    }
}

fn retain(pointer: NonNull<Header>) -> Result<(), NumericReject> {
    let header = unsafe { pointer.as_ref() };
    let mut current = header.refs.load(Ordering::Relaxed);
    loop {
        if current >= header.max_refs {
            return Err(NumericReject::RefcountOverflow);
        }
        match header.refs.compare_exchange_weak(
            current,
            current + 1,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => return Ok(()),
            Err(next) => current = next,
        }
    }
}
fn release(mut pointer: NonNull<Header>) {
    if unsafe { pointer.as_ref().refs.fetch_sub(1, Ordering::Release) } != 1 {
        return;
    }
    fence(Ordering::Acquire);
    // 最后 waker 不触及已销毁的 F；取栈上 permit 后 free 整块，再返额。
    let layout = unsafe { pointer.as_ref().original_layout };
    let permit = unsafe { ManuallyDrop::take(&mut pointer.as_mut().permit) };
    unsafe { dealloc(pointer.as_ptr().cast(), layout) };
    drop(permit);
}
fn raw(pointer: NonNull<Header>) -> RawWaker {
    RawWaker::new(pointer.as_ptr().cast(), &VTABLE)
}
unsafe fn clone_waker(data: *const ()) -> RawWaker {
    let pointer = unsafe { NonNull::new_unchecked(data.cast_mut().cast::<Header>()) };
    if retain(pointer).is_err() {
        std::process::abort();
    }
    raw(pointer)
}
unsafe fn wake_ref(data: *const ()) {
    let header = unsafe { &*data.cast::<Header>() };
    header.ready.store(true, Ordering::Release);
}
unsafe fn wake(data: *const ()) {
    unsafe {
        wake_ref(data);
        drop_waker(data);
    }
}
unsafe fn drop_waker(data: *const ()) {
    release(unsafe { NonNull::new_unchecked(data.cast_mut().cast::<Header>()) });
}
static VTABLE: RawWakerVTable = RawWakerVTable::new(clone_waker, wake, wake_ref, drop_waker);

/// 同一实际 run_prepared_session F 类型的 intrusive 轮转，不建 Vec/Box/heap queue。
/// list 只由父常驻 poller 访问，waker 仍只设置 ready 位；不加 helper 的 4 槽运行 cap。
pub struct InlinePollList<F: Future> {
    head: *mut Header,
    tail: *mut Header,
    len: usize,
    marker: PhantomData<Cell<F>>,
}
impl<F: Future> Default for InlinePollList<F> {
    fn default() -> Self {
        Self {
            head: std::ptr::null_mut(),
            tail: std::ptr::null_mut(),
            len: 0,
            marker: PhantomData,
        }
    }
}
// SAFETY: list 和所有 F 只有一个 poll owner，跨线程移动要求每个 F 可 Send。
unsafe impl<F: Future + Send> Send for InlinePollList<F> {}
impl<F: Future> InlinePollList<F> {
    pub fn push(&mut self, owner: InlineSessionOwner<F>) -> Result<(), InlineSessionOwner<F>> {
        let Some(len) = self.len.checked_add(1) else {
            return Err(owner);
        };
        let header = owner.header.as_ptr();
        unsafe {
            (*header).next.set(std::ptr::null_mut());
        }
        if self.tail.is_null() {
            self.head = header;
        } else {
            unsafe {
                (*self.tail).next.set(header);
            }
        }
        self.tail = header;
        self.len = len;
        std::mem::forget(owner);
        Ok(())
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    fn pop(&mut self) -> Option<InlineSessionOwner<F>> {
        let header = NonNull::new(self.head)?;
        self.head = unsafe { header.as_ref().next.get() };
        if self.head.is_null() {
            self.tail = std::ptr::null_mut();
        }
        self.len -= 1;
        unsafe {
            header.as_ref().next.set(std::ptr::null_mut());
        }
        let future = unsafe {
            NonNull::new_unchecked(
                header
                    .as_ptr()
                    .cast::<u8>()
                    .add(header.as_ref().future_offset)
                    .cast::<F>(),
            )
        };
        let future_live = !unsafe { header.as_ref().complete.load(Ordering::Acquire) };
        Some(InlineSessionOwner {
            header,
            future,
            future_live,
            not_sync: PhantomData,
        })
    }
    /// 最多 poll visits 个不同节点，余下及重复 wake 留在下一轮；Complete 从列表取走。
    pub fn poll_round(&mut self, visits: usize) -> usize {
        let limit = visits.min(self.len);
        let mut finished = 0;
        for _ in 0..limit {
            let Some(mut owner) = self.pop() else {
                break;
            };
            match owner.poll_if_ready() {
                PollState::Complete | PollState::Cancelled => {
                    drop(owner);
                    finished += 1;
                }
                PollState::Pending | PollState::Idle => {
                    // pop 后恢复同一计数，不可能 overflow；不丢弃存活 owner。
                    if let Err(owner) = self.push(owner) {
                        drop(owner);
                        std::process::abort();
                    }
                }
            }
        }
        finished
    }
}
impl<F: Future> Drop for InlinePollList<F> {
    fn drop(&mut self) {
        while let Some(owner) = self.pop() {
            let mut remainder = DrainOnUnwind {
                list: self,
                armed: true,
            };
            drop(owner);
            remainder.armed = false;
        }
    }
}

struct DrainOnUnwind<'a, F: Future> {
    list: &'a mut InlinePollList<F>,
    armed: bool,
}
impl<F: Future> Drop for DrainOnUnwind<'_, F> {
    fn drop(&mut self) {
        if self.armed {
            // 单次 F::drop unwind 仍销毁其余真实 roots；再次 panic 遵循 Rust 双 panic abort。
            while let Some(owner) = self.list.pop() {
                drop(owner);
            }
        }
    }
}

#[cfg(test)]
#[path = "reserved_tests.rs"]
mod reserved_tests;
