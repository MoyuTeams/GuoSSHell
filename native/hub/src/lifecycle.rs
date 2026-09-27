//! App 前后台：进后台时向系统申请一小段后台运行时间（iOS 约 30 秒），刚切出去的连接不会随
//! App 立刻挂起；回到前台或时间用完时交还。挂起后的连接是否还活着，靠 keepalive 发现。

use rinf::DartSignal;

use crate::signals::AppLifecycle;

pub async fn run() {
    let receiver = AppLifecycle::get_dart_signal_receiver();
    while let Some(pack) = receiver.recv().await {
        platform::set_foreground(pack.message.foreground);
    }
}

#[cfg(target_os = "ios")]
mod platform {
    use std::cell::Cell;

    use block2::RcBlock;
    use dispatch2::DispatchQueue;
    use objc2::MainThreadMarker;
    use objc2_ui_kit::{UIApplication, UIBackgroundTaskIdentifier, UIBackgroundTaskInvalid};

    thread_local! {
        /// 进行中的后台任务。只在主线程上用。
        static TASK: Cell<Option<UIBackgroundTaskIdentifier>> = const { Cell::new(None) };
    }

    /// UIApplication 的后台任务接口要在主线程上调。
    pub fn set_foreground(foreground: bool) {
        DispatchQueue::main().exec_async(move || {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            if foreground {
                end(mtm);
            } else {
                begin(mtm);
            }
        });
    }

    fn begin(mtm: MainThreadMarker) {
        if TASK.get().is_some() {
            return;
        }
        // 时间用完时系统在主线程上回调：交还任务，App 随后挂起。
        let expired = RcBlock::new(move || end(mtm));
        let task = UIApplication::sharedApplication(mtm)
            .beginBackgroundTaskWithExpirationHandler(Some(&expired));
        // SAFETY: 读系统导出的常量。
        if task != unsafe { UIBackgroundTaskInvalid } {
            TASK.set(Some(task));
        }
    }

    fn end(mtm: MainThreadMarker) {
        if let Some(task) = TASK.take() {
            UIApplication::sharedApplication(mtm).endBackgroundTask(task);
        }
    }
}

/// 其他平台（macOS 等）进后台不会挂起 App。
#[cfg(not(target_os = "ios"))]
mod platform {
    pub fn set_foreground(_foreground: bool) {}
}
