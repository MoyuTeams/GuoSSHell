#![allow(clippy::expect_used)]

use std::sync::Arc;

use base64ct::{Base64UrlUnpadded, Encoding as _};
use p256::ecdsa::signature::Verifier as _;
use p256::ecdsa::{Signature, VerifyingKey};
use rshell_m0::rshell_session::ExternalSigner as _;
use rshell_m0::russh::keys::PublicKey;
use rshell_m0::russh::keys::ssh_key::public::KeyData;
use rshell_m0::russh::keys::ssh_key::sha2::{Digest as _, Sha256};
use tokio::sync::mpsc;

use super::virtual_key::VirtualSecurityKey;
use super::{
    Assertion, Authenticator, OPENSSH_APPLICATION, Registration, SecurityKeySigner, SkFailure,
    public_key, webauthn_signature,
};
use crate::external_signer::SignerRequest;

const SEED: [u8; 32] = [9; 32];
const DATA: &[u8] = b"session identifier and userauth request";

fn virtual_key() -> VirtualSecurityKey {
    VirtualSecurityKey::new(SEED).expect("valid seed")
}

fn registered(key: &VirtualSecurityKey) -> (Registration, PublicKey) {
    let registration = key.register(OPENSSH_APPLICATION, "me").expect("register");
    let public_key = public_key(&registration, OPENSSH_APPLICATION).expect("public key");
    (registration, public_key)
}

/// SSH 数据的读取游标。
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn bytes(&mut self, length: usize) -> &'a [u8] {
        let (head, rest) = self.0.split_at(length);
        self.0 = rest;
        head
    }

    fn u8(&mut self) -> u8 {
        self.bytes(1)[0]
    }

    fn u32(&mut self) -> u32 {
        u32::from_be_bytes(self.bytes(4).try_into().expect("u32"))
    }

    fn string(&mut self) -> &'a [u8] {
        let length = self.u32() as usize;
        self.bytes(length)
    }
}

/// 照 OpenSSH（`ssh-ecdsa-sk.c` 的 WebAuthn 分支）验证签名 blob：拆出各字段，核对
/// clientDataJSON 的开头与标志位，重建被签的数据后验 ECDSA。
fn sshd_verifies(public_key: &PublicKey, data: &[u8], blob: &[u8]) -> Result<(u8, u32), String> {
    let KeyData::SkEcdsaSha2NistP256(sk) = public_key.key_data() else {
        return Err("not an sk-ecdsa key".into());
    };
    let mut reader = Reader(blob);
    if reader.string() != b"webauthn-sk-ecdsa-sha2-nistp256@openssh.com" {
        return Err("signature type".into());
    }
    let mut inner = Reader(reader.string());
    let (r, s) = (inner.string(), inner.string());
    let flags = reader.u8();
    let counter = reader.u32();
    let origin = std::str::from_utf8(reader.string()).map_err(|error| error.to_string())?;
    let client_data = reader.string();
    let extensions = reader.string();
    if !reader.0.is_empty() || !inner.0.is_empty() {
        return Err("trailing data".into());
    }
    if origin.contains('"') || flags & 0x40 != 0 || (flags & 0x80 == 0) != extensions.is_empty() {
        return Err("origin or flags".into());
    }
    let preamble = format!(
        r#"{{"type":"webauthn.get","challenge":"{}","origin":"{origin}""#,
        Base64UrlUnpadded::encode_string(data)
    );
    if !client_data.starts_with(preamble.as_bytes()) {
        return Err("clientDataJSON preamble".into());
    }
    // mpint：去掉为正数补的前导 0，左侧补齐到 32 字节。
    let scalar = |mpint: &[u8]| {
        let trimmed = mpint.strip_prefix(&[0]).unwrap_or(mpint);
        let mut out = [0; 32];
        out[32 - trimmed.len()..].copy_from_slice(trimmed);
        out
    };
    let signature = Signature::from_scalars(scalar(r), scalar(s)).map_err(|e| e.to_string())?;
    let mut signed = Sha256::digest(sk.application().as_bytes()).to_vec();
    signed.push(flags);
    signed.extend_from_slice(&counter.to_be_bytes());
    signed.extend_from_slice(extensions);
    signed.extend_from_slice(&Sha256::digest(client_data));
    let key = VerifyingKey::from_sec1_bytes(sk.ec_point().as_bytes()).map_err(|e| e.to_string())?;
    key.verify(&signed, &signature)
        .map_err(|error| error.to_string())?;
    Ok((flags, counter))
}

#[test]
fn registration_gives_an_sk_ecdsa_key_bound_to_the_rp_id() {
    let key = virtual_key();
    let (registration, public_key) = registered(&key);
    let KeyData::SkEcdsaSha2NistP256(sk) = public_key.key_data() else {
        panic!("expected sk-ecdsa, got {:?}", public_key.algorithm());
    };
    assert_eq!(sk.application(), OPENSSH_APPLICATION);
    assert_eq!(
        public_key.algorithm().as_str(),
        "sk-ecdsa-sha2-nistp256@openssh.com"
    );
    assert_eq!(
        super::public_key(&registration, "example.com").err(),
        Some(SkFailure::Invalid(
            "authData is for another relying party".into()
        ))
    );
}

#[test]
fn an_assertion_becomes_a_signature_sshd_accepts() {
    let key = virtual_key();
    let (registration, public_key) = registered(&key);
    for expected_counter in 1..=2 {
        let assertion = key
            .assert(OPENSSH_APPLICATION, &registration.credential_id, DATA)
            .expect("assert");
        let blob =
            webauthn_signature(&assertion, OPENSSH_APPLICATION, DATA).expect("signature blob");
        let (flags, counter) = sshd_verifies(&public_key, DATA, &blob).expect("sshd verifies");
        assert_eq!(flags & 0x01, 0x01, "user presence");
        assert_eq!(counter, expected_counter);
        assert!(
            sshd_verifies(&public_key, b"other data", &blob).is_err(),
            "bound to the signed data"
        );
    }
}

#[test]
fn a_domain_relying_party_becomes_the_key_application() {
    let key = virtual_key().with_relying_party("ssh.example.com");
    let rp_id = key.relying_party().expect("relying party");
    let registration = key.register(&rp_id, "me").expect("register");
    let public_key = public_key(&registration, &rp_id).expect("public key");
    let KeyData::SkEcdsaSha2NistP256(sk) = public_key.key_data() else {
        panic!("expected sk-ecdsa");
    };
    assert_eq!(sk.application(), "ssh.example.com");
    let assertion = key
        .assert(&rp_id, &registration.credential_id, DATA)
        .expect("assert");
    let blob = webauthn_signature(&assertion, &rp_id, DATA).expect("signature blob");
    sshd_verifies(&public_key, DATA, &blob).expect("sshd verifies");
}

#[test]
fn assertions_sshd_would_refuse_are_not_handed_over() {
    let key = virtual_key();
    let (registration, _) = registered(&key);
    let assertion = key
        .assert(OPENSSH_APPLICATION, &registration.credential_id, DATA)
        .expect("assert");
    let refused = |assertion: &Assertion, rp_id: &str, data: &[u8]| {
        matches!(
            webauthn_signature(assertion, rp_id, data),
            Err(SkFailure::Invalid(_))
        )
    };

    assert!(
        refused(&assertion, OPENSSH_APPLICATION, b"other data"),
        "another challenge"
    );
    assert!(
        refused(&assertion, "example.com", DATA),
        "another relying party"
    );

    let reordered = String::from_utf8(assertion.client_data_json.clone())
        .expect("utf-8")
        .replacen(r#""type":"webauthn.get","#, "", 1)
        .replacen('}', r#","type":"webauthn.get"}"#, 1);
    let reordered = Assertion {
        client_data_json: reordered.into_bytes(),
        authenticator_data: assertion.authenticator_data.clone(),
        signature: assertion.signature.clone(),
    };
    assert!(
        refused(&reordered, OPENSSH_APPLICATION, DATA),
        "fields in another order"
    );

    let mut extensions_flag = assertion.authenticator_data.clone();
    extensions_flag[32] |= 0x80;
    let flagged = Assertion {
        authenticator_data: extensions_flag,
        client_data_json: assertion.client_data_json.clone(),
        signature: assertion.signature.clone(),
    };
    assert!(
        refused(&flagged, OPENSSH_APPLICATION, DATA),
        "ED flag without extensions"
    );
}

struct Refusing(SkFailure);

impl Authenticator for Refusing {
    fn relying_party(&self) -> Option<String> {
        Some(OPENSSH_APPLICATION.to_owned())
    }

    fn register(&self, _rp_id: &str, _user_name: &str) -> Result<Registration, SkFailure> {
        Err(self.0.clone())
    }

    fn assert(
        &self,
        _rp_id: &str,
        _credential_id: &[u8],
        _challenge: &[u8],
    ) -> Result<Assertion, SkFailure> {
        Err(self.0.clone())
    }
}

#[tokio::test]
async fn the_signer_waits_for_the_user_and_signs() {
    let key = Arc::new(virtual_key());
    let (registration, public_key) = registered(&key);
    let (requests, mut received) = mpsc::unbounded_channel();
    let signer = SecurityKeySigner::new(
        key,
        1,
        String::new(),
        OPENSSH_APPLICATION.to_owned(),
        registration.credential_id,
        requests,
    );

    let blob = signer.sign(DATA, None).await.expect("signature");
    sshd_verifies(&public_key, DATA, &blob).expect("sshd verifies");
    assert!(matches!(
        received.try_recv(),
        Ok(SignerRequest::Waiting(true))
    ));
    assert!(matches!(
        received.try_recv(),
        Ok(SignerRequest::Waiting(false))
    ));
    assert_eq!(signer.failure(), None);
}

#[tokio::test]
async fn the_signer_keeps_the_reason_it_could_not_sign() {
    for failure in [SkFailure::Cancelled, SkFailure::Failed("timeout".into())] {
        let (requests, mut received) = mpsc::unbounded_channel();
        let signer = SecurityKeySigner::new(
            Arc::new(Refusing(failure.clone())),
            1,
            String::new(),
            OPENSSH_APPLICATION.to_owned(),
            vec![1, 2, 3],
            requests,
        );
        assert!(signer.sign(DATA, None).await.is_err());
        assert_eq!(signer.failure(), Some(failure));
        assert!(
            matches!(received.try_recv(), Ok(SignerRequest::Waiting(true)))
                && matches!(received.try_recv(), Ok(SignerRequest::Waiting(false))),
            "waiting ends even when signing fails"
        );
    }

    let (requests, _received) = mpsc::unbounded_channel();
    let signer = SecurityKeySigner::new(
        Arc::new(virtual_key()),
        1,
        String::new(),
        OPENSSH_APPLICATION.to_owned(),
        b"another credential".to_vec(),
        requests,
    );
    assert!(signer.sign(DATA, None).await.is_err());
    assert!(matches!(signer.failure(), Some(SkFailure::Failed(_))));
}
