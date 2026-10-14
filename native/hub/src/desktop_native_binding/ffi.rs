//! 独立 normal ABI。所有裸指针仅在单次调用内借用，handle 不含可解引用地址。

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, TryLockError};

use crate::desktop_input_policy::{
    AltProvenance, PolicyKeyInput, RawChord, TextOwnership, VerifiedKeyCodes,
};
use crate::desktop_input_port::transactions::{ActionClaim, ActionReply, TextPayload};
use crate::desktop_input_port::{
    self as port, Binding, InputPort, PortBudget, Reject, Route, Surface, Ticket,
};
use crate::desktop_runtime::{INPUT_BYTES, INPUT_EVENTS};
use crate::signals::desktop_session::{DesktopAction, DesktopKeyPhase, DesktopSource};
use rshell_m0::rshell_core::{KeyCode, KeyModifiers};

#[path = "platform.rs"]
mod platform;

pub(super) const ABI_VERSION: u32 = 1;

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Status {
    Ok = 0,
    InvalidArgument = 1,
    UnsupportedVersion = 2,
    UnknownHandle = 3,
    NotAttached = 4,
    StaleSource = 5,
    StaleAttempt = 6,
    StaleFocus = 7,
    NotAuthenticated = 8,
    WrongThread = 9,
    Retired = 10,
    Busy = 11,
    TooLarge = 12,
    MixedMode = 13,
    Duplicate = 14,
    InvalidEvent = 15,
    InvalidTicket = 16,
    Cancelled = 17,
    PolicyRejected = 18,
    BufferTooSmall = 19,
    #[cfg(not(target_os = "windows"))]
    UnsupportedPlatform = 20,
    Internal = 21,
}

#[derive(Debug)]
pub(super) struct Error(Status);
impl From<Status> for Error {
    fn from(status: Status) -> Self {
        Self(status)
    }
}
impl From<Reject> for Error {
    fn from(reject: Reject) -> Self {
        Self(match reject {
            Reject::UnsupportedVersion => Status::UnsupportedVersion,
            Reject::NotAttached => Status::NotAttached,
            Reject::StaleSource => Status::StaleSource,
            Reject::StaleAttempt => Status::StaleAttempt,
            Reject::StaleFocus => Status::StaleFocus,
            Reject::NotAuthenticated => Status::NotAuthenticated,
            Reject::WrongThread => Status::WrongThread,
            Reject::Retired => Status::Retired,
            Reject::Busy => Status::Busy,
            Reject::TooLarge => Status::TooLarge,
            Reject::MixedMode => Status::MixedMode,
            Reject::Duplicate => Status::Duplicate,
            Reject::InvalidEvent => Status::InvalidEvent,
            Reject::InvalidTicket => Status::InvalidTicket,
            Reject::Cancelled => Status::Cancelled,
            Reject::Policy(_) => Status::PolicyRejected,
        })
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct Header {
    pub version: u32,
    pub size: u32,
}

// 所有顶层布局都有相同头；先读取头再验证完整长度，不借异常恢复访问越界指针。
pub(super) trait Layout: Copy {}
macro_rules! layout {
    ($name:ident { $($field:ident : $ty:ty),* $(,)? }) => {
        #[repr(C)]
        #[derive(Clone, Copy)]
        pub(super) struct $name { pub header: Header, $(pub $field: $ty),* }
        impl Layout for $name {}
    };
}

layout!(Context {
    handle: u64, session_id: u32, generation: u64,
    identity: *const u8, identity_len: u64, attempt: u64, focus_epoch: u64,
});
layout!(Registration {
    session_id: u32, generation: u64, identity: *const u8, identity_len: u64,
    attempt: u64, window: u64, input_object: u64,
});
layout!(Bound {
    handle: u64,
    attempt: u64,
    focus_epoch: u64
});
layout!(Key {
    physical: u64,
    phase: u32,
    modifiers: u32,
    raw_kind: u32,
    raw_value: u32,
    code_kind: u32,
    code_value: u32,
    legacy_kind: u32,
    legacy_value: u32,
    raw_known: u32,
    same_event_dual: u32,
    alt_provenance: u32,
    text_ownership: u32,
    default_action: u32,
    context_id: u64,
    resume_text_ticket: u64,
});
layout!(Admission {
    effect: u64,
    text_ticket: u64,
    route: u32,
    cycle_id: u64,
    text_op: u32,
    end_fence: u64,
    end_context_id: u64,
    end_transaction_id: u64,
});
layout!(TextEnd {
    context_id: u64,
    transaction_id: u64,
    end_fence: u64
});
layout!(PresentedGeometry {
    native_request_id: u64,
    geometry: port::PresentedGeometry
});
layout!(Pointer {
    native_request_id: u64,
    input: port::PointerInput
});
layout!(Composition { text_ticket: u64, kind: u32, text: *const u8, text_len: u64 });
layout!(TextReservation {
    text_ticket: u64,
    utf8_bytes: u64
});
layout!(TextChunk { text_ticket: u64, offset: u64, bytes: *const u8, len: u64 });
layout!(TextFinish { text_ticket: u64 });
layout!(ActionRequest {
    request_id: u64,
    action: u32
});
layout!(ClaimRequest { effect: u64 });
layout!(ClaimInfo {
    claim: u64,
    effect: u64,
    action: u32,
    rejection: u32,
    payload_len: u64
});
layout!(PayloadCopy { claim: u64, buffer: *mut u8, capacity: u64 });
layout!(Ack { claim: u64, kind: u32, text: *const u8, text_len: u64 });
layout!(Projection {
    frame_revision: u32,
    applied_effect: u64,
    row_revision: u64,
    follow_bottom: u32,
    first_stable_row: i64,
    screen_top_stable_row: i64,
    viewport_top: i64,
    mouse_reporting: u32,
    bracketed_paste: u32,
    selection_present: u32,
    start_row: i64,
    start_col: u32,
    end_row: i64,
    end_col: u32,
    rectangular: u32,
    caret_present: u32,
    caret_x: f64,
    caret_y: f64,
    caret_width: f64,
    caret_height: f64,
    dpi: u32,
});

fn header<T>() -> Header {
    Header {
        version: ABI_VERSION,
        size: std::mem::size_of::<T>() as u32,
    }
}

fn boundary(body: impl FnOnce() -> Result<(), Error>) -> u32 {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(Ok(())) => Status::Ok as u32,
        Ok(Err(error)) => error.0 as u32,
        Err(_) => Status::Internal as u32,
    }
}

fn span<T>(pointer: *const T, count: u64) -> Result<usize, Error> {
    let count = usize::try_from(count).map_err(|_| Status::TooLarge)?;
    let bytes = count
        .checked_mul(std::mem::size_of::<T>())
        .ok_or(Status::TooLarge)?;
    if bytes > isize::MAX as usize {
        return Err(Status::TooLarge.into());
    }
    if count != 0
        && (pointer.is_null() || !(pointer as usize).is_multiple_of(std::mem::align_of::<T>()))
    {
        return Err(Status::InvalidArgument.into());
    }
    if !pointer.is_null() && (pointer as usize).checked_add(bytes).is_none() {
        return Err(Status::InvalidArgument.into());
    }
    Ok(count)
}

// Safety: 调用方保证指针指向可读、正确对齐且在本次调用内存活的 Header/完整结构。
unsafe fn read<T: Layout>(pointer: *const T) -> Result<T, Error> {
    span(pointer.cast::<Header>(), 1)?;
    let first = unsafe { pointer.cast::<Header>().read() };
    if first.version != ABI_VERSION {
        return Err(Status::UnsupportedVersion.into());
    }
    if first.size as usize != std::mem::size_of::<T>() {
        return Err(Status::InvalidArgument.into());
    }
    span(pointer, 1)?;
    Ok(unsafe { pointer.read() })
}

// 输出也由宿主预填 version/size；失败不改输出或 count。
unsafe fn output<T: Layout>(pointer: *mut T) -> Result<(), Error> {
    span(pointer.cast::<Header>(), 1)?;
    let first = unsafe { pointer.cast::<Header>().read() };
    if first.version != ABI_VERSION {
        return Err(Status::UnsupportedVersion.into());
    }
    if first.size as usize != std::mem::size_of::<T>() {
        return Err(Status::InvalidArgument.into());
    }
    span(pointer, 1)?;
    Ok(())
}

unsafe fn utf8<'a>(pointer: *const u8, len: u64, limit: usize) -> Result<&'a str, Error> {
    let count = span(pointer, len)?;
    if count > limit {
        return Err(Status::TooLarge.into());
    }
    if count == 0 {
        return Ok("");
    }
    let bytes = unsafe { std::slice::from_raw_parts(pointer, count) };
    std::str::from_utf8(bytes).map_err(|_| Status::InvalidArgument.into())
}

struct State {
    tickets: [Option<(u64, Ticket)>; INPUT_EVENTS],
    claim: Option<ActionClaim>,
    payload: Option<(u64, TextPayload)>,
}

fn state_error<T>(error: TryLockError<T>) -> Error {
    match error {
        TryLockError::WouldBlock => Status::Busy.into(),
        TryLockError::Poisoned(_) => Status::Internal.into(),
    }
}

pub(super) struct NativeHost {
    surface: Arc<Surface>,
    port: Arc<InputPort>,
    platform: platform::Host,
    revoked: AtomicBool,
    state: Mutex<State>,
    _budget: PortBudget,
    #[cfg(test)]
    local_test: bool,
}

struct Entry {
    host: Arc<NativeHost>,
    _budget: PortBudget,
}
static HOSTS: OnceLock<Mutex<HashMap<u64, Entry>>> = OnceLock::new();
static HANDLES: AtomicU64 = AtomicU64::new(1);

fn hosts() -> &'static Mutex<HashMap<u64, Entry>> {
    HOSTS.get_or_init(Mutex::default)
}

fn insert(host: Arc<NativeHost>) -> Result<u64, Error> {
    let budget = port::reserve_output(std::mem::size_of::<Entry>() + 64).ok_or(Status::Busy)?;
    let id = HANDLES
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |id| id.checked_add(1))
        .map_err(|_| Status::Retired)?;
    hosts().try_lock().map_err(|_| Status::Busy)?.insert(
        id,
        Entry {
            host,
            _budget: budget,
        },
    );
    Ok(id)
}

impl NativeHost {
    fn active(&self, focus: bool) -> Result<(), Error> {
        self.platform.thread()?;
        if self.state.is_poisoned() {
            return Err(Status::Internal.into());
        }
        if self.revoked.load(Ordering::Acquire) {
            return Err(Status::NotAttached.into());
        }
        #[cfg(test)]
        if !self.local_test {
            self.platform.check(focus)?;
        }
        #[cfg(not(test))]
        self.platform.check(focus)?;
        Ok(())
    }

    fn seal(&self) -> Result<(), Error> {
        match self.surface.revoke() {
            Ok(()) | Err(Reject::StaleFocus | Reject::StaleAttempt | Reject::Retired) => {}
            Err(error) => return Err(error.into()),
        }
        self.revoked.store(true, Ordering::Release);
        Ok(())
    }

    fn cancel_poisoned(&self, mut state: MutexGuard<'_, State>) -> Result<(), Error> {
        self.seal()?;
        // 半写状态只移出责任，不读取票据/claim 决策，也不清除永久准入封闭的 poison。
        let abandoned = std::mem::replace(
            &mut *state,
            State {
                tickets: [None; INPUT_EVENTS],
                claim: None,
                payload: None,
            },
        );
        drop(state);
        // 既有 ActionClaim::Drop 恢复完成邮箱并通知原 actor 取消。
        drop(abandoned);
        Ok(())
    }
}

unsafe fn get(context: *const Context) -> Result<Arc<NativeHost>, Error> {
    let context = unsafe { read(context) }?;
    let identity = unsafe { utf8(context.identity, context.identity_len, INPUT_BYTES) }?;
    let registry = hosts().try_lock().map_err(|_| Status::Busy)?;
    let host = registry
        .get(&context.handle)
        .ok_or(Status::UnknownHandle)?
        .host
        .clone();
    drop(registry);
    let source = host.surface.source();
    if source.source.session_id != context.session_id
        || source.source.generation != context.generation
        || source.identity != identity
    {
        return Err(Status::StaleSource.into());
    }
    if host.surface.binding().attempt != context.attempt {
        return Err(Status::StaleAttempt.into());
    }
    if host.surface.binding().epoch != context.focus_epoch {
        return Err(Status::StaleFocus.into());
    }
    host.platform.thread()?;
    Ok(host)
}

/// # Safety
/// 只能由真实原生适配器调用。窗口 property 必须关联实际存活的 input_object；宿主已
/// 将该对象排他绑定到本次 Prepared 的精确 Source/identity/attempt，而非 current pane。
/// HWND 核验不证明物理键、raw/dual 同事件或 IME；这些真实来源由宿主承担，禁止 Dart 数字
/// 或自报 bool 签发。所有指针须指向有效可读/可写内存并满足头部声明的完整布局。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guossh_normal_v1_register_trusted_host(
    request: *const Registration,
    result: *mut Bound,
) -> u32 {
    boundary(|| {
        let request = unsafe { read(request) }?;
        unsafe { output(result) }?;
        let identity = unsafe { utf8(request.identity, request.identity_len, INPUT_BYTES) }?;
        if identity.is_empty() {
            return Err(Status::InvalidArgument.into());
        }
        let source = DesktopSource {
            session_id: request.session_id,
            generation: request.generation,
        };
        let platform = platform::Host::register(request.window, request.input_object)?;
        let port = port::lookup(&source, identity)?;
        let budget =
            port::reserve_output(std::mem::size_of::<NativeHost>() + 64).ok_or(Status::Busy)?;
        let entry_budget =
            port::reserve_output(std::mem::size_of::<Entry>() + 64).ok_or(Status::Busy)?;
        let handle = HANDLES
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |id| id.checked_add(1))
            .map_err(|_| Status::Retired)?;
        let mut registry = hosts().try_lock().map_err(|_| Status::Busy)?;
        // 原 port Arc 保持来源登记存活；相同 Source 查找仍取得同一原运行登记。
        // 先预约 ABI 登记，attach 成功后没有会返回 Busy 的二次查询或补偿撤权。
        let surface = unsafe { super::register_verified_host(&source, identity, request.attempt) }?;
        let binding = surface.binding();
        let host = Arc::new(NativeHost {
            surface,
            port,
            platform,
            revoked: AtomicBool::new(false),
            state: Mutex::new(State {
                tickets: [None; INPUT_EVENTS],
                claim: None,
                payload: None,
            }),
            _budget: budget,
            #[cfg(test)]
            local_test: false,
        });
        registry.insert(
            handle,
            Entry {
                host,
                _budget: entry_budget,
            },
        );
        drop(registry);
        unsafe {
            result.write(Bound {
                header: header::<Bound>(),
                handle,
                attempt: binding.attempt,
                focus_epoch: binding.epoch,
            })
        };
        Ok(())
    })
}

/// # Safety
/// 指针遵守头部及寿命合同。只能领取已登记的同一原生 surface，不签发 OS 身份。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guossh_normal_v1_bind(request: *const Context, result: *mut Bound) -> u32 {
    boundary(|| {
        let request = unsafe { read(request) }?;
        unsafe { output(result) }?;
        let identity = unsafe { utf8(request.identity, request.identity_len, INPUT_BYTES) }?;
        let source = DesktopSource {
            session_id: request.session_id,
            generation: request.generation,
        };
        let surface = super::bind(
            super::VERSION,
            &source,
            identity,
            Binding {
                attempt: request.attempt,
                epoch: request.focus_epoch,
            },
        )?;
        let registry = hosts().try_lock().map_err(|_| Status::Busy)?;
        let host = registry
            .values()
            .find(|entry| Arc::ptr_eq(&entry.host.surface, &surface))
            .ok_or(Status::NotAttached)?
            .host
            .clone();
        // handle=0 是首次查找；非零 handle 必须是原 host，不允许借 bind retarget。
        if request.handle != 0
            && !registry
                .get(&request.handle)
                .is_some_and(|entry| Arc::ptr_eq(&entry.host, &host))
        {
            return Err(Status::UnknownHandle.into());
        }
        drop(registry);
        host.active(false)?;
        let binding = surface.binding();
        let handle = insert(host)?;
        unsafe {
            result.write(Bound {
                header: header::<Bound>(),
                handle,
                attempt: binding.attempt,
                focus_epoch: binding.epoch,
            })
        };
        Ok(())
    })
}

fn code(kind: u32, value: u32) -> Result<KeyCode, Error> {
    if kind != 1 && kind != 16 && value != 0 {
        return Err(Status::InvalidEvent.into());
    }
    Ok(match kind {
        1 => KeyCode::Character(char::from_u32(value).ok_or(Status::InvalidEvent)?),
        2 => KeyCode::Enter,
        3 => KeyCode::Escape,
        4 => KeyCode::Tab,
        5 => KeyCode::Backspace,
        6 => KeyCode::Delete,
        7 => KeyCode::Insert,
        8 => KeyCode::Home,
        9 => KeyCode::End,
        10 => KeyCode::PageUp,
        11 => KeyCode::PageDown,
        12 => KeyCode::ArrowUp,
        13 => KeyCode::ArrowDown,
        14 => KeyCode::ArrowLeft,
        15 => KeyCode::ArrowRight,
        16 if (1..=24).contains(&value) => {
            KeyCode::F(u8::try_from(value).map_err(|_| Status::InvalidEvent)?)
        }
        _ => return Err(Status::InvalidEvent.into()),
    })
}

fn action(value: u32) -> Result<DesktopAction, Error> {
    Ok(match value {
        1 => DesktopAction::NewTab,
        2 => DesktopAction::SplitRight,
        3 => DesktopAction::SplitDown,
        4 => DesktopAction::ClosePane,
        5 => DesktopAction::NextTab,
        6 => DesktopAction::PreviousTab,
        7 => DesktopAction::NextPane,
        8 => DesktopAction::Search,
        9 => DesktopAction::Settings,
        10 => DesktopAction::ToggleSidebar,
        11 => DesktopAction::Copy,
        12 => DesktopAction::Paste,
        13 => DesktopAction::SelectAll,
        14 => DesktopAction::ZoomIn,
        15 => DesktopAction::ZoomOut,
        16 => DesktopAction::ZoomReset,
        _ => return Err(Status::InvalidEvent.into()),
    })
}

fn action_id(value: DesktopAction) -> u32 {
    match value {
        DesktopAction::NewTab => 1,
        DesktopAction::SplitRight => 2,
        DesktopAction::SplitDown => 3,
        DesktopAction::ClosePane => 4,
        DesktopAction::NextTab => 5,
        DesktopAction::PreviousTab => 6,
        DesktopAction::NextPane => 7,
        DesktopAction::Search => 8,
        DesktopAction::Settings => 9,
        DesktopAction::ToggleSidebar => 10,
        DesktopAction::Copy => 11,
        DesktopAction::Paste => 12,
        DesktopAction::SelectAll => 13,
        DesktopAction::ZoomIn => 14,
        DesktopAction::ZoomOut => 15,
        DesktopAction::ZoomReset => 16,
    }
}

fn key_input(key: Key) -> Result<PolicyKeyInput, Error> {
    if key.modifiers & !63 != 0 || key.raw_known > 1 || key.same_event_dual > 1 {
        return Err(Status::InvalidEvent.into());
    }
    // ABI 不允许缺失的 raw/同事件 dual 或 Alt 来源凭 NativeText 标记退成 IME。
    // 数值标记只有在受信宿主的真实事件合同下才有效，不在这里制造来源证明。
    if key.physical == 0
        || key.raw_known == 0
        || key.same_event_dual == 0
        || key.text_ownership == 0
        || (key.modifiers & 4 != 0 && key.alt_provenance == 0)
        || (key.alt_provenance == 1 && (key.modifiers & 4 != 0) != (key.modifiers & 48 != 0))
    {
        return Err(Status::PolicyRejected.into());
    }
    let primary = code(key.code_kind, key.code_value)?;
    let legacy = code(key.legacy_kind, key.legacy_value)?;
    if primary != legacy
        && !matches!(
            (&primary, &legacy),
            (KeyCode::Character(_), KeyCode::Character(_))
        )
    {
        return Err(Status::PolicyRejected.into());
    }
    Ok(PolicyKeyInput {
        physical_identity: NonZeroU64::new(key.physical),
        phase: match key.phase {
            1 => DesktopKeyPhase::Down,
            2 => DesktopKeyPhase::Repeat,
            3 => DesktopKeyPhase::Up,
            _ => return Err(Status::InvalidEvent.into()),
        },
        raw_chord: if key.raw_known == 1 {
            Some(RawChord {
                code: code(key.raw_kind, key.raw_value)?,
                modifiers: KeyModifiers {
                    shift: key.modifiers & 1 != 0,
                    control: key.modifiers & 2 != 0,
                    alt: key.modifiers & 4 != 0,
                    super_key: key.modifiers & 8 != 0,
                },
            })
        } else {
            None
        },
        dual: if key.same_event_dual == 1 {
            Some(VerifiedKeyCodes {
                code: primary,
                legacy_code: legacy,
            })
        } else {
            None
        },
        alt_provenance: match key.alt_provenance {
            0 => AltProvenance::Unknown,
            1 => AltProvenance::Known {
                left: key.modifiers & 16 != 0,
                right: key.modifiers & 32 != 0,
            },
            _ => return Err(Status::InvalidEvent.into()),
        },
        text_ownership: match key.text_ownership {
            0 => TextOwnership::Unverified,
            1 => TextOwnership::TerminalKey,
            2 => TextOwnership::NativeText,
            _ => return Err(Status::InvalidEvent.into()),
        },
        default_action: if key.default_action == 0 {
            None
        } else {
            Some(action(key.default_action)?)
        },
    })
}

/// # Safety
/// 指针须有效。raw_known/same_event_dual/ownership 只描述受信宿主已证明的真实事件，
/// 不能由 Dart/self-reported 数值当成证明。每次事件仍属于登记的原输入对象。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guossh_normal_v1_key(
    context: *const Context,
    request: *const Key,
    result: *mut Admission,
) -> u32 {
    boundary(|| {
        let host = unsafe { get(context) }?;
        let request = unsafe { read(request) }?;
        unsafe { output(result) }?;
        host.active(true)?;
        let input = match key_input(request) {
            Ok(input) => input,
            Err(error) => {
                host.surface
                    .reject_native_packet(request.physical, request.phase);
                return Err(error);
            }
        };
        let mut state = host.state.try_lock().map_err(state_error)?;
        let slot = state.tickets.iter().position(Option::is_none);
        // Surface 自己处理 actor 临界区及满额 Busy Down 的原按压锁存；
        // ABI 不先 attach/截断准入，以免绕过该生命周期保护。
        let admitted =
            host.surface
                .key_with_context(input, request.context_id, request.resume_text_ticket)?;
        let (route, text_ticket) = match admitted.route {
            Route::Terminal => (1, 0),
            Route::NativeText => {
                let slot = match state
                    .tickets
                    .iter()
                    .position(|entry| entry.is_some_and(|(id, _)| id == admitted.effect))
                    .or(slot)
                {
                    Some(slot) => slot,
                    None => {
                        host.surface.composition(admitted.ticket, None, false)?;
                        return Err(Status::Internal.into());
                    }
                };
                state.tickets[slot] = Some((admitted.effect, admitted.ticket));
                (2, admitted.effect)
            }
            Route::Consumed => (3, 0),
        };
        unsafe {
            result.write(Admission {
                header: header::<Admission>(),
                effect: admitted.effect,
                text_ticket,
                route,
                cycle_id: admitted.cycle_id,
                text_op: admitted.text_op as u32,
                end_fence: admitted.end.map_or(0, |end| end.end_fence),
                end_context_id: admitted.end.map_or(0, |end| end.context_id),
                end_transaction_id: admitted.end.map_or(0, |end| end.transaction_id),
            })
        };
        Ok(())
    })
}

/// # Safety
/// 原 IME callback 的指针及 UTF-8 有效；只可消费 key 返回的原 one-shot text_ticket。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guossh_normal_v1_composition(
    context: *const Context,
    request: *const Composition,
) -> u32 {
    boundary(|| {
        let host = unsafe { get(context) }?;
        let request = unsafe { read(request) }?;
        host.active(true)?;
        let text = match request.kind {
            1 | 2 => Some(unsafe { utf8(request.text, request.text_len, INPUT_BYTES) }?.to_owned()),
            3 if request.text_len == 0 => None,
            _ => return Err(Status::InvalidEvent.into()),
        };
        let mut state = host.state.try_lock().map_err(state_error)?;
        let slot = state
            .tickets
            .iter()
            .position(|ticket| ticket.is_some_and(|(id, _)| id == request.text_ticket))
            .ok_or(Status::InvalidTicket)?;
        let (_, ticket) = state.tickets[slot].ok_or(Status::InvalidTicket)?;
        host.surface.composition(ticket, text, request.kind == 2)?;
        if request.kind != 1 {
            state.tickets[slot] = None;
        }
        let old_payload = if request.kind != 1 {
            state.payload.take()
        } else {
            None
        };
        drop(state);
        drop(old_payload);
        Ok(())
    })
}

/// # Safety
/// 宿主先取得完整 UTF-8 授额，再分配/复制对应原事务的 native 缓冲。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guossh_normal_v1_reserve_payload(
    context: *const Context,
    request: *const TextReservation,
) -> u32 {
    boundary(|| {
        let host = unsafe { get(context) }?;
        let request = unsafe { read(request) }?;
        host.active(true)?;
        let mut state = host.state.try_lock().map_err(state_error)?;
        let ticket = state
            .tickets
            .iter()
            .flatten()
            .find(|(id, _)| *id == request.text_ticket)
            .map(|(_, ticket)| *ticket)
            .ok_or(Status::InvalidTicket)?;
        host.port.text_open(ticket)?;
        if state.payload.is_some() {
            return Err(Status::Busy.into());
        }
        let bytes = usize::try_from(request.utf8_bytes).map_err(|_| Status::TooLarge)?;
        state.payload = Some((request.text_ticket, host.port.reserve_text_payload(bytes)?));
        Ok(())
    })
}

/// # Safety
/// bytes 可读 len 字节；offset 连续，从零开始，可跨 UTF-8 scalar 分块。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guossh_normal_v1_payload(
    context: *const Context,
    request: *const TextChunk,
) -> u32 {
    boundary(|| {
        let host = unsafe { get(context) }?;
        let request = unsafe { read(request) }?;
        host.active(true)?;
        let count = span(request.bytes, request.len)?;
        if count > INPUT_BYTES {
            return Err(Status::TooLarge.into());
        }
        let mut state = host.state.try_lock().map_err(state_error)?;
        let ticket = state
            .tickets
            .iter()
            .flatten()
            .find(|(id, _)| *id == request.text_ticket)
            .map(|(_, ticket)| *ticket)
            .ok_or(Status::InvalidTicket)?;
        host.port.text_open(ticket)?;
        let payload = state
            .payload
            .as_mut()
            .filter(|(id, _)| *id == request.text_ticket)
            .ok_or(Status::InvalidTicket)?;
        let bytes = if count == 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(request.bytes, count) }
        };
        payload.1.append(
            usize::try_from(request.offset).map_err(|_| Status::TooLarge)?,
            bytes,
        )?;
        Ok(())
    })
}

/// # Safety
/// 只提交原授额的完整 UTF-8；不完整/无效正文不截断，不迁移原 owner。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guossh_normal_v1_finish(
    context: *const Context,
    request: *const TextFinish,
) -> u32 {
    boundary(|| {
        let host = unsafe { get(context) }?;
        let request = unsafe { read(request) }?;
        host.active(true)?;
        let mut state = host.state.try_lock().map_err(state_error)?;
        let slot = state
            .tickets
            .iter()
            .position(|entry| entry.is_some_and(|(id, _)| id == request.text_ticket))
            .ok_or(Status::InvalidTicket)?;
        let (_, ticket) = state.tickets[slot].ok_or(Status::InvalidTicket)?;
        host.port.text_open(ticket)?;
        if !state
            .payload
            .as_ref()
            .is_some_and(|(id, _)| *id == request.text_ticket)
        {
            return Err(Status::InvalidTicket.into());
        }
        host.surface.finish_payload(ticket, &mut state.payload)?;
        state.tickets[slot] = None;
        Ok(())
    })
}

/// # Safety
/// 输出遵守完整布局合同；查询只返回原 binding 的待结算 fence，不授予新输入权。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guossh_normal_v1_text_end_query(
    context: *const Context,
    result: *mut TextEnd,
) -> u32 {
    boundary(|| {
        let host = unsafe { get(context) }?;
        unsafe { output(result) }?;
        let _state = match host.state.try_lock() {
            Ok(state) => state,
            Err(TryLockError::WouldBlock) => return Err(Status::Busy.into()),
            Err(TryLockError::Poisoned(error)) => error.into_inner(),
        };
        let end = host.port.text_end(host.surface.binding())?;
        unsafe {
            result.write(TextEnd {
                header: header::<TextEnd>(),
                context_id: end.map_or(0, |end| end.context_id),
                transaction_id: end.map_or(0, |end| end.transaction_id),
                end_fence: end.map_or(0, |end| end.end_fence),
            })
        };
        Ok(())
    })
}

/// # Safety
/// 原 context 的 callbacks、pending locks 与 COM 确已静默；S_OK 或超时不是证明。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guossh_normal_v1_text_end_ack(
    context: *const Context,
    request: *const TextEnd,
) -> u32 {
    boundary(|| {
        let host = unsafe { get(context) }?;
        let request = unsafe { read(request) }?;
        let mut state = match host.state.try_lock() {
            Ok(state) => state,
            Err(TryLockError::WouldBlock) => return Err(Status::Busy.into()),
            Err(TryLockError::Poisoned(error)) => {
                host.cancel_poisoned(error.into_inner())?;
                host.surface.text_end_ack(port::TextEnd {
                    binding: host.surface.binding(),
                    context_id: request.context_id,
                    transaction_id: request.transaction_id,
                    end_fence: request.end_fence,
                })?;
                return Ok(());
            }
        };
        host.surface.text_end_ack(port::TextEnd {
            binding: host.surface.binding(),
            context_id: request.context_id,
            transaction_id: request.transaction_id,
            end_fence: request.end_fence,
        })?;
        for slot in &mut state.tickets {
            if slot.is_some_and(|(id, _)| id == request.transaction_id) {
                *slot = None;
            }
        }
        let payload = if state
            .payload
            .as_ref()
            .is_some_and(|(id, _)| *id == request.transaction_id)
        {
            state.payload.take()
        } else {
            None
        };
        drop(state);
        drop(payload);
        Ok(())
    })
}

/// # Safety
/// 原宿主已经实际呈现对应帧；所有指针遵守完整 C 布局和调用寿命合同。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guossh_normal_v1_presented_geometry(
    context: *const Context,
    request: *const PresentedGeometry,
) -> u32 {
    boundary(|| {
        let host = unsafe { get(context) }?;
        let request = unsafe { read(request) }?;
        host.active(false)?;
        host.surface
            .presented_geometry(request.native_request_id, request.geometry)?;
        Ok(())
    })
}

/// # Safety
/// 布局/DPI/缩放变化先调用此入口；不以 frame ACK 代替呈现几何证据。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guossh_normal_v1_invalidate_geometry(
    context: *const Context,
    native_request_id: u64,
) -> u32 {
    boundary(|| {
        let host = unsafe { get(context) }?;
        host.active(false)?;
        host.surface.invalidate_geometry(native_request_id)?;
        Ok(())
    })
}

/// # Safety
/// 命中与 modifiers/buttons 来自原 HWND 的实际事件及已确认呈现帧。Busy 保留原请求。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guossh_normal_v1_pointer(
    context: *const Context,
    request: *const Pointer,
    result: *mut Admission,
) -> u32 {
    boundary(|| {
        let host = unsafe { get(context) }?;
        let request = unsafe { read(request) }?;
        unsafe { output(result) }?;
        host.active(true)?;
        let admitted = host
            .surface
            .pointer(request.native_request_id, request.input)?;
        unsafe {
            result.write(Admission {
                header: header::<Admission>(),
                effect: admitted.effect,
                text_ticket: 0,
                route: 1,
                cycle_id: 0,
                text_op: admitted.text_op as u32,
                end_fence: admitted.end.map_or(0, |end| end.end_fence),
                end_context_id: admitted.end.map_or(0, |end| end.context_id),
                end_transaction_id: admitted.end.map_or(0, |end| end.transaction_id),
            })
        };
        Ok(())
    })
}

/// # Safety
/// 指针须有效。动作仅准入现有原会话队列，返回 effect 不代表 GUI 完成。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guossh_normal_v1_action(
    context: *const Context,
    request: *const ActionRequest,
    effect: *mut u64,
) -> u32 {
    boundary(|| {
        let host = unsafe { get(context) }?;
        let request = unsafe { read(request) }?;
        span(effect, 1)?;
        host.active(true)?;
        let seq = host
            .surface
            .action(request.request_id, action(request.action)?)?;
        unsafe { effect.write(seq) };
        Ok(())
    })
}

/// # Safety
/// 指针须有效。claim 绑定原目标，持有真实 ActionClaim 租约直到 ACK 或 abort。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guossh_normal_v1_claim(
    context: *const Context,
    request: *const ClaimRequest,
    result: *mut ClaimInfo,
) -> u32 {
    boundary(|| {
        let host = unsafe { get(context) }?;
        let request = unsafe { read(request) }?;
        unsafe { output(result) }?;
        host.active(true)?;
        let mut state = host.state.try_lock().map_err(state_error)?;
        if state.claim.is_some() {
            return Err(Status::Duplicate.into());
        }
        let claim = host.surface.claim_action(request.effect)?;
        let len = claim
            .value
            .copy_text
            .as_ref()
            .map_or(0, |text| text.text.len());
        let info = ClaimInfo {
            header: header::<ClaimInfo>(),
            claim: claim.seq,
            effect: claim.seq,
            action: action_id(claim.value.action),
            rejection: claim
                .value
                .rejection
                .map_or(0, |reject| Error::from(reject).0 as u32),
            payload_len: u64::try_from(len).map_err(|_| Status::TooLarge)?,
        };
        state.claim = Some(claim);
        unsafe { result.write(info) };
        Ok(())
    })
}

/// # Safety
/// buffer 必须可写 capacity 字节；count 可写。query 使用 null/0；短 buffer 不截断且不改 count。
/// payload 来自同一个 claim 的完整原文本，撤权后仍只允许原 claim 结算，不查询 current pane。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guossh_normal_v1_action_payload(
    context: *const Context,
    request: *const PayloadCopy,
    count: *mut u64,
) -> u32 {
    boundary(|| {
        let host = unsafe { get(context) }?;
        let request = unsafe { read(request) }?;
        span(count, 1)?;
        let capacity = span(request.buffer, request.capacity)?;
        if capacity != 0 {
            let start = request.buffer as usize;
            let end = start.checked_add(capacity).ok_or(Status::InvalidArgument)?;
            let count_start = count as usize;
            let count_end = count_start
                .checked_add(std::mem::size_of::<u64>())
                .ok_or(Status::InvalidArgument)?;
            if start < count_end && count_start < end {
                return Err(Status::InvalidArgument.into());
            }
        }
        let state = host.state.try_lock().map_err(state_error)?;
        let claim = state
            .claim
            .as_ref()
            .filter(|claim| claim.seq == request.claim)
            .ok_or(Status::InvalidTicket)?;
        let text = claim
            .value
            .copy_text
            .as_ref()
            .map_or("", |text| text.text.as_str());
        let count_value = u64::try_from(text.len()).map_err(|_| Status::TooLarge)?;
        if request.buffer.is_null() && capacity == 0 {
            unsafe { count.write(count_value) };
            return Ok(());
        }
        if capacity < text.len() {
            return Err(Status::BufferTooSmall.into());
        }
        if !text.is_empty() {
            unsafe { std::ptr::copy(text.as_ptr(), request.buffer, text.len()) };
        }
        unsafe { count.write(count_value) };
        Ok(())
    })
}

/// # Safety
/// Completed 只能在原实际消费者完成后调用；Paste 是原消费者实际读到的文本，
/// Cancelled 是明确 abort。ACK 不验证新焦点、不产生新 effect；指针须有效。
/// poisoned 状态只允许明确 abort，并封口、取消原 Surface 的全部残余责任。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guossh_normal_v1_ack(context: *const Context, request: *const Ack) -> u32 {
    boundary(|| {
        let host = unsafe { get(context) }?;
        let request = unsafe { read(request) }?;
        if request.kind == 3 {
            host.active(true)?;
        }
        let reply = match request.kind {
            1 if request.text_len == 0 => ActionReply::Completed,
            2 if request.text_len == 0 => ActionReply::Cancelled,
            3 => ActionReply::Paste(
                unsafe { utf8(request.text, request.text_len, INPUT_BYTES) }?.to_owned(),
            ),
            _ => return Err(Status::InvalidEvent.into()),
        };
        let mut state = match host.state.try_lock() {
            Ok(state) => state,
            Err(TryLockError::WouldBlock) => return Err(Status::Busy.into()),
            Err(TryLockError::Poisoned(error)) => {
                if request.kind != 2 {
                    return Err(Status::Internal.into());
                }
                return host.cancel_poisoned(error.into_inner());
            }
        };
        let claim = state
            .claim
            .as_ref()
            .filter(|claim| claim.seq == request.claim)
            .ok_or(Status::InvalidTicket)?;
        claim.ack(reply)?;
        let completed = state.claim.take();
        drop(state);
        drop(completed);
        Ok(())
    })
}

/// # Safety
/// 输出完整布局须可写。仅查询已发布 RenderFrame；caret 是 viewport 相对像素格坐标。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guossh_normal_v1_projection(
    context: *const Context,
    result: *mut Projection,
) -> u32 {
    boundary(|| {
        let host = unsafe { get(context) }?;
        unsafe { output(result) }?;
        host.active(false)?;
        // 投影没有输入准入操作，单独核验当前绑定；输入调用由 Surface 原子核验。
        let attached = host.port.attach(host.surface.binding())?;
        if !Arc::ptr_eq(&attached, &host.surface) {
            return Err(Status::StaleSource.into());
        }
        let published = host.port.projection_result()?.ok_or(Status::NotAttached)?;
        if published.source.source != host.surface.source().source
            || published.source.identity != host.surface.source().identity
        {
            return Err(Status::StaleSource.into());
        }
        let frame = published.frame();
        let selection = published.selection;
        let caret = frame.cursor.as_ref().filter(|cursor| {
            frame
                .rows
                .iter()
                .any(|row| row.stable_row == cursor.position.stable_row)
        });
        let (x, y, width, height) = if let Some(cursor) = caret {
            if frame.size.cols == 0 || frame.size.rows == 0 {
                return Err(Status::InvalidEvent.into());
            }
            let width = f64::from(frame.size.pixel_width) / f64::from(frame.size.cols);
            let height = f64::from(frame.size.pixel_height) / f64::from(frame.size.rows);
            let row = cursor
                .position
                .stable_row
                .checked_sub(frame.viewport_top)
                .ok_or(Status::InvalidEvent)?;
            (
                f64::from(cursor.position.column) * width,
                row as f64 * height,
                width,
                height,
            )
        } else {
            (0.0, 0.0, 0.0, 0.0)
        };
        let projection = Projection {
            header: header::<Projection>(),
            frame_revision: published.frame_revision,
            applied_effect: published.applied_effect,
            row_revision: published.row_revision,
            follow_bottom: u32::from(published.follow_bottom),
            first_stable_row: published.bounds.first_stable_row,
            screen_top_stable_row: published.bounds.bottom_top_stable_row,
            viewport_top: frame.viewport_top,
            mouse_reporting: u32::from(published.mouse_reporting),
            bracketed_paste: u32::from(published.modes.bracketed_paste),
            selection_present: u32::from(selection.is_some()),
            start_row: selection.map_or(0, |value| value.start.stable_row),
            start_col: selection.map_or(0, |value| u32::from(value.start.column)),
            end_row: selection.map_or(0, |value| value.end.stable_row),
            end_col: selection.map_or(0, |value| u32::from(value.end.column)),
            rectangular: u32::from(selection.is_some_and(|value| value.rectangular)),
            caret_present: u32::from(caret.is_some()),
            caret_x: x,
            caret_y: y,
            caret_width: width,
            caret_height: height,
            dpi: frame.size.dpi,
        };
        unsafe { result.write(projection) };
        Ok(())
    })
}

/// # Safety
/// context 有效。只封口原输入 surface，不 Disconnect、不 join actor、不 unload hub。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guossh_normal_v1_revoke(context: *const Context) -> u32 {
    boundary(|| {
        let host = unsafe { get(context) }?;
        match host.state.try_lock() {
            Ok(mut state) => {
                host.seal()?;
                let payload = state.payload.take();
                drop(state);
                drop(payload);
            }
            Err(TryLockError::WouldBlock) => return Err(Status::Busy.into()),
            Err(TryLockError::Poisoned(error)) => {
                host.cancel_poisoned(error.into_inner())?;
            }
        }
        Ok(())
    })
}

/// # Safety
/// 受信宿主已完成这个 handle 的实际 key/IME/COM callback 退休且不会再调用它；
/// 单纯自报数字/调用 revoke 不能证明退休。必须先封口；健康 claim 先由实际消费者 ACK 或明确 abort。
/// poisoned 状态的最终退休请求明确取消半写状态的残余责任，不恢复准入。
/// 移除登记不释放正在执行调用的原 Surface Arc；剩余 callback clone 仍只能得到已封口的原目标。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guossh_normal_v1_release_retired(context: *const Context) -> u32 {
    boundary(|| {
        let host = unsafe { get(context) }?;
        let context = unsafe { read(context) }?;
        if !host.revoked.load(Ordering::Acquire) {
            return Err(Status::NotAttached.into());
        }
        match host.state.try_lock() {
            Ok(state) if state.claim.is_some() => return Err(Status::Busy.into()),
            Ok(_) => {}
            Err(TryLockError::WouldBlock) => return Err(Status::Busy.into()),
            Err(TryLockError::Poisoned(error)) => {
                host.cancel_poisoned(error.into_inner())?;
            }
        }
        if host.port.text_end(host.surface.binding())?.is_some() {
            return Err(Status::Busy.into());
        }
        let old = hosts()
            .try_lock()
            .map_err(|_| Status::Busy)?
            .remove(&context.handle)
            .ok_or(Status::UnknownHandle)?;
        drop(old);
        Ok(())
    })
}

#[cfg(test)]
pub(super) mod test_support {
    use super::{
        Arc, AtomicBool, Context, Error, Header, INPUT_EVENTS, InputPort, Layout, Mutex,
        NativeHost, State, Status, header, insert, platform, port,
    };

    pub(crate) fn attach(port: Arc<InputPort>) -> Result<u64, Error> {
        let surface = unsafe { port.attach_verified(port.binding().attempt) }?;
        let budget =
            port::reserve_output(std::mem::size_of::<NativeHost>() + 64).ok_or(Status::Busy)?;
        insert(Arc::new(NativeHost {
            surface,
            port,
            platform: platform::Host::for_local_test(),
            revoked: AtomicBool::new(false),
            state: Mutex::new(State {
                tickets: [None; INPUT_EVENTS],
                claim: None,
                payload: None,
            }),
            _budget: budget,
            local_test: true,
        }))
    }

    pub(crate) fn context(port: &Arc<InputPort>, handle: u64) -> Context {
        let binding = port.binding();
        Context {
            header: header::<Context>(),
            handle,
            session_id: port.source.source.session_id,
            generation: port.source.source.generation,
            identity: port.source.identity.as_ptr(),
            identity_len: port.source.identity.len() as u64,
            attempt: binding.attempt,
            focus_epoch: binding.epoch,
        }
    }

    pub(crate) fn initialized<T: Layout>() -> T {
        // 测试布局均为整数、浮点及裸指针；零初始化不会构造引用或非零枚举。
        let mut value: T = unsafe { std::mem::zeroed() };
        unsafe { (&mut value as *mut T).cast::<Header>().write(header::<T>()) };
        value
    }

    pub(crate) fn with_state(context: &Context, body: impl FnOnce()) -> Result<(), Error> {
        let host = unsafe { super::get(context) }?;
        let _state = host.state.lock().map_err(|_| Status::Internal)?;
        body();
        Ok(())
    }

    pub(crate) fn state_poisoned(context: &Context) -> Result<bool, Error> {
        let host = unsafe { super::get(context) }?;
        Ok(host.state.is_poisoned())
    }
}
