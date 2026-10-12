use std::{
    future::Future,
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

use super::core::{MODES, Owner, READ_BYTES, STOPPED, TRANSPORT_FAILURE};

const SERVICE_TICK: Duration = Duration::from_millis(1);

/// 单个固定 fixture worker；不是上游 SessionTransport 的包装或修复。
pub(super) fn start(owner: Arc<Owner>, stream: DuplexStream) -> Result<(), u32> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .map_err(|_| TRANSPORT_FAILURE)?;
    let worker = Arc::clone(&owner);
    let mut handle = owner.worker.try_lock().map_err(|_| STOPPED)?;
    *handle = Some(
        std::thread::Builder::new()
            .name("native-input-fixture".into())
            .spawn(move || {
                // 分离读写 future；长 write 不持 core 锁，也不暂停另一个方向读取。
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    runtime.block_on(run(&worker, stream));
                }));
                if result.is_err() {
                    worker.stop();
                }
                worker.retire();
                #[cfg(test)]
                super::core::test_pause(&worker.thread_tail);
            })
            .map_err(|_| TRANSPORT_FAILURE)?,
    );
    Ok(())
}

/// 只有 worker 使用 timer；UI 只写 atomic／固定 CPU 状态，不调用 Notify 的内部锁
/// 或 waker。正常 tick 不重新创建读写操作，partial write 绝不能由 tick 触发重放。
async fn until_stopped<F: Future>(owner: &Owner, future: F) -> Option<F::Output> {
    tokio::pin!(future);
    loop {
        if owner.pending_revoke.load(Ordering::Acquire) {
            owner.stop();
        }
        if owner.stopped.load(Ordering::Acquire) {
            return None;
        }
        tokio::select! {
            result = &mut future => return Some(result),
            _ = tokio::time::sleep(SERVICE_TICK) => {}
        }
    }
}

#[cfg(test)]
async fn observe_write<F: Future>(owner: &Owner, future: F) -> F::Output {
    tokio::pin!(future);
    std::future::poll_fn(|cx| {
        let result = future.as_mut().poll(cx);
        if result.is_pending() {
            // 先真实 poll write_all，再停在 Pending；保留同一个 future 的部分写位置。
            super::core::test_pause(&owner.write_pending);
        }
        result
    })
    .await
}

pub(super) async fn run(owner: &Owner, stream: DuplexStream) {
    let (mut reader, mut writer) = tokio::io::split(stream);
    let read = async {
        loop {
            if owner.stopped.load(Ordering::Acquire) {
                break;
            }
            let mut len = [0u8; 1];
            let Some(length) = until_stopped(owner, reader.read_exact(&mut len)).await else {
                break;
            };
            if length.is_err() || !(1..=READ_BYTES as u8).contains(&len[0]) {
                owner.stop();
                break;
            }
            let mut bytes = [0u8; READ_BYTES];
            let Some(body) =
                until_stopped(owner, reader.read_exact(&mut bytes[..usize::from(len[0])])).await
            else {
                break;
            };
            if body.is_err() {
                owner.stop();
                break;
            }
            let bytes = &bytes[..usize::from(len[0])];
            if !MODES.contains(&bytes) {
                owner.stop();
                break;
            }
            loop {
                if owner.stopped.load(Ordering::Acquire) {
                    return;
                }
                let accepted = match owner.core.lock() {
                    Ok(mut core) => {
                        if owner.stopped.load(Ordering::Acquire) {
                            Err(STOPPED)
                        } else {
                            core.remote(bytes)
                        }
                    }
                    Err(_) => Err(STOPPED),
                };
                match accepted {
                    Ok(true) => break,
                    Ok(false) => tokio::time::sleep(SERVICE_TICK).await,
                    Err(_) => {
                        owner.stop();
                        return;
                    }
                }
            }
        }
    };
    let write = async {
        loop {
            #[cfg(test)]
            super::core::test_pause(&owner.write_boundary);
            if owner.stopped.load(Ordering::Acquire) {
                break;
            }
            if owner.release_pending().is_err() {
                owner.stop();
                break;
            }
            let next = match owner.core.lock() {
                Ok(mut core) => {
                    if owner.stopped.load(Ordering::Acquire) {
                        break;
                    }
                    core.take_write()
                }
                Err(_) => {
                    owner.stop();
                    break;
                }
            };
            let (slot, packet) = match next {
                Ok(Some(packet)) => packet,
                Ok(None) => {
                    tokio::time::sleep(SERVICE_TICK).await;
                    continue;
                }
                Err(_) => {
                    owner.stop();
                    break;
                }
            };
            // 原槽保持 Flight；直到完整写成功或者停用后丢弃，预算均不提前释放。
            let write = writer.write_all(&packet.bytes[..packet.len]);
            #[cfg(test)]
            let write = observe_write(owner, write);
            let Some(result) = until_stopped(owner, write).await else {
                break;
            };
            if result.is_err() {
                owner.stop();
                break;
            }
            #[cfg(test)]
            super::core::test_pause(&owner.write_complete);
            let completed = match owner.core.lock() {
                Ok(mut core) => {
                    if owner.stopped.load(Ordering::Acquire) {
                        Err(STOPPED)
                    } else {
                        core.finish_write(slot, packet.seq)
                    }
                }
                Err(_) => Err(STOPPED),
            };
            if completed.is_err() {
                owner.stop();
                break;
            }
        }
    };
    let timer = async {
        while !owner.stopped.load(Ordering::Acquire) {
            let armed = match owner.core.lock() {
                Ok(core) => match core.effects.sync.cell {
                    super::deadline::Cell::Armed { wake, witness } => {
                        Some((wake, witness.deadline))
                    }
                    _ => None,
                },
                Err(_) => {
                    owner.stop();
                    break;
                }
            };
            if let Some((wake, deadline)) = armed {
                if until_stopped(
                    owner,
                    tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)),
                )
                .await
                .is_none()
                {
                    break;
                }
                let result = match owner.core.lock() {
                    Ok(mut core) => {
                        let super::core::Core {
                            effects, next_seq, ..
                        } = &mut *core;
                        effects.timer_wake(next_seq, wake, std::time::Instant::now())
                    }
                    Err(_) => Err(STOPPED),
                };
                if result.is_err() {
                    owner.stop();
                    break;
                }
            } else {
                // Waiting/Queued 不创建到期 timer；容量重试只由实际释放驱动。
                tokio::time::sleep(SERVICE_TICK).await;
            }
        }
    };
    tokio::join!(read, write, timer);
}
