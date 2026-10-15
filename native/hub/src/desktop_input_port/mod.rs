//! 正常会话的同步准入端。这里没有引擎、transport、认证材料或独立 runtime。
//!
//! 宿主接入必须持有真实回调寿命；仅注册 Prepared 来源不会签发原生事件身份。
#![allow(dead_code)] // 跨平台宿主在同一 crate 的受控接缝，未连接的平台返回 NotAttached。

use std::collections::VecDeque;
use std::num::NonZeroU64;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use rshell_m0::rshell_core::{
    RenderFrame, ResolvedTerminalProfile, SelectionRange, TerminalDisplayModes,
};
use rshell_m0::rshell_session::ViewportBounds;
use tokio::sync::Notify;

use crate::desktop_input_policy::{
    self as policy, GatedIntent, InputPolicyState, InputReject, KeyResult, MAX_ACTIVE_KEYS,
    NoSendReason, PolicyKeyInput,
};
use crate::desktop_runtime::{INPUT_BYTES, INPUT_EVENTS, Source};
use crate::signals::desktop_native_input::{
    DesktopNativeLifecycle, DesktopNativeLifecycleQuery, DesktopNativeLifecycleQueryResult,
    NativeLifecycleQueryFailure, NativeLifecycleState,
};
use crate::signals::desktop_session::{
    DesktopAction, DesktopEvent, DesktopKeyPhase, DesktopSource,
};
use rinf::RustSignal;

mod gui;
mod pointer;
pub(crate) mod transactions;
pub(crate) use pointer::{FrozenPointer, PointerInput, PresentedGeometry};
pub(crate) use transactions::ActionValue;

const APP_BYTES: usize = 16 * INPUT_BYTES;
const EVENT_BYTES: usize = 512;
static APP_USAGE: AtomicUsize = AtomicUsize::new(0);
static REGISTRATION_IDS: AtomicU64 = AtomicU64::new(1);
static REGISTRY: OnceLock<Mutex<Vec<RegistryEntry>>> = OnceLock::new();
static ENDED_REGISTRY: OnceLock<Mutex<VecDeque<EndedLifecycle>>> = OnceLock::new();
/// 已结束来源只保留有界只读结局，不保留 port、输入身份、引擎或 transport。
const ENDED_LIFECYCLES: usize = 64;
tokio::task_local! { pub(crate) static CURRENT: Arc<InputPort>; }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reject {
    UnsupportedVersion,
    NotAttached,
    StaleSource,
    StaleAttempt,
    StaleFocus,
    NotAuthenticated,
    WrongThread,
    Retired,
    Busy,
    TooLarge,
    MixedMode,
    Duplicate,
    InvalidEvent,
    InvalidTicket,
    Cancelled,
    Policy(InputReject),
}

#[derive(Default)]
pub(crate) struct InputMode(AtomicU8);
impl InputMode {
    pub(crate) fn legacy(&self) -> bool {
        self.choose(1)
    }
    fn native(&self) -> bool {
        self.choose(2)
    }
    fn choose(&self, value: u8) -> bool {
        self.0
            .compare_exchange(0, value, Ordering::AcqRel, Ordering::Acquire)
            .unwrap_or_else(|old| old)
            == 0
            || self.0.load(Ordering::Acquire) == value
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Binding {
    pub attempt: u64,
    pub epoch: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Ticket {
    registration: u64,
    binding: Binding,
    cycle: u64,
    transaction: u64,
    context: u64,
}
impl Ticket {
    pub(crate) fn binding_attempt(self) -> u64 {
        self.binding.attempt
    }
    pub(crate) fn binding_epoch(self) -> u64 {
        self.binding.epoch
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Route {
    Terminal,
    NativeText,
    Consumed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Admission {
    pub ticket: Ticket,
    pub effect: u64,
    pub route: Route,
    pub cycle_id: u64,
    pub text_op: TextOp,
    pub end: Option<TextEnd>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub(crate) enum TextOp {
    None = 0,
    Start = 1,
    Continue = 2,
    End = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TextEnd {
    pub binding: Binding,
    pub context_id: u64,
    pub transaction_id: u64,
    pub end_fence: u64,
}

struct Cycle {
    id: u64,
    binding: Binding,
    physical: NonZeroU64,
    up: bool,
    pending: usize,
    rejected: bool,
    route: Route,
    native_text: Option<Ticket>,
}

struct TextTransaction {
    ticket: Ticket,
    text: transactions::TextState,
}

pub(crate) enum EffectKind {
    Key(GatedIntent),
    Text,
    Command(DesktopEvent),
    Action(DesktopAction),
    Pointer(FrozenPointer),
    GuiIntent(rshell_m0::rshell_core::TerminalInput),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CommandAdmission {
    pub effect: u64,
    pub consumed_latches: u8,
}

pub(crate) struct Effect {
    pub seq: u64,
    pub ticket: Ticket,
    pub kind: EffectKind,
    pub(crate) lease: Arc<EffectLease>,
}

pub(crate) struct EffectLease {
    port: Arc<InputPort>,
    ticket: Ticket,
    bytes: usize,
    data: bool,
    disconnect: bool,
}
impl Drop for EffectLease {
    fn drop(&mut self) {
        let mut state = self.port.state.lock().unwrap_or_else(|e| e.into_inner());
        state.bytes -= self.bytes;
        state.data_count -= usize::from(self.data);
        if !self.data && self.ticket.cycle == 0 {
            state.control_count -= 1;
        }
        if self.disconnect {
            state.disconnect_queued = false;
        }
        let retired_cycle = if let Some(slot) = cycle_slot(&state, self.ticket.cycle)
            && let Some(cycle) = state.cycles[slot].as_mut()
        {
            cycle.pending -= 1;
            if cycle.up && cycle.pending == 0 {
                state.cycles[slot].take()
            } else {
                None
            }
        } else {
            None
        };
        let retired_text = state
            .texts
            .iter_mut()
            .find(|slot| slot.as_ref().is_some_and(|text| text.ticket == self.ticket))
            .and_then(Option::take);
        drop(state);
        drop((retired_cycle, retired_text));
        APP_USAGE.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

/// 与已发送帧共享不可变 RenderFrame；查询不会重新 render，也不克隆网格。
pub(crate) struct Projection {
    pub source: Source,
    pub frame_revision: u32,
    pub applied_effect: u64,
    pub row_revision: u64,
    frame: Arc<RenderFrame>,
    pub bounds: ViewportBounds,
    pub follow_bottom: bool,
    pub selection: Option<SelectionRange>,
    pub modes: TerminalDisplayModes,
    pub mouse_reporting: bool,
}
impl Projection {
    pub(crate) fn frame(&self) -> &RenderFrame {
        &self.frame
    }
}

pub(crate) struct PublishedProjection {
    value: Projection,
    bytes: usize,
}
impl std::ops::Deref for PublishedProjection {
    type Target = Projection;
    fn deref(&self) -> &Projection {
        &self.value
    }
}
impl Drop for PublishedProjection {
    fn drop(&mut self) {
        APP_USAGE.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

struct State {
    binding: Binding,
    lifecycle_revision: u64,
    lifecycle_state: NativeLifecycleState,
    lifecycle_budget: Option<PortBudget>,
    authenticated: bool,
    attached: bool,
    host_surface: Weak<Surface>,
    retired: bool,
    policy: InputPolicyState,
    cycles: [Option<Cycle>; MAX_ACTIVE_KEYS],
    texts: [Option<TextTransaction>; INPUT_EVENTS],
    active_text: Option<Ticket>,
    last_context: u64,
    next_fence: u64,
    next_cycle: u64,
    next_effect: u64,
    applied: u64,
    control_applied: u64,
    row_revision: u64,
    queue: VecDeque<Effect>,
    data_count: usize,
    control_count: usize,
    disconnect_queued: bool,
    bytes: usize,
    last_request: u64,
    last_native_request: u64,
    busy_pointer: Option<u64>,
    presented: Option<(PresentedGeometry, Arc<PublishedProjection>)>,
    action: Option<transactions::PendingAction>,
    projection: Option<Arc<PublishedProjection>>,
    projection_failure: Option<Reject>,
}

pub(crate) struct InputPort {
    pub source: Source,
    pub profile: Arc<ResolvedTerminalProfile>,
    pub mode: Arc<InputMode>,
    registration: u64,
    state: Mutex<State>,
    wake: Notify,
    #[cfg(test)]
    lifecycle_observer: Mutex<Option<tokio::sync::mpsc::UnboundedSender<DesktopNativeLifecycle>>>,
    _budget: PortBudget,
}

struct RegistryEntry {
    source: DesktopSource,
    identity: String,
    port: Weak<InputPort>,
}

struct EndedLifecycle {
    source: DesktopSource,
    identity: String,
    record: LifecycleRecord,
    _budget: PortBudget,
}

#[derive(Clone, Copy)]
struct LifecycleRecord {
    binding: Binding,
    revision: u64,
    state: NativeLifecycleState,
}
impl LifecycleRecord {
    fn projection(self, source: DesktopSource, identity: String) -> DesktopNativeLifecycle {
        DesktopNativeLifecycle {
            version: crate::signals::desktop_native_input::VERSION,
            source,
            identity,
            attempt: self.binding.attempt,
            focus_epoch: self.binding.epoch,
            lifecycle_revision: self.revision,
            state: self.state,
        }
    }
}

pub(crate) fn query_lifecycle(
    request: DesktopNativeLifecycleQuery,
) -> DesktopNativeLifecycleQueryResult {
    let mut result = DesktopNativeLifecycleQueryResult {
        version: crate::signals::desktop_native_input::VERSION,
        source: request.source,
        identity: request.identity,
        request_id: request.request_id,
        lifecycle: None,
        failure: NativeLifecycleQueryFailure::None,
    };
    let projection = || -> Result<DesktopNativeLifecycle, NativeLifecycleQueryFailure> {
        if request.version != crate::signals::desktop_native_input::VERSION {
            return Err(NativeLifecycleQueryFailure::UnsupportedVersion);
        }
        let port = match lookup(&result.source, &result.identity) {
            Ok(port) => Some(port),
            Err(Reject::Busy) => return Err(NativeLifecycleQueryFailure::Busy),
            Err(_) => None,
        };
        if let Some(port) = port {
            let state = port
                .state
                .try_lock()
                .map_err(|_| NativeLifecycleQueryFailure::Busy)?;
            Ok(port
                .lifecycle_record(&state)
                .projection(result.source.clone(), result.identity.clone()))
        } else {
            let ended = ENDED_REGISTRY
                .get_or_init(Mutex::default)
                .try_lock()
                .map_err(|_| NativeLifecycleQueryFailure::Busy)?;
            let entry = ended
                .iter()
                .find(|entry| entry.source == result.source && entry.identity == result.identity)
                .ok_or(NativeLifecycleQueryFailure::StaleSource)?;
            Ok(entry
                .record
                .projection(result.source.clone(), result.identity.clone()))
        }
    };
    match projection() {
        Ok(value) => result.lifecycle = Some(value),
        Err(failure) => result.failure = failure,
    }
    result
}
pub(crate) struct PortBudget(usize);
impl Drop for PortBudget {
    fn drop(&mut self) {
        APP_USAGE.fetch_sub(self.0, Ordering::AcqRel);
    }
}

pub(crate) fn reserve_output(bytes: usize) -> Option<PortBudget> {
    APP_USAGE
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
            used.checked_add(bytes).filter(|next| *next <= APP_BYTES)
        })
        .ok()
        .map(|_| PortBudget(bytes))
}

/// supervisor 的真实运行登记。任务退出先撤权；既有 callback/claim 仍持原端口。
pub(crate) struct Registration(pub Arc<InputPort>);
impl Drop for Registration {
    fn drop(&mut self) {
        self.0.retire_owner();
    }
}

pub(crate) fn lookup(source: &DesktopSource, identity: &str) -> Result<Arc<InputPort>, Reject> {
    let registry = REGISTRY
        .get_or_init(Mutex::default)
        .try_lock()
        .map_err(|_| Reject::Busy)?;
    let entry = registry
        .iter()
        .find(|entry| entry.source == *source && entry.identity == identity)
        .map(|entry| entry.port.clone());
    drop(registry);
    entry
        .and_then(|entry| entry.upgrade())
        .ok_or(Reject::StaleSource)
}

impl InputPort {
    pub(crate) fn register(
        source: Source,
        profile: Arc<ResolvedTerminalProfile>,
        mode: Arc<InputMode>,
    ) -> Result<Registration, Reject> {
        // 此 nonce 仅区分真实 Source 的运行登记，不签发 OS 来源证明，也不依赖可复用地址。
        let registration = REGISTRATION_IDS
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                value.checked_add(1)
            })
            .map_err(|_| Reject::Retired)?;
        let bytes = std::mem::size_of::<InputPort>()
            + 2 * (INPUT_EVENTS + MAX_ACTIVE_KEYS + 2) * std::mem::size_of::<Effect>()
            + (MAX_ACTIVE_KEYS + 2)
                * (std::mem::size_of::<EffectLease>() + 2 * std::mem::size_of::<usize>())
            + MAX_ACTIVE_KEYS * std::mem::size_of::<transactions::TextState>()
            + std::mem::size_of::<ResolvedTerminalProfile>()
            + profile.answerback.len()
            + profile.terminal_type.len()
            + profile.font_family.len()
            + profile
                .key_bindings
                .iter()
                .map(|binding| std::mem::size_of_val(binding) + binding.action.len())
                .sum::<usize>()
            + source.identity.len() * 2;
        APP_USAGE
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes).filter(|next| *next <= APP_BYTES)
            })
            .map_err(|_| Reject::Busy)?;
        let budget = PortBudget(bytes);
        let lifecycle_budget =
            reserve_output(std::mem::size_of::<EndedLifecycle>() + source.identity.len())
                .ok_or(Reject::Busy)?;
        let port = Arc::new(Self {
            source,
            profile,
            mode,
            registration,
            state: Mutex::new(State {
                binding: Binding {
                    attempt: 0,
                    epoch: 0,
                },
                lifecycle_revision: 0,
                lifecycle_state: NativeLifecycleState::Registered,
                lifecycle_budget: Some(lifecycle_budget),
                authenticated: false,
                attached: false,
                host_surface: Weak::new(),
                retired: false,
                policy: InputPolicyState::default(),
                cycles: std::array::from_fn(|_| None),
                texts: std::array::from_fn(|_| None),
                active_text: None,
                last_context: 0,
                next_fence: 0,
                next_cycle: 0,
                next_effect: 0,
                applied: 0,
                control_applied: 0,
                row_revision: 0,
                queue: VecDeque::with_capacity(INPUT_EVENTS + MAX_ACTIVE_KEYS + 2),
                data_count: 0,
                control_count: 0,
                disconnect_queued: false,
                bytes: 0,
                last_request: 0,
                last_native_request: 0,
                busy_pointer: None,
                presented: None,
                action: None,
                projection: None,
                projection_failure: None,
            }),
            wake: Notify::new(),
            #[cfg(test)]
            lifecycle_observer: Mutex::new(None),
            _budget: budget,
        });
        #[cfg(test)]
        let _isolation = tests::isolate_source_registration();
        let mut registry = REGISTRY
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        registry.retain(|entry| entry.port.strong_count() != 0);
        registry.push(RegistryEntry {
            source: port.source.source.clone(),
            identity: port.source.identity.clone(),
            port: Arc::downgrade(&port),
        });
        drop(registry);
        Ok(Registration(port))
    }

    /// 绑定只能取得已由真实宿主登记的同一 surface；数字身份本身不能创建它。
    pub(crate) fn attach(self: &Arc<Self>, binding: Binding) -> Result<Arc<Surface>, Reject> {
        let state = self.state.try_lock().map_err(|_| Reject::Busy)?;
        valid_binding(&state, binding)?;
        let host = state.host_surface.clone();
        drop(state);
        let surface = host.upgrade().ok_or(Reject::NotAttached)?;
        surface.on_ui_thread()?;
        Ok(surface)
    }

    /// # Safety
    /// 仅原生宿主可调用：已证明 UI 线程、单一输入 surface、排他焦点域与 OS 事件来源。
    /// 宿主须让所有真实 key/IME callback 持有此 Surface，撤权后确认 callback 退休再释放。
    /// Dart、Rinf 字段及 fixture 名称不能满足此前提。
    pub(crate) unsafe fn attach_verified(
        self: &Arc<Self>,
        attempt: u64,
    ) -> Result<Arc<Surface>, Reject> {
        let previous = {
            let state = self.state.try_lock().map_err(|_| Reject::Busy)?;
            if state.retired {
                return Err(Reject::Retired);
            }
            if state.binding.attempt != attempt {
                return Err(Reject::StaleAttempt);
            }
            if state.attached {
                return Err(Reject::Busy);
            }
            state.binding
        };
        let binding = Binding {
            attempt,
            epoch: previous.epoch.checked_add(1).ok_or(Reject::Retired)?,
        };
        let bytes = std::mem::size_of::<Surface>() + 2 * std::mem::size_of::<usize>();
        APP_USAGE
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes).filter(|next| *next <= APP_BYTES)
            })
            .map_err(|_| Reject::Busy)?;
        let budget = PortBudget(bytes);
        let surface = Arc::new(Surface {
            port: self.clone(),
            binding,
            ui_thread: std::thread::current().id(),
            rejected_down: std::array::from_fn(|_| AtomicU64::new(0)),
            reject_overflow: AtomicBool::new(false),
            pointer_pending: Mutex::new(None),
            _budget: budget,
        });
        let host = Arc::downgrade(&surface);
        let mut state = self.state.try_lock().map_err(|_| Reject::Busy)?;
        if state.retired {
            return Err(Reject::Retired);
        }
        if state.binding.attempt != attempt {
            return Err(Reject::StaleAttempt);
        }
        if state.binding != previous {
            return Err(Reject::StaleFocus);
        }
        if state.attached {
            return Err(Reject::Busy);
        }
        state.binding = binding;
        state.attached = true;
        state.last_context = 0;
        state.last_native_request = 0;
        state.busy_pointer = None;
        let lifecycle = if state.authenticated {
            NativeLifecycleState::Authenticated
        } else {
            state.lifecycle_state
        };
        self.publish_lifecycle(&mut state, lifecycle);
        let old = std::mem::replace(&mut state.host_surface, host);
        drop(state);
        drop(old);
        Ok(surface)
    }

    pub(crate) fn begin_attempt(&self) {
        self.revoke();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.authenticated = false;
        state.binding.attempt += 1;
        self.publish_lifecycle(&mut state, NativeLifecycleState::Connecting);
    }
    pub(crate) fn connected(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.authenticated = true;
        self.publish_lifecycle(&mut state, NativeLifecycleState::Authenticated);
    }
    pub(crate) fn attempt_failed(&self, authentication: bool) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.publish_lifecycle(
            &mut state,
            if authentication {
                NativeLifecycleState::AuthenticationFailed
            } else {
                NativeLifecycleState::AttemptFailed
            },
        );
    }
    pub(crate) fn disconnected(&self) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .authenticated = false;
        self.revoke();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.publish_lifecycle(&mut state, NativeLifecycleState::Disconnected);
    }

    pub(crate) fn revoke(&self) {
        let _ = self.revoke_binding(None);
    }
    fn revoke_binding(&self, binding: Option<Binding>) -> Result<(), Reject> {
        let empty_queue = VecDeque::with_capacity(INPUT_EVENTS + MAX_ACTIVE_KEYS + 2);
        let mut texts = Vec::with_capacity(INPUT_EVENTS);
        let mut state = if binding.is_some() {
            self.state.try_lock().map_err(|_| Reject::Busy)?
        } else {
            self.state.lock().unwrap_or_else(|e| e.into_inner())
        };
        if binding.is_some_and(|binding| binding != state.binding) {
            return Err(Reject::StaleFocus);
        }
        let focus_only = binding.is_some() && !state.retired;
        if focus_only && let Some(old) = self.seal_text(&mut state) {
            texts.push(old);
        }
        state.attached = false;
        state.binding.epoch += 1;
        state.policy = InputPolicyState::default();
        state.busy_pointer = None;
        let presented = state.presented.take();
        let mut queued = std::mem::replace(&mut state.queue, empty_queue);
        // 焦点撤权不撤销原会话的布局/帧控制；生命周期退出仍取消全部队列。
        if focus_only {
            for _ in 0..queued.len() {
                if let Some(effect) = queued.pop_front() {
                    if matches!(&effect.kind, EffectKind::Command(event) if source_control(event))
                        || matches!(effect.kind, EffectKind::Text)
                    {
                        state.queue.push_back(effect);
                    } else {
                        queued.push_back(effect);
                    }
                }
            }
        }
        for cycle in state.cycles.iter_mut().filter_map(Option::as_mut) {
            cycle.up = true;
        }
        if !focus_only {
            state.active_text = None;
            for text in state.texts.iter_mut().filter_map(Option::as_mut) {
                texts.push(std::mem::replace(
                    &mut text.text,
                    transactions::TextState::Cancelled,
                ));
            }
        }
        for slot in &mut state.cycles {
            if slot.as_ref().is_some_and(|cycle| cycle.pending == 0) {
                *slot = None;
            }
        }
        let action = if state.action.as_ref().is_some_and(|action| !action.claimed) {
            state.action.take()
        } else {
            None
        };
        // 只观察既有撤权完成点，不改变队列、文本或 physical latch 的退休规则。
        if state.binding.attempt != 0 {
            self.publish_lifecycle(&mut state, NativeLifecycleState::InputRevoked);
        }
        drop(state);
        drop((queued, texts, action, presented));
        self.wake.notify_waiters();
        Ok(())
    }
    fn retire_owner(&self) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).retired = true;
        self.disconnected();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.publish_lifecycle(&mut state, NativeLifecycleState::Ended);
        let ended = self.lifecycle_record(&state);
        let budget = state.lifecycle_budget.take();
        drop(state);
        let Some(budget) = budget else { return };
        #[cfg(test)]
        let _isolation = tests::isolate_source_registration();
        let mut registry = ENDED_REGISTRY
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if registry.len() == ENDED_LIFECYCLES {
            registry.pop_front();
        }
        registry.push_back(EndedLifecycle {
            source: self.source.source.clone(),
            identity: self.source.identity.clone(),
            record: ended,
            _budget: budget,
        });
    }

    fn lifecycle_record(&self, state: &State) -> LifecycleRecord {
        LifecycleRecord {
            binding: state.binding,
            revision: state.lifecycle_revision,
            state: state.lifecycle_state,
        }
    }
    #[cfg(test)]
    pub(crate) fn observe_lifecycle(
        &self,
    ) -> tokio::sync::mpsc::UnboundedReceiver<DesktopNativeLifecycle> {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        *self
            .lifecycle_observer
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(sender);
        receiver
    }
    fn publish_lifecycle(&self, state: &mut State, lifecycle: NativeLifecycleState) {
        // Ended 是最终状态；迟到的原 callback 只能继续被既有输入守护拒绝。
        if state.lifecycle_state == NativeLifecycleState::Ended {
            return;
        }
        state.lifecycle_revision += 1;
        state.lifecycle_state = lifecycle;
        let value = self
            .lifecycle_record(state)
            .projection(self.source.source.clone(), self.source.identity.clone());
        #[cfg(test)]
        if let Some(observer) = self
            .lifecycle_observer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            let _ = observer.send(value.clone());
        }
        value.send_signal_to_dart();
    }

    pub(crate) async fn next(self: &Arc<Self>) -> Effect {
        loop {
            let notified = self.wake.notified();
            let effect = self
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .queue
                .pop_front();
            if let Some(effect) = effect {
                return effect;
            }
            notified.await;
        }
    }
    /// 接收输出的线性化点，与原生输入的 effect 准入共用同一短临界区。
    pub(crate) fn output_boundary(&self) -> (u64, bool) {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        (state.next_effect, !state.queue.is_empty())
    }

    pub(crate) fn take_through(&self, boundary: u64) -> Option<Effect> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state
            .queue
            .front()
            .is_some_and(|effect| effect.seq <= boundary)
        {
            state.queue.pop_front()
        } else {
            None
        }
    }

    /// 仅 ACK 可在待决输入前结算；其他输入仍留在原队列。
    pub(crate) async fn next_control(self: &Arc<Self>) -> Effect {
        loop {
            let notified = self.wake.notified();
            let effect = {
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                let index = state.queue.iter().position(|effect| {
                    matches!(
                        &effect.kind,
                        EffectKind::Command(DesktopEvent::FrameAck { .. })
                    )
                });
                index.and_then(|index| state.queue.remove(index))
            };
            if let Some(effect) = effect {
                return effect;
            }
            notified.await;
        }
    }
    pub(crate) async fn disconnect_requested(&self) {
        loop {
            let notified = self.wake.notified();
            if self
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .disconnect_queued
            {
                return;
            }
            notified.await;
        }
    }
    pub(crate) fn applicable(&self, ticket: Ticket) -> bool {
        if ticket.registration != self.registration {
            return false;
        }
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        valid_binding(&state, ticket.binding).is_ok()
            && cycle_slot(&state, ticket.cycle).is_none_or(|slot| {
                state.cycles[slot]
                    .as_ref()
                    .is_some_and(|cycle| !cycle.rejected)
            })
    }
    pub(crate) fn applicable_effect(&self, ticket: Ticket, kind: &EffectKind) -> bool {
        if matches!(kind, EffectKind::GuiIntent(_)) {
            let Ok(state) = self.state.lock() else {
                return false;
            };
            ticket.registration == self.registration
                && !state.disconnect_queued
                && valid(&state, ticket.binding).is_ok()
        } else if matches!(kind, EffectKind::Text) {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            ticket.registration == self.registration
                && valid_attempt(&state, ticket.binding.attempt).is_ok()
                && state.texts.iter().flatten().any(|text| {
                    text.ticket == ticket
                        && !matches!(text.text, transactions::TextState::Cancelled)
                })
        } else if matches!(kind, EffectKind::Command(event) if source_control(event)) {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            ticket.registration == self.registration
                && valid_attempt(&state, ticket.binding.attempt).is_ok()
        } else {
            self.applicable(ticket)
        }
    }
    pub(crate) fn reject_ticket(&self, ticket: Ticket) {
        if ticket.registration != self.registration {
            return;
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(slot) = cycle_slot(&state, ticket.cycle)
            && let Some(cycle) = state.cycles[slot].as_mut()
        {
            cycle.rejected = true;
            let physical = cycle.physical;
            if !cycle.up && ticket.binding == state.binding {
                state.policy.reject_cycle(physical);
            }
        }
    }
    pub(crate) fn completed(&self, seq: u64) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let through = state.queue.front().map_or(state.control_applied, |next| {
            state.control_applied.min(next.seq.saturating_sub(1))
        });
        state.applied = state.applied.max(seq).max(through);
    }
    pub(crate) fn control_completed(&self, seq: u64) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.control_applied = state.control_applied.max(seq);
    }
    pub(crate) fn applied(&self) -> u64 {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).applied
    }
    pub(crate) fn binding(&self) -> Binding {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).binding
    }
    pub(crate) fn publish(
        &self,
        frame_revision: u32,
        frame: Arc<RenderFrame>,
        bounds: ViewportBounds,
        follow_bottom: bool,
        selection: Option<SelectionRange>,
        modes: TerminalDisplayModes,
    ) -> Result<(), Reject> {
        let projection = Projection {
            source: self.source.clone(),
            frame_revision,
            applied_effect: self.applied(),
            row_revision: self.row_revision(),
            frame,
            bounds,
            follow_bottom,
            selection,
            modes,
            mouse_reporting: self.profile.mouse_reporting && modes.mouse_reporting,
        };
        // 计量应用保留的真实投影内容；同帧查询仅增 Arc，最后一个投影消费者释放才退款。
        let bytes = projection.frame.rows.iter().fold(
            std::mem::size_of::<PublishedProjection>()
                + std::mem::size_of::<RenderFrame>()
                + projection.frame.title.len(),
            |total, row| {
                row.cells.iter().fold(
                    total.saturating_add(std::mem::size_of_val(row)),
                    |total, cell| {
                        total
                            .saturating_add(std::mem::size_of_val(cell))
                            .saturating_add(cell.text.len())
                    },
                )
            },
        );
        let old = self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .projection
            .take();
        drop(old);
        let reserved = APP_USAGE.fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
            used.checked_add(bytes).filter(|next| *next <= APP_BYTES)
        });
        if reserved.is_err() {
            self.state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .projection_failure = Some(Reject::Busy);
            return Err(Reject::Busy);
        }
        let new = Arc::new(PublishedProjection {
            value: projection,
            bytes,
        });
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.projection_failure = None;
        state.projection = Some(new);
        Ok(())
    }
    pub(crate) fn row_revision(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .row_revision
    }
    pub(crate) fn invalidate_projection(&self) -> u64 {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.row_revision += 1;
        let revision = state.row_revision;
        let old = state.projection.take();
        let presented = state.presented.take();
        drop(state);
        drop((old, presented));
        revision
    }
    pub(crate) fn projection(&self) -> Option<Arc<PublishedProjection>> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .projection
            .clone()
    }
    pub(crate) fn projection_result(&self) -> Result<Option<Arc<PublishedProjection>>, Reject> {
        let state = self.state.try_lock().map_err(|_| Reject::Busy)?;
        if let Some(error) = state.projection_failure {
            return Err(error);
        }
        Ok(state.projection.clone())
    }
}

pub(crate) struct Surface {
    port: Arc<InputPort>,
    binding: Binding,
    ui_thread: std::thread::ThreadId,
    rejected_down: [AtomicU64; MAX_ACTIVE_KEYS],
    reject_overflow: AtomicBool,
    pointer_pending: Mutex<Option<(u64, PointerInput)>>,
    _budget: PortBudget,
}
impl Surface {
    pub(crate) fn reject_native_packet(&self, physical: u64, phase: u32) {
        if physical == 0 || phase == 3 {
            return;
        }
        for latch in &self.rejected_down {
            if latch.load(Ordering::Acquire) == physical
                || latch
                    .compare_exchange(0, physical, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
            {
                return;
            }
        }
        self.reject_overflow.store(true, Ordering::Release);
    }
    fn on_ui_thread(&self) -> Result<(), Reject> {
        if std::thread::current().id() != self.ui_thread {
            return Err(Reject::WrongThread);
        }
        Ok(())
    }
    pub(crate) fn source(&self) -> &Source {
        &self.port.source
    }
    pub(crate) fn binding(&self) -> Binding {
        self.binding
    }
    pub(crate) fn command(&self, request_id: u64, event: DesktopEvent) -> Result<u64, Reject> {
        self.on_ui_thread()?;
        self.port
            .command(
                self.binding,
                request_id,
                crate::signals::desktop_native_input::NativeCommand::Event(event),
            )
            .map(|admission| admission.effect)
    }
    pub(crate) fn action(&self, request_id: u64, action: DesktopAction) -> Result<u64, Reject> {
        self.on_ui_thread()?;
        self.port
            .command(
                self.binding,
                request_id,
                crate::signals::desktop_native_input::NativeCommand::Action(action),
            )
            .map(|admission| admission.effect)
    }
    pub(crate) fn revoke(&self) -> Result<(), Reject> {
        self.on_ui_thread()?;
        self.port.revoke_binding(Some(self.binding))
    }

    #[cfg(test)]
    pub(crate) fn key(&self, input: PolicyKeyInput) -> Result<Admission, Reject> {
        let (context, resume) = {
            match self.port.state.try_lock() {
                Ok(state) => state
                    .active_text
                    .map_or((state.last_context + 1, 0), |ticket| {
                        (ticket.context, ticket.transaction)
                    }),
                Err(_) => (1, 0),
            }
        };
        self.key_with_context(input, context, resume)
    }

    pub(crate) fn key_with_context(
        &self,
        input: PolicyKeyInput,
        context: u64,
        resume: u64,
    ) -> Result<Admission, Reject> {
        self.on_ui_thread()?;
        if self
            .pointer_pending
            .try_lock()
            .map_err(|_| Reject::Busy)?
            .is_some()
        {
            return Err(Reject::Busy);
        }
        let physical = input.physical_identity;
        let phase = input.phase;
        let result = self.admit_key(input, context, resume);
        if result == Err(Reject::Busy)
            && phase == DesktopKeyPhase::Down
            && let Some(physical) = physical
        {
            for latch in &self.rejected_down {
                if latch.load(Ordering::Acquire) == physical.get()
                    || latch
                        .compare_exchange(0, physical.get(), Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                {
                    return result;
                }
            }
            self.reject_overflow.store(true, Ordering::Release);
        }
        result
    }

    fn admit_key(
        &self,
        input: PolicyKeyInput,
        context: u64,
        resume: u64,
    ) -> Result<Admission, Reject> {
        let physical = input
            .physical_identity
            .ok_or(Reject::Policy(InputReject::MissingPhysicalIdentity))?;
        let phase = input.phase;
        if self.reject_overflow.load(Ordering::Acquire) && phase != DesktopKeyPhase::Up {
            return Err(Reject::Busy);
        }
        for latch in &self.rejected_down {
            if latch.load(Ordering::Acquire) == physical.get() {
                if phase != DesktopKeyPhase::Up {
                    return Err(Reject::Cancelled);
                }
                // 生命周期的 Up 仍清除锁内 Busy cycle；没有 cycle 的竞争拒绝也在此结束。
                let mut state = self.port.state.try_lock().map_err(|_| Reject::Busy)?;
                valid_binding(&state, self.binding)?;
                if let Some(slot) = state.cycles.iter().position(|slot| {
                    slot.as_ref().is_some_and(|cycle| {
                        cycle.binding == self.binding && cycle.physical == physical && !cycle.up
                    })
                }) {
                    if let Some(cycle) = state.cycles[slot].as_mut() {
                        cycle.up = true;
                    }
                    if state.cycles[slot]
                        .as_ref()
                        .is_some_and(|cycle| cycle.pending == 0)
                    {
                        state.cycles[slot] = None;
                    }
                }
                let _ = policy::gate_key(&mut state.policy, &self.port.profile, input);
                latch.store(0, Ordering::Release);
                return Err(Reject::Cancelled);
            }
        }
        let mut state = match self.port.state.try_lock() {
            Ok(state) => state,
            Err(_) => {
                return Err(Reject::Busy);
            }
        };
        valid(&state, self.binding)?;
        if state.busy_pointer.is_some() {
            return Err(Reject::Busy);
        }
        let existing = state.cycles.iter().position(|slot| {
            slot.as_ref().is_some_and(|cycle| {
                cycle.binding == self.binding && cycle.physical == physical && !cycle.up
            })
        });
        let slot = if phase == DesktopKeyPhase::Down {
            if existing.is_some() {
                return Err(Reject::Policy(InputReject::DuplicateDown));
            }
            let slot = state
                .cycles
                .iter()
                .position(Option::is_none)
                .ok_or(Reject::Busy)?;
            state.next_cycle += 1;
            let id = state.next_cycle;
            state.cycles[slot] = Some(Cycle {
                id,
                binding: self.binding,
                physical,
                up: false,
                pending: 0,
                rejected: true,
                route: Route::Consumed,
                native_text: None,
            });
            slot
        } else {
            existing.ok_or(Reject::Policy(InputReject::UnassociatedPhase))?
        };
        let cycle = state.cycles[slot].as_ref().ok_or(Reject::InvalidTicket)?;
        let mut ticket = Ticket {
            registration: self.port.registration,
            binding: self.binding,
            cycle: cycle.id,
            transaction: 0,
            context: 0,
        };
        if phase != DesktopKeyPhase::Down && cycle.rejected {
            if phase == DesktopKeyPhase::Up {
                let cycle = state.cycles[slot].as_mut().ok_or(Reject::InvalidTicket)?;
                cycle.up = true;
                if cycle.pending == 0 {
                    state.cycles[slot] = None;
                }
                let _ = policy::gate_key(&mut state.policy, &self.port.profile, input);
            }
            return Err(Reject::Cancelled);
        }
        let mut shadow = state.policy.clone();
        let intent = policy::gate_native_key(&mut shadow, &self.port.profile, input);
        let route = match &intent {
            GatedIntent::Quiet(outcome) => match outcome.result {
                KeyResult::TypedInputReject(reason) => {
                    state.policy = shadow;
                    if let Some(cycle) = state.cycles[slot].as_mut() {
                        cycle.rejected = true;
                    }
                    state.policy.reject_cycle(physical);
                    if phase == DesktopKeyPhase::Up {
                        let cycle = state.cycles[slot].as_mut().ok_or(Reject::InvalidTicket)?;
                        cycle.up = true;
                        if cycle.pending == 0 {
                            state.cycles[slot] = None;
                        }
                    }
                    return Err(Reject::Policy(reason));
                }
                KeyResult::NoSend(NoSendReason::NativeText) if phase != DesktopKeyPhase::Up => {
                    Route::NativeText
                }
                _ => Route::Consumed,
            },
            _ => Route::Terminal,
        };
        let cycle_id = ticket.cycle;
        if phase == DesktopKeyPhase::Up
            && state.cycles[slot]
                .as_ref()
                .is_some_and(|cycle| cycle.route == Route::NativeText)
        {
            let native = state.cycles[slot]
                .as_ref()
                .and_then(|cycle| cycle.native_text);
            let authorized = native
                .filter(|ticket| ticket.context == context && state.active_text == Some(*ticket));
            state.policy = shadow;
            let cycle = state.cycles[slot].as_mut().ok_or(Reject::InvalidTicket)?;
            cycle.up = true;
            if cycle.pending == 0 {
                state.cycles[slot] = None;
            }
            return Ok(Admission {
                ticket: authorized.unwrap_or(ticket),
                effect: authorized.map_or(0, |ticket| ticket.transaction),
                route: if authorized.is_some() {
                    Route::NativeText
                } else {
                    Route::Consumed
                },
                cycle_id,
                text_op: TextOp::None,
                end: None,
            });
        }
        let data = phase != DesktopKeyPhase::Up;
        if route == Route::NativeText {
            if context == 0 {
                return Err(Reject::InvalidTicket);
            }
            if let Some(active) = state.active_text {
                if active.context != context
                    || active.transaction != resume
                    || active.binding != self.binding
                {
                    return Err(Reject::InvalidTicket);
                }
                state.policy = shadow;
                let cycle = state.cycles[slot].as_mut().ok_or(Reject::InvalidTicket)?;
                cycle.rejected = false;
                cycle.route = Route::NativeText;
                cycle.native_text = Some(active);
                return Ok(Admission {
                    ticket: active,
                    effect: active.transaction,
                    route,
                    cycle_id,
                    text_op: TextOp::Continue,
                    end: None,
                });
            }
            if resume != 0 || context <= state.last_context {
                return Err(Reject::InvalidTicket);
            }
            if state
                .texts
                .iter()
                .flatten()
                .any(|text| matches!(text.text, transactions::TextState::Ending(_)))
            {
                return Err(Reject::Busy);
            }
        }
        let text_slot = if route == Route::NativeText {
            Some(
                state
                    .texts
                    .iter()
                    .position(Option::is_none)
                    .ok_or(Reject::Busy)?,
            )
        } else {
            None
        };
        let bytes = if data { EVENT_BYTES } else { 0 };
        reserve(&mut state, bytes, data)?;
        if !self.port.mode.native() {
            refund(&mut state, bytes, data);
            return Err(Reject::MixedMode);
        }
        state.policy = shadow;
        let cycle = state.cycles[slot].as_mut().ok_or(Reject::InvalidTicket)?;
        cycle.rejected = false;
        cycle.up |= phase == DesktopKeyPhase::Up;
        cycle.route = route;
        if route != Route::NativeText {
            cycle.pending += 1;
        }
        let kind = if route == Route::NativeText {
            EffectKind::Text
        } else {
            EffectKind::Key(intent)
        };
        if let Some(slot) = text_slot {
            ticket.transaction = state.next_effect + 1;
            ticket.context = context;
            ticket.cycle = 0;
            state.last_context = context;
            state.active_text = Some(ticket);
            let cycle = state
                .cycles
                .iter_mut()
                .flatten()
                .find(|cycle| cycle.id == cycle_id)
                .ok_or(Reject::InvalidTicket)?;
            cycle.native_text = Some(ticket);
            state.texts[slot] = Some(TextTransaction {
                ticket,
                text: transactions::TextState::Waiting,
            });
        }
        let ended = if route != Route::NativeText && phase != DesktopKeyPhase::Up {
            self.port.seal_text(&mut state)
        } else {
            None
        };
        let end = state
            .texts
            .iter()
            .flatten()
            .find_map(|text| match text.text {
                transactions::TextState::Ending(end) if ended.is_some() => Some(end),
                _ => None,
            });
        let seq = enqueue(&self.port, &mut state, ticket, kind, bytes, data);
        drop(state);
        drop(ended);
        self.port.wake.notify_waiters();
        Ok(Admission {
            ticket,
            effect: seq,
            route,
            cycle_id,
            text_op: if route == Route::NativeText {
                TextOp::Start
            } else if end.is_some() {
                TextOp::End
            } else {
                TextOp::None
            },
            end,
        })
    }
}

fn valid(state: &State, binding: Binding) -> Result<(), Reject> {
    valid_binding(state, binding)?;
    if !state.authenticated {
        return Err(Reject::NotAuthenticated);
    }
    Ok(())
}
fn valid_binding(state: &State, binding: Binding) -> Result<(), Reject> {
    valid_attempt(state, binding.attempt)?;
    if binding.epoch != state.binding.epoch {
        return Err(Reject::StaleFocus);
    }
    if !state.attached {
        return Err(Reject::NotAttached);
    }
    Ok(())
}
fn valid_attempt(state: &State, attempt: u64) -> Result<(), Reject> {
    if state.retired {
        return Err(Reject::Retired);
    }
    if attempt != state.binding.attempt {
        return Err(Reject::StaleAttempt);
    }
    Ok(())
}

fn source_control(event: &DesktopEvent) -> bool {
    matches!(
        event,
        DesktopEvent::Resize { .. }
            | DesktopEvent::Viewport { .. }
            | DesktopEvent::FrameAck { .. }
            | DesktopEvent::Disconnect
            | DesktopEvent::Reconnect
    )
}
fn cycle_slot(state: &State, cycle: u64) -> Option<usize> {
    state
        .cycles
        .iter()
        .position(|entry| entry.as_ref().is_some_and(|entry| entry.id == cycle))
}
fn reserve(state: &mut State, bytes: usize, data: bool) -> Result<(), Reject> {
    if bytes > INPUT_BYTES {
        return Err(Reject::TooLarge);
    }
    if data && (state.data_count >= INPUT_EVENTS || state.bytes.saturating_add(bytes) > INPUT_BYTES)
    {
        return Err(Reject::Busy);
    }
    APP_USAGE
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
            used.checked_add(bytes).filter(|next| *next <= APP_BYTES)
        })
        .map_err(|_| Reject::Busy)?;
    state.bytes += bytes;
    state.data_count += usize::from(data);
    Ok(())
}
fn refund(state: &mut State, bytes: usize, data: bool) {
    state.bytes -= bytes;
    state.data_count -= usize::from(data);
    APP_USAGE.fetch_sub(bytes, Ordering::AcqRel);
}
fn enqueue(
    port: &Arc<InputPort>,
    state: &mut State,
    ticket: Ticket,
    kind: EffectKind,
    bytes: usize,
    data: bool,
) -> u64 {
    state.next_effect += 1;
    let seq = state.next_effect;
    let disconnect = matches!(&kind, EffectKind::Command(DesktopEvent::Disconnect));
    state.queue.push_back(Effect {
        seq,
        ticket,
        kind,
        lease: Arc::new(EffectLease {
            port: port.clone(),
            ticket,
            bytes,
            data,
            disconnect,
        }),
    });
    seq
}

#[cfg(test)]
pub(crate) mod tests;

#[cfg(test)]
mod lifecycle_tests;

#[cfg(test)]
mod gui_tests;
