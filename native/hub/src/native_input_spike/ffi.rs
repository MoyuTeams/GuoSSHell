//! app-private ABI；有效指针、实际缓冲区范围和同一原生事件的可信来源是宿主合约。
//! 检查数值不能证明 OS provenance；尚未接入真实宿主，不能因此声称 TSF／IME 成立。

use std::{
    ffi::c_void,
    mem::{align_of, size_of},
    num::NonZeroU64,
    panic::{AssertUnwindSafe, catch_unwind},
    pin::Pin,
    sync::{Arc, Mutex, OnceLock, TryLockError, atomic::Ordering},
    task::{Context, Poll, Waker},
};

use rshell_m0::rshell_core::{KeyCode, KeyModifiers};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use super::{
    core::{
        BUSY, Binding, DISABLED, EXHAUSTED, INVALID, INVALID_SOURCE, MODES, OK, Owner, STALE_OWNER,
        STOPPED, STREAM_BYTES, TRANSPORT_FAILURE, UNSUPPORTED,
    },
    transport,
};
use crate::{
    desktop_input_policy::{
        AltProvenance, PolicyKeyInput, RawChord, TextOwnership, VerifiedKeyCodes,
    },
    signals::desktop_session::{DesktopAction, DesktopKeyPhase},
};

pub(super) const VERSION: u32 = 3;
pub(super) const FIXTURE_NAME: &[u8] = b"native-input-fixture-v1";
const REGISTRATIONS: usize = 8;
const LIVE_WORKERS: usize = 2;
static HUB_INSTANCE: u8 = 0;
static OWNERS: [OnceLock<Arc<Owner>>; REGISTRATIONS] = [const { OnceLock::new() }; REGISTRATIONS];
static FACTORY: Mutex<u64> = Mutex::new(1);

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SpikeOwner {
    pub handle: u64,
    pub native_context: *const c_void,
    pub session: u32,
    pub reserved: u32,
    pub generation: u64,
    pub identity: [u8; 16],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct SpikeOpen {
    pub version: u32,
    pub size: u32,
    pub explicit_opt_in: u32,
    pub reserved: u32,
    pub native_context: *const c_void,
    pub fixture_name: *const u8,
    pub fixture_name_len: usize,
    pub origin: u32,
    pub platform: u32,
    pub registration: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct SpikeKey {
    pub version: u32,
    pub size: u32,
    pub physical: u64,
    pub phase: u32,
    pub raw_source: u32,
    pub raw_kind: u32,
    pub raw_value: u32,
    pub dual_source: u32,
    pub code_kind: u32,
    pub code_value: u32,
    pub legacy_kind: u32,
    pub legacy_value: u32,
    pub modifiers: u32,
    pub alt_source: u32,
    pub alt_sides: u32,
    pub text_ownership: u32,
    pub default_action: u32,
    pub native_event: u64,
    pub layout_revision: u64,
    pub same_event_state: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct SpikeAdmission {
    pub version: u32,
    pub size: u32,
    pub status: u32,
    pub disposition: u32,
    pub seq: u64,
    pub action: u32,
    pub byte_len: u32,
    pub effect: u64,
    pub ticket: super::payload::TicketRef,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct SpikeSnapshot {
    pub version: u32,
    pub size: u32,
    pub status: u32,
    pub owners: u32,
    pub hub_instance: u64,
    pub engine_instance: u64,
    pub remote_applied: u64,
    pub next_seq: u64,
    pub charged_slots: u32,
    pub charged_bytes: u32,
    pub inflight: u32,
    pub stopped: u32,
    // CPU/transport 已终止；已 claim action 仍需原消费者 ACK，不能代替 native retirement。
    // 不代表 runtime/TLS／线程尾退出。
    pub worker_done: u32,
    // 仅真实管理 join 后为 1；不授权卸载整个动态 hub。
    pub worker_joined: u32,
    pub last_written_seq: u64,
    pub retained_bytes: u64,
    pub applied_effect: u64,
    pub frame_revision: u64,
    pub source_origin: u32,
    pub sync_state: u32,
    pub sync_epoch: u64,
    pub sync_wakes: u64,
    pub remote_received: u64,
    pub core_revoked: u32,
    pub native_retired: u32,
    pub scope_delta: u64,
    pub root_scope_delta: u64,
    pub profile_revision: u64,
    pub cell_width: u32,
    pub cell_height: u32,
}

pub(super) fn guarded(f: impl FnOnce() -> u32) -> u32 {
    // 禁止 unwind 跨 C 边界；数值错误不制造 String／Vec／错误 peer。
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(STOPPED)
}

pub(super) fn guarded_owner(owner: &Owner, f: impl FnOnce() -> u32) -> u32 {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(status) => status,
        Err(_) => {
            owner.stop();
            STOPPED
        }
    }
}

pub(super) fn valid_pointer<T>(ptr: *const T) -> bool {
    !ptr.is_null() && (ptr as usize).is_multiple_of(align_of::<T>())
}

/// # Safety
/// 调用者须保证非空指针指向对齐、可读且在整个调用中不变的 T。
pub(super) unsafe fn borrowed<T: Copy>(ptr: *const T) -> Result<T, u32> {
    if !valid_pointer(ptr) {
        return Err(INVALID);
    }
    // SAFETY: 范围与生命周期由私有 ABI caller contract 保证；不保存调用者指针。
    Ok(unsafe { ptr.read() })
}

pub(super) fn capability(owner: &Owner) -> SpikeOwner {
    let b = owner.binding;
    SpikeOwner {
        handle: b.handle,
        native_context: b.context as *const c_void,
        session: b.session,
        reserved: 0,
        generation: b.generation,
        identity: b.identity,
    }
}

pub(super) fn lookup(cap: SpikeOwner) -> Result<&'static Owner, u32> {
    let index = cap
        .handle
        .checked_sub(1)
        .and_then(|i| usize::try_from(i).ok())
        .ok_or(STALE_OWNER)?;
    let owner = OWNERS
        .get(index)
        .and_then(OnceLock::get)
        .ok_or(STALE_OWNER)?;
    // 没有 source 数值登记入口；nonce、source 与原生对象全由唯一工厂绑定。
    if cap != capability(owner) {
        return Err(INVALID_SOURCE);
    }
    Ok(owner)
}

pub(super) fn registration(generation: u64) -> Result<(usize, u64), u32> {
    let next = generation.checked_add(1).ok_or(EXHAUSTED)?;
    let index = generation
        .checked_sub(1)
        .and_then(|g| usize::try_from(g).ok())
        .ok_or(INVALID)?;
    if index >= REGISTRATIONS {
        return Err(DISABLED);
    }
    Ok((index, next))
}

pub(super) fn decode(key: SpikeKey) -> Result<PolicyKeyInput, u32> {
    if key.version != VERSION
        || key.size as usize != size_of::<SpikeKey>()
        || key.modifiers & !15 != 0
        || key.raw_source > 2
        || key.dual_source > 1
        || key.alt_source > 1
        || key.alt_sides > 3
        || key.text_ownership > 2
        || key.native_event == 0
        || key.layout_revision != 1
    {
        return Err(INVALID);
    }
    let modifiers = KeyModifiers {
        shift: key.modifiers & 1 != 0,
        control: key.modifiers & 2 != 0,
        alt: key.modifiers & 4 != 0,
        super_key: key.modifiers & 8 != 0,
    };
    let raw_chord = if key.raw_source == 1 {
        Some(RawChord {
            code: code(key.raw_kind, key.raw_value)?,
            modifiers,
        })
    } else {
        if key.raw_kind != 0 || key.raw_value != 0 {
            return Err(INVALID);
        }
        // Unknown／已知域外都不制造 Some，不放行默认候选或原生文本。
        None
    };
    let dual = if key.dual_source == 1 {
        Some(VerifiedKeyCodes {
            code: code(key.code_kind, key.code_value)?,
            legacy_code: code(key.legacy_kind, key.legacy_value)?,
        })
    } else {
        if key.code_kind != 0
            || key.code_value != 0
            || key.legacy_kind != 0
            || key.legacy_value != 0
        {
            return Err(INVALID);
        }
        None
    };
    let alt_provenance = if key.alt_source == 1 {
        AltProvenance::Known {
            left: key.alt_sides & 1 != 0,
            right: key.alt_sides & 2 != 0,
        }
    } else {
        if key.alt_sides != 0 {
            return Err(INVALID);
        }
        AltProvenance::Unknown
    };
    Ok(PolicyKeyInput {
        raw_chord,
        dual,
        phase: phase(key.phase)?,
        physical_identity: NonZeroU64::new(key.physical),
        alt_provenance,
        text_ownership: match key.text_ownership {
            0 => TextOwnership::Unverified,
            1 => TextOwnership::TerminalKey,
            2 => TextOwnership::NativeText,
            _ => return Err(INVALID),
        },
        default_action: action(key.default_action)?,
    })
}

pub(super) fn phase(value: u32) -> Result<DesktopKeyPhase, u32> {
    match value {
        0 => Ok(DesktopKeyPhase::Down),
        1 => Ok(DesktopKeyPhase::Repeat),
        2 => Ok(DesktopKeyPhase::Up),
        _ => Err(INVALID),
    }
}

fn code(kind: u32, value: u32) -> Result<KeyCode, u32> {
    if kind != 1 && kind != 16 && value != 0 {
        return Err(INVALID);
    }
    Ok(match kind {
        1 => KeyCode::Character(char::from_u32(value).ok_or(INVALID)?),
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
        16 if (1..=24).contains(&value) => KeyCode::F(value as u8),
        _ => return Err(INVALID),
    })
}

pub(super) fn action(value: u32) -> Result<Option<DesktopAction>, u32> {
    Ok(Some(match value {
        0 => return Ok(None),
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
        _ => return Err(INVALID),
    }))
}

/// # Safety
/// request/out 为有效借用；native_context 须是宿主实际拥有、窗口内终端限定、
/// 不重绑的原生对象。此函数只能由显式 fixture 宿主调用，不能注册 Dart 的数字 token。
/// origin=1 明确声明合成 fixture；origin=2 仅限 native builder 已核验的 source-owned
/// surface，platform/registration 属于同一实际 UI context。数值校验不是 OS provenance
/// 证明，不能把 PostMessage 或一个非空指针升级为 physical/native-text 验收。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guossh_native_input_spike_open(
    request: *const SpikeOpen,
    out: *mut SpikeOwner,
) -> u32 {
    guarded(|| {
        // SAFETY: 本入口的借用合约。
        let request = match unsafe { borrowed(request) } {
            Ok(r) => r,
            Err(e) => return e,
        };
        if request.version != VERSION
            || request.size as usize != size_of::<SpikeOpen>()
            || request.reserved != 0
            || request.explicit_opt_in > 1
        {
            return INVALID;
        }
        if request.explicit_opt_in == 0 {
            return DISABLED;
        }
        if !(1..=2).contains(&request.origin)
            || request.registration == 0
            || (request.origin == 1 && request.platform != 0)
            || (request.origin == 2
                && request.platform
                    != if cfg!(target_os = "windows") {
                        1
                    } else if cfg!(target_os = "macos") {
                        2
                    } else {
                        3
                    })
        {
            return INVALID_SOURCE;
        }
        if !valid_pointer(out)
            || request.native_context.is_null()
            || request.fixture_name.is_null()
            || request.fixture_name_len != FIXTURE_NAME.len()
        {
            return INVALID;
        }
        // SAFETY: caller 保证整个借用范围可读；先检查精确长度，再借用，不复制到堆。
        let name =
            unsafe { std::slice::from_raw_parts(request.fixture_name, request.fixture_name_len) };
        if std::str::from_utf8(name).is_err() || name != FIXTURE_NAME {
            return INVALID;
        }
        let mut generation = match FACTORY.try_lock() {
            Ok(g) => g,
            Err(TryLockError::WouldBlock) => return BUSY,
            Err(TryLockError::Poisoned(_)) => return STOPPED,
        };
        let (index, next) = match registration(*generation) {
            Ok(r) => r,
            Err(e) => return e,
        };
        if OWNERS
            .iter()
            .filter_map(OnceLock::get)
            .any(|o| o.binding.context == request.native_context as usize)
        {
            return INVALID_SOURCE;
        }
        if OWNERS
            .iter()
            .filter_map(OnceLock::get)
            .filter(|o| !o.worker_joined.load(Ordering::Acquire))
            .count()
            >= LIVE_WORKERS
        {
            return BUSY;
        }
        let (stream, peer) = tokio::io::duplex(STREAM_BYTES);
        let binding = Binding {
            handle: *generation,
            context: request.native_context as usize,
            session: 0xfa00_0000 + index as u32,
            generation: *generation,
            identity: *uuid::Uuid::new_v4().as_bytes(),
            ui_thread: std::thread::current().id(),
        };
        let owner = match Owner::new(binding, peer) {
            Ok(o) => o,
            Err(e) => return e,
        };
        if let Ok(mut core) = owner.core.lock() {
            core.origin = if request.origin == 1 {
                super::core::Origin::DeclaredSynthetic(request.registration)
            } else {
                super::core::Origin::VerifiedNative {
                    platform: request.platform,
                    registration: request.registration,
                }
            };
        } else {
            return STOPPED;
        }
        // 初始化所有固定资源成功后才发布 owner；不访问 Rinf 输入订阅或生产工厂。
        if let Err(e) = transport::start(Arc::clone(&owner), stream) {
            owner.stop();
            return e;
        }
        let cap = capability(&owner);
        if OWNERS[index].set(owner).is_err() {
            return EXHAUSTED;
        }
        *generation = next;
        // SAFETY: out 的有效、独占可写范围由 caller 保证。
        unsafe { out.write(cap) };
        OK
    })
}

/// # Safety
/// cap/key/out 在调用中有效；raw/dual 的 Trusted 只能来自同一实际原生事件。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guossh_native_input_spike_key(
    cap: *const SpikeOwner,
    key: *const SpikeKey,
    out: *mut SpikeAdmission,
) -> u32 {
    guarded(|| {
        if !valid_pointer(out) {
            return INVALID;
        }
        // SAFETY: 私有入口借用合约；先校验不可变来源，再处理 key，无 Owned copy。
        let owner = match unsafe { borrowed(cap) }.and_then(lookup) {
            Ok(o) => o,
            Err(e) => return e,
        };
        let status = guarded_owner(owner, || {
            let key = match unsafe { borrowed(key) } {
                Ok(k) => k,
                Err(e) => {
                    owner.stop();
                    return e;
                }
            };
            let (Some(id), Ok(phase)) = (NonZeroU64::new(key.physical), phase(key.phase)) else {
                owner.stop();
                return INVALID;
            };
            let admission = owner.key(id, phase, decode(key));
            let result = SpikeAdmission {
                version: VERSION,
                size: size_of::<SpikeAdmission>() as u32,
                status: admission.status,
                disposition: admission.disposition,
                seq: admission.seq,
                action: admission.action,
                byte_len: admission.bytes,
                effect: admission.effect,
                ticket: admission.ticket,
            };
            // SAFETY: out 的独占借用合约。
            unsafe { out.write(result) };
            admission.status
        });
        if status == STOPPED {
            // SAFETY: out 已通过本入口有效、独占可写的 caller contract。
            unsafe {
                out.write(SpikeAdmission {
                    version: VERSION,
                    size: size_of::<SpikeAdmission>() as u32,
                    status: STOPPED,
                    ..SpikeAdmission::default()
                })
            };
        }
        status
    })
}

/// # Safety
/// cap 在调用中是有效借用。撤权不等待 worker、I/O 或宿主回调。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guossh_native_input_spike_revoke(cap: *const SpikeOwner) -> u32 {
    guarded(|| {
        // SAFETY: cap 借用合约。
        match unsafe { borrowed(cap) }.and_then(lookup) {
            Ok(owner) => {
                owner.stop();
                OK
            }
            Err(e) => e,
        }
    })
}

/// # Safety
/// cap 是有效借用；只能由非绑定 UI 线程的专用 fixture 管理调用。
/// 先 revoke，再 join；可等待实际线程退出，不能从 native callback/UI/actor 调用。
/// join 只归还固定 worker 名额，不释放永久 owner/context，也不授权卸载整个 hub。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guossh_native_input_spike_join(cap: *const SpikeOwner) -> u32 {
    guarded(|| {
        // SAFETY: cap 借用合约；未知／伪造来源不捏造 owner。
        match unsafe { borrowed(cap) }.and_then(lookup) {
            Ok(owner) => guarded_owner(owner, || owner.join_worker()),
            Err(e) => e,
        }
    })
}

/// # Safety
/// cap/out 是有效借用；cap=null 只查询默认隔离状态，不创建 owner／worker。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guossh_native_input_spike_inspect(
    cap: *const SpikeOwner,
    out: *mut SpikeSnapshot,
) -> u32 {
    guarded(|| {
        if !valid_pointer(out) {
            return INVALID;
        }
        let mut snapshot = SpikeSnapshot {
            version: VERSION,
            size: size_of::<SpikeSnapshot>() as u32,
            status: DISABLED,
            owners: OWNERS
                .iter()
                .filter_map(OnceLock::get)
                .filter(|o| !o.stopped.load(Ordering::Acquire))
                .count() as u32,
            hub_instance: (&HUB_INSTANCE as *const u8) as usize as u64,
            ..SpikeSnapshot::default()
        };
        if !cap.is_null() {
            // SAFETY: 非空 cap 的借用合约。
            let owner = match unsafe { borrowed(cap) }.and_then(lookup) {
                Ok(o) => o,
                Err(e) => return e,
            };
            snapshot.status = guarded_owner(owner, || {
                let poisoned = owner.poisoned();
                if poisoned {
                    owner.stop();
                }
                snapshot.stopped = u32::from(owner.stopped.load(Ordering::Acquire));
                snapshot.worker_done = u32::from(owner.worker_done.load(Ordering::Acquire));
                snapshot.worker_joined = u32::from(owner.worker_joined.load(Ordering::Acquire));
                let (core, status) = match owner.core.try_lock() {
                    Ok(c) => (c, if poisoned { STOPPED } else { OK }),
                    Err(TryLockError::WouldBlock) => return if poisoned { STOPPED } else { BUSY },
                    Err(TryLockError::Poisoned(e)) => {
                        owner.stop();
                        // 只读终结诊断，不恢复 engine/service，也不清除 poison。
                        (e.into_inner(), STOPPED)
                    }
                };
                let (charged, flight) = core.charge();
                snapshot.engine_instance = (&core.engine as *const _) as usize as u64;
                snapshot.remote_applied = core.remote_applied;
                snapshot.next_seq = core.next_seq;
                snapshot.charged_slots = charged;
                snapshot.charged_bytes = core.effects.window_bytes();
                snapshot.inflight = flight;
                snapshot.stopped = u32::from(owner.stopped.load(Ordering::Acquire));
                snapshot.worker_done = u32::from(owner.worker_done.load(Ordering::Acquire));
                snapshot.worker_joined = u32::from(owner.worker_joined.load(Ordering::Acquire));
                snapshot.last_written_seq = core.last_written_seq;
                snapshot.retained_bytes = core.effects.retained_bytes() as u64;
                snapshot.applied_effect = core.effects.applied;
                snapshot.frame_revision = core.effects.frame_revision;
                snapshot.source_origin = match core.origin {
                    super::core::Origin::DeclaredSynthetic(_) => 1,
                    super::core::Origin::VerifiedNative { .. } => 2,
                };
                snapshot.sync_state = match core.effects.sync.cell {
                    super::deadline::Cell::Idle => 0,
                    super::deadline::Cell::Armed { .. } => 1,
                    super::deadline::Cell::Waiting { .. } => 2,
                    super::deadline::Cell::Queued { .. } => 3,
                    super::deadline::Cell::Applying { .. } => 4,
                };
                snapshot.sync_epoch = core.effects.sync.epoch;
                snapshot.sync_wakes = core.effects.sync.wakes;
                snapshot.remote_received = core.effects.remote_received;
                snapshot.core_revoked = u32::from(core.effects.revoked);
                snapshot.native_retired = u32::from(core.effects.retired);
                snapshot.scope_delta = core.scope_budget.per_owner_delta;
                snapshot.root_scope_delta = core.scope_budget.root_delta;
                (
                    snapshot.profile_revision,
                    snapshot.cell_width,
                    snapshot.cell_height,
                ) = core.captured_geometry();
                status
            });
        }
        // SAFETY: out 的独占借用合约。
        unsafe { out.write(snapshot) };
        snapshot.status
    })
}

/// # Safety
/// cap 是有效借用；这是 fixture peer，不是 native 输入或生产控制。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guossh_native_input_spike_peer_mode(
    cap: *const SpikeOwner,
    command: u32,
) -> u32 {
    guarded(|| {
        // SAFETY: cap 借用合约。
        let owner = match unsafe { borrowed(cap) }.and_then(lookup) {
            Ok(o) => o,
            Err(e) => return e,
        };
        guarded_owner(owner, || {
            if owner.stopped.load(Ordering::Acquire) {
                return STOPPED;
            }
            let Some(bytes) = MODES.get(command as usize) else {
                return INVALID;
            };
            let mut frame = [0u8; super::core::READ_BYTES + 1];
            frame[0] = bytes.len() as u8;
            frame[1..=bytes.len()].copy_from_slice(bytes);
            let mut peer = match owner.peer.try_lock() {
                Ok(p) => p,
                Err(TryLockError::WouldBlock) => return BUSY,
                Err(TryLockError::Poisoned(_)) => {
                    owner.stop();
                    return STOPPED;
                }
            };
            if owner.stopped.load(Ordering::Acquire) {
                return STOPPED;
            }
            let mut cx = Context::from_waker(Waker::noop());
            match Pin::new(&mut *peer).poll_write(&mut cx, &frame[..bytes.len() + 1]) {
                Poll::Ready(Ok(n)) if n == bytes.len() + 1 => OK,
                Poll::Pending => BUSY,
                // 部分 framing 写不能回滚／重放；停止 fixture，而不是假成功。
                _ => {
                    owner.stop();
                    TRANSPORT_FAILURE
                }
            }
        })
    })
}

/// # Safety
/// cap/out/count 有效，out 独占且至少 capacity bytes；capacity 必须在 1..=128。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guossh_native_input_spike_peer_read(
    cap: *const SpikeOwner,
    out: *mut u8,
    capacity: usize,
    count: *mut usize,
) -> u32 {
    guarded(|| {
        if out.is_null() || !valid_pointer(count) || !(1..=STREAM_BYTES).contains(&capacity) {
            return INVALID;
        }
        // SAFETY: cap 借用合约；先验证来源，再借用输出缓冲，不持 core 锁做 I/O。
        let owner = match unsafe { borrowed(cap) }.and_then(lookup) {
            Ok(o) => o,
            Err(e) => return e,
        };
        guarded_owner(owner, || {
            let mut peer = match owner.peer.try_lock() {
                Ok(p) => p,
                Err(TryLockError::WouldBlock) => return BUSY,
                Err(TryLockError::Poisoned(_)) => {
                    owner.stop();
                    return STOPPED;
                }
            };
            // SAFETY: 私有 ABI 的独占可写范围合约，长度已限制。
            let bytes = unsafe { std::slice::from_raw_parts_mut(out, capacity) };
            let mut buf = ReadBuf::new(bytes);
            let mut cx = Context::from_waker(Waker::noop());
            match Pin::new(&mut *peer).poll_read(&mut cx, &mut buf) {
                Poll::Ready(Ok(())) => {
                    // SAFETY: count 的独占可写借用合约。
                    unsafe { count.write(buf.filled().len()) };
                    OK
                }
                Poll::Pending => {
                    unsafe { count.write(0) };
                    BUSY
                }
                Poll::Ready(Err(_)) => {
                    owner.stop();
                    TRANSPORT_FAILURE
                }
            }
        })
    })
}

/// # Safety
/// cap 是有效借用；1:pointer/2:text/3:paste 为 fixture 接缝；4:生产控制拒绝。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guossh_native_input_spike_support(
    cap: *const SpikeOwner,
    operation: u32,
) -> u32 {
    guarded(|| {
        if cap.is_null() {
            return DISABLED;
        }
        // SAFETY: cap 借用合约；这里绝不签发 dummy ticket 或放行 IME。
        match unsafe { borrowed(cap) }.and_then(lookup) {
            Ok(owner) if owner.stopped.load(Ordering::Acquire) => STOPPED,
            Ok(_) if matches!(operation, 1..=3) => OK,
            Ok(_) if operation == 4 => UNSUPPORTED,
            Ok(_) if operation == 5 => super::core::PRODUCTION_NOT_READY,
            Ok(_) if operation == 6 => super::core::SCOPED_TRANSPORT_UNAVAILABLE,
            Ok(_) => INVALID,
            Err(e) => e,
        }
    })
}
