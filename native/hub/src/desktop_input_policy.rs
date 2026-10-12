//! 捕获快照上的独立桌面输入策略；来源、顺序与租约仍由外层持有。

use std::num::NonZeroU64;

#[cfg(test)]
use rshell_m0::rshell_core::TerminalDisplayModes;
use rshell_m0::rshell_core::{
    KeyCode, KeyEventPhase, KeyModifiers, ResolvedTerminalProfile, TerminalKeyAction,
    TerminalKeyEvent, TerminalSendSequence, parse_terminal_key_action,
};
use rshell_m0::rshell_session::{EngineError, TerminalEngine, ViewportBounds};

use crate::signals::desktop_session::{DesktopAction, DesktopKeyPhase};

pub(crate) const MAX_ACTIVE_KEYS: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawChord {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

/// 适配器须证明两表示来自同一实际事件；这里仅检查结构，不推断布局。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VerifiedKeyCodes {
    pub code: KeyCode,
    pub legacy_code: KeyCode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AltProvenance {
    Known { left: bool, right: bool },
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextOwnership {
    /// 已证明排他走终端键路径；Ctrl 不是尚未分类的 AltGr 合成部分。
    TerminalKey,
    /// 已证明该物理事件归原生文本独占；不携带或制造提交文本。
    NativeText,
    Unverified,
}

/// Some 的 raw/physical 须由调用方证明可信；非零身份不能由 logical/chord 猜出。
#[derive(Debug, Clone)]
pub(crate) struct PolicyKeyInput {
    pub raw_chord: Option<RawChord>,
    pub dual: Option<VerifiedKeyCodes>,
    pub phase: DesktopKeyPhase,
    pub physical_identity: Option<NonZeroU64>,
    pub alt_provenance: AltProvenance,
    pub text_ownership: TextOwnership,
    /// 终端焦点路径已确认、但尚未执行的既有壳候选。
    pub default_action: Option<DesktopAction>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputReject {
    MissingPhysicalIdentity,
    MissingRawChord,
    InvalidRawChord,
    MissingDual,
    InvalidDual,
    InvalidAction,
    InconsistentAlt,
    AmbiguousAltProvenance,
    UnverifiedTextOwnership,
    DuplicateDown,
    UnassociatedPhase,
    ActiveKeyBudget,
    UnsupportedInput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoSendReason {
    EngineEmpty,
    ConsumedKey,
    InitialReject,
    NativeText,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum KeyResult {
    Bytes(Vec<u8>),
    NoSend(NoSendReason),
    TypedInputReject(InputReject),
    Action(DesktopAction),
    ClearScrollback,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct KeyOutcome {
    pub result: KeyResult,
    pub scroll_to_bottom: bool,
}

impl KeyOutcome {
    fn quiet(result: KeyResult) -> Self {
        Self {
            result,
            scroll_to_bottom: false,
        }
    }

    fn reject(reason: InputReject) -> Self {
        Self::quiet(KeyResult::TypedInputReject(reason))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    Terminal,
    Consumed,
    Native,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ActiveKey {
    identity: NonZeroU64,
    route: Route,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InputPolicyState {
    active: [Option<ActiveKey>; MAX_ACTIVE_KEYS],
}

impl Default for InputPolicyState {
    fn default() -> Self {
        Self {
            active: [None; MAX_ACTIVE_KEYS],
        }
    }
}

/// 不分配输出、不访问 engine；外层先在固定影子状态分类，预约成功后提交该状态。
pub(crate) enum GatedIntent {
    Quiet(KeyOutcome),
    Send {
        sequence: TerminalSendSequence,
        scroll: bool,
    },
    Encode {
        event: TerminalKeyEvent,
        scroll: bool,
    },
}

impl GatedIntent {
    fn quiet(result: KeyResult) -> Self {
        Self::Quiet(KeyOutcome::quiet(result))
    }

    fn reject(reason: InputReject) -> Self {
        Self::Quiet(KeyOutcome::reject(reason))
    }
}

/// 同一 owner 始终使用捕获快照；没有第二个绑定 matcher。
pub(crate) fn gate_key(
    state: &mut InputPolicyState,
    profile: &ResolvedTerminalProfile,
    input: PolicyKeyInput,
) -> GatedIntent {
    let Some(identity) = input.physical_identity else {
        return GatedIntent::reject(InputReject::MissingPhysicalIdentity);
    };
    let existing = state.active.iter().enumerate().find_map(|(slot, key)| {
        key.filter(|key| key.identity == identity)
            .map(|key| (slot, key.route))
    });
    let (slot, route) = match (input.phase, existing) {
        (DesktopKeyPhase::Down, Some(_)) => {
            return GatedIntent::reject(InputReject::DuplicateDown);
        }
        (DesktopKeyPhase::Down, None) => {
            let Some(slot) = state.active.iter().position(Option::is_none) else {
                return GatedIntent::reject(InputReject::ActiveKeyBudget);
            };
            state.active[slot] = Some(ActiveKey {
                identity,
                route: Route::Rejected,
            });
            (slot, Route::Rejected)
        }
        (_, Some(existing)) => existing,
        (_, None) => return GatedIntent::reject(InputReject::UnassociatedPhase),
    };

    if input.phase == DesktopKeyPhase::Up {
        state.active[slot] = None;
        match route {
            Route::Consumed => {
                return GatedIntent::quiet(KeyResult::NoSend(NoSendReason::ConsumedKey));
            }
            Route::Rejected => {
                return GatedIntent::quiet(KeyResult::NoSend(NoSendReason::InitialReject));
            }
            Route::Native => {
                return GatedIntent::quiet(KeyResult::NoSend(NoSendReason::NativeText));
            }
            Route::Terminal => {}
        }
    } else if input.phase == DesktopKeyPhase::Repeat && route == Route::Rejected {
        return GatedIntent::quiet(KeyResult::NoSend(NoSendReason::InitialReject));
    }

    let Some(raw) = input.raw_chord else {
        return GatedIntent::reject(InputReject::MissingRawChord);
    };
    if !valid_code(&raw.code) {
        return GatedIntent::reject(InputReject::InvalidRawChord);
    }

    if input.phase != DesktopKeyPhase::Up {
        if let Some(binding) = profile
            .key_bindings
            .iter()
            .find(|binding| binding.code == raw.code && binding.modifiers == raw.modifiers)
        {
            let action = match parse_terminal_key_action(&binding.action) {
                Ok(action) => action,
                Err(_) => {
                    if input.phase == DesktopKeyPhase::Repeat {
                        state.active[slot] = Some(ActiveKey {
                            identity,
                            route: Route::Consumed,
                        });
                    }
                    return GatedIntent::reject(InputReject::InvalidAction);
                }
            };
            state.active[slot] = Some(ActiveKey {
                identity,
                route: Route::Consumed,
            });
            let result = match action {
                TerminalKeyAction::Send(sequence) => {
                    return GatedIntent::Send {
                        sequence,
                        scroll: profile.scroll_on_keypress,
                    };
                }
                _ if input.phase == DesktopKeyPhase::Repeat => {
                    KeyResult::NoSend(NoSendReason::ConsumedKey)
                }
                TerminalKeyAction::ClearScrollback => KeyResult::ClearScrollback,
                TerminalKeyAction::NewTab => KeyResult::Action(DesktopAction::NewTab),
                TerminalKeyAction::SplitVertical => KeyResult::Action(DesktopAction::SplitRight),
            };
            return GatedIntent::quiet(result);
        }
        if matches!(route, Route::Consumed | Route::Native) {
            return GatedIntent::quiet(KeyResult::NoSend(if route == Route::Native {
                NoSendReason::NativeText
            } else {
                NoSendReason::ConsumedKey
            }));
        }
        if input.phase == DesktopKeyPhase::Down
            && let Some(action) = input.default_action
        {
            state.active[slot] = Some(ActiveKey {
                identity,
                route: Route::Consumed,
            });
            return GatedIntent::quiet(KeyResult::Action(action));
        }
    }

    let mut modifiers = raw.modifiers;
    modifiers.alt = match input.alt_provenance {
        AltProvenance::Known { left, right } => {
            if modifiers.alt != (left || right) {
                return GatedIntent::reject(InputReject::InconsistentAlt);
            }
            (left && profile.left_alt_as_meta) || (right && profile.right_alt_as_meta)
        }
        AltProvenance::Unknown => modifiers.alt,
    };
    match input.text_ownership {
        TextOwnership::NativeText => {
            if input.phase != DesktopKeyPhase::Up {
                state.active[slot] = Some(ActiveKey {
                    identity,
                    route: Route::Native,
                });
            }
            return GatedIntent::quiet(KeyResult::NoSend(NoSendReason::NativeText));
        }
        TextOwnership::Unverified => {
            let reason = if raw.modifiers.alt
                && (raw.modifiers.control || matches!(raw.code, KeyCode::Character(_)))
            {
                InputReject::AmbiguousAltProvenance
            } else {
                InputReject::UnverifiedTextOwnership
            };
            return GatedIntent::reject(reason);
        }
        TextOwnership::TerminalKey => {}
    }
    let Some(dual) = input.dual else {
        return GatedIntent::reject(InputReject::MissingDual);
    };
    if !valid_code(&dual.code)
        || !valid_code(&dual.legacy_code)
        || !(matches!(
            (&dual.code, &dual.legacy_code),
            (KeyCode::Character(_), KeyCode::Character(_))
        ) || dual.code == dual.legacy_code)
    {
        return GatedIntent::reject(InputReject::InvalidDual);
    }
    let phase = match input.phase {
        DesktopKeyPhase::Down => KeyEventPhase::Press,
        DesktopKeyPhase::Repeat => KeyEventPhase::Repeat,
        DesktopKeyPhase::Up => KeyEventPhase::Release,
    };
    if input.phase == DesktopKeyPhase::Down {
        state.active[slot] = Some(ActiveKey {
            identity,
            route: Route::Terminal,
        });
    }
    GatedIntent::Encode {
        event: TerminalKeyEvent {
            code: dual.code,
            legacy_code: dual.legacy_code,
            modifiers,
            phase,
        },
        scroll: input.phase != DesktopKeyPhase::Up && profile.scroll_on_keypress,
    }
}

/// 仅执行已分类意图；不读取 profile、不重新匹配 chord。
pub(crate) fn apply_intent(
    engine: &mut dyn TerminalEngine,
    intent: GatedIntent,
) -> Result<KeyOutcome, EngineError> {
    let (event, scroll) = match intent {
        GatedIntent::Quiet(outcome) => return Ok(outcome),
        GatedIntent::Send { sequence, scroll } => {
            return Ok(KeyOutcome {
                result: KeyResult::Bytes(sequence.as_str().as_bytes().to_vec()),
                scroll_to_bottom: scroll,
            });
        }
        GatedIntent::Encode { event, scroll } => (event, scroll),
    };
    let result = match engine.encode_key_event(event) {
        Ok(bytes) if bytes.is_empty() => KeyResult::NoSend(NoSendReason::EngineEmpty),
        Ok(bytes) => KeyResult::Bytes(bytes),
        Err(EngineError::UnsupportedInput(_)) => {
            return Ok(KeyOutcome::reject(InputReject::UnsupportedInput));
        }
        Err(error) => return Err(error),
    };
    Ok(KeyOutcome {
        result,
        scroll_to_bottom: scroll,
    })
}

fn valid_code(code: &KeyCode) -> bool {
    !matches!(code, KeyCode::F(number) if !(1..=24).contains(number))
}

// 非按键 effect 尚无原型消费者；真实接线前保留独立测试边界。
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Activity {
    Output,
    AcceptedTerminalKey,
    AcceptedText,
    AcceptedPaste,
    Other,
}

#[cfg(test)]
pub(crate) fn scroll_effect(profile: &ResolvedTerminalProfile, activity: Activity) -> bool {
    match activity {
        Activity::Output => profile.scroll_on_output,
        Activity::AcceptedTerminalKey | Activity::AcceptedText | Activity::AcceptedPaste => {
            profile.scroll_on_keypress
        }
        Activity::Other => false,
    }
}

#[cfg(test)]
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct MousePolicy {
    pub report_to_remote: bool,
    pub local_selection: bool,
}

#[cfg(test)]
pub(crate) fn mouse_policy(
    profile: &ResolvedTerminalProfile,
    modes: TerminalDisplayModes,
) -> MousePolicy {
    let report_to_remote = profile.mouse_reporting && modes.mouse_reporting;
    MousePolicy {
        report_to_remote,
        local_selection: !report_to_remote,
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ClearEffects {
    pub bounds: ViewportBounds,
    pub clear_selection: bool,
    pub invalidate_frame: bool,
    pub invalidate_row_ranges: bool,
    pub request_frame: bool,
}

/// 只有引擎清理成功才交付失效意图；Screen/Dart 的实际消费属于 4C。
pub(crate) fn clear_scrollback(
    engine: &mut dyn TerminalEngine,
) -> Result<ClearEffects, EngineError> {
    engine.clear_scrollback()?;
    Ok(ClearEffects {
        bounds: engine.viewport_bounds(),
        clear_selection: true,
        invalidate_frame: true,
        invalidate_row_ranges: true,
        request_frame: true,
    })
}

#[cfg(test)]
#[path = "desktop_input_policy_tests.rs"]
mod tests;
