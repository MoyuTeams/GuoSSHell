use std::future::Future;
use std::num::NonZeroUsize;

use crate::selected_p1::resource::ledger::{Grant, ResourceClass};
use crate::selected_p1::session_owner::inline::{
    HolderBound, InlineLayout, InlinePollList, InlineSessionOwner,
};

use super::{AppScope, RuntimeFault, Site};

/// 未装箱 F 和已预约的 actual header+F；由父的 reserved installer 接管。
/// 捕获图在构造 F 前已分别收费，Grant 不能先退再预约。
pub struct TaskAdmission<F> {
    pub future: F,
    pub permit: Grant<'static>,
    pub holders: HolderBound,
    pub layout: InlineLayout,
}

impl<F: Future> TaskAdmission<F> {
    pub fn try_new(scope: AppScope, future: F, holders: HolderBound) -> Result<Self, RuntimeFault> {
        let layout = InlineLayout::actual::<F>()
            .map_err(|reason| RuntimeFault::numeric(Site::Task, reason))?;
        let permit = scope.reserve(Site::Task, layout.backing, 1, ResourceClass::Runtime)?;
        Ok(Self {
            future,
            permit,
            holders,
            layout,
        })
    }
}

/// 复用 S 的唯一 intrusive list，不建第二个任务 engine 或 heap wake queue。
pub struct BoundedExecutor<F: Future> {
    tasks: InlinePollList<F>,
    visits: NonZeroUsize,
}

impl<F: Future> BoundedExecutor<F> {
    pub fn new(visits: NonZeroUsize) -> Self {
        Self {
            tasks: InlinePollList::default(),
            visits,
        }
    }

    pub fn enqueue(&mut self, owner: InlineSessionOwner<F>) -> Result<(), InlineSessionOwner<F>> {
        self.tasks.push(owner)
    }

    pub fn poll_round(&mut self) -> usize {
        self.tasks.poll_round(self.visits.get())
    }

    pub fn len(&self) -> usize {
        self.tasks.len()
    }
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }
}
