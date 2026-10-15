//! serde 1.0.229 expected/OneOf 与 serde_json 1.0.151 Error::custom 的源式。
//! 表只参与资源公式；最终错误仍由真实 typed decoder 判定，不做文字锁测试。

use crate::selected_p1::{
    decode_shape::{CostInputs, ErrorFormatInputs},
    model::{
        capacity::maximum_length,
        certificate::TargetLayouts,
        layout::{NumericReject, add, mul},
    },
};
use std::alloc::Layout;

const SETTINGS: &[&str] = &[
    "version",
    "terminal_type",
    "initial_cols",
    "initial_rows",
    "scrollback_lines",
    "font_family",
    "font_size",
    "color_scheme",
    "key_bindings",
    "left_alt_as_meta",
    "right_alt_as_meta",
    "enable_csi_u",
    "enable_kitty_keyboard",
    "mouse_reporting",
    "scroll_on_output",
    "scroll_on_keypress",
    "answerback",
    "extensions",
];
const OVERRIDES: &[&str] = &[
    "terminal_type",
    "initial_cols",
    "initial_rows",
    "scrollback_lines",
    "font_family",
    "font_size",
    "color_scheme",
    "key_bindings",
    "left_alt_as_meta",
    "right_alt_as_meta",
    "enable_csi_u",
    "enable_kitty_keyboard",
    "mouse_reporting",
    "scroll_on_output",
    "scroll_on_keypress",
    "answerback",
];
const CODE: &[&str] = &[
    "character",
    "enter",
    "escape",
    "tab",
    "backspace",
    "delete",
    "insert",
    "home",
    "end",
    "page_up",
    "page_down",
    "arrow_up",
    "arrow_down",
    "arrow_left",
    "arrow_right",
    "f",
];
const COLOR: &[&str] = &[
    "default",
    "one_dark",
    "solarized_dark",
    "solarized_light",
    "dracula",
    "monokai",
    "nord",
    "gruvbox_dark",
    "tokyo_night",
    "campbell_powershell",
];
const FIELD_SETS: &[&[&str]] = &[
    SETTINGS,
    OVERRIDES,
    &["version"],
    &["version", "key_bindings"],
    &["code", "modifiers", "action"],
    &["shift", "control", "alt", "super_key"],
    CODE,
    COLOR,
];
const EXPECTED: &[&str] = &[
    "struct TerminalSettingsV1",
    "struct TerminalOverrides",
    "struct OverridesDocument",
    "struct KeyBindingsDocument",
    "struct KeyBinding",
    "struct KeyModifiers",
    "enum KeyCode",
    "enum ColorScheme",
    "variant identifier",
    "field identifier",
    "unit variant",
    "newtype variant",
    "tuple variant",
    "struct variant",
    "a string",
    "a character",
    "a boolean",
    "u8",
    "u16",
    "u64",
    "usize",
    "f32",
    "a sequence",
    "a map",
    "any valid JSON value",
    "any value",
    "a string or map",
    "a map with a single key",
    "an empty map",
    "one element in map",
    "fewer elements in array",
    "a string or byte array",
    "a sequence or map",
    "a borrowed string",
    "a number",
    "variant index 0 <= i < 16",
    "variant index 0 <= i < 10",
    "field index 0 <= i < 18",
    "can only flatten structs and maps",
    "newtype variant KeyCode::Character",
    "newtype variant KeyCode::F",
];

fn one_of(names: &[&str]) -> Result<usize, NumericReject> {
    let mut bytes = match names.len() {
        0 => 0,
        1 | 2 => 0,
        _ => "one of ".len(),
    };
    for (index, name) in names.iter().enumerate() {
        if index != 0 {
            bytes = add(
                bytes,
                if names.len() == 2 {
                    " or ".len()
                } else {
                    ", ".len()
                },
            )?;
        }
        bytes = add(bytes, add(name.len(), 2)?)?;
    }
    Ok(bytes)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ErrorUpper {
    pub message_bytes: usize,
    pub initial_capacity: usize,
    pub source_peak: usize,
    pub message_peak: usize,
    pub boxed_message: usize,
    pub error_boxes: usize,
    pub total: usize,
}

pub fn bound(
    costs: CostInputs,
    position: usize,
    target: &TargetLayouts,
) -> Result<ErrorUpper, NumericReject> {
    let private = target.private_bounds()?;
    let mut expected = EXPECTED.iter().map(|text| text.len()).max().unwrap_or(0);
    let mut field_len = 0;
    for names in FIELD_SETS {
        expected = expected.max(one_of(names)?);
        field_len = field_len.max(names.iter().map(|text| text.len()).max().unwrap_or(0));
    }
    // derive seq expected 的实际字段数，Content ExpectedInSeq/Map 的输入 count。
    let mut digits = 1;
    let mut count = costs.error_count_upper().max(18);
    while count >= 10 {
        digits += 1;
        count /= 10;
    }
    for literal in [
        "struct TerminalSettingsV1 with  elements",
        "struct KeyModifiers with  elements",
        " elements in sequence",
        " elements in map",
    ] {
        expected = expected.max(add(literal.len(), digits)?);
    }
    // Unexpected 的非字符串分支：integer 最长20、float 使用 zmij inline24；其余固定模板。
    let unexpected = add("floating point ``".len(), 24)?
        .max(add("integer ``".len(), 20)?)
        .max("newtype variant".len());
    let templates = [
        ErrorFormatInputs {
            literal_bytes: "invalid type: , expected ".len(),
            expected_bytes: expected,
            other_dynamic_bytes: unexpected,
            has_arguments: true,
            ..Default::default()
        },
        ErrorFormatInputs {
            literal_bytes: "invalid value: , expected ".len(),
            expected_bytes: expected,
            other_dynamic_bytes: unexpected,
            has_arguments: true,
            ..Default::default()
        },
        ErrorFormatInputs {
            literal_bytes: "invalid value: string , expected ".len(),
            expected_bytes: expected,
            debug_strings: 1,
            has_arguments: true,
            ..Default::default()
        },
        ErrorFormatInputs {
            literal_bytes: "invalid type: string , expected ".len(),
            expected_bytes: expected,
            debug_strings: 1,
            has_arguments: true,
            ..Default::default()
        },
        ErrorFormatInputs {
            literal_bytes: "invalid length , expected ".len(),
            expected_bytes: expected,
            decimal_counts: 1,
            has_arguments: true,
            ..Default::default()
        },
        ErrorFormatInputs {
            literal_bytes: "unknown variant ``, expected ".len(),
            expected_bytes: expected,
            display_strings: 1,
            has_arguments: true,
            ..Default::default()
        },
        ErrorFormatInputs {
            literal_bytes: "unknown field ``, expected ".len(),
            expected_bytes: expected,
            display_strings: 1,
            has_arguments: true,
            ..Default::default()
        },
        ErrorFormatInputs {
            literal_bytes: "missing field ``".len(),
            other_dynamic_bytes: field_len,
            has_arguments: true,
            ..Default::default()
        },
        ErrorFormatInputs {
            literal_bytes: "duplicate field ``".len(),
            other_dynamic_bytes: field_len,
            has_arguments: true,
            ..Default::default()
        },
        ErrorFormatInputs {
            literal_bytes: "unsupported terminal settings version ".len(),
            other_dynamic_bytes: 3,
            has_arguments: true,
            ..Default::default()
        },
        ErrorFormatInputs {
            literal_bytes: " at line  column ".len(),
            expected_bytes: expected,
            decimal_counts: 2,
            decimal_value_upper: position,
            has_arguments: true,
            starts_with_argument: true,
            ..Default::default()
        },
        ErrorFormatInputs {
            literal_bytes: "invalid value: character ``, expected ".len(),
            expected_bytes: expected,
            other_dynamic_bytes: 4,
            has_arguments: true,
            ..Default::default()
        },
    ];
    let mut message_bytes = 0;
    let mut initial_capacity = 0;
    for template in templates {
        let upper = template.upper(costs).map_err(|_| NumericReject::Overflow)?;
        message_bytes = message_bytes.max(upper.output_bytes);
        initial_capacity = initial_capacity.max(upper.initial_capacity);
    }
    let source_literal = "unsupported terminal settings version ".len();
    let source_peak = maximum_length(
        Layout::new::<u8>(),
        mul(source_literal, 2)?,
        add(source_literal, 3)?,
    )?
    .peak_bytes;
    let message_peak =
        maximum_length(Layout::new::<u8>(), initial_capacity, message_bytes)?.peak_bytes;
    let boxed_message = message_bytes;
    let error_boxes = mul(private.error_impl_upper.size(), 2)?;
    let total = add(
        add(source_peak, message_peak)?,
        add(boxed_message, error_boxes)?,
    )?;
    Ok(ErrorUpper {
        message_bytes,
        initial_capacity,
        source_peak,
        message_peak,
        boxed_message,
        error_boxes,
        total,
    })
}
