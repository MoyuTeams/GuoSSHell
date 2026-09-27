//! 会话：每条 SSH 会话一个任务。连接由 [`crate::connect`] 建立；连上后把远端字节
//! 喂进 `DefaultTerminalEngine` → `render` → run 压缩 → `FrameUpdate`，并把 Dart 的
//! 输入、鼠标、选区、粘贴转给引擎与远端。
//!
//! 单任务拥有 transport（它的方法都要 `&mut self`），用 `select!` 同时听远端事件与
//! Dart 命令——这是上游 actor 的形状。会话类信号都带 `session_id`，[`supervisor`]
//! 按它把请求分给对应的会话任务。

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use rinf::{DartSignal, RustSignal, RustSignalBinary, debug_print};
use secrecy::SecretString;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::time::{Instant, sleep_until};

use rshell_m0::rshell_core::{
    CellPosition, KeyCode, KeyModifiers, MouseButton, MouseEventKind, RenderFrame, SelectionRange,
    SessionFailure, TerminalInput, TerminalMouseEvent, TerminalSize, Viewport,
};
use rshell_m0::rshell_session::{
    DefaultTerminalEngine, EngineDelta, EngineError, NativeSshTransport, SessionTransport,
    TerminalEngine, TransportEvent, ViewportBounds,
};

use crate::app::AppContext;
use crate::connect::{self, Abort, Channels, Connected, PromptIds};
use crate::frame_codec::pack_runs;
use crate::local_network;
use crate::settings;
use crate::signals::interaction::InteractionReply;
use crate::signals::{
    ClipboardText, ConnectHint, ConnectRequest, CopyRequest, DisconnectRequest, FailureKind,
    FrameAck, FrameUpdate, InputRequest, MouseRequest, PerfStats, ReconnectRequest, ResizeRequest,
    ScreenCheck, ScreenCheckRequest, SelectionRequest, SelectionState, SessionState, SessionStatus,
    ViewportRequest,
};

const NO_CURSOR: i32 = -1;
/// 远端 PTY 最小可行尺寸；Flutter 在极端布局下可能量出 0。
const MIN_COLS: u16 = 2;
const MIN_ROWS: u16 = 2;

pub enum SessionCommand {
    Resize(TerminalSize),
    Viewport(Window),
    Disconnect,
    /// 断开之后再连一次（画面与滚回保留）。
    Reconnect,
    Input(TerminalInput),
    Mouse(TerminalMouseEvent),
    Selection(SelectionRequest),
    Copy,
    Paste(String),
    FrameAck(u32),
    /// 画面一致性自检（M6，测试用）：按列回报引擎眼中的当前屏幕。
    ScreenCheck,
}

/// 显示的窗口：跟着屏幕（随输出滚动），或停在滚回里的某一段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    Bottom,
    /// 从 `top`（绝对行）起的 `rows` 行，含 Dart 要的上下余量。
    At {
        top: i64,
        rows: u16,
    },
}

impl Window {
    fn viewport(self, screen_rows: u16) -> Viewport {
        match self {
            // 引擎把越界的 top 夹到屏幕首行。
            Self::Bottom => Viewport {
                top_stable_row: i64::MAX,
                rows: screen_rows,
            },
            Self::At { top, rows } => Viewport {
                top_stable_row: top,
                rows: rows.max(1),
            },
        }
    }
}

/// 一个会话任务的收件口。
struct SessionHandle {
    /// 区分同一 `session_id` 先后的两个任务（结束通知只清自己的登记）。
    generation: u64,
    commands: UnboundedSender<SessionCommand>,
    replies: UnboundedSender<InteractionReply>,
}

/// 会话的结局。
struct SessionEnd {
    state: SessionState,
    failure: FailureKind,
    detail: String,
}

impl SessionEnd {
    fn closed(detail: impl Into<String>) -> Self {
        Self {
            state: SessionState::Closed,
            failure: FailureKind::None,
            detail: detail.into(),
        }
    }

    fn failed(failure: FailureKind, detail: impl Into<String>) -> Self {
        Self {
            state: SessionState::Failed,
            failure,
            detail: detail.into(),
        }
    }
}

/// 常驻任务：接住 Dart 的会话类请求，按 `session_id` 转给对应的会话任务。
pub async fn supervisor(context: Arc<AppContext>) {
    let connect_rx = ConnectRequest::get_dart_signal_receiver();
    let resize_rx = ResizeRequest::get_dart_signal_receiver();
    let disconnect_rx = DisconnectRequest::get_dart_signal_receiver();
    let viewport_rx = ViewportRequest::get_dart_signal_receiver();
    let reconnect_rx = ReconnectRequest::get_dart_signal_receiver();
    let input_rx = InputRequest::get_dart_signal_receiver();
    let mouse_rx = MouseRequest::get_dart_signal_receiver();
    let selection_rx = SelectionRequest::get_dart_signal_receiver();
    let copy_rx = CopyRequest::get_dart_signal_receiver();
    let ack_rx = FrameAck::get_dart_signal_receiver();
    let screen_check_rx = ScreenCheckRequest::get_dart_signal_receiver();
    let reply_rx = InteractionReply::get_dart_signal_receiver();
    let (finished_tx, mut finished_rx) = unbounded_channel::<(u32, u64)>();
    let mut sessions: HashMap<u32, SessionHandle> = HashMap::new();
    let mut generations: u64 = 0;

    let send =
        |sessions: &HashMap<u32, SessionHandle>, session_id: u32, command: SessionCommand| {
            if let Some(handle) = sessions.get(&session_id) {
                let _ = handle.commands.send(command);
            }
        };

    loop {
        tokio::select! {
            pack = connect_rx.recv() => {
                let Some(pack) = pack else { break };
                let request = pack.message;
                let session_id = request.session_id;
                debug_print!("[session {session_id}] connect ({}x{})", request.cols, request.rows);
                if let Some(old) = sessions.remove(&session_id) {
                    let _ = old.commands.send(SessionCommand::Disconnect);
                }
                generations += 1;
                let generation = generations;
                let (commands_tx, commands) = unbounded_channel();
                let (replies_tx, replies) = unbounded_channel();
                sessions.insert(session_id, SessionHandle {
                    generation,
                    commands: commands_tx,
                    replies: replies_tx,
                });
                let context = context.clone();
                let finished = finished_tx.clone();
                tokio::spawn(async move {
                    run_session(context, request, commands, replies).await;
                    let _ = finished.send((session_id, generation));
                });
            }
            finished = finished_rx.recv() => {
                if let Some((session_id, generation)) = finished
                    && sessions.get(&session_id).is_some_and(|handle| handle.generation == generation)
                {
                    sessions.remove(&session_id);
                }
            }
            pack = resize_rx.recv() => {
                let Some(pack) = pack else { break };
                let request = pack.message;
                let size = clamped_size(TerminalSize {
                    cols: request.cols,
                    rows: request.rows,
                    pixel_width: request.pixel_width,
                    pixel_height: request.pixel_height,
                    dpi: request.dpi,
                });
                send(&sessions, request.session_id, SessionCommand::Resize(size));
            }
            pack = disconnect_rx.recv() => {
                let Some(pack) = pack else { break };
                send(&sessions, pack.message.session_id, SessionCommand::Disconnect);
            }
            pack = reconnect_rx.recv() => {
                let Some(pack) = pack else { break };
                send(&sessions, pack.message.session_id, SessionCommand::Reconnect);
            }
            pack = viewport_rx.recv() => {
                let Some(pack) = pack else { break };
                let request = pack.message;
                let window = if request.follow_bottom {
                    Window::Bottom
                } else {
                    Window::At { top: request.top_stable_row, rows: request.rows }
                };
                send(&sessions, request.session_id, SessionCommand::Viewport(window));
            }
            pack = input_rx.recv() => {
                let Some(pack) = pack else { break };
                let session_id = pack.message.session_id;
                if let Some(command) = input_command_from_request(pack.message) {
                    send(&sessions, session_id, command);
                }
            }
            pack = mouse_rx.recv() => {
                let Some(pack) = pack else { break };
                let session_id = pack.message.session_id;
                if let Some(event) = mouse_event_from_request(pack.message) {
                    send(&sessions, session_id, SessionCommand::Mouse(event));
                }
            }
            pack = selection_rx.recv() => {
                let Some(pack) = pack else { break };
                send(&sessions, pack.message.session_id, SessionCommand::Selection(pack.message));
            }
            pack = copy_rx.recv() => {
                let Some(pack) = pack else { break };
                send(&sessions, pack.message.session_id, SessionCommand::Copy);
            }
            pack = ack_rx.recv() => {
                let Some(pack) = pack else { break };
                send(&sessions, pack.message.session_id, SessionCommand::FrameAck(pack.message.seq));
            }
            pack = screen_check_rx.recv() => {
                let Some(pack) = pack else { break };
                send(&sessions, pack.message.session_id, SessionCommand::ScreenCheck);
            }
            pack = reply_rx.recv() => {
                let Some(pack) = pack else { break };
                if let Some(handle) = sessions.get(&pack.message.session_id) {
                    let _ = handle.replies.send(pack.message);
                }
            }
        }
    }
}

/// 终端：引擎（网格与滚回）与显示状态。第一次连上时建立，断线重连时保留——重连后画面与
/// 滚回都还在，新的输出接在后面。
struct Screen {
    engine: DefaultTerminalEngine,
    /// 显示的窗口由 Dart 按滚动位置请求；打字、粘贴时回到屏幕。
    window: Window,
    rows: u16,
    stats: PerfWindow,
    pacer: FramePacer,
    mouse_motion: MouseMotion,
    /// 选区权威在引擎（M2a 方案 A）：一直持有当前选区，每次 render 都带上它——内容重排、
    /// 滚动时高亮跟着引擎走，不是 Dart 侧自己维护一套坐标。
    selection: Option<SelectionRange>,
    /// 最近发出的一帧与它的序号（画面一致性自检回报的就是它）。
    last_sent: Option<(u32, Arc<RenderFrame>)>,
}

impl Screen {
    fn new(engine: DefaultTerminalEngine, size: TerminalSize) -> Self {
        Self {
            engine,
            window: Window::Bottom,
            rows: size.rows,
            stats: PerfWindow::new(),
            pacer: FramePacer::new(),
            mouse_motion: MouseMotion::default(),
            selection: None,
            last_sent: None,
        }
    }

    /// 节拍器已允许本次发送，实际渲染并记录在途帧。调用者必须先经 changed / flush。
    fn present(&mut self, session_id: u32) -> Result<(), String> {
        self.last_sent = Some(present(
            session_id,
            &mut self.engine,
            self.window.viewport(self.rows),
            self.selection,
            &mut self.stats,
            &mut self.pacer,
        )?);
        Ok(())
    }

    /// 内容或窗口变了：节拍允许就出帧，否则记为待发。
    fn changed(&mut self, session_id: u32) -> Result<(), String> {
        if self.pacer.mark_dirty(Instant::now()) {
            self.present(session_id)
        } else {
            Ok(())
        }
    }

    /// 攒着的变化：节拍允许了就画出来。
    fn flush(&mut self, session_id: u32) -> Result<(), String> {
        if self.pacer.ready_for_pending(Instant::now()) {
            self.present(session_id)
        } else {
            Ok(())
        }
    }

    /// 远端输出进引擎（顺便记进吞吐统计）。
    fn advance(&mut self, bytes: &[u8]) -> Result<EngineDelta, EngineError> {
        self.stats.record_input(bytes.len());
        self.engine.advance(bytes)
    }

    /// 同步输出（DEC 2026）进行中时它的截止时刻：到点还没收到结束序列就由 [`Self::end_sync`] 结束。
    fn sync_deadline(&self) -> Option<Instant> {
        self.engine.sync_deadline().map(Instant::from_std)
    }

    /// 结束到点的同步输出，把攒着的输出画上去（程序在一帧中途退出时画面不卡住）；
    /// 返回要回写远端的查询应答。
    fn end_sync(&mut self, session_id: u32) -> Result<Vec<u8>, String> {
        let delta = self
            .engine
            .end_sync()
            .map_err(|error| format!("end sync: {error:?}"))?;
        if delta.dirty {
            self.stats.sync_timeouts += 1;
            self.changed(session_id)?;
        }
        Ok(delta.outbound)
    }

    fn resize(&mut self, session_id: u32, size: TerminalSize) -> Result<(), String> {
        self.engine
            .resize(size)
            .map_err(|error| format!("engine resize: {error:?}"))?;
        self.rows = size.rows;
        self.changed(session_id)
    }

    /// 与连接无关的命令（滚回、选区、复制、帧确认）：处理掉返回 `None`，其余原样交回。
    fn handle_local(
        &mut self,
        session_id: u32,
        command: SessionCommand,
    ) -> Result<Option<SessionCommand>, String> {
        match command {
            SessionCommand::Viewport(window) => {
                self.window = window;
                self.changed(session_id)?;
            }
            SessionCommand::Selection(request) => {
                // 选区变化：更新引擎持有的选区 → 重渲染发帧（高亮跟着内容走）→ 把引擎的
                // 选区原样回显（Dart 用它对耳朵/气泡定位）。
                self.selection = selection_range_from_request(request);
                self.changed(session_id)?;
                send_selection_state(session_id, self.selection);
            }
            SessionCommand::Copy => {
                // 取文在引擎里（跨行拼接、裁行尾空格都由它负责）。
                let text = match self.selection {
                    Some(range) => self.engine.selected_text(range).unwrap_or_default(),
                    None => String::new(),
                };
                ClipboardText { session_id, text }.send_signal_to_dart();
            }
            SessionCommand::FrameAck(seq) => {
                // 上一帧 Dart 已处理完：攒着的变化现在可以画了（节拍允许的话）。
                if let Some(latency) = self.pacer.acked(seq, Instant::now()) {
                    self.stats.record_latency(latency);
                }
                self.flush(session_id)?;
            }
            SessionCommand::ScreenCheck => {
                // 自检只在测试里用：顺带交出当前统计窗口（不等满 5 秒），几秒就结束的场景也有数据。
                if let Some(perf) = self.stats.report(session_id) {
                    perf.send_signal_to_dart();
                }
                if let Some((seq, frame)) = &self.last_sent {
                    screen_check(session_id, *seq, frame).send_signal_to_dart();
                }
            }
            other => return Ok(Some(other)),
        }
        Ok(None)
    }
}

/// 断开之后怎么办。
enum Next {
    Reconnect,
    /// 窗格关了（或 App 退出）：会话结束。
    Quit,
}

async fn run_session(
    context: Arc<AppContext>,
    mut request: ConnectRequest,
    mut commands: UnboundedReceiver<SessionCommand>,
    mut replies: UnboundedReceiver<InteractionReply>,
) {
    let session_id = request.session_id;
    // 快速连接的密码：所有权直接移进 SecretString（drop 时清零），请求里不留明文副本
    // （PLAN.md §2.2：secret 不可序列化，边界上手写转换）。只用于第一次连接，重连时再问。
    let mut quick_password = Some(SecretString::from(std::mem::take(&mut request.password)));
    // 首个 PTY 尺寸就是连接请求带来的几何（Dart 在布局完成后才发请求）；之后跟着改尺寸走。
    let mut size = clamped_size(TerminalSize {
        cols: request.cols,
        rows: request.rows,
        pixel_width: request.pixel_width,
        pixel_height: request.pixel_height,
        dpi: request.dpi,
    });
    // 终端在第一次连上时建立，重连时保留。
    let mut terminal: Option<Screen> = None;
    // 连接期间收到的输入 / 尺寸变化：连上后先按原顺序处理。
    let mut backlog = VecDeque::new();
    // 回答通道跨重连复用，提示编号也必须跨重连唯一，旧回答不能命中新提示。
    let mut prompts = PromptIds::default();

    loop {
        let first_attempt = quick_password.is_some();
        let password = quick_password
            .take()
            .unwrap_or_else(|| SecretString::from(String::new()));
        let attempt = async {
            let target = connect::resolve_target(&context, &request)
                .await
                .map_err(|abort| (abort, String::new(), 0))?;
            let described = target.describe();
            let (host, port) = (target.profile.host.clone(), target.profile.port);
            report(session_id, SessionState::Connecting, described.clone());
            let channels = Channels {
                commands: &mut commands,
                replies: &mut replies,
                backlog: &mut backlog,
            };
            connect::establish(
                &context,
                session_id,
                target,
                password,
                size,
                channels,
                &mut prompts,
            )
            .await
            .map(|connected| (connected, described))
            .map_err(|abort| (abort, host, port))
        };
        let (
            Connected {
                mut transport,
                profile,
            },
            described,
        ) = match attempt.await {
            Ok(connected) => connected,
            Err((Abort::Closed, _, _)) => {
                report(session_id, SessionState::Closed, "disconnect".into());
                return;
            }
            Err((abort, host, port)) => {
                // 第一次连接就被取消（关了密码框等）：会话就此结束（Dart 关掉窗格）。
                // 其余情况留在断开状态，等用户重试或关掉窗格。
                if first_attempt && matches!(abort, Abort::Cancelled) {
                    report(session_id, SessionState::Cancelled, String::new());
                    return;
                }
                match abort {
                    Abort::Cancelled => {
                        report(session_id, SessionState::Closed, "cancelled".into())
                    }
                    abort => report_abort(session_id, abort, &host, port).await,
                }
                match wait_offline(
                    session_id,
                    &mut terminal,
                    &mut size,
                    &mut commands,
                    &mut backlog,
                )
                .await
                {
                    Next::Reconnect => continue,
                    Next::Quit => return,
                }
            }
        };
        debug_print!("[session {session_id}] connected");
        report(session_id, SessionState::Connected, described);

        let screen = match &mut terminal {
            // 重连：回到主屏、复位上一个远端留下的模式（滚回不动），另起一行接新的输出。
            Some(screen) => {
                let _ = screen.engine.recover_display();
                let _ = screen.engine.advance(b"\r\n");
                screen.window = Window::Bottom;
                screen
            }
            None => {
                let resolved = settings::terminal_settings(&context)
                    .await
                    .resolve(&profile.terminal_overrides);
                match DefaultTerminalEngine::new(&resolved, size) {
                    Ok(engine) => terminal.insert(Screen::new(engine, size)),
                    Err(error) => {
                        report_end(
                            session_id,
                            SessionEnd::failed(FailureKind::Other, format!("engine: {error:?}")),
                        );
                        let _ = transport.shutdown().await;
                        return;
                    }
                }
            }
        };
        let (end, quit) = run_connected(
            session_id,
            screen,
            &mut transport,
            &mut size,
            &mut commands,
            &mut backlog,
        )
        .await;
        // 最后一屏的待发变化仍由离线循环按 ACK / 节拍发送；窗格关闭时直接释放。
        report_end(session_id, end);
        let _ = transport.shutdown().await;
        if quit {
            return;
        }
        match wait_offline(
            session_id,
            &mut terminal,
            &mut size,
            &mut commands,
            &mut backlog,
        )
        .await
        {
            Next::Reconnect => {}
            Next::Quit => return,
        }
    }
}

/// 连着的时候：远端输出进引擎、Dart 的命令转给引擎与远端。返回会话怎么结束的，以及是不是
/// 窗格关了（此时不再等重连）。
async fn run_connected(
    session_id: u32,
    screen: &mut Screen,
    transport: &mut NativeSshTransport,
    size: &mut TerminalSize,
    commands: &mut UnboundedReceiver<SessionCommand>,
    backlog: &mut VecDeque<SessionCommand>,
) -> (SessionEnd, bool) {
    // 第一次连接立即送首帧；重连时仍遵守已有画面的 ACK / 节拍。
    if let Err(detail) = screen.changed(session_id) {
        return (SessionEnd::failed(FailureKind::Other, detail), false);
    }
    send_selection_state(session_id, screen.selection);

    loop {
        let frame_deadline = screen.pacer.deadline();
        let sync_deadline = screen.sync_deadline();
        tokio::select! {
            event = transport.next_event() => match event {
                Ok(TransportEvent::Output(bytes)) => match screen.advance(&bytes) {
                    Ok(delta) => {
                        // 引擎对远端查询的应答（DA / 光标位置报告…）必须回写，
                        // 否则对端会一直等。
                        if !delta.outbound.is_empty() {
                            let _ = transport.write(&delta.outbound).await;
                        }
                        if delta.dirty
                            && let Err(detail) = screen.changed(session_id)
                        {
                            return (SessionEnd::failed(FailureKind::Other, detail), false);
                        }
                    }
                    Err(error) => {
                        return (SessionEnd::failed(FailureKind::Other, format!("advance: {error:?}")), false);
                    }
                },
                Ok(TransportEvent::Failure(failure)) => {
                    return (SessionEnd::failed(lost(failure.failure()), format!("session: {failure:?}")), false);
                }
                Ok(TransportEvent::Eof) => return (SessionEnd::closed("eof"), false),
                Ok(TransportEvent::Exit(status)) => {
                    return (
                        SessionEnd::closed(format!("exit code={:?} success={}", status.code, status.success)),
                        false,
                    );
                }
                Ok(_) => {}
                Err(error) => {
                    return (SessionEnd::failed(lost(error.failure()), format!("transport: {error:?}")), false);
                }
            },
            // 合帧：高输出期间攒下的内容到点一次性画出。
            () = sleep_until(frame_deadline.unwrap_or_else(Instant::now)), if frame_deadline.is_some() => {
                if let Err(detail) = screen.flush(session_id) {
                    return (SessionEnd::failed(FailureKind::Other, detail), false);
                }
            }
            () = sleep_until(sync_deadline.unwrap_or_else(Instant::now)), if sync_deadline.is_some() => {
                match screen.end_sync(session_id) {
                    Ok(outbound) if !outbound.is_empty() => {
                        let _ = transport.write(&outbound).await;
                    }
                    Ok(_) => {}
                    Err(detail) => return (SessionEnd::failed(FailureKind::Other, detail), false),
                }
            }
            command = next_command(backlog, commands) => {
                let command = match command {
                    Some(command) => command,
                    None => return (SessionEnd::closed("disconnect"), true),
                };
                let command = match screen.handle_local(session_id, command) {
                    Ok(Some(command)) => command,
                    Ok(None) => continue,
                    Err(detail) => return (SessionEnd::failed(FailureKind::Other, detail), false),
                };
                match command {
                    SessionCommand::Resize(new_size) => {
                        *size = new_size;
                        if let Err(detail) = screen.resize(session_id, new_size) {
                            return (SessionEnd::failed(FailureKind::Other, detail), false);
                        }
                        if let Err(error) = transport.resize(new_size).await {
                            // window-change 失败不立刻判死；远端布局暂旧，后续 resize 可再试。
                            debug_print!("window-change failed: {error:?}");
                        }
                    }
                    SessionCommand::Input(input) => {
                        // 键编码（ETX/Kitty/CSI-u）在引擎里，这里只负责把编码结果写进
                        // transport。打字回到屏幕（Dart 同时滚到底）。
                        screen.window = Window::Bottom;
                        match screen.engine.encode_input(input) {
                            Ok(bytes) if !bytes.is_empty() => {
                                if let Err(error) = transport.write(&bytes).await {
                                    return (SessionEnd::failed(lost(error.failure()), format!("input write: {error:?}")), false);
                                }
                            }
                            Ok(_) => {}
                            Err(error) => {
                                return (SessionEnd::failed(FailureKind::Other, format!("encode_input: {error:?}")), false);
                            }
                        }
                    }
                    SessionCommand::Mouse(event) => {
                        // 远端没开对应的鼠标上报（shell 等）时 encode 返回 Err——
                        // 远端不要这类事件，静默忽略（M2a）。
                        if screen.mouse_motion.admit(&event)
                            && let Ok(bytes) = screen.engine.encode_mouse(event)
                            && !bytes.is_empty()
                            && let Err(error) = transport.write(&bytes).await
                        {
                            return (SessionEnd::failed(lost(error.failure()), format!("mouse write: {error:?}")), false);
                        }
                    }
                    SessionCommand::Paste(text) => {
                        screen.window = Window::Bottom;
                        let bytes = paste_bytes(&text, screen.engine.display_modes().bracketed_paste);
                        if !bytes.is_empty()
                            && let Err(error) = transport.write(&bytes).await
                        {
                            return (SessionEnd::failed(lost(error.failure()), format!("paste write: {error:?}")), false);
                        }
                    }
                    SessionCommand::Disconnect => return (SessionEnd::closed("disconnect"), true),
                    // 连着的时候重连无意义。
                    SessionCommand::Reconnect => {}
                    SessionCommand::Viewport(_)
                    | SessionCommand::Selection(_)
                    | SessionCommand::Copy
                    | SessionCommand::FrameAck(_)
                    | SessionCommand::ScreenCheck => {}
                }
            }
        }
    }
}

/// 断开之后：滚回、选区、复制照常（引擎还在），尺寸变化记下来留给重连；等用户重连或关掉
/// 窗格。连接期间攒下的输入已经没有去处，丢掉。
async fn wait_offline(
    session_id: u32,
    screen: &mut Option<Screen>,
    size: &mut TerminalSize,
    commands: &mut UnboundedReceiver<SessionCommand>,
    backlog: &mut VecDeque<SessionCommand>,
) -> Next {
    for command in backlog.drain(..) {
        if let SessionCommand::Resize(new_size) = command {
            *size = new_size;
        }
    }
    loop {
        let frame_deadline = screen.as_ref().and_then(|screen| screen.pacer.deadline());
        // 断在一帧同步输出的中途（程序被杀、连接断开）：到点把攒着的输出画上去。
        let sync_deadline = screen.as_ref().and_then(Screen::sync_deadline);
        tokio::select! {
            () = sleep_until(frame_deadline.unwrap_or_else(Instant::now)), if frame_deadline.is_some() => {
                if let Some(screen) = screen.as_mut()
                    && let Err(detail) = screen.flush(session_id)
                {
                    debug_print!("[session {session_id}] offline frame: {detail}");
                }
            }
            () = sleep_until(sync_deadline.unwrap_or_else(Instant::now)), if sync_deadline.is_some() => {
                if let Some(screen) = screen.as_mut()
                    && let Err(detail) = screen.end_sync(session_id)
                {
                    debug_print!("[session {session_id}] offline sync: {detail}");
                }
            }
            command = commands.recv() => match command {
                Some(SessionCommand::Reconnect) => return Next::Reconnect,
                Some(SessionCommand::Disconnect) | None => return Next::Quit,
                Some(SessionCommand::Resize(new_size)) => {
                    *size = new_size;
                    if let Some(screen) = screen.as_mut()
                        && let Err(detail) = screen.resize(session_id, new_size)
                    {
                        debug_print!("[session {session_id}] offline resize: {detail}");
                    }
                }
                // 没有连接可写：输入、鼠标、粘贴都丢掉。
                Some(command) => {
                    if let Some(screen) = screen.as_mut()
                        && let Err(detail) = screen.handle_local(session_id, command)
                    {
                        debug_print!("[session {session_id}] offline: {detail}");
                    }
                }
            },
        }
    }
}

/// 远端设置的窗口标题；没设置时上游给的是它自己的名字，当作没有标题（标签上显示连接名）。
fn remote_title(title: &str) -> String {
    const UPSTREAM_DEFAULT_TITLE: &str = "rsHell";
    if title == UPSTREAM_DEFAULT_TITLE {
        String::new()
    } else {
        title.to_owned()
    }
}

/// 连上之后的网络失败与超时：连接断了（区别于连不上）。
fn lost(failure: SessionFailure) -> FailureKind {
    match failure {
        SessionFailure::Network | SessionFailure::Timeout => FailureKind::ConnectionLost,
        failure => connect::failure_kind(failure),
    }
}

/// 下一条命令：连接期间攒下的先出。
async fn next_command(
    backlog: &mut VecDeque<SessionCommand>,
    commands: &mut UnboundedReceiver<SessionCommand>,
) -> Option<SessionCommand> {
    match backlog.pop_front() {
        Some(command) => Some(command),
        None => commands.recv().await,
    }
}

fn report(session_id: u32, state: SessionState, detail: String) {
    SessionStatus {
        session_id,
        state,
        failure: FailureKind::None,
        detail,
        local_network_settings_url: String::new(),
        hint: ConnectHint::None,
    }
    .send_signal_to_dart();
}

fn report_end(session_id: u32, end: SessionEnd) {
    SessionStatus {
        session_id,
        state: end.state,
        failure: end.failure,
        detail: end.detail,
        local_network_settings_url: String::new(),
        hint: ConnectHint::None,
    }
    .send_signal_to_dart();
}

/// 连接没建成：取消报 Closed；失败时顺带判断是否可能是本地网络权限。
async fn report_abort(session_id: u32, abort: Abort, host: &str, port: u16) {
    match abort {
        Abort::Closed => report(session_id, SessionState::Closed, "disconnect".into()),
        Abort::Cancelled => report(session_id, SessionState::Cancelled, String::new()),
        Abort::Failed { failure, detail } => SessionStatus {
            session_id,
            state: SessionState::Failed,
            failure,
            detail,
            local_network_settings_url: local_network::settings_url(host, port, failure).await,
            hint: ConnectHint::None,
        }
        .send_signal_to_dart(),
    }
}

/// 鼠标移动只在跨格（或换了按住的键）时上报——与 xterm 一致；指针在同一格内
/// 的移动对远端没有信息量。按下 / 松开也记下位置，紧随其后的同格移动不重报。
#[derive(Default)]
struct MouseMotion {
    last: Option<(u16, u16, Option<MouseButton>)>,
}

impl MouseMotion {
    fn admit(&mut self, event: &TerminalMouseEvent) -> bool {
        let held = match event.kind {
            MouseEventKind::Scroll => return true,
            MouseEventKind::Press | MouseEventKind::Move => event.button,
            MouseEventKind::Release => None,
        };
        let here = Some((event.cell.column, event.viewport_row, held));
        if event.kind == MouseEventKind::Move && here == self.last {
            return false;
        }
        self.last = here;
        true
    }
}

const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &[u8] = b"\x1b[201~";

/// 粘贴文本 → 写给远端的字节：
/// * 换行统一成 CR（CRLF / LF → CR，与 xterm / VTE 一致；回车就是 CR）；
/// * 去掉 Tab / 换行以外的 C0 控制字符、DEL 与 C1 控制字符——粘贴内容里的 ESC 等
///   能伪造按键或提前结束括号粘贴（`ESC[201~` 注入），一律不外发；
/// * 远端开了 bracketed paste（DECSET 2004）时包上 `ESC[200~` … `ESC[201~`。
fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let mut body = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                body.push('\r');
            }
            '\n' => body.push('\r'),
            '\t' => body.push('\t'),
            ch if ch.is_control() => {}
            ch => body.push(ch),
        }
    }
    if body.is_empty() {
        return Vec::new();
    }
    if !bracketed {
        return body.into_bytes();
    }
    let mut bytes = Vec::with_capacity(PASTE_START.len() + body.len() + PASTE_END.len());
    bytes.extend_from_slice(PASTE_START);
    bytes.extend_from_slice(body.as_bytes());
    bytes.extend_from_slice(PASTE_END);
    bytes
}

/// 远端 PTY 的尺寸下限：Flutter 在极端布局下可能量出 0 行/列。
fn clamped_size(size: TerminalSize) -> TerminalSize {
    TerminalSize {
        cols: size.cols.max(MIN_COLS),
        rows: size.rows.max(MIN_ROWS),
        ..size
    }
}

/// 渲染当前视口并发帧（附带 5 秒一次的性能汇总）。
fn present<E: TerminalEngine>(
    session_id: u32,
    engine: &mut E,
    viewport: Viewport,
    selection: Option<SelectionRange>,
    stats: &mut PerfWindow,
    pacer: &mut FramePacer,
) -> Result<(u32, Arc<RenderFrame>), String> {
    if pacer.awaiting_ack() {
        // 上一帧 Dart 还没确认就照发了（节拍器等 ACK 超时才会走到这里）。
        stats.ack_timeouts += 1;
    }
    pacer.started(Instant::now());
    let render_start = std::time::Instant::now();
    let frame = engine
        .render(viewport, selection)
        .map_err(|error| format!("render: {error:?}"))?;
    let render_us = micros_since(render_start);
    let seq = send_frame(
        session_id,
        &frame,
        engine.viewport_bounds(),
        stats,
        render_us,
    );
    pacer.sent(seq, Instant::now());
    if let Some(perf) = stats.maybe_report(session_id) {
        perf.send_signal_to_dart();
    }
    Ok((seq, frame))
}

/// 帧节拍与流控：
/// * 同一时刻最多一帧在途——Dart 回 [`FrameAck`](crate::signals::FrameAck) 之前不发下一帧，
///   期间的变化只标记待发，Dart 跟不上时只画最新状态（rinf 的队列无界，不能靠它积压）；
/// * 两帧的渲染起点至少相隔 [`Self::INTERVAL`]（上限约 120 Hz），`cat` 大文件这类
///   高输出不会逐块出帧；远小于 60 Hz 源的周期，不会误合并 60 Hz 的刷新；
/// * 空闲后的第一帧立即发（打字回显不等）；ACK 迟迟不来（App 暂停等）时
///   [`Self::ACK_TIMEOUT`] 后照发，防止卡死。
struct FramePacer {
    last_start: Option<Instant>,
    in_flight: Option<InFlight>,
    pending: bool,
    /// 还没画出去的变化最早是什么时候来的（显示延迟的起点）。
    dirty_since: Option<Instant>,
}

/// 在途的一帧：帧序号、发出时刻、它所含内容最早的到达时刻。
#[derive(Clone, Copy)]
struct InFlight {
    seq: u32,
    sent: Instant,
    since: Option<Instant>,
}

impl FramePacer {
    const INTERVAL: Duration = Duration::from_millis(8);
    const ACK_TIMEOUT: Duration = Duration::from_millis(250);

    fn new() -> Self {
        Self {
            last_start: None,
            in_flight: None,
            pending: false,
            dirty_since: None,
        }
    }

    /// 内容变了。返回 true = 现在就出帧；false = 已记为待发。
    fn mark_dirty(&mut self, now: Instant) -> bool {
        self.dirty_since.get_or_insert(now);
        if self.may_send(now) {
            return true;
        }
        self.pending = true;
        false
    }

    /// 有待发的帧，且此刻允许发。
    fn ready_for_pending(&self, now: Instant) -> bool {
        self.pending && self.may_send(now)
    }

    /// 需要醒来检查待发帧的时刻：节拍到点，或在途帧的 ACK 超时。
    /// 等 ACK 的情况下 ACK 本身会唤醒循环，这里只给超时兜底。
    fn deadline(&self) -> Option<Instant> {
        if !self.pending {
            return None;
        }
        let beat = self.last_start.map(|start| start + Self::INTERVAL);
        let ack = self.in_flight.map(|frame| frame.sent + Self::ACK_TIMEOUT);
        match (beat, ack) {
            (Some(beat), Some(ack)) => Some(beat.max(ack)),
            (beat, ack) => beat.or(ack),
        }
    }

    fn may_send(&self, now: Instant) -> bool {
        let beat_ok = self
            .last_start
            .is_none_or(|start| now >= start + Self::INTERVAL);
        let ack_ok = self
            .in_flight
            .is_none_or(|frame| now >= frame.sent + Self::ACK_TIMEOUT);
        beat_ok && ack_ok
    }

    /// 开始渲染一帧（节拍从渲染起点算，渲染耗时不推迟下一拍）。
    fn started(&mut self, now: Instant) {
        self.last_start = Some(now);
    }

    /// 帧 `seq` 已发出；它包含此前所有变化。
    fn sent(&mut self, seq: u32, now: Instant) {
        self.in_flight = Some(InFlight {
            seq,
            sent: now,
            since: self.dirty_since.take(),
        });
        self.pending = false;
    }

    /// 上一帧发出后还没等到确认。
    fn awaiting_ack(&self) -> bool {
        self.in_flight.is_some()
    }

    /// Dart 处理完了帧 `seq`（以及它之前的帧）。返回这一帧所含内容的显示延迟：从最早的
    /// 那次变化到现在。
    fn acked(&mut self, seq: u32, now: Instant) -> Option<Duration> {
        let frame = self.in_flight?;
        if seq.wrapping_sub(frame.seq) >= u32::MAX / 2 {
            return None;
        }
        self.in_flight = None;
        frame
            .since
            .map(|since| now.saturating_duration_since(since))
    }
}

/// 把引擎当前持有的选区回显给 Dart。保留 anchor/focus 的原始角色不排序
/// （`SelectionRange` 渲染/取文时才 `ordered`），拖耳朵越过对端时角色才不会乱。
fn send_selection_state(session_id: u32, selection: Option<SelectionRange>) {
    let state = match selection {
        Some(range) => SelectionState {
            session_id,
            has_selection: true,
            anchor_row: range.start.stable_row,
            anchor_col: range.start.column,
            focus_row: range.end.stable_row,
            focus_col: range.end.column,
        },
        None => SelectionState {
            session_id,
            has_selection: false,
            anchor_row: 0,
            anchor_col: 0,
            focus_row: 0,
            focus_col: 0,
        },
    };
    state.send_signal_to_dart();
}

/// 边界上的选区请求 → 上游 `SelectionRange`。`clear` 或端点相同都归为「无选区」。
fn selection_range_from_request(request: SelectionRequest) -> Option<SelectionRange> {
    if request.clear {
        return None;
    }
    let start = CellPosition {
        stable_row: request.anchor_row,
        column: request.anchor_col,
    };
    let end = CellPosition {
        stable_row: request.focus_row,
        column: request.focus_col,
    };
    if start == end {
        return None;
    }
    Some(SelectionRange {
        start,
        end,
        rectangular: request.rectangular,
    })
}

/// 把边界上的键名解析成上游 `KeyCode`（PLAN §5 M2：编码权威在 Rust）。
/// 返回 `None` = 无法识别的键，静默丢弃（不记日志——键入高频路径）。
fn parse_key_code(key: &str) -> Option<KeyCode> {
    const CHAR_PREFIX: &str = "character:";
    if let Some(rest) = key.strip_prefix(CHAR_PREFIX) {
        let mut chars = rest.chars();
        let first = chars.next()?;
        if chars.next().is_none() {
            return Some(KeyCode::Character(first));
        }
        return None;
    }
    Some(match key {
        "enter" => KeyCode::Enter,
        "escape" => KeyCode::Escape,
        "tab" => KeyCode::Tab,
        "backspace" => KeyCode::Backspace,
        "delete" => KeyCode::Delete,
        "insert" => KeyCode::Insert,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "page_up" => KeyCode::PageUp,
        "page_down" => KeyCode::PageDown,
        "arrow_up" => KeyCode::ArrowUp,
        "arrow_down" => KeyCode::ArrowDown,
        "arrow_left" => KeyCode::ArrowLeft,
        "arrow_right" => KeyCode::ArrowRight,
        other => {
            let digits = other.strip_prefix('f')?;
            let index = digits.parse::<u8>().ok()?;
            if !(1..=24).contains(&index) {
                return None;
            }
            KeyCode::F(index)
        }
    })
}

/// 键、IME 文本与粘贴共用一个 rinf 通道，按接收顺序进入同一会话命令队列。
fn input_command_from_request(request: InputRequest) -> Option<SessionCommand> {
    if request.paste {
        return Some(SessionCommand::Paste(request.text));
    }
    terminal_input_from_request(request).map(SessionCommand::Input)
}

fn terminal_input_from_request(request: InputRequest) -> Option<TerminalInput> {
    if !request.text.is_empty() {
        return Some(TerminalInput::CommittedText(request.text));
    }
    let code = parse_key_code(&request.key)?;
    Some(TerminalInput::Key {
        code,
        modifiers: KeyModifiers {
            shift: request.shift,
            control: request.control,
            alt: request.alt,
            super_key: false,
        },
    })
}

/// 鼠标请求 → 上游事件。滚轮必须走 Scroll；press/release 带普通键；
/// move 带按住的键（拖动）或不带（悬停）。
fn mouse_event_from_request(request: MouseRequest) -> Option<TerminalMouseEvent> {
    let kind = match request.kind.as_str() {
        "press" => MouseEventKind::Press,
        "release" => MouseEventKind::Release,
        "move" => MouseEventKind::Move,
        "scroll" => MouseEventKind::Scroll,
        _ => return None,
    };
    let button = match request.button.as_str() {
        "left" => Some(MouseButton::Left),
        "middle" => Some(MouseButton::Middle),
        "right" => Some(MouseButton::Right),
        "wheel_up" => Some(MouseButton::WheelUp),
        "wheel_down" => Some(MouseButton::WheelDown),
        _ => None,
    };
    Some(TerminalMouseEvent {
        kind,
        button,
        cell: CellPosition {
            stable_row: i64::from(request.row),
            column: request.col,
        },
        viewport_row: request.row,
        pixel_x: 0,
        pixel_y: 0,
        modifiers: KeyModifiers {
            shift: request.shift,
            control: request.control,
            alt: request.alt,
            super_key: false,
        },
    })
}

/// 一个统计窗口（5 秒）内的帧开销累计 + 会话帧序号。
struct PerfWindow {
    window_start: std::time::Instant,
    seq: u32,
    frames: u32,
    render_us_total: u64,
    render_us_max: u32,
    pack_us_total: u64,
    pack_us_max: u32,
    bytes_total: u64,
    /// 每帧的显示延迟（微秒），出报告时取分位数。
    latencies_us: Vec<u32>,
    ack_timeouts: u32,
    input_bytes: u64,
    sync_timeouts: u32,
}

impl PerfWindow {
    fn new() -> Self {
        Self {
            window_start: std::time::Instant::now(),
            seq: 0,
            frames: 0,
            render_us_total: 0,
            render_us_max: 0,
            pack_us_total: 0,
            pack_us_max: 0,
            bytes_total: 0,
            latencies_us: Vec::new(),
            ack_timeouts: 0,
            input_bytes: 0,
            sync_timeouts: 0,
        }
    }

    fn record_latency(&mut self, latency: Duration) {
        self.latencies_us
            .push(u32::try_from(latency.as_micros()).unwrap_or(u32::MAX));
    }

    fn record_input(&mut self, bytes: usize) {
        self.input_bytes += bytes as u64;
    }

    fn record(&mut self, render_us: u32, pack_us: u32, bytes: usize) -> u32 {
        self.seq = self.seq.wrapping_add(1);
        self.frames += 1;
        self.render_us_total += u64::from(render_us);
        self.render_us_max = self.render_us_max.max(render_us);
        self.pack_us_total += u64::from(pack_us);
        self.pack_us_max = self.pack_us_max.max(pack_us);
        self.bytes_total += bytes as u64;
        self.seq
    }

    /// 满 5 秒发一条汇总并重开窗口。
    fn maybe_report(&mut self, session_id: u32) -> Option<PerfStats> {
        if self.window_start.elapsed() < std::time::Duration::from_secs(5) {
            return None;
        }
        self.report(session_id)
    }

    /// 汇总当前窗口（有帧才发）并重开窗口。
    fn report(&mut self, session_id: u32) -> Option<PerfStats> {
        if self.frames == 0 {
            return None;
        }
        let elapsed = self.window_start.elapsed();
        let frames = self.frames;
        self.latencies_us.sort_unstable();
        let percentile = |p: usize| -> u32 {
            if self.latencies_us.is_empty() {
                return 0;
            }
            let index = (self.latencies_us.len() - 1) * p / 100;
            self.latencies_us[index]
        };
        let stats = PerfStats {
            session_id,
            frames,
            window_ms: u32::try_from(elapsed.as_millis()).unwrap_or(u32::MAX),
            render_us_avg: (self.render_us_total / u64::from(frames)) as u32,
            render_us_max: self.render_us_max,
            pack_us_avg: (self.pack_us_total / u64::from(frames)) as u32,
            pack_us_max: self.pack_us_max,
            bytes_avg: (self.bytes_total / u64::from(frames)) as u32,
            latency_us_p50: percentile(50),
            latency_us_p95: percentile(95),
            latency_us_max: percentile(100),
            ack_timeouts: self.ack_timeouts,
            input_bytes: self.input_bytes,
            sync_timeouts: self.sync_timeouts,
        };
        *self = Self {
            seq: self.seq,
            ..Self::new()
        };
        Some(stats)
    }
}

fn micros_since(start: std::time::Instant) -> u32 {
    u32::try_from(start.elapsed().as_micros()).unwrap_or(u32::MAX)
}

/// 发出的一帧按列展开：宽字符的第二列为空串（画面一致性自检）。
fn screen_check(session_id: u32, seq: u32, frame: &RenderFrame) -> ScreenCheck {
    let rows = frame
        .rows
        .iter()
        .map(|row| {
            let mut columns = Vec::with_capacity(usize::from(frame.size.cols));
            for cell in row.cells.iter() {
                columns.push(cell.text.clone());
                if cell.width == 2 {
                    columns.push(String::new());
                }
            }
            columns
        })
        .collect();
    ScreenCheck {
        session_id,
        seq,
        cols: frame.size.cols,
        stable_rows: frame.rows.iter().map(|row| row.stable_row).collect(),
        rows,
    }
}

/// 发出一帧，返回它的帧序号。
fn send_frame(
    session_id: u32,
    frame: &RenderFrame,
    bounds: ViewportBounds,
    stats: &mut PerfWindow,
    render_us: u32,
) -> u32 {
    // 屏幕首行：跟着屏幕时的视口起点（引擎按范围夹过）。
    let screen_top = bounds.clamp_top(i64::MAX);
    let (cursor_col, cursor_row) = match &frame.cursor {
        Some(cursor) => match i32::try_from(cursor.position.stable_row - screen_top) {
            Ok(row) if (0..i32::from(frame.size.rows)).contains(&row) => {
                (i32::from(cursor.position.column), row)
            }
            _ => (NO_CURSOR, NO_CURSOR),
        },
        None => (NO_CURSOR, NO_CURSOR),
    };
    let pack_start = std::time::Instant::now();
    let binary = pack_runs(frame);
    let pack_us = micros_since(pack_start);
    let seq = stats.record(render_us, pack_us, binary.len());
    FrameUpdate {
        session_id,
        cols: frame.size.cols,
        rows: frame.size.rows,
        seq,
        top_stable_row: frame.viewport_top,
        first_stable_row: bounds.first_stable_row,
        screen_top_stable_row: screen_top,
        cursor_col,
        cursor_row,
        title: remote_title(&frame.title),
        mouse_reporting: frame.mouse_reporting,
        alternate_screen: frame.alternate_screen,
    }
    .send_signal_to_dart(binary);
    seq
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::{
        FramePacer, MouseMotion, Next, Screen, SessionCommand, Window, input_command_from_request,
        lost, mouse_event_from_request, next_command, paste_bytes, terminal_input_from_request,
        wait_offline,
    };
    use crate::signals::FailureKind;
    use crate::signals::{InputRequest, MouseRequest, SelectionRequest};
    use rshell_m0::rshell_core::SessionFailure;
    use rshell_m0::rshell_core::{
        KeyCode, MouseButton, MouseEventKind, TerminalInput, TerminalOverrides, TerminalSettingsV1,
        TerminalSize,
    };
    use rshell_m0::rshell_session::{DefaultTerminalEngine, TerminalEngine};
    use std::collections::VecDeque;
    use std::time::Duration;
    use tokio::sync::mpsc::unbounded_channel;
    use tokio::time::Instant;

    fn input(key: &str, control: bool) -> Option<TerminalInput> {
        terminal_input_from_request(InputRequest {
            session_id: 1,
            paste: false,
            text: String::new(),
            key: key.to_owned(),
            shift: false,
            control,
            alt: false,
        })
    }

    #[test]
    fn named_keys_parse() {
        for (name, expected) in [
            ("enter", KeyCode::Enter),
            ("escape", KeyCode::Escape),
            ("tab", KeyCode::Tab),
            ("backspace", KeyCode::Backspace),
            ("arrow_up", KeyCode::ArrowUp),
            ("arrow_left", KeyCode::ArrowLeft),
        ] {
            assert!(
                matches!(input(name, false), Some(TerminalInput::Key { code, .. }) if code == expected)
            );
        }
        assert!(matches!(
            input("f12", false),
            Some(TerminalInput::Key {
                code: KeyCode::F(12),
                ..
            })
        ));
        assert!(input("f25", false).is_none());
        assert!(input("not_a_key", false).is_none());
    }

    #[test]
    fn function_keys_use_the_dart_wire_format() {
        // Dart 侧（frame_terminal.dart 的 _fKeyNames）发 "f1".."f12"。
        for index in 1..=12u8 {
            let name = format!("f{index}");
            assert!(matches!(
                input(&name, false),
                Some(TerminalInput::Key { code: KeyCode::F(parsed), .. }) if parsed == index
            ));
        }
        assert!(input("f:1", false).is_none());
    }

    #[test]
    fn pacer_sends_first_frame_immediately() {
        let now = Instant::now();
        let mut pacer = FramePacer::new();
        assert!(pacer.mark_dirty(now));
        assert_eq!(pacer.deadline(), None);
    }

    #[test]
    fn pacer_keeps_one_frame_in_flight() {
        let start = Instant::now();
        let mut pacer = FramePacer::new();
        pacer.started(start);
        pacer.sent(1, start);
        // 节拍已过，但上一帧还没 ACK：攒着。
        let later = start + FramePacer::INTERVAL * 2;
        assert!(!pacer.mark_dirty(later));
        assert!(!pacer.ready_for_pending(later));
        // ACK 到了：待发帧可以发。
        pacer.acked(1, later);
        assert!(pacer.ready_for_pending(later));
    }

    #[test]
    fn pacer_spaces_frames_by_the_interval() {
        let start = Instant::now();
        let mut pacer = FramePacer::new();
        pacer.started(start);
        pacer.sent(1, start);
        pacer.acked(1, start);
        assert!(!pacer.mark_dirty(start + Duration::from_millis(3)));
        assert_eq!(pacer.deadline(), Some(start + FramePacer::INTERVAL));
        // 60 Hz 的源（16.7 ms 一次）不会被合并。
        assert!(pacer.ready_for_pending(start + Duration::from_micros(16_700)));
    }

    /// 显示延迟从「还没画出去的最早一次变化」算起，到含它的那一帧被确认为止。
    #[test]
    fn pacer_measures_display_latency_from_the_first_pending_change() {
        let start = Instant::now();
        let mut pacer = FramePacer::new();
        pacer.started(start);
        pacer.sent(1, start);
        assert_eq!(
            pacer.acked(1, start),
            None,
            "a frame without pending changes has no latency"
        );
        let first = start + Duration::from_millis(2);
        assert!(!pacer.mark_dirty(first), "within the beat: kept pending");
        assert!(!pacer.mark_dirty(first + Duration::from_millis(1)));
        let beat = start + FramePacer::INTERVAL;
        pacer.started(beat);
        pacer.sent(2, beat);
        assert!(pacer.awaiting_ack());
        let acked = beat + Duration::from_millis(5);
        assert_eq!(pacer.acked(2, acked), Some(acked - first));
        assert!(!pacer.awaiting_ack());
    }

    #[test]
    fn pacer_gives_up_on_a_missing_ack() {
        let start = Instant::now();
        let mut pacer = FramePacer::new();
        pacer.started(start);
        pacer.sent(7, start);
        assert!(!pacer.mark_dirty(start + FramePacer::INTERVAL));
        assert_eq!(pacer.deadline(), Some(start + FramePacer::ACK_TIMEOUT));
        assert!(pacer.ready_for_pending(start + FramePacer::ACK_TIMEOUT));
    }

    #[test]
    fn pacer_ignores_stale_acks() {
        let start = Instant::now();
        let mut pacer = FramePacer::new();
        pacer.started(start);
        pacer.sent(5, start);
        pacer.acked(4, start);
        assert!(!pacer.mark_dirty(start + FramePacer::INTERVAL));
        pacer.acked(5, start);
        assert!(pacer.ready_for_pending(start + FramePacer::INTERVAL));
    }

    #[tokio::test(start_paused = true)]
    async fn selection_and_resize_share_the_pending_frame_until_ack() {
        let profile = TerminalSettingsV1::default().resolve(&TerminalOverrides::default());
        let mut size = TerminalSize {
            cols: 80,
            rows: 24,
            pixel_width: 0,
            pixel_height: 0,
            dpi: 96,
        };
        let engine = DefaultTerminalEngine::new(&profile, size).expect("引擎");
        let mut screen = Screen::new(engine, size);
        screen.changed(1).expect("第一帧");
        let first_seq = screen.last_sent.as_ref().expect("第一帧").0;
        for focus in 1..=10 {
            tokio::time::advance(FramePacer::INTERVAL).await;
            screen
                .handle_local(
                    1,
                    SessionCommand::Selection(SelectionRequest {
                        session_id: 1,
                        clear: false,
                        anchor_row: 0,
                        anchor_col: 0,
                        focus_row: 0,
                        focus_col: focus,
                        rectangular: false,
                    }),
                )
                .expect("拖动选区");
            size.cols -= 1;
            screen.resize(1, size).expect("连续缩放");
            screen.flush(1).expect("检查待发帧");
            assert_eq!(screen.last_sent.as_ref().expect("在途帧").0, first_seq);
            assert_eq!(screen.pacer.in_flight.expect("在途序号").seq, first_seq);
        }
        assert!(screen.pacer.pending);
        screen
            .handle_local(1, SessionCommand::FrameAck(first_seq))
            .expect("确认第一帧");
        let (seq, frame) = screen.last_sent.as_ref().expect("合并后的帧");
        assert_eq!(*seq, first_seq + 1);
        assert_eq!(frame.size.cols, 70);
        assert_eq!(screen.selection.expect("最终选区").end.column, 10);
        assert!(!screen.pacer.pending);
        assert_eq!(screen.stats.ack_timeouts, 0);
    }

    #[tokio::test(start_paused = true)]
    async fn offline_final_output_respects_the_original_ack() {
        for current_ack in [false, true] {
            let profile = TerminalSettingsV1::default().resolve(&TerminalOverrides::default());
            let mut size = TerminalSize {
                cols: 80,
                rows: 24,
                pixel_width: 0,
                pixel_height: 0,
                dpi: 96,
            };
            let engine = DefaultTerminalEngine::new(&profile, size).expect("引擎");
            let mut screen = Screen::new(engine, size);
            screen.changed(1).expect("第一帧");
            let first_seq = screen.last_sent.as_ref().expect("第一帧").0;
            screen.advance(b"final output").expect("断线前最后一段输出");
            screen.changed(1).expect("最后输出等待确认");
            tokio::time::advance(FramePacer::INTERVAL).await;

            let (sender, mut commands) = unbounded_channel();
            let acknowledged = if current_ack {
                first_seq
            } else {
                first_seq.wrapping_sub(1)
            };
            sender
                .send(SessionCommand::FrameAck(acknowledged))
                .expect("确认帧");
            sender
                .send(SessionCommand::Disconnect)
                .expect("结束离线循环");
            let mut terminal = Some(screen);
            assert!(matches!(
                wait_offline(
                    1,
                    &mut terminal,
                    &mut size,
                    &mut commands,
                    &mut VecDeque::new()
                )
                .await,
                Next::Quit
            ));
            let screen = terminal.expect("断线后仍保留画面");
            let (seq, frame) = screen.last_sent.expect("最后发出的帧");
            assert_eq!(seq, first_seq + u32::from(current_ack));
            assert_eq!(screen.pacer.pending, !current_ack);
            assert_eq!(screen.stats.ack_timeouts, 0);
            let visible: String = frame.rows[0]
                .cells
                .iter()
                .map(|cell| cell.text.as_str())
                .collect();
            assert_eq!(visible.starts_with("final output"), current_ack);
        }
    }

    #[tokio::test]
    async fn ordered_input_commands_keep_their_order_across_the_connect_backlog() {
        let request = |text: &str, key: &str, paste| InputRequest {
            session_id: 1,
            paste,
            text: text.to_owned(),
            key: key.to_owned(),
            shift: false,
            control: false,
            alt: false,
        };
        let mut backlog = VecDeque::new();
        for value in [request("echo ", "", false), request("粘贴", "", true)] {
            backlog.push_back(input_command_from_request(value).expect("连接中的输入"));
        }
        let (sender, mut commands) = unbounded_channel();
        for value in [request("", "enter", false), request("下一段", "", true)] {
            assert!(
                sender
                    .send(input_command_from_request(value).expect("已连接输入"))
                    .is_ok()
            );
        }
        assert!(matches!(next_command(&mut backlog, &mut commands).await,
            Some(SessionCommand::Input(TerminalInput::CommittedText(text))) if text == "echo "));
        assert!(matches!(next_command(&mut backlog, &mut commands).await,
            Some(SessionCommand::Paste(text)) if text == "粘贴"));
        assert!(matches!(
            next_command(&mut backlog, &mut commands).await,
            Some(SessionCommand::Input(TerminalInput::Key {
                code: KeyCode::Enter,
                ..
            }))
        ));
        assert!(matches!(next_command(&mut backlog, &mut commands).await,
            Some(SessionCommand::Paste(text)) if text == "下一段"));
    }

    #[test]
    fn paste_normalizes_newlines_to_carriage_returns() {
        assert_eq!(paste_bytes("a\nb\r\nc\rd", false), b"a\rb\rc\rd");
    }

    #[test]
    fn paste_is_bracketed_only_when_the_remote_asked() {
        assert_eq!(paste_bytes("ls\n", false), b"ls\r");
        assert_eq!(paste_bytes("ls\n", true), b"\x1b[200~ls\r\x1b[201~");
    }

    #[test]
    fn paste_drops_control_characters_that_could_escape_the_bracket() {
        // 内嵌的 ESC[201~ 会提前结束括号粘贴，后面的文本就被当成键入执行。
        assert_eq!(
            paste_bytes("safe\x1b[201~rm -rf ~\n", true),
            b"\x1b[200~safe[201~rm -rf ~\r\x1b[201~"
        );
        assert_eq!(paste_bytes("tab\there\u{7f}\u{9b}x", false), b"tab\therex");
        assert_eq!(paste_bytes("\x03", true), b"");
    }

    #[test]
    fn character_key_parses() {
        assert!(matches!(
            input("character:c", false),
            Some(TerminalInput::Key {
                code: KeyCode::Character('c'),
                ..
            })
        ));
        // 多字符不是合法键
        assert!(input("character:ab", false).is_none());
    }

    #[test]
    fn text_wins_and_carries_no_key() {
        let request = InputRequest {
            session_id: 1,
            paste: false,
            text: "你好".to_owned(),
            key: String::new(),
            shift: false,
            control: false,
            alt: false,
        };
        assert!(matches!(
            terminal_input_from_request(request),
            Some(TerminalInput::CommittedText(text)) if text == "你好"
        ));
    }

    #[test]
    fn modifiers_survive_the_boundary() {
        assert!(matches!(
            input("character:c", true),
            Some(TerminalInput::Key { code: KeyCode::Character('c'), modifiers })
                if modifiers.control && !modifiers.shift && !modifiers.alt
        ));
    }

    fn mouse(kind: &str, button: &str, col: u16, row: u16) -> MouseRequest {
        MouseRequest {
            session_id: 1,
            kind: kind.to_owned(),
            button: button.to_owned(),
            col,
            row,
            shift: false,
            control: false,
            alt: false,
        }
    }

    #[test]
    fn mouse_requests_parse() {
        let drag = mouse_event_from_request(mouse("move", "left", 3, 2)).expect("drag");
        assert_eq!(drag.kind, MouseEventKind::Move);
        assert_eq!(drag.button, Some(MouseButton::Left));
        assert_eq!((drag.cell.column, drag.viewport_row), (3, 2));

        let hover = mouse_event_from_request(mouse("move", "", 0, 0)).expect("hover");
        assert_eq!(hover.button, None);

        let control = mouse_event_from_request(MouseRequest {
            control: true,
            ..mouse("press", "right", 1, 1)
        })
        .expect("press");
        assert!(control.modifiers.control && !control.modifiers.shift);

        assert!(mouse_event_from_request(mouse("hover", "", 0, 0)).is_none());
    }

    #[test]
    fn mouse_moves_are_reported_once_per_cell() {
        let mut motion = MouseMotion::default();
        let mut admit = |kind, button, col, row| {
            motion.admit(&mouse_event_from_request(mouse(kind, button, col, row)).expect("event"))
        };

        assert!(admit("press", "left", 1, 1));
        assert!(!admit("move", "left", 1, 1), "same cell as the press");
        assert!(admit("move", "left", 2, 1));
        assert!(!admit("move", "left", 2, 1));
        assert!(admit("release", "left", 2, 1));
        assert!(
            !admit("move", "", 2, 1),
            "hover right where the button was released"
        );
        assert!(admit("move", "", 3, 1));
        assert!(
            admit("move", "right", 3, 1),
            "same cell, different button held"
        );
        assert!(admit("scroll", "wheel_up", 3, 1));
        assert!(admit("scroll", "wheel_up", 3, 1), "every wheel step counts");
    }

    /// 拖动与悬停按远端的鼠标模式编码：1000 只要点击，1002 加拖动，1003 加悬停。
    #[test]
    fn mouse_motion_is_encoded_per_tracking_mode() {
        let profile = TerminalSettingsV1::default().resolve(&TerminalOverrides::default());
        let size = TerminalSize {
            cols: 80,
            rows: 24,
            pixel_width: 0,
            pixel_height: 0,
            dpi: 0,
        };
        let mut engine = DefaultTerminalEngine::new(&profile, size).expect("engine");
        let encode = |engine: &mut DefaultTerminalEngine, kind, button| {
            let event = mouse_event_from_request(mouse(kind, button, 3, 2)).expect("event");
            engine.encode_mouse(event).ok()
        };

        engine
            .advance(b"\x1b[?1000h\x1b[?1006h")
            .expect("click tracking");
        assert_eq!(
            encode(&mut engine, "press", "left"),
            Some(b"\x1b[<0;4;3M".to_vec())
        );
        assert_eq!(encode(&mut engine, "move", "left"), None);

        engine.advance(b"\x1b[?1002h").expect("drag tracking");
        assert_eq!(
            encode(&mut engine, "move", "left"),
            Some(b"\x1b[<32;4;3M".to_vec())
        );
        assert_eq!(encode(&mut engine, "move", ""), None);

        engine.advance(b"\x1b[?1003h").expect("any-motion tracking");
        assert_eq!(
            encode(&mut engine, "move", ""),
            Some(b"\x1b[<35;4;3M".to_vec())
        );
        assert_eq!(
            encode(&mut engine, "release", "left"),
            Some(b"\x1b[<0;4;3m".to_vec())
        );
    }

    #[test]
    fn a_window_in_the_scrollback_renders_from_its_top() {
        let profile = TerminalSettingsV1::default().resolve(&TerminalOverrides::default());
        let size = TerminalSize {
            cols: 20,
            rows: 5,
            pixel_width: 0,
            pixel_height: 0,
            dpi: 0,
        };
        let mut engine = DefaultTerminalEngine::new(&profile, size).expect("engine");
        let output: String = (0..50).map(|i| format!("line {i}\r\n")).collect();
        engine.advance(output.as_bytes()).expect("output");
        let bounds = engine.viewport_bounds();
        let screen_top = bounds.clamp_top(i64::MAX);
        assert!(
            screen_top > bounds.first_stable_row,
            "output scrolled into history"
        );

        let bottom = engine
            .render(Window::Bottom.viewport(size.rows), None)
            .expect("bottom");
        assert_eq!(bottom.viewport_top, screen_top);
        assert_eq!(bottom.rows.len(), 5);

        let top = bounds.first_stable_row + 3;
        let window = engine
            .render(Window::At { top, rows: 4 }.viewport(size.rows), None)
            .expect("window");
        assert_eq!(window.viewport_top, top);
        assert_eq!(window.rows.len(), 4);
        let text: String = window.rows[0]
            .cells
            .iter()
            .map(|cell| cell.text.as_str())
            .collect();
        assert_eq!(text.trim_end(), "line 3");

        // 窗口越过屏幕底部时只渲染到最后一行。
        let tail = engine
            .render(
                Window::At {
                    top: screen_top,
                    rows: 20,
                }
                .viewport(size.rows),
                None,
            )
            .expect("tail");
        assert_eq!(tail.rows.len(), 5);
    }

    #[test]
    fn network_failures_after_connecting_mean_the_connection_was_lost() {
        assert_eq!(lost(SessionFailure::Network), FailureKind::ConnectionLost);
        assert_eq!(lost(SessionFailure::Timeout), FailureKind::ConnectionLost);
        assert_eq!(
            lost(SessionFailure::Authentication),
            FailureKind::Authentication
        );
    }
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod lifecycle_tests;
