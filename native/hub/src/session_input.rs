//! InputPort 的唯一执行者是既有 session actor；每个写 future 从首次 poll 保留至完成。

use super::{
    Arc, CellPosition, DesktopEvent, FailureKind, MouseRequest, RustSignal, Screen, SelectionRange,
    SelectionRequest, SessionCommand, SessionEnd, SessionTransport, TerminalEngine, TerminalInput,
    TerminalSize, Window, desktop_prepare, lost, mouse_event_from_request, paste_bytes,
    send_selection_state,
};
use crate::desktop_input_policy::{self as policy, KeyResult};
use crate::desktop_input_port::transactions::ActionCompletion;
use crate::desktop_input_port::{self as port, Effect, EffectKind, InputPort};
use crate::signals::desktop_session::DesktopAction;

pub(super) enum Resolution {
    Immediate,
    Text(Result<port::transactions::BudgetedText, port::Reject>),
    Action(ActionCompletion),
}
pub(super) struct Ready {
    pub effect: Effect,
    pub resolution: Resolution,
    pub settlement: Settlement,
}
pub(super) struct Settlement {
    port: Arc<InputPort>,
    seq: u64,
}
impl Drop for Settlement {
    fn drop(&mut self) {
        self.port.completed(self.seq);
    }
}
pub(super) type Pending = std::pin::Pin<Box<dyn std::future::Future<Output = Ready> + Send>>;

/// 等待只持票据/租约，不借用 Screen 或 transport；actor 在原 cursor 上等待它。
pub(super) fn begin(screen: Option<&Screen>, effect: Effect) -> Pending {
    let port = port::CURRENT.with(Arc::clone);
    let action = match &effect.kind {
        EffectKind::Action(action) => Some(*action),
        EffectKind::Key(policy::GatedIntent::Quiet(outcome)) => match outcome.result {
            KeyResult::Action(action) => Some(action),
            _ => None,
        },
        _ => None,
    };
    let eligible = port.applicable_effect(effect.ticket, &effect.kind);
    let value = if eligible {
        action
            .filter(|action| *action != DesktopAction::SelectAll)
            .and_then(|action| screen.map(|screen| action_value(&port, screen, action)))
    } else {
        None
    };
    let settlement = Settlement {
        port: port.clone(),
        seq: effect.seq,
    };
    Box::pin(async move {
        let resolution = if let Some(value) = value {
            Resolution::Action(
                port.action(effect.seq, effect.ticket, value, &effect.lease)
                    .await,
            )
        } else if eligible && matches!(effect.kind, EffectKind::Text) {
            Resolution::Text(port.text(effect.ticket).await)
        } else {
            Resolution::Immediate
        };
        Ready {
            effect,
            resolution,
            settlement,
        }
    })
}

pub(super) async fn pending_ready(pending: &mut Option<Pending>) -> Ready {
    match pending {
        Some(pending) => pending.await,
        None => std::future::pending().await,
    }
}
pub(super) async fn next_control() -> Effect {
    match port::CURRENT.try_with(Arc::clone) {
        Ok(port) => port.next_control().await,
        Err(_) => std::future::pending().await,
    }
}
pub(super) async fn disconnect_requested() {
    match port::CURRENT.try_with(Arc::clone) {
        Ok(port) => port.disconnect_requested().await,
        Err(_) => std::future::pending().await,
    }
}
pub(super) fn control(
    session_id: u32,
    screen: Option<&mut Screen>,
    effect: Effect,
) -> Result<(), SessionEnd> {
    let port = port::CURRENT.with(Arc::clone);
    if port.applicable_effect(effect.ticket, &effect.kind)
        && let EffectKind::Command(event) = effect.kind
        && let Some(screen) = screen
    {
        local_event(session_id, screen, event)?;
    }
    port.control_completed(effect.seq);
    Ok(())
}

/// 只保留已从 transport 取得的原始输出，待决输入前不 advance；达到容量后停止读取。
#[derive(Default)]
pub(super) struct DeferredOutput {
    queue: std::collections::VecDeque<ReceivedOutput>,
    bytes: usize,
    budget: Option<port::PortBudget>,
}
struct ReceivedOutput {
    after_effect: u64,
    bytes: Box<[u8]>,
}
impl DeferredOutput {
    pub(super) fn awaiting_budget(&self) -> bool {
        self.budget.is_none()
    }
    pub(super) fn can_read(&mut self) -> bool {
        if self.budget.is_none() {
            // 一个接收事件的余量：不拆开已接收块，也不因末块跨越暂停阈值而丢数据。
            self.budget = port::reserve_output(
                2 * crate::desktop_runtime::INPUT_BYTES
                    + crate::desktop_runtime::INPUT_EVENTS * std::mem::size_of::<ReceivedOutput>(),
            );
        }
        self.budget.is_some()
            && self.bytes < crate::desktop_runtime::INPUT_BYTES
            && self.queue.len() < crate::desktop_runtime::INPUT_EVENTS
    }
    pub(super) fn push(
        &mut self,
        after_effect: u64,
        bytes: Vec<u8>,
    ) -> Result<(), (SessionEnd, Vec<u8>)> {
        if bytes.len() > crate::desktop_runtime::INPUT_BYTES {
            return Err((
                SessionEnd::failed(FailureKind::Other, "pending output budget exceeded"),
                bytes,
            ));
        }
        if self.budget.is_none() && !self.can_read() {
            return Err((
                SessionEnd::failed(FailureKind::Other, "pending output budget unavailable"),
                bytes,
            ));
        }
        self.bytes += bytes.len();
        self.queue.push_back(ReceivedOutput {
            after_effect,
            bytes: bytes.into_boxed_slice(),
        });
        Ok(())
    }
    pub(super) fn boundary(&self) -> Option<u64> {
        self.queue.front().map(|output| output.after_effect)
    }
    pub(super) fn pop(&mut self) -> Option<Box<[u8]>> {
        let bytes = self.queue.pop_front()?.bytes;
        self.bytes -= bytes.len();
        Some(bytes)
    }
    pub(super) fn release_empty(&mut self) {
        if self.queue.is_empty() {
            self.queue = std::collections::VecDeque::new();
            self.budget = None;
        }
    }
}

pub(super) fn end_deferred(
    session_id: u32,
    screen: &mut Screen,
    pending: &mut Option<Pending>,
    deferred: &mut DeferredOutput,
    rejected: Option<&[u8]>,
) -> Result<(), SessionEnd> {
    // 原连接结束先取消待决 cursor；已收到的输出随后入原引擎，应答不再写往失效连接。
    *pending = None;
    let mut consume = |bytes: &[u8]| {
        let delta = screen.advance(bytes).map_err(failure)?;
        let scrolled = scroll_output(screen);
        if delta.dirty || scrolled {
            screen.changed(session_id).map_err(failure)?;
        }
        Ok(())
    };
    let mut error = None;
    while let Some(bytes) = deferred.pop() {
        if let Err(failure) = consume(&bytes) {
            error.get_or_insert(failure);
        }
    }
    if let Some(bytes) = rejected
        && let Err(failure) = consume(bytes)
    {
        error.get_or_insert(failure);
    }
    error.map_or(Ok(()), Err)
}

pub(super) fn output_boundary() -> (u64, bool) {
    port::CURRENT
        .try_with(|port| port.output_boundary())
        .unwrap_or((0, false))
}

pub(super) fn take_through(boundary: u64) -> Option<Effect> {
    port::CURRENT
        .try_with(|port| port.take_through(boundary))
        .ok()
        .flatten()
}

pub(super) fn route_command(
    request: crate::signals::desktop_native_input::DesktopNativeCommand,
) -> crate::signals::desktop_native_input::DesktopNativeCommandResult {
    use crate::signals::desktop_native_input::{
        DesktopNativeCommandResult, NativeCommand, NativeCommandAdmission,
        NativeCommandFailure as F, VERSION,
    };
    let gui_modifier_revision = match &request.command {
        NativeCommand::GuiIntent(intent) => Some(intent.modifier_revision),
        _ => None,
    };
    let result = if request.version != VERSION {
        Err(F::UnsupportedVersion)
    } else {
        port::lookup(&request.source, &request.identity)
            .and_then(|port| {
                port.command(
                    port::Binding {
                        attempt: request.attempt,
                        epoch: request.focus_epoch,
                    },
                    request.request_id,
                    request.command,
                )
            })
            .map_err(|error| match error {
                port::Reject::NotAttached | port::Reject::StaleFocus => F::NotAttached,
                port::Reject::StaleSource | port::Reject::StaleAttempt => F::StaleSource,
                port::Reject::NotAuthenticated => F::NotAuthenticated,
                port::Reject::Retired => F::Retired,
                port::Reject::Busy => F::Busy,
                port::Reject::TooLarge => F::TooLarge,
                port::Reject::MixedMode => F::MixedMode,
                port::Reject::Duplicate => F::Duplicate,
                _ => F::InvalidEvent,
            })
    };
    let (effect, failure, admission, consumed_latches) = match result {
        Ok(result) => (
            result.effect,
            F::None,
            NativeCommandAdmission::Accepted,
            result.consumed_latches,
        ),
        Err(F::Duplicate) => (0, F::Duplicate, NativeCommandAdmission::Unknown, 0),
        Err(error) => (0, error, NativeCommandAdmission::Rejected, 0),
    };
    DesktopNativeCommandResult {
        version: VERSION,
        source: request.source,
        identity: request.identity,
        attempt: request.attempt,
        focus_epoch: request.focus_epoch,
        request_id: request.request_id,
        admission,
        effect,
        consumed_latches,
        gui_modifier_revision,
        failure,
    }
}

pub(super) async fn next_effect() -> Effect {
    match port::CURRENT.try_with(Arc::clone) {
        Ok(port) => port.next().await,
        Err(_) => std::future::pending().await,
    }
}

pub(super) async fn output(
    session_id: u32,
    screen: &mut Screen,
    transport: &mut dyn SessionTransport,
    bytes: &[u8],
) -> Result<(), SessionEnd> {
    let delta = screen.advance(bytes).map_err(failure)?;
    let scrolled = scroll_output(screen);
    let written = write(transport, &delta.outbound).await;
    // 已推进的可见变化必须出帧；出帧失败不能覆盖原传输错误。
    let changed = if delta.dirty || scrolled {
        screen.changed(session_id).map_err(failure)
    } else {
        Ok(())
    };
    written.and(changed)
}

pub(super) fn scroll_keypress(screen: &mut Screen) {
    if port::CURRENT
        .try_with(|port| port.profile.scroll_on_keypress)
        .unwrap_or(true)
    {
        screen.window = Window::Bottom;
    }
}

pub(super) fn scroll_output(screen: &mut Screen) -> bool {
    let previous = screen.window;
    if port::CURRENT
        .try_with(|port| port.profile.scroll_on_output)
        .unwrap_or(false)
    {
        screen.window = Window::Bottom;
    }
    screen.window != previous
}

pub(super) fn mouse_allowed() -> bool {
    port::CURRENT
        .try_with(|port| port.profile.mouse_reporting)
        .unwrap_or(true)
}

fn caret_for_frame(
    frame: &super::RenderFrame,
) -> Option<crate::signals::desktop_native_input::NativeCaret> {
    let cursor = frame.cursor.as_ref()?;
    if !frame
        .rows
        .iter()
        .any(|rendered| rendered.stable_row == cursor.position.stable_row)
    {
        return None;
    }
    let width = f64::from(frame.size.pixel_width) / f64::from(frame.size.cols);
    let height = f64::from(frame.size.pixel_height) / f64::from(frame.size.rows);
    Some(crate::signals::desktop_native_input::NativeCaret {
        x: f64::from(cursor.position.column) * width,
        y: (cursor.position.stable_row - frame.viewport_top) as f64 * height,
        width,
        height,
        dpi: frame.size.dpi,
    })
}

pub(super) fn publish_projection(screen: &Screen) {
    use crate::signals::desktop_native_input::{DesktopNativeProjection, NativeSelection};
    if let Some((seq, frame)) = &screen.last_sent {
        let _ = port::CURRENT.try_with(|port| {
            let bounds = screen.engine.viewport_bounds();
            let modes = screen.engine.display_modes();
            let applied_effect = port.applied();
            let row_revision = port.row_revision();
            let binding = port.binding();
            let caret = caret_for_frame(frame);
            DesktopNativeProjection {
                source: port.source.source.clone(),
                identity: port.source.identity.clone(),
                frame_revision: *seq,
                applied_effect,
                row_revision,
                attempt: binding.attempt,
                focus_epoch: binding.epoch,
                follow_bottom: screen.window == Window::Bottom,
                first_stable_row: bounds.first_stable_row,
                screen_top_stable_row: bounds.bottom_top_stable_row,
                viewport_top: frame.viewport_top,
                mouse_reporting: port.profile.mouse_reporting && modes.mouse_reporting,
                bracketed_paste: modes.bracketed_paste,
                selection: screen.selection.map(|selection| NativeSelection {
                    start_row: selection.start.stable_row,
                    start_col: selection.start.column,
                    end_row: selection.end.stable_row,
                    end_col: selection.end.column,
                    rectangular: selection.rectangular,
                }),
                caret,
            }
            .send_signal_to_dart();
            let _ = port.publish(
                *seq,
                frame.clone(),
                bounds,
                screen.window == Window::Bottom,
                screen.selection,
                modes,
            );
        });
    }
}

fn failure(error: impl std::fmt::Debug) -> SessionEnd {
    SessionEnd::failed(FailureKind::Other, format!("native input: {error:?}"))
}

async fn write(transport: &mut dyn SessionTransport, bytes: &[u8]) -> Result<(), SessionEnd> {
    if !bytes.is_empty() {
        transport.write(bytes).await.map_err(|error| {
            SessionEnd::failed(lost(error.failure()), format!("input write: {error:?}"))
        })?;
    }
    Ok(())
}

pub(super) async fn apply(
    session_id: u32,
    screen: &mut Screen,
    transport: &mut dyn SessionTransport,
    size: &mut TerminalSize,
    ready: Ready,
) -> Result<bool, SessionEnd> {
    let port = port::CURRENT.with(Arc::clone);
    let Ready {
        effect,
        resolution,
        settlement: _settlement,
    } = ready;
    let Effect {
        seq,
        ticket,
        kind,
        lease: _lease,
    } = effect;
    if !port.applicable_effect(ticket, &kind) {
        port.completed(seq);
        return Ok(false);
    }
    let previous_window = screen.window;
    let quit = match kind {
        EffectKind::Key(intent) => {
            let outcome = policy::apply_intent(&mut screen.engine, intent).map_err(failure)?;
            if outcome.scroll_to_bottom {
                screen.window = Window::Bottom;
            }
            match outcome.result {
                KeyResult::Bytes(bytes) => write(transport, &bytes).await?,
                KeyResult::TypedInputReject(_) => port.reject_ticket(ticket),
                KeyResult::Action(action) => {
                    finish_action(
                        &port,
                        session_id,
                        screen,
                        Some(transport),
                        ticket,
                        action,
                        resolution,
                    )
                    .await?
                }
                KeyResult::ClearScrollback => {
                    clear_local(screen)?;
                    let row_revision = port.invalidate_projection();
                    crate::signals::desktop_native_input::DesktopNativeInvalidation {
                        source: port.source.source.clone(),
                        identity: port.source.identity.clone(),
                        effect: seq,
                        row_revision,
                    }
                    .send_signal_to_dart();
                    send_selection_state(session_id, None);
                    screen.changed(session_id).map_err(failure)?;
                }
                KeyResult::NoSend(_) => {}
            }
            false
        }
        EffectKind::Text => {
            if let Resolution::Text(Ok(text)) = resolution {
                // 已提交投递属于原 attempt，不随焦点撤权丢弃。
                if port.applicable_effect(ticket, &EffectKind::Text) {
                    scroll_keypress(screen);
                    let bytes = screen
                        .engine
                        .encode_input(TerminalInput::CommittedText(text.text.clone()))
                        .map_err(failure)?;
                    write(transport, &bytes).await?;
                }
            }
            false
        }
        EffectKind::Action(action) => {
            finish_action(
                &port,
                session_id,
                screen,
                Some(transport),
                ticket,
                action,
                resolution,
            )
            .await?;
            false
        }
        EffectKind::Pointer(pointer) => {
            apply_pointer(session_id, screen, transport, &port, seq, pointer).await?;
            false
        }
        EffectKind::GuiIntent(input) => {
            scroll_keypress(screen);
            let bytes = screen.engine.encode_input(input).map_err(failure)?;
            if !bytes.is_empty() {
                write(transport, &bytes).await?;
            }
            false
        }
        EffectKind::Command(event) => match event {
            DesktopEvent::Resize { geometry } => {
                if let Ok(new_size) = desktop_prepare::geometry(geometry) {
                    *size = new_size;
                    screen.resize(session_id, new_size).map_err(failure)?;
                    transport.resize(new_size).await.map_err(failure)?;
                }
                false
            }
            DesktopEvent::Paste { text } => {
                scroll_keypress(screen);
                write(
                    transport,
                    &paste_bytes(&text, screen.engine.display_modes().bracketed_paste),
                )
                .await?;
                false
            }
            DesktopEvent::Disconnect => true,
            event => {
                local_event(session_id, screen, event)?;
                false
            }
        },
    };
    port.completed(seq);
    if screen.window != previous_window {
        screen.changed(session_id).map_err(failure)?;
    }
    Ok(quit)
}

/// 引擎清理成功后才清选区、回到底部并作废已发帧；信号与出帧由调用方负责。
fn clear_local(screen: &mut Screen) -> Result<(), SessionEnd> {
    policy::clear_scrollback(&mut screen.engine).map_err(failure)?;
    screen.selection = None;
    screen.window = Window::Bottom;
    screen.last_sent = None;
    Ok(())
}

/// 有序通道已门控的按键意图：与原生路径同一编码，写入与清屏在原 actor 内执行。
/// 壳动作已由 supervisor 回报，不会到达这里。
pub(super) async fn apply_key(
    session_id: u32,
    screen: &mut Screen,
    transport: &mut dyn SessionTransport,
    intent: policy::GatedIntent,
) -> Result<(), SessionEnd> {
    let previous_window = screen.window;
    let outcome = policy::apply_intent(&mut screen.engine, intent).map_err(failure)?;
    if outcome.scroll_to_bottom {
        screen.window = Window::Bottom;
    }
    match outcome.result {
        KeyResult::Bytes(bytes) => write(transport, &bytes).await?,
        KeyResult::ClearScrollback => {
            clear_local(screen)?;
            send_selection_state(session_id, None);
            return screen.changed(session_id).map_err(failure);
        }
        KeyResult::Action(_) | KeyResult::NoSend(_) | KeyResult::TypedInputReject(_) => {}
    }
    if screen.window != previous_window {
        screen.changed(session_id).map_err(failure)?;
    }
    Ok(())
}

async fn apply_pointer(
    session_id: u32,
    screen: &mut Screen,
    transport: &mut dyn SessionTransport,
    port: &Arc<InputPort>,
    seq: u64,
    pointer: port::FrozenPointer,
) -> Result<(), SessionEnd> {
    let input = pointer.input;
    let bounds = screen.engine.viewport_bounds();
    if input.geometry.row_revision != port.row_revision()
        || input.stable_hit_row < bounds.first_stable_row
        || input.stable_hit_row >= bounds.bottom_top_stable_row + i64::from(screen.rows)
    {
        return Ok(());
    }
    let remote = port.profile.mouse_reporting
        && screen.engine.display_modes().mouse_reporting
        && input.modifiers & 1 == 0;
    if remote {
        let kind = match input.kind {
            1 => "press",
            2 => "move",
            3 => "release",
            _ => "scroll",
        };
        let button = match input.button {
            1 => "left",
            2 => "middle",
            3 => "right",
            _ => "",
        };
        let count = if input.kind == 4 {
            input.delta_y.unsigned_abs()
        } else {
            1
        };
        for _ in 0..count {
            let button = if input.kind == 4 {
                if input.delta_y < 0 {
                    "wheel_up"
                } else {
                    "wheel_down"
                }
            } else {
                button
            };
            if let Some(event) = mouse_event_from_request(MouseRequest {
                session_id,
                kind: kind.into(),
                button: button.into(),
                col: input.remote_column as u16,
                row: input.remote_row as u16,
                shift: false,
                control: input.modifiers & 2 != 0,
                alt: input.modifiers & 4 != 0,
            }) && screen.mouse_motion.admit(&event)
            {
                let bytes = match screen.engine.encode_mouse(event) {
                    Ok(bytes) => bytes,
                    Err(rshell_m0::rshell_session::EngineError::UnsupportedMouse(_)) => continue,
                    Err(error) => return Err(failure(error)),
                };
                write(transport, &bytes).await?;
            }
        }
    } else if input.kind == 4 {
        let top = match screen.window {
            Window::Bottom => bounds.bottom_top_stable_row,
            Window::At { top, .. } => {
                top.clamp(bounds.first_stable_row, bounds.bottom_top_stable_row)
            }
        };
        screen.window = Window::At {
            top: top
                .saturating_add(i64::from(input.delta_y))
                .clamp(bounds.first_stable_row, bounds.bottom_top_stable_row),
            rows: screen.rows,
        };
    } else if input.button == 1 || input.kind == 2 && input.buttons & 1 != 0 {
        let position = CellPosition {
            stable_row: input.stable_hit_row,
            column: input.column as u16,
        };
        let start = if input.kind == 1 {
            position
        } else {
            screen.selection.map_or(position, |range| range.start)
        };
        screen.selection = Some(SelectionRange {
            start,
            end: position,
            rectangular: input.modifiers & 4 != 0,
        });
        send_selection_state(session_id, screen.selection);
        screen.changed(session_id).map_err(failure)?;
    } else if input.button == 3 && input.kind == 1 {
        crate::signals::desktop_native_input::DesktopNativeContextMenuReady {
            version: crate::signals::desktop_native_input::VERSION,
            source: port.source.source.clone(),
            identity: port.source.identity.clone(),
            attempt: pointer.ticket.binding_attempt(),
            focus_epoch: pointer.ticket.binding_epoch(),
            effect: seq,
            stable_row: input.stable_hit_row,
            column: input.column as u16,
        }
        .send_signal_to_dart();
    }
    Ok(())
}

pub(super) fn local_event(
    session_id: u32,
    screen: &mut Screen,
    event: DesktopEvent,
) -> Result<(), SessionEnd> {
    let command = match event {
        DesktopEvent::Viewport {
            follow_bottom,
            top_stable_row,
            rows,
        } => SessionCommand::Viewport(if follow_bottom {
            Window::Bottom
        } else {
            Window::At {
                top: top_stable_row,
                rows,
            }
        }),
        DesktopEvent::FrameAck { frame_seq } => SessionCommand::FrameAck(frame_seq),
        DesktopEvent::Selection {
            clear,
            anchor_row,
            anchor_col,
            focus_row,
            focus_col,
            rectangular,
        } => SessionCommand::Selection(SelectionRequest {
            session_id,
            clear,
            anchor_row,
            anchor_col,
            focus_row,
            focus_col,
            rectangular,
        }),
        _ => return Ok(()),
    };
    screen.handle_local(session_id, command).map_err(failure)?;
    Ok(())
}

pub(super) async fn finish_action(
    port: &Arc<InputPort>,
    session_id: u32,
    screen: &mut Screen,
    transport: Option<&mut dyn SessionTransport>,
    ticket: port::Ticket,
    action: DesktopAction,
    resolution: Resolution,
) -> Result<(), SessionEnd> {
    if action == DesktopAction::SelectAll {
        let bounds = screen.engine.viewport_bounds();
        let cols = screen.cols;
        screen.selection = Some(SelectionRange {
            start: CellPosition {
                stable_row: bounds.first_stable_row,
                column: 0,
            },
            end: CellPosition {
                stable_row: bounds.bottom_top_stable_row + i64::from(screen.rows) - 1,
                column: cols,
            },
            rectangular: false,
        });
        send_selection_state(session_id, screen.selection);
        screen.changed(session_id).map_err(failure)?;
        return Ok(());
    }
    if let Resolution::Action(ActionCompletion::Paste(text)) = resolution
        && port.applicable(ticket)
        && let Some(transport) = transport
    {
        scroll_keypress(screen);
        write(
            transport,
            &paste_bytes(&text.text, screen.engine.display_modes().bracketed_paste),
        )
        .await?;
    }
    Ok(())
}

fn action_value(
    port: &Arc<InputPort>,
    screen: &Screen,
    action: DesktopAction,
) -> port::ActionValue {
    let (copy_text, rejection) = if action == DesktopAction::Copy {
        match port.capture_copy(|| match screen.selection {
            Some(range) => screen.engine.selected_text(range),
            None => Ok(String::new()),
        }) {
            Ok(text) => (Some(text), None),
            Err(error) => (None, Some(error)),
        }
    } else {
        (None, None)
    };
    port::ActionValue {
        action,
        copy_text,
        rejection,
    }
}

#[cfg(test)]
#[path = "session_input_tests.rs"]
mod tests;
