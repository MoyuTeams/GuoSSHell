//! 单次固定 projection lease；CPU snapshot 与锁外 ABI packing 分离。

use super::{
    core::{BUSY, INVALID, OK, STOPPED},
    ffi::{SpikeOwner, VERSION, borrowed, guarded, guarded_owner, lookup, valid_pointer},
};
use rshell_m0::rshell_core::Color;
use std::{mem::size_of, sync::atomic::Ordering};

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(super) struct Cell {
    pub scalar: u32,
    pub foreground: u32,
    pub background: u32,
    pub attributes: u32,
    pub width: u32,
    pub selected: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct Projection {
    pub version: u32,
    pub size: u32,
    pub owner: u64,
    pub source_generation: u64,
    pub frame: u64,
    pub effect: u64,
    pub rows: u32,
    pub cols: u32,
    pub cursor_row: i64,
    pub cursor_column: u32,
    pub count: u32,
    pub cells: [Cell; 36],
}

fn color(color: Color) -> u32 {
    match color {
        Color::Default => 0,
        Color::Ansi(index) => 0x0100_0000 | u32::from(index),
        Color::Rgb(r, g, b) => 0x0200_0000 | u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b),
    }
}

/// # Safety
/// 原 ActionLease 必须仍处于 Claimed；out/count 是有效独占借用，capacity<=128。
/// 剪贴板写入由 caller 在本函数返回、所有 core 锁释放后完成，再 ACK 原 lease。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guossh_native_input_spike_copy(
    cap: *const SpikeOwner,
    lease: *const super::effects::ActionLease,
    out: *mut u8,
    capacity: usize,
    count: *mut usize,
) -> u32 {
    guarded(|| {
        if out.is_null() || !valid_pointer(count) || !(1..=128).contains(&capacity) {
            return INVALID;
        }
        let owner = match unsafe { borrowed(cap) }.and_then(lookup) {
            Ok(owner) => owner,
            Err(e) => return e,
        };
        guarded_owner(owner, || {
            let lease = match unsafe { borrowed(lease) } {
                Ok(lease) => lease,
                Err(e) => return e,
            };
            let mut captured = [0; 128];
            let length = {
                let core = match owner.core.try_lock() {
                    Ok(core) => core,
                    Err(_) => return BUSY,
                };
                if owner.stopped.load(Ordering::Acquire) || core.effects.revoked {
                    return STOPPED;
                }
                if !core.effects.claimed_copy(lease) || core.effects.copied.len() > capacity {
                    return INVALID;
                }
                let bytes = core.effects.copied.as_bytes();
                captured[..bytes.len()].copy_from_slice(bytes);
                bytes.len()
            };
            unsafe {
                std::ptr::copy_nonoverlapping(captured.as_ptr(), out, length);
                count.write(length);
            }
            OK
        })
    })
}

/// # Safety
/// cap/out 是有效借用；out 至少为一个 Projection。只允许 fixture source 的只读投影。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guossh_native_input_spike_projection(
    cap: *const SpikeOwner,
    out: *mut Projection,
) -> u32 {
    guarded(|| {
        if !valid_pointer(out) {
            return INVALID;
        }
        let owner = match unsafe { borrowed(cap) }.and_then(lookup) {
            Ok(owner) => owner,
            Err(e) => return e,
        };
        guarded_owner(owner, || {
            let _lease = match owner.projection.try_lock() {
                Ok(lease) => lease,
                Err(_) => return BUSY,
            };
            if owner.stopped.load(Ordering::Acquire) {
                return STOPPED;
            }
            let (frame, revision, effect) = {
                let core = match owner.core.try_lock() {
                    Ok(core) => core,
                    Err(_) => return BUSY,
                };
                let Some(frame) = &core.effects.frame else {
                    return BUSY;
                };
                (
                    std::sync::Arc::clone(frame),
                    core.effects.frame_revision,
                    core.effects.applied,
                )
            };
            let mut projection = Projection {
                version: VERSION,
                size: size_of::<Projection>() as u32,
                owner: owner.binding.handle,
                source_generation: owner.binding.generation,
                frame: revision,
                effect,
                rows: u32::from(frame.size.rows),
                cols: u32::from(frame.size.cols),
                cursor_row: frame.cursor.map_or(0, |c| c.position.stable_row),
                cursor_column: frame.cursor.map_or(0, |c| u32::from(c.position.column)),
                count: 0,
                cells: [Cell::default(); 36],
            };
            for cell in frame.rows.iter().flat_map(|row| row.cells.iter()) {
                let Some(out) = projection.cells.get_mut(projection.count as usize) else {
                    return INVALID;
                };
                let mut chars = cell.text.chars();
                let Some(scalar) = chars.next() else {
                    return INVALID;
                };
                if chars.next().is_some() {
                    return INVALID;
                }
                *out = Cell {
                    scalar: u32::from(scalar),
                    foreground: color(cell.foreground),
                    background: color(cell.background),
                    attributes: u32::from(cell.attributes.bold)
                        | u32::from(cell.attributes.italic) << 1
                        | u32::from(cell.attributes.underline) << 2
                        | u32::from(cell.attributes.strike) << 3
                        | u32::from(cell.attributes.reverse) << 4,
                    width: u32::from(cell.width),
                    selected: u32::from(cell.selected),
                };
                projection.count += 1;
            }
            unsafe { out.write(projection) };
            OK
        })
    })
}
