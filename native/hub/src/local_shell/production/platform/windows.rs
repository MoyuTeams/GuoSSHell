use std::io;
use std::mem::size_of;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::Arc;

use portable_pty::{Child, CommandBuilder, SlavePty};
use rshell_platform::WindowsProcessJob;
use windows_sys::Win32::Foundation::{
    DUPLICATE_SAME_ACCESS, DuplicateHandle, FILETIME, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_READ,
    FILE_SHARE_WRITE,
};
use windows_sys::Win32::System::JobObjects::{
    IsProcessInJob, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JobObjectBasicAccountingInformation,
    QueryInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, GetCurrentThreadId, GetExitCodeProcess, GetProcessId,
    GetProcessTimes, GetThreadId, WaitForSingleObject,
};

use super::super::{Failure, Shared, TreeOutcome, lock};

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CancelSynchronousIo(thread: HANDLE) -> i32;
}

fn duplicate(raw: HANDLE) -> Result<OwnedHandle, Failure> {
    let mut copy = std::ptr::null_mut();
    // SAFETY: 输入来自仍存活的原对象或当前线程；只复制本进程所有权。
    if unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            raw,
            GetCurrentProcess(),
            &mut copy,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    } == 0
    {
        return Err(Failure::io("duplicate-handle", io::Error::last_os_error()));
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(copy) })
}

pub(crate) struct Identity {
    handle: OwnedHandle,
    pub pid: u32,
    pub started: u64,
}
impl Identity {
    pub fn capture(child: &dyn Child) -> Result<Self, Failure> {
        let raw = child
            .as_raw_handle()
            .ok_or_else(|| Failure::invalid("child-handle", "child 没有原句柄"))?;
        let handle = duplicate(raw)?;
        let pid = unsafe { GetProcessId(handle.as_raw_handle()) };
        let mut created = FILETIME::default();
        let mut exited = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        if pid == 0
            || unsafe {
                GetProcessTimes(
                    handle.as_raw_handle(),
                    &mut created,
                    &mut exited,
                    &mut kernel,
                    &mut user,
                )
            } == 0
        {
            return Err(Failure::io("child-identity", io::Error::last_os_error()));
        }
        Ok(Self {
            handle,
            pid,
            started: ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64,
        })
    }
    pub fn is_exited(&self) -> Result<bool, Failure> {
        signaled(self.handle.as_raw_handle())
    }
    pub fn raw_handle(&self) -> HANDLE {
        self.handle.as_raw_handle()
    }
}

pub(in super::super) struct ThreadTarget {
    handle: OwnedHandle,
    id: u32,
}
pub(in super::super) fn prepare_writer(shared: &Shared) -> Result<(), Failure> {
    let target = ThreadTarget {
        handle: duplicate(unsafe { GetCurrentThread() })?,
        id: unsafe { GetCurrentThreadId() },
    };
    *lock(&shared.writer_target) = Some(target);
    Ok(())
}
pub(in super::super) fn cancel_write(shared: &Shared) -> Result<(), Failure> {
    if let Some(target) = &*lock(&shared.writer_target) {
        if unsafe { GetThreadId(target.handle.as_raw_handle()) } != target.id {
            return Err(Failure::invalid(
                "cancel-write-identity",
                "原线程身份不匹配",
            ));
        }
        if unsafe { CancelSynchronousIo(target.handle.as_raw_handle()) } == 0 {
            let error = io::Error::last_os_error();
            // ERROR_NOT_FOUND 不是退出确认；controller 仍等待实际 worker finished/join。
            if error.raw_os_error() != Some(1168) {
                return Err(Failure::io("cancel-write", error));
            }
        }
    }
    Ok(())
}
pub(in super::super) fn release_writer_target(shared: &Shared) {
    *lock(&shared.writer_target) = None;
}

pub(in super::super) fn hold_cwd(path: &std::path::Path) -> Result<std::fs::File, Failure> {
    // 禁止删除共享，保持已经验证的目录到 CreateProcess 返回；不读取用户文件。
    std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES | FILE_LIST_DIRECTORY)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .map_err(|error| Failure::io("cwd-handle", error))
}

pub(in super::super) fn validate_environment(command: &CommandBuilder) -> Result<(), Failure> {
    // get_env 使用上游最终有效环境的 Windows 大小写规则，包含继承、注册表与显式覆盖。
    if let Some(value) = command.get_env("PATHEXT") {
        for entry in std::env::split_paths(value) {
            match entry.to_str() {
                Some(entry)
                    if entry
                        .get(1..)
                        .is_some_and(|extension| !extension.contains(['/', '\\'])) => {}
                _ => {
                    return Err(Failure::invalid(
                        "launch-pathext",
                        "PATHEXT 条目无法安全解析",
                    ));
                }
            }
        }
    }
    Ok(())
}

pub(in super::super) struct Tree {
    job: WindowsProcessJob,
    observer: OwnedHandle,
    spawned: bool,
    #[cfg(test)]
    faults: Option<Arc<std::sync::atomic::AtomicUsize>>,
}
impl Tree {
    pub fn new() -> Result<Self, Failure> {
        let job = WindowsProcessJob::new().map_err(|error| Failure {
            operation: "job",
            source: Arc::new(error),
        })?;
        let observer = duplicate(job.as_borrowed_handle().as_raw_handle())?;
        Ok(Self {
            job,
            observer,
            spawned: false,
            #[cfg(test)]
            faults: None,
        })
    }
    pub fn spawn(
        &mut self,
        slave: &dyn SlavePty,
        command: CommandBuilder,
    ) -> Result<Box<dyn Child + Send + Sync>, Failure> {
        let child = slave
            .spawn_command_in_job(command, self.job.as_borrowed_handle())
            .map_err(|error| Failure {
                operation: "spawn",
                source: Arc::from(error.into_boxed_dyn_error()),
            })?;
        self.spawned = true;
        Ok(child)
    }
    pub fn verify(&self, identity: &Identity) -> Result<(), Failure> {
        let mut contained = 0;
        if unsafe {
            IsProcessInJob(
                identity.raw_handle(),
                self.observer.as_raw_handle(),
                &mut contained,
            )
        } == 0
        {
            return Err(Failure::io("job-containment", io::Error::last_os_error()));
        }
        if contained == 0 {
            return Err(Failure::invalid(
                "job-containment",
                "child 未包含在本 owner Job",
            ));
        }
        Ok(())
    }
    pub fn terminate(
        &mut self,
        _: Option<&mut (dyn Child + Send + Sync + 'static)>,
    ) -> Result<(), Failure> {
        #[cfg(test)]
        if self.faults.as_ref().is_some_and(|faults| {
            use std::sync::atomic::Ordering;
            faults
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                    if count == 0 {
                        None
                    } else {
                        Some(count.saturating_sub(1))
                    }
                })
                .is_ok()
        }) {
            return Err(Failure::io(
                "job-terminate",
                io::Error::from_raw_os_error(5),
            ));
        }
        // 使用自有 Job 观察句柄，保留失败重试的所有权；不杀 PID，不释放最后责任根。
        if unsafe { TerminateJobObject(self.observer.as_raw_handle(), 1) } == 0 {
            return Err(Failure::io("job-terminate", io::Error::last_os_error()));
        }
        Ok(())
    }
    #[cfg(test)]
    pub fn set_faults(&mut self, faults: Option<Arc<std::sync::atomic::AtomicUsize>>) {
        self.faults = faults;
    }
    pub fn poll_finished(&self) -> Result<Option<TreeOutcome>, Failure> {
        let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        if unsafe {
            QueryInformationJobObject(
                self.observer.as_raw_handle(),
                JobObjectBasicAccountingInformation,
                (&raw mut info).cast(),
                size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                std::ptr::null_mut(),
            )
        } != 0
        {
            if info.ActiveProcesses != 0 {
                return Ok(None);
            }
        } else {
            return Err(Failure::io("job-observe", io::Error::last_os_error()));
        }
        Ok(Some(if self.spawned {
            TreeOutcome::ContainedWindows
        } else {
            TreeOutcome::NotSpawned
        }))
    }
}

fn signaled(raw: HANDLE) -> Result<bool, Failure> {
    match unsafe { WaitForSingleObject(raw, 0) } {
        WAIT_OBJECT_0 => Ok(true),
        WAIT_TIMEOUT => Ok(false),
        _ => Err(Failure::io("child-poll", io::Error::last_os_error())),
    }
}
pub(in super::super) fn poll_exit(child: &mut dyn Child) -> Result<Option<u32>, Failure> {
    let raw = child
        .as_raw_handle()
        .ok_or_else(|| Failure::invalid("child-handle", "child 没有原句柄"))?;
    if !signaled(raw)? {
        return Ok(None);
    }
    let mut code = 0;
    if unsafe { GetExitCodeProcess(raw, &mut code) } == 0 {
        return Err(Failure::io("child-exit-code", io::Error::last_os_error()));
    }
    Ok(Some(code))
}

pub(in super::super) fn wait_child(
    child: &mut dyn Child,
) -> Result<portable_pty::ExitStatus, Failure> {
    let raw = child
        .as_raw_handle()
        .ok_or_else(|| Failure::invalid("child-handle", "child 没有原句柄"))?;
    // 上游 wait 可在退出码已可见但对象尚未 signaled 时返回；先等待原对象真实完成。
    match unsafe { WaitForSingleObject(raw, 0) } {
        WAIT_OBJECT_0 => {}
        WAIT_TIMEOUT => {
            return Err(Failure::io(
                "child-object-wait",
                io::Error::from(io::ErrorKind::WouldBlock),
            ));
        }
        _ => return Err(Failure::io("child-object-wait", io::Error::last_os_error())),
    }
    child
        .wait()
        .map_err(|error| Failure::io("child-wait", error))
}
