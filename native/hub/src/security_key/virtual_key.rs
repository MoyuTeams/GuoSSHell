//! 软件模拟的安全密钥：一把 P-256 凭据，按 WebAuthn 的格式给出注册结果与断言
//! （clientDataJSON 的字段顺序与浏览器、系统一致）。测试用；debug 构建里设了环境变量
//! `GUOSH_VIRTUAL_SECURITY_KEY` 时代替系统的安全密钥，用来在模拟器上走通整条流程。
//! 它不受「RP ID 须是关联域名」的限制，RP ID 可以是 `ssh:`，也可以是任意域名。

use std::sync::atomic::{AtomicU32, Ordering};

use base64ct::{Base64UrlUnpadded, Encoding as _};
use ciborium::Value;
use p256::ecdsa::signature::Signer as _;
use p256::ecdsa::{Signature, SigningKey};
use rshell_m0::russh::keys::ssh_key::sha2::{Digest as _, Sha256};

use super::{Assertion, Authenticator, OPENSSH_APPLICATION, Registration, SkFailure};

/// debug 构建里模拟安全密钥的固定种子：凭据跨启动不变，验收用的服务器只需授权一次。
#[cfg(debug_assertions)]
const DEBUG_SEED: [u8; 32] = *b"GuoSSHell virtual security key!!";

const FLAG_USER_PRESENT: u8 = 0x01;
const FLAG_ATTESTED: u8 = 0x40;

pub struct VirtualSecurityKey {
    key: SigningKey,
    credential_id: Vec<u8>,
    counter: AtomicU32,
    relying_party: String,
}

impl VirtualSecurityKey {
    /// RP ID 为 `ssh:` 的模拟安全密钥。
    pub fn new(seed: [u8; 32]) -> Option<Self> {
        let key = SigningKey::from_slice(&seed).ok()?;
        let credential_id = Sha256::digest(seed).to_vec();
        Some(Self {
            key,
            credential_id,
            counter: AtomicU32::new(0),
            relying_party: OPENSSH_APPLICATION.to_owned(),
        })
    }

    pub fn with_relying_party(mut self, relying_party: &str) -> Self {
        relying_party.clone_into(&mut self.relying_party);
        self
    }

    /// 环境变量的值是域名（含 `.`）时用它作 RP ID，否则用 `ssh:`。
    #[cfg(debug_assertions)]
    pub fn for_debug(value: &str) -> std::sync::Arc<dyn Authenticator> {
        match Self::new(DEBUG_SEED) {
            Some(key) if value.contains('.') => std::sync::Arc::new(key.with_relying_party(value)),
            Some(key) => std::sync::Arc::new(key),
            None => std::sync::Arc::new(super::Unavailable),
        }
    }

    /// rpIdHash ‖ flags ‖ signCount。
    fn authenticator_data(&self, rp_id: &str, flags: u8, counter: u32) -> Vec<u8> {
        let mut data = Sha256::digest(rp_id.as_bytes()).to_vec();
        data.push(flags);
        data.extend_from_slice(&counter.to_be_bytes());
        data
    }
}

impl Authenticator for VirtualSecurityKey {
    fn relying_party(&self) -> Option<String> {
        Some(self.relying_party.clone())
    }

    fn register(&self, rp_id: &str, _user_name: &str) -> Result<Registration, SkFailure> {
        let point = self.key.verifying_key().to_sec1_point(false);
        let point = point.as_bytes();
        let (x, y) = point
            .get(1..33)
            .zip(point.get(33..65))
            .ok_or_else(|| SkFailure::Failed("P-256 point".into()))?;
        let cose = Value::Map(vec![
            (Value::from(1), Value::from(2)),
            (Value::from(3), Value::from(-7)),
            (Value::from(-1), Value::from(1)),
            (Value::from(-2), Value::Bytes(x.to_vec())),
            (Value::from(-3), Value::Bytes(y.to_vec())),
        ]);
        let mut auth_data = self.authenticator_data(rp_id, FLAG_USER_PRESENT | FLAG_ATTESTED, 0);
        auth_data.extend_from_slice(&[0; 16]);
        auth_data.extend_from_slice(
            &u16::try_from(self.credential_id.len())
                .unwrap_or(u16::MAX)
                .to_be_bytes(),
        );
        auth_data.extend_from_slice(&self.credential_id);
        ciborium::into_writer(&cose, &mut auth_data)
            .map_err(|error| SkFailure::Failed(error.to_string()))?;
        let object = Value::Map(vec![
            (Value::from("fmt"), Value::from("none")),
            (Value::from("attStmt"), Value::Map(Vec::new())),
            (Value::from("authData"), Value::Bytes(auth_data)),
        ]);
        let mut attestation_object = Vec::new();
        ciborium::into_writer(&object, &mut attestation_object)
            .map_err(|error| SkFailure::Failed(error.to_string()))?;
        Ok(Registration {
            credential_id: self.credential_id.clone(),
            attestation_object,
        })
    }

    fn assert(
        &self,
        rp_id: &str,
        credential_id: &[u8],
        challenge: &[u8],
    ) -> Result<Assertion, SkFailure> {
        if credential_id != self.credential_id {
            return Err(SkFailure::Failed("unknown credential".into()));
        }
        let counter = self.counter.fetch_add(1, Ordering::SeqCst) + 1;
        let authenticator_data = self.authenticator_data(rp_id, FLAG_USER_PRESENT, counter);
        let client_data_json = format!(
            r#"{{"type":"webauthn.get","challenge":"{}","origin":"https://{rp_id}","crossOrigin":false}}"#,
            Base64UrlUnpadded::encode_string(challenge)
        )
        .into_bytes();
        let mut signed = authenticator_data.clone();
        signed.extend_from_slice(&Sha256::digest(&client_data_json));
        let signature: Signature = self.key.sign(&signed);
        Ok(Assertion {
            authenticator_data,
            client_data_json,
            signature: signature.to_der().as_bytes().to_vec(),
        })
    }
}
