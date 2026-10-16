//! 显式退出链：通知、等待可取消 owner，超时返回未完成，绝不无限等待阻塞 I/O。

use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinSet;

pub(crate) const DRAIN_LIMIT: Duration = Duration::from_secs(3);
pub(crate) const APP_LIMIT: Duration = Duration::from_secs(5);
pub(crate) const RUNTIME_LIMIT: Duration = Duration::from_secs(2);
pub(crate) const SERVICE_LIMIT: Duration = Duration::from_secs(1);

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ShutdownError {
    StartupWorkMayRemain,
    DrainTimedOut { loading: usize, sessions: usize },
    TaskFailures(u32),
    AppTimedOut,
    LifecycleTimedOut,
}

pub(crate) async fn requested(receiver: &mut watch::Receiver<bool>) {
    if *receiver.borrow() {
        return;
    }
    while receiver.changed().await.is_ok() {
        if *receiver.borrow() {
            return;
        }
    }
}

/// supervisor 的实际退出接缝。超时时保留加载任务自身的持有，不把清表冒充释放。
pub(crate) async fn drain<T: Send + 'static>(
    loading: &mut JoinSet<T>,
    running: &mut JoinSet<u32>,
    mut failures: u32,
) -> Result<(), ShutdownError> {
    let result = tokio::time::timeout(DRAIN_LIMIT, async {
        while !loading.is_empty() || !running.is_empty() {
            tokio::select! {
                result = loading.join_next(), if !loading.is_empty() => {
                    if matches!(result, Some(Err(_))) { failures += 1; }
                }
                result = running.join_next(), if !running.is_empty() => {
                    failures += match result { Some(Ok(count)) => count, Some(Err(_)) => 1, None => 0 };
                }
            }
        }
    }).await;
    if result.is_err() {
        let error = ShutdownError::DrainTimedOut {
            loading: loading.len(),
            sessions: running.len(),
        };
        // 尚在 spawn_blocking 中的工作不能强杀。退出后不再接纳请求，真实释放由该工作结束决定。
        loading.detach_all();
        running.abort_all();
        running.detach_all();
        return Err(error);
    }
    if failures == 0 {
        Ok(())
    } else {
        Err(ShutdownError::TaskFailures(failures))
    }
}

pub(crate) async fn run_until(
    stop: impl std::future::Future<Output = ()>,
) -> Result<(), ShutdownError> {
    run_until_with(stop, crate::app::run).await
}

pub(crate) async fn run_until_with<F, A>(
    stop: impl std::future::Future<Output = ()>,
    start: F,
) -> Result<(), ShutdownError>
where
    F: FnOnce(watch::Receiver<bool>) -> A,
    A: std::future::Future<Output = Result<(), ShutdownError>> + Send + 'static,
{
    let (sender, receiver) = watch::channel(false);
    let mut app = tokio::spawn(start(receiver));
    let mut lifecycle = tokio::spawn(crate::lifecycle::run());
    stop.await;
    sender.send_replace(true);
    let result = match tokio::time::timeout(APP_LIMIT, &mut app).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(ShutdownError::TaskFailures(1)),
        Err(_) => {
            app.abort();
            Err(ShutdownError::AppTimedOut)
        }
    };
    lifecycle.abort();
    match tokio::time::timeout(SERVICE_LIMIT, &mut lifecycle).await {
        Ok(Err(error)) if !error.is_cancelled() && result.is_ok() => {
            return Err(ShutdownError::TaskFailures(1));
        }
        Err(_) if result.is_ok() => return Err(ShutdownError::LifecycleTimedOut),
        _ => {}
    }
    result
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn production_app_can_exit_before_app_start_without_leaving_the_start_receiver_waiting() {
        assert_eq!(super::run_until(async {}).await, Ok(()));
    }
}
