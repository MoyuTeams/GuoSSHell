//! 连接目录：查询、保存、删除、复制。目录是上游的 SQLite 存储，密码经上游
//! `CredentialCoordinator` 进钥匙串（目录与钥匙串之间的两阶段提交由它负责）。

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use rinf::{DartSignal, RustSignal, debug_print};
use rshell_m0::rshell_core::{
    AuthenticationKind, CatalogMutation, ConnectionCatalog, ConnectionId, ConnectionProfile,
    SecretUpdate, TerminalProfileId, TransportKind,
};
use rshell_m0::rshell_storage::CredentialOperationError;
use secrecy::{ExposeSecret, SecretString};
use tokio::task::spawn_blocking;

use crate::app::AppContext;
use crate::keys::{self, Item};
use crate::signals::catalog::{
    AuthMethod, CatalogError, CatalogQuery, CatalogResult, CatalogState, ConnectionSummary,
    DESKTOP_CATALOG_VERSION, DeleteConnection, DesktopAuthMethod, DesktopCatalogError,
    DesktopCatalogQuery, DesktopCatalogResult, DesktopCatalogState, DesktopCatalogTargetKind,
    DesktopConnectionSummary, DesktopSaveConnection, DesktopTransport, DuplicateConnection,
    PasswordAction, SaveConnection,
};

pub async fn run(context: Arc<AppContext>) {
    let query_rx = CatalogQuery::get_dart_signal_receiver();
    let save_rx = SaveConnection::get_dart_signal_receiver();
    let delete_rx = DeleteConnection::get_dart_signal_receiver();
    let duplicate_rx = DuplicateConnection::get_dart_signal_receiver();
    let desktop_query_rx = DesktopCatalogQuery::get_dart_signal_receiver();
    let desktop_save_rx = DesktopSaveConnection::get_dart_signal_receiver();
    let mut query = String::new();
    loop {
        tokio::select! {
            pack = desktop_query_rx.recv() => {
                let Some(pack) = pack else { break };
                let request = pack.message;
                let request_id = request.request_id;
                let result = match desktop_gate(request.version) {
                    Ok(()) => {
                        blocking(&context, move |context| {
                            let catalog = load(context).map_err(desktop_error)?;
                            let query = request.query;
                            Ok(desktop_summaries(&catalog, &query))
                        }).await
                    }
                    Err(error) => Err(error),
                };
                desktop_state(request_id, result);
            }
            pack = desktop_save_rx.recv() => {
                let Some(pack) = pack else { break };
                // 解码后即转移明文所有权；版本、目标及存储失败均只释放清零容器。
                let draft = DesktopSaveDraft::new(pack.message);
                let request_id = draft.request.request_id;
                let result = blocking(&context, move |context| desktop_save_secured(context, draft)).await;
                desktop_reply(request_id, result);
            }
            pack = query_rx.recv() => {
                let Some(pack) = pack else { break };
                query = pack.message.query;
            }
            pack = save_rx.recv() => {
                let Some(pack) = pack else { break };
                let mut request = pack.message;
                let password = SecretString::from(std::mem::take(&mut request.password));
                let request_id = request.request_id;
                let result = blocking(&context, move |context| save_secured(context, request, password)).await;
                reply(request_id, result);
            }
            pack = delete_rx.recv() => {
                let Some(pack) = pack else { break };
                let request_id = pack.message.request_id;
                let result = blocking(&context, move |context| delete(context, &pack.message.id)).await;
                reply(request_id, result.map(|()| String::new()));
            }
            pack = duplicate_rx.recv() => {
                let Some(pack) = pack else { break };
                let request_id = pack.message.request_id;
                let result = blocking(&context, move |context| duplicate(context, &pack.message.id)).await;
                reply(request_id, result);
            }
            () = context.catalog_changed.notified() => {}
        }
        publish(&context, &query).await;
    }
}

/// 在阻塞线程上跑一次目录操作（SQLite 与钥匙串调用都会阻塞）。
async fn blocking<T, E>(
    context: &Arc<AppContext>,
    operation: impl FnOnce(&AppContext) -> Result<T, E> + Send + 'static,
) -> Result<T, E>
where
    T: Send + 'static,
    E: Send + From<CatalogError> + 'static,
{
    let context = context.clone();
    spawn_blocking(move || operation(&context))
        .await
        .unwrap_or_else(|_| Err(CatalogError::Storage.into()))
}

impl From<CatalogError> for DesktopCatalogError {
    fn from(value: CatalogError) -> Self {
        desktop_error(value)
    }
}

fn desktop_error(value: CatalogError) -> DesktopCatalogError {
    match value {
        CatalogError::None => DesktopCatalogError::None,
        CatalogError::HostRequired => DesktopCatalogError::HostRequired,
        CatalogError::HostInvalid => DesktopCatalogError::HostInvalid,
        CatalogError::PortInvalid => DesktopCatalogError::PortInvalid,
        CatalogError::UsernameRequired => DesktopCatalogError::UsernameRequired,
        CatalogError::PasswordRequired => DesktopCatalogError::PasswordRequired,
        CatalogError::KeyRequired => DesktopCatalogError::KeyRequired,
        CatalogError::NotFound => DesktopCatalogError::NotFound,
        CatalogError::Keychain => DesktopCatalogError::Keychain,
        CatalogError::Storage => DesktopCatalogError::Storage,
    }
}

fn desktop_gate(version: u16) -> Result<(), DesktopCatalogError> {
    if version != DESKTOP_CATALOG_VERSION {
        return Err(DesktopCatalogError::UnsupportedVersion);
    }
    if !cfg!(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "linux"
    )) {
        return Err(DesktopCatalogError::DesktopOnly);
    }
    Ok(())
}

fn desktop_state(
    request_id: u32,
    result: Result<Vec<DesktopConnectionSummary>, DesktopCatalogError>,
) {
    let (error, connections) = match result {
        Ok(connections) => (DesktopCatalogError::None, connections),
        Err(error) => (error, Vec::new()),
    };
    DesktopCatalogState {
        version: DESKTOP_CATALOG_VERSION,
        request_id,
        error,
        connections,
    }
    .send_signal_to_dart();
}

fn desktop_reply(request_id: u32, result: Result<String, DesktopCatalogError>) {
    let (error, connection_id) = match result {
        Ok(connection_id) => (DesktopCatalogError::None, connection_id),
        Err(error) => (error, String::new()),
    };
    DesktopCatalogResult {
        version: DESKTOP_CATALOG_VERSION,
        request_id,
        error,
        connection_id,
    }
    .send_signal_to_dart();
}

fn reply(request_id: u32, result: Result<String, CatalogError>) {
    let (error, connection_id) = match result {
        Ok(connection_id) => (CatalogError::None, connection_id),
        Err(error) => (error, String::new()),
    };
    CatalogResult {
        request_id,
        error,
        connection_id,
    }
    .send_signal_to_dart();
}

async fn publish(context: &Arc<AppContext>, query: &str) {
    let task_context = context.clone();
    let catalog = match spawn_blocking(move || task_context.repository.load_catalog()).await {
        Ok(Ok(catalog)) => catalog,
        Ok(Err(error)) => {
            debug_print!("[catalog] load: {error:?}");
            return;
        }
        Err(error) => {
            debug_print!("[catalog] load task: {error}");
            return;
        }
    };
    CatalogState {
        query: query.to_owned(),
        connections: summaries(&catalog, query),
    }
    .send_signal_to_dart();
}

/// 按查询过滤（名称、主机、用户名、标签；上游的规则）并保持目录顺序。
fn summaries(catalog: &ConnectionCatalog, query: &str) -> Vec<ConnectionSummary> {
    catalog
        .search(query)
        .into_iter()
        .filter_map(|id| catalog.connections.get(&id))
        .filter_map(summary)
        .collect()
}

/// 边界上的连接摘要。只有本 App 写这个目录：Native 的私钥认证总是引用钥匙串里的私钥，
/// 也不会有 agent 认证。
fn summary(profile: &ConnectionProfile) -> Option<ConnectionSummary> {
    summary_for(profile, crate::connect::desktop_supported())
}

fn summary_for(profile: &ConnectionProfile, desktop: bool) -> Option<ConnectionSummary> {
    if profile.transport == TransportKind::SystemOpenSsh {
        // 移动旧列表不能展示移动端无法编辑或连接的 System 项；桌面由桌面编辑器与 V2 会话处理。
        if !desktop {
            return None;
        }
        return Some(ConnectionSummary {
            id: profile.id.0.to_string(),
            name: profile.name.clone(),
            host: profile.host.clone(),
            port: profile.port,
            username: profile.username.clone(),
            auth: AuthMethod::Password,
            password_saved: false,
            key_id: String::new(),
            command: profile.remote_command.clone().unwrap_or_default(),
            transport: DesktopTransport::SystemOpenSsh,
        });
    }
    let auth = match profile.authentication {
        AuthenticationKind::Password => AuthMethod::Password,
        AuthenticationKind::PublicKey => {
            keys::key_id(profile)?;
            AuthMethod::PublicKey
        }
        AuthenticationKind::KeyboardInteractive => AuthMethod::KeyboardInteractive,
        AuthenticationKind::Agent => return None,
    };
    Some(ConnectionSummary {
        id: profile.id.0.to_string(),
        name: profile.name.clone(),
        host: profile.host.clone(),
        port: profile.port,
        username: profile.username.clone(),
        auth,
        password_saved: auth == AuthMethod::Password && profile.credential_ref.is_some(),
        key_id: keys::key_id(profile).unwrap_or_default().to_owned(),
        command: profile.remote_command.clone().unwrap_or_default(),
        transport: DesktopTransport::NativeSsh,
    })
}

fn desktop_summaries(catalog: &ConnectionCatalog, query: &str) -> Vec<DesktopConnectionSummary> {
    catalog
        .search(query)
        .into_iter()
        .filter_map(|id| catalog.connections.get(&id))
        .map(desktop_summary)
        .collect()
}

fn desktop_summary(profile: &ConnectionProfile) -> DesktopConnectionSummary {
    let transport = match profile.transport {
        TransportKind::NativeSsh => DesktopTransport::NativeSsh,
        TransportKind::SystemOpenSsh => DesktopTransport::SystemOpenSsh,
    };
    let auth = match profile.authentication {
        AuthenticationKind::Password => DesktopAuthMethod::Password,
        AuthenticationKind::PublicKey => DesktopAuthMethod::PublicKey,
        AuthenticationKind::KeyboardInteractive => DesktopAuthMethod::KeyboardInteractive,
        AuthenticationKind::Agent => DesktopAuthMethod::Agent,
    };
    DesktopConnectionSummary {
        id: profile.id.0.to_string(),
        name: profile.name.clone(),
        target_kind: DesktopCatalogTargetKind::Ssh,
        transport,
        host: profile.host.clone(),
        port: profile.port,
        username: profile.username.clone(),
        auth,
        password_saved: profile.transport == TransportKind::NativeSsh
            && profile.authentication == AuthenticationKind::Password
            && profile.credential_ref.is_some(),
        key_id: if profile.transport == TransportKind::NativeSsh {
            keys::key_id(profile).unwrap_or_default().to_owned()
        } else {
            String::new()
        },
        system_identity_path: if profile.transport == TransportKind::SystemOpenSsh {
            profile
                .identity_file
                .as_ref()
                .and_then(|path| path.to_str())
                .filter(|path| valid_system_identity_path(path))
                .unwrap_or_default()
                .to_owned()
        } else {
            String::new()
        },
        command: profile.remote_command.clone().unwrap_or_default(),
        profile_id: profile
            .terminal_profile_id
            .map_or_else(String::new, |id| id.0.to_string()),
    }
}

fn parse_id(id: &str) -> Result<ConnectionId, CatalogError> {
    uuid::Uuid::parse_str(id)
        .map(ConnectionId::from)
        .map_err(|_| CatalogError::NotFound)
}

fn load(context: &AppContext) -> Result<ConnectionCatalog, CatalogError> {
    context.repository.load_catalog().map_err(|error| {
        debug_print!("[catalog] load: {error:?}");
        CatalogError::Storage
    })
}

fn apply(
    context: &AppContext,
    mutation: CatalogMutation,
    secret: SecretUpdate,
) -> Result<ConnectionCatalog, CatalogError> {
    context
        .credentials
        .apply_catalog(mutation, secret)
        .map_err(|error| match error {
            CredentialOperationError::Vault => CatalogError::Keychain,
            other => {
                debug_print!("[catalog] apply: {other:?}");
                CatalogError::Storage
            }
        })
}

/// 新建或修改一条连接，返回它的 id。
#[cfg(test)]
fn save(context: &AppContext, mut request: SaveConnection) -> Result<String, CatalogError> {
    let password = SecretString::from(std::mem::take(&mut request.password));
    save_secured(context, request, password)
}

fn save_secured(
    context: &AppContext,
    request: SaveConnection,
    password: SecretString,
) -> Result<String, CatalogError> {
    let _operations = context
        .key_operations
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let existing = if request.id.is_empty() {
        None
    } else {
        let id = parse_id(&request.id)?;
        Some(
            load(context)?
                .connections
                .get(&id)
                .cloned()
                .ok_or(CatalogError::NotFound)?,
        )
    };
    let creating = existing.is_none();
    let profile = profile_from_request(existing, &request)?;
    if request.auth == AuthMethod::PublicKey {
        let found = context
            .keys
            .get(Item::Key, &request.key_id)
            .map_err(|_| CatalogError::Keychain)?;
        if found.is_none() {
            return Err(CatalogError::KeyRequired);
        }
    }
    let secret = secret_update_secured(&request, creating, password)?;
    let id = profile.id;
    let mutation = if creating {
        CatalogMutation::Create(profile)
    } else {
        CatalogMutation::Update(profile)
    };
    apply(context, mutation, secret)?;
    Ok(id.0.to_string())
}

/// 入口在任何可失败的校验前接管密码分配；空置请求字段防止普通 String 离开作用域时
/// 留下明文。这里只保证 Rust 已解码请求的所有权，不保证跨语言缓冲和分配器无副本。
struct DesktopSaveDraft {
    request: DesktopSaveConnection,
    password: SecretString,
}

impl DesktopSaveDraft {
    fn new(mut request: DesktopSaveConnection) -> Self {
        let password = SecretString::from(std::mem::take(&mut request.password));
        Self { request, password }
    }
}

/// 测试也走同一条所有权转移路径，不绕过入口保护。
#[cfg(test)]
fn desktop_save(
    context: &AppContext,
    request: DesktopSaveConnection,
) -> Result<String, DesktopCatalogError> {
    desktop_save_secured(context, DesktopSaveDraft::new(request))
}

/// 桌面保存仅提交目录本身；profile 存在性在保存前验证，设置/profile 与
/// CredentialCoordinator 没有跨存储原子事务，不把目录结果标成设置 revision。
fn desktop_save_secured(
    context: &AppContext,
    draft: DesktopSaveDraft,
) -> Result<String, DesktopCatalogError> {
    let DesktopSaveDraft { request, password } = draft;
    desktop_gate(request.version)?;
    let _operations = context
        .key_operations
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let existing = if request.id.is_empty() {
        None
    } else {
        let id = uuid::Uuid::parse_str(&request.id)
            .map(ConnectionId::from)
            .map_err(|_| DesktopCatalogError::InvalidConnectionId)?;
        Some(
            load(context)
                .map_err(desktop_error)?
                .connections
                .get(&id)
                .cloned()
                .ok_or(DesktopCatalogError::NotFound)?,
        )
    };
    let creating = existing.is_none();
    let (profile, secret) = desktop_profile_from_request(context, existing, &request, password)?;
    if profile.transport == TransportKind::NativeSsh
        && profile.authentication == AuthenticationKind::PublicKey
    {
        let found = context
            .keys
            .get(Item::Key, request.key_id.trim())
            .map_err(|_| DesktopCatalogError::Keychain)?;
        if found.is_none() {
            return Err(DesktopCatalogError::KeyRequired);
        }
    }
    let id = profile.id;
    let mutation = if creating {
        CatalogMutation::Create(profile)
    } else {
        CatalogMutation::Update(profile)
    };
    apply(context, mutation, secret).map_err(desktop_error)?;
    Ok(id.0.to_string())
}

fn desktop_profile_from_request(
    context: &AppContext,
    existing: Option<ConnectionProfile>,
    request: &DesktopSaveConnection,
    password: SecretString,
) -> Result<(ConnectionProfile, SecretUpdate), DesktopCatalogError> {
    if request.target_kind == DesktopCatalogTargetKind::LocalShell
        && request.transport == DesktopTransport::LocalShell
    {
        return Err(DesktopCatalogError::UnsupportedTransport);
    }
    if request.target_kind != DesktopCatalogTargetKind::Ssh {
        return Err(DesktopCatalogError::TargetMismatch);
    }
    let transport = match request.transport {
        DesktopTransport::NativeSsh => TransportKind::NativeSsh,
        DesktopTransport::SystemOpenSsh => TransportKind::SystemOpenSsh,
        DesktopTransport::LocalShell => return Err(DesktopCatalogError::UnsupportedTransport),
    };
    let authentication = match request.auth {
        DesktopAuthMethod::Password => AuthenticationKind::Password,
        DesktopAuthMethod::PublicKey => AuthenticationKind::PublicKey,
        DesktopAuthMethod::KeyboardInteractive => AuthenticationKind::KeyboardInteractive,
        DesktopAuthMethod::Agent => AuthenticationKind::Agent,
    };
    if !matches!(
        (transport, authentication),
        (
            TransportKind::SystemOpenSsh,
            AuthenticationKind::Agent | AuthenticationKind::PublicKey
        ) | (
            TransportKind::NativeSsh,
            AuthenticationKind::Password
                | AuthenticationKind::PublicKey
                | AuthenticationKind::KeyboardInteractive
        )
    ) {
        return Err(DesktopCatalogError::InvalidAuthentication);
    }
    let (host, username) =
        validate_target(&request.host, request.port, &request.username).map_err(desktop_error)?;
    if host.chars().any(char::is_control) {
        return Err(DesktopCatalogError::HostInvalid);
    }
    if username.chars().any(char::is_control) {
        return Err(DesktopCatalogError::UsernameInvalid);
    }
    let profile_id = if request.profile_id.is_empty() {
        None
    } else {
        let id = uuid::Uuid::parse_str(&request.profile_id)
            .map(TerminalProfileId::from)
            .map_err(|_| DesktopCatalogError::InvalidProfileId)?;
        let found = context
            .repository
            .load_terminal_profiles()
            .map_err(|_| DesktopCatalogError::Storage)?
            .iter()
            .any(|profile| profile.id == id);
        if !found {
            return Err(DesktopCatalogError::ProfileNotFound);
        }
        Some(id)
    };

    let native_key = request.key_id.trim();
    let system_path = request.system_identity_path.trim();
    if !password.expose_secret().is_empty() && request.password_action != PasswordAction::Set {
        return Err(DesktopCatalogError::CredentialMismatch);
    }
    let identity = match (transport, authentication) {
        (TransportKind::NativeSsh, AuthenticationKind::PublicKey) => {
            if native_key.is_empty() {
                return Err(DesktopCatalogError::KeyRequired);
            }
            if !system_path.is_empty() {
                return Err(DesktopCatalogError::CredentialMismatch);
            }
            Some(keys::key_ref(native_key))
        }
        (TransportKind::SystemOpenSsh, AuthenticationKind::PublicKey) => {
            if !native_key.is_empty() {
                return Err(DesktopCatalogError::CredentialMismatch);
            }
            if !valid_system_identity_path(system_path) {
                return Err(DesktopCatalogError::InvalidIdentity);
            }
            Some(PathBuf::from(system_path))
        }
        _ => {
            if !native_key.is_empty() || !system_path.is_empty() {
                return Err(DesktopCatalogError::CredentialMismatch);
            }
            None
        }
    };
    let saved_secret = existing
        .as_ref()
        .is_some_and(|profile| profile.credential_ref.is_some());
    if saved_secret
        && request.password_action == PasswordAction::Keep
        && existing
            .as_ref()
            .is_some_and(|profile| profile.transport != transport)
    {
        // 不允许跨 transport 复用来源不明的旧 credential_ref。
        return Err(DesktopCatalogError::CredentialMismatch);
    }
    let secret = if authentication == AuthenticationKind::Password {
        match request.password_action {
            PasswordAction::Set if password.expose_secret().is_empty() => {
                return Err(DesktopCatalogError::PasswordRequired);
            }
            PasswordAction::Set => SecretUpdate::Set(password),
            PasswordAction::Keep => SecretUpdate::Unchanged,
            PasswordAction::Clear if saved_secret => SecretUpdate::Clear,
            PasswordAction::Clear => SecretUpdate::Unchanged,
        }
    } else {
        if request.password_action == PasswordAction::Set
            || saved_secret && request.password_action != PasswordAction::Clear
        {
            return Err(DesktopCatalogError::CredentialMismatch);
        }
        if saved_secret {
            SecretUpdate::Clear
        } else {
            SecretUpdate::Unchanged
        }
    };
    let mut profile = existing.unwrap_or_else(|| ConnectionProfile::new("", host));
    profile.name = request.name.trim().to_owned();
    profile.host = host.to_owned();
    profile.port = request.port;
    profile.username = username.to_owned();
    profile.transport = transport;
    profile.authentication = authentication;
    profile.identity_file = identity;
    profile.terminal_profile_id = profile_id;
    profile.remote_command =
        Some(request.command.trim().to_owned()).filter(|value| !value.is_empty());
    Ok((profile, secret))
}

fn valid_system_identity_path(path: &str) -> bool {
    let path = path.trim();
    !path.is_empty()
        && !path.starts_with('-')
        && !path.chars().any(char::is_control)
        && !path.to_ascii_lowercase().starts_with(keys::KEY_REF_PREFIX)
}

/// 连接目标的校验（目录与快速连接共用）：主机非空、不像命令行选项、不含空白；
/// 端口非零；用户名非空。返回去掉首尾空白的主机与用户名。
pub fn validate_target<'a>(
    host: &'a str,
    port: u16,
    username: &'a str,
) -> Result<(&'a str, &'a str), CatalogError> {
    let host = host.trim();
    if host.is_empty() {
        return Err(CatalogError::HostRequired);
    }
    if host.starts_with('-') || host.chars().any(char::is_whitespace) {
        return Err(CatalogError::HostInvalid);
    }
    if port == 0 {
        return Err(CatalogError::PortInvalid);
    }
    let username = username.trim();
    if username.is_empty() {
        return Err(CatalogError::UsernameRequired);
    }
    Ok((host, username))
}

/// 旧表单只修改 Native SSH；其他传输不经旧表单转换认证与凭据。
/// 修改时从目录里的版本出发，表单之外的字段原样保留。
fn profile_from_request(
    existing: Option<ConnectionProfile>,
    request: &SaveConnection,
) -> Result<ConnectionProfile, CatalogError> {
    if existing
        .as_ref()
        .is_some_and(|profile| profile.transport != TransportKind::NativeSsh)
    {
        // 公开协议暂无专用错误；只记录固定原因，不记录条目或凭据。
        debug_print!("[catalog] legacy save: unsupported transport");
        return Err(CatalogError::Storage);
    }
    let (host, username) = validate_target(&request.host, request.port, &request.username)?;

    let mut profile = existing.unwrap_or_else(|| {
        let mut profile = ConnectionProfile::new("", host);
        profile.transport = TransportKind::NativeSsh;
        profile
    });
    profile.name = request.name.trim().to_owned();
    profile.host = host.to_owned();
    profile.port = request.port;
    profile.username = username.to_owned();
    profile.authentication = match request.auth {
        AuthMethod::Password => AuthenticationKind::Password,
        AuthMethod::PublicKey => AuthenticationKind::PublicKey,
        AuthMethod::KeyboardInteractive => AuthenticationKind::KeyboardInteractive,
    };
    profile.identity_file = match request.auth {
        AuthMethod::PublicKey if request.key_id.trim().is_empty() => {
            return Err(CatalogError::KeyRequired);
        }
        AuthMethod::PublicKey => Some(keys::key_ref(request.key_id.trim())),
        AuthMethod::Password | AuthMethod::KeyboardInteractive => None,
    };
    profile.remote_command =
        Some(request.command.trim().to_owned()).filter(|command| !command.is_empty());
    Ok(profile)
}

/// 表单里的密码操作 → 上游的 `SecretUpdate`。私钥与 keyboard-interactive 不存密码
/// （修改时清掉可能存过的；私钥的口令跟着私钥存）；新建时没有可「保留」或「清除」的旧密码。
#[cfg(test)]
fn secret_update(request: &SaveConnection, creating: bool) -> Result<SecretUpdate, CatalogError> {
    secret_update_secured(
        request,
        creating,
        SecretString::from(request.password.clone()),
    )
}

fn secret_update_secured(
    request: &SaveConnection,
    creating: bool,
    password: SecretString,
) -> Result<SecretUpdate, CatalogError> {
    Ok(match (request.auth, request.password_action) {
        (AuthMethod::PublicKey | AuthMethod::KeyboardInteractive, _) if creating => {
            SecretUpdate::Unchanged
        }
        (AuthMethod::PublicKey | AuthMethod::KeyboardInteractive, _) => SecretUpdate::Clear,
        (AuthMethod::Password, PasswordAction::Set) => {
            if password.expose_secret().is_empty() {
                return Err(CatalogError::PasswordRequired);
            }
            SecretUpdate::Set(password)
        }
        (AuthMethod::Password, PasswordAction::Keep) => SecretUpdate::Unchanged,
        (AuthMethod::Password, PasswordAction::Clear) if creating => SecretUpdate::Unchanged,
        (AuthMethod::Password, PasswordAction::Clear) => SecretUpdate::Clear,
    })
}

fn delete(context: &AppContext, id: &str) -> Result<(), CatalogError> {
    let _operations = context
        .key_operations
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let id = parse_id(id)?;
    if !load(context)?.connections.contains_key(&id) {
        return Err(CatalogError::NotFound);
    }
    apply(
        context,
        CatalogMutation::Delete(id),
        SecretUpdate::Unchanged,
    )
    .map(|_| ())
}

/// 复制出的连接排在原连接所在分组的末尾；已存的密码与原连接共用
/// （上游按引用计数删除）。返回新连接的 id。
fn duplicate(context: &AppContext, id: &str) -> Result<String, CatalogError> {
    let _operations = context
        .key_operations
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let source = parse_id(id)?;
    let before = load(context)?;
    let group = before
        .connections
        .get(&source)
        .ok_or(CatalogError::NotFound)?
        .group_id;
    let existing: BTreeSet<ConnectionId> = before.connections.keys().copied().collect();
    let after = apply(
        context,
        CatalogMutation::Duplicate {
            source,
            destination: group,
        },
        SecretUpdate::Unchanged,
    )?;
    after
        .connections
        .keys()
        .find(|id| !existing.contains(id))
        .map(|id| id.0.to_string())
        .ok_or(CatalogError::Storage)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use std::path::PathBuf;
    use std::sync::Arc;

    use super::{
        DesktopSaveDraft, delete, desktop_gate, desktop_profile_from_request, desktop_save,
        desktop_save_secured, desktop_summaries, duplicate, load, profile_from_request, save,
        secret_update, summary,
    };
    use crate::app::AppContext;
    use crate::card::{CardContext, NoCards};
    use crate::keys::{MemoryKeyStore, PreferenceFile};
    use crate::signals::catalog::{
        AuthMethod, CatalogError, DESKTOP_CATALOG_VERSION, DesktopAuthMethod, DesktopCatalogError,
        DesktopCatalogTargetKind, DesktopSaveConnection, DesktopTransport, PasswordAction,
        SaveConnection,
    };
    use rshell_m0::rshell_core::{
        AuthenticationKind, CatalogMutation, ConnectionProfile, CredentialRef, SecretUpdate,
        TerminalProfile, TransportKind,
    };
    use rshell_m0::rshell_storage::{
        ConfigurationChange, CredentialCoordinator, CredentialVault, MemoryCredentialVault,
        SqliteRepository, StorageError,
    };
    use secrecy::ExposeSecret;
    use tokio::sync::Notify;

    /// 内存目录 + 内存钥匙串。
    fn context() -> (AppContext, Arc<MemoryCredentialVault>) {
        let repository = Arc::new(SqliteRepository::open_in_memory().expect("in-memory catalog"));
        repository.migrate().expect("migrate");
        context_for(repository)
    }

    fn context_for(repository: Arc<SqliteRepository>) -> (AppContext, Arc<MemoryCredentialVault>) {
        let vault = Arc::new(MemoryCredentialVault::new());
        let context = AppContext {
            credentials: CredentialCoordinator::new(repository.clone(), vault.clone()),
            repository,
            known_hosts: PathBuf::new(),
            keys: Arc::new(MemoryKeyStore::new(false)),
            key_operations: std::sync::Mutex::new(()),
            cards: Arc::new(CardContext::new(Arc::new(NoCards))),
            security_keys: Arc::new(crate::security_key::Unavailable),
            preferences: PreferenceFile::open(
                std::env::temp_dir().join("guosh-test-preferences.json"),
            ),
            catalog_changed: Notify::new(),
            keys_changed: Notify::new(),
        };
        (context, vault)
    }

    fn desktop_request() -> DesktopSaveConnection {
        DesktopSaveConnection {
            version: DESKTOP_CATALOG_VERSION,
            request_id: 42,
            id: String::new(),
            name: "测试".into(),
            target_kind: DesktopCatalogTargetKind::Ssh,
            transport: DesktopTransport::NativeSsh,
            host: "test.example".into(),
            port: 22,
            username: "tester".into(),
            auth: DesktopAuthMethod::Password,
            password_action: PasswordAction::Set,
            password: "secret-canary-not-for-projection".into(),
            key_id: String::new(),
            system_identity_path: String::new(),
            command: String::new(),
            profile_id: String::new(),
        }
    }

    #[tokio::test]
    async fn remembered_password_only_updates_the_authenticated_catalog_record() {
        for change in [
            "host",
            "user",
            "port",
            "auth",
            "credential",
            "deleted",
            "unchanged",
            "name",
            "note",
        ] {
            let (context, vault) = context();
            let context = Arc::new(context);
            let mut original = ConnectionProfile::new("认证目标", "original.test");
            original.username = "original".into();
            original.transport = TransportKind::NativeSsh;
            original.authentication = AuthenticationKind::Password;
            let catalog = context
                .credentials
                .apply_catalog(
                    CatalogMutation::Create(original.clone()),
                    SecretUpdate::Unchanged,
                )
                .expect("创建目标");
            let original = catalog.connections[&original.id].clone();
            let mut edited = original.clone();
            match change {
                "host" => edited.host = "other.test".into(),
                "user" => edited.username = "other".into(),
                "port" => edited.port = 2222,
                "auth" => edited.authentication = AuthenticationKind::KeyboardInteractive,
                "name" => edited.name = "连接期间改名".into(),
                "note" => {
                    edited.note = "连接期间补充备注".into();
                    edited.tags.insert("新标签".into());
                }
                _ => {}
            }
            let mutation = if change == "deleted" {
                CatalogMutation::Delete(original.id)
            } else {
                CatalogMutation::Update(edited)
            };
            let secret = if change == "credential" {
                SecretUpdate::Set(secrecy::SecretString::from("new secret".to_owned()))
            } else {
                SecretUpdate::Unchanged
            };
            let before = context
                .credentials
                .apply_catalog(mutation, secret)
                .expect("编辑目标");
            let stored = crate::connect::store_password(
                &context,
                original.clone(),
                secrecy::SecretString::from("authenticated secret".to_owned()),
            )
            .await;
            let after = context.repository.load_catalog().expect("读取目录");
            if matches!(change, "unchanged" | "name" | "note") {
                let stored = stored.expect("认证目标未变时允许记住密码");
                // 保存的是目录当前记录：连接期间的无关编辑不被覆盖回旧值。
                let saved = &after.connections[&original.id];
                let edited = &before.connections[&original.id];
                assert_eq!(
                    (&saved.name, &saved.note, &saved.tags),
                    (&edited.name, &edited.note, &edited.tags),
                    "{change}"
                );
                let reference = stored.credential_ref.expect("新引用返回同一会话");
                assert_eq!(
                    vault
                        .get(&reference)
                        .expect("读取凭据")
                        .expect("密码")
                        .expose_secret(),
                    "authenticated secret"
                );
            } else {
                assert!(stored.is_none(), "{change}");
                assert_eq!(before, after, "{change}");
            }
        }
    }

    #[test]
    fn desktop_roundtrip_and_old_native_patch_preserve_profile_reference() {
        let (context, vault) = context();
        let mut terminal = TerminalProfile::p0_default();
        terminal.id = uuid::Uuid::new_v4().into();
        context
            .repository
            .save_terminal_profile(terminal.clone())
            .expect("保存 profile");
        let mut form = desktop_request();
        form.profile_id = terminal.id.0.to_string();
        let id = desktop_save(&context, form).expect("桌面创建");
        let catalog = load(&context).expect("读取目录");
        let entry = &desktop_summaries(&catalog, "test.example")[0];
        assert_eq!(entry.profile_id, terminal.id.0.to_string());
        assert!(entry.password_saved);
        assert!(entry.system_identity_path.is_empty());
        let projection = serde_json::to_string(entry).expect("投影序列化");
        assert!(!projection.contains("secret-canary"));
        assert!(!projection.contains("credential_ref"));
        assert_eq!(
            saved_password(&context, &vault, &id).as_deref(),
            Some("secret-canary-not-for-projection")
        );

        let mut legacy = request(AuthMethod::Password, PasswordAction::Keep, "");
        legacy.id = id.clone();
        legacy.name = "修改名称".into();
        save(&context, legacy).expect("旧版 Native patch");
        let after_legacy = load(&context).expect("目录");
        let updated = &after_legacy.connections[&super::parse_id(&id).expect("id")];
        assert_eq!(updated.terminal_profile_id, Some(terminal.id));
        assert_eq!(updated.transport, TransportKind::NativeSsh);
        assert_eq!(
            saved_password(&context, &vault, &id).as_deref(),
            Some("secret-canary-not-for-projection")
        );

        let mut advanced = after_legacy.connections[&super::parse_id(&id).expect("id")].clone();
        advanced.note = "目录额外字段".into();
        advanced.tags.insert("原标签".into());
        context
            .credentials
            .apply_catalog(CatalogMutation::Update(advanced), SecretUpdate::Unchanged)
            .expect("保存附加字段");

        let mut desktop_edit = desktop_request();
        desktop_edit.id = id.clone();
        desktop_edit.name = "桌面修改".into();
        desktop_edit.profile_id = terminal.id.0.to_string();
        desktop_edit.password_action = PasswordAction::Keep;
        desktop_edit.password.clear();
        desktop_save(&context, desktop_edit).expect("桌面增量编辑");
        let after_desktop = load(&context).expect("目录");
        let saved = &after_desktop.connections[&super::parse_id(&id).expect("id")];
        assert_eq!(saved.note, "目录额外字段");
        assert!(saved.tags.contains("原标签"));
        assert_eq!(saved.terminal_profile_id, Some(terminal.id));
    }

    #[test]
    fn referenced_profile_cannot_be_deleted_after_catalog_save() {
        let (context, _) = context();
        let mut terminal = TerminalProfile::p0_default();
        terminal.id = uuid::Uuid::new_v4().into();
        context
            .repository
            .save_terminal_profile(terminal.clone())
            .expect("保存独立 profile");
        let mut form = desktop_request();
        form.profile_id = terminal.id.0.to_string();
        form.password.clear();
        form.password_action = PasswordAction::Keep;
        let id = desktop_save(&context, form).expect("目录引用 profile");
        let snapshot = context
            .repository
            .load_configuration()
            .expect("加载配置快照");
        assert_eq!(
            context
                .repository
                .commit_configuration(ConfigurationChange {
                    expected_revision: snapshot.revision,
                    settings: snapshot.settings,
                    upsert_profiles: vec![],
                    delete_profiles: vec![terminal.id],
                }),
            Err(StorageError::Constraint)
        );
        assert_eq!(
            load(&context).expect("目录未删除").connections[&super::parse_id(&id).expect("id")]
                .terminal_profile_id,
            Some(terminal.id)
        );
    }

    #[test]
    fn desktop_system_identity_never_reuses_app_key_or_saved_password() {
        let (context, vault) = context();
        let native_id = desktop_save(&context, desktop_request()).expect("Native 创建");
        let mut system = desktop_request();
        system.id = native_id.clone();
        system.transport = DesktopTransport::SystemOpenSsh;
        system.auth = DesktopAuthMethod::PublicKey;
        system.password_action = PasswordAction::Clear;
        system.password.clear();
        system.system_identity_path = "user selected identity".into();
        assert_eq!(desktop_save(&context, system), Ok(native_id.clone()));
        assert!(vault.is_empty());
        let entry = &desktop_summaries(&load(&context).expect("目录"), "")[0];
        assert_eq!(entry.transport, DesktopTransport::SystemOpenSsh);
        assert_eq!(entry.system_identity_path, "user selected identity");
        assert!(!entry.password_saved);
        assert!(entry.key_id.is_empty());
        // 旧列表：移动端隐藏 System 项；桌面列出且不带凭据信息。
        let profile = load(&context)
            .expect("目录")
            .connections
            .into_values()
            .next()
            .expect("System 项");
        assert!(super::summary_for(&profile, false).is_none());
        let listed = super::summary_for(&profile, true).expect("桌面列出 System 项");
        assert_eq!(listed.transport, DesktopTransport::SystemOpenSsh);
        assert!(!listed.password_saved);
        assert!(listed.key_id.is_empty());

        let mut legacy = request(AuthMethod::Password, PasswordAction::Clear, "");
        legacy.id = native_id.clone();
        assert_eq!(save(&context, legacy), Err(CatalogError::Storage));
        assert_eq!(
            desktop_summaries(&load(&context).expect("目录"), "")[0].system_identity_path,
            "user selected identity"
        );

        let mut corrupt = ConnectionProfile::new("旧数据", "test.example");
        corrupt.authentication = AuthenticationKind::PublicKey;
        corrupt.identity_file = Some(PathBuf::from("keychain:private-key-id"));
        assert!(
            super::desktop_summary(&corrupt)
                .system_identity_path
                .is_empty()
        );
    }

    #[test]
    fn desktop_never_inherits_a_system_entry_credential_as_native_password() {
        let (context, vault) = context();
        let mut system = ConnectionProfile::new("旧条目", "test.example");
        system.username = "tester".into();
        system.authentication = AuthenticationKind::PublicKey;
        system.identity_file = Some(PathBuf::from("user selected identity"));
        let id = system.id;
        context
            .credentials
            .apply_catalog(
                CatalogMutation::Create(system),
                SecretUpdate::Set("legacy-credential".into()),
            )
            .expect("导入旧条目");
        let before = load(&context).expect("目录");
        let mut edit = desktop_request();
        edit.id = id.0.to_string();
        edit.password.clear();
        edit.password_action = PasswordAction::Keep;
        assert_eq!(
            desktop_save(&context, edit),
            Err(DesktopCatalogError::CredentialMismatch)
        );
        assert_eq!(load(&context).expect("未修改目录"), before);
        assert!(!vault.is_empty());
    }

    #[test]
    fn desktop_invalid_targets_profiles_and_credentials_fail_before_mutation() {
        let (context, vault) = context();
        let id = desktop_save(&context, desktop_request()).expect("创建");
        let before = load(&context).expect("目录");
        type InvalidCase = (Box<dyn Fn(&mut DesktopSaveConnection)>, DesktopCatalogError);
        let cases: Vec<InvalidCase> = vec![
            (
                Box::new(|form| form.version += 1),
                DesktopCatalogError::UnsupportedVersion,
            ),
            (
                Box::new(|form| form.target_kind = DesktopCatalogTargetKind::LocalShell),
                DesktopCatalogError::TargetMismatch,
            ),
            (
                Box::new(|form| form.transport = DesktopTransport::LocalShell),
                DesktopCatalogError::UnsupportedTransport,
            ),
            (
                Box::new(|form| form.profile_id = "not-an-id".into()),
                DesktopCatalogError::InvalidProfileId,
            ),
            (
                Box::new(|form| form.profile_id = uuid::Uuid::new_v4().to_string()),
                DesktopCatalogError::ProfileNotFound,
            ),
            (
                Box::new(|form| form.auth = DesktopAuthMethod::Agent),
                DesktopCatalogError::InvalidAuthentication,
            ),
            (
                Box::new(|form| form.key_id = "app-private-key".into()),
                DesktopCatalogError::CredentialMismatch,
            ),
            (
                Box::new(|form| form.system_identity_path = "keychain:any".into()),
                DesktopCatalogError::CredentialMismatch,
            ),
            (
                Box::new(|form| {
                    form.transport = DesktopTransport::SystemOpenSsh;
                    form.auth = DesktopAuthMethod::PublicKey;
                    form.password.clear();
                    form.password_action = PasswordAction::Clear;
                    form.system_identity_path = "keychain:any".into();
                }),
                DesktopCatalogError::InvalidIdentity,
            ),
            (
                Box::new(|form| {
                    form.transport = DesktopTransport::SystemOpenSsh;
                    form.auth = DesktopAuthMethod::Agent;
                    form.password.clear();
                    form.password_action = PasswordAction::Keep;
                }),
                DesktopCatalogError::CredentialMismatch,
            ),
            (
                Box::new(|form| form.id = "malformed".into()),
                DesktopCatalogError::InvalidConnectionId,
            ),
            (
                Box::new(|form| form.id = uuid::Uuid::new_v4().to_string()),
                DesktopCatalogError::NotFound,
            ),
        ];
        for (change, expected) in cases {
            let mut form = desktop_request();
            form.id = id.clone();
            change(&mut form);
            assert_eq!(desktop_save(&context, form), Err(expected));
            assert_eq!(load(&context).expect("未修改目录"), before);
            assert_eq!(
                saved_password(&context, &vault, &id).as_deref(),
                Some("secret-canary-not-for-projection")
            );
        }
    }

    #[test]
    fn desktop_file_sqlite_survives_reopen_and_legacy_data_remains_native() {
        let path =
            std::env::temp_dir().join(format!("guosh-catalog-{}.sqlite", uuid::Uuid::new_v4()));
        let repository = Arc::new(SqliteRepository::open(&path).expect("文件数据库"));
        repository.migrate().expect("迁移");
        let (context, _) = context_for(repository);
        let mut request = desktop_request();
        request.password_action = PasswordAction::Keep;
        request.password.clear();
        let id = desktop_save(&context, request).expect("保存文件数据库");
        let mut system = desktop_request();
        system.password_action = PasswordAction::Keep;
        system.password.clear();
        system.transport = DesktopTransport::SystemOpenSsh;
        system.auth = DesktopAuthMethod::PublicKey;
        system.system_identity_path = "用户选择的 identity".into();
        let system_id = desktop_save(&context, system).expect("保存 System SSH");
        drop(context);
        let reopened = SqliteRepository::open(&path).expect("重开数据库");
        let catalog = reopened.load_catalog().expect("持久化目录");
        let profile = &catalog.connections[&super::parse_id(&id).expect("id")];
        assert_eq!(profile.transport, TransportKind::NativeSsh);
        assert_eq!(profile.terminal_profile_id, None);
        let system_profile = &catalog.connections[&super::parse_id(&system_id).expect("id")];
        assert_eq!(system_profile.transport, TransportKind::SystemOpenSsh);
        assert_eq!(
            system_profile.identity_file.as_deref(),
            Some(std::path::Path::new("用户选择的 identity"))
        );
        drop(reopened);
        std::fs::remove_file(&path).expect("清理临时数据库");
    }

    #[test]
    fn desktop_password_canary_never_enters_file_sqlite_or_projection() {
        let path = std::env::temp_dir().join(format!(
            "guosh-catalog-secret-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let repository = Arc::new(SqliteRepository::open(&path).expect("文件数据库"));
        repository.migrate().expect("迁移");
        let (context, vault) = context_for(repository);
        let id = desktop_save(&context, desktop_request()).expect("保存凭据引用");
        let serialized =
            serde_json::to_string(&desktop_summaries(&load(&context).expect("目录"), ""))
                .expect("目录投影");
        assert!(!serialized.contains("secret-canary-not-for-projection"));
        assert_eq!(
            saved_password(&context, &vault, &id).as_deref(),
            Some("secret-canary-not-for-projection")
        );
        drop(context);
        for suffix in ["", "-wal", "-shm"] {
            let file = PathBuf::from(format!("{}{suffix}", path.display()));
            if file.exists() {
                let bytes = std::fs::read(&file).expect("扫描数据库文件");
                assert!(
                    !bytes
                        .windows(b"secret-canary-not-for-projection".len())
                        .any(|window| window == b"secret-canary-not-for-projection"),
                    "凭据不能进入 SQLite 或日志"
                );
                std::fs::remove_file(file).expect("清理临时数据库文件");
            }
        }
    }

    #[test]
    fn desktop_gate_uses_compiled_platform_not_caller_claim() {
        assert_eq!(
            desktop_gate(0),
            Err(DesktopCatalogError::UnsupportedVersion)
        );
        let current = desktop_gate(DESKTOP_CATALOG_VERSION);
        if cfg!(any(
            target_os = "windows",
            target_os = "macos",
            target_os = "linux"
        )) {
            assert_eq!(current, Ok(()));
        } else {
            assert_eq!(current, Err(DesktopCatalogError::DesktopOnly));
        }
    }

    #[test]
    fn desktop_secret_ownership_moves_before_failures_and_into_secret_update_without_clone() {
        let (context, _) = context();
        let form = desktop_request();
        let source = form.password.as_ptr();
        let mut draft = DesktopSaveDraft::new(form);
        assert!(draft.request.password.is_empty());
        assert_eq!(draft.password.expose_secret().as_ptr(), source);
        draft.request.version = 0;
        assert_eq!(
            desktop_save_secured(&context, draft),
            Err(DesktopCatalogError::UnsupportedVersion)
        );

        let form = desktop_request();
        let source = form.password.as_ptr();
        let draft = DesktopSaveDraft::new(form);
        let (_, secret) =
            desktop_profile_from_request(&context, None, &draft.request, draft.password)
                .expect("构造保存变更");
        match secret {
            SecretUpdate::Set(secret) => assert_eq!(secret.expose_secret().as_ptr(), source),
            _ => panic!("密码应通过 SecretUpdate 转移所有权"),
        }

        context.repository.shutdown().expect("关闭数据库");
        let draft = DesktopSaveDraft::new(desktop_request());
        assert_eq!(
            desktop_save_secured(&context, draft),
            Err(DesktopCatalogError::Storage)
        );
        // 指针一致只证明本层没有复制 String；不证明反序列化缓冲或分配器没有历史副本。
    }

    #[tokio::test]
    async fn desktop_save_signal_uses_rinf_decoder_and_request_id() {
        use rinf::DartSignal;
        use zeroize::Zeroize;
        let (context, vault) = context();
        let receiver = DesktopSaveConnection::get_dart_signal_receiver();
        let mut request = desktop_request();
        let request_id = request.request_id;
        let mut encoded = rinf::serialize(&request).expect("rinf 编码");
        request.password.zeroize();
        crate::signals::catalog::inject_desktop_save(&encoded);
        encoded.zeroize();
        let received = tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv())
            .await
            .expect("收到信号")
            .expect("信号通道未关闭")
            .message;
        assert_eq!(received.request_id, request_id);
        let source = received.password.as_ptr();
        let draft = DesktopSaveDraft::new(received);
        assert!(draft.request.password.is_empty());
        assert_eq!(draft.password.expose_secret().as_ptr(), source);
        let result = desktop_save_secured(&context, draft);
        if cfg!(any(
            target_os = "windows",
            target_os = "macos",
            target_os = "linux"
        )) {
            let id = result.expect("桌面目录写入");
            assert!(
                load(&context)
                    .expect("真实目录")
                    .connections
                    .contains_key(&super::parse_id(&id).expect("id"))
            );
            let catalog = load(&context).expect("目录");
            let credential = catalog.connections[&super::parse_id(&id).expect("id")]
                .credential_ref
                .as_ref()
                .expect("凭据引用");
            assert_eq!(
                vault
                    .get(credential)
                    .expect("凭据读取")
                    .expect("已保存凭据")
                    .expose_secret(),
                "secret-canary-not-for-projection"
            );
        } else {
            assert_eq!(result, Err(DesktopCatalogError::DesktopOnly));
        }
    }

    fn saved_password(
        context: &AppContext,
        vault: &MemoryCredentialVault,
        id: &str,
    ) -> Option<String> {
        let catalog = load(context).expect("catalog");
        let profile = catalog
            .connections
            .values()
            .find(|profile| profile.id.0.to_string() == id)
            .expect("connection");
        let reference = profile.credential_ref.as_ref()?;
        vault
            .get(reference)
            .expect("vault")
            .map(|secret| secret.expose_secret().to_owned())
    }

    #[test]
    fn passwords_are_kept_replaced_and_forgotten_through_the_keychain() {
        let (context, vault) = context();
        let id = save(
            &context,
            request(AuthMethod::Password, PasswordAction::Set, "first"),
        )
        .expect("create");
        let connection_id = super::parse_id(&id).expect("id");
        assert_eq!(
            load(&context).expect("catalog").connections[&connection_id].transport,
            TransportKind::NativeSsh
        );
        assert_eq!(
            saved_password(&context, &vault, &id).as_deref(),
            Some("first")
        );

        let mut keep = request(AuthMethod::Password, PasswordAction::Keep, "");
        keep.id = id.clone();
        keep.name = "renamed".to_owned();
        save(&context, keep).expect("keep");
        let after_edit = &load(&context).expect("catalog").connections[&connection_id];
        assert_eq!(after_edit.transport, TransportKind::NativeSsh);
        assert_eq!(after_edit.name, "renamed");
        assert_eq!(
            saved_password(&context, &vault, &id).as_deref(),
            Some("first")
        );

        let mut replace = request(AuthMethod::Password, PasswordAction::Set, "second");
        replace.id = id.clone();
        save(&context, replace).expect("replace");
        assert_eq!(
            saved_password(&context, &vault, &id).as_deref(),
            Some("second")
        );

        let mut forget = request(AuthMethod::Password, PasswordAction::Clear, "");
        forget.id = id.clone();
        save(&context, forget).expect("forget");
        assert_eq!(saved_password(&context, &vault, &id), None);
        assert!(vault.is_empty());
    }

    #[test]
    fn legacy_save_rejects_system_ssh_without_changing_profile_or_credential() {
        let (context, vault) = context();
        let mut profile = ConnectionProfile::new("system", "original.example");
        profile.username = "original".to_owned();
        profile.transport = TransportKind::SystemOpenSsh;
        profile.authentication = AuthenticationKind::PublicKey;
        profile.identity_file = Some(PathBuf::from("existing-key"));
        let id = profile.id;
        context
            .credentials
            .apply_catalog(
                CatalogMutation::Create(profile),
                SecretUpdate::Set("existing-secret".to_owned().into()),
            )
            .expect("create system entry");
        let before = load(&context).expect("catalog").connections[&id].clone();
        let reference = before.credential_ref.as_ref().expect("reference");
        assert_eq!(
            vault
                .get(reference)
                .expect("vault")
                .expect("secret")
                .expose_secret(),
            "existing-secret"
        );

        let mut edit = request(AuthMethod::Password, PasswordAction::Clear, "");
        edit.id = id.0.to_string();
        edit.name = "changed".to_owned();
        edit.host = "other.example".to_owned();
        assert_eq!(save(&context, edit), Err(CatalogError::Storage));
        assert_eq!(load(&context).expect("catalog").connections[&id], before);
        assert_eq!(
            vault
                .get(reference)
                .expect("vault")
                .expect("secret")
                .expose_secret(),
            "existing-secret"
        );

        let mut missing = request(AuthMethod::Password, PasswordAction::Keep, "");
        missing.id = uuid::Uuid::new_v4().to_string();
        assert_eq!(save(&context, missing), Err(CatalogError::NotFound));
    }

    #[test]
    fn a_duplicate_shares_the_saved_password_until_the_last_one_is_deleted() {
        let (context, vault) = context();
        let original = save(
            &context,
            request(AuthMethod::Password, PasswordAction::Set, "shared"),
        )
        .expect("create");
        let copy = duplicate(&context, &original).expect("duplicate");
        assert_ne!(copy, original);
        assert_eq!(
            saved_password(&context, &vault, &copy).as_deref(),
            Some("shared")
        );

        delete(&context, &original).expect("delete original");
        assert_eq!(
            saved_password(&context, &vault, &copy).as_deref(),
            Some("shared")
        );
        delete(&context, &copy).expect("delete copy");
        assert!(vault.is_empty());
        assert_eq!(delete(&context, &copy), Err(CatalogError::NotFound));
    }

    fn request(auth: AuthMethod, action: PasswordAction, password: &str) -> SaveConnection {
        SaveConnection {
            request_id: 1,
            id: String::new(),
            name: "  NAS  ".to_owned(),
            host: " nas.example ".to_owned(),
            port: 22,
            username: " admin ".to_owned(),
            auth,
            password_action: action,
            password: password.to_owned(),
            key_id: String::new(),
            command: "  ".to_owned(),
        }
    }

    #[test]
    fn form_fields_are_trimmed_and_validated() {
        let profile = profile_from_request(
            None,
            &request(AuthMethod::Password, PasswordAction::Keep, ""),
        )
        .expect("valid form");
        assert_eq!(profile.name, "NAS");
        assert_eq!(profile.host, "nas.example");
        assert_eq!(profile.username, "admin");
        assert_eq!(profile.transport, TransportKind::NativeSsh);
        assert_eq!(profile.remote_command, None);

        for (host, port, username, expected) in [
            ("", 22, "admin", CatalogError::HostRequired),
            ("-oProxyCommand=x", 22, "admin", CatalogError::HostInvalid),
            ("two words", 22, "admin", CatalogError::HostInvalid),
            ("nas", 0, "admin", CatalogError::PortInvalid),
            ("nas", 22, " ", CatalogError::UsernameRequired),
        ] {
            let mut form = request(AuthMethod::Password, PasswordAction::Keep, "");
            form.host = host.to_owned();
            form.port = port;
            form.username = username.to_owned();
            assert_eq!(
                profile_from_request(None, &form).map(|_| ()),
                Err(expected),
                "{host:?}:{port} as {username:?}"
            );
        }
    }

    #[test]
    fn password_actions_map_to_secret_updates() {
        let set = request(AuthMethod::Password, PasswordAction::Set, "hunter2");
        assert!(matches!(
            secret_update(&set, true),
            Ok(SecretUpdate::Set(_))
        ));
        let empty = request(AuthMethod::Password, PasswordAction::Set, "");
        assert!(matches!(
            secret_update(&empty, true),
            Err(CatalogError::PasswordRequired)
        ));
        let clear = request(AuthMethod::Password, PasswordAction::Clear, "");
        assert!(matches!(
            secret_update(&clear, true),
            Ok(SecretUpdate::Unchanged)
        ));
        assert!(matches!(
            secret_update(&clear, false),
            Ok(SecretUpdate::Clear)
        ));
        let keyboard = request(AuthMethod::KeyboardInteractive, PasswordAction::Set, "x");
        assert!(matches!(
            secret_update(&keyboard, true),
            Ok(SecretUpdate::Unchanged)
        ));
        assert!(matches!(
            secret_update(&keyboard, false),
            Ok(SecretUpdate::Clear)
        ));
    }

    #[test]
    fn summaries_report_saved_passwords_only_for_password_auth() {
        let mut profile = profile_from_request(
            None,
            &request(AuthMethod::Password, PasswordAction::Keep, ""),
        )
        .expect("valid form");
        profile.credential_ref = Some(CredentialRef::new("rshell://credential/x"));
        assert!(summary(&profile).expect("listed").password_saved);

        profile.authentication = AuthenticationKind::KeyboardInteractive;
        assert!(!summary(&profile).expect("listed").password_saved);
    }

    /// 在存在性检查或删除真正落地前暂停，确定性构造保存与删除的交错。
    struct PausedKeyStore {
        inner: Arc<MemoryKeyStore>,
        pause_delete: bool,
        pause: std::sync::Mutex<
            Option<(
                std::sync::mpsc::SyncSender<()>,
                std::sync::mpsc::Receiver<()>,
            )>,
        >,
    }

    impl PausedKeyStore {
        fn pause_if_needed(&self, item: crate::keys::Item, deleting: bool) {
            if item == crate::keys::Item::Key && deleting == self.pause_delete {
                let pause = self.pause.lock().expect("暂停锁").take();
                if let Some((entered, resume)) = pause {
                    entered.send(()).expect("通知已进入关键区");
                    resume.recv().expect("继续操作");
                }
            }
        }
    }

    impl crate::keys::KeyStore for PausedKeyStore {
        fn put(
            &self,
            item: crate::keys::Item,
            id: &str,
            secret: &[u8],
            synchronized: bool,
        ) -> Result<(), crate::keys::StoreError> {
            self.inner.put(item, id, secret, synchronized)
        }

        fn get_in(
            &self,
            item: crate::keys::Item,
            id: &str,
            synchronized: bool,
        ) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, crate::keys::StoreError> {
            self.pause_if_needed(item, false);
            self.inner.get_in(item, id, synchronized)
        }

        fn delete(
            &self,
            item: crate::keys::Item,
            id: &str,
            synchronized: bool,
        ) -> Result<(), crate::keys::StoreError> {
            self.pause_if_needed(item, true);
            self.inner.delete(item, id, synchronized)
        }

        fn list(
            &self,
            item: crate::keys::Item,
            synchronized: bool,
        ) -> Result<Vec<String>, crate::keys::StoreError> {
            self.inner.list(item, synchronized)
        }

        fn sync_available(&self) -> bool {
            false
        }
    }

    #[test]
    fn 目录保存与私钥删除共享关键区而不产生悬空引用() {
        use crate::keys::{self, KeyStore as _};
        use crate::signals::keys::KeyError;
        use rshell_m0::russh::keys::ssh_key::LineEnding;
        use rshell_m0::russh::keys::{Algorithm, PrivateKey, key::safe_rng};

        for delete_first in [false, true] {
            let (mut context, _) = context();
            let store = Arc::new(MemoryKeyStore::new(false));
            context.keys = store.clone();
            let key = PrivateKey::random(&mut safe_rng(), Algorithm::Ed25519).expect("测试私钥");
            let private = key.to_openssh(LineEnding::LF).expect("测试私钥编码");
            let id = keys::import(&context, "并发测试", &private, "").expect("导入私钥");
            let (entered, wait_entered) = std::sync::mpsc::sync_channel(1);
            let (resume, wait_resume) = std::sync::mpsc::sync_channel(1);
            context.keys = Arc::new(PausedKeyStore {
                inner: store.clone(),
                pause_delete: delete_first,
                pause: std::sync::Mutex::new(Some((entered, wait_resume))),
            });
            let context = Arc::new(context);
            let mut form = request(AuthMethod::PublicKey, PasswordAction::Keep, "");
            form.key_id = id.clone();
            if delete_first {
                let deleting = {
                    let context = context.clone();
                    let id = id.clone();
                    std::thread::spawn(move || keys::delete(&context, &id))
                };
                wait_entered
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .expect("删除暂停");
                assert!(
                    context.key_operations.try_lock().is_err(),
                    "删除前的引用检查与实际删除必须持同一把锁"
                );
                let saving = {
                    let context = context.clone();
                    std::thread::spawn(move || save(&context, form))
                };
                resume.send(()).expect("完成删除");
                assert_eq!(deleting.join().expect("删除线程"), Ok(()));
                assert_eq!(
                    saving.join().expect("保存线程"),
                    Err(CatalogError::KeyRequired)
                );
                assert!(
                    context
                        .repository
                        .load_catalog()
                        .expect("目录")
                        .connections
                        .is_empty()
                );
                assert!(store.get(keys::Item::Key, &id).expect("密钥存储").is_none());
            } else {
                let saving = {
                    let context = context.clone();
                    std::thread::spawn(move || save(&context, form))
                };
                wait_entered
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .expect("保存暂停");
                assert!(
                    context.key_operations.try_lock().is_err(),
                    "密钥存在性检查与目录提交必须持同一把锁"
                );
                let deleting = {
                    let context = context.clone();
                    let id = id.clone();
                    std::thread::spawn(move || keys::delete(&context, &id))
                };
                resume.send(()).expect("完成保存");
                assert!(saving.join().expect("保存线程").is_ok());
                assert_eq!(deleting.join().expect("删除线程"), Err(KeyError::InUse));
                assert_eq!(
                    context
                        .repository
                        .load_catalog()
                        .expect("目录")
                        .connections
                        .len(),
                    1
                );
                assert!(store.get(keys::Item::Key, &id).expect("密钥存储").is_some());
            }
        }
    }
}
