//! 应用自有网络驱动；跨库资源根由调用者传入，不定义 russh Root 的替身。

pub mod admission;
pub mod blocking;
pub mod cooperative;
pub mod driver;
pub mod executor;
mod platform;
pub mod stream;
pub mod timer;

pub use admission::{AppScope, Prerequisite, RuntimeFault, Site};
pub use cooperative::{Budgeted, Cooperative};
pub use driver::{NetworkHandle, NetworkRuntime, ReadyBatch};
pub use stream::{Connect, IoEvent, NativeStream, SocketPeer, StreamCancel};
pub use timer::{Sleep, TimerCancel};
