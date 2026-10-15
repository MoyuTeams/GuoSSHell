//! 认证回答的队列所有权。边界 DTO 保持原布局，内部拒绝和通道丢弃均清零。

use crate::signals::interaction::InteractionReply;
use secrecy::SecretString;
use zeroize::Zeroize;

#[cfg(test)]
tokio::task_local! {
    pub(crate) static AUDIT: std::sync::Arc<std::sync::Mutex<Vec<usize>>>;
}

pub(crate) struct ProtectedReply {
    message: InteractionReply,
    #[cfg(test)]
    audit: Option<std::sync::Arc<std::sync::Mutex<Vec<usize>>>>,
}

impl From<InteractionReply> for ProtectedReply {
    fn from(message: InteractionReply) -> Self {
        Self {
            message,
            #[cfg(test)]
            audit: AUDIT.try_with(Clone::clone).ok(),
        }
    }
}

impl std::ops::Deref for ProtectedReply {
    type Target = InteractionReply;
    fn deref(&self) -> &Self::Target {
        &self.message
    }
}

impl std::ops::DerefMut for ProtectedReply {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.message
    }
}

impl ProtectedReply {
    pub(crate) fn first_secret(&mut self) -> Option<SecretString> {
        self.message
            .answers
            .first_mut()
            .map(|answer| SecretString::from(std::mem::take(answer)))
    }

    pub(crate) fn all_secrets(&mut self) -> Vec<SecretString> {
        self.message
            .answers
            .iter_mut()
            .map(|answer| SecretString::from(std::mem::take(answer)))
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn audited(
        message: InteractionReply,
        audit: std::sync::Arc<std::sync::Mutex<Vec<usize>>>,
    ) -> Self {
        Self {
            message,
            audit: Some(audit),
        }
    }
}

impl Drop for ProtectedReply {
    fn drop(&mut self) {
        #[cfg(test)]
        let remaining = self
            .message
            .answers
            .iter()
            .filter(|s| !s.is_empty())
            .count();
        self.message.answers.zeroize();
        #[cfg(test)]
        if let Some(audit) = &self.audit {
            assert!(self.message.answers.is_empty());
            audit
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(remaining);
        }
    }
}
