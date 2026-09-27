//! 外部签名器（OpenPGP 卡、安全密钥）与会话的连接循环之间的往来：签名器要会话代为问
//! 用户的事（PIN），以及它在等用户（按卡、靠卡、碰安全密钥）的起止——等用户的时间不算进
//! 连接时限。

use rinf::RustSignal;
use secrecy::SecretString;
use tokio::sync::oneshot;

use crate::signals::{ConnectHint, FailureKind, SessionState, SessionStatus};

pub enum SignerRequest {
    /// 问 PIN。回答是 PIN 与「记住到退出 App」；取消为 `None`。
    Pin {
        question: PinQuestion,
        reply: oneshot::Sender<Option<(SecretString, bool)>>,
    },
    Waiting(bool),
}

pub struct PinQuestion {
    /// 这把密钥在本 App 里的名字。
    pub key_name: String,
    /// PIN 还能试几次；`None` = 还不知道（NFC：卡还没靠近）。
    pub tries_left: Option<u8>,
    /// 上一次的 PIN 不对。
    pub retry: bool,
}

/// 连接中状态附带的提示（按卡上的按键、把卡靠近设备、使用安全密钥）。
pub fn send_hint(session_id: u32, target: &str, hint: ConnectHint) {
    SessionStatus {
        session_id,
        state: SessionState::Connecting,
        failure: FailureKind::None,
        detail: target.to_owned(),
        local_network_settings_url: String::new(),
        hint,
    }
    .send_signal_to_dart();
}
