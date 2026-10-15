//! 已收费原生线程单元；DNS/signer 的闭包、结果和服务槽由父完成后再接入。
//! 不提供未证明的 std resolver.collect 或 spawn_blocking 回退。

#[cfg(windows)]
mod windows_job {
    use std::alloc::Layout;
    use std::cell::{Cell, UnsafeCell};
    use std::marker::PhantomData;
    use std::ptr;
    use std::sync::atomic::{AtomicBool, Ordering};

    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        CreateThread, INFINITE, TLS_OUT_OF_INDEXES, TlsAlloc, TlsFree, TlsGetValue, TlsSetValue,
        WaitForSingleObject,
    };

    use crate::selected_p1::model::layout::NumericReject;
    use crate::selected_p1::resource::ledger::Grant;
    use crate::selected_p1::resource::owner::{OwnerLayout, RcMemberLayout, SharedOwner};

    use super::super::{AppScope, RuntimeFault, Site};

    struct TlsSlot(u32);
    impl Drop for TlsSlot {
        fn drop(&mut self) {
            // SAFETY: 独占成功取得的槽；ThreadJob 的 OS join 先于 owner/TLS 析构。
            unsafe {
                TlsFree(self.0);
            }
        }
    }

    struct JobRecord<F, R> {
        cancelled: AtomicBool,
        ready: AtomicBool,
        job: UnsafeCell<Option<F>>,
        result: UnsafeCell<Option<R>>,
        tls: TlsSlot,
    }
    // SAFETY: F 只由唯一 OS 线程 take/call；R 在 ready Release 之前唯一写入。
    // 其他 holder 只读取消原子；结果只在 OS join（包含 TLS 清理/线程退出）之后借用。
    unsafe impl<F: Send, R: Send> Sync for JobRecord<F, R> {}

    /// closure/result 都内嵌在 actual SharedOwner<JobRecord<F,R>>，不擦除 F 为 dyn 指针。
    pub struct ThreadJob<F: Send + 'static, R: Send + 'static> {
        owner: SharedOwner<'static, JobRecord<F, R>>,
        handle: HANDLE,
        joined: bool,
        unique: PhantomData<Cell<()>>,
    }

    impl<F: FnOnce(&AtomicBool, bool) -> R + Send + 'static, R: Send + 'static> ThreadJob<F, R> {
        pub fn layout() -> Result<Layout, RuntimeFault> {
            OwnerLayout::actual::<JobRecord<F, R>>(0)
                .map(|model| model.backing)
                .map_err(|reason| RuntimeFault::numeric(Site::ThreadJob, reason))
        }

        /// # Safety
        /// job 不 unwind，不调用 std thread/任意未收费 Rust TLS，也不产生未收费外置分配。
        /// 捕获/返回的 out-of-line 图必须预先有实际 owner；本 grant 只涵盖 inline F/R/header。
        /// job 存活时模块不得卸载；调用者的服务须另有已收费的固定 job/result 槽。
        /// 当前源式仅闭合原生线程启动及显式 Win32 TLS，不签 DNS 或 signer 调用闭包。
        pub unsafe fn try_start(scope: AppScope, job: F) -> Result<Self, RuntimeFault> {
            let fault = Cell::new(None);
            let owner = scope
                .own(Site::ThreadJob, 1, || {
                    // SAFETY: OS TLS index 无 Rust heap allocation；失败未创建线程。
                    let slot = unsafe { TlsAlloc() };
                    if slot == TLS_OUT_OF_INDEXES {
                        fault.set(Some(os_error()));
                        return Err(NumericReject::ConstructionFailure);
                    }
                    Ok(JobRecord {
                        cancelled: AtomicBool::new(false),
                        ready: AtomicBool::new(false),
                        job: UnsafeCell::new(Some(job)),
                        result: UnsafeCell::new(None),
                        tls: TlsSlot(slot),
                    })
                })
                .map_err(|error| fault.get().unwrap_or(error))?;
            let thread_owner = owner
                .try_clone()
                .map_err(|reason| RuntimeFault::numeric(Site::ThreadJob, reason))?;
            let raw = thread_owner.into_raw();
            // SAFETY: raw 移交一个真实 strong 引用；泛型 callback 与原 F/R 类型一致。
            // 不用 std::thread::spawn，因其 thread/Packet/Box/TLS 不是此源式的零成本项。
            let handle = unsafe {
                CreateThread(
                    ptr::null(),
                    0,
                    Some(run::<F, R>),
                    raw.cast(),
                    0,
                    ptr::null_mut(),
                )
            };
            if handle.is_null() {
                let error = os_error();
                // SAFETY: CreateThread 失败，没有 callback 消费 raw，恰好收回该引用。
                drop(unsafe { SharedOwner::<JobRecord<F, R>>::from_raw(raw) });
                return Err(error);
            }
            Ok(Self {
                owner,
                handle,
                joined: false,
                unique: PhantomData,
            })
        }

        pub fn cancel(&self) {
            self.owner.value().cancelled.store(true, Ordering::Release);
        }

        /// ready 只表示 R 已写入，不表示线程/TLS/root 已退出或可以退款。
        pub fn result_ready(&self) -> bool {
            self.owner.value().ready.load(Ordering::Acquire)
        }

        pub fn join(&mut self) -> Result<(), RuntimeFault> {
            join(self.handle, &mut self.joined)
        }

        pub fn with_result<U>(&self, inspect: impl FnOnce(&R) -> U) -> Option<U> {
            if !self.joined || self.owner.value().cancelled.load(Ordering::Acquire) {
                return None;
            }
            // SAFETY: OS 线程已退出，没有并发写；R 始终由 owner root 保持，不移出裸结果。
            unsafe { (&*self.owner.value().result.get()).as_ref().map(inspect) }
        }
    }

    fn os_error() -> RuntimeFault {
        RuntimeFault::Os {
            site: Site::ThreadJob,
            code: unsafe { GetLastError() } as i32,
        }
    }

    fn join(handle: HANDLE, joined: &mut bool) -> Result<(), RuntimeFault> {
        if *joined {
            return Ok(());
        }
        // SAFETY: 独占有效线程 handle；无限等待不是 timeout/free 的替代。
        if unsafe { WaitForSingleObject(handle, INFINITE) } != WAIT_OBJECT_0 {
            return Err(os_error());
        }
        *joined = true;
        Ok(())
    }

    impl<F: Send + 'static, R: Send + 'static> Drop for ThreadJob<F, R> {
        fn drop(&mut self) {
            self.owner.value().cancelled.store(true, Ordering::Release);
            if join(self.handle, &mut self.joined).is_err() {
                // 有效独占 handle 的无限 wait 不应失败；不能在线程仍存活时退额或卸载。
                std::process::abort();
            }
            // SAFETY: 已真正退出的 OS 线程，独占 handle 恰好 close 一次。
            unsafe {
                CloseHandle(self.handle);
            }
            // 随后字段 owner drop：R/F/TLS slot → backing dealloc → grant refund。
        }
    }

    unsafe extern "system" fn run<
        F: FnOnce(&AtomicBool, bool) -> R + Send + 'static,
        R: Send + 'static,
    >(
        raw: *mut core::ffi::c_void,
    ) -> u32 {
        // SAFETY: CreateThread 移交的单个引用；JobRecord 由额外 join owner 保持到实际线程退出。
        let owner = unsafe {
            SharedOwner::<JobRecord<F, R>>::from_raw(raw.cast::<RcMemberLayout<Grant<'static>>>())
        };
        let record = owner.value();
        // SAFETY: 独占 TLS slot；借用的 record 至少由 thread_owner 和 join owner 两根保持。
        if unsafe { TlsSetValue(record.tls.0, raw) } == 0 {
            std::process::abort();
        }
        let attached = unsafe { TlsGetValue(record.tls.0) } == raw;
        // SAFETY: 该 OS callback 是 F/R 的唯一写者；调用者已证明 closure 不 unwind。
        let Some(job) = (unsafe { &mut *record.job.get() }).take() else {
            std::process::abort();
        };
        let result = job(&record.cancelled, attached);
        unsafe {
            *record.result.get() = Some(result);
        }
        record.ready.store(true, Ordering::Release);
        // SAFETY: 清除自己的 TLS value，不销毁 owner 或取消 OS join 的最后根。
        if unsafe { TlsSetValue(record.tls.0, ptr::null()) } == 0 {
            std::process::abort();
        }
        drop(owner);
        0
    }
}

#[cfg(windows)]
pub use windows_job::ThreadJob;
