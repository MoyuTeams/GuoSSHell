//! rinf 边界上的信号类型（适配层）。
//!
//! 业务模型（`RenderFrame` / `ConnectionProfile` ...）属于上游 `rshell-core`，
//! 不在这里重建（PLAN.md 铁律 1/4）。这里只放边界上的平铺投影：
//! * 本文件：启动与会话（请求、状态、帧——帧的 run 压缩字节走二进制通道）
//! * [`catalog`]：连接目录；[`keys`]：私钥；[`interaction`]：连接过程中的交互；
//!   [`settings`]：设置
//!
//! 会话类信号都带 `session_id`（Dart 分配，本进程内唯一）：rinf 的信号按类型
//! 全局广播，多个会话并存时靠它分流。

pub mod catalog;
pub mod interaction;
pub mod keys;
pub mod settings;

use rinf::{DartSignal, RustSignal, RustSignalBinary, SignalPiece};
use serde::{Deserialize, Serialize};

// ── 启动 ─────────────────────────────────────────────────────────────────────

/// App 启动时发一次：平台给 App 的私有数据目录（Flutter 用 path_provider 取）。
/// 连接目录（SQLite）与 known_hosts 都放在这里。
#[derive(Deserialize, DartSignal)]
pub struct AppStart {
    pub support_dir: String,
}

/// App 进入后台 / 回到前台（Flutter 的 AppLifecycleState）。进后台时向系统申请一小段后台
/// 运行时间，刚切出去的连接不会立刻被挂起。
#[derive(Deserialize, DartSignal)]
pub struct AppLifecycle {
    pub foreground: bool,
}

/// 启动结果。`ok = false` 时存储不可用（`detail` 说明原因），目录与连接都不能用。
///
/// `auto_*`：debug 构建的自动连接目标，取自进程环境变量 `GUOSH_HOST` / `GUOSH_PORT` /
/// `GUOSH_USER` / `GUOSH_PASS` / `GUOSH_CMD`（XCUITest 的 launchEnvironment、simctl 的
/// `SIMCTL_CHILD_` 前缀都经它传入；iOS 上 Dart 读不到进程环境变量，由 Rust 转交）。
/// release 构建恒为空。
#[derive(Serialize, RustSignal, Default)]
pub struct AppReady {
    pub ok: bool,
    pub detail: String,
    pub auto_host: String,
    pub auto_port: u16,
    pub auto_user: String,
    pub auto_pass: String,
    pub auto_command: String,
}

// ── Dart → Rust ──────────────────────────────────────────────────────────────

/// 建立一条 SSH 会话：`connection_id` 非空连目录里的连接；为空是**快速连接**
/// （不存目录），目标取下面几个字段。
///
/// 快速连接的密码可以随请求带来（按明文过边界——上游的 `SecretString` 刻意不可
/// 序列化，PLAN.md §2.2；Rust 在接收处立刻包成 `SecretString`），为空则连接时弹框问。
#[derive(Deserialize, DartSignal)]
pub struct ConnectRequest {
    pub session_id: u32,
    pub connection_id: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    /// 非空 = exec 模式：连接后在远端直接执行该命令（如 `top`），不进 shell。
    pub command: String,
    /// 首次几何。度量的唯一权威是 Flutter（PLAN.md §8）：
    /// 它量完格子后随连接请求一起带来。
    pub cols: u16,
    pub rows: u16,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub dpi: u32,
}

/// 视口变化（旋转 / 改窗口）。→ `engine.resize` + `transport.resize`（远端 window-change）。
#[derive(Deserialize, DartSignal)]
pub struct ResizeRequest {
    pub session_id: u32,
    pub cols: u16,
    pub rows: u16,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub dpi: u32,
}

#[derive(Deserialize, DartSignal)]
pub struct DisconnectRequest {
    pub session_id: u32,
}

/// 断开之后再连一次（同一会话：画面与滚回都保留，新的输出接在后面）。
#[derive(Deserialize, DartSignal)]
pub struct ReconnectRequest {
    pub session_id: u32,
}

/// 要显示的窗口（滚回）。`follow_bottom` 时显示屏幕、随输出滚动；否则渲染从
/// `top_stable_row` 起的 `rows` 行——Dart 按滚动位置请求，含上下余量。
#[derive(Deserialize, DartSignal)]
pub struct ViewportRequest {
    pub session_id: u32,
    pub follow_bottom: bool,
    pub top_stable_row: i64,
    pub rows: u16,
}

/// 终端输入（M2 输入闭环的边界）。
///
/// 键、IME 文本与粘贴共用此通道，保留用户的发送顺序。
/// `paste = true` 时 `text` 是剪贴板原文，Rust 负责过滤控制字符、规范化换行与 bracketed paste；
/// 其余输入二选一：`text` 非空 = IME 提交的文本（`CommittedText`）；
/// 否则 `key` 携带键名——`"character:x"`（单字符）或命名键
/// （enter/escape/tab/backspace/delete/insert/home/end/page_up/page_down/
/// arrow_up/arrow_down/arrow_left/arrow_right/`f1`…`f24`）。
/// 键编码（ETX/Kitty/CSI-u…）是 Rust 侧 `encode_input` 的事，Dart 只转发。
#[derive(Deserialize, DartSignal)]
pub struct InputRequest {
    pub session_id: u32,
    pub paste: bool,
    pub text: String,
    pub key: String,
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
}

/// 鼠标事件（M2a 鼠标转发的边界）。
///
/// Rust 组装成 `TerminalMouseEvent` 交给 `engine.encode_mouse`；远端没开
/// 对应的鼠标上报（点击 / 拖动 / 任意移动）时 encode 返回 Err，静默忽略。
/// 滚轮必须用 `kind: "scroll"`（上游 validate 会拒绝「滚轮走 press」）。
#[derive(Deserialize, DartSignal)]
pub struct MouseRequest {
    pub session_id: u32,
    /// press / release / move / scroll
    pub kind: String,
    /// left / middle / right / wheel_up / wheel_down；无键移动（悬停）为空
    pub button: String,
    pub col: u16,
    pub row: u16,
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
}

/// 选区变更（M2a 方案 A：选区的权威在引擎，M2a 之前我们在 Dart 侧重造了一遍）。
///
/// `clear = true` 表示清除，其余字段忽略；否则 anchor/focus 是**引擎绝对行号**
/// （`stable_row`，不是视口行号——Dart 已换算好）。两个端点不分先后，引擎
/// 渲染/取文时自己排序，所以拖耳朵越过对端不用特殊处理。
#[derive(Deserialize, DartSignal)]
pub struct SelectionRequest {
    pub session_id: u32,
    pub clear: bool,
    pub anchor_row: i64,
    pub anchor_col: u16,
    pub focus_row: i64,
    pub focus_col: u16,
    /// 方块选（列选区）。M2a 只做整行，恒 false。
    pub rectangular: bool,
}

/// 复制当前选区（取文在引擎里，见 `TerminalEngine::selected_text`）。
#[derive(Deserialize, DartSignal)]
pub struct CopyRequest {
    pub session_id: u32,
}

/// Dart 已处理完 `seq` 这一帧（流控：同一时刻最多一帧在途，Dart 跟不上时
/// Rust 只保留最新状态，不在 rinf 的无界队列里积压）。
#[derive(Deserialize, DartSignal)]
pub struct FrameAck {
    pub session_id: u32,
    pub seq: u32,
}

// ── Rust → Dart ──────────────────────────────────────────────────────────────

#[derive(Serialize, SignalPiece)]
pub enum SessionState {
    Connecting,
    Connected,
    Failed,
    /// 会话结束（远端退出、断开）。
    Closed,
    /// 连接完成前被用户取消（关闭页面、取消密码框）。
    Cancelled,
}

/// 失败分类（`SessionState::Failed` 时有意义）。文案由 Dart 按分类给出。
#[derive(Serialize, SignalPiece, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    None,
    /// 目录里没有这条连接（已被删除）。
    NotFound,
    /// 快速连接的目标无效（主机、端口或用户名）。
    InvalidTarget,
    /// 连接用的私钥不在钥匙串里了。
    KeyNotFound,
    Authentication,
    HostKeyRejected,
    /// 主机密钥与记录的不一致，且没有被接受替换。
    HostKeyChanged,
    Network,
    Timeout,
    /// 连上之后断了：网络中断，或服务器不再响应（keepalive 没有回音）。
    ConnectionLost,
    /// 钥匙串读写失败。
    Keychain,
    /// OpenPGP 卡：没有找到这张卡（没插上、没靠近）。
    CardNotFound,
    /// OpenPGP 卡：认证槽的密钥与登记时的不同。
    CardKeyMismatch,
    /// OpenPGP 卡：认证槽没有密钥，或算法暂不支持。
    CardUnsupported,
    /// OpenPGP 卡：PIN 已锁定。
    CardPinBlocked,
    /// OpenPGP 卡：要在卡上按键确认，但没有等到。
    CardTouchTimeout,
    /// OpenPGP 卡：其他读卡错误。
    CardError,
    /// 安全密钥没有完成签名（用不了、出错，或给的签名 OpenSSH 验证不了）。
    SecurityKeyFailed,
    Other,
}

/// 连接中的提示。
#[derive(Serialize, SignalPiece, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectHint {
    None,
    /// 请按 OpenPGP 卡上的按键确认。
    TouchCard,
    /// 请把 OpenPGP 卡靠近设备（NFC）。
    TapCard,
    /// 请按系统界面的提示使用安全密钥（插上或靠近，再触摸）。
    SecurityKey,
}

#[derive(Serialize, RustSignal)]
pub struct SessionStatus {
    pub session_id: u32,
    pub state: SessionState,
    pub failure: FailureKind,
    /// 补充信息（目标地址、诊断分类等）。绝不包含密码。
    pub detail: String,
    /// 失败可能源于系统的本地网络权限（目标在局域网，失败是网络或超时类）。
    /// 非空时是打开系统设置对应页面的 URL（平台相关，由 Rust 给出）。
    pub local_network_settings_url: String,
    /// Connecting：要用户在设备之外做的事（按卡上的按键等）。
    pub hint: ConnectHint,
}

/// 一帧终端画面。二进制部分是 [`crate::frame_codec::pack_runs`] 的产物，
/// 走 `RustSignalBinary` 的原始字节通道（PLAN.md §4.3 的落地方案）。
#[derive(Serialize, RustSignalBinary)]
pub struct FrameUpdate {
    pub session_id: u32,
    /// 终端尺寸（屏幕的列数与行数）；帧里的行数可以不同（滚回窗口）。
    pub cols: u16,
    pub rows: u16,
    /// 本会话内单调递增的帧序号。Dart 侧用它数**丢帧**：
    /// 收到的 seq 跳变 = 中间有帧没送达（验收：表现为晚一帧，不是花屏）。
    pub seq: u32,
    /// 帧里第一行的绝对行号（`stable_row`）。
    pub top_stable_row: i64,
    /// 滚回范围：最早一行与屏幕首行的绝对行号（备用屏没有滚回，两者相同）。
    pub first_stable_row: i64,
    pub screen_top_stable_row: i64,
    /// 光标在屏幕上的坐标（列, 行）。`-1` 表示隐藏。
    pub cursor_col: i32,
    pub cursor_row: i32,
    /// 窗口标题（远端用 OSC 0 / 2 设置），标签页上显示。
    pub title: String,
    /// 远端开启了鼠标上报（DECSET 1000/1002/1003）。Dart 据此决定
    /// 触摸点击/滚轮是转发给远端还是保持本地行为（M2a）。
    pub mouse_reporting: bool,
    /// 远端在备用屏（vim/less 等 TUI）。
    pub alternate_screen: bool,
}

/// 选区回显（引擎当前持有选区的原样投影）。
///
/// Dart 用它给耳朵/气泡定位。`anchor`/`focus` **保留 Dart 传入时的角色、不排序**
/// ——拖耳朵越过对端时角色才不会乱（引擎自己渲染/取文时才排序）。
#[derive(Serialize, RustSignal)]
pub struct SelectionState {
    pub session_id: u32,
    pub has_selection: bool,
    pub anchor_row: i64,
    pub anchor_col: u16,
    pub focus_row: i64,
    pub focus_col: u16,
}

/// 引擎取出的选区文本。引擎已按自己的规则跨行拼接、裁掉行尾空格（`text.rs`
/// 的 `selection_text`），Dart 直接进剪贴板。无选区时是空串。
#[derive(Serialize, RustSignal)]
pub struct ClipboardText {
    pub session_id: u32,
    pub text: String,
}

/// 每 5 秒一条的性能汇总（M1 帧率与 M6 性能验收的数字来源）。
/// 单帧预算 16.67 ms（PLAN §4）：render_us + pack_us 的 max 是 Rust 侧的真实开销。
#[derive(Serialize, RustSignal)]
pub struct PerfStats {
    pub session_id: u32,
    /// 统计窗口内实际打包发出的帧数。fps = frames / window_ms * 1000。
    pub frames: u32,
    pub window_ms: u32,
    pub render_us_avg: u32,
    pub render_us_max: u32,
    pub pack_us_avg: u32,
    pub pack_us_max: u32,
    /// 每帧压缩字节数的平均值（典型 5.8–7.2 KB，TUI 满屏最坏 61 KB）。
    pub bytes_avg: u32,
    /// 显示延迟：远端输出进了引擎、到含它的那一帧被 Dart 确认（画完）为止，微秒。
    /// Dart 跟不上时帧流控让它变大，而不是积压。
    pub latency_us_p50: u32,
    pub latency_us_p95: u32,
    pub latency_us_max: u32,
    /// 因 Dart 250 ms 内没确认上一帧而照发的帧数（Dart 卡住的迹象）。
    pub ack_timeouts: u32,
    /// 收到的远端输出字节数。
    pub input_bytes: u64,
    /// 到截止时刻才结束的同步输出（DEC 2026）次数（程序在一帧中途停下）。
    pub sync_timeouts: u32,
}

/// 画面一致性自检（M6，测试用）：请求最近发出的那一帧的逐列内容。
#[derive(Deserialize, DartSignal)]
pub struct ScreenCheckRequest {
    pub session_id: u32,
}

/// 最近发出的一帧（序号 `seq`），每行每列一格、宽字符的第二列为空串，`stable_rows` 是各行的
/// 绝对行号。Dart 画完同一序号的帧后逐列比对：帧编码或行池出错（丢字、错位）都会显出来，
/// 与画面还在不在刷新无关。
#[derive(Serialize, RustSignal)]
pub struct ScreenCheck {
    pub session_id: u32,
    pub seq: u32,
    pub cols: u16,
    pub stable_rows: Vec<i64>,
    pub rows: Vec<Vec<String>>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::InputRequest;
    use rinf::DartSignal;

    #[tokio::test]
    async fn keys_text_and_paste_use_one_fifo_signal_queue() {
        let receiver = InputRequest::get_dart_signal_receiver();
        let events = [
            (false, "echo ", ""),
            (true, "第一段", ""),
            (false, "", "enter"),
            (true, "第二段", ""),
            (false, "尾部", ""),
        ];
        for (paste, text, key) in events {
            let bytes = rinf::serialize(&(1u32, paste, text, key, false, false, false))
                .expect("输入信号编码");
            InputRequest::send_dart_signal(&bytes, &[]);
        }
        for (paste, text, key) in events {
            let message = receiver.recv().await.expect("输入信号").message;
            assert_eq!(
                (message.paste, message.text.as_str(), message.key.as_str()),
                (paste, text, key)
            );
        }
    }
}
