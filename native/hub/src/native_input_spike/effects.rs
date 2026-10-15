//! 原槽 effect log：admission、apply、write completion 是三个不同事实。

use super::{
    core::{
        ACTIVE_KEYS, Admission, BUSY, Binding, ENGINE_FAILURE, EXHAUSTED, INVALID, MODES,
        NORMAL_SLOTS, OK, Packet, REMOTE_LIMIT, REMOTE_START, SLOT_BYTES, SLOTS, STALE_OWNER,
    },
    deadline::{Cell, SyncTimer},
    payload::{self, Payloads, Phase, TicketRef},
};
use crate::desktop_input_policy::{
    GatedIntent, InputPolicyState, InputReject, KeyResult, NoSendReason, PolicyKeyInput,
    apply_intent, clear_scrollback, gate_key,
};
use crate::signals::desktop_session::{DesktopAction, DesktopKeyPhase};
use rshell_m0::rshell_core::{
    MouseEventKind, RenderFrame, SelectionRange, TerminalInput, TerminalMouseEvent, TerminalSize,
    Viewport,
};
use rshell_m0::rshell_session::TerminalEngine;
use std::{num::NonZeroU64, sync::Arc, time::Instant};

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct ActionLease {
    pub owner: u64,
    pub context: u64,
    pub effect: u64,
    pub action: u32,
    pub target: u32,
}

pub(super) enum Effect {
    Key {
        intent: GatedIntent,
        cycle: usize,
        down: bool,
    },
    Text(usize),
    Remote(usize),
    Resize(TerminalSize),
    Clear,
    Selection(Option<SelectionRange>),
    Copy,
    Viewport(i64),
    FrameAck(u64),
    Action(DesktopAction),
    Deadline(u64),
    Pointer {
        event: TerminalMouseEvent,
        frame: u64,
    },
}

enum Slot {
    Free,
    Release,
    Pending {
        seq: u64,
        effect: Effect,
    },
    Writing {
        seq: u64,
        bytes: Vec<u8>,
        offset: usize,
        flight: usize,
        ticket: Option<usize>,
        deadline: bool,
        chunk: usize,
        cycle: Option<usize>,
    },
    Action {
        lease: ActionLease,
        claimed: bool,
        cycle: Option<usize>,
    },
}

impl Slot {
    fn seq(&self) -> Option<u64> {
        match self {
            Self::Pending { seq, .. } | Self::Writing { seq, .. } => Some(*seq),
            Self::Action { lease, .. } => Some(lease.effect),
            Self::Free | Self::Release => None,
        }
    }
}

#[derive(Clone, Copy)]
struct Cycle {
    physical: NonZeroU64,
    held: bool,
    pending: usize,
    rejected: bool,
    ticket: Option<TicketRef>,
}

pub(super) struct Effects {
    binding: Binding,
    slots: [Slot; SLOTS],
    cycles: [Option<Cycle>; ACTIVE_KEYS],
    pub payloads: Payloads,
    pub sync: SyncTimer,
    pub selection: Option<SelectionRange>,
    pub size: TerminalSize,
    pub frame: Option<Arc<RenderFrame>>,
    pub frame_revision: u64,
    pub frame_ack: u64,
    pub applied: u64,
    pub last_written: u64,
    pub remote_received: u64,
    pub remote_applied: u64,
    pub copied: String,
    pub active_text: Option<TicketRef>,
    pub revoked: bool,
    pub retired: bool,
    pub chunk: usize,
    viewport: i64,
    independent_allowed: bool,
    pointer_remote: Option<bool>,
    last_edit_scope: u64,
    closed_text: u64,
}

pub(super) fn sequence(next: &mut u64) -> Result<u64, u32> {
    let seq = *next;
    *next = next.checked_add(1).ok_or(EXHAUSTED)?;
    Ok(seq)
}

impl Effects {
    pub fn new(binding: Binding, size: TerminalSize) -> Self {
        Self {
            binding,
            slots: std::array::from_fn(|_| Slot::Free),
            cycles: [None; ACTIVE_KEYS],
            payloads: Payloads::default(),
            sync: SyncTimer::new(binding.handle, binding.generation),
            selection: None,
            size,
            frame: None,
            frame_revision: 0,
            frame_ack: 0,
            applied: 0,
            last_written: 0,
            remote_received: 0,
            remote_applied: 0,
            copied: String::new(),
            active_text: None,
            revoked: false,
            retired: false,
            chunk: SLOT_BYTES,
            viewport: 0,
            independent_allowed: true,
            pointer_remote: None,
            last_edit_scope: 0,
            closed_text: 0,
        }
    }

    fn free(&self, start: usize, end: usize) -> Option<usize> {
        (start..end).find(|&i| matches!(self.slots[i], Slot::Free))
    }

    fn receipt(seq: u64, effect: u64, disposition: u32, ticket: Option<TicketRef>) -> Admission {
        Admission {
            status: OK,
            disposition,
            seq,
            action: 0,
            bytes: 0,
            effect,
            ticket: ticket.unwrap_or_default(),
        }
    }

    pub fn key(
        &mut self,
        policy: &mut InputPolicyState,
        profile: &rshell_m0::rshell_core::ResolvedTerminalProfile,
        next: &mut u64,
        input: PolicyKeyInput,
    ) -> Result<Admission, u32> {
        if self.revoked {
            return Err(STALE_OWNER);
        }
        let id = input.physical_identity.ok_or(INVALID)?;
        let phase = input.phase;
        let existing = self
            .cycles
            .iter()
            .position(|c| c.is_some_and(|c| c.physical == id && c.held));
        let cycle = match (phase, existing) {
            (DesktopKeyPhase::Down, Some(_)) => return Err(INVALID),
            (DesktopKeyPhase::Down, None) => match self.cycles.iter().position(Option::is_none) {
                Some(cycle) => cycle,
                None => {
                    if let Some(ticket) = self.active_text {
                        self.payloads.cancel(ticket)?;
                        self.ticket_finished(ticket);
                        let seq = sequence(next)?;
                        let mut receipt = Self::receipt(seq, ticket.effect, 0, Some(ticket));
                        receipt.status = super::core::ACTIVE_KEY_BUDGET;
                        return Ok(receipt);
                    }
                    return Err(super::core::ACTIVE_KEY_BUDGET);
                }
            },
            (_, Some(c)) => c,
            (_, None) => return Err(INVALID),
        };
        // 固定 4096-byte 影子策略属于同步 scratch，不进入任意长度队列。
        let mut classified = policy.clone();
        let intent = gate_key(&mut classified, profile, input);
        let native = matches!(&intent, GatedIntent::Quiet(o) if o.result == KeyResult::NoSend(NoSendReason::NativeText));
        let rejected = matches!(&intent, GatedIntent::Quiet(o) if matches!(o.result, KeyResult::TypedInputReject(_)));
        let quiet = matches!(&intent, GatedIntent::Quiet(o) if matches!(o.result, KeyResult::NoSend(_) | KeyResult::TypedInputReject(_)));
        let paste = matches!(&intent, GatedIntent::Quiet(o) if o.result == KeyResult::Action(DesktopAction::Paste));
        let old_ticket = existing.and_then(|c| self.cycles[c]).and_then(|c| c.ticket);
        let member = if native {
            if phase == DesktopKeyPhase::Down {
                self.active_text
            } else {
                old_ticket
            }
        } else {
            None
        };
        let new_text = (native && member.is_none() && phase == DesktopKeyPhase::Down) || paste;
        let ticket_slot = if new_text {
            Some(self.payloads.available().ok_or(BUSY)?)
        } else {
            None
        };
        let effect_slot = if new_text || !quiet {
            Some(if phase == DesktopKeyPhase::Up {
                NORMAL_SLOTS + cycle
            } else {
                self.free(0, NORMAL_SLOTS).ok_or(BUSY)?
            })
        } else {
            None
        };
        let seq = sequence(next)?;
        let mut ticket = member;
        if let Some(t) = ticket_slot {
            ticket = Some(self.payloads.open(
                t,
                self.binding.handle,
                self.binding.context as u64,
                seq,
                paste,
            )?);
            if native {
                self.active_text = ticket;
            }
        }
        if phase == DesktopKeyPhase::Down {
            self.cycles[cycle] = Some(Cycle {
                physical: id,
                held: true,
                pending: 0,
                rejected,
                ticket,
            });
            self.slots[NORMAL_SLOTS + cycle] = Slot::Release;
        }
        if let Some(slot) = effect_slot {
            let effect = if let Some(t) = ticket_slot {
                Effect::Text(t)
            } else {
                Effect::Key {
                    intent,
                    cycle,
                    down: phase == DesktopKeyPhase::Down,
                }
            };
            if ticket_slot.is_none()
                && let Some(c) = &mut self.cycles[cycle]
            {
                c.pending += 1;
            }
            self.slots[slot] = Slot::Pending { seq, effect };
        }
        *policy = classified;
        if rejected {
            self.independent_allowed = false;
        }
        if phase == DesktopKeyPhase::Up {
            if let Some(c) = &mut self.cycles[cycle] {
                c.held = false;
            }
            self.release_cycle(cycle);
            self.capacity_released(next)?;
        }
        let effect = ticket.map_or(seq, |t| t.effect);
        Ok(Self::receipt(
            seq,
            effect,
            if rejected {
                0
            } else if native {
                3
            } else if quiet {
                2
            } else {
                1
            },
            ticket,
        ))
    }

    fn release_cycle(&mut self, cycle: usize) {
        if self.cycles[cycle].is_some_and(|c| !c.held && c.pending == 0) {
            self.cycles[cycle] = None;
            if matches!(self.slots[NORMAL_SLOTS + cycle], Slot::Release) {
                self.slots[NORMAL_SLOTS + cycle] = Slot::Free;
            }
        }
    }

    pub fn independent(
        &mut self,
        next: &mut u64,
        context: u64,
        edit_scope: u64,
        retired_through: u64,
    ) -> Result<Admission, u32> {
        if context != self.binding.context as u64
            || edit_scope <= self.last_edit_scope
            || retired_through != self.closed_text
            || !self.independent_allowed
            || self.revoked
            || self.active_text.is_some()
            || self.cycles.iter().any(Option::is_some)
        {
            return Err(INVALID);
        }
        let slot = self.free(0, NORMAL_SLOTS).ok_or(BUSY)?;
        let t = self.payloads.available().ok_or(BUSY)?;
        let seq = sequence(next)?;
        let ticket = self
            .payloads
            .open(t, self.binding.handle, context, seq, false)?;
        self.active_text = Some(ticket);
        self.last_edit_scope = edit_scope;
        self.slots[slot] = Slot::Pending {
            seq,
            effect: Effect::Text(t),
        };
        Ok(Self::receipt(seq, seq, 3, Some(ticket)))
    }

    pub fn control(&mut self, next: &mut u64, effect: Effect) -> Result<Admission, u32> {
        if self.revoked {
            return Err(STALE_OWNER);
        }
        if let Effect::Pointer { event, frame } = &effect
            && (*frame != self.frame_revision
                || event.cell.column >= self.size.cols
                || event.viewport_row >= self.size.rows)
        {
            return Err(INVALID);
        }
        if let Effect::Resize(size) = &effect
            && (size.cols == 0 || size.cols > 12 || size.rows == 0 || size.rows > 3)
        {
            return Err(INVALID);
        }
        if let Effect::Selection(Some(range)) = &effect {
            let valid = |p: rshell_m0::rshell_core::CellPosition| {
                p.column <= self.size.cols && (0..i64::from(self.size.rows)).contains(&p.stable_row)
            };
            if !valid(range.start) || !valid(range.end) {
                return Err(INVALID);
            }
        }
        let slot = self.free(0, NORMAL_SLOTS).ok_or(BUSY)?;
        let paste = matches!(effect, Effect::Action(DesktopAction::Paste));
        let ticket_slot = if paste {
            Some(self.payloads.available().ok_or(BUSY)?)
        } else {
            None
        };
        let seq = sequence(next)?;
        let ticket = if let Some(t) = ticket_slot {
            let ticket = self.payloads.open(
                t,
                self.binding.handle,
                self.binding.context as u64,
                seq,
                true,
            )?;
            self.slots[slot] = Slot::Pending {
                seq,
                effect: Effect::Text(t),
            };
            Some(ticket)
        } else {
            self.slots[slot] = Slot::Pending { seq, effect };
            None
        };
        Ok(Self::receipt(seq, seq, 1, ticket))
    }

    pub fn remote(&mut self, next: &mut u64, bytes: &[u8]) -> Result<bool, u32> {
        let command = MODES.iter().position(|c| *c == bytes).ok_or(INVALID)?;
        if self.revoked {
            return Err(STALE_OWNER);
        }
        if self.remote_received == REMOTE_LIMIT {
            return Err(EXHAUSTED);
        }
        let Some(slot) = self.free(REMOTE_START, SLOTS) else {
            return Ok(false);
        };
        let seq = sequence(next)?;
        self.slots[slot] = Slot::Pending {
            seq,
            effect: Effect::Remote(command),
        };
        self.remote_received += 1;
        Ok(true)
    }

    pub fn ticket_finished(&mut self, reference: TicketRef) {
        if self.active_text == Some(reference) {
            self.active_text = None;
            self.closed_text = reference.effect;
        }
    }

    pub fn timer_wake(&mut self, next: &mut u64, wake: u64, now: Instant) -> Result<(), u32> {
        if !self.revoked && self.sync.wake(wake, now) {
            self.capacity_released(next)?;
        }
        Ok(())
    }

    fn capacity_released(&mut self, next: &mut u64) -> Result<(), u32> {
        if let Cell::Waiting { wake, witness } = self.sync.cell
            && let Some(slot) = self.free(0, NORMAL_SLOTS)
        {
            let seq = sequence(next)?;
            self.slots[slot] = Slot::Pending {
                seq,
                effect: Effect::Deadline(wake),
            };
            self.sync.cell = Cell::Queued {
                wake,
                witness,
                effect: seq,
            };
        }
        Ok(())
    }

    fn head(&self) -> Option<usize> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.seq().map(|seq| (i, seq)))
            .min_by_key(|(_, seq)| *seq)
            .map(|(i, _)| i)
    }

    pub fn payload_ready(&self, ticket: TicketRef) -> Result<(), u32> {
        let t = self.payloads.find(ticket)?;
        if self.payloads.tickets[t].paste && !self.head().is_some_and(|slot| matches!(self.slots[slot], Slot::Pending { effect: Effect::Text(candidate), .. } if candidate == t)) {
            return Err(BUSY);
        }
        Ok(())
    }

    pub fn claimed_copy(&self, lease: ActionLease) -> bool {
        self.head().is_some_and(|slot| matches!(self.slots[slot], Slot::Action { lease: expected, claimed: true, .. } if expected == lease && lease.action == 11))
    }

    fn project(&mut self, engine: &mut dyn TerminalEngine) -> Result<(), u32> {
        // 只提取 fixture 的 <=36 个 ASCII cell；不 pack/pub、不调用用户回调。
        self.frame_revision = self.frame_revision.checked_add(1).ok_or(EXHAUSTED)?;
        self.frame = Some(
            engine
                .render(
                    Viewport {
                        top_stable_row: self.viewport,
                        rows: self.size.rows,
                    },
                    self.selection,
                )
                .map_err(|_| ENGINE_FAILURE)?,
        );
        Ok(())
    }

    fn action_slot(&mut self, slot: usize, seq: u64, action: DesktopAction) {
        self.slots[slot] = Slot::Action {
            lease: ActionLease {
                owner: self.binding.handle,
                context: self.binding.context as u64,
                effect: seq,
                action: super::core::action_number(action),
                target: self.binding.session,
            },
            claimed: false,
            cycle: None,
        };
    }

    fn prepare_action(
        &mut self,
        engine: &mut dyn TerminalEngine,
        action: DesktopAction,
    ) -> Result<(), u32> {
        if action == DesktopAction::Copy {
            self.copied = if let Some(range) = self.selection {
                engine.selected_text(range).map_err(|_| ENGINE_FAILURE)?
            } else {
                String::new()
            };
        } else if action == DesktopAction::SelectAll {
            self.selection = Some(SelectionRange {
                start: rshell_m0::rshell_core::CellPosition {
                    stable_row: self.viewport,
                    column: 0,
                },
                end: rshell_m0::rshell_core::CellPosition {
                    stable_row: self.viewport + i64::from(self.size.rows) - 1,
                    column: self.size.cols,
                },
                rectangular: false,
            });
            self.project(engine)?;
        }
        Ok(())
    }

    pub fn take_write(
        &mut self,
        engine: &mut dyn TerminalEngine,
    ) -> Result<Option<(usize, Packet)>, u32> {
        let Some(slot) = self.head() else {
            return Ok(None);
        };
        if matches!(&self.slots[slot], Slot::Action { .. }) {
            return Ok(None);
        }
        if let Slot::Pending {
            effect: Effect::Text(t),
            ..
        } = &self.slots[slot]
            && self.payloads.tickets[*t].phase == Phase::Open
        {
            return Ok(None);
        }
        if matches!(&self.slots[slot], Slot::Pending { .. }) {
            let Slot::Pending { seq, effect } =
                std::mem::replace(&mut self.slots[slot], Slot::Release)
            else {
                return Err(INVALID);
            };
            let mut ticket = None;
            let mut deadline = false;
            let mut changed = false;
            let mut chunk = SLOT_BYTES;
            let mut writing_cycle = None;
            let bytes = match effect {
                Effect::Key {
                    intent,
                    cycle,
                    down,
                } => {
                    writing_cycle = Some(cycle);
                    if matches!(intent, GatedIntent::Send { .. }) {
                        chunk = self.chunk;
                    }
                    let rejected = self.cycles[cycle].is_some_and(|c| c.rejected);
                    let outcome = if rejected {
                        None
                    } else {
                        Some(apply_intent(engine, intent).map_err(|_| ENGINE_FAILURE)?)
                    };
                    if let Some(c) = &mut self.cycles[cycle]
                        && down
                        && outcome.as_ref().is_some_and(|o| {
                            o.result == KeyResult::TypedInputReject(InputReject::UnsupportedInput)
                        })
                    {
                        c.rejected = true;
                        self.independent_allowed = false;
                    }
                    match outcome.map(|o| o.result) {
                        Some(KeyResult::Bytes(bytes)) => bytes,
                        Some(KeyResult::Action(action)) => {
                            self.prepare_action(engine, action)?;
                            self.action_slot(slot, seq, action);
                            if let Slot::Action {
                                cycle: action_cycle,
                                ..
                            } = &mut self.slots[slot]
                            {
                                *action_cycle = Some(cycle);
                            }
                            self.applied = seq;
                            return Ok(None);
                        }
                        Some(KeyResult::ClearScrollback) => {
                            self.sync.mutate()?;
                            clear_scrollback(engine).map_err(|_| ENGINE_FAILURE)?;
                            self.selection = None;
                            changed = true;
                            Vec::new()
                        }
                        _ => Vec::new(),
                    }
                }
                Effect::Text(t) => {
                    ticket = Some(t);
                    let transaction = &mut self.payloads.tickets[t];
                    if transaction.phase == Phase::Cancelled {
                        Vec::new()
                    } else if transaction.paste {
                        payload::paste(&transaction.text, engine.display_modes().bracketed_paste)
                    } else {
                        // 已授 full input 容量；CommittedText 的固定实现只转移 String 所有权。
                        engine
                            .encode_input(TerminalInput::CommittedText(std::mem::take(
                                &mut transaction.text,
                            )))
                            .map_err(|_| ENGINE_FAILURE)?
                    }
                }
                Effect::Remote(command) => {
                    self.sync.mutate()?;
                    let delta = engine.advance(MODES[command]).map_err(|_| ENGINE_FAILURE)?;
                    self.remote_applied += 1;
                    changed = delta.dirty && engine.sync_deadline().is_none();
                    delta.outbound
                }
                Effect::Resize(size) => {
                    self.sync.mutate()?;
                    engine.resize(size).map_err(|_| ENGINE_FAILURE)?;
                    self.size = size;
                    changed = true;
                    Vec::new()
                }
                Effect::Clear => {
                    self.sync.mutate()?;
                    clear_scrollback(engine).map_err(|_| ENGINE_FAILURE)?;
                    self.selection = None;
                    changed = true;
                    Vec::new()
                }
                Effect::Selection(range) => {
                    self.selection = range;
                    changed = true;
                    Vec::new()
                }
                Effect::Copy => {
                    self.copied = if let Some(range) = self.selection {
                        engine.selected_text(range).map_err(|_| ENGINE_FAILURE)?
                    } else {
                        String::new()
                    };
                    self.action_slot(slot, seq, DesktopAction::Copy);
                    self.applied = seq;
                    return Ok(None);
                }
                Effect::Viewport(top) => {
                    self.viewport = top.clamp(
                        engine.viewport_bounds().first_stable_row,
                        engine.viewport_bounds().bottom_top_stable_row,
                    );
                    changed = true;
                    Vec::new()
                }
                Effect::FrameAck(revision) => {
                    if revision <= self.frame_revision {
                        self.frame_ack = self.frame_ack.max(revision);
                    }
                    Vec::new()
                }
                Effect::Action(action) => {
                    self.prepare_action(engine, action)?;
                    self.action_slot(slot, seq, action);
                    self.applied = seq;
                    return Ok(None);
                }
                Effect::Pointer { event, frame } => {
                    if frame != self.frame_revision {
                        self.pointer_remote = None;
                        Vec::new()
                    } else {
                        if event.kind == MouseEventKind::Press {
                            self.pointer_remote = Some(
                                !event.modifiers.shift && engine.display_modes().mouse_reporting,
                            );
                        }
                        let remote = self.pointer_remote.unwrap_or(
                            !event.modifiers.shift && engine.display_modes().mouse_reporting,
                        );
                        let bytes = if remote {
                            match engine.encode_mouse(event) {
                                Ok(bytes) => bytes,
                                Err(rshell_m0::rshell_session::EngineError::UnsupportedMouse(
                                    _,
                                )) => Vec::new(),
                                Err(_) => return Err(ENGINE_FAILURE),
                            }
                        } else {
                            if matches!(
                                event.kind,
                                MouseEventKind::Press
                                    | MouseEventKind::Move
                                    | MouseEventKind::Release
                            ) {
                                let start = if event.kind == MouseEventKind::Press {
                                    event.cell
                                } else {
                                    self.selection.map_or(event.cell, |range| range.start)
                                };
                                self.selection = Some(SelectionRange {
                                    start,
                                    end: event.cell,
                                    rectangular: false,
                                });
                                changed = true;
                            }
                            Vec::new()
                        };
                        if event.kind == MouseEventKind::Release {
                            self.pointer_remote = None;
                        }
                        bytes
                    }
                }
                Effect::Deadline(generation) => {
                    if let Cell::Queued {
                        wake,
                        witness,
                        effect,
                    } = self.sync.cell
                        && wake == generation
                        && effect == seq
                    {
                        self.sync.cell = Cell::Applying {
                            wake,
                            witness,
                            effect,
                        };
                        deadline = true;
                        if self
                            .sync
                            .matches(witness, engine.sync_deadline(), Instant::now())
                        {
                            self.sync.mutate()?;
                            let delta = engine.end_sync().map_err(|_| ENGINE_FAILURE)?;
                            self.sync.applied += 1;
                            changed = delta.dirty;
                            delta.outbound
                        } else {
                            self.sync.stale += 1;
                            Vec::new()
                        }
                    } else {
                        Vec::new()
                    }
                }
            };
            if changed {
                self.project(engine)?;
            }
            self.sync.reconcile(engine.sync_deadline())?;
            self.applied = seq;
            self.slots[slot] = Slot::Writing {
                seq,
                bytes,
                offset: 0,
                flight: 0,
                ticket,
                deadline,
                chunk,
                cycle: writing_cycle,
            };
        }
        let Slot::Writing {
            seq,
            bytes,
            offset,
            flight,
            chunk,
            ..
        } = &mut self.slots[slot]
        else {
            return Ok(None);
        };
        if *flight != 0 {
            return Ok(None);
        }
        let mut end = (*offset + *chunk).min(bytes.len());
        // 引擎 key/binding 输出和文本均是 UTF-8；仅在 scalar 边界切块。
        while end > *offset && end < bytes.len() && bytes[end] & 0xc0 == 0x80 {
            end -= 1;
        }
        if end == *offset && end < bytes.len() {
            return Err(INVALID);
        }
        let mut packet = Packet {
            seq: *seq,
            bytes: [0; SLOT_BYTES],
            len: end - *offset,
        };
        packet.bytes[..packet.len].copy_from_slice(&bytes[*offset..end]);
        *flight = packet.len + 1;
        Ok(Some((slot, packet)))
    }

    pub fn finish_write(
        &mut self,
        engine: &dyn TerminalEngine,
        next: &mut u64,
        slot: usize,
        seq: u64,
    ) -> Result<(), u32> {
        let Some(Slot::Writing {
            seq: actual,
            bytes,
            offset,
            flight,
            ticket,
            deadline,
            cycle,
            ..
        }) = self.slots.get_mut(slot)
        else {
            return Err(INVALID);
        };
        if *actual != seq || *flight == 0 {
            return Err(INVALID);
        }
        *offset += *flight - 1;
        *flight = 0;
        if *offset < bytes.len() && !self.revoked {
            return Ok(());
        }
        if let Some(t) = *ticket {
            self.payloads.release(t);
            self.payloads.tickets[t].phase = Phase::Done;
        }
        if *deadline {
            self.sync.settle();
        }
        let cycle = *cycle;
        self.slots[slot] = Slot::Free;
        if let Some(cycle) = cycle {
            if let Some(c) = &mut self.cycles[cycle] {
                c.pending -= 1;
            }
            self.release_cycle(cycle);
        }
        self.last_written = seq;
        self.capacity_released(next)?;
        if !self.revoked {
            self.sync.reconcile(engine.sync_deadline())?;
        }
        Ok(())
    }

    pub fn claim_action(&mut self) -> Option<ActionLease> {
        let slot = self.head()?;
        if let Slot::Action { lease, claimed, .. } = &mut self.slots[slot]
            && !*claimed
        {
            *claimed = true;
            return Some(*lease);
        }
        None
    }

    pub fn ack_action(
        &mut self,
        next: &mut u64,
        lease: ActionLease,
        result: u32,
    ) -> Result<u64, u32> {
        if result > 2 {
            return Err(INVALID);
        }
        let slot = self.head().ok_or(INVALID)?;
        if !matches!(&self.slots[slot], Slot::Action { lease: expected, claimed: true, .. } if *expected == lease)
        {
            return Err(INVALID);
        }
        let seq = sequence(next)?;
        let cycle = match self.slots[slot] {
            Slot::Action { cycle, .. } => cycle,
            _ => None,
        };
        self.slots[slot] = Slot::Free;
        if let Some(cycle) = cycle {
            if let Some(c) = &mut self.cycles[cycle] {
                c.pending -= 1;
            }
            self.release_cycle(cycle);
        }
        self.capacity_released(next)?;
        Ok(seq)
    }

    pub fn charge(&self) -> (u32, u32) {
        (
            self.slots
                .iter()
                .filter(|s| !matches!(s, Slot::Free))
                .count() as u32,
            self.slots
                .iter()
                .filter(|s| matches!(s, Slot::Writing { flight, .. } if *flight != 0))
                .count() as u32,
        )
    }

    pub fn retained_bytes(&self) -> usize {
        self.payloads.retained
            + self
                .slots
                .iter()
                .map(|s| match s {
                    Slot::Writing { bytes, .. } => bytes.capacity(),
                    _ => 0,
                })
                .sum::<usize>()
    }

    pub fn window_bytes(&self) -> u32 {
        self.slots
            .iter()
            .map(|s| match s {
                Slot::Writing { flight, .. } if *flight != 0 => (*flight - 1) as u32,
                _ => 0,
            })
            .sum()
    }

    pub fn revoke(&mut self) {
        self.revoked = true;
        self.active_text = None;
        self.sync.settle();
        for t in 0..payload::TICKETS {
            let inflight = self.slots.iter().any(|s| matches!(s, Slot::Writing { ticket: Some(ticket), flight, .. } if *ticket == t && *flight != 0));
            if !inflight {
                self.payloads.release(t);
                self.payloads.tickets[t].phase = Phase::Done;
            }
        }
        self.cycles = [None; ACTIVE_KEYS];
        for slot in &mut self.slots {
            if !matches!(
                slot,
                Slot::Action { claimed: true, .. } | Slot::Writing { flight: 1.., .. }
            ) {
                *slot = Slot::Free;
            }
        }
    }

    pub fn cancel_deadline(
        &mut self,
        next: &mut u64,
        engine: &dyn TerminalEngine,
    ) -> Result<(), u32> {
        // 日志节点先去引用，再重用单一 timer cell；已经在写的输出不能撤回。
        if matches!(self.sync.cell, Cell::Applying { .. }) {
            return Err(BUSY);
        }
        if let Cell::Queued { effect, .. } = self.sync.cell {
            for slot in &mut self.slots {
                if matches!(slot, Slot::Pending { seq, effect: Effect::Deadline(_), .. } if *seq == effect)
                {
                    *slot = Slot::Free;
                }
            }
        }
        self.sync.settle();
        if !self.revoked {
            self.sync.reconcile(engine.sync_deadline())?;
        }
        self.capacity_released(next)
    }

    pub fn confirm_retired(&mut self, context: u64) -> Result<(), u32> {
        if context != self.binding.context as u64 || !self.revoked || self.head().is_some() {
            return Err(BUSY);
        }
        self.retired = true;
        Ok(())
    }

    pub fn discard(&mut self) {
        self.revoke();
        for slot in &mut self.slots {
            if !matches!(slot, Slot::Action { claimed: true, .. }) {
                *slot = Slot::Free;
            }
        }
        self.payloads.discard();
        self.frame = None;
        self.copied = String::new();
    }
}
