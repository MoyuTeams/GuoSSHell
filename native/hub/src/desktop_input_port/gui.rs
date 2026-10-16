//! GUI 意图只冻结语义与修饰状态，不读取或创建 terminal/physical 状态。

use rshell_m0::rshell_core::{KeyCode, KeyModifiers, TerminalInput};

use super::{EVENT_BYTES, Reject};
use crate::signals::desktop_native_input::{GuiInput, GuiIntent, GuiKey};

const CTRL: u8 = 1;
const ALT: u8 = 2;
const MODIFIERS: u8 = CTRL | ALT;
const MAX_TEXT_SCALARS: usize = 32;

pub(super) struct PreparedGui {
    pub input: TerminalInput,
    pub bytes: usize,
    pub consumed_latches: u8,
}

pub(super) fn prepare(intent: GuiIntent) -> Result<PreparedGui, Reject> {
    let modifiers = intent.modifiers;
    if (modifiers.button | modifiers.latched | modifiers.locked) & !MODIFIERS != 0 {
        return Err(Reject::InvalidEvent);
    }
    let (code, text, scalars, payload, intrinsic) = match intent.input {
        GuiInput::GuiKey(key) => {
            let (code, intrinsic) = key_code(key)?;
            (Some(code), None, 1, 0, intrinsic)
        }
        GuiInput::GuiText(text) => {
            let mut chars = text.chars();
            let first = chars.next().ok_or(Reject::InvalidEvent)?;
            let scalars = 1 + chars.take(MAX_TEXT_SCALARS).count();
            if scalars > MAX_TEXT_SCALARS {
                return Err(Reject::TooLarge);
            }
            let payload = text.len();
            let code = (scalars == 1).then_some(KeyCode::Character(first));
            (code, Some(text), scalars, payload, 0)
        }
    };
    let explicit = modifiers.button | intrinsic;
    let effective = explicit | modifiers.locked | modifiers.latched;
    let uses_modifiers = code.is_some() && (text.is_none() || effective != 0);
    let consumed_latches = if uses_modifiers {
        modifiers.latched & !(explicit | modifiers.locked)
    } else {
        0
    };
    let input = match (code, text) {
        (Some(code), _) if uses_modifiers => TerminalInput::Key {
            code,
            modifiers: KeyModifiers {
                control: effective & CTRL != 0,
                alt: effective & ALT != 0,
                ..KeyModifiers::default()
            },
        },
        (_, Some(text)) => TerminalInput::CommittedText(text),
        _ => return Err(Reject::InvalidEvent),
    };
    // 保留原文/编码临时副本与每字符协议展开的余量，租约覆盖原 write future。
    let bytes = EVENT_BYTES + payload * 3 + scalars * 32;
    Ok(PreparedGui {
        input,
        bytes,
        consumed_latches,
    })
}

fn key_code(key: GuiKey) -> Result<(KeyCode, u8), Reject> {
    Ok(match key {
        GuiKey::Enter => (KeyCode::Enter, 0),
        GuiKey::Escape => (KeyCode::Escape, 0),
        GuiKey::Tab => (KeyCode::Tab, 0),
        GuiKey::Backspace => (KeyCode::Backspace, 0),
        GuiKey::ArrowUp => (KeyCode::ArrowUp, 0),
        GuiKey::ArrowDown => (KeyCode::ArrowDown, 0),
        GuiKey::ArrowLeft => (KeyCode::ArrowLeft, 0),
        GuiKey::ArrowRight => (KeyCode::ArrowRight, 0),
        GuiKey::Delete => (KeyCode::Delete, 0),
        GuiKey::Home => (KeyCode::Home, 0),
        GuiKey::End => (KeyCode::End, 0),
        GuiKey::PageUp => (KeyCode::PageUp, 0),
        GuiKey::PageDown => (KeyCode::PageDown, 0),
        GuiKey::F(number) if (1..=12).contains(&number) => (KeyCode::F(number), 0),
        GuiKey::F(_) => return Err(Reject::InvalidEvent),
        GuiKey::CtrlC => (KeyCode::Character('c'), CTRL),
        GuiKey::CtrlD => (KeyCode::Character('d'), CTRL),
        GuiKey::CtrlZ => (KeyCode::Character('z'), CTRL),
        GuiKey::CtrlL => (KeyCode::Character('l'), CTRL),
    })
}
