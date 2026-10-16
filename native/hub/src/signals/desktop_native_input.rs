//! 正常 NativePort 的非键命令；此信号不能证明 OS 原生按键或文本来源。
use super::desktop_session::{DesktopAction, DesktopEvent, DesktopSource};
use rinf::{DartSignal, RustSignal, SignalPiece};
use serde::{Deserialize, Serialize};

pub const VERSION: u16 = 1;

/// 原 actor/port 的只读生命周期；这些字段不证明 OS 来源，也不能登记输入 surface。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, SignalPiece, RustSignal)]
pub struct DesktopNativeLifecycle {
    pub version: u16,
    pub source: DesktopSource,
    pub identity: String,
    /// Registered 尚未创建 attempt；消费者须等待 Connecting，不得猜测编号。
    pub attempt: u64,
    pub focus_epoch: u64,
    pub lifecycle_revision: u64,
    pub state: NativeLifecycleState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, SignalPiece)]
pub enum NativeLifecycleState {
    Registered,
    Connecting,
    Authenticated,
    AuthenticationFailed,
    AttemptFailed,
    InputRevoked,
    Disconnected,
    Ended,
}

#[derive(Clone, Serialize, Deserialize, DartSignal)]
pub struct DesktopNativeLifecycleQuery {
    pub version: u16,
    pub source: DesktopSource,
    pub identity: String,
    /// 仅关联查询响应，不占用输入命令的 request_id 空间。
    pub request_id: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, SignalPiece)]
pub enum NativeLifecycleQueryFailure {
    None,
    UnsupportedVersion,
    StaleSource,
    Busy,
}

#[derive(Debug, Clone, Serialize, RustSignal)]
pub struct DesktopNativeLifecycleQueryResult {
    pub version: u16,
    pub source: DesktopSource,
    pub identity: String,
    pub request_id: u64,
    pub lifecycle: Option<DesktopNativeLifecycle>,
    pub failure: NativeLifecycleQueryFailure,
}

#[derive(Clone, Serialize, Deserialize, DartSignal)]
pub struct DesktopNativeCommand {
    pub version: u16,
    pub source: DesktopSource,
    pub identity: String,
    pub attempt: u64,
    /// 输入/动作匹配原焦点；布局、视口、FrameAck 与连接控制只作用于原 source/attempt。
    pub focus_epoch: u64,
    /// 来源内单调递增的关联 ID，允许跳号；effect 序只由 Rust 分配。
    pub request_id: u64,
    pub command: NativeCommand,
}

#[cfg(test)]
pub(crate) fn inject_test_request(request: &DesktopNativeCommand) {
    let encoded =
        rinf::serialize(request).unwrap_or_else(|error| panic!("新输入接口编码失败：{error}"));
    DesktopNativeCommand::send_dart_signal(&encoded, &[]);
}

#[derive(Clone, Serialize, Deserialize, SignalPiece)]
pub enum NativeCommand {
    Event(DesktopEvent),
    Action(DesktopAction),
    GuiIntent(GuiIntent),
}

/// 诚实的非物理 UI 意图；修饰状态不证明 OS 来源，也不进入物理 matcher。
#[derive(Debug, Clone, Serialize, Deserialize, SignalPiece)]
pub struct GuiIntent {
    pub input: GuiInput,
    pub modifiers: GuiModifiers,
    /// 只关联原 UI 修饰状态快照；Rust 不推定或改变后续 UI 状态。
    pub modifier_revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, SignalPiece)]
pub enum GuiInput {
    GuiKey(GuiKey),
    GuiText(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, SignalPiece)]
pub enum GuiKey {
    Enter,
    Escape,
    Tab,
    Backspace,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    F(u8),
    CtrlC,
    CtrlD,
    CtrlZ,
    CtrlL,
}

/// 位集：Ctrl=1、Alt=2；其余位拒绝。锁定与按钮已有的位不消费同位 latch。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, SignalPiece)]
pub struct GuiModifiers {
    pub button: u8,
    pub latched: u8,
    pub locked: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, SignalPiece)]
pub enum NativeCommandFailure {
    None,
    UnsupportedVersion,
    StaleSource,
    NotAttached,
    NotAuthenticated,
    Busy,
    TooLarge,
    MixedMode,
    Duplicate,
    InvalidEvent,
    Retired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, SignalPiece)]
pub enum NativeCommandAdmission {
    Accepted,
    Rejected,
    /// Duplicate 不证明原请求的准入结果；消费者继续隔离原待决请求。
    Unknown,
}

#[derive(Serialize, RustSignal)]
pub struct DesktopNativeCommandResult {
    pub version: u16,
    pub source: DesktopSource,
    pub identity: String,
    pub attempt: u64,
    pub focus_epoch: u64,
    pub request_id: u64,
    pub admission: NativeCommandAdmission,
    pub effect: u64,
    pub consumed_latches: u8,
    pub gui_modifier_revision: Option<u64>,
    /// None 只确认准入，不冒充远端写入或 GUI 动作完成。
    pub failure: NativeCommandFailure,
}

/// 只通知原目标领取已排到 cursor 的动作；不是 GUI 执行完成，也不携带剪贴板内容。
#[derive(Serialize, RustSignal)]
pub struct DesktopNativeActionReady {
    pub version: u16,
    pub source: DesktopSource,
    pub identity: String,
    pub attempt: u64,
    pub focus_epoch: u64,
    pub effect: u64,
}

/// 准入时立即通知原宿主终结原 context；不等待 actor 或 GUI 动作领取。
#[derive(Debug, Clone, Serialize, RustSignal)]
pub struct DesktopNativeTextEndRequired {
    pub version: u16,
    pub source: DesktopSource,
    pub identity: String,
    pub attempt: u64,
    pub focus_epoch: u64,
    pub context_id: u64,
    pub transaction_id: u64,
    pub end_fence: u64,
}

/// 已排到原 actor 的本地右键菜单意图；菜单选项仍经原非键命令 writer 准入。
#[derive(Serialize, RustSignal)]
pub struct DesktopNativeContextMenuReady {
    pub version: u16,
    pub source: DesktopSource,
    pub identity: String,
    pub attempt: u64,
    pub focus_epoch: u64,
    pub effect: u64,
    pub stable_row: i64,
    pub column: u16,
}

/// 与 DesktopFrame 同一 RenderFrame 的元数据，不改变旧帧布局。
#[derive(Serialize, RustSignal)]
pub struct DesktopNativeProjection {
    pub source: DesktopSource,
    pub identity: String,
    pub attempt: u64,
    pub focus_epoch: u64,
    pub frame_revision: u32,
    pub applied_effect: u64,
    pub row_revision: u64,
    pub follow_bottom: bool,
    pub first_stable_row: i64,
    pub screen_top_stable_row: i64,
    pub viewport_top: i64,
    pub mouse_reporting: bool,
    pub bracketed_paste: bool,
    pub selection: Option<NativeSelection>,
    pub caret: Option<NativeCaret>,
}

#[derive(Serialize, SignalPiece)]
pub struct NativeSelection {
    pub start_row: i64,
    pub start_col: u16,
    pub end_row: i64,
    pub end_col: u16,
    pub rectangular: bool,
}

/// 相对该帧 viewport_top 的像素格矩形；宿主结合真实布局偏移与可视裁剪，
/// 转换为 OS 文本服务要求的客户区／屏幕坐标。
#[derive(Serialize, SignalPiece)]
pub struct NativeCaret {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub dpi: u32,
}

#[derive(Serialize, RustSignal)]
pub struct DesktopNativeInvalidation {
    pub source: DesktopSource,
    pub identity: String,
    pub effect: u64,
    pub row_revision: u64,
}
