//! 建立连接：连接配置（目录里的，或快速连接的临时目标）→ 认证材料 → 握手
//! （含主机密钥确认）。
//!
//! 期间要用户回答的问题——密码、私钥口令、OpenPGP 卡的 PIN、主机密钥确认、
//! keyboard-interactive——发成 `InteractionPrompt`，回答经会话的 `replies` 通道回来。用户在此期间的其他操作
//! （输入、改尺寸）存进 backlog，连上后按原顺序处理；Disconnect 立即中止连接。
//! 私钥在 OpenPGP 卡或安全密钥上时，签名交给外部签名器，它等用户（PIN、按卡、触摸）的
//! 时间同样不算进连接时限。

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use base64ct::{Base64UrlUnpadded, Encoding as _};
use rinf::{RustSignal, debug_print};
use rshell_m0::rshell_core::{
    AuthenticationKind, CatalogMutation, ConnectionId, ConnectionProfile, HostKeyDecision,
    HostKeyPrompt, InteractionId, InteractionRequest, InteractionResponse,
    KeyboardInteractivePrompt, SecretUpdate, SessionFailure, TerminalSize, TransportKind,
};
use rshell_m0::rshell_session::{
    AuthPlan, ExternalSigner, InteractionBroker, KnownHostsVerifier, NativeSshTransport,
    SessionTransport, TransportRequest, interaction_channel,
};
use rshell_m0::russh::keys::{PrivateKey, PublicKey};
use secrecy::{ExposeSecret, SecretString};
use tokio::sync::mpsc::{self, UnboundedReceiver};
use tokio::sync::oneshot;
use tokio::task::spawn_blocking;
use tokio::time::Instant;

use crate::app::AppContext;
use crate::card::{CardFailure, CardSigner};
use crate::catalog;
use crate::external_signer::{PinQuestion, SignerRequest};
use crate::keys::{self, StoredKey};
use crate::security_key::{SecurityKeySigner, SkFailure};
use crate::session::SessionCommand;
use crate::signals::interaction::{InteractionPrompt, InteractionReply, PromptField, PromptKind};
use crate::signals::keys::KeyError;
use crate::signals::{ConnectRequest, FailureKind};

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
    pub transport: NativeSshTransport,
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
    pub replies: &'a mut UnboundedReceiver<InteractionReply>,
    /// 连接期间收到的非 Disconnect 命令，连上后按顺序处理。
    pub backlog: &'a mut VecDeque<SessionCommand>,
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
pub struct Target {
    pub profile: ConnectionProfile,
    pub saved: bool,
}

impl Target {
    /// 状态与提示里显示的目标。
    pub fn describe(&self) -> String {
        format!(
            "{}@{}:{}",
            self.profile.username, self.profile.host, self.profile.port
        )
    }
}

/// 找到连接配置：目录里的按 id 取；快速连接由请求里的字段临时组成（密码认证）。
pub async fn resolve_target(
    context: &Arc<AppContext>,
    request: &ConnectRequest,
) -> Result<Target, Abort> {
    if request.connection_id.is_empty() {
        let (host, username) =
            catalog::validate_target(&request.host, request.port, &request.username)
                .map_err(|error| Abort::failed(FailureKind::InvalidTarget, format!("{error:?}")))?;
        let mut profile = ConnectionProfile::new(format!("{username}@{host}"), host);
        profile.port = request.port;
        profile.username = username.to_owned();
        profile.transport = TransportKind::NativeSsh;
        profile.authentication = AuthenticationKind::Password;
        profile.remote_command =
            Some(request.command.trim().to_owned()).filter(|command| !command.is_empty());
        return Ok(Target {
            profile,
            saved: false,
        });
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
    Ok(Target {
        profile,
        saved: true,
    })
}

/// 建立连接。`quick_password` 是快速连接随请求带来的密码（可能为空）。
pub async fn establish(
    context: &Arc<AppContext>,
    session_id: u32,
    target: Target,
    quick_password: SecretString,
    size: TerminalSize,
    mut channels: Channels<'_>,
    prompts: &mut PromptIds,
) -> Result<Connected, Abort> {
    let described = target.describe();
    let Target { profile, saved } = target;
    // 外部签名器（OpenPGP 卡或安全密钥）与它要会话代办的事（问 PIN、等用户）。
    let mut external: Option<(Signer, UnboundedReceiver<SignerRequest>)> = None;

    // 密码：目录里存了就从钥匙串读（每次连接读，不缓存——PLAN §9.2.2）；
    // 没存、读不到、或快速连接没带，就问用户。
    let mut remember = None;
    let auth = match profile.authentication {
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
                        remember = Some(SecretString::from(password.expose_secret().to_owned()));
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
    .map_err(|error| Abort::failed(FailureKind::Other, format!("auth plan: {error:?}")))?;

    // 主机密钥：新主机与变更的密钥都问用户（变更时带 changed，Dart 给出醒目警告）。
    let verifier = KnownHostsVerifier::new(&context.known_hosts)
        .with_changed_key_prompt()
        .with_timeout(CONNECT_LIMIT);
    let mut transport = NativeSshTransport::new(profile.clone(), auth, verifier)
        .and_then(|transport| transport.with_connect_timeout(CONNECT_LIMIT))
        .and_then(|transport| transport.with_keepalive(KEEPALIVE_INTERVAL, KEEPALIVE_MAX))
        .map_err(|error| {
            Abort::failed(
                failure_kind(error.failure()),
                format!("transport: {error:?}"),
            )
        })?;

    let (broker, mut requests) = interaction_channel();
    let transport_request = TransportRequest::new(size);
    let (signer, mut signer_requests) = external.unzip();
    let result = {
        let connect = transport.connect(&transport_request, broker.clone());
        tokio::pin!(connect);
        let mut pending = HashMap::new();
        let mut pending_pins = PendingPins::new();
        let mut budget = Budget::new(CONNECT_BUDGET);
        let mut signer_waiting = false;
        loop {
            budget.pause(!pending.is_empty() || !pending_pins.is_empty() || signer_waiting);
            tokio::select! {
                result = &mut connect => break result,
                () = budget.expired() => {
                    return Err(Abort::failed(FailureKind::Timeout, "connect: timed out".to_owned()));
                }
                request = requests.recv() => {
                    if let Some((id, request)) = request {
                        forward_prompt(session_id, &profile, &broker, prompts, &mut pending, id, request)?;
                    }
                }
                request = next_signer_request(&mut signer_requests) => match request {
                    Some(SignerRequest::Pin { question, reply }) => {
                        let prompt_id = prompts.next()?;
                        card_pin_prompt(session_id, prompt_id, &profile, question)
                            .send_signal_to_dart();
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
    };
    if let Err(error) = result {
        debug_print!("[connect] {error:?}");
        // 外部签名失败时认证失败只是表象，原因在签名器里。
        if let Some(abort) = signer.as_ref().and_then(Signer::abort) {
            return Err(abort);
        }
        return Err(Abort::failed(
            failure_kind(error.failure()),
            format!("connect: {error:?}"),
        ));
    }

    if let Some(password) = remember {
        store_password(context, profile.id, password).await;
    }
    Ok(Connected { transport, profile })
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
    InteractionPrompt {
        name: question.name,
        can_remember: question.can_remember,
        retry: question.retry,
        ..empty_prompt(session_id, prompt_id, question.kind, profile)
    }
    .send_signal_to_dart();
    loop {
        tokio::select! {
            reply = channels.replies.recv() => {
                let Some(reply) = reply else { return Err(Abort::Closed) };
                if reply.prompt_id != prompt_id {
                    continue;
                }
                if !reply.accept {
                    return Err(Abort::Cancelled);
                }
                let password = reply.answers.into_iter().next().unwrap_or_default();
                return Ok((SecretString::from(password), reply.remember));
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
fn pin_answer(reply: InteractionReply) -> Option<(SecretString, bool)> {
    if !reply.accept {
        return None;
    }
    let pin = reply.answers.into_iter().next()?;
    Some((SecretString::from(pin), reply.remember))
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
    prompt.send_signal_to_dart();
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
    reply: InteractionReply,
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
fn broker_response(kind: PromptKind, reply: InteractionReply) -> InteractionResponse {
    match (kind, reply.accept) {
        (PromptKind::HostKey, true) => {
            InteractionResponse::HostKey(HostKeyDecision::AcceptAndStore)
        }
        (PromptKind::HostKey, false) => InteractionResponse::HostKey(HostKeyDecision::Reject),
        (PromptKind::KeyboardInteractive, true) => InteractionResponse::Answers(
            reply.answers.into_iter().map(SecretString::from).collect(),
        ),
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
        forward_reply, is_empty_round,
    };
    use crate::signals::interaction::InteractionReply;
    use rshell_m0::rshell_core::{
        AuthPrompt, ConnectionProfile, HostKeyDecision, HostKeyPrompt, InteractionId,
        InteractionRequest, InteractionResponse, KeyboardInteractivePrompt,
    };
    use rshell_m0::rshell_session::interaction_channel;
    use secrecy::ExposeSecret;
    use std::collections::HashMap;
    use std::time::Duration;

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

    fn reply(accept: bool, answers: &[&str]) -> InteractionReply {
        InteractionReply {
            session_id: 1,
            prompt_id: 1,
            accept,
            answers: answers.iter().map(|answer| (*answer).to_owned()).collect(),
            remember: false,
        }
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
}
