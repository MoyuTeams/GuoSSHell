//! 独立 App runtime 与稳定 S 回归；不注册生产模块，不访问真实认证或用户配置。

#[path = "selected_p1_foundation.rs"]
mod foundation;
pub use foundation::{audit, capacity, certificate, layout, ledger, owner, secret};

#[path = "../src/selected_p1/session_owner/mod.rs"]
pub mod session_owner;
pub use session_owner::inline;
pub mod selected_p1 {
    pub use crate::foundation::selected_p1::{model, resource};
    pub use crate::session_owner;
}

#[path = "../src/selected_p1/network_runtime/mod.rs"]
pub mod network_runtime;

#[cfg(windows)]
mod network_tests {
    use std::cell::{Cell, RefCell};
    use std::future::{Future, poll_fn};
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, TcpListener, TcpStream};
    use std::num::{NonZeroU32, NonZeroUsize};
    use std::pin::Pin;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Poll, Waker};
    use std::time::{Duration, Instant};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use crate::inline::{HolderBound, InlineLayout, InlineSessionOwner, PollState};
    use crate::ledger::{
        Admission, Amount, BYTE_LIMIT, CLASS_COUNT, ProcessLedger, ReservationReject, ResourceClass,
    };
    use crate::network_runtime::blocking::ThreadJob;
    use crate::network_runtime::executor::{BoundedExecutor, TaskAdmission};
    use crate::network_runtime::{
        AppScope, Cooperative, NativeStream, NetworkRuntime, RuntimeFault, Site, Sleep,
    };
    use crate::{audit::AuditScope, layout::NumericReject};

    static SERIAL: Mutex<()> = Mutex::new(());
    static ROOT: ProcessLedger = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
    static ZERO: ProcessLedger = ProcessLedger::new(0, [u32::MAX; CLASS_COUNT]);
    static NO_IO_SLOTS: ProcessLedger = {
        let mut limits = [u32::MAX; CLASS_COUNT];
        limits[ResourceClass::Runtime as usize] = 0;
        ProcessLedger::new(BYTE_LIMIT, limits)
    };
    type Runtime = NetworkRuntime<(), 16, 16>;
    type Stream = NativeStream<(), 16, 16>;

    fn scope() -> AppScope {
        AppScope {
            ledger: &ROOT,
            admission: Admission {
                epoch: ROOT.snapshot().epoch,
                source: None,
            },
        }
    }
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    fn numeric(reason: NumericReject) -> RuntimeFault {
        RuntimeFault::Numeric {
            site: Site::Task,
            reason,
        }
    }
    fn actual<F>(_: &F) -> Result<InlineLayout, RuntimeFault> {
        InlineLayout::actual::<F>().map_err(numeric)
    }
    fn thread_layout<F, R>(_: &F) -> Result<std::alloc::Layout, RuntimeFault>
    where
        F: FnOnce(&AtomicBool, bool) -> R + Send + 'static,
        R: Send + 'static,
    {
        ThreadJob::<F, R>::layout()
    }
    fn os(error: std::io::Error) -> RuntimeFault {
        RuntimeFault::Os {
            site: Site::Socket,
            code: error.raw_os_error().unwrap_or(-1),
        }
    }

    /// 仅以下实际闭包：owner+Context+cancel+已用 IO/timer 槽+外部保留+注册替换临时 clone。
    /// 不把此证明传给正常 SSH；每个测试逐项传入其实际图，而非固定 fixture 常数。
    fn holders(
        cancel: usize,
        io: usize,
        timer: usize,
        external: usize,
    ) -> Result<HolderBound, RuntimeFault> {
        let replacement = usize::from(io + timer != 0);
        // SAFETY: 下面的有限 F 不使用库 channel/split/broker，只有明确列出的 waker 保存点。
        unsafe { HolderBound::from_source_proof(2 + cancel + io + timer + external + replacement) }
            .map_err(numeric)
    }

    fn budget(limit: u32) -> Result<Cooperative, RuntimeFault> {
        Cooperative::try_new(
            scope(),
            NonZeroU32::new(limit).ok_or(numeric(NumericReject::ConstructionFailure))?,
        )
    }

    fn connected(runtime: &mut Runtime) -> Result<(Stream, TcpStream), RuntimeFault> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(os)?;
        listener.set_nonblocking(true).map_err(os)?;
        let address = listener.local_addr().map_err(os)?;
        let handle = runtime.handle()?;
        let mut connect = handle.connect(address, budget(256)?)?;
        let mut cx = Context::from_waker(Waker::noop());
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut accepted = None;
        let stream = loop {
            match Pin::new(&mut connect).poll(&mut cx) {
                Poll::Ready(result) => break result?,
                Poll::Pending => {
                    runtime.drive(Duration::from_millis(1))?;
                }
            }
            if accepted.is_none() {
                match listener.accept() {
                    Ok((stream, _)) => accepted = Some(stream),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(error) => return Err(os(error)),
                }
            }
            assert!(
                Instant::now() < deadline,
                "run-owned loopback connect 未完成"
            );
        };
        let peer = if let Some(peer) = accepted {
            peer
        } else {
            listener.accept().map_err(os)?.0
        };
        peer.set_read_timeout(Some(Duration::from_secs(2)))
            .map_err(os)?;
        peer.set_write_timeout(Some(Duration::from_secs(2)))
            .map_err(os)?;
        assert_eq!(stream.peer()?.peer, address);
        assert_eq!(stream.peer()?.local, peer.peer_addr().map_err(os)?);
        Ok((stream, peer))
    }

    fn verify_allocations(report: &crate::audit::Report) {
        assert!(!report.overflow);
        let mut live = 0;
        for event in report.events() {
            match event.kind {
                1 => live += event.size,
                2 => {
                    assert!(event.held_rust >= live);
                    live -= event.size;
                }
                4 => assert!(event.held_rust >= event.size),
                _ => panic!("该源式不含 realloc 或其他 allocator 操作"),
            }
            assert!(event.held_rust >= live);
        }
        assert_eq!(live, 0);
    }

    #[test]
    fn runtime_real_peer_io_traits_without_ambient_tokio_and_exact_backings()
    -> Result<(), RuntimeFault> {
        let _serial = serial();
        assert!(tokio::runtime::Handle::try_current().is_err());
        assert_eq!(ROOT.snapshot().rust_bytes, 0);
        let (binding, registry) = Runtime::layouts()?;
        let io = Stream::layout()?;
        let coop = Cooperative::layout()?;
        let audit = AuditScope::start(&ROOT);
        let mut runtime = Runtime::try_new(scope(), ())?;
        assert_eq!(ROOT.snapshot().rust_bytes, binding.size() + registry.size());
        let (mut stream, mut peer) = connected(&mut runtime)?;
        assert_eq!(
            ROOT.snapshot().rust_bytes,
            binding.size() + registry.size() + io.size() + coop.size()
        );
        let done = Cell::new(false);
        let future = async {
            stream.write_all(b"native-runtime").await.map_err(os)?;
            let mut echoed = [0u8; 4];
            stream.read_exact(&mut echoed).await.map_err(os)?;
            assert_eq!(&echoed, b"peer");
            done.set(true);
            Ok::<(), RuntimeFault>(())
        };
        // 此 F 在本测试无完成结果外置 allocation，输出由显式检查闭包转为 ()。
        let future = async {
            assert_eq!(future.await, Ok(()));
        };
        let actual = actual(&future)?;
        let mut owner =
            InlineSessionOwner::try_install(future, &ROOT, scope().admission, holders(0, 1, 0, 0)?)
                .map_err(numeric)?;
        assert_eq!(owner.layout(), actual.backing);
        assert_eq!(owner.poll_if_ready(), PollState::Pending);
        let mut bytes = [0u8; 14];
        peer.read_exact(&mut bytes).map_err(os)?;
        assert_eq!(&bytes, b"native-runtime");
        peer.write_all(b"peer").map_err(os)?;
        let deadline = Instant::now() + Duration::from_secs(2);
        while !done.get() {
            runtime.drive(Duration::from_millis(1))?;
            owner.poll_if_ready();
            assert!(Instant::now() < deadline);
        }
        drop(owner);
        drop(stream);
        drop(peer);
        drop(runtime);
        verify_allocations(&audit.finish());
        assert_eq!(ROOT.snapshot().rust_bytes, 0);
        assert!(tokio::runtime::Handle::try_current().is_err());
        println!(
            "runtime binding={binding:?}, registry={registry:?}, socket={io:?}, cooperative={coop:?}, actual loopback F={actual:?}"
        );
        Ok(())
    }

    #[test]
    fn blocked_read_cancel_late_event_and_waker_refund_only_after_actual_free()
    -> Result<(), RuntimeFault> {
        let _serial = serial();
        let audit = AuditScope::start(&ROOT);
        let mut runtime = Runtime::try_new(scope(), ())?;
        let baseline = ROOT.snapshot().rust_bytes;
        let (mut stream, mut peer) = connected(&mut runtime)?;
        let external = RefCell::new(None::<Waker>);
        let reached = Cell::new(false);
        let future = async {
            poll_fn(|cx| {
                *external.borrow_mut() = Some(cx.waker().clone());
                Poll::Ready(())
            })
            .await;
            reached.set(true);
            let mut byte = [0];
            let _ = stream.read(&mut byte).await;
        };
        let mut owner =
            InlineSessionOwner::try_install(future, &ROOT, scope().admission, holders(1, 1, 0, 1)?)
                .map_err(numeric)?;
        let header = owner.layout().size();
        let cancel = owner.cancel_handle().map_err(numeric)?;
        assert_eq!(owner.poll_if_ready(), PollState::Pending);
        assert!(reached.get());
        peer.write_all(b"late").map_err(os)?;
        let mut batch = runtime.collect_ready(Duration::from_millis(10))?;
        let event = batch
            .take(0)
            .ok_or(numeric(NumericReject::ConstructionFailure))?;
        drop(batch);
        cancel.cancel();
        assert_eq!(owner.poll_if_ready(), PollState::Cancelled);
        drop(cancel);
        drop(owner);
        assert_eq!(stream.registered_waiters(), (true, false));
        stream.cancel();
        drop(stream);
        assert_eq!(
            ROOT.snapshot().rust_bytes,
            baseline + Stream::layout()?.size() + header
        );
        assert!(!event.deliver());
        assert_eq!(ROOT.snapshot().rust_bytes, baseline + header);
        if let Some(waker) = external.borrow_mut().take() {
            waker.wake_by_ref();
            drop(waker);
        }
        assert_eq!(ROOT.snapshot().rust_bytes, baseline);
        drop(peer);
        drop(runtime);
        verify_allocations(&audit.finish());
        assert_eq!(ROOT.snapshot().rust_bytes, 0);
        println!("实际 timer backing={:?}", Sleep::<(), 16, 16>::layout()?);
        Ok(())
    }

    #[test]
    fn real_tcp_write_backpressure_cancel_uses_reserved_control_not_data_slots()
    -> Result<(), RuntimeFault> {
        let _serial = serial();
        let mut runtime = Runtime::try_new(scope(), ())?;
        let (mut stream, peer) = connected(&mut runtime)?;
        stream.set_send_buffer(4096)?;
        let cancel_io = stream.cancel_handle()?;
        let cooperative = stream.cooperative()?;
        let bytes = [7u8; 16 * 1024];
        let written = Cell::new(0usize);
        let blocked = Cell::new(false);
        let future = poll_fn(|cx| {
            for _ in 0..32 {
                match stream.poll_write_scoped(cx, &bytes) {
                    Poll::Ready(Ok(count)) => written.set(written.get() + count),
                    Poll::Ready(Err(fault)) => panic!("真实 write 错误: {fault:?}"),
                    Poll::Pending => {
                        blocked.set(stream.registered_waiters().1);
                        return Poll::<()>::Pending;
                    }
                }
            }
            cx.waker().wake_by_ref();
            Poll::Pending
        });
        // F 每轮限制 32 次写，wrapper 重置同一 stream 预算，停在真实 OS WouldBlock。
        let mut owner = InlineSessionOwner::try_install(
            cooperative.bind(future),
            &ROOT,
            scope().admission,
            holders(1, 1, 0, 0)?,
        )
        .map_err(numeric)?;
        let cancel_task = owner.cancel_handle().map_err(numeric)?;
        let deadline = Instant::now() + Duration::from_secs(2);
        while !blocked.get() {
            owner.poll_if_ready();
            assert!(
                Instant::now() < deadline,
                "必须观察真正 TCP WouldBlock，而非 coop Pending"
            );
        }
        assert!(written.get() > 0);
        // 不消费任务/IO/消息槽；数据被背压时仍可执行 cancel/cleanup。
        cancel_io.cancel();
        cancel_task.cancel();
        assert_eq!(owner.poll_if_ready(), PollState::Cancelled);
        drop(cancel_task);
        drop(owner);
        drop(cancel_io);
        drop(stream);
        drop(peer);
        drop(runtime);
        assert_eq!(ROOT.snapshot().rust_bytes, 0);
        Ok(())
    }

    #[test]
    fn monotonic_timer_expires_resets_cancels_and_keeps_late_cancel_root()
    -> Result<(), RuntimeFault> {
        let _serial = serial();
        let audit = AuditScope::start(&ROOT);
        let mut runtime = Runtime::try_new(scope(), ())?;
        let baseline = ROOT.snapshot().rust_bytes;
        let handle = runtime.handle()?;
        let mut sleep = handle.sleep_until(Instant::now() + Duration::from_secs(30))?;
        let timer_bytes = Sleep::<(), 16, 16>::layout()?.size();
        let mut cx = Context::from_waker(Waker::noop());
        assert!(Pin::new(&mut sleep).poll(&mut cx).is_pending());
        sleep.reset(Instant::now() + Duration::from_millis(15))?;
        let deadline = Instant::now() + Duration::from_secs(2);
        while Pin::new(&mut sleep).poll(&mut cx).is_pending() {
            runtime.drive(Duration::from_millis(10))?;
            assert!(Instant::now() < deadline);
        }
        sleep.reset(Instant::now() + Duration::from_secs(30))?;
        assert!(Pin::new(&mut sleep).poll(&mut cx).is_pending());
        let cancel = sleep.cancel_handle()?;
        cancel.cancel();
        assert_eq!(
            Pin::new(&mut sleep).poll(&mut cx),
            Poll::Ready(Err(RuntimeFault::Cancelled))
        );
        assert_eq!(sleep.reset(Instant::now()), Err(RuntimeFault::Cancelled));
        drop(sleep);
        assert_eq!(ROOT.snapshot().rust_bytes, baseline + timer_bytes);
        drop(cancel);
        assert_eq!(ROOT.snapshot().rust_bytes, baseline);
        drop(handle);
        drop(runtime);
        verify_allocations(&audit.finish());
        assert_eq!(ROOT.snapshot().rust_bytes, 0);
        Ok(())
    }

    struct TimerFuture<'a> {
        sleep: Sleep<(), 1, 40>,
        done: &'a Cell<bool>,
    }
    impl Future for TimerFuture<'_> {
        type Output = ();
        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            match Pin::new(&mut self.sleep).poll(cx) {
                Poll::Ready(Ok(())) => {
                    self.done.set(true);
                    Poll::Ready(())
                }
                Poll::Ready(Err(fault)) => panic!("timer 错误: {fault:?}"),
                Poll::Pending => Poll::Pending,
            }
        }
    }

    #[test]
    fn timer_driver_wakes_real_tasks_and_rotates_beyond_one_os_batch() -> Result<(), RuntimeFault> {
        let _serial = serial();
        let audit = AuditScope::start(&ROOT);
        let mut runtime = NetworkRuntime::<(), 1, 40>::try_new(scope(), ())?;
        let handle = runtime.handle()?;
        let flags: [Cell<bool>; 40] = std::array::from_fn(|_| Cell::new(false));
        let mut executor = BoundedExecutor::new(
            NonZeroUsize::new(40).ok_or(numeric(NumericReject::ConstructionFailure))?,
        );
        let deadline = Instant::now() + Duration::from_millis(30);
        for done in &flags {
            let sleep = handle.sleep_until(deadline)?;
            let owner = InlineSessionOwner::try_install(
                TimerFuture { sleep, done },
                &ROOT,
                scope().admission,
                holders(0, 0, 1, 0)?,
            )
            .map_err(numeric)?;
            assert!(executor.enqueue(owner).is_ok());
        }
        executor.poll_round();
        assert!(flags.iter().all(|flag| !flag.get()));
        // 没有 IO/socket，单调 timer 独立唤醒 actual S waker；超过 BATCH 的注册会轮转到达。
        let timeout = Instant::now() + Duration::from_secs(2);
        while !executor.is_empty() {
            runtime.drive(Duration::from_millis(10))?;
            executor.poll_round();
            assert!(Instant::now() < timeout);
        }
        assert!(flags.iter().all(Cell::get));
        drop(executor);
        drop(handle);
        drop(runtime);
        verify_allocations(&audit.finish());
        assert_eq!(ROOT.snapshot().rust_bytes, 0);
        Ok(())
    }

    struct FairFuture<'a> {
        id: usize,
        polls: &'a RefCell<[usize; 2]>,
        cooperative: Cooperative,
    }
    impl Future for FairFuture<'_> {
        type Output = ();
        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            let this = self.get_mut();
            this.polls.borrow_mut()[this.id] += 1;
            for _ in 0..100 {
                cx.waker().wake_by_ref();
                if this.cooperative.poll_work(cx, NonZeroU32::MIN).is_pending() {
                    return Poll::Pending;
                }
            }
            panic!("实际预算应先让出");
        }
    }

    #[test]
    fn repeated_wakes_and_continuously_ready_work_rotate_without_starvation()
    -> Result<(), RuntimeFault> {
        let _serial = serial();
        let audit = AuditScope::start(&ROOT);
        let polls = RefCell::new([0; 2]);
        let mut executor = BoundedExecutor::new(NonZeroUsize::MIN);
        for id in 0..2 {
            let cooperative = budget(3)?;
            let outer = cooperative.try_clone()?;
            let future = outer.bind(FairFuture {
                id,
                polls: &polls,
                cooperative,
            });
            let owner = InlineSessionOwner::try_install(
                future,
                &ROOT,
                scope().admission,
                holders(0, 0, 0, 0)?,
            )
            .map_err(numeric)?;
            assert!(executor.enqueue(owner).is_ok());
        }
        for _ in 0..40 {
            executor.poll_round();
        }
        assert_eq!(*polls.borrow(), [20, 20]);
        drop(executor);
        verify_allocations(&audit.finish());
        assert_eq!(ROOT.snapshot().rust_bytes, 0);
        Ok(())
    }

    #[test]
    fn typed_admission_before_allocation_null_and_actual_registry_capacity()
    -> Result<(), RuntimeFault> {
        let _serial = serial();
        let zero_scope = AppScope {
            ledger: &ZERO,
            admission: Admission {
                epoch: 1,
                source: None,
            },
        };
        let audit = AuditScope::start(&ZERO);
        let expected = Runtime::layouts()?.0.size();
        assert_eq!(
            Runtime::try_new(zero_scope, ()).err(),
            Some(RuntimeFault::Admission {
                site: Site::Binding,
                reason: ReservationReject::ByteLimit {
                    requested: expected,
                    current: 0,
                    limit: 0
                },
            })
        );
        assert!(audit.finish().events().is_empty());
        let future = async {};
        let audit = AuditScope::start(&ZERO);
        assert!(matches!(
            TaskAdmission::try_new(zero_scope, future, holders(0, 0, 0, 0)?).err(),
            Some(RuntimeFault::Admission {
                site: Site::Task,
                reason: ReservationReject::ByteLimit { .. }
            })
        ));
        assert!(audit.finish().events().is_empty());
        let audit = AuditScope::start(&ROOT);
        audit.fail_next_size(expected);
        assert_eq!(
            Runtime::try_new(scope(), ()).err(),
            Some(RuntimeFault::Numeric {
                site: Site::Binding,
                reason: NumericReject::AllocationFailure
            })
        );
        let report = audit.finish();
        assert_eq!(report.events().len(), 1);
        assert_eq!(report.events()[0].kind, 4);
        assert_eq!(ROOT.snapshot().rust_bytes, 0);
        let mut runtime = NetworkRuntime::<(), 1, 1>::try_new(scope(), ())?;
        let handle = runtime.handle()?;
        let sleep = handle.sleep_until(Instant::now() + Duration::from_secs(30))?;
        let audit = AuditScope::start(&ROOT);
        assert_eq!(
            handle.sleep_until(Instant::now()).err(),
            Some(RuntimeFault::RegistryFull {
                site: Site::Timer,
                capacity: 1
            })
        );
        assert!(audit.finish().events().is_empty());
        drop(sleep);
        let next = handle.sleep_until(Instant::now())?;
        drop(next);
        drop(handle);
        runtime.close();
        drop(runtime);
        assert_eq!(ROOT.snapshot().rust_bytes, 0);
        Ok(())
    }

    #[test]
    fn io_slot_rejection_is_typed_and_precedes_socket_root_allocation() -> Result<(), RuntimeFault>
    {
        let _serial = serial();
        let scope = AppScope {
            ledger: &NO_IO_SLOTS,
            admission: Admission {
                epoch: 1,
                source: None,
            },
        };
        let runtime = Runtime::try_new(scope, ())?;
        let handle = runtime.handle()?;
        let cooperative = Cooperative::try_new(scope, NonZeroU32::MIN)?;
        let address = std::net::SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
        let audit = AuditScope::start(&NO_IO_SLOTS);
        assert_eq!(
            handle.connect(address, cooperative.try_clone()?).err(),
            Some(RuntimeFault::Admission {
                site: Site::IoRegistration,
                reason: ReservationReject::ClassSlots {
                    class: ResourceClass::Runtime,
                    requested: 1,
                    current: 0,
                    limit: 0
                },
            })
        );
        assert!(audit.finish().events().is_empty());
        drop(cooperative);
        drop(handle);
        drop(runtime);
        assert_eq!(NO_IO_SLOTS.snapshot().rust_bytes, 0);
        Ok(())
    }

    #[test]
    fn old_source_epoch_never_rebinds_and_real_foreign_backing_waits_for_last_holder()
    -> Result<(), RuntimeFault> {
        let _serial = serial();
        let epoch = ROOT.snapshot().epoch;
        let source = ROOT.open_source(epoch, 0).map_err(numeric)?;
        let admission = Admission {
            epoch,
            source: Some(source),
        };
        let model = crate::owner::OwnerLayout::actual::<u64>(0).map_err(numeric)?;
        let grant = ROOT
            .try_reserve_detailed(
                Amount {
                    rust_bytes: model.backing.size(),
                    c_bytes: 0,
                    slots: 1,
                },
                ResourceClass::Source,
                admission,
            )
            .map_err(|reason| RuntimeFault::Admission {
                site: Site::Binding,
                reason,
            })?;
        // 这里只证明真实 App owned scalar backing 的跨根持有，不把它标为 SourceF 字体证书。
        let foreign =
            crate::owner::SharedOwner::try_init(grant, 0, 0, |_| Ok(17u64)).map_err(numeric)?;
        let app_scope = AppScope {
            ledger: &ROOT,
            admission,
        };
        let mut runtime = NetworkRuntime::<_, 1, 1>::try_new(app_scope, foreign)?;
        let handle = runtime.handle()?;
        let sleep = handle.sleep_until(Instant::now() + Duration::from_secs(30))?;
        let cancel = sleep.cancel_handle()?;
        sleep.cancel();
        ROOT.invalidate_source(source).map_err(numeric)?;
        let replacement = ROOT.open_source(epoch, 0).map_err(numeric)?;
        assert!(matches!(
            handle.sleep_until(Instant::now()).err(),
            Some(RuntimeFault::Admission {
                site: Site::Timer,
                reason: ReservationReject::Numeric(NumericReject::SourceMismatch)
            })
        ));
        ROOT.invalidate_source(replacement).map_err(numeric)?;
        let held = ROOT.snapshot().rust_bytes;
        ROOT.advance_epoch(epoch).map_err(numeric)?;
        assert_eq!(ROOT.snapshot().rust_bytes, held);
        runtime.close();
        drop(runtime);
        drop(handle);
        drop(sleep);
        assert!(ROOT.snapshot().rust_bytes >= model.backing.size());
        cancel.cancel();
        drop(cancel);
        assert_eq!(ROOT.snapshot().rust_bytes, 0);
        Ok(())
    }

    static JOB_ENTERED: AtomicBool = AtomicBool::new(false);
    static JOB_RELEASE: AtomicBool = AtomicBool::new(false);
    static JOB_TLS: AtomicBool = AtomicBool::new(false);
    static JOB_CANCELLED: AtomicBool = AtomicBool::new(false);
    struct ReleaseJob;
    impl Drop for ReleaseJob {
        fn drop(&mut self) {
            JOB_RELEASE.store(true, Ordering::Release);
        }
    }
    fn owned_job(cancelled: &AtomicBool, tls_attached: bool) -> [u8; 17] {
        JOB_TLS.store(tls_attached, Ordering::Release);
        JOB_ENTERED.store(true, Ordering::Release);
        while !JOB_RELEASE.load(Ordering::Acquire) {
            // 本闭包只有原子和 OS Sleep，无 std thread context 或 Rust TLS allocation。
            unsafe {
                windows_sys::Win32::System::Threading::Sleep(1);
            }
        }
        JOB_CANCELLED.store(cancelled.load(Ordering::Acquire), Ordering::Release);
        [11; 17]
    }

    #[test]
    fn native_thread_cancel_result_and_tls_exit_do_not_refund_at_ready_or_join()
    -> Result<(), RuntimeFault> {
        let _serial = serial();
        JOB_ENTERED.store(false, Ordering::Release);
        JOB_RELEASE.store(false, Ordering::Release);
        JOB_TLS.store(false, Ordering::Release);
        JOB_CANCELLED.store(false, Ordering::Release);
        let audit = AuditScope::start(&ROOT);
        let captured = [3u8; 23];
        let function = move |cancelled: &AtomicBool, attached: bool| {
            let mut result = owned_job(cancelled, attached);
            result[0] = captured[0];
            result
        };
        let layout = thread_layout(&function)?;
        // SAFETY: owned_job 的完整调用图在上方，返回固定数组，无外置图、Rust TLS 或 unwind。
        // 本测试只有一个栈内 job/result 槽，ThreadJob 保持 join root 至真实 OS 退出。
        let mut job = unsafe { ThreadJob::try_start(scope(), function) }?;
        let _release_on_unwind = ReleaseJob;
        let deadline = Instant::now() + Duration::from_secs(2);
        while !JOB_ENTERED.load(Ordering::Acquire) {
            unsafe {
                windows_sys::Win32::System::Threading::Sleep(1);
            }
            assert!(Instant::now() < deadline);
        }
        assert!(JOB_TLS.load(Ordering::Acquire));
        job.cancel();
        assert_eq!(ROOT.snapshot().rust_bytes, layout.size());
        assert!(!job.result_ready());
        assert!(job.with_result(|_| ()).is_none());
        JOB_RELEASE.store(true, Ordering::Release);
        while !job.result_ready() {
            unsafe {
                windows_sys::Win32::System::Threading::Sleep(1);
            }
            assert!(Instant::now() < deadline);
        }
        assert_eq!(ROOT.snapshot().rust_bytes, layout.size());
        job.join()?;
        assert_eq!(job.with_result(|result| *result), None);
        assert!(JOB_CANCELLED.load(Ordering::Acquire));
        assert_eq!(ROOT.snapshot().rust_bytes, layout.size());
        drop(job);
        let report = audit.finish();
        assert!(report.symmetric_single_backing());
        assert!(
            report
                .events()
                .iter()
                .all(|event| event.held_rust == layout.size())
        );
        assert_eq!(ROOT.snapshot().rust_bytes, 0);
        println!("实际原生 closure/result/TLS owner backing={layout:?}");
        Ok(())
    }
}
