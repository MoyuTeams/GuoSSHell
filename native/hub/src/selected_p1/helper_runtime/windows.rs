//! Windows 同步原生适配；不经 Command、PATH、shell、环境覆盖或异步 pipe relay。

use super::process::{
    Failure, HelperBuildRecord, LaunchIdentity, LaunchShape, LoadCosts, PipeReader, RawStorage,
    Request,
};
use crate::selected_p1::{
    model::layout::{NumericReject, add, extend},
    resource::ledger::{Amount, Grant, ResourceClass},
};
use sha2::{Digest, Sha256};
use std::alloc::{Layout, alloc, dealloc};
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::ptr::NonNull;
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, ERROR_BROKEN_PIPE, ERROR_INSUFFICIENT_BUFFER, GetLastError, HANDLE,
        HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation, WAIT_OBJECT_0,
    },
    Security::SECURITY_ATTRIBUTES,
    Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_SHARE_READ, OPEN_EXISTING,
        ReadFile, WriteFile,
    },
    System::{
        Pipes::CreatePipe,
        Threading::{
            CreateEventW, CreateProcessW, DeleteProcThreadAttributeList,
            EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, InitializeProcThreadAttributeList,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROCESS_INFORMATION, STARTF_USESTDHANDLES,
            STARTUPINFOEXW, SetEvent, TerminateProcess, UpdateProcThreadAttribute,
            WaitForSingleObject,
        },
    },
};

pub struct Handle(pub usize);
impl Handle {
    pub fn raw(&self) -> HANDLE {
        self.0 as HANDLE
    }
    pub fn event() -> Result<Self, Failure> {
        let h = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
        if h.is_null() {
            Err(Failure::Io)
        } else {
            Ok(Self(h as usize))
        }
    }
    pub fn notify(&self) {
        unsafe {
            SetEvent(self.raw());
        }
    }
    pub fn wait(&self, milliseconds: u32) -> u32 {
        unsafe { WaitForSingleObject(self.raw(), milliseconds) }
    }
    pub fn read(&self, out: &mut [u8]) -> Result<usize, Failure> {
        let len = u32::try_from(out.len()).map_err(|_| Failure::Protocol)?;
        let mut n = 0;
        if unsafe {
            ReadFile(
                self.raw(),
                out.as_mut_ptr(),
                len,
                &mut n,
                std::ptr::null_mut(),
            )
        } == 0
        {
            if unsafe { GetLastError() } == ERROR_BROKEN_PIPE {
                Ok(0)
            } else {
                Err(Failure::Io)
            }
        } else {
            Ok(n as usize)
        }
    }
    pub fn write_all(&self, mut bytes: &[u8]) -> Result<(), Failure> {
        while !bytes.is_empty() {
            let len = u32::try_from(bytes.len()).map_err(|_| Failure::Protocol)?;
            let mut n = 0;
            if unsafe {
                WriteFile(
                    self.raw(),
                    bytes.as_ptr(),
                    len,
                    &mut n,
                    std::ptr::null_mut(),
                )
            } == 0
                || n == 0
            {
                return Err(Failure::Io);
            }
            bytes = &bytes[n as usize..];
        }
        Ok(())
    }
}
impl PipeReader for Handle {
    fn read(&mut self, bytes: &mut [u8]) -> Result<usize, Failure> {
        Handle::read(self, bytes)
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        if self.0 != 0 {
            unsafe {
                CloseHandle(self.raw());
            }
            self.0 = 0;
        }
    }
}

pub struct ExactBuffer<'r> {
    pointer: NonNull<u8>,
    layout: Layout,
    permit: Grant<'r>,
}
// SAFETY: owner 独占 buffer，只按移动交给一个 process owner。
unsafe impl Send for ExactBuffer<'_> {}

impl<'r> ExactBuffer<'r> {
    fn from_grant(permit: Grant<'r>, layout: Layout) -> Result<Self, Failure> {
        if permit.class() != ResourceClass::Runtime || permit.amount().rust_bytes < layout.size() {
            return Err(Failure::Budget);
        }
        let pointer = NonNull::new(unsafe { alloc(layout) }).ok_or(Failure::Allocation)?;
        unsafe {
            pointer.as_ptr().write_bytes(0, layout.size());
        }
        Ok(Self {
            pointer,
            layout,
            permit,
        })
    }
    pub fn admitted(&self) -> Amount {
        self.permit.amount()
    }
}
impl Drop for ExactBuffer<'_> {
    fn drop(&mut self) {
        unsafe {
            dealloc(self.pointer.as_ptr(), self.layout);
        }
    }
}

fn map_numeric(e: NumericReject) -> Failure {
    if e == NumericReject::AllocationFailure {
        Failure::Allocation
    } else {
        Failure::Budget
    }
}

pub struct TrustedLaunch<'a> {
    executable: &'a OsStr,
    arguments: &'a [&'a OsStr],
}
impl<'a> TrustedLaunch<'a> {
    /// # Safety
    /// 父必须拥有完整 canonical exe/DB path 与固定参数 recipe；它们来自受信 bundle 和
    /// 已有 catalog owner，而非 Dart、PATH、环境或当前目录发现。借用期间不得修改。
    pub unsafe fn borrowed(
        executable: &'a OsStr,
        arguments: &'a [&'a OsStr],
    ) -> Result<Self, Failure> {
        if !std::path::Path::new(executable).is_absolute()
            || arguments.len() > 8
            || executable.encode_wide().any(|u| u == 0)
            || arguments.iter().any(|a| a.encode_wide().any(|u| u == 0))
        {
            return Err(Failure::Unsupported);
        }
        Ok(Self {
            executable,
            arguments,
        })
    }
}

/// CRT argv 引用规则；两遍使用同一 iterator，空格/引号/尾反斜杠均由此计算。
fn quoted(arg: &OsStr, mut emit: impl FnMut(u16) -> Result<(), Failure>) -> Result<(), Failure> {
    emit(34)?;
    let mut slashes = 0usize;
    for unit in arg.encode_wide() {
        if unit == 92 {
            slashes = slashes.checked_add(1).ok_or(Failure::Unsupported)?;
            continue;
        }
        let repetitions = if unit == 34 {
            slashes
                .checked_mul(2)
                .and_then(|n| n.checked_add(1))
                .ok_or(Failure::Unsupported)?
        } else {
            slashes
        };
        for _ in 0..repetitions {
            emit(92)?;
        }
        slashes = 0;
        emit(unit)?;
    }
    for _ in 0..slashes.checked_mul(2).ok_or(Failure::Unsupported)? {
        emit(92)?;
    }
    emit(34)
}

fn command(
    launch: &TrustedLaunch<'_>,
    mut emit: impl FnMut(u16) -> Result<(), Failure>,
) -> Result<(), Failure> {
    quoted(launch.executable, &mut emit)?;
    for argument in launch.arguments {
        emit(32)?;
        quoted(argument, &mut emit)?;
    }
    Ok(())
}

pub struct SpawnBuffers<'r> {
    backing: ExactBuffer<'r>,
    command_offset: usize,
    attributes: ExactBuffer<'r>,
    pub shape: LaunchShape,
    artifact_sha256: [u8; 32],
}

#[derive(Clone, Copy)]
pub struct SpawnCostPlan<'a> {
    launch: &'a TrustedLaunch<'a>,
    shape: LaunchShape,
    executable_command: Layout,
    attributes: Layout,
    command_offset: usize,
    costs: LoadCosts,
    artifact_sha256: [u8; 32],
}

impl<'a> SpawnCostPlan<'a> {
    /// 纯借用两遍计划的第一遍；attribute query 是 OS size 查询，不分配 Rust backing。
    pub fn bound(
        launch: &'a TrustedLaunch<'a>,
        request: &Request,
        record: &impl HelperBuildRecord,
    ) -> Result<Self, Failure> {
        let executable_units = launch.executable.encode_wide().count();
        let mut command_units = 0usize;
        command(launch, |_| {
            command_units = command_units.checked_add(1).ok_or(Failure::Unsupported)?;
            Ok(())
        })?;
        let shape = LaunchShape {
            argument_units: std::array::from_fn(|i| {
                launch
                    .arguments
                    .get(i)
                    .map_or(0, |a| a.encode_wide().count())
            }),
            arguments: launch.arguments.len(),
            executable_units,
            command_units,
            inherits_environment: true,
            inherits_cwd: true,
        };
        let certified = record.authorize(
            LaunchIdentity {
                executable: launch.executable,
                arguments: launch.arguments,
            },
            &shape,
            request,
        )?;
        let charge = certified.amount;
        if charge.rust_bytes != request.helper_rust
            || charge.c_bytes != request.c_max
            || charge.slots != 1
        {
            return Err(Failure::Unsupported);
        }
        let exe = Layout::array::<u16>(add(executable_units, 1).map_err(map_numeric)?)
            .map_err(|_| Failure::Unsupported)?;
        let cmd = Layout::array::<u16>(add(command_units, 1).map_err(map_numeric)?)
            .map_err(|_| Failure::Unsupported)?;
        let (layout, command_offset) = extend(exe, cmd).map_err(map_numeric)?;
        let mut size = 0usize;
        let ok =
            unsafe { InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut size) };
        if ok != 0 || size == 0 || unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER {
            return Err(Failure::Unsupported);
        }
        let attributes = Layout::from_size_align(size, std::mem::align_of::<usize>())
            .map_err(|_| Failure::Unsupported)?;
        let raw = RawStorage::layout(request).map_err(map_numeric)?.0;
        Ok(Self {
            launch,
            shape,
            executable_command: layout,
            attributes,
            command_offset,
            costs: LoadCosts {
                helper: charge,
                spawn: Amount {
                    rust_bytes: add(layout.size(), attributes.size()).map_err(map_numeric)?,
                    c_bytes: 0,
                    slots: 0,
                },
                raw: Amount {
                    rust_bytes: raw.size(),
                    c_bytes: 0,
                    slots: 1,
                },
            },
            artifact_sha256: certified.artifact_sha256,
        })
    }
    pub fn costs(&self) -> LoadCosts {
        self.costs
    }
    pub fn shape(&self) -> LaunchShape {
        self.shape
    }
    pub fn layouts(&self) -> (Layout, Layout) {
        (self.executable_command, self.attributes)
    }
    pub fn artifact_sha256(&self) -> [u8; 32] {
        self.artifact_sha256
    }
}

impl<'r> SpawnBuffers<'r> {
    pub fn from_grant(mut permit: Grant<'r>, plan: SpawnCostPlan<'_>) -> Result<Self, Failure> {
        let launch = plan.launch;
        if permit.class() != ResourceClass::Runtime
            || permit.amount().rust_bytes < plan.costs.spawn.rust_bytes
        {
            return Err(Failure::Budget);
        }
        let backing_permit = permit
            .split(Amount {
                rust_bytes: plan.executable_command.size(),
                c_bytes: 0,
                slots: 0,
            })
            .map_err(map_numeric)?;
        let attribute_permit = permit
            .split(Amount {
                rust_bytes: plan.attributes.size(),
                c_bytes: 0,
                slots: 0,
            })
            .map_err(map_numeric)?;
        let backing = ExactBuffer::from_grant(backing_permit, plan.executable_command)?;
        let attributes = ExactBuffer::from_grant(attribute_permit, plan.attributes)?;
        let exe_ptr = backing.pointer.as_ptr().cast::<u16>();
        for (i, unit) in launch.executable.encode_wide().enumerate() {
            unsafe {
                exe_ptr.add(i).write(unit);
            }
        }
        let cmd_ptr = unsafe {
            backing
                .pointer
                .as_ptr()
                .add(plan.command_offset)
                .cast::<u16>()
        };
        let mut at = 0;
        command(launch, |unit| {
            if at >= plan.shape.command_units {
                return Err(Failure::Unsupported);
            }
            unsafe {
                cmd_ptr.add(at).write(unit);
            }
            at += 1;
            Ok(())
        })?;
        if at != plan.shape.command_units {
            return Err(Failure::Unsupported);
        }
        Ok(Self {
            backing,
            command_offset: plan.command_offset,
            attributes,
            shape: plan.shape,
            artifact_sha256: plan.artifact_sha256,
        })
    }
    pub fn parent_charge(&self) -> usize {
        self.backing.layout.size() + self.attributes.layout.size()
    }
    pub fn spawn(self) -> Result<Child, Failure> {
        // 不共享写入或删除：实际映像在 hash 校验到 child wait 之间保持同一文件。
        let file = unsafe {
            CreateFileW(
                self.backing.pointer.as_ptr().cast(),
                FILE_GENERIC_READ,
                FILE_SHARE_READ,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            )
        };
        if file == INVALID_HANDLE_VALUE {
            return Err(Failure::Spawn);
        }
        let artifact = Handle(file as usize);
        let mut hash = Sha256::new();
        let mut scratch = [0u8; 4096];
        loop {
            let n = artifact.read(&mut scratch)?;
            if n == 0 {
                break;
            }
            hash.update(&scratch[..n]);
        }
        let actual: [u8; 32] = hash.finalize().into();
        if actual != self.artifact_sha256 {
            return Err(Failure::Unsupported);
        }
        fn pipe(parent_reads: bool) -> Result<(Handle, Handle), Failure> {
            let security = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: std::ptr::null_mut(),
                bInheritHandle: 1,
            };
            let mut read = std::ptr::null_mut();
            let mut write = std::ptr::null_mut();
            if unsafe { CreatePipe(&mut read, &mut write, &security, 0) } == 0 {
                return Err(Failure::Spawn);
            }
            let read = Handle(read as usize);
            let write = Handle(write as usize);
            let parent = if parent_reads {
                read.raw()
            } else {
                write.raw()
            };
            if unsafe { SetHandleInformation(parent, HANDLE_FLAG_INHERIT, 0) } == 0 {
                return Err(Failure::Spawn);
            }
            Ok((read, write))
        }
        let (stdin_child, stdin) = pipe(false)?;
        let (stdout, stdout_child) = pipe(true)?;
        let (stderr, stderr_child) = pipe(true)?;
        let list = self.attributes.pointer.as_ptr().cast();
        let mut size = self.attributes.layout.size();
        if unsafe { InitializeProcThreadAttributeList(list, 1, 0, &mut size) } == 0 {
            return Err(Failure::Spawn);
        }
        struct Attributes(windows_sys::Win32::System::Threading::LPPROC_THREAD_ATTRIBUTE_LIST);
        impl Drop for Attributes {
            fn drop(&mut self) {
                unsafe {
                    DeleteProcThreadAttributeList(self.0);
                }
            }
        }
        let _delete = Attributes(list);
        let handles = [stdin_child.raw(), stdout_child.raw(), stderr_child.raw()];
        if unsafe {
            UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_ptr().cast(),
                std::mem::size_of_val(&handles),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        } == 0
        {
            return Err(Failure::Spawn);
        }
        let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
        startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = stdin_child.raw();
        startup.StartupInfo.hStdOutput = stdout_child.raw();
        startup.StartupInfo.hStdError = stderr_child.raw();
        startup.lpAttributeList = list;
        let mut process: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        let ok = unsafe {
            CreateProcessW(
                self.backing.pointer.as_ptr().cast(),
                self.backing
                    .pointer
                    .as_ptr()
                    .add(self.command_offset)
                    .cast(),
                std::ptr::null(),
                std::ptr::null(),
                1,
                EXTENDED_STARTUPINFO_PRESENT,
                std::ptr::null(),
                std::ptr::null(),
                &startup.StartupInfo,
                &mut process,
            )
        };
        if ok == 0 {
            return Err(Failure::Spawn);
        }
        drop(Handle(process.hThread as usize));
        Ok(Child {
            process: Handle(process.hProcess as usize),
            stdin,
            stdout,
            stderr,
            artifact,
        })
    }
}

pub struct Child {
    pub process: Handle,
    pub stdin: Handle,
    pub stdout: Handle,
    pub stderr: Handle,
    pub artifact: Handle,
}
pub fn kill(raw: usize) -> Result<(), Failure> {
    if unsafe { TerminateProcess(raw as HANDLE, 1) } == 0 {
        Err(Failure::Io)
    } else {
        Ok(())
    }
}
pub fn exited(raw: usize) -> bool {
    raw != 0 && unsafe { WaitForSingleObject(raw as HANDLE, 0) } == WAIT_OBJECT_0
}
pub fn wait(raw: usize) -> Result<u32, Failure> {
    if unsafe { WaitForSingleObject(raw as HANDLE, u32::MAX) } != WAIT_OBJECT_0 {
        return Err(Failure::Wait);
    }
    let mut code = 0;
    if unsafe { GetExitCodeProcess(raw as HANDLE, &mut code) } == 0 {
        Err(Failure::Wait)
    } else {
        Ok(code)
    }
}
