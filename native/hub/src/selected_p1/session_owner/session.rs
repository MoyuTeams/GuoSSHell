//! P1 的 loading/running completion 不产生 Tokio task cell；父在同一 poller 消费。
//! 固定通道仅承载父指定的 session 控制，不改无关键盘 public API。

use std::array;
use std::task::{Context, Poll, Waker};

/// 内嵌在已经计入 actual F 的控制 storage；无 Arc/Box/channel 节点或队列增长。
pub struct InlineControl<T, const N: usize> {
    slots: [Option<T>; N],
    read: usize,
    write: usize,
    count: usize,
    closed: bool,
    waiter: Option<Waker>,
}
impl<T, const N: usize> Default for InlineControl<T, N> {
    fn default() -> Self {
        Self {
            slots: array::from_fn(|_| None),
            read: 0,
            write: 0,
            count: 0,
            closed: false,
            waiter: None,
        }
    }
}
impl<T, const N: usize> InlineControl<T, N> {
    pub fn try_send(&mut self, value: T) -> Result<(), T> {
        if self.closed || self.count == N {
            return Err(value);
        }
        self.slots[self.write] = Some(value);
        self.write = (self.write + 1) % N;
        self.count += 1;
        if let Some(waiter) = self.waiter.take() {
            waiter.wake();
        }
        Ok(())
    }
    pub fn poll_recv(&mut self, cx: &mut Context<'_>) -> Poll<Option<T>> {
        if self.count != 0 {
            let value = self.slots[self.read].take();
            self.read = (self.read + 1) % N;
            self.count -= 1;
            return Poll::Ready(value);
        }
        if self.closed {
            return Poll::Ready(None);
        }
        if self
            .waiter
            .as_ref()
            .is_none_or(|w| !w.will_wake(cx.waker()))
        {
            self.waiter = Some(cx.waker().clone());
        }
        Poll::Pending
    }
    pub fn close(&mut self) {
        self.closed = true;
        if let Some(waiter) = self.waiter.take() {
            waiter.wake();
        }
    }
}

/// close/cancel 不需要通道 slot；与跨线程 CancelHandle 配合由唯一 poll owner 销毁 F。
pub struct InlineSessionControls<C, R, const COMMANDS: usize, const REPLIES: usize> {
    pub commands: InlineControl<C, COMMANDS>,
    pub replies: InlineControl<R, REPLIES>,
    pub closing: bool,
    pub completion: Option<u32>,
}
impl<C, R, const COMMANDS: usize, const REPLIES: usize> Default
    for InlineSessionControls<C, R, COMMANDS, REPLIES>
{
    fn default() -> Self {
        Self {
            commands: InlineControl::default(),
            replies: InlineControl::default(),
            closing: false,
            completion: None,
        }
    }
}
