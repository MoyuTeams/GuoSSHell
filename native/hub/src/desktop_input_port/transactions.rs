use super::{
    APP_USAGE, Arc, Binding, DesktopAction, DesktopEvent, EVENT_BYTES, EffectKind, EffectLease,
    INPUT_BYTES, InputPort, Mutex, Ordering, Reject, Source, Surface, Ticket, Weak, enqueue,
    refund, reserve, source_control, valid, valid_attempt, valid_binding,
};
use crate::signals::desktop_native_input::NativeCommand;
use rinf::RustSignal;

pub(super) enum TextState {
    Waiting,
    Preedit(BudgetedText),
    Committed(BudgetedText),
    Ending(super::TextEnd),
    Consumed,
    Cancelled,
}

pub(crate) struct BudgetedText {
    pub text: String,
    _budget: ByteLease,
}

pub(crate) struct TextPayload {
    bytes: Vec<u8>,
    expected: usize,
    budget: ByteLease,
}
impl TextPayload {
    pub(crate) fn append(&mut self, offset: usize, bytes: &[u8]) -> Result<(), Reject> {
        if offset != self.bytes.len() || bytes.len() > self.expected.saturating_sub(offset) {
            return Err(Reject::InvalidEvent);
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }
    fn ready(&self) -> Result<(), Reject> {
        if self.bytes.len() != self.expected {
            return Err(Reject::InvalidEvent);
        }
        std::str::from_utf8(&self.bytes).map_err(|_| Reject::InvalidEvent)?;
        Ok(())
    }
}

pub(crate) struct ByteLease {
    port: Weak<InputPort>,
    bytes: usize,
}
impl Drop for ByteLease {
    fn drop(&mut self) {
        if let Some(port) = self.port.upgrade() {
            port.state.lock().unwrap_or_else(|e| e.into_inner()).bytes -= self.bytes;
        }
        APP_USAGE.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

pub(crate) struct ActionValue {
    pub action: DesktopAction,
    pub copy_text: Option<BudgetedText>,
    pub rejection: Option<Reject>,
}

pub(super) struct PendingAction {
    pub seq: u64,
    pub ticket: Ticket,
    pub claimed: bool,
    value: Arc<ActionValue>,
    reply: Arc<Mutex<ActionMailbox>>,
    lease: Weak<EffectLease>,
}

struct ActionMailbox {
    reply: Option<ActionCompletion>,
    finished: bool,
}

impl Drop for PendingAction {
    fn drop(&mut self) {
        if !self.claimed {
            let mut mailbox = self.reply.lock().unwrap_or_else(|e| e.into_inner());
            if !mailbox.finished {
                mailbox.reply = Some(ActionCompletion::Cancelled);
                mailbox.finished = true;
            }
        }
    }
}

struct ActionWait {
    port: Arc<InputPort>,
    seq: u64,
    ticket: Ticket,
}
impl Drop for ActionWait {
    fn drop(&mut self) {
        let old = {
            let mut state = self.port.state.lock().unwrap_or_else(|e| e.into_inner());
            if state
                .action
                .as_ref()
                .is_some_and(|action| action.seq == self.seq && action.ticket == self.ticket)
            {
                state.action.take()
            } else {
                None
            }
        };
        drop(old);
        self.port.wake.notify_waiters();
    }
}

pub(crate) struct ActionClaim {
    port: Arc<InputPort>,
    pub source: Source,
    pub seq: u64,
    pub ticket: Ticket,
    pub value: Arc<ActionValue>,
    reply: Arc<Mutex<ActionMailbox>>,
    _lease: Option<Arc<EffectLease>>,
}

pub(crate) enum ActionReply {
    Completed,
    Cancelled,
    Paste(String),
}
pub(crate) enum ActionCompletion {
    Completed,
    Cancelled,
    Paste(BudgetedText),
}

impl InputPort {
    pub(crate) fn reserve_text_payload(
        self: &Arc<Self>,
        bytes: usize,
    ) -> Result<TextPayload, Reject> {
        let retained = bytes
            .checked_mul(3)
            .and_then(|n| n.checked_add(EVENT_BYTES))
            .ok_or(Reject::TooLarge)?;
        let mut state = self.state.try_lock().map_err(|_| Reject::Busy)?;
        if retained > INPUT_BYTES {
            return Err(Reject::TooLarge);
        }
        if state.bytes.saturating_add(retained) > INPUT_BYTES {
            return Err(Reject::Busy);
        }
        reserve(&mut state, retained, false)?;
        drop(state);
        Ok(TextPayload {
            bytes: Vec::with_capacity(bytes),
            expected: bytes,
            budget: ByteLease {
                port: Arc::downgrade(self),
                bytes: retained,
            },
        })
    }

    pub(crate) fn text_open(&self, ticket: Ticket) -> Result<(), Reject> {
        let state = self.state.try_lock().map_err(|_| Reject::Busy)?;
        valid(&state, ticket.binding)?;
        if ticket.registration != self.registration || state.active_text != Some(ticket) {
            return Err(Reject::InvalidTicket);
        }
        Ok(())
    }
    pub(super) fn seal_text(&self, state: &mut super::State) -> Option<TextState> {
        let ticket = state.active_text.take()?;
        let text = state
            .texts
            .iter_mut()
            .flatten()
            .find(|text| text.ticket == ticket)?;
        state.next_fence += 1;
        let end = super::TextEnd {
            binding: ticket.binding,
            context_id: ticket.context,
            transaction_id: ticket.transaction,
            end_fence: state.next_fence,
        };
        let old = std::mem::replace(&mut text.text, TextState::Ending(end));
        crate::signals::desktop_native_input::DesktopNativeTextEndRequired {
            version: crate::signals::desktop_native_input::VERSION,
            source: self.source.source.clone(),
            identity: self.source.identity.clone(),
            attempt: end.binding.attempt,
            focus_epoch: end.binding.epoch,
            context_id: end.context_id,
            transaction_id: end.transaction_id,
            end_fence: end.end_fence,
        }
        .send_signal_to_dart();
        Some(old)
    }

    pub(crate) fn text_end(&self, binding: Binding) -> Result<Option<super::TextEnd>, Reject> {
        let state = self.state.try_lock().map_err(|_| Reject::Busy)?;
        Ok(state
            .texts
            .iter()
            .flatten()
            .find_map(|text| match text.text {
                TextState::Ending(end) if end.binding == binding => Some(end),
                _ => None,
            }))
    }
    /// selected_text 的稳定接口返回完整 String；先预约本端剩余额度，取文后按实际 UTF-8
    /// 收缩租约。不宣称约束第三方函数内部的临时分配。
    pub(crate) fn capture_copy(
        self: &Arc<Self>,
        capture: impl FnOnce() -> Result<String, rshell_m0::rshell_session::EngineError>,
    ) -> Result<BudgetedText, Reject> {
        let mut state = self.state.try_lock().map_err(|_| Reject::Busy)?;
        let bytes = INPUT_BYTES.saturating_sub(state.bytes);
        if bytes < EVENT_BYTES {
            return Err(Reject::Busy);
        }
        reserve(&mut state, bytes, false)?;
        drop(state);
        let mut budget = ByteLease {
            port: Arc::downgrade(self),
            bytes,
        };
        let text = capture().map_err(|_| Reject::InvalidEvent)?;
        let retained = text.len().saturating_mul(3).saturating_add(EVENT_BYTES);
        if retained > bytes {
            return Err(Reject::TooLarge);
        }
        let released = bytes - retained;
        self.state.lock().unwrap_or_else(|e| e.into_inner()).bytes -= released;
        APP_USAGE.fetch_sub(released, Ordering::AcqRel);
        budget.bytes = retained;
        Ok(BudgetedText {
            text,
            _budget: budget,
        })
    }

    pub(crate) fn budget_text(self: &Arc<Self>, text: String) -> Result<BudgetedText, Reject> {
        let bytes = text
            .len()
            .checked_mul(3)
            .and_then(|bytes| bytes.checked_add(EVENT_BYTES))
            .ok_or(Reject::TooLarge)?;
        let mut state = self.state.try_lock().map_err(|_| Reject::Busy)?;
        if bytes > INPUT_BYTES {
            return Err(Reject::TooLarge);
        }
        if state.bytes.saturating_add(bytes) > INPUT_BYTES {
            return Err(Reject::Busy);
        }
        reserve(&mut state, bytes, false)?;
        drop(state);
        Ok(BudgetedText {
            text,
            _budget: ByteLease {
                port: Arc::downgrade(self),
                bytes,
            },
        })
    }

    pub(crate) async fn text(self: &Arc<Self>, ticket: Ticket) -> Result<BudgetedText, Reject> {
        if ticket.registration != self.registration {
            return Err(Reject::InvalidTicket);
        }
        loop {
            let notified = self.wake.notified();
            let text = {
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                valid_attempt(&state, ticket.binding.attempt)?;
                let text = state
                    .texts
                    .iter_mut()
                    .flatten()
                    .find(|text| text.ticket == ticket)
                    .ok_or(Reject::InvalidTicket)?;
                match &text.text {
                    TextState::Committed(_) => {
                        Some(std::mem::replace(&mut text.text, TextState::Consumed))
                    }
                    TextState::Waiting | TextState::Preedit(_) | TextState::Ending(_) => None,
                    _ => return Err(Reject::Cancelled),
                }
            };
            if let Some(TextState::Committed(text)) = text {
                return Ok(text);
            }
            notified.await;
        }
    }

    /// 队头动作的完成邮箱独立于数据队列，ACK 不排在等待自己的动作之后。
    pub(crate) async fn action(
        self: &Arc<Self>,
        seq: u64,
        ticket: Ticket,
        value: ActionValue,
        lease: &Arc<EffectLease>,
    ) -> ActionCompletion {
        if ticket.registration != self.registration {
            return ActionCompletion::Cancelled;
        }
        let value = Arc::new(value);
        let reply = Arc::new(Mutex::new(ActionMailbox {
            reply: None,
            finished: false,
        }));
        let old = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if valid_binding(&state, ticket.binding).is_err() {
                return ActionCompletion::Cancelled;
            }
            state.action.replace(PendingAction {
                seq,
                ticket,
                value,
                claimed: false,
                reply: reply.clone(),
                lease: Arc::downgrade(lease),
            })
        };
        drop(old);
        let _wait = ActionWait {
            port: self.clone(),
            seq,
            ticket,
        };
        crate::signals::desktop_native_input::DesktopNativeActionReady {
            version: crate::signals::desktop_native_input::VERSION,
            source: self.source.source.clone(),
            identity: self.source.identity.clone(),
            attempt: ticket.binding.attempt,
            focus_epoch: ticket.binding.epoch,
            effect: seq,
        }
        .send_signal_to_dart();
        loop {
            let notified = self.wake.notified();
            let ready = reply.lock().unwrap_or_else(|e| e.into_inner()).reply.take();
            if let Some(reply) = ready {
                return reply;
            }
            notified.await;
        }
    }

    fn claim_action(
        self: &Arc<Self>,
        binding: Binding,
        effect: u64,
    ) -> Result<ActionClaim, Reject> {
        let mut state = self.state.try_lock().map_err(|_| Reject::Busy)?;
        valid_binding(&state, binding)?;
        let action = state.action.as_mut().ok_or(Reject::InvalidTicket)?;
        if action.seq != effect {
            return Err(Reject::InvalidTicket);
        }
        if action.claimed {
            return Err(Reject::Duplicate);
        }
        action.claimed = true;
        Ok(ActionClaim {
            port: self.clone(),
            source: self.source.clone(),
            seq: action.seq,
            ticket: action.ticket,
            value: action.value.clone(),
            reply: action.reply.clone(),
            _lease: action.lease.upgrade(),
        })
    }

    /// 非键命令共享此端口分配的 effect 序；request_id 仅负责去重。
    pub(crate) fn command(
        self: &Arc<Self>,
        binding: Binding,
        request_id: u64,
        command: NativeCommand,
    ) -> Result<super::CommandAdmission, Reject> {
        let (kind, bytes, consumed_latches) = match command {
            NativeCommand::GuiIntent(intent) => {
                let gui = super::gui::prepare(intent)?;
                (
                    EffectKind::GuiIntent(gui.input),
                    gui.bytes,
                    gui.consumed_latches,
                )
            }
            NativeCommand::Event(
                DesktopEvent::Key { .. } | DesktopEvent::Text { .. } | DesktopEvent::Mouse { .. },
            ) => return Err(Reject::InvalidEvent),
            NativeCommand::Event(DesktopEvent::Copy) => {
                (EffectKind::Action(DesktopAction::Copy), EVENT_BYTES, 0)
            }
            NativeCommand::Event(event) => {
                let payload = match &event {
                    DesktopEvent::Paste { text } => text.len(),
                    _ => 0,
                };
                let bytes = payload.saturating_mul(3).saturating_add(EVENT_BYTES);
                (EffectKind::Command(event), bytes, 0)
            }
            NativeCommand::Action(action) => (EffectKind::Action(action), EVENT_BYTES, 0),
        };
        let control = matches!(
            kind,
            EffectKind::Command(
                DesktopEvent::FrameAck { .. } | DesktopEvent::Disconnect | DesktopEvent::Reconnect
            )
        );
        let disconnect = matches!(kind, EffectKind::Command(DesktopEvent::Disconnect));
        let mut state = self.state.try_lock().map_err(|_| Reject::Busy)?;
        if matches!(&kind, EffectKind::Command(event) if source_control(event)) {
            valid_attempt(&state, binding.attempt)?;
        } else {
            valid_binding(&state, binding)?;
        }
        if let EffectKind::Command(DesktopEvent::Resize { geometry }) = &kind
            && crate::desktop_prepare::geometry(*geometry).is_err()
        {
            return Err(Reject::InvalidEvent);
        }
        if !state.authenticated
            && matches!(
                kind,
                EffectKind::Command(DesktopEvent::Paste { .. })
                    | EffectKind::Action(DesktopAction::Paste)
                    | EffectKind::GuiIntent(_)
            )
        {
            return Err(Reject::NotAuthenticated);
        }
        if request_id <= state.last_request {
            return Err(Reject::Duplicate);
        }
        if matches!(kind, EffectKind::GuiIntent(_)) && state.disconnect_queued {
            return Err(Reject::NotAttached);
        }
        if matches!(kind, EffectKind::GuiIntent(_)) && state.busy_pointer.is_some() {
            return Err(Reject::Busy);
        }
        if disconnect && state.disconnect_queued {
            return Err(Reject::Duplicate);
        }
        if control && !disconnect && state.control_count != 0 {
            return Err(Reject::Busy);
        }
        let bytes = if control { 0 } else { bytes };
        reserve(&mut state, bytes, !control)?;
        if !self.mode.native() {
            refund(&mut state, bytes, !control);
            return Err(Reject::MixedMode);
        }
        state.control_count += usize::from(control);
        state.disconnect_queued |= disconnect;
        state.last_request = request_id;
        let ticket = Ticket {
            registration: self.registration,
            binding,
            cycle: 0,
            transaction: 0,
            context: 0,
        };
        let ended = if !control {
            self.seal_text(&mut state)
        } else {
            None
        };
        let seq = enqueue(self, &mut state, ticket, kind, bytes, !control);
        drop(state);
        drop(ended);
        self.wake.notify_waiters();
        Ok(super::CommandAdmission {
            effect: seq,
            consumed_latches,
        })
    }
}

impl Surface {
    /// 回执仅结算原 fence，不恢复输入权；宿主必须已经证明原 callbacks/COM 静默。
    pub(crate) fn text_end_ack(&self, end: super::TextEnd) -> Result<(), Reject> {
        self.on_ui_thread()?;
        if end.binding != self.binding {
            return Err(Reject::InvalidTicket);
        }
        let mut state = self.port.state.try_lock().map_err(|_| Reject::Busy)?;
        let text = state
            .texts
            .iter_mut()
            .flatten()
            .find(|text| {
                text.ticket.context == end.context_id
                    && text.ticket.transaction == end.transaction_id
                    && text.ticket.binding == end.binding
            })
            .ok_or(Reject::InvalidTicket)?;
        if !matches!(text.text, TextState::Ending(actual) if actual == end) {
            return Err(Reject::InvalidTicket);
        }
        text.text = TextState::Cancelled;
        drop(state);
        self.port.wake.notify_waiters();
        Ok(())
    }
    pub(crate) fn claim_action(&self, effect: u64) -> Result<ActionClaim, Reject> {
        self.on_ui_thread()?;
        self.port.claim_action(self.binding, effect)
    }

    /// preedit/commit/cancel 只可填充本次 NativeText 队头事务，不能凭当前 pane 重签票据。
    pub(crate) fn composition(
        &self,
        ticket: Ticket,
        text: Option<String>,
        commit: bool,
    ) -> Result<(), Reject> {
        self.on_ui_thread()?;
        if ticket.registration != self.port.registration || ticket.binding != self.binding {
            return Err(Reject::InvalidTicket);
        }
        let next = match text {
            Some(text) => {
                let text = self.port.budget_text(text)?;
                if commit {
                    TextState::Committed(text)
                } else {
                    TextState::Preedit(text)
                }
            }
            None => TextState::Cancelled,
        };
        self.set_text(ticket, next)
    }

    pub(crate) fn finish_payload(
        &self,
        ticket: Ticket,
        payload: &mut Option<(u64, TextPayload)>,
    ) -> Result<(), Reject> {
        self.on_ui_thread()?;
        if ticket.registration != self.port.registration || ticket.binding != self.binding {
            return Err(Reject::InvalidTicket);
        }
        let (_, pending) = payload
            .as_ref()
            .filter(|(id, _)| *id == ticket.transaction)
            .ok_or(Reject::InvalidTicket)?;
        pending.ready()?;
        let mut state = self.port.state.try_lock().map_err(|_| Reject::Busy)?;
        valid(&state, self.binding)?;
        let text = state
            .texts
            .iter_mut()
            .flatten()
            .find(|text| text.ticket == ticket)
            .ok_or(Reject::InvalidTicket)?;
        if !matches!(text.text, TextState::Waiting | TextState::Preedit(_)) {
            return Err(Reject::InvalidTicket);
        }
        let (_, payload) = payload.take().ok_or(Reject::InvalidTicket)?;
        // 同一个排他借用已验证完整 UTF-8；取出到提交之间不再修改 bytes。
        let committed = BudgetedText {
            text: unsafe { String::from_utf8_unchecked(payload.bytes) },
            _budget: payload.budget,
        };
        let old = std::mem::replace(&mut text.text, TextState::Committed(committed));
        state.active_text = None;
        drop(state);
        drop(old);
        self.port.wake.notify_waiters();
        Ok(())
    }

    fn set_text(&self, ticket: Ticket, next: TextState) -> Result<(), Reject> {
        let old = {
            let mut state = self.port.state.try_lock().map_err(|_| Reject::Busy)?;
            valid(&state, self.binding)?;
            let text = state
                .texts
                .iter_mut()
                .flatten()
                .find(|text| text.ticket == ticket)
                .ok_or(Reject::InvalidTicket)?;
            if !matches!(text.text, TextState::Waiting | TextState::Preedit(_)) {
                return Err(Reject::InvalidTicket);
            }
            let sealed = matches!(next, TextState::Committed(_) | TextState::Cancelled);
            let old = std::mem::replace(&mut text.text, next);
            if sealed && state.active_text == Some(ticket) {
                state.active_text = None;
            }
            old
        };
        drop(old);
        self.port.wake.notify_waiters();
        Ok(())
    }
}

impl ActionClaim {
    /// Completed 必须由原目标 GUI 消费者实际完成后报告；提交另一个旧信号不算完成。
    pub(crate) fn ack(&self, reply: ActionReply) -> Result<(), Reject> {
        {
            let mailbox = self.reply.try_lock().map_err(|_| Reject::Busy)?;
            if mailbox.finished {
                return Err(Reject::Duplicate);
            }
        }
        let reply = match reply {
            ActionReply::Completed
                if self.value.action != DesktopAction::Paste && self.value.rejection.is_none() =>
            {
                ActionCompletion::Completed
            }
            ActionReply::Completed => return Err(Reject::InvalidEvent),
            ActionReply::Cancelled => ActionCompletion::Cancelled,
            ActionReply::Paste(text) if self.value.action == DesktopAction::Paste => {
                let state = self.port.state.try_lock().map_err(|_| Reject::Busy)?;
                valid(&state, self.ticket.binding)?;
                drop(state);
                ActionCompletion::Paste(self.port.budget_text(text)?)
            }
            ActionReply::Paste(_) => return Err(Reject::InvalidEvent),
        };
        let state = self.port.state.try_lock().map_err(|_| Reject::Busy)?;
        if matches!(reply, ActionCompletion::Paste(_)) {
            valid(&state, self.ticket.binding)?;
        }
        let mut mailbox = self.reply.try_lock().map_err(|_| Reject::Busy)?;
        if mailbox.finished {
            return Err(Reject::Duplicate);
        }
        mailbox.reply = Some(reply);
        mailbox.finished = true;
        drop(mailbox);
        drop(state);
        self.port.wake.notify_waiters();
        Ok(())
    }
}

impl Drop for ActionClaim {
    fn drop(&mut self) {
        let mut mailbox = self.reply.lock().unwrap_or_else(|e| e.into_inner());
        if !mailbox.finished {
            mailbox.reply = Some(ActionCompletion::Cancelled);
            mailbox.finished = true;
        }
        drop(mailbox);
        self.port.wake.notify_waiters();
    }
}
