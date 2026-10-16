//! 单 engine 的保守 mutation witness；notice 不读取或解析 CSI 内容。

use super::core::EXHAUSTED;
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Witness {
    pub engine: u64,
    pub source_generation: u64,
    pub epoch: u64,
    pub deadline: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Cell {
    Idle,
    Armed {
        wake: u64,
        witness: Witness,
    },
    Waiting {
        wake: u64,
        witness: Witness,
    },
    Queued {
        wake: u64,
        witness: Witness,
        effect: u64,
    },
    Applying {
        wake: u64,
        witness: Witness,
        effect: u64,
    },
}

pub(super) struct SyncTimer {
    pub cell: Cell,
    pub epoch: u64,
    pub wakes: u64,
    pub stale: u64,
    pub applied: u64,
    generation: u64,
    engine: u64,
    source_generation: u64,
}

impl SyncTimer {
    pub fn new(engine: u64, source_generation: u64) -> Self {
        Self {
            cell: Cell::Idle,
            epoch: 0,
            wakes: 0,
            stale: 0,
            applied: 0,
            generation: 0,
            engine,
            source_generation,
        }
    }
    pub fn mutate(&mut self) -> Result<(), u32> {
        self.epoch = self.epoch.checked_add(1).ok_or(EXHAUSTED)?;
        Ok(())
    }
    pub fn reconcile(&mut self, deadline: Option<Instant>) -> Result<(), u32> {
        if !matches!(self.cell, Cell::Idle | Cell::Armed { .. }) {
            return Ok(());
        }
        let witness = deadline.map(|deadline| Witness {
            engine: self.engine,
            source_generation: self.source_generation,
            epoch: self.epoch,
            deadline,
        });
        if matches!((self.cell, witness), (Cell::Armed { witness: old, .. }, Some(new)) if old == new)
        {
            return Ok(());
        }
        self.generation = self.generation.checked_add(1).ok_or(EXHAUSTED)?;
        self.cell = witness.map_or(Cell::Idle, |witness| Cell::Armed {
            wake: self.generation,
            witness,
        });
        Ok(())
    }
    pub fn wake(&mut self, wake: u64, now: Instant) -> bool {
        if let Cell::Armed {
            wake: current,
            witness,
        } = self.cell
            && current == wake
            && witness.deadline <= now
        {
            self.cell = Cell::Waiting { wake, witness };
            self.wakes += 1;
            return true;
        }
        false
    }
    pub fn matches(&self, witness: Witness, deadline: Option<Instant>, now: Instant) -> bool {
        witness.engine == self.engine
            && witness.source_generation == self.source_generation
            && witness.epoch == self.epoch
            && deadline == Some(witness.deadline)
            && witness.deadline <= now
    }
    pub fn settle(&mut self) {
        self.cell = Cell::Idle;
    }
}
