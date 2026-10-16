use std::cell::Cell;
use std::future::Future;
use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::pin::Pin;
use std::sync::Mutex;
use std::task::{Context, Poll, Waker};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::selected_p1::model::layout::NumericReject;
use crate::selected_p1::resource::owner::{OwnerLayout, SharedOwner};

use super::driver::{Binding, IoOwner, NetworkHandle, lock};
use super::platform::{self, ERROR, IO_CHUNK, READ, WRITE};
use super::{Cooperative, RuntimeFault, Site};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SocketPeer {
    pub local: SocketAddr,
    pub peer: SocketAddr,
}

pub(crate) struct IoState {
    pub socket: platform::Socket,
    pub active: bool,
    pub generation: u64,
    pub reader: Option<Waker>,
    pub writer: Option<Waker>,
    connected: bool,
    write_ready: bool,
    peer: Option<SocketPeer>,
    fault: Option<RuntimeFault>,
}

// 字段顺序保证 socket/wakers 先于 startup/foreign root 析构。
pub(crate) struct SocketRecord<R: Send + Sync + 'static> {
    pub state: Mutex<IoState>,
    pub _binding: SharedOwner<'static, Binding<R>>,
}

fn register(slot: &mut Option<Waker>, cx: &Context<'_>) {
    if slot.as_ref().is_none_or(|old| !old.will_wake(cx.waker())) {
        *slot = Some(cx.waker().clone());
    }
}

pub struct NativeStream<R: Send + Sync + 'static, const IO: usize, const TIMERS: usize> {
    owner: IoOwner<R>,
    handle: NetworkHandle<R, IO, TIMERS>,
    index: usize,
    generation: u64,
    cooperative: Cooperative,
}

impl<R: Send + Sync + 'static, const IO: usize, const TIMERS: usize> NetworkHandle<R, IO, TIMERS> {
    /// 数值 endpoint 不做 DNS；所有 owned host/解析/签名图由上层 rooted 服务另行授权。
    pub fn connect(
        &self,
        address: SocketAddr,
        cooperative: Cooperative,
    ) -> Result<Connect<R, IO, TIMERS>, RuntimeFault> {
        let handle = self.try_clone()?;
        let mut registry = lock(&self.core.value().registry);
        if registry.closed {
            return Err(RuntimeFault::Cancelled);
        }
        let index =
            registry
                .io
                .iter()
                .position(Option::is_none)
                .ok_or(RuntimeFault::RegistryFull {
                    site: Site::IoRegistration,
                    capacity: IO,
                })?;
        let generation = registry.next_generation(Site::IoRegistration)?;
        let binding = self
            .core
            .value()
            .binding
            .try_clone()
            .map_err(|reason| RuntimeFault::numeric(Site::Binding, reason))?;
        let fault = Cell::new(None);
        let owner = self
            .scope
            .own(Site::IoRegistration, 1, || {
                let (socket, connected) = platform::Socket::connect(address).map_err(|error| {
                    fault.set(Some(error));
                    NumericReject::ConstructionFailure
                })?;
                let peer = if connected {
                    Some(socket.finish_connect().map_err(|error| {
                        fault.set(Some(error));
                        NumericReject::ConstructionFailure
                    })?)
                } else {
                    None
                };
                Ok(SocketRecord {
                    state: Mutex::new(IoState {
                        socket,
                        active: true,
                        generation,
                        reader: None,
                        writer: None,
                        connected,
                        write_ready: false,
                        peer,
                        fault: None,
                    }),
                    _binding: binding,
                })
            })
            .map_err(|error| fault.get().unwrap_or(error))?;
        registry.io[index] = Some(
            owner
                .try_clone()
                .map_err(|reason| RuntimeFault::numeric(Site::IoRegistration, reason))?,
        );
        Ok(Connect {
            stream: Some(NativeStream {
                owner,
                handle,
                index,
                generation,
                cooperative,
            }),
        })
    }
}

impl<R: Send + Sync + 'static, const IO: usize, const TIMERS: usize> NativeStream<R, IO, TIMERS> {
    pub fn layout() -> Result<std::alloc::Layout, RuntimeFault> {
        OwnerLayout::actual::<SocketRecord<R>>(0)
            .map(|model| model.backing)
            .map_err(|reason| RuntimeFault::numeric(Site::IoRegistration, reason))
    }

    pub fn peer(&self) -> Result<SocketPeer, RuntimeFault> {
        lock(&self.owner.value().state)
            .peer
            .ok_or(RuntimeFault::Cancelled)
    }

    pub fn fault(&self) -> Option<RuntimeFault> {
        lock(&self.owner.value().state).fault
    }

    /// 驱动本 stream 的 task 必须用同一个预算 bind；不依赖 Tokio cooperative。
    pub fn cooperative(&self) -> Result<Cooperative, RuntimeFault> {
        self.cooperative.try_clone()
    }

    #[cfg(test)]
    pub fn registered_waiters(&self) -> (bool, bool) {
        let state = lock(&self.owner.value().state);
        (state.reader.is_some(), state.writer.is_some())
    }

    pub fn cancel_handle(&self) -> Result<StreamCancel<R, IO, TIMERS>, RuntimeFault> {
        Ok(StreamCancel {
            owner: self
                .owner
                .try_clone()
                .map_err(|reason| RuntimeFault::numeric(Site::IoRegistration, reason))?,
            handle: self.handle.try_clone()?,
            index: self.index,
            generation: self.generation,
        })
    }

    pub fn set_send_buffer(&self, bytes: i32) -> Result<(), RuntimeFault> {
        lock(&self.owner.value().state).socket.send_buffer(bytes)
    }

    pub fn poll_read_scoped(
        &mut self,
        cx: &mut Context<'_>,
        bytes: &mut [u8],
    ) -> Poll<Result<usize, RuntimeFault>> {
        if bytes.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let mut state = lock(&self.owner.value().state);
        if !state.active {
            return Poll::Ready(Err(RuntimeFault::Cancelled));
        }
        if let Some(fault) = state.fault {
            return Poll::Ready(Err(fault));
        }
        if self.cooperative.poll_work(cx, NonZeroU32::MIN).is_pending() {
            return Poll::Pending;
        }
        let limit = bytes.len().min(IO_CHUNK);
        match state.socket.read(&mut bytes[..limit]) {
            Ok(Some(count)) => Poll::Ready(Ok(count)),
            Ok(None) => {
                register(&mut state.reader, cx);
                Poll::Pending
            }
            Err(fault) => {
                state.fault = Some(fault);
                Poll::Ready(Err(fault))
            }
        }
    }

    pub fn poll_write_scoped(
        &mut self,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<Result<usize, RuntimeFault>> {
        if bytes.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let mut state = lock(&self.owner.value().state);
        if !state.active {
            return Poll::Ready(Err(RuntimeFault::Cancelled));
        }
        if let Some(fault) = state.fault {
            return Poll::Ready(Err(fault));
        }
        if self.cooperative.poll_work(cx, NonZeroU32::MIN).is_pending() {
            return Poll::Pending;
        }
        match state.socket.write(&bytes[..bytes.len().min(IO_CHUNK)]) {
            Ok(Some(count)) => Poll::Ready(Ok(count)),
            Ok(None) => {
                register(&mut state.writer, cx);
                Poll::Pending
            }
            Err(fault) => {
                state.fault = Some(fault);
                Poll::Ready(Err(fault))
            }
        }
    }

    pub fn cancel(&self) {
        cancel_record(&self.owner);
        self.handle.remove_io(self.index, self.generation);
    }
}

impl<R: Send + Sync + 'static, const IO: usize, const TIMERS: usize> Drop
    for NativeStream<R, IO, TIMERS>
{
    fn drop(&mut self) {
        self.cancel();
    }
}

pub struct Connect<R: Send + Sync + 'static, const IO: usize, const TIMERS: usize> {
    stream: Option<NativeStream<R, IO, TIMERS>>,
}
impl<R: Send + Sync + 'static, const IO: usize, const TIMERS: usize> Future
    for Connect<R, IO, TIMERS>
{
    type Output = Result<NativeStream<R, IO, TIMERS>, RuntimeFault>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let Some(stream) = this.stream.as_ref() else {
            return Poll::Ready(Err(RuntimeFault::Cancelled));
        };
        let result = {
            let mut state = lock(&stream.owner.value().state);
            if !state.active {
                Some(Err(RuntimeFault::Cancelled))
            } else if state.connected {
                Some(Ok(()))
            } else if state.write_ready {
                match state.socket.finish_connect() {
                    Ok(peer) => {
                        state.peer = Some(peer);
                        state.connected = true;
                        Some(Ok(()))
                    }
                    Err(fault) => {
                        state.fault = Some(fault);
                        Some(Err(fault))
                    }
                }
            } else {
                register(&mut state.writer, cx);
                None
            }
        };
        match result {
            None => Poll::Pending,
            Some(Ok(())) => match this.stream.take() {
                Some(stream) => Poll::Ready(Ok(stream)),
                None => Poll::Ready(Err(RuntimeFault::Cancelled)),
            },
            Some(Err(fault)) => {
                drop(this.stream.take());
                Poll::Ready(Err(fault))
            }
        }
    }
}

pub struct StreamCancel<R: Send + Sync + 'static, const IO: usize, const TIMERS: usize> {
    owner: IoOwner<R>,
    handle: NetworkHandle<R, IO, TIMERS>,
    index: usize,
    generation: u64,
}
impl<R: Send + Sync + 'static, const IO: usize, const TIMERS: usize> StreamCancel<R, IO, TIMERS> {
    pub fn cancel(&self) {
        cancel_record(&self.owner);
        self.handle.remove_io(self.index, self.generation);
    }
}

pub(crate) fn cancel_record<R: Send + Sync + 'static>(owner: &IoOwner<R>) {
    let (reader, writer) = {
        let mut state = lock(&owner.value().state);
        if !state.active {
            return;
        }
        state.active = false;
        state.socket.shutdown();
        (state.reader.take(), state.writer.take())
    };
    if let Some(reader) = reader {
        reader.wake();
    }
    if let Some(writer) = writer {
        writer.wake();
    }
}

/// OS readiness 对应旧 root/代次；注销或取消后只能丢弃，不触及新注册。
pub struct IoEvent<R: Send + Sync + 'static> {
    pub(crate) owner: IoOwner<R>,
    pub(crate) generation: u64,
    pub(crate) ready: u8,
}
impl<R: Send + Sync + 'static> IoEvent<R> {
    pub fn deliver(self) -> bool {
        let (reader, writer) = {
            let mut state = lock(&self.owner.value().state);
            if !state.active || state.generation != self.generation {
                return false;
            }
            let reader = if self.ready & (READ | ERROR) != 0 {
                state.reader.take()
            } else {
                None
            };
            let writer = if self.ready & (WRITE | ERROR) != 0 {
                state.write_ready = true;
                state.writer.take()
            } else {
                None
            };
            (reader, writer)
        };
        if let Some(reader) = reader {
            reader.wake();
        }
        if let Some(writer) = writer {
            writer.wake();
        }
        true
    }
}

impl<R: Send + Sync + 'static, const IO: usize, const TIMERS: usize> AsyncRead
    for NativeStream<R, IO, TIMERS>
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let limit = buffer.remaining().min(IO_CHUNK);
        match self
            .get_mut()
            .poll_read_scoped(cx, buffer.initialize_unfilled_to(limit))
        {
            Poll::Ready(Ok(count)) => {
                buffer.advance(count);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(fault)) => Poll::Ready(Err(fault.io())),
            Poll::Pending => Poll::Pending,
        }
    }
}
impl<R: Send + Sync + 'static, const IO: usize, const TIMERS: usize> AsyncWrite
    for NativeStream<R, IO, TIMERS>
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.get_mut()
            .poll_write_scoped(cx, bytes)
            .map(|result| result.map_err(RuntimeFault::io))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let state = lock(&self.owner.value().state);
        if !state.active {
            Poll::Ready(Err(RuntimeFault::Cancelled.io()))
        } else if let Some(fault) = state.fault {
            Poll::Ready(Err(fault.io()))
        } else {
            Poll::Ready(Ok(()))
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.cancel();
        Poll::Ready(Ok(()))
    }
}
