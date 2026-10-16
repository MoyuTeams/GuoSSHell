//! workspace/runtime actor 独占此栈内状态；pane 不能拥有或关闭共享 port。

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ReplyPortReject {
    InvalidPort = 1,
    ProtocolMismatch = 2,
    EpochMismatch = 3,
    OwnerMismatch = 4,
    Closed = 5,
    AlreadyRegistered = 6,
    GenerationOverflow = 7,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PortOwner {
    pub epoch: u64,
    pub token: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PrivateIsolatePort(i64);

impl PrivateIsolatePort {
    pub fn value(self) -> i64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Unregistered,
    Active,
    Closed,
}

pub struct ReplyPortState {
    epoch: u64,
    generation: u64,
    protocol: u32,
    phase: Phase,
    port: Option<PrivateIsolatePort>,
}

impl ReplyPortState {
    /// 父在已收费的 persistent runtime owner 内嵌本状态，不独立初始化每个 pane。
    pub const fn new(epoch: u64, protocol: u32) -> Self {
        Self {
            epoch,
            generation: 0,
            protocol,
            phase: Phase::Unregistered,
            port: None,
        }
    }

    /// 参数只来自父的 bounded RegisterReplyControl；这里不解码/排队/注册 SDK callback。
    pub fn register(
        &mut self,
        epoch: u64,
        protocol: u32,
        port: i64,
        token: u64,
    ) -> Result<PortOwner, ReplyPortReject> {
        if epoch != self.epoch {
            return Err(ReplyPortReject::EpochMismatch);
        }
        if protocol != self.protocol {
            return Err(ReplyPortReject::ProtocolMismatch);
        }
        if port <= 0 {
            return Err(ReplyPortReject::InvalidPort);
        }
        match self.phase {
            Phase::Closed => return Err(ReplyPortReject::Closed),
            Phase::Active => {
                if self.port != Some(PrivateIsolatePort(port)) {
                    return Err(ReplyPortReject::AlreadyRegistered);
                }
                if token != 0 && token != self.generation {
                    return Err(ReplyPortReject::OwnerMismatch);
                }
                return Ok(self.owner());
            }
            Phase::Unregistered => {}
        }
        if token != 0 {
            return Err(ReplyPortReject::OwnerMismatch);
        }
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or(ReplyPortReject::GenerationOverflow)?;
        self.port = Some(PrivateIsolatePort(port));
        self.phase = Phase::Active;
        Ok(self.owner())
    }

    fn owner(&self) -> PortOwner {
        PortOwner {
            epoch: self.epoch,
            token: self.generation,
        }
    }

    /// invalid/duplicate 控制只同步返回数字；不能为它们先构造回复或 error peer。
    pub fn authorize(&self, owner: PortOwner) -> Result<PrivateIsolatePort, ReplyPortReject> {
        if owner.epoch != self.epoch {
            return Err(ReplyPortReject::EpochMismatch);
        }
        if owner.token == 0 || owner.token != self.generation {
            return Err(ReplyPortReject::OwnerMismatch);
        }
        if self.phase != Phase::Active {
            return Err(ReplyPortReject::Closed);
        }
        self.port.ok_or(ReplyPortReject::Closed)
    }

    pub fn close(&mut self, owner: PortOwner) -> Result<(), ReplyPortReject> {
        if owner != self.owner() || owner.token == 0 {
            return Err(ReplyPortReject::OwnerMismatch);
        }
        self.phase = Phase::Closed;
        self.port = None;
        Ok(())
    }

    /// 退出先封闭本 epoch；父随后失效 controls/drain entries，不在此释放任何 VM owner。
    pub fn shutdown(&mut self) {
        self.phase = Phase::Closed;
        self.port = None;
    }

    /// 仅父的 runtime epoch 转换调用；同一 epoch/old generation 永远不能重开。
    /// process ledger 的 advance_epoch 和旧资源计账由父另行维护，不能按 pane dispose 调用。
    pub fn begin_epoch(&mut self, next: u64) -> Result<(), ReplyPortReject> {
        if next <= self.epoch {
            return Err(ReplyPortReject::EpochMismatch);
        }
        if self.phase != Phase::Closed {
            return Err(ReplyPortReject::AlreadyRegistered);
        }
        self.epoch = next;
        self.phase = Phase::Unregistered;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{ReplyPortReject, ReplyPortState};
    use crate::audit::AuditScope;
    use crate::selected_p1::resource::ledger::{CLASS_COUNT, ProcessLedger};

    #[test]
    fn reply_port_generation_overflow_does_not_register_or_allocate() {
        let ledger = ProcessLedger::new(0, [u32::MAX; CLASS_COUNT]);
        let mut state = ReplyPortState::new(1, 2);
        state.generation = u64::MAX;
        let scope = AuditScope::start(&ledger);
        assert_eq!(
            state.register(1, 2, 71, 0),
            Err(ReplyPortReject::GenerationOverflow)
        );
        assert_eq!(state.generation, u64::MAX);
        assert!(state.port.is_none());
        assert!(scope.finish().events().is_empty());
    }
}
