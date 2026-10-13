//! 独立 integration 消费 Local；有限监督仅保护 fixture，不放行产品关口。

#[path = "../src/selected_p1/font_source/audit.rs"]
pub mod audit;
#[path = "../src/selected_p1/model/layout.rs"]
pub mod layout;
#[path = "../src/selected_p1/resource/ledger.rs"]
pub mod ledger;
#[path = "../src/selected_p1/resource/owner.rs"]
pub mod owner;
#[path = "../src/selected_p1/resource/secret.rs"]
pub mod secret;

pub mod selected_p1 {
    pub mod model {
        pub use crate::layout;
    }
    pub mod resource {
        pub use crate::{ledger, owner, secret};
    }
}

#[path = "../src/local_shell/mod.rs"]
mod local_shell;

#[global_allocator]
static ALLOCATOR: audit::Allocator = audit::Allocator;

use ledger::{BYTE_LIMIT, CLASS_COUNT, ProcessLedger};
use local_shell::{
    DispatchGate, INPUT_LIMIT, LocalReject, OUTPUT_LIMIT, ProbeBoundary, app_root_shape,
    reserve_probe_root, validate_local_fields,
};
#[cfg(windows)]
use local_shell::{Fixture, LocalProcessOwner};

fn succeeds<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("预期成功：{error:?}"),
    }
}

#[test]
fn local_shell_fields_reject_saved_ssh_before_lookup() {
    for id in ["synthetic", "12dde45b-6b62-4a24-bcf7-3c8fd4a74948"] {
        assert_eq!(
            validate_local_fields(id, "", 0, "", "", ""),
            Err(LocalReject::SavedLocalContractMissing)
        );
    }
    succeeds(validate_local_fields("", "", 0, "", "", ""));
    for fields in [
        ("host", 0, "", "", ""),
        ("", 22, "", "", ""),
        ("", 0, "user", "", ""),
        ("", 0, "", "echo", ""),
        ("", 0, "", "", "synthetic"),
    ] {
        assert_eq!(
            validate_local_fields("", fields.0, fields.1, fields.2, fields.3, fields.4),
            Err(LocalReject::RemoteFields)
        );
    }
}

#[test]
fn local_shell_app_root_granted_before_allocate_and_released_after_last_holder() {
    let ledger = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
    let source = succeeds(ledger.open_source(1, 0));
    let shape = succeeds(app_root_shape(17));
    let scope = audit::AuditScope::start(&ledger);
    let root = succeeds(reserve_probe_root(&ledger, source, 17));
    assert_eq!(root.value().source, source);
    assert_eq!(root.admitted().rust_bytes, shape.backing.size());
    assert_eq!(root.payload(), &[b'x'; 17]);
    let late = succeeds(root.try_clone());
    succeeds(ledger.advance_epoch(1));
    root.value().cancel_dispatch();
    assert_eq!(root.value().gate.check(), Err(LocalReject::Cancelled));
    assert_eq!(
        reserve_probe_root(&ledger, source, 0).err(),
        Some(LocalReject::Resource(layout::NumericReject::EpochMismatch))
    );
    drop(root);
    assert_eq!(ledger.snapshot().rust_bytes, shape.backing.size());
    late.value().observe(ProbeBoundary::LateHolderReleased);
    drop(late);
    let report = scope.finish();
    assert!(report.symmetric_single_backing());
    assert!(
        report
            .events()
            .iter()
            .all(|event| event.held_rust == shape.backing.size())
    );
    assert_eq!(ledger.snapshot().rust_bytes, 0);
    eprintln!(
        "APP_ROOT H={} C=0 align={} 最后 backing 释放后返额",
        shape.backing.size(),
        shape.backing.align()
    );
    eprintln!(
        "APP_ROOT H(0)={} H(8MiB)={}",
        succeeds(app_root_shape(0)).backing.size(),
        succeeds(app_root_shape(INPUT_LIMIT)).backing.size()
    );
}

#[test]
fn local_shell_app_root_shortfall_and_failure_have_no_allocation_or_child() {
    let shape = succeeds(app_root_shape(0));
    let ledger = ProcessLedger::new(shape.backing.size() - 1, [u32::MAX; CLASS_COUNT]);
    let source = succeeds(ledger.open_source(1, 0));
    let scope = audit::AuditScope::start(&ledger);
    assert_eq!(
        reserve_probe_root(&ledger, source, 0).err(),
        Some(LocalReject::Resource(
            layout::NumericReject::RustBudgetExceeded
        ))
    );
    assert!(scope.finish().events().is_empty());
    let ledger = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
    let source = succeeds(ledger.open_source(1, 0));
    let scope = audit::AuditScope::start(&ledger);
    scope.fail_next_size(shape.backing.size());
    assert_eq!(
        reserve_probe_root(&ledger, source, 0).err(),
        Some(LocalReject::Resource(
            layout::NumericReject::AllocationFailure
        ))
    );
    assert_eq!(scope.finish().events()[0].kind, 4);
    assert_eq!(ledger.snapshot().rust_bytes, 0);
    assert_eq!(
        app_root_shape(INPUT_LIMIT + 1).err(),
        Some(LocalReject::InputCapacity)
    );
}

#[test]
fn local_shell_fixed_ui_slot_is_bounded_and_cleanup_does_not_need_to_publish() {
    let ledger = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
    let source = succeeds(ledger.open_source(1, 0));
    let root = succeeds(reserve_probe_root(&ledger, source, 0));
    succeeds(root.value().publish(b"synthetic"));
    assert_eq!(
        root.value().publish(b"later"),
        Err(LocalReject::OutputCapacity)
    );
    succeeds(ledger.invalidate_source(source));
    root.value().cancel_dispatch();
    let mut output = [0; OUTPUT_LIMIT];
    let count = root.value().take_output(&mut output);
    assert_eq!(&output[..count], b"synthetic");
    drop(root);
    assert_eq!(ledger.snapshot().rust_bytes, 0);
}

#[test]
fn local_shell_dispatch_stop_is_not_system_cancel_confirmation() {
    let gate = DispatchGate::new();
    succeeds(gate.check());
    gate.request_cancel();
    assert_eq!(gate.check(), Err(LocalReject::Cancelled));
}

#[test]
#[cfg(windows)]
fn local_shell_cancel_before_spawn_creates_no_transport() {
    let ledger = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
    let source = succeeds(ledger.open_source(1, 0));
    let root = succeeds(reserve_probe_root(&ledger, source, 0));
    let observer = succeeds(root.try_clone());
    observer.value().cancel_dispatch();
    let runtime = succeeds(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build(),
    );
    let (result, report) = std::thread::scope(|workers| {
        let audit = audit::AuditScope::start(&ledger);
        let result = runtime
            .block_on(LocalProcessOwner::launch_probe(
                workers,
                root,
                Fixture::EchoGeometryExit,
                rshell_m0::rshell_core::TerminalSize {
                    cols: 93,
                    rows: 31,
                    pixel_width: 0,
                    pixel_height: 0,
                    dpi: 96,
                },
                "guo-local-fixture",
                |_| panic!("取消的启动不能到达 spawn 完成"),
            ))
            .err();
        (result, audit.finish())
    });
    assert_eq!(result, Some(LocalReject::Cancelled));
    assert_eq!(observer.value().observed(ProbeBoundary::SpawnReturned), 0);
    assert!(report.events().is_empty());
    drop(observer);
    assert_eq!(ledger.snapshot().rust_bytes, 0);
}

#[cfg(windows)]
mod windows_probe {
    use std::io::{Read, Write};
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use super::{
        BYTE_LIMIT, CLASS_COUNT, Fixture, INPUT_LIMIT, LocalProcessOwner, ProbeBoundary,
        ProcessLedger, reserve_probe_root, succeeds,
    };
    use crate::local_shell::pty_owner::{ProbeFault, PtyOwner, WriteOutcome, WriterInput};
    use rshell_m0::rshell_core::TerminalSize;
    use rshell_m0::rshell_session::TransportEvent;
    use windows_sys::Win32::Foundation::{FILETIME, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };

    const SUPERVISOR_LIMIT: Duration = Duration::from_secs(12);
    const REPORT_CAPACITY: usize = 32 * 1024;
    const FIXTURE_MARKER: &str = "GUO_LOCAL_RUN_OWNED";
    static NEXT_RUN: AtomicUsize = AtomicUsize::new(1);

    fn geometry(cols: u16, rows: u16) -> TerminalSize {
        TerminalSize {
            cols,
            rows,
            pixel_width: 0,
            pixel_height: 0,
            dpi: 96,
        }
    }

    struct Report {
        bytes: [u8; REPORT_CAPACITY],
        len: usize,
        success: bool,
        forced: bool,
        shell_exit: Option<u32>,
    }

    struct ProcessObservation {
        handle: OwnedHandle,
        creation: u64,
    }

    fn process_creation(handle: windows_sys::Win32::Foundation::HANDLE) -> u64 {
        let mut creation = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        // SAFETY: 调用者保持原有效 process handle，四个 out 是实际 SDK 类型。
        let ok =
            unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) };
        assert_ne!(ok, 0, "读取本轮进程创建身份失败");
        (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime)
    }

    impl ProcessObservation {
        fn open(pid: u32) -> Self {
            // SAFETY: 仅观测本轮已确认 Job 包含的 child，权限只有 query/synchronize。
            let raw = unsafe {
                OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                    0,
                    pid,
                )
            };
            assert!(
                !raw.is_null(),
                "打开本轮 child 观测句柄失败：{}",
                std::io::Error::last_os_error()
            );
            let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
            let creation = process_creation(handle.as_raw_handle());
            Self { handle, creation }
        }

        fn wait_exit(&self) -> u32 {
            // SAFETY: 保留原 process handle，有限 2 秒等待；不按 PID 重新查找或终止。
            let waited = unsafe { WaitForSingleObject(self.handle.as_raw_handle(), 2_000) };
            assert_eq!(waited, WAIT_OBJECT_0, "本轮 child 未在清理期限退出");
            let mut code = 0;
            let ok = unsafe { GetExitCodeProcess(self.handle.as_raw_handle(), &mut code) };
            assert_ne!(ok, 0, "读取实际 child 退出码失败");
            code
        }
    }

    fn read_report(path: &std::path::Path) -> ([u8; REPORT_CAPACITY], usize) {
        // 独立 File cursor，不能移动 stdout 的共享文件偏移。
        let mut file = succeeds(std::fs::File::open(path));
        assert!(succeeds(file.metadata()).len() <= REPORT_CAPACITY as u64);
        let mut bytes = [0; REPORT_CAPACITY];
        let mut len = 0;
        while len < REPORT_CAPACITY {
            let count = succeeds(file.read(&mut bytes[len..]));
            if count == 0 {
                break;
            }
            len += count;
        }
        (bytes, len)
    }

    impl Report {
        fn text(&self) -> &str {
            succeeds(std::str::from_utf8(&self.bytes[..self.len]))
        }
    }

    struct OwnedReport {
        path: std::path::PathBuf,
        parent: std::path::PathBuf,
        file: Option<std::fs::File>,
    }

    impl Drop for OwnedReport {
        fn drop(&mut self) {
            drop(self.file.take());
            let parent = match self.path.parent() {
                Some(parent) => parent,
                None => panic!("报告没有父目录"),
            };
            assert_eq!(succeeds(std::fs::canonicalize(parent)), self.parent);
            succeeds(std::fs::remove_file(&self.path));
            assert!(!succeeds(self.path.try_exists()));
            eprintln!("LOCAL_SUPERVISOR_REMOVED {:?}", self.path.file_name());
        }
    }

    struct OwnedFixture {
        child: std::process::Child,
        pid: u32,
        creation: u64,
        reaped: bool,
    }

    impl OwnedFixture {
        fn terminate(&mut self) -> std::process::ExitStatus {
            assert_eq!(self.child.id(), self.pid);
            assert_eq!(process_creation(self.child.as_raw_handle()), self.creation);
            succeeds(self.child.kill());
            // SAFETY: 本轮原 Child handle；有限等待结束后才调用 std wait 收取状态。
            let waited = unsafe { WaitForSingleObject(self.child.as_raw_handle(), 2_000) };
            assert_eq!(waited, WAIT_OBJECT_0, "监督 fixture 终止未完成");
            let status = succeeds(self.child.wait());
            self.reaped = true;
            status
        }
    }

    impl Drop for OwnedFixture {
        fn drop(&mut self) {
            if !self.reaped {
                if succeeds(self.child.try_wait()).is_some() {
                    self.reaped = true;
                } else {
                    self.terminate();
                }
            }
        }
    }

    /// 保留 spawn 返回的 Child handle，只终止本轮 test fixture，不按裸 PID 杀宿主。
    fn supervise(mode: &str, write_entered: Option<&AtomicBool>) -> Report {
        let executable = succeeds(std::env::current_exe());
        let parent = std::env::temp_dir();
        assert!(parent.is_dir());
        let nonce = format!(
            "{}-{}",
            std::process::id(),
            NEXT_RUN.fetch_add(1, Ordering::Relaxed)
        );
        let path = parent.join(format!("guo-local-{nonce}.report"));
        eprintln!("LOCAL_SUPERVISOR_TEMPFILE guo-local-{nonce}.report");
        // create_new 成功后才持有删除责任；只删除此精确文件，不创建/删除任何目录。
        let file = succeeds(
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&path),
        );
        let owned_report = OwnedReport {
            path,
            parent: succeeds(std::fs::canonicalize(&parent)),
            file: Some(file),
        };
        let stdout = match &owned_report.file {
            Some(file) => succeeds(file.try_clone()),
            None => panic!("报告文件未打开"),
        };
        let child = succeeds(
            Command::new(&executable)
                .args([
                    "--exact",
                    "windows_probe::local_shell_run_owned_fixture",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(FIXTURE_MARKER, &nonce)
                .env("GUO_LOCAL_PROBE_MODE", mode)
                .env("RUSTUP_AUTO_INSTALL", "0")
                .env_remove("GUOSH_GUI_FIXTURE")
                .current_dir(&parent)
                .stdin(Stdio::piped())
                .stdout(stdout)
                .stderr(Stdio::inherit())
                .spawn(),
        );
        let mut fixture = OwnedFixture {
            pid: child.id(),
            creation: process_creation(child.as_raw_handle()),
            child,
            reaped: false,
        };
        let started = Instant::now();
        let mut forced = false;
        let mut shell = None;
        let status = loop {
            if shell.is_none() || write_entered.is_some_and(|flag| !flag.load(Ordering::Acquire)) {
                let (bytes, len) = read_report(&owned_report.path);
                let text = String::from_utf8_lossy(&bytes[..len]);
                if shell.is_none()
                    && let Some(identity) = text
                        .split_inclusive('\n')
                        .filter(|line| line.ends_with('\n'))
                        .find_map(|line| {
                            line.split_once("SHELL_IDENTITY ")
                                .map(|(_, identity)| identity)
                        })
                {
                    let mut fields = identity.split_whitespace();
                    let pid = succeeds(fields.next().ok_or("PID 缺失")).parse::<u32>();
                    let creation = succeeds(fields.next().ok_or("创建身份缺失")).parse::<u64>();
                    let observation = ProcessObservation::open(succeeds(pid));
                    assert_eq!(
                        observation.creation,
                        succeeds(creation),
                        "拒绝 PID 重用或非本轮 child"
                    );
                    shell = Some(observation);
                    // 只确认自己 fixture 的观测对象已保有；不控制 PTY 退出或延迟其清理。
                    let mut acknowledgement =
                        succeeds(fixture.child.stdin.take().ok_or("本轮确认管道缺失"));
                    succeeds(acknowledgement.write_all(b"1"));
                    drop(acknowledgement);
                }
                if text.contains("TRANSPORT_WRITE_STILL_PENDING")
                    && let Some(flag) = write_entered
                {
                    flag.store(true, Ordering::Release);
                }
            }
            if let Some(status) = succeeds(fixture.child.try_wait()) {
                fixture.reaped = true;
                break status;
            }
            if started.elapsed() >= SUPERVISOR_LIMIT {
                forced = true;
                break fixture.terminate();
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        drop(fixture);
        let shell_exit = shell.as_ref().map(ProcessObservation::wait_exit);
        drop(shell);
        let (bytes, len) = read_report(&owned_report.path);
        let report = Report {
            bytes,
            len,
            success: status.success(),
            forced,
            shell_exit,
        };
        eprintln!(
            "LOCAL_PROBE mode={mode} forced={forced} shell_exit={shell_exit:?} elapsed={:?}\n{}",
            started.elapsed(),
            report.text()
        );
        report
    }

    struct OutputCapture {
        bytes: [u8; REPORT_CAPACITY],
        len: usize,
    }

    impl OutputCapture {
        fn new() -> Self {
            Self {
                bytes: [0; REPORT_CAPACITY],
                len: 0,
            }
        }
        fn push(&mut self, bytes: &[u8]) {
            assert!(self.len + bytes.len() <= REPORT_CAPACITY);
            self.bytes[self.len..self.len + bytes.len()].copy_from_slice(bytes);
            self.len += bytes.len();
        }
        fn contains(&self, token: &str) -> bool {
            self.bytes[..self.len]
                .windows(token.len())
                .any(|bytes| bytes == token.as_bytes())
        }
        fn text(&self) -> String {
            String::from_utf8_lossy(&self.bytes[..self.len]).into_owned()
        }
    }

    #[test]
    fn unicode_output_fragments_preserve_two_three_and_four_byte_scalars() {
        let input = "GUO_ECHO:λ-终-😀\r\n".as_bytes();
        let mut capture = OutputCapture::new();
        for byte in input {
            capture.push(std::slice::from_ref(byte));
        }
        assert_eq!(&capture.bytes[..capture.len], input);
        assert!(capture.contains("GUO_ECHO:λ-终-😀"));
    }

    async fn output_until(owner: &mut LocalProcessOwner<'_, '_, '_>, token: &str) -> String {
        let mut output = OutputCapture::new();
        loop {
            let event =
                succeeds(tokio::time::timeout(Duration::from_secs(3), owner.next_event()).await);
            match succeeds(event) {
                TransportEvent::Output(bytes) => {
                    output.push(&bytes);
                    if output.contains(token) {
                        return output.text();
                    }
                }
                event => panic!("等待合成输出时提前终结：{event:?}"),
            }
        }
    }

    fn emit(message: &str) {
        println!("{message}");
        succeeds(std::io::stdout().flush());
    }

    fn supervisor_has_original_shell_object() {
        // 应用清理已返回；仅延长本 fixture 的观测句柄寿命至 supervisor 取得同一身份。
        let mut acknowledgement = [0; 1];
        succeeds(std::io::stdin().read_exact(&mut acknowledgement));
        assert_eq!(acknowledgement, *b"1");
    }

    async fn final_tail_and_exit(
        owner: &mut LocalProcessOwner<'_, '_, '_>,
        root: &crate::local_shell::ProbeRoot<'_>,
    ) {
        let mut tail = OutputCapture::new();
        loop {
            match succeeds(owner.next_event().await) {
                TransportEvent::Output(bytes) => {
                    tail.push(&bytes);
                }
                TransportEvent::Exit(status) => {
                    assert_eq!(status.code, Some(23));
                    assert!(!status.success);
                    assert!(tail.contains("GUO_RESIZED 107x37"));
                    assert!(tail.contains("GUO_TAIL"));
                    emit(&tail.text());
                    emit("EXIT_CODE_23");
                    break;
                }
                event => panic!("自然完成出现非正常事件：{event:?}"),
            }
        }
        assert!(owner.next_event().await.is_err());
        let order = [
            ProbeBoundary::ProcessObjectSignaled,
            ProbeBoundary::FinalExitCodeRead,
            ProbeBoundary::WriterDropReturned,
            ProbeBoundary::ChildReaped,
            ProbeBoundary::MasterNonLastDropReturned,
            ProbeBoundary::PtyLastHolderReturned,
            ProbeBoundary::ReaderJoined,
            ProbeBoundary::AllWorkersDone,
            ProbeBoundary::CompletionJoined,
            ProbeBoundary::TransportExitReceived,
        ];
        let mut previous = 0;
        for boundary in order {
            let sequence = root.value().observed(boundary);
            assert!(sequence > previous, "自然完成边界顺序：{boundary:?}");
            previous = sequence;
            emit(&format!(
                "NATURAL_BOUNDARY {boundary:?} sequence={sequence}"
            ));
        }
        assert!(
            root.value().observed(ProbeBoundary::ReaderDropReturned)
                < root.value().observed(ProbeBoundary::ReaderJoined)
        );
        assert_eq!(
            root.value().io.drained.load(Ordering::Acquire),
            0,
            "自然完成不能走丢弃尾输出的 drain-only 分支"
        );
    }

    fn native_output_until(root: &crate::local_shell::ProbeRoot<'_>, token: &str) -> String {
        let mut collected = [0; REPORT_CAPACITY];
        let mut len = 0;
        let mut chunk = [0; super::OUTPUT_LIMIT];
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let count = root.value().take_output(&mut chunk);
            assert!(len + count <= REPORT_CAPACITY);
            collected[len..len + count].copy_from_slice(&chunk[..count]);
            len += count;
            if collected[..len]
                .windows(token.len())
                .any(|bytes| bytes == token.as_bytes())
            {
                return String::from_utf8_lossy(&collected[..len]).into_owned();
            }
            assert!(
                Instant::now() < deadline,
                "等待独立 PTY 合成输出超时：{token}"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn run_native(mode: &str) {
        let ledger = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
        let source = succeeds(ledger.open_source(1, 0));
        let backpressure = mode == "backpressure";
        let before_write = mode == "native_before_write";
        let between = mode == "native_between";
        let root = succeeds(reserve_probe_root(
            &ledger,
            source,
            if backpressure || before_write {
                INPUT_LIMIT
            } else {
                0
            },
        ));
        let observer = succeeds(root.try_clone());
        emit(&format!(
            "NATIVE_LAYOUT H_APP_0={} H_APP_17={} H_APP_8M={} ALIGN={} APP_BODY={} IO_STATE={} OWNER_INLINE={}",
            succeeds(super::app_root_shape(0)).backing.size(),
            succeeds(super::app_root_shape(17)).backing.size(),
            succeeds(super::app_root_shape(INPUT_LIMIT)).backing.size(),
            succeeds(super::app_root_shape(0)).backing.align(),
            std::mem::size_of_val(observer.value()),
            size_of::<crate::local_shell::pty_owner::IoState>(),
            size_of::<PtyOwner<'_, '_>>()
        ));
        std::thread::scope(|scope| {
            if matches!(mode, "native_preowned" | "native_partial") {
                if mode == "native_preowned" {
                    observer.value().cancel_dispatch();
                }
                let rejected = PtyOwner::launch(
                    scope,
                    root,
                    Fixture::EchoGeometryExit,
                    if mode == "native_partial" {
                        WriterInput::CancelBeforeChild
                    } else {
                        WriterInput::TwoStage {
                            first: b"unused",
                            second: b"unused",
                        }
                    },
                    geometry(93, 31),
                    "guo-local-fixture",
                    |_| {},
                );
                assert!(matches!(rejected, Err(super::LocalReject::Cancelled)));
                assert_eq!(observer.value().observed(ProbeBoundary::SpawnReturned), 0);
                if mode == "native_partial" {
                    for boundary in [
                        ProbeBoundary::WriterDropReturned,
                        ProbeBoundary::ReaderDropReturned,
                        ProbeBoundary::PtyLastHolderReturned,
                        ProbeBoundary::WriterJoined,
                        ProbeBoundary::ReaderJoined,
                    ] {
                        assert!(observer.value().observed(boundary) > 0);
                    }
                    assert_eq!(observer.value().observed(ProbeBoundary::ChildReaped), 0);
                    emit("NATIVE_PARTIAL_WORKERS_AND_PTY_RETURN_NO_CHILD");
                }
                emit("NATIVE_PREOWNED_REJECT_NO_CHILD");
                return;
            }
            let mut owner = succeeds(PtyOwner::launch(
                scope,
                root,
                if backpressure || before_write {
                    Fixture::NoReadFlood
                } else {
                    Fixture::EchoGeometryExit
                },
                if backpressure || before_write {
                    WriterInput::Reserved
                } else {
                    WriterInput::TwoStage {
                        first: "hello-λ-终端\r\n".as_bytes(),
                        second: b"size\r\n",
                    }
                },
                geometry(93, 31),
                "guo-local-fixture",
                |_| {},
            ));
            succeeds(owner.probe_visible_code_before_object_signal(23));
            emit("EARLY_CODE_23_OBJECT_PENDING_RESOURCES_RETAINED");
            let pid = succeeds(owner.process_id().ok_or("child PID 缺失"));
            assert!(succeeds(owner.contains_process(pid)));
            let shell = ProcessObservation::open(pid);
            emit(&format!("SHELL_IDENTITY {pid} {}", shell.creation));
            if before_write {
                emit(&native_output_until(&observer, "GUO_NO_READ"));
                assert_eq!(observer.value().io.entered.load(Ordering::Acquire), 0);
                succeeds(ledger.advance_epoch(1));
            } else if backpressure {
                emit(&native_output_until(&observer, "GUO_NO_READ"));
                let deadline = Instant::now() + Duration::from_secs(3);
                while !observer.value().output_is_full()
                    || !observer.value().io.waiting_ui.load(Ordering::Acquire)
                {
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(1));
                }
                emit("UI_QUEUE_FULL NO_READ_READY");
                observer
                    .value()
                    .io
                    .start_writer
                    .store(true, Ordering::Release);
                let deadline = Instant::now() + Duration::from_secs(3);
                loop {
                    let io = &observer.value().io;
                    let entered = io.entered.load(Ordering::Acquire);
                    let returned = io.returned.load(Ordering::Acquire);
                    if entered > returned && io.written.load(Ordering::Acquire) > 0 {
                        std::thread::sleep(Duration::from_millis(300));
                        if io.entered.load(Ordering::Acquire) > io.returned.load(Ordering::Acquire)
                        {
                            break;
                        }
                    }
                    assert!(Instant::now() < deadline, "未形成部分写后的系统调用等待");
                    std::thread::sleep(Duration::from_millis(1));
                }
                emit("TRANSPORT_WRITE_BEGIN bytes=8388608");
                emit("TRANSPORT_WRITE_STILL_PENDING");
                let io = &observer.value().io;
                emit(&format!(
                    "SYSTEM_WRITE_PROGRESS committed={} entered={} returned={}",
                    io.written.load(Ordering::Acquire),
                    io.entered.load(Ordering::Acquire),
                    io.returned.load(Ordering::Acquire)
                ));

                // 同一 fixture 进程、同一 ledger 的第二个独立 PTY；不借隔离进程作响应证明。
                let peer_source = succeeds(ledger.open_source(1, 1));
                let peer_root = succeeds(reserve_probe_root(&ledger, peer_source, 0));
                let peer_observer = succeeds(peer_root.try_clone());
                let peer_runtime = succeeds(
                    tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build(),
                );
                peer_runtime.block_on(async {
                    let mut peer = succeeds(
                        LocalProcessOwner::launch_probe(
                            scope,
                            peer_root,
                            Fixture::EchoGeometryExit,
                            geometry(93, 31),
                            "guo-local-fixture",
                            |_| {},
                        )
                        .await,
                    );
                    let peer_pid = succeeds(peer.process_id().ok_or("第二套 child PID 缺失"));
                    assert!(succeeds(peer.contains_process(peer_pid)));
                    let peer_process = ProcessObservation::open(peer_pid);
                    emit(
                        &output_until(&mut peer, "GUO_READY TERM=guo-local-fixture SIZE=93x31")
                            .await,
                    );
                    succeeds(peer.write("hello-λ-终端\r\n".as_bytes()).await);
                    emit(&output_until(&mut peer, "GUO_ECHO:hello-λ-终端").await);
                    succeeds(peer.resize(geometry(107, 37)).await);
                    succeeds(peer.write(b"size\r\n").await);
                    final_tail_and_exit(&mut peer, &peer_observer).await;
                    assert_eq!(peer_process.wait_exit(), 23);
                    drop(peer_process);
                    drop(peer);
                });
                drop(peer_observer);
                emit("SAME_PROCESS_PEER_RESPONSIVE");
                assert!(
                    observer.value().io.entered.load(Ordering::Acquire)
                        > observer.value().io.returned.load(Ordering::Acquire)
                );
                succeeds(ledger.advance_epoch(1));
            } else {
                emit(&native_output_until(
                    &observer,
                    "GUO_READY TERM=guo-local-fixture SIZE=93x31",
                ));
                observer
                    .value()
                    .io
                    .start_writer
                    .store(true, Ordering::Release);
                emit(&native_output_until(&observer, "GUO_ECHO:hello-λ-终端"));
                if between {
                    let deadline = Instant::now() + Duration::from_secs(3);
                    while !observer.value().io.first_done.load(Ordering::Acquire) {
                        assert!(Instant::now() < deadline);
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    succeeds(ledger.advance_epoch(1));
                } else {
                    succeeds(owner.resize(geometry(107, 37)));
                    observer
                        .value()
                        .io
                        .resume_second
                        .store(true, Ordering::Release);
                    emit(&native_output_until(&observer, "GUO_TAIL"));
                    assert_eq!(succeeds(owner.wait_natural_exit()), 23);
                }
            }
            succeeds(owner.request_cancel());
            let ack_calls = observer.value().io.entered.load(Ordering::Acquire);
            let ack_sequence = observer.value().observed(ProbeBoundary::CancelConfirmed);
            observer
                .value()
                .io
                .start_writer
                .store(true, Ordering::Release);
            observer
                .value()
                .io
                .resume_second
                .store(true, Ordering::Release);
            assert_eq!(succeeds(owner.request_cancel()), ());
            assert_eq!(
                observer.value().observed(ProbeBoundary::CancelConfirmed),
                ack_sequence
            );
            let (outcome, exit) = succeeds(owner.shutdown());
            let last_holder = observer
                .value()
                .observed(ProbeBoundary::PtyLastHolderReturned);
            assert_eq!(succeeds(owner.shutdown()), (outcome, exit));
            assert_eq!(
                observer
                    .value()
                    .observed(ProbeBoundary::PtyLastHolderReturned),
                last_holder
            );
            assert_eq!(
                observer.value().io.entered.load(Ordering::Acquire),
                ack_calls
            );
            if backpressure {
                assert!(
                    matches!(outcome, WriteOutcome::Closing { committed, .. } if committed > 0 && committed < INPUT_LIMIT)
                );
                assert!(
                    observer.value().io.cancel_marked.load(Ordering::Acquire) > 0,
                    "没有实际被标记的系统取消"
                );
                assert_eq!(
                    observer.value().io.last_write_error.load(Ordering::Acquire),
                    995
                );
                assert!(observer.value().io.drained.load(Ordering::Acquire) > 0);
            } else if before_write {
                assert_eq!(
                    outcome,
                    WriteOutcome::Closing {
                        committed: 0,
                        error: None
                    }
                );
                assert_eq!(observer.value().io.entered.load(Ordering::Acquire), 0);
            } else if between {
                assert_eq!(
                    outcome,
                    WriteOutcome::Closing {
                        committed: "hello-λ-终端\r\n".len(),
                        error: None
                    }
                );
                assert_eq!(observer.value().io.entered.load(Ordering::Acquire), 1);
                assert_eq!(observer.value().io.cancel_marked.load(Ordering::Acquire), 0);
            } else {
                assert!(matches!(outcome, WriteOutcome::Complete(_)));
                assert_eq!(exit, 23);
            }
            assert_eq!(shell.wait_exit(), exit);
            supervisor_has_original_shell_object();
            drop(shell);
            let io = &observer.value().io;
            assert_eq!(
                io.entered.load(Ordering::Acquire),
                io.returned.load(Ordering::Acquire)
            );
            assert!(io.batch_done.load(Ordering::Acquire));
            let call_return = observer
                .value()
                .observed(ProbeBoundary::SystemWriteReturned);
            let ack = observer.value().observed(ProbeBoundary::CancelConfirmed);
            let writer_drop = observer.value().observed(ProbeBoundary::WriterDropReturned);
            let writer_join = observer.value().observed(ProbeBoundary::WriterJoined);
            let reader_drop = observer.value().observed(ProbeBoundary::ReaderDropReturned);
            let reader_join = observer.value().observed(ProbeBoundary::ReaderJoined);
            let last_pty = observer
                .value()
                .observed(ProbeBoundary::PtyLastHolderReturned);
            assert!(call_return < ack && ack < writer_drop && writer_drop < writer_join);
            assert!(reader_drop < reader_join && last_pty < reader_join);
            assert!(observer.value().observed(ProbeBoundary::ReaderDrainStarted) > 0);
            let calls_after_ack = io.entered.load(Ordering::Acquire);
            io.start_writer.store(true, Ordering::Release);
            io.resume_second.store(true, Ordering::Release);
            assert_eq!(io.entered.load(Ordering::Acquire), calls_after_ack);
            emit(&format!(
                "NATIVE_CANCEL calls={} marked={} not_found={} error={} drained={} outcome={outcome:?}",
                io.cancel_calls.load(Ordering::Acquire),
                io.cancel_marked.load(Ordering::Acquire),
                io.cancel_not_found.load(Ordering::Acquire),
                io.last_write_error.load(Ordering::Acquire),
                io.drained.load(Ordering::Acquire)
            ));
            for (boundary, token) in [
                (ProbeBoundary::CancelConfirmed, "SYSTEM_CANCEL_ACK"),
                (ProbeBoundary::SystemWriteReturned, "SYSTEM_WRITE_RETURN"),
                (ProbeBoundary::WriterDropReturned, "WRITER_DROP_RETURN"),
                (
                    ProbeBoundary::PtyLastHolderReturned,
                    "PTY_LAST_HOLDER_RETURN",
                ),
                (ProbeBoundary::ChildReaped, "CHILD_REAP"),
                (ProbeBoundary::WriterJoined, "WRITER_JOIN_RETURN"),
                (ProbeBoundary::ReaderJoined, "READER_JOIN_RETURN"),
                (ProbeBoundary::AllWorkersDone, "ALL_WORKERS_DONE"),
            ] {
                if before_write && boundary == ProbeBoundary::SystemWriteReturned {
                    emit("NO_SYSTEM_WRITE_ADMITTED");
                    continue;
                }
                assert!(observer.value().observed(boundary) > 0);
                emit(&format!(
                    "BOUNDARY {boundary:?} sequence={}",
                    observer.value().observed(boundary)
                ));
                emit(token);
            }
            drop(owner);
        });
        // Scope 的实际 join/closure packet 释放已完成；最后 app backing 仍由 observer 持有。
        emit("LATE_CALLBACKS_DONE");
        assert!(ledger.snapshot().rust_bytes > 0);
        let audit = crate::audit::AuditScope::start(&ledger);
        drop(observer);
        let report = audit.finish();
        assert_eq!(report.events().len(), 1);
        assert_eq!(report.events()[0].kind, 2);
        assert!(report.events()[0].held_rust > 0);
        assert_eq!(ledger.snapshot().rust_bytes, 0);
        emit("PROCESS_ROOT_REFUND");
    }

    fn run_fault_target() {
        let ledger = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
        let source = succeeds(ledger.open_source(1, 0));
        let shape = succeeds(super::app_root_shape(0));
        let root = succeeds(reserve_probe_root(&ledger, source, 0));
        root.value().io.inject(ProbeFault::ThreadTarget);
        std::thread::scope(|scope| {
            let audit_slot = std::cell::RefCell::new(None);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                PtyOwner::launch(
                    scope,
                    root,
                    Fixture::NoReadFlood,
                    WriterInput::Reserved,
                    geometry(93, 31),
                    "guo-local-fixture",
                    |owner| {
                        owner.audit_unique_root(&ledger);
                        audit_slot.replace(Some(crate::audit::AuditScope::start(&ledger)));
                    },
                )
            }));
            assert!(
                matches!(result, Ok(Err(super::LocalReject::Transport))),
                "原操作错误必须保留，不能被已完成清理的 Drop panic 遮盖"
            );
            let report = succeeds(audit_slot.into_inner().ok_or("端点已准备审计")).finish();
            assert!(!report.overflow);
            let deallocation = report
                .events()
                .iter()
                .find(|event| event.kind == 2 && event.size == shape.backing.size());
            assert!(deallocation.is_some_and(|event| event.held_rust == shape.backing.size()));
            assert_eq!(ledger.snapshot().rust_bytes, 0);
        });
        emit("TARGET_SETUP_PRIMARY_ERROR_CLEANUP_COMPLETE_UNIQUE_ROOT_REFUND");
    }

    fn run_fault_recovery(mode: &str) {
        let ledger = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
        let source = succeeds(ledger.open_source(1, 0));
        let shape = succeeds(super::app_root_shape(0));
        let root = succeeds(reserve_probe_root(&ledger, source, 0));
        std::thread::scope(|scope| {
            let mut owner = succeeds(PtyOwner::launch(
                scope,
                root,
                Fixture::NoReadFlood,
                WriterInput::TwoStage {
                    first: b"",
                    second: b"",
                },
                geometry(93, 31),
                "guo-local-fixture",
                |_| {},
            ));
            owner.audit_unique_root(&ledger);
            let pid = succeeds(owner.process_id().ok_or("故障 child PID"));
            assert!(succeeds(owner.contains_process(pid)));
            let shell = ProcessObservation::open(pid);
            emit(&format!("SHELL_IDENTITY {pid} {}", shell.creation));
            let audit = crate::audit::AuditScope::start(&ledger);
            if mode == "fault_unwind" {
                owner.root.value().io.inject(ProbeFault::ChildWait);
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                    let _owner = owner;
                    std::panic::panic_any(173_u32);
                }));
                assert!(
                    matches!(result, Err(payload) if payload.downcast_ref::<u32>() == Some(&173))
                );
                assert_eq!(shell.wait_exit(), 1);
                supervisor_has_original_shell_object();
                drop(shell);
                let report = audit.finish();
                assert!(!report.overflow);
                assert!(report.events().iter().any(|event| event.kind == 2
                    && event.size == shape.backing.size()
                    && event.held_rust == shape.backing.size()));
                assert_eq!(ledger.snapshot().rust_bytes, 0);
                return;
            }
            if mode == "fault_cancel" {
                owner.root.value().io.inject(ProbeFault::CancelCall);
                assert_eq!(owner.request_cancel(), Err(super::LocalReject::Transport));
                assert!(owner.root.value().observed(ProbeBoundary::CancelConfirmed) > 0);
            } else {
                owner.root.value().io.inject(ProbeFault::ChildWait);
            }
            assert_eq!(owner.shutdown(), Err(super::LocalReject::Transport));
            let joined = owner.root.value().observed(ProbeBoundary::AllWorkersDone);
            assert!(joined > 0);
            assert!(owner.root.value().observed(ProbeBoundary::JobDropReturned) > 0);
            assert_eq!(owner.shutdown(), Err(super::LocalReject::Transport));
            assert_eq!(
                owner.root.value().observed(ProbeBoundary::AllWorkersDone),
                joined
            );
            assert_eq!(shell.wait_exit(), 1);
            supervisor_has_original_shell_object();
            drop(shell);
            assert_eq!(ledger.snapshot().rust_bytes, shape.backing.size());
            drop(owner);
            let report = audit.finish();
            assert!(report.events().iter().any(|event| event.kind == 2
                && event.size == shape.backing.size()
                && event.held_rust == shape.backing.size()));
            assert_eq!(ledger.snapshot().rust_bytes, 0);
        });
        emit("RECOVERED_PRIMARY_ERROR_ALL_RESOURCES_RETURNED_UNIQUE_ROOT_REFUND");
    }

    fn run_unique_completion() {
        let ledger = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
        let source = succeeds(ledger.open_source(1, 0));
        let shape = succeeds(super::app_root_shape(0));
        let root = succeeds(reserve_probe_root(&ledger, source, 0));
        let runtime = succeeds(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build(),
        );
        std::thread::scope(|scope| {
            runtime.block_on(async {
                let mut owner = succeeds(
                    LocalProcessOwner::launch_probe(
                        scope,
                        root,
                        Fixture::EchoGeometryExit,
                        geometry(93, 31),
                        "guo-local-fixture",
                        |_| {},
                    )
                    .await,
                );
                let pid = succeeds(owner.process_id().ok_or("唯一 completion child PID"));
                assert!(succeeds(owner.contains_process(pid)));
                let shell = ProcessObservation::open(pid);
                emit(&format!("SHELL_IDENTITY {pid} {}", shell.creation));
                let audit = crate::audit::AuditScope::start(&ledger);
                emit(
                    &output_until(&mut owner, "GUO_READY TERM=guo-local-fixture SIZE=93x31").await,
                );
                succeeds(owner.write("hello-λ-终端\r\n".as_bytes()).await);
                emit(&output_until(&mut owner, "GUO_ECHO:hello-λ-终端").await);
                succeeds(owner.resize(geometry(107, 37)).await);
                succeeds(owner.write(b"size\r\n").await);
                let mut tail = OutputCapture::new();
                loop {
                    match succeeds(owner.next_event().await) {
                        TransportEvent::Output(bytes) => tail.push(&bytes),
                        TransportEvent::Exit(status) => {
                            assert_eq!(status.code, Some(23));
                            break;
                        }
                        event => panic!("唯一 completion 异常事件：{event:?}"),
                    }
                }
                assert!(tail.contains("GUO_TAIL"));
                assert!(
                    owner
                        .probe_root()
                        .value()
                        .observed(ProbeBoundary::CompletionJoined)
                        > 0
                );
                assert!(
                    owner
                        .probe_root()
                        .value()
                        .observed(ProbeBoundary::JobDropReturned)
                        > 0
                );
                assert_eq!(ledger.snapshot().rust_bytes, shape.backing.size());
                assert_eq!(shell.wait_exit(), 23);
                supervisor_has_original_shell_object();
                drop(shell);
                drop(owner);
                let report = audit.finish();
                assert!(!report.overflow);
                assert!(report.events().iter().any(|event| event.kind == 2
                    && event.size == shape.backing.size()
                    && event.held_rust == shape.backing.size()));
                assert_eq!(ledger.snapshot().rust_bytes, 0);
            })
        });
        emit("COMPLETION_JOINED_UNIQUE_ROOT_FINAL_REFUND");
    }

    #[test]
    fn local_shell_completion_keeps_own_root_without_external_observer_clone() {
        let report = supervise("unique_completion", None);
        assert!(!report.forced && report.success);
        assert_eq!(report.shell_exit, Some(23));
        assert!(
            report
                .text()
                .contains("COMPLETION_JOINED_UNIQUE_ROOT_FINAL_REFUND")
        );
    }

    #[test]
    fn local_shell_cancel_and_child_wait_faults_keep_unique_root_until_real_cleanup() {
        for mode in ["fault_cancel", "fault_child_wait", "fault_unwind"] {
            let report = supervise(mode, None);
            assert!(!report.forced && report.success);
            assert_eq!(report.shell_exit, Some(1));
            assert!(
                report
                    .text()
                    .contains("RECOVERED_PRIMARY_ERROR_ALL_RESOURCES_RETURNED_UNIQUE_ROOT_REFUND")
            );
        }
    }

    #[test]
    fn local_shell_thread_target_failure_preserves_primary_error_and_reclaims_unique_root() {
        let report = supervise("fault_target", None);
        assert!(!report.forced && report.success);
        assert!(
            report
                .text()
                .contains("TARGET_SETUP_PRIMARY_ERROR_CLEANUP_COMPLETE_UNIQUE_ROOT_REFUND")
        );
    }

    #[test]
    fn local_shell_run_owned_fixture() {
        if std::env::var_os(FIXTURE_MARKER).is_none() {
            return;
        }
        let mode = succeeds(std::env::var("GUO_LOCAL_PROBE_MODE"));
        if mode == "unique_completion" {
            run_unique_completion();
            return;
        }
        if mode == "fault_target" {
            run_fault_target();
            return;
        }
        if matches!(
            mode.as_str(),
            "fault_cancel" | "fault_child_wait" | "fault_unwind"
        ) {
            run_fault_recovery(&mode);
            return;
        }
        if mode == "backpressure" || mode.starts_with("native_") {
            run_native(&mode);
            return;
        }
        eprintln!(
            "WINDOWS_GNU_ABI FILETIME={}:{} OwnedHandle={}:{} TerminalSize={}:{} PtyOwner={}:{}",
            size_of::<FILETIME>(),
            align_of::<FILETIME>(),
            size_of::<OwnedHandle>(),
            align_of::<OwnedHandle>(),
            size_of::<TerminalSize>(),
            align_of::<TerminalSize>(),
            size_of::<PtyOwner<'_, '_>>(),
            align_of::<PtyOwner<'_, '_>>()
        );
        let ledger = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
        let source = succeeds(ledger.open_source(1, 0));
        let root = succeeds(reserve_probe_root(&ledger, source, 0));
        if mode == "unicode_fragments" {
            root.value().io.read_limit.store(1, Ordering::Release);
        }
        let observation = succeeds(root.try_clone());
        let runtime = succeeds(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build(),
        );
        std::thread::scope(|scope| {
            runtime.block_on(async {
                let fixture = if mode == "drain" {
                    Fixture::NoReadFlood
                } else {
                    Fixture::EchoGeometryExit
                };
                let shell_observation = std::cell::RefCell::new(None);
                let callback_holder = succeeds(observation.try_clone());
                let launch = LocalProcessOwner::launch_probe(
                    scope,
                    root,
                    fixture,
                    geometry(93, 31),
                    "guo-local-fixture",
                    |spawned| {
                        let pid = match spawned.process_id() {
                            Some(pid) => pid,
                            None => panic!("PTY 没有 child PID"),
                        };
                        assert!(succeeds(spawned.contains_process(pid)));
                        let shell = ProcessObservation::open(pid);
                        emit(&format!("SHELL_IDENTITY {pid} {}", shell.creation));
                        shell_observation.replace(Some(shell));
                        if mode == "late" {
                            callback_holder.value().cancel_dispatch();
                            succeeds(ledger.advance_epoch(1));
                        }
                        drop(callback_holder);
                    },
                )
                .await;
                let shell = match shell_observation.into_inner() {
                    Some(shell) => shell,
                    None => panic!("真实 child 观测缺失"),
                };
                if mode == "late" {
                    assert_eq!(launch.err(), Some(super::LocalReject::Cancelled));
                    assert!(observation.value().observed(ProbeBoundary::SpawnReturned) > 0);
                    assert!(
                        observation
                            .value()
                            .observed(ProbeBoundary::LateHolderReleased)
                            > 0
                    );
                    emit(&format!("SHELL_HANDLE_WAIT_RETURN {}", shell.wait_exit()));
                    supervisor_has_original_shell_object();
                    drop(shell);
                    emit("RETURNED_SPAWN_CANCELLED CALLBACK_HOLDER_RELEASED");
                    return;
                }
                let mut owner = succeeds(launch);
                emit("SPAWN_RETURN JOB_CONTAINED");
                eprintln!(
                    "完整 process-root 前件：{:?}",
                    LocalProcessOwner::CLOSURE_GAPS
                );
                if mode == "drain" {
                    emit(&output_until(&mut owner, "GUO_NO_READ").await);
                    let deadline = Instant::now() + Duration::from_secs(3);
                    while !observation.value().output_is_full()
                        || !observation.value().io.waiting_ui.load(Ordering::Acquire)
                    {
                        assert!(Instant::now() < deadline, "未形成真实 reader/UI 满槽");
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                    assert!(observation.value().publish(b"full").is_err());
                    emit("UI_QUEUE_FULL NO_READ_READY");
                    observation.value().cancel_dispatch();
                    succeeds(ledger.advance_epoch(1));
                    succeeds(owner.shutdown().await);
                    emit("TRANSPORT_SHUTDOWN_RETURN FULL_UI_QUEUE");
                } else {
                    let ready =
                        output_until(&mut owner, "GUO_READY TERM=guo-local-fixture SIZE=93x31")
                            .await;
                    emit(&ready);
                    let input = if mode == "unicode_fragments" {
                        "hello-λ-终端😀\r\n"
                    } else {
                        "hello-λ-终端\r\n"
                    };
                    let expected = if mode == "unicode_fragments" {
                        "GUO_ECHO:hello-λ-终端😀"
                    } else {
                        "GUO_ECHO:hello-λ-终端"
                    };
                    succeeds(owner.write(input.as_bytes()).await);
                    emit(&output_until(&mut owner, expected).await);
                    if mode == "between" {
                        observation.value().cancel_dispatch();
                        assert_eq!(
                            owner.write(b"must-not-enter\r\n").await,
                            Err(super::LocalReject::Cancelled)
                        );
                        emit("BETWEEN_WRITES_REJECTED");
                        succeeds(owner.shutdown().await);
                    } else {
                        succeeds(owner.resize(geometry(107, 37)).await);
                        succeeds(owner.write(b"size\r\n").await);
                        final_tail_and_exit(&mut owner, &observation).await;
                        succeeds(owner.shutdown().await);
                    }
                }
                let exit = shell.wait_exit();
                emit(&format!("SHELL_HANDLE_WAIT_RETURN {exit}"));
                supervisor_has_original_shell_object();
                drop(shell);
                drop(owner);
                observation.value().observe(ProbeBoundary::OwnerFinished);
                emit("OWNER_DROP_RETURN");
            })
        });
        drop(runtime);
        assert!(
            observation
                .value()
                .observed(ProbeBoundary::TransportDropReturned)
                > 0
        );
        assert!(ledger.snapshot().rust_bytes > 0);
        let audit = crate::audit::AuditScope::start(&ledger);
        drop(observation);
        let deallocation = audit.finish();
        assert_eq!(deallocation.events().len(), 1);
        assert_eq!(deallocation.events()[0].kind, 2);
        assert!(deallocation.events()[0].held_rust > 0);
        assert_eq!(ledger.snapshot().rust_bytes, 0);
        emit("APP_ROOT_FINAL_REFUND");
    }

    #[test]
    fn local_shell_windows_real_pty_read_write_geometry_term_exit() {
        let report = supervise("basic", None);
        assert!(!report.forced, "基础 PTY 探针必须自己完成");
        assert!(report.success, "基础 PTY 探针失败");
        assert_eq!(report.shell_exit, Some(23));
        for token in [
            "SIZE=93x31",
            "GUO_RESIZED 107x37",
            "GUO_ECHO:hello-λ-终端",
            "EXIT_CODE_23",
            "OWNER_DROP_RETURN",
            "APP_ROOT_FINAL_REFUND",
        ] {
            assert!(
                report.text().contains(token),
                "缺少实际 fixture 证据：{token}"
            );
        }
    }

    #[test]
    fn local_shell_one_byte_pty_reads_preserve_unicode_and_natural_exit() {
        let report = supervise("unicode_fragments", None);
        assert!(!report.forced && report.success);
        assert_eq!(report.shell_exit, Some(23));
        assert!(report.text().contains("GUO_ECHO:hello-λ-终端😀"));
        assert!(report.text().contains("GUO_RESIZED 107x37"));
    }

    #[test]
    fn local_shell_windows_full_ui_queue_drain_between_write_and_returned_spawn_stop() {
        for mode in ["drain", "between", "late"] {
            let report = supervise(mode, None);
            assert!(!report.forced);
            assert!(report.success);
            assert_eq!(report.shell_exit, Some(1));
            assert!(report.text().contains("APP_ROOT_FINAL_REFUND"));
        }
    }

    #[test]
    fn local_shell_a1_full_backpressure_cancel_requires_actual_convergence() {
        let report = supervise("backpressure", None);
        assert!(
            !report.forced,
            "A1 未闭合：独立监督强制回收，不能算产品取消成功"
        );
        assert!(report.success);
        assert!(report.text().contains("SAME_PROCESS_PEER_RESPONSIVE"));
        for token in [
            "SYSTEM_CANCEL_ACK",
            "SYSTEM_WRITE_RETURN",
            "WRITER_DROP_RETURN",
            "PTY_LAST_HOLDER_RETURN",
            "CHILD_REAP",
            "ALL_WORKERS_DONE",
            "LATE_CALLBACKS_DONE",
            "PROCESS_ROOT_REFUND",
        ] {
            assert!(
                report.text().lines().any(|line| line == token),
                "A1 边界未被独立观测：{token}"
            );
        }
    }

    #[test]
    fn local_shell_native_ownership_keeps_real_tty_bytes_and_geometry() {
        let report = supervise("native_basic", None);
        assert!(!report.forced && report.success);
        assert_eq!(report.shell_exit, Some(23));
        assert!(report.text().contains("GUO_RESIZED 107x37"));
        assert!(report.text().contains("GUO_ECHO:hello-λ-终端"));
    }

    #[test]
    fn local_shell_native_cancel_at_admission_and_between_system_writes() {
        for mode in [
            "native_preowned",
            "native_partial",
            "native_before_write",
            "native_between",
        ] {
            let report = supervise(mode, None);
            assert!(
                !report.forced && report.success,
                "{mode} 必须由原 owner 自行收口"
            );
            assert!(report.text().contains("PROCESS_ROOT_REFUND"));
        }
    }
}
