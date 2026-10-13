//! 固定 fixture 的独立非 UI 所有权；原进程对象完成与 I/O/析构分别观测。

#[cfg(windows)]
use std::collections::BTreeMap;
#[cfg(windows)]
use std::ffi::OsString;
#[cfg(windows)]
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[cfg(windows)]
use rshell_m0::rshell_core::TerminalSize;
#[cfg(windows)]
use rshell_m0::rshell_session::LocalLaunch;

use crate::selected_p1::model::layout::NumericReject;
use crate::selected_p1::resource::ledger::{
    Admission, Amount, ProcessLedger, ResourceClass, SourceToken,
};
use crate::selected_p1::resource::owner::{OwnerLayout, SharedOwner};

pub(crate) const INPUT_LIMIT: usize = 8 * 1024 * 1024;
pub(crate) const OUTPUT_LIMIT: usize = 8 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LocalReject {
    SavedLocalContractMissing,
    RemoteFields,
    #[cfg(windows)]
    InvalidGeometry,
    Cancelled,
    InputCapacity,
    OutputCapacity,
    #[cfg(windows)]
    Transport,
    #[cfg(windows)]
    CleanupIncomplete,
    Resource(NumericReject),
}

impl From<NumericReject> for LocalReject {
    fn from(error: NumericReject) -> Self {
        Self::Resource(error)
    }
}

/// 仅 Local 判别分支调用；必须先于任何 saved SSH 查找、复制或 spawn。
pub(crate) fn validate_local_fields(
    connection_id: &str,
    host: &str,
    port: u16,
    username: &str,
    command: &str,
    password: &str,
) -> Result<(), LocalReject> {
    if !connection_id.is_empty() {
        return Err(LocalReject::SavedLocalContractMissing);
    }
    if !host.is_empty()
        || port != 0
        || !username.is_empty()
        || !command.is_empty()
        || !password.is_empty()
    {
        return Err(LocalReject::RemoteFields);
    }
    Ok(())
}

/// 仅阻止新的 dispatch；系统写的取消确认由 IoState 单独给出。
pub(crate) struct DispatchGate(AtomicBool);

impl DispatchGate {
    pub(crate) const fn new() -> Self {
        Self(AtomicBool::new(false))
    }

    pub(crate) fn request_cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub(crate) fn check(&self) -> Result<(), LocalReject> {
        if self.0.load(Ordering::Acquire) {
            Err(LocalReject::Cancelled)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(usize)]
pub(crate) enum ProbeBoundary {
    #[cfg(windows)]
    SpawnReturned,
    #[cfg(windows)]
    TransportWriteEntered,
    #[cfg(windows)]
    TransportWriteReturned,
    DispatchCancelRequested,
    #[cfg(windows)]
    TransportShutdownReturned,
    #[cfg(windows)]
    TransportDropReturned,
    #[cfg(windows)]
    TransportExitReceived,
    #[cfg(windows)]
    OwnerFinished,
    LateHolderReleased,
    #[cfg(windows)]
    SystemWriteEntered,
    #[cfg(windows)]
    SystemWriteReturned,
    #[cfg(windows)]
    CancelConfirmed,
    #[cfg(windows)]
    WriterDropReturned,
    #[cfg(windows)]
    ReaderDrainStarted,
    #[cfg(windows)]
    ReaderDropReturned,
    #[cfg(windows)]
    MasterNonLastDropReturned,
    #[cfg(windows)]
    PtyLastHolderReturned,
    #[cfg(windows)]
    ChildReaped,
    #[cfg(windows)]
    WriterJoined,
    #[cfg(windows)]
    ReaderJoined,
    #[cfg(windows)]
    AllWorkersDone,
    #[cfg(windows)]
    ProcessObjectSignaled,
    #[cfg(windows)]
    FinalExitCodeRead,
    #[cfg(windows)]
    CompletionJoined,
    #[cfg(windows)]
    JobDropReturned,
    #[cfg(windows)]
    OperationErrorRecorded,
}

#[cfg(windows)]
const BOUNDARIES: usize = ProbeBoundary::OperationErrorRecorded as usize + 1;
#[cfg(not(windows))]
const BOUNDARIES: usize = ProbeBoundary::LateHolderReleased as usize + 1;

struct OutputSlot {
    bytes: [u8; OUTPUT_LIMIT],
    len: usize,
}

/// 只覆盖已知应用 backing；完整 PTY/runtime 源闭包不能由此类型冒充。
pub(crate) struct AppRoot {
    pub(crate) source: SourceToken,
    pub(crate) gate: DispatchGate,
    sequence: AtomicUsize,
    boundaries: [AtomicUsize; BOUNDARIES],
    output: Mutex<OutputSlot>,
    #[cfg(windows)]
    pub(crate) io: pty_owner::IoState,
}

impl AppRoot {
    pub(crate) fn observe(&self, boundary: ProbeBoundary) {
        let sequence = self.sequence.fetch_add(1, Ordering::AcqRel) + 1;
        self.boundaries[boundary as usize].store(sequence, Ordering::Release);
    }

    #[cfg(windows)]
    pub(crate) fn observed(&self, boundary: ProbeBoundary) -> usize {
        self.boundaries[boundary as usize].load(Ordering::Acquire)
    }

    pub(crate) fn cancel_dispatch(&self) {
        if !self.gate.0.swap(true, Ordering::AcqRel) {
            self.observe(ProbeBoundary::DispatchCancelRequested);
        }
    }

    /// 固定单槽队列；满时把事件退回调用者，不增长或静默截断正常输出。
    pub(crate) fn publish(&self, bytes: &[u8]) -> Result<(), LocalReject> {
        let mut output = self
            .output
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if output.len != 0 || bytes.len() > OUTPUT_LIMIT {
            return Err(LocalReject::OutputCapacity);
        }
        output.bytes[..bytes.len()].copy_from_slice(bytes);
        output.len = bytes.len();
        Ok(())
    }

    pub(crate) fn take_output(&self, target: &mut [u8; OUTPUT_LIMIT]) -> usize {
        let mut output = self
            .output
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let count = output.len;
        target[..count].copy_from_slice(&output.bytes[..count]);
        output.len = 0;
        count
    }

    #[cfg(windows)]
    pub(crate) fn output_is_full(&self) -> bool {
        self.output
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len
            != 0
    }
}

pub(crate) type ProbeRoot<'root> = SharedOwner<'root, AppRoot>;

pub(crate) fn app_root_shape(input_bytes: usize) -> Result<OwnerLayout, LocalReject> {
    if input_bytes > INPUT_LIMIT {
        return Err(LocalReject::InputCapacity);
    }
    Ok(OwnerLayout::actual::<AppRoot>(input_bytes)?)
}

/// grant-before-allocation；cleanup/观测全部内嵌，无第二次 Source grant。
pub(crate) fn reserve_probe_root(
    ledger: &ProcessLedger,
    source: SourceToken,
    input_bytes: usize,
) -> Result<ProbeRoot<'_>, LocalReject> {
    let shape = app_root_shape(input_bytes)?;
    let grant = ledger.try_reserve(
        Amount {
            rust_bytes: shape.backing.size(),
            c_bytes: 0,
            slots: 1,
        },
        ResourceClass::Runtime,
        Admission {
            epoch: source.epoch,
            source: Some(source),
        },
    )?;
    Ok(SharedOwner::try_init(grant, input_bytes, 0, |input| {
        input.fill(b'x');
        Ok(AppRoot {
            source,
            gate: DispatchGate::new(),
            sequence: AtomicUsize::new(0),
            boundaries: std::array::from_fn(|_| AtomicUsize::new(0)),
            output: Mutex::new(OutputSlot {
                bytes: [0; OUTPUT_LIMIT],
                len: 0,
            }),
            #[cfg(windows)]
            io: pty_owner::IoState::new(),
        })
    })?)
}

/// 当前可准确报告的接线前件，不是 Local 平台不可用结论。
#[cfg(windows)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProcessClosureGap {
    OpaqueRuntimeAllocationGraph,
    PartialInitializationDrain,
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Fixture {
    EchoGeometryExit,
    NoReadFlood,
}

#[cfg(windows)]
impl Fixture {
    fn launch(self) -> LocalLaunch {
        let script = match self {
            Self::EchoGeometryExit => concat!(
                "[Console]::InputEncoding=[Text.UTF8Encoding]::new($false);",
                "[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false);",
                "[Console]::WriteLine(('GUO_READY TERM={0} SIZE={1}x{2}' -f ",
                "$env:TERM,[Console]::WindowWidth,[Console]::WindowHeight));",
                "$line=[Console]::ReadLine();[Console]::WriteLine(('GUO_ECHO:'+ $line));",
                "$null=[Console]::ReadLine();",
                "[Console]::WriteLine(('GUO_RESIZED {0}x{1}' -f ",
                "[Console]::WindowWidth,[Console]::WindowHeight));",
                "[Console]::WriteLine('GUO_TAIL');exit 23"
            ),
            Self::NoReadFlood => concat!(
                "[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false);",
                "[Console]::WriteLine('GUO_NO_READ');",
                "for($i=0;$i -lt 8192;$i++){[Console]::WriteLine(('z'*256))};",
                "[Threading.Thread]::Sleep(60000);exit 0"
            ),
        };
        LocalLaunch::Command {
            program: PathBuf::from("pwsh.exe"),
            args: [
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                script,
            ]
            .into_iter()
            .map(OsString::from)
            .collect(),
            // 不创建或读写任何临时数据文件；工作目录由外层监督固定到批准目录。
            cwd: None,
            env: BTreeMap::new(),
        }
    }
}

/// 独立非 UI 消费者；自然关闭在独立 completion worker 上保持正常尾输出交付。
#[cfg(windows)]
pub(crate) struct LocalProcessOwner<'scope, 'env: 'scope, 'root: 'env> {
    scope: &'scope std::thread::Scope<'scope, 'env>,
    pty: Option<pty_owner::PtyOwner<'scope, 'root>>,
    completion: Option<std::thread::ScopedJoinHandle<'scope, Result<u32, LocalReject>>>,
    completion_code: Option<u32>,
    root: ProbeRoot<'root>,
    exit_emitted: bool,
}

#[cfg(windows)]
impl<'scope, 'env: 'scope, 'root: 'env> LocalProcessOwner<'scope, 'env, 'root> {
    pub(crate) const CLOSURE_GAPS: [ProcessClosureGap; 2] = [
        ProcessClosureGap::OpaqueRuntimeAllocationGraph,
        ProcessClosureGap::PartialInitializationDrain,
    ];

    /// 仅独立测试可启动固定合成 fixture；生产必须先交完整 process-root 与 A1。
    #[cfg(test)]
    pub(crate) async fn launch_probe(
        scope: &'scope std::thread::Scope<'scope, 'env>,
        root: ProbeRoot<'root>,
        fixture: Fixture,
        size: TerminalSize,
        terminal_type: &str,
        after_spawn: impl FnOnce(&Self),
    ) -> Result<Self, LocalReject> {
        let pty_root = root.try_clone()?;
        let pty = pty_owner::PtyOwner::launch(
            scope,
            pty_root,
            fixture,
            pty_owner::WriterInput::Manual,
            size,
            terminal_type,
            |_| {},
        )?;
        let mut owner = Self {
            scope,
            pty: Some(pty),
            completion: None,
            completion_code: None,
            root,
            exit_emitted: false,
        };
        owner
            .pty_mut()?
            .probe_visible_code_before_object_signal(23)?;
        after_spawn(&owner);
        owner
            .root
            .value()
            .observe(ProbeBoundary::LateHolderReleased);
        // 不丢掉迟到 spawn 结果；原 owner 持有并负责 shutdown。
        if owner.root.value().gate.check().is_err() {
            owner.shutdown().await?;
            return Err(LocalReject::Cancelled);
        }
        Ok(owner)
    }

    fn pty_mut(&mut self) -> Result<&mut pty_owner::PtyOwner<'scope, 'root>, LocalReject> {
        self.pty.as_mut().ok_or(LocalReject::Transport)
    }

    #[cfg(test)]
    pub(crate) fn probe_root(&self) -> &ProbeRoot<'root> {
        &self.root
    }

    pub(crate) fn process_id(&self) -> Option<u32> {
        self.pty.as_ref().and_then(pty_owner::PtyOwner::process_id)
    }

    #[cfg(windows)]
    pub(crate) fn contains_process(&self, pid: u32) -> Result<bool, LocalReject> {
        self.pty
            .as_ref()
            .ok_or(LocalReject::Transport)?
            .contains_process(pid)
    }

    pub(crate) async fn next_event(
        &mut self,
    ) -> Result<rshell_m0::rshell_session::TransportEvent, LocalReject> {
        use rshell_m0::rshell_core::ExitStatus;
        use rshell_m0::rshell_session::TransportEvent;
        if self.exit_emitted {
            return Err(LocalReject::Transport);
        }
        loop {
            // 非阻塞检查原 child 对象，不调用 WinChild::try_wait 或上游 Exit 通知。
            if let Some(pty) = self.pty.as_mut()
                && let Some(code) = pty.poll_natural_exit()?
            {
                let mut pty = self.pty.take().ok_or(LocalReject::Transport)?;
                self.root
                    .value()
                    .io
                    .preserve_output
                    .store(true, Ordering::Release);
                self.completion = Some(
                    std::thread::Builder::new()
                        .name("guo-local-completion".into())
                        .spawn_scoped(self.scope, move || pty.finish_natural(code))
                        .map_err(|_| LocalReject::Transport)?,
                );
            }
            let mut bytes = [0; OUTPUT_LIMIT];
            let count = self.root.value().take_output(&mut bytes);
            if count != 0 {
                return Ok(TransportEvent::Output(bytes[..count].to_vec()));
            }
            if self
                .completion
                .as_ref()
                .is_some_and(|worker| worker.is_finished())
            {
                let code = self
                    .completion
                    .take()
                    .ok_or(LocalReject::Transport)?
                    .join()
                    .map_err(|_| LocalReject::Transport)??;
                self.root.value().observe(ProbeBoundary::CompletionJoined);
                // join 后再取一次单槽：EOF 前 reader 的最后 publish 不能落在 Exit 之后。
                let count = self.root.value().take_output(&mut bytes);
                if count != 0 {
                    self.completion_code = Some(code);
                    return Ok(TransportEvent::Output(bytes[..count].to_vec()));
                }
                self.completion_code = Some(code);
            }
            if let Some(code) = self.completion_code.take() {
                self.exit_emitted = true;
                self.root
                    .value()
                    .observe(ProbeBoundary::TransportExitReceived);
                return Ok(TransportEvent::Exit(ExitStatus {
                    code: Some(code as i32),
                    success: code == 0,
                }));
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    }

    pub(crate) async fn write(&mut self, bytes: &[u8]) -> Result<(), LocalReject> {
        if bytes.len() > INPUT_LIMIT {
            return Err(LocalReject::InputCapacity);
        }
        self.root.value().gate.check()?;
        self.root
            .value()
            .observe(ProbeBoundary::TransportWriteEntered);
        let result = self.pty_mut()?.write_manual(bytes);
        self.root
            .value()
            .observe(ProbeBoundary::TransportWriteReturned);
        result
    }

    pub(crate) async fn resize(&mut self, size: TerminalSize) -> Result<(), LocalReject> {
        check_geometry(size)?;
        self.root.value().gate.check()?;
        self.pty_mut()?.resize(size)
    }

    pub(crate) async fn shutdown(&mut self) -> Result<(), LocalReject> {
        self.root.value().gate.request_cancel();
        // 放弃交付时切换为 drain-only；不能让 completion join 被满 UI 槽卡住。
        self.root
            .value()
            .io
            .preserve_output
            .store(false, Ordering::Release);
        let result = if let Some(pty) = self.pty.as_mut() {
            pty.shutdown().map(|_| ())
        } else if let Some(worker) = self.completion.take() {
            let result = worker
                .join()
                .map_err(|_| LocalReject::Transport)?
                .map(|_| ());
            self.root.value().observe(ProbeBoundary::CompletionJoined);
            result
        } else {
            Ok(())
        };
        self.root
            .value()
            .observe(ProbeBoundary::TransportShutdownReturned);
        result
    }
}

#[cfg(windows)]
impl Drop for LocalProcessOwner<'_, '_, '_> {
    fn drop(&mut self) {
        self.root
            .value()
            .io
            .preserve_output
            .store(false, Ordering::Release);
        drop(self.pty.take());
        if let Some(worker) = self.completion.take() {
            match worker.join() {
                Ok(Ok(_)) => {}
                Ok(Err(_)) | Err(_) => self
                    .root
                    .value()
                    .observe(ProbeBoundary::OperationErrorRecorded),
            }
            self.root.value().observe(ProbeBoundary::CompletionJoined);
        }
        self.root
            .value()
            .observe(ProbeBoundary::TransportDropReturned);
    }
}

#[cfg(windows)]
fn check_geometry(size: TerminalSize) -> Result<(), LocalReject> {
    if size.cols == 0
        || size.rows == 0
        || size.cols > i16::MAX as u16
        || size.rows > i16::MAX as u16
        || size.pixel_width > u16::MAX as u32
        || size.pixel_height > u16::MAX as u32
    {
        return Err(LocalReject::InvalidGeometry);
    }
    Ok(())
}

/// Windows 独立 PTY 所有权接缝；尚未注册生产适配器或认证完整源预算。
#[cfg(windows)]
pub(crate) mod pty_owner {
    use std::io::{Read, Write};
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, AtomicUsize, Ordering};
    use std::thread::{Scope, ScopedJoinHandle};
    use std::time::{Duration, Instant};

    use portable_pty::win::conpty::ConPtyMasterPty;
    use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, SlavePty, native_pty_system};
    use rshell_platform::WindowsProcessJob;
    use windows_sys::Win32::Foundation::{
        DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentThread, GetCurrentThreadId, GetExitCodeProcess, GetThreadId,
        WaitForSingleObject,
    };

    use super::{
        AppRoot, Fixture, INPUT_LIMIT, LocalLaunch, LocalReject, OUTPUT_LIMIT, ProbeBoundary,
        ProbeRoot, TerminalSize, check_geometry,
    };

    // ABI 与已锁 windows-sys/Windows SDK 相同；不为一个函数修改父持 feature。
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CancelSynchronousIo(thread: HANDLE) -> i32;
    }

    const OPEN: u8 = 0;
    const IN_CALL: u8 = 1;
    const CLOSING: u8 = 2;
    const CLOSED: u8 = 4;
    const WRITE_CHUNK: usize = 8 * 1024;
    const CLOSE_LIMIT: Duration = Duration::from_secs(2);

    struct ThreadTarget {
        handle: OwnedHandle,
        id: u32,
    }

    pub(crate) struct IoState {
        state: AtomicU8,
        closing: AtomicBool,
        pub(crate) preserve_output: AtomicBool,
        pub(crate) start_writer: AtomicBool,
        pub(crate) resume_second: AtomicBool,
        target: OnceLock<ThreadTarget>,
        pub(crate) entered: AtomicUsize,
        pub(crate) returned: AtomicUsize,
        pub(crate) written: AtomicUsize,
        pub(crate) drained: AtomicUsize,
        pub(crate) cancel_calls: AtomicUsize,
        pub(crate) cancel_marked: AtomicUsize,
        pub(crate) cancel_not_found: AtomicUsize,
        pub(crate) last_write_error: AtomicI32,
        pub(crate) batch_done: AtomicBool,
        pub(crate) first_done: AtomicBool,
        pub(crate) waiting_ui: AtomicBool,
        operation_error: AtomicBool,
        #[cfg(test)]
        pub(crate) read_limit: AtomicUsize,
        #[cfg(test)]
        faults: AtomicU8,
    }

    impl IoState {
        pub(crate) const fn new() -> Self {
            Self {
                state: AtomicU8::new(OPEN),
                closing: AtomicBool::new(false),
                preserve_output: AtomicBool::new(false),
                start_writer: AtomicBool::new(false),
                resume_second: AtomicBool::new(false),
                target: OnceLock::new(),
                entered: AtomicUsize::new(0),
                returned: AtomicUsize::new(0),
                written: AtomicUsize::new(0),
                drained: AtomicUsize::new(0),
                cancel_calls: AtomicUsize::new(0),
                cancel_marked: AtomicUsize::new(0),
                cancel_not_found: AtomicUsize::new(0),
                last_write_error: AtomicI32::new(0),
                batch_done: AtomicBool::new(false),
                first_done: AtomicBool::new(false),
                waiting_ui: AtomicBool::new(false),
                operation_error: AtomicBool::new(false),
                #[cfg(test)]
                read_limit: AtomicUsize::new(OUTPUT_LIMIT),
                #[cfg(test)]
                faults: AtomicU8::new(0),
            }
        }

        #[cfg(test)]
        pub(crate) fn inject(&self, fault: ProbeFault) {
            self.faults.fetch_or(fault as u8, Ordering::AcqRel);
        }

        #[cfg(test)]
        fn take_fault(&self, fault: ProbeFault) -> bool {
            self.faults.fetch_and(!(fault as u8), Ordering::AcqRel) & fault as u8 != 0
        }
    }

    #[cfg(test)]
    #[derive(Clone, Copy)]
    pub(crate) enum ProbeFault {
        ThreadTarget = 1,
        CancelCall = 2,
        ChildWait = 4,
    }

    #[derive(Clone, Copy)]
    pub(crate) enum WriterInput<'scope> {
        Reserved,
        /// 仅独立非 UI basic 消费者在自身线程同步写；不是 A1 worker 准入证明。
        Manual,
        TwoStage {
            first: &'scope [u8],
            second: &'scope [u8],
        },
        #[cfg(test)]
        CancelBeforeChild,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub(crate) enum WriteOutcome {
        Complete(usize),
        Closing {
            committed: usize,
            error: Option<i32>,
        },
        Failed {
            committed: usize,
            error: Option<i32>,
        },
    }

    fn thread_target() -> Result<ThreadTarget, LocalReject> {
        let mut handle = std::ptr::null_mut();
        // SAFETY: 在真正 writer 线程复制自身 pseudo handle；不查找或终止别人的线程。
        let ok = unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                GetCurrentThread(),
                GetCurrentProcess(),
                &mut handle,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        };
        if ok == 0 {
            return Err(LocalReject::Transport);
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
        let id = unsafe { GetCurrentThreadId() };
        Ok(ThreadTarget { handle, id })
    }

    fn write_bytes(writer: &mut dyn Write, root: &AppRoot, bytes: &[u8]) -> WriteOutcome {
        let mut committed = root.io.written.load(Ordering::Acquire);
        for chunk in bytes.chunks(WRITE_CHUNK) {
            let mut remaining = chunk;
            while !remaining.is_empty() {
                if root
                    .io
                    .state
                    .compare_exchange(OPEN, IN_CALL, Ordering::AcqRel, Ordering::Acquire)
                    .is_err()
                {
                    return WriteOutcome::Closing {
                        committed,
                        error: None,
                    };
                }
                root.io.entered.fetch_add(1, Ordering::AcqRel);
                root.observe(ProbeBoundary::SystemWriteEntered);
                // 固定 Native ConPTY 的 LibraryContract：这里对应一次同步 WriteFile。
                let result = writer.write(remaining);
                root.io.returned.fetch_add(1, Ordering::AcqRel);
                root.observe(ProbeBoundary::SystemWriteReturned);
                let closing = root
                    .io
                    .state
                    .compare_exchange(IN_CALL, OPEN, Ordering::AcqRel, Ordering::Acquire)
                    .is_err();
                match result {
                    Ok(count) if count > 0 => {
                        committed += count;
                        root.io.written.store(committed, Ordering::Release);
                        remaining = &remaining[count..];
                        if closing {
                            return WriteOutcome::Closing {
                                committed,
                                error: None,
                            };
                        }
                    }
                    Ok(_) => {
                        return WriteOutcome::Failed {
                            committed,
                            error: None,
                        };
                    }
                    Err(error) => {
                        let code = error.raw_os_error();
                        if let Some(code) = code {
                            root.io.last_write_error.store(code, Ordering::Release);
                        }
                        return if closing {
                            WriteOutcome::Closing {
                                committed,
                                error: code,
                            }
                        } else {
                            WriteOutcome::Failed {
                                committed,
                                error: code,
                            }
                        };
                    }
                }
            }
        }
        WriteOutcome::Complete(committed)
    }

    fn writer_loop(
        mut writer: Box<dyn Write + Send>,
        root: ProbeRoot<'_>,
        input: WriterInput<'_>,
    ) -> Result<WriteOutcome, LocalReject> {
        let result = (|| {
            #[cfg(test)]
            if root.value().io.take_fault(ProbeFault::ThreadTarget) {
                return Err(LocalReject::Transport);
            }
            let target = thread_target()?;
            if root.value().io.target.set(target).is_err() {
                return Err(LocalReject::Transport);
            }
            while !root.value().io.start_writer.load(Ordering::Acquire)
                && !root.value().io.closing.load(Ordering::Acquire)
            {
                std::thread::sleep(Duration::from_millis(1));
            }
            let first = match input {
                WriterInput::Reserved => root.payload(),
                WriterInput::TwoStage { first, .. } => first,
                WriterInput::Manual => unreachable!(),
                #[cfg(test)]
                WriterInput::CancelBeforeChild => b"",
            };
            let mut outcome = write_bytes(&mut *writer, root.value(), first);
            root.value().io.first_done.store(true, Ordering::Release);
            if matches!(outcome, WriteOutcome::Complete(_))
                && let WriterInput::TwoStage { second, .. } = input
            {
                while !root.value().io.resume_second.load(Ordering::Acquire)
                    && !root.value().io.closing.load(Ordering::Acquire)
                {
                    std::thread::sleep(Duration::from_millis(1));
                }
                outcome = write_bytes(&mut *writer, root.value(), second);
            }
            if matches!(outcome, WriteOutcome::Complete(_)) {
                writer.flush().map_err(|_| LocalReject::Transport)?;
            }
            root.value().io.batch_done.store(true, Ordering::Release);
            while !root.value().io.closing.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(outcome)
        })();
        // 系统调用已实际返回；此确认后此 worker 没有新的 write/flush 路径。
        publish_closed(root.value(), || {});
        drop(writer);
        root.value().observe(ProbeBoundary::WriterDropReturned);
        result
    }

    fn publish_closed(root: &AppRoot, between: impl FnOnce()) {
        root.observe(ProbeBoundary::CancelConfirmed);
        between();
        root.io.state.store(CLOSED, Ordering::Release);
    }

    fn reader_loop(mut reader: Box<dyn Read + Send>, root: ProbeRoot<'_>) -> std::io::Result<()> {
        let mut bytes = [0; OUTPUT_LIMIT];
        let mut draining = false;
        let result = loop {
            if root.value().io.closing.load(Ordering::Acquire) && !draining {
                draining = true;
                root.value().observe(ProbeBoundary::ReaderDrainStarted);
            }
            #[cfg(test)]
            let limit = root
                .value()
                .io
                .read_limit
                .load(Ordering::Acquire)
                .clamp(1, OUTPUT_LIMIT);
            #[cfg(not(test))]
            let limit = OUTPUT_LIMIT;
            let count = match reader.read(&mut bytes[..limit]) {
                Ok(count) => count,
                Err(error) => break Err(error),
            };
            if count == 0 {
                break Ok(());
            }
            loop {
                if root.value().io.closing.load(Ordering::Acquire)
                    && !root.value().io.preserve_output.load(Ordering::Acquire)
                {
                    if !draining {
                        draining = true;
                        root.value().observe(ProbeBoundary::ReaderDrainStarted);
                    }
                    root.value().io.drained.fetch_add(count, Ordering::AcqRel);
                    root.value().io.waiting_ui.store(false, Ordering::Release);
                    break;
                }
                match root.value().publish(&bytes[..count]) {
                    Ok(()) => {
                        root.value().io.waiting_ui.store(false, Ordering::Release);
                        break;
                    }
                    Err(LocalReject::OutputCapacity) => {
                        root.value().io.waiting_ui.store(true, Ordering::Release);
                        std::thread::sleep(Duration::from_millis(1))
                    }
                    Err(_) => unreachable!(),
                }
            }
        };
        drop(reader);
        root.value().observe(ProbeBoundary::ReaderDropReturned);
        result
    }

    /// 完成源只暴露零超时对象等待与 signaled 后的 final code 读取。
    trait CompletionSource {
        fn signaled(&self) -> Result<bool, LocalReject>;
        fn final_code(&self) -> Result<u32, LocalReject>;
    }

    struct OriginalChild(HANDLE);

    impl CompletionSource for OriginalChild {
        fn signaled(&self) -> Result<bool, LocalReject> {
            // SAFETY: 仅在仍自持 child 时创建此短借用；不按 PID 重新打开进程。
            match unsafe { WaitForSingleObject(self.0, 0) } {
                WAIT_OBJECT_0 => Ok(true),
                WAIT_TIMEOUT => Ok(false),
                _ => Err(LocalReject::Transport),
            }
        }

        fn final_code(&self) -> Result<u32, LocalReject> {
            let mut code = 0;
            if unsafe { GetExitCodeProcess(self.0, &mut code) } == 0 {
                return Err(LocalReject::Transport);
            }
            Ok(code)
        }
    }

    #[derive(Default)]
    struct CompletionLatch(Option<u32>);

    impl CompletionLatch {
        fn poll(
            &mut self,
            source: &impl CompletionSource,
            root: &AppRoot,
        ) -> Result<Option<u32>, LocalReject> {
            if self.0.is_some() {
                return Ok(self.0);
            }
            if !source.signaled()? {
                return Ok(None);
            }
            root.observe(ProbeBoundary::ProcessObjectSignaled);
            let code = source.final_code()?;
            root.observe(ProbeBoundary::FinalExitCodeRead);
            self.0 = Some(code);
            Ok(Some(code))
        }
    }

    pub(crate) struct PtyOwner<'scope, 'root: 'scope> {
        master: Option<Box<dyn MasterPty + Send>>,
        slave: Option<Box<dyn SlavePty + Send>>,
        last_master: Option<ConPtyMasterPty>,
        child: Option<Box<dyn Child + Send + Sync>>,
        job: Option<WindowsProcessJob>,
        writer: Option<ScopedJoinHandle<'scope, Result<WriteOutcome, LocalReject>>>,
        manual_writer: Option<Box<dyn Write + Send>>,
        reader: Option<ScopedJoinHandle<'scope, std::io::Result<()>>>,
        completion: CompletionLatch,
        closed: bool,
        closed_result: Option<Result<(WriteOutcome, u32), LocalReject>>,
        primary_error: Option<LocalReject>,
        #[cfg(test)]
        probe_ledger: Option<&'root super::ProcessLedger>,
        // 字段析构也是最后一道持额边界；不把 Root 放在资源字段之前。
        pub(crate) root: ProbeRoot<'root>,
    }

    impl<'scope, 'root: 'scope> PtyOwner<'scope, 'root> {
        #[cfg(test)]
        pub(crate) fn launch(
            scope: &'scope Scope<'scope, '_>,
            root: ProbeRoot<'root>,
            fixture: Fixture,
            input: WriterInput<'scope>,
            size: TerminalSize,
            terminal_type: &str,
            prepared: impl FnOnce(&mut Self),
        ) -> Result<Self, LocalReject> {
            check_geometry(size)?;
            root.value().gate.check()?;
            if terminal_type.is_empty() || terminal_type.contains('\0') {
                return Err(LocalReject::Transport);
            }
            if let WriterInput::TwoStage { first, second } = input
                && first
                    .len()
                    .checked_add(second.len())
                    .ok_or(LocalReject::InputCapacity)?
                    > INPUT_LIMIT
            {
                return Err(LocalReject::InputCapacity);
            }
            let LocalLaunch::Command {
                program,
                args,
                cwd,
                env,
            } = fixture.launch()
            else {
                unreachable!()
            };
            let mut command = CommandBuilder::new(program);
            command.args(args);
            if let Some(cwd) = cwd {
                command.cwd(cwd);
            }
            for (key, value) in env {
                command.env(key, value);
            }
            command.env("TERM", terminal_type);
            let read_root = root.try_clone()?;
            let write_root = root.try_clone()?;
            let job = WindowsProcessJob::new().map_err(|_| LocalReject::Transport)?;
            let pair = native_pty_system()
                .openpty(PtySize {
                    cols: size.cols,
                    rows: size.rows,
                    pixel_width: size.pixel_width as u16,
                    pixel_height: size.pixel_height as u16,
                })
                .map_err(|_| LocalReject::Transport)?;
            let master: &dyn MasterPty = &*pair.master;
            let last_master = master
                .downcast_ref::<ConPtyMasterPty>()
                .ok_or(LocalReject::Transport)?
                .clone();
            // 所有可失败的后续步骤由这个部分初始化 owner 持有。
            let mut owner = Self {
                root,
                master: Some(pair.master),
                slave: Some(pair.slave),
                last_master: Some(last_master),
                child: None,
                job: Some(job),
                writer: None,
                manual_writer: None,
                reader: None,
                completion: CompletionLatch::default(),
                closed: false,
                closed_result: None,
                primary_error: None,
                probe_ledger: None,
            };
            if !matches!(input, WriterInput::CancelBeforeChild) {
                // LibraryContract：spawn fallback 能替换 pipes，业务 endpoints 必须随后取得。
                let child = owner
                    .slave
                    .as_ref()
                    .ok_or(LocalReject::Transport)?
                    .spawn_command_in_job(
                        command,
                        owner
                            .job
                            .as_ref()
                            .ok_or(LocalReject::Transport)?
                            .as_borrowed_handle(),
                    )
                    .map_err(|_| LocalReject::Transport)?;
                owner.child = Some(child);
                drop(owner.slave.take());
                owner.root.value().observe(ProbeBoundary::SpawnReturned);
            }
            let reader = owner
                .master
                .as_ref()
                .ok_or(LocalReject::Transport)?
                .try_clone_reader()
                .map_err(|_| LocalReject::Transport)?;
            let writer = owner
                .master
                .as_ref()
                .ok_or(LocalReject::Transport)?
                .take_writer()
                .map_err(|_| LocalReject::Transport)?;
            let reader = std::thread::Builder::new()
                .name("guo-local-reader".into())
                .spawn_scoped(scope, move || reader_loop(reader, read_root))
                .map_err(|_| LocalReject::Transport)?;
            owner.reader = Some(reader);
            prepared(&mut owner);
            if matches!(input, WriterInput::Manual) {
                drop(write_root);
                owner.manual_writer = Some(writer);
                return Ok(owner);
            }
            let writer = std::thread::Builder::new()
                .name("guo-local-writer".into())
                .spawn_scoped(scope, move || writer_loop(writer, write_root, input))
                .map_err(|_| LocalReject::Transport)?;
            owner.writer = Some(writer);
            let deadline = Instant::now() + CLOSE_LIMIT;
            while owner.root.value().io.target.get().is_none() {
                if owner.root.value().io.state.load(Ordering::Acquire) == CLOSED
                    || Instant::now() >= deadline
                {
                    return Err(LocalReject::Transport);
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            if matches!(input, WriterInput::CancelBeforeChild) {
                return Err(LocalReject::Cancelled);
            }
            Ok(owner)
        }

        pub(crate) fn process_id(&self) -> Option<u32> {
            self.child.as_ref().and_then(|child| child.process_id())
        }

        pub(crate) fn contains_process(&self, pid: u32) -> Result<bool, LocalReject> {
            self.job
                .as_ref()
                .ok_or(LocalReject::Transport)?
                .contains_process(pid)
                .map_err(|_| LocalReject::Transport)
        }

        pub(crate) fn resize(&self, size: TerminalSize) -> Result<(), LocalReject> {
            check_geometry(size)?;
            self.root.value().gate.check()?;
            self.master
                .as_ref()
                .ok_or(LocalReject::Transport)?
                .resize(PtySize {
                    cols: size.cols,
                    rows: size.rows,
                    pixel_width: size.pixel_width as u16,
                    pixel_height: size.pixel_height as u16,
                })
                .map_err(|_| LocalReject::Transport)
        }

        pub(crate) fn write_manual(&mut self, bytes: &[u8]) -> Result<(), LocalReject> {
            if bytes.len() > INPUT_LIMIT {
                return Err(LocalReject::InputCapacity);
            }
            self.root.value().gate.check()?;
            let writer = self.manual_writer.as_mut().ok_or(LocalReject::Transport)?;
            match write_bytes(&mut **writer, self.root.value(), bytes) {
                WriteOutcome::Complete(_) => writer.flush().map_err(|_| LocalReject::Transport),
                WriteOutcome::Closing { .. } => Err(LocalReject::Cancelled),
                WriteOutcome::Failed { .. } => Err(LocalReject::Transport),
            }
        }

        pub(crate) fn poll_natural_exit(&mut self) -> Result<Option<u32>, LocalReject> {
            let handle = self
                .child
                .as_ref()
                .ok_or(LocalReject::Transport)?
                .as_raw_handle()
                .ok_or(LocalReject::Transport)?;
            self.completion
                .poll(&OriginalChild(handle), self.root.value())
        }

        #[cfg(test)]
        pub(crate) fn probe_visible_code_before_object_signal(
            &mut self,
            visible_code: u32,
        ) -> Result<(), LocalReject> {
            let handle = self
                .child
                .as_ref()
                .ok_or(LocalReject::Transport)?
                .as_raw_handle()
                .ok_or(LocalReject::Transport)?;
            assert_eq!(OriginalChild(handle).signaled(), Ok(false));
            struct PendingSource(u32);
            impl CompletionSource for PendingSource {
                fn signaled(&self) -> Result<bool, LocalReject> {
                    Ok(false)
                }
                fn final_code(&self) -> Result<u32, LocalReject> {
                    Ok(self.0)
                }
            }
            assert_eq!(
                self.completion
                    .poll(&PendingSource(visible_code), self.root.value()),
                Ok(None)
            );
            assert!(self.master.is_some() && self.last_master.is_some());
            assert!(self.writer.is_some() || self.manual_writer.is_some());
            assert!(self.child.is_some());
            for boundary in [
                ProbeBoundary::TransportExitReceived,
                ProbeBoundary::WriterDropReturned,
                ProbeBoundary::MasterNonLastDropReturned,
                ProbeBoundary::PtyLastHolderReturned,
            ] {
                assert_eq!(self.root.value().observed(boundary), 0);
            }
            Ok(())
        }

        pub(crate) fn finish_natural(&mut self, code: u32) -> Result<u32, LocalReject> {
            if self.completion.0 != Some(code) {
                return Err(LocalReject::Transport);
            }
            let (_, exit) = self.shutdown()?;
            if exit != code {
                return Err(LocalReject::Transport);
            }
            Ok(exit)
        }

        pub(crate) fn request_cancel(&self) -> Result<(), LocalReject> {
            let io = &self.root.value().io;
            let mut operation_error = None;
            #[cfg(test)]
            if io.take_fault(ProbeFault::CancelCall) {
                io.operation_error.store(true, Ordering::Release);
                operation_error = Some(LocalReject::Transport);
            }
            self.root.value().cancel_dispatch();
            io.closing.store(true, Ordering::Release);
            let _ = io
                .state
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                    if state == CLOSED {
                        None
                    } else {
                        Some(state | CLOSING)
                    }
                });
            let deadline = Instant::now() + CLOSE_LIMIT;
            while io.state.load(Ordering::Acquire) != CLOSED {
                if Instant::now() >= deadline {
                    return Err(LocalReject::CleanupIncomplete);
                }
                if let Some(target) = io.target.get() {
                    // SAFETY: 真实 writer 自身复制并持有的 handle；不查询裸线程 ID 来打开目标。
                    if unsafe { GetThreadId(target.handle.as_raw_handle()) } != target.id {
                        return Err(LocalReject::Transport);
                    }
                    io.cancel_calls.fetch_add(1, Ordering::AcqRel);
                    let marked = unsafe { CancelSynchronousIo(target.handle.as_raw_handle()) };
                    if marked != 0 {
                        io.cancel_marked.fetch_add(1, Ordering::AcqRel);
                    } else {
                        let error = std::io::Error::last_os_error();
                        if error.raw_os_error() == Some(1168) {
                            io.cancel_not_found.fetch_add(1, Ordering::AcqRel);
                        } else {
                            io.operation_error.store(true, Ordering::Release);
                            operation_error = Some(LocalReject::Transport);
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            // ERROR_NOT_FOUND 或取消调用成功不是确认；只有 worker 的 CLOSED 交接才是。
            if io.entered.load(Ordering::Acquire) != io.returned.load(Ordering::Acquire) {
                return Err(LocalReject::Transport);
            }
            operation_error.map_or(Ok(()), Err)
        }

        fn cleanup(&mut self) -> Result<(Option<WriteOutcome>, Option<u32>), LocalReject> {
            if self.closed {
                return Ok((None, None));
            }
            self.root.value().io.closing.store(true, Ordering::Release);
            if self.root.value().io.operation_error.load(Ordering::Acquire) {
                self.primary_error.get_or_insert(LocalReject::Transport);
            }
            let outcome = if self.writer.is_some() {
                if let Err(error) = self.request_cancel() {
                    if error != LocalReject::CleanupIncomplete {
                        self.primary_error.get_or_insert(error);
                    }
                    if self.root.value().io.state.load(Ordering::Acquire) != CLOSED {
                        return Err(LocalReject::CleanupIncomplete);
                    }
                }
                let writer = self.writer.take().ok_or(LocalReject::Transport)?;
                let outcome = match writer.join() {
                    Ok(Ok(outcome)) => Some(outcome),
                    Ok(Err(error)) => {
                        self.primary_error.get_or_insert(error);
                        None
                    }
                    Err(_) => {
                        self.primary_error.get_or_insert(LocalReject::Transport);
                        None
                    }
                };
                self.observe_cleanup(ProbeBoundary::WriterJoined);
                outcome
            } else if let Some(writer) = self.manual_writer.take() {
                // Manual 只由同一非 UI owner 调用；所有同步写已返回后才可到达清理。
                self.root.value().cancel_dispatch();
                if self.root.value().io.entered.load(Ordering::Acquire)
                    != self.root.value().io.returned.load(Ordering::Acquire)
                {
                    return Err(LocalReject::Transport);
                }
                publish_closed(self.root.value(), || {});
                drop(writer);
                self.observe_cleanup(ProbeBoundary::WriterDropReturned);
                Some(WriteOutcome::Complete(
                    self.root.value().io.written.load(Ordering::Acquire),
                ))
            } else {
                None
            };
            if let Some(job) = self.job.as_mut()
                && job.terminate().is_err()
            {
                self.primary_error.get_or_insert(LocalReject::Transport);
            }
            let exit = if let Some(child) = self.child.as_mut() {
                let handle = child.as_raw_handle().ok_or(LocalReject::Transport)?;
                // SAFETY: child 原持有者仍存活；有限等待之后再观察 trait wait/reap 返回。
                if unsafe { WaitForSingleObject(handle, 2_000) } != WAIT_OBJECT_0 {
                    return Err(LocalReject::CleanupIncomplete);
                }
                let code = if let Some(code) = self.completion.0 {
                    code
                } else {
                    self.completion
                        .poll(&OriginalChild(handle), self.root.value())?
                        .ok_or(LocalReject::Transport)?
                };
                #[cfg(test)]
                if self.root.value().io.take_fault(ProbeFault::ChildWait) {
                    self.primary_error.get_or_insert(LocalReject::Transport);
                }
                let status = match child.wait() {
                    Ok(status) => status,
                    Err(_) => {
                        self.primary_error.get_or_insert(LocalReject::Transport);
                        return Err(LocalReject::CleanupIncomplete);
                    }
                };
                if status.exit_code() != code {
                    self.primary_error.get_or_insert(LocalReject::Transport);
                }
                self.observe_cleanup(ProbeBoundary::ChildReaped);
                Some(code)
            } else {
                None
            };
            drop(self.child.take());
            drop(self.slave.take());
            drop(self.master.take());
            self.observe_cleanup(ProbeBoundary::MasterNonLastDropReturned);
            // slave 已释放且 writer/reader 只持有 pipe；这个 Clone 是 HPCON 的最后 strong holder。
            drop(self.last_master.take());
            self.observe_cleanup(ProbeBoundary::PtyLastHolderReturned);
            if let Some(reader) = self.reader.take() {
                if !matches!(reader.join(), Ok(Ok(()))) {
                    self.primary_error.get_or_insert(LocalReject::Transport);
                }
                self.observe_cleanup(ProbeBoundary::ReaderJoined);
            }
            drop(self.job.take());
            self.observe_cleanup(ProbeBoundary::JobDropReturned);
            self.observe_cleanup(ProbeBoundary::AllWorkersDone);
            self.closed = true;
            Ok((outcome, exit))
        }

        fn observe_cleanup(&self, boundary: ProbeBoundary) {
            #[cfg(test)]
            if let Some(ledger) = self.probe_ledger {
                assert_eq!(
                    ledger.snapshot().rust_bytes,
                    self.root.admitted().rust_bytes,
                    "仅 owner 自持 Root 必须覆盖实际资源结束：{boundary:?}"
                );
            }
            self.root.value().observe(boundary);
        }

        #[cfg(test)]
        pub(crate) fn audit_unique_root(&mut self, ledger: &'root super::ProcessLedger) {
            self.probe_ledger = Some(ledger);
        }

        pub(crate) fn shutdown(&mut self) -> Result<(WriteOutcome, u32), LocalReject> {
            if let Some(result) = self.closed_result {
                return result;
            }
            let (outcome, exit) = self.cleanup()?;
            let result = if let Some(error) = self.primary_error {
                Err(error)
            } else {
                Ok((
                    outcome.ok_or(LocalReject::Transport)?,
                    exit.ok_or(LocalReject::Transport)?,
                ))
            };
            self.closed_result = Some(result);
            result
        }

        pub(crate) fn wait_natural_exit(&mut self) -> Result<u32, LocalReject> {
            let child = self.child.as_ref().ok_or(LocalReject::Transport)?;
            let handle = child.as_raw_handle().ok_or(LocalReject::Transport)?;
            // SAFETY: 保持 child 原 handle；沿既有 2 秒期限验证自然退出。
            if unsafe { WaitForSingleObject(handle, 2_000) } != WAIT_OBJECT_0 {
                return Err(LocalReject::Transport);
            }
            self.poll_natural_exit()?.ok_or(LocalReject::Transport)
        }
    }

    impl<'scope, 'root: 'scope> Drop for PtyOwner<'scope, 'root> {
        fn drop(&mut self) {
            if !self.closed {
                // 未完成返回/线程创建失败时已没有输出消费者，必须实际 drain 而非等满槽。
                self.root
                    .value()
                    .io
                    .preserve_output
                    .store(false, Ordering::Release);
                // 返回清理未完成不能让字段析构丢掉 join 权。独立非 UI 根保持 pending，
                // 不把 Drop 的实际收口算作限时 shutdown 成功或延长它的期限。
                while !self.closed {
                    if let Err(error) = self.cleanup()
                        && error != LocalReject::CleanupIncomplete
                    {
                        self.primary_error.get_or_insert(error);
                    }
                    if !self.closed {
                        std::thread::yield_now();
                    }
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::super::{ProcessLedger, app_root_shape, reserve_probe_root};
        use super::*;
        use crate::selected_p1::resource::ledger::{BYTE_LIMIT, CLASS_COUNT};

        #[test]
        fn closed_acquire_always_observes_cancel_confirmation_before_writer_drop()
        -> Result<(), LocalReject> {
            let ledger = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
            let source = ledger.open_source(1, 0)?;
            let root = reserve_probe_root(&ledger, source, 0)?;
            let writer_root = root.try_clone()?;
            let barrier = std::sync::Barrier::new(2);
            let (state, ack) = std::thread::scope(|scope| {
                let worker = scope.spawn(|| {
                    publish_closed(writer_root.value(), || {
                        barrier.wait();
                        barrier.wait();
                    })
                });
                barrier.wait();
                let state = root.value().io.state.load(Ordering::Acquire);
                let ack = root.value().observed(ProbeBoundary::CancelConfirmed);
                barrier.wait();
                assert!(worker.join().is_ok());
                (state, ack)
            });
            assert!(state != CLOSED || ack != 0, "CLOSED 可见但确认字段尚未发布");
            assert_eq!(root.value().io.state.load(Ordering::Acquire), CLOSED);
            assert!(root.value().observed(ProbeBoundary::CancelConfirmed) > 0);
            assert_eq!(root.value().observed(ProbeBoundary::WriterDropReturned), 0);
            Ok(())
        }

        #[test]
        fn completion_requires_signaled_object_even_when_exit_code_is_visible()
        -> Result<(), LocalReject> {
            use std::cell::Cell;
            struct ControlledSource {
                signaled: Cell<bool>,
                visible_code: u32,
                reads: Cell<usize>,
            }
            impl CompletionSource for ControlledSource {
                fn signaled(&self) -> Result<bool, LocalReject> {
                    Ok(self.signaled.get())
                }
                fn final_code(&self) -> Result<u32, LocalReject> {
                    self.reads.set(self.reads.get() + 1);
                    Ok(self.visible_code)
                }
            }
            let ledger = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
            let source = ledger.open_source(1, 0)?;
            let root = reserve_probe_root(&ledger, source, 0)?;
            let witness = ControlledSource {
                signaled: Cell::new(false),
                visible_code: 23,
                reads: Cell::new(0),
            };
            let mut completion = CompletionLatch::default();
            assert_eq!(completion.poll(&witness, root.value()), Ok(None));
            assert_eq!(witness.reads.get(), 0);
            assert_eq!(
                root.value().observed(ProbeBoundary::ProcessObjectSignaled),
                0
            );
            assert_eq!(
                root.value().observed(ProbeBoundary::TransportExitReceived),
                0
            );
            assert_eq!(root.value().observed(ProbeBoundary::WriterDropReturned), 0);
            assert_eq!(
                root.value().observed(ProbeBoundary::PtyLastHolderReturned),
                0
            );
            assert!(root.value().gate.check().is_ok());
            root.value().publish(b"GUO_TAIL")?;
            witness.signaled.set(true);
            assert_eq!(completion.poll(&witness, root.value()), Ok(Some(23)));
            assert_eq!(completion.poll(&witness, root.value()), Ok(Some(23)));
            assert_eq!(witness.reads.get(), 1);
            assert!(
                root.value().observed(ProbeBoundary::ProcessObjectSignaled)
                    < root.value().observed(ProbeBoundary::FinalExitCodeRead)
            );
            let mut bytes = [0; OUTPUT_LIMIT];
            let count = root.value().take_output(&mut bytes);
            assert_eq!(&bytes[..count], b"GUO_TAIL");
            Ok(())
        }

        #[test]
        fn short_system_writes_preserve_byte_order_across_unicode_chunk_boundaries() {
            const LEN: usize = WRITE_CHUNK * 2 + 19;
            struct ShortWriter {
                bytes: [u8; LEN],
                len: usize,
            }
            impl Write for ShortWriter {
                fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                    let count = bytes.len().min(7);
                    self.bytes[self.len..self.len + count].copy_from_slice(&bytes[..count]);
                    self.len += count;
                    Ok(count)
                }
                fn flush(&mut self) -> std::io::Result<()> {
                    Ok(())
                }
            }
            let mut input = [0; LEN];
            for (index, byte) in input.iter_mut().enumerate() {
                *byte = b'a' + (index % 26) as u8;
            }
            let unicode = "λ-终端".as_bytes();
            input[WRITE_CHUNK - 1..WRITE_CHUNK - 1 + unicode.len()].copy_from_slice(unicode);
            let ledger = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
            let source = match ledger.open_source(1, 0) {
                Ok(source) => source,
                Err(error) => panic!("测试 Source 失败：{error:?}"),
            };
            let root = match reserve_probe_root(&ledger, source, 0) {
                Ok(root) => root,
                Err(error) => panic!("测试 root 失败：{error:?}"),
            };
            let shape = match app_root_shape(0) {
                Ok(shape) => shape,
                Err(error) => panic!("测试 layout 失败：{error:?}"),
            };
            assert_eq!(ledger.snapshot().rust_bytes, shape.backing.size());
            let mut writer = ShortWriter {
                bytes: [0; LEN],
                len: 0,
            };
            assert_eq!(
                write_bytes(&mut writer, root.value(), &input),
                WriteOutcome::Complete(LEN)
            );
            assert_eq!(writer.bytes, input);
            assert_eq!(
                root.value().io.entered.load(Ordering::Acquire),
                root.value().io.returned.load(Ordering::Acquire)
            );
            root.value().io.state.store(CLOSING, Ordering::Release);
            assert_eq!(
                write_bytes(&mut writer, root.value(), b"not-admitted"),
                WriteOutcome::Closing {
                    committed: LEN,
                    error: None
                }
            );
            assert_eq!(writer.len, LEN);
            drop(root);
            assert_eq!(ledger.snapshot().rust_bytes, 0);
        }
    }
}
