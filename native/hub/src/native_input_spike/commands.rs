//! ABI 3 的有界原型命令；所有借用指针仅活于本次调用。

use super::{
    core::{BUSY, Core, INVALID, STALE_OWNER, STOPPED},
    effects::{ActionLease, Effect, sequence},
    ffi::{SpikeOwner, VERSION, action, borrowed, guarded, guarded_owner, lookup, valid_pointer},
    payload::{PayloadGrant, TicketRef},
};
use rshell_m0::rshell_core::{CellPosition, SelectionRange, TerminalSize};
use std::{mem::size_of, sync::atomic::Ordering};

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct Command {
    pub version: u32,
    pub size: u32,
    pub opcode: u32,
    pub flags: u32,
    pub ticket: TicketRef,
    pub grant: PayloadGrant,
    pub args: [u64; 6],
    pub bytes: *const u8,
    pub length: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(super) struct Reply {
    pub version: u32,
    pub size: u32,
    pub status: u32,
    pub kind: u32,
    pub receipt: u64,
    pub effect: u64,
    pub ticket: TicketRef,
    pub grant: PayloadGrant,
    pub action: ActionLease,
}

pub(super) fn execute(core: &mut Core, command: Command, bytes: &[u8]) -> Result<Reply, u32> {
    let mut reply = Reply {
        version: VERSION,
        size: size_of::<Reply>() as u32,
        ..Reply::default()
    };
    if core.effects.revoked && !matches!(command.opcode, 9..=13) {
        return Err(STALE_OWNER);
    }
    if core.next_seq == u64::MAX && command.opcode != 13 {
        return Err(super::core::EXHAUSTED);
    }
    match command.opcode {
        1 => {
            // preedit：固定 native edit store，不申请远端槽。
            core.effects
                .payloads
                .update(command.ticket, command.args[0], bytes)?;
            reply.effect = command.ticket.effect;
        }
        2 => {
            core.effects.payload_ready(command.ticket)?;
            match core.effects.payloads.grant(
                command.ticket,
                usize::try_from(command.args[0]).map_err(|_| INVALID)?,
            ) {
                Ok(grant) => reply.grant = grant,
                Err(super::core::TEXT_CAPACITY) => {
                    core.effects.payloads.cancel(command.ticket)?;
                    core.effects.ticket_finished(command.ticket);
                    reply.status = super::core::TEXT_CAPACITY;
                    reply.kind = 2;
                    reply.effect = command.ticket.effect;
                    reply.ticket = command.ticket;
                    reply.receipt = sequence(&mut core.next_seq)?;
                    return Ok(reply);
                }
                Err(error) => return Err(error),
            }
            reply.effect = command.ticket.effect;
        }
        3 => {
            core.effects.payloads.chunk(
                command.ticket,
                command.grant,
                usize::try_from(command.args[0]).map_err(|_| INVALID)?,
                bytes,
                command.flags == 1,
            )?;
            reply.effect = command.ticket.effect;
        }
        4 => {
            reply.kind = u32::from(
                core.effects
                    .payloads
                    .finish(command.ticket, command.args[0])?,
            );
            core.effects.ticket_finished(command.ticket);
            reply.effect = command.ticket.effect;
        }
        5 => {
            core.effects.payloads.cancel(command.ticket)?;
            core.effects.ticket_finished(command.ticket);
            reply.effect = command.ticket.effect;
        }
        6 => {
            let admission = core.effects.independent(
                &mut core.next_seq,
                command.args[0],
                command.args[1],
                command.args[2],
            )?;
            reply.receipt = admission.seq;
            reply.effect = admission.effect;
            reply.ticket = admission.ticket;
            return Ok(reply);
        }
        7 => {
            let effect = match command.args[0] {
                1 => Effect::Resize(TerminalSize {
                    cols: u16::try_from(command.args[1]).map_err(|_| INVALID)?,
                    rows: u16::try_from(command.args[2]).map_err(|_| INVALID)?,
                    pixel_width: 120,
                    pixel_height: 60,
                    dpi: 96,
                }),
                2 => Effect::Clear,
                3 => Effect::Selection(Some(SelectionRange {
                    start: CellPosition {
                        stable_row: i64::try_from(command.args[1]).map_err(|_| INVALID)?,
                        column: u16::try_from(command.args[2]).map_err(|_| INVALID)?,
                    },
                    end: CellPosition {
                        stable_row: i64::try_from(command.args[3]).map_err(|_| INVALID)?,
                        column: u16::try_from(command.args[4]).map_err(|_| INVALID)?,
                    },
                    rectangular: false,
                })),
                4 => Effect::Copy,
                5 => Effect::Viewport(i64::try_from(command.args[1]).map_err(|_| INVALID)?),
                6 => Effect::FrameAck(command.args[1]),
                7 => Effect::Action(
                    action(u32::try_from(command.args[1]).map_err(|_| INVALID)?)?.ok_or(INVALID)?,
                ),
                8 => Effect::Selection(None),
                _ => return Err(INVALID),
            };
            let admission = core.effects.control(&mut core.next_seq, effect)?;
            reply.receipt = admission.seq;
            reply.effect = admission.effect;
            reply.ticket = admission.ticket;
            return Ok(reply);
        }
        8 => {
            // 单次 claim；实际消费者在锁外执行。
            reply.action = core.effects.claim_action().ok_or(BUSY)?;
            reply.effect = reply.action.effect;
        }
        9 | 10 => {
            let lease = ActionLease {
                owner: command.args[0],
                context: command.args[1],
                effect: command.args[2],
                action: u32::try_from(command.args[3]).map_err(|_| INVALID)?,
                target: u32::try_from(command.args[4]).map_err(|_| INVALID)?,
            };
            reply.receipt = core.effects.ack_action(
                &mut core.next_seq,
                lease,
                u32::try_from(command.args[5]).map_err(|_| INVALID)?,
            )?;
            reply.effect = lease.effect;
            // editable action 先确认已进入 pending-revoke，再线性化撤权，避免等待自己的 ACK。
            if command.opcode == 10 {
                core.effects.revoke();
            }
            return Ok(reply);
        }
        11 => core.effects.revoke(),
        12 => core.effects.confirm_retired(command.args[0])?,
        13 => {
            reply.effect = core.effects.applied;
            reply.kind = u32::from(core.effects.revoked);
            return Ok(reply);
        }
        14 => {
            // 显式 fixture chunk；生产资源能力不由此签发。
            let chunk = usize::try_from(command.args[0]).map_err(|_| INVALID)?;
            if !(1..=super::core::SLOT_BYTES).contains(&chunk) || core.charge().0 != 0 {
                return Err(INVALID);
            }
            core.effects.chunk = chunk;
        }
        15 => core
            .effects
            .cancel_deadline(&mut core.next_seq, &core.engine)?,
        17 => {
            use rshell_m0::rshell_core::{
                KeyModifiers, MouseButton, MouseEventKind, TerminalMouseEvent,
            };
            if command.args[5] & !15 != 0 {
                return Err(INVALID);
            }
            let row = u16::try_from(command.args[2]).map_err(|_| INVALID)?;
            let event = TerminalMouseEvent {
                kind: match command.args[4] {
                    0 => MouseEventKind::Press,
                    1 => MouseEventKind::Move,
                    2 => MouseEventKind::Release,
                    3 => MouseEventKind::Scroll,
                    _ => return Err(INVALID),
                },
                button: Some(match command.args[3] {
                    0 => MouseButton::Left,
                    1 => MouseButton::Middle,
                    2 => MouseButton::Right,
                    3 => MouseButton::WheelUp,
                    4 => MouseButton::WheelDown,
                    _ => return Err(INVALID),
                }),
                cell: CellPosition {
                    stable_row: i64::from(row),
                    column: u16::try_from(command.args[1]).map_err(|_| INVALID)?,
                },
                viewport_row: row,
                pixel_x: 0,
                pixel_y: 0,
                modifiers: KeyModifiers {
                    shift: command.args[5] & 1 != 0,
                    control: command.args[5] & 2 != 0,
                    alt: command.args[5] & 4 != 0,
                    super_key: command.args[5] & 8 != 0,
                },
            };
            let admission = core.effects.control(
                &mut core.next_seq,
                Effect::Pointer {
                    event,
                    frame: command.args[0],
                },
            )?;
            reply.receipt = admission.seq;
            reply.effect = admission.effect;
            return Ok(reply);
        }
        _ => return Err(INVALID),
    }
    reply.receipt = sequence(&mut core.next_seq)?;
    Ok(reply)
}

/// # Safety
/// cap/command/out 是有效、对齐的独占借用；bytes 在整个调用中可读且不可变。
/// 独立 text 只允许原生适配器已证明排他且静默的非硬件 edit scope；不得由 Dart 铸造。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guossh_native_input_spike_command(
    cap: *const SpikeOwner,
    command: *const Command,
    out: *mut Reply,
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
            let command = match unsafe { borrowed(command) } {
                Ok(command) => command,
                Err(e) => return e,
            };
            if command.version != VERSION
                || command.size as usize != size_of::<Command>()
                || command.flags > 1
                || command.length > super::payload::PAYLOAD_LIMIT
                || (command.length != 0 && command.bytes.is_null())
            {
                return INVALID;
            }
            let completion = matches!(command.opcode, 9..=13);
            if owner.stopped.load(Ordering::Acquire) && !completion {
                return STOPPED;
            }
            let bytes = if command.length == 0 {
                &[][..]
            } else {
                unsafe { std::slice::from_raw_parts(command.bytes, command.length) }
            };
            let mut core = match owner.core.try_lock() {
                Ok(core) => core,
                Err(std::sync::TryLockError::WouldBlock) => {
                    return if command.opcode == 11 {
                        owner.pending_revoke.store(true, Ordering::Release);
                        BUSY
                    } else if command.opcode == 5 {
                        owner.defer_cancel(command.ticket)
                    } else {
                        BUSY
                    };
                }
                Err(_) => {
                    owner.stop();
                    return STOPPED;
                }
            };
            if owner.stopped.load(Ordering::Acquire) && !completion {
                return STOPPED;
            }
            let reply = execute(&mut core, command, bytes).unwrap_or_else(|status| Reply {
                version: VERSION,
                size: size_of::<Reply>() as u32,
                status,
                ..Reply::default()
            });
            if reply.status == super::core::OK && matches!(command.opcode, 10 | 11) {
                // 终结同一 fixture transport owner；不让 pending write 阻挡撤权 fence。
                // writer future 的取消／stream 关闭完成之前，Core 仍保留 Flight charge。
                owner.stop();
            }
            unsafe { out.write(reply) };
            reply.status
        })
    })
}
