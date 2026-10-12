//! exact backing 的保护适配器；尾部全容量清零，真实 owner free 后才归还 Grant。

use std::ptr::NonNull;

use zeroize::Zeroize;

use crate::protected_reply::ProtectedReply;
use crate::selected_p1::model::layout::NumericReject;
use crate::selected_p1::resource::ledger::{Amount, Grant, ResourceClass};
use crate::selected_p1::resource::owner::{OwnerLayout, SharedOwner};
use crate::selected_p1::resource::secret::LockedSecretString;

use super::Reject;
use super::transport::Cancellation;

struct WipePayload {
    pointer: NonNull<u8>,
    capacity: usize,
}

// SAFETY: 初始化结束后只有不可变借用；最后 SharedOwner 独占析构才访问可变 tail。
unsafe impl Send for WipePayload {}
// SAFETY: 所有 strong holder 只读，引用计数阻止借用期间的最后析构。
unsafe impl Sync for WipePayload {}

impl Drop for WipePayload {
    fn drop(&mut self) {
        // SAFETY: pointer 是同一 owner 已初始化的完整 tail；最后析构或 init 失败独占它。
        let bytes = unsafe { std::slice::from_raw_parts_mut(self.pointer.as_ptr(), self.capacity) };
        bytes.zeroize();
        #[cfg(test)]
        audit::wiped(bytes);
    }
}

/// 实际外层布局包含引用计数、Grant、原 Layout、wipe owner 和整个 payload。
pub struct ProtectedBuffer<'root> {
    owner: SharedOwner<'root, WipePayload>,
}

impl<'root> ProtectedBuffer<'root> {
    pub fn layout(capacity: usize) -> Result<OwnerLayout, Reject> {
        Ok(OwnerLayout::actual::<WipePayload>(capacity)?)
    }

    pub fn admitted(&self) -> Amount {
        self.owner.admitted()
    }

    pub fn len(&self) -> usize {
        self.owner.payload().len()
    }

    pub fn is_empty(&self) -> bool {
        self.owner.payload().is_empty()
    }

    /// 克隆只延长同一真实 backing 的寿命，不产生未收费的秘密副本。
    pub fn try_clone(&self) -> Result<Self, Reject> {
        Ok(Self {
            owner: self.owner.try_clone()?,
        })
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        self.owner.payload()
    }

    pub(crate) fn initialize(
        grant: Grant<'root>,
        capacity: usize,
        fill: impl FnOnce(&mut [u8]) -> Result<(), Reject>,
    ) -> Result<Self, Reject> {
        let mut rejection = None;
        let owner = SharedOwner::try_init(grant, capacity, 0, |bytes| {
            let guard = WipePayload {
                pointer: NonNull::new(bytes.as_mut_ptr())
                    .ok_or(NumericReject::ConstructionFailure)?,
                capacity: bytes.len(),
            };
            #[cfg(test)]
            audit::allocated(bytes.len());
            if let Err(error) = fill(bytes) {
                rejection = Some(error);
                return Err(NumericReject::ConstructionFailure);
            }
            Ok(guard)
        });
        match owner {
            Ok(owner) => Ok(Self { owner }),
            Err(error) => Err(rejection.unwrap_or(Reject::Resource(error))),
        }
    }

    pub(crate) fn copy(
        bytes: &[u8],
        grant: Grant<'root>,
        cancellation: &Cancellation,
    ) -> Result<Self, Reject> {
        cancellation.check()?;
        Self::initialize(grant, bytes.len(), |out| {
            for (source, destination) in bytes.chunks(1024).zip(out.chunks_mut(1024)) {
                cancellation.check()?;
                destination.copy_from_slice(source);
            }
            cancellation.check()
        })
    }

    pub(crate) fn from_locked(
        secret: &LockedSecretString<'_>,
        grant: Grant<'root>,
        cancellation: &Cancellation,
    ) -> Result<Self, Reject> {
        Self::copy_line(secret.expose_secret().as_bytes(), grant, cancellation)
    }

    pub(crate) fn from_reply(
        reply: &ProtectedReply,
        session: u32,
        prompt: u64,
        grant: Grant<'root>,
        cancellation: &Cancellation,
    ) -> Result<Self, Reject> {
        if !reply.accept
            || reply.session_id != session
            || u64::from(reply.prompt_id) != prompt
            || reply.answers.len() != 1
        {
            return Err(Reject::InvalidReply);
        }
        Self::copy_line(reply.answers[0].as_bytes(), grant, cancellation)
    }

    fn copy_line(
        bytes: &[u8],
        grant: Grant<'root>,
        cancellation: &Cancellation,
    ) -> Result<Self, Reject> {
        if grant.class() != ResourceClass::Secret {
            return Err(Reject::Resource(NumericReject::UnsupportedEnvelope));
        }
        validate_line(bytes)?;
        Self::copy(bytes, grant, cancellation)
    }
}

/// 仅 System ASKPASS 的行式表达限制；不修改 Native 的密码语义。
pub(crate) fn validate_line(bytes: &[u8]) -> Result<(), Reject> {
    if bytes.iter().any(|b| matches!(b, 0 | b'\n' | b'\r')) {
        return Err(Reject::LineUnrepresentable);
    }
    Ok(())
}

/// 本 helper 自有源图的固定事件，不拦截或替换全局 allocator。
#[cfg(test)]
pub mod audit {
    use std::cell::Cell;

    use crate::selected_p1::resource::ledger::ProcessLedger;

    #[derive(Clone, Copy, Debug, Default)]
    pub struct Event {
        pub capacity: usize,
        pub held: usize,
        pub wiped: bool,
    }

    #[derive(Clone, Copy)]
    struct State {
        root: *const ProcessLedger,
        events: [Event; 64],
        len: usize,
        allocations: usize,
    }

    thread_local! {
        static STATE: Cell<State> = const { Cell::new(State {
            root: std::ptr::null(),
            events: [Event { capacity: 0, held: 0, wiped: false }; 64],
            len: 0,
            allocations: 0,
        }) };
    }

    pub struct Capture<'a> {
        root: &'a ProcessLedger,
        marker: std::marker::PhantomData<*mut ()>,
    }

    impl<'a> Capture<'a> {
        pub fn start(root: &'a ProcessLedger) -> Self {
            STATE.with(|cell| {
                let mut state = cell.get();
                assert!(state.root.is_null());
                state.root = root;
                state.len = 0;
                state.allocations = 0;
                cell.set(state);
            });
            Self {
                root,
                marker: std::marker::PhantomData,
            }
        }

        pub fn events(&self) -> ([Event; 64], usize, usize) {
            STATE.with(|cell| {
                let state = cell.get();
                (state.events, state.len, state.allocations)
            })
        }
    }

    impl Drop for Capture<'_> {
        fn drop(&mut self) {
            let _ = self.root;
            STATE.with(|cell| {
                let mut state = cell.get();
                state.root = std::ptr::null();
                cell.set(state);
            });
        }
    }

    pub(super) fn allocated(_capacity: usize) {
        STATE.with(|cell| {
            let mut state = cell.get();
            if !state.root.is_null() {
                state.allocations += 1;
                cell.set(state);
            }
        });
    }

    pub(super) fn wiped(bytes: &[u8]) {
        STATE.with(|cell| {
            let mut state = cell.get();
            if !state.root.is_null() {
                assert!(state.len < state.events.len());
                // SAFETY: Capture 的借用保持 root 活着；本固定 TLS 只用于同步 fixture。
                let held = unsafe { &*state.root }.snapshot().rust_bytes;
                state.events[state.len] = Event {
                    capacity: bytes.len(),
                    held,
                    wiped: bytes.iter().all(|b| *b == 0),
                };
                state.len += 1;
                cell.set(state);
            }
        });
    }
}
