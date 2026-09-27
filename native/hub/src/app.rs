//! 启动：等 Dart 给出数据目录 → 打开存储（SQLite 连接目录 + 钥匙串）→
//! 启动目录、设置与会话三个处理任务。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rinf::{DartSignal, RustSignal, debug_print};
use rshell_m0::rshell_storage::{CredentialCoordinator, SqliteRepository, SystemCredentialVault};
use tokio::sync::Notify;
use tokio::task::spawn_blocking;

use crate::card::{self, CardContext};
use crate::keys::{KeyStore, PreferenceFile};
use crate::security_key::{self, Authenticator};
use crate::signals::{AppReady, AppStart};
use crate::{catalog, keys, session, settings};

const CATALOG_FILE: &str = "catalog.sqlite3";
const KNOWN_HOSTS_FILE: &str = "known_hosts";
const PREFERENCES_FILE: &str = "preferences.json";

/// 进程内共享的存储。SQLite 与钥匙串的调用都是阻塞的，一律经 `spawn_blocking`。
pub struct AppContext {
    pub repository: Arc<SqliteRepository>,
    /// 目录改动与钥匙串读写（两者的一致性由上游负责）。
    pub credentials: CredentialCoordinator,
    /// 本 App 自己的 known_hosts（不是用户的 OpenSSH 文件）。
    pub known_hosts: PathBuf,
    /// 私钥与口令（钥匙串）。
    pub keys: Arc<dyn KeyStore>,
    /// 串行化密钥写入、迁移、删除与连接目录的密钥引用提交。
    pub key_operations: Mutex<()>,
    /// OpenPGP 卡（读卡器、记住的 PIN）。
    pub cards: Arc<CardContext>,
    /// 安全密钥（FIDO2）。
    pub security_keys: Arc<dyn Authenticator>,
    pub preferences: PreferenceFile,
    /// 目录在目录任务之外被改动（连接成功后存密码）时通知它重发。
    pub catalog_changed: Notify,
    /// 私钥在私钥任务之外被改动（连接时存口令）时通知它重发。
    pub keys_changed: Notify,
}

impl AppContext {
    fn open(support_dir: &Path) -> Result<Self, String> {
        rshell_m0::register_credential_store()?;
        let repository = Arc::new(
            SqliteRepository::open(support_dir.join(CATALOG_FILE))
                .map_err(|error| format!("open catalog: {error:?}"))?,
        );
        repository
            .migrate()
            .map_err(|error| format!("migrate catalog: {error:?}"))?;
        let credentials =
            CredentialCoordinator::new(repository.clone(), Arc::new(SystemCredentialVault::new()));
        // 上次没做完的钥匙串写入 / 删除（被杀、崩溃）在这里收尾；失败不挡启动，下次再试。
        if let Err(error) = credentials.reconcile() {
            debug_print!("[app] credential reconcile: {error:?}");
        }
        settings::adopt_app_defaults(&repository)?;
        let context = Self {
            repository,
            credentials,
            known_hosts: support_dir.join(KNOWN_HOSTS_FILE),
            keys: platform_key_store(support_dir)?,
            key_operations: Mutex::new(()),
            cards: Arc::new(CardContext::new(card::platform_reader())),
            security_keys: security_key::platform_authenticator(),
            preferences: PreferenceFile::open(support_dir.join(PREFERENCES_FILE)),
            catalog_changed: Notify::new(),
            keys_changed: Notify::new(),
        };
        if let Err(error) = keys::reconcile_sync(&context) {
            debug_print!("[app] 私钥同步迁移待完成：{error:?}");
        }
        Ok(context)
    }
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
fn platform_key_store(_support_dir: &Path) -> Result<Arc<dyn KeyStore>, String> {
    Ok(Arc::new(keys::KeychainKeyStore::new()?))
}

#[cfg(any(target_os = "android", target_os = "linux"))]
fn platform_key_store(_support_dir: &Path) -> Result<Arc<dyn KeyStore>, String> {
    crate::system_keys::SystemKeyStore::new().map(|store| Arc::new(store) as Arc<dyn KeyStore>)
}

#[cfg(target_os = "windows")]
fn platform_key_store(support_dir: &Path) -> Result<Arc<dyn KeyStore>, String> {
    crate::windows_keys::WindowsKeyStore::new(support_dir.join("protected-keys"))
        .map(|store| Arc::new(store) as Arc<dyn KeyStore>)
}

/// debug 构建的自动连接目标（见 [`AppReady`]）；release 构建与没设环境变量时为空。
fn debug_auto_connect() -> AppReady {
    if !cfg!(debug_assertions) {
        return AppReady::default();
    }
    let var = |name: &str| std::env::var(name).unwrap_or_default();
    AppReady {
        auto_host: var("GUOSH_HOST"),
        auto_port: var("GUOSH_PORT").parse().unwrap_or(22),
        auto_user: var("GUOSH_USER"),
        auto_pass: var("GUOSH_PASS"),
        auto_command: var("GUOSH_CMD"),
        ..AppReady::default()
    }
}

pub async fn run() {
    let start = AppStart::get_dart_signal_receiver();
    let Some(pack) = start.recv().await else {
        return;
    };
    let support_dir = PathBuf::from(pack.message.support_dir);
    let context = match spawn_blocking(move || AppContext::open(&support_dir)).await {
        Ok(Ok(context)) => Arc::new(context),
        Ok(Err(detail)) => {
            AppReady {
                ok: false,
                detail,
                ..AppReady::default()
            }
            .send_signal_to_dart();
            return;
        }
        Err(error) => {
            AppReady {
                ok: false,
                detail: format!("startup task: {error}"),
                ..AppReady::default()
            }
            .send_signal_to_dart();
            return;
        }
    };
    AppReady {
        ok: true,
        ..debug_auto_connect()
    }
    .send_signal_to_dart();

    tokio::spawn(catalog::run(context.clone()));
    tokio::spawn(settings::run(context.clone()));
    tokio::spawn(keys::run(context.clone()));
    session::supervisor(context).await;
}
