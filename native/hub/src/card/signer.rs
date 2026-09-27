//! 卡签名接进 SSH 认证：实现上游的 `ExternalSigner`（rsHell 补丁 P5）。
//!
//! 服务器接受了公钥、要签名时：先确认卡在、认证槽的公钥就是登记的那把，再问 PIN
//! （经会话的连接循环弹给用户）、让卡签名；PIN 错了带剩余次数重问。卡不在而设备能用
//! NFC 时，先问 PIN、再弹系统的 NFC 界面一次完成（NFC 界面会盖住 App）。
//! 失败原因留在签名器里，连接失败后由会话取回、给出具体提示。

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rinf::debug_print;
use rshell_m0::rshell_session::{ExternalSigner, ExternalSignerError};
use rshell_m0::russh::keys::{HashAlg, PublicKey};
use secrecy::SecretString;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;
use tokio::task::spawn_blocking;

use super::{CardContext, CardFailure, lock};
use crate::external_signer::{PinQuestion, SignerRequest, send_hint};
use crate::signals::ConnectHint;

pub struct CardSigner {
    cards: Arc<CardContext>,
    session_id: u32,
    /// 连接中状态里显示的目标。
    target: String,
    key_name: String,
    ident: String,
    public_key: PublicKey,
    requests: UnboundedSender<SignerRequest>,
    failure: Mutex<Option<CardFailure>>,
}

impl CardSigner {
    pub fn new(
        cards: Arc<CardContext>,
        session_id: u32,
        target: String,
        key_name: String,
        ident: String,
        public_key: PublicKey,
        requests: UnboundedSender<SignerRequest>,
    ) -> Self {
        Self {
            cards,
            session_id,
            target,
            key_name,
            ident,
            public_key,
            requests,
            failure: Mutex::new(None),
        }
    }

    /// 签名失败的原因（连接失败后取）。
    pub fn failure(&self) -> Option<CardFailure> {
        lock(&self.failure).clone()
    }

    async fn try_sign(&self, data: &[u8], hash: Option<HashAlg>) -> Result<Vec<u8>, CardFailure> {
        let probe = {
            let cards = self.cards.clone();
            let ident = self.ident.clone();
            let key = self.public_key.clone();
            spawn_blocking(move || cards.probe(&ident, &key))
                .await
                .map_err(|error| CardFailure::Io(error.to_string()))?
        };
        let (nfc, mut tries_left) = match probe {
            Ok(info) => (false, Some(info.pin_tries_left)),
            Err(CardFailure::NotFound) if self.cards.reader.nfc_supported() => (true, None),
            Err(failure) => return Err(failure),
        };
        if tries_left == Some(0) {
            return Err(CardFailure::PinBlocked);
        }

        let mut retry = false;
        let mut provided = None;
        loop {
            if nfc {
                send_hint(self.session_id, &self.target, ConnectHint::TapCard);
            }
            let _ = self.requests.send(SignerRequest::Waiting(true));
            let result = {
                let cards = self.cards.clone();
                let ident = self.ident.clone();
                let key = self.public_key.clone();
                let pin = provided.take();
                let data = data.to_vec();
                let session_id = self.session_id;
                let target = self.target.clone();
                spawn_blocking(move || {
                    cards.sign_with_pin(&ident, &key, pin, &data, hash, nfc, &|touch| {
                        if touch {
                            send_hint(session_id, &target, ConnectHint::TouchCard);
                        }
                    })
                })
                .await
                .map_err(|error| CardFailure::Io(error.to_string()))?
            };
            let _ = self.requests.send(SignerRequest::Waiting(false));
            send_hint(self.session_id, &self.target, ConnectHint::None);
            match result {
                Ok(Some(blob)) => return Ok(blob),
                Ok(None) => provided = Some(self.ask_pin(tries_left, retry).await?),
                Err(CardFailure::PinWrong { tries_left: left }) => {
                    tries_left = Some(left);
                    retry = true;
                    provided = Some(self.ask_pin(tries_left, retry).await?);
                }
                Err(failure) => return Err(failure),
            }
        }
    }

    async fn ask_pin(
        &self,
        tries_left: Option<u8>,
        retry: bool,
    ) -> Result<(SecretString, bool), CardFailure> {
        let (reply, answer) = oneshot::channel();
        let question = PinQuestion {
            key_name: self.key_name.clone(),
            tries_left,
            retry,
        };
        self.requests
            .send(SignerRequest::Pin { question, reply })
            .map_err(|_| CardFailure::Cancelled)?;
        answer.await.ok().flatten().ok_or(CardFailure::Cancelled)
    }
}

#[async_trait]
impl ExternalSigner for CardSigner {
    async fn sign(
        &self,
        data: &[u8],
        hash: Option<HashAlg>,
    ) -> Result<Vec<u8>, ExternalSignerError> {
        self.try_sign(data, hash).await.map_err(|failure| {
            debug_print!("[card] sign: {failure:?}");
            *lock(&self.failure) = Some(failure);
            ExternalSignerError
        })
    }
}
