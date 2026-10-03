//! 连接目录（上游 `ConnectionCatalog`，存储在 SQLite；密码在钥匙串）。

use rinf::{DartSignal, RustSignal, SignalPiece};
use serde::{Deserialize, Serialize};

/// 认证方式（上游 `AuthenticationKind` 里 iOS 上可用的部分）。
#[derive(Deserialize, Serialize, SignalPiece, Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMethod {
    Password,
    /// 钥匙串里的私钥（`key_id`）。
    PublicKey,
    KeyboardInteractive,
}

/// 目录里的一条连接（列表与编辑页用）。密码本身永不过边界。
#[derive(Serialize, SignalPiece)]
pub struct ConnectionSummary {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: AuthMethod,
    /// 钥匙串里存了密码（没存则连接时弹框问）。
    pub password_saved: bool,
    /// PublicKey：用的私钥。
    pub key_id: String,
    pub command: String,
}

/// 目录（按 `query` 过滤后的有序列表）。查询或目录变化后整份重发。
#[derive(Serialize, RustSignal)]
pub struct CatalogState {
    pub query: String,
    pub connections: Vec<ConnectionSummary>,
}

/// 设置搜索词并取目录（空串 = 全部）。
#[derive(Deserialize, DartSignal)]
pub struct CatalogQuery {
    pub query: String,
}

#[derive(Deserialize, Serialize, SignalPiece, Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordAction {
    /// 编辑时不动已存的密码。
    Keep,
    /// 存入 `password`（覆盖旧的）。
    Set,
    /// 不存密码：删掉已存的，连接时弹框问。
    Clear,
}

/// 新建（`id` 为空）或修改一条连接。
#[derive(Deserialize, DartSignal)]
pub struct SaveConnection {
    pub request_id: u32,
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: AuthMethod,
    pub password_action: PasswordAction,
    pub password: String,
    /// PublicKey：用的私钥。
    pub key_id: String,
    pub command: String,
}

#[derive(Deserialize, DartSignal)]
pub struct DeleteConnection {
    pub request_id: u32,
    pub id: String,
}

/// 复制一条连接（含已存的密码引用，由上游按引用计数管理）。
#[derive(Deserialize, DartSignal)]
pub struct DuplicateConnection {
    pub request_id: u32,
    pub id: String,
}

/// 目录操作失败的原因。文案由 Dart 按分类给出。
#[derive(Serialize, SignalPiece, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogError {
    None,
    HostRequired,
    HostInvalid,
    PortInvalid,
    UsernameRequired,
    PasswordRequired,
    /// 私钥认证没选私钥（或选的私钥不在了）。
    KeyRequired,
    NotFound,
    Keychain,
    Storage,
}

/// 保存 / 删除 / 复制的结果，`request_id` 原样带回。
/// `connection_id` 是被保存或复制出的连接（删除时为空）。
#[derive(Serialize, RustSignal)]
pub struct CatalogResult {
    pub request_id: u32,
    pub error: CatalogError,
    pub connection_id: String,
}

/// 桌面目录协议独立于移动旧信号；目录读写不参与设置/profile 的 revision 事务。
pub const DESKTOP_CATALOG_VERSION: u16 = 1;

#[derive(Deserialize, Serialize, SignalPiece, Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopTransport {
    NativeSsh,
    SystemOpenSsh,
    /// 上游连接目录尚无本地目标的存储类型；目前只能返回明确的不支持错误。
    LocalShell,
}

#[derive(Deserialize, Serialize, SignalPiece, Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopCatalogTargetKind {
    Ssh,
    LocalShell,
}

#[derive(Deserialize, Serialize, SignalPiece, Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopAuthMethod {
    Password,
    PublicKey,
    KeyboardInteractive,
    Agent,
}

/// 完整目录投影。只有凭据存在性、应用私钥 ID 或用户选择的 System identity 路径，
/// 不回传密码、私钥内容、口令或 credential_ref。
#[derive(Serialize, SignalPiece)]
pub struct DesktopConnectionSummary {
    pub id: String,
    pub name: String,
    pub target_kind: DesktopCatalogTargetKind,
    pub transport: DesktopTransport,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: DesktopAuthMethod,
    pub password_saved: bool,
    pub key_id: String,
    pub system_identity_path: String,
    pub command: String,
    /// 空串表示沿用应用默认 profile；非空为 UUID 引用。
    pub profile_id: String,
}

#[derive(Deserialize, Serialize, DartSignal)]
pub struct DesktopCatalogQuery {
    pub version: u16,
    pub request_id: u32,
    pub query: String,
}

#[derive(Serialize, SignalPiece, Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopCatalogError {
    None,
    UnsupportedVersion,
    DesktopOnly,
    UnsupportedTransport,
    TargetMismatch,
    InvalidProfileId,
    ProfileNotFound,
    InvalidConnectionId,
    InvalidIdentity,
    InvalidAuthentication,
    CredentialMismatch,
    HostRequired,
    HostInvalid,
    PortInvalid,
    UsernameRequired,
    UsernameInvalid,
    KeyRequired,
    PasswordRequired,
    NotFound,
    Keychain,
    Storage,
}

/// 查询的结果与请求 ID 关联，不依赖不具原子保证的“完整目录 revision”。
#[derive(Serialize, RustSignal)]
pub struct DesktopCatalogState {
    pub version: u16,
    pub request_id: u32,
    pub error: DesktopCatalogError,
    pub connections: Vec<DesktopConnectionSummary>,
}

/// 完整桌面编辑请求；空 ID 新建。每次保存须显式给出目标类型及传输种类。
/// 密码只在请求中出现，绝不进入结果或日志。
#[derive(Deserialize, Serialize, DartSignal)]
pub struct DesktopSaveConnection {
    pub version: u16,
    pub request_id: u32,
    pub id: String,
    pub name: String,
    pub target_kind: DesktopCatalogTargetKind,
    pub transport: DesktopTransport,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: DesktopAuthMethod,
    pub password_action: PasswordAction,
    pub password: String,
    pub key_id: String,
    pub system_identity_path: String,
    pub command: String,
    /// 空串表示应用默认 profile；非空必须是已存在的 UUID。
    pub profile_id: String,
}

#[derive(Serialize, RustSignal)]
pub struct DesktopCatalogResult {
    pub version: u16,
    pub request_id: u32,
    pub error: DesktopCatalogError,
    pub connection_id: String,
}

#[cfg(test)]
pub(crate) fn inject_desktop_save(bytes: &[u8]) {
    DesktopSaveConnection::send_dart_signal(bytes, &[]);
}
