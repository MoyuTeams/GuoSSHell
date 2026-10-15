//! 锁定 RawVec 增长及 BTree insert/clone 的固定标量算法。

use std::alloc::Layout;

use super::layout::{NodeLayouts, NumericReject, add, array, mul};

pub fn min_non_zero_cap(size: usize) -> usize {
    match size {
        0 => 0,
        1 => 8,
        2..=1024 => 4,
        _ => 1,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Growth {
    Amortized,
    Exact,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VecPeak {
    element: Layout,
    pub len: usize,
    pub capacity: usize,
    pub peak_bytes: usize,
}

impl VecPeak {
    pub fn new(element: Layout, initial: usize) -> Result<Self, NumericReject> {
        Ok(Self {
            element,
            len: 0,
            capacity: if element.size() == 0 {
                usize::MAX
            } else {
                initial
            },
            peak_bytes: array(element, initial)?.size(),
        })
    }

    pub fn append(&mut self, additional: usize, growth: Growth) -> Result<(), NumericReject> {
        let required = add(self.len, additional)?;
        let mut capacity = self.capacity;
        let mut peak = self.peak_bytes;
        if required > capacity {
            capacity = match growth {
                Growth::Exact => required,
                Growth::Amortized => mul(capacity, 2)?
                    .max(required)
                    .max(min_non_zero_cap(self.element.size())),
            };
            peak = peak.max(add(
                array(self.element, self.capacity)?.size(),
                array(self.element, capacity)?.size(),
            )?);
        }
        self.len = required;
        self.capacity = capacity;
        self.peak_bytes = peak;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapacityUpper {
    pub capacity: usize,
    pub peak_bytes: usize,
}

/// 仅知最大长度 n：最后一次增长的 oldcap <= n-1；包含 initial backing。
pub fn maximum_length(
    element: Layout,
    initial: usize,
    n: usize,
) -> Result<CapacityUpper, NumericReject> {
    let initial_bytes = array(element, initial)?.size();
    if element.size() == 0 || n <= initial {
        return Ok(CapacityUpper {
            capacity: if element.size() == 0 {
                usize::MAX
            } else {
                initial
            },
            peak_bytes: initial_bytes,
        });
    }
    let old_cap = n - 1;
    let capacity = min_non_zero_cap(element.size())
        .max(n)
        .max(mul(old_cap, 2)?);
    let peak_bytes = initial_bytes.max(add(
        array(element, old_cap)?.size(),
        array(element, capacity)?.size(),
    )?);
    Ok(CapacityUpper {
        capacity,
        peak_bytes,
    })
}

pub fn cautious(element: Layout, n: usize) -> usize {
    if element.size() == 0 {
        n
    } else {
        n.min((1024 * 1024) / element.size())
    }
}

pub fn completed_nodes(n: usize) -> Result<usize, NumericReject> {
    if n == 0 { Ok(0) } else { add(1, (n - 1) / 5) }
}

pub fn height_upper(n: usize) -> usize {
    // 2*6^h-1 <= n 等价于 6^h <= ceil(n/2)，不计算 n+1。
    let half = n / 2 + n % 2;
    let mut power = 1usize;
    let mut height = 0;
    while power <= half / 6 {
        power *= 6;
        height += 1;
    }
    height
}

pub fn insert_peak_nodes(n: usize) -> Result<usize, NumericReject> {
    match n {
        0 => Ok(0),
        1..=11 => Ok(1),
        _ => Ok(
            completed_nodes(n)?.max(add(completed_nodes(n - 1)?, add(height_upper(n - 1), 2)?)?)
        ),
    }
}

pub fn tree_insert_peak(nodes: NodeLayouts, occurrences: usize) -> Result<usize, NumericReject> {
    mul(
        insert_peak_nodes(occurrences)?,
        nodes.leaf.size().max(nodes.internal.size()),
    )
}

pub fn tree_clone_backing(nodes: NodeLayouts, entries: usize) -> Result<usize, NumericReject> {
    mul(
        completed_nodes(entries)?,
        nodes.leaf.size().max(nodes.internal.size()),
    )
}

#[cfg(test)]
#[path = "capacity_tests.rs"]
mod tests;
