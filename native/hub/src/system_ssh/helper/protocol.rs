//! 私有二进制帧先借用、校验，之后才允许 owned backing 的准入。

use crate::selected_p1::resource::ledger::SourceToken;

use super::Reject;

pub const HEADER_BYTES: usize = 128;
pub const MAGIC: &[u8; 8] = b"GSSHHLPR";
pub const VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Role {
    KnownHosts = 1,
    Askpass = 2,
    PostAuth = 3,
}

impl Role {
    fn parse(value: u8) -> Result<Self, Reject> {
        match value {
            1 => Ok(Self::KnownHosts),
            2 => Ok(Self::Askpass),
            3 => Ok(Self::PostAuth),
            _ => Err(Reject::WrongRole),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    KnownHostsQuery,
    SecretRequest,
    SecretReply,
    AuthenticationCompleted,
}

/// 每个已登记 callback 进程使用独立有序流；纯数字 gate 不授予角色或 peer 身份。
#[derive(Default)]
pub struct SequenceGate {
    accepted: u64,
}

impl SequenceGate {
    pub fn check(&self, sequence: u64) -> Result<(), Reject> {
        if self.accepted.checked_add(1) != Some(sequence) {
            return Err(Reject::Sequence);
        }
        Ok(())
    }

    pub fn accept(&mut self, sequence: u64) -> Result<(), Reject> {
        self.check(sequence)?;
        self.accepted = sequence;
        Ok(())
    }
}

/// process_link 仅关联已登记的实际进程；这些数字本身绝不是身份认证凭据。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Binding {
    pub source: SourceToken,
    pub attempt: [u8; 16],
    pub process_link: [u8; 16],
}

impl Binding {
    pub fn validate(self) -> Result<(), Reject> {
        if self.source.epoch == 0 || self.source.generation == 0 {
            return Err(Reject::Source);
        }
        if self.attempt == [0; 16] {
            return Err(Reject::Attempt);
        }
        if self.process_link == [0; 16] {
            return Err(Reject::ProcessLink);
        }
        Ok(())
    }

    pub fn matches(self, expected: Self) -> Result<(), Reject> {
        if self.source != expected.source {
            return Err(Reject::Source);
        }
        if self.attempt != expected.attempt {
            return Err(Reject::Attempt);
        }
        if self.process_link != expected.process_link {
            return Err(Reject::ProcessLink);
        }
        Ok(())
    }
}

/// 每项来自 caller 的非秘密资源策略；总 payload 上限由这些字段 checked 相加。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Policy {
    host_bytes: usize,
    algorithm_bytes: usize,
    key_base64_bytes: usize,
    prompt_bytes: usize,
    nonsecret_max: usize,
}

impl Policy {
    pub fn new(
        host_bytes: usize,
        algorithm_bytes: usize,
        key_base64_bytes: usize,
        prompt_bytes: usize,
    ) -> Result<Self, Reject> {
        if [host_bytes, algorithm_bytes, key_base64_bytes, prompt_bytes].contains(&0) {
            return Err(Reject::Limit);
        }
        let known = 8usize
            .checked_add(host_bytes)
            .and_then(|n| n.checked_add(algorithm_bytes))
            .and_then(|n| n.checked_add(key_base64_bytes))
            .ok_or(Reject::Limit)?;
        let nonsecret_max = known.max(prompt_bytes);
        std::alloc::Layout::array::<u8>(nonsecret_max).map_err(|_| Reject::Limit)?;
        Ok(Self {
            host_bytes,
            algorithm_bytes,
            key_base64_bytes,
            prompt_bytes,
            nonsecret_max,
        })
    }

    pub fn nonsecret_max(self) -> usize {
        self.nonsecret_max
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Header {
    pub(crate) role: Role,
    pub(crate) kind: Kind,
    pub(crate) binding: Binding,
    pub(crate) sequence: u64,
    pub(crate) prompt: u64,
    lengths: [usize; 4],
    payload_bytes: usize,
}

impl Header {
    pub fn role(self) -> Role {
        self.role
    }
    pub fn kind(self) -> Kind {
        self.kind
    }
    pub fn binding(self) -> Binding {
        self.binding
    }
    pub fn sequence(self) -> u64 {
        self.sequence
    }
    pub fn prompt(self) -> u64 {
        self.prompt
    }

    pub fn parse(bytes: &[u8], policy: Policy) -> Result<Self, Reject> {
        let bytes: &[u8; HEADER_BYTES] = bytes.try_into().map_err(|_| Reject::Malformed)?;
        if &bytes[..8] != MAGIC {
            return Err(Reject::Malformed);
        }
        if u16::from_le_bytes([bytes[8], bytes[9]]) != VERSION {
            return Err(Reject::Version);
        }
        if bytes[12..16] != [0; 4] || bytes[28..32] != [0; 4] || bytes[88..96] != [0; 8] {
            return Err(Reject::Malformed);
        }
        let role = Role::parse(bytes[10])?;
        let kind = match (role, bytes[11]) {
            (Role::KnownHosts, 1) => Kind::KnownHostsQuery,
            (Role::Askpass, 1) => Kind::SecretRequest,
            (Role::Askpass, 5) => Kind::SecretReply,
            (Role::PostAuth, 1) => Kind::AuthenticationCompleted,
            (Role::Askpass, _) => return Err(Reject::UnsupportedInteraction),
            _ => return Err(Reject::Malformed),
        };
        let mut attempt = [0; 16];
        attempt.copy_from_slice(&bytes[40..56]);
        let mut process_link = [0; 16];
        process_link.copy_from_slice(&bytes[56..72]);
        let binding = Binding {
            source: SourceToken {
                epoch: read_u64(bytes, 16)?,
                index: u32::from_le_bytes(bytes[24..28].try_into().map_err(|_| Reject::Malformed)?),
                generation: read_u64(bytes, 32)?,
            },
            attempt,
            process_link,
        };
        binding.validate()?;
        let sequence = read_u64(bytes, 72)?;
        let prompt = read_u64(bytes, 80)?;
        if sequence == 0 {
            return Err(Reject::Sequence);
        }
        let mut lengths = [0; 4];
        let mut payload_bytes = 0usize;
        for (i, length) in lengths.iter_mut().enumerate() {
            *length = usize::try_from(read_u64(bytes, 96 + i * 8)?).map_err(|_| Reject::Limit)?;
            payload_bytes = payload_bytes.checked_add(*length).ok_or(Reject::Limit)?;
        }
        std::alloc::Layout::array::<u8>(payload_bytes).map_err(|_| Reject::Limit)?;
        match kind {
            Kind::KnownHostsQuery => {
                if prompt != 0
                    || lengths[0] == 0
                    || lengths[0] > 8
                    || lengths[1] == 0
                    || lengths[1] > policy.host_bytes
                    || lengths[2] > policy.algorithm_bytes
                    || lengths[3] > policy.key_base64_bytes
                {
                    return Err(Reject::Limit);
                }
            }
            Kind::SecretRequest => {
                if prompt == 0 || lengths[0] > policy.prompt_bytes || lengths[1..] != [0; 3] {
                    return Err(Reject::Limit);
                }
            }
            Kind::SecretReply => {
                // 秘密仅受实际 Grant 和 checked Layout 约束，不套非秘密业务长度上限。
                if prompt == 0 || lengths[1..] != [0; 3] {
                    return Err(Reject::Malformed);
                }
            }
            Kind::AuthenticationCompleted => {
                if prompt != 0 || payload_bytes != 0 {
                    return Err(Reject::Malformed);
                }
            }
        }
        Ok(Self {
            role,
            kind,
            binding,
            sequence,
            prompt,
            lengths,
            payload_bytes,
        })
    }

    pub fn payload_bytes(self) -> usize {
        self.payload_bytes
    }

    pub fn encode(self) -> [u8; HEADER_BYTES] {
        let mut bytes = [0; HEADER_BYTES];
        bytes[..8].copy_from_slice(MAGIC);
        bytes[8..10].copy_from_slice(&VERSION.to_le_bytes());
        bytes[10] = self.role as u8;
        bytes[11] = if self.kind == Kind::SecretReply { 5 } else { 1 };
        bytes[16..24].copy_from_slice(&self.binding.source.epoch.to_le_bytes());
        bytes[24..28].copy_from_slice(&self.binding.source.index.to_le_bytes());
        bytes[32..40].copy_from_slice(&self.binding.source.generation.to_le_bytes());
        bytes[40..56].copy_from_slice(&self.binding.attempt);
        bytes[56..72].copy_from_slice(&self.binding.process_link);
        bytes[72..80].copy_from_slice(&self.sequence.to_le_bytes());
        bytes[80..88].copy_from_slice(&self.prompt.to_le_bytes());
        for (i, length) in self.lengths.iter().enumerate() {
            bytes[96 + i * 8..104 + i * 8].copy_from_slice(&(*length as u64).to_le_bytes());
        }
        bytes
    }

    pub(crate) fn secret_reply(self, length: usize) -> Result<Self, Reject> {
        if self.role != Role::Askpass || self.kind != Kind::SecretRequest {
            return Err(Reject::WrongRole);
        }
        std::alloc::Layout::array::<u8>(length).map_err(|_| Reject::Limit)?;
        Ok(Self {
            kind: Kind::SecretReply,
            lengths: [length, 0, 0, 0],
            payload_bytes: length,
            ..self
        })
    }
}

fn read_u64(bytes: &[u8], at: usize) -> Result<u64, Reject> {
    let value = bytes.get(at..at + 8).ok_or(Reject::Malformed)?;
    Ok(u64::from_le_bytes(
        value.try_into().map_err(|_| Reject::Malformed)?,
    ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostReason {
    Order,
    Hostname,
}

#[derive(Clone, Copy)]
pub struct HostQuery<'a> {
    pub reason: HostReason,
    pub host: &'a str,
    pub algorithm: &'a str,
    pub key_base64: &'a str,
}

#[derive(Clone, Copy)]
pub enum Request<'a> {
    KnownHosts(HostQuery<'a>),
    Askpass { untrusted_display: &'a str },
    PostAuth,
}

pub struct RawFrame<'a> {
    pub header: Header,
    pub request: Request<'a>,
    pub(crate) payload: &'a [u8],
}

impl<'a> RawFrame<'a> {
    pub fn parse(bytes: &'a [u8], policy: Policy) -> Result<Self, Reject> {
        let prefix = bytes.get(..HEADER_BYTES).ok_or(Reject::Malformed)?;
        Self::from_parts(Header::parse(prefix, policy)?, &bytes[HEADER_BYTES..])
    }

    pub fn from_parts(header: Header, payload: &'a [u8]) -> Result<Self, Reject> {
        if payload.len() != header.payload_bytes || header.kind == Kind::SecretReply {
            return Err(Reject::Malformed);
        }
        let mut fields = [""; 4];
        let mut at = 0;
        for (i, length) in header.lengths.iter().enumerate() {
            let end = at + length;
            fields[i] = std::str::from_utf8(&payload[at..end]).map_err(|_| Reject::Malformed)?;
            if fields[i]
                .bytes()
                .any(|b| b == 0 || b == b'\r' || b == b'\n')
            {
                return Err(Reject::Malformed);
            }
            at = end;
        }
        let request = match header.kind {
            Kind::KnownHostsQuery => {
                if fields[1]
                    .bytes()
                    .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
                {
                    return Err(Reject::Malformed);
                }
                let reason = match fields[0] {
                    "ORDER" if fields[2].is_empty() && fields[3].is_empty() => HostReason::Order,
                    "HOSTNAME" if !fields[2].is_empty() && !fields[3].is_empty() => {
                        if !fields[2]
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"-@._".contains(&b))
                            || fields[2].ends_with("-cert-v01@openssh.com")
                            || !valid_base64(fields[3].as_bytes())
                        {
                            return Err(Reject::Malformed);
                        }
                        HostReason::Hostname
                    }
                    _ => return Err(Reject::Malformed),
                };
                Request::KnownHosts(HostQuery {
                    reason,
                    host: fields[1],
                    algorithm: fields[2],
                    key_base64: fields[3],
                })
            }
            Kind::SecretRequest => Request::Askpass {
                untrusted_display: fields[0],
            },
            Kind::AuthenticationCompleted => Request::PostAuth,
            Kind::SecretReply => return Err(Reject::Malformed),
        };
        Ok(Self {
            header,
            request,
            payload,
        })
    }
}

fn valid_base64(bytes: &[u8]) -> bool {
    if bytes.is_empty() || !bytes.len().is_multiple_of(4) {
        return false;
    }
    let data = bytes.len() - bytes.iter().rev().take_while(|b| **b == b'=').count();
    bytes.len() - data <= 2
        && bytes[..data]
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || b"+/".contains(b))
}
