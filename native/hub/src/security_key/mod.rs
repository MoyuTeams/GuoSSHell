//! 安全密钥（FIDO2 / WebAuthn，M3d）：私钥在安全密钥里，SSH 公钥类型是
//! `sk-ecdsa-sha2-nistp256@openssh.com`，其中的 application 就是 WebAuthn 的 RP ID。
//!
//! iOS / macOS 只提供 WebAuthn 层的接口（AuthenticationServices）：clientDataJSON 由系统生成，
//! 安全密钥签的是 `authenticatorData ‖ SHA256(clientDataJSON)`，challenge 取 SSH 要签的数据。
//! OpenSSH 为这种签名定义了 `webauthn-sk-ecdsa-sha2-nistp256@openssh.com`（附上 origin 与
//! clientDataJSON，服务器据此重算被签的数据）。
//!
//! 系统只接受与 App 关联的域名（Associated Domains 的 `webcredentials`）作 RP ID，
//! OpenSSH 默认的 `ssh:` 用不了；RP ID 由构建配置给出，没配置就不提供安全密钥。

#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;
mod signer;
#[cfg(any(test, debug_assertions))]
pub mod virtual_key;

use std::sync::Arc;

use base64ct::{Base64UrlUnpadded, Encoding as _};
use ciborium::Value;
use p256::ecdsa::Signature as EcdsaSignature;
use rshell_m0::russh::keys::PublicKey;
use rshell_m0::russh::keys::ssh_key::Mpint;
use rshell_m0::russh::keys::ssh_key::public::{EcdsaPublicKey, KeyData, SkEcdsaSha2NistP256};
use rshell_m0::russh::keys::ssh_key::sha2::{Digest as _, Sha256};

pub use signer::SecurityKeySigner;

/// OpenSSH 生成安全密钥时默认的 application（模拟安全密钥默认也用它）。
pub const OPENSSH_APPLICATION: &str = "ssh:";
/// OpenSSH 的 WebAuthn 签名格式名。
const WEBAUTHN_SIGNATURE: &str = "webauthn-sk-ecdsa-sha2-nistp256@openssh.com";
/// authenticatorData 的标志位。
const FLAG_ATTESTED: u8 = 0x40;
const FLAG_EXTENSIONS: u8 = 0x80;

/// 新建凭据的结果（WebAuthn 注册）。
pub struct Registration {
    pub credential_id: Vec<u8>,
    /// CBOR 编码的 attestation object。
    pub attestation_object: Vec<u8>,
}

/// 一次签名（WebAuthn 断言）。
pub struct Assertion {
    pub authenticator_data: Vec<u8>,
    pub client_data_json: Vec<u8>,
    /// DER 编码的 ECDSA 签名。
    pub signature: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkFailure {
    /// 这台设备（或这个构建）用不了安全密钥。
    Unavailable,
    /// 用户取消了系统的安全密钥界面。
    #[cfg_attr(not(any(target_os = "ios", target_os = "macos")), allow(dead_code))]
    Cancelled,
    /// 安全密钥给的数据不对（或不是 OpenSSH 能验证的格式）。
    Invalid(String),
    Failed(String),
}

/// 安全密钥的来源：系统的 AuthenticationServices，或测试与 debug 构建里的模拟安全密钥。
/// 调用会等用户操作（插上、靠近、触摸），只在阻塞线程上调。
pub trait Authenticator: Send + Sync {
    /// 新建凭据用的 RP ID（即 SSH 公钥的 application）；`None` = 用不了安全密钥。
    fn relying_party(&self) -> Option<String>;
    /// 在安全密钥上新建一把 ES256 凭据。
    fn register(&self, rp_id: &str, user_name: &str) -> Result<Registration, SkFailure>;
    /// 用凭据对 `challenge` 签名。
    fn assert(
        &self,
        rp_id: &str,
        credential_id: &[u8],
        challenge: &[u8],
    ) -> Result<Assertion, SkFailure>;
}

/// 用不了安全密钥的平台。
pub struct Unavailable;

impl Authenticator for Unavailable {
    fn relying_party(&self) -> Option<String> {
        None
    }

    fn register(&self, _rp_id: &str, _user_name: &str) -> Result<Registration, SkFailure> {
        Err(SkFailure::Unavailable)
    }

    fn assert(
        &self,
        _rp_id: &str,
        _credential_id: &[u8],
        _challenge: &[u8],
    ) -> Result<Assertion, SkFailure> {
        Err(SkFailure::Unavailable)
    }
}

/// 本平台的安全密钥。debug 构建里设了 `GUOSH_VIRTUAL_SECURITY_KEY` 时换成模拟安全密钥
/// （模拟器没有安全密钥）：值是域名时用它作 RP ID，否则用 `ssh:`。
pub fn platform_authenticator() -> Arc<dyn Authenticator> {
    #[cfg(debug_assertions)]
    if let Some(value) = std::env::var_os("GUOSH_VIRTUAL_SECURITY_KEY") {
        return virtual_key::VirtualSecurityKey::for_debug(&value.to_string_lossy());
    }
    #[cfg(any(target_os = "ios", target_os = "macos"))]
    {
        Arc::new(apple::AppleSecurityKeys)
    }
    #[cfg(not(any(target_os = "ios", target_os = "macos")))]
    {
        Arc::new(Unavailable)
    }
}

fn invalid(what: impl std::fmt::Display) -> SkFailure {
    SkFailure::Invalid(what.to_string())
}

/// 注册结果 → SSH 公钥：从 attestation object 的 authData 里取出 COSE 格式的 P-256 公钥。
/// authData 的 rpIdHash 必须是 SHA256(RP ID)——它就是以后签名时服务器按 application 重算的值。
pub fn public_key(registration: &Registration, rp_id: &str) -> Result<PublicKey, SkFailure> {
    let object: Value = ciborium::from_reader(registration.attestation_object.as_slice())
        .map_err(|error| invalid(format!("attestation object: {error}")))?;
    let auth_data = text_entry(&object, "authData")
        .and_then(Value::as_bytes)
        .ok_or_else(|| invalid("attestation object without authData"))?;
    // rpIdHash(32) flags(1) signCount(4) aaguid(16) credentialIdLength(2) credentialId COSE_Key
    let (rp_id_hash, rest) = auth_data
        .split_first_chunk::<32>()
        .ok_or_else(|| invalid("short authData"))?;
    if rp_id_hash[..] != Sha256::digest(rp_id.as_bytes())[..] {
        return Err(invalid("authData is for another relying party"));
    }
    let flags = *rest.first().ok_or_else(|| invalid("short authData"))?;
    if flags & FLAG_ATTESTED == 0 {
        return Err(invalid("authData without attested credential data"));
    }
    let length = rest
        .get(21..23)
        .map(|bytes| usize::from(u16::from_be_bytes([bytes[0], bytes[1]])))
        .ok_or_else(|| invalid("short authData"))?;
    let cose = rest
        .get(23 + length..)
        .ok_or_else(|| invalid("short authData"))?;
    let cose: Value =
        ciborium::from_reader(cose).map_err(|error| invalid(format!("COSE key: {error}")))?;
    // EC2（kty 2）、ES256（alg -7）、P-256（crv 1）。
    let (Some(2), Some(-7), Some(1)) = (
        int_entry(&cose, 1),
        int_entry(&cose, 3),
        int_entry(&cose, -1),
    ) else {
        return Err(invalid("not an ES256 P-256 credential"));
    };
    let (Some(x), Some(y)) = (bytes_entry(&cose, -2), bytes_entry(&cose, -3)) else {
        return Err(invalid("COSE key without coordinates"));
    };
    let mut point = Vec::with_capacity(65);
    point.push(0x04);
    point.extend_from_slice(x);
    point.extend_from_slice(y);
    let EcdsaPublicKey::NistP256(point) = EcdsaPublicKey::from_sec1_bytes(&point)
        .map_err(|error| invalid(format!("P-256 point: {error}")))?
    else {
        return Err(invalid("not a P-256 point"));
    };
    Ok(PublicKey::new(
        KeyData::SkEcdsaSha2NistP256(SkEcdsaSha2NistP256::new(point, rp_id)),
        "",
    ))
}

/// 断言 → OpenSSH 的 WebAuthn 签名 blob：
/// `string(格式名) ‖ string(mpint r ‖ mpint s) ‖ byte flags ‖ uint32 counter ‖ string(origin) ‖
/// string(clientDataJSON) ‖ string(extensions)`。
///
/// 服务器会按 `{"type":"webauthn.get","challenge":"<base64url(数据)>","origin":"<origin>"` 重建
/// clientDataJSON 的开头并逐字节比较，这里先照同样的规则检查，格式不符就不交出去。
pub fn webauthn_signature(
    assertion: &Assertion,
    rp_id: &str,
    data: &[u8],
) -> Result<Vec<u8>, SkFailure> {
    let (rp_id_hash, rest) = assertion
        .authenticator_data
        .split_first_chunk::<32>()
        .ok_or_else(|| invalid("short authenticatorData"))?;
    if rp_id_hash[..] != Sha256::digest(rp_id.as_bytes())[..] {
        return Err(invalid("authenticatorData is for another relying party"));
    }
    let (&flags, rest) = rest
        .split_first()
        .ok_or_else(|| invalid("short authenticatorData"))?;
    let (counter, extensions) = rest
        .split_first_chunk::<4>()
        .ok_or_else(|| invalid("short authenticatorData"))?;
    if flags & FLAG_ATTESTED != 0 || (flags & FLAG_EXTENSIONS != 0) == extensions.is_empty() {
        return Err(invalid("unexpected authenticatorData flags"));
    }

    let client_data: serde_json::Value = serde_json::from_slice(&assertion.client_data_json)
        .map_err(|error| invalid(format!("clientDataJSON: {error}")))?;
    let origin = client_data
        .get("origin")
        .and_then(serde_json::Value::as_str)
        .filter(|origin| !origin.contains('"'))
        .ok_or_else(|| invalid("clientDataJSON without a usable origin"))?;
    let expected = format!(
        r#"{{"type":"webauthn.get","challenge":"{}","origin":"{origin}""#,
        Base64UrlUnpadded::encode_string(data)
    );
    if !assertion.client_data_json.starts_with(expected.as_bytes()) {
        return Err(invalid(
            "clientDataJSON is not in the form OpenSSH verifies",
        ));
    }

    let signature = EcdsaSignature::from_der(&assertion.signature)
        .map_err(|error| invalid(format!("ECDSA signature: {error}")))?;
    let (r, s) = signature.split_bytes();
    let mut inner = Vec::with_capacity(72);
    push_string(&mut inner, Mpint::from_positive_bytes(&r).as_bytes());
    push_string(&mut inner, Mpint::from_positive_bytes(&s).as_bytes());

    let mut blob = Vec::new();
    push_string(&mut blob, WEBAUTHN_SIGNATURE.as_bytes());
    push_string(&mut blob, &inner);
    blob.push(flags);
    blob.extend_from_slice(counter);
    push_string(&mut blob, origin.as_bytes());
    push_string(&mut blob, &assertion.client_data_json);
    push_string(&mut blob, extensions);
    Ok(blob)
}

fn text_entry<'a>(map: &'a Value, key: &str) -> Option<&'a Value> {
    map.as_map()?
        .iter()
        .find(|(entry, _)| entry.as_text() == Some(key))
        .map(|(_, value)| value)
}

fn int_entry(map: &Value, key: i128) -> Option<i128> {
    map.as_map()?
        .iter()
        .find(|(entry, _)| entry.as_integer().map(i128::from) == Some(key))
        .and_then(|(_, value)| value.as_integer())
        .map(i128::from)
}

fn bytes_entry(map: &Value, key: i128) -> Option<&[u8]> {
    map.as_map()?
        .iter()
        .find(|(entry, _)| entry.as_integer().map(i128::from) == Some(key))
        .and_then(|(_, value)| value.as_bytes())
        .map(Vec::as_slice)
}

/// SSH 的 string：u32 长度 + 内容。
fn push_string(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&u32::try_from(bytes.len()).unwrap_or(u32::MAX).to_be_bytes());
    out.extend_from_slice(bytes);
}

#[cfg(test)]
mod tests;
