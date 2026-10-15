//! 只验证 normal Rust 端口与 ABI，不为真实 OS 输入/GUI 来源签发证明。

use super::ffi::{self, test_support as support};
use crate::desktop_input_port::transactions::ActionCompletion;
use crate::desktop_input_port::{ActionValue, InputPort, Registration};
use crate::desktop_runtime::Source;
use crate::signals::desktop_session::{DesktopAction, DesktopSource};
use rshell_m0::rshell_core::{TerminalOverrides, TerminalSettingsV1};
use std::sync::Arc;

type TestResult = Result<(), Box<dyn std::error::Error>>;
// ABI 登记采用 try_lock 返回 Busy；本组局部测试不把互相争用作为宿主来源证据。
static ABI_TESTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn root(id: u32) -> Result<Registration, Box<dyn std::error::Error>> {
    InputPort::register(
        Source {
            source: DesktopSource {
                session_id: id,
                generation: 1,
            },
            identity: format!("normal-{id}"),
        },
        Arc::new(TerminalSettingsV1::default().resolve(&TerminalOverrides::default())),
        Arc::default(),
    )
    .map_err(|error| format!("运行登记失败：{error:?}").into())
}

fn attach(port: &Arc<InputPort>) -> Result<ffi::Context, Box<dyn std::error::Error>> {
    port.connected();
    let handle =
        support::attach(port.clone()).map_err(|error| format!("局部附接失败：{error:?}"))?;
    Ok(support::context(port, handle))
}

fn key(physical: u64, ownership: u32) -> ffi::Key {
    ffi::Key {
        physical,
        phase: 1,
        modifiers: 0,
        raw_kind: 1,
        raw_value: u32::from('a'),
        code_kind: 1,
        code_value: u32::from('a'),
        legacy_kind: 1,
        legacy_value: u32::from('a'),
        raw_known: 1,
        same_event_dual: 1,
        alt_provenance: 1,
        text_ownership: ownership,
        default_action: 0,
        context_id: physical,
        ..support::initialized()
    }
}

fn close(context: &ffi::Context) {
    assert_eq!(unsafe { ffi::guossh_normal_v1_revoke(context) }, 0);
    let mut end: ffi::TextEnd = support::initialized();
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_text_end_query(context, &mut end) },
        0
    );
    if end.end_fence != 0 {
        assert_eq!(
            unsafe { ffi::guossh_normal_v1_text_end_ack(context, &end) },
            0
        );
    }
    assert_eq!(unsafe { ffi::guossh_normal_v1_release_retired(context) }, 0);
}

#[test]
fn ordinary_bind_never_registers_and_layout_inputs_fail_closed() -> TestResult {
    let _guard = ABI_TESTS.blocking_lock();
    let root = root(9101)?;
    let context = support::context(&root.0, 0);
    let mut bound: ffi::Bound = support::initialized();
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_bind(&context, &mut bound) },
        ffi::Status::NotAttached as u32
    );
    assert_eq!(bound.handle, 0);
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_bind(std::ptr::null(), &mut bound) },
        ffi::Status::InvalidArgument as u32
    );
    let mut invalid = context;
    invalid.header.version = 2;
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_bind(&invalid, &mut bound) },
        ffi::Status::UnsupportedVersion as u32
    );
    invalid = context;
    invalid.header.size -= 1;
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_bind(&invalid, &mut bound) },
        ffi::Status::InvalidArgument as u32
    );
    invalid = context;
    invalid.identity_len = u64::MAX;
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_bind(&invalid, &mut bound) },
        ffi::Status::TooLarge as u32
    );
    let malformed = [0xff];
    invalid = context;
    invalid.identity = malformed.as_ptr();
    invalid.identity_len = 1;
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_bind(&invalid, &mut bound) },
        ffi::Status::InvalidArgument as u32
    );
    Ok(())
}

#[tokio::test]
async fn native_ticket_is_original_one_shot_and_missing_evidence_never_becomes_ime() -> TestResult {
    let _guard = ABI_TESTS.lock().await;
    let root = root(9102)?;
    let context = attach(&root.0)?;
    let mut admission: ffi::Admission = support::initialized();
    for (physical, raw, dual, alt) in [(1, 0, 1, 1), (2, 1, 0, 1), (3, 1, 1, 0)] {
        let mut input = key(physical, 2);
        input.raw_known = raw;
        input.same_event_dual = dual;
        input.alt_provenance = alt;
        if alt == 0 {
            input.modifiers = 6;
        }
        assert_eq!(
            unsafe { ffi::guossh_normal_v1_key(&context, &input, &mut admission) },
            ffi::Status::PolicyRejected as u32
        );
        assert_eq!(admission.text_ticket, 0);
    }
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_key(&context, &key(4, 2), &mut admission) },
        0
    );
    assert_eq!(admission.route, 2);
    assert_ne!(admission.text_ticket, 0);
    let mut repeat = key(4, 2);
    repeat.phase = 2;
    repeat.same_event_dual = 0;
    let original_ticket = admission.text_ticket;
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_key(&context, &repeat, &mut admission) },
        ffi::Status::PolicyRejected as u32
    );
    assert_eq!(admission.text_ticket, original_ticket);
    let text = "完整文本🙂";
    let request = ffi::Composition {
        text_ticket: admission.text_ticket,
        kind: 2,
        text: text.as_ptr(),
        text_len: text.len() as u64,
        ..support::initialized()
    };
    let mut wrong = context;
    wrong.session_id += 1;
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_composition(&wrong, &request) },
        ffi::Status::StaleSource as u32
    );
    wrong = context;
    wrong.attempt += 1;
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_composition(&wrong, &request) },
        ffi::Status::StaleAttempt as u32
    );
    wrong = context;
    wrong.focus_epoch += 1;
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_composition(&wrong, &request) },
        ffi::Status::StaleFocus as u32
    );
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_composition(&context, &request) },
        0
    );
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_composition(&context, &request) },
        ffi::Status::InvalidTicket as u32
    );
    let effect = root.0.next().await;
    let actual = root
        .0
        .text(effect.ticket)
        .await
        .map_err(|error| format!("原队头文本：{error:?}"))?;
    assert_eq!(actual.text, text);
    drop((effect, actual));
    close(&context);
    Ok(())
}

#[tokio::test]
async fn full_copy_claim_retains_lease_until_actual_ack_and_survives_revoke() -> TestResult {
    let _guard = ABI_TESTS.lock().await;
    let root = root(9103)?;
    let context = attach(&root.0)?;
    let action = ffi::ActionRequest {
        request_id: 1,
        action: 11,
        ..support::initialized()
    };
    let mut seq = 0;
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_action(&context, &action, &mut seq) },
        0
    );
    let effect = root.0.next().await;
    let text = "完整选择🙂".repeat(400);
    let captured = root
        .0
        .capture_copy(|| Ok(text.clone()))
        .map_err(|error| format!("完整选文：{error:?}"))?;
    let mut waiting = Box::pin(root.0.action(
        effect.seq,
        effect.ticket,
        ActionValue {
            action: DesktopAction::Copy,
            copy_text: Some(captured),
            rejection: None,
        },
        &effect.lease,
    ));
    tokio::select! { biased; _ = &mut waiting => return Err("无消费者不应完成".into()), _ = tokio::task::yield_now() => {} }
    let request = ffi::ClaimRequest {
        effect: seq,
        ..support::initialized()
    };
    let mut info: ffi::ClaimInfo = support::initialized();
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_claim(&context, &request, &mut info) },
        0
    );
    assert_eq!(info.payload_len, text.len() as u64);
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_claim(&context, &request, &mut info) },
        ffi::Status::Duplicate as u32
    );
    let mut copy = ffi::PayloadCopy {
        claim: info.claim,
        buffer: std::ptr::null_mut(),
        capacity: 0,
        ..support::initialized()
    };
    let mut count = 77;
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_action_payload(&context, &copy, &mut count) },
        0
    );
    assert_eq!(count, text.len() as u64);
    let mut short = [0x55u8; 128];
    copy.buffer = short.as_mut_ptr();
    copy.capacity = short.len() as u64;
    count = 77;
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_action_payload(&context, &copy, &mut count) },
        ffi::Status::BufferTooSmall as u32
    );
    assert_eq!(count, 77);
    assert_eq!(short, [0x55; 128]);
    let mut full = vec![0u8; text.len()];
    copy.buffer = full.as_mut_ptr();
    copy.capacity = full.len() as u64;
    assert_eq!(unsafe { ffi::guossh_normal_v1_revoke(&context) }, 0);
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_release_retired(&context) },
        ffi::Status::Busy as u32
    );
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_action_payload(&context, &copy, &mut count) },
        0
    );
    assert_eq!(full, text.as_bytes());
    tokio::select! { biased; _ = &mut waiting => return Err("copy 不是消费者 ACK".into()), _ = tokio::task::yield_now() => {} }
    let ack = ffi::Ack {
        claim: info.claim,
        kind: 1,
        ..support::initialized()
    };
    assert_eq!(unsafe { ffi::guossh_normal_v1_ack(&context, &ack) }, 0);
    assert!(matches!(waiting.await, ActionCompletion::Completed));
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_ack(&context, &ack) },
        ffi::Status::InvalidTicket as u32
    );
    drop(effect);
    close(&context);
    Ok(())
}

#[test]
fn callback_handles_are_non_retargetable_and_retirement_does_not_disconnect() -> TestResult {
    let _guard = ABI_TESTS.blocking_lock();
    let root = root(9104)?;
    let context = attach(&root.0)?;
    let mut clone: ffi::Bound = support::initialized();
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_bind(&context, &mut clone) },
        0
    );
    let mut callback = context;
    callback.handle = clone.handle;
    assert_ne!(callback.handle, context.handle);
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_release_retired(&context) },
        ffi::Status::NotAttached as u32
    );
    close(&context);
    assert!(!root.0.output_boundary().1, "封口不排 Disconnect 控制");
    let handle = support::attach(root.0.clone())
        .map_err(|error| format!("原已认证端口再附接：{error:?}"))?;
    let current = support::context(&root.0, handle);
    let mut admission: ffi::Admission = support::initialized();
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_key(&callback, &key(1, 1), &mut admission) },
        ffi::Status::NotAttached as u32
    );
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_key(&context, &key(1, 1), &mut admission) },
        ffi::Status::UnknownHandle as u32
    );
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_key(&current, &key(1, 1), &mut admission) },
        0
    );
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_release_retired(&callback) },
        0
    );
    let thread_context = support::context(&root.0, current.handle);
    // 原生指针只借用调用期；跨线程测试在 worker 中重新构造 Context。
    let port = root.0.clone();
    let handle = thread_context.handle;
    let status = std::thread::spawn(move || {
        let context = support::context(&port, handle);
        let mut result: ffi::Admission = support::initialized();
        unsafe { ffi::guossh_normal_v1_key(&context, &key(2, 1), &mut result) }
    })
    .join()
    .map_err(|_| "局部线程测试失败")?;
    assert_eq!(status, ffi::Status::WrongThread as u32);
    close(&current);
    Ok(())
}

#[test]
fn query_reads_the_same_published_frame_and_only_viewport_relative_caret() -> TestResult {
    use rshell_m0::rshell_core::{TerminalSize, Viewport};
    use rshell_m0::rshell_session::{DefaultTerminalEngine, TerminalEngine};
    let _guard = ABI_TESTS.blocking_lock();
    let root = root(9105)?;
    let context = attach(&root.0)?;
    let mut projection: ffi::Projection = support::initialized();
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_projection(&context, &mut projection) },
        ffi::Status::NotAttached as u32
    );
    let size = TerminalSize {
        cols: 80,
        rows: 4,
        pixel_width: 800,
        pixel_height: 80,
        dpi: 96,
    };
    let mut engine = DefaultTerminalEngine::new(&root.0.profile, size)?;
    engine.advance("原帧\r\n".repeat(12).as_bytes())?;
    let frame = engine.render(
        Viewport {
            top_stable_row: i64::MAX,
            rows: 4,
        },
        None,
    )?;
    let original_top = frame.viewport_top;
    root.0
        .publish(
            17,
            frame,
            engine.viewport_bounds(),
            true,
            None,
            engine.display_modes(),
        )
        .map_err(|error| format!("投影发布：{error:?}"))?;
    let retained = root
        .0
        .projection_result()
        .map_err(|error| format!("原投影：{error:?}"))?
        .ok_or("缺少已发布帧")?;
    // 已发布后原引擎继续变化；查询不得渲染或读取这个新状态。
    engine.advance(b"\x1b[?2004hnew\r\n")?;
    for _ in 0..2 {
        assert_eq!(
            unsafe { ffi::guossh_normal_v1_projection(&context, &mut projection) },
            0
        );
        assert_eq!(projection.frame_revision, 17);
        assert_eq!(projection.viewport_top, original_top);
        assert_eq!(projection.bracketed_paste, 0);
        let current = root
            .0
            .projection_result()
            .map_err(|error| format!("复查投影：{error:?}"))?
            .ok_or("查询丢失已发布帧")?;
        assert!(Arc::ptr_eq(&retained, &current));
        if let Some(cursor) = retained.frame().cursor.as_ref() {
            assert_eq!(projection.caret_present, 1);
            assert_eq!(projection.caret_x, f64::from(cursor.position.column) * 10.0);
            assert_eq!(
                projection.caret_y,
                (cursor.position.stable_row - original_top) as f64 * 20.0
            );
        }
    }
    close(&context);
    Ok(())
}

#[tokio::test]
async fn explicit_abort_and_late_ack_never_complete_the_next_claim() -> TestResult {
    let _guard = ABI_TESTS.lock().await;
    let root = root(9106)?;
    let context = attach(&root.0)?;
    let mut seq = 0;
    let action = ffi::ActionRequest {
        request_id: 1,
        action: 1,
        ..support::initialized()
    };
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_action(&context, &action, &mut seq) },
        0
    );
    let first = root.0.next().await;
    let mut waiting = Box::pin(root.0.action(
        first.seq,
        first.ticket,
        ActionValue {
            action: DesktopAction::NewTab,
            copy_text: None,
            rejection: None,
        },
        &first.lease,
    ));
    tokio::select! { biased; _ = &mut waiting => return Err("必须等待消费者".into()), _ = tokio::task::yield_now() => {} }
    let mut info: ffi::ClaimInfo = support::initialized();
    let request = ffi::ClaimRequest {
        effect: seq,
        ..support::initialized()
    };
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_claim(&context, &request, &mut info) },
        0
    );
    let old_ack = ffi::Ack {
        claim: info.claim,
        kind: 2,
        ..support::initialized()
    };
    assert_eq!(unsafe { ffi::guossh_normal_v1_ack(&context, &old_ack) }, 0);
    assert!(matches!(waiting.await, ActionCompletion::Cancelled));
    drop(first);
    let action = ffi::ActionRequest {
        request_id: 2,
        action: 1,
        ..support::initialized()
    };
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_action(&context, &action, &mut seq) },
        0
    );
    let second = root.0.next().await;
    let mut waiting = Box::pin(root.0.action(
        second.seq,
        second.ticket,
        ActionValue {
            action: DesktopAction::NewTab,
            copy_text: None,
            rejection: None,
        },
        &second.lease,
    ));
    tokio::select! { biased; _ = &mut waiting => return Err("必须等待新消费者".into()), _ = tokio::task::yield_now() => {} }
    let request = ffi::ClaimRequest {
        effect: seq,
        ..support::initialized()
    };
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_claim(&context, &request, &mut info) },
        0
    );
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_ack(&context, &old_ack) },
        ffi::Status::InvalidTicket as u32
    );
    tokio::select! { biased; _ = &mut waiting => return Err("迟到 ACK 不可完成新 claim".into()), _ = tokio::task::yield_now() => {} }
    let ack = ffi::Ack {
        claim: info.claim,
        kind: 1,
        ..support::initialized()
    };
    assert_eq!(unsafe { ffi::guossh_normal_v1_ack(&context, &ack) }, 0);
    assert!(matches!(waiting.await, ActionCompletion::Completed));
    drop(second);
    close(&context);
    Ok(())
}

#[test]
#[cfg(target_pointer_width = "64")]
fn abi_layout_has_the_native_c_alignment_and_fixed_v1_sizes() {
    use std::mem::{align_of, offset_of, size_of};
    assert_eq!(size_of::<ffi::Header>(), 8);
    assert_eq!(size_of::<ffi::Context>(), 64);
    assert_eq!(size_of::<ffi::Registration>(), 64);
    assert_eq!(size_of::<ffi::Bound>(), 32);
    assert_eq!(size_of::<ffi::Key>(), 88);
    assert_eq!(size_of::<ffi::Admission>(), 72);
    assert_eq!(size_of::<ffi::Composition>(), 40);
    assert_eq!(size_of::<ffi::ActionRequest>(), 24);
    assert_eq!(size_of::<ffi::ClaimRequest>(), 16);
    assert_eq!(size_of::<ffi::ClaimInfo>(), 40);
    assert_eq!(size_of::<ffi::PayloadCopy>(), 32);
    assert_eq!(size_of::<ffi::Ack>(), 40);
    assert_eq!(size_of::<ffi::Projection>(), 160);
    assert_eq!(size_of::<ffi::TextEnd>(), 32);
    assert_eq!(
        size_of::<crate::desktop_input_port::PresentedGeometry>(),
        128
    );
    assert_eq!(size_of::<crate::desktop_input_port::PointerInput>(), 176);
    assert_eq!(size_of::<ffi::PresentedGeometry>(), 144);
    assert_eq!(size_of::<ffi::Pointer>(), 192);
    assert_eq!(align_of::<ffi::Context>(), 8);
    assert_eq!(offset_of!(ffi::Context, identity), 32);
    assert_eq!(offset_of!(ffi::Registration, input_object), 56);
    assert_eq!(offset_of!(ffi::Key, default_action), 64);
    assert_eq!(offset_of!(ffi::Composition, text), 24);
    assert_eq!(offset_of!(ffi::Projection, caret_x), 120);
}

#[tokio::test]
async fn abi_composition_continuation_streaming_payload_and_original_fence_settlement() -> TestResult
{
    let _guard = ABI_TESTS.lock().await;
    let root = root(9301)?;
    let context = attach(&root.0)?;
    let mut input = key(1, 2);
    input.context_id = 10;
    let mut admission: ffi::Admission = support::initialized();
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_key(&context, &input, &mut admission) },
        0
    );
    let first = admission;
    assert_eq!(first.text_op, 1);
    input.physical = 2;
    input.resume_text_ticket = first.text_ticket;
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_key(&context, &input, &mut admission) },
        0
    );
    assert_eq!(admission.text_op, 2);
    assert_eq!(admission.text_ticket, first.text_ticket);
    assert_ne!(admission.cycle_id, first.cycle_id);
    let text = "分块🙂完整 UTF-8";
    let reservation = ffi::TextReservation {
        text_ticket: first.text_ticket,
        utf8_bytes: text.len() as u64,
        ..support::initialized()
    };
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_reserve_payload(&context, &reservation) },
        0
    );
    let invalid = ffi::TextChunk {
        text_ticket: first.text_ticket,
        offset: 1,
        bytes: text.as_ptr(),
        len: 1,
        ..support::initialized()
    };
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_payload(&context, &invalid) },
        ffi::Status::InvalidEvent as u32
    );
    let mut chunk = ffi::TextChunk {
        text_ticket: first.text_ticket,
        offset: 0,
        bytes: text.as_ptr(),
        len: 2,
        ..support::initialized()
    };
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_payload(&context, &chunk) },
        0
    );
    let finish = ffi::TextFinish {
        text_ticket: first.text_ticket,
        ..support::initialized()
    };
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_finish(&context, &finish) },
        ffi::Status::InvalidEvent as u32
    );
    chunk.offset = 2;
    chunk.bytes = unsafe { text.as_ptr().add(2) };
    chunk.len = text.len() as u64 - 2;
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_payload(&context, &chunk) },
        0
    );
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_finish(&context, &finish) },
        0
    );
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_finish(&context, &finish) },
        ffi::Status::InvalidTicket as u32
    );
    let effect = root.0.next().await;
    assert_eq!(
        root.0
            .text(effect.ticket)
            .await
            .map_err(|e| format!("delivery: {e:?}"))?
            .text,
        text
    );
    drop(effect);
    input.phase = 2;
    input.context_id = 11;
    input.resume_text_ticket = 0;
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_key(&context, &input, &mut admission) },
        0
    );
    let second = admission;
    let action = ffi::ActionRequest {
        request_id: 1,
        action: 8,
        ..support::initialized()
    };
    let mut seq = 0;
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_action(&context, &action, &mut seq) },
        0
    );
    let mut end: ffi::TextEnd = support::initialized();
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_text_end_query(&context, &mut end) },
        0
    );
    assert_eq!(end.context_id, 11);
    assert_eq!(end.transaction_id, second.text_ticket);
    assert_ne!(end.end_fence, 0);
    assert_eq!(unsafe { ffi::guossh_normal_v1_revoke(&context) }, 0);
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_release_retired(&context) },
        ffi::Status::Busy as u32
    );
    let mut wrong = end;
    wrong.end_fence += 1;
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_text_end_ack(&context, &wrong) },
        ffi::Status::InvalidTicket as u32
    );
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_text_end_ack(&context, &end) },
        0
    );
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_text_end_ack(&context, &end) },
        ffi::Status::InvalidTicket as u32
    );
    let effect = root.0.next().await;
    assert!(root.0.text(effect.ticket).await.is_err());
    drop(effect);
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_release_retired(&context) },
        0
    );
    Ok(())
}

#[tokio::test]
async fn revoked_paste_claim_rejects_late_clipboard_but_original_abort_still_settles() -> TestResult
{
    let _guard = ABI_TESTS.lock().await;
    let root = root(9302)?;
    let context = attach(&root.0)?;
    let action = ffi::ActionRequest {
        request_id: 1,
        action: 12,
        ..support::initialized()
    };
    let mut seq = 0;
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_action(&context, &action, &mut seq) },
        0
    );
    let effect = root.0.next().await;
    let mut waiting = Box::pin(root.0.action(
        seq,
        effect.ticket,
        ActionValue {
            action: DesktopAction::Paste,
            copy_text: None,
            rejection: None,
        },
        &effect.lease,
    ));
    tokio::select! { biased; _ = &mut waiting => return Err("必须等待剪贴板消费者".into()), _ = tokio::task::yield_now() => {} }
    let request = ffi::ClaimRequest {
        effect: seq,
        ..support::initialized()
    };
    let mut info: ffi::ClaimInfo = support::initialized();
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_claim(&context, &request, &mut info) },
        0
    );
    assert_eq!(unsafe { ffi::guossh_normal_v1_revoke(&context) }, 0);
    let late = ffi::Ack {
        claim: info.claim,
        kind: 3,
        text: b"forbidden".as_ptr(),
        text_len: 9,
        ..support::initialized()
    };
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_ack(&context, &late) },
        ffi::Status::NotAttached as u32
    );
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_release_retired(&context) },
        ffi::Status::Busy as u32
    );
    let abort = ffi::Ack {
        kind: 2,
        text_len: 0,
        ..late
    };
    assert_eq!(unsafe { ffi::guossh_normal_v1_ack(&context, &abort) }, 0);
    assert!(matches!(waiting.await, ActionCompletion::Cancelled));
    drop(effect);
    close(&context);
    Ok(())
}

#[test]
fn unbound_native_registration_and_retired_root_callbacks_fail_closed() -> TestResult {
    let _guard = ABI_TESTS.blocking_lock();
    let root = root(9107)?;
    let registration = ffi::Registration {
        session_id: root.0.source.source.session_id,
        generation: 1,
        identity: root.0.source.identity.as_ptr(),
        identity_len: root.0.source.identity.len() as u64,
        attempt: 0,
        window: 0,
        input_object: 0,
        ..support::initialized()
    };
    let mut bound: ffi::Bound = support::initialized();
    #[cfg(target_os = "windows")]
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_register_trusted_host(&registration, &mut bound) },
        ffi::Status::NotAttached as u32
    );
    #[cfg(not(target_os = "windows"))]
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_register_trusted_host(&registration, &mut bound) },
        ffi::Status::UnsupportedPlatform as u32
    );
    assert_eq!(bound.handle, 0);
    let context = attach(&root.0)?;
    let weak = Arc::downgrade(&root.0);
    // 保留已退役来源的 identity 借用，只验证回调仍持原端口而非 new pane。
    let identity = root.0.source.identity.clone();
    let mut context = context;
    context.identity = identity.as_ptr();
    drop(root);
    assert!(weak.upgrade().is_some());
    let mut admission: ffi::Admission = support::initialized();
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_key(&context, &key(1, 1), &mut admission) },
        ffi::Status::Retired as u32
    );
    close(&context);
    assert!(weak.upgrade().is_none());
    Ok(())
}

#[tokio::test]
async fn abi_full_native_tickets_preserve_busy_down_latch_and_up_control() -> TestResult {
    let _guard = ABI_TESTS.lock().await;
    let root = root(9108)?;
    let context = attach(&root.0)?;
    let mut admission: ffi::Admission = support::initialized();
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_key(&context, &key(1, 2), &mut admission) },
        0
    );
    let first_ticket = admission.text_ticket;
    let commit = ffi::Composition {
        text_ticket: first_ticket,
        kind: 2,
        text: b"a".as_ptr(),
        text_len: 1,
        ..support::initialized()
    };
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_composition(&context, &commit) },
        0
    );
    let mut repeat = key(1, 1);
    repeat.phase = 2;
    for _ in 1..crate::desktop_runtime::INPUT_EVENTS {
        assert_eq!(
            unsafe { ffi::guossh_normal_v1_key(&context, &repeat, &mut admission) },
            0
        );
    }
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_key(&context, &key(2, 1), &mut admission) },
        ffi::Status::Busy as u32
    );
    let first = root.0.next().await;
    let text = root
        .0
        .text(first.ticket)
        .await
        .map_err(|error| format!("已提交原文本：{error:?}"))?;
    drop((first, text));
    // 额度释放不能把被 Busy 拒绝的 Down 改成一次新的按压。
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_key(&context, &key(2, 1), &mut admission) },
        ffi::Status::Cancelled as u32
    );
    let mut up = key(2, 1);
    up.phase = 3;
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_key(&context, &up, &mut admission) },
        ffi::Status::Cancelled as u32
    );
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_key(&context, &key(2, 1), &mut admission) },
        0
    );
    // 数据再满时，原按压的 Up 仍可使用控制预留。
    up.physical = 1;
    up.text_ownership = 2;
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_key(&context, &up, &mut admission) },
        0
    );
    close(&context);
    Ok(())
}

#[tokio::test]
async fn poisoned_claim_cleanup_cancels_original_waiter_and_permanently_seals_admission()
-> TestResult {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    let _guard = ABI_TESTS.lock().await;
    // 无 poison 的明确 abort 是对照；其余覆盖 abort、revoke 与已封口的最终退休。
    for (id, poisoned, cleanup) in [
        (9201, false, 0),
        (9202, true, 0),
        (9203, true, 1),
        (9204, true, 2),
    ] {
        let root = root(id)?;
        let context = attach(&root.0)?;
        let action = ffi::ActionRequest {
            request_id: 1,
            action: 1,
            ..support::initialized()
        };
        let mut seq = 0;
        assert_eq!(
            unsafe { ffi::guossh_normal_v1_action(&context, &action, &mut seq) },
            0
        );
        let effect = root.0.next().await;
        let mut waiting = Box::pin(root.0.action(
            effect.seq,
            effect.ticket,
            ActionValue {
                action: DesktopAction::NewTab,
                copy_text: None,
                rejection: None,
            },
            &effect.lease,
        ));
        tokio::select! { biased; _ = &mut waiting => return Err("必须等待原消费者".into()), _ = tokio::task::yield_now() => {} }
        let request = ffi::ClaimRequest {
            effect: seq,
            ..support::initialized()
        };
        let mut info: ffi::ClaimInfo = support::initialized();
        assert_eq!(
            unsafe { ffi::guossh_normal_v1_claim(&context, &request, &mut info) },
            0
        );
        if cleanup == 2 {
            assert_eq!(unsafe { ffi::guossh_normal_v1_revoke(&context) }, 0);
            tokio::select! { biased; _ = &mut waiting => return Err("正常 revoke 保留原 claim 责任".into()), _ = tokio::task::yield_now() => {} }
        }
        let caught = catch_unwind(AssertUnwindSafe(|| {
            support::with_state(&context, || {
                if poisoned {
                    panic!("受控原生 claim 状态故障");
                }
            })
        }));
        match caught {
            Err(_) if poisoned => {}
            Ok(Ok(())) if !poisoned => {}
            Ok(Err(error)) => return Err(format!("持有真实 ABI 状态失败：{error:?}").into()),
            _ => return Err("故障注入未产生预期的锁状态".into()),
        }
        assert_eq!(
            support::state_poisoned(&context).map_err(|error| format!("锁状态：{error:?}"))?,
            poisoned
        );
        let boundary = root.0.output_boundary();
        let mut admission = ffi::Admission {
            effect: 701,
            text_ticket: 702,
            route: 703,
            ..support::initialized()
        };
        let mut output_effect = 704;
        let mut key_status = 0;
        let mut action_status = 0;
        let mut completed_ack_status = 0;
        if poisoned {
            key_status = unsafe { ffi::guossh_normal_v1_key(&context, &key(1, 2), &mut admission) };
            let action = ffi::ActionRequest {
                request_id: 2,
                action: 1,
                ..support::initialized()
            };
            action_status =
                unsafe { ffi::guossh_normal_v1_action(&context, &action, &mut output_effect) };
            let completed = ffi::Ack {
                claim: info.claim,
                kind: 1,
                ..support::initialized()
            };
            completed_ack_status = unsafe { ffi::guossh_normal_v1_ack(&context, &completed) };
        }
        let abort = ffi::Ack {
            claim: info.claim,
            kind: 2,
            ..support::initialized()
        };
        let cleanup_status = match cleanup {
            0 => unsafe { ffi::guossh_normal_v1_ack(&context, &abort) },
            1 => unsafe { ffi::guossh_normal_v1_revoke(&context) },
            _ => unsafe { ffi::guossh_normal_v1_release_retired(&context) },
        };
        let mut completion = None;
        tokio::select! { biased; result = &mut waiting => completion = Some(result), _ = tokio::task::yield_now() => {} }
        assert_eq!(
            cleanup_status, 0,
            "poisoned claim 必须能取消并退休，不能永远 Busy"
        );
        assert!(
            matches!(completion, Some(ActionCompletion::Cancelled)),
            "只能通知原 actor 取消，不能伪造 Completed"
        );
        if poisoned {
            assert_eq!(key_status, ffi::Status::Internal as u32);
            assert_eq!(action_status, ffi::Status::Internal as u32);
            assert_eq!(completed_ack_status, ffi::Status::Internal as u32);
            assert_eq!(
                (admission.effect, admission.text_ticket, admission.route),
                (701, 702, 703)
            );
            assert_eq!(output_effect, 704);
            assert_eq!(
                root.0.output_boundary(),
                boundary,
                "拒绝及取消不产生新 effect"
            );
        }
        if cleanup != 2 {
            let late_completed = ffi::Ack {
                claim: info.claim,
                kind: 1,
                ..support::initialized()
            };
            assert_eq!(
                unsafe { ffi::guossh_normal_v1_ack(&context, &late_completed) },
                if poisoned {
                    ffi::Status::Internal as u32
                } else {
                    ffi::Status::InvalidTicket as u32
                }
            );
            if poisoned {
                assert!(
                    support::state_poisoned(&context)
                        .map_err(|error| format!("清理后锁状态：{error:?}"))?
                );
                assert_eq!(
                    unsafe { ffi::guossh_normal_v1_key(&context, &key(1, 2), &mut admission) },
                    ffi::Status::Internal as u32
                );
                assert_eq!(
                    (admission.effect, admission.text_ticket, admission.route),
                    (701, 702, 703)
                );
                assert!(
                    matches!(
                        root.0.attach(crate::desktop_input_port::Binding {
                            attempt: context.attempt,
                            epoch: context.focus_epoch,
                        }),
                        Err(crate::desktop_input_port::Reject::StaleFocus)
                    ),
                    "原 Port 输入绑定必须已封口"
                );
            }
            assert_eq!(unsafe { ffi::guossh_normal_v1_revoke(&context) }, 0);
            assert_eq!(
                unsafe { ffi::guossh_normal_v1_release_retired(&context) },
                0
            );
        }
        if poisoned {
            assert_eq!(
                root.0.output_boundary(),
                boundary,
                "迟到 ACK 不产生新 effect"
            );
        }
        assert_eq!(
            unsafe { ffi::guossh_normal_v1_ack(&context, &abort) },
            ffi::Status::UnknownHandle as u32
        );
        drop(waiting);
        drop(effect);
    }
    Ok(())
}

#[tokio::test]
async fn contended_claim_cleanup_remains_busy_until_unlock_then_can_retry() -> TestResult {
    let _guard = ABI_TESTS.lock().await;
    let root = root(9205)?;
    let context = attach(&root.0)?;
    let action = ffi::ActionRequest {
        request_id: 1,
        action: 1,
        ..support::initialized()
    };
    let mut seq = 0;
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_action(&context, &action, &mut seq) },
        0
    );
    let effect = root.0.next().await;
    let mut waiting = Box::pin(root.0.action(
        effect.seq,
        effect.ticket,
        ActionValue {
            action: DesktopAction::NewTab,
            copy_text: None,
            rejection: None,
        },
        &effect.lease,
    ));
    tokio::select! { biased; _ = &mut waiting => return Err("必须等待原消费者".into()), _ = tokio::task::yield_now() => {} }
    let request = ffi::ClaimRequest {
        effect: seq,
        ..support::initialized()
    };
    let mut info: ffi::ClaimInfo = support::initialized();
    assert_eq!(
        unsafe { ffi::guossh_normal_v1_claim(&context, &request, &mut info) },
        0
    );
    let abort = ffi::Ack {
        claim: info.claim,
        kind: 2,
        ..support::initialized()
    };
    let mut blocked = [0; 2];
    support::with_state(&context, || {
        blocked[0] = unsafe { ffi::guossh_normal_v1_ack(&context, &abort) };
        blocked[1] = unsafe { ffi::guossh_normal_v1_revoke(&context) };
    })
    .map_err(|error| format!("受控 ABI 竞争：{error:?}"))?;
    assert_eq!(blocked[0], ffi::Status::Busy as u32);
    assert_eq!(blocked[1], ffi::Status::Busy as u32);
    assert!(!support::state_poisoned(&context).map_err(|error| format!("竞争锁状态：{error:?}"))?);
    assert_eq!(unsafe { ffi::guossh_normal_v1_revoke(&context) }, 0);
    let mut release_status = 0;
    support::with_state(&context, || {
        release_status = unsafe { ffi::guossh_normal_v1_release_retired(&context) };
    })
    .map_err(|error| format!("受控退休竞争：{error:?}"))?;
    assert_eq!(release_status, ffi::Status::Busy as u32);
    assert_eq!(unsafe { ffi::guossh_normal_v1_ack(&context, &abort) }, 0);
    assert!(matches!(waiting.await, ActionCompletion::Cancelled));
    drop(effect);
    close(&context);
    Ok(())
}
