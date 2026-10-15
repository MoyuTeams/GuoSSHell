use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::task::{Context, Poll, Waker};
use std::time::Instant;

use crate::selected_p1::resource::owner::{OwnerLayout, SharedOwner};

use super::driver::{Binding, NetworkHandle, TimerOwner, lock};
use super::{RuntimeFault, Site};

pub(crate) struct TimerState {
    pub active: bool,
    pub expired: bool,
    pub deadline: Instant,
    pub generation: u64,
    waiter: Option<Waker>,
}
pub(crate) struct TimerRecord<R: Send + Sync + 'static> {
    pub state: Mutex<TimerState>,
    _binding: SharedOwner<'static, Binding<R>>,
}

pub struct Sleep<R: Send + Sync + 'static, const IO: usize, const TIMERS: usize> {
    owner: TimerOwner<R>,
    handle: NetworkHandle<R, IO, TIMERS>,
    index: usize,
}

impl<R: Send + Sync + 'static, const IO: usize, const TIMERS: usize> NetworkHandle<R, IO, TIMERS> {
    pub fn sleep_until(&self, deadline: Instant) -> Result<Sleep<R, IO, TIMERS>, RuntimeFault> {
        let handle = self.try_clone()?;
        let mut registry = lock(&self.core.value().registry);
        if registry.closed {
            return Err(RuntimeFault::Cancelled);
        }
        let index =
            registry
                .timers
                .iter()
                .position(Option::is_none)
                .ok_or(RuntimeFault::RegistryFull {
                    site: Site::Timer,
                    capacity: TIMERS,
                })?;
        let generation = registry.next_generation(Site::Timer)?;
        let binding = self
            .core
            .value()
            .binding
            .try_clone()
            .map_err(|reason| RuntimeFault::numeric(Site::Binding, reason))?;
        let owner = self.scope.own(Site::Timer, 1, || {
            Ok(TimerRecord {
                state: Mutex::new(TimerState {
                    active: true,
                    expired: false,
                    deadline,
                    generation,
                    waiter: None,
                }),
                _binding: binding,
            })
        })?;
        registry.timers[index] = Some(
            owner
                .try_clone()
                .map_err(|reason| RuntimeFault::numeric(Site::Timer, reason))?,
        );
        Ok(Sleep {
            owner,
            handle,
            index,
        })
    }
}

impl<R: Send + Sync + 'static, const IO: usize, const TIMERS: usize> Sleep<R, IO, TIMERS> {
    pub fn layout() -> Result<std::alloc::Layout, RuntimeFault> {
        OwnerLayout::actual::<TimerRecord<R>>(0)
            .map(|model| model.backing)
            .map_err(|reason| RuntimeFault::numeric(Site::Timer, reason))
    }
    pub fn reset(&mut self, deadline: Instant) -> Result<(), RuntimeFault> {
        let mut registry = lock(&self.handle.core.value().registry);
        if registry.closed {
            return Err(RuntimeFault::Cancelled);
        }
        let mut state = lock(&self.owner.value().state);
        if !state.active {
            return Err(RuntimeFault::Cancelled);
        }
        let generation = registry.next_generation(Site::Timer)?;
        state.deadline = deadline;
        state.generation = generation;
        state.expired = false;
        Ok(())
    }
    pub fn cancel_handle(&self) -> Result<TimerCancel<R, IO, TIMERS>, RuntimeFault> {
        Ok(TimerCancel {
            owner: self
                .owner
                .try_clone()
                .map_err(|reason| RuntimeFault::numeric(Site::Timer, reason))?,
            handle: self.handle.try_clone()?,
            index: self.index,
        })
    }
    pub fn cancel(&self) {
        cancel_record(&self.owner);
        let generation = lock(&self.owner.value().state).generation;
        self.handle.remove_timer(self.index, generation);
    }
}
impl<R: Send + Sync + 'static, const IO: usize, const TIMERS: usize> Future
    for Sleep<R, IO, TIMERS>
{
    type Output = Result<(), RuntimeFault>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = lock(&self.owner.value().state);
        if !state.active {
            return Poll::Ready(Err(RuntimeFault::Cancelled));
        }
        if state.expired || Instant::now() >= state.deadline {
            state.expired = true;
            drop(state.waiter.take());
            return Poll::Ready(Ok(()));
        }
        if state
            .waiter
            .as_ref()
            .is_none_or(|old| !old.will_wake(cx.waker()))
        {
            state.waiter = Some(cx.waker().clone());
        }
        Poll::Pending
    }
}
impl<R: Send + Sync + 'static, const IO: usize, const TIMERS: usize> Drop for Sleep<R, IO, TIMERS> {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub struct TimerCancel<R: Send + Sync + 'static, const IO: usize, const TIMERS: usize> {
    owner: TimerOwner<R>,
    handle: NetworkHandle<R, IO, TIMERS>,
    index: usize,
}
impl<R: Send + Sync + 'static, const IO: usize, const TIMERS: usize> TimerCancel<R, IO, TIMERS> {
    pub fn cancel(&self) {
        cancel_record(&self.owner);
        let generation = lock(&self.owner.value().state).generation;
        self.handle.remove_timer(self.index, generation);
    }
}
pub(crate) fn cancel_record<R: Send + Sync + 'static>(owner: &TimerOwner<R>) {
    let waiter = {
        let mut state = lock(&owner.value().state);
        state.active = false;
        state.waiter.take()
    };
    if let Some(waiter) = waiter {
        waiter.wake();
    }
}
pub(crate) fn expire<R: Send + Sync + 'static>(
    owner: &TimerOwner<R>,
    generation: u64,
    now: Instant,
) {
    let waiter = {
        let mut state = lock(&owner.value().state);
        if !state.active || state.generation != generation || state.expired || now < state.deadline
        {
            return;
        }
        state.expired = true;
        state.waiter.take()
    };
    if let Some(waiter) = waiter {
        waiter.wake();
    }
}
