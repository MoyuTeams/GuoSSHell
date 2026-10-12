//! 同一个上游 trait 的故障注入；不替代 Native/PTY/System 的真实宿主验收。
#![allow(clippy::expect_used)]

use super::{Screen, SessionCommand, run_connected};
#[path = "session_shutdown_tests.rs"]
mod shutdown_tests;
use crate::connect::{Abort, Channels, PromptIds, handshake};
use crate::transport_lifecycle::{self, CLOSING};
use async_trait::async_trait;
use rshell_m0::rshell_core::{
    ConnectionProfile, TerminalInput, TerminalOverrides, TerminalSettingsV1, TerminalSize,
};
use rshell_m0::rshell_session::{
    DefaultTerminalEngine, InteractionBroker, SessionTransport, TransportCapabilities,
    TransportError, TransportEvent, TransportRequest,
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::{Notify, mpsc::unbounded_channel, watch};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stall {
    Connect,
    Event,
    Write,
    Resize,
    Shutdown,
}

#[derive(Default)]
struct Observed {
    entered: Notify,
    active: AtomicUsize,
    dropped: AtomicUsize,
    shutdowns: AtomicUsize,
    writes: Mutex<Vec<Vec<u8>>>,
}
struct BorrowGuard(Arc<Observed>);
impl Drop for BorrowGuard {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}
struct Fake {
    stall: Stall,
    observed: Arc<Observed>,
    output: Option<Vec<u8>>,
}
impl Fake {
    async fn stop(&self, stage: Stall) {
        if stage == self.stall {
            self.observed.active.fetch_add(1, Ordering::SeqCst);
            let _borrow = BorrowGuard(self.observed.clone());
            self.observed.entered.notify_one();
            std::future::pending::<()>().await;
        }
    }
}
impl Drop for Fake {
    fn drop(&mut self) {
        self.observed.dropped.fetch_add(1, Ordering::SeqCst);
    }
}
#[async_trait]
impl SessionTransport for Fake {
    fn capabilities(&self) -> TransportCapabilities {
        TransportCapabilities::default()
    }
    fn child_process_id(&self) -> Option<u32> {
        None
    }
    async fn connect(
        &mut self,
        _: &TransportRequest,
        _: InteractionBroker,
    ) -> Result<(), TransportError> {
        self.stop(Stall::Connect).await;
        Ok(())
    }
    async fn next_event(&mut self) -> Result<TransportEvent, TransportError> {
        if let Some(bytes) = self.output.take() {
            return Ok(TransportEvent::Output(bytes));
        }
        if self.stall == Stall::Event {
            self.stop(Stall::Event).await;
        }
        std::future::pending().await
    }
    async fn write(&mut self, bytes: &[u8]) -> Result<(), TransportError> {
        // 模拟写了前缀后阻塞；不能关闭后重放整段。
        self.observed
            .writes
            .lock()
            .expect("写记录")
            .push(bytes[..bytes.len().min(1)].to_vec());
        self.stop(Stall::Write).await;
        Ok(())
    }
    async fn resize(&mut self, _: TerminalSize) -> Result<(), TransportError> {
        self.stop(Stall::Resize).await;
        Ok(())
    }
    async fn shutdown(&mut self) -> Result<(), TransportError> {
        assert_eq!(
            self.observed.active.load(Ordering::SeqCst),
            0,
            "关闭前已释放当前可变借用"
        );
        self.observed.shutdowns.fetch_add(1, Ordering::SeqCst);
        self.stop(Stall::Shutdown).await;
        Ok(())
    }
}

fn size() -> TerminalSize {
    TerminalSize {
        cols: 80,
        rows: 24,
        pixel_width: 640,
        pixel_height: 480,
        dpi: 96,
    }
}

#[tokio::test]
async fn connect_cancellation_uses_the_production_handshake_and_releases_broker_and_transport() {
    let observed = Arc::new(Observed::default());
    let fake = Fake {
        stall: Stall::Connect,
        observed: observed.clone(),
        output: None,
    };
    let (close, closing) = watch::channel(false);
    let task = tokio::spawn(CLOSING.scope(closing, async move {
        let mut transport: Box<dyn SessionTransport> = Box::new(fake);
        let (_tx, mut commands) = unbounded_channel();
        let (_reply_tx, mut replies) = unbounded_channel();
        let mut backlog = std::collections::VecDeque::new();
        let mut prompts = PromptIds::default();
        let profile = ConnectionProfile::new("替身连接", "example.test");
        handshake(
            transport.as_mut(),
            1,
            &profile,
            &TransportRequest::new(size()),
            Channels {
                commands: &mut commands,
                replies: &mut replies,
                backlog: &mut backlog,
            },
            &mut prompts,
            None,
        )
        .await
    }));
    observed.entered.notified().await;
    close.send_replace(true);
    assert!(matches!(task.await.expect("握手任务"), Err(Abort::Closed)));
    assert_eq!(observed.active.load(Ordering::SeqCst), 0);
    assert_eq!(observed.shutdowns.load(Ordering::SeqCst), 1);
    assert_eq!(observed.dropped.load(Ordering::SeqCst), 1);
}

async fn cancel_running(
    stall: Stall,
    command: Option<SessionCommand>,
    output: Option<Vec<u8>>,
) -> Arc<Observed> {
    let observed = Arc::new(Observed::default());
    let fake = Fake {
        stall,
        observed: observed.clone(),
        output,
    };
    let (close, closing) = watch::channel(false);
    let (tx, mut commands) = unbounded_channel();
    if let Some(command) = command {
        assert!(tx.send(command).is_ok());
    }
    let task = tokio::spawn(CLOSING.scope(closing, async move {
        let mut transport: Box<dyn SessionTransport> = Box::new(fake);
        let mut size = size();
        let settings = TerminalSettingsV1::default().resolve(&TerminalOverrides::default());
        let mut screen = Screen::new(
            DefaultTerminalEngine::new(&settings, size).expect("真实引擎"),
            size,
        );
        let (_, quit) = run_connected(
            1,
            &mut screen,
            transport.as_mut(),
            &mut size,
            &mut commands,
            &mut std::collections::VecDeque::new(),
        )
        .await;
        assert!(quit);
        transport_lifecycle::shutdown(transport.as_mut())
            .await
            .expect("关闭");
    }));
    observed.entered.notified().await;
    close.send_replace(true);
    tokio::time::timeout(std::time::Duration::from_secs(1), task)
        .await
        .expect("控制路径不等阻塞 I/O")
        .expect("会话任务");
    assert!(tx.is_closed());
    assert_eq!(observed.active.load(Ordering::SeqCst), 0);
    assert_eq!(observed.shutdowns.load(Ordering::SeqCst), 1);
    assert_eq!(observed.dropped.load(Ordering::SeqCst), 1);
    observed
}

#[tokio::test]
async fn next_event_resize_and_partial_write_are_interruptible_without_replay() {
    cancel_running(Stall::Event, None, None).await;
    cancel_running(Stall::Resize, Some(SessionCommand::Resize(size())), None).await;
    for command in [
        SessionCommand::Input(TerminalInput::CommittedText("abcdef".into())),
        SessionCommand::Paste("abcdef".into()),
    ] {
        let observed = cancel_running(Stall::Write, Some(command), None).await;
        assert_eq!(*observed.writes.lock().expect("前缀"), vec![b"a".to_vec()]);
    }
}

#[tokio::test]
async fn engine_query_and_synchronized_output_writes_share_the_cancellable_owner() {
    for bytes in [b"\x1b[6n".to_vec(), b"\x1b[?2026h\x1b[6n".to_vec()] {
        let observed = cancel_running(Stall::Write, None, Some(bytes)).await;
        assert_eq!(observed.writes.lock().expect("查询回答").len(), 1);
    }
}

#[tokio::test(start_paused = true)]
async fn stalled_shutdown_is_bounded_and_never_reported_as_success() {
    let observed = Arc::new(Observed::default());
    let mut transport: Box<dyn SessionTransport> = Box::new(Fake {
        stall: Stall::Shutdown,
        observed: observed.clone(),
        output: None,
    });
    let start = tokio::time::Instant::now();
    assert_eq!(
        transport_lifecycle::shutdown(transport.as_mut()).await,
        Err(transport_lifecycle::ShutdownFailure::TimedOut)
    );
    assert_eq!(start.elapsed(), transport_lifecycle::SHUTDOWN_LIMIT);
    assert_eq!(observed.active.load(Ordering::SeqCst), 0);
    drop(transport);
    assert_eq!(observed.dropped.load(Ordering::SeqCst), 1);
}

#[test]
fn ordered_receiver_rejects_mixed_mode_old_generation_and_duplicate_acks_without_touching_the_new_queue()
 {
    use crate::desktop_runtime::Protocol;
    use crate::signals::desktop_session::{
        DesktopEvent, DesktopFailure, DesktopOrdered, DesktopSource,
    };
    let source = DesktopSource {
        session_id: 1,
        generation: 5,
    };
    let (commands, mut receiver) = unbounded_channel();
    let (replies, _reply_rx) = unbounded_channel();
    let (close, _closing) = watch::channel(false);
    let mut sessions = std::collections::HashMap::new();
    sessions.insert(
        1,
        super::SessionHandle {
            generation: 123,
            commands,
            replies,
            close,
            protocol: Some(Protocol::new(source.clone(), "identity".into())),
        },
    );
    let mut request = DesktopOrdered {
        version: 2,
        source,
        identity: "identity".into(),
        seq: 1,
        event: DesktopEvent::FrameAck { frame_seq: 1 },
    };
    request.source.generation = 4;
    assert_eq!(
        super::route_ordered(&mut sessions, request.clone()),
        DesktopFailure::StaleGeneration
    );
    assert!(receiver.try_recv().is_err());
    request.source.generation = 5;
    assert_eq!(
        super::route_ordered(&mut sessions, request.clone()),
        DesktopFailure::None
    );
    assert_eq!(
        super::route_ordered(&mut sessions, request.clone()),
        DesktopFailure::Duplicate
    );
    let SessionCommand::Ordered(command, lease) = receiver.try_recv().expect("只有一份本代ACK")
    else {
        panic!("必须持有预算");
    };
    assert!(matches!(*command, SessionCommand::FrameAck(1)));
    drop(lease);
    assert!(receiver.try_recv().is_err());
    sessions.get_mut(&1).expect("登记").protocol = None;
    request.seq = 2;
    assert_eq!(
        super::route_ordered(&mut sessions, request),
        DesktopFailure::MixedMode
    );
    assert!(receiver.try_recv().is_err());
}

#[tokio::test]
async fn legacy_reply_route_zeroizes_missing_session_answers() {
    let audit = Arc::new(Mutex::new(Vec::new()));
    crate::protected_reply::AUDIT
        .scope(audit.clone(), async {
            super::route_legacy_reply(&std::collections::HashMap::new(), legacy_reply());
        })
        .await;
    assert_eq!(*audit.lock().expect("清零审计"), vec![2]);
}

#[tokio::test]
async fn legacy_reply_route_zeroizes_mixed_v2_answers_without_delivery() {
    let audit = Arc::new(Mutex::new(Vec::new()));
    let (commands, _commands) = unbounded_channel();
    let (replies, mut answers) = unbounded_channel();
    let (close, _closing) = watch::channel(false);
    let sessions = std::collections::HashMap::from([(
        1,
        super::SessionHandle {
            generation: 1,
            commands,
            replies,
            close,
            protocol: Some(crate::desktop_runtime::Protocol::new(
                crate::signals::desktop_session::DesktopSource {
                    session_id: 1,
                    generation: 2,
                },
                "prepared".into(),
            )),
        },
    )]);
    crate::protected_reply::AUDIT
        .scope(audit.clone(), async {
            super::route_legacy_reply(&sessions, legacy_reply());
        })
        .await;
    assert!(answers.try_recv().is_err(), "旧入口不能投递给 V2");
    assert_eq!(*audit.lock().expect("清零审计"), vec![2]);
}

fn legacy_reply() -> crate::signals::interaction::InteractionReply {
    crate::signals::interaction::InteractionReply {
        session_id: 1,
        prompt_id: 7,
        accept: true,
        answers: vec!["测试回答".into(), "多余回答".into()],
        remember: false,
    }
}
