use std::future::Future;
use std::num::NonZeroU32;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::task::{Context, Poll};

use crate::selected_p1::resource::owner::{OwnerLayout, SharedOwner};

use super::{AppScope, RuntimeFault, Site};

struct Budget {
    limit: u32,
    remaining: AtomicU32,
    yielded: AtomicBool,
}

/// 所有协议循环和 IO 每次实际工作前消费；一次 poll 耗尽后只安排一次 wake。
pub struct Cooperative {
    owner: SharedOwner<'static, Budget>,
}

impl Cooperative {
    pub fn layout() -> Result<std::alloc::Layout, RuntimeFault> {
        OwnerLayout::actual::<Budget>(0)
            .map(|model| model.backing)
            .map_err(|reason| RuntimeFault::numeric(Site::Cooperative, reason))
    }

    pub fn try_new(scope: AppScope, limit: NonZeroU32) -> Result<Self, RuntimeFault> {
        Ok(Self {
            owner: scope.own(Site::Cooperative, 0, || {
                Ok(Budget {
                    limit: limit.get(),
                    remaining: AtomicU32::new(limit.get()),
                    yielded: AtomicBool::new(false),
                })
            })?,
        })
    }

    pub fn try_clone(&self) -> Result<Self, RuntimeFault> {
        Ok(Self {
            owner: self
                .owner
                .try_clone()
                .map_err(|reason| RuntimeFault::numeric(Site::Cooperative, reason))?,
        })
    }

    fn reset(&self) {
        let budget = self.owner.value();
        budget.remaining.store(budget.limit, Ordering::Release);
        budget.yielded.store(false, Ordering::Release);
    }

    pub fn poll_work(&self, cx: &mut Context<'_>, work: NonZeroU32) -> Poll<()> {
        let budget = self.owner.value();
        if budget
            .remaining
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                remaining.checked_sub(work.get())
            })
            .is_ok()
        {
            Poll::Ready(())
        } else {
            if !budget.yielded.swap(true, Ordering::AcqRel) {
                cx.waker().wake_by_ref();
            }
            Poll::Pending
        }
    }

    /// 返回未装箱的真实 F 包装；父对该具体类型计算 InlineLayout。
    pub fn bind<F: Future>(self, future: F) -> Budgeted<F> {
        Budgeted {
            cooperative: self,
            future,
        }
    }
}

pub struct Budgeted<F> {
    cooperative: Cooperative,
    future: F,
}

impl<F: Future> Future for Budgeted<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: 不移动 future；外层 pin 存活期间只投影同一个内嵌 F。
        let this = unsafe { self.get_unchecked_mut() };
        this.cooperative.reset();
        unsafe { Pin::new_unchecked(&mut this.future) }.poll(cx)
    }
}
