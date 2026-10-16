//! 默认关闭的固定原型，不是生产会话、LoadedRaw 或资源证书。
//!
//! 只拥有真实引擎的 CPU 状态，不拥有 Screen、帧、Rinf 发布或 GUI 效果。
//! 原型 stream 的独立读写不改变生产 SessionTransport 的串行读写限制。

mod budget;
mod commands;
mod core;
mod deadline;
mod effects;
mod ffi;
mod payload;
mod projection;
mod transport;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
