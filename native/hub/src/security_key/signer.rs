//! 安全密钥签名接进 SSH 认证：实现上游的 `ExternalSigner`（rsHell 补丁 P5）。
//! 系统界面引导用户插上、靠近或触摸安全密钥；这段时间连接状态里带上提示，并告诉连接循环
//! 「在等用户」，不算进连接时限。失败原因留在签名器里，连接失败后由会话取回。

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rinf::debug_print;
use rshell_m0::rshell_session::{ExternalSigner, ExternalSignerError};
use rshell_m0::russh::keys::HashAlg;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::spawn_blocking;

use super::{Authenticator, SkFailure, webauthn_signature};
use crate::external_signer::{SignerRequest, send_hint};
use crate::signals::ConnectHint;

pub struct SecurityKeySigner {
    authenticator: Arc<dyn Authenticator>,
    session_id: u32,
    /// 连接中状态里显示的目标。
    target: String,
    rp_id: String,
    credential_id: Vec<u8>,
    requests: UnboundedSender<SignerRequest>,
    failure: Mutex<Option<SkFailure>>,
}

impl SecurityKeySigner {
    pub fn new(
        authenticator: Arc<dyn Authenticator>,
        session_id: u32,
        target: String,
        rp_id: String,
        credential_id: Vec<u8>,
        requests: UnboundedSender<SignerRequest>,
    ) -> Self {
        Self {
            authenticator,
            session_id,
            target,
            rp_id,
            credential_id,
            requests,
            failure: Mutex::new(None),
        }
    }

    /// 签名失败的原因（连接失败后取）。
    pub fn failure(&self) -> Option<SkFailure> {
        self.failure
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    async fn try_sign(&self, data: &[u8]) -> Result<Vec<u8>, SkFailure> {
        let authenticator = self.authenticator.clone();
        let rp_id = self.rp_id.clone();
        let credential_id = self.credential_id.clone();
        let challenge = data.to_vec();
        let _ = self.requests.send(SignerRequest::Waiting(true));
        send_hint(self.session_id, &self.target, ConnectHint::SecurityKey);
        let assertion =
            spawn_blocking(move || authenticator.assert(&rp_id, &credential_id, &challenge)).await;
        send_hint(self.session_id, &self.target, ConnectHint::None);
        let _ = self.requests.send(SignerRequest::Waiting(false));
        let assertion = assertion.map_err(|error| SkFailure::Failed(error.to_string()))??;
        webauthn_signature(&assertion, &self.rp_id, data)
    }
}

#[async_trait]
impl ExternalSigner for SecurityKeySigner {
    async fn sign(
        &self,
        data: &[u8],
        _hash: Option<HashAlg>,
    ) -> Result<Vec<u8>, ExternalSignerError> {
        self.try_sign(data).await.map_err(|failure| {
            debug_print!("[security key] sign: {failure:?}");
            *self
                .failure
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = Some(failure);
            ExternalSignerError
        })
    }
}
