//! 设置（上游默认 `TerminalProfile` 的字体、字号与滚回行数）。

use rinf::{DartSignal, RustSignal};
use serde::{Deserialize, Serialize};

/// 取当前设置。
#[derive(Deserialize, DartSignal)]
pub struct SettingsQuery {}

/// 只提交被修改的字段，避免尚未收到旧回包时覆盖其他设置。
#[derive(Default, Deserialize, DartSignal)]
pub struct SaveSettings {
    pub font_family: Option<String>,
    pub font_size: Option<f64>,
    pub scrollback_lines: Option<u32>,
    pub show_key_bar: Option<bool>,
}

/// 键位条排布独立保存，避免覆盖同时修改的字体和滚回设置。
#[derive(Deserialize, DartSignal)]
pub struct SaveKeyBarLayout {
    pub rows: Vec<Vec<String>>,
}

#[derive(Serialize, RustSignal)]
pub struct KeyBarLayoutResult {
    pub ok: bool,
    pub detail: String,
}

/// 当前设置。设置变化后重发。
#[derive(Serialize, RustSignal)]
pub struct SettingsState {
    pub font_family: String,
    pub font_size: f64,
    /// 可选的字体（App 内置或系统自带）。
    pub font_families: Vec<String>,
    pub min_font_size: f64,
    pub max_font_size: f64,
    /// 每个会话保留的滚回行数（已按本机上界收窄）。
    pub scrollback_lines: u32,
    /// 本机的滚回上界（按物理内存分档）。
    pub max_scrollback_lines: u32,
    /// 终端下方显示键位条（Esc、Tab、方向键、Ctrl、Alt…）。
    pub show_key_bar: bool,
    /// 两排按钮的稳定标识；空排允许保留。
    pub key_bar_rows: Vec<Vec<String>>,
}
