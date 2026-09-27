//! 连接目录：查询、保存、删除、复制。目录是上游的 SQLite 存储，密码经上游
//! `CredentialCoordinator` 进钥匙串（目录与钥匙串之间的两阶段提交由它负责）。

use std::collections::BTreeSet;
use std::sync::Arc;

use rinf::{DartSignal, RustSignal, debug_print};
use rshell_m0::rshell_core::{
    AuthenticationKind, CatalogMutation, ConnectionCatalog, ConnectionId, ConnectionProfile,
    SecretUpdate, TransportKind,
};
use rshell_m0::rshell_storage::CredentialOperationError;
use secrecy::SecretString;
use tokio::task::spawn_blocking;

use crate::app::AppContext;
use crate::keys::{self, Item};
use crate::signals::catalog::{
    AuthMethod, CatalogError, CatalogQuery, CatalogResult, CatalogState, ConnectionSummary,
    DeleteConnection, DuplicateConnection, PasswordAction, SaveConnection,
};

pub async fn run(context: Arc<AppContext>) {
    let query_rx = CatalogQuery::get_dart_signal_receiver();
    let save_rx = SaveConnection::get_dart_signal_receiver();
    let delete_rx = DeleteConnection::get_dart_signal_receiver();
    let duplicate_rx = DuplicateConnection::get_dart_signal_receiver();
    let mut query = String::new();
    loop {
        tokio::select! {
            pack = query_rx.recv() => {
                let Some(pack) = pack else { break };
                query = pack.message.query;
            }
            pack = save_rx.recv() => {
                let Some(pack) = pack else { break };
                let request_id = pack.message.request_id;
                let result = blocking(&context, move |context| save(context, pack.message)).await;
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
async fn blocking<T: Send + 'static>(
    context: &Arc<AppContext>,
    operation: impl FnOnce(&AppContext) -> Result<T, CatalogError> + Send + 'static,
) -> Result<T, CatalogError> {
    let context = context.clone();
    spawn_blocking(move || operation(&context))
        .await
        .unwrap_or(Err(CatalogError::Storage))
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

/// 边界上的连接摘要。只有本 App 写这个目录：私钥认证总是引用钥匙串里的私钥，
/// 也不会有 agent 认证。
fn summary(profile: &ConnectionProfile) -> Option<ConnectionSummary> {
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
    })
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
fn save(context: &AppContext, request: SaveConnection) -> Result<String, CatalogError> {
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
    let secret = secret_update(&request, creating)?;
    let id = profile.id;
    let mutation = if creating {
        CatalogMutation::Create(profile)
    } else {
        CatalogMutation::Update(profile)
    };
    apply(context, mutation, secret)?;
    Ok(id.0.to_string())
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

/// 表单 → 连接配置。修改时从目录里的版本出发，表单之外的字段（分组、标签、
/// 终端覆盖项、已存密码的引用）原样保留。
fn profile_from_request(
    existing: Option<ConnectionProfile>,
    request: &SaveConnection,
) -> Result<ConnectionProfile, CatalogError> {
    let (host, username) = validate_target(&request.host, request.port, &request.username)?;

    let mut profile = existing.unwrap_or_else(|| ConnectionProfile::new("", host));
    profile.name = request.name.trim().to_owned();
    profile.host = host.to_owned();
    profile.port = request.port;
    profile.username = username.to_owned();
    profile.transport = TransportKind::NativeSsh;
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
fn secret_update(request: &SaveConnection, creating: bool) -> Result<SecretUpdate, CatalogError> {
    Ok(match (request.auth, request.password_action) {
        (AuthMethod::PublicKey | AuthMethod::KeyboardInteractive, _) if creating => {
            SecretUpdate::Unchanged
        }
        (AuthMethod::PublicKey | AuthMethod::KeyboardInteractive, _) => SecretUpdate::Clear,
        (AuthMethod::Password, PasswordAction::Set) => {
            if request.password.is_empty() {
                return Err(CatalogError::PasswordRequired);
            }
            SecretUpdate::Set(SecretString::from(request.password.clone()))
        }
        (AuthMethod::Password, PasswordAction::Keep) => SecretUpdate::Unchanged,
        (AuthMethod::Password, PasswordAction::Clear) if creating => SecretUpdate::Unchanged,
        (AuthMethod::Password, PasswordAction::Clear) => SecretUpdate::Clear,
    })
}

fn delete(context: &AppContext, id: &str) -> Result<(), CatalogError> {
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

    use super::{delete, duplicate, load, profile_from_request, save, secret_update, summary};
    use crate::app::AppContext;
    use crate::card::{CardContext, NoCards};
    use crate::keys::{MemoryKeyStore, PreferenceFile};
    use crate::signals::catalog::{AuthMethod, CatalogError, PasswordAction, SaveConnection};
    use rshell_m0::rshell_core::{AuthenticationKind, CredentialRef, SecretUpdate, TransportKind};
    use rshell_m0::rshell_storage::{
        CredentialCoordinator, CredentialVault, MemoryCredentialVault, SqliteRepository,
    };
    use secrecy::ExposeSecret;
    use tokio::sync::Notify;

    /// 内存目录 + 内存钥匙串。
    fn context() -> (AppContext, Arc<MemoryCredentialVault>) {
        let repository = Arc::new(SqliteRepository::open_in_memory().expect("in-memory catalog"));
        repository.migrate().expect("migrate");
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
        assert_eq!(
            saved_password(&context, &vault, &id).as_deref(),
            Some("first")
        );

        let mut keep = request(AuthMethod::Password, PasswordAction::Keep, "");
        keep.id = id.clone();
        keep.name = "renamed".to_owned();
        save(&context, keep).expect("keep");
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
