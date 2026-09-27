//! OpenPGP 卡（M3b / M3c）：用卡上**认证槽**的密钥做 SSH 公钥认证——VERIFY PW1（P2=82）后
//! INTERNAL AUTHENTICATE。Ed25519 直接签 SSH 要签的数据；RSA 送 DigestInfo（卡做 PKCS#1
//! 填充，`rsa-sha2-256/512`，旧服务器 `ssh-rsa`）；ECDSA（NIST P-256/384/521）送摘要，卡回
//! 定长 r‖s，拆开转成 SSH 的 mpint。私钥从不离开卡。
//!
//! APDU 层是 openpgp-card；卡从哪里来由 [`CardReader`] 决定：CryptoTokenKit（iOS /
//! macOS 的读卡器与 NFC 卡槽）、测试与 debug 构建里的模拟卡。签名经上游的
//! `ExternalSigner`（rsHell 补丁 P5）接进认证，见 [`signer`]。

#[cfg(any(target_os = "ios", target_os = "macos"))]
mod ctk;
mod signer;
#[cfg(any(test, debug_assertions))]
pub mod virtual_card;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use card_backend::CardBackend;
use openpgp_card::Card;
use openpgp_card::ocard::algorithm::{AlgorithmAttributes, Curve};
use openpgp_card::ocard::crypto::{EccType, HashAlgo, PublicKeyMaterial, SigningAlgo};
use openpgp_card::ocard::{KeyType, StatusBytes};
use openpgp_card::state::{Open, Transaction};
use rshell_m0::russh::keys::ssh_key::public::{
    EcdsaPublicKey, Ed25519PublicKey, KeyData, RsaPublicKey,
};
use rshell_m0::russh::keys::ssh_key::sha2::{Digest as _, Sha256, Sha384, Sha512};
use rshell_m0::russh::keys::ssh_key::{EcdsaCurve, Mpint};
use rshell_m0::russh::keys::{HashAlg, PublicKey};
use secrecy::SecretString;

pub use signer::CardSigner;

/// 能提供卡的地方。
pub trait CardReader: Send + Sync {
    /// 现在能用的卡（每个读卡器上一张），尚未打开。
    fn cards(&self) -> Vec<Box<dyn CardBackend + Send + Sync>>;

    /// 这台设备能用 NFC 读卡。
    fn nfc_supported(&self) -> bool {
        false
    }

    /// 弹出系统的 NFC 读卡界面，等卡靠近；返回的会话在 drop 时结束、界面收起。
    /// 会话存续期间，靠近的卡出现在 [`Self::cards`] 里。
    fn begin_nfc(&self, _message: &str) -> Result<Box<dyn NfcSession>, CardFailure> {
        Err(CardFailure::NotFound)
    }
}

/// 一次 NFC 读卡（系统界面）。drop 即结束。
pub trait NfcSession: Send {}

/// 没有卡（测试）。
#[cfg(test)]
pub struct NoCards;

#[cfg(test)]
impl CardReader for NoCards {
    fn cards(&self) -> Vec<Box<dyn CardBackend + Send + Sync>> {
        Vec::new()
    }
}

/// 几个来源合在一起（真实读卡器 + debug 的模拟卡）；NFC 交给第一个支持的。
pub struct Readers(pub Vec<Arc<dyn CardReader>>);

impl CardReader for Readers {
    fn cards(&self) -> Vec<Box<dyn CardBackend + Send + Sync>> {
        self.0.iter().flat_map(|reader| reader.cards()).collect()
    }

    fn nfc_supported(&self) -> bool {
        self.0.iter().any(|reader| reader.nfc_supported())
    }

    fn begin_nfc(&self, message: &str) -> Result<Box<dyn NfcSession>, CardFailure> {
        self.0
            .iter()
            .find(|reader| reader.nfc_supported())
            .ok_or(CardFailure::NotFound)?
            .begin_nfc(message)
    }
}

/// 本平台的卡来源。debug 构建里设了 `GUOSH_VIRTUAL_CARD` 时再加一张模拟卡。
pub fn platform_reader() -> Arc<dyn CardReader> {
    let mut readers: Vec<Arc<dyn CardReader>> = Vec::new();
    #[cfg(any(target_os = "ios", target_os = "macos"))]
    readers.push(Arc::new(ctk::CtkReader));
    #[cfg(debug_assertions)]
    if let Some(kind) = std::env::var_os("GUOSH_VIRTUAL_CARD") {
        readers.push(Arc::new(virtual_card::VirtualCard::for_debug(
            &kind.to_string_lossy(),
        )));
    }
    Arc::new(Readers(readers))
}

/// 卡操作失败的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CardFailure {
    /// 没有找到这张卡（没插、没靠近，或 NFC 读卡被取消）。
    NotFound,
    /// 卡上认证槽的密钥与登记时的不同。
    KeyMismatch,
    /// 认证槽没有密钥，或算法 SSH 不支持（brainpool、secp256k1 等）。
    Unsupported,
    PinWrong {
        tries_left: u8,
    },
    PinBlocked,
    /// 卡要求按键确认，但没有等到。
    TouchTimeout,
    /// 用户取消了 PIN 输入。
    Cancelled,
    /// 其他读卡错误。
    Io(String),
}

impl From<openpgp_card::Error> for CardFailure {
    fn from(error: openpgp_card::Error) -> Self {
        match error {
            openpgp_card::Error::CardStatus(StatusBytes::AuthenticationMethodBlocked) => {
                Self::PinBlocked
            }
            openpgp_card::Error::CardStatus(StatusBytes::SecurityRelatedIssues) => {
                Self::TouchTimeout
            }
            error => Self::Io(error.to_string()),
        }
    }
}

/// 读卡得到的认证槽信息。
#[derive(Debug, Clone)]
pub struct CardInfo {
    /// 卡号（`厂商:序列号`，openpgp-card 的 ident）。
    pub ident: String,
    pub cardholder: String,
    /// 认证槽的算法（界面显示用）。
    pub algorithm: String,
    /// 认证槽的公钥；`None` = 没有密钥，或算法 SSH 不支持。
    pub public_key: Option<PublicKey>,
    /// PIN 还能试几次。
    pub pin_tries_left: u8,
    /// 签名要在卡上按键确认。
    pub touch: bool,
}

/// 进程内的卡状态：卡来源、上次读卡的结果（登记时用）、用户选择记住的 PIN。
pub struct CardContext {
    pub reader: Arc<dyn CardReader>,
    scanned: Mutex<Vec<CardInfo>>,
    pins: Mutex<HashMap<String, SecretString>>,
    /// 同一时刻只有一个操作在用卡（读卡器会话是独占的）。
    busy: Mutex<()>,
}

impl CardContext {
    pub fn new(reader: Arc<dyn CardReader>) -> Self {
        Self {
            reader,
            scanned: Mutex::new(Vec::new()),
            pins: Mutex::new(HashMap::new()),
            busy: Mutex::new(()),
        }
    }

    /// 读一遍现在能用的卡（`nfc` 时先弹 NFC 界面等卡）。结果留着，登记时用。
    pub fn scan(&self, nfc: bool) -> Result<Vec<CardInfo>, CardFailure> {
        let _busy = lock(&self.busy);
        let _nfc = if nfc {
            Some(self.reader.begin_nfc("将 OpenPGP 卡靠近设备")?)
        } else {
            None
        };
        let cards: Vec<CardInfo> = self
            .reader
            .cards()
            .into_iter()
            .filter_map(|backend| {
                match Card::new(backend)
                    .map_err(CardFailure::from)
                    .and_then(|mut card| read_info(&mut card))
                {
                    Ok(info) => Some(info),
                    Err(error) => {
                        rinf::debug_print!("[card] skipping a card: {error:?}");
                        None
                    }
                }
            })
            .collect();
        *lock(&self.scanned) = cards.clone();
        Ok(cards)
    }

    /// 上次读卡看到的这张卡。
    pub fn scanned(&self, ident: &str) -> Option<CardInfo> {
        lock(&self.scanned)
            .iter()
            .find(|card| card.ident == ident)
            .cloned()
    }

    fn remembered_pin(&self, ident: &str) -> Option<SecretString> {
        lock(&self.pins).get(ident).cloned()
    }

    fn remember_pin(&self, ident: &str, pin: SecretString) {
        lock(&self.pins).insert(ident.to_owned(), pin);
    }

    /// PIN 错了或卡锁了：立刻丢掉记住的 PIN，免得再拿它去试、把卡试锁。
    fn forget_pin(&self, ident: &str) {
        lock(&self.pins).remove(ident);
    }

    /// 连接前的检查：卡在不在、认证槽的公钥是否就是登记的那把、PIN 还剩几次。
    fn probe(&self, ident: &str, expected: &PublicKey) -> Result<CardInfo, CardFailure> {
        let _busy = lock(&self.busy);
        let info = read_info(&mut self.open(ident)?)?;
        check_key(&info, expected)?;
        Ok(info)
    }

    /// 缓存 PIN 的取用、验证与失效都在卡锁内完成，排队者不能复用已失败的缓存。
    /// 没有显式输入或有效缓存时返回 `None`，让调用者在锁外询问用户。
    #[allow(clippy::too_many_arguments)]
    fn sign_with_pin(
        &self,
        ident: &str,
        expected: &PublicKey,
        provided: Option<(SecretString, bool)>,
        data: &[u8],
        hash: Option<HashAlg>,
        nfc: bool,
        before_sign: &dyn Fn(bool),
    ) -> Result<Option<Vec<u8>>, CardFailure> {
        let _busy = lock(&self.busy);
        let (pin, remember) = match provided {
            Some(provided) => provided,
            None => match self.remembered_pin(ident) {
                Some(pin) => (pin, false),
                None => return Ok(None),
            },
        };
        let remembered = remember.then(|| pin.clone());
        let result = self.sign_locked(ident, expected, pin, data, hash, nfc, before_sign);
        match &result {
            Ok(_) => {
                if let Some(pin) = remembered {
                    self.remember_pin(ident, pin);
                }
            }
            Err(CardFailure::PinWrong { .. } | CardFailure::PinBlocked) => self.forget_pin(ident),
            Err(_) => {}
        }
        result.map(Some)
    }

    /// 测试直接提供 PIN，仍经过生产代码的卡锁与缓存失效路径。
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    fn sign(
        &self,
        ident: &str,
        expected: &PublicKey,
        pin: SecretString,
        data: &[u8],
        hash: Option<HashAlg>,
        nfc: bool,
        before_sign: &dyn Fn(bool),
    ) -> Result<Vec<u8>, CardFailure> {
        self.sign_with_pin(
            ident,
            expected,
            Some((pin, false)),
            data,
            hash,
            nfc,
            before_sign,
        )?
        .ok_or(CardFailure::Cancelled)
    }

    /// 持有卡锁时验证 PIN 并签名；NFC 会话覆盖整个卡操作。
    #[allow(clippy::too_many_arguments)]
    fn sign_locked(
        &self,
        ident: &str,
        expected: &PublicKey,
        pin: SecretString,
        data: &[u8],
        hash: Option<HashAlg>,
        nfc: bool,
        before_sign: &dyn Fn(bool),
    ) -> Result<Vec<u8>, CardFailure> {
        let _nfc = if nfc {
            Some(self.reader.begin_nfc("将 OpenPGP 卡靠近设备以完成登录")?)
        } else {
            None
        };
        let mut card = self.open(ident)?;
        let info = read_info(&mut card)?;
        check_key(&info, expected)?;
        let mut tx = card.transaction()?;
        let scheme = Scheme::of(&tx.algorithm_attributes(KeyType::Authentication)?)
            .ok_or(CardFailure::Unsupported)?;
        if let Err(error) = tx.verify_user_pin(pin) {
            return Err(pin_failure(&mut tx, error));
        }
        before_sign(info.touch);
        match scheme {
            Scheme::Ed25519 => {
                let signature = tx.card().internal_authenticate(data.to_vec())?;
                if signature.len() != 64 {
                    return Err(CardFailure::Io(format!(
                        "unexpected Ed25519 signature length {}",
                        signature.len()
                    )));
                }
                Ok(ssh_signature("ssh-ed25519", &signature))
            }
            Scheme::Rsa => {
                let (name, algorithm, digest) = rsa_digest(hash, data);
                let signature = tx
                    .card()
                    .authenticate_for_hash(SigningAlgo::RSA(algorithm), &digest)?;
                Ok(ssh_signature(name, &signature))
            }
            Scheme::Ecdsa(curve) => {
                let digest = ecdsa_digest(curve, data);
                let signature = tx.card().authenticate_for_hash(SigningAlgo::ECC, &digest)?;
                ecdsa_signature(curve, &signature)
            }
        }
    }

    /// 找到卡号为 `ident` 的卡（已选中 OpenPGP 应用）。
    fn open(&self, ident: &str) -> Result<Card<Open>, CardFailure> {
        for backend in self.reader.cards() {
            let mut card = match Card::new(backend) {
                Ok(card) => card,
                Err(error) => {
                    rinf::debug_print!("[card] not an OpenPGP card: {error}");
                    continue;
                }
            };
            let found = card
                .transaction()
                .and_then(|tx| tx.application_identifier())
                .is_ok_and(|aid| aid.ident() == ident);
            if found {
                return Ok(card);
            }
        }
        Err(CardFailure::NotFound)
    }
}

/// 认证槽的公钥得是登记的那把（M3b 只认 Ed25519）。
fn check_key(info: &CardInfo, expected: &PublicKey) -> Result<(), CardFailure> {
    match &info.public_key {
        Some(key) if key.key_data() == expected.key_data() => Ok(()),
        Some(_) => Err(CardFailure::KeyMismatch),
        None => Err(CardFailure::Unsupported),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

fn read_info(card: &mut Card<Open>) -> Result<CardInfo, CardFailure> {
    let mut tx = card.transaction()?;
    let ident = tx.application_identifier()?.ident();
    let cardholder = tx.cardholder_name().unwrap_or_default();
    let algorithm = tx.algorithm_attributes(KeyType::Authentication)?;
    let public_key = match Scheme::of(&algorithm) {
        Some(scheme) => tx
            .public_key_material(KeyType::Authentication)
            .ok()
            .and_then(|material| ssh_public_key(&material, scheme, &ident)),
        None => None,
    };
    let pin_tries_left = tx.pw_status_bytes()?.err_count_pw1();
    let touch = tx
        .user_interaction_flag(KeyType::Authentication)?
        .is_some_and(|uif| uif.touch_policy().touch_required());
    Ok(CardInfo {
        ident,
        cardholder,
        algorithm: algorithm_name(&algorithm),
        public_key,
        pin_tries_left,
        touch,
    })
}

/// VERIFY 失败 → 失败原因。卡对错 PIN 回 63Cx（x = 剩余次数），有的卡（YubiKey）回 6982，
/// 那就再读一次 PIN 状态。
fn pin_failure(tx: &mut Card<Transaction<'_>>, error: openpgp_card::Error) -> CardFailure {
    let tries_left = match error {
        openpgp_card::Error::CardStatus(StatusBytes::PasswordNotChecked(tries_left)) => tries_left,
        openpgp_card::Error::CardStatus(StatusBytes::SecurityStatusNotSatisfied) => {
            match tx.invalidate_cache().and_then(|()| tx.pw_status_bytes()) {
                Ok(status) => status.err_count_pw1(),
                Err(error) => return error.into(),
            }
        }
        error => return error.into(),
    };
    if tries_left == 0 {
        CardFailure::PinBlocked
    } else {
        CardFailure::PinWrong { tries_left }
    }
}

/// 认证槽的签名方式（SSH 支持的几种）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scheme {
    Ed25519,
    Rsa,
    Ecdsa(EcdsaCurve),
}

impl Scheme {
    fn of(algorithm: &AlgorithmAttributes) -> Option<Self> {
        match algorithm {
            AlgorithmAttributes::Rsa(_) => Some(Self::Rsa),
            AlgorithmAttributes::Ecc(ecc) => match (ecc.ecc_type(), ecc.curve()) {
                (EccType::EdDSA, Curve::Ed25519) => Some(Self::Ed25519),
                (EccType::ECDSA, Curve::NistP256r1) => Some(Self::Ecdsa(EcdsaCurve::NistP256)),
                (EccType::ECDSA, Curve::NistP384r1) => Some(Self::Ecdsa(EcdsaCurve::NistP384)),
                (EccType::ECDSA, Curve::NistP521r1) => Some(Self::Ecdsa(EcdsaCurve::NistP521)),
                _ => None,
            },
            AlgorithmAttributes::Unknown(_) => None,
        }
    }
}

fn algorithm_name(algorithm: &AlgorithmAttributes) -> String {
    match algorithm {
        AlgorithmAttributes::Rsa(rsa) => format!("RSA {}", rsa.len_n()),
        AlgorithmAttributes::Ecc(ecc) => match ecc.curve() {
            Curve::Ed25519 => "Ed25519".to_owned(),
            Curve::NistP256r1 => "NIST P-256".to_owned(),
            Curve::NistP384r1 => "NIST P-384".to_owned(),
            Curve::NistP521r1 => "NIST P-521".to_owned(),
            curve => format!("{curve:?}"),
        },
        AlgorithmAttributes::Unknown(_) => "unknown".to_owned(),
    }
}

/// 卡给的公钥 → OpenSSH 公钥，注释写卡号。Ed25519 是 32 字节的点（个别卡带 0x40 前缀），
/// RSA 是模数与指数，ECDSA 是 SEC1 未压缩点。
fn ssh_public_key(material: &PublicKeyMaterial, scheme: Scheme, ident: &str) -> Option<PublicKey> {
    let key_data = match (scheme, material) {
        (Scheme::Ed25519, PublicKeyMaterial::E(ecc)) => {
            let point = match ecc.data() {
                [0x40, rest @ ..] if rest.len() == 32 => rest,
                point => point,
            };
            KeyData::Ed25519(Ed25519PublicKey(point.try_into().ok()?))
        }
        (Scheme::Rsa, PublicKeyMaterial::R(rsa)) => KeyData::Rsa(
            RsaPublicKey::new(
                Mpint::from_positive_bytes(rsa.v()),
                Mpint::from_positive_bytes(rsa.n()),
            )
            .ok()?,
        ),
        (Scheme::Ecdsa(curve), PublicKeyMaterial::E(ecc)) => {
            let key = EcdsaPublicKey::from_sec1_bytes(ecc.data()).ok()?;
            if key.curve() != curve {
                return None;
            }
            KeyData::Ecdsa(key)
        }
        _ => return None,
    };
    Some(PublicKey::new(
        key_data,
        format!("cardno:{}", ident.replace(':', "")),
    ))
}

/// RSA：按服务器接受的签名算法取摘要。没有 SHA-2 可用的旧服务器只认 `ssh-rsa`（SHA-1）。
fn rsa_digest(hash: Option<HashAlg>, data: &[u8]) -> (&'static str, HashAlgo, Vec<u8>) {
    match hash {
        Some(HashAlg::Sha512) => (
            "rsa-sha2-512",
            HashAlgo::SHA512,
            Sha512::digest(data).to_vec(),
        ),
        Some(_) => (
            "rsa-sha2-256",
            HashAlgo::SHA256,
            Sha256::digest(data).to_vec(),
        ),
        None => {
            use sha1::Digest as _;
            ("ssh-rsa", HashAlgo::SHA1, sha1::Sha1::digest(data).to_vec())
        }
    }
}

/// ECDSA：摘要算法由曲线决定（RFC 5656）。
fn ecdsa_digest(curve: EcdsaCurve, data: &[u8]) -> Vec<u8> {
    match curve {
        EcdsaCurve::NistP256 => Sha256::digest(data).to_vec(),
        EcdsaCurve::NistP384 => Sha384::digest(data).to_vec(),
        EcdsaCurve::NistP521 => Sha512::digest(data).to_vec(),
    }
}

/// 卡回的 ECDSA 签名是定长的 r‖s（个别卡每半多一个前导 0）→ SSH 签名 blob：
/// `string(算法名) || string(mpint(r) || mpint(s))`。
fn ecdsa_signature(curve: EcdsaCurve, signature: &[u8]) -> Result<Vec<u8>, CardFailure> {
    if signature.is_empty() || !signature.len().is_multiple_of(2) {
        return Err(CardFailure::Io(format!(
            "unexpected ECDSA signature length {}",
            signature.len()
        )));
    }
    let (r, s) = signature.split_at(signature.len() / 2);
    let mut inner = Vec::with_capacity(signature.len() + 10);
    for half in [r, s] {
        let mpint = Mpint::from_positive_bytes(half);
        push_string(&mut inner, mpint.as_bytes());
    }
    let name = format!("ecdsa-sha2-{}", curve.as_str());
    Ok(ssh_signature(&name, &inner))
}

/// SSH 签名 blob：`string(算法名) || string(签名)`。
fn ssh_signature(algorithm: &str, signature: &[u8]) -> Vec<u8> {
    let mut blob = Vec::with_capacity(8 + algorithm.len() + signature.len());
    push_string(&mut blob, algorithm.as_bytes());
    push_string(&mut blob, signature);
    blob
}

/// SSH 的 string：u32 长度 + 内容。
fn push_string(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&u32::try_from(bytes.len()).unwrap_or(u32::MAX).to_be_bytes());
    out.extend_from_slice(bytes);
}

#[cfg(test)]
mod tests;
