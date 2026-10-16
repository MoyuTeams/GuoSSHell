//! 普通 Cargo test target 的单次源码装配；不注册 hub、app 或生产 helper 入口。

#![deny(unsafe_op_in_unsafe_fn)]

#[path = "../../../tests/selected_p1_foundation.rs"]
pub mod foundation;

#[path = "../../signals/interaction.rs"]
pub mod interaction;
pub mod signals {
    pub use crate::interaction;
}
#[path = "../../protected_reply.rs"]
pub(crate) mod protected_reply;

#[path = "mod.rs"]
pub mod helper;
