use std::net::SocketAddr;
use std::time::Duration;

use super::super::{Prerequisite, RuntimeFault};
use super::PollEntry;

fn unavailable() -> RuntimeFault {
    RuntimeFault::Unsupported(Prerequisite::UnixPollerSourceClosure)
}

// Unix 的 std 单调时钟可用；当前 App 无已证明的原生 poll ABI/准入闭包。
// 在创建 socket、线程或注册之前拒绝，不能回退到 Tokio 或阻塞 connect。
pub(crate) enum Startup {}
impl Startup {
    pub fn new() -> Result<Self, RuntimeFault> {
        Err(unavailable())
    }
}
pub(crate) enum Socket {}
impl Socket {
    pub fn connect(_: SocketAddr) -> Result<(Self, bool), RuntimeFault> {
        Err(unavailable())
    }
    pub fn finish_connect(&self) -> Result<super::super::stream::SocketPeer, RuntimeFault> {
        match *self {}
    }
    pub fn read(&self, _: &mut [u8]) -> Result<Option<usize>, RuntimeFault> {
        match *self {}
    }
    pub fn write(&self, _: &[u8]) -> Result<Option<usize>, RuntimeFault> {
        match *self {}
    }
    pub fn shutdown(&self) {
        match *self {}
    }
    pub fn send_buffer(&self, _: i32) -> Result<(), RuntimeFault> {
        match *self {}
    }
}
pub(crate) fn poll(_: &mut [PollEntry], _: Duration) -> Result<(), RuntimeFault> {
    Err(unavailable())
}
