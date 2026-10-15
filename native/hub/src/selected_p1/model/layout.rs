//! checked Layout；内嵌字段由外层计一次，递归图只另计 out-of-line backing。

use std::alloc::Layout;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicUsize};

use rshell_m0::rshell_core::{KeyBinding, KeyCode, KeyModifiers};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum NumericReject {
    Overflow = 1,
    InvalidLayout = 2,
    RustBudgetExceeded = 3,
    EpochMismatch = 4,
    SourceMismatch = 5,
    InvalidSource = 6,
    AllocationFailure = 7,
    RefcountOverflow = 8,
    Cancelled = 9,
    ConstructionFailure = 10,
    UnsupportedEnvelope = 11,
}

pub fn add(a: usize, b: usize) -> Result<usize, NumericReject> {
    a.checked_add(b).ok_or(NumericReject::Overflow)
}

pub fn mul(a: usize, b: usize) -> Result<usize, NumericReject> {
    a.checked_mul(b).ok_or(NumericReject::Overflow)
}

pub fn align_up(size: usize, align: usize) -> Result<usize, NumericReject> {
    if !align.is_power_of_two() {
        return Err(NumericReject::InvalidLayout);
    }
    Ok(add(size, align - 1)? & !(align - 1))
}

pub fn array(element: Layout, count: usize) -> Result<Layout, NumericReject> {
    let size = mul(element.pad_to_align().size(), count)?;
    Layout::from_size_align(size, element.align()).map_err(|_| NumericReject::InvalidLayout)
}

pub fn extend(a: Layout, b: Layout) -> Result<(Layout, usize), NumericReject> {
    a.extend(b)
        .map(|(layout, offset)| (layout.pad_to_align(), offset))
        .map_err(|_| NumericReject::InvalidLayout)
}

/// repr(Rust) 无 packed/额外 repr alignment 的字段排列上界。
/// 锁定 compiler 的 univariant_biased 从 aggregate ABI alignment 起逐字段对齐。
pub fn struct_upper<const N: usize>(
    fields: [Layout; N],
    aggregate_align: usize,
) -> Result<Layout, NumericReject> {
    let initial =
        Layout::from_size_align(0, aggregate_align).map_err(|_| NumericReject::InvalidLayout)?;
    fn visit<const N: usize>(
        fields: &[Layout; N],
        used: &mut [bool; N],
        depth: usize,
        layout: Layout,
        largest: &mut Layout,
    ) -> Result<(), NumericReject> {
        if depth == N {
            let padded = layout.pad_to_align();
            if padded.size() > largest.size()
                || (padded.size() == largest.size() && padded.align() > largest.align())
            {
                *largest = padded;
            }
            return Ok(());
        }
        for index in 0..N {
            if !used[index] {
                used[index] = true;
                // 中间不 pad；Layout::extend 自行按下一字段对齐。
                let next = layout
                    .extend(fields[index])
                    .map_err(|_| NumericReject::InvalidLayout)?
                    .0;
                visit(fields, used, depth + 1, next, largest)?;
                used[index] = false;
            }
        }
        Ok(())
    }
    let mut largest = initial;
    visit(&fields, &mut [false; N], 0, initial, &mut largest)?;
    Ok(largest)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NodeLayouts {
    pub leaf: Layout,
    pub internal: Layout,
}

/// exact std btree::node：B=6，Leaf 为五字段 repr(Rust)，Internal 为 repr(C)。
pub fn btree_nodes(
    key: Layout,
    value: Layout,
    aggregate_align: usize,
) -> Result<NodeLayouts, NumericReject> {
    let leaf = struct_upper(
        [
            Layout::new::<Option<std::ptr::NonNull<u8>>>(),
            Layout::new::<u16>(),
            Layout::new::<u16>(),
            array(key, 11)?,
            array(value, 11)?,
        ],
        aggregate_align,
    )?;
    let internal = extend(
        leaf,
        Layout::array::<std::ptr::NonNull<u8>>(12).map_err(|_| NumericReject::InvalidLayout)?,
    )?
    .0;
    Ok(NodeLayouts { leaf, internal })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PublicLayouts {
    pub string: Layout,
    pub path: Layout,
    pub value: Layout,
    pub key_code: Layout,
    pub key_modifiers: Layout,
    pub key_binding: Layout,
    pub pointer: Layout,
    pub atomic_usize: Layout,
    pub atomic_u32: Layout,
}

impl PublicLayouts {
    pub fn actual() -> Self {
        Self {
            string: Layout::new::<String>(),
            path: Layout::new::<PathBuf>(),
            value: Layout::new::<serde_json::Value>(),
            key_code: Layout::new::<KeyCode>(),
            key_modifiers: Layout::new::<KeyModifiers>(),
            key_binding: Layout::new::<KeyBinding>(),
            pointer: Layout::new::<*mut u8>(),
            atomic_usize: Layout::new::<AtomicUsize>(),
            atomic_u32: Layout::new::<AtomicU32>(),
        }
    }
}

/// 由 D/F/C/S/reply 的最终编译类型实例化；不提供占位产品 schema。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProductLayouts {
    pub descriptor: Layout,
    pub snapshot: Layout,
    pub source: Layout,
    pub payload: Layout,
    pub secret: Layout,
    pub future: Layout,
}

impl ProductLayouts {
    pub fn actual<D, S, F, P, Secret, Future>() -> Self {
        Self {
            descriptor: Layout::new::<D>(),
            snapshot: Layout::new::<S>(),
            source: Layout::new::<F>(),
            payload: Layout::new::<P>(),
            secret: Layout::new::<Secret>(),
            future: Layout::new::<Future>(),
        }
    }
}

#[cfg(test)]
#[path = "layout_tests.rs"]
mod tests;
