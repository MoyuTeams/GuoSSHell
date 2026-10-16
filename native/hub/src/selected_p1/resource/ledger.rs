//! 进程账本：epoch/source 失效只关闭准入，不释放旧资源。

use std::sync::{Mutex, MutexGuard};

use crate::selected_p1::model::layout::{NumericReject, add};

pub const BYTE_LIMIT: usize = 403_701_760;
pub const SOURCE_COUNT: usize = 8;
pub const CLASS_COUNT: usize = 9;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ResourceClass {
    Persistent = 0,
    Helper = 1,
    Raw = 2,
    Decode = 3,
    Control = 4,
    Secret = 5,
    Source = 6,
    Runtime = 7,
    Reply = 8,
}

/// 拒绝记录来自同一次锁内评估；Count/Inflight 的用途分类由准入调用点决定。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReservationReject {
    Numeric(NumericReject),
    ByteLimit {
        requested: usize,
        current: usize,
        limit: usize,
    },
    ClassSlots {
        class: ResourceClass,
        requested: u32,
        current: u32,
        limit: u32,
    },
}

impl ReservationReject {
    pub fn into_numeric(self) -> NumericReject {
        match self {
            Self::Numeric(error) => error,
            Self::ByteLimit { .. } | Self::ClassSlots { .. } => NumericReject::RustBudgetExceeded,
        }
    }
}

impl From<NumericReject> for ReservationReject {
    fn from(error: NumericReject) -> Self {
        Self::Numeric(error)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Amount {
    pub rust_bytes: usize,
    pub c_bytes: usize,
    pub slots: u32,
}

impl Amount {
    pub fn total(self) -> Result<usize, NumericReject> {
        add(self.rust_bytes, self.c_bytes)
    }

    fn subtract(self, other: Self) -> Result<Self, NumericReject> {
        Ok(Self {
            rust_bytes: self
                .rust_bytes
                .checked_sub(other.rust_bytes)
                .ok_or(NumericReject::RustBudgetExceeded)?,
            c_bytes: self
                .c_bytes
                .checked_sub(other.c_bytes)
                .ok_or(NumericReject::RustBudgetExceeded)?,
            slots: self
                .slots
                .checked_sub(other.slots)
                .ok_or(NumericReject::RustBudgetExceeded)?,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceToken {
    pub epoch: u64,
    pub generation: u64,
    pub index: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Admission {
    pub epoch: u64,
    pub source: Option<SourceToken>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ClassUsage {
    pub rust_bytes: usize,
    pub c_bytes: usize,
    pub slots: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LedgerSnapshot {
    pub epoch: u64,
    pub rust_bytes: usize,
    pub c_bytes: usize,
    pub classes: [ClassUsage; CLASS_COUNT],
}

#[derive(Clone, Copy)]
struct SourceRecord {
    generation: u64,
    active: bool,
}

struct State {
    epoch: u64,
    rust_bytes: usize,
    c_bytes: usize,
    classes: [ClassUsage; CLASS_COUNT],
    sources: [SourceRecord; SOURCE_COUNT],
}

pub struct ProcessLedger {
    byte_limit: usize,
    slot_limits: [u32; CLASS_COUNT],
    state: Mutex<State>,
}

/// 静态控制无 Rust allocator request。hub 仍须在所有 owner/worker 存活时保持 loaded。
pub static PROCESS_LEDGER: ProcessLedger =
    ProcessLedger::new(BYTE_LIMIT, [1, 4, 8, 8, 8, 8, u32::MAX, u32::MAX, u32::MAX]);

impl ProcessLedger {
    pub const fn new(byte_limit: usize, slot_limits: [u32; CLASS_COUNT]) -> Self {
        Self {
            byte_limit: if byte_limit > BYTE_LIMIT {
                BYTE_LIMIT
            } else {
                byte_limit
            },
            slot_limits,
            state: Mutex::new(State {
                epoch: 1,
                rust_bytes: 0,
                c_bytes: 0,
                classes: [ClassUsage {
                    rust_bytes: 0,
                    c_bytes: 0,
                    slots: 0,
                }; CLASS_COUNT],
                sources: [SourceRecord {
                    generation: 0,
                    active: false,
                }; SOURCE_COUNT],
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // 锁内只有 checked 数字更新，没有调用用户代码或分配。
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn snapshot(&self) -> LedgerSnapshot {
        let state = self.lock();
        LedgerSnapshot {
            epoch: state.epoch,
            rust_bytes: state.rust_bytes,
            c_bytes: state.c_bytes,
            classes: state.classes,
        }
    }

    pub fn open_source(&self, epoch: u64, index: u32) -> Result<SourceToken, NumericReject> {
        let index_usize = usize::try_from(index).map_err(|_| NumericReject::InvalidSource)?;
        let mut state = self.lock();
        if state.epoch != epoch {
            return Err(NumericReject::EpochMismatch);
        }
        let record = state
            .sources
            .get_mut(index_usize)
            .ok_or(NumericReject::InvalidSource)?;
        if record.active {
            return Err(NumericReject::SourceMismatch);
        }
        let generation = record
            .generation
            .checked_add(1)
            .ok_or(NumericReject::Overflow)?;
        record.generation = generation;
        record.active = true;
        Ok(SourceToken {
            epoch,
            generation,
            index,
        })
    }

    pub fn invalidate_source(&self, token: SourceToken) -> Result<(), NumericReject> {
        let mut state = self.lock();
        Self::validate(
            &state,
            Admission {
                epoch: token.epoch,
                source: Some(token),
            },
        )?;
        state.sources[token.index as usize].active = false;
        Ok(())
    }

    pub fn advance_epoch(&self, expected: u64) -> Result<u64, NumericReject> {
        let mut state = self.lock();
        if state.epoch != expected {
            return Err(NumericReject::EpochMismatch);
        }
        let next = expected.checked_add(1).ok_or(NumericReject::Overflow)?;
        state.epoch = next;
        for source in &mut state.sources {
            source.active = false;
        }
        Ok(next)
    }

    fn validate(state: &State, admission: Admission) -> Result<(), NumericReject> {
        if state.epoch != admission.epoch {
            return Err(NumericReject::EpochMismatch);
        }
        if let Some(token) = admission.source {
            if token.epoch != admission.epoch {
                return Err(NumericReject::EpochMismatch);
            }
            let index = usize::try_from(token.index).map_err(|_| NumericReject::InvalidSource)?;
            let record = state
                .sources
                .get(index)
                .ok_or(NumericReject::InvalidSource)?;
            if !record.active || record.generation != token.generation {
                return Err(NumericReject::SourceMismatch);
            }
        }
        Ok(())
    }

    pub fn try_reserve(
        &self,
        amount: Amount,
        class: ResourceClass,
        admission: Admission,
    ) -> Result<Grant<'_>, NumericReject> {
        self.try_reserve_detailed(amount, class, admission)
            .map_err(ReservationReject::into_numeric)
    }

    pub fn try_reserve_detailed(
        &self,
        amount: Amount,
        class: ResourceClass,
        admission: Admission,
    ) -> Result<Grant<'_>, ReservationReject> {
        let requested = amount.total()?;
        let mut state = self.lock();
        Self::validate(&state, admission)?;
        let occupied = add(state.rust_bytes, state.c_bytes)?;
        let total = add(occupied, requested)?;
        let index = class as usize;
        let current = state.classes[index];
        let slots = current
            .slots
            .checked_add(amount.slots)
            .ok_or(NumericReject::Overflow)?;
        if total > self.byte_limit {
            return Err(ReservationReject::ByteLimit {
                requested,
                current: occupied,
                limit: self.byte_limit,
            });
        }
        if slots > self.slot_limits[index] {
            return Err(ReservationReject::ClassSlots {
                class,
                requested: amount.slots,
                current: current.slots,
                limit: self.slot_limits[index],
            });
        }
        let class_rust = add(current.rust_bytes, amount.rust_bytes)?;
        let class_c = add(current.c_bytes, amount.c_bytes)?;
        let rust_bytes = add(state.rust_bytes, amount.rust_bytes)?;
        let c_bytes = add(state.c_bytes, amount.c_bytes)?;
        state.rust_bytes = rust_bytes;
        state.c_bytes = c_bytes;
        state.classes[index] = ClassUsage {
            rust_bytes: class_rust,
            c_bytes: class_c,
            slots,
        };
        Ok(Grant {
            ledger: self,
            rust_bytes: amount.rust_bytes,
            c_bytes: amount.c_bytes,
            epoch: admission.epoch,
            class,
            slots: amount.slots,
        })
    }

    fn release(&self, class: ResourceClass, amount: Amount) {
        let mut state = self.lock();
        // Grant 不能由外部伪造；split/tighten 守恒，release 不受当前 epoch 影响。
        state.rust_bytes -= amount.rust_bytes;
        state.c_bytes -= amount.c_bytes;
        let usage = &mut state.classes[class as usize];
        usage.rust_bytes -= amount.rust_bytes;
        usage.c_bytes -= amount.c_bytes;
        usage.slots -= amount.slots;
    }
}

/// 未绑定的额度可以拆分/退还；真实 owner 接管后不再暴露可修改 Grant。
#[repr(C)]
pub struct Grant<'root> {
    ledger: &'root ProcessLedger,
    rust_bytes: usize,
    c_bytes: usize,
    epoch: u64,
    class: ResourceClass,
    slots: u32,
}

impl<'root> Grant<'root> {
    pub fn amount(&self) -> Amount {
        Amount {
            rust_bytes: self.rust_bytes,
            c_bytes: self.c_bytes,
            slots: self.slots,
        }
    }
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    pub fn class(&self) -> ResourceClass {
        self.class
    }

    pub fn split(&mut self, amount: Amount) -> Result<Self, NumericReject> {
        let remaining = self.amount().subtract(amount)?;
        self.rust_bytes = remaining.rust_bytes;
        self.c_bytes = remaining.c_bytes;
        self.slots = remaining.slots;
        Ok(Self {
            ledger: self.ledger,
            rust_bytes: amount.rust_bytes,
            c_bytes: amount.c_bytes,
            epoch: self.epoch,
            class: self.class,
            slots: amount.slots,
        })
    }

    /// 只对尚未绑定 backing 的部分调用；owner 内部额度不能通过此接口提早释放。
    pub fn tighten_unused(&mut self, retained: Amount) -> Result<(), NumericReject> {
        let returned = self.amount().subtract(retained)?;
        self.rust_bytes = retained.rust_bytes;
        self.c_bytes = retained.c_bytes;
        self.slots = retained.slots;
        self.ledger.release(self.class, returned);
        Ok(())
    }
}

impl Drop for Grant<'_> {
    fn drop(&mut self) {
        self.ledger.release(self.class, self.amount());
    }
}

#[cfg(test)]
#[path = "ledger_tests.rs"]
mod tests;
