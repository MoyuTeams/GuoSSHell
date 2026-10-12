use std::{num::NonZeroU64, sync::Arc};

use rshell_m0::rshell_core::{
    CellPosition, DisplayRecovery, KeyBinding, KeyCode, KeyModifiers, MouseButton, MouseEventKind,
    RenderFrame, ResolvedTerminalProfile, SearchMatch, SearchQuery, SelectionRange,
    TerminalDisplayModes, TerminalInput, TerminalMouseEvent, TerminalOverrides, TerminalSettingsV1,
    TerminalSize, Viewport,
};
use rshell_m0::rshell_session::{
    DefaultTerminalEngine, EngineDelta, EngineError, TerminalEngine, ViewportBounds,
};

use super::{
    ActiveKey, Activity, AltProvenance, ClearEffects, InputPolicyState, InputReject, KeyOutcome,
    KeyResult, MAX_ACTIVE_KEYS, NoSendReason, PolicyKeyInput, RawChord, Route, TextOwnership,
    VerifiedKeyCodes, apply_intent, clear_scrollback, gate_key, mouse_policy, scroll_effect,
};
use crate::desktop_runtime::{INPUT_BYTES, INPUT_EVENTS, InputLease, Protocol};
use crate::signals::desktop_session::{
    DesktopAction, DesktopAltSource, DesktopEvent, DesktopFailure, DesktopKeyPhase, DesktopOrdered,
    DesktopSource, VERSION,
};

fn profile() -> ResolvedTerminalProfile {
    TerminalSettingsV1::default().resolve(&TerminalOverrides::default())
}

fn engine(profile: &ResolvedTerminalProfile) -> DefaultTerminalEngine {
    match DefaultTerminalEngine::new(
        profile,
        TerminalSize {
            cols: 12,
            rows: 3,
            pixel_width: 120,
            pixel_height: 60,
            dpi: 96,
        },
    ) {
        Ok(engine) => engine,
        Err(error) => panic!("真实引擎构造失败：{error}"),
    }
}

fn key(identity: u64, code: KeyCode) -> PolicyKeyInput {
    PolicyKeyInput {
        raw_chord: Some(RawChord {
            code: code.clone(),
            modifiers: KeyModifiers::default(),
        }),
        dual: Some(VerifiedKeyCodes {
            code: code.clone(),
            legacy_code: code,
        }),
        phase: DesktopKeyPhase::Down,
        physical_identity: NonZeroU64::new(identity),
        alt_provenance: AltProvenance::Known {
            left: false,
            right: false,
        },
        text_ownership: TextOwnership::TerminalKey,
        default_action: None,
    }
}

fn chord(input: &mut PolicyKeyInput, modifiers: KeyModifiers) {
    if let Some(raw) = &mut input.raw_chord {
        raw.modifiers = modifiers;
    } else {
        panic!("fixture 必须已有可信 raw");
    }
}

fn binding(code: KeyCode, modifiers: KeyModifiers, action: &str) -> KeyBinding {
    KeyBinding {
        code,
        modifiers,
        action: action.into(),
    }
}

/// 同步测试 driver：使用真实 gate/apply，按同步结果锁存 Down 拒绝并归一化 native 生命周期。
fn drive_sync_key(
    state: &mut InputPolicyState,
    profile: &ResolvedTerminalProfile,
    engine: &mut dyn TerminalEngine,
    input: PolicyKeyInput,
) -> Result<KeyOutcome, EngineError> {
    let phase = input.phase;
    let identity = input.physical_identity;
    let intent = gate_key(state, profile, input);
    let result = apply_intent(engine, intent);
    if phase == DesktopKeyPhase::Down
        && (result.is_err()
            || matches!(
                &result,
                Ok(KeyOutcome {
                    result: KeyResult::TypedInputReject(InputReject::UnsupportedInput),
                    ..
                })
            ))
        && let Some(active) = state
            .active
            .iter_mut()
            .flatten()
            .find(|key| Some(key.identity) == identity)
    {
        active.route = Route::Rejected;
    }
    result.map(|mut outcome| {
        if phase != DesktopKeyPhase::Down
            && outcome.result == KeyResult::NoSend(NoSendReason::NativeText)
        {
            outcome.result = KeyResult::NoSend(NoSendReason::ConsumedKey);
        }
        outcome
    })
}

fn run(
    state: &mut InputPolicyState,
    profile: &ResolvedTerminalProfile,
    engine: &mut dyn TerminalEngine,
    input: PolicyKeyInput,
) -> KeyOutcome {
    match drive_sync_key(state, profile, engine, input) {
        Ok(outcome) => outcome,
        Err(error) => panic!("策略返回真实引擎故障：{error}"),
    }
}

fn quiet(result: KeyResult) -> KeyOutcome {
    KeyOutcome {
        result,
        scroll_to_bottom: false,
    }
}

fn rejected(reason: InputReject) -> KeyOutcome {
    quiet(KeyResult::TypedInputReject(reason))
}

fn no_send(reason: NoSendReason) -> KeyOutcome {
    quiet(KeyResult::NoSend(reason))
}

/// 不实现 encode_key_event，观察对象安全默认方法实际交给旧入口的材料。
struct OldEngine {
    inner: DefaultTerminalEngine,
    inputs: Vec<TerminalInput>,
    input_error: Option<EngineError>,
    empty_input: bool,
    clears: usize,
    clear_error: Option<EngineError>,
}

impl OldEngine {
    fn new(profile: &ResolvedTerminalProfile) -> Self {
        Self {
            inner: engine(profile),
            inputs: Vec::new(),
            input_error: None,
            empty_input: false,
            clears: 0,
            clear_error: None,
        }
    }
}

impl TerminalEngine for OldEngine {
    fn display_modes(&self) -> TerminalDisplayModes {
        self.inner.display_modes()
    }
    fn recover_display(&mut self) -> Result<DisplayRecovery, EngineError> {
        self.inner.recover_display()
    }
    fn advance(&mut self, bytes: &[u8]) -> Result<EngineDelta, EngineError> {
        self.inner.advance(bytes)
    }
    fn resize(&mut self, size: TerminalSize) -> Result<(), EngineError> {
        TerminalEngine::resize(&mut self.inner, size)
    }
    fn render(
        &mut self,
        viewport: Viewport,
        selection: Option<SelectionRange>,
    ) -> Result<Arc<RenderFrame>, EngineError> {
        self.inner.render(viewport, selection)
    }
    fn encode_input(&mut self, input: TerminalInput) -> Result<Vec<u8>, EngineError> {
        self.inputs.push(input.clone());
        if let Some(error) = self.input_error.take() {
            return Err(error);
        }
        if self.empty_input {
            return Ok(Vec::new());
        }
        self.inner.encode_input(input)
    }
    fn encode_mouse(&mut self, input: TerminalMouseEvent) -> Result<Vec<u8>, EngineError> {
        self.inner.encode_mouse(input)
    }
    fn clear_scrollback(&mut self) -> Result<(), EngineError> {
        self.clears += 1;
        if let Some(error) = self.clear_error.take() {
            return Err(error);
        }
        TerminalEngine::clear_scrollback(&mut self.inner)
    }
    fn scroll(&mut self, delta_rows: i32) -> Result<(), EngineError> {
        self.inner.scroll(delta_rows)
    }
    fn viewport_bounds(&self) -> ViewportBounds {
        self.inner.viewport_bounds()
    }
    fn search(&self, query: &SearchQuery) -> Result<Vec<SearchMatch>, EngineError> {
        TerminalEngine::search(&self.inner, query)
    }
    fn selected_text(&self, range: SelectionRange) -> Result<String, EngineError> {
        self.inner.selected_text(range)
    }
}

#[test]
fn missing_and_invalid_material_never_call_engine_or_release_default_candidates() {
    let p = profile();
    let mut old = OldEngine::new(&p);
    let cases = [
        (InputReject::MissingPhysicalIdentity, {
            let mut k = key(0, KeyCode::Character('a'));
            k.default_action = Some(DesktopAction::Copy);
            k
        }),
        (InputReject::MissingRawChord, {
            let mut k = key(1, KeyCode::Character('a'));
            k.raw_chord = None;
            k.default_action = Some(DesktopAction::ZoomIn);
            k
        }),
        (InputReject::InvalidRawChord, {
            let mut k = key(1, KeyCode::F(0));
            k.default_action = Some(DesktopAction::Paste);
            k
        }),
        (InputReject::InvalidRawChord, key(1, KeyCode::F(25))),
        (InputReject::MissingDual, {
            let mut k = key(1, KeyCode::ArrowUp);
            k.dual = None;
            k
        }),
        (InputReject::InvalidDual, {
            let mut k = key(1, KeyCode::ArrowUp);
            k.dual = Some(VerifiedKeyCodes {
                code: KeyCode::ArrowUp,
                legacy_code: KeyCode::ArrowDown,
            });
            k
        }),
        (InputReject::InvalidDual, {
            let mut k = key(1, KeyCode::ArrowUp);
            k.dual = Some(VerifiedKeyCodes {
                code: KeyCode::Character('a'),
                legacy_code: KeyCode::ArrowUp,
            });
            k
        }),
        (InputReject::InvalidDual, {
            let mut k = key(1, KeyCode::ArrowUp);
            k.dual = Some(VerifiedKeyCodes {
                code: KeyCode::ArrowUp,
                legacy_code: KeyCode::Character('a'),
            });
            k
        }),
        (InputReject::InvalidDual, {
            let mut k = key(1, KeyCode::ArrowUp);
            k.dual = Some(VerifiedKeyCodes {
                code: KeyCode::F(25),
                legacy_code: KeyCode::F(25),
            });
            k
        }),
    ];
    for (reason, input) in cases {
        let mut state = InputPolicyState::default();
        assert_eq!(
            run(&mut state, &p, &mut old, input.clone()),
            rejected(reason)
        );
        if input.physical_identity.is_some() {
            let mut repeat = key(1, KeyCode::Character('a'));
            repeat.phase = DesktopKeyPhase::Repeat;
            assert_eq!(
                run(&mut state, &p, &mut old, repeat),
                no_send(NoSendReason::InitialReject)
            );
            let mut up = input;
            up.phase = DesktopKeyPhase::Up;
            up.raw_chord = None;
            up.dual = None;
            assert_eq!(
                run(&mut state, &p, &mut old, up),
                no_send(NoSendReason::InitialReject)
            );
            assert_eq!(state, InputPolicyState::default());
        }
    }
    assert!(old.inputs.is_empty());
    assert_eq!(old.clears, 0);
}

#[test]
fn six_core_actions_are_exclusive_and_repeat_only_send() {
    for (action, expected) in [
        ("send:\x1b[3~", KeyResult::Bytes(b"\x1b[3~".to_vec())),
        ("send:\x7f", KeyResult::Bytes(vec![0x7f])),
        ("send:\x08", KeyResult::Bytes(vec![0x08])),
        ("clear_scrollback", KeyResult::ClearScrollback),
        ("new_tab", KeyResult::Action(DesktopAction::NewTab)),
        (
            "split_vertical",
            KeyResult::Action(DesktopAction::SplitRight),
        ),
    ] {
        let mut p = profile();
        p.scroll_on_keypress = true;
        p.key_bindings
            .push(binding(KeyCode::F(1), KeyModifiers::default(), action));
        let mut old = OldEngine::new(&p);
        let mut state = InputPolicyState::default();
        let mut input = key(1, KeyCode::F(1));
        input.dual = None;
        input.text_ownership = TextOwnership::Unverified;
        input.default_action = Some(DesktopAction::ZoomOut);
        let outcome = run(&mut state, &p, &mut old, input.clone());
        assert_eq!(outcome.result, expected);
        assert_eq!(outcome.scroll_to_bottom, action.starts_with("send:"));
        if outcome.result == KeyResult::ClearScrollback {
            let effects = match clear_scrollback(&mut old) {
                Ok(effects) => effects,
                Err(error) => panic!("清理失败：{error}"),
            };
            assert!(
                effects.clear_selection
                    && effects.invalidate_frame
                    && effects.invalidate_row_ranges
                    && effects.request_frame
            );
        }
        input.phase = DesktopKeyPhase::Repeat;
        let repeat = run(&mut state, &p, &mut old, input.clone());
        if action.starts_with("send:") {
            assert_eq!(repeat.result, expected);
            assert!(repeat.scroll_to_bottom);
        } else {
            assert_eq!(repeat, no_send(NoSendReason::ConsumedKey));
        }
        input.phase = DesktopKeyPhase::Up;
        input.raw_chord = None;
        input.default_action = Some(DesktopAction::NewTab);
        assert_eq!(
            run(&mut state, &p, &mut old, input),
            no_send(NoSendReason::ConsumedKey)
        );
        assert!(old.inputs.is_empty(), "绑定不再调用编码器");
        assert_eq!(old.clears, usize::from(action == "clear_scrollback"));
        assert_eq!(state, InputPolicyState::default());
    }
}

const ACTIONS: [DesktopAction; 16] = [
    DesktopAction::NewTab,
    DesktopAction::SplitRight,
    DesktopAction::SplitDown,
    DesktopAction::ClosePane,
    DesktopAction::NextTab,
    DesktopAction::PreviousTab,
    DesktopAction::NextPane,
    DesktopAction::Search,
    DesktopAction::Settings,
    DesktopAction::ToggleSidebar,
    DesktopAction::Copy,
    DesktopAction::Paste,
    DesktopAction::SelectAll,
    DesktopAction::ZoomIn,
    DesktopAction::ZoomOut,
    DesktopAction::ZoomReset,
];

#[test]
fn every_default_candidate_is_once_only_and_overridden_by_merged_bindings() {
    // 已合并 fixture 只证明消费；真实 repository/resolver 合并由既有测试覆盖。
    let mut merged = profile();
    merged.key_bindings = vec![
        binding(KeyCode::F(1), KeyModifiers::default(), "clear_scrollback"),
        binding(KeyCode::F(2), KeyModifiers::default(), "split_vertical"),
    ];
    let mut old = OldEngine::new(&merged);
    for action in ACTIONS {
        for (code, expected) in [
            (KeyCode::F(1), KeyResult::ClearScrollback),
            (KeyCode::F(2), KeyResult::Action(DesktopAction::SplitRight)),
        ] {
            let mut state = InputPolicyState::default();
            let mut input = key(1, code);
            input.default_action = Some(action);
            input.dual = None;
            assert_eq!(run(&mut state, &merged, &mut old, input), quiet(expected));
        }
        let mut state = InputPolicyState::default();
        let mut input = key(1, KeyCode::F(3));
        input.default_action = Some(action);
        input.dual = None;
        assert_eq!(
            run(&mut state, &merged, &mut old, input.clone()),
            quiet(KeyResult::Action(action))
        );
        input.phase = DesktopKeyPhase::Repeat;
        input.default_action = Some(DesktopAction::NewTab);
        assert_eq!(
            run(&mut state, &merged, &mut old, input.clone()),
            no_send(NoSendReason::ConsumedKey)
        );
        input.phase = DesktopKeyPhase::Up;
        assert_eq!(
            run(&mut state, &merged, &mut old, input),
            no_send(NoSendReason::ConsumedKey)
        );
    }
    assert!(old.inputs.is_empty());
    assert_eq!(old.clears, 0, "返回请求不偷偷执行 clear");
}

#[test]
fn raw_matching_precedes_alt_mask_and_is_exact_for_case_shift_and_super() {
    let mut p = profile();
    p.left_alt_as_meta = false;
    p.right_alt_as_meta = false;
    let modifiers = KeyModifiers {
        shift: true,
        alt: true,
        super_key: true,
        ..KeyModifiers::default()
    };
    p.key_bindings
        .push(binding(KeyCode::Character('A'), modifiers, "new_tab"));
    let mut old = OldEngine::new(&p);
    let mut input = key(1, KeyCode::Character('A'));
    chord(&mut input, modifiers);
    input.dual = None;
    input.text_ownership = TextOwnership::Unverified;
    input.default_action = Some(DesktopAction::Copy);
    assert_eq!(
        run(
            &mut InputPolicyState::default(),
            &p,
            &mut old,
            input.clone()
        ),
        quiet(KeyResult::Action(DesktopAction::NewTab))
    );
    for raw in [
        RawChord {
            code: KeyCode::Character('a'),
            modifiers,
        },
        RawChord {
            code: KeyCode::Character('A'),
            modifiers: KeyModifiers {
                shift: false,
                ..modifiers
            },
        },
        RawChord {
            code: KeyCode::Character('A'),
            modifiers: KeyModifiers {
                super_key: false,
                ..modifiers
            },
        },
    ] {
        input.raw_chord = Some(raw);
        assert_eq!(
            run(
                &mut InputPolicyState::default(),
                &p,
                &mut old,
                input.clone()
            ),
            quiet(KeyResult::Action(DesktopAction::Copy))
        );
    }
    assert!(old.inputs.is_empty());
}

#[test]
fn invalid_action_is_rejected_and_latched_until_up_without_fallback() {
    let mut p = profile();
    p.key_bindings.push(binding(
        KeyCode::F(1),
        KeyModifiers::default(),
        "send:arbitrary",
    ));
    let mut old = OldEngine::new(&p);
    let mut state = InputPolicyState::default();
    let mut input = key(1, KeyCode::F(1));
    input.default_action = Some(DesktopAction::Copy);
    assert_eq!(
        run(&mut state, &p, &mut old, input.clone()),
        rejected(InputReject::InvalidAction)
    );
    input = key(1, KeyCode::Character('a'));
    input.phase = DesktopKeyPhase::Repeat;
    assert_eq!(
        run(&mut state, &p, &mut old, input.clone()),
        no_send(NoSendReason::InitialReject)
    );
    input.phase = DesktopKeyPhase::Up;
    assert_eq!(
        run(&mut state, &p, &mut old, input),
        no_send(NoSendReason::InitialReject)
    );
    assert!(old.inputs.is_empty());
}

#[test]
fn physical_lifecycle_latches_consumption_and_rejects_unassociated_phases() {
    let mut p = profile();
    p.enable_kitty_keyboard = true;
    p.key_bindings
        .push(binding(KeyCode::F(1), KeyModifiers::default(), "new_tab"));
    p.key_bindings
        .push(binding(KeyCode::F(2), KeyModifiers::default(), "send:\x7f"));
    let mut real = engine(&p);
    assert!(real.advance(b"\x1b[>3u").is_ok());
    let mut state = InputPolicyState::default();
    for phase in [DesktopKeyPhase::Repeat, DesktopKeyPhase::Up] {
        let mut k = key(1, KeyCode::F(1));
        k.phase = phase;
        assert_eq!(
            run(&mut state, &p, &mut real, k),
            rejected(InputReject::UnassociatedPhase)
        );
    }
    assert_eq!(
        run(&mut state, &p, &mut real, key(1, KeyCode::F(1))),
        quiet(KeyResult::Action(DesktopAction::NewTab))
    );
    let mut duplicate = key(1, KeyCode::Character('a'));
    duplicate.default_action = Some(DesktopAction::Copy);
    let before = state.active;
    assert_eq!(
        run(&mut state, &p, &mut real, duplicate),
        rejected(InputReject::DuplicateDown)
    );
    assert_eq!(state.active, before);
    let mut repeat = key(1, KeyCode::Character('a'));
    repeat.phase = DesktopKeyPhase::Repeat;
    assert_eq!(
        run(&mut state, &p, &mut real, repeat.clone()),
        no_send(NoSendReason::ConsumedKey)
    );
    repeat.raw_chord = Some(RawChord {
        code: KeyCode::F(2),
        modifiers: KeyModifiers::default(),
    });
    assert_eq!(
        run(&mut state, &p, &mut real, repeat.clone()).result,
        KeyResult::Bytes(vec![0x7f])
    );
    repeat.phase = DesktopKeyPhase::Up;
    repeat.raw_chord = None;
    repeat.dual = None;
    assert_eq!(
        run(&mut state, &p, &mut real, repeat.clone()),
        no_send(NoSendReason::ConsumedKey)
    );
    assert_eq!(
        run(&mut state, &p, &mut real, repeat),
        rejected(InputReject::UnassociatedPhase)
    );
    assert_eq!(state, InputPolicyState::default());
}

#[test]
fn first_binding_on_terminal_repeat_consumes_without_forging_down_or_release() {
    for action in ["new_tab", "clear_scrollback", "send:\x08", "invalid"] {
        let mut p = profile();
        p.key_bindings.push(binding(
            KeyCode::Character('a'),
            KeyModifiers {
                control: true,
                ..KeyModifiers::default()
            },
            action,
        ));
        let mut old = OldEngine::new(&p);
        let mut state = InputPolicyState::default();
        assert_eq!(
            run(&mut state, &p, &mut old, key(1, KeyCode::Character('a'))).result,
            KeyResult::Bytes(b"a".to_vec())
        );
        let mut repeat = key(1, KeyCode::Character('a'));
        repeat.phase = DesktopKeyPhase::Repeat;
        chord(
            &mut repeat,
            KeyModifiers {
                control: true,
                ..KeyModifiers::default()
            },
        );
        let expected = match action {
            "send:\x08" => quiet(KeyResult::Bytes(vec![0x08])),
            "invalid" => rejected(InputReject::InvalidAction),
            _ => no_send(NoSendReason::ConsumedKey),
        };
        assert_eq!(run(&mut state, &p, &mut old, repeat), expected);
        let mut changed = key(1, KeyCode::Character('a'));
        changed.phase = DesktopKeyPhase::Repeat;
        changed.default_action = Some(DesktopAction::Copy);
        assert_eq!(
            run(&mut state, &p, &mut old, changed.clone()),
            no_send(NoSendReason::ConsumedKey)
        );
        changed.phase = DesktopKeyPhase::Up;
        assert_eq!(
            run(&mut state, &p, &mut old, changed),
            no_send(NoSendReason::ConsumedKey)
        );
        assert_eq!(old.inputs.len(), 1, "不补造按下或 release");
        assert_eq!(old.clears, 0);
    }
}

#[test]
fn initial_reject_cannot_recover_to_send_but_terminal_repeat_error_keeps_release() {
    let mut p = profile();
    p.enable_kitty_keyboard = true;
    p.key_bindings
        .push(binding(KeyCode::F(1), KeyModifiers::default(), "send:\x08"));
    let mut real = engine(&p);
    assert!(real.advance(b"\x1b[>3u").is_ok());
    let mut state = InputPolicyState::default();
    let mut missing = key(1, KeyCode::F(1));
    missing.raw_chord = None;
    assert_eq!(
        run(&mut state, &p, &mut real, missing),
        rejected(InputReject::MissingRawChord)
    );
    let mut k = key(1, KeyCode::F(1));
    k.phase = DesktopKeyPhase::Repeat;
    assert_eq!(
        run(&mut state, &p, &mut real, k.clone()),
        no_send(NoSendReason::InitialReject)
    );
    k.phase = DesktopKeyPhase::Up;
    assert_eq!(
        run(&mut state, &p, &mut real, k),
        no_send(NoSendReason::InitialReject)
    );
    let mut control = key(1, KeyCode::Character('a'));
    chord(
        &mut control,
        KeyModifiers {
            control: true,
            ..KeyModifiers::default()
        },
    );
    assert_eq!(
        run(&mut state, &p, &mut real, control.clone()).result,
        KeyResult::Bytes(b"\x1b[97;5u".to_vec())
    );
    let before = state.active;
    control.phase = DesktopKeyPhase::Repeat;
    let mut bad = control.clone();
    bad.dual = None;
    assert_eq!(
        run(&mut state, &p, &mut real, bad),
        rejected(InputReject::MissingDual)
    );
    assert_eq!(state.active, before);
    control.phase = DesktopKeyPhase::Up;
    assert_eq!(
        run(&mut state, &p, &mut real, control).result,
        KeyResult::Bytes(b"\x1b[97;5:3u".to_vec())
    );
    assert_eq!(state, InputPolicyState::default());
}

#[test]
fn unsupported_terminal_repeat_preserves_real_negotiated_release() -> Result<(), EngineError> {
    let mut p = profile();
    p.enable_kitty_keyboard = true;
    let mut real = engine(&p);
    real.advance(b"\x1b[>3u")?;
    let mut state = InputPolicyState::default();
    let mut input = key(1, KeyCode::Character('a'));
    chord(
        &mut input,
        KeyModifiers {
            super_key: true,
            ..KeyModifiers::default()
        },
    );
    assert_eq!(
        run(&mut state, &p, &mut real, input.clone()).result,
        KeyResult::Bytes(b"\x1b[97;9u".to_vec())
    );
    real.advance(b"\x1b[<u")?;
    let before = state.active;
    input.phase = DesktopKeyPhase::Repeat;
    assert_eq!(
        run(&mut state, &p, &mut real, input.clone()),
        rejected(InputReject::UnsupportedInput)
    );
    assert_eq!(state.active, before);
    real.advance(b"\x1b[>3u")?;
    input.phase = DesktopKeyPhase::Up;
    assert_eq!(
        run(&mut state, &p, &mut real, input).result,
        KeyResult::Bytes(b"\x1b[97;9:3u".to_vec())
    );
    assert_eq!(state, InputPolicyState::default());
    Ok(())
}

#[test]
fn fixed_table_reserves_before_effects_and_preserves_same_logical_and_pane_isolation() {
    let mut p = profile();
    p.key_bindings
        .push(binding(KeyCode::F(1), KeyModifiers::default(), "new_tab"));
    p.key_bindings
        .push(binding(KeyCode::F(2), KeyModifiers::default(), "send:\x7f"));
    let mut old = OldEngine::new(&p);
    let mut state = InputPolicyState::default();
    for identity in 1..=MAX_ACTIVE_KEYS as u64 {
        assert_eq!(
            run(&mut state, &p, &mut old, key(identity, KeyCode::F(1))),
            quiet(KeyResult::Action(DesktopAction::NewTab))
        );
    }
    let before = state.active;
    let mut full = key(257, KeyCode::Character('a'));
    full.default_action = Some(DesktopAction::Copy);
    assert_eq!(
        run(&mut state, &p, &mut old, full.clone()),
        rejected(InputReject::ActiveKeyBudget)
    );
    full.raw_chord = Some(RawChord {
        code: KeyCode::F(1),
        modifiers: KeyModifiers::default(),
    });
    assert_eq!(
        run(&mut state, &p, &mut old, full.clone()),
        rejected(InputReject::ActiveKeyBudget)
    );
    full.raw_chord = Some(RawChord {
        code: KeyCode::F(2),
        modifiers: KeyModifiers::default(),
    });
    assert_eq!(
        run(&mut state, &p, &mut old, full),
        rejected(InputReject::ActiveKeyBudget)
    );
    assert_eq!(state.active, before);
    let mut other = InputPolicyState::default();
    assert_eq!(
        run(&mut other, &p, &mut old, key(1, KeyCode::Character('a'))).result,
        KeyResult::Bytes(b"a".to_vec())
    );
    let mut up = key(1, KeyCode::F(1));
    up.phase = DesktopKeyPhase::Up;
    up.raw_chord = None;
    assert_eq!(
        run(&mut state, &p, &mut old, up),
        no_send(NoSendReason::ConsumedKey)
    );
    assert_eq!(other.active.iter().flatten().count(), 1);
    assert_eq!(
        run(&mut state, &p, &mut old, key(257, KeyCode::F(1))),
        quiet(KeyResult::Action(DesktopAction::NewTab))
    );
    assert_eq!(state.active.iter().flatten().count(), MAX_ACTIVE_KEYS);
    assert_eq!(old.inputs.len(), 1, "满表不调用编码或驱逐已有键");
    assert_eq!(
        std::mem::size_of::<InputPolicyState>(),
        MAX_ACTIVE_KEYS * std::mem::size_of::<Option<ActiveKey>>()
    );
    println!(
        "active 槽={} bytes，固定表={} bytes，无堆所有权",
        std::mem::size_of::<Option<ActiveKey>>(),
        std::mem::size_of::<InputPolicyState>()
    );
}

#[test]
fn two_physical_keys_with_same_logical_key_and_terminal_up_do_not_run_candidates() {
    let p = profile();
    let mut old = OldEngine::new(&p);
    let mut state = InputPolicyState::default();
    let mut consumed = key(1, KeyCode::Character('a'));
    consumed.default_action = Some(DesktopAction::Copy);
    assert_eq!(
        run(&mut state, &p, &mut old, consumed),
        quiet(KeyResult::Action(DesktopAction::Copy))
    );
    assert_eq!(
        run(&mut state, &p, &mut old, key(2, KeyCode::Character('a'))).result,
        KeyResult::Bytes(b"a".to_vec())
    );
    let mut repeat = key(2, KeyCode::Character('a'));
    repeat.phase = DesktopKeyPhase::Repeat;
    repeat.default_action = Some(DesktopAction::NewTab);
    assert_eq!(
        run(&mut state, &p, &mut old, repeat).result,
        KeyResult::Bytes(b"a".to_vec()),
        "终端 Repeat 不执行新壳候选"
    );
    for (identity, reason) in [
        (1, NoSendReason::ConsumedKey),
        (2, NoSendReason::EngineEmpty),
    ] {
        let mut up = key(identity, KeyCode::Character('a'));
        up.phase = DesktopKeyPhase::Up;
        up.default_action = Some(DesktopAction::NewTab);
        assert_eq!(run(&mut state, &p, &mut old, up), no_send(reason));
    }
    assert_eq!(old.inputs.len(), 2);
    assert_eq!(state, InputPolicyState::default());
}

struct ShiftFixture {
    code: char,
    legacy: char,
    modifiers: KeyModifiers,
    legacy_bytes: &'static [u8],
    csi_bytes: &'static [u8],
    kitty: [&'static [u8]; 3],
}

// 仅是同物理 US 事件的受控双表示，不是生产布局映射。
const SHIFT_FIXTURES: [ShiftFixture; 3] = [
    ShiftFixture {
        code: '6',
        legacy: '^',
        modifiers: KeyModifiers {
            shift: true,
            control: true,
            alt: false,
            super_key: false,
        },
        legacy_bytes: b"\x1e",
        csi_bytes: b"\x1b[94;5u",
        kitty: [b"\x1b[54;6u", b"\x1b[54;6:2u", b"\x1b[54;6:3u"],
    },
    ShiftFixture {
        code: '.',
        legacy: '>',
        modifiers: KeyModifiers {
            shift: true,
            control: false,
            alt: true,
            super_key: false,
        },
        legacy_bytes: b"\x1b>",
        csi_bytes: b"\x1b>",
        kitty: [b"\x1b[46;4u", b"\x1b[46;4:2u", b"\x1b[46;4:3u"],
    },
    ShiftFixture {
        code: 'a',
        legacy: 'A',
        modifiers: KeyModifiers {
            shift: true,
            control: true,
            alt: false,
            super_key: false,
        },
        legacy_bytes: b"\x01",
        csi_bytes: b"\x1b[65;5u",
        kitty: [b"\x1b[97;6u", b"\x1b[97;6:2u", b"\x1b[97;6:3u"],
    },
];

fn shifted(fixture: &ShiftFixture) -> PolicyKeyInput {
    let mut input = key(1, KeyCode::Character(fixture.code));
    input.dual = Some(VerifiedKeyCodes {
        code: KeyCode::Character(fixture.code),
        legacy_code: KeyCode::Character(fixture.legacy),
    });
    chord(&mut input, fixture.modifiers);
    input.alt_provenance = AltProvenance::Unknown;
    input
}

fn assert_shift_cycle(p: &ResolvedTerminalProfile, real: &mut dyn TerminalEngine, flags: u8) {
    for fixture in &SHIFT_FIXTURES {
        let fallback = if p.enable_csi_u {
            fixture.csi_bytes
        } else {
            fixture.legacy_bytes
        };
        let enabled = p.enable_kitty_keyboard;
        let expected = [
            if enabled && flags & 1 != 0 {
                fixture.kitty[0]
            } else {
                fallback
            },
            if enabled && flags & 2 != 0 {
                fixture.kitty[1]
            } else if enabled && flags & 1 != 0 {
                fixture.kitty[0]
            } else {
                fallback
            },
            if enabled && flags & 2 != 0 {
                fixture.kitty[2]
            } else {
                b""
            },
        ];
        let mut state = InputPolicyState::default();
        for (phase, bytes) in [
            DesktopKeyPhase::Down,
            DesktopKeyPhase::Repeat,
            DesktopKeyPhase::Up,
        ]
        .into_iter()
        .zip(expected)
        {
            let mut input = shifted(fixture);
            input.phase = phase;
            let outcome = run(&mut state, p, real, input);
            let expected = if bytes.is_empty() {
                KeyResult::NoSend(NoSendReason::EngineEmpty)
            } else {
                KeyResult::Bytes(bytes.to_vec())
            };
            assert_eq!(
                outcome.result, expected,
                "code={} CSI-U={} Kitty={} flags={flags} phase={phase:?}",
                fixture.code, p.enable_csi_u, p.enable_kitty_keyboard
            );
        }
        assert_eq!(state, InputPolicyState::default());
    }
}

#[test]
fn real_engine_shift_matrix_negotiates_via_advance_and_restores_without_policy_cache()
-> Result<(), EngineError> {
    for csi in [false, true] {
        for kitty in [false, true] {
            let mut p = profile();
            p.enable_csi_u = csi;
            p.enable_kitty_keyboard = kitty;
            let mut real = engine(&p);
            assert_shift_cycle(&p, &mut real, 0);
            for (push, flags) in [
                (b"\x1b[>1u".as_slice(), 1),
                (b"\x1b[>2u".as_slice(), 2),
                (b"\x1b[>3u".as_slice(), 3),
            ] {
                real.advance(push)?;
                assert_shift_cycle(&p, &mut real, flags);
                let expected_query: &[u8] = match (kitty, flags) {
                    (false, _) => b"",
                    (true, 1) => b"\x1b[?1u",
                    (true, 2) => b"\x1b[?2u",
                    (true, 3) => b"\x1b[?3u",
                    _ => unreachable!(),
                };
                assert_eq!(real.advance(b"\x1b[?u")?.outbound, expected_query);
                real.advance(b"\x1b[<u")?;
                assert_shift_cycle(&p, &mut real, 0);
            }
            real.advance(b"\x1b[>1u\x1b[>3u")?;
            assert_shift_cycle(&p, &mut real, 3);
            real.advance(b"\x1b[<u")?;
            assert_shift_cycle(&p, &mut real, 1);
            real.advance(b"\x1b[=2u")?;
            assert_shift_cycle(&p, &mut real, 2);
            real.advance(b"\x1b[<u")?;
            assert_shift_cycle(&p, &mut real, 0);
            real.advance(b"\x1b[>3u")?;
            real.reset();
            assert_shift_cycle(&p, &mut real, 0);
            assert_shift_cycle(&p, &mut engine(&p), 0);
        }
    }
    Ok(())
}

#[test]
fn real_super_is_not_stripped_and_engine_unsupported_is_nonfatal() -> Result<(), EngineError> {
    for csi in [false, true] {
        for kitty in [false, true] {
            let mut p = profile();
            p.enable_csi_u = csi;
            p.enable_kitty_keyboard = kitty;
            let mut real = engine(&p);
            for (push, flags) in [
                (b"".as_slice(), 0),
                (b"\x1b[>1u".as_slice(), 1),
                (b"\x1b[>2u".as_slice(), 2),
                (b"\x1b[>3u".as_slice(), 3),
            ] {
                real.advance(push)?;
                let mut state = InputPolicyState::default();
                let mut input = key(1, KeyCode::Character('a'));
                chord(
                    &mut input,
                    KeyModifiers {
                        super_key: true,
                        ..KeyModifiers::default()
                    },
                );
                let press = run(&mut state, &p, &mut real, input.clone());
                let allowed = kitty && flags != 0;
                assert_eq!(
                    press,
                    if allowed {
                        quiet(KeyResult::Bytes(b"\x1b[97;9u".to_vec()))
                    } else {
                        rejected(InputReject::UnsupportedInput)
                    }
                );
                input.phase = DesktopKeyPhase::Repeat;
                assert_eq!(
                    run(&mut state, &p, &mut real, input.clone()),
                    if !allowed {
                        no_send(NoSendReason::InitialReject)
                    } else if flags & 2 != 0 {
                        quiet(KeyResult::Bytes(b"\x1b[97;9:2u".to_vec()))
                    } else {
                        quiet(KeyResult::Bytes(b"\x1b[97;9u".to_vec()))
                    }
                );
                input.phase = DesktopKeyPhase::Up;
                assert_eq!(
                    run(&mut state, &p, &mut real, input),
                    if !allowed {
                        no_send(NoSendReason::InitialReject)
                    } else if flags & 2 != 0 {
                        quiet(KeyResult::Bytes(b"\x1b[97;9:3u".to_vec()))
                    } else {
                        no_send(NoSendReason::EngineEmpty)
                    }
                );
                assert_eq!(
                    run(&mut state, &p, &mut real, key(2, KeyCode::Character('b'))).result,
                    KeyResult::Bytes(b"b".to_vec())
                );
                if flags != 0 {
                    real.advance(b"\x1b[<u")?;
                }
            }
        }
    }
    let mut p = profile();
    p.enable_kitty_keyboard = true;
    let mut real = engine(&p);
    real.advance(b"\x1b[>3u")?;
    for (code, expected) in [
        (KeyCode::ArrowUp, b"\x1b[1;9:2A".as_slice()),
        (KeyCode::Delete, b"\x1b[3;9:2~".as_slice()),
    ] {
        let mut state = InputPolicyState::default();
        let mut input = key(1, code);
        chord(
            &mut input,
            KeyModifiers {
                super_key: true,
                ..KeyModifiers::default()
            },
        );
        run(&mut state, &p, &mut real, input.clone());
        input.phase = DesktopKeyPhase::Repeat;
        assert_eq!(
            run(&mut state, &p, &mut real, input.clone()).result,
            KeyResult::Bytes(expected.to_vec())
        );
        real.advance(b"\x1b[<u")?;
        input.phase = DesktopKeyPhase::Up;
        assert_eq!(
            run(&mut state, &p, &mut real, input),
            rejected(InputReject::UnsupportedInput)
        );
        assert_eq!(state, InputPolicyState::default());
        real.advance(b"\x1b[>3u")?;
    }
    Ok(())
}

#[test]
fn old_trait_object_receives_legacy_code_and_empty_is_not_rejection() {
    let mut p = profile();
    p.scroll_on_keypress = true;
    for fixture in &SHIFT_FIXTURES {
        let mut old = OldEngine::new(&p);
        let mut state = InputPolicyState::default();
        let object: &mut dyn TerminalEngine = &mut old;
        for phase in [
            DesktopKeyPhase::Down,
            DesktopKeyPhase::Repeat,
            DesktopKeyPhase::Up,
        ] {
            let mut input = shifted(fixture);
            input.phase = phase;
            let outcome = run(&mut state, &p, object, input);
            if phase == DesktopKeyPhase::Up {
                assert_eq!(outcome, no_send(NoSendReason::EngineEmpty));
            } else {
                assert_eq!(
                    outcome.result,
                    KeyResult::Bytes(fixture.legacy_bytes.to_vec())
                );
                assert!(outcome.scroll_to_bottom);
            }
        }
        assert_eq!(
            old.inputs,
            vec![
                TerminalInput::Key {
                    code: KeyCode::Character(fixture.legacy),
                    modifiers: fixture.modifiers
                };
                2
            ]
        );
        assert_eq!(state, InputPolicyState::default());
    }
    let mut old = OldEngine::new(&p);
    let mut state = InputPolicyState::default();
    let mut super_key = key(1, KeyCode::Character('a'));
    chord(
        &mut super_key,
        KeyModifiers {
            super_key: true,
            ..KeyModifiers::default()
        },
    );
    assert_eq!(
        run(&mut state, &p, &mut old, super_key),
        rejected(InputReject::UnsupportedInput)
    );
    assert!(old.inputs.is_empty());
    old.empty_input = true;
    let outcome = run(&mut state, &p, &mut old, key(2, KeyCode::ArrowUp));
    assert_eq!(outcome.result, KeyResult::NoSend(NoSendReason::EngineEmpty));
    assert!(outcome.scroll_to_bottom, "合法空终端 Down 仍是已接受按键");
    let mut up = key(2, KeyCode::ArrowUp);
    up.phase = DesktopKeyPhase::Up;
    assert_eq!(
        run(&mut state, &p, &mut old, up),
        no_send(NoSendReason::EngineEmpty)
    );
}

#[test]
fn fatal_engine_errors_and_clear_failure_are_not_input_reject_or_success() {
    let p = profile();
    let mut old = OldEngine::new(&p);
    old.input_error = Some(EngineError::PresentationGenerationExhausted);
    let mut state = InputPolicyState::default();
    assert_eq!(
        drive_sync_key(&mut state, &p, &mut old, key(1, KeyCode::Character('a'))),
        Err(EngineError::PresentationGenerationExhausted)
    );
    assert_eq!(state.active[0].map(|key| key.route), Some(Route::Rejected));
    old.clear_error = Some(EngineError::PresentationGenerationExhausted);
    assert_eq!(
        clear_scrollback(&mut old),
        Err(EngineError::PresentationGenerationExhausted)
    );
    assert_eq!(old.clears, 1);
}

#[test]
fn alt_sides_or_together_unknown_preserves_total_and_known_must_be_consistent() {
    for left_enabled in [false, true] {
        for right_enabled in [false, true] {
            let mut p = profile();
            p.left_alt_as_meta = left_enabled;
            p.right_alt_as_meta = right_enabled;
            let mut real = engine(&p);
            for (left, right) in [(false, false), (true, false), (false, true), (true, true)] {
                let mut input = key(1, KeyCode::Character('a'));
                chord(
                    &mut input,
                    KeyModifiers {
                        alt: left || right,
                        ..KeyModifiers::default()
                    },
                );
                input.alt_provenance = AltProvenance::Known { left, right };
                let outcome = run(&mut InputPolicyState::default(), &p, &mut real, input);
                let expected = if (left && left_enabled) || (right && right_enabled) {
                    b"\x1ba".as_slice()
                } else {
                    b"a".as_slice()
                };
                assert_eq!(outcome.result, KeyResult::Bytes(expected.to_vec()));
            }
            for total in [false, true] {
                let mut input = key(1, KeyCode::Character('a'));
                chord(
                    &mut input,
                    KeyModifiers {
                        alt: total,
                        ..KeyModifiers::default()
                    },
                );
                input.alt_provenance = AltProvenance::Unknown;
                assert_eq!(
                    run(
                        &mut InputPolicyState::default(),
                        &p,
                        &mut real,
                        input.clone()
                    )
                    .result,
                    KeyResult::Bytes(if total {
                        b"\x1ba".to_vec()
                    } else {
                        b"a".to_vec()
                    })
                );
                input.alt_provenance = AltProvenance::Known {
                    left: !total,
                    right: false,
                };
                assert_eq!(
                    run(&mut InputPolicyState::default(), &p, &mut real, input),
                    rejected(InputReject::InconsistentAlt)
                );
            }
        }
    }
}

#[test]
fn ctrl_alt_requires_real_ownership_and_native_text_never_constructs_committed_text() {
    let mut p = profile();
    p.left_alt_as_meta = false;
    p.right_alt_as_meta = false;
    p.scroll_on_keypress = true;
    let mut old = OldEngine::new(&p);
    for provenance in [
        AltProvenance::Known {
            left: false,
            right: true,
        },
        AltProvenance::Unknown,
    ] {
        let mut input = key(1, KeyCode::Character('a'));
        chord(
            &mut input,
            KeyModifiers {
                control: true,
                alt: true,
                ..KeyModifiers::default()
            },
        );
        input.alt_provenance = provenance;
        input.text_ownership = TextOwnership::Unverified;
        assert_eq!(
            run(
                &mut InputPolicyState::default(),
                &p,
                &mut old,
                input.clone()
            ),
            rejected(InputReject::AmbiguousAltProvenance)
        );
        input.text_ownership = TextOwnership::NativeText;
        input.dual = None;
        let mut state = InputPolicyState::default();
        assert_eq!(
            run(&mut state, &p, &mut old, input.clone()),
            no_send(NoSendReason::NativeText)
        );
        input.phase = DesktopKeyPhase::Repeat;
        input.text_ownership = TextOwnership::TerminalKey;
        assert_eq!(
            run(&mut state, &p, &mut old, input.clone()),
            no_send(NoSendReason::ConsumedKey)
        );
        input.phase = DesktopKeyPhase::Up;
        input.raw_chord = None;
        assert_eq!(
            run(&mut state, &p, &mut old, input),
            no_send(NoSendReason::ConsumedKey)
        );
    }
    for code in [KeyCode::Character('a'), KeyCode::ArrowUp] {
        let mut input = key(1, code);
        input.text_ownership = TextOwnership::Unverified;
        assert_eq!(
            run(&mut InputPolicyState::default(), &p, &mut old, input),
            rejected(InputReject::UnverifiedTextOwnership)
        );
    }
    let mut printable_alt = key(1, KeyCode::Character('a'));
    chord(
        &mut printable_alt,
        KeyModifiers {
            alt: true,
            ..KeyModifiers::default()
        },
    );
    printable_alt.alt_provenance = AltProvenance::Known {
        left: true,
        right: false,
    };
    printable_alt.text_ownership = TextOwnership::Unverified;
    assert_eq!(
        run(
            &mut InputPolicyState::default(),
            &p,
            &mut old,
            printable_alt
        ),
        rejected(InputReject::AmbiguousAltProvenance)
    );
    assert!(
        old.inputs.is_empty(),
        "不将 AltGr/Option/普通硬件制造成文本或控制字节"
    );
    let mut real_ctrl_alt = key(1, KeyCode::Character('a'));
    chord(
        &mut real_ctrl_alt,
        KeyModifiers {
            control: true,
            alt: true,
            ..KeyModifiers::default()
        },
    );
    real_ctrl_alt.alt_provenance = AltProvenance::Known {
        left: false,
        right: true,
    };
    assert_eq!(
        run(
            &mut InputPolicyState::default(),
            &p,
            &mut old,
            real_ctrl_alt.clone()
        )
        .result,
        KeyResult::Bytes(vec![1])
    );
    real_ctrl_alt.alt_provenance = AltProvenance::Unknown;
    assert_eq!(
        run(
            &mut InputPolicyState::default(),
            &p,
            &mut old,
            real_ctrl_alt
        )
        .result,
        KeyResult::Bytes(vec![0x1b, 1])
    );
    assert!(
        old.inputs
            .iter()
            .all(|input| matches!(input, TerminalInput::Key { .. }))
    );
    let mut unsafe_raw = key(1, KeyCode::Character('a'));
    unsafe_raw.raw_chord = None;
    unsafe_raw.default_action = Some(DesktopAction::Copy);
    assert_eq!(
        run(&mut InputPolicyState::default(), &p, &mut old, unsafe_raw),
        rejected(InputReject::MissingRawChord)
    );
}

#[test]
fn four_scroll_configurations_classify_only_accepted_activity() {
    for output in [false, true] {
        for keypress in [false, true] {
            let mut p = profile();
            p.scroll_on_output = output;
            p.scroll_on_keypress = keypress;
            assert_eq!(scroll_effect(&p, Activity::Output), output);
            for activity in [
                Activity::AcceptedTerminalKey,
                Activity::AcceptedText,
                Activity::AcceptedPaste,
            ] {
                assert_eq!(scroll_effect(&p, activity), keypress);
            }
            assert!(!scroll_effect(&p, Activity::Other));
            let mut old = OldEngine::new(&p);
            let mut state = InputPolicyState::default();
            let down = run(&mut state, &p, &mut old, key(1, KeyCode::Character('a')));
            assert_eq!(down.scroll_to_bottom, keypress);
            let mut input = key(1, KeyCode::Character('a'));
            input.phase = DesktopKeyPhase::Repeat;
            assert_eq!(
                run(&mut state, &p, &mut old, input.clone()).scroll_to_bottom,
                keypress
            );
            input.phase = DesktopKeyPhase::Up;
            assert!(!run(&mut state, &p, &mut old, input).scroll_to_bottom);
            let mut paste = key(2, KeyCode::F(1));
            paste.default_action = Some(DesktopAction::Paste);
            assert_eq!(
                run(&mut state, &p, &mut old, paste),
                quiet(KeyResult::Action(DesktopAction::Paste))
            );
            let mut text = key(3, KeyCode::Character('a'));
            text.text_ownership = TextOwnership::NativeText;
            assert_eq!(
                run(&mut state, &p, &mut old, text),
                no_send(NoSendReason::NativeText)
            );
            let mut missing = key(4, KeyCode::Character('a'));
            missing.dual = None;
            assert_eq!(
                run(&mut state, &p, &mut old, missing),
                rejected(InputReject::MissingDual)
            );
        }
    }
}

#[test]
fn real_mouse_double_gate_preserves_local_selection_and_uses_engine_bytes()
-> Result<(), EngineError> {
    let mouse = TerminalMouseEvent {
        kind: MouseEventKind::Press,
        button: Some(MouseButton::Left),
        cell: CellPosition {
            stable_row: 1,
            column: 2,
        },
        viewport_row: 1,
        pixel_x: 0,
        pixel_y: 0,
        modifiers: KeyModifiers::default(),
    };
    for enabled in [false, true] {
        let mut p = profile();
        p.mouse_reporting = enabled;
        let mut real = engine(&p);
        for (sequence, negotiated) in [
            (b"".as_slice(), false),
            (b"\x1b[?1000h\x1b[?1006h".as_slice(), true),
            (b"\x1b[?1000l".as_slice(), false),
        ] {
            real.advance(sequence)?;
            let policy = mouse_policy(&p, real.display_modes());
            assert_eq!(policy.report_to_remote, enabled && negotiated);
            assert_eq!(policy.local_selection, !(enabled && negotiated));
            if policy.report_to_remote {
                assert_eq!(real.encode_mouse(mouse)?, b"\x1b[<0;3;2M");
            } else {
                assert!(matches!(
                    real.encode_mouse(mouse),
                    Err(EngineError::UnsupportedMouse(_))
                ));
            }
            assert!(!scroll_effect(&p, Activity::Other));
        }
    }
    Ok(())
}

#[test]
fn real_clear_consumer_refreshes_bounds_and_delivers_only_minimal_invalidation_intent()
-> Result<(), EngineError> {
    let mut p = profile();
    p.key_bindings.push(binding(
        KeyCode::F(1),
        KeyModifiers::default(),
        "clear_scrollback",
    ));
    let mut real = engine(&p);
    real.advance(b"0\r\n1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n")?;
    let before = real.viewport_bounds();
    assert!(before.first_stable_row < before.bottom_top_stable_row);
    let outcome = run(
        &mut InputPolicyState::default(),
        &p,
        &mut real,
        key(1, KeyCode::F(1)),
    );
    assert_eq!(outcome, quiet(KeyResult::ClearScrollback));
    assert_eq!(
        real.viewport_bounds(),
        before,
        "结果是待消费请求，不是 Screen 已完成"
    );
    let effects = clear_scrollback(&mut real)?;
    assert_eq!(
        effects,
        ClearEffects {
            bounds: real.viewport_bounds(),
            clear_selection: true,
            invalidate_frame: true,
            invalidate_row_ranges: true,
            request_frame: true,
        }
    );
    assert_eq!(
        effects.bounds.first_stable_row,
        effects.bounds.bottom_top_stable_row
    );
    assert!(effects.bounds.first_stable_row > before.first_stable_row);
    Ok(())
}

#[test]
fn enq_uses_captured_answerback_advance_and_sync_outbound_exactly_once() -> Result<(), EngineError>
{
    for answer in ["first", "第二", ""] {
        let mut p = profile();
        p.answerback = answer.into();
        let mut real = engine(&p);
        let mut outbound = Vec::new();
        for segment in [
            b"\x05\x05".as_slice(),
            b"x\x05".as_slice(),
            b"\x1b[?2026h\x05".as_slice(),
            b"y\x05".as_slice(),
        ] {
            outbound.extend(real.advance(segment)?.outbound);
        }
        assert!(real.sync_deadline().is_some(), "真实同步输出仍在等待刷新");
        outbound.extend(real.end_sync()?.outbound);
        assert!(real.sync_deadline().is_none());
        outbound.extend(real.advance(b"\x1b[?2026l\x05")?.outbound);
        outbound.extend(real.end_sync()?.outbound);
        assert_eq!(outbound, answer.as_bytes().repeat(6));
        assert!(real.end_sync()?.outbound.is_empty());
        let other = profile();
        let mut other_engine = engine(&other);
        assert_eq!(other_engine.advance(b"\x05")?.outbound, b"rsHell");
    }
    Ok(())
}

struct AssociatedOutcome {
    source: DesktopSource,
    identity: String,
    seq: u64,
    outcome: KeyOutcome,
}

struct AdmittedConsumer {
    protocol: Protocol,
    state: InputPolicyState,
    profile: ResolvedTerminalProfile,
    engine: OldEngine,
    outcomes: Vec<AssociatedOutcome>,
}

impl AdmittedConsumer {
    fn new(session_id: u32) -> Self {
        let p = profile();
        Self {
            protocol: Protocol::new(
                DesktopSource {
                    session_id,
                    generation: 3,
                },
                "captured".into(),
            ),
            state: InputPolicyState::default(),
            engine: OldEngine::new(&p),
            profile: p,
            outcomes: Vec::new(),
        }
    }

    fn request(&self, seq: u64) -> DesktopOrdered {
        DesktopOrdered {
            version: VERSION,
            source: self.protocol.source.source.clone(),
            identity: self.protocol.source.identity.clone(),
            seq,
            event: DesktopEvent::Key {
                key: "character:a".into(),
                phase: DesktopKeyPhase::Down,
                physical: 1,
                logical: 1,
                shift: false,
                control: false,
                alt: false,
                left_alt: false,
                right_alt: false,
                alt_source: DesktopAltSource::Unknown,
                super_key: false,
                default_action: None,
            },
        }
    }

    fn request_for(&self, seq: u64, material: &PolicyKeyInput) -> DesktopOrdered {
        let mut request = self.request(seq);
        if let DesktopEvent::Key {
            phase,
            physical,
            default_action,
            ..
        } = &mut request.event
        {
            *phase = material.phase;
            *default_action = material.default_action;
            if let Some(identity) = material.physical_identity {
                *physical = identity.get();
            }
        }
        request
    }

    fn submit(
        &mut self,
        request: &DesktopOrdered,
        material: PolicyKeyInput,
    ) -> Result<InputLease, DesktopFailure> {
        let lease = self.protocol.admit(request)?;
        // 材料由测试显式提供，绝不把当前 DTO 自动升级成可信 raw/dual/文本来源。
        let outcome = run(&mut self.state, &self.profile, &mut self.engine, material);
        self.outcomes.push(AssociatedOutcome {
            source: request.source.clone(),
            identity: request.identity.clone(),
            seq: request.seq,
            outcome,
        });
        Ok(lease)
    }
}

#[test]
fn real_admission_filters_before_policy_and_typed_reject_consumes_original_sequence()
-> Result<(), DesktopFailure> {
    let mut consumer = AdmittedConsumer::new(1);
    let first = consumer.request(1);
    let lease = consumer.submit(&first, key(1, KeyCode::Character('a')))?;
    let associated = &consumer.outcomes[0];
    assert_eq!(associated.source, first.source);
    assert_eq!(associated.identity, first.identity);
    assert_eq!(associated.seq, 1);
    assert_eq!(associated.outcome.result, KeyResult::Bytes(b"a".to_vec()));
    let mut invalid = Vec::new();
    let mut version = consumer.request(2);
    version.version = 1;
    invalid.push((version, DesktopFailure::UnsupportedVersion));
    let mut session = consumer.request(2);
    session.source.session_id = 2;
    invalid.push((session, DesktopFailure::StaleGeneration));
    let mut generation = consumer.request(2);
    generation.source.generation += 1;
    invalid.push((generation, DesktopFailure::StaleGeneration));
    let mut identity = consumer.request(2);
    identity.identity = "other".into();
    invalid.push((identity, DesktopFailure::IdentityMismatch));
    invalid.push((consumer.request(1), DesktopFailure::Duplicate));
    invalid.push((consumer.request(0), DesktopFailure::Duplicate));
    invalid.push((consumer.request(3), DesktopFailure::OutOfOrder));
    let mut large = consumer.request(2);
    large.event = DesktopEvent::Paste {
        text: "x".repeat(INPUT_BYTES),
    };
    invalid.push((large, DesktopFailure::InputTooLarge));
    let before = consumer.state.active;
    for (request, expected) in invalid {
        assert!(
            matches!(consumer.submit(&request, key(2, KeyCode::F(1))), Err(error) if error == expected)
        );
        assert_eq!(consumer.outcomes.len(), 1, "拒绝 admission 不调用策略");
        assert_eq!(consumer.state.active, before);
        assert_eq!(consumer.engine.inputs.len(), 1);
    }
    let mut missing = key(2, KeyCode::Character('a'));
    missing.raw_chord = None;
    let second = consumer.request_for(2, &missing);
    let second_lease = consumer.submit(&second, missing)?;
    assert_eq!(consumer.outcomes[1].seq, 2);
    assert_eq!(
        consumer.outcomes[1].outcome,
        rejected(InputReject::MissingRawChord)
    );
    assert!(matches!(
        consumer.submit(&second, key(2, KeyCode::Character('a'))),
        Err(DesktopFailure::Duplicate)
    ));
    let mut up = key(1, KeyCode::Character('a'));
    up.phase = DesktopKeyPhase::Up;
    let third = consumer.request_for(3, &up);
    let third_lease = consumer.submit(&third, up)?;
    assert_eq!(consumer.outcomes[2].seq, 3);
    assert_eq!(
        consumer.outcomes[2].outcome,
        no_send(NoSendReason::EngineEmpty)
    );
    drop((lease, second_lease, third_lease));
    Ok(())
}

#[test]
fn real_leases_busy_retry_does_not_mutate_policy_and_other_pane_is_independent()
-> Result<(), DesktopFailure> {
    let mut consumer = AdmittedConsumer::new(1);
    let mut leases = Vec::new();
    for seq in 1..=INPUT_EVENTS as u64 {
        let mut material = key(seq, KeyCode::Character('a'));
        material.default_action = Some(DesktopAction::Copy);
        let request = consumer.request_for(seq, &material);
        leases.push(consumer.submit(&request, material)?);
    }
    let before = consumer.state.active;
    let mut up = key(1, KeyCode::Character('a'));
    up.phase = DesktopKeyPhase::Up;
    let next = consumer.request_for(INPUT_EVENTS as u64 + 1, &up);
    assert!(matches!(
        consumer.submit(&next, up.clone()),
        Err(DesktopFailure::Busy)
    ));
    assert_eq!(consumer.state.active, before);
    assert_eq!(consumer.outcomes.len(), INPUT_EVENTS);
    assert!(consumer.engine.inputs.is_empty());
    let mut other = AdmittedConsumer::new(2);
    let other_request = other.request(1);
    let other_lease = other.submit(&other_request, key(1, KeyCode::Character('a')))?;
    assert_eq!(
        other.outcomes[0].outcome.result,
        KeyResult::Bytes(b"a".to_vec())
    );
    drop(leases);
    let retry_lease = consumer.submit(&next, up)?;
    assert_eq!(consumer.outcomes.len(), INPUT_EVENTS + 1);
    assert_eq!(consumer.outcomes[INPUT_EVENTS].seq, next.seq);
    assert_eq!(
        consumer.outcomes[INPUT_EVENTS].outcome,
        no_send(NoSendReason::ConsumedKey)
    );
    assert_eq!(
        consumer.state.active.iter().flatten().count(),
        MAX_ACTIVE_KEYS - 1
    );
    assert_eq!(other.state.active.iter().flatten().count(), 1);
    drop((retry_lease, other_lease));
    // 连续两轮占满真实租约上限，释放后均可继续；不访问或重造私有计数。
    let mut seq = next.seq;
    for _ in 0..2 {
        let mut batch = Vec::new();
        for _ in 0..INPUT_EVENTS {
            seq += 1;
            let request = consumer.request(seq);
            batch.push(consumer.submit(&request, key(0, KeyCode::Character('a')))?);
        }
        drop(batch);
    }
    seq += 1;
    let final_request = consumer.request(seq);
    drop(consumer.submit(&final_request, key(0, KeyCode::Character('a')))?);
    Ok(())
}
