//! 私钥（存在钥匙串里；私钥本身永不过边界，导入时的原文除外），以及 OpenPGP 卡与安全密钥
//! 上的密钥（钥匙串里只登记公钥与卡号 / 凭据 id，私钥在卡或安全密钥里）。

use rinf::{DartSignal, RustSignal, SignalPiece};
use serde::{Deserialize, Serialize};

/// 列表里的一把私钥。
#[derive(Serialize, SignalPiece)]
pub struct KeySummary {
    pub id: String,
    pub name: String,
    /// 算法（`ssh-ed25519`、`ssh-rsa`、`ecdsa-sha2-nistp256` …）。
    pub algorithm: String,
    /// SHA256 指纹。
    pub fingerprint: String,
    /// OpenSSH 一行格式的公钥（复制到服务器的 authorized_keys）。
    pub public_key: String,
    /// 私钥有口令保护。
    pub encrypted: bool,
    /// 口令已存进钥匙串。
    pub passphrase_saved: bool,
    /// 在 iCloud 钥匙串里（随 iCloud 同步）。
    pub synchronized: bool,
    /// 使用它的连接数。
    pub used_by: u32,
    /// OpenPGP 卡上的密钥：卡号（`厂商:序列号`）；钥匙串里的私钥为空。
    pub card_ident: String,
    /// 安全密钥（FIDO2）上的密钥。
    pub security_key: bool,
}

/// 私钥列表与同步开关。私钥或开关变化后重发。
#[derive(Serialize, RustSignal)]
pub struct KeyListState {
    pub keys: Vec<KeySummary>,
    /// 列表读取失败时不把空列表当作没有私钥。
    pub list_error: KeyError,
    /// 迁移或来源清理尚未完成，此时两个存储可能都留有副本。
    pub sync_pending: bool,
    /// 未完成迁移的目标；重试时使用它。
    pub sync_target_enabled: bool,
    /// 私钥经 iCloud 钥匙串同步（新导入的私钥也放进 iCloud 钥匙串）。
    pub sync_enabled: bool,
    /// 这个构建能用 iCloud 钥匙串（没有团队签名的 macOS 构建不能）。
    pub sync_available: bool,
    /// 这台设备能用安全密钥（FIDO2）。
    pub security_keys_available: bool,
}

#[derive(Deserialize, DartSignal)]
pub struct KeyQuery {}

/// 导入私钥。`private_key` 是私钥原文（OpenSSH / PEM / PKCS#8 / PuTTY 格式）；
/// `passphrase` 可为空——OpenSSH 格式的加密私钥不需要口令也能导入，其他格式要。
#[derive(Deserialize, DartSignal)]
pub struct ImportKey {
    pub request_id: u32,
    pub name: String,
    pub private_key: String,
    pub passphrase: String,
}

#[derive(Deserialize, DartSignal)]
pub struct RenameKey {
    pub request_id: u32,
    pub id: String,
    pub name: String,
}

/// 删除私钥（连同存下的口令）。有连接在用时拒绝。
#[derive(Deserialize, DartSignal)]
pub struct DeleteKey {
    pub request_id: u32,
    pub id: String,
}

/// 删掉存下的口令，下次连接时再问。
#[derive(Deserialize, DartSignal)]
pub struct ForgetPassphrase {
    pub request_id: u32,
    pub id: String,
}

/// 开关 iCloud 钥匙串同步：已有的私钥与口令随之移入或移出 iCloud 钥匙串。
#[derive(Deserialize, DartSignal)]
pub struct SetKeySync {
    pub request_id: u32,
    pub enabled: bool,
}

/// 读卡：找出现在能用的 OpenPGP 卡（`nfc = true` 时弹出系统的 NFC 界面等卡靠近）。
/// 回答是 `CardScanResult`。
#[derive(Deserialize, DartSignal)]
pub struct ScanCards {
    pub request_id: u32,
    pub nfc: bool,
}

/// 读到的一张卡（认证槽）。
#[derive(Serialize, SignalPiece)]
pub struct CardSummary {
    pub ident: String,
    pub cardholder: String,
    pub algorithm: String,
    /// OpenSSH 一行格式的公钥；空 = 认证槽没有密钥，或算法暂不支持。
    pub public_key: String,
    pub fingerprint: String,
    pub pin_tries_left: u32,
    /// 签名要在卡上按键确认。
    pub touch: bool,
    /// 已经登记过（同一把公钥）。
    pub added: bool,
}

#[derive(Serialize, RustSignal)]
pub struct CardScanResult {
    pub request_id: u32,
    pub error: KeyError,
    pub cards: Vec<CardSummary>,
    /// 这台设备能用 NFC 读卡。
    pub nfc_available: bool,
}

/// 登记上次读卡看到的一张卡（卡号 `ident`）的认证密钥。回答是 `KeyResult`。
#[derive(Deserialize, DartSignal)]
pub struct AddCardKey {
    pub request_id: u32,
    pub ident: String,
    pub name: String,
}

/// 在安全密钥上新建一把 SSH 密钥（系统界面引导插上、靠近或触摸）。回答是 `KeyResult`。
#[derive(Deserialize, DartSignal)]
pub struct RegisterSecurityKey {
    pub request_id: u32,
    pub name: String,
}

#[derive(Serialize, SignalPiece, Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyError {
    None,
    /// 不是能识别的私钥。
    Invalid,
    /// 私钥加密了，这种格式要口令才能读出公钥。
    PassphraseRequired,
    PassphraseWrong,
    /// 同一把私钥已经导入过。
    AlreadyExists,
    NotFound,
    /// 还有连接在用。
    InUse,
    Keychain,
    /// 写不进 iCloud 钥匙串。
    SyncUnavailable,
    /// 没有找到 OpenPGP 卡（没插上、没靠近，或 NFC 读卡被取消）。
    CardNotFound,
    /// 卡的认证槽没有密钥，或算法暂不支持。
    CardUnsupported,
    /// 这台设备用不了安全密钥。
    SecurityKeyUnavailable,
    /// 用户取消了安全密钥的系统界面。
    SecurityKeyCancelled,
    /// 安全密钥没能完成注册。
    SecurityKeyFailed,
}

/// 私钥操作的结果，`request_id` 原样带回；`key_id` 是导入的私钥。
#[derive(Serialize, RustSignal)]
pub struct KeyResult {
    pub request_id: u32,
    pub error: KeyError,
    pub key_id: String,
}
