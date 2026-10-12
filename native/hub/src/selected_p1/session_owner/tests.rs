use std::alloc::Layout;
use std::cell::{Cell, RefCell};
use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Waker};

use rshell_m0::rshell_core::{
    AuthenticationKind, ConnectionId, ConnectionProfile, HostKeyPolicy, TerminalSize, TransportKind,
};
use rshell_m0::rshell_session::{KnownHostsVerifier, NativeSshTransport, TransportRequest};

use super::gates::{bounded_chan_upper, create_interaction_root, prepare_auth_plan};
use super::inline::{HolderBound, InlineLayout, InlineSessionOwner, PollState};
use super::native::{NativeFutureLayouts, UnconnectedNative};
use super::operation::{Opcode, OperationBound};
use crate::audit::AuditScope;
use crate::layout::NumericReject;
use crate::ledger::{Admission, Amount, BYTE_LIMIT, CLASS_COUNT, ProcessLedger, ResourceClass};
use crate::owner::{OwnerLayout, SharedOwner};
use crate::secret::{CopyValidity, LockedSecretString};

static SERIAL: Mutex<()> = Mutex::new(());
static ROOT: ProcessLedger = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
static ZERO_ROOT: ProcessLedger = ProcessLedger::new(0, [u32::MAX; CLASS_COUNT]);
fn admission() -> Admission {
    Admission {
        epoch: ROOT.snapshot().epoch,
        source: None,
    }
}
pub(super) fn verify_each_allocation(report: &crate::audit::Report) -> Result<(), NumericReject> {
    assert!(!report.overflow);
    let mut live = 0usize;
    for event in report.events() {
        match event.kind {
            1 => {
                live = live
                    .checked_add(event.size)
                    .ok_or(NumericReject::Overflow)?
            }
            2 => {
                assert!(
                    live <= event.held_rust,
                    "live={live}, held={}",
                    event.held_rust
                );
                live = live
                    .checked_sub(event.size)
                    .ok_or(NumericReject::ConstructionFailure)?;
            }
            3 => {
                assert!(
                    live.checked_add(event.size)
                        .is_some_and(|coexist| coexist <= event.held_rust)
                );
                live = live
                    .checked_sub(event.old_size)
                    .and_then(|n| n.checked_add(event.size))
                    .ok_or(NumericReject::ConstructionFailure)?;
            }
            4 => assert!(event.size <= event.held_rust),
            _ => return Err(NumericReject::ConstructionFailure),
        }
        assert!(live <= event.held_rust);
    }
    assert_eq!(live, 0);
    Ok(())
}
fn holders() -> Result<HolderBound, NumericReject> {
    // SAFETY: poll fixture 为 owner/context/cancel/固定外部及 IO 槽，总数<=8；
    // holder 边界 fixture 不 poll，为 owner 1 + cancel 7，没有不可见 holder。
    unsafe { HolderBound::from_source_proof(8) }
}
fn profile() -> ConnectionProfile {
    ConnectionProfile {
        id: ConnectionId(uuid::Uuid::from_u128(1)),
        group_id: None,
        name: "合成".into(),
        host: "127.0.0.1".into(),
        port: 22,
        username: "fixture".into(),
        transport: TransportKind::NativeSsh,
        authentication: AuthenticationKind::Password,
        credential_ref: None,
        identity_file: None,
        host_key_policy: HostKeyPolicy::Strict,
        remote_command: None,
        note: String::new(),
        position: 0,
        tags: Default::default(),
        terminal_profile_id: None,
        terminal_overrides: Default::default(),
    }
}

struct RetainFuture<'a> {
    external: &'a RefCell<Option<Waker>>,
    done: &'a Cell<bool>,
    drops: &'a Cell<u32>,
    secret: Option<super::gates::LockedSecretOperation<'static>>,
    source: Option<SharedOwner<'static, String>>,
}
impl Future for RetainFuture<'_> {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.external.borrow().is_none() {
            *self.external.borrow_mut() = Some(cx.waker().clone());
        }
        if self.done.get() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}
impl Drop for RetainFuture<'_> {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
        drop(self.secret.take());
        drop(self.source.take());
    }
}

#[test]
fn w1s_actual_secret_source_future_complete_waits_for_external_header() -> Result<(), NumericReject>
{
    let _serial = SERIAL
        .lock()
        .map_err(|_| NumericReject::ConstructionFailure)?;
    assert_eq!(ROOT.snapshot().rust_bytes, 0);
    let profile = profile();
    let external = RefCell::new(None);
    let done = Cell::new(false);
    let drops = Cell::new(0);
    let audit = AuditScope::start(&ROOT);
    audit.inspect_secret_free(17);
    let secret = LockedSecretString::try_copy(
        "synthetic-secret!",
        ROOT.try_reserve(
            Amount {
                rust_bytes: 17,
                c_bytes: 0,
                slots: 1,
            },
            ResourceClass::Secret,
            admission(),
        )?,
        &CopyValidity::default(),
    )?;
    let auth = prepare_auth_plan(&profile, secret, &ROOT, admission())?;
    assert!(auth.value().operation().is_ok());
    let source_layout = OwnerLayout::actual::<String>(4)?;
    let source_grant = ROOT.try_reserve(
        Amount {
            rust_bytes: source_layout.required_rust(6)?,
            c_bytes: 0,
            slots: 1,
        },
        ResourceClass::Source,
        admission(),
    )?;
    let source = SharedOwner::try_init(source_grant, 4, 6, |bytes| {
        bytes.fill(9);
        Ok("source".to_owned())
    })?;
    let retained_source = source.try_clone()?;
    let future = RetainFuture {
        external: &external,
        done: &done,
        drops: &drops,
        secret: Some(auth),
        source: Some(source),
    };
    let model = InlineLayout::actual::<RetainFuture<'_>>()?;
    let mut owner = InlineSessionOwner::try_install(future, &ROOT, admission(), holders()?)?;
    assert_eq!(owner.layout(), model.backing);
    assert_eq!(owner.poll_if_ready(), PollState::Pending);
    assert_eq!(owner.poll_if_ready(), PollState::Idle);
    done.set(true);
    external
        .borrow()
        .as_ref()
        .ok_or(NumericReject::ConstructionFailure)?
        .wake_by_ref();
    assert_eq!(owner.poll_if_ready(), PollState::Complete);
    assert!(owner.is_complete());
    assert_eq!(drops.get(), 1);
    drop(owner);
    assert_eq!(
        ROOT.snapshot().rust_bytes,
        model.backing.size() + source_layout.required_rust(6)?
    );
    drop(retained_source);
    assert_eq!(ROOT.snapshot().rust_bytes, model.backing.size());
    // 迟到回调只设置 header ready，不访问已销毁 F。
    let late = external
        .borrow_mut()
        .take()
        .ok_or(NumericReject::ConstructionFailure)?;
    late.wake_by_ref();
    drop(late);
    assert_eq!(ROOT.snapshot().rust_bytes, 0);
    let report = audit.finish();
    verify_each_allocation(&report)?;
    let frees: Vec<_> = report
        .events()
        .iter()
        .filter(|event| event.kind == 2)
        .collect();
    assert!(
        frees
            .iter()
            .any(|event| event.size == 17 && event.zero_before_free && event.held_rust >= 17)
    );
    assert_eq!(
        frees.last().map(|event| event.size),
        Some(model.backing.size())
    );
    assert!(
        report
            .events()
            .iter()
            .all(|event| event.size <= event.held_rust)
    );
    Ok(())
}

#[test]
fn w1s_fallible_inline_alloc_and_budget_precede_install() -> Result<(), NumericReject> {
    let _serial = SERIAL
        .lock()
        .map_err(|_| NumericReject::ConstructionFailure)?;
    let audit = AuditScope::start(&ZERO_ROOT);
    assert_eq!(
        InlineSessionOwner::try_install(async {}, &ZERO_ROOT, admission(), holders()?).err(),
        Some(NumericReject::RustBudgetExceeded)
    );
    assert!(audit.finish().events().is_empty());
    let future = async {};
    fn layout_of<F>(_: &F) -> Result<InlineLayout, NumericReject> {
        InlineLayout::actual::<F>()
    }
    let model = layout_of(&future)?;
    let audit = AuditScope::start(&ROOT);
    audit.fail_next_size(model.backing.size());
    assert_eq!(
        InlineSessionOwner::try_install(future, &ROOT, admission(), holders()?).err(),
        Some(NumericReject::AllocationFailure)
    );
    let report = audit.finish();
    assert_eq!(report.events().len(), 1);
    assert_eq!(report.events()[0].kind, 4);
    assert_eq!(ROOT.snapshot().rust_bytes, 0);
    assert_eq!(
        OperationBound::sum(Opcode::AttemptClone, &[Layout::new::<usize>()], usize::MAX).err(),
        Some(NumericReject::Overflow)
    );
    Ok(())
}

#[test]
fn w1s_cancel_during_poll_never_cross_thread_drops_future() -> Result<(), NumericReject> {
    let _serial = SERIAL
        .lock()
        .map_err(|_| NumericReject::ConstructionFailure)?;
    let polling = AtomicBool::new(false);
    let cancelled = AtomicBool::new(false);
    let external = RefCell::new(None);
    let drops = Cell::new(0);
    struct Guard<'a>(&'a Cell<u32>);
    impl Drop for Guard<'_> {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }
    let guard = Guard(&drops);
    let future = async {
        let _guard = guard;
        poll_fn(|cx| {
            *external.borrow_mut() = Some(cx.waker().clone());
            polling.store(true, Ordering::Release);
            while !cancelled.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            assert_eq!(drops.get(), 0);
            Poll::<()>::Pending
        })
        .await;
    };
    let mut owner = InlineSessionOwner::try_install(future, &ROOT, admission(), holders()?)?;
    let layout = owner.layout();
    let cancel = owner.cancel_handle()?;
    std::thread::scope(|scope| -> Result<(), NumericReject> {
        let callback = scope.spawn(|| {
            while !polling.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            cancel.cancel();
            assert!(ROOT.snapshot().rust_bytes >= layout.size());
            cancelled.store(true, Ordering::Release);
        });
        assert_eq!(owner.poll_if_ready(), PollState::Cancelled);
        callback
            .join()
            .map_err(|_| NumericReject::ConstructionFailure)?;
        Ok(())
    })?;
    assert_eq!(drops.get(), 1);
    drop(cancel);
    drop(owner);
    assert_eq!(ROOT.snapshot().rust_bytes, layout.size());
    drop(external.into_inner());
    assert_eq!(ROOT.snapshot().rust_bytes, 0);
    Ok(())
}

fn native_layouts() -> Result<NativeFutureLayouts, NumericReject> {
    fn layout(size: usize, align: usize) -> Result<Layout, NumericReject> {
        Layout::from_size_align(size, align).map_err(|_| NumericReject::InvalidLayout)
    }
    // SAFETY: 当前 locked 实际 impl 编译诊断（SOURCE.md）逐个 generated F 非 Box 指针。
    unsafe {
        NativeFutureLayouts::from_monomorph_proof([
            layout(8608, 16)?,
            layout(80, 8)?,
            layout(3336, 8)?,
            layout(3112, 8)?,
            layout(3128, 8)?,
        ])
    }
}

#[test]
fn w1s_real_native_five_boxed_calls_reserved_before_actual_methods() -> Result<(), NumericReject> {
    let _serial = SERIAL
        .lock()
        .map_err(|_| NumericReject::ConstructionFailure)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| NumericReject::ConstructionFailure)?;
    let _enter = runtime.enter();
    runtime.block_on(async {});
    let profile = profile();
    let auth = rshell_m0::rshell_session::AuthPlan::from_secret(
        &profile,
        Some(secrecy::SecretString::from(String::new())),
    )
    .map_err(|_| NumericReject::ConstructionFailure)?;
    let native = NativeSshTransport::new(
        profile,
        auth,
        KnownHostsVerifier::new("unused-fixture-trust"),
    )
    .map_err(|_| NumericReject::ConstructionFailure)?;
    let layouts = native_layouts()?;
    let mut native = unsafe { UnconnectedNative::from_new(native, layouts) };
    let channel = unsafe { create_interaction_root(bounded_chan_upper()?, 1, &ROOT, admission()) }?;
    let request = TransportRequest::new(TerminalSize {
        cols: 0,
        rows: 1,
        pixel_width: 0,
        pixel_height: 0,
        dpi: 0,
    });
    let audit = AuditScope::start(&ROOT);
    let mut connect =
        native.connect_validation(&request, channel.value().0.clone(), &ROOT, admission())?;
    runtime.block_on(&mut connect);
    assert!(connect.result().is_some_and(Result::is_err));
    drop(connect);
    let mut next = native.next_event(&ROOT, admission())?;
    runtime.block_on(&mut next);
    assert!(next.result().is_some_and(Result::is_err));
    drop(next);
    let mut write = native.write(b"not copied without channel", &ROOT, admission())?;
    runtime.block_on(&mut write);
    assert!(write.result().is_some_and(Result::is_err));
    drop(write);
    let mut resize = native.resize(request.initial_size(), &ROOT, admission())?;
    runtime.block_on(&mut resize);
    assert!(resize.result().is_some_and(Result::is_err));
    drop(resize);
    let mut shutdown = native.shutdown(&ROOT, admission())?;
    runtime.block_on(&mut shutdown);
    assert!(shutdown.result().is_some_and(Result::is_ok));
    drop(shutdown);
    let report = audit.finish();
    verify_each_allocation(&report)?;
    for (opcode, size) in [
        (Opcode::TransportConnect, 8608),
        (Opcode::TransportNextEvent, 80),
        (Opcode::TransportWrite, 3336),
        (Opcode::TransportResize, 3112),
        (Opcode::TransportShutdown, 3128),
    ] {
        let events: Vec<_> = report
            .events()
            .iter()
            .filter(|event| event.size == size)
            .collect();
        assert_eq!(events.len(), 2, "{opcode:?}");
        assert_eq!(events[0].kind, 1);
        assert_eq!(events[1].kind, 2);
        assert!(events.iter().all(|event| event.held_rust >= size));
    }
    drop(channel);
    assert_eq!(ROOT.snapshot().rust_bytes, 0);
    Ok(())
}

#[test]
fn w1s_each_opcode_quota_shortfall_before_call_has_zero_callback_count() -> Result<(), NumericReject>
{
    let root = ProcessLedger::new(0, [u32::MAX; CLASS_COUNT]);
    for (index, opcode) in [
        Opcode::SessionChannels,
        Opcode::TransportConnect,
        Opcode::TransportNextEvent,
        Opcode::TransportWrite,
        Opcode::TransportResize,
        Opcode::TransportShutdown,
        Opcode::AuthPlan,
        Opcode::InteractionChannel,
        Opcode::InteractionRequest,
        Opcode::ReactorContext,
    ]
    .into_iter()
    .enumerate()
    {
        let source = root.open_source(1, (index % crate::ledger::SOURCE_COUNT) as u32)?;
        let called = Cell::new(0);
        let result = unsafe {
            super::operation::OperationRoot::before_call(
                OperationBound {
                    opcode,
                    rust_bytes: 1,
                },
                &root,
                Admission {
                    epoch: 1,
                    source: Some(source),
                },
                || {
                    called.set(called.get() + 1);
                },
            )
        };
        assert_eq!(result.err(), Some(NumericReject::RustBudgetExceeded));
        assert_eq!(called.get(), 0);
        root.invalidate_source(source)?;
    }
    Ok(())
}

#[test]
fn w1s_actual_interaction_pending_reply_cancel_and_last_channel_root() -> Result<(), NumericReject>
{
    use super::gates::interaction_request_bound;
    use rshell_m0::rshell_core::{
        AuthPrompt, InteractionId, InteractionRequest, InteractionResponse,
    };
    let _serial = SERIAL
        .lock()
        .map_err(|_| NumericReject::ConstructionFailure)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| NumericReject::ConstructionFailure)?;
    // 栈内调用不 spawn；第一次 runtime TLS 和 driver 属该 fixture 的独立 runtime 根。
    runtime.block_on(async {});
    let audit = AuditScope::start(&ROOT);
    // 单 request send + sender close 共 2 个位置，未越过 32-slot Block，无竞争 grow。
    // 当前锁定 GNU actual Chan 私有 monomorph 为384/128；不使用宽字段 padding 余量。
    let actual_chan =
        Layout::from_size_align(384, 128).map_err(|_| NumericReject::InvalidLayout)?;
    let mut channel = unsafe { create_interaction_root(actual_chan, 1, &ROOT, admission()) }?;
    let channel_bytes = ROOT.snapshot().rust_bytes;
    let grant = interaction_request_bound(0, 0)?.reserve(&ROOT, admission())?;
    let request_bytes = grant.amount().rust_bytes;
    let id = InteractionId(uuid::Uuid::from_u128(2));
    // SAFETY: tuple 两字段同时借用，绝不 move/clone 到外部，request future 在 channel 前 drop。
    {
        let (broker, receiver) = unsafe { channel.value_mut() };
        let mut request =
            std::pin::pin!(broker.request(InteractionRequest::Password(AuthPrompt {
                id,
                label: String::new(),
                echo: false,
            })));
        let mut outside = None;
        runtime.block_on(poll_fn(|cx| {
            assert!(request.as_mut().poll(cx).is_pending());
            match receiver.poll_recv(cx) {
                Poll::Ready(Some((actual, _))) => {
                    assert_eq!(actual, id);
                    outside = Some(cx.waker().clone());
                    Poll::Ready(())
                }
                _ => Poll::Pending,
            }
        }));
        assert!(ROOT.snapshot().rust_bytes > channel_bytes);
        broker
            .respond(id, InteractionResponse::Cancel)
            .map_err(|_| NumericReject::ConstructionFailure)?;
        let response = runtime
            .block_on(request.as_mut())
            .map_err(|_| NumericReject::ConstructionFailure)?;
        assert!(matches!(response, InteractionResponse::Cancel));
        if let Some(waker) = outside.take() {
            waker.wake();
        }
    }
    // PendingResponses 的树属于 channel 内 broker；空叶根仍由 request grant 承担。
    assert_eq!(ROOT.snapshot().rust_bytes, channel_bytes + request_bytes);
    drop(channel);
    assert_eq!(ROOT.snapshot().rust_bytes, request_bytes);
    drop(grant);
    assert_eq!(ROOT.snapshot().rust_bytes, 0);
    let report = audit.finish();
    verify_each_allocation(&report)?;
    // actual private Chan Arc、Block、pending tree、oneshot 逐笔都有真实额度。
    assert!(
        report
            .events()
            .iter()
            .any(|event| event.kind == 1 && event.size == 512 && event.align == 128)
    );
    assert!(
        report
            .events()
            .iter()
            .any(|event| event.kind == 1 && event.size == 80)
    );
    assert!(
        report
            .events()
            .iter()
            .all(|event| event.size <= event.held_rust)
    );
    Ok(())
}

#[test]
fn w1s_inline_control_schema_no_heap_and_close_lane_with_full_slots() -> Result<(), NumericReject> {
    use super::session::InlineSessionControls;
    let root = ProcessLedger::new(0, [u32::MAX; CLASS_COUNT]);
    let audit = AuditScope::start(&root);
    let mut controls = InlineSessionControls::<u32, u32, 2, 1>::default();
    assert!(controls.commands.try_send(1).is_ok());
    assert!(controls.commands.try_send(2).is_ok());
    assert_eq!(controls.commands.try_send(3), Err(3));
    controls.closing = true;
    controls.commands.close();
    let mut cx = Context::from_waker(Waker::noop());
    assert_eq!(controls.commands.poll_recv(&mut cx), Poll::Ready(Some(1)));
    assert_eq!(controls.commands.poll_recv(&mut cx), Poll::Ready(Some(2)));
    assert_eq!(controls.commands.poll_recv(&mut cx), Poll::Ready(None));
    controls.completion = Some(7);
    assert_eq!(controls.completion.take(), Some(7));
    drop(controls);
    assert!(audit.finish().events().is_empty());
    Ok(())
}

#[test]
fn w1s_real_socket_read_and_write_waits_cancel_with_reactor_late_waker() -> Result<(), NumericReject>
{
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let _serial = SERIAL
        .lock()
        .map_err(|_| NumericReject::ConstructionFailure)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| NumericReject::ConstructionFailure)?;
    // reactor/socket 图是 fixture 根；不把它们并入已证 inline grant 或签 reactor 证书。
    let (mut client, server) = runtime
        .block_on(async {
            let listener =
                tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
            let address = listener.local_addr()?;
            let (client, accepted) =
                tokio::join!(tokio::net::TcpStream::connect(address), listener.accept());
            Ok::<_, std::io::Error>((client?, accepted?.0))
        })
        .map_err(|_| NumericReject::ConstructionFailure)?;
    let retained = RefCell::new(None);
    let reached_read = Cell::new(false);
    let future = async {
        poll_fn(|cx| {
            *retained.borrow_mut() = Some(cx.waker().clone());
            Poll::Ready(())
        })
        .await;
        let mut byte = [0u8; 1];
        reached_read.set(true);
        let _ = client.read(&mut byte).await;
    };
    // owner/context/cancel/外部槽、单 socket reader、reactor WakeList 各最多一份，<=8。
    let mut owner = InlineSessionOwner::try_install(future, &ROOT, admission(), holders()?)?;
    let header_bytes = owner.layout().size();
    let cancel = owner.cancel_handle()?;
    let _enter = runtime.enter();
    assert_eq!(owner.poll_if_ready(), PollState::Pending);
    assert!(reached_read.get());
    cancel.cancel();
    assert_eq!(owner.poll_if_ready(), PollState::Cancelled);
    drop(cancel);
    drop(owner);
    assert!(ROOT.snapshot().rust_bytes >= header_bytes);
    drop(retained.borrow_mut().take());
    // borrower F drop 并没有 drop client；真实 IO root 仍可保存晚 waker。
    drop(client);
    drop(server);
    drop(_enter);
    drop(runtime);
    assert_eq!(ROOT.snapshot().rust_bytes, 0);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| NumericReject::ConstructionFailure)?;
    let (mut writer, reader) = tokio::io::duplex(1);
    runtime
        .block_on(writer.write_all(b"x"))
        .map_err(|_| NumericReject::ConstructionFailure)?;
    let future = async {
        let _ = writer.write_all(b"y").await;
    };
    let mut owner = InlineSessionOwner::try_install(future, &ROOT, admission(), holders()?)?;
    assert_eq!(owner.poll_if_ready(), PollState::Pending);
    let cancel = owner.cancel_handle()?;
    cancel.cancel();
    assert_eq!(owner.poll_if_ready(), PollState::Cancelled);
    drop(cancel);
    drop(owner);
    drop(writer);
    drop(reader);
    drop(runtime);
    assert_eq!(ROOT.snapshot().rust_bytes, 0);
    Ok(())
}

/// 实际 Native TCP/SSH identification 到达，不读取 trust file 或配置；这不是连接证书。
#[test]
fn w1s_native_localhost_identification_pending_then_cancel_fixture() -> Result<(), NumericReject> {
    use rshell_m0::rshell_session::SessionTransport;
    use tokio::io::AsyncReadExt;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| NumericReject::ConstructionFailure)?;
    runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await
            .map_err(|_| NumericReject::ConstructionFailure)?;
        let mut profile = profile();
        profile.port = listener.local_addr().map_err(|_| NumericReject::ConstructionFailure)?.port();
        let auth = rshell_m0::rshell_session::AuthPlan::from_secret(&profile,
            Some(secrecy::SecretString::from(String::new()))).map_err(|_| NumericReject::ConstructionFailure)?;
        let mut native = NativeSshTransport::new(profile, auth, KnownHostsVerifier::new(std::path::PathBuf::new()))
            .map_err(|_| NumericReject::ConstructionFailure)?;
        let (broker, receiver) = rshell_m0::rshell_session::interaction_channel();
        let request = TransportRequest::new(TerminalSize { cols: 80, rows: 24, pixel_width: 0, pixel_height: 0, dpi: 0 });
        let mut connect = native.connect(&request, broker);
        let server = async {
            let (mut socket, _) = listener.accept().await.map_err(|_| NumericReject::ConstructionFailure)?;
            let mut line = [0u8; 256];
            let mut length = 0;
            while length < line.len() {
                socket.read_exact(&mut line[length..length + 1]).await.map_err(|_| NumericReject::ConstructionFailure)?;
                length += 1;
                if line[length - 1] == b'\n' { break; }
            }
            assert!(line[..length].starts_with(b"SSH-2.0-"));
            Ok::<_, NumericReject>(socket)
        };
        let socket = tokio::select! {
            result = &mut connect => { let _ = result; return Err(NumericReject::ConstructionFailure); },
            result = server => result?,
        };
        // 服务端不发送 identification，实际 connect read 阻塞；显式 drop 取消，无 task join 假退款。
        drop(connect);
        drop(socket);
        drop(receiver);
        drop(native);
        Ok(())
    })
}

#[test]
fn w1s_intrusive_round_robin_wake_without_queue_allocation() -> Result<(), NumericReject> {
    use super::inline::InlinePollList;
    let _serial = SERIAL
        .lock()
        .map_err(|_| NumericReject::ConstructionFailure)?;
    struct Turn<'a> {
        calls: &'a Cell<u32>,
        saved: &'a RefCell<Option<Waker>>,
        done: &'a Cell<bool>,
    }
    impl Future for Turn<'_> {
        type Output = ();
        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            self.calls.set(self.calls.get() + 1);
            *self.saved.borrow_mut() = Some(cx.waker().clone());
            if self.done.get() {
                Poll::Ready(())
            } else {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }
    let (a, b, c) = (Cell::new(0), Cell::new(0), Cell::new(0));
    let (wa, wb, wc) = (RefCell::new(None), RefCell::new(None), RefCell::new(None));
    let done = Cell::new(false);
    let audit = AuditScope::start(&ROOT);
    let mut list = InlinePollList::default();
    for (calls, saved) in [(&a, &wa), (&b, &wb), (&c, &wc)] {
        let owner = InlineSessionOwner::try_install(
            Turn {
                calls,
                saved,
                done: &done,
            },
            &ROOT,
            admission(),
            holders()?,
        )?;
        assert!(list.push(owner).is_ok());
    }
    assert_eq!(list.poll_round(1), 0);
    assert_eq!((a.get(), b.get(), c.get()), (1, 0, 0));
    assert_eq!(list.poll_round(2), 0);
    assert_eq!((a.get(), b.get(), c.get()), (1, 1, 1));
    done.set(true);
    assert_eq!(list.poll_round(3), 3);
    assert!(list.is_empty());
    let last_a = wa
        .borrow_mut()
        .take()
        .ok_or(NumericReject::ConstructionFailure)?;
    let copy_a = last_a.clone();
    last_a.wake();
    drop(wb.borrow_mut().take());
    drop(wc.borrow_mut().take());
    assert!(ROOT.snapshot().rust_bytes > 0);
    drop(copy_a);
    drop(list);
    assert_eq!(ROOT.snapshot().rust_bytes, 0);
    let report = audit.finish();
    verify_each_allocation(&report)?;
    assert_eq!(
        report
            .events()
            .iter()
            .filter(|event| event.kind == 1)
            .count(),
        3
    );
    Ok(())
}

#[test]
fn w1s_holder_bound_checked_without_wrap_or_losing_existing_root() -> Result<(), NumericReject> {
    let _serial = SERIAL
        .lock()
        .map_err(|_| NumericReject::ConstructionFailure)?;
    assert_eq!(
        unsafe { HolderBound::from_source_proof(usize::MAX) }.err(),
        Some(NumericReject::RefcountOverflow)
    );
    let owner = InlineSessionOwner::try_install(async {}, &ROOT, admission(), holders()?)?;
    let layout = owner.layout();
    let handles = [
        owner.cancel_handle()?,
        owner.cancel_handle()?,
        owner.cancel_handle()?,
        owner.cancel_handle()?,
        owner.cancel_handle()?,
        owner.cancel_handle()?,
        owner.cancel_handle()?,
    ];
    assert_eq!(
        owner.cancel_handle().err(),
        Some(NumericReject::RefcountOverflow)
    );
    assert_eq!(ROOT.snapshot().rust_bytes, layout.size());
    drop(owner);
    assert_eq!(ROOT.snapshot().rust_bytes, layout.size());
    drop(handles);
    assert_eq!(ROOT.snapshot().rust_bytes, 0);
    Ok(())
}

#[test]
fn w1s_single_future_drop_unwind_drains_other_intrusive_roots() -> Result<(), NumericReject> {
    use super::inline::InlinePollList;
    let _serial = SERIAL
        .lock()
        .map_err(|_| NumericReject::ConstructionFailure)?;
    struct Drops<'a> {
        panic_once: bool,
        count: &'a Cell<u32>,
    }
    impl Future for Drops<'_> {
        type Output = ();
        fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
            Poll::Pending
        }
    }
    impl Drop for Drops<'_> {
        fn drop(&mut self) {
            self.count.set(self.count.get() + 1);
            if self.panic_once {
                std::panic::panic_any(1u32);
            }
        }
    }
    let count = Cell::new(0);
    let mut list = InlinePollList::default();
    for panic_once in [true, false, false] {
        let owner = InlineSessionOwner::try_install(
            Drops {
                panic_once,
                count: &count,
            },
            &ROOT,
            admission(),
            holders()?,
        )?;
        assert!(list.push(owner).is_ok());
    }
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(list))).is_err());
    assert_eq!(count.get(), 3);
    assert_eq!(ROOT.snapshot().rust_bytes, 0);
    Ok(())
}
