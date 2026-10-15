#![allow(clippy::expect_used)]
use super::transactions::ActionReply;
use super::{
    ActionValue, Arc, DesktopAction, DesktopEvent, DesktopKeyPhase, DesktopSource, INPUT_EVENTS,
    InputPort, NonZeroU64, PolicyKeyInput, Registration, Reject, Route, Source, Surface,
    cycle_slot, policy, transactions,
};
use rshell_m0::rshell_core::{KeyCode, KeyModifiers, TerminalOverrides, TerminalSettingsV1};

// 只隔离登记表临界区；测试本体、port 状态和 actor 仍并行运行。
static SOURCE_REGISTRATION: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) fn isolate_source_registration() -> impl Drop {
    SOURCE_REGISTRATION.lock().expect("登记表测试隔离")
}

pub(crate) fn hold_source_registration() -> (impl Drop, impl Drop) {
    let isolation = isolate_source_registration();
    let registry = super::REGISTRY
        .get()
        .expect("登记表")
        .lock()
        .expect("受控登记临界区");
    (registry, isolation)
}

pub(crate) fn registration(session_id: u32) -> Registration {
    registration_with_profile(
        session_id,
        TerminalSettingsV1::default().resolve(&TerminalOverrides::default()),
    )
}

pub(crate) fn registration_with_profile(
    session_id: u32,
    profile: rshell_m0::rshell_core::ResolvedTerminalProfile,
) -> Registration {
    InputPort::register(
        Source {
            source: DesktopSource {
                session_id,
                generation: 1,
            },
            identity: format!("prepared-{session_id}"),
        },
        Arc::new(profile),
        Arc::default(),
    )
    .expect("运行端口预算")
}

fn surface(port: &Arc<InputPort>) -> Arc<Surface> {
    port.connected();
    // 测试仅驱动受控 Rust 接缝，不声称证明 OS 输入来源。
    unsafe {
        port.attach_verified(port.binding().attempt)
            .expect("受控来源")
    }
}
pub(crate) fn key(physical: u64, phase: DesktopKeyPhase) -> PolicyKeyInput {
    PolicyKeyInput {
        raw_chord: Some(policy::RawChord {
            code: KeyCode::Character('a'),
            modifiers: KeyModifiers::default(),
        }),
        dual: Some(policy::VerifiedKeyCodes {
            code: KeyCode::Character('a'),
            legacy_code: KeyCode::Character('a'),
        }),
        phase,
        physical_identity: NonZeroU64::new(physical),
        alt_provenance: policy::AltProvenance::Known {
            left: false,
            right: false,
        },
        text_ownership: policy::TextOwnership::TerminalKey,
        default_action: None,
    }
}

#[tokio::test]
async fn full_queue_keeps_up_and_busy_down_latched_and_isolates_sources() {
    let root = registration(701);
    let port = &root.0;
    let source = surface(port);
    let first = source.key(key(1, DesktopKeyPhase::Down)).expect("down");
    for _ in 1..INPUT_EVENTS {
        source.key(key(1, DesktopKeyPhase::Repeat)).expect("repeat");
    }
    assert_eq!(source.key(key(2, DesktopKeyPhase::Down)), Err(Reject::Busy));
    source.key(key(1, DesktopKeyPhase::Up)).expect("控制预留");
    assert_eq!(
        source.key(key(2, DesktopKeyPhase::Repeat)),
        Err(Reject::Cancelled)
    );
    assert_eq!(
        source.key(key(2, DesktopKeyPhase::Up)),
        Err(Reject::Cancelled)
    );
    let other = registration(702);
    let other_surface = surface(&other.0);
    other_surface
        .key(key(1, DesktopKeyPhase::Down))
        .expect("另一来源独立");
    for _ in 0..=INPUT_EVENTS {
        drop(port.next().await);
    }
    assert!(cycle_slot(&port.state.lock().expect("state"), first.ticket.cycle).is_none());
    source
        .key(key(2, DesktopKeyPhase::Down))
        .expect("原 up 后才可新 down");
}

#[tokio::test]
async fn original_native_ticket_completes_head_once_without_queued_ack_deadlock() {
    let root = registration(703);
    let port = &root.0;
    let source = surface(port);
    let mut native = key(1, DesktopKeyPhase::Down);
    native.text_ownership = policy::TextOwnership::NativeText;
    let admission = source.key(native).expect("原生文本");
    assert_eq!(admission.route, Route::NativeText);
    let effect = port.next().await;
    source.key(key(1, DesktopKeyPhase::Up)).expect("up");
    source
        .composition(admission.ticket, Some("预编辑".into()), false)
        .expect("preedit");
    source
        .composition(admission.ticket, Some("完整文本🙂".into()), true)
        .expect("commit");
    assert_eq!(
        port.text(effect.ticket).await.expect("队头完成").text,
        "完整文本🙂"
    );
    assert_eq!(
        source.composition(admission.ticket, Some("重复".into()), true),
        Err(Reject::InvalidTicket)
    );
    drop(effect);
    assert!(
        port.state
            .lock()
            .expect("state")
            .cycles
            .iter()
            .all(Option::is_none)
    );
}

#[tokio::test]
async fn released_cycle_keeps_settlement_without_blocking_the_next_press() {
    let root = registration(731);
    let source = surface(&root.0);
    let first = source.key(key(1, DesktopKeyPhase::Down)).expect("旧 down");
    source.key(key(1, DesktopKeyPhase::Up)).expect("旧 up");
    let second = source
        .key(key(1, DesktopKeyPhase::Down))
        .expect("旧 effect 未消费也可再次按下");
    assert_ne!(first.ticket, second.ticket);
    root.0.reject_ticket(first.ticket);
    assert!(!root.0.applicable(first.ticket));
    assert!(root.0.applicable(second.ticket));
    let repeat = source
        .key(key(1, DesktopKeyPhase::Repeat))
        .expect("旧拒绝不污染新按压");
    assert_eq!(repeat.route, Route::Terminal);
    source.key(key(1, DesktopKeyPhase::Up)).expect("新 up");
    for _ in 0..5 {
        drop(root.0.next().await);
    }
    assert!(
        root.0
            .state
            .lock()
            .expect("state")
            .cycles
            .iter()
            .all(Option::is_none)
    );
}

#[tokio::test]
async fn native_repeat_has_its_own_one_shot_transaction_within_the_press() {
    let root = registration(732);
    let source = surface(&root.0);
    let input = |phase| {
        let mut input = key(1, phase);
        input.text_ownership = policy::TextOwnership::NativeText;
        input.default_action = Some(DesktopAction::NewTab);
        input.raw_chord.as_mut().expect("raw").modifiers.control = true;
        input.raw_chord.as_mut().expect("raw").modifiers.alt = true;
        input.alt_provenance = policy::AltProvenance::Known {
            left: false,
            right: true,
        };
        input
    };
    let first = source.key(input(DesktopKeyPhase::Down)).expect("down");
    source
        .composition(first.ticket, Some("首段🙂".into()), true)
        .expect("commit");
    let first_effect = root.0.next().await;
    assert_eq!(
        root.0.text(first.ticket).await.expect("首段").text,
        "首段🙂"
    );
    drop(first_effect);
    let repeat = source.key(input(DesktopKeyPhase::Repeat)).expect("repeat");
    assert_eq!(repeat.route, Route::NativeText);
    assert_ne!(first.ticket, repeat.ticket);
    assert_eq!(
        source.composition(first.ticket, Some("迟到".into()), true),
        Err(Reject::InvalidTicket)
    );
    source
        .composition(repeat.ticket, Some("重复多字é".into()), true)
        .expect("独立 commit");
    let repeat_effect = root.0.next().await;
    assert_eq!(
        root.0.text(repeat.ticket).await.expect("重复文本").text,
        "重复多字é"
    );
    assert_eq!(
        source.composition(repeat.ticket, Some("再提交".into()), true),
        Err(Reject::InvalidTicket)
    );
    drop(repeat_effect);
    source.key(input(DesktopKeyPhase::Up)).expect("up");
}

#[tokio::test]
async fn claimed_action_survives_revoke_but_late_paste_cannot_target_new_focus() {
    let root = registration(704);
    let port = &root.0;
    let source = surface(port);
    let mut input = key(1, DesktopKeyPhase::Down);
    input.default_action = Some(DesktopAction::Paste);
    source.key(input).expect("paste action");
    let effect = port.next().await;
    let mut waiting = Box::pin(port.action(
        effect.seq,
        effect.ticket,
        ActionValue {
            action: DesktopAction::Paste,
            copy_text: None,
            rejection: None,
        },
        &effect.lease,
    ));
    tokio::select! { biased; _ = &mut waiting => panic!("没有消费者不能完成"), _ = tokio::task::yield_now() => {} }
    assert!(matches!(
        source.claim_action(effect.seq + 1),
        Err(Reject::InvalidTicket)
    ));
    let claim = source.claim_action(effect.seq).expect("claim");
    assert!(matches!(
        source.claim_action(effect.seq),
        Err(Reject::Duplicate)
    ));
    source.revoke().expect("revoke");
    let _new = surface(port);
    assert_eq!(
        claim.ack(ActionReply::Paste("原目标".into())),
        Err(Reject::StaleFocus)
    );
    claim
        .ack(ActionReply::Cancelled)
        .expect("原 claim 明确 abort");
    assert!(matches!(
        waiting.await,
        transactions::ActionCompletion::Cancelled
    ));
    assert!(!port.applicable(effect.ticket));
    assert_eq!(claim.ack(ActionReply::Completed), Err(Reject::Duplicate));
}

#[test]
fn no_native_proof_authentication_and_retirement_fail_closed() {
    let root = registration(705);
    let port = root.0.clone();
    assert!(matches!(
        port.attach(port.binding()),
        Err(Reject::NotAttached)
    ));
    let surface = unsafe { port.attach_verified(0).expect("受控 attach") };
    assert_eq!(
        surface.key(key(1, DesktopKeyPhase::Down)),
        Err(Reject::NotAuthenticated)
    );
    port.connected();
    surface
        .key(key(1, DesktopKeyPhase::Down))
        .expect("authenticated");
    drop(root);
    assert_eq!(
        surface.key(key(2, DesktopKeyPhase::Down)),
        Err(Reject::Retired)
    );
    assert_eq!(port.state.lock().expect("state").data_count, 0);
}

#[tokio::test]
async fn cancelled_waiter_keeps_original_claim_separate_from_the_next_action() {
    let root = registration(733);
    let source = surface(&root.0);
    source.action(1, DesktopAction::NewTab).expect("原动作");
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
    tokio::select! { biased; _ = &mut waiting => panic!("需要消费者"), _ = tokio::task::yield_now() => {} }
    let old = source.claim_action(first.seq).expect("原 claim");
    drop(waiting);
    drop(first);
    source.revoke().expect("原焦点撤权");
    let next_source = surface(&root.0);
    next_source
        .action(2, DesktopAction::NewTab)
        .expect("新动作");
    let next = root.0.next().await;
    let mut waiting = Box::pin(root.0.action(
        next.seq,
        next.ticket,
        ActionValue {
            action: DesktopAction::NewTab,
            copy_text: None,
            rejection: None,
        },
        &next.lease,
    ));
    tokio::select! { biased; _ = &mut waiting => panic!("需要新消费者"), _ = tokio::task::yield_now() => {} }
    let current = next_source.claim_action(next.seq).expect("新 claim");
    old.ack(ActionReply::Completed)
        .expect("迟到 ACK 仍结算原目标");
    tokio::select! { biased; _ = &mut waiting => panic!("旧 ACK 不得结算新动作"), _ = tokio::task::yield_now() => {} }
    current.ack(ActionReply::Completed).expect("新消费者完成");
    assert!(matches!(
        waiting.await,
        transactions::ActionCompletion::Completed
    ));
}

#[tokio::test]
async fn unsupported_result_rejects_pending_repeat_in_same_cycle() {
    let root = registration(706);
    let source = surface(&root.0);
    let admission = source.key(key(1, DesktopKeyPhase::Down)).expect("down");
    source.key(key(1, DesktopKeyPhase::Repeat)).expect("repeat");
    source.key(key(1, DesktopKeyPhase::Up)).expect("up");
    let first = root.0.next().await;
    root.0.reject_ticket(admission.ticket);
    assert!(!root.0.applicable(root.0.next().await.ticket));
    drop(first);
    drop(root.0.next().await);
}

#[test]
fn lock_contention_down_remains_rejected_until_up_and_old_surface_cannot_revoke_new_focus() {
    let root = registration(707);
    let source = surface(&root.0);
    let guard = root.0.state.lock().expect("actor 短临界区");
    assert_eq!(source.key(key(1, DesktopKeyPhase::Down)), Err(Reject::Busy));
    drop(guard);
    assert_eq!(
        source.key(key(1, DesktopKeyPhase::Repeat)),
        Err(Reject::Cancelled)
    );
    assert_eq!(
        source.key(key(1, DesktopKeyPhase::Up)),
        Err(Reject::Cancelled)
    );
    source
        .key(key(1, DesktopKeyPhase::Down))
        .expect("up 后重新准入");
    source.revoke().expect("撤销原 surface");
    let next = surface(&root.0);
    assert_eq!(source.revoke(), Err(Reject::StaleFocus));
    assert_eq!(
        source.command(1, DesktopEvent::Copy),
        Err(Reject::StaleFocus)
    );
    next.command(1, DesktopEvent::Copy)
        .expect("新 focus 未被旧 revoke 撤销");
}

#[tokio::test]
async fn queue_control_ack_and_committed_text_cancellation_release_without_reentrant_lock() {
    let root = registration(708);
    let source = surface(&root.0);
    let mut native = key(1, DesktopKeyPhase::Down);
    native.text_ownership = policy::TextOwnership::NativeText;
    let ticket = source.key(native).expect("native").ticket;
    source
        .composition(ticket, Some("尚未执行的完整文本".into()), true)
        .expect("commit");
    for _ in 1..INPUT_EVENTS {
        source.key(key(1, DesktopKeyPhase::Repeat)).expect("fill");
    }
    source
        .command(10, DesktopEvent::FrameAck { frame_seq: 1 })
        .expect("满数据队列控制额度");
    assert_eq!(
        source.command(11, DesktopEvent::FrameAck { frame_seq: 2 }),
        Err(Reject::Busy)
    );
    source
        .command(11, DesktopEvent::Disconnect)
        .expect("ACK 占控制槽时仍能关闭；Busy 未消耗 ID");
    source
        .key(key(1, DesktopKeyPhase::Up))
        .expect("up 独立容量");
    for _ in 0..INPUT_EVENTS + 3 {
        drop(root.0.next().await);
    }
    assert_eq!(root.0.state.lock().expect("预算已退款").bytes, 0);
    source
        .command(12, DesktopEvent::FrameAck { frame_seq: 2 })
        .expect("Busy 不消耗关联 ID");
}

#[test]
fn first_admitted_stream_is_exclusive_without_changing_legacy_sequence_contract() {
    use crate::desktop_runtime::Protocol;
    use crate::signals::desktop_session::{DesktopFailure, DesktopOrdered};
    for native_first in [true, false] {
        let mut legacy = Protocol::new(
            DesktopSource {
                session_id: 709 + u32::from(native_first),
                generation: 1,
            },
            "identity".into(),
        );
        let root = InputPort::register(
            legacy.source.clone(),
            Arc::new(TerminalSettingsV1::default().resolve(&TerminalOverrides::default())),
            legacy.mode.clone(),
        )
        .expect("运行端口预算");
        let source = surface(&root.0);
        let request = DesktopOrdered {
            version: 2,
            source: legacy.source.source.clone(),
            identity: "identity".into(),
            seq: 1,
            event: DesktopEvent::Copy,
        };
        if native_first {
            source
                .key(key(1, DesktopKeyPhase::Down))
                .expect("native 先准入");
            assert!(matches!(
                legacy.admit(&request),
                Err(DesktopFailure::MixedMode)
            ));
        } else {
            drop(legacy.admit(&request).expect("legacy 先准入"));
            assert_eq!(
                source.key(key(1, DesktopKeyPhase::Down)),
                Err(Reject::MixedMode)
            );
            let next = DesktopOrdered { seq: 2, ..request };
            assert!(legacy.admit(&next).is_ok());
        }
    }
}

#[tokio::test]
async fn native_altgr_never_releases_a_shell_candidate_and_retains_original_text_ticket() {
    let root = registration(711);
    let source = surface(&root.0);
    let mut native = key(1, DesktopKeyPhase::Down);
    native.raw_chord.as_mut().expect("raw").modifiers = KeyModifiers {
        control: true,
        alt: true,
        ..KeyModifiers::default()
    };
    native.alt_provenance = policy::AltProvenance::Known {
        left: false,
        right: true,
    };
    native.text_ownership = policy::TextOwnership::NativeText;
    native.default_action = Some(DesktopAction::NewTab);
    let admitted = source.key(native.clone()).expect("AltGr 归原生文本");
    assert_eq!(admitted.route, Route::NativeText);
    native.phase = DesktopKeyPhase::Repeat;
    source.key(native).expect("repeat 不重复候选");
    source
        .composition(admitted.ticket, Some("€".into()), true)
        .expect("原票据");
    let effect = root.0.next().await;
    assert!(matches!(effect.kind, super::EffectKind::Text));
    assert_eq!(root.0.text(effect.ticket).await.expect("text").text, "€");
}

#[test]
fn callback_lifetime_keeps_retired_root_until_actual_last_holder_is_released() {
    let root = registration(712);
    let weak = Arc::downgrade(&root.0);
    let callback = surface(&root.0);
    callback.key(key(1, DesktopKeyPhase::Down)).expect("queued");
    drop(root);
    assert!(weak.upgrade().is_some());
    assert_eq!(
        callback.key(key(2, DesktopKeyPhase::Down)),
        Err(Reject::Retired)
    );
    drop(callback);
    assert!(weak.upgrade().is_none());
}

#[tokio::test]
async fn late_old_cycle_rejection_does_not_latch_same_physical_key_in_new_focus() {
    let root = registration(713);
    let old_surface = surface(&root.0);
    old_surface
        .key(key(1, DesktopKeyPhase::Down))
        .expect("原 down");
    let inflight = root.0.next().await;
    old_surface.revoke().expect("原 epoch 撤权");
    let current = surface(&root.0);
    let admitted = current
        .key(key(1, DesktopKeyPhase::Down))
        .expect("新 epoch 独立 cycle");
    root.0.reject_ticket(inflight.ticket);
    assert!(root.0.applicable(admitted.ticket));
    current
        .key(key(1, DesktopKeyPhase::Repeat))
        .expect("旧 Unsupported 不锁存新键");
    drop(inflight);
    current.key(key(1, DesktopKeyPhase::Up)).expect("原生新 up");
}

#[tokio::test]
async fn same_cycle_numbers_from_two_registered_sources_never_authorize_each_others_text() {
    let first = registration(714);
    let second = registration(715);
    let a = surface(&first.0);
    let b = surface(&second.0);
    let mut input = key(1, DesktopKeyPhase::Down);
    input.text_ownership = policy::TextOwnership::NativeText;
    let a_ticket = a.key(input.clone()).expect("A native").ticket;
    let b_ticket = b.key(input).expect("B native").ticket;
    assert_eq!(a_ticket.cycle, b_ticket.cycle);
    assert_eq!(a_ticket.binding, b_ticket.binding);
    assert!(!second.0.applicable(a_ticket));
    assert_eq!(
        b.composition(a_ticket, Some("错来源".into()), true),
        Err(Reject::InvalidTicket)
    );
    first.0.reject_ticket(b_ticket);
    assert!(first.0.applicable(a_ticket));
    b.composition(b_ticket, Some("原 B 文本".into()), true)
        .expect("原票据仍有效");
    assert_eq!(
        second.0.text(b_ticket).await.expect("B 完整文本").text,
        "原 B 文本"
    );
    a.composition(a_ticket, None, false)
        .expect("原 A 取消不排队");
    assert!(matches!(
        first.0.text(a_ticket).await,
        Err(Reject::Cancelled)
    ));
}

#[test]
fn native_callback_on_another_thread_cannot_mutate_the_ui_owned_cycle() {
    let root = registration(716);
    let source = surface(&root.0);
    let callback = source.clone();
    let result = std::thread::spawn(move || callback.key(key(1, DesktopKeyPhase::Down)))
        .join()
        .expect("测试 worker 已 join");
    assert_eq!(result, Err(Reject::WrongThread));
    source
        .key(key(1, DesktopKeyPhase::Down))
        .expect("原 UI 回调未被污染");
}

#[tokio::test]
async fn composition_continues_across_presses_and_repeat_then_up_only_releases_its_press() {
    let root = registration(741);
    let source = surface(&root.0);
    let input = |physical, phase| {
        let mut input = key(physical, phase);
        input.text_ownership = policy::TextOwnership::NativeText;
        input
    };
    let first = source
        .key_with_context(input(1, DesktopKeyPhase::Down), 100, 0)
        .expect("Start");
    assert_eq!(first.text_op, super::TextOp::Start);
    source
        .composition(first.ticket, Some("预编辑".into()), false)
        .expect("Preedit");
    let second = source
        .key_with_context(input(2, DesktopKeyPhase::Down), 100, first.effect)
        .expect("Continue");
    let repeat = source
        .key_with_context(input(1, DesktopKeyPhase::Repeat), 100, first.effect)
        .expect("Repeat Continue");
    assert_eq!(first.ticket, second.ticket);
    assert_eq!(first.effect, repeat.effect);
    assert_ne!(first.cycle_id, second.cycle_id);
    assert_eq!(first.cycle_id, repeat.cycle_id);
    assert_eq!(second.text_op, super::TextOp::Continue);
    let up = source
        .key_with_context(input(1, DesktopKeyPhase::Up), 100, first.effect)
        .expect("原 context Up");
    assert_eq!(up.route, Route::NativeText);
    assert_eq!(up.text_op, super::TextOp::None);
    assert_eq!(root.0.output_boundary().0, first.effect);
    source
        .composition(first.ticket, Some("合并🙂".into()), true)
        .expect("唯一 Finish");
    let effect = root.0.next().await;
    assert_eq!(
        root.0
            .text(effect.ticket)
            .await
            .expect("唯一 delivery")
            .text,
        "合并🙂"
    );
    assert_eq!(
        source.composition(first.ticket, Some("重复".into()), true),
        Err(Reject::InvalidTicket)
    );
    let old_up = source
        .key_with_context(input(2, DesktopKeyPhase::Up), 101, 0)
        .expect("旧 press Up");
    assert_eq!(old_up.route, Route::Consumed);
    assert_eq!(root.0.output_boundary().0, first.effect);
    drop(effect);
}

#[tokio::test]
async fn repeat_matches_changed_chord_and_requires_the_original_quiet_fence_before_action() {
    use rshell_m0::rshell_core::KeyBinding;
    let mut profile = TerminalSettingsV1::default().resolve(&TerminalOverrides::default());
    profile.key_bindings.push(KeyBinding {
        code: KeyCode::Character('a'),
        modifiers: KeyModifiers {
            control: true,
            ..KeyModifiers::default()
        },
        action: "send:\x7f".into(),
    });
    let root = registration_with_profile(742, profile);
    let source = surface(&root.0);
    let mut input = key(1, DesktopKeyPhase::Down);
    input.text_ownership = policy::TextOwnership::NativeText;
    let start = source
        .key_with_context(input.clone(), 1, 0)
        .expect("文本 Start");
    let effect = root.0.next().await;
    let mut waiter = Box::pin(root.0.text(effect.ticket));
    input.phase = DesktopKeyPhase::Repeat;
    input.raw_chord.as_mut().expect("raw").modifiers.control = true;
    let interrupted = source
        .key_with_context(input.clone(), 1, start.effect)
        .expect("Repeat 重新 matcher");
    assert_eq!(interrupted.route, Route::Terminal);
    assert_eq!(interrupted.text_op, super::TextOp::End);
    let end = interrupted.end.expect("原 context fence");
    assert_eq!(
        root.0.text_end(source.binding()).expect("即时查询"),
        Some(end)
    );
    assert_eq!(
        source.composition(start.ticket, Some("迟到".into()), true),
        Err(Reject::InvalidTicket)
    );
    tokio::select! { biased; _ = &mut waiter => panic!("没有 quiet 不得越过文本"), _ = tokio::task::yield_now() => {} }
    let mut wrong = end;
    wrong.context_id += 1;
    assert_eq!(source.text_end_ack(wrong), Err(Reject::InvalidTicket));
    source.text_end_ack(end).expect("真实 quiet 回执");
    assert!(matches!(waiter.await, Err(Reject::Cancelled)));
    let next = root.0.next().await;
    assert_eq!(next.seq, interrupted.effect);
    assert!(matches!(
        next.kind,
        super::EffectKind::Key(policy::GatedIntent::Send { .. })
    ));
    input.raw_chord.as_mut().expect("raw").modifiers.control = false;
    let repeat = source
        .key_with_context(input.clone(), 2, 0)
        .expect("已消费 press Repeat");
    assert_eq!(repeat.route, Route::Consumed);
    input.physical_identity = NonZeroU64::new(2);
    input.phase = DesktopKeyPhase::Down;
    let new = source
        .key_with_context(input, 2, 0)
        .expect("新 press 新 context");
    assert_ne!(new.cycle_id, start.cycle_id);
    assert_eq!(new.text_op, super::TextOp::Start);
    assert_ne!(new.ticket, start.ticket);
    source
        .composition(new.ticket, None, false)
        .expect("原事务取消");
    drop((effect, next));
}

#[test]
fn native_repeat_rechecks_source_and_old_context_cannot_resume_or_receive_up() {
    let root = registration(743);
    let source = surface(&root.0);
    let mut input = key(1, DesktopKeyPhase::Down);
    input.text_ownership = policy::TextOwnership::NativeText;
    let start = source
        .key_with_context(input.clone(), 10, 0)
        .expect("Start");
    source
        .composition(start.ticket, Some("已提交".into()), true)
        .expect("Commit");
    input.phase = DesktopKeyPhase::Repeat;
    assert_eq!(
        source.key_with_context(input.clone(), 10, start.effect),
        Err(Reject::InvalidTicket)
    );
    let new = source
        .key_with_context(input.clone(), 11, 0)
        .expect("轮换无需松键");
    input.phase = DesktopKeyPhase::Up;
    let up = source
        .key_with_context(input, 10, start.effect)
        .expect("真实旧 Up");
    assert_eq!(up.route, Route::Consumed);
    assert_eq!(up.text_op, super::TextOp::None);
    source
        .composition(new.ticket, None, false)
        .expect("取消新 context");
    let mut input = key(2, DesktopKeyPhase::Down);
    input.text_ownership = policy::TextOwnership::NativeText;
    source
        .key_with_context(input.clone(), 12, 0)
        .expect("新 press");
    input.phase = DesktopKeyPhase::Repeat;
    input.raw_chord = None;
    assert_eq!(
        source.key_with_context(input, 12, 0),
        Err(Reject::Policy(policy::InputReject::MissingRawChord))
    );
    assert_eq!(
        source.key_with_context(key(2, DesktopKeyPhase::Repeat), 12, 0),
        Err(Reject::Cancelled)
    );
}

#[tokio::test]
async fn focus_revoke_preserves_queued_committed_delivery_but_attempt_end_cancels_without_replay() {
    let root = registration(744);
    let source = surface(&root.0);
    let mut input = key(1, DesktopKeyPhase::Down);
    input.text_ownership = policy::TextOwnership::NativeText;
    let committed = source.key_with_context(input, 1, 0).expect("Start");
    source
        .composition(committed.ticket, Some("完整 delivery".into()), true)
        .expect("Commit");
    source.revoke().expect("失焦");
    let effect = root.0.next().await;
    assert_eq!(effect.seq, committed.effect);
    assert!(root.0.applicable_effect(effect.ticket, &effect.kind));
    let text = root.0.text(effect.ticket).await.expect("失焦后可领取");
    assert_eq!(text.text, "完整 delivery");
    assert!(root.0.applicable_effect(effect.ticket, &effect.kind));
    root.0.begin_attempt();
    assert!(!root.0.applicable_effect(effect.ticket, &effect.kind));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(1), root.0.next())
            .await
            .is_err()
    );
    drop((text, effect));
}

#[tokio::test]
async fn source_end_cancels_queued_committed_text_and_cannot_replay_into_another_registration() {
    let root = registration(745);
    let port = root.0.clone();
    let source = surface(&port);
    let mut input = key(1, DesktopKeyPhase::Down);
    input.text_ownership = policy::TextOwnership::NativeText;
    let original = source.key_with_context(input, 1, 0).expect("原文本");
    source
        .composition(original.ticket, Some("旧 source 正文".into()), true)
        .expect("原 Committed");
    drop(root);
    assert!(matches!(
        port.text(original.ticket).await,
        Err(Reject::Retired)
    ));
    assert!(!port.output_boundary().1);
    let next = registration(746);
    let next_source = surface(&next.0);
    assert_eq!(
        next_source.composition(original.ticket, Some("不得重放".into()), true),
        Err(Reject::InvalidTicket)
    );
    assert!(!next.0.output_boundary().1);
}
