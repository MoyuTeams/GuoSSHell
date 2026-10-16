//! 独立扫描器 harness：通用 JSON/IgnoredAny 差分及可访问 core derive/resolve 的有限证据。
//! 不覆盖 storage 私有 mapping wrapper、UUID/validator 或完整 selected owner/grant。

#[path = "../src/selected_p1/decode_shape/mod.rs"]
pub mod decode_shape;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use decode_shape::{
    ContainerKind, ContainerPrefix, CostInputs, ErrorFormatInputs, NumericReject, PrefixErrorKind,
    ScratchFrame, ScratchRequirement, Shape, ShapeStatus, needed_scratch, scan, scan_with,
};
use rshell_m0::rshell_core::{
    ColorScheme, KeyBinding, KeyCode, KeyModifiers, ResolvedTerminalProfile, TerminalOverrides,
    TerminalSettingsV1,
};
use serde::{Deserialize, de::IgnoredAny};

thread_local! {
    static AUDIT_ACTIVE: Cell<bool> = const { Cell::new(false) };
    static AUDIT_ALLOCS: Cell<usize> = const { Cell::new(0) };
}

struct ScannerAuditAllocator;

fn allocation_event() {
    let _ = AUDIT_ACTIVE.try_with(|active| {
        if active.get() {
            let _ = AUDIT_ALLOCS.try_with(|count| count.set(count.get().saturating_add(1)));
        }
    });
}

// 仅这个独立 test executable 安装审计器；不改变 hub 主库 allocator。
unsafe impl GlobalAlloc for ScannerAuditAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        allocation_event();
        // SAFETY: 按 GlobalAlloc 原始契约转交同一 Layout。
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        allocation_event();
        // SAFETY: 按 GlobalAlloc 原始契约转交同一 Layout。
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        allocation_event();
        // SAFETY: 指针、旧 Layout 与新 size 均由调用者的 allocator 契约保证。
        unsafe { System.realloc(ptr, layout, size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: 配对释放同一 System allocation 和原 Layout。
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: ScannerAuditAllocator = ScannerAuditAllocator;

struct AuditGuard;

impl Drop for AuditGuard {
    fn drop(&mut self) {
        AUDIT_ACTIVE.with(|active| active.set(false));
    }
}

fn without_allocation<T>(f: impl FnOnce() -> T) -> T {
    AUDIT_ALLOCS.with(|count| count.set(0));
    AUDIT_ACTIVE.with(|active| assert!(!active.replace(true)));
    let guard = AuditGuard;
    let result = f();
    drop(guard);
    assert_eq!(AUDIT_ALLOCS.with(Cell::get), 0);
    result
}

fn succeeds<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("预期成功：{error:?}"),
    }
}

fn shape(input: &[u8]) -> Shape {
    let requirement = succeeds(needed_scratch(input));
    let mut scratch = vec![ScratchFrame::default(); requirement.slots];
    succeeds(without_allocation(|| scan(input, &mut scratch)))
}

fn complete(input: &[u8]) -> CostInputs {
    let result = shape(input);
    assert_eq!(result.status, ShapeStatus::Complete);
    assert_eq!(result.prefix_bytes, input.len());
    result.costs
}

fn invalid(input: &[u8], kind: PrefixErrorKind) -> Shape {
    let result = shape(input);
    let ShapeStatus::Invalid(error) = result.status else {
        panic!("预期语法失败");
    };
    assert_eq!(error.kind, kind);
    assert!(error.offset <= input.len());
    result
}

#[test]
fn hand_counted_duplicate_keys_and_nested_values() {
    let input = "{\"a\":[\"é\",null],\"a\":{\"b\":\"\\uD83D\\uDE00\"}}".as_bytes();
    let c = complete(input);
    assert_eq!(c.tokens_started, 9);
    assert_eq!(c.strings_started, 5);
    assert_eq!(c.keys_started, 3);
    assert_eq!(c.decoded_string_bytes_upper, 9);
    assert_eq!(c.max_decoded_string_bytes_upper, 4);
    assert_eq!(c.max_string_scalars_upper, 1);
    assert_eq!(c.escaped_strings, 1);
    assert_eq!(c.escape_sequences, 2);
    assert_eq!(c.arrays.occurrences, 1);
    assert_eq!(c.arrays.started_members, 2);
    assert_eq!(c.arrays.completed_members, 2);
    assert_eq!(c.objects.occurrences, 2);
    assert_eq!(c.objects.started_members, 3);
    assert_eq!(c.objects.completed_members, 3);
    assert_eq!(c.objects.max_started_members, 2);
    assert_eq!(
        (c.max_depth, c.max_array_depth, c.max_object_depth),
        (2, 1, 2)
    );
    let value: serde_json::Value = succeeds(serde_json::from_slice(input));
    assert_eq!(value.as_object().map(|object| object.len()), Some(1));
}

#[test]
fn empty_tokens_and_empty_input_are_distinct() {
    for input in [b"[]".as_slice(), b"{}", b"\"\"", b"null", b"true", b"false"] {
        let c = complete(input);
        assert_eq!(c.tokens_started, 1);
        assert_eq!(c.decoded_string_bytes_upper, 0);
        assert_eq!(c.arrays.started_members + c.objects.started_members, 0);
        assert!(serde_json::from_slice::<IgnoredAny>(input).is_ok());
    }
    let c = complete(b" \t\n [ ]\r ");
    assert_eq!(c.arrays.occurrences, 1);
    assert_eq!(c.max_depth, 1);
    let result = invalid(b" \r\n", PrefixErrorKind::UnexpectedEnd);
    assert_eq!(result.costs.tokens_started, 0);
    assert_eq!(result.prefix_bytes, 3);
}

#[test]
fn escaped_unicode_nul_and_reserve_four_have_separate_inputs() {
    let c = complete("\"é中😀\\u0000\\u0080\\u0800\\uD83D\\uDE00\\n\"".as_bytes());
    assert_eq!(c.decoded_string_bytes_upper, 20);
    assert_eq!(c.max_string_scalars_upper, 8);
    assert_eq!(c.escape_sequences, 6);
    assert_eq!(c.max_string_scratch_required_upper, 20);
    let c = complete(br#""a\u0080""#);
    assert_eq!(c.decoded_string_bytes_upper, 3);
    assert_eq!(c.max_string_scratch_required_upper, 5);
    assert_eq!(c.parser_scratch_required_upper(), 5);
    let text: String = succeeds(serde_json::from_slice(br#""a\u0080""#));
    assert_eq!(text.as_bytes(), &[b'a', 0xC2, 0x80]);
    invalid(b"\"a\0b\"", PrefixErrorKind::ControlInString);
}

#[test]
fn partial_strings_and_containers_keep_allocatable_prefix() {
    let input = br#"{"a":["abcdefgh\nxyz"#;
    let result = invalid(input, PrefixErrorKind::UnexpectedEnd);
    assert_eq!(result.prefix_bytes, input.len());
    assert_eq!(result.costs.tokens_started, 4);
    assert_eq!(result.costs.decoded_string_bytes_upper, 13);
    assert_eq!(result.costs.max_decoded_string_bytes_upper, 12);
    assert_eq!(result.costs.max_string_scratch_required_upper, 12);
    assert_eq!(result.costs.arrays.started_members, 1);
    assert_eq!(result.costs.arrays.completed_members, 0);
    assert_eq!(result.costs.objects.started_members, 1);
    assert_eq!(result.costs.objects.completed_members, 0);
    assert_eq!(result.costs.max_depth, 2);
    assert!(serde_json::from_slice::<IgnoredAny>(input).is_err());
    assert!(serde_json::from_slice::<serde_json::Value>(input).is_err());
    let result = invalid(br#"["abc\u00"#, PrefixErrorKind::UnexpectedEnd);
    assert_eq!(result.costs.decoded_string_bytes_upper, 3);
    assert_eq!(result.costs.max_string_scratch_required_upper, 3);
    let result = invalid(br#"["abc\q"]"#, PrefixErrorKind::InvalidEscape);
    assert_eq!(result.costs.decoded_string_bytes_upper, 3);
    assert_eq!(result.costs.arrays.started_members, 1);
    let result = invalid(b"[\"ok\",]", PrefixErrorKind::TrailingComma);
    assert_eq!(result.costs.arrays.started_members, 1);
    assert_eq!(result.costs.arrays.completed_members, 1);
}

#[test]
fn replay_reports_each_container_including_unfinished_members() {
    let input = br#"{"x":[[],{"y":"bad\u"#;
    let mut scratch = vec![ScratchFrame::default(); succeeds(needed_scratch(input)).slots];
    let mut records = [None; 4];
    let mut count = 0;
    let result = succeeds(without_allocation(|| {
        scan_with(input, &mut scratch, |prefix| {
            records[count] = Some(prefix);
            count += 1;
            Ok(())
        })
    }));
    assert!(matches!(result.status, ShapeStatus::Invalid(_)));
    assert_eq!(count, 4);
    assert_eq!(
        records[0],
        Some(ContainerPrefix {
            kind: ContainerKind::Array,
            start: 6,
            end: 8,
            started_members: 0,
            completed_members: 0,
            closed: true,
        })
    );
    assert_eq!(records[1].map(|r| r.kind), Some(ContainerKind::Object));
    assert_eq!(records[1].map(|r| r.started_members), Some(1));
    assert_eq!(records[1].map(|r| r.completed_members), Some(0));
    assert_eq!(records[1].map(|r| r.closed), Some(false));
    assert_eq!(records[2].map(|r| r.started_members), Some(2));
    assert_eq!(records[2].map(|r| r.completed_members), Some(1));
    assert_eq!(records[3].map(|r| r.started_members), Some(1));
    assert_eq!(records[3].map(|r| r.completed_members), Some(0));
}

#[test]
fn scratch_is_checked_before_scanning_and_is_not_a_depth_policy() {
    let input = br#"[{},["[{}]",[]]]"#;
    let requirement = succeeds(needed_scratch(input));
    assert_eq!(requirement.slots, 3);
    assert_eq!(
        requirement.bytes,
        succeeds(Layout::array::<ScratchFrame>(3)).size()
    );
    assert_eq!(requirement.alignment, std::mem::align_of::<ScratchFrame>());
    let mut short = [ScratchFrame::default(); 2];
    assert_eq!(
        without_allocation(|| scan(input, &mut short)),
        Err(NumericReject::NeededScratch(requirement))
    );
    let mut enough = [ScratchFrame::default(); 3];
    assert_eq!(
        succeeds(without_allocation(|| scan(input, &mut enough))).status,
        ShapeStatus::Complete
    );
    assert_eq!(succeeds(needed_scratch(br#""\"[{}]""#)).slots, 0);
    assert_eq!(
        ScratchRequirement::for_input_len(usize::MAX),
        Err(NumericReject::ArithmeticOverflow)
    );
    assert_eq!(complete(br#"["[{}]",{}]"#).max_depth, 2);
}

#[test]
fn ignored_deep_branch_and_surrogates_remain_accepted_by_real_core() {
    let depth = 4096;
    let nested = format!("{}\"\\uD800\"{}", "[".repeat(depth), "]".repeat(depth));
    let input = format!("{{\"terminal_type\":\"x\",\"unknown\":{nested}}}");
    let c = complete(input.as_bytes());
    assert_eq!(c.max_depth, depth + 1);
    assert_eq!(c.ignored_stack_required_upper, depth + 1);
    assert_eq!(c.arrays.occurrences, depth);
    assert_eq!(c.arrays.started_members, depth);
    assert_eq!(c.unpaired_surrogates, 1);
    assert!(serde_json::from_slice::<IgnoredAny>(input.as_bytes()).is_ok());
    let overrides: TerminalOverrides = succeeds(serde_json::from_slice(input.as_bytes()));
    assert_eq!(overrides.terminal_type.as_deref(), Some("x"));
    assert!(serde_json::from_slice::<serde_json::Value>(input.as_bytes()).is_err());
    let truncated = &input.as_bytes()[..input.len() - 2];
    invalid(truncated, PrefixErrorKind::UnexpectedEnd);
    assert!(serde_json::from_slice::<TerminalOverrides>(truncated).is_err());
}

#[test]
fn owned_unicode_risks_do_not_become_universal_rejections() {
    for input in [
        br#""\uD800""#.as_slice(),
        br#""\uDC00""#,
        br#""\uD800\u0041""#,
    ] {
        let c = complete(input);
        assert_eq!(c.unpaired_surrogates, 1);
        assert!(serde_json::from_slice::<IgnoredAny>(input).is_ok());
        assert!(serde_json::from_slice::<String>(input).is_err());
    }
    for input in [
        b"\"\xFF\"".as_slice(),
        b"\"\xC0\xAF\"",
        b"\"\xED\xA0\x80\"",
        b"\"\xF4\x90\x80\x80\"",
    ] {
        let c = complete(input);
        assert_eq!(c.strings_with_invalid_utf8, 1);
        assert_eq!(c.max_decoded_string_bytes_upper, input.len() - 2);
        assert!(serde_json::from_slice::<IgnoredAny>(input).is_ok());
        assert!(serde_json::from_slice::<String>(input).is_err());
    }
    assert_eq!(complete(br#""\uD83D\uDE00""#).unpaired_surrogates, 0);
}

#[test]
fn all_hex_units_match_owned_and_ignored_unicode_boundaries() {
    let hex = b"0123456789abcdef";
    let mut input = *br#""\u0000""#;
    for unit in 0u16..=u16::MAX {
        for (index, shift) in [12, 8, 4, 0].into_iter().enumerate() {
            input[index + 3] = hex[usize::from((unit >> shift) & 0xF)];
        }
        let c = complete(&input);
        let surrogate = (0xD800..=0xDFFF).contains(&unit);
        let bytes = if unit < 0x80 {
            1
        } else if unit < 0x800 {
            2
        } else {
            3
        };
        assert_eq!(c.decoded_string_bytes_upper, bytes);
        assert_eq!(c.max_string_scalars_upper, 1);
        assert_eq!(c.unpaired_surrogates, usize::from(surrogate));
        assert_eq!(
            c.max_string_scratch_required_upper,
            if unit < 0x80 { 1 } else { 4 }
        );
        assert!(serde_json::from_slice::<IgnoredAny>(&input).is_ok());
        assert_eq!(serde_json::from_slice::<String>(&input).is_ok(), !surrogate);
    }
}

#[test]
fn numeric_range_is_deferred_and_differentiated_from_number_grammar() {
    for (input, value_accepts) in [
        ("18446744073709551615", true),
        ("18446744073709551616", true),
        ("-9223372036854775809", true),
        ("1e400", false),
        ("0e99999999999999999999", true),
        ("1e-99999999999999999999", true),
        ("-0", true),
    ] {
        let c = complete(input.as_bytes());
        assert_eq!(c.numbers_started, 1);
        assert_eq!(c.max_number_bytes, input.len());
        assert!(serde_json::from_slice::<IgnoredAny>(input.as_bytes()).is_ok());
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(input.as_bytes()).is_ok(),
            value_accepts
        );
    }
    let c = complete(b"256");
    assert_eq!(c.numbers_started, 1);
    assert!(serde_json::from_slice::<u8>(b"256").is_err());
    for input in [
        b"01".as_slice(),
        b"-",
        b"1.",
        b"1e",
        b"1e+",
        b"1.e2",
        b"+1",
        b"NaN",
        b"[1e+]",
        b"[1 2]",
    ] {
        assert!(matches!(shape(input).status, ShapeStatus::Invalid(_)));
        assert!(serde_json::from_slice::<IgnoredAny>(input).is_err());
        assert!(serde_json::from_slice::<serde_json::Value>(input).is_err());
    }
}

#[test]
fn generic_grammar_matches_ignored_any_on_short_exhaustive_inputs() {
    let alphabet = b"[]{}:,\"0n\\ \n";
    let mut input = [0; 4];
    let mut scratch = [ScratchFrame::default(); 4];
    for len in 0..=input.len() {
        for mut code in 0..alphabet.len().pow(len as u32) {
            for byte in &mut input[..len] {
                *byte = alphabet[code % alphabet.len()];
                code /= alphabet.len();
            }
            let input = &input[..len];
            let result = succeeds(without_allocation(|| scan(input, &mut scratch)));
            assert_eq!(
                result.status == ShapeStatus::Complete,
                serde_json::from_slice::<IgnoredAny>(input).is_ok(),
                "输入 {input:?}"
            );
            assert!(result.costs.tokens_started <= result.prefix_bytes);
            assert!(result.costs.strings_started <= result.costs.tokens_started);
            assert!(result.prefix_bytes <= input.len());
        }
    }
}

#[test]
fn grammar_errors_and_escape_tails_match_both_generic_oracles() {
    for input in [
        br#"{"x" 1}"#.as_slice(),
        br#"{x:1}"#,
        br#"{"x":1,}"#,
        b"[}",
        b"{]",
        b"true false",
        b"nulx",
        b"[truefalse]",
        br#""\uQQQQ""#,
        br#""\uD800\u00Q1""#,
        br#""\uD800\q""#,
        br#""\uD800\""#,
        b"\"\\",
        b"\"x\x1F\"",
    ] {
        let result = shape(input);
        assert!(
            matches!(result.status, ShapeStatus::Invalid(_)),
            "输入 {input:?}"
        );
        assert!(serde_json::from_slice::<IgnoredAny>(input).is_err());
        assert!(serde_json::from_slice::<serde_json::Value>(input).is_err());
        assert!(result.costs.tokens_started <= result.prefix_bytes);
    }
}

#[test]
fn actual_binding_struct_array_has_hand_counted_metrics() {
    let input = "[\"enter\",[false,true,false,false],\"send:\\u0000é\"]".as_bytes();
    let c = complete(input);
    assert_eq!(c.tokens_started, 8);
    assert_eq!(c.strings_started, 2);
    assert_eq!(c.decoded_string_bytes_upper, 13);
    assert_eq!(c.max_string_scalars_upper, 7);
    assert_eq!(c.arrays.occurrences, 2);
    assert_eq!(c.arrays.started_members, 7);
    assert_eq!(c.max_array_depth, 2);
    let binding: KeyBinding = succeeds(serde_json::from_slice(input));
    assert_eq!(
        binding,
        KeyBinding {
            code: KeyCode::Enter,
            modifiers: KeyModifiers {
                shift: false,
                control: true,
                alt: false,
                super_key: false
            },
            action: "send:\0é".into(),
        }
    );
    for input in [
        br#"["enter",[false],"a"]"#.as_slice(),
        br#"["enter",[false,true,false,false]]"#,
    ] {
        complete(input);
        assert!(serde_json::from_slice::<KeyBinding>(input).is_err());
    }
}

#[derive(Debug, Deserialize)]
struct FlattenFixture {
    version: u8,
    #[serde(flatten)]
    options: TerminalOverrides,
}

#[test]
fn flatten_content_duplicates_defaults_and_error_prefix_are_counted() {
    let input = br#"{"version":1,"terminal_type":"a","terminal_type":"b","extra":[{},"\u0000"]}"#;
    let c = complete(input);
    assert_eq!(c.objects.started_members, 4);
    assert_eq!(c.objects.occurrences, 2);
    assert_eq!(c.arrays.started_members, 2);
    assert_eq!(c.strings_started, 7);
    assert!(serde_json::from_slice::<FlattenFixture>(input).is_err());
    // 真正 core derive 的 unknown 是 IgnoredAny；flatten fixture 先缓冲 Content。
    let unpaired = br#"{"version":1,"extra":"\uD800"}"#;
    complete(unpaired);
    assert!(serde_json::from_slice::<TerminalOverrides>(unpaired).is_ok());
    assert!(serde_json::from_slice::<FlattenFixture>(unpaired).is_err());
    let valid: FlattenFixture =
        succeeds(serde_json::from_slice(br#"{"version":1,"answerback":"x"}"#));
    assert_eq!(valid.version, 1);
    assert_eq!(valid.options.answerback.as_deref(), Some("x"));
    complete(b"{}");
    assert_eq!(
        succeeds(serde_json::from_slice::<TerminalOverrides>(b"{}")),
        TerminalOverrides::default()
    );
    assert!(serde_json::from_slice::<TerminalSettingsV1>(b"{}").is_err());
    let prefix = br#"{"terminal_type":"first","terminal_type":["allocated"]}"#;
    let c = complete(prefix);
    assert_eq!(c.objects.started_members, 2);
    assert_eq!(c.arrays.started_members, 1);
    assert!(serde_json::from_slice::<TerminalOverrides>(prefix).is_err());
}

#[test]
fn actual_settings_object_array_and_full_resolve_preserve_extensions() {
    let object = br#"{"version":1,"terminal_type":" x ","initial_cols":0,"initial_rows":65535,"scrollback_lines":0,"font_family":" F ","font_size":18,"color_scheme":"default","key_bindings":[],"left_alt_as_meta":true,"right_alt_as_meta":false,"enable_csi_u":false,"enable_kitty_keyboard":true,"mouse_reporting":true,"scroll_on_output":true,"scroll_on_keypress":false,"answerback":" A ","extensions":{"future":{"v":[1,null,"\u00e9"]}}}"#;
    let array = br#"[1," x ",0,65535,0," F ",18,"default",[],true,false,false,true,true,true,false," A ",{"future":{"v":[1,null,"\u00e9"]}}]"#;
    complete(object);
    let array_costs = complete(array);
    assert_eq!(array_costs.arrays.occurrences, 3);
    assert_eq!(array_costs.arrays.started_members, 21);
    let object_settings: TerminalSettingsV1 = succeeds(serde_json::from_slice(object));
    let array_settings: TerminalSettingsV1 = succeeds(serde_json::from_slice(array));
    assert_eq!(object_settings, array_settings);
    let overrides: TerminalOverrides = succeeds(serde_json::from_slice(
        br#"{"terminal_type":" ","initial_cols":42}"#,
    ));
    let app: Vec<KeyBinding> = succeeds(serde_json::from_slice(br#"[["enter",[false,false,false,false],"copy"],["enter",[false,false,false,false],"paste"]]"#));
    let resolved = object_settings
        .resolve(&overrides)
        .with_app_key_bindings(&app);
    assert_eq!(
        resolved,
        ResolvedTerminalProfile {
            terminal_type: "xterm-256color".into(),
            cols: 42,
            rows: 999,
            scrollback_lines: 100,
            font_family: "F".into(),
            font_size: 18.0,
            color_scheme: ColorScheme::Default,
            key_bindings: vec![app[0].clone()],
            left_alt_as_meta: true,
            right_alt_as_meta: false,
            enable_csi_u: false,
            enable_kitty_keyboard: true,
            mouse_reporting: true,
            scroll_on_output: true,
            scroll_on_keypress: false,
            answerback: "A".into(),
            extensions: object_settings.extensions.clone(),
        }
    );
    assert_eq!(
        array_settings
            .resolve(&overrides)
            .with_app_key_bindings(&app),
        resolved
    );
    for input in [
        br#"{"version":2}"#.as_slice(),
        br#"[2]"#,
        br#"{"version":1,"initial_cols":65536}"#,
    ] {
        complete(input);
        assert!(serde_json::from_slice::<TerminalSettingsV1>(input).is_err());
    }
}

#[test]
fn error_format_inputs_are_checked_without_owned_error_in_scanner() {
    let c = complete("\"a\\n\\u0000é\"".as_bytes());
    assert_eq!(c.max_decoded_string_bytes_upper, 5);
    assert_eq!(c.max_string_scalars_upper, 4);
    assert_eq!(succeeds(c.debug_string_bytes_upper()), 42);
    let template = ErrorFormatInputs {
        literal_bytes: 17,
        expected_bytes: 9,
        debug_strings: 1,
        decimal_counts: 1,
        has_arguments: true,
        ..ErrorFormatInputs::default()
    };
    let upper = succeeds(without_allocation(|| template.upper(c)));
    assert_eq!(upper.output_bytes, 69);
    assert_eq!(upper.initial_capacity, 34);
    let upper = succeeds(
        ErrorFormatInputs {
            literal_bytes: 3,
            display_strings: 1,
            starts_with_argument: true,
            has_arguments: true,
            ..ErrorFormatInputs::default()
        }
        .upper(c),
    );
    assert_eq!((upper.output_bytes, upper.initial_capacity), (8, 0));
    assert_eq!(
        succeeds(
            ErrorFormatInputs {
                literal_bytes: 7,
                ..ErrorFormatInputs::default()
            }
            .upper(c)
        )
        .initial_capacity,
        7
    );
    assert_eq!(
        ErrorFormatInputs {
            literal_bytes: usize::MAX,
            expected_bytes: 1,
            ..ErrorFormatInputs::default()
        }
        .upper(c),
        Err(NumericReject::ArithmeticOverflow)
    );
    assert_eq!(
        CostInputs {
            max_string_scalars_upper: usize::MAX,
            ..CostInputs::default()
        }
        .debug_string_bytes_upper(),
        Err(NumericReject::ArithmeticOverflow)
    );
    assert_eq!(
        ErrorFormatInputs {
            display_strings: usize::MAX,
            ..ErrorFormatInputs::default()
        }
        .upper(c),
        Err(NumericReject::ArithmeticOverflow)
    );
    // 真实 primitive visitor 的 Debug 输入错误；输出仅在 oracle 中创建，且不锁措辞。
    let input = br#""a\n\u0000\u00e9""#;
    let costs = complete(input);
    let error = match serde_json::from_slice::<u16>(input) {
        Err(error) => error,
        Ok(_) => panic!("预期类型错误"),
    };
    let upper = succeeds(
        ErrorFormatInputs {
            literal_bytes: "invalid type: string , expected  at line  column ".len(),
            expected_bytes: "u16".len(),
            debug_strings: 1,
            decimal_counts: 2,
            decimal_value_upper: input.len(),
            has_arguments: true,
            ..ErrorFormatInputs::default()
        }
        .upper(costs),
    );
    assert!(error.to_string().len() <= upper.output_bytes);
}

#[test]
fn position_value_upper_covers_serde_decimal_carry() {
    for (input, expected_line, expected_column, expected_upper) in [
        (b"\n\n\n\n\n\n\n\n\n".as_slice(), 10, 0, 10),
        (b"         ?".as_slice(), 1, 10, 11),
    ] {
        let result = shape(input);
        let position_upper = succeeds(without_allocation(|| result.position_value_upper()));
        assert_eq!(position_upper, expected_upper);
        let error = match serde_json::from_slice::<serde_json::Value>(input) {
            Err(error) => error,
            Ok(_) => panic!("预期位置错误"),
        };
        assert_eq!(
            (error.line(), error.column()),
            (expected_line, expected_column)
        );
        assert!(error.line() <= position_upper);
        assert!(error.column() <= position_upper);
        let upper = succeeds(without_allocation(|| {
            ErrorFormatInputs {
                decimal_counts: 2,
                decimal_value_upper: position_upper,
                has_arguments: true,
                ..ErrorFormatInputs::default()
            }
            .upper(result.costs)
        }));
        assert_eq!(upper.output_bytes, 4);
        let actual_digits = error.line().to_string().len() + error.column().to_string().len();
        assert_eq!(actual_digits, 3);
        assert!(actual_digits <= upper.output_bytes);
    }
}

#[test]
fn position_value_upper_checks_overflow() {
    let result = Shape {
        input_bytes: usize::MAX,
        ..shape(b"")
    };
    assert_eq!(
        without_allocation(|| result.position_value_upper()),
        Err(NumericReject::ArithmeticOverflow)
    );
}

#[test]
fn scanner_allocation_audit_is_thread_scoped_and_covers_numeric_failure() {
    let nested = format!(
        "{}[\"a\\u0080\",{{\"x\":0}}]{}",
        "[".repeat(2048),
        "]".repeat(2048)
    );
    let mut scratch =
        vec![ScratchFrame::default(); succeeds(needed_scratch(nested.as_bytes())).slots];
    let handle = std::thread::spawn(|| {
        let allocations_outside_scanner = vec![0u8; 8192];
        assert_eq!(allocations_outside_scanner.len(), 8192);
    });
    let result = succeeds(without_allocation(|| {
        let requirement = needed_scratch(nested.as_bytes())?;
        assert!(requirement.bytes > 0);
        scan_with(nested.as_bytes(), &mut scratch, |_| Ok(()))
    }));
    assert_eq!(result.status, ShapeStatus::Complete);
    succeeds(handle.join());
    let mut empty = [];
    assert!(matches!(
        without_allocation(|| scan(b"[", &mut empty)),
        Err(NumericReject::NeededScratch(_))
    ));
    let mut one = [ScratchFrame::default(); 1];
    assert!(matches!(
        succeeds(without_allocation(|| scan(br#"["partial\u"#, &mut one))).status,
        ShapeStatus::Invalid(_)
    ));
}
