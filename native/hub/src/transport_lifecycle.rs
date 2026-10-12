//! 传输层始终由会话任务独占；关闭先丢弃当前借用，再在同一 owner 上有界 shutdown。

use rshell_m0::rshell_session::SessionTransport;
use std::time::Duration;
use tokio::sync::watch;

pub(crate) const SHUTDOWN_LIMIT: Duration = Duration::from_secs(2);
tokio::task_local! {
    pub(crate) static CLOSING: watch::Receiver<bool>;
    static FAILURES: std::sync::Arc<std::sync::atomic::AtomicU32>;
}

pub(crate) async fn owner(
    closing: watch::Receiver<bool>,
    future: impl std::future::Future<Output = ()>,
) -> u32 {
    let failures = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    FAILURES
        .scope(failures.clone(), CLOSING.scope(closing, future))
        .await;
    failures.load(std::sync::atomic::Ordering::Relaxed)
}

pub(crate) async fn closing() {
    let Ok(mut receiver) = CLOSING.try_with(Clone::clone) else {
        return std::future::pending().await;
    };
    if *receiver.borrow() {
        return;
    }
    while receiver.changed().await.is_ok() {
        if *receiver.borrow() {
            return;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShutdownFailure {
    Failed,
    TimedOut,
}

impl ShutdownFailure {
    pub(crate) fn detail(self) -> &'static str {
        match self {
            Self::Failed => "transport_shutdown_failed",
            Self::TimedOut => "transport_shutdown_timeout",
        }
    }
    pub(crate) fn kind(self) -> crate::signals::FailureKind {
        match self {
            Self::Failed => crate::signals::FailureKind::Other,
            Self::TimedOut => crate::signals::FailureKind::Timeout,
        }
    }
}

pub(crate) async fn shutdown(transport: &mut dyn SessionTransport) -> Result<(), ShutdownFailure> {
    let result = match tokio::time::timeout(SHUTDOWN_LIMIT, transport.shutdown()).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_)) => Err(ShutdownFailure::Failed),
        Err(_) => Err(ShutdownFailure::TimedOut),
    };
    if result.is_err() {
        let _ = FAILURES.try_with(|count| count.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    }
    result
}
