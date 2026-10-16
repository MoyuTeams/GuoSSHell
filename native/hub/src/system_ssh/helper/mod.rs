//! System 固定三角色的独立基础；实际平台身份和信任事务由父 owner 提供。

pub mod buffer;
pub mod pipeline;
pub mod protocol;
pub mod transport;

use crate::selected_p1::model::layout::NumericReject;

/// 私有 IPC 的固定数字结果，不携带提示、秘密或动态错误文本。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Reject {
    Malformed = 1,
    Version = 2,
    WrongRole = 3,
    UnsupportedInteraction = 4,
    Limit = 5,
    Source = 6,
    Attempt = 7,
    ProcessLink = 8,
    Sequence = 9,
    NotReady = 10,
    Peer = 11,
    Cancelled = 12,
    Closed = 13,
    Full = 14,
    UnexpectedEof = 15,
    Io = 16,
    LineUnrepresentable = 17,
    InvalidReply = 18,
    HostNotAdmitted = 19,
    AlreadyAuthenticated = 20,
    Resource(NumericReject) = 21,
}

impl From<NumericReject> for Reject {
    fn from(value: NumericReject) -> Self {
        Self::Resource(value)
    }
}
