//! 建立连接：连接配置（目录里的，或快速连接的临时目标）→ 认证材料 → 握手
//! （含主机密钥确认）。
//!
//! 期间要用户回答的问题——密码、私钥口令、OpenPGP 卡的 PIN、主机密钥确认、
//! keyboard-interactive——发成 `InteractionPrompt`，回答经会话的 `replies` 通道回来。用户在此期间的其他操作
//! （输入、改尺寸）存进 backlog，连上后按原顺序处理；Disconnect 立即中止连接。
//! 私钥在 OpenPGP 卡或安全密钥上时，签名交给外部签名器，它等用户（PIN、按卡、触摸）的
//! 时间同样不算进连接时限。

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use base64ct::{Base64UrlUnpadded, Encoding as _};
use rinf::{RustSignal, debug_print};
use rshell_m0::rshell_core::{
    AuthenticationKind, CatalogMutation, ConnectionId, ConnectionProfile, HostKeyDecision,
    HostKeyPrompt, InteractionId, InteractionRequest, InteractionResponse,
    KeyboardInteractivePrompt, ResolvedTerminalProfile, SecretUpdate, SessionFailure, TerminalSize,
    TransportKind,
};
/// 本地 shell 的启动描述：桌面目标为上游类型。
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
pub use rshell_m0::rshell_session::LocalLaunch;
use rshell_m0::rshell_session::{
    AuthPlan, ExternalSigner, InteractionBroker, KnownHostsVerifier, NativeSshTransport,
    SessionTransport, TransportRequest, build_system_ssh_argv, interaction_channel,
};
use rshell_m0::russh::keys::{PrivateKey, PublicKey};
use secrecy::{ExposeSecret, SecretString};
use tokio::sync::mpsc::{self, UnboundedReceiver};
use tokio::sync::oneshot;
use tokio::task::spawn_blocking;
use tokio::time::Instant;
use zeroize::Zeroize;

use crate::app::AppContext;
use crate::card::{CardFailure, CardSigner};
use crate::catalog;
use crate::external_signer::{PinQuestion, SignerRequest};
use crate::keys::{self, StoredKey};
use crate::protected_reply::ProtectedReply;
use crate::security_key::{SecurityKeySigner, SkFailure};
use crate::session::SessionCommand;
use crate::signals::interaction::{InteractionPrompt, PromptField, PromptKind};
use crate::signals::keys::KeyError;
use crate::signals::{
    ConnectRequest, DESKTOP_CONNECT_VERSION, DesktopConnectError, DesktopConnectRequest,
    DesktopTargetKind, FailureKind,
};

/// 连接时等网络的总时长上限。等用户（主机密钥确认、问答、PIN、按卡）的时间不算在内——
/// 那段由服务器把关（OpenSSH 的 LoginGraceTime），与 OpenSSH 客户端一致。
const CONNECT_BUDGET: Duration = Duration::from_secs(60);
/// 交给传输层的连接上限（含等用户的时间），只防一直没人回答。
const CONNECT_LIMIT: Duration = Duration::from_secs(30 * 60);
/// 连上之后：这么久收不到服务器的任何东西就发 keepalive，连续这么多个没有回音就算断了
/// （对端悄悄没了——休眠、换网络——一分钟内结束会话，可以重连）。空闲时才发，挂起时不发。
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
const KEEPALIVE_MAX: usize = 3;

type PendingPins = HashMap<u32, oneshot::Sender<Option<(SecretString, bool)>>>;

pub struct Connected {
    pub transport: Box<dyn SessionTransport>,
    pub profile: ConnectionProfile,
}

/// 私钥不在 App 里时代为签名的一方。
enum Signer {
    Card(Arc<CardSigner>),
    SecurityKey(Arc<SecurityKeySigner>),
}

impl Signer {
    fn external(&self) -> Arc<dyn ExternalSigner> {
        match self {
            Self::Card(signer) => signer.clone(),
            Self::SecurityKey(signer) => signer.clone(),
        }
    }

    /// 签名失败时的真正原因（连接失败后取）。
    fn abort(&self) -> Option<Abort> {
        match self {
            Self::Card(signer) => signer.failure().map(card_abort),
            Self::SecurityKey(signer) => signer.failure().map(security_key_abort),
        }
    }
}

/// 连接没有建成的原因。
pub enum Abort {
    /// 窗格关闭或会话通道已释放：首次连接与重连都结束会话。
    Closed,
    /// 用户取消本次交互；重连时保留断开画面，以便再次尝试。
    Cancelled,
    Failed {
        failure: FailureKind,
        detail: String,
    },
}

impl Abort {
    fn failed(failure: FailureKind, detail: String) -> Self {
        Self::Failed { failure, detail }
    }
}

/// 连接过程中会话收到的东西。
pub struct Channels<'a> {
    pub commands: &'a mut UnboundedReceiver<SessionCommand>,
    pub replies: &'a mut UnboundedReceiver<ProtectedReply>,
    /// 连接期间收到的非 Disconnect 命令，连上后按顺序处理。
    pub backlog: &'a mut VecDeque<SessionCommand>,
}

/// 一次连接的远端 PTY 参数；桌面传有效 TERM，旧连接沿用传输层默认值。
pub struct RemoteTerminal<'a> {
    pub size: TerminalSize,
    pub terminal_type: Option<&'a str>,
}

impl Channels<'_> {
    /// 连接期间收到命令：Disconnect（或会话被丢弃）即中止，其余留到连上以后。
    fn defer(&mut self, command: Option<SessionCommand>) -> Result<(), Abort> {
        match command {
            Some(SessionCommand::Disconnect) | None => Err(Abort::Closed),
            Some(command) => {
                self.backlog.push_back(command);
                Ok(())
            }
        }
    }
}

/// 连接目标的配置，以及它是否在目录里（只有目录里的连接能存密码）。
#[derive(Clone)]
pub struct Target {
    pub profile: ConnectionProfile,
    pub saved: bool,
    pub kind: TargetKind,
}

/// 目标的传输种类。
#[derive(Clone)]
pub enum TargetKind {
    Ssh,
    /// 本地 shell：不认证，每次连接（含重连）都按同一启动描述新起进程。
    /// `None` 在连接时取平台默认 shell，工作目录为用户主目录。
    Local(Option<LocalLaunch>),
    /// 系统 OpenSSH：在本地 PTY 中运行系统 `ssh`，配置、agent、known_hosts、主机密钥确认
    /// 与密码提示都由 ssh 自己在终端里处理。每次连接（含重连）重新查找 ssh 并按连接配置组装参数。
    /// 携带的路径只用于测试隔离：作为 `-F <path>` 放在参数最前（产品路径为 `None`）。
    System(Option<PathBuf>),
}

impl Target {
    pub fn ssh(profile: ConnectionProfile, saved: bool) -> Self {
        Self {
            profile,
            saved,
            kind: TargetKind::Ssh,
        }
    }

    /// 系统 ssh；`config` 见 [`TargetKind::System`]。
    pub fn system(profile: ConnectionProfile, saved: bool, config: Option<PathBuf>) -> Self {
        Self {
            profile,
            saved,
            kind: TargetKind::System(config),
        }
    }

    pub fn is_system(&self) -> bool {
        matches!(self.kind, TargetKind::System(_))
    }

    /// 本地 shell；`profile` 只是占位，主机、端口与用户名为空，不进入目录。
    pub fn local(launch: Option<LocalLaunch>) -> Self {
        let mut profile = ConnectionProfile::new(LOCAL_DESCRIPTION, "");
        profile.port = 0;
        Self {
            profile,
            saved: false,
            kind: TargetKind::Local(launch),
        }
    }

    pub fn is_local(&self) -> bool {
        matches!(self.kind, TargetKind::Local(_))
    }
}

const LOCAL_DESCRIPTION: &str = "本地 shell";

/// 移动目标不引用上游本地 PTY 类型：空枚举无法构造，本地目标只能取默认（随后被拒）。
#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
#[derive(Clone)]
pub enum LocalLaunch {}

pub struct DesktopTarget {
    pub request: ConnectRequest,
    pub target: Target,
    pub settings: ResolvedTerminalProfile,
    pub quick_password: SecretString,
    pub(crate) font: Option<RetainedFont>,
}

/// 字体与预约共同克隆；异步加载、运行、离线与关闭持同一资源责任。
#[derive(Clone)]
pub(crate) struct RetainedFont {
    font: crate::profile_fonts::ResolvedProfileFont,
    _lease: Option<crate::desktop_prepare::Lease>,
}

impl std::ops::Deref for RetainedFont {
    type Target = crate::profile_fonts::ResolvedProfileFont;
    fn deref(&self) -> &Self::Target {
        &self.font
    }
}

/// 准备持有只包含非秘密目标和配置；不保存带 password 字段的旧连接 DTO。
pub(crate) struct PreparedTarget {
    pub target: Target,
    pub settings: ResolvedTerminalProfile,
    pub font: RetainedFont,
}

/// 新协议只在桌面编译目标开放；即使移动端伪造信号，也无法进入目标解析或传输层。
pub const fn desktop_supported() -> bool {
    cfg!(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "linux"
    ))
}

/// 校验桌面信号的判别目标及字段组合，不依赖存储或网络。
fn validate_desktop(
    request: &DesktopConnectRequest,
) -> Result<(), (DesktopConnectError, &'static str)> {
    validate_desktop_for::<{ desktop_supported() }>(request)
}

fn validate_desktop_for<const DESKTOP: bool>(
    request: &DesktopConnectRequest,
) -> Result<(), (DesktopConnectError, &'static str)> {
    use DesktopConnectError as Error;
    if request.version != DESKTOP_CONNECT_VERSION {
        return Err((Error::UnsupportedVersion, "version"));
    }
    if !DESKTOP {
        return Err((
            Error::PlatformUnavailable,
            "desktop transport is unavailable on this platform",
        ));
    }
    if request.cols == 0 || request.rows == 0 {
        return Err((Error::InvalidField, "cols/rows"));
    }
    if !request.profile_id.is_empty() && uuid::Uuid::parse_str(&request.profile_id).is_err() {
        return Err((Error::InvalidProfileId, "profile_id"));
    }
    if !request.connection_id.is_empty() {
        // 本地 shell 不在目录里，只能由空 connection_id 表示。
        if request.target == DesktopTargetKind::LocalShell {
            return Err((Error::InvalidField, "connection_id"));
        }
        if uuid::Uuid::parse_str(&request.connection_id).is_err() {
            return Err((Error::InvalidConnectionId, "connection_id"));
        }
        if !request.host.is_empty()
            || request.port != 0
            || !request.username.is_empty()
            || !request.password.is_empty()
            || !request.command.is_empty()
        {
            return Err((Error::InvalidField, "connection_id/temporary_target"));
        }
    } else {
        match request.target {
            DesktopTargetKind::LocalShell => {
                if !request.host.is_empty()
                    || request.port != 0
                    || !request.username.is_empty()
                    || !request.password.is_empty()
                    || !request.command.is_empty()
                {
                    return Err((Error::InvalidField, "local_target"));
                }
            }
            DesktopTargetKind::NativeSsh | DesktopTargetKind::SystemOpenSsh => {
                if catalog::validate_target(&request.host, request.port, &request.username).is_err()
                {
                    return Err((Error::InvalidField, "host/port/username"));
                }
                if request.target == DesktopTargetKind::SystemOpenSsh
                    && !request.password.is_empty()
                {
                    return Err((Error::InvalidField, "password"));
                }
            }
        }
    }
    Ok(())
}

/// 解析目录与终端配置。解析不构造传输层或启动程序；本地 shell 与系统 ssh 在连接时才启动。
pub async fn resolve_desktop(
    context: &Arc<AppContext>,
    request: DesktopConnectRequest,
) -> Result<DesktopTarget, (DesktopConnectError, String)> {
    resolve_desktop_inner(context, request, false, None).await
}

pub(crate) async fn resolve_prepared(
    context: &Arc<AppContext>,
    request: DesktopConnectRequest,
    lease: crate::desktop_prepare::Lease,
) -> (
    Result<PreparedTarget, (DesktopConnectError, String)>,
    crate::desktop_prepare::Lease,
) {
    let result = resolve_desktop_inner(context, request, true, Some(lease.clone()))
        .await
        .and_then(|resolved| {
            Ok(PreparedTarget {
                target: resolved.target,
                settings: resolved.settings,
                font: resolved
                    .font
                    .ok_or((DesktopConnectError::Storage, "font_snapshot".into()))?,
            })
        });
    (result, lease)
}

/// 不可取消的 I/O 连同结果持有预约；外层 future 销毁也不能提前释放。
pub(crate) async fn load_desktop_io<T: Send + 'static>(
    lease: Option<crate::desktop_prepare::Lease>,
    load: impl FnOnce() -> T + Send + 'static,
) -> Result<(T, Option<crate::desktop_prepare::Lease>), tokio::task::JoinError> {
    spawn_blocking(move || {
        let result = load();
        (result, lease)
    })
    .await
}

async fn resolve_desktop_inner(
    context: &Arc<AppContext>,
    mut request: DesktopConnectRequest,
    capture_font: bool,
    lease: Option<crate::desktop_prepare::Lease>,
) -> Result<DesktopTarget, (DesktopConnectError, String)> {
    if let Err((error, field)) = validate_desktop(&request) {
        request.password.zeroize();
        return Err((error, field.to_owned()));
    }
    // 校验后立刻接管秘密；后续目录读取、目标拒绝均不留下普通 String 副本。
    let quick_password = SecretString::from(std::mem::take(&mut request.password));
    let task_context = context.clone();
    let profile_id = request.profile_id.clone();
    let connection_id = request.connection_id.clone();
    // 接回的租约与结果同寿命，解析结果期间也继续计账。
    let (loaded, _io_lease) = load_desktop_io(lease, move || {
        let saved = if connection_id.is_empty() {
            None
        } else {
            let id = uuid::Uuid::parse_str(&connection_id)
                .map(ConnectionId::from)
                .map_err(|_| (DesktopConnectError::InvalidConnectionId, "connection_id"))?;
            Some(
                task_context
                    .repository
                    .load_catalog()
                    .map_err(|_| (DesktopConnectError::Storage, "catalog"))?
                    .connections
                    .get(&id)
                    .cloned()
                    .ok_or((DesktopConnectError::ConnectionNotFound, "connection_id"))?,
            )
        };
        let snapshot = task_context
            .repository
            .load_configuration()
            .map_err(|_| (DesktopConnectError::Storage, "configuration"))?;
        let wanted = if !profile_id.is_empty() {
            profile_id
        } else if let Some(id) = saved
            .as_ref()
            .and_then(|profile| profile.terminal_profile_id)
        {
            id.0.to_string()
        } else {
            snapshot.settings.default_terminal_profile.0.to_string()
        };
        let profile = snapshot
            .profiles
            .iter()
            .find(|profile| profile.id.0.to_string() == wanted)
            .ok_or((DesktopConnectError::ProfileNotFound, "profile_id"))?;
        // 只持有所选有效配置；未知业务元数据仍完整留在 configuration 中。
        let overrides = saved
            .as_ref()
            .map(|profile| profile.terminal_overrides.clone())
            .unwrap_or_default();
        let settings = profile
            .settings
            .resolve(&overrides)
            .with_app_key_bindings(&snapshot.settings.key_bindings);
        let font = if capture_font {
            let root = task_context
                .known_hosts
                .parent()
                .unwrap_or_else(|| std::path::Path::new(""));
            crate::profile_fonts::resolve_profile_font(
                &snapshot,
                profile.id,
                task_context.preferences.get().terminal_font_file,
                root,
            )
        } else {
            None
        };
        Ok::<_, (DesktopConnectError, &'static str)>((settings, saved, font))
    })
    .await
    .map_err(|_| (DesktopConnectError::Storage, "storage task".to_owned()))?;
    let (settings, saved, font) = loaded.map_err(|(error, field)| (error, field.to_owned()))?;

    let expected = match request.target {
        DesktopTargetKind::NativeSsh => Some(TransportKind::NativeSsh),
        DesktopTargetKind::SystemOpenSsh => Some(TransportKind::SystemOpenSsh),
        DesktopTargetKind::LocalShell => None,
    };
    if saved
        .as_ref()
        .is_some_and(|profile| Some(profile.transport) != expected)
    {
        return Err((
            DesktopConnectError::InvalidField,
            "target/connection_id".to_owned(),
        ));
    }
    let native = ConnectRequest {
        session_id: request.session_id,
        connection_id: request.connection_id,
        host: request.host,
        port: request.port,
        username: request.username,
        password: String::new(),
        command: request.command,
        cols: request.cols,
        rows: request.rows,
        pixel_width: request.pixel_width,
        pixel_height: request.pixel_height,
        dpi: request.dpi,
    };
    let target = match (request.target, saved) {
        (DesktopTargetKind::LocalShell, _) => Target::local(None),
        (DesktopTargetKind::SystemOpenSsh, Some(profile)) => {
            Target::system(profile, true, system_ssh_test_config())
        }
        (DesktopTargetKind::NativeSsh, Some(profile)) => Target::ssh(profile, true),
        (kind, None) => {
            let (transport, authentication) = if kind == DesktopTargetKind::SystemOpenSsh {
                (TransportKind::SystemOpenSsh, AuthenticationKind::Agent)
            } else {
                (TransportKind::NativeSsh, AuthenticationKind::Password)
            };
            let profile = quick_profile(&native, transport, authentication).map_err(|_| {
                (
                    DesktopConnectError::InvalidField,
                    "host/port/username".to_owned(),
                )
            })?;
            if transport == TransportKind::SystemOpenSsh {
                Target::system(profile, false, system_ssh_test_config())
            } else {
                Target::ssh(profile, false)
            }
        }
    };
    // 系统 ssh 的参数在准备时就校验（拒绝像选项的主机名等），找不到 ssh 也在此拒绝；
    // 都不启动进程。
    if target.is_system() {
        if build_system_ssh_argv(&target.profile).is_err() {
            return Err((
                DesktopConnectError::InvalidField,
                "system_ssh_argv".to_owned(),
            ));
        }
        if !spawn_blocking(system_ssh_found).await.unwrap_or(false) {
            return Err((
                DesktopConnectError::TransportUnavailable,
                "ssh not found".to_owned(),
            ));
        }
    }
    let resolved = DesktopTarget {
        request: native,
        target,
        settings,
        quick_password,
        font: font.map(|font| RetainedFont {
            font,
            _lease: _io_lease,
        }),
    };
    if capture_font {
        let font = resolved
            .font
            .as_ref()
            .ok_or((DesktopConnectError::Storage, "font_snapshot".to_owned()))?;
        let projection =
            crate::desktop_prepare::project(&resolved.settings, &font.projection.profile_id);
        // 仅计所选冻结对象，不序列化整库；三份覆盖运行、attempt 及 Start/回复副本。
        // 保留目录扩展的完整值，不因忽略未知字段低估实际持有的数据。
        serde_json::to_writer(
            MetadataBudget::default(),
            &(
                &projection,
                &projection,
                &projection,
                &resolved.target.profile,
                &resolved.target.profile,
                &resolved.target.profile,
                &font.projection,
                &font.projection,
                &font.projection,
            ),
        )
        .map_err(|_| {
            (
                DesktopConnectError::InvalidField,
                "snapshot_budget".to_owned(),
            )
        })?;
    }
    Ok(resolved)
}

/// 对实际输出计数，不为检查额度先分配一份 JSON 副本。
#[derive(Default)]
struct MetadataBudget(usize);

impl std::io::Write for MetadataBudget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > crate::desktop_prepare::METADATA_LIMIT.saturating_sub(self.0) {
            return Err(std::io::Error::other("snapshot_budget"));
        }
        self.0 += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Target {
    /// 状态与提示里显示的目标。
    pub fn describe(&self) -> String {
        match self.kind {
            TargetKind::Local(_) => LOCAL_DESCRIPTION.to_owned(),
            TargetKind::Ssh | TargetKind::System(_) => format!(
                "{}@{}:{}",
                self.profile.username, self.profile.host, self.profile.port
            ),
        }
    }
}

/// 快速连接的临时配置：由请求里的字段组成，不进入目录。
fn quick_profile(
    request: &ConnectRequest,
    transport: TransportKind,
    authentication: AuthenticationKind,
) -> Result<ConnectionProfile, Abort> {
    let (host, username) = catalog::validate_target(&request.host, request.port, &request.username)
        .map_err(|error| Abort::failed(FailureKind::InvalidTarget, format!("{error:?}")))?;
    let mut profile = ConnectionProfile::new(format!("{username}@{host}"), host);
    profile.port = request.port;
    profile.username = username.to_owned();
    profile.transport = transport;
    profile.authentication = authentication;
    profile.remote_command =
        Some(request.command.trim().to_owned()).filter(|command| !command.is_empty());
    Ok(profile)
}

/// 找到连接配置：目录里的按 id 取；快速连接由请求里的字段临时组成（密码认证）。
/// 旧路径（移动端）只支持 Native SSH。
pub async fn resolve_target(
    context: &Arc<AppContext>,
    request: &ConnectRequest,
) -> Result<Target, Abort> {
    if request.connection_id.is_empty() {
        let profile = quick_profile(
            request,
            TransportKind::NativeSsh,
            AuthenticationKind::Password,
        )?;
        return Ok(Target::ssh(profile, false));
    }

    let id = uuid::Uuid::parse_str(&request.connection_id)
        .map(ConnectionId::from)
        .map_err(|_| Abort::failed(FailureKind::NotFound, request.connection_id.clone()))?;
    let context = context.clone();
    let catalog = spawn_blocking(move || context.repository.load_catalog())
        .await
        .map_err(|error| Abort::failed(FailureKind::Other, format!("catalog task: {error}")))?
        .map_err(|error| Abort::failed(FailureKind::Other, format!("catalog: {error:?}")))?;
    let profile = catalog
        .connections
        .get(&id)
        .cloned()
        .ok_or_else(|| Abort::failed(FailureKind::NotFound, request.connection_id.clone()))?;
    if profile.transport != TransportKind::NativeSsh {
        return Err(Abort::failed(
            FailureKind::InvalidTarget,
            "目录目标不是 Native SSH".to_owned(),
        ));
    }
    Ok(Target::ssh(profile, true))
}

/// 建立连接。`quick_password` 是快速连接随请求带来的密码（可能为空）。
pub async fn establish(
    context: &Arc<AppContext>,
    session_id: u32,
    target: Target,
    quick_password: SecretString,
    terminal: RemoteTerminal<'_>,
    mut channels: Channels<'_>,
    prompts: &mut PromptIds,
) -> Result<Connected, Abort> {
    let described = target.describe();
    let Target {
        profile,
        saved,
        kind,
    } = target;
    let transport_request = native_transport_request(terminal.size, terminal.terminal_type)?;
    let launch = match kind {
        TargetKind::Ssh => None,
        TargetKind::Local(launch) => Some(launch),
        // 系统 ssh 每次连接都重新组装：重连即重新运行 ssh。
        TargetKind::System(config) => {
            let task_profile = profile.clone();
            let launch = spawn_blocking(move || system_ssh_launch(&task_profile, config))
                .await
                .map_err(|error| {
                    Abort::failed(FailureKind::Other, format!("ssh task: {error}"))
                })??;
            Some(Some(launch))
        }
    };
    if let Some(launch) = launch {
        // 本地 shell 与系统 ssh 不经应用认证、不校验主机密钥（系统 ssh 在终端里自行处理）；
        // TERM 由传输层按请求设入子进程环境。spawn 成功即视为已连接。
        drop(quick_password);
        let transport = connect_local(
            launch,
            session_id,
            &profile,
            &transport_request,
            channels,
            prompts,
        )
        .await?;
        return Ok(Connected { transport, profile });
    }
    // 外部签名器（OpenPGP 卡或安全密钥）与它要会话代办的事（问 PIN、等用户）。
    let mut external: Option<(Signer, UnboundedReceiver<SignerRequest>)> = None;

    // 密码：目录里存了就从钥匙串读（每次连接读，不缓存——PLAN §9.2.2）；
    // 没存、读不到、或快速连接没带，就问用户。
    let mut remember = None;
    let auth = async {
        match profile.authentication {
            AuthenticationKind::Password => {
                let password = match saved_password(context, &profile).await {
                    Some(password) => password,
                    None if !quick_password.expose_secret().is_empty() => quick_password,
                    None => {
                        let question = Question {
                            kind: PromptKind::Password,
                            name: String::new(),
                            can_remember: saved,
                            retry: false,
                        };
                        let (password, keep) = ask_secret(
                            session_id,
                            prompts.next()?,
                            &profile,
                            question,
                            &mut channels,
                        )
                        .await?;
                        if keep && saved {
                            remember =
                                Some(SecretString::from(password.expose_secret().to_owned()));
                        }
                        password
                    }
                };
                AuthPlan::from_secret(&profile, Some(password))
            }
            AuthenticationKind::PublicKey => {
                let (id, stored) = load_key(context, &profile).await?;
                match external_signer(context, session_id, described, &id, &stored)? {
                    Some((signer, receiver, public_key)) => {
                        let plan = AuthPlan::from_signer(&profile, public_key, signer.external());
                        external = Some((signer, receiver));
                        plan
                    }
                    None => {
                        let key = unlock_key(
                            context,
                            session_id,
                            prompts,
                            &profile,
                            id,
                            stored,
                            &mut channels,
                        )
                        .await?;
                        AuthPlan::from_private_key(&profile, Arc::new(key))
                    }
                }
            }
            _ => AuthPlan::from_secret(&profile, None),
        }
        .map_err(|error| Abort::failed(FailureKind::Other, format!("auth plan: {error:?}")))
    };
    let auth = tokio::select! {
        biased;
        () = crate::transport_lifecycle::closing() => return Err(Abort::Closed),
        result = auth => result?,
    };

    // 主机密钥：新主机与变更的密钥都问用户（变更时带 changed，Dart 给出醒目警告）。
    let verifier = KnownHostsVerifier::new(&context.known_hosts)
        .with_changed_key_prompt()
        .with_timeout(CONNECT_LIMIT);
    let transport = NativeSshTransport::new(profile.clone(), auth, verifier)
        .and_then(|transport| transport.with_connect_timeout(CONNECT_LIMIT))
        .and_then(|transport| transport.with_keepalive(KEEPALIVE_INTERVAL, KEEPALIVE_MAX))
        .map_err(|error| {
            Abort::failed(
                failure_kind(error.failure()),
                format!("transport: {error:?}"),
            )
        })?;

    let mut transport: Box<dyn SessionTransport> = Box::new(transport);
    let (signer, signer_requests) = external.unzip();
    let result = handshake(
        transport.as_mut(),
        session_id,
        &profile,
        &transport_request,
        channels,
        prompts,
        signer_requests,
    )
    .await;
    if let Err(error) = result {
        // shutdown 超时不能被认证表象覆盖。
        if matches!(&error, Abort::Failed { detail, .. } if detail.starts_with("transport_shutdown_"))
        {
            return Err(error);
        }
        if let Some(abort) = signer.as_ref().and_then(Signer::abort) {
            return Err(abort);
        }
        return Err(error);
    }
    if let Some(password) = remember {
        tokio::select! {
            biased;
            () = crate::transport_lifecycle::closing() => {
                if let Err(detail) = crate::transport_lifecycle::shutdown(transport.as_mut()).await {
                    return Err(Abort::failed(detail.kind(), detail.detail().into()));
                }
                return Err(Abort::Closed);
            }
            () = store_password(context, profile.id, password) => {}
        }
    }
    Ok(Connected { transport, profile })
}

pub(crate) async fn handshake(
    transport: &mut dyn SessionTransport,
    session_id: u32,
    profile: &ConnectionProfile,
    transport_request: &TransportRequest,
    mut channels: Channels<'_>,
    prompts: &mut PromptIds,
    mut signer_requests: Option<UnboundedReceiver<SignerRequest>>,
) -> Result<(), Abort> {
    let (broker, mut requests) = interaction_channel();
    let result = async {
        let connect = transport.connect(transport_request, broker.clone());
        tokio::pin!(connect);
        let mut pending = HashMap::new();
        let mut pending_pins = PendingPins::new();
        let mut budget = Budget::new(CONNECT_BUDGET);
        let mut signer_waiting = false;
        loop {
            budget.pause(!pending.is_empty() || !pending_pins.is_empty() || signer_waiting);
            tokio::select! {
                biased;
                () = crate::transport_lifecycle::closing() => return Err(Abort::Closed),
                result = &mut connect => break result.map_err(|error| Abort::failed(failure_kind(error.failure()), format!("connect: {error:?}"))),
                () = budget.expired() => {
                    return Err(Abort::failed(FailureKind::Timeout, "connect: timed out".to_owned()));
                }
                request = requests.recv() => {
                    if let Some((id, request)) = request {
                        forward_prompt(session_id, profile, &broker, prompts, &mut pending, id, request)?;
                    }
                }
                request = next_signer_request(&mut signer_requests) => match request {
                    Some(SignerRequest::Pin { question, reply }) => {
                        let prompt_id = prompts.next()?;
                        send_prompt(card_pin_prompt(session_id, prompt_id, profile, question));
                        pending_pins.insert(prompt_id, reply);
                    }
                    Some(SignerRequest::Waiting(waiting)) => signer_waiting = waiting,
                    None => signer_requests = None,
                },
                reply = channels.replies.recv() => {
                    let Some(reply) = reply else { return Err(Abort::Closed) };
                    forward_reply(&broker, &mut pending, &mut pending_pins, reply);
                }
                command = channels.commands.recv() => channels.defer(command)?,
            }
        }
    }.await;
    if let Err(error) = result {
        if let Err(detail) = crate::transport_lifecycle::shutdown(transport).await {
            return Err(Abort::failed(detail.kind(), detail.detail().into()));
        }
        return Err(error);
    }
    Ok(())
}

/// 启动本地 shell 并完成连接；未指定启动描述时取平台默认 shell。
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
async fn connect_local(
    launch: Option<LocalLaunch>,
    session_id: u32,
    profile: &ConnectionProfile,
    transport_request: &TransportRequest,
    channels: Channels<'_>,
    prompts: &mut PromptIds,
) -> Result<Box<dyn SessionTransport>, Abort> {
    let launch = match launch {
        Some(launch) => launch,
        None => spawn_blocking(default_local_launch)
            .await
            .map_err(|error| Abort::failed(FailureKind::Other, format!("shell task: {error}")))??,
    };
    let mut transport: Box<dyn SessionTransport> =
        Box::new(rshell_m0::rshell_session::LocalPtyTransport::launch(launch));
    handshake(
        transport.as_mut(),
        session_id,
        profile,
        transport_request,
        channels,
        prompts,
        None,
    )
    .await?;
    Ok(transport)
}

/// 移动目标不编译本地 PTY 传输（让 iOS 链接时剥离 PTY/进程符号）；桌面协议在校验时
/// 已以 PlatformUnavailable 拒绝，这里只作兜底拒绝。
#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
async fn connect_local(
    launch: Option<LocalLaunch>,
    _session_id: u32,
    _profile: &ConnectionProfile,
    _transport_request: &TransportRequest,
    _channels: Channels<'_>,
    _prompts: &mut PromptIds,
) -> Result<Box<dyn SessionTransport>, Abort> {
    match launch {
        Some(never) => match never {},
        None => Err(Abort::failed(
            FailureKind::InvalidTarget,
            "local shell unavailable".to_owned(),
        )),
    }
}

/// 系统 ssh 的启动描述：在 PATH 中查找 ssh，参数由上游按连接配置组装（含参数校验与
/// `StrictHostKeyChecking=ask`）；环境沿用应用进程，用户自己的 ssh 配置照常生效。
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
fn system_ssh_launch(
    profile: &ConnectionProfile,
    config: Option<PathBuf>,
) -> Result<LocalLaunch, Abort> {
    let program = rshell_platform::ssh_executable()
        .map_err(|_| Abort::failed(FailureKind::Other, "ssh not found".to_owned()))?;
    Ok(LocalLaunch::Command {
        program,
        args: system_ssh_args(profile, config)?,
        cwd: None,
        env: std::collections::BTreeMap::new(),
    })
}

/// 是否找到系统 ssh：只看 PATH 中的文件是否存在，不执行它。移动目标恒为 false。
pub(crate) fn system_ssh_found() -> bool {
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    {
        rshell_platform::ssh_executable().is_ok()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        false
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn system_ssh_launch(
    _profile: &ConnectionProfile,
    _config: Option<PathBuf>,
) -> Result<LocalLaunch, Abort> {
    Err(Abort::failed(
        FailureKind::InvalidTarget,
        "system ssh unavailable".to_owned(),
    ))
}

/// 系统 ssh 的参数：可选的测试配置 `-F <path>` 在最前，其余全部来自上游组装。
pub(crate) fn system_ssh_args(
    profile: &ConnectionProfile,
    config: Option<PathBuf>,
) -> Result<Vec<std::ffi::OsString>, Abort> {
    let mut args = Vec::new();
    if let Some(config) = config {
        args.push("-F".into());
        args.push(config.into_os_string());
    }
    args.extend(build_system_ssh_argv(profile).map_err(|_| {
        Abort::failed(
            FailureKind::InvalidTarget,
            "system ssh arguments".to_owned(),
        )
    })?);
    Ok(args)
}

/// 仅 debug 构建读取 `GUOSH_SYSTEM_SSH_CONFIG`：端到端测试借它让系统 ssh 使用隔离的
/// `-F` 配置、不碰用户自己的 ~/.ssh。release 构建不读取，产品路径恒为 `None`。
fn system_ssh_test_config() -> Option<PathBuf> {
    #[cfg(debug_assertions)]
    {
        std::env::var_os("GUOSH_SYSTEM_SSH_CONFIG")
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
    }
    #[cfg(not(debug_assertions))]
    {
        None
    }
}

/// 平台默认 shell，工作目录为用户主目录（取不到时沿用当前目录）。
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
fn default_local_launch() -> Result<LocalLaunch, Abort> {
    let shell = rshell_platform::default_local_shell()
        .map_err(|_| Abort::failed(FailureKind::Other, "local shell not found".to_owned()))?;
    Ok(LocalLaunch::Command {
        program: shell.program,
        args: shell.args,
        cwd: std::env::home_dir(),
        env: shell.env,
    })
}

/// 旧会话维持上游默认 TERM；桌面会话由不可变的有效配置指定远端 PTY 的 TERM。
fn native_transport_request(
    size: TerminalSize,
    terminal_type: Option<&str>,
) -> Result<TransportRequest, Abort> {
    let request = TransportRequest::new(size);
    match terminal_type {
        Some(terminal_type) => request
            .with_terminal_type(terminal_type)
            .map_err(|_| Abort::failed(FailureKind::InvalidTarget, "terminal_type".to_owned())),
        None => Ok(request),
    }
}

/// 连接的时限，只在等网络时走：有问题等用户回答时暂停。
struct Budget {
    deadline: Instant,
    paused_at: Option<Instant>,
}

impl Budget {
    fn new(limit: Duration) -> Self {
        Self {
            deadline: Instant::now() + limit,
            paused_at: None,
        }
    }

    fn pause(&mut self, paused: bool) {
        let now = Instant::now();
        match (paused, self.paused_at) {
            (true, None) => self.paused_at = Some(now),
            (false, Some(since)) => {
                self.deadline += now - since;
                self.paused_at = None;
            }
            _ => {}
        }
    }

    async fn expired(&self) {
        match self.paused_at {
            Some(_) => std::future::pending().await,
            None => tokio::time::sleep_until(self.deadline).await,
        }
    }
}

/// 整个会话内递增的问题编号，由会话持有，重连不会重新分配旧编号。
#[derive(Default)]
pub struct PromptIds(u32);

impl PromptIds {
    fn next(&mut self) -> Result<u32, Abort> {
        self.0 = self
            .0
            .checked_add(1)
            .ok_or_else(|| Abort::failed(FailureKind::Other, "会话交互编号已用尽".to_owned()))?;
        Ok(self.0)
    }
}

async fn saved_password(
    context: &Arc<AppContext>,
    profile: &ConnectionProfile,
) -> Option<SecretString> {
    let reference = profile.credential_ref.clone()?;
    let context = context.clone();
    match spawn_blocking(move || context.credentials.get(&reference)).await {
        Ok(Ok(password)) => password,
        Ok(Err(error)) => {
            // 读不到（钥匙串拒绝等）就改为询问，不让连接直接失败。
            debug_print!("[connect] keychain read: {error:?}");
            None
        }
        Err(error) => {
            debug_print!("[connect] keychain task: {error}");
            None
        }
    }
}

/// hub 自己问的一个秘密（密码或私钥口令）。
struct Question {
    kind: PromptKind,
    /// Passphrase：私钥名称。
    name: String,
    can_remember: bool,
    retry: bool,
}

/// 问一个秘密。返回回答与「存进钥匙串」。
async fn ask_secret(
    session_id: u32,
    prompt_id: u32,
    profile: &ConnectionProfile,
    question: Question,
    channels: &mut Channels<'_>,
) -> Result<(SecretString, bool), Abort> {
    send_prompt(InteractionPrompt {
        name: question.name,
        can_remember: question.can_remember,
        retry: question.retry,
        ..empty_prompt(session_id, prompt_id, question.kind, profile)
    });
    loop {
        tokio::select! {
            biased;
            () = crate::transport_lifecycle::closing() => return Err(Abort::Closed),
            reply = channels.replies.recv() => {
                let Some(mut reply) = reply else { return Err(Abort::Closed) };
                if reply.prompt_id != prompt_id {
                    continue;
                }
                if !reply.accept {
                    return Err(Abort::Cancelled);
                }
                let password = reply.first_secret().unwrap_or_else(|| SecretString::from(String::new()));
                return Ok((password, reply.remember));
            }
            command = channels.commands.recv() => channels.defer(command)?,
        }
    }
}

/// 私钥在 OpenPGP 卡或安全密钥上时，建对应的签名器与它的请求通道；普通私钥返回 `None`。
fn external_signer(
    context: &Arc<AppContext>,
    session_id: u32,
    described: String,
    id: &str,
    stored: &StoredKey,
) -> Result<Option<(Signer, UnboundedReceiver<SignerRequest>, PublicKey)>, Abort> {
    let not_found = || Abort::failed(FailureKind::KeyNotFound, id.to_owned());
    let (requests, receiver) = mpsc::unbounded_channel();
    let (signer, public_key) = match (&stored.card, &stored.security_key) {
        (Some(ident), _) => {
            let public_key =
                PublicKey::from_openssh(&stored.public_key).map_err(|_| not_found())?;
            let signer = CardSigner::new(
                context.cards.clone(),
                session_id,
                described,
                stored.name.clone(),
                ident.clone(),
                public_key.clone(),
                requests,
            );
            (Signer::Card(Arc::new(signer)), public_key)
        }
        (None, Some(reference)) => {
            let public_key =
                PublicKey::from_openssh(&stored.public_key).map_err(|_| not_found())?;
            let credential_id =
                Base64UrlUnpadded::decode_vec(&reference.credential_id).map_err(|_| not_found())?;
            let signer = SecurityKeySigner::new(
                context.security_keys.clone(),
                session_id,
                described,
                reference.application.clone(),
                credential_id,
                requests,
            );
            (Signer::SecurityKey(Arc::new(signer)), public_key)
        }
        (None, None) => return Ok(None),
    };
    Ok(Some((signer, receiver, public_key)))
}

/// 从钥匙串取出连接用的私钥（或 OpenPGP 卡、安全密钥的登记）。
async fn load_key(
    context: &Arc<AppContext>,
    profile: &ConnectionProfile,
) -> Result<(String, StoredKey), Abort> {
    let id = keys::key_id(profile)
        .map(str::to_owned)
        .ok_or_else(|| Abort::failed(FailureKind::KeyNotFound, String::new()))?;
    let task_context = context.clone();
    let task_id = id.clone();
    let stored = spawn_blocking(move || keys::load(&task_context, &task_id))
        .await
        .map_err(|error| Abort::failed(FailureKind::Other, format!("key task: {error}")))?
        .map_err(|error| Abort::failed(FailureKind::Keychain, format!("key: {error:?}")))?
        .ok_or_else(|| Abort::failed(FailureKind::KeyNotFound, id.clone()))?;
    Ok((id, stored))
}

/// 解开钥匙串里的私钥：加密的先用存下的口令，解不开就问用户（口令错了带 `retry`
/// 再问）。勾选保存的口令在解开后存进私钥所在的存储。
async fn unlock_key(
    context: &Arc<AppContext>,
    session_id: u32,
    prompts: &mut PromptIds,
    profile: &ConnectionProfile,
    id: String,
    stored: StoredKey,
    channels: &mut Channels<'_>,
) -> Result<PrivateKey, Abort> {
    if !stored.encrypted {
        return decode_key(&stored.private_key, None)
            .await
            .map_err(|error| Abort::failed(FailureKind::Other, format!("key: {error:?}")));
    }
    if let Some(passphrase) = &stored.passphrase
        && let Ok(key) = decode_key(&stored.private_key, Some(passphrase)).await
    {
        return Ok(key);
    }
    let mut retry = false;
    loop {
        let question = Question {
            kind: PromptKind::Passphrase,
            name: stored.name.clone(),
            can_remember: true,
            retry,
        };
        let (passphrase, keep) =
            ask_secret(session_id, prompts.next()?, profile, question, channels).await?;
        let Ok(key) = decode_key(&stored.private_key, Some(&passphrase)).await else {
            retry = true;
            continue;
        };
        if keep {
            let task_context = context.clone();
            let task_id = id.clone();
            let synchronized = stored.synchronized;
            let saved = spawn_blocking(move || {
                keys::save_passphrase(&task_context, &task_id, &passphrase, synchronized)
            })
            .await;
            match saved {
                Ok(Ok(())) => context.keys_changed.notify_one(),
                Ok(Err(error)) => debug_print!("[connect] saving passphrase: {error:?}"),
                Err(error) => debug_print!("[connect] saving passphrase task: {error}"),
            }
        }
        return Ok(key);
    }
}

/// 解出私钥（加密私钥的密钥派生较慢，放到阻塞线程）。
async fn decode_key(
    private_key: &SecretString,
    passphrase: Option<&SecretString>,
) -> Result<PrivateKey, KeyError> {
    let private_key = SecretString::from(private_key.expose_secret().to_owned());
    let passphrase =
        passphrase.map(|passphrase| SecretString::from(passphrase.expose_secret().to_owned()));
    spawn_blocking(move || {
        keys::decode(
            private_key.expose_secret(),
            passphrase.as_ref().map(ExposeSecret::expose_secret),
        )
    })
    .await
    .unwrap_or(Err(KeyError::Invalid))
}

/// 外部签名器要会话代办的下一件事；没有外部签名器（或它已结束）时永远等待。
async fn next_signer_request(
    requests: &mut Option<UnboundedReceiver<SignerRequest>>,
) -> Option<SignerRequest> {
    match requests {
        Some(requests) => requests.recv().await,
        None => std::future::pending().await,
    }
}

fn card_pin_prompt(
    session_id: u32,
    prompt_id: u32,
    profile: &ConnectionProfile,
    question: PinQuestion,
) -> InteractionPrompt {
    InteractionPrompt {
        name: question.key_name,
        can_remember: true,
        retry: question.retry,
        tries_left: question.tries_left.map_or(-1, i32::from),
        ..empty_prompt(session_id, prompt_id, PromptKind::CardPin, profile)
    }
}

/// PIN 框的回答：PIN 与「记住到退出 App」；取消为 `None`。
fn pin_answer(mut reply: ProtectedReply) -> Option<(SecretString, bool)> {
    if !reply.accept {
        return None;
    }
    let pin = reply.first_secret()?;
    Some((pin, reply.remember))
}

/// 卡签名失败的原因 → 连接失败的分类。
fn card_abort(failure: CardFailure) -> Abort {
    let kind = match &failure {
        CardFailure::Cancelled => return Abort::Cancelled,
        CardFailure::NotFound => FailureKind::CardNotFound,
        CardFailure::KeyMismatch => FailureKind::CardKeyMismatch,
        CardFailure::Unsupported => FailureKind::CardUnsupported,
        CardFailure::PinBlocked | CardFailure::PinWrong { tries_left: 0 } => {
            FailureKind::CardPinBlocked
        }
        CardFailure::PinWrong { .. } => FailureKind::Authentication,
        CardFailure::TouchTimeout => FailureKind::CardTouchTimeout,
        CardFailure::Io(_) => FailureKind::CardError,
    };
    Abort::failed(kind, format!("card: {failure:?}"))
}

/// 安全密钥签名失败的原因 → 连接失败的分类。
fn security_key_abort(failure: SkFailure) -> Abort {
    match failure {
        SkFailure::Cancelled => Abort::Cancelled,
        failure => Abort::failed(
            FailureKind::SecurityKeyFailed,
            format!("security key: {failure:?}"),
        ),
    }
}

/// 上游 broker 的问题 → Dart。上游只会问主机密钥与 keyboard-interactive；
/// 其他种类（不会出现）直接取消。keyboard-interactive 里什么都不用填、也没有
/// 说明文字的一轮（PAM 认证通过后常见的收尾请求）直接回空答案，不打扰用户——
/// 与 OpenSSH 客户端的做法一致。
fn forward_prompt(
    session_id: u32,
    profile: &ConnectionProfile,
    broker: &InteractionBroker,
    prompts: &mut PromptIds,
    pending: &mut HashMap<u32, (InteractionId, PromptKind)>,
    id: InteractionId,
    request: InteractionRequest,
) -> Result<(), Abort> {
    let prompt_id = prompts.next()?;
    let prompt = match request {
        InteractionRequest::HostKey(host_key) => {
            host_key_prompt(session_id, prompt_id, profile, host_key)
        }
        InteractionRequest::KeyboardInteractive(questions) if is_empty_round(&questions) => {
            let _ = broker.respond(id, InteractionResponse::Answers(Vec::new()));
            return Ok(());
        }
        InteractionRequest::KeyboardInteractive(questions) => {
            keyboard_interactive_prompt(session_id, prompt_id, profile, questions)
        }
        InteractionRequest::Password(_) | InteractionRequest::PrivateKeyPassphrase(_) => {
            let _ = broker.respond(id, InteractionResponse::Cancel);
            return Ok(());
        }
    };
    pending.insert(prompt_id, (id, prompt.kind));
    send_prompt(prompt);
    Ok(())
}

fn is_empty_round(questions: &KeyboardInteractivePrompt) -> bool {
    questions.prompts.is_empty()
        && questions.name.trim().is_empty()
        && questions.instruction.trim().is_empty()
}

/// 只把答案交给本轮仍在等待的提示；前一次连接遗留的答案直接丢弃。
fn forward_reply(
    broker: &InteractionBroker,
    pending: &mut HashMap<u32, (InteractionId, PromptKind)>,
    pending_pins: &mut PendingPins,
    reply: ProtectedReply,
) {
    if let Some((id, kind)) = pending.remove(&reply.prompt_id) {
        let _ = broker.respond(id, broker_response(kind, reply));
    } else if let Some(answer) = pending_pins.remove(&reply.prompt_id) {
        let _ = answer.send(pin_answer(reply));
    }
}

fn empty_prompt(
    session_id: u32,
    prompt_id: u32,
    kind: PromptKind,
    profile: &ConnectionProfile,
) -> InteractionPrompt {
    InteractionPrompt {
        session_id,
        prompt_id,
        kind,
        username: profile.username.clone(),
        host: profile.host.clone(),
        port: profile.port,
        address: String::new(),
        algorithm: String::new(),
        fingerprint: String::new(),
        changed: false,
        name: String::new(),
        instruction: String::new(),
        fields: Vec::new(),
        can_remember: false,
        retry: false,
        tries_left: -1,
    }
}

fn send_prompt(prompt: InteractionPrompt) {
    match crate::desktop_runtime::SOURCE.try_with(Clone::clone) {
        Ok(source) => crate::signals::desktop_session::DesktopPrompt {
            source: source.source,
            identity: source.identity,
            prompt,
        }
        .send_signal_to_dart(),
        Err(_) => prompt.send_signal_to_dart(),
    }
}

fn host_key_prompt(
    session_id: u32,
    prompt_id: u32,
    profile: &ConnectionProfile,
    host_key: HostKeyPrompt,
) -> InteractionPrompt {
    InteractionPrompt {
        address: host_key.host,
        algorithm: host_key.algorithm,
        fingerprint: host_key.sha256,
        changed: host_key.changed,
        ..empty_prompt(session_id, prompt_id, PromptKind::HostKey, profile)
    }
}

fn keyboard_interactive_prompt(
    session_id: u32,
    prompt_id: u32,
    profile: &ConnectionProfile,
    questions: KeyboardInteractivePrompt,
) -> InteractionPrompt {
    InteractionPrompt {
        name: questions.name,
        instruction: questions.instruction,
        fields: questions
            .prompts
            .into_iter()
            .map(|prompt| PromptField {
                label: prompt.label,
                echo: prompt.echo,
            })
            .collect(),
        ..empty_prompt(
            session_id,
            prompt_id,
            PromptKind::KeyboardInteractive,
            profile,
        )
    }
}

/// Dart 的回答 → 上游 broker 的回答。拒绝主机密钥是 Reject（连接以
/// HostKeyRejected / HostKeyChanged 失败），其余的「不答」是 Cancel。
fn broker_response(kind: PromptKind, mut reply: ProtectedReply) -> InteractionResponse {
    match (kind, reply.accept) {
        (PromptKind::HostKey, true) => {
            InteractionResponse::HostKey(HostKeyDecision::AcceptAndStore)
        }
        (PromptKind::HostKey, false) => InteractionResponse::HostKey(HostKeyDecision::Reject),
        (PromptKind::KeyboardInteractive, true) => {
            InteractionResponse::Answers(reply.all_secrets())
        }
        (
            PromptKind::KeyboardInteractive
            | PromptKind::Password
            | PromptKind::Passphrase
            | PromptKind::CardPin,
            _,
        ) => InteractionResponse::Cancel,
    }
}

/// 连接成功后把用户输入的密码存进钥匙串（目录与钥匙串的一致性由上游
/// `CredentialCoordinator` 保证）。存不进去不影响本次会话。
async fn store_password(context: &Arc<AppContext>, id: ConnectionId, password: SecretString) {
    let task_context = context.clone();
    let stored = spawn_blocking(move || {
        let catalog = task_context
            .repository
            .load_catalog()
            .map_err(|error| format!("{error:?}"))?;
        // 取目录里的当前版本：连接期间用户可能刚改过这条连接。
        let Some(profile) = catalog.connections.get(&id).cloned() else {
            return Ok(false);
        };
        task_context
            .credentials
            .apply_catalog(
                CatalogMutation::Update(profile),
                SecretUpdate::Set(password),
            )
            .map(|_| true)
            .map_err(|error| format!("{error:?}"))
    })
    .await;
    match stored {
        Ok(Ok(true)) => context.catalog_changed.notify_one(),
        Ok(Ok(false)) => {}
        Ok(Err(error)) => debug_print!("[connect] saving password: {error}"),
        Err(error) => debug_print!("[connect] saving password task: {error}"),
    }
}

/// 上游的失败分类 → 边界上的分类。
pub fn failure_kind(failure: SessionFailure) -> FailureKind {
    match failure {
        SessionFailure::Authentication => FailureKind::Authentication,
        SessionFailure::HostKeyRejected => FailureKind::HostKeyRejected,
        SessionFailure::HostKeyChanged => FailureKind::HostKeyChanged,
        SessionFailure::Network => FailureKind::Network,
        SessionFailure::Timeout => FailureKind::Timeout,
        SessionFailure::Vault => FailureKind::Keychain,
        _ => FailureKind::Other,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::{
        Abort, Budget, PendingPins, PromptIds, PromptKind, broker_response, forward_prompt,
        forward_reply, is_empty_round, native_transport_request, validate_desktop,
        validate_desktop_for,
    };
    use crate::signals::interaction::InteractionReply;
    use crate::signals::{
        DESKTOP_CONNECT_VERSION, DesktopConnectError, DesktopConnectRequest, DesktopTargetKind,
    };
    use rshell_m0::rshell_core::{
        AuthPrompt, ConnectionProfile, HostKeyDecision, HostKeyPrompt, InteractionId,
        InteractionRequest, InteractionResponse, KeyboardInteractivePrompt, TerminalSize,
    };
    use rshell_m0::rshell_session::interaction_channel;
    use secrecy::ExposeSecret;
    use std::collections::HashMap;
    use std::time::Duration;

    #[test]
    fn transport_request_uses_desktop_term_and_preserves_legacy_default() {
        let size = TerminalSize {
            cols: 80,
            rows: 24,
            pixel_width: 640,
            pixel_height: 480,
            dpi: 96,
        };
        let desktop = native_transport_request(size, Some("vt220"))
            .ok()
            .expect("桌面 TERM");
        assert_eq!(desktop.initial_size(), size);
        assert_eq!(desktop.terminal_type(), "vt220");
        let legacy = native_transport_request(size, None)
            .ok()
            .expect("旧连接默认 TERM");
        assert_eq!(legacy.terminal_type(), "xterm-256color");
        assert!(matches!(
            native_transport_request(size, Some("invalid\0term")),
            Err(Abort::Failed { .. })
        ));
    }

    #[test]
    fn system_ssh_arguments_come_from_upstream_with_only_an_optional_config_prefix() {
        use rshell_m0::rshell_core::{AuthenticationKind, TransportKind};
        use std::path::PathBuf;
        let mut profile = ConnectionProfile::new("系统 ssh", "example.test");
        profile.port = 2200;
        profile.username = "user".into();
        profile.transport = TransportKind::SystemOpenSsh;
        profile.authentication = AuthenticationKind::Agent;
        let args = |profile: &ConnectionProfile, config: Option<PathBuf>| {
            super::system_ssh_args(profile, config)
                .ok()
                .expect("合法参数")
                .into_iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            args(&profile, None),
            [
                "-p",
                "2200",
                "-o",
                "StrictHostKeyChecking=ask",
                "--",
                "user@example.test"
            ]
        );

        profile.port = 22;
        profile.authentication = AuthenticationKind::PublicKey;
        profile.identity_file = Some(PathBuf::from("id_test"));
        profile.remote_command = Some("uptime".into());
        assert_eq!(
            args(&profile, Some(PathBuf::from("fixture_config"))),
            [
                "-F",
                "fixture_config",
                "-i",
                "id_test",
                "-o",
                "IdentitiesOnly=yes",
                "-o",
                "StrictHostKeyChecking=ask",
                "--",
                "user@example.test",
                "uptime"
            ]
        );

        // 上游校验仍生效：像选项的主机、改写目的地的用户名都拒绝。
        for (host, username) in [("-oProxyCommand=x", "user"), ("example.test", "a@b")] {
            let mut bad = profile.clone();
            bad.host = host.into();
            bad.username = username.into();
            assert!(super::system_ssh_args(&bad, None).is_err());
        }
    }

    #[test]
    fn forged_mobile_desktop_transport_is_rejected_even_for_a_valid_shape() {
        let request = DesktopConnectRequest {
            version: DESKTOP_CONNECT_VERSION,
            request_id: 1,
            session_id: 1,
            target: DesktopTargetKind::LocalShell,
            profile_id: String::new(),
            connection_id: String::new(),
            host: String::new(),
            port: 0,
            username: String::new(),
            password: String::new(),
            command: String::new(),
            cols: 80,
            rows: 24,
            pixel_width: 0,
            pixel_height: 0,
            dpi: 96,
        };
        assert!(matches!(
            validate_desktop_for::<false>(&request),
            Err((DesktopConnectError::PlatformUnavailable, _))
        ));
        assert_eq!(
            validate_desktop(&request).is_ok(),
            super::desktop_supported()
        );
    }

    #[test]
    fn keyboard_interactive_rounds_without_questions_or_text_are_answered_silently() {
        let round = |name: &str, instruction: &str, prompts: usize| KeyboardInteractivePrompt {
            id: InteractionId::new(),
            name: name.to_owned(),
            instruction: instruction.to_owned(),
            prompts: (0..prompts)
                .map(|_| AuthPrompt {
                    id: InteractionId::new(),
                    label: "Password: ".to_owned(),
                    echo: false,
                })
                .collect(),
        };
        assert!(is_empty_round(&round("", " ", 0)));
        assert!(!is_empty_round(&round("", "", 1)));
        assert!(!is_empty_round(&round("", "Your password expires soon", 0)));
    }

    fn reply(accept: bool, answers: &[&str]) -> crate::protected_reply::ProtectedReply {
        InteractionReply {
            session_id: 1,
            prompt_id: 1,
            accept,
            answers: answers.iter().map(|answer| (*answer).to_owned()).collect(),
            remember: false,
        }
        .into()
    }

    #[test]
    fn host_key_answers_accept_or_reject() {
        assert!(matches!(
            broker_response(PromptKind::HostKey, reply(true, &[])),
            InteractionResponse::HostKey(HostKeyDecision::AcceptAndStore)
        ));
        assert!(matches!(
            broker_response(PromptKind::HostKey, reply(false, &[])),
            InteractionResponse::HostKey(HostKeyDecision::Reject)
        ));
    }

    #[tokio::test]
    async fn stale_host_key_acceptance_and_rejection_do_not_answer_a_new_connection() {
        for stale_accept in [true, false] {
            let mut prompts = PromptIds::default();
            let old_prompt = prompts.next().ok().expect("上一轮提示");
            let profile = ConnectionProfile::new("主机密钥回归", "example.test");
            let (broker, mut requests) = interaction_channel();
            let request_broker = broker.clone();
            let response = tokio::spawn(async move {
                request_broker
                    .request(InteractionRequest::HostKey(HostKeyPrompt {
                        id: InteractionId::new(),
                        host: "example.test".into(),
                        port: 22,
                        algorithm: "ssh-ed25519".into(),
                        sha256: "SHA256:current".into(),
                        changed: true,
                    }))
                    .await
            });
            let (id, request) = requests.recv().await.expect("上游主机密钥提示");
            let mut pending = HashMap::new();
            let mut pending_pins = PendingPins::new();
            assert!(
                forward_prompt(
                    1,
                    &profile,
                    &broker,
                    &mut prompts,
                    &mut pending,
                    id,
                    request
                )
                .is_ok()
            );
            let current = *pending.keys().next().expect("本轮提示");
            assert_ne!(current, old_prompt);
            let mut stale = reply(stale_accept, &[]);
            stale.prompt_id = old_prompt;
            forward_reply(&broker, &mut pending, &mut pending_pins, stale);
            assert!(pending.contains_key(&current));
            assert!(
                !response.is_finished(),
                "旧答案不能让新主机密钥通过或被拒绝"
            );
            let mut fresh = reply(!stale_accept, &[]);
            fresh.prompt_id = current;
            forward_reply(&broker, &mut pending, &mut pending_pins, fresh);
            let answer = response.await.expect("broker 任务").expect("本轮回答");
            let expected = if stale_accept {
                HostKeyDecision::Reject
            } else {
                HostKeyDecision::AcceptAndStore
            };
            assert!(
                matches!(answer, InteractionResponse::HostKey(decision) if decision == expected)
            );
        }
    }

    #[test]
    fn exhausted_prompt_ids_fail_instead_of_reusing_an_old_number() {
        let mut prompts = PromptIds(u32::MAX);
        assert!(matches!(prompts.next(), Err(Abort::Failed { .. })));
    }

    #[test]
    fn keyboard_interactive_answers_keep_their_order() {
        let InteractionResponse::Answers(answers) = broker_response(
            PromptKind::KeyboardInteractive,
            reply(true, &["first", "second"]),
        ) else {
            panic!("expected answers");
        };
        let answers: Vec<&str> = answers
            .iter()
            .map(|answer| answer.expose_secret())
            .collect();
        assert_eq!(answers, ["first", "second"]);
        assert!(matches!(
            broker_response(PromptKind::KeyboardInteractive, reply(false, &["x"])),
            InteractionResponse::Cancel
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn the_connect_budget_does_not_count_time_spent_waiting_for_the_user() {
        let mut budget = Budget::new(Duration::from_secs(60));
        tokio::time::advance(Duration::from_secs(50)).await;
        budget.pause(true);
        tokio::time::advance(Duration::from_secs(300)).await;
        budget.pause(false);
        // 还剩 10 秒网络时间。
        assert!(
            tokio::time::timeout(Duration::from_secs(9), budget.expired())
                .await
                .is_err()
        );
        assert!(
            tokio::time::timeout(Duration::from_secs(2), budget.expired())
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn protected_answers_survive_queue_ownership_and_are_cleared_on_every_rejection() {
        use crate::protected_reply::ProtectedReply;
        use std::sync::{Arc, Mutex};
        use tokio::sync::mpsc::unbounded_channel;
        let audit = Arc::new(Mutex::new(Vec::new()));
        let answer = |prompt_id, accept| {
            ProtectedReply::audited(
                InteractionReply {
                    session_id: 1,
                    prompt_id,
                    accept,
                    answers: vec!["秘密一".into(), "多余二".into(), "多余三".into()],
                    remember: false,
                },
                audit.clone(),
            )
        };

        let (sender, receiver) = unbounded_channel();
        drop(receiver);
        let rejected = sender.send(answer(1, true));
        assert!(
            audit.lock().expect("审计").is_empty(),
            "发送错误仍拥有 guard"
        );
        drop(rejected);
        assert_eq!(*audit.lock().expect("审计"), vec![3]);

        let (sender, receiver) = unbounded_channel();
        assert!(sender.send(answer(1, true)).is_ok());
        drop(receiver);
        assert_eq!(*audit.lock().expect("审计"), vec![3, 3], "排队后关闭清零");

        let (broker, _requests) = interaction_channel();
        let mut pending = HashMap::new();
        let mut pins = PendingPins::new();
        forward_reply(&broker, &mut pending, &mut pins, answer(99, true));
        assert_eq!(
            *audit.lock().expect("审计"),
            vec![3, 3, 3],
            "陈旧 prompt 清零"
        );
        assert!(matches!(
            broker_response(PromptKind::KeyboardInteractive, answer(1, false)),
            InteractionResponse::Cancel
        ));
        assert!(super::pin_answer(answer(1, false)).is_none());

        let (commands_tx, mut commands) = unbounded_channel();
        let (replies_tx, mut replies) = unbounded_channel();
        assert!(replies_tx.send(answer(1, true)).is_ok());
        assert!(replies_tx.send(answer(2, true)).is_ok());
        let mut backlog = std::collections::VecDeque::new();
        let profile = ConnectionProfile::new("认证回答消费测试", "example.test");
        let result = super::ask_secret(
            1,
            2,
            &profile,
            super::Question {
                kind: PromptKind::Password,
                name: String::new(),
                can_remember: false,
                retry: false,
            },
            &mut super::Channels {
                commands: &mut commands,
                replies: &mut replies,
                backlog: &mut backlog,
            },
        )
        .await;
        let (secret, _) = result.ok().expect("真实密码消费入口");
        assert_eq!(secret.expose_secret(), "秘密一");
        drop(secret);
        assert_eq!(
            *audit.lock().expect("审计"),
            vec![3, 3, 3, 3, 3, 3, 2],
            "旧回答和多余答案都经清零析构"
        );
        let (pin, _) = super::pin_answer(answer(3, true)).expect("真实 PIN 消费");
        assert_eq!(pin.expose_secret(), "秘密一");
        assert_eq!(audit.lock().expect("审计").last(), Some(&2));
        drop(commands_tx);
    }
}
