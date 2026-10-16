//! 应用私有单 external buffer；父持有 bounded control、wire、actor 与模块接线。
//! 本模块不注册 SDK callback，不读取 Rinf 的私有 isolate，也不开放正常 P1。

pub mod frame;
pub mod owner;
pub mod post;
pub mod registration;

pub use frame::{ByteSink, FiniteMetadata, FrameSpans, MetadataHeaderBuilder, Span};
pub use owner::{NoUnwindRoot, OwnedReplyBody, ReplyLayout, UnpostedReply};
pub use post::{LibraryPinProof, PostOutcome};
pub use registration::{PortOwner, ReplyPortReject, ReplyPortState};
