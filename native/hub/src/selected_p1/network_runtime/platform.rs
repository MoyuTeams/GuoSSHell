#[cfg(windows)]
#[path = "windows.rs"]
mod implementation;
#[cfg(not(windows))]
#[path = "unsupported.rs"]
mod implementation;

pub(crate) use implementation::{Socket, Startup, poll};

pub(crate) const READ: u8 = 1;
pub(crate) const WRITE: u8 = 2;
pub(crate) const ERROR: u8 = 4;
pub(crate) const BATCH: usize = 32;
pub(crate) const IO_CHUNK: usize = 16 * 1024;

#[derive(Clone, Copy, Default)]
pub(crate) struct PollEntry {
    pub raw: usize,
    pub interest: u8,
    pub ready: u8,
}
