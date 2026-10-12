//! 单角色、单进程绑定的准入与一次性 reply；等待/复制/I/O 不占控制锁。

use std::sync::{Mutex, MutexGuard};

use crate::protected_reply::ProtectedReply;
use crate::selected_p1::resource::ledger::{Amount, ResourceClass};
use crate::selected_p1::resource::owner::{OwnerLayout, SharedOwner};
use crate::selected_p1::resource::secret::LockedSecretString;

use super::Reject;
use super::buffer::ProtectedBuffer;
use super::protocol::{
    Binding, Header, HostQuery, HostReason, Kind, Policy, RawFrame, Request, Role, SequenceGate,
};
use super::transport::{self, Cancellation, PipePurpose, Resources, TrustedPipe, WriteFailure};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrozenAuth {
    SystemPassword,
    FileIdentity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SecretPurpose {
    SystemPassword,
    SelectedFilePassphrase,
}

#[derive(Clone, Copy)]
struct PromptSlot {
    sequence: u64,
    prompt: u64,
    used: bool,
}

struct State {
    sequence: SequenceGate,
    request: Option<u64>,
    prompt: Option<PromptSlot>,
    terminal: Option<Reject>,
    authenticated: bool,
}

struct Control {
    binding: Binding,
    role: Role,
    auth: FrozenAuth,
    session: u32,
    policy: Policy,
    cancellation: Cancellation,
    state: Mutex<State>,
}

impl Control {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// attempt owner 在终结时显式 cancel/close；Drop 不代表延迟资源或认证事件已释放。
pub struct Pipeline<'root> {
    control: SharedOwner<'root, Control>,
    resources: Resources<'root>,
}

impl<'root> Pipeline<'root> {
    /// 三个内嵌槽分别保留 request、prompt、terminal；不存在共用可被数据填满的队列。
    pub fn layout() -> Result<OwnerLayout, Reject> {
        Ok(OwnerLayout::actual::<Control>(0)?)
    }

    pub fn new(
        resources: Resources<'root>,
        binding: Binding,
        role: Role,
        auth: FrozenAuth,
        session: u32,
        policy: Policy,
    ) -> Result<Self, Reject> {
        binding.validate()?;
        resources.validate_binding(binding)?;
        if session == 0 {
            return Err(Reject::InvalidReply);
        }
        let model = Self::layout()?;
        let grant = resources.root.try_reserve(
            Amount {
                rust_bytes: model.backing.size(),
                c_bytes: 0,
                slots: 3,
            },
            ResourceClass::Control,
            resources.admission,
        )?;
        let control = SharedOwner::try_init(grant, 0, 0, |_| {
            Ok(Control {
                binding,
                role,
                auth,
                session,
                policy,
                cancellation: Cancellation::default(),
                state: Mutex::new(State {
                    sequence: SequenceGate::default(),
                    request: None,
                    prompt: None,
                    terminal: None,
                    authenticated: false,
                }),
            })
        })?;
        Ok(Self { control, resources })
    }

    pub fn cancel(&self) {
        self.control.value().cancellation.cancel();
        self.control
            .value()
            .lock()
            .terminal
            .get_or_insert(Reject::Cancelled);
    }

    pub fn close(&self) {
        self.control.value().cancellation.close();
        self.control
            .value()
            .lock()
            .terminal
            .get_or_insert(Reject::Closed);
    }

    pub fn terminal(&self) -> Option<Reject> {
        self.control.value().lock().terminal
    }

    fn claim(&self, header: Header, pipe: &impl TrustedPipe) -> Result<Lease<'root>, Reject> {
        let control = self.control.value();
        control.cancellation.check()?;
        header.binding.matches(control.binding)?;
        if header.role != control.role || header.kind == Kind::SecretReply {
            return Err(Reject::WrongRole);
        }
        self.resources.validate_binding(header.binding)?;
        Self::check_state(&control.lock(), header)?;
        // 不在状态锁中调用平台身份检查。
        pipe.verify(control.binding, control.role, PipePurpose::HelperRequest)?;
        let mut state = control.lock();
        control.cancellation.check()?;
        Self::check_state(&state, header)?;
        let owner = self.control.try_clone()?;
        state.request = Some(header.sequence);
        state.prompt = if header.kind == Kind::SecretRequest {
            Some(PromptSlot {
                sequence: header.sequence,
                prompt: header.prompt,
                used: false,
            })
        } else {
            None
        };
        state.sequence.accept(header.sequence)?;
        Ok(Lease {
            control: owner,
            resources: self.resources,
            header,
            finished: false,
        })
    }

    fn check_state(state: &State, header: Header) -> Result<(), Reject> {
        state.sequence.check(header.sequence)?;
        if state.request.is_some() {
            return Err(Reject::Full);
        }
        if state.authenticated {
            return Err(Reject::AlreadyAuthenticated);
        }
        Ok(())
    }

    /// caller 提供的完整帧保持 borrowed；所有字段、角色、来源与顺序拒绝先于 payload Grant。
    pub fn admit(
        &self,
        raw: &[u8],
        pipe: &impl TrustedPipe,
    ) -> Result<OwnedRequest<'root>, Reject> {
        let frame = RawFrame::parse(raw, self.control.value().policy)?;
        let lease = self.claim(frame.header, pipe)?;
        let grant = self
            .resources
            .reserve_buffer(frame.payload.len(), ResourceClass::Raw)?;
        let payload =
            ProtectedBuffer::copy(frame.payload, grant, &self.control.value().cancellation)?;
        self.resources.validate_binding(frame.header.binding)?;
        self.control.value().cancellation.check()?;
        Ok(OwnedRequest { lease, payload })
    }

    /// 先读固定 stack header、检查绑定/顺序和 quota，再分配 owned payload。
    pub fn read(&self, pipe: &mut impl TrustedPipe) -> Result<OwnedRequest<'root>, Reject> {
        let control = self.control.value();
        let header = transport::read_header(
            pipe,
            control.binding,
            control.role,
            PipePurpose::HelperRequest,
            control.policy,
            &control.cancellation,
        )?;
        let lease = self.claim(header, pipe)?;
        let grant = self
            .resources
            .reserve_buffer(header.payload_bytes(), ResourceClass::Raw)?;
        let payload = ProtectedBuffer::initialize(grant, header.payload_bytes(), |out| {
            transport::read_exact(pipe, out, &control.cancellation)
        })?;
        RawFrame::from_parts(header, payload.bytes())?;
        self.resources.validate_binding(header.binding)?;
        control.cancellation.check()?;
        Ok(OwnedRequest { lease, payload })
    }
}

struct Lease<'root> {
    control: SharedOwner<'root, Control>,
    resources: Resources<'root>,
    header: Header,
    finished: bool,
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        let control = self.control.value();
        let mut state = control.lock();
        if state.request == Some(self.header.sequence) {
            state.request = None;
            state.prompt = None;
            if !self.finished {
                state.terminal.get_or_insert(Reject::Closed);
                control.cancellation.close();
            }
        }
    }
}

/// 延迟工作持有实际 control/payload root；caller drop 不会提前退款。
pub struct OwnedRequest<'root> {
    lease: Lease<'root>,
    payload: ProtectedBuffer<'root>,
}

#[derive(Clone, Copy)]
pub enum HostOperation<'a> {
    ReadAlreadyTrustedOrder { host: &'a str },
    VerifyActualHandshakeKey(HostQuery<'a>),
}

/// 父的实际 System trust owner 提供，不接受公开 bool/nonce/自填 receipt。
///
/// # Safety
/// 必须验证 binding 对应当前实际 ssh 握手 key，System 持久信任事务已成功且取消没有先获准；
/// 所有 source/attempt/进程根保持有效。ORDER、终端 banner、keyscan 或其他握手不能授予此条件。
pub unsafe trait HostAdmission {
    fn verify_current_handshake(&self, binding: Binding) -> Result<(), Reject>;
}

/// 只表示认证完成；绝不表示 channel、PTY 或 exec 已获准，也不直接产生 Connected。
pub struct AuthenticationCompleted<'root> {
    control: SharedOwner<'root, Control>,
    resources: Resources<'root>,
}

impl AuthenticationCompleted<'_> {
    /// 关联字段不单独证明当前仍可进入 Connected；消费时须执行 verify_current。
    pub fn binding(&self) -> Binding {
        self.control.value().binding
    }

    pub fn verify_current(
        &self,
        pipe: &impl TrustedPipe,
        host: &impl HostAdmission,
    ) -> Result<(), Reject> {
        let control = self.control.value();
        control.cancellation.check()?;
        self.resources.validate_binding(control.binding)?;
        pipe.verify(control.binding, Role::PostAuth, PipePurpose::HelperRequest)?;
        host.verify_current_handshake(control.binding)?;
        control.cancellation.check()
    }
}

impl<'root> OwnedRequest<'root> {
    pub fn outer_layout() -> std::alloc::Layout {
        std::alloc::Layout::new::<Self>()
    }

    pub fn header(&self) -> Header {
        self.lease.header
    }

    pub fn request(&self) -> Result<Request<'_>, Reject> {
        Ok(RawFrame::from_parts(self.lease.header, self.payload.bytes())?.request)
    }

    pub fn host_operation(&self) -> Result<HostOperation<'_>, Reject> {
        match self.request()? {
            Request::KnownHosts(query) => match query.reason {
                HostReason::Order => {
                    Ok(HostOperation::ReadAlreadyTrustedOrder { host: query.host })
                }
                HostReason::Hostname => Ok(HostOperation::VerifyActualHandshakeKey(query)),
            },
            _ => Err(Reject::WrongRole),
        }
    }

    pub fn secret_purpose(&self) -> Result<SecretPurpose, Reject> {
        if !matches!(self.request()?, Request::Askpass { .. }) {
            return Err(Reject::WrongRole);
        }
        Ok(match self.lease.control.value().auth {
            FrozenAuth::SystemPassword => SecretPurpose::SystemPassword,
            FrozenAuth::FileIdentity => SecretPurpose::SelectedFilePassphrase,
        })
    }

    fn claim_reply(
        &self,
        pipe: &impl TrustedPipe,
        host: &impl HostAdmission,
    ) -> Result<(), Reject> {
        let control = self.lease.control.value();
        self.secret_purpose()?;
        control.cancellation.check()?;
        self.lease.resources.validate_binding(control.binding)?;
        pipe.verify(
            control.binding,
            Role::Askpass,
            PipePurpose::OwnerSecretReply,
        )?;
        host.verify_current_handshake(control.binding)?;
        let mut state = control.lock();
        control.cancellation.check()?;
        let prompt = state.prompt.as_mut().ok_or(Reject::InvalidReply)?;
        if prompt.sequence != self.lease.header.sequence
            || prompt.prompt != self.lease.header.prompt
            || prompt.used
        {
            return Err(Reject::Sequence);
        }
        // 在任何秘密副本或写入之前消费一次性槽；失败也不能重放。
        prompt.used = true;
        Ok(())
    }

    pub fn send_locked(
        &mut self,
        secret: &LockedSecretString<'_>,
        pipe: &mut impl TrustedPipe,
        host: &impl HostAdmission,
    ) -> Result<(), WriteFailure> {
        let result = (|| {
            self.claim_reply(pipe, host)?;
            let control = self.lease.control.value();
            let grant = self
                .lease
                .resources
                .reserve_buffer(secret.expose_secret().len(), ResourceClass::Secret)?;
            let buffer = ProtectedBuffer::from_locked(secret, grant, &control.cancellation)?;
            Ok(buffer)
        })();
        self.send_buffer(result, pipe)
    }

    pub(crate) fn send_reply(
        &mut self,
        reply: &ProtectedReply,
        pipe: &mut impl TrustedPipe,
        host: &impl HostAdmission,
    ) -> Result<(), WriteFailure> {
        let result = (|| {
            self.claim_reply(pipe, host)?;
            let control = self.lease.control.value();
            let length = reply.answers.first().map_or(0, |answer| answer.len());
            let grant = self
                .lease
                .resources
                .reserve_buffer(length, ResourceClass::Secret)?;
            ProtectedBuffer::from_reply(
                reply,
                control.session,
                self.lease.header.prompt,
                grant,
                &control.cancellation,
            )
        })();
        self.send_buffer(result, pipe)
    }

    fn send_buffer(
        &mut self,
        result: Result<ProtectedBuffer<'root>, Reject>,
        pipe: &mut impl TrustedPipe,
    ) -> Result<(), WriteFailure> {
        let result = result
            .map_err(|cause| WriteFailure {
                cause,
                header_prefix: 0,
                payload_prefix: 0,
                line_ending_prefix: 0,
            })
            .and_then(|buffer| {
                self.lease
                    .resources
                    .validate_binding(self.lease.header.binding)
                    .map_err(|cause| WriteFailure {
                        cause,
                        header_prefix: 0,
                        payload_prefix: 0,
                        line_ending_prefix: 0,
                    })?;
                transport::send_secret(
                    pipe,
                    self.lease.header,
                    &buffer,
                    &self.lease.control.value().cancellation,
                )
            });
        if let Err(error) = result {
            self.fail(error.cause);
        } else {
            self.lease.finished = true;
        }
        result
    }

    pub fn receive_and_reply(
        &mut self,
        owner_pipe: &mut impl TrustedPipe,
        ssh_reply_pipe: &mut impl TrustedPipe,
        host: &impl HostAdmission,
    ) -> Result<(), WriteFailure> {
        let result = (|| {
            self.claim_reply(owner_pipe, host)?;
            // 分配或读取秘密之前同时验证最终 ssh 专属回复 pipe。
            ssh_reply_pipe.verify(
                self.lease.header.binding,
                Role::Askpass,
                PipePurpose::SshAskpassReply,
            )?;
            let control = self.lease.control.value();
            transport::receive_secret(
                owner_pipe,
                self.lease.header,
                control.policy,
                &self.lease.resources,
                &control.cancellation,
            )
        })();
        let result = result
            .map_err(|cause| WriteFailure {
                cause,
                header_prefix: 0,
                payload_prefix: 0,
                line_ending_prefix: 0,
            })
            .and_then(|buffer| {
                self.lease
                    .resources
                    .validate_binding(self.lease.header.binding)
                    .map_err(|cause| WriteFailure {
                        cause,
                        header_prefix: 0,
                        payload_prefix: 0,
                        line_ending_prefix: 0,
                    })?;
                transport::send_line(
                    ssh_reply_pipe,
                    self.lease.header.binding,
                    &buffer,
                    &self.lease.control.value().cancellation,
                )
            });
        if let Err(error) = result {
            self.fail(error.cause);
        } else {
            self.lease.finished = true;
        }
        result
    }

    pub fn post_auth(
        &mut self,
        pipe: &impl TrustedPipe,
        host: &impl HostAdmission,
    ) -> Result<AuthenticationCompleted<'root>, Reject> {
        if !matches!(self.request()?, Request::PostAuth) {
            return Err(Reject::WrongRole);
        }
        let control = self.lease.control.value();
        control.cancellation.check()?;
        self.lease.resources.validate_binding(control.binding)?;
        pipe.verify(control.binding, Role::PostAuth, PipePurpose::HelperRequest)?;
        host.verify_current_handshake(control.binding)?;
        let mut state = control.lock();
        control.cancellation.check()?;
        if state.authenticated {
            return Err(Reject::AlreadyAuthenticated);
        }
        let event_control = self.lease.control.try_clone()?;
        state.authenticated = true;
        self.lease.finished = true;
        Ok(AuthenticationCompleted {
            control: event_control,
            resources: self.lease.resources,
        })
    }

    /// 已完成的非秘密 host 请求由父 trust owner 决定输出；本库不模拟持久信任提交。
    pub fn finish_host_request(&mut self) -> Result<(), Reject> {
        self.host_operation()?;
        self.lease.control.value().cancellation.check()?;
        self.lease.finished = true;
        Ok(())
    }

    fn fail(&mut self, reason: Reject) {
        let control = self.lease.control.value();
        control.cancellation.close();
        control.lock().terminal.get_or_insert(reason);
    }
}

#[cfg(test)]
mod tests {
    use super::super::Reject;
    use super::super::buffer::{ProtectedBuffer, audit};
    use super::super::protocol::{Binding, Policy, RawFrame, Role};
    use super::super::transport::{Cancellation, NotReadyPipe, Resources};
    use super::{FrozenAuth, HostAdmission, Lease, OwnedRequest, Pipeline};
    use crate::protected_reply::ProtectedReply;
    use crate::selected_p1::resource::ledger::{
        Admission, BYTE_LIMIT, CLASS_COUNT, ProcessLedger, ResourceClass,
    };
    use crate::signals::interaction::InteractionReply;

    struct NotReadyHost;

    // SAFETY: 只返回未就绪；不授予信任、不构造成功身份或认证事件。
    unsafe impl HostAdmission for NotReadyHost {
        fn verify_current_handshake(&self, _binding: Binding) -> Result<(), Reject> {
            Err(Reject::NotReady)
        }
    }

    #[test]
    fn protected_reply_send_without_peer_identity_has_no_secret_copy_or_write() {
        let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
        let source = root
            .open_source(1, 0)
            .unwrap_or_else(|e| panic!("source: {e:?}"));
        let resources = Resources {
            root: &root,
            admission: Admission {
                epoch: 1,
                source: Some(source),
            },
        };
        let binding = Binding {
            source,
            attempt: [0x41; 16],
            process_link: [0x52; 16],
        };
        let policy = Policy::new(256, 64, 4096, 128).unwrap_or_else(|e| panic!("policy: {e:?}"));
        let pipeline = Pipeline::new(
            resources,
            binding,
            Role::Askpass,
            FrozenAuth::SystemPassword,
            1,
            policy,
        )
        .unwrap_or_else(|e| panic!("pipeline: {e:?}"));
        let mut bytes = [0u8; 128];
        bytes[..8].copy_from_slice(b"GSSHHLPR");
        bytes[8..10].copy_from_slice(&1u16.to_le_bytes());
        bytes[10] = 2;
        bytes[11] = 1;
        bytes[16..24].copy_from_slice(&source.epoch.to_le_bytes());
        bytes[32..40].copy_from_slice(&source.generation.to_le_bytes());
        bytes[40..56].fill(0x41);
        bytes[56..72].fill(0x52);
        bytes[72..80].copy_from_slice(&1u64.to_le_bytes());
        bytes[80..88].copy_from_slice(&9u64.to_le_bytes());
        let header = RawFrame::parse(&bytes, policy)
            .unwrap_or_else(|e| panic!("frame: {e:?}"))
            .header;
        let grant = resources
            .reserve_buffer(0, ResourceClass::Raw)
            .unwrap_or_else(|e| panic!("request grant: {e:?}"));
        let payload = ProtectedBuffer::copy(&[], grant, &Cancellation::default())
            .unwrap_or_else(|e| panic!("request: {e:?}"));
        // 仅构造负向待处理对象；没有调用身份准入，也不能取得成功的 reply 槽。
        let mut request = OwnedRequest {
            lease: Lease {
                control: pipeline
                    .control
                    .try_clone()
                    .unwrap_or_else(|e| panic!("control: {e:?}")),
                resources,
                header,
                finished: false,
            },
            payload,
        };
        let reply = ProtectedReply::from(InteractionReply {
            session_id: 1,
            prompt_id: 9,
            accept: true,
            answers: vec![String::from("fixed-fixture-canary")],
            remember: false,
        });
        let capture = audit::Capture::start(&root);
        let failure = request
            .send_reply(&reply, &mut NotReadyPipe, &NotReadyHost)
            .err()
            .unwrap_or_else(|| panic!("身份缺失不得发送"));
        assert_eq!(failure.cause, Reject::NotReady);
        assert_eq!(
            (
                failure.header_prefix,
                failure.payload_prefix,
                failure.line_ending_prefix
            ),
            (0, 0, 0)
        );
        assert_eq!(capture.events().2, 0);
        assert_eq!(
            root.snapshot().classes[ResourceClass::Secret as usize].rust_bytes,
            0
        );
        drop(request);
        drop(pipeline);
        assert_eq!(root.snapshot().rust_bytes, 0);
    }
}
