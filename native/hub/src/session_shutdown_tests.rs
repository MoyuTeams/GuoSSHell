//! 主退出编排、supervisor 退出接缝及真实状态出口的故障注入。
#![allow(clippy::expect_used)]
use super::{Fake, Observed, Stall, size};
use crate::connect::{Channels, PromptIds, handshake};
use crate::session::{self, Screen, SessionCommand, run_connected};
use crate::transport_lifecycle;
use async_trait::async_trait;
use rshell_m0::rshell_core::{
    ConnectionProfile, TerminalInput, TerminalOverrides, TerminalSettingsV1, TerminalSize,
};
use rshell_m0::rshell_session::{
    DefaultTerminalEngine, InteractionBroker, SessionTransport, TransportCapabilities,
    TransportError, TransportEvent, TransportRequest,
};
use std::sync::{Arc, atomic::Ordering};
use tokio::sync::{mpsc::unbounded_channel, watch};

struct Faulty {
    at: Stall,
    resized: bool,
    timeout_shutdown: bool,
}

#[async_trait]
impl SessionTransport for Faulty {
    fn capabilities(&self) -> TransportCapabilities {
        TransportCapabilities::default()
    }
    async fn connect(
        &mut self,
        _: &TransportRequest,
        _: InteractionBroker,
    ) -> Result<(), TransportError> {
        Err(TransportError::new(
            rshell_m0::rshell_core::SessionFailure::Network,
        ))
    }
    async fn next_event(&mut self) -> Result<TransportEvent, TransportError> {
        match self.at {
            Stall::Event => Err(TransportError::new(
                rshell_m0::rshell_core::SessionFailure::Network,
            )),
            Stall::Write => std::future::pending().await,
            Stall::Resize if !self.resized => std::future::pending().await,
            _ => Ok(TransportEvent::Eof),
        }
    }
    async fn write(&mut self, _: &[u8]) -> Result<(), TransportError> {
        Err(TransportError::new(
            rshell_m0::rshell_core::SessionFailure::Network,
        ))
    }
    async fn resize(&mut self, _: TerminalSize) -> Result<(), TransportError> {
        self.resized = true;
        Err(TransportError::new(
            rshell_m0::rshell_core::SessionFailure::Network,
        ))
    }
    async fn shutdown(&mut self) -> Result<(), TransportError> {
        if self.timeout_shutdown {
            return std::future::pending().await;
        }
        if self.at == Stall::Shutdown {
            return Err(TransportError::new(
                rshell_m0::rshell_core::SessionFailure::Network,
            ));
        }
        Ok(())
    }
}

#[tokio::test(start_paused = true)]
async fn trait_errors_and_shutdown_timeout_reach_the_production_status_exit() {
    for (at, timeout_shutdown, expected) in [
        (Stall::Connect, false, "Network"),
        (Stall::Event, false, "ConnectionLost"),
        (Stall::Write, false, "ConnectionLost"),
        (Stall::Resize, false, "None"),
        (Stall::Shutdown, false, "Other"),
        (Stall::Shutdown, true, "Timeout"),
    ] {
        let (events, mut observed) = unbounded_channel();
        let source = crate::desktop_runtime::Source {
            source: crate::signals::desktop_session::DesktopSource {
                session_id: 1,
                generation: 3,
            },
            identity: "错误测试来源".into(),
        };
        crate::status_sink::OBSERVER
            .scope(
                events,
                crate::desktop_runtime::SOURCE.scope(source, async {
                    let mut transport: Box<dyn SessionTransport> = Box::new(Faulty {
                        at,
                        resized: false,
                        timeout_shutdown,
                    });
                    let (tx, mut commands) = unbounded_channel();
                    let (_reply_tx, mut replies) = unbounded_channel();
                    let mut backlog = std::collections::VecDeque::new();
                    if at == Stall::Connect {
                        let profile = ConnectionProfile::new("错误测试", "example.test");
                        let result = handshake(
                            transport.as_mut(),
                            1,
                            &profile,
                            &TransportRequest::new(size()),
                            Channels {
                                commands: &mut commands,
                                replies: &mut replies,
                                backlog: &mut backlog,
                            },
                            &mut PromptIds::default(),
                            None,
                        )
                        .await;
                        let Err(abort) = result else {
                            panic!("必须注入实际 trait 错误");
                        };
                        session::report_abort(1, abort, "", 0).await;
                    } else {
                        if at == Stall::Write {
                            assert!(
                                tx.send(SessionCommand::Input(TerminalInput::CommittedText(
                                    "test".into()
                                )))
                                .is_ok()
                            );
                        }
                        if at == Stall::Resize {
                            assert!(tx.send(SessionCommand::Resize(size())).is_ok());
                        }
                        let mut size = size();
                        let settings =
                            TerminalSettingsV1::default().resolve(&TerminalOverrides::default());
                        let mut screen = Screen::new(
                            DefaultTerminalEngine::new(&settings, size).expect("引擎"),
                            size,
                        );
                        let (end, _) = run_connected(
                            1,
                            &mut screen,
                            transport.as_mut(),
                            &mut size,
                            &mut commands,
                            &mut backlog,
                        )
                        .await;
                        session::finish_transport(1, transport.as_mut(), end).await;
                    }
                }),
            )
            .await;
        let (wrapped, event) = observed.recv().await.expect("生产出口");
        assert!(wrapped);
        assert_eq!(event["source"]["generation"], 3);
        assert_eq!(event["status"]["failure"], expected);
        assert_eq!(
            event["status"]["state"],
            if at == Stall::Resize {
                "Closed"
            } else {
                "Failed"
            }
        );
        if timeout_shutdown {
            assert_eq!(event["status"]["detail"], "transport_shutdown_timeout");
        }
        assert!(
            observed.try_recv().is_err(),
            "关闭超时不能先发 Closed 冒充成功"
        );
    }
}

fn preparation() -> crate::signals::desktop_session::DesktopPrepare {
    crate::signals::desktop_session::DesktopPrepare {
        version: 2,
        request_id: 1,
        source: crate::signals::desktop_session::DesktopSource {
            session_id: 2,
            generation: 1,
        },
        target: crate::signals::DesktopTargetKind::NativeSsh,
        profile_id: String::new(),
        connection_id: String::new(),
        host: "example.test".into(),
        port: 22,
        username: "fixture".into(),
        command: String::new(),
    }
}

#[tokio::test(start_paused = true)]
async fn application_exit_preserves_transport_shutdown_failure_in_its_result_and_status() {
    for timeout_shutdown in [false, true] {
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (events, mut statuses) = unbounded_channel();
        let result = crate::shutdown::run_until_with(
            async move { ready.await.expect("传输 owner 就绪") },
            move |mut shutdown| async move {
                let (close, closing) = watch::channel(false);
                let (commands, _receiver) = unbounded_channel();
                let (replies, _answers) = unbounded_channel();
                let mut sessions = std::collections::HashMap::new();
                sessions.insert(
                    1,
                    session::SessionHandle {
                        generation: 1,
                        commands,
                        replies,
                        close,
                        protocol: None,
                    },
                );
                let mut running = tokio::task::JoinSet::new();
                running.spawn(crate::status_sink::OBSERVER.scope(
                    events,
                    transport_lifecycle::owner(closing, async move {
                        let mut transport: Box<dyn SessionTransport> = Box::new(Faulty {
                            at: Stall::Shutdown,
                            resized: false,
                            timeout_shutdown,
                        });
                        entered.send(()).expect("退出前有真实 owner");
                        session::finish_transport(
                            1,
                            transport.as_mut(),
                            session::SessionEnd::closed("disconnect"),
                        )
                        .await;
                    }),
                ));
                crate::shutdown::requested(&mut shutdown).await;
                session::stop_supervisor(
                    &mut sessions,
                    &mut crate::desktop_prepare::Preparations::default(),
                    &mut tokio::task::JoinSet::new(),
                    &mut running,
                    0,
                )
                .await
            },
        )
        .await;
        assert_eq!(result, Err(crate::shutdown::ShutdownError::TaskFailures(1)));
        let (_, status) = statuses.recv().await.expect("退出失败状态");
        assert_eq!(status["state"], "Failed");
        assert_eq!(
            status["failure"],
            if timeout_shutdown { "Timeout" } else { "Other" }
        );
        assert!(statuses.try_recv().is_err());
    }
}

#[tokio::test(start_paused = true)]
async fn application_stop_notifies_real_supervisor_exit_and_waits_loading_and_transport_owner() {
    let observed = Arc::new(Observed::default());
    let stop = observed.clone();
    let task_observed = observed.clone();
    let (events, mut statuses) = unbounded_channel();
    let (budget_tx, budget_rx) = tokio::sync::oneshot::channel();
    let result = crate::shutdown::run_until_with(
        async move { stop.entered.notified().await },
        move |mut shutdown| async move {
            let mut preparations = crate::desktop_prepare::Preparations::enabled_for_test();
            let (identity, lease, _) = preparations
                .begin(preparation(), tokio::time::Instant::now())
                .expect("加载预约");
            assert!(budget_tx.send(preparations.budget.clone()).is_ok());
            let mut loading = tokio::task::JoinSet::new();
            let mut load_shutdown = shutdown.clone();
            loading.spawn(async move {
                crate::shutdown::requested(&mut load_shutdown).await;
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                crate::desktop_prepare::Loaded {
                    identity,
                    lease,
                    result: Err((
                        crate::signals::DesktopConnectError::Storage,
                        "退出测试".into(),
                    )),
                }
            });
            let (commands, mut receiver) = unbounded_channel();
            assert!(
                commands
                    .send(SessionCommand::Input(TerminalInput::CommittedText(
                        "partial".into()
                    )))
                    .is_ok()
            );
            let (replies, _answers) = unbounded_channel();
            let (close, closing) = watch::channel(false);
            let mut sessions = std::collections::HashMap::new();
            sessions.insert(
                1,
                session::SessionHandle {
                    generation: 1,
                    commands,
                    replies,
                    close,
                    protocol: None,
                },
            );
            let mut running = tokio::task::JoinSet::new();
            running.spawn(crate::status_sink::OBSERVER.scope(
                events,
                transport_lifecycle::owner(closing, async move {
                    let mut transport: Box<dyn SessionTransport> = Box::new(Fake {
                        stall: Stall::Write,
                        observed: task_observed,
                        output: None,
                    });
                    let mut size = size();
                    let settings =
                        TerminalSettingsV1::default().resolve(&TerminalOverrides::default());
                    let mut screen = Screen::new(
                        DefaultTerminalEngine::new(&settings, size).expect("引擎"),
                        size,
                    );
                    let (end, quit) = run_connected(
                        1,
                        &mut screen,
                        transport.as_mut(),
                        &mut size,
                        &mut receiver,
                        &mut std::collections::VecDeque::new(),
                    )
                    .await;
                    assert!(quit);
                    session::finish_transport(1, transport.as_mut(), end).await;
                }),
            ));
            crate::shutdown::requested(&mut shutdown).await;
            session::stop_supervisor(
                &mut sessions,
                &mut preparations,
                &mut loading,
                &mut running,
                0,
            )
            .await
        },
    )
    .await;
    assert_eq!(result, Ok(()));
    assert_eq!(
        budget_rx.await.expect("预算").usage(),
        crate::desktop_prepare::Usage::default()
    );
    assert_eq!(observed.active.load(Ordering::SeqCst), 0);
    assert_eq!(observed.dropped.load(Ordering::SeqCst), 1);
    assert_eq!(observed.shutdowns.load(Ordering::SeqCst), 1);
    assert_eq!(observed.writes.lock().expect("部分写").len(), 1);
    let (desktop, status) = statuses.recv().await.expect("真实结束状态");
    assert!(!desktop);
    assert_eq!(status["state"], "Closed");
}

#[tokio::test(start_paused = true)]
async fn application_stop_reports_unfinished_blocking_loader_instead_of_waiting_forever_or_releasing_its_lease()
 {
    let (release, blocked) = std::sync::mpsc::channel();
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (finished, done) = tokio::sync::oneshot::channel();
    let (budget_tx, budget_rx) = tokio::sync::oneshot::channel();
    let (draining, drain_started) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(crate::shutdown::run_until_with(
        async move { ready.await.expect("阻塞加载开始") },
        move |mut shutdown| async move {
            let mut preparations = crate::desktop_prepare::Preparations::enabled_for_test();
            let (identity, lease, _) = preparations
                .begin(preparation(), tokio::time::Instant::now())
                .expect("加载预约");
            assert!(budget_tx.send(preparations.budget.clone()).is_ok());
            let mut loading = tokio::task::JoinSet::new();
            loading.spawn(async move {
                let (_, _io_lease) =
                    crate::connect::load_desktop_io(Some(lease.clone()), move || {
                        entered.send(()).expect("已进入");
                        blocked.recv().expect("释放底层 I/O");
                    })
                    .await
                    .expect("阻塞工作完成");
                finished.send(()).expect("释放通知");
                crate::desktop_prepare::Loaded {
                    identity,
                    lease,
                    result: Err((
                        crate::signals::DesktopConnectError::Storage,
                        "退出测试".into(),
                    )),
                }
            });
            crate::shutdown::requested(&mut shutdown).await;
            draining.send(()).expect("真实退出路径已通知");
            session::stop_supervisor(
                &mut std::collections::HashMap::new(),
                &mut preparations,
                &mut loading,
                &mut tokio::task::JoinSet::new(),
                0,
            )
            .await
        },
    ));
    drain_started.await.expect("已进入 supervisor 退出接缝");
    assert!(!task.is_finished());
    // 真实 blocking 工作存在时 Tokio 暂停时钟不自动快进，显式推进被测截止时间。
    tokio::time::advance(crate::shutdown::DRAIN_LIMIT).await;
    let result = task.await.expect("主退出编排");
    assert_eq!(
        result,
        Err(crate::shutdown::ShutdownError::DrainTimedOut {
            loading: 1,
            sessions: 0
        })
    );
    let budget = budget_rx.await.expect("预算");
    assert_eq!(budget.usage().inflight, 1, "不能把超时说成底层工作释放");
    release.send(()).expect("结束本轮真实阻塞替身");
    done.await.expect("等待真实释放");
    assert_eq!(budget.usage(), crate::desktop_prepare::Usage::default());
}

#[test]
fn runtime_shutdown_timeout_keeps_blocking_io_lease_until_the_real_worker_finishes() {
    // 独立 Runtime 真的被消费；不能用仍存活的测试 Runtime 代替最终收尾。
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("独立 Runtime");
    let (release, blocked) = std::sync::mpsc::channel();
    let (finished, done) = std::sync::mpsc::channel();
    let (entered, ready) = tokio::sync::oneshot::channel();
    let mut preparations = crate::desktop_prepare::Preparations::enabled_for_test();
    let (identity, lease, _) = preparations
        .begin(preparation(), tokio::time::Instant::now())
        .expect("加载预约");
    let budget = preparations.budget.clone();
    let reserved = budget.usage();
    assert_eq!(reserved.count, 1);
    assert_eq!(reserved.inflight, 1);
    assert!(reserved.bytes > 0);
    let result = runtime.block_on(crate::shutdown::run_until_with(
        async move { ready.await.expect("真实阻塞工作开始") },
        move |mut shutdown| async move {
            let mut loading = tokio::task::JoinSet::new();
            loading.spawn(async move {
                let (result, _io_lease) =
                    crate::connect::load_desktop_io(Some(lease.clone()), move || {
                        entered.send(()).expect("进入 I/O");
                        // 失败展开时 sender 的 Drop 也会放行，不留下阻塞线程。
                        let _ = blocked.recv();
                        finished.send(()).expect("真实工作完成");
                        Err((
                            crate::signals::DesktopConnectError::Storage,
                            "退出测试".into(),
                        ))
                    })
                    .await
                    .expect("I/O 返回");
                crate::desktop_prepare::Loaded {
                    identity,
                    result,
                    lease,
                }
            });
            crate::shutdown::requested(&mut shutdown).await;
            session::stop_supervisor(
                &mut std::collections::HashMap::new(),
                &mut preparations,
                &mut loading,
                &mut tokio::task::JoinSet::new(),
                0,
            )
            .await
        },
    ));
    runtime.shutdown_timeout(crate::shutdown::RUNTIME_LIMIT);
    let after_runtime = budget.usage();
    let worker_still_blocked = matches!(done.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty));

    // 先确保真实工作结束，再断言退出状态；断言失败时也不遗留阻塞工作。
    release.send(()).expect("放行真实阻塞工作");
    done.recv_timeout(std::time::Duration::from_secs(3))
        .expect("底层工作结束");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while budget.usage() != crate::desktop_prepare::Usage::default()
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(
        result,
        Err(crate::shutdown::ShutdownError::DrainTimedOut {
            loading: 1,
            sessions: 0
        })
    );
    assert!(worker_still_blocked, "Runtime 超时不能冒充 thread kill");
    assert_eq!(
        after_runtime,
        crate::desktop_prepare::Usage {
            count: 0,
            ..reserved
        },
        "准备槽已撤销，真实 I/O 结束前仍须保留完整 bytes/inflight 预约"
    );
    assert_eq!(budget.usage(), crate::desktop_prepare::Usage::default());
}
