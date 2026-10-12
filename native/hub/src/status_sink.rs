//! 在会话 owner 上捕获出口身份；后台签名回调不依赖 task_local 的传播。

use crate::desktop_runtime::{self, Source};
use crate::signals::{SessionStatus, desktop_session::DesktopSessionStatus};
use rinf::RustSignal;

#[derive(Clone)]
pub(crate) struct StatusSink {
    source: Option<Source>,
    #[cfg(test)]
    observer: Option<tokio::sync::mpsc::UnboundedSender<(bool, serde_json::Value)>>,
}

#[cfg(test)]
tokio::task_local! {
    pub(crate) static OBSERVER: tokio::sync::mpsc::UnboundedSender<(bool, serde_json::Value)>;
}

impl StatusSink {
    pub(crate) fn capture() -> Self {
        Self {
            source: desktop_runtime::SOURCE.try_with(Clone::clone).ok(),
            #[cfg(test)]
            observer: OBSERVER.try_with(Clone::clone).ok(),
        }
    }

    pub(crate) fn send(&self, status: SessionStatus) {
        match &self.source {
            Some(source) => {
                let message = DesktopSessionStatus {
                    source: source.source.clone(),
                    identity: source.identity.clone(),
                    status,
                };
                #[cfg(test)]
                self.observe(true, &message);
                message.send_signal_to_dart();
            }
            None => {
                #[cfg(test)]
                self.observe(false, &status);
                status.send_signal_to_dart();
            }
        }
    }

    #[cfg(test)]
    fn observe(&self, desktop: bool, status: &impl serde::Serialize) {
        if let Some(observer) = &self.observer {
            let value = serde_json::to_value(status).unwrap_or_else(|_| panic!("状态序列化失败"));
            let _ = observer.send((desktop, value));
        }
    }
}
