//! Windows 窗口关联只核验 OS 对象；事件来源及 COM 退休仍是受信宿主的 unsafe 合同。

use super::{Error, Status};

pub(super) struct Host {
    thread: std::thread::ThreadId,
    window: usize,
    object: usize,
}

impl Host {
    pub(super) fn register(window: u64, object: u64) -> Result<Self, Error> {
        let host = Self {
            thread: std::thread::current().id(),
            window: usize::try_from(window).map_err(|_| Status::InvalidArgument)?,
            object: usize::try_from(object).map_err(|_| Status::InvalidArgument)?,
        };
        host.check(true)?;
        Ok(host)
    }

    pub(super) fn thread(&self) -> Result<(), Error> {
        if self.thread != std::thread::current().id() {
            return Err(Status::WrongThread.into());
        }
        Ok(())
    }

    pub(super) fn check(&self, focus: bool) -> Result<(), Error> {
        self.thread()?;
        #[cfg(target_os = "windows")]
        {
            use windows_sys::Win32::Foundation::{HANDLE, HWND};
            // 标准系统 DLL 的稳定 ABI；不需要新增 crate、SDK 或 windows-sys features。
            #[link(name = "user32")]
            unsafe extern "system" {
                fn IsWindow(window: HWND) -> i32;
                fn GetWindowThreadProcessId(window: HWND, process: *mut u32) -> u32;
                fn GetFocus() -> HWND;
                fn GetPropW(window: HWND, name: *const u16) -> HANDLE;
            }
            #[link(name = "kernel32")]
            unsafe extern "system" {
                fn GetCurrentThreadId() -> u32;
                fn GetCurrentProcessId() -> u32;
            }
            const PROPERTY: &[u16] = &[
                71, 117, 111, 83, 83, 72, 101, 108, 108, 46, 78, 111, 114, 109, 97, 108, 73, 110,
                112, 117, 116, 46, 86, 49, 0,
            ];
            let window = self.window as HWND;
            if self.window == 0 || self.object == 0 {
                return Err(Status::NotAttached.into());
            }
            let mut process = 0;
            // 指针来自当前进程受信宿主；这里只比较对象身份，不解引用 input_object。
            unsafe {
                if IsWindow(window) == 0 {
                    return Err(Status::NotAttached.into());
                }
                let thread = GetWindowThreadProcessId(window, &mut process);
                if thread != GetCurrentThreadId() || process != GetCurrentProcessId() {
                    return Err(Status::WrongThread.into());
                }
                if GetPropW(window, PROPERTY.as_ptr()) as usize != self.object {
                    return Err(Status::NotAttached.into());
                }
                if focus && GetFocus() != window {
                    return Err(Status::NotAttached.into());
                }
            }
            Ok(())
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (self.window, self.object, focus);
            Err(Status::UnsupportedPlatform.into())
        }
    }

    #[cfg(test)]
    pub(super) fn for_local_test() -> Self {
        Self {
            thread: std::thread::current().id(),
            window: 0,
            object: 0,
        }
    }
}
