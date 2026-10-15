#![allow(clippy::expect_used)]
use super::{Arc, DesktopAction, DesktopEvent, InputPort, Screen, policy, port};
use crate::desktop_input_port::tests::{key, registration, registration_with_profile};
use crate::desktop_input_port::transactions::ActionReply;
use crate::session::{
    DefaultTerminalEngine, Duration, SessionTransport, TerminalSize, TransportEvent, VecDeque,
    desktop_runtime, unbounded_channel,
};
use crate::signals::desktop_native_input::{
    DesktopNativeCommand, GuiInput, GuiIntent, GuiKey, GuiModifiers, NativeCommand,
    NativeCommandAdmission, NativeCommandFailure,
};
use crate::signals::desktop_session::{DesktopKeyPhase, DesktopSource};
use async_trait::async_trait;
use rshell_m0::rshell_session::{
    InteractionBroker, TransportCapabilities, TransportError, TransportRequest,
};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::sync::{Notify, watch};

// Rinf 的同一信号只有一个接收队列；独占真实 decoder 的获取、注入与消费区间。
static NATIVE_COMMAND_DECODER: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test]
async fn committed_text_delivery_survives_focus_revoke_in_all_three_actor_windows_exactly_once() {
    for window in 0..3 {
        let root = registration(880 + window);
        root.0.connected();
        let surface = unsafe { root.0.attach_verified(0).expect("原宿主") };
        let mut input = key(1, DesktopKeyPhase::Down);
        input.text_ownership = policy::TextOwnership::NativeText;
        let admitted = surface.key_with_context(input, 1, 0).expect("Start");
        let text = "Committed🙂完整";
        surface
            .composition(admitted.ticket, Some(text.into()), true)
            .expect("Commit 线性化");
        let wire = Arc::new(Wire::default());
        let mut transport = Transport {
            wire: wire.clone(),
            output: None,
            pause: window == 2,
        };
        let mut size = size();
        let mut screen = Screen::new(
            DefaultTerminalEngine::new(&root.0.profile, size).expect("原引擎"),
            size,
        );
        if window == 0 {
            surface.revoke().expect("queued 撤权");
        }
        let effect = root.0.next().await;
        let ready = port::CURRENT
            .scope(root.0.clone(), async { super::begin(None, effect).await })
            .await;
        if window == 1 {
            surface.revoke().expect("已领取未 apply 撤权");
        }
        let mut applying = Box::pin(port::CURRENT.scope(
            root.0.clone(),
            super::apply(880 + window, &mut screen, &mut transport, &mut size, ready),
        ));
        if window == 2 {
            tokio::select! { biased; _ = &mut applying => panic!("必须暂停原 write future"), _ = wire.entered.notified() => {} }
            surface.revoke().expect("inflight 撤权");
            assert_eq!(wire.calls.load(Ordering::Acquire), 1);
            wire.resume.notify_one();
        }
        assert!(matches!(applying.await, Ok(false)), "原 attempt 投递");
        assert_eq!(*wire.bytes.lock().expect("精确一次正文"), text.as_bytes());
        assert_eq!(wire.calls.load(Ordering::Acquire), 1);
        assert_eq!(root.0.applied(), admitted.effect);
        root.0.begin_attempt();
        root.0.connected();
        assert!(
            tokio::time::timeout(Duration::from_millis(1), root.0.next())
                .await
                .is_err()
        );
        assert_eq!(wire.calls.load(Ordering::Acquire), 1, "不跨 reconnect 重放");
    }
}

#[tokio::test]
async fn acquired_committed_delivery_is_cancelled_when_its_source_attempt_ends_before_apply() {
    let root = registration(884);
    root.0.connected();
    let surface = unsafe { root.0.attach_verified(0).expect("宿主") };
    let mut input = key(1, DesktopKeyPhase::Down);
    input.text_ownership = policy::TextOwnership::NativeText;
    let admitted = surface.key_with_context(input, 1, 0).expect("Start");
    surface
        .composition(admitted.ticket, Some("旧 attempt".into()), true)
        .expect("Commit");
    let effect = root.0.next().await;
    let ready = port::CURRENT
        .scope(root.0.clone(), async { super::begin(None, effect).await })
        .await;
    root.0.disconnected();
    root.0.begin_attempt();
    root.0.connected();
    let wire = Arc::new(Wire::default());
    let mut transport = Transport {
        wire: wire.clone(),
        output: None,
        pause: false,
    };
    let mut size = size();
    let mut screen = Screen::new(
        DefaultTerminalEngine::new(&root.0.profile, size).expect("原引擎"),
        size,
    );
    assert!(
        port::CURRENT
            .scope(
                root.0.clone(),
                super::apply(884, &mut screen, &mut transport, &mut size, ready)
            )
            .await
            .is_ok(),
        "取消旧 delivery"
    );
    assert_eq!(wire.calls.load(Ordering::Acquire), 0);
    assert!(wire.bytes.lock().expect("不重放").is_empty());
}

fn gui_request(
    port: &InputPort,
    binding: port::Binding,
    request_id: u64,
    input: GuiInput,
    modifiers: GuiModifiers,
) -> DesktopNativeCommand {
    DesktopNativeCommand {
        version: crate::signals::desktop_native_input::VERSION,
        source: port.source.source.clone(),
        identity: port.source.identity.clone(),
        attempt: binding.attempt,
        focus_epoch: binding.epoch,
        request_id,
        command: NativeCommand::GuiIntent(GuiIntent {
            input,
            modifiers,
            modifier_revision: 42,
        }),
    }
}

fn route_command(
    request: DesktopNativeCommand,
) -> crate::signals::desktop_native_input::DesktopNativeCommandResult {
    let _registration = crate::desktop_input_port::tests::isolate_source_registration();
    super::route_command(request)
}

#[tokio::test]
async fn gui_rinf_keys_and_literal_text_use_original_actor_encoder_without_physical_matching() {
    use rinf::DartSignal;
    use rshell_m0::rshell_core::{
        KeyBinding, KeyCode, KeyModifiers, TerminalOverrides, TerminalSettingsV1,
    };
    let mut profile = TerminalSettingsV1::default().resolve(&TerminalOverrides::default());
    profile.key_bindings.push(KeyBinding {
        code: KeyCode::Enter,
        modifiers: KeyModifiers::default(),
        action: "close_tab".into(),
    });
    let root = registration_with_profile(9911, profile);
    root.0.connected();
    let surface = unsafe { root.0.attach_verified(0).expect("原宿主") };
    let mut screen = Screen::new(
        DefaultTerminalEngine::new(&root.0.profile, size()).expect("真实引擎"),
        size(),
    );
    screen
        .advance(b"\x1b[?1h\x1b[?2004h")
        .expect("真实 application cursor/bracketed paste 模式");
    let wire = Arc::new(Wire::default());
    let (close, task) = actor_with_screen(
        root.0.clone(),
        Transport {
            wire: wire.clone(),
            output: None,
            pause: false,
        },
        true,
        Some(screen),
    );
    let decoder_guard = NATIVE_COMMAND_DECODER.lock().await;
    let receiver = DesktopNativeCommand::get_dart_signal_receiver();
    let keys: &[(GuiKey, &[u8])] = &[
        (GuiKey::Enter, b"\r"),
        (GuiKey::Escape, b"\x1b"),
        (GuiKey::Tab, b"\t"),
        (GuiKey::Backspace, b"\x7f"),
        (GuiKey::ArrowUp, b"\x1bOA"),
        (GuiKey::ArrowDown, b"\x1bOB"),
        (GuiKey::ArrowLeft, b"\x1bOD"),
        (GuiKey::ArrowRight, b"\x1bOC"),
        (GuiKey::Delete, b"\x1b[3~"),
        (GuiKey::Home, b"\x1bOH"),
        (GuiKey::End, b"\x1bOF"),
        (GuiKey::PageUp, b"\x1b[5~"),
        (GuiKey::PageDown, b"\x1b[6~"),
        (GuiKey::F(1), b"\x1bOP"),
        (GuiKey::F(2), b"\x1bOQ"),
        (GuiKey::F(3), b"\x1bOR"),
        (GuiKey::F(4), b"\x1bOS"),
        (GuiKey::F(5), b"\x1b[15~"),
        (GuiKey::F(6), b"\x1b[17~"),
        (GuiKey::F(7), b"\x1b[18~"),
        (GuiKey::F(8), b"\x1b[19~"),
        (GuiKey::F(9), b"\x1b[20~"),
        (GuiKey::F(10), b"\x1b[21~"),
        (GuiKey::F(11), b"\x1b[23~"),
        (GuiKey::F(12), b"\x1b[24~"),
        (GuiKey::CtrlC, b"\x03"),
        (GuiKey::CtrlD, b"\x04"),
        (GuiKey::CtrlZ, b"\x1a"),
        (GuiKey::CtrlL, b"\x0c"),
    ];
    let mut expected = Vec::new();
    for (index, &(key, bytes)) in keys.iter().enumerate() {
        let request = gui_request(
            &root.0,
            surface.binding(),
            index as u64 + 1,
            GuiInput::GuiKey(key),
            GuiModifiers::default(),
        );
        crate::signals::desktop_native_input::inject_test_request(&request);
        let decoded = receiver.recv().await.expect("真实 Rinf GUI 解码").message;
        assert_eq!(decoded.source, request.source);
        assert_eq!(decoded.identity, request.identity);
        assert_eq!(decoded.request_id, request.request_id);
        let result = route_command(decoded);
        assert_eq!(result.admission, NativeCommandAdmission::Accepted);
        assert_eq!(result.failure, NativeCommandFailure::None);
        assert_eq!(result.request_id, request.request_id);
        assert_eq!(result.attempt, request.attempt);
        assert_eq!(result.focus_epoch, request.focus_epoch);
        assert_eq!(result.gui_modifier_revision, Some(42));
        assert_eq!(result.consumed_latches, 0);
        until(|| root.0.applied() >= result.effect || task.is_finished()).await;
        assert!(!task.is_finished(), "GUI 键不进入 physical 壳 matcher");
        expected.extend_from_slice(bytes);
        assert_eq!(*wire.bytes.lock().expect("真实 encoder 字节"), expected);
        let duplicate = route_command(request);
        assert_eq!(duplicate.admission, NativeCommandAdmission::Unknown);
        assert_eq!(duplicate.failure, NativeCommandFailure::Duplicate);
        assert_eq!(duplicate.effect, 0);
        assert_eq!(duplicate.consumed_latches, 0);
        assert_eq!(
            wire.calls.load(Ordering::Acquire),
            index + 1,
            "重复 ID 不重新 write"
        );
    }
    drop(receiver);
    drop(decoder_guard);
    let text = "Aa你好🙂\n|/-~.";
    let request = gui_request(
        &root.0,
        surface.binding(),
        keys.len() as u64 + 1,
        GuiInput::GuiText(text.into()),
        GuiModifiers {
            latched: 3,
            locked: 3,
            ..GuiModifiers::default()
        },
    );
    let result = route_command(request);
    assert_eq!(result.admission, NativeCommandAdmission::Accepted);
    assert_eq!(result.consumed_latches, 0);
    until(|| root.0.applied() >= result.effect || task.is_finished()).await;
    assert!(!task.is_finished());
    expected.extend_from_slice(text.as_bytes());
    assert_eq!(*wire.bytes.lock().expect("不加回车或 paste 包装"), expected);
    close.send_replace(true);
    task.await.expect("原 actor 正常关闭");
}

#[tokio::test]
async fn gui_scalar_kitty_encoding_preserves_explicit_ctrl_latch() {
    use rshell_m0::rshell_core::{TerminalOverrides, TerminalSettingsV1};
    let mut profile = TerminalSettingsV1::default().resolve(&TerminalOverrides::default());
    profile.enable_kitty_keyboard = true;
    let root = registration_with_profile(9912, profile);
    root.0.connected();
    let surface = unsafe { root.0.attach_verified(0).expect("原宿主") };
    let mut screen = Screen::new(
        DefaultTerminalEngine::new(&root.0.profile, size()).expect("真实引擎"),
        size(),
    );
    screen.advance(b"\x1b[>3u").expect("真实 Kitty 协商");
    let wire = Arc::new(Wire::default());
    let (close, task) = actor_with_screen(
        root.0.clone(),
        Transport {
            wire: wire.clone(),
            output: None,
            pause: false,
        },
        true,
        Some(screen),
    );
    for (id, input, consumed, expected) in [
        (
            1,
            GuiInput::GuiText("a".into()),
            1,
            b"\x1b[97;5u".as_slice(),
        ),
        (
            2,
            GuiInput::GuiKey(GuiKey::CtrlC),
            0,
            b"\x1b[97;5u\x1b[99;5u".as_slice(),
        ),
    ] {
        let result = route_command(gui_request(
            &root.0,
            surface.binding(),
            id,
            input,
            GuiModifiers {
                latched: 1,
                ..GuiModifiers::default()
            },
        ));
        assert_eq!(result.admission, NativeCommandAdmission::Accepted);
        assert_eq!(result.consumed_latches, consumed);
        until(|| root.0.applied() >= result.effect || task.is_finished()).await;
        assert!(!task.is_finished());
        assert_eq!(*wire.bytes.lock().expect("原 engine 协议字节"), expected);
    }
    close.send_replace(true);
    task.await.expect("原 actor 正常关闭");
}

#[tokio::test]
async fn gui_admission_seals_original_composition_and_waits_only_for_native_end_fence() {
    let root = registration(9913);
    root.0.connected();
    let surface = unsafe { root.0.attach_verified(0).expect("原宿主") };
    let mut native = key(1, DesktopKeyPhase::Down);
    native.text_ownership = policy::TextOwnership::NativeText;
    let text = surface.key_with_context(native, 1, 0).expect("原 text");
    surface
        .composition(text.ticket, Some("原 preedit".into()), false)
        .expect("原 preedit");
    let wire = Arc::new(Wire::default());
    let (close, task) = actor(
        root.0.clone(),
        Transport {
            wire: wire.clone(),
            output: None,
            pause: false,
        },
    );
    let result = route_command(gui_request(
        &root.0,
        surface.binding(),
        1,
        GuiInput::GuiText("x".into()),
        GuiModifiers {
            latched: 2,
            ..GuiModifiers::default()
        },
    ));
    assert_eq!(result.admission, NativeCommandAdmission::Accepted);
    assert_eq!(result.effect, text.effect + 1);
    assert_eq!(result.consumed_latches, 2);
    let end = root
        .0
        .text_end(surface.binding())
        .expect("立即查询")
        .expect("原 context fence 已产生");
    assert_eq!(end.context_id, 1);
    assert_eq!(end.transaction_id, text.effect);
    assert!(end.end_fence != 0);
    assert_eq!(
        surface.composition(text.ticket, Some("迟到 commit".into()), true),
        Err(port::Reject::InvalidTicket)
    );
    surface
        .command(2, DesktopEvent::FrameAck { frame_seq: 1 })
        .expect("原控制旁路");
    until(|| root.0.projection().is_some()).await;
    assert!(root.0.applied() < result.effect);
    assert_eq!(wire.calls.load(Ordering::Acquire), 0);
    surface
        .text_end_ack(end)
        .expect("仅原 nativequiet，不等 GUI 自己的 ACK");
    until(|| root.0.applied() >= result.effect || task.is_finished()).await;
    assert!(!task.is_finished());
    assert_eq!(*wire.bytes.lock().expect("quiet 后原 GUI 字节"), b"\x1bx");
    close.send_replace(true);
    task.await.expect("原 actor 正常关闭");
}

#[tokio::test]
async fn gui_original_result_correlates_rejections_and_revoked_queued_input_never_replays() {
    let root = registration(9914);
    root.0.connected();
    let surface = unsafe { root.0.attach_verified(0).expect("原宿主") };
    let request = gui_request(
        &root.0,
        surface.binding(),
        1,
        GuiInput::GuiKey(GuiKey::Enter),
        GuiModifiers {
            latched: 1,
            ..GuiModifiers::default()
        },
    );
    {
        let _contention = crate::desktop_input_port::tests::hold_source_registration();
        let busy = super::route_command(request.clone());
        assert_eq!(busy.admission, NativeCommandAdmission::Rejected);
        assert_eq!(busy.failure, NativeCommandFailure::Busy);
        assert_eq!(busy.effect, 0);
        assert_eq!(busy.consumed_latches, 0);
        assert_eq!(busy.source, request.source);
        assert_eq!(busy.identity, request.identity);
        assert_eq!(busy.request_id, request.request_id);
    }
    let mut stale = request.clone();
    stale.identity = "其他 identity".into();
    let result = route_command(stale);
    assert_eq!(result.admission, NativeCommandAdmission::Rejected);
    assert_eq!(result.failure, NativeCommandFailure::StaleSource);
    assert_eq!(result.effect, 0);
    assert_eq!(result.consumed_latches, 0);
    let admitted = route_command(request.clone());
    assert_eq!(admitted.admission, NativeCommandAdmission::Accepted);
    assert_eq!(admitted.consumed_latches, 1);
    let effect = root.0.next().await;
    let ready = port::CURRENT
        .scope(root.0.clone(), async { super::begin(None, effect).await })
        .await;
    surface.revoke().expect("准入后、apply 前撤权");
    let wire = Arc::new(Wire::default());
    let mut transport = Transport {
        wire: wire.clone(),
        output: None,
        pause: false,
    };
    let mut size = size();
    let mut screen = Screen::new(
        DefaultTerminalEngine::new(&root.0.profile, size).expect("真实引擎"),
        size,
    );
    assert!(
        port::CURRENT
            .scope(
                root.0.clone(),
                super::apply(9914, &mut screen, &mut transport, &mut size, ready)
            )
            .await
            .is_ok()
    );
    assert_eq!(root.0.applied(), admitted.effect);
    assert_eq!(wire.calls.load(Ordering::Acquire), 0);
    let rejected = route_command(request.clone());
    assert_eq!(rejected.admission, NativeCommandAdmission::Rejected);
    assert_eq!(rejected.failure, NativeCommandFailure::NotAttached);
    assert_eq!(rejected.consumed_latches, 0);
    assert_eq!(
        admitted.gui_modifier_revision,
        Some(42),
        "原结果仍只属于原快照"
    );
    assert_eq!(admitted.request_id, request.request_id);
    assert_eq!(admitted.source, request.source);
    assert_eq!(admitted.identity, request.identity);
    assert_eq!(admitted.attempt, request.attempt);
    assert_eq!(admitted.focus_epoch, request.focus_epoch);
    root.0.disconnected();
    root.0.begin_attempt();
    root.0.connected();
    let rejected = route_command(request);
    assert_eq!(rejected.admission, NativeCommandAdmission::Rejected);
    assert_eq!(rejected.failure, NativeCommandFailure::StaleSource);
    assert_eq!(wire.calls.load(Ordering::Acquire), 0);
}

fn geometry(projection: &port::PublishedProjection) -> port::PresentedGeometry {
    port::PresentedGeometry {
        frame_revision: projection.frame_revision,
        row_revision: projection.row_revision,
        viewport_top: projection.frame().viewport_top,
        font_revision: 1,
        layout_revision: 1,
        transform_revision: 1,
        clip_x: 0.0,
        clip_y: 0.0,
        clip_width: 1000.0,
        clip_height: 160.0,
        transform: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
    }
}

#[tokio::test]
async fn rinf_non_key_writer_cannot_forge_physical_pointer_or_consume_its_request_id() {
    use crate::signals::desktop_native_input::{
        DesktopNativeCommand, NativeCommand, NativeCommandFailure,
    };
    let root = registration(887);
    root.0.connected();
    let surface = unsafe { root.0.attach_verified(0).expect("宿主") };
    let mut request = DesktopNativeCommand {
        version: 1,
        source: root.0.source.source.clone(),
        identity: root.0.source.identity.clone(),
        attempt: surface.binding().attempt,
        focus_epoch: surface.binding().epoch,
        request_id: 1,
        command: NativeCommand::Event(DesktopEvent::Mouse {
            kind: "press".into(),
            button: "left".into(),
            col: 0,
            row: 0,
            shift: false,
            control: false,
            alt: false,
        }),
    };
    let refused = route_command(request.clone());
    assert_eq!(refused.failure, NativeCommandFailure::InvalidEvent);
    assert_eq!(refused.effect, 0);
    request.command = NativeCommand::Event(DesktopEvent::Paste {
        text: "合法显式 paste".into(),
    });
    let accepted = route_command(request);
    assert_eq!(accepted.failure, NativeCommandFailure::None);
    assert_eq!(accepted.effect, 1);
    drop(root.0.next().await);
}

fn pointer(
    geometry: port::PresentedGeometry,
    stable_hit_row: i64,
    shift: bool,
) -> port::PointerInput {
    port::PointerInput {
        geometry,
        kind: 1,
        button: 1,
        modifiers: u32::from(shift),
        buttons: 1,
        stable_hit_row,
        column: 3,
        remote_column: 3,
        remote_row: 1,
        delta_x: 0,
        delta_y: 0,
    }
}

async fn check_pointer_tracking(mode: &[u8], button: u32, expected: Option<&[u8]>, id: u32) {
    use rshell_m0::rshell_session::{EngineError, TerminalEngine};
    let root = registration(id);
    root.0.connected();
    let surface = unsafe { root.0.attach_verified(0).expect("原宿主") };
    let mut screen = Screen::new(
        DefaultTerminalEngine::new(&root.0.profile, size()).expect("真实引擎"),
        size(),
    );
    screen.advance(mode).expect("真实 tracking 协商");
    let event = super::mouse_event_from_request(super::MouseRequest {
        session_id: id,
        kind: "move".into(),
        button: if button == 1 { "left" } else { "" }.into(),
        col: 3,
        row: 1,
        shift: false,
        control: false,
        alt: false,
    })
    .expect("真实鼠标事件");
    let encoded = screen.engine.encode_mouse(event);
    match expected {
        Some(bytes) => assert_eq!(encoded.expect("模式支持本事件"), bytes),
        None => assert!(matches!(encoded, Err(EngineError::UnsupportedMouse(_)))),
    }
    let wire = Arc::new(Wire::default());
    let (close, task) = actor_with_screen(
        root.0.clone(),
        Transport {
            wire: wire.clone(),
            output: None,
            pause: false,
        },
        true,
        Some(screen),
    );
    until(|| root.0.projection().is_some()).await;
    let projection = root.0.projection().expect("真实帧");
    let frozen = geometry(&projection);
    surface.presented_geometry(1, frozen).expect("原呈现几何");
    let mut input = pointer(frozen, projection.frame().rows[1].stable_row, false);
    input.kind = 2;
    input.button = button;
    input.buttons = u32::from(button == 1);
    let admitted = surface.pointer(2, input).expect("同步事件已准入");
    until(|| root.0.applied() >= admitted.effect || task.is_finished()).await;
    assert!(!task.is_finished(), "tracking 不支持本事件不应终止连接");
    assert_eq!(
        *wire.bytes.lock().expect("鼠标字节"),
        expected.unwrap_or(&[])
    );
    let following = surface
        .key(key(1, DesktopKeyPhase::Down))
        .expect("后续有效键");
    until(|| root.0.applied() >= following.effect || task.is_finished()).await;
    assert!(!task.is_finished(), "原连接必须继续执行后续有效输入");
    let mut bytes = expected.unwrap_or(&[]).to_vec();
    bytes.push(b'a');
    assert_eq!(*wire.bytes.lock().expect("同一连接的后续字节"), bytes);
    assert_eq!(
        wire.calls.load(Ordering::Acquire),
        usize::from(expected.is_some()) + 1
    );
    close.send_replace(true);
    task.await.expect("原 actor 正常关闭");
}

#[tokio::test]
async fn pointer_click_tracking_ignores_move_and_keeps_connection_accepting_input() {
    check_pointer_tracking(b"\x1b[?1000h\x1b[?1006h", 1, None, 9871).await;
}

#[tokio::test]
async fn queued_pointer_invalidated_by_clear_keeps_original_actor_alive_for_following_input() {
    use rshell_m0::rshell_core::{
        KeyBinding, KeyCode, KeyModifiers, TerminalOverrides, TerminalSettingsV1,
    };
    let mut profile = TerminalSettingsV1::default().resolve(&TerminalOverrides::default());
    profile.scroll_on_keypress = false;
    profile.key_bindings.push(KeyBinding {
        code: KeyCode::Character('c'),
        modifiers: KeyModifiers::default(),
        action: "clear_scrollback".into(),
    });
    let root = registration_with_profile(9891, profile);
    root.0.connected();
    let surface = unsafe { root.0.attach_verified(0).expect("原宿主") };
    let mut screen = Screen::new(
        DefaultTerminalEngine::new(&root.0.profile, size()).expect("真实引擎"),
        size(),
    );
    screen
        .advance("滚回\r\n".repeat(20).as_bytes())
        .expect("真实滚回");
    let wire = Arc::new(Wire::default());
    let (close, task) = actor_with_screen(
        root.0.clone(),
        Transport {
            wire: wire.clone(),
            output: None,
            pause: false,
        },
        true,
        Some(screen),
    );
    until(|| root.0.projection().is_some()).await;
    let selected = surface.action(1, DesktopAction::SelectAll).expect("原选区");
    until(|| root.0.applied() >= selected).await;
    let original = root.0.projection().expect("原帧");
    let frame_lease = Arc::downgrade(&original);
    let frozen = geometry(&original);
    surface.presented_geometry(1, frozen).expect("原呈现几何");
    let copy = surface.command(2, DesktopEvent::Copy).expect("原 Copy");
    let claim = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(claim) = surface.claim_action(copy) {
                break claim;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("真实 Copy claim 稳定队头");
    let mut clear = key(1, DesktopKeyPhase::Down);
    clear.raw_chord.as_mut().expect("原 raw").code = KeyCode::Character('c');
    clear.dual.as_mut().expect("原 dual").code = KeyCode::Character('c');
    clear.dual.as_mut().expect("原 dual").legacy_code = KeyCode::Character('c');
    let cleared = surface.key(clear).expect("排队 clear");
    let stale = surface
        .pointer(
            2,
            pointer(frozen, original.frame().rows[1].stable_row, true),
        )
        .expect("clear 尚未执行，原 pointer 有效");
    drop(original);
    let following = surface
        .key(key(2, DesktopKeyPhase::Down))
        .expect("原来源后续键");
    assert_eq!(stale.effect, cleared.effect + 1);
    assert_eq!(following.effect, stale.effect + 1);
    assert_eq!(root.0.row_revision(), frozen.row_revision);
    assert_eq!(root.0.binding(), surface.binding());
    assert_eq!(wire.calls.load(Ordering::Acquire), 0);
    claim.ack(ActionReply::Completed).expect("原消费者完成");
    drop(claim);
    until(|| root.0.applied() >= following.effect || task.is_finished()).await;
    assert!(root.0.row_revision() > frozen.row_revision);
    assert_eq!(
        root.0.binding(),
        surface.binding(),
        "原来源没有撤权或换 attempt"
    );
    assert!(root.0.applied() >= stale.effect, "原 pointer effect 已结算");
    assert!(
        frame_lease.upgrade().is_none(),
        "旧 pointer 的冻结帧租约已释放"
    );
    assert!(
        !task.is_finished(),
        "失效命中只能取消原事件，不能结束原连接：applied={}，following={}，row_revision={}，write_calls={}",
        root.0.applied(),
        following.effect,
        root.0.row_revision(),
        wire.calls.load(Ordering::Acquire)
    );
    assert_eq!(root.0.applied(), following.effect);
    assert_eq!(wire.calls.load(Ordering::Acquire), 1);
    assert_eq!(*wire.bytes.lock().expect("只有后续键写入"), b"a");
    let copy = surface
        .action(3, DesktopAction::Copy)
        .expect("验证归还后的额度");
    let mut claim = None;
    until(|| {
        claim = surface.claim_action(copy).ok();
        claim.is_some()
    })
    .await;
    let claim = claim.expect("原 actor 再次进入 Copy 等待");
    for index in 0..desktop_runtime::INPUT_EVENTS - 1 {
        let phase = if index == 0 {
            DesktopKeyPhase::Down
        } else {
            DesktopKeyPhase::Repeat
        };
        surface
            .key(key(3, phase))
            .expect("旧 effect 不得占用剩余额度");
    }
    assert!(matches!(
        surface.key(key(3, DesktopKeyPhase::Repeat)),
        Err(port::Reject::Busy)
    ));
    close.send_replace(true);
    let screen = task.await.expect("原 actor 正常关闭");
    claim.ack(ActionReply::Cancelled).expect("结算原消费者");
    assert!(screen.selection.is_none(), "旧命中不能重新选择已清理的行");
    assert_eq!(screen.window, super::Window::Bottom);
}

#[tokio::test]
async fn pointer_local_wheel_obeys_actor_keypress_scroll_boundary_with_frozen_geometry() {
    use rshell_m0::rshell_core::{TerminalOverrides, TerminalSettingsV1};
    for (index, scroll_on_output, scroll_on_keypress) in [
        (0, false, false),
        (1, false, true),
        (2, true, false),
        (3, true, true),
    ] {
        let mut profile = TerminalSettingsV1::default().resolve(&TerminalOverrides::default());
        profile.scroll_on_output = scroll_on_output;
        profile.scroll_on_keypress = scroll_on_keypress;
        let root = registration_with_profile(9892 + index, profile);
        root.0.connected();
        let surface = unsafe { root.0.attach_verified(0).expect("原宿主") };
        let mut screen = Screen::new(
            DefaultTerminalEngine::new(&root.0.profile, size()).expect("真实引擎"),
            size(),
        );
        screen
            .advance("行\r\n".repeat(140).as_bytes())
            .expect("真实滚回");
        screen.window = super::Window::At {
            top: 100,
            rows: size().rows,
        };
        let wire = Arc::new(Wire::default());
        let (close, task) = actor_with_screen(
            root.0.clone(),
            Transport {
                wire: wire.clone(),
                output: None,
                pause: false,
            },
            true,
            Some(screen),
        );
        until(|| root.0.projection().is_some()).await;
        let original = root.0.projection().expect("原呈现帧");
        let frozen = geometry(&original);
        assert_eq!(frozen.viewport_top, 100);
        surface.presented_geometry(1, frozen).expect("原几何");
        let mut wheel = pointer(frozen, original.frame().rows[1].stable_row, true);
        wheel.kind = 4;
        wheel.button = 0;
        wheel.buttons = 0;
        wheel.delta_y = -3;
        let pressed = surface
            .key(key(1, DesktopKeyPhase::Down))
            .expect("真实按键");
        let scrolled = surface.pointer(2, wheel).expect("紧随按键的冻结 wheel");
        assert_eq!(scrolled.effect, pressed.effect + 1);
        until(|| root.0.applied() >= scrolled.effect || task.is_finished()).await;
        assert!(!task.is_finished(), "按键与 wheel 应在原连接依序结算");
        assert_eq!(wire.calls.load(Ordering::Acquire), 1);
        assert_eq!(*wire.bytes.lock().expect("只有真实按键写远端"), b"a");
        close.send_replace(true);
        let screen = task.await.expect("原 actor 正常关闭");
        let expected = if scroll_on_keypress {
            original.bounds.bottom_top_stable_row - 3
        } else {
            97
        };
        assert_eq!(
            screen.window,
            super::Window::At {
                top: expected,
                rows: size().rows
            }
        );
        assert!(screen.selection.is_none());
    }
}

#[tokio::test]
async fn pointer_button_tracking_ignores_hover_and_keeps_connection_accepting_input() {
    check_pointer_tracking(b"\x1b[?1002h\x1b[?1006h", 0, None, 9872).await;
}

#[tokio::test]
async fn horizontal_wheel_does_not_revoke_surface_or_move_vertical_viewport() {
    let root = registration(9881);
    root.0.connected();
    let surface = unsafe { root.0.attach_verified(0).expect("原宿主") };
    let wire = Arc::new(Wire::default());
    let (close, task) = actor_with_screen(
        root.0.clone(),
        Transport {
            wire: wire.clone(),
            output: None,
            pause: false,
        },
        true,
        None,
    );
    until(|| root.0.projection().is_some()).await;
    let original = root.0.projection().expect("原呈现帧");
    let frozen = geometry(&original);
    surface.presented_geometry(1, frozen).expect("原几何");
    let mut wheel = pointer(frozen, original.frame().rows[0].stable_row, true);
    wheel.kind = 4;
    wheel.button = 0;
    wheel.buttons = 0;
    wheel.delta_x = 3;
    wheel.delta_y = 0;
    let horizontal = surface.pointer(2, wheel).expect("水平滚轮有效");
    until(|| root.0.applied() >= horizontal.effect || task.is_finished()).await;
    assert!(!task.is_finished());
    let pressed = surface
        .key(key(1, DesktopKeyPhase::Down))
        .expect("继续接受键盘输入");
    until(|| root.0.applied() >= pressed.effect || task.is_finished()).await;
    assert_eq!(*wire.bytes.lock().expect("真实输入"), b"a");
    close.send_replace(true);
    let screen = task.await.expect("正常结束");
    assert_eq!(screen.window, super::Window::Bottom);
}

#[tokio::test]
async fn pointer_button_tracking_reports_drag_and_keeps_connection_accepting_input() {
    check_pointer_tracking(b"\x1b[?1002h\x1b[?1006h", 1, Some(b"\x1b[<32;4;2M"), 9873).await;
}

#[tokio::test]
async fn pointer_local_wheel_accumulates_on_actor_viewport_across_presentation_and_commands() {
    use rshell_m0::rshell_session::TerminalEngine;
    let root = registration(9874);
    root.0.connected();
    let surface = unsafe { root.0.attach_verified(0).expect("原宿主") };
    port::CURRENT
        .scope(root.0.clone(), async {
            let mut size = size();
            let mut screen = Screen::new(
                DefaultTerminalEngine::new(&root.0.profile, size).expect("真实引擎"),
                size,
            );
            screen
                .advance("行\r\n".repeat(140).as_bytes())
                .expect("真实滚回");
            screen.window = super::Window::At {
                top: 100,
                rows: size.rows,
            };
            screen.changed(9874).expect("原帧");
            let original = root.0.projection().expect("原投影");
            let frozen = geometry(&original);
            assert_eq!(frozen.viewport_top, 100);
            assert!(original.bounds.bottom_top_stable_row > 100);
            surface.presented_geometry(1, frozen).expect("原几何");
            let mut wheel = pointer(frozen, original.frame().rows[1].stable_row, true);
            wheel.kind = 4;
            wheel.button = 0;
            wheel.buttons = 0;
            wheel.delta_y = -3;
            let first = surface.pointer(2, wheel).expect("首个 wheel");
            let second = surface
                .pointer(3, wheel)
                .expect("同 geometry 独立 native ID");
            assert_eq!(second.effect, first.effect + 1);
            let wire = Arc::new(Wire::default());
            let mut transport = Transport {
                wire: wire.clone(),
                output: None,
                pause: false,
            };
            for expected in [97, 94] {
                let effect = root.0.next().await;
                let ready = super::begin(Some(&screen), effect).await;
                assert!(
                    super::apply(9874, &mut screen, &mut transport, &mut size, ready)
                        .await
                        .is_ok()
                );
                assert_eq!(
                    screen.window,
                    super::Window::At {
                        top: expected,
                        rows: size.rows
                    }
                );
            }
            let viewport = surface
                .command(
                    1,
                    DesktopEvent::Viewport {
                        follow_bottom: false,
                        top_stable_row: 60,
                        rows: size.rows,
                    },
                )
                .expect("原 actor 显式 viewport");
            let after_viewport = surface.pointer(4, wheel).expect("仍是原冻结命中");
            assert_eq!(after_viewport.effect, viewport + 1);
            for expected in [60, 57] {
                let effect = root.0.next().await;
                let ready = super::begin(Some(&screen), effect).await;
                assert!(
                    super::apply(9874, &mut screen, &mut transport, &mut size, ready)
                        .await
                        .is_ok()
                );
                assert_eq!(
                    screen.window,
                    super::Window::At {
                        top: expected,
                        rows: size.rows
                    }
                );
            }
            screen.present(9874).expect("同 root 行身份的新呈现帧");
            let presented = root.0.projection().expect("新投影");
            let new = geometry(&presented);
            assert_eq!(new.row_revision, frozen.row_revision);
            surface.presented_geometry(5, new).expect("有效新 geometry");
            wheel.geometry = new;
            wheel.stable_hit_row = presented.frame().rows[1].stable_row;
            surface.pointer(6, wheel).expect("新 geometry wheel");
            let effect = root.0.next().await;
            let ready = super::begin(Some(&screen), effect).await;
            assert!(
                super::apply(9874, &mut screen, &mut transport, &mut size, ready)
                    .await
                    .is_ok()
            );
            assert_eq!(
                screen.window,
                super::Window::At {
                    top: 54,
                    rows: size.rows
                }
            );
            assert_eq!(wire.calls.load(Ordering::Acquire), 0);
            let stale = surface.pointer(7, wheel).expect("原行身份的已准入请求");
            TerminalEngine::clear_scrollback(&mut screen.engine).expect("真实行身份失效");
            root.0.invalidate_projection();
            let previous_window = screen.window;
            let effect = root.0.next().await;
            let ready = super::begin(Some(&screen), effect).await;
            assert!(
                super::apply(9874, &mut screen, &mut transport, &mut size, ready)
                    .await
                    .is_ok()
            );
            assert_eq!(root.0.applied(), stale.effect);
            assert_eq!(
                screen.window, previous_window,
                "失效 wheel 不得替换 viewport"
            );
            assert!(screen.selection.is_none(), "失效命中不得重新选择");
            assert_eq!(wire.calls.load(Ordering::Acquire), 0, "失效命中不得送远端");
        })
        .await;
}

#[tokio::test]
async fn pointer_local_wheel_obeys_actor_output_scroll_boundary_with_frozen_geometry() {
    use rshell_m0::rshell_core::{TerminalOverrides, TerminalSettingsV1};
    use rshell_m0::rshell_session::TerminalEngine;
    for (index, scroll_on_output, scroll_on_keypress) in [
        (0, false, false),
        (1, false, true),
        (2, true, false),
        (3, true, true),
    ] {
        let mut profile = TerminalSettingsV1::default().resolve(&TerminalOverrides::default());
        profile.scroll_on_output = scroll_on_output;
        profile.scroll_on_keypress = scroll_on_keypress;
        let root = registration_with_profile(9880 + index, profile);
        root.0.connected();
        let surface = unsafe { root.0.attach_verified(0).expect("原宿主") };
        port::CURRENT
            .scope(root.0.clone(), async {
                let mut size = size();
                let mut screen = Screen::new(
                    DefaultTerminalEngine::new(&root.0.profile, size).expect("真实引擎"),
                    size,
                );
                screen
                    .advance("行\r\n".repeat(140).as_bytes())
                    .expect("真实滚回");
                screen.window = super::Window::At {
                    top: 100,
                    rows: size.rows,
                };
                screen.changed(9880 + index).expect("原帧");
                let original = root.0.projection().expect("原投影");
                let frozen = geometry(&original);
                surface.presented_geometry(1, frozen).expect("原 geometry");
                let mut wheel = pointer(frozen, original.frame().rows[1].stable_row, true);
                wheel.kind = 4;
                wheel.button = 0;
                wheel.buttons = 0;
                wheel.delta_y = -3;
                let wire = Arc::new(Wire::default());
                let mut transport = Transport {
                    wire: wire.clone(),
                    output: None,
                    pause: false,
                };
                surface.pointer(2, wheel).expect("首个 wheel");
                let effect = root.0.next().await;
                let ready = super::begin(Some(&screen), effect).await;
                assert!(
                    super::apply(9880 + index, &mut screen, &mut transport, &mut size, ready)
                        .await
                        .is_ok()
                );
                assert_eq!(
                    screen.window,
                    super::Window::At {
                        top: 97,
                        rows: size.rows
                    }
                );
                assert!(
                    super::output(
                        9880 + index,
                        &mut screen,
                        &mut transport,
                        "新输出\r\n".as_bytes()
                    )
                    .await
                    .is_ok()
                );
                let expected = if scroll_on_output {
                    screen.engine.viewport_bounds().bottom_top_stable_row - 3
                } else {
                    94
                };
                surface
                    .pointer(3, wheel)
                    .expect("输出后仍是原冻结 geometry");
                let effect = root.0.next().await;
                let ready = super::begin(Some(&screen), effect).await;
                assert!(
                    super::apply(9880 + index, &mut screen, &mut transport, &mut size, ready)
                        .await
                        .is_ok()
                );
                assert_eq!(
                    screen.window,
                    super::Window::At {
                        top: expected,
                        rows: size.rows
                    }
                );
                assert_eq!(wire.calls.load(Ordering::Acquire), 0);
            })
            .await;
    }
}

#[tokio::test]
async fn frozen_pointer_applies_original_stable_selection_after_presented_frame_and_transform_change()
 {
    let root = registration(885);
    root.0.connected();
    let surface = unsafe { root.0.attach_verified(0).expect("宿主") };
    port::CURRENT
        .scope(root.0.clone(), async {
            let mut size = size();
            let mut screen = Screen::new(
                DefaultTerminalEngine::new(&root.0.profile, size).expect("原引擎"),
                size,
            );
            screen.advance(b"first\r\nsecond\r\nthird").expect("原输出");
            screen.changed(885).expect("原帧呈现");
            let original = root.0.projection().expect("同一发布帧");
            let frozen_geometry = geometry(&original);
            let hit = original.frame().rows[1].stable_row;
            surface
                .presented_geometry(1, frozen_geometry)
                .expect("实际布局确认");
            let admitted = surface
                .pointer(2, pointer(frozen_geometry, hit, true))
                .expect("同步 pointer");
            let effect = root.0.next().await;
            screen
                .advance(b"\r\nnew\r\nnew\r\nnew\r\nnew\r\nnew")
                .expect("后续输出");
            screen.window = super::Window::At {
                top: 2,
                rows: size.rows,
            };
            size.pixel_width = 1400;
            screen.resize(885, size).expect("后续字体/布局");
            screen.present(885).expect("新的真实帧");
            let mut new_geometry = geometry(&root.0.projection().expect("新帧"));
            new_geometry.font_revision = 2;
            new_geometry.layout_revision = 2;
            new_geometry.transform_revision = 2;
            new_geometry.transform[4] = 200.0;
            surface
                .presented_geometry(3, new_geometry)
                .expect("新实际布局");
            let wire = Arc::new(Wire::default());
            let mut transport = Transport {
                wire: wire.clone(),
                output: None,
                pause: false,
            };
            let ready = super::begin(Some(&screen), effect).await;
            assert!(
                super::apply(885, &mut screen, &mut transport, &mut size, ready)
                    .await
                    .is_ok(),
                "原冻结命中执行"
            );
            let selection = screen.selection.expect("本地选择");
            assert_eq!(selection.start.stable_row, hit);
            assert_eq!(selection.start.column, 3);
            assert_eq!(root.0.applied(), admitted.effect);
            assert_eq!(wire.calls.load(Ordering::Acquire), 0, "Shift 不送远端");
        })
        .await;
}

#[tokio::test]
async fn busy_pointer_freezes_request_blocks_geometry_and_keys_without_consuming_native_id() {
    let root = registration(886);
    root.0.connected();
    let surface = unsafe { root.0.attach_verified(0).expect("宿主") };
    port::CURRENT
        .scope(root.0.clone(), async {
            let mut screen = Screen::new(
                DefaultTerminalEngine::new(&root.0.profile, size()).expect("原引擎"),
                size(),
            );
            screen.changed(886).expect("呈现");
            let projection = root.0.projection().expect("原帧");
            let geometry = geometry(&projection);
            let input = pointer(geometry, projection.frame().rows[1].stable_row, true);
            surface.presented_geometry(1, geometry).expect("原几何");
            surface.key(key(1, DesktopKeyPhase::Down)).expect("key");
            for _ in 1..desktop_runtime::INPUT_EVENTS {
                surface.key(key(1, DesktopKeyPhase::Repeat)).expect("满额");
            }
            let boundary = root.0.output_boundary().0;
            assert_eq!(surface.pointer(2, input), Err(port::Reject::Busy));
            assert_eq!(
                surface.presented_geometry(3, geometry),
                Err(port::Reject::Busy)
            );
            assert_eq!(
                surface.key(key(1, DesktopKeyPhase::Repeat)),
                Err(port::Reject::Busy)
            );
            assert_eq!(root.0.output_boundary().0, boundary);
            drop(root.0.next().await);
            let gui = gui_request(
                &root.0,
                surface.binding(),
                2,
                GuiInput::GuiKey(GuiKey::Enter),
                GuiModifiers {
                    latched: 3,
                    ..GuiModifiers::default()
                },
            );
            let rejected = route_command(gui.clone());
            assert_eq!(rejected.admission, NativeCommandAdmission::Rejected);
            assert_eq!(rejected.failure, NativeCommandFailure::Busy);
            assert_eq!(rejected.effect, 0);
            assert_eq!(rejected.consumed_latches, 0);
            assert_eq!(root.0.output_boundary().0, boundary);
            let pointer = surface.pointer(2, input).expect("原 ID 原快照重投成功");
            assert_eq!(pointer.effect, boundary + 1);
            drop(root.0.next().await);
            let admitted = route_command(gui);
            assert_eq!(admitted.admission, NativeCommandAdmission::Accepted);
            assert_eq!(admitted.effect, pointer.effect + 1);
            assert_eq!(admitted.consumed_latches, 3);
            drop(root.0.next().await);
            let dart = route_command(crate::signals::desktop_native_input::DesktopNativeCommand {
                version: 1,
                source: root.0.source.source.clone(),
                identity: root.0.source.identity.clone(),
                attempt: surface.binding().attempt,
                focus_epoch: surface.binding().epoch,
                request_id: 3,
                command: crate::signals::desktop_native_input::NativeCommand::Event(
                    DesktopEvent::Paste {
                        text: "Dart".into(),
                    },
                ),
            });
            assert_eq!(
                dart.failure,
                crate::signals::desktop_native_input::NativeCommandFailure::None
            );
            assert_eq!(dart.effect, admitted.effect + 1);
            assert_eq!(surface.pointer(2, input), Err(port::Reject::Duplicate));
            surface
                .presented_geometry(3, geometry)
                .expect("Busy 未消费后续 ID");
        })
        .await;
}

#[tokio::test]
async fn frozen_pointer_remote_coordinates_and_mouse_double_gate_with_shift_stay_in_the_original_actor()
 {
    use rshell_m0::rshell_core::{TerminalOverrides, TerminalSettingsV1};
    for (index, profile_mouse, remote_mouse, shift) in [
        (0, true, true, false),
        (1, false, true, false),
        (2, true, false, false),
        (3, true, true, true),
    ] {
        let mut profile = TerminalSettingsV1::default().resolve(&TerminalOverrides::default());
        profile.mouse_reporting = profile_mouse;
        let root = registration_with_profile(890 + index, profile);
        root.0.connected();
        let surface = unsafe { root.0.attach_verified(0).expect("宿主") };
        port::CURRENT
            .scope(root.0.clone(), async {
                let mut size = size();
                let mut screen = Screen::new(
                    DefaultTerminalEngine::new(&root.0.profile, size).expect("原引擎"),
                    size,
                );
                if remote_mouse {
                    screen
                        .advance(b"\x1b[?1000h\x1b[?1006h")
                        .expect("远端 mouse 模式");
                }
                screen.changed(890 + index).expect("原帧");
                let original = root.0.projection().expect("原投影");
                let frozen = geometry(&original);
                let hit = original.frame().rows[1].stable_row;
                surface.presented_geometry(1, frozen).expect("原布局");
                surface
                    .pointer(2, pointer(frozen, hit, shift))
                    .expect("冻结原远端坐标");
                let effect = root.0.next().await;
                screen
                    .advance("新行\r\n".repeat(12).as_bytes())
                    .expect("准入后的帧变化");
                screen.present(890 + index).expect("新帧");
                let mut new = geometry(&root.0.projection().expect("新投影"));
                new.font_revision = 2;
                new.layout_revision = 2;
                new.transform_revision = 2;
                surface.presented_geometry(3, new).expect("新布局");
                let wire = Arc::new(Wire::default());
                let mut transport = Transport {
                    wire: wire.clone(),
                    output: None,
                    pause: false,
                };
                let ready = super::begin(Some(&screen), effect).await;
                assert!(
                    super::apply(890 + index, &mut screen, &mut transport, &mut size, ready)
                        .await
                        .is_ok()
                );
                if profile_mouse && remote_mouse && !shift {
                    assert_eq!(*wire.bytes.lock().expect("冻结 SGR 坐标"), b"\x1b[<0;4;2M");
                    assert!(screen.selection.is_none());
                } else {
                    assert!(wire.bytes.lock().expect("双门控/Shift 本地").is_empty());
                    assert_eq!(
                        screen.selection.expect("原稳定行选择").start.stable_row,
                        hit
                    );
                }
            })
            .await;
    }
}

#[tokio::test]
async fn search_and_close_claims_do_not_wait_for_their_own_ack_to_seal_input_and_settle_quiet() {
    for (index, action) in [(0, DesktopAction::Search), (1, DesktopAction::ClosePane)] {
        let root = registration(894 + index);
        root.0.connected();
        let surface = unsafe { root.0.attach_verified(0).expect("宿主") };
        let wire = Arc::new(Wire::default());
        let (close, task) = actor(
            root.0.clone(),
            Transport {
                wire,
                output: None,
                pause: false,
            },
        );
        let mut input = key(1, DesktopKeyPhase::Down);
        input.text_ownership = policy::TextOwnership::NativeText;
        let text = surface.key_with_context(input, 1, 0).expect("composition");
        let effect = surface.action(1, action).expect("动作立即封原 context");
        let end = root
            .0
            .text_end(surface.binding())
            .expect("非阻塞 query")
            .expect("原 quiet fence");
        assert_eq!(end.transaction_id, text.effect);
        assert!(matches!(
            surface.claim_action(effect),
            Err(port::Reject::InvalidTicket)
        ));
        assert!(root.0.applied() < text.effect);
        surface.text_end_ack(end).expect("原 TSF quiet 结算");
        let mut claimed = None;
        until(|| match surface.claim_action(effect) {
            Ok(claim) => {
                claimed = Some(claim);
                true
            }
            Err(_) => false,
        })
        .await;
        let claim = claimed.expect("原 GUI consumer 领取");
        assert!(root.0.applied() < effect);
        let mut later = key(2, DesktopKeyPhase::Down);
        later.text_ownership = policy::TextOwnership::NativeText;
        let later_text = surface
            .key_with_context(later, 2, 0)
            .expect("claim 后的原 context 文本");
        surface.revoke().expect("claim 未 ACK 也立即 seal");
        let ending = root
            .0
            .text_end(surface.binding())
            .expect("quiet 不依赖 actor 到达后序 text effect")
            .expect("原 context 终结");
        assert_eq!(ending.transaction_id, later_text.effect);
        surface
            .text_end_ack(ending)
            .expect("原 callbacks quiet，先于 GUI ACK");
        assert!(
            root.0
                .text_end(surface.binding())
                .expect("quiet 不等 effect/claim")
                .is_none()
        );
        assert!(root.0.applied() < effect, "nativequiet 不是 GUI 已完成");
        claim
            .ack(ActionReply::Completed)
            .expect("原 GUI 完成后独立结算");
        until(|| root.0.applied() >= effect).await;
        close.send_replace(true);
        task.await.expect("原 actor 退出");
    }
}

// 宿主登记/绑定允许将 Busy 请求原样再投递；按键 Down 不可走此路径。
async fn host_binding_request<T>(
    mut request: impl FnMut() -> Result<T, port::Reject>,
) -> Result<T, port::Reject> {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match request() {
                Err(port::Reject::Busy) => tokio::task::yield_now().await,
                result => return result,
            }
        }
    })
    .await
    .expect("原宿主绑定请求必须完成")
}

async fn native_command_request(
    request: crate::signals::desktop_native_input::DesktopNativeCommand,
) -> crate::signals::desktop_native_input::DesktopNativeCommandResult {
    use crate::signals::desktop_native_input::NativeCommandFailure;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let result = route_command(request.clone());
            if result.failure != NativeCommandFailure::Busy {
                return result;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("原非键请求必须完成准入")
}

#[derive(Default)]
struct Wire {
    bytes: Mutex<Vec<u8>>,
    calls: AtomicUsize,
    entered: Notify,
    resume: Notify,
    finished: Notify,
    remote: Mutex<VecDeque<Vec<u8>>>,
    remote_ready: Notify,
    read_bytes: AtomicUsize,
    eof: AtomicBool,
    polls: AtomicUsize,
    fail_write_at: AtomicUsize,
    end: Mutex<Option<super::super::SessionEnd>>,
}
struct Transport {
    wire: Arc<Wire>,
    output: Option<Vec<u8>>,
    pause: bool,
}
#[async_trait]
impl SessionTransport for Transport {
    fn capabilities(&self) -> TransportCapabilities {
        TransportCapabilities::default()
    }
    async fn connect(
        &mut self,
        _: &TransportRequest,
        _: InteractionBroker,
    ) -> Result<(), TransportError> {
        Ok(())
    }
    async fn next_event(&mut self) -> Result<TransportEvent, TransportError> {
        self.wire.polls.fetch_add(1, Ordering::AcqRel);
        if let Some(output) = self.output.take() {
            self.wire
                .read_bytes
                .fetch_add(output.len(), Ordering::AcqRel);
            return Ok(TransportEvent::Output(output));
        }
        loop {
            let notified = self.wire.remote_ready.notified();
            if let Some(bytes) = self.wire.remote.lock().expect("受控远端队列").pop_front() {
                self.wire
                    .read_bytes
                    .fetch_add(bytes.len(), Ordering::AcqRel);
                return Ok(TransportEvent::Output(bytes));
            }
            if self.wire.eof.load(Ordering::Acquire) {
                return Ok(TransportEvent::Eof);
            }
            notified.await;
        }
    }
    async fn write(&mut self, bytes: &[u8]) -> Result<(), TransportError> {
        let call = self.wire.calls.fetch_add(1, Ordering::AcqRel) + 1;
        self.wire
            .bytes
            .lock()
            .expect("prefix")
            .extend_from_slice(&bytes[..1]);
        self.wire.entered.notify_one();
        if self.pause {
            self.wire.resume.notified().await;
        }
        if self.wire.fail_write_at.load(Ordering::Acquire) == call {
            return Err(TransportError::new(
                rshell_m0::rshell_core::SessionFailure::Network,
            ));
        }
        self.wire
            .bytes
            .lock()
            .expect("suffix")
            .extend_from_slice(&bytes[1..]);
        self.wire.finished.notify_one();
        Ok(())
    }
    async fn resize(&mut self, _: TerminalSize) -> Result<(), TransportError> {
        Ok(())
    }
    async fn shutdown(&mut self) -> Result<(), TransportError> {
        Ok(())
    }
}

fn size() -> TerminalSize {
    TerminalSize {
        cols: 100,
        rows: 8,
        pixel_width: 1000,
        pixel_height: 160,
        dpi: 96,
    }
}

async fn until(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !predicate() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("有界等待实际消费");
}

fn actor(
    port: Arc<InputPort>,
    transport: Transport,
) -> (watch::Sender<bool>, tokio::task::JoinHandle<Screen>) {
    actor_with_exit(port, transport, true)
}

fn actor_with_exit(
    port: Arc<InputPort>,
    transport: Transport,
    expected_quit: bool,
) -> (watch::Sender<bool>, tokio::task::JoinHandle<Screen>) {
    actor_with_screen(port, transport, expected_quit, None)
}

fn actor_with_screen(
    port: Arc<InputPort>,
    transport: Transport,
    expected_quit: bool,
    existing: Option<Screen>,
) -> (watch::Sender<bool>, tokio::task::JoinHandle<Screen>) {
    let (close, closing) = watch::channel(false);
    let task = tokio::spawn(port::CURRENT.scope(
        port.clone(),
        desktop_runtime::SOURCE.scope(
            port.source.clone(),
            crate::transport_lifecycle::CLOSING.scope(closing, async move {
                let mut transport = transport;
                let mut size = size();
                let mut screen = existing.unwrap_or_else(|| {
                    Screen::new(
                        DefaultTerminalEngine::new(&port.profile, size).expect("真实终端"),
                        size,
                    )
                });
                let (_sender, mut commands) = unbounded_channel();
                let mut backlog = VecDeque::new();
                let (end, quit) = super::super::run_connected(
                    port.source.source.session_id,
                    &mut screen,
                    &mut transport,
                    &mut size,
                    &mut commands,
                    &mut backlog,
                )
                .await;
                *transport.wire.end.lock().expect("原连接结果") = Some(end);
                assert_eq!(quit, expected_quit);
                screen
            }),
        ),
    ));
    (close, task)
}

#[tokio::test]
async fn received_output_interleaves_at_its_admitted_effect_boundary() {
    let root = registration(841);
    root.0.connected();
    let surface = unsafe { root.0.attach_verified(0).expect("宿主") };
    let wire = Arc::new(Wire::default());
    let (close, task) = actor(
        root.0.clone(),
        Transport {
            wire: wire.clone(),
            output: None,
            pause: false,
        },
    );
    until(|| root.0.projection().is_some()).await;
    let frame_seq = root.0.projection().expect("首帧").frame_revision;
    let mut input = key(1, DesktopKeyPhase::Down);
    input.text_ownership = policy::TextOwnership::NativeText;
    let head = surface.key(input).expect("IME1");
    surface
        .command(
            1,
            DesktopEvent::Paste {
                text: "early".into(),
            },
        )
        .expect("较早 Paste2");
    surface
        .command(2, DesktopEvent::FrameAck { frame_seq })
        .expect("ACK");
    until(|| {
        surface
            .command(3, DesktopEvent::FrameAck { frame_seq })
            .is_ok()
    })
    .await;
    wire.remote
        .lock()
        .expect("远端")
        .push_back(b"\x1b[?2004h".to_vec());
    wire.remote_ready.notify_one();
    until(|| wire.read_bytes.load(Ordering::Acquire) == 8).await;
    surface
        .command(
            4,
            DesktopEvent::Paste {
                text: "late".into(),
            },
        )
        .expect("接收之后的粘贴");
    assert_eq!(
        surface.composition(head.ticket, Some("I".into()), true),
        Err(port::Reject::InvalidTicket)
    );
    let end = root
        .0
        .text_end(surface.binding())
        .expect("fence 查询")
        .expect("非键中断");
    surface.text_end_ack(end).expect("原 context quiet");
    until(|| wire.calls.load(Ordering::Acquire) == 2).await;
    assert_eq!(
        *wire.bytes.lock().expect("原传输字节"),
        b"early\x1b[200~late\x1b[201~"
    );
    wire.remote
        .lock()
        .expect("普通远端输出")
        .push_back(b"\x1b[?2004l".to_vec());
    wire.remote_ready.notify_one();
    until(|| wire.read_bytes.load(Ordering::Acquire) == 16).await;
    surface
        .command(
            5,
            DesktopEvent::Paste {
                text: "plain".into(),
            },
        )
        .expect("普通接收后的粘贴");
    until(|| wire.calls.load(Ordering::Acquire) == 3).await;
    assert_eq!(
        *wire.bytes.lock().expect("模式次序"),
        b"early\x1b[200~late\x1b[201~plain"
    );
    close.send_replace(true);
    task.await.expect("actor");
}

#[tokio::test]
async fn failed_native_write_preserves_received_tail_and_never_replays_its_reply() {
    use rshell_m0::rshell_core::{TerminalOverrides, TerminalSettingsV1};
    use rshell_m0::rshell_session::TerminalEngine;
    for fail_write_at in [1, 2] {
        let mut profile = TerminalSettingsV1::default().resolve(&TerminalOverrides::default());
        profile.answerback = "forbidden-reply".into();
        let root = registration_with_profile(841 + fail_write_at as u32, profile);
        root.0.connected();
        let surface = unsafe { root.0.attach_verified(0).expect("宿主") };
        let wire = Arc::new(Wire::default());
        wire.fail_write_at.store(fail_write_at, Ordering::Release);
        let (_close, task) = actor_with_exit(
            root.0.clone(),
            Transport {
                wire: wire.clone(),
                output: None,
                pause: false,
            },
            false,
        );
        until(|| root.0.projection().is_some()).await;
        let frame_seq = root.0.projection().expect("首帧").frame_revision;
        let mut input = key(1, DesktopKeyPhase::Down);
        input.text_ownership = policy::TextOwnership::NativeText;
        let head = surface.key(input).expect("IME 等待");
        surface
            .command(1, DesktopEvent::FrameAck { frame_seq })
            .expect("ACK");
        until(|| {
            surface
                .command(2, DesktopEvent::FrameAck { frame_seq })
                .is_ok()
        })
        .await;
        wire.remote
            .lock()
            .expect("已收尾输出")
            .extend([b"\x05tail".to_vec(), b"after".to_vec()]);
        wire.remote_ready.notify_one();
        until(|| wire.read_bytes.load(Ordering::Acquire) == 10).await;
        surface
            .composition(head.ticket, Some("X".into()), true)
            .expect("触发原传输失败");
        let screen = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("原连接退出")
            .expect("actor");
        assert_eq!(
            wire.end
                .lock()
                .expect("结果")
                .as_ref()
                .expect("已结束")
                .failure,
            super::super::FailureKind::ConnectionLost
        );
        let selected = screen
            .engine
            .selected_text(super::SelectionRange {
                start: super::CellPosition {
                    stable_row: 0,
                    column: 0,
                },
                end: super::CellPosition {
                    stable_row: 0,
                    column: 100,
                },
                rectangular: false,
            })
            .expect("原 Screen");
        assert_eq!(selected.trim_end(), "tailafter");
        assert_eq!(wire.calls.load(Ordering::Acquire), fail_write_at);
        assert_eq!(
            *wire.bytes.lock().expect("失败前缀"),
            if fail_write_at == 1 {
                b"X".as_slice()
            } else {
                b"Xf".as_slice()
            }
        );
        root.0.disconnected();
        root.0.begin_attempt();
        root.0.connected();
        let next_wire = Arc::new(Wire::default());
        let (close, task) = actor_with_screen(
            root.0.clone(),
            Transport {
                wire: next_wire.clone(),
                output: None,
                pause: false,
            },
            true,
            Some(screen),
        );
        until(|| next_wire.polls.load(Ordering::Acquire) != 0).await;
        assert_eq!(
            next_wire.calls.load(Ordering::Acquire),
            0,
            "旧应答不进入重连"
        );
        close.send_replace(true);
        task.await.expect("同一 Screen 的重连 actor");
    }
}

#[tokio::test(start_paused = true)]
async fn failed_answerback_publishes_the_single_final_output_offline_without_replay() {
    use rshell_m0::rshell_core::{TerminalOverrides, TerminalSettingsV1};
    let mut profile = TerminalSettingsV1::default().resolve(&TerminalOverrides::default());
    profile.answerback = "forbidden-reply".into();
    let root = registration_with_profile(844, profile);
    root.0.connected();
    let surface = unsafe { root.0.attach_verified(0).expect("宿主") };
    let wire = Arc::new(Wire::default());
    wire.fail_write_at.store(2, Ordering::Release);
    let (_close, task) = actor_with_exit(
        root.0.clone(),
        Transport {
            wire: wire.clone(),
            output: None,
            pause: false,
        },
        false,
    );
    until(|| root.0.projection().is_some()).await;
    let initial = root.0.projection().expect("真实首帧");
    let mut input = key(1, DesktopKeyPhase::Down);
    input.text_ownership = policy::TextOwnership::NativeText;
    let head = surface.key(input).expect("原 IME 票据");
    wire.remote
        .lock()
        .expect("唯一末块")
        .push_back(b"\x05tail".to_vec());
    wire.remote_ready.notify_one();
    until(|| wire.read_bytes.load(Ordering::Acquire) == 5).await;
    surface
        .composition(head.ticket, Some("X".into()), true)
        .expect("完成原输入后消费末块");
    let screen = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect("应答写失败结束原连接")
        .expect("原 actor");
    assert_eq!(
        wire.end
            .lock()
            .expect("原结果")
            .as_ref()
            .expect("已结束")
            .failure,
        super::super::FailureKind::ConnectionLost
    );
    assert_eq!(wire.calls.load(Ordering::Acquire), 2);
    assert_eq!(*wire.bytes.lock().expect("只保留失败前缀"), b"Xf");
    assert_eq!(
        screen.last_sent.as_ref().expect("未 ACK 的原帧").0,
        initial.frame_revision
    );
    assert!(
        initial.frame().rows[0]
            .cells
            .iter()
            .all(|cell| cell.text.trim().is_empty())
    );

    root.0.disconnected();
    // 原帧 ACK 与节拍放行共同驱动离线出帧；没有额外输出或直接 render。
    tokio::time::advance(super::super::FramePacer::INTERVAL).await;
    let (sender, mut commands) = unbounded_channel();
    sender
        .send(super::super::SessionCommand::FrameAck(
            initial.frame_revision,
        ))
        .expect("确认实际发出的首帧");
    sender
        .send(super::super::SessionCommand::Disconnect)
        .expect("确认之后退出离线循环");
    let mut terminal = Some(screen);
    port::CURRENT
        .scope(
            root.0.clone(),
            desktop_runtime::SOURCE.scope(root.0.source.clone(), async {
                assert!(matches!(
                    super::super::wait_offline(
                        844,
                        &mut terminal,
                        &mut size(),
                        &mut commands,
                        &mut VecDeque::new(),
                    )
                    .await,
                    super::super::Next::Quit
                ));
            }),
        )
        .await;
    let screen = terminal.expect("离线保留原 Screen");
    let (seq, frame) = screen.last_sent.as_ref().expect("实际发出的离线帧");
    let visible: String = frame.rows[0]
        .cells
        .iter()
        .map(|cell| cell.text.as_str())
        .collect();
    assert_eq!(
        visible.trim_end(),
        "tail",
        "离线 last_sent 必须显示唯一末块"
    );
    assert_ne!(*seq, initial.frame_revision);
    let projection = root.0.projection().expect("离线发布的真实投影");
    assert_eq!(projection.frame_revision, *seq);
    assert_eq!(projection.source.source, root.0.source.source);
    assert_eq!(projection.source.identity, root.0.source.identity);
    assert!(std::ptr::eq(projection.frame(), frame.as_ref()));
    let projected: String = projection.frame().rows[0]
        .cells
        .iter()
        .map(|cell| cell.text.as_str())
        .collect();
    assert_eq!(projected.trim_end(), "tail");
    assert_eq!(wire.calls.load(Ordering::Acquire), 2);
    assert_eq!(*wire.bytes.lock().expect("离线不重放应答"), b"Xf");

    root.0.begin_attempt();
    root.0.connected();
    let next_wire = Arc::new(Wire::default());
    let (close, task) = actor_with_screen(
        root.0.clone(),
        Transport {
            wire: next_wire.clone(),
            output: None,
            pause: false,
        },
        true,
        Some(screen),
    );
    until(|| next_wire.polls.load(Ordering::Acquire) != 0).await;
    assert_eq!(next_wire.calls.load(Ordering::Acquire), 0);
    assert!(
        next_wire
            .bytes
            .lock()
            .expect("新连接不重放原应答")
            .is_empty()
    );
    close.send_replace(true);
    task.await.expect("原 Screen 的新 attempt");
}

#[tokio::test]
async fn offline_claimed_copy_close_exits_without_retiring_the_live_claim() {
    let root = registration(831);
    let surface = unsafe { root.0.attach_verified(0).expect("离线宿主") };
    let weak = Arc::downgrade(&root.0);
    let (close, closing) = watch::channel(false);
    let port = root.0.clone();
    let mut task = tokio::spawn(port::CURRENT.scope(
        port.clone(),
        crate::transport_lifecycle::CLOSING.scope(closing, async move {
            let mut screen = Some(Screen::new(
                DefaultTerminalEngine::new(&port.profile, size()).expect("原引擎"),
                size(),
            ));
            let (_sender, mut receiver) = unbounded_channel();
            let mut backlog = VecDeque::new();
            assert!(matches!(
                super::super::wait_offline(
                    831,
                    &mut screen,
                    &mut size(),
                    &mut receiver,
                    &mut backlog
                )
                .await,
                super::super::Next::Quit
            ));
        }),
    ));
    let seq = surface.action(1, DesktopAction::Copy).expect("离线 copy");
    let claim = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(claim) = surface.claim_action(seq) {
                break claim;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("copy 已发布");
    surface.revoke().expect("claimed 仍保留");
    close.send_replace(true);
    let exited = tokio::time::timeout(Duration::from_millis(200), &mut task).await;
    if exited.is_err() {
        task.abort();
        let _ = task.await;
    }
    assert!(exited.is_ok(), "离线等待 GUI ACK 不能遮蔽 closing");
    drop(surface);
    drop(root);
    assert!(weak.upgrade().is_some(), "真实 claim 仍持原资源");
    claim
        .ack(ActionReply::Completed)
        .expect("原目标可以迟到结算");
    drop(claim);
    assert!(weak.upgrade().is_none());
}

#[tokio::test]
async fn pending_ime_services_frame_ack_and_eof_without_applying_later_output() {
    let root = registration(832);
    root.0.connected();
    let surface = unsafe { root.0.attach_verified(0).expect("宿主") };
    let wire = Arc::new(Wire::default());
    let (_close, mut task) = actor_with_exit(
        root.0.clone(),
        Transport {
            wire: wire.clone(),
            output: None,
            pause: false,
        },
        false,
    );
    until(|| root.0.projection().is_some()).await;
    let frame_seq = root.0.projection().expect("首帧").frame_revision;
    let mut input = key(1, DesktopKeyPhase::Down);
    input.text_ownership = policy::TextOwnership::NativeText;
    let head = surface.key(input).expect("IME 队头");
    surface
        .command(
            1,
            DesktopEvent::Paste {
                text: "later-input".into(),
            },
        )
        .expect("后序输入");
    surface
        .command(2, DesktopEvent::FrameAck { frame_seq })
        .expect("控制 ACK");
    until(|| {
        surface
            .command(3, DesktopEvent::FrameAck { frame_seq })
            .is_ok()
    })
    .await;
    assert!(root.0.applied() < head.effect, "控制 ACK 不越过待决 cursor");
    wire.remote
        .lock()
        .expect("远端")
        .push_back(b"later-output".to_vec());
    wire.remote_ready.notify_one();
    until(|| wire.read_bytes.load(Ordering::Acquire) == 12).await;
    assert_eq!(wire.calls.load(Ordering::Acquire), 0);
    assert!(
        !root
            .0
            .projection()
            .expect("缓存帧")
            .frame()
            .rows
            .iter()
            .any(|row| row.cells.iter().any(|cell| cell.text.contains('l')))
    );
    wire.eof.store(true, Ordering::Release);
    wire.remote_ready.notify_one();
    let exited = tokio::time::timeout(Duration::from_millis(200), &mut task).await;
    if exited.is_err() {
        task.abort();
        let _ = task.await;
    }
    assert!(exited.is_ok(), "EOF 不等待 IME commit");
    let screen = exited.expect("有界退出").expect("actor");
    let text = rshell_m0::rshell_session::TerminalEngine::selected_text(
        &screen.engine,
        super::SelectionRange {
            start: super::CellPosition {
                stable_row: 0,
                column: 0,
            },
            end: super::CellPosition {
                stable_row: 0,
                column: 100,
            },
            rectangular: false,
        },
    )
    .expect("原引擎最后输出");
    assert_eq!(text.trim_end(), "later-output");
    assert_eq!(
        wire.calls.load(Ordering::Acquire),
        0,
        "EOF 后不发送后序输入"
    );
}

#[tokio::test]
async fn pending_claim_keeps_sync_deadline_live_and_accepts_late_ack_after_eof() {
    let root = registration(833);
    root.0.connected();
    let surface = unsafe { root.0.attach_verified(0).expect("宿主") };
    let wire = Arc::new(Wire::default());
    let output = b"\x1b[?2026hSYNC".to_vec();
    let count = output.len();
    let (_close, mut task) = actor_with_exit(
        root.0.clone(),
        Transport {
            wire: wire.clone(),
            output: Some(output),
            pause: false,
        },
        false,
    );
    until(|| wire.read_bytes.load(Ordering::Acquire) == count).await;
    let seq = surface
        .action(1, DesktopAction::NewTab)
        .expect("原窗口动作");
    let claim = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(claim) = surface.claim_action(seq) {
                break claim;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("实际领取");
    until(|| {
        root.0.projection().is_some_and(|projection| {
            projection
                .frame()
                .rows
                .iter()
                .any(|row| row.cells.iter().any(|cell| cell.text.contains('S')))
        })
    })
    .await;
    assert!(root.0.applied() < seq, "同步输出 deadline 不冒充 GUI 完成");
    wire.eof.store(true, Ordering::Release);
    wire.remote_ready.notify_one();
    let exited = tokio::time::timeout(Duration::from_millis(200), &mut task).await;
    if exited.is_err() {
        task.abort();
        let _ = task.await;
    }
    assert!(exited.is_ok(), "claimed 动作不遮蔽 EOF");
    claim
        .ack(ActionReply::Completed)
        .expect("原窗口消费者迟到 ACK");
}

#[tokio::test]
async fn pending_output_is_backpressured_and_native_disconnect_still_exits() {
    let root = registration(834);
    root.0.connected();
    let surface = unsafe { root.0.attach_verified(0).expect("宿主") };
    let wire = Arc::new(Wire::default());
    let (_close, mut task) = actor(
        root.0.clone(),
        Transport {
            wire: wire.clone(),
            output: None,
            pause: false,
        },
    );
    until(|| root.0.projection().is_some()).await;
    let frame_seq = root.0.projection().expect("首帧").frame_revision;
    let mut input = key(1, DesktopKeyPhase::Down);
    input.text_ownership = policy::TextOwnership::NativeText;
    surface.key(input).expect("待决文本");
    surface
        .command(1, DesktopEvent::FrameAck { frame_seq })
        .expect("ACK");
    until(|| {
        surface
            .command(2, DesktopEvent::FrameAck { frame_seq })
            .is_ok()
    })
    .await;
    let capacity = desktop_runtime::INPUT_BYTES;
    wire.remote.lock().expect("输出").extend([
        vec![b'x'; capacity - 17],
        vec![b'y'; 128],
        b"tail".to_vec(),
    ]);
    wire.remote_ready.notify_one();
    until(|| wire.read_bytes.load(Ordering::Acquire) == capacity + 111).await;
    until(|| {
        surface
            .command(3, DesktopEvent::FrameAck { frame_seq })
            .is_ok()
    })
    .await;
    until(|| {
        surface
            .command(4, DesktopEvent::FrameAck { frame_seq })
            .is_ok()
    })
    .await;
    assert_eq!(wire.read_bytes.load(Ordering::Acquire), capacity + 111);
    assert_eq!(wire.remote.lock().expect("背压").len(), 1);
    surface
        .command(5, DesktopEvent::Disconnect)
        .expect("独立关闭容量");
    let exited = tokio::time::timeout(Duration::from_millis(200), &mut task).await;
    if exited.is_err() {
        task.abort();
        let _ = task.await;
    }
    assert!(exited.is_ok(), "满输出缓存不能阻塞原来源关闭");
}

#[tokio::test]
async fn native_text_repeats_write_distinct_multichar_commits_without_shell_actions() {
    let root = registration(835);
    root.0.connected();
    let surface = unsafe { root.0.attach_verified(0).expect("宿主") };
    let wire = Arc::new(Wire::default());
    let (close, task) = actor(
        root.0.clone(),
        Transport {
            wire: wire.clone(),
            output: None,
            pause: false,
        },
    );
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
    let first = surface.key(input(DesktopKeyPhase::Down)).expect("Down");
    surface
        .composition(first.ticket, Some("开始🙂".into()), true)
        .expect("commit");
    until(|| wire.calls.load(Ordering::Acquire) == 1).await;
    let second = surface.key(input(DesktopKeyPhase::Repeat)).expect("Repeat");
    surface
        .composition(second.ticket, Some("继续é".into()), true)
        .expect("原 Repeat 提交");
    let third = surface
        .key(input(DesktopKeyPhase::Repeat))
        .expect("下一 Repeat");
    assert_ne!(first.ticket, second.ticket);
    assert_ne!(second.ticket, third.ticket);
    assert_eq!(
        surface.composition(first.ticket, Some("迟到".into()), true),
        Err(port::Reject::InvalidTicket)
    );
    surface
        .composition(third.ticket, Some("終".into()), true)
        .expect("可先完成后序提交");
    until(|| wire.calls.load(Ordering::Acquire) == 3).await;
    assert_eq!(
        *wire.bytes.lock().expect("原 transport"),
        "开始🙂继续é終".as_bytes()
    );
    assert!(matches!(
        surface.claim_action(third.effect),
        Err(port::Reject::InvalidTicket)
    ));
    surface.key(input(DesktopKeyPhase::Up)).expect("Up");
    close.send_replace(true);
    task.await.expect("actor");
}

#[tokio::test]
async fn partial_write_keeps_original_future_across_frame_ack_focus_revoke_and_new_attempt() {
    let root = registration(801);
    let port = root.0.clone();
    port.connected();
    let surface = unsafe { port.attach_verified(0).expect("受控宿主") };
    let wire = Arc::new(Wire::default());
    let (close, task) = actor(
        port.clone(),
        Transport {
            wire: wire.clone(),
            output: Some(b"\x1b[?2004h".to_vec()),
            pause: true,
        },
    );
    until(|| {
        port.projection()
            .is_some_and(|projection| projection.modes.bracketed_paste)
    })
    .await;
    surface
        .command(
            1,
            DesktopEvent::Paste {
                text: "完整文本🙂\n下一行".into(),
            },
        )
        .expect("粘贴");
    wire.entered.notified().await;
    let frame = port.projection().expect("实际帧").frame_revision;
    surface
        .command(2, DesktopEvent::FrameAck { frame_seq: frame })
        .expect("ack");
    surface.revoke().expect("撤权不是断连");
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    assert_eq!(wire.calls.load(Ordering::Acquire), 1);
    assert_eq!(*wire.bytes.lock().expect("prefix"), vec![0x1b]);
    wire.resume.notify_one();
    wire.finished.notified().await;
    assert_eq!(
        *wire.bytes.lock().expect("exact"),
        "\x1b[200~完整文本🙂\r下一行\x1b[201~".as_bytes()
    );
    assert_eq!(wire.calls.load(Ordering::Acquire), 1);
    close.send_replace(true);
    task.await.expect("actor");
    port.begin_attempt();
    port.connected();
    let _next = unsafe { port.attach_verified(1).expect("新 attempt") };
    assert_eq!(
        surface.key(key(1, DesktopKeyPhase::Down)),
        Err(port::Reject::StaleAttempt)
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(1), port.next())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn composition_blocks_later_input_and_copy_uses_full_actor_selection_and_cached_frame() {
    let root = registration(802);
    let port = root.0.clone();
    port.connected();
    let surface = unsafe { port.attach_verified(0).expect("受控宿主") };
    let wire = Arc::new(Wire::default());
    let output = ("长选区abcdef".repeat(30) + "\r\n结束").into_bytes();
    let (close, task) = actor(
        port.clone(),
        Transport {
            wire: wire.clone(),
            output: Some(output),
            pause: false,
        },
    );
    until(|| {
        port.projection().is_some_and(|projection| {
            projection
                .frame()
                .rows
                .iter()
                .any(|row| row.cells.iter().any(|cell| cell.text.contains('长')))
        })
    })
    .await;
    let projection = port.projection().expect("projection");
    assert!(
        projection
            .frame()
            .rows
            .iter()
            .map(|row| row.cells.len())
            .sum::<usize>()
            > 36
    );
    assert!(Arc::ptr_eq(
        &projection,
        &port.projection().expect("同 snapshot")
    ));
    let mut input = key(1, DesktopKeyPhase::Down);
    input.text_ownership = policy::TextOwnership::NativeText;
    let native = surface.key(input).expect("文本票据");
    surface
        .command(
            1,
            DesktopEvent::Paste {
                text: "after".into(),
            },
        )
        .expect("后续粘贴");
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    assert_eq!(wire.calls.load(Ordering::Acquire), 0);
    assert_eq!(
        surface.composition(native.ticket, Some("原文本🙂".into()), true),
        Err(port::Reject::InvalidTicket)
    );
    surface
        .text_end_ack(
            port.text_end(surface.binding())
                .expect("fence")
                .expect("原 context"),
        )
        .expect("quiet 后继续");
    until(|| wire.calls.load(Ordering::Acquire) == 1).await;
    assert_eq!(*wire.bytes.lock().expect("有序字节"), "after".as_bytes());
    surface.key(key(1, DesktopKeyPhase::Up)).expect("up");
    let mut select = key(2, DesktopKeyPhase::Down);
    select.default_action = Some(DesktopAction::SelectAll);
    surface.key(select).expect("select all");
    let copy_effect = surface.command(2, DesktopEvent::Copy).expect("copy");
    let claim = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(claim) = surface.claim_action(copy_effect) {
                break claim;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("真实 copy 消费者");
    let text = &claim.value.copy_text.as_ref().expect("copy payload").text;
    assert!(text.len() > 128);
    assert_eq!(
        text.trim_end_matches('\n'),
        "长选区abcdef".repeat(30) + "\n结束"
    );
    assert_eq!(
        claim.source.source,
        DesktopSource {
            session_id: 802,
            generation: 1
        }
    );
    claim.ack(ActionReply::Completed).expect("消费者实际完成");
    close.send_replace(true);
    task.await.expect("actor");
}

#[tokio::test]
async fn select_all_copy_includes_final_column_of_final_screen_row() {
    let root = registration(803);
    root.0.connected();
    let surface = unsafe { root.0.attach_verified(0).expect("受控宿主") };
    let (close, task) = actor(
        root.0.clone(),
        Transport {
            wire: Arc::default(),
            output: Some(vec![b'x'; 800]),
            pause: false,
        },
    );
    until(|| {
        root.0.projection().is_some_and(|projection| {
            projection
                .frame()
                .rows
                .last()
                .is_some_and(|row| row.cells.last().is_some_and(|cell| cell.text == "x"))
        })
    })
    .await;
    surface
        .action(1, DesktopAction::SelectAll)
        .expect("menu select all");
    let copy_effect = surface.command(2, DesktopEvent::Copy).expect("copy");
    let claim = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(claim) = surface.claim_action(copy_effect) {
                break claim;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("copy claim");
    assert_eq!(
        claim
            .value
            .copy_text
            .as_ref()
            .expect("完整选区")
            .text
            .chars()
            .filter(|ch| *ch == 'x')
            .count(),
        800
    );
    claim.ack(ActionReply::Completed).expect("completed");
    close.send_replace(true);
    task.await.expect("actor");
}

#[tokio::test]
async fn real_window_projection_preserves_all_four_output_and_keypress_scroll_policies() {
    use rshell_m0::rshell_core::{TerminalOverrides, TerminalSettingsV1};
    for (index, (output, keypress)) in [(false, false), (false, true), (true, false), (true, true)]
        .into_iter()
        .enumerate()
    {
        let mut profile = TerminalSettingsV1::default().resolve(&TerminalOverrides::default());
        profile.scroll_on_output = output;
        profile.scroll_on_keypress = keypress;
        let root = registration_with_profile(810 + index as u32, profile);
        root.0.connected();
        let surface = unsafe { root.0.attach_verified(0).expect("受控宿主") };
        let wire = Arc::new(Wire::default());
        let (close, task) = actor(
            root.0.clone(),
            Transport {
                wire: wire.clone(),
                output: Some("滚回\r\n".repeat(20).into_bytes()),
                pause: false,
            },
        );
        until(|| {
            root.0.projection().is_some_and(|projection| {
                projection.bounds.first_stable_row < projection.bounds.bottom_top_stable_row
            })
        })
        .await;
        let first = root.0.projection().expect("frame");
        surface
            .command(
                1,
                DesktopEvent::Viewport {
                    follow_bottom: false,
                    top_stable_row: first.bounds.first_stable_row,
                    rows: 8,
                },
            )
            .expect("停在滚回");
        until(|| {
            root.0.projection().is_some_and(|projection| {
                projection.frame_revision > first.frame_revision && !projection.follow_bottom
            })
        })
        .await;
        let before_output = root.0.projection().expect("历史视口").frame_revision;
        wire.remote
            .lock()
            .expect("远端输入")
            .push_back(b"remote\r\n".to_vec());
        wire.remote_ready.notify_one();
        until(|| {
            root.0
                .projection()
                .is_some_and(|projection| projection.frame_revision > before_output)
        })
        .await;
        assert_eq!(
            root.0.projection().expect("真实输出投影").follow_bottom,
            output
        );
        let before_key = root.0.projection().expect("frame").frame_revision;
        // 即使 At 的行恰好等于屏幕底部，也不能把它误投影成 followBottom。
        surface
            .command(
                2,
                DesktopEvent::Viewport {
                    follow_bottom: false,
                    top_stable_row: i64::MAX,
                    rows: 8,
                },
            )
            .expect("显式 At");
        until(|| {
            root.0.projection().is_some_and(|projection| {
                projection.frame_revision > before_key && !projection.follow_bottom
            })
        })
        .await;
        let admitted = surface
            .key(key(1, DesktopKeyPhase::Down))
            .expect("真实键策略");
        until(|| root.0.applied() >= admitted.effect).await;
        if keypress {
            until(|| {
                root.0
                    .projection()
                    .is_some_and(|projection| projection.follow_bottom)
            })
            .await;
        }
        assert_eq!(
            root.0.projection().expect("按键投影").follow_bottom,
            keypress
        );
        close.send_replace(true);
        task.await.expect("actor");
    }
}

#[tokio::test]
async fn clear_scrollback_invalidates_real_selection_and_retained_row_projection() {
    use rshell_m0::rshell_core::{
        KeyBinding, KeyCode, KeyModifiers, TerminalOverrides, TerminalSettingsV1,
    };
    let mut profile = TerminalSettingsV1::default().resolve(&TerminalOverrides::default());
    profile.key_bindings.push(KeyBinding {
        code: KeyCode::Character('a'),
        modifiers: KeyModifiers::default(),
        action: "clear_scrollback".into(),
    });
    let root = registration_with_profile(815, profile);
    root.0.connected();
    let surface = unsafe { root.0.attach_verified(0).expect("受控宿主") };
    let (close, task) = actor(
        root.0.clone(),
        Transport {
            wire: Arc::default(),
            output: Some("滚回\r\n".repeat(20).into_bytes()),
            pause: false,
        },
    );
    until(|| {
        root.0.projection().is_some_and(|projection| {
            projection.bounds.first_stable_row < projection.bounds.bottom_top_stable_row
        })
    })
    .await;
    surface.action(1, DesktopAction::SelectAll).expect("选区");
    until(|| {
        root.0
            .projection()
            .is_some_and(|projection| projection.selection.is_some())
    })
    .await;
    let old = root.0.projection().expect("原投影仍可被原消费者持有");
    let admitted = surface
        .key(key(1, DesktopKeyPhase::Down))
        .expect("clear 策略");
    until(|| root.0.applied() >= admitted.effect).await;
    until(|| {
        root.0
            .projection()
            .is_some_and(|projection| projection.row_revision > old.row_revision)
    })
    .await;
    let current = root.0.projection().expect("clear 后投影");
    assert!(current.selection.is_none());
    assert_eq!(
        current.bounds.first_stable_row,
        current.bounds.bottom_top_stable_row
    );
    assert!(old.selection.is_some());
    assert!(current.follow_bottom);
    close.send_replace(true);
    task.await.expect("actor");
}

#[tokio::test]
async fn native_command_rinf_decoder_uses_original_binding_and_rust_effect_sequence() {
    use crate::signals::desktop_native_input::{
        DesktopNativeCommand, NativeCommand, NativeCommandFailure, VERSION,
    };
    use rinf::DartSignal;
    let root = registration(820);
    let source = root.0.source.clone();
    let request = DesktopNativeCommand {
        version: VERSION,
        source: source.source.clone(),
        identity: source.identity.clone(),
        attempt: 0,
        focus_epoch: 0,
        request_id: 1000,
        command: NativeCommand::Action(DesktopAction::NewTab),
    };
    assert_eq!(
        native_command_request(request).await.failure,
        NativeCommandFailure::NotAttached
    );
    root.0.connected();
    let surface = unsafe { root.0.attach_verified(0).expect("受控宿主") };
    let binding = surface.binding();
    let decoder_guard = NATIVE_COMMAND_DECODER.lock().await;
    let receiver = DesktopNativeCommand::get_dart_signal_receiver();
    for request_id in [1000, 4000] {
        let request = DesktopNativeCommand {
            version: VERSION,
            source: source.source.clone(),
            identity: source.identity.clone(),
            attempt: binding.attempt,
            focus_epoch: binding.epoch,
            request_id,
            command: NativeCommand::Action(DesktopAction::NewTab),
        };
        crate::signals::desktop_native_input::inject_test_request(&request);
    }
    for (effect_seq, request_id) in (1..=2).zip([1000, 4000]) {
        let decoded = receiver.recv().await.expect("真实 Rinf 解码").message;
        assert_eq!(decoded.source, source.source);
        assert_eq!(decoded.identity, source.identity);
        assert_eq!(decoded.request_id, request_id);
        let result = native_command_request(decoded).await;
        assert_eq!(result.failure, NativeCommandFailure::None);
        assert_eq!(result.effect, effect_seq);
        let effect = root.0.next().await;
        assert_eq!(effect.seq, effect_seq);
        assert!(matches!(
            effect.kind,
            port::EffectKind::Action(DesktopAction::NewTab)
        ));
    }
    drop(receiver);
    drop(decoder_guard);
    surface.revoke().expect("revoke");
    let _next = unsafe { root.0.attach_verified(0).expect("新焦点") };
    let stale = DesktopNativeCommand {
        version: VERSION,
        source: source.source,
        identity: source.identity,
        attempt: binding.attempt,
        focus_epoch: binding.epoch,
        request_id: 9000,
        command: NativeCommand::Action(DesktopAction::NewTab),
    };
    assert_eq!(
        native_command_request(stale).await.failure,
        NativeCommandFailure::NotAttached
    );
}

#[tokio::test]
async fn rinf_decoder_keeps_source_and_effect_sequence_for_concurrent_protocol_tests() {
    use rinf::DartSignal;
    let decode = |session_id| async move {
        let root = registration(session_id);
        root.0.connected();
        let surface = unsafe { root.0.attach_verified(0).expect("原宿主") };
        let binding = surface.binding();
        let _decoder_guard = NATIVE_COMMAND_DECODER.lock().await;
        let receiver = DesktopNativeCommand::get_dart_signal_receiver();
        tokio::task::yield_now().await;
        for (effect_seq, request_id) in (1..=2).zip([10, 30]) {
            let request = DesktopNativeCommand {
                version: crate::signals::desktop_native_input::VERSION,
                source: root.0.source.source.clone(),
                identity: root.0.source.identity.clone(),
                attempt: binding.attempt,
                focus_epoch: binding.epoch,
                request_id,
                command: NativeCommand::Action(DesktopAction::NewTab),
            };
            crate::signals::desktop_native_input::inject_test_request(&request);
            let decoded = receiver.recv().await.expect("原来源 decoder").message;
            assert_eq!(decoded.source, request.source);
            assert_eq!(decoded.identity, request.identity);
            assert_eq!(decoded.request_id, request_id);
            let result = native_command_request(decoded).await;
            assert_eq!(result.admission, NativeCommandAdmission::Accepted);
            assert_eq!(result.failure, NativeCommandFailure::None);
            assert_eq!(result.source, request.source);
            assert_eq!(result.identity, request.identity);
            assert_eq!(result.effect, effect_seq);
            let effect = root.0.next().await;
            assert_eq!(effect.seq, effect_seq);
            assert!(matches!(
                effect.kind,
                port::EffectKind::Action(DesktopAction::NewTab)
            ));
        }
    };
    tokio::join!(decode(8431), decode(8432));
}

#[tokio::test]
async fn independent_normal_bind_consumes_registered_profile_on_the_original_actor() {
    use crate::desktop_native_binding::{VERSION, bind, register_verified_host};
    use rshell_m0::rshell_core::{TerminalOverrides, TerminalSettingsV1};
    let mut profile = TerminalSettingsV1::default().resolve(&TerminalOverrides::default());
    profile.answerback = "冻结答复".into();
    profile.mouse_reporting = false;
    let root = registration_with_profile(821, profile);
    let source = root.0.source.clone();
    let initial = root.0.binding();
    let registration_guard = crate::desktop_input_port::tests::hold_source_registration();
    let busy = bind(VERSION, &source.source, &source.identity, initial).err();
    drop(registration_guard);
    assert_eq!(busy, Some(port::Reject::Busy));
    assert_eq!(
        host_binding_request(|| bind(VERSION, &source.source, &source.identity, initial))
            .await
            .err(),
        Some(port::Reject::NotAttached)
    );
    assert!(matches!(
        bind(VERSION + 1, &source.source, &source.identity, initial),
        Err(port::Reject::UnsupportedVersion)
    ));
    assert!(matches!(
        host_binding_request(|| unsafe {
            register_verified_host(&source.source, &source.identity, initial.attempt + 1)
        })
        .await,
        Err(port::Reject::StaleAttempt)
    ));
    // 只验证 Rust 接缝；真实平台 OS 来源证明仍由原生宿主提供。
    let registered = host_binding_request(|| unsafe {
        register_verified_host(&source.source, &source.identity, initial.attempt)
    })
    .await
    .expect("受控宿主登记");
    let context = host_binding_request(|| {
        bind(
            VERSION,
            &source.source,
            &source.identity,
            registered.binding(),
        )
    })
    .await
    .expect("同一 surface 的正常 bind");
    assert!(Arc::ptr_eq(&registered, &context));
    assert!(matches!(
        host_binding_request(|| bind(
            VERSION,
            &source.source,
            "wrong-identity",
            registered.binding()
        ))
        .await,
        Err(port::Reject::StaleSource)
    ));
    assert!(matches!(
        host_binding_request(|| bind(VERSION, &source.source, &source.identity, initial)).await,
        Err(port::Reject::StaleFocus)
    ));
    assert_eq!(
        context.key(key(1, DesktopKeyPhase::Down)),
        Err(port::Reject::NotAuthenticated)
    );
    root.0.connected();
    let wire = Arc::new(Wire::default());
    let (close, task) = actor(
        root.0.clone(),
        Transport {
            wire: wire.clone(),
            output: Some(b"\x1b[?1000h\x05".to_vec()),
            pause: false,
        },
    );
    until(|| wire.calls.load(Ordering::Acquire) == 1).await;
    let admitted = context
        .key(key(1, DesktopKeyPhase::Down))
        .expect("正常 bind 入原 actor");
    until(|| root.0.applied() >= admitted.effect).await;
    assert_eq!(
        *wire.bytes.lock().expect("原 transport 消费"),
        "冻结答复a".as_bytes()
    );
    assert!(
        !root
            .0
            .projection_result()
            .expect("投影预算")
            .expect("真实 frame")
            .mouse_reporting
    );
    let resize = context
        .command(
            10,
            DesktopEvent::Resize {
                geometry: crate::signals::desktop_session::DesktopGeometry {
                    cols: 90,
                    rows: 7,
                    pixel_width: 900,
                    pixel_height: 140,
                    dpi: 96,
                },
            },
        )
        .expect("原来源布局控制");
    context.revoke().expect("仅撤输入权");
    assert!(!task.is_finished(), "surface revoke 不断开原 SSH actor");
    until(|| root.0.applied() >= resize).await;
    until(|| {
        root.0
            .projection()
            .is_some_and(|projection| projection.frame().size.cols == 90)
    })
    .await;
    let frame_seq = root
        .0
        .projection()
        .expect("未聚焦时仍更新布局")
        .frame_revision;
    let ack = context
        .command(11, DesktopEvent::FrameAck { frame_seq })
        .expect("撤权后原 frame 仍可 ACK");
    until(|| root.0.applied() >= ack).await;
    assert_eq!(
        context.key(key(2, DesktopKeyPhase::Down)),
        Err(port::Reject::StaleFocus)
    );
    let replacement = host_binding_request(|| unsafe {
        register_verified_host(&source.source, &source.identity, initial.attempt)
    })
    .await
    .expect("同一来源的新焦点");
    assert!(matches!(
        host_binding_request(|| bind(VERSION, &source.source, &source.identity, context.binding()))
            .await,
        Err(port::Reject::StaleFocus)
    ));
    let next = host_binding_request(|| {
        bind(
            VERSION,
            &source.source,
            &source.identity,
            replacement.binding(),
        )
    })
    .await
    .expect("明确新焦点绑定");
    let admitted = next
        .key(key(1, DesktopKeyPhase::Down))
        .expect("新焦点同物理键");
    until(|| root.0.applied() >= admitted.effect).await;
    assert_eq!(
        *wire.bytes.lock().expect("仍为原 transport"),
        "冻结答复aa".as_bytes()
    );
    close.send_replace(true);
    task.await.expect("原 actor 退出");
    drop(root);
    assert!(matches!(
        host_binding_request(|| bind(VERSION, &source.source, &source.identity, next.binding()))
            .await,
        Err(port::Reject::Retired)
    ));
}

#[tokio::test]
async fn caret_projection_uses_the_rendered_overscan_frame_without_query_render() {
    let root = registration(822);
    root.0.connected();
    let surface = unsafe { root.0.attach_verified(0).expect("受控宿主") };
    let (close, task) = actor(
        root.0.clone(),
        Transport {
            wire: Arc::default(),
            output: Some("滚回\r\n".repeat(20).into_bytes()),
            pause: false,
        },
    );
    until(|| {
        root.0.projection().is_some_and(|projection| {
            projection.bounds.bottom_top_stable_row > projection.bounds.first_stable_row + 4
        })
    })
    .await;
    let original = root.0.projection().expect("当前帧");
    surface
        .command(
            1,
            DesktopEvent::Viewport {
                follow_bottom: false,
                top_stable_row: original.bounds.bottom_top_stable_row - 4,
                rows: 12,
            },
        )
        .expect("含余量的视口");
    until(|| {
        root.0.projection().is_some_and(|projection| {
            projection.frame_revision > original.frame_revision && !projection.follow_bottom
        })
    })
    .await;
    let retained = root.0.projection().expect("实际渲染帧");
    let frame = retained.frame();
    let cursor_row = frame
        .cursor
        .as_ref()
        .expect("原引擎光标")
        .position
        .stable_row
        - frame.viewport_top;
    assert!(cursor_row >= i64::from(frame.size.rows));
    let caret = super::caret_for_frame(frame).expect("光标包含在实际 overscan 帧内");
    assert_eq!(
        caret.y,
        cursor_row as f64 * f64::from(frame.size.pixel_height) / f64::from(frame.size.rows)
    );
    assert!(Arc::ptr_eq(
        &retained,
        &root.0.projection().expect("查询复用原 snapshot")
    ));
    close.send_replace(true);
    task.await.expect("actor");
}

/// 有序通道按键：supervisor 侧门控 + 原 actor 执行，使用真实引擎与受控 transport。
struct OrderedKeys {
    root: crate::desktop_input_port::Registration,
    sessions: std::collections::HashMap<u32, crate::session::SessionHandle>,
    commands: tokio::sync::mpsc::UnboundedReceiver<crate::session::SessionCommand>,
    seq: u64,
    screen: Screen,
    wire: Arc<Wire>,
    transport: Transport,
}

impl OrderedKeys {
    fn new(session_id: u32, profile: rshell_m0::rshell_core::ResolvedTerminalProfile) -> Self {
        let root = registration_with_profile(session_id, profile);
        let port = root.0.clone();
        let mut protocol = desktop_runtime::Protocol::new(
            port.source.source.clone(),
            port.source.identity.clone(),
        );
        protocol.input = Some(port.clone());
        let (commands_tx, commands) = unbounded_channel();
        let (replies, _) = unbounded_channel();
        let (close, _) = watch::channel(false);
        let mut sessions = std::collections::HashMap::new();
        sessions.insert(
            session_id,
            crate::session::SessionHandle {
                generation: 1,
                commands: commands_tx,
                replies,
                close,
                protocol: Some(protocol),
            },
        );
        let wire = Arc::new(Wire::default());
        Self {
            screen: Screen::new(
                DefaultTerminalEngine::new(&port.profile, size()).expect("真实终端"),
                size(),
            ),
            transport: Transport {
                wire: wire.clone(),
                output: None,
                pause: false,
            },
            root,
            sessions,
            commands,
            seq: 0,
            wire,
        }
    }

    fn send(
        &mut self,
        event: DesktopEvent,
    ) -> Result<Option<DesktopAction>, crate::signals::desktop_session::DesktopFailure> {
        self.seq += 1;
        let port = &self.root.0;
        crate::session::route_ordered(
            &mut self.sessions,
            crate::signals::desktop_session::DesktopOrdered {
                version: crate::signals::desktop_session::VERSION,
                source: port.source.source.clone(),
                identity: port.source.identity.clone(),
                seq: self.seq,
                event,
            },
        )
    }

    /// 按 actor 的同一入口执行下一条有序输入（按键、文本/软件按键、粘贴），返回本次写出的
    /// 字节；没有排队命令时为 None。
    async fn apply(&mut self) -> Option<Vec<u8>> {
        let crate::session::SessionCommand::Ordered(command, _lease) =
            self.commands.try_recv().ok()?
        else {
            panic!("有序命令必须持有预算");
        };
        let session_id = self.root.0.source.source.session_id;
        let (screen, transport) = (&mut self.screen, &mut self.transport);
        match *command {
            crate::session::SessionCommand::Key(intent) => {
                super::apply_key(session_id, screen, transport, intent).await
            }
            crate::session::SessionCommand::Input(input) => {
                super::apply_input(session_id, screen, transport, input).await
            }
            crate::session::SessionCommand::Paste(text) => {
                super::apply_paste(session_id, screen, transport, &text).await
            }
            _ => panic!("只执行输入类有序命令"),
        }
        .unwrap_or_else(|end| panic!("输入执行失败：{}", end.detail));
        Some(std::mem::take(
            &mut *self.wire.bytes.lock().expect("写出字节"),
        ))
    }
}

#[allow(clippy::too_many_arguments)]
fn ordered_key(
    key: &str,
    phase: DesktopKeyPhase,
    physical: u64,
    control: bool,
    left_alt: bool,
    right_alt: bool,
    default_action: Option<DesktopAction>,
) -> DesktopEvent {
    DesktopEvent::Key {
        key: key.into(),
        phase,
        physical,
        logical: 0,
        shift: false,
        control,
        alt: left_alt || right_alt,
        left_alt,
        right_alt,
        alt_source: crate::signals::desktop_session::DesktopAltSource::Known,
        super_key: false,
        default_action,
    }
}

fn plain(key: &str, phase: DesktopKeyPhase, physical: u64) -> DesktopEvent {
    ordered_key(key, phase, physical, false, false, false, None)
}

fn bound_profile() -> rshell_m0::rshell_core::ResolvedTerminalProfile {
    use rshell_m0::rshell_core::{
        KeyBinding, KeyCode, KeyModifiers, TerminalOverrides, TerminalSettingsV1,
    };
    let mut profile = TerminalSettingsV1::default().resolve(&TerminalOverrides::default());
    for (n, action) in [(1, "new_tab"), (2, "send:\x1b[3~"), (3, "clear_scrollback")] {
        profile.key_bindings.push(KeyBinding {
            code: KeyCode::F(n),
            modifiers: KeyModifiers::default(),
            action: action.into(),
        });
    }
    profile
}

#[tokio::test]
async fn ordered_key_reports_shell_actions_once_and_writes_send_sequences_in_actor() {
    use crate::signals::desktop_session::DesktopFailure;
    let mut keys = OrderedKeys::new(9101, bound_profile());
    // 绑定动作经结果回报给壳，不进入 actor；Repeat 不重复触发，Up 只释放锁存。
    assert_eq!(
        keys.send(plain("f1", DesktopKeyPhase::Down, 11)),
        Ok(Some(DesktopAction::NewTab))
    );
    assert_eq!(
        keys.send(plain("f1", DesktopKeyPhase::Repeat, 11)),
        Ok(None)
    );
    assert_eq!(keys.send(plain("f1", DesktopKeyPhase::Up, 11)), Ok(None));
    assert!(keys.apply().await.is_none());
    // 未被 profile 覆盖的默认候选同样回报；profile 绑定优先于候选。
    assert_eq!(
        keys.send(ordered_key(
            "character:c",
            DesktopKeyPhase::Down,
            12,
            true,
            false,
            false,
            Some(DesktopAction::Copy)
        )),
        Ok(Some(DesktopAction::Copy))
    );
    assert_eq!(
        keys.send(ordered_key(
            "f1",
            DesktopKeyPhase::Down,
            13,
            false,
            false,
            false,
            Some(DesktopAction::Copy)
        )),
        Ok(Some(DesktopAction::NewTab))
    );
    // send: 序列在原 actor 内写出；Repeat 按策略重复发送。
    assert_eq!(keys.send(plain("f2", DesktopKeyPhase::Down, 14)), Ok(None));
    assert_eq!(keys.apply().await.as_deref(), Some(b"\x1b[3~".as_slice()));
    assert_eq!(
        keys.send(plain("f2", DesktopKeyPhase::Repeat, 14)),
        Ok(None)
    );
    assert_eq!(keys.apply().await.as_deref(), Some(b"\x1b[3~".as_slice()));
    assert_eq!(keys.send(plain("f2", DesktopKeyPhase::Up, 14)), Ok(None));
    assert!(keys.apply().await.is_none());
    // 键名表与旧路径相同；无法识别的键消费 seq 但不外发。
    assert_eq!(
        keys.send(plain("not_a_key", DesktopKeyPhase::Down, 15)),
        Err(DesktopFailure::InvalidEvent)
    );
    assert!(keys.apply().await.is_none());
}

#[tokio::test]
async fn ordered_key_reports_new_local_terminal_candidate_once_without_actor_input() {
    let mut keys = OrderedKeys::new(9105, bound_profile());
    let chord = |phase| DesktopEvent::Key {
        key: "character:l".into(),
        phase,
        physical: 41,
        logical: 0,
        shift: true,
        control: true,
        alt: false,
        left_alt: false,
        right_alt: false,
        alt_source: crate::signals::desktop_session::DesktopAltSource::Known,
        super_key: false,
        default_action: (phase == DesktopKeyPhase::Down).then_some(DesktopAction::NewLocalTerminal),
    };
    // 终端持焦时 Ctrl+Shift+L 经结果回报给壳一次，不写入终端。
    assert_eq!(
        keys.send(chord(DesktopKeyPhase::Down)),
        Ok(Some(DesktopAction::NewLocalTerminal))
    );
    assert_eq!(keys.send(chord(DesktopKeyPhase::Repeat)), Ok(None));
    assert_eq!(keys.send(chord(DesktopKeyPhase::Up)), Ok(None));
    assert!(keys.apply().await.is_none());
}

#[tokio::test]
async fn ordered_key_left_and_right_alt_follow_profile_meta_and_kitty_release_is_encoded() {
    let mut profile = bound_profile();
    profile.left_alt_as_meta = true;
    profile.right_alt_as_meta = false;
    profile.enable_kitty_keyboard = true;
    let mut keys = OrderedKeys::new(9102, profile);
    for (left, right, expected) in [
        (true, false, b"\x1bx".as_slice()),
        (false, true, b"x".as_slice()),
    ] {
        let down = ordered_key(
            "character:x",
            DesktopKeyPhase::Down,
            21,
            false,
            left,
            right,
            None,
        );
        assert_eq!(keys.send(down), Ok(None));
        assert_eq!(keys.apply().await.as_deref(), Some(expected));
        let up = ordered_key(
            "character:x",
            DesktopKeyPhase::Up,
            21,
            false,
            left,
            right,
            None,
        );
        assert_eq!(keys.send(up), Ok(None));
        assert_eq!(
            keys.apply().await.as_deref(),
            Some(b"".as_slice()),
            "未协商 Kitty 时释放不外发"
        );
    }
    // 远端协商 Kitty 事件类型后，Up 产生真实释放序列。
    keys.screen.advance(b"\x1b[>3u").expect("真实 Kitty 协商");
    let ctrl = |phase| ordered_key("character:a", phase, 22, true, false, false, None);
    assert_eq!(keys.send(ctrl(DesktopKeyPhase::Down)), Ok(None));
    assert_eq!(
        keys.apply().await.as_deref(),
        Some(b"\x1b[97;5u".as_slice())
    );
    assert_eq!(keys.send(ctrl(DesktopKeyPhase::Up)), Ok(None));
    assert_eq!(
        keys.apply().await.as_deref(),
        Some(b"\x1b[97;5:3u".as_slice())
    );
}

#[tokio::test]
async fn ordered_key_lost_up_does_not_lock_the_key() {
    let mut keys = OrderedKeys::new(9103, bound_profile());
    // 焦点切走丢了 Up：同物理键再按仍按新 Down 处理，绑定动作也能再次触发。
    for _ in 0..2 {
        assert_eq!(
            keys.send(plain("character:a", DesktopKeyPhase::Down, 31)),
            Ok(None)
        );
        assert_eq!(keys.apply().await.as_deref(), Some(b"a".as_slice()));
    }
    for _ in 0..2 {
        assert_eq!(
            keys.send(plain("f1", DesktopKeyPhase::Down, 32)),
            Ok(Some(DesktopAction::NewTab))
        );
    }
    assert_eq!(
        keys.send(plain("f1", DesktopKeyPhase::Repeat, 32)),
        Ok(None)
    );
    assert_eq!(keys.send(plain("f1", DesktopKeyPhase::Up, 32)), Ok(None));
    assert_eq!(
        keys.send(plain("character:a", DesktopKeyPhase::Up, 31)),
        Ok(None)
    );
    assert_eq!(keys.apply().await.as_deref(), Some(b"".as_slice()));
    assert!(keys.apply().await.is_none());
}

#[tokio::test]
async fn ordered_key_clear_scrollback_runs_in_actor_and_resets_selection_and_window() {
    use rshell_m0::rshell_session::TerminalEngine as _;
    let mut keys = OrderedKeys::new(9104, bound_profile());
    keys.screen
        .advance("滚回\r\n".repeat(20).as_bytes())
        .expect("真实输出");
    let bounds = keys.screen.engine.viewport_bounds();
    assert!(bounds.first_stable_row < bounds.bottom_top_stable_row);
    keys.screen.selection = Some(super::SelectionRange {
        start: super::CellPosition {
            stable_row: bounds.first_stable_row,
            column: 0,
        },
        end: super::CellPosition {
            stable_row: bounds.first_stable_row,
            column: 2,
        },
        rectangular: false,
    });
    keys.screen.window = super::Window::At {
        top: bounds.first_stable_row,
        rows: 8,
    };
    assert_eq!(keys.send(plain("f3", DesktopKeyPhase::Down, 41)), Ok(None));
    assert_eq!(
        keys.apply().await.as_deref(),
        Some(b"".as_slice()),
        "清屏不写远端"
    );
    let bounds = keys.screen.engine.viewport_bounds();
    assert_eq!(bounds.first_stable_row, bounds.bottom_top_stable_row);
    assert!(keys.screen.selection.is_none());
    assert_eq!(keys.screen.window, super::Window::Bottom);
    assert_eq!(keys.send(plain("f3", DesktopKeyPhase::Up, 41)), Ok(None));
    assert!(keys.apply().await.is_none());
}

fn soft(key: &str, control: bool, alt: bool) -> DesktopEvent {
    DesktopEvent::SoftKey {
        key: key.into(),
        shift: false,
        control,
        alt,
    }
}

#[tokio::test]
async fn ordered_soft_keys_encode_without_physical_identity_bindings_or_shell_actions() {
    use crate::signals::desktop_session::DesktopFailure;
    let mut profile = bound_profile();
    // 软件 Alt 是显式 Meta 请求，不受左右 Alt 的 Meta 设置约束。
    profile.left_alt_as_meta = false;
    profile.right_alt_as_meta = false;
    let mut keys = OrderedKeys::new(9106, profile);
    // 键位条 Esc / Tab / ↑ / Ctrl+C 与挂住 Alt 的字母。
    for (event, expected) in [
        (soft("escape", false, false), b"\x1b".as_slice()),
        (soft("tab", false, false), b"\t".as_slice()),
        (soft("arrow_up", false, false), b"\x1b[A".as_slice()),
        (soft("character:c", true, false), b"\x03".as_slice()),
        (soft("character:x", false, true), b"\x1bx".as_slice()),
        // 物理 F1 绑定了 new_tab；软件 F1 不匹配绑定，也不回报壳动作，直接编码。
        (soft("f1", false, false), b"\x1bOP".as_slice()),
    ] {
        assert_eq!(keys.send(event), Ok(None));
        assert_eq!(keys.apply().await.as_deref(), Some(expected));
    }
    assert_eq!(
        keys.send(soft("not_a_key", false, false)),
        Err(DesktopFailure::InvalidEvent)
    );
    assert!(keys.apply().await.is_none());
    // 软件按键不进入物理锁存。
    let protocol = keys.sessions[&9106].protocol.as_ref().expect("有序协议");
    assert!(protocol.keys == policy::InputPolicyState::default());
}

#[tokio::test]
async fn ordered_invalid_resize_consumes_seq_and_next_event_still_runs() {
    use crate::signals::desktop_session::{DesktopFailure, DesktopGeometry};
    let mut keys = OrderedKeys::new(9107, bound_profile());
    let invalid = DesktopEvent::Resize {
        geometry: DesktopGeometry {
            cols: 0,
            rows: 24,
            pixel_width: 640,
            pixel_height: 480,
            dpi: 96,
        },
    };
    // 准入后拒绝按已消费语义回报，下一条事件用下一个 seq 仍被处理。
    assert_eq!(keys.send(invalid), Err(DesktopFailure::InvalidEvent));
    assert!(keys.apply().await.is_none());
    assert_eq!(
        keys.send(DesktopEvent::Text { text: "ok".into() }),
        Ok(None)
    );
    assert_eq!(keys.apply().await.as_deref(), Some(b"ok".as_slice()));
    assert_eq!(keys.send(soft("enter", false, false)), Ok(None));
    assert_eq!(keys.apply().await.as_deref(), Some(b"\r".as_slice()));
}

#[tokio::test]
async fn ordered_text_and_paste_return_to_bottom_and_publish_frame_without_remote_output() {
    use rshell_m0::rshell_session::TerminalEngine as _;
    for (index, keypress, event) in [
        (0, true, DesktopEvent::Text { text: "字".into() }),
        (
            1,
            true,
            DesktopEvent::Paste {
                text: "粘贴".into(),
            },
        ),
        (2, true, soft("tab", false, false)),
        (3, false, DesktopEvent::Text { text: "字".into() }),
        (
            4,
            false,
            DesktopEvent::Paste {
                text: "粘贴".into(),
            },
        ),
    ] {
        let mut profile = bound_profile();
        profile.scroll_on_keypress = keypress;
        let session_id = 9110 + index;
        let mut keys = OrderedKeys::new(session_id, profile);
        keys.screen
            .advance("滚回\r\n".repeat(40).as_bytes())
            .expect("真实滚回");
        let bounds = keys.screen.engine.viewport_bounds();
        assert!(bounds.first_stable_row < bounds.bottom_top_stable_row);
        let held = super::Window::At {
            top: bounds.first_stable_row,
            rows: size().rows,
        };
        keys.screen.window = held;
        assert!(keys.screen.last_sent.is_none());
        assert_eq!(keys.send(event), Ok(None));
        let port = keys.root.0.clone();
        let written = port::CURRENT.scope(port.clone(), keys.apply()).await;
        assert!(
            written.is_some_and(|bytes| !bytes.is_empty()),
            "输入照常写远端"
        );
        if keypress {
            // 远端无输出也要发布新帧，视口回到底部。
            assert_eq!(keys.screen.window, super::Window::Bottom);
            assert!(keys.screen.last_sent.is_some(), "回底必须出帧");
            assert!(port.projection().expect("新投影").follow_bottom);
        } else {
            assert_eq!(keys.screen.window, held);
            assert!(keys.screen.last_sent.is_none(), "视口未变不出帧");
        }
    }
}
