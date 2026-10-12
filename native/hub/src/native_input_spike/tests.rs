use super::{
    commands::{Command, Reply, execute},
    core::{
        self, BUSY, Binding, Core, INVALID, MODES, NORMAL_SLOTS, OK, Owner, STOPPED, STREAM_BYTES,
    },
    deadline::Cell,
    effects::{ActionLease, Effect},
    ffi::{self, SpikeKey, SpikeOpen, SpikeOwner, VERSION},
    payload::{PayloadGrant, TicketRef},
    transport,
};
use rshell_m0::rshell_core::{CellPosition, SelectionRange, TerminalSize};
use rshell_m0::rshell_session::TerminalEngine;
use std::{
    ffi::c_void,
    mem::{offset_of, size_of},
    num::NonZeroU64,
    sync::{Arc, MutexGuard, atomic::Ordering},
    time::{Duration, Instant},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

fn success<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("fixture 失败：{error:?}"),
    }
}
fn lock(owner: &Owner) -> MutexGuard<'_, Core> {
    success(owner.core.lock())
}
fn key(id: u64, kind: u32, value: u32) -> SpikeKey {
    SpikeKey {
        version: VERSION,
        size: size_of::<SpikeKey>() as u32,
        physical: id,
        phase: 0,
        raw_source: 1,
        raw_kind: kind,
        raw_value: value,
        dual_source: 1,
        code_kind: kind,
        code_value: value,
        legacy_kind: kind,
        legacy_value: value,
        modifiers: 0,
        alt_source: 1,
        alt_sides: 0,
        text_ownership: 1,
        default_action: 0,
        native_event: id,
        layout_revision: 1,
        same_event_state: 0,
    }
}
fn native(id: u64) -> SpikeKey {
    SpikeKey {
        text_ownership: 2,
        ..key(id, 1, u32::from('a'))
    }
}
fn submit(owner: &Owner, input: SpikeKey) -> core::Admission {
    owner.key(
        match NonZeroU64::new(input.physical) {
            Some(id) => id,
            None => panic!("physical=0"),
        },
        success(ffi::phase(input.phase)),
        ffi::decode(input),
    )
}
fn fixture(capacity: usize) -> (Arc<Owner>, DuplexStream, DuplexStream) {
    let (stream, peer) = tokio::io::duplex(capacity);
    let (_, unused_peer) = tokio::io::duplex(STREAM_BYTES);
    let owner = success(Owner::new(
        Binding {
            handle: 1,
            context: 1,
            session: 0xfa00_0000,
            generation: 1,
            identity: [42; 16],
            ui_thread: std::thread::current().id(),
        },
        unused_peer,
    ));
    (owner, stream, peer)
}
fn command(opcode: u32, ticket: TicketRef) -> Command {
    Command {
        version: VERSION,
        size: size_of::<Command>() as u32,
        opcode,
        flags: 0,
        ticket,
        grant: PayloadGrant::default(),
        args: [0; 6],
        bytes: std::ptr::null(),
        length: 0,
    }
}
fn finish(owner: &Owner, ticket: TicketRef, text: &str, revision: u64) {
    let mut c = lock(owner);
    let mut grant = command(2, ticket);
    grant.args[0] = text.len() as u64;
    let grant = success(execute(&mut c, grant, &[])).grant;
    let mut offset = 0;
    while offset < text.len() {
        let mut end = (offset + grant.chunk as usize).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let mut chunk = command(3, ticket);
        chunk.grant = grant;
        chunk.args[0] = offset as u64;
        chunk.flags = u32::from(end == text.len());
        success(execute(&mut c, chunk, &text.as_bytes()[offset..end]));
        offset = end;
    }
    if text.is_empty() {
        let mut chunk = command(3, ticket);
        chunk.grant = grant;
        chunk.flags = 1;
        success(execute(&mut c, chunk, &[]));
    }
    let mut commit = command(4, ticket);
    commit.args[0] = revision;
    assert_eq!(success(execute(&mut c, commit, &[])).kind, 1);
    assert_eq!(
        success(execute(&mut c, commit, &[])).kind,
        0,
        "重复 commit 按实际 revision 状态幂等"
    );
}
fn drain(owner: &Owner) -> Vec<u8> {
    let mut c = lock(owner);
    let mut bytes = Vec::new();
    while let Some((slot, packet)) = success(c.take_write()) {
        bytes.extend_from_slice(&packet.bytes[..packet.len]);
        success(c.finish_write(slot, packet.seq));
    }
    bytes
}
fn control(owner: &Owner, effect: Effect) -> core::Admission {
    let mut c = lock(owner);
    let Core {
        effects, next_seq, ..
    } = &mut *c;
    success(effects.control(next_seq, effect))
}
fn ack(owner: &Owner, lease: ActionLease, result: u32) {
    let mut c = lock(owner);
    let Core {
        effects, next_seq, ..
    } = &mut *c;
    success(effects.ack_action(next_seq, lease, result));
}
fn claim(owner: &Owner) -> ActionLease {
    match lock(owner).effects.claim_action() {
        Some(lease) => lease,
        None => panic!("尚无 action lease"),
    }
}
async fn until(predicate: impl Fn() -> bool) {
    success(
        tokio::time::timeout(Duration::from_secs(3), async {
            while !predicate() {
                tokio::task::yield_now().await;
            }
        })
        .await,
    );
}
async fn observed(peer: &mut DuplexStream, expected: &[u8]) {
    let mut bytes = vec![0; expected.len()];
    success(success(
        tokio::time::timeout(Duration::from_secs(3), peer.read_exact(&mut bytes)).await,
    ));
    assert_eq!(bytes, expected);
}
async fn send(peer: &mut DuplexStream, command: usize) {
    success(peer.write_all(&[MODES[command].len() as u8]).await);
    success(peer.write_all(MODES[command]).await);
}
fn at_write_boundary<T>(owner: &Owner, body: impl FnOnce() -> T) -> T {
    let gate = Arc::new(core::TestGate {
        entered: std::sync::Barrier::new(2),
        release: std::sync::Barrier::new(2),
    });
    *success(owner.write_boundary.lock()) = Some(Arc::clone(&gate));
    gate.entered.wait();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
    *success(owner.write_boundary.lock()) = None;
    gate.release.wait();
    match result {
        Ok(value) => value,
        Err(error) => std::panic::resume_unwind(error),
    }
}
struct Running(Arc<Owner>);
impl Running {
    fn start(owner: &Arc<Owner>, stream: DuplexStream) -> Self {
        success(transport::start(Arc::clone(owner), stream));
        Self(Arc::clone(owner))
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        self.0.stop();
        let owner = Arc::clone(&self.0);
        let join = match std::thread::Builder::new().spawn(move || owner.join_worker()) {
            Ok(thread) => match thread.join() {
                Ok(status) => (0, status),
                Err(_) => (2, core::TRANSPORT_FAILURE),
            },
            Err(_) => (1, core::TRANSPORT_FAILURE),
        };
        // 停用与真实 join 后只检查清理；poison 不清除，也不恢复服务。
        let core = self.0.core.lock().unwrap_or_else(|e| e.into_inner());
        let state = (
            join,
            self.0.worker_done.load(Ordering::Acquire),
            self.0.worker_joined.load(Ordering::Acquire),
            core.charge(),
            core.effects.retained_bytes(),
        );
        drop(core);
        let clean = ((0, OK), true, true, (0, 0), 0);
        if std::thread::panicking() {
            if state != clean {
                eprintln!("fixture 清理失败：{state:?}");
            }
        } else {
            assert_eq!(state, clean);
        }
    }
}

#[test]
fn primary_panic_cleanup_joins_releases_payload_and_preserves_poisoned_owner() {
    let (owner, stream, _peer) = fixture(1);
    let text = submit(&owner, native(1));
    finish(&owner, text.ticket, &"字🦀".repeat(20), 1);
    assert_ne!(lock(&owner).effects.retained_bytes(), 0);
    let primary = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _running = Running::start(&owner, stream);
        let _guard = lock(&owner);
        panic!("固定 primary panic 注入");
    }));
    assert!(primary.is_err());
    assert!(owner.worker_done.load(Ordering::Acquire));
    assert!(owner.worker_joined.load(Ordering::Acquire));
    assert!(owner.core.is_poisoned());
    let core = owner.core.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(core.charge(), (0, 0));
    assert_eq!(core.effects.retained_bytes(), 0);
    drop(core);
    assert_eq!(submit(&owner, native(2)).status, STOPPED);
    assert!(owner.core.is_poisoned());
}

#[derive(Default)]
struct Gui {
    tabs: u32,
    zoom: i32,
    claims: Vec<u64>,
}
impl Gui {
    fn apply(&mut self, owner: &Owner, lease: ActionLease) {
        assert_eq!(lease.target, owner.binding.session);
        assert!(!self.claims.contains(&lease.effect));
        self.claims.push(lease.effect);
        match lease.action {
            1 => self.tabs += 1,
            14 => self.zoom += 1,
            15 => self.zoom -= 1,
            16 => self.zoom = 0,
            _ => {}
        }
    }
}

#[tokio::test]
async fn composition_members_barrier_actual_stream_projection_and_gui_ack() {
    let (owner, stream, mut peer) = fixture(1);
    let engine = {
        let c = lock(&owner);
        (&c.engine as *const _) as usize
    };
    let t = submit(&owner, native(1));
    assert_eq!((t.status, t.disposition), (OK, 3));
    assert_eq!(
        submit(
            &owner,
            SpikeKey {
                phase: 2,
                ..native(1)
            }
        )
        .ticket,
        t.ticket
    );
    success(lock(&owner).remote(MODES[2]));
    control(
        &owner,
        Effect::Resize(TerminalSize {
            cols: 8,
            rows: 2,
            pixel_width: 80,
            pixel_height: 40,
            dpi: 96,
        }),
    );
    control(&owner, Effect::Clear);
    let action = submit(
        &owner,
        SpikeKey {
            default_action: 14,
            ..key(2, 16, 2)
        },
    );
    assert_eq!(action.action, 0, "gate 不是 GUI dispatch");
    assert_eq!(
        submit(
            &owner,
            SpikeKey {
                phase: 2,
                ..key(2, 16, 2)
            }
        )
        .status,
        OK
    );
    for revision in 1..=3 {
        let mut update = command(1, t.ticket);
        update.args[0] = revision;
        success(execute(&mut lock(&owner), update, "组合".as_bytes()));
    }
    for id in 3..100 {
        let down = submit(&owner, native(id));
        assert_eq!((down.ticket, down.effect), (t.ticket, t.effect));
        assert_eq!(
            submit(
                &owner,
                SpikeKey {
                    phase: 1,
                    ..native(id)
                }
            )
            .ticket,
            t.ticket
        );
        assert_eq!(
            submit(
                &owner,
                SpikeKey {
                    phase: 2,
                    ..native(id)
                }
            )
            .ticket,
            t.ticket
        );
    }
    assert!(drain(&owner).is_empty());
    {
        let c = lock(&owner);
        assert_eq!(c.remote_applied, 0);
        assert_eq!(c.effects.size.cols, 12);
        assert_eq!(c.effects.frame_revision, 0);
        assert_eq!(c.last_written_seq, 0);
    }
    let text = "同一原票据🦀组合结果".repeat(12);
    assert!(text.len() > 32);
    finish(&owner, t.ticket, &text, 4);
    control(&owner, Effect::FrameAck(2));
    let _running = Running::start(&owner, stream);
    until(|| lock(&owner).charge().1 == 1).await;
    {
        let c = lock(&owner);
        assert_eq!(c.remote_applied, 0);
        assert_eq!(c.effects.size.cols, 12);
        assert!(c.effects.retained_bytes() >= text.len());
    }
    observed(&mut peer, text.as_bytes()).await;
    until(|| {
        let c = lock(&owner);
        c.effects.size.cols == 8 && c.effects.applied == action.effect
    })
    .await;
    let lease = claim(&owner);
    let mut gui = Gui::default();
    gui.apply(&owner, lease);
    assert_eq!((gui.tabs, gui.zoom), (1, 0));
    assert!(lock(&owner).effects.claim_action().is_none());
    assert_eq!(lock(&owner).effects.frame_ack, 0);
    ack(&owner, lease, 0);
    until(|| lock(&owner).effects.frame_ack == 2).await;
    assert_eq!((&lock(&owner).engine as *const _) as usize, engine);
    assert_eq!(lock(&owner).effects.size.rows, 2);
    assert!(lock(&owner).effects.selection.is_none());
    assert_eq!(
        lock(&owner).effects.frame.as_ref().map(|f| f.size.cols),
        Some(8)
    );
}

#[tokio::test]
async fn valid_binding_chunk_two_and_large_text_paste_are_complete_not_truncated() {
    let (owner, stream, mut peer) = fixture(1);
    lock(&owner).effects.chunk = 2;
    assert_eq!(submit(&owner, key(1, 16, 4)).status, OK);
    assert_eq!(
        submit(
            &owner,
            SpikeKey {
                phase: 2,
                ..key(1, 16, 4)
            }
        )
        .status,
        OK
    );
    let _running = Running::start(&owner, stream);
    observed(&mut peer, b"\x1b[3~").await;
    until(|| lock(&owner).charge() == (0, 0)).await;
    lock(&owner).effects.chunk = 32;
    let complete = Arc::new(core::TestGate {
        entered: std::sync::Barrier::new(2),
        release: std::sync::Barrier::new(2),
    });
    *success(owner.write_complete.lock()) = Some(Arc::clone(&complete));
    send(&mut peer, 13).await;
    complete.entered.wait();
    let remote_seq = lock(&owner).next_seq - 1;
    let paste = submit(
        &owner,
        SpikeKey {
            default_action: 12,
            ..key(2, 1, u32::from('v'))
        },
    );
    let input = "粘贴🦀\n中\r\n文\t\x1b".repeat(20);
    let body = "粘贴🦀\r中\r文\t".repeat(20);
    let (result, remote_applied, flight, retained) = {
        let mut c = lock(&owner);
        let mut request = command(2, paste.ticket);
        request.args[0] = input.len() as u64;
        let result = execute(&mut c, request, &[]);
        (
            result,
            c.remote_applied,
            c.charge().1,
            c.effects.retained_bytes(),
        )
    };
    *success(owner.write_complete.lock()) = None;
    complete.release.wait();
    assert_eq!(paste.status, OK);
    assert_ne!(paste.ticket, TicketRef::default());
    assert_eq!(result.err(), Some(BUSY));
    assert_eq!((remote_applied, flight, retained), (1, 1, 0));
    at_write_boundary(&owner, || {
        assert_eq!(lock(&owner).remote_applied, 1);
        assert_eq!(lock(&owner).last_written_seq, remote_seq);
        finish(&owner, paste.ticket, &input, 1);
    });
    let expected = format!("\x1b[200~{body}\x1b[201~");
    observed(&mut peer, expected.as_bytes()).await;
    until(|| lock(&owner).effects.retained_bytes() == 0).await;
    assert_eq!(
        at_write_boundary(&owner, || {
            submit(
                &owner,
                SpikeKey {
                    phase: 2,
                    ..key(2, 1, u32::from('v'))
                },
            )
            .status
        }),
        OK
    );
    until(|| lock(&owner).charge() == (0, 0)).await;
}

#[test]
fn equal_text_twice_empty_multiscalar_cancel_and_old_callbacks_have_transaction_identity() {
    let (owner, _, _) = fixture(128);
    for (id, text) in [(1, "同文🦀"), (2, "同文🦀"), (3, "")] {
        let t = submit(&owner, native(id));
        finish(&owner, t.ticket, text, 1);
        assert_eq!(
            submit(
                &owner,
                SpikeKey {
                    phase: 2,
                    ..native(id)
                }
            )
            .ticket,
            t.ticket
        );
        assert_eq!(drain(&owner), text.as_bytes());
        let mut duplicate = command(4, t.ticket);
        duplicate.args[0] = 1;
        assert_eq!(success(execute(&mut lock(&owner), duplicate, &[])).kind, 0);
        assert!(execute(&mut lock(&owner), command(5, t.ticket), &[]).is_err());
    }
    let cancelled = submit(&owner, native(4));
    let mut edit = command(1, cancelled.ticket);
    edit.args[0] = 1;
    success(execute(&mut lock(&owner), edit, "未提交".as_bytes()));
    success(execute(
        &mut lock(&owner),
        command(5, cancelled.ticket),
        &[],
    ));
    assert!(drain(&owner).is_empty());
    assert_eq!(
        submit(
            &owner,
            SpikeKey {
                phase: 2,
                ..native(4)
            }
        )
        .ticket,
        cancelled.ticket
    );
    let next = submit(&owner, native(5));
    assert_ne!(next.ticket, cancelled.ticket);
    assert!(execute(&mut lock(&owner), edit, "迟到".as_bytes()).is_err());
    finish(&owner, next.ticket, "新", 1);
    assert_eq!(drain(&owner), "新".as_bytes());
}

#[test]
fn full_normal_slots_preserve_composition_members_release_cancel_and_reject_cycle() {
    let (owner, _, _) = fixture(128);
    let t = submit(&owner, native(1));
    submit(
        &owner,
        SpikeKey {
            phase: 2,
            ..native(1)
        },
    );
    for _ in 1..NORMAL_SLOTS {
        control(&owner, Effect::FrameAck(0));
    }
    let rejected = submit(&owner, key(2, 16, 2));
    assert_eq!((rejected.status, rejected.seq), (BUSY, 0));
    for id in 3..100 {
        assert_eq!(submit(&owner, native(id)).ticket, t.ticket);
        assert_eq!(
            submit(
                &owner,
                SpikeKey {
                    phase: 2,
                    ..native(id)
                }
            )
            .status,
            OK
        );
    }
    success(execute(&mut lock(&owner), command(5, t.ticket), &[]));
    assert!(drain(&owner).is_empty());
    assert_eq!(
        submit(
            &owner,
            SpikeKey {
                phase: 1,
                ..key(2, 16, 2)
            }
        )
        .status,
        INVALID
    );
    assert_eq!(
        submit(
            &owner,
            SpikeKey {
                phase: 2,
                ..key(2, 16, 2)
            }
        )
        .status,
        INVALID
    );
    assert_eq!(lock(&owner).charge(), (0, 0));
    for id in 1..=4 {
        assert_eq!(submit(&owner, key(id, 1, u32::from('a'))).status, OK);
    }
    for id in 1..=4 {
        assert_eq!(
            submit(
                &owner,
                SpikeKey {
                    phase: 1,
                    ..key(id, 1, u32::from('a'))
                }
            )
            .status,
            OK
        );
    }
    for id in 1..=4 {
        assert_eq!(
            submit(
                &owner,
                SpikeKey {
                    phase: 2,
                    ..key(id, 1, u32::from('a'))
                }
            )
            .status,
            OK
        );
    }
    assert_eq!(drain(&owner), b"aaaaaaaa");
    assert_eq!(lock(&owner).charge(), (0, 0));
}

#[test]
fn active_key_capacity_cancels_original_text_with_receipt_and_recovers_after_release() {
    let (owner, _, _) = fixture(128);
    let original = submit(&owner, native(1));
    for id in 2..=4 {
        assert_eq!(submit(&owner, native(id)).ticket, original.ticket);
    }
    let rejected = submit(&owner, native(5));
    assert_eq!(rejected.status, core::ACTIVE_KEY_BUDGET);
    assert!(rejected.seq > original.seq);
    assert_eq!(rejected.effect, original.effect);
    assert!(drain(&owner).is_empty());
    assert_eq!(
        submit(
            &owner,
            SpikeKey {
                phase: 2,
                ..native(5)
            }
        )
        .status,
        INVALID
    );
    for id in 1..=4 {
        assert_eq!(
            submit(
                &owner,
                SpikeKey {
                    phase: 2,
                    ..native(id)
                }
            )
            .status,
            OK
        );
    }
    assert_eq!(lock(&owner).charge(), (0, 0));
    let next = submit(&owner, native(6));
    assert_eq!(next.disposition, 3);
    finish(&owner, next.ticket, "恢复", 1);
    assert_eq!(drain(&owner), "恢复".as_bytes());
}

#[tokio::test]
async fn review_old_cycle_continuation_keeps_original_ticket_and_cannot_edit_new_text() {
    let (owner, stream, mut peer) = fixture(128);
    let first = submit(&owner, native(1));
    finish(&owner, first.ticket, "旧A🦀", 1);
    let running = Running::start(&owner, stream);
    observed(&mut peer, "旧A🦀".as_bytes()).await;
    until(|| lock(&owner).effects.retained_bytes() == 0).await;
    let second = at_write_boundary(&owner, || {
        let second = submit(&owner, native(2));
        assert_eq!(second.status, OK);
        assert_ne!(second.ticket, first.ticket);
        let mut update = command(1, second.ticket);
        update.args[0] = 2;
        success(execute(&mut lock(&owner), update, "新B预编辑🦀".as_bytes()));
        let repeat = submit(
            &owner,
            SpikeKey {
                phase: 1,
                ..native(1)
            },
        );
        let up = submit(
            &owner,
            SpikeKey {
                phase: 2,
                ..native(1)
            },
        );
        assert_eq!(repeat.ticket, first.ticket);
        assert_eq!(up.ticket, first.ticket);
        for old in [repeat.ticket, up.ticket] {
            let mut stale = command(1, old);
            stale.args[0] = 100;
            assert!(execute(&mut lock(&owner), stale, b"late").is_err());
            assert!(execute(&mut lock(&owner), command(5, old), &[]).is_err());
        }
        {
            let c = lock(&owner);
            let index = success(c.effects.payloads.find(second.ticket));
            let text = &c.effects.payloads.tickets[index];
            assert_eq!(text.revision, 2);
            assert_eq!(&text.preedit[..text.preedit_len], "新B预编辑🦀".as_bytes());
            assert_eq!(c.effects.active_text, Some(second.ticket));
        }
        finish(&owner, second.ticket, "新B UTF🦀", 3);
        assert_eq!(
            submit(
                &owner,
                SpikeKey {
                    phase: 2,
                    ..native(2)
                }
            )
            .ticket,
            second.ticket
        );
        second
    });
    observed(&mut peer, "新B UTF🦀".as_bytes()).await;
    until(|| lock(&owner).charge() == (0, 0)).await;
    let mut duplicate = command(4, second.ticket);
    duplicate.args[0] = 3;
    assert_eq!(success(execute(&mut lock(&owner), duplicate, &[])).kind, 0);
    drop(running);
    let mut trailing = Vec::new();
    success(peer.read_to_end(&mut trailing).await);
    assert!(trailing.is_empty());
}

#[test]
fn review_deferred_unsupported_down_closes_independent_scope_but_not_new_native_cycles() {
    let (owner, _, _) = fixture(128);
    let super_key = SpikeKey {
        modifiers: 8,
        ..key(1, 1, u32::from('a'))
    };
    assert_eq!(submit(&owner, super_key).status, OK);
    assert!(drain(&owner).is_empty());
    success(lock(&owner).remote(MODES[3]));
    assert!(drain(&owner).is_empty());
    for phase in [1, 2] {
        assert_eq!(submit(&owner, SpikeKey { phase, ..super_key }).status, OK);
    }
    assert!(drain(&owner).is_empty());
    assert_eq!(lock(&owner).charge(), (0, 0));
    let mut independent = command(6, TicketRef::default());
    independent.args = [owner.binding.context as u64, 1, 0, 0, 0, 0];
    assert_eq!(
        execute(&mut lock(&owner), independent, &[]).err(),
        Some(INVALID)
    );
    let next = SpikeKey {
        physical: 2,
        ..super_key
    };
    assert_eq!(submit(&owner, next).status, OK);
    assert_eq!(drain(&owner), b"\x1b[97;9u");
    assert_eq!(submit(&owner, SpikeKey { phase: 2, ..next }).status, OK);
    assert_eq!(drain(&owner), b"\x1b[97;9:3u");
    let text = submit(&owner, native(3));
    assert_eq!((text.status, text.disposition), (OK, 3));
    finish(&owner, text.ticket, "合法🦀", 1);
    assert_eq!(
        submit(
            &owner,
            SpikeKey {
                phase: 2,
                ..native(3)
            }
        )
        .ticket,
        text.ticket
    );
    assert_eq!(drain(&owner), "合法🦀".as_bytes());
    assert_eq!(lock(&owner).charge(), (0, 0));
}

#[test]
fn deferred_unsupported_down_rejects_its_repeat_and_up_after_mode_changes() {
    let (owner, _, _) = fixture(128);
    let t = submit(&owner, native(1));
    submit(
        &owner,
        SpikeKey {
            phase: 2,
            ..native(1)
        },
    );
    let super_key = SpikeKey {
        modifiers: 8,
        ..key(2, 1, u32::from('a'))
    };
    assert_eq!(submit(&owner, super_key).status, OK);
    success(lock(&owner).remote(MODES[3]));
    assert_eq!(
        submit(
            &owner,
            SpikeKey {
                phase: 1,
                ..super_key
            }
        )
        .status,
        OK
    );
    assert_eq!(
        submit(
            &owner,
            SpikeKey {
                phase: 2,
                ..super_key
            }
        )
        .status,
        OK
    );
    finish(&owner, t.ticket, "", 1);
    assert!(drain(&owner).is_empty());
    let next = SpikeKey {
        physical: 3,
        ..super_key
    };
    assert_eq!(submit(&owner, next).status, OK);
    assert_eq!(drain(&owner), b"\x1b[97;9u");
}

#[test]
fn unknown_raw_cannot_begin_independent_text_and_binding_still_precedes_native() {
    let (owner, _, _) = fixture(128);
    let bad = SpikeKey {
        raw_source: 0,
        raw_kind: 0,
        raw_value: 0,
        default_action: 14,
        ..native(1)
    };
    assert_eq!(submit(&owner, bad).disposition, 0);
    let mut independent = command(6, TicketRef::default());
    independent.args[0] = 1;
    assert!(execute(&mut lock(&owner), independent, &[]).is_err());
    submit(&owner, SpikeKey { phase: 2, ..bad });
    assert!(execute(&mut lock(&owner), independent, &[]).is_err());
    let binding = submit(
        &owner,
        SpikeKey {
            text_ownership: 2,
            default_action: 14,
            ..key(2, 16, 4)
        },
    );
    assert_eq!(binding.disposition, 1);
    assert_eq!(binding.ticket, TicketRef::default());
    assert_eq!(drain(&owner), b"\x1b[3~");
}

#[tokio::test]
async fn pending_write_keeps_charge_and_independent_read_admits_but_does_not_advance() {
    let (owner, stream, mut peer) = fixture(1);
    assert_eq!(submit(&owner, key(1, 16, 4)).status, OK);
    let _running = Running::start(&owner, stream);
    until(|| lock(&owner).charge().1 == 1).await;
    send(&mut peer, 2).await;
    send(&mut peer, 0).await;
    until(|| lock(&owner).effects.remote_received == 2).await;
    {
        let c = lock(&owner);
        assert_eq!(c.remote_applied, 0);
        assert_eq!(c.charge().1, 1);
        assert!(c.effects.retained_bytes() >= 4);
    }
    observed(&mut peer, b"\x1b[3~\x1b[?2u").await;
    until(|| lock(&owner).remote_applied == 2).await;
    submit(
        &owner,
        SpikeKey {
            phase: 2,
            ..key(1, 16, 4)
        },
    );
    until(|| lock(&owner).charge() == (0, 0)).await;
}

#[tokio::test]
async fn partial_write_disconnect_cancels_without_replay_and_releases_payloads() {
    let (owner, stream, mut peer) = fixture(1);
    let t = submit(&owner, native(1));
    finish(&owner, t.ticket, &"长🦀".repeat(50), 1);
    let running = Running::start(&owner, stream);
    observed(&mut peer, &"长".as_bytes()[..1]).await;
    assert_eq!(lock(&owner).charge().1, 1);
    drop(peer);
    until(|| owner.worker_done.load(Ordering::Acquire)).await;
    assert_eq!(submit(&owner, key(2, 16, 4)).status, STOPPED);
    assert!(drain(&owner).is_empty());
    drop(running);
}

#[tokio::test]
async fn pending_revoke_terminates_blocked_writer_without_waiting_for_peer_or_replaying() {
    let (owner, stream, mut peer) = fixture(1);
    let t = submit(&owner, native(1));
    finish(&owner, t.ticket, &"原source🦀".repeat(20), 1);
    let pending = Arc::new(core::TestGate {
        entered: std::sync::Barrier::new(2),
        release: std::sync::Barrier::new(2),
    });
    *success(owner.write_pending.lock()) = Some(Arc::clone(&pending));
    let running = Running::start(&owner, stream);
    pending.entered.wait();
    let charged = {
        let c = lock(&owner);
        (c.charge().1, c.effects.retained_bytes())
    };
    owner.pending_revoke.store(true, Ordering::Release);
    *success(owner.write_pending.lock()) = None;
    pending.release.wait();
    assert_eq!(charged.0, 1);
    assert_ne!(charged.1, 0);
    until(|| owner.worker_done.load(Ordering::Acquire)).await;
    assert!(lock(&owner).effects.revoked);
    assert_eq!(lock(&owner).effects.retained_bytes(), 0);
    let mut delivered = Vec::new();
    success(peer.read_to_end(&mut delivered).await);
    assert_eq!(delivered, &"原".as_bytes()[..1]);
    drop(running);
    let (owner, stream, _peer) = fixture(128);
    assert_eq!(submit(&owner, native(1)).disposition, 3);
    let running = Running::start(&owner, stream);
    owner.pending_revoke.store(true, Ordering::Release);
    until(|| owner.worker_done.load(Ordering::Acquire)).await;
    assert_eq!(lock(&owner).charge(), (0, 0));
    drop(running);
}

#[test]
fn busy_down_latches_busy_up_mailbox_serves_and_ledger_exhaustion_stops() {
    let (owner, _, _) = fixture(128);
    let guard = lock(&owner);
    assert_eq!(submit(&owner, key(1, 16, 4)).status, BUSY);
    drop(guard);
    assert_eq!(
        submit(
            &owner,
            SpikeKey {
                phase: 1,
                ..key(1, 16, 4)
            }
        )
        .status,
        INVALID
    );
    assert_eq!(
        submit(
            &owner,
            SpikeKey {
                phase: 2,
                ..key(1, 16, 4)
            }
        )
        .status,
        INVALID
    );
    assert_eq!(submit(&owner, key(1, 16, 4)).status, OK);
    let guard = lock(&owner);
    assert_eq!(
        submit(
            &owner,
            SpikeKey {
                phase: 2,
                ..key(1, 16, 4)
            }
        )
        .status,
        BUSY
    );
    drop(guard);
    assert_eq!(
        submit(&owner, key(2, 16, 4)).status,
        BUSY,
        "pending Up 暂停新 Down"
    );
    success(owner.release_pending());
    assert_eq!(drain(&owner), b"\x1b[3~");
    assert_eq!(
        submit(
            &owner,
            SpikeKey {
                phase: 2,
                ..key(2, 16, 4)
            }
        )
        .status,
        INVALID
    );
    let guard = lock(&owner);
    for id in 1..=8 {
        assert_eq!(submit(&owner, key(id, 16, 4)).status, BUSY);
    }
    assert_eq!(submit(&owner, key(9, 16, 4)).status, STOPPED);
    drop(guard);
    owner.retire();
    assert_eq!(lock(&owner).charge(), (0, 0));
}

fn armed(owner: &Owner) -> (u64, Instant) {
    match lock(owner).effects.sync.cell {
        Cell::Armed { wake, witness } => (wake, witness.deadline),
        _ => panic!("未 arm"),
    }
}
fn wake(owner: &Owner, wake: u64) {
    let mut c = lock(owner);
    let Core {
        effects, next_seq, ..
    } = &mut *c;
    success(effects.timer_wake(next_seq, wake, Instant::now()));
}
fn frame_text(owner: &Owner) -> String {
    lock(owner)
        .effects
        .frame
        .as_ref()
        .map(|f| {
            f.rows
                .iter()
                .flat_map(|r| r.cells.iter())
                .map(|c| c.text.as_str())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn deadline_waiting_capacity_is_single_dearmed_and_advances_before_new_ordinary() {
    let (owner, _, _) = fixture(128);
    success(lock(&owner).remote(MODES[9]));
    drain(&owner);
    let (w, d) = armed(&owner);
    let t = submit(&owner, native(1));
    submit(
        &owner,
        SpikeKey {
            phase: 2,
            ..native(1)
        },
    );
    for _ in 1..NORMAL_SLOTS {
        control(&owner, Effect::FrameAck(0));
    }
    tokio::time::sleep_until(tokio::time::Instant::from_std(d)).await;
    wake(&owner, w);
    assert!(matches!(
        lock(&owner).effects.sync.cell,
        Cell::Waiting { .. }
    ));
    for _ in 0..20 {
        wake(&owner, w);
    }
    assert_eq!(lock(&owner).effects.sync.wakes, 1);
    assert!(lock(&owner).engine.sync_deadline().is_some());
    assert!(!frame_text(&owner).contains('A'));
    success(execute(&mut lock(&owner), command(5, t.ticket), &[]));
    {
        let mut c = lock(&owner);
        let (slot, p) = match success(c.take_write()) {
            Some(p) => p,
            None => panic!("取消槽未完成"),
        };
        success(c.finish_write(slot, p.seq));
        assert!(matches!(c.effects.sync.cell, Cell::Queued { .. }));
    }
    let mut c = lock(&owner);
    let Core {
        effects, next_seq, ..
    } = &mut *c;
    assert!(matches!(
        effects.control(next_seq, Effect::FrameAck(0)),
        Err(BUSY)
    ));
    drop(c);
    drain(&owner);
    assert_eq!(lock(&owner).effects.sync.applied, 1);
    assert!(lock(&owner).engine.sync_deadline().is_none());
    assert!(frame_text(&owner).contains('A'));
}

#[tokio::test]
async fn old_deadline_a_cannot_end_new_b_and_gui_ack_holds_expiry() {
    let (owner, _, _) = fixture(128);
    success(lock(&owner).remote(MODES[9]));
    drain(&owner);
    let (w, d) = armed(&owner);
    let t = submit(&owner, native(1));
    submit(
        &owner,
        SpikeKey {
            phase: 2,
            ..native(1)
        },
    );
    success(lock(&owner).remote(MODES[10]));
    tokio::time::sleep_until(tokio::time::Instant::from_std(d)).await;
    wake(&owner, w);
    finish(&owner, t.ticket, "", 1);
    drain(&owner);
    assert_eq!(lock(&owner).effects.sync.stale, 1);
    assert_eq!(lock(&owner).effects.sync.applied, 0);
    assert!(lock(&owner).engine.sync_deadline().is_some());
    let (wb, db) = armed(&owner);
    assert!(db > d);
    control(
        &owner,
        Effect::Action(crate::signals::desktop_session::DesktopAction::ZoomIn),
    );
    drain(&owner);
    let lease = claim(&owner);
    tokio::time::sleep_until(tokio::time::Instant::from_std(db)).await;
    wake(&owner, wb);
    assert!(drain(&owner).is_empty());
    assert!(lock(&owner).engine.sync_deadline().is_some());
    let mut gui = Gui::default();
    gui.apply(&owner, lease);
    assert_eq!(gui.zoom, 1);
    ack(&owner, lease, 0);
    drain(&owner);
    assert!(lock(&owner).engine.sync_deadline().is_none());
    assert!(frame_text(&owner).contains("AB"));
}

#[tokio::test]
async fn ordinary_advance_keeps_absolute_deadline_then_really_flushes_expired_frame() {
    let (owner, _, _) = fixture(128);
    success(lock(&owner).remote(MODES[9]));
    drain(&owner);
    let (_, d) = armed(&owner);
    success(lock(&owner).remote(MODES[12]));
    drain(&owner);
    assert_eq!(armed(&owner).1, d);
    tokio::time::sleep_until(tokio::time::Instant::from_std(d)).await;
    success(lock(&owner).remote(MODES[12]));
    drain(&owner);
    assert!(lock(&owner).engine.sync_deadline().is_none());
    assert!(frame_text(&owner).contains("Axx"));
}

#[test]
fn witness_epoch_rejects_same_timestamp_and_cancel_rearms_original_absolute_deadline() {
    let (owner, _, _) = fixture(128);
    success(lock(&owner).remote(MODES[9]));
    drain(&owner);
    let old = match lock(&owner).effects.sync.cell {
        Cell::Armed { witness, .. } => witness,
        _ => panic!("未arm"),
    };
    success(lock(&owner).effects.sync.mutate());
    assert!(
        !lock(&owner)
            .effects
            .sync
            .matches(old, Some(old.deadline), old.deadline)
    );
    success(execute(
        &mut lock(&owner),
        command(15, TicketRef::default()),
        &[],
    ));
    assert_eq!(armed(&owner).1, old.deadline);
    success(execute(
        &mut lock(&owner),
        command(11, TicketRef::default()),
        &[],
    ));
    assert_eq!(lock(&owner).effects.sync.cell, Cell::Idle);
}

#[test]
fn selection_copy_and_editable_action_revoke_ack_are_ordered_and_original_target() {
    let (owner, _, _) = fixture(128);
    success(lock(&owner).remote(MODES[15]));
    drain(&owner);
    let t = submit(&owner, native(1));
    submit(
        &owner,
        SpikeKey {
            phase: 2,
            ..native(1)
        },
    );
    control(
        &owner,
        Effect::Selection(Some(SelectionRange {
            start: CellPosition {
                stable_row: 0,
                column: 0,
            },
            end: CellPosition {
                stable_row: 0,
                column: 5,
            },
            rectangular: false,
        })),
    );
    control(&owner, Effect::Copy);
    assert!(drain(&owner).is_empty());
    assert!(lock(&owner).effects.selection.is_none());
    finish(&owner, t.ticket, "", 1);
    drain(&owner);
    let lease = claim(&owner);
    assert_eq!(lease.action, 11);
    assert_eq!(lock(&owner).effects.copied, "hello");
    let mut enter = command(10, TicketRef::default());
    enter.args = [
        lease.owner,
        lease.context,
        lease.effect,
        u64::from(lease.action),
        u64::from(lease.target),
        0,
    ];
    success(execute(&mut lock(&owner), enter, &[]));
    assert!(lock(&owner).effects.revoked);
    assert!(!lock(&owner).effects.retired);
    let mut retired = command(12, TicketRef::default());
    retired.args[0] = 1;
    success(execute(&mut lock(&owner), retired, &[]));
    assert!(lock(&owner).effects.retired);
    assert!(execute(&mut lock(&owner), command(2, t.ticket), &[]).is_err());
    assert_eq!(lock(&owner).effects.retained_bytes(), 0);
}

#[test]
fn actual_records_and_send_constraints_are_explicit() {
    fn send<T: Send>() {}
    send::<Core>();
    send::<Owner>();
    assert_eq!(size_of::<TicketRef>(), 40);
    assert_eq!(size_of::<PayloadGrant>(), 16);
    assert_eq!(size_of::<ActionLease>(), 32);
    assert_eq!(size_of::<ffi::SpikeAdmission>(), 80);
    assert_eq!(size_of::<Command>(), 136);
    assert_eq!(size_of::<Reply>(), 120);
    assert_eq!(size_of::<SpikeOpen>(), 56);
    assert_eq!(size_of::<SpikeKey>(), 96);
    assert_eq!(size_of::<ffi::SpikeSnapshot>(), 176);
    assert_eq!(size_of::<super::projection::Projection>(), 928);
    assert_eq!(offset_of!(Command, bytes), 120);
    assert_eq!(ffi::registration(9), Err(core::DISABLED));
    assert_eq!(ffi::registration(u64::MAX), Err(core::EXHAUSTED));
    println!(
        "ABI3 owner={} open={} key={} admission={} command={} reply={} ticket={} grant={} action={}",
        size_of::<SpikeOwner>(),
        size_of::<SpikeOpen>(),
        size_of::<SpikeKey>(),
        size_of::<ffi::SpikeAdmission>(),
        size_of::<Command>(),
        size_of::<Reply>(),
        size_of::<TicketRef>(),
        size_of::<PayloadGrant>(),
        size_of::<ActionLease>()
    );
}

#[test]
fn grant_boundaries_utf8_offsets_cancel_mailbox_and_revoke_preserve_source_charge() {
    let (owner, _, _) = fixture(128);
    let t = submit(&owner, native(1));
    let mut request = command(2, t.ticket);
    request.args[0] = 16;
    let grant = success(execute(&mut lock(&owner), request, &[])).grant;
    let mut chunk = command(3, t.ticket);
    chunk.grant = grant;
    assert!(execute(&mut lock(&owner), chunk, &[0xf0, 0x9f]).is_err());
    chunk.args[0] = 1;
    assert!(execute(&mut lock(&owner), chunk, b"x").is_err());
    let guard = lock(&owner);
    assert_eq!(owner.defer_cancel(t.ticket), BUSY);
    assert_eq!(owner.defer_cancel(t.ticket), BUSY);
    drop(guard);
    success(owner.release_pending());
    assert!(drain(&owner).is_empty());
    assert_eq!(lock(&owner).effects.retained_bytes(), 0);
    submit(
        &owner,
        SpikeKey {
            phase: 2,
            ..native(1)
        },
    );
    let next = submit(&owner, native(2));
    request.ticket = next.ticket;
    request.args[0] = 4097;
    assert_eq!(
        success(execute(&mut lock(&owner), request, &[])).status,
        core::TEXT_CAPACITY
    );
    assert!(drain(&owner).is_empty());
    submit(
        &owner,
        SpikeKey {
            phase: 2,
            ..native(2)
        },
    );
    let t = submit(&owner, native(3));
    finish(&owner, t.ticket, &"字🦀".repeat(20), 1);
    let mut c = lock(&owner);
    let (slot, packet) = match success(c.take_write()) {
        Some(packet) => packet,
        None => panic!("未开始写"),
    };
    let charged = c.effects.retained_bytes();
    c.effects.revoke();
    assert_eq!(c.effects.retained_bytes(), charged);
    assert_eq!(c.charge().1, 1);
    success(c.finish_write(slot, packet.seq));
    assert_eq!(c.effects.retained_bytes(), 0);
    assert!(success(c.take_write()).is_none());
}

#[test]
fn clipboard_payload_waits_for_original_head_and_independent_edit_scope_cannot_replay() {
    let (owner, _, _) = fixture(128);
    let text = submit(&owner, native(1));
    submit(
        &owner,
        SpikeKey {
            phase: 2,
            ..native(1)
        },
    );
    let paste = control(
        &owner,
        Effect::Action(crate::signals::desktop_session::DesktopAction::Paste),
    );
    let mut grant = command(2, paste.ticket);
    grant.args[0] = 128;
    assert_eq!(execute(&mut lock(&owner), grant, &[]).err(), Some(BUSY));
    assert_eq!(lock(&owner).effects.payloads.retained, 0);
    finish(&owner, text.ticket, "", 1);
    drain(&owner);
    success(execute(&mut lock(&owner), grant, &[]));
    success(execute(&mut lock(&owner), command(5, paste.ticket), &[]));
    drain(&owner);
    let mut independent = command(6, TicketRef::default());
    independent.args = [1, 1, text.effect, 0, 0, 0];
    let ticket = success(execute(&mut lock(&owner), independent, &[])).ticket;
    success(execute(&mut lock(&owner), command(5, ticket), &[]));
    drain(&owner);
    independent.args[2] = ticket.effect;
    assert!(execute(&mut lock(&owner), independent, &[]).is_err());
    independent.args[1] = 2;
    assert!(execute(&mut lock(&owner), independent, &[]).is_ok());
    owner.retire();
    assert_eq!(lock(&owner).effects.retained_bytes(), 0);
}

#[test]
fn pointer_uses_real_engine_mode_and_source_frame_while_shift_stays_local() {
    let (owner, _, _) = fixture(128);
    success(lock(&owner).remote(MODES[16]));
    drain(&owner);
    let mut pointer = command(17, TicketRef::default());
    pointer.args = [lock(&owner).effects.frame_revision, 2, 1, 0, 0, 0];
    success(execute(&mut lock(&owner), pointer, &[]));
    assert_eq!(drain(&owner), b"\x1b[<0;3;2M");
    pointer.args[4] = 2;
    success(execute(&mut lock(&owner), pointer, &[]));
    assert_eq!(drain(&owner), b"\x1b[<0;3;2m");
    pointer.args[4] = 0;
    pointer.args[5] = 1;
    success(execute(&mut lock(&owner), pointer, &[]));
    assert!(drain(&owner).is_empty());
    assert!(lock(&owner).effects.selection.is_some());
    assert_eq!(
        execute(&mut lock(&owner), pointer, &[]).err(),
        Some(INVALID)
    );
    let budget = lock(&owner).scope_budget;
    assert_eq!(budget.root_delta, budget.per_owner_delta * 8);
    assert_eq!(budget.existing_vte_sync, 2 * 1024 * 1024);
    assert!(budget.encoder_workspace >= 4096 + 12);
    assert_eq!(budget.chunk_window, 32);
    println!(
        "B scope delta：per-owner={}，eight-lifetime={}，existing-vte-sync={}",
        budget.per_owner_delta, budget.root_delta, budget.existing_vte_sync
    );
}

#[test]
fn shutdown_keeps_claimed_action_fence_until_ack_and_native_retirement() {
    let (owner, _, _) = fixture(128);
    control(
        &owner,
        Effect::Action(crate::signals::desktop_session::DesktopAction::Search),
    );
    drain(&owner);
    let lease = claim(&owner);
    owner.retire();
    assert_eq!(lock(&owner).charge().0, 1);
    assert_eq!(lock(&owner).effects.confirm_retired(1), Err(BUSY));
    ack(&owner, lease, 2);
    success(lock(&owner).effects.confirm_retired(1));
    assert_eq!(lock(&owner).charge(), (0, 0));
}

#[test]
fn physical_up_keeps_its_cycle_and_release_slot_until_actual_write_completion() {
    let (owner, _, _) = fixture(128);
    success(lock(&owner).remote(MODES[3]));
    drain(&owner);
    let input = SpikeKey {
        modifiers: 2,
        ..key(1, 1, u32::from('a'))
    };
    assert_eq!(submit(&owner, input).status, OK);
    assert_eq!(submit(&owner, SpikeKey { phase: 2, ..input }).status, OK);
    let (slot, packet) = {
        let mut c = lock(&owner);
        let (slot, p) = match success(c.take_write()) {
            Some(p) => p,
            None => panic!("无Down"),
        };
        success(c.finish_write(slot, p.seq));
        match success(c.take_write()) {
            Some(p) => p,
            None => panic!("无Up"),
        }
    };
    assert_eq!(&packet.bytes[..packet.len], b"\x1b[97;5:3u");
    assert_eq!(submit(&owner, key(2, 1, u32::from('b'))).status, OK);
    success(lock(&owner).finish_write(slot, packet.seq));
    assert_eq!(drain(&owner), b"b");
}

#[test]
fn key_paused_after_entry_cannot_resurrect_a_finished_owner() {
    let (owner, stream, _) = fixture(128);
    let gate = Arc::new(core::TestGate {
        entered: std::sync::Barrier::new(2),
        release: std::sync::Barrier::new(2),
    });
    *success(owner.key_entry.lock()) = Some(Arc::clone(&gate));
    success(transport::start(Arc::clone(&owner), stream));
    let management = Arc::clone(&owner);
    let management_gate = Arc::clone(&gate);
    let handle = std::thread::spawn(move || {
        management_gate.entered.wait();
        management.stop();
        let result = management.join_worker();
        management_gate.release.wait();
        result
    });
    assert_eq!(submit(&owner, key(1, 16, 2)).status, STOPPED);
    assert_eq!(success(handle.join()), OK);
    assert_eq!(lock(&owner).next_seq, 1);
    assert_eq!(lock(&owner).charge(), (0, 0));
}

#[tokio::test]
async fn actual_timer_worker_dearms_during_gui_ack_and_completes_after_ack() {
    let (owner, stream, mut peer) = fixture(128);
    let _running = Running::start(&owner, stream);
    send(&mut peer, 9).await;
    until(|| lock(&owner).engine.sync_deadline().is_some()).await;
    control(
        &owner,
        Effect::Action(crate::signals::desktop_session::DesktopAction::ZoomIn),
    );
    until(|| lock(&owner).effects.applied >= 2).await;
    let lease = claim(&owner);
    until(|| lock(&owner).effects.sync.wakes == 1).await;
    let cell = lock(&owner).effects.sync.cell;
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(lock(&owner).effects.sync.cell, cell);
    assert_eq!(lock(&owner).effects.sync.wakes, 1);
    assert!(lock(&owner).engine.sync_deadline().is_some());
    ack(&owner, lease, 0);
    until(|| lock(&owner).engine.sync_deadline().is_none()).await;
    assert!(frame_text(&owner).contains('A'));
}

#[test]
fn exported_factory_origin_source_version_revoke_and_actual_non_ui_join() {
    fn wait(mut predicate: impl FnMut() -> bool) {
        let end = Instant::now() + Duration::from_secs(3);
        while !predicate() {
            assert!(Instant::now() < end, "ABI fixture 超时");
            std::thread::yield_now();
        }
    }
    fn call(cap: &SpikeOwner, mut request: Command, bytes: &[u8]) -> Reply {
        request.bytes = bytes.as_ptr();
        request.length = bytes.len();
        let mut reply = Reply::default();
        wait(|| {
            let status = unsafe {
                super::commands::guossh_native_input_spike_command(cap, &request, &mut reply)
            };
            if status == BUSY {
                return false;
            }
            assert_eq!(status, OK);
            true
        });
        reply
    }
    static CONTEXT: u8 = 1;
    let mut cap = SpikeOwner {
        handle: 0,
        native_context: std::ptr::null(),
        session: 0,
        reserved: 0,
        generation: 0,
        identity: [0; 16],
    };
    let mut request = SpikeOpen {
        version: VERSION,
        size: size_of::<SpikeOpen>() as u32,
        explicit_opt_in: 0,
        reserved: 0,
        native_context: (&CONTEXT as *const u8).cast::<c_void>(),
        fixture_name: ffi::FIXTURE_NAME.as_ptr(),
        fixture_name_len: ffi::FIXTURE_NAME.len(),
        origin: 1,
        platform: 0,
        registration: 1,
    };
    unsafe {
        assert_eq!(
            ffi::guossh_native_input_spike_open(&request, &mut cap),
            core::DISABLED
        );
        request.explicit_opt_in = 1;
        assert_eq!(ffi::guossh_native_input_spike_open(&request, &mut cap), OK);
    }
    let owner = success(ffi::lookup(cap));
    assert_eq!(lock(owner).origin, core::Origin::DeclaredSynthetic(1));
    let forged = SpikeOwner {
        generation: cap.generation + 1,
        ..cap
    };
    assert!(matches!(ffi::lookup(forged), Err(core::INVALID_SOURCE)));
    let mut begin = command(6, TicketRef::default());
    begin.args = [1, 1, 0, 0, 0, 0];
    begin.args[0] = cap.native_context as usize as u64;
    let t = call(&cap, begin, &[]).ticket;
    const TEXT: &str = "真实私有入口多标量🦀源票据，不经过当前pane；完整写入。";
    let mut grant = command(2, t);
    grant.args[0] = TEXT.len() as u64;
    let grant = call(&cap, grant, &[]).grant;
    let mut offset = 0;
    while offset < TEXT.len() {
        let mut end = (offset + 32).min(TEXT.len());
        while !TEXT.is_char_boundary(end) {
            end -= 1;
        }
        let mut chunk = command(3, t);
        chunk.grant = grant;
        chunk.args[0] = offset as u64;
        chunk.flags = u32::from(end == TEXT.len());
        call(&cap, chunk, &TEXT.as_bytes()[offset..end]);
        offset = end;
    }
    let mut commit = command(4, t);
    commit.args[0] = 1;
    call(&cap, commit, &[]);
    let mut received = Vec::new();
    wait(|| {
        let mut bytes = [0; 128];
        let mut n = 0;
        let status = unsafe {
            ffi::guossh_native_input_spike_peer_read(&cap, bytes.as_mut_ptr(), 128, &mut n)
        };
        assert!(matches!(status, OK | BUSY));
        received.extend_from_slice(&bytes[..n]);
        received.len() == TEXT.len()
    });
    assert_eq!(received, TEXT.as_bytes());
    wait(|| unsafe { ffi::guossh_native_input_spike_peer_mode(&cap, 15) } == OK);
    wait(|| lock(owner).effects.frame.is_some());
    let mut projection = std::mem::MaybeUninit::<super::projection::Projection>::uninit();
    wait(||unsafe{super::projection::guossh_native_input_spike_projection(&cap,projection.as_mut_ptr())}==OK);
    let projection = unsafe { projection.assume_init() };
    assert_eq!(projection.owner, cap.handle);
    assert_eq!(projection.cells[0].scalar, u32::from('h'));
    let mut select = command(7, TicketRef::default());
    select.args = [3, 0, 0, 0, 5, 0];
    call(&cap, select, &[]);
    let mut copy = command(7, TicketRef::default());
    copy.args[0] = 4;
    call(&cap, copy, &[]);
    let lease = call(&cap, command(8, TicketRef::default()), &[]).action;
    let mut copied = [0; 128];
    let mut count = 0;
    wait(||unsafe{super::projection::guossh_native_input_spike_copy(&cap,&lease,copied.as_mut_ptr(),128,&mut count)}==OK);
    assert_eq!(&copied[..count], b"hello");
    let mut complete = command(9, TicketRef::default());
    complete.args = [
        lease.owner,
        lease.context,
        lease.effect,
        u64::from(lease.action),
        u64::from(lease.target),
        0,
    ];
    call(&cap, complete, &[]);
    let mut late_copy = command(7, TicketRef::default());
    late_copy.args[0] = 4;
    call(&cap, late_copy, &[]);
    let revoked_lease = call(&cap, command(8, TicketRef::default()), &[]).action;
    static CONTEXTS: [u8; 8] = [2, 3, 4, 5, 6, 7, 8, 9];
    let second_request = SpikeOpen {
        native_context: (&CONTEXTS[0] as *const u8).cast(),
        registration: 2,
        ..request
    };
    let mut second = cap;
    assert_eq!(
        unsafe { ffi::guossh_native_input_spike_open(&second_request, &mut second) },
        OK
    );
    let second_owner = success(ffi::lookup(second));
    let tail = Arc::new(core::TestGate {
        entered: std::sync::Barrier::new(2),
        release: std::sync::Barrier::new(2),
    });
    *success(owner.thread_tail.lock()) = Some(Arc::clone(&tail));
    let mut reply = Reply::default();
    let c = command(13, TicketRef::default());
    unsafe {
        assert_eq!(
            super::commands::guossh_native_input_spike_command(&cap, &c, &mut reply),
            OK
        );
        assert_eq!(ffi::guossh_native_input_spike_revoke(&cap), OK);
        assert_eq!(
            ffi::guossh_native_input_spike_join(&cap),
            core::INVALID_SOURCE
        );
    }
    tail.entered.wait();
    let held_copy_charge = lock(owner).charge();
    let claimed_after_retire = lock(owner).effects.claimed_copy(revoked_lease);
    let mut revoked_output = [0x5a; 128];
    let mut revoked_count = 128;
    let revoked_copy_status = unsafe {
        super::projection::guossh_native_input_spike_copy(
            &cap,
            &revoked_lease,
            revoked_output.as_mut_ptr(),
            128,
            &mut revoked_count,
        )
    };
    let mut cancelled = command(9, TicketRef::default());
    cancelled.args = [
        revoked_lease.owner,
        revoked_lease.context,
        revoked_lease.effect,
        u64::from(revoked_lease.action),
        u64::from(revoked_lease.target),
        2,
    ];
    call(&cap, cancelled, &[]);
    let released_copy_charge = lock(owner).charge();
    let mut retirement = command(12, TicketRef::default());
    retirement.args[0] = revoked_lease.context;
    call(&cap, retirement, &[]);
    let retired_after_ack = lock(owner).effects.retired;
    let third_request = SpikeOpen {
        native_context: (&CONTEXTS[1] as *const u8).cast(),
        registration: 3,
        ..request
    };
    let mut third = cap;
    let premature = unsafe { ffi::guossh_native_input_spike_open(&third_request, &mut third) };
    assert!(owner.worker_done.load(Ordering::Acquire));
    let not_joined = !owner.worker_joined.load(Ordering::Acquire);
    tail.release.wait();
    let joined = std::thread::spawn(move || owner.join_worker());
    assert_eq!(success(joined.join()), OK);
    assert!(owner.worker_joined.load(Ordering::Acquire));
    assert_eq!(lock(owner).charge(), (0, 0));
    assert_eq!(premature, BUSY);
    assert!(not_joined);
    second_owner.stop();
    assert_eq!(
        success(std::thread::spawn(move || second_owner.join_worker()).join()),
        OK
    );
    for (i, context) in CONTEXTS.iter().take(7).enumerate().skip(1) {
        let request = SpikeOpen {
            native_context: (context as *const u8).cast(),
            registration: (i + 2) as u64,
            ..request
        };
        let mut next = cap;
        assert_eq!(
            unsafe { ffi::guossh_native_input_spike_open(&request, &mut next) },
            OK
        );
        assert_eq!(next.handle, (i + 2) as u64);
        assert_ne!(next.identity, cap.identity);
        let actual = success(ffi::lookup(next));
        actual.stop();
        assert_eq!(
            success(std::thread::spawn(move || actual.join_worker()).join()),
            OK
        );
    }
    let exhausted = SpikeOpen {
        native_context: (&CONTEXTS[7] as *const u8).cast(),
        ..request
    };
    assert_eq!(
        unsafe { ffi::guossh_native_input_spike_open(&exhausted, &mut cap) },
        core::DISABLED
    );
    assert_eq!(revoked_copy_status, STOPPED);
    assert_eq!(revoked_count, 128);
    assert_eq!(revoked_output, [0x5a; 128]);
    assert_eq!(held_copy_charge, (1, 0));
    assert!(claimed_after_retire);
    assert_eq!(released_copy_charge, (0, 0));
    assert!(retired_after_ack);
    assert_eq!(submit(owner, native(99)).status, STOPPED);
}

#[test]
fn worker_done_is_not_joined_and_poisoned_core_cleanup_does_not_resume_service() {
    let (owner, stream, _) = fixture(128);
    let gate = Arc::new(core::TestGate {
        entered: std::sync::Barrier::new(2),
        release: std::sync::Barrier::new(2),
    });
    *success(owner.thread_tail.lock()) = Some(Arc::clone(&gate));
    success(transport::start(Arc::clone(&owner), stream));
    owner.stop();
    gate.entered.wait();
    assert!(owner.worker_done.load(Ordering::Acquire));
    assert!(!owner.worker_joined.load(Ordering::Acquire));
    gate.release.wait();
    let management = Arc::clone(&owner);
    assert_eq!(
        success(std::thread::spawn(move || management.join_worker()).join()),
        OK
    );
    let (other, _, _) = fixture(128);
    let poisoned = Arc::clone(&other);
    assert!(
        std::thread::spawn(move || {
            let _guard = success(poisoned.core.lock());
            panic!("有界 poison 注入");
        })
        .join()
        .is_err()
    );
    other.retire();
    assert!(other.core.is_poisoned());
    assert!(other.worker_done.load(Ordering::Acquire));
    assert_eq!(submit(&other, key(1, 16, 4)).status, STOPPED);
    let charge = other
        .core
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .charge();
    assert_eq!(charge, (0, 0));
}
