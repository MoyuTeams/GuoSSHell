//! 桌面有序入口的来源、资源上限与输入流排他选择。

use crate::signals::desktop_session::{
    DesktopEvent, DesktopFailure, DesktopOrdered, DesktopSource,
};
use std::sync::{Arc, Mutex};

pub(crate) const INPUT_EVENTS: usize = 256;
pub(crate) const INPUT_BYTES: usize = 1024 * 1024;

#[derive(Clone)]
pub(crate) struct Source {
    pub source: DesktopSource,
    pub identity: String,
}
tokio::task_local! { pub(crate) static SOURCE: Source; }

#[derive(Default)]
struct Usage {
    events: usize,
    bytes: usize,
}
pub(crate) struct InputLease {
    usage: Arc<Mutex<Usage>>,
    bytes: usize,
}
impl Drop for InputLease {
    fn drop(&mut self) {
        let mut u = self.usage.lock().unwrap_or_else(|e| e.into_inner());
        u.events -= 1;
        u.bytes -= self.bytes;
    }
}

pub(crate) struct Protocol {
    pub source: Source,
    pub mode: Arc<crate::desktop_input_port::InputMode>,
    pub input: Option<Arc<crate::desktop_input_port::InputPort>>,
    /// 有序通道的物理键锁存；只在 supervisor 内按 seq 顺序门控，与原生端口状态互斥。
    pub keys: crate::desktop_input_policy::InputPolicyState,
    last_seq: u64,
    usage: Arc<Mutex<Usage>>,
}
impl Protocol {
    pub(crate) fn new(source: DesktopSource, identity: String) -> Self {
        Self {
            source: Source { source, identity },
            mode: Arc::default(),
            input: None,
            keys: Default::default(),
            last_seq: 0,
            usage: Arc::default(),
        }
    }
    pub(crate) fn admit(&mut self, r: &DesktopOrdered) -> Result<InputLease, DesktopFailure> {
        if r.version != crate::signals::desktop_session::VERSION {
            return Err(DesktopFailure::UnsupportedVersion);
        }
        if r.source != self.source.source {
            return Err(DesktopFailure::StaleGeneration);
        }
        if r.identity != self.source.identity {
            return Err(DesktopFailure::IdentityMismatch);
        }
        if r.seq <= self.last_seq {
            return Err(DesktopFailure::Duplicate);
        }
        if self.last_seq.checked_add(1) != Some(r.seq) {
            return Err(DesktopFailure::OutOfOrder);
        }
        let payload = match &r.event {
            DesktopEvent::Text { text } | DesktopEvent::Paste { text } => text.len(),
            DesktopEvent::Key { key, .. } => key.len(),
            DesktopEvent::Mouse { kind, button, .. } => kind.len() + button.len(),
            _ => 0,
        };
        // 括号粘贴可能同时持有原文、过滤后文本及最终字节；元数据一起按峰值预约。
        let bytes = payload.saturating_mul(3).saturating_add(512);
        let mut u = self.usage.lock().unwrap_or_else(|e| e.into_inner());
        // 关闭是控制通道，不因数据队列写满而失效，也不进入数据 backlog。
        let control = matches!(r.event, DesktopEvent::Disconnect);
        if bytes > INPUT_BYTES {
            return Err(DesktopFailure::InputTooLarge);
        }
        if !control && (u.events >= INPUT_EVENTS || u.bytes.saturating_add(bytes) > INPUT_BYTES) {
            return Err(DesktopFailure::Busy);
        }
        if !self.mode.legacy() {
            return Err(DesktopFailure::MixedMode);
        }
        self.last_seq = r.seq;
        u.events += 1;
        u.bytes += bytes;
        Ok(InputLease {
            usage: self.usage.clone(),
            bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::{
        DesktopEvent, DesktopFailure, DesktopOrdered, DesktopSource, INPUT_BYTES, INPUT_EVENTS,
        Protocol,
    };
    fn event(seq: u64, event: DesktopEvent) -> DesktopOrdered {
        DesktopOrdered {
            version: 2,
            source: DesktopSource {
                session_id: 1,
                generation: 3,
            },
            identity: "prepared-id".into(),
            seq,
            event,
        }
    }
    #[test]
    fn ordered_count_bytes_and_identity_cover_backlog_without_blocking_another_pane() {
        let mut protocol = Protocol::new(
            DesktopSource {
                session_id: 1,
                generation: 3,
            },
            "prepared-id".into(),
        );
        let mut other = Protocol::new(
            DesktopSource {
                session_id: 2,
                generation: 1,
            },
            "other".into(),
        );
        let mut queued = Vec::new();
        for seq in 1..=INPUT_EVENTS as u64 {
            queued.push(
                protocol
                    .admit(&event(seq, DesktopEvent::Text { text: "a".into() }))
                    .expect("有界入队"),
            );
        }
        let next = INPUT_EVENTS as u64 + 1;
        assert!(matches!(
            protocol.admit(&event(next, DesktopEvent::Paste { text: "b".into() })),
            Err(DesktopFailure::Busy)
        ));
        let mut request = event(
            1,
            DesktopEvent::Text {
                text: "另一窗格".into(),
            },
        );
        request.source = other.source.source.clone();
        request.identity = "other".into();
        assert!(other.admit(&request).is_ok());
        drop(queued);
        let lease = protocol
            .admit(&event(next, DesktopEvent::Paste { text: "b".into() }))
            .expect("同 seq 重试");
        assert!(matches!(
            protocol.admit(&event(next, DesktopEvent::Text { text: "dup".into() })),
            Err(DesktopFailure::Duplicate)
        ));
        drop(lease);
        assert!(matches!(
            protocol.admit(&event(next + 2, DesktopEvent::Reconnect)),
            Err(DesktopFailure::OutOfOrder)
        ));
        assert!(matches!(
            protocol.admit(&event(
                next + 1,
                DesktopEvent::Paste {
                    text: "a".repeat(INPUT_BYTES)
                }
            )),
            Err(DesktopFailure::InputTooLarge)
        ));
        let mut stale = event(next + 1, DesktopEvent::FrameAck { frame_seq: 999 });
        stale.source.generation -= 1;
        assert!(matches!(
            protocol.admit(&stale),
            Err(DesktopFailure::StaleGeneration)
        ));
        let first = protocol
            .admit(&event(
                next + 1,
                DesktopEvent::Paste {
                    text: "a".repeat(INPUT_BYTES / 4),
                },
            ))
            .expect("按真实字节预约");
        assert!(matches!(
            protocol.admit(&event(
                next + 2,
                DesktopEvent::Paste {
                    text: "b".repeat(INPUT_BYTES / 4)
                }
            )),
            Err(DesktopFailure::Busy)
        ));
        assert!(
            protocol
                .admit(&event(next + 2, DesktopEvent::Disconnect))
                .is_ok(),
            "关闭不受数据预算限制"
        );
        drop(first);
    }
}
