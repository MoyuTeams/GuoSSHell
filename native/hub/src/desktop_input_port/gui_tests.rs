#![allow(clippy::expect_used)]
use super::*;
use crate::signals::desktop_native_input::{
    GuiInput, GuiIntent, GuiKey, GuiModifiers, NativeCommand,
};
use rshell_m0::rshell_core::{KeyCode, TerminalInput};

fn intent(input: GuiInput, button: u8, latched: u8, locked: u8) -> NativeCommand {
    NativeCommand::GuiIntent(GuiIntent {
        input,
        modifiers: GuiModifiers {
            button,
            latched,
            locked,
        },
        modifier_revision: 7,
    })
}

fn attach(port: &Arc<InputPort>) -> Arc<Surface> {
    port.connected();
    unsafe {
        port.attach_verified(port.binding().attempt)
            .expect("受控宿主")
    }
}

#[tokio::test]
async fn gui_modifiers_freeze_effective_bits_and_consume_only_uncovered_latches() {
    for (index, input, button, latched, locked, consumed, code, bits) in [
        (
            0,
            GuiInput::GuiKey(GuiKey::Enter),
            0,
            3,
            0,
            3,
            KeyCode::Enter,
            3,
        ),
        (
            1,
            GuiInput::GuiKey(GuiKey::CtrlC),
            0,
            1,
            0,
            0,
            KeyCode::Character('c'),
            1,
        ),
        (
            2,
            GuiInput::GuiKey(GuiKey::CtrlC),
            0,
            3,
            0,
            2,
            KeyCode::Character('c'),
            3,
        ),
        (
            3,
            GuiInput::GuiKey(GuiKey::Enter),
            1,
            3,
            2,
            0,
            KeyCode::Enter,
            3,
        ),
        (
            4,
            GuiInput::GuiText("C".into()),
            0,
            3,
            1,
            2,
            KeyCode::Character('C'),
            3,
        ),
        (
            5,
            GuiInput::GuiText("🙂".into()),
            0,
            2,
            0,
            2,
            KeyCode::Character('🙂'),
            2,
        ),
    ] {
        let root = tests::registration(9900 + index);
        let surface = attach(&root.0);
        let admitted = root
            .0
            .command(surface.binding(), 1, intent(input, button, latched, locked))
            .expect("GUI 准入");
        assert_eq!(admitted.consumed_latches, consumed);
        let effect = root.0.next().await;
        assert_eq!(effect.seq, admitted.effect);
        assert_eq!(effect.ticket.cycle, 0, "不是物理 press");
        assert_eq!(effect.ticket.transaction, 0, "不是 native text transaction");
        let EffectKind::GuiIntent(TerminalInput::Key {
            code: actual,
            modifiers,
        }) = &effect.kind
        else {
            panic!("明确字符/固定键意图必须冻结成 engine 键语义");
        };
        assert_eq!(*actual, code);
        assert_eq!(modifiers.control, bits & 1 != 0);
        assert_eq!(modifiers.alt, bits & 2 != 0);
        assert!(!modifiers.shift && !modifiers.super_key);
        assert!(
            root.0
                .state
                .lock()
                .expect("原状态")
                .cycles
                .iter()
                .all(Option::is_none)
        );
        drop(effect);
    }
}

#[tokio::test]
async fn gui_multiscalar_literal_preserves_text_and_never_consumes_modifiers() {
    let root = tests::registration(9906);
    let surface = attach(&root.0);
    let text = "大写Aa\n🙂";
    let admitted = root
        .0
        .command(
            surface.binding(),
            1,
            intent(GuiInput::GuiText(text.into()), 3, 3, 3),
        )
        .expect("原文");
    assert_eq!(admitted.consumed_latches, 0);
    let effect = root.0.next().await;
    assert!(
        matches!(&effect.kind, EffectKind::GuiIntent(TerminalInput::CommittedText(value)) if value == text)
    );
    drop(effect);
}

#[tokio::test]
async fn rejected_gui_kind_modifiers_and_text_do_not_consume_request_effect_or_fence() {
    let root = tests::registration(9907);
    let surface = attach(&root.0);
    let mut native = tests::key(1, DesktopKeyPhase::Down);
    native.text_ownership = policy::TextOwnership::NativeText;
    let text = surface
        .key_with_context(native, 1, 0)
        .expect("原 Open text");
    for (command, expected) in [
        (
            intent(GuiInput::GuiKey(GuiKey::F(13)), 0, 1, 0),
            Reject::InvalidEvent,
        ),
        (
            intent(GuiInput::GuiKey(GuiKey::Enter), 4, 1, 0),
            Reject::InvalidEvent,
        ),
        (
            intent(GuiInput::GuiText(String::new()), 0, 1, 0),
            Reject::InvalidEvent,
        ),
        (
            intent(GuiInput::GuiText("🙂".repeat(33)), 0, 1, 0),
            Reject::TooLarge,
        ),
        (
            NativeCommand::Event(DesktopEvent::Text {
                text: "伪造".into(),
            }),
            Reject::InvalidEvent,
        ),
        (
            NativeCommand::Event(DesktopEvent::Key {
                key: "enter".into(),
                physical: 1,
                logical: 1,
                shift: false,
                control: false,
                alt: false,
                super_key: false,
                phase: DesktopKeyPhase::Down,
                left_alt: false,
                right_alt: false,
                alt_source: crate::signals::desktop_session::DesktopAltSource::Unknown,
                default_action: None,
            }),
            Reject::InvalidEvent,
        ),
    ] {
        assert_eq!(root.0.command(surface.binding(), 1, command), Err(expected));
        let state = root.0.state.lock().expect("拒绝不改变准入");
        assert_eq!(state.last_request, 0);
        assert_eq!(state.next_effect, text.effect);
        assert_eq!(state.active_text, Some(text.ticket));
        assert_eq!(state.next_fence, 0);
    }
    let admitted = root
        .0
        .command(
            surface.binding(),
            1,
            intent(GuiInput::GuiKey(GuiKey::F(12)), 0, 1, 0),
        )
        .expect("原 ID 可合法准入");
    assert_eq!(admitted.effect, text.effect + 1);
    assert_eq!(admitted.consumed_latches, 1);
    assert!(
        root.0
            .text_end(surface.binding())
            .expect("原 fence")
            .is_some()
    );
}

#[tokio::test]
async fn gui_shares_native_count_and_command_byte_budget_and_refunds_last_holder() {
    let root = tests::registration(9908);
    let surface = attach(&root.0);
    surface
        .key(tests::key(1, DesktopKeyPhase::Down))
        .expect("native");
    for _ in 1..INPUT_EVENTS {
        surface
            .key(tests::key(1, DesktopKeyPhase::Repeat))
            .expect("native repeat");
    }
    assert_eq!(
        root.0.command(
            surface.binding(),
            1,
            intent(GuiInput::GuiKey(GuiKey::Enter), 0, 1, 0)
        ),
        Err(Reject::Busy)
    );
    surface
        .command(2, DesktopEvent::FrameAck { frame_seq: 1 })
        .expect("额度满时控制旁路");
    drop(root.0.next_control().await);
    let effect = root.0.next().await;
    let lease = effect.lease.clone();
    drop(effect);
    assert_eq!(
        root.0.command(
            surface.binding(),
            3,
            intent(GuiInput::GuiKey(GuiKey::Enter), 0, 1, 0)
        ),
        Err(Reject::Busy)
    );
    drop(lease);
    let admitted = root
        .0
        .command(
            surface.binding(),
            3,
            intent(GuiInput::GuiKey(GuiKey::Enter), 0, 1, 0),
        )
        .expect("最后持有者归还槽位");
    assert_eq!(admitted.consumed_latches, 1);
    for _ in 0..INPUT_EVENTS {
        drop(root.0.next().await);
    }
    let text = "p".repeat((INPUT_BYTES - 2 * EVENT_BYTES) / 3);
    surface
        .command(4, DesktopEvent::Paste { text })
        .expect("占用真实共享字节额度");
    let command = intent(GuiInput::GuiText("🙂".repeat(32)), 0, 1, 0);
    assert_eq!(
        root.0.command(surface.binding(), 5, command.clone()),
        Err(Reject::Busy)
    );
    let effect = root.0.next().await;
    let lease = effect.lease.clone();
    drop(effect);
    assert_eq!(
        root.0.command(surface.binding(), 5, command.clone()),
        Err(Reject::Busy)
    );
    drop(lease);
    let admitted = root
        .0
        .command(surface.binding(), 5, command)
        .expect("完整 UTF-8 文本一次准入");
    assert_eq!(admitted.consumed_latches, 0);
    drop(root.0.next().await);
    let state = root.0.state.lock().expect("额度回收");
    assert_eq!(state.bytes, 0);
    assert_eq!(state.data_count, 0);
}

#[tokio::test]
async fn gui_disconnect_seals_queued_and_late_input_without_consuming_late_request() {
    let root = tests::registration(9915);
    let surface = attach(&root.0);
    let command = intent(GuiInput::GuiKey(GuiKey::Enter), 0, 1, 0);
    root.0
        .command(surface.binding(), 1, command.clone())
        .expect("原准入");
    surface
        .command(2, DesktopEvent::Disconnect)
        .expect("原关闭控制");
    let effect = root.0.next().await;
    assert!(!root.0.applicable_effect(effect.ticket, &effect.kind));
    assert_eq!(
        root.0.command(surface.binding(), 3, command),
        Err(Reject::NotAttached)
    );
    let state = root.0.state.lock().expect("关闭不消费迟到 GUI");
    assert_eq!(state.last_request, 2);
    assert_eq!(state.next_effect, 2);
    drop(state);
    drop(effect);
}

#[tokio::test]
async fn gui_rejects_prompt_stale_focus_attempt_mode_and_retirement_without_admission() {
    let root = tests::registration(9909);
    let port = root.0.clone();
    port.begin_attempt();
    let surface = unsafe {
        port.attach_verified(port.binding().attempt)
            .expect("认证提示前宿主")
    };
    let command = intent(GuiInput::GuiKey(GuiKey::Enter), 0, 1, 0);
    assert_eq!(
        port.command(surface.binding(), 1, command.clone()),
        Err(Reject::NotAuthenticated)
    );
    port.connected();
    let admitted = port
        .command(surface.binding(), 1, command.clone())
        .expect("同 ID 认证后准入");
    let effect = port.next().await;
    assert_eq!(effect.seq, admitted.effect);
    surface.revoke().expect("prompt/quiet 撤权");
    assert!(!port.applicable_effect(effect.ticket, &effect.kind));
    assert_eq!(
        port.command(surface.binding(), 2, command.clone()),
        Err(Reject::StaleFocus)
    );
    drop(effect);
    port.disconnected();
    port.begin_attempt();
    port.connected();
    assert_eq!(
        port.command(surface.binding(), 2, command.clone()),
        Err(Reject::StaleAttempt)
    );
    let current = unsafe {
        port.attach_verified(port.binding().attempt)
            .expect("原 source 新 attempt")
    };
    drop(root);
    assert_eq!(
        port.command(current.binding(), 2, command.clone()),
        Err(Reject::Retired)
    );
    let root = tests::registration(9910);
    let current = attach(&root.0);
    assert!(root.0.mode.legacy());
    assert_eq!(
        root.0.command(current.binding(), 1, command),
        Err(Reject::MixedMode)
    );
    let state = root.0.state.lock().expect("混合模式不准入");
    assert_eq!(state.last_request, 0);
    assert_eq!(state.next_effect, 0);
    assert_eq!(state.data_count, 0);
}
