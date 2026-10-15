//! 可取消 pipe 接缝；身份验证、I/O owner 和实际句柄由平台实现负责。

use std::sync::atomic::{AtomicU8, Ordering};

use crate::selected_p1::resource::ledger::{Admission, Amount, ProcessLedger, ResourceClass};

use super::Reject;
use super::buffer::{ProtectedBuffer, validate_line};
use super::protocol::{Binding, HEADER_BYTES, Header, Kind, Policy, Role};

#[derive(Default)]
pub struct Cancellation {
    state: AtomicU8,
}

impl Cancellation {
    pub fn cancel(&self) {
        let _ = self
            .state
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire);
    }

    pub fn close(&self) {
        self.state.store(2, Ordering::Release);
    }

    pub fn check(&self) -> Result<(), Reject> {
        match self.state.load(Ordering::Acquire) {
            0 => Ok(()),
            1 => Err(Reject::Cancelled),
            _ => Err(Reject::Closed),
        }
    }
}

/// 不在 UI Mutex 内执行。实现必须唤醒阻塞 I/O；取消返回前记录已经写出的 prefix。
/// 返回值不得超过输入 slice；返回 0 表示 EOF/断管，不以整段重放恢复部分写。
/// write_some 已完成任何字节时必须先返回 Ok(n)，不能用 Err 丢弃该次已完成的计数；
/// 取消类别由取消位或后续调用返回，流式层据此保留准确 prefix。
pub trait CancellableIo {
    fn read_some(&mut self, out: &mut [u8], cancellation: &Cancellation) -> Result<usize, Reject>;
    fn write_some(&mut self, bytes: &[u8], cancellation: &Cancellation) -> Result<usize, Reject>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PipePurpose {
    HelperRequest,
    OwnerSecretReply,
    SshAskpassReply,
}

/// 实际双向 OS 身份证明的唯一执行接缝，没有默认成功实现或可自填的 proof token。
///
/// # Safety
/// verify 必须检查当前 IPC 的双方 UID/SID、OS peer PID、不可复用 process handle/出生身份、
/// 实际双方 executable 映像锁及同 module 来源、已登记 ssh 的合法受控子链、活着的 attempt
/// 和 source。关联数字不是身份凭据；须与此对象实际读写的同一个 endpoint 绑定。
/// purpose 也必须匹配登记的实际 pipe 用途；SshAskpassReply 必须是 ssh 独占匿名回复管道，
/// 不能只是带该标签的 IPC、PTY、stderr 或通用 stdout。
/// 缺任一平台前件返回 NotReady；伪 peer 返回 Peer。验证到使用期间应持有实际身份/句柄根，
/// 取消要失效准入并唤醒 I/O，最后使用结束前不得释放这些根。不得以同用户、PID 单值、
/// nonce、Candidate 标签或应用侧虚构的 Native/System H 代替上述检查。
/// I/O 实现不得把借用秘密转成普通 heap 副本。额外 Rust backing/adapter/worker 必须先取得
/// 实际源模型的 Grant，秘密先保护、最后使用后 wipe/free 再退款；外层及 before-spawn H
/// 由实际平台 caller 证明。OS/OpenSSH 的缓冲不属于这些 Rust backing 的保证。
pub unsafe trait TrustedPipe: CancellableIo {
    fn verify(&self, binding: Binding, role: Role, purpose: PipePurpose) -> Result<(), Reject>;
}

/// 平台装配尚未提供时保持类型化未就绪，不执行任何角色或 I/O。
pub struct NotReadyPipe;

impl CancellableIo for NotReadyPipe {
    fn read_some(
        &mut self,
        _out: &mut [u8],
        _cancellation: &Cancellation,
    ) -> Result<usize, Reject> {
        Err(Reject::NotReady)
    }
    fn write_some(&mut self, _bytes: &[u8], _cancellation: &Cancellation) -> Result<usize, Reject> {
        Err(Reject::NotReady)
    }
}

// SAFETY: 始终拒绝；没有实际身份不足时可执行的路径。
unsafe impl TrustedPipe for NotReadyPipe {
    fn verify(&self, _binding: Binding, _role: Role, _purpose: PipePurpose) -> Result<(), Reject> {
        Err(Reject::NotReady)
    }
}

#[derive(Clone, Copy)]
pub struct Resources<'root> {
    pub root: &'root ProcessLedger,
    pub admission: Admission,
}

impl<'root> Resources<'root> {
    pub(crate) fn reserve_buffer(
        &self,
        capacity: usize,
        class: ResourceClass,
    ) -> Result<crate::selected_p1::resource::ledger::Grant<'root>, Reject> {
        let model = ProtectedBuffer::layout(capacity)?;
        Ok(self.root.try_reserve(
            Amount {
                rust_bytes: model.backing.size(),
                c_bytes: 0,
                slots: 1,
            },
            class,
            self.admission,
        )?)
    }

    pub(crate) fn validate_binding(&self, binding: Binding) -> Result<(), Reject> {
        if self.admission.epoch != binding.source.epoch
            || self.admission.source != Some(binding.source)
        {
            return Err(Reject::Source);
        }
        // 短锁数字准入同时复核当前 source/epoch；不持有它进行 copy 或 pipe I/O。
        drop(
            self.root
                .try_reserve(Amount::default(), ResourceClass::Control, self.admission)?,
        );
        Ok(())
    }
}

pub(crate) fn read_exact(
    pipe: &mut impl CancellableIo,
    out: &mut [u8],
    cancellation: &Cancellation,
) -> Result<(), Reject> {
    let mut at = 0;
    while at < out.len() {
        cancellation.check()?;
        let count = pipe.read_some(&mut out[at..], cancellation)?;
        if count == 0 {
            return Err(Reject::UnexpectedEof);
        }
        if count > out.len() - at {
            return Err(Reject::Io);
        }
        at += count;
    }
    cancellation.check()
}

pub(crate) fn read_header(
    pipe: &mut impl TrustedPipe,
    binding: Binding,
    role: Role,
    purpose: PipePurpose,
    policy: Policy,
    cancellation: &Cancellation,
) -> Result<Header, Reject> {
    cancellation.check()?;
    pipe.verify(binding, role, purpose)?;
    let mut bytes = [0; HEADER_BYTES];
    read_exact(pipe, &mut bytes, cancellation)?;
    Header::parse(&bytes, policy)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WriteFailure {
    pub cause: Reject,
    pub header_prefix: usize,
    pub payload_prefix: usize,
    pub line_ending_prefix: usize,
}

fn write_piece(
    pipe: &mut impl CancellableIo,
    bytes: &[u8],
    cancellation: &Cancellation,
    prefix: &mut usize,
) -> Result<(), Reject> {
    while *prefix < bytes.len() {
        cancellation.check()?;
        let count = pipe.write_some(&bytes[*prefix..], cancellation)?;
        if count == 0 || count > bytes.len() - *prefix {
            return Err(Reject::Io);
        }
        *prefix += count;
    }
    cancellation.check()
}

pub(crate) fn send_secret(
    pipe: &mut impl TrustedPipe,
    request: Header,
    buffer: &ProtectedBuffer<'_>,
    cancellation: &Cancellation,
) -> Result<(), WriteFailure> {
    let mut failure = WriteFailure {
        cause: Reject::Io,
        header_prefix: 0,
        payload_prefix: 0,
        line_ending_prefix: 0,
    };
    let result = (|| {
        cancellation.check()?;
        pipe.verify(
            request.binding,
            Role::Askpass,
            PipePurpose::OwnerSecretReply,
        )?;
        let header = request.secret_reply(buffer.len())?.encode();
        write_piece(pipe, &header, cancellation, &mut failure.header_prefix)?;
        write_piece(
            pipe,
            buffer.bytes(),
            cancellation,
            &mut failure.payload_prefix,
        )
    })();
    result.map_err(|cause| {
        failure.cause = cause;
        failure
    })
}

pub(crate) fn receive_secret<'root>(
    pipe: &mut impl TrustedPipe,
    request: Header,
    policy: Policy,
    resources: &Resources<'root>,
    cancellation: &Cancellation,
) -> Result<ProtectedBuffer<'root>, Reject> {
    resources.validate_binding(request.binding)?;
    let header = read_header(
        pipe,
        request.binding,
        Role::Askpass,
        PipePurpose::OwnerSecretReply,
        policy,
        cancellation,
    )?;
    header.binding.matches(request.binding)?;
    if header.role != Role::Askpass
        || header.kind != Kind::SecretReply
        || header.sequence != request.sequence
        || header.prompt != request.prompt
    {
        return Err(Reject::Sequence);
    }
    let grant = resources.reserve_buffer(header.payload_bytes(), ResourceClass::Secret)?;
    let buffer = ProtectedBuffer::initialize(grant, header.payload_bytes(), |out| {
        read_exact(pipe, out, cancellation)
    })?;
    validate_line(buffer.bytes())?;
    resources.validate_binding(header.binding)?;
    cancellation.check()?;
    Ok(buffer)
}

/// 专属 ssh 匿名回复管道同样要由平台绑定到当前 ssh，不允许写到 PTY/日志/任意 stdout。
pub(crate) fn send_line(
    pipe: &mut impl TrustedPipe,
    binding: Binding,
    buffer: &ProtectedBuffer<'_>,
    cancellation: &Cancellation,
) -> Result<(), WriteFailure> {
    let mut failure = WriteFailure {
        cause: Reject::Io,
        header_prefix: 0,
        payload_prefix: 0,
        line_ending_prefix: 0,
    };
    let result = (|| {
        cancellation.check()?;
        pipe.verify(binding, Role::Askpass, PipePurpose::SshAskpassReply)?;
        validate_line(buffer.bytes())?;
        write_piece(
            pipe,
            buffer.bytes(),
            cancellation,
            &mut failure.payload_prefix,
        )?;
        write_piece(pipe, b"\n", cancellation, &mut failure.line_ending_prefix)
    })();
    result.map_err(|cause| {
        failure.cause = cause;
        failure
    })
}

/// 仅测试可调用的同代码 pipe 原语；不产生受信角色或认证成功事件。
#[cfg(test)]
pub mod fixture {
    use super::{
        CancellableIo, Cancellation, ProtectedBuffer, Reject, ResourceClass, Resources,
        WriteFailure, read_exact, validate_line, write_piece,
    };

    pub fn read<'root>(
        pipe: &mut impl CancellableIo,
        capacity: usize,
        resources: &Resources<'root>,
        cancellation: &Cancellation,
    ) -> Result<ProtectedBuffer<'root>, Reject> {
        let grant = resources.reserve_buffer(capacity, ResourceClass::Secret)?;
        ProtectedBuffer::initialize(grant, capacity, |out| read_exact(pipe, out, cancellation))
    }

    pub fn line(
        pipe: &mut impl CancellableIo,
        buffer: &ProtectedBuffer<'_>,
        cancellation: &Cancellation,
    ) -> Result<(), WriteFailure> {
        let mut failure = WriteFailure {
            cause: Reject::Io,
            header_prefix: 0,
            payload_prefix: 0,
            line_ending_prefix: 0,
        };
        let result = (|| {
            validate_line(buffer.bytes())?;
            write_piece(
                pipe,
                buffer.bytes(),
                cancellation,
                &mut failure.payload_prefix,
            )?;
            write_piece(pipe, b"\n", cancellation, &mut failure.line_ending_prefix)
        })();
        result.map_err(|cause| {
            failure.cause = cause;
            failure
        })
    }

    pub fn copy_locked<'root>(
        secret: &crate::selected_p1::resource::secret::LockedSecretString<'_>,
        resources: &Resources<'root>,
        cancellation: &Cancellation,
    ) -> Result<ProtectedBuffer<'root>, Reject> {
        let grant =
            resources.reserve_buffer(secret.expose_secret().len(), ResourceClass::Secret)?;
        ProtectedBuffer::from_locked(secret, grant, cancellation)
    }

    pub fn equals(buffer: &ProtectedBuffer<'_>, expected: &[u8]) -> bool {
        buffer.bytes() == expected
    }

    pub fn checked_header(
        pipe: &mut impl CancellableIo,
        policy: super::Policy,
        cancellation: &Cancellation,
    ) -> Result<super::Header, Reject> {
        let mut bytes = [0; super::HEADER_BYTES];
        read_exact(pipe, &mut bytes, cancellation)?;
        super::Header::parse(&bytes, policy)
    }
}
