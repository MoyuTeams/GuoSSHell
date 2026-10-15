use std::cell::Cell;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::selected_p1::model::layout::NumericReject;
use crate::selected_p1::resource::owner::{OwnerLayout, SharedOwner};

use super::platform::{self, BATCH, PollEntry};
use super::stream::{IoEvent, SocketRecord};
use super::timer::TimerRecord;
use super::{AppScope, RuntimeFault, Site};

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(crate) struct Binding<R> {
    pub _foreign_root: R,
    pub _startup: platform::Startup,
}

pub(crate) type IoOwner<R> = SharedOwner<'static, SocketRecord<R>>;
pub(crate) type TimerOwner<R> = SharedOwner<'static, TimerRecord<R>>;

pub(crate) struct Registry<R: Send + Sync + 'static, const IO: usize, const TIMERS: usize> {
    pub closed: bool,
    pub generation: u64,
    pub io: [Option<IoOwner<R>>; IO],
    pub timers: [Option<TimerOwner<R>>; TIMERS],
}

impl<R: Send + Sync + 'static, const IO: usize, const TIMERS: usize> Registry<R, IO, TIMERS> {
    pub fn next_generation(&mut self, site: Site) -> Result<u64, RuntimeFault> {
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or(RuntimeFault::numeric(site, NumericReject::Overflow))?;
        Ok(self.generation)
    }
}

pub(crate) struct Core<R: Send + Sync + 'static, const IO: usize, const TIMERS: usize> {
    pub registry: Mutex<Registry<R, IO, TIMERS>>,
    pub binding: SharedOwner<'static, Binding<R>>,
}

pub struct NetworkHandle<R: Send + Sync + 'static, const IO: usize, const TIMERS: usize> {
    pub(crate) core: SharedOwner<'static, Core<R, IO, TIMERS>>,
    pub(crate) scope: AppScope,
}

impl<R: Send + Sync + 'static, const IO: usize, const TIMERS: usize> NetworkHandle<R, IO, TIMERS> {
    pub fn try_clone(&self) -> Result<Self, RuntimeFault> {
        Ok(Self {
            core: self
                .core
                .try_clone()
                .map_err(|reason| RuntimeFault::numeric(Site::Registry, reason))?,
            scope: self.scope,
        })
    }

    pub(crate) fn remove_io(&self, index: usize, generation: u64) {
        let removed = {
            let mut registry = lock(&self.core.value().registry);
            if registry.io[index]
                .as_ref()
                .is_some_and(|owner| lock(&owner.value().state).generation == generation)
            {
                registry.io[index].take()
            } else {
                None
            }
        };
        drop(removed);
    }

    pub(crate) fn remove_timer(&self, index: usize, generation: u64) {
        let removed = {
            let mut registry = lock(&self.core.value().registry);
            if registry.timers[index]
                .as_ref()
                .is_some_and(|owner| lock(&owner.value().state).generation == generation)
            {
                registry.timers[index].take()
            } else {
                None
            }
        };
        drop(removed);
    }
}

/// 注册容量由实际数组 backing 决定，不套用 helper 槽数；满表在分配/OS 调用前 typed 拒绝。
pub struct NetworkRuntime<R: Send + Sync + 'static, const IO: usize, const TIMERS: usize> {
    handle: NetworkHandle<R, IO, TIMERS>,
    io_cursor: usize,
    timer_cursor: usize,
}

impl<R: Send + Sync + 'static, const IO: usize, const TIMERS: usize> NetworkRuntime<R, IO, TIMERS> {
    pub fn layouts() -> Result<(std::alloc::Layout, std::alloc::Layout), RuntimeFault> {
        Ok((
            OwnerLayout::actual::<Binding<R>>(0)
                .map_err(|reason| RuntimeFault::numeric(Site::Binding, reason))?
                .backing,
            OwnerLayout::actual::<Core<R, IO, TIMERS>>(0)
                .map_err(|reason| RuntimeFault::numeric(Site::Registry, reason))?
                .backing,
        ))
    }

    /// foreign_root 必须是已收费的真实所有权输入；本函数不 clone/rebuild Source 图。
    /// russh pin 完成后可以直接 move 其唯一 ResourceRoot 到 R；有限 loopback 可传 ()。
    pub fn try_new(scope: AppScope, foreign_root: R) -> Result<Self, RuntimeFault> {
        let fault = Cell::new(None);
        let binding = scope
            .own(Site::Binding, 0, || {
                let startup = platform::Startup::new().map_err(|error| {
                    fault.set(Some(error));
                    NumericReject::ConstructionFailure
                })?;
                Ok(Binding {
                    _foreign_root: foreign_root,
                    _startup: startup,
                })
            })
            .map_err(|error| fault.get().unwrap_or(error))?;
        let core = scope.own(Site::Registry, 0, || {
            Ok(Core {
                registry: Mutex::new(Registry {
                    closed: false,
                    generation: 0,
                    io: std::array::from_fn(|_| None),
                    timers: std::array::from_fn(|_| None),
                }),
                binding,
            })
        })?;
        Ok(Self {
            handle: NetworkHandle { core, scope },
            io_cursor: 0,
            timer_cursor: 0,
        })
    }

    pub fn handle(&self) -> Result<NetworkHandle<R, IO, TIMERS>, RuntimeFault> {
        self.handle.try_clone()
    }

    /// 最多扫描/处理各 BATCH 个 IO/timer；每轮 cursor 前进，连续 ready 不无限 drain。
    /// wait 上界限制未扫描 timer 的调度延迟；没有 worker 线程或 ambient runtime。
    pub fn collect_ready(
        &mut self,
        requested_wait: Duration,
    ) -> Result<ReadyBatch<R>, RuntimeFault> {
        let mut owners: [Option<IoOwner<R>>; BATCH] = std::array::from_fn(|_| None);
        let mut generations = [0u64; BATCH];
        let mut fds = [PollEntry::default(); BATCH];
        let mut io_count = 0;
        let mut timers: [Option<TimerOwner<R>>; BATCH] = std::array::from_fn(|_| None);
        let mut timer_generations = [0u64; BATCH];
        let mut timer_count = 0;
        let now = Instant::now();
        let mut wait = requested_wait.min(Duration::from_millis(10));
        {
            let registry = lock(&self.handle.core.value().registry);
            if registry.closed {
                return Err(RuntimeFault::Cancelled);
            }
            for _ in 0..IO.min(BATCH) {
                let index = self.io_cursor;
                self.io_cursor = (self.io_cursor + 1) % IO;
                if let Some(owner) = &registry.io[index] {
                    let state = lock(&owner.value().state);
                    if !state.active {
                        continue;
                    }
                    let interest = (u8::from(state.reader.is_some()) * platform::READ)
                        | (u8::from(state.writer.is_some()) * platform::WRITE);
                    if interest == 0 {
                        continue;
                    }
                    fds[io_count] = PollEntry {
                        #[cfg(windows)]
                        raw: state.socket.raw(),
                        #[cfg(windows)]
                        interest,
                        ready: 0,
                    };
                    generations[io_count] = state.generation;
                    owners[io_count] = Some(
                        owner
                            .try_clone()
                            .map_err(|reason| RuntimeFault::numeric(Site::Poller, reason))?,
                    );
                    io_count += 1;
                }
            }
            for _ in 0..TIMERS.min(BATCH) {
                let index = self.timer_cursor;
                self.timer_cursor = (self.timer_cursor + 1) % TIMERS;
                if let Some(owner) = &registry.timers[index] {
                    let state = lock(&owner.value().state);
                    if !state.active || state.expired {
                        continue;
                    }
                    wait = wait.min(state.deadline.saturating_duration_since(now));
                    timer_generations[timer_count] = state.generation;
                    timers[timer_count] = Some(
                        owner
                            .try_clone()
                            .map_err(|reason| RuntimeFault::numeric(Site::Timer, reason))?,
                    );
                    timer_count += 1;
                }
            }
        }
        // 每个 OS fd 在整个 WSAPoll 期间都由 snapshot strong owner 保持，不与注销竞争 free。
        platform::poll(&mut fds[..io_count], wait)?;
        let now = Instant::now();
        for (owner, generation) in timers.into_iter().zip(timer_generations).take(timer_count) {
            if let Some(owner) = owner {
                super::timer::expire(&owner, generation, now);
            }
        }
        let mut batch = ReadyBatch {
            events: std::array::from_fn(|_| None),
        };
        for index in 0..io_count {
            if fds[index].ready != 0 {
                batch.events[index] = owners[index].take().map(|owner| IoEvent {
                    owner,
                    generation: generations[index],
                    ready: fds[index].ready,
                });
            }
        }
        Ok(batch)
    }

    pub fn drive(&mut self, wait: Duration) -> Result<usize, RuntimeFault> {
        Ok(self.collect_ready(wait)?.deliver())
    }

    /// 关闭新准入及所有注册。晚 snapshot/event/root 仍持有原 binding，直到实际最后 free。
    pub fn close(&mut self) {
        {
            lock(&self.handle.core.value().registry).closed = true;
        }
        for index in 0..IO {
            let owner = { lock(&self.handle.core.value().registry).io[index].take() };
            if let Some(owner) = owner {
                super::stream::cancel_record(&owner);
            }
        }
        for index in 0..TIMERS {
            let owner = { lock(&self.handle.core.value().registry).timers[index].take() };
            if let Some(owner) = owner {
                super::timer::cancel_record(&owner);
            }
        }
    }
}

impl<R: Send + Sync + 'static, const IO: usize, const TIMERS: usize> Drop
    for NetworkRuntime<R, IO, TIMERS>
{
    fn drop(&mut self) {
        self.close();
    }
}

/// 就绪批次本身内嵌，不创建 queue allocation；事件保有实际旧 socket root。
pub struct ReadyBatch<R: Send + Sync + 'static> {
    events: [Option<IoEvent<R>>; BATCH],
}
impl<R: Send + Sync + 'static> ReadyBatch<R> {
    pub fn take(&mut self, index: usize) -> Option<IoEvent<R>> {
        self.events.get_mut(index).and_then(Option::take)
    }
    pub fn deliver(self) -> usize {
        self.events
            .into_iter()
            .flatten()
            .map(|event| usize::from(event.deliver()))
            .sum()
    }
}
