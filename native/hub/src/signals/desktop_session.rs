//! 桌面准备及有序会话协议。独立于阶段二 V1，不改变任何旧信号布局。

use super::settings::{DesktopTerminalProfile, ProfileFontSnapshot};
use super::{DesktopTargetKind, FrameUpdate, SessionStatus};
use rinf::{DartSignal, RustSignal, RustSignalBinary, SignalPiece};
use serde::{Deserialize, Serialize};

pub const VERSION: u16 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, SignalPiece)]
pub struct DesktopSource {
    pub session_id: u32,
    /// 客户端代次；不是 supervisor 的内部任务编号。
    pub generation: u64,
}

/// 不含密码，也不触发认证或网络。request_id 在同一来源内不得复用。
#[derive(Clone, Serialize, Deserialize, DartSignal)]
pub struct DesktopPrepare {
    pub version: u16,
    pub request_id: u32,
    pub source: DesktopSource,
    pub target: DesktopTargetKind,
    pub profile_id: String,
    pub connection_id: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub command: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, SignalPiece)]
pub struct DesktopGeometry {
    pub cols: u16,
    pub rows: u16,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub dpi: u32,
}

#[derive(Deserialize, DartSignal)]
pub struct DesktopStart {
    pub version: u16,
    pub request_id: u32,
    pub prepare_request_id: u32,
    pub source: DesktopSource,
    pub identity: String,
    pub geometry: DesktopGeometry,
    /// 只能回传 Prepared 给出的主投影或允许回退投影，不接收任意 family/path。
    pub font_projection: String,
    pub password: String,
}

#[derive(Serialize, Deserialize, DartSignal)]
pub struct DesktopCancel {
    pub version: u16,
    pub request_id: u32,
    pub prepare_request_id: u32,
    pub source: DesktopSource,
    /// Preparing 尚未收到 identity 时为空，仍须匹配原 Prepare 关联键。
    pub identity: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, SignalPiece)]
pub enum DesktopFailure {
    None,
    UnsupportedVersion,
    PlatformUnavailable,
    TransportUnavailable,
    InvalidTarget,
    ProfileNotFound,
    Storage,
    CountBudget,
    ByteBudget,
    InflightBudget,
    Expired,
    Cancelled,
    Replaced,
    Duplicate,
    StaleGeneration,
    IdentityMismatch,
    NotPrepared,
    AlreadyStarted,
    InvalidGeometry,
    InvalidFont,
    MixedMode,
    OutOfOrder,
    Busy,
    InputTooLarge,
    InvalidEvent,
    PolicyUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, SignalPiece)]
pub enum DesktopPreparationState {
    Preparing,
    Prepared,
    Accepted,
    Cancelled,
    Failed,
}

#[derive(Serialize, SignalPiece)]
pub struct DesktopPreparedSnapshot {
    /// 已合并 overrides/应用绑定的 16 项有效设置；id 为所选配置，name 留空。
    /// 这是运行投影，不可作为设置编辑草稿回写。实际字体以 font 投影为准。
    pub profile: DesktopTerminalProfile,
    pub font: ProfileFontSnapshot,
    pub font_projection: String,
    pub fallback_projection: String,
    pub fallback_family: String,
    pub resource_bytes: u32,
    /// 有效值与配置值的差异说明；不回写存储。
    pub adjustments: Vec<String>,
}

/// 字体字节来自本次捕获的实际持有，不重新查询槽或缓存。
#[derive(Serialize, RustSignalBinary)]
pub struct DesktopPreparationResult {
    pub request_id: u32,
    pub prepare_request_id: u32,
    pub source: DesktopSource,
    pub identity: String,
    pub state: DesktopPreparationState,
    pub failure: DesktopFailure,
    pub remaining_ms: u32,
    pub snapshot: Option<DesktopPreparedSnapshot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, SignalPiece)]
pub enum DesktopKeyPhase {
    Down,
    Repeat,
    Up,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, SignalPiece)]
pub enum DesktopAltSource {
    Known,
    Unknown,
}

/// 默认候选不抢先执行；4B-I 匹配有效绑定后才可返回给壳。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, SignalPiece)]
pub enum DesktopAction {
    NewTab,
    /// 上游 split_vertical 对应竖向分割线，即现有左右分屏。
    SplitRight,
    SplitDown,
    ClosePane,
    NextTab,
    PreviousTab,
    NextPane,
    Search,
    Settings,
    ToggleSidebar,
    Copy,
    Paste,
    SelectAll,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    /// 仅作默认候选（Ctrl+Shift+L）；不属于 profile 可绑定的动作集合。
    NewLocalTerminal,
}

#[derive(Debug, Clone, Serialize, Deserialize, SignalPiece)]
pub enum DesktopEvent {
    Key {
        key: String,
        phase: DesktopKeyPhase,
        physical: u64,
        logical: u64,
        shift: bool,
        control: bool,
        alt: bool,
        left_alt: bool,
        right_alt: bool,
        alt_source: DesktopAltSource,
        super_key: bool,
        default_action: Option<DesktopAction>,
    },
    Text {
        text: String,
    },
    Paste {
        text: String,
    },
    Resize {
        geometry: DesktopGeometry,
    },
    Viewport {
        follow_bottom: bool,
        top_stable_row: i64,
        rows: u16,
    },
    FrameAck {
        frame_seq: u32,
    },
    Mouse {
        kind: String,
        button: String,
        col: u16,
        row: u16,
        shift: bool,
        control: bool,
        alt: bool,
    },
    Selection {
        clear: bool,
        anchor_row: i64,
        anchor_col: u16,
        focus_row: i64,
        focus_col: u16,
        rectangular: bool,
    },
    Copy,
    Reconnect,
    Disconnect,
}

/// 所有有序输入及控制在此单一接收口，seq 从 1 开始严格连续。
#[derive(Debug, Clone, Serialize, Deserialize, DartSignal)]
pub struct DesktopOrdered {
    pub version: u16,
    pub source: DesktopSource,
    pub identity: String,
    pub seq: u64,
    pub event: DesktopEvent,
}

#[derive(Serialize, RustSignal)]
pub struct DesktopEventResult {
    pub source: DesktopSource,
    pub identity: String,
    pub seq: u64,
    /// None 只表示入队，不是远端收到了字节。Busy/InputTooLarge 不推进 seq。
    /// 通过来源/顺序/预算校验后，InvalidEvent/PolicyUnavailable 消费该 seq 但不外发。
    pub failure: DesktopFailure,
    pub action: Option<DesktopAction>,
}

/// 新帧自带来源；新会话拒绝旧 FrameAck，旧 frame/ack 的 wire 不变。
#[derive(Serialize, RustSignalBinary)]
pub struct DesktopFrame {
    pub source: DesktopSource,
    pub identity: String,
    pub frame: FrameUpdate,
}

#[derive(Serialize, RustSignal)]
pub struct DesktopSessionStatus {
    pub source: DesktopSource,
    pub identity: String,
    pub status: SessionStatus,
}

#[derive(Serialize, RustSignal)]
pub struct DesktopPrompt {
    pub source: DesktopSource,
    pub identity: String,
    pub prompt: super::interaction::InteractionPrompt,
}

#[derive(Deserialize, DartSignal)]
pub struct DesktopReply {
    pub version: u16,
    pub source: DesktopSource,
    pub identity: String,
    pub prompt_id: u32,
    pub accept: bool,
    pub answers: Vec<String>,
    pub remember: bool,
}

#[derive(Serialize, RustSignal)]
pub struct DesktopSelection {
    pub source: DesktopSource,
    pub identity: String,
    pub selection: super::SelectionState,
}

#[derive(Serialize, RustSignal)]
pub struct DesktopClipboard {
    pub source: DesktopSource,
    pub identity: String,
    pub text: String,
}

/// 视口与缓存失效投影；frame_seq 仅属于同一来源，不能用于另一代会话。
#[derive(Serialize, RustSignal)]
pub struct DesktopViewportState {
    pub source: DesktopSource,
    pub identity: String,
    pub frame_seq: u32,
    pub oldest_row: i64,
    pub screen_top: i64,
    pub follow_bottom: bool,
    pub mouse_reporting: bool,
    pub invalidate_selection: bool,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::{
        DesktopEvent, DesktopGeometry, DesktopOrdered, DesktopPrepare, DesktopSource, DesktopStart,
        DesktopTargetKind, VERSION,
    };
    use rinf::DartSignal;

    #[tokio::test]
    async fn preparation_has_no_password_and_new_start_and_ordered_signals_use_real_decoders() {
        let source = DesktopSource {
            session_id: 17,
            generation: 9,
        };
        let prepare = DesktopPrepare {
            version: VERSION,
            request_id: 11,
            source: source.clone(),
            target: DesktopTargetKind::NativeSsh,
            profile_id: String::new(),
            connection_id: String::new(),
            host: "example.test".into(),
            port: 22,
            username: "fixture".into(),
            command: String::new(),
        };
        let json = serde_json::to_value(&prepare).expect("准备投影");
        assert!(json.get("password").is_none());
        let receiver = DesktopPrepare::get_dart_signal_receiver();
        DesktopPrepare::send_dart_signal(&rinf::serialize(&prepare).expect("准备编码"), &[]);
        let request = receiver.recv().await.expect("准备解码").message;
        assert_eq!(request.source, source);
        assert_eq!(request.request_id, 11);
        let starts = DesktopStart::get_dart_signal_receiver();
        let geometry = DesktopGeometry {
            cols: 80,
            rows: 24,
            pixel_width: 640,
            pixel_height: 480,
            dpi: 96,
        };
        DesktopStart::send_dart_signal(
            &rinf::serialize(&(
                VERSION,
                12u32,
                11u32,
                source.clone(),
                "identity",
                geometry,
                "identity:font",
                "最终启动测试秘密",
            ))
            .expect("启动布局"),
            &[],
        );
        let mut start = starts.recv().await.expect("启动解码").message;
        assert_eq!(start.source, source);
        assert_eq!(start.geometry, geometry);
        let password = secrecy::SecretString::from(std::mem::take(&mut start.password));
        assert!(start.password.is_empty());
        drop(password);
        let receiver = DesktopOrdered::get_dart_signal_receiver();
        for (index, event) in [
            DesktopEvent::Text {
                text: "输入".into(),
            },
            DesktopEvent::Paste {
                text: "粘贴".into(),
            },
            DesktopEvent::FrameAck { frame_seq: 3 },
        ]
        .into_iter()
        .enumerate()
        {
            let request = DesktopOrdered {
                version: VERSION,
                source: source.clone(),
                identity: "identity".into(),
                seq: index as u64 + 1,
                event,
            };
            DesktopOrdered::send_dart_signal(
                &rinf::serialize(&request).expect("单一输入信封"),
                &[],
            );
        }
        for seq in 1..=3 {
            let request = receiver.recv().await.expect("保序解码").message;
            assert_eq!(request.seq, seq);
            assert_eq!(request.source, source);
        }
    }
}
