//! 原生固定 helper pool 独立 harness；测试 fixture 不进入生产定位或环境契约。

#[path = "selected_p1_control_runtime.rs"]
mod control_harness;
pub use control_harness::{
    admission, audit, frame, inbox, layout, ledger, owner, persistent, process, secret, selected_p1,
};
#[path = "../src/selected_p1/helper_runtime/unix.rs"]
pub mod unix;
#[cfg(windows)]
#[path = "../src/selected_p1/helper_runtime/windows.rs"]
pub mod windows;
#[path = "../src/selected_p1/helper_runtime/worker.rs"]
pub mod worker;

#[test]
fn helper_actual_persistent_layout_is_final_type_not_note_constants() {
    #[cfg(windows)]
    {
        let control =
            persistent::inbox_layout().unwrap_or_else(|error| panic!("control layout: {error:?}"));
        let pool =
            worker::persistent_layout().unwrap_or_else(|error| panic!("pool layout: {error:?}"));
        let classes =
            worker::cost_classes().unwrap_or_else(|error| panic!("final classes: {error:?}"));
        println!(
            "control={}/{} pool={}/{} worker_tls={} workers={}",
            control.backing.size(),
            control.backing.align(),
            pool.backing.size(),
            pool.backing.align(),
            worker::worker_tls_upper().unwrap_or_else(|error| panic!("TLS layout: {error:?}")),
            worker::WORKERS
        );
        assert_eq!(worker::PROCESS_OWNERS, 4);
        assert_eq!(worker::STDERR_DRAINERS, 4);
        assert_eq!(classes.backing, pool);
        assert_eq!(
            classes.worker_tls_rust,
            worker::worker_tls_upper().unwrap_or_else(|error| panic!("TLS: {error:?}"))
        );
        println!(
            "slots_inline={}/{} boots_inline={}/{} events_inline={}/{} os_stack={}",
            classes.process_slots_inline.size(),
            classes.process_slots_inline.align(),
            classes.boots_inline.size(),
            classes.boots_inline.align(),
            classes.events_inline.size(),
            classes.events_inline.align(),
            classes.os_stack_reservation
        );
    }
}

#[cfg(windows)]
mod actual_child {
    use crate::{
        audit,
        ledger::{Admission, Amount, BYTE_LIMIT, CLASS_COUNT, ProcessLedger},
        process::{
            CertifiedHelper, Failure, HelperBuildRecord, LaunchIdentity, LaunchShape, Request,
        },
        windows::TrustedLaunch,
        worker::{Completion, HelperPool, Ticket},
    };
    use std::ffi::OsStr;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    static ROOT: ProcessLedger = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
    const MAGIC: &[u8; 8] = b"FIXTURE2";

    // 此权限只针对下方独立生成的测试 exe，不签发正常 helper 的生产构建证书。
    struct FixtureRecord<'a> {
        executable: &'a OsStr,
        arguments: &'a [&'a OsStr],
        hash: [u8; 32],
    }
    // SAFETY: 独占 synthetic exe 的测试授权，只验证父持额及 hash 绑定，不授予生产 H 证明。
    unsafe impl HelperBuildRecord for FixtureRecord<'_> {
        fn request_magic(&self) -> &[u8; 8] {
            MAGIC
        }
        fn authorize(
            &self,
            identity: LaunchIdentity<'_>,
            shape: &LaunchShape,
            request: &Request,
        ) -> Result<CertifiedHelper, Failure> {
            if identity.executable != self.executable
                || identity.arguments != self.arguments
                || request.bytes != request_bytes()
                || shape.arguments != 1
                || !shape.inherits_cwd
                || !shape.inherits_environment
            {
                return Err(Failure::Unsupported);
            }
            Ok(CertifiedHelper {
                amount: Amount {
                    rust_bytes: request.helper_rust,
                    c_bytes: request.c_max,
                    slots: 1,
                },
                artifact_sha256: self.hash,
            })
        }
    }

    struct Fixture {
        directory: PathBuf,
        source: PathBuf,
        executable: PathBuf,
        hash: [u8; 32],
    }
    impl Fixture {
        fn new() -> Self {
            let parent = std::env::temp_dir().join("opencode");
            assert!(parent.is_dir());
            let directory = parent.join(format!("guosh-w1c-fixture-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&directory)
                .unwrap_or_else(|error| panic!("创建任务目录: {error:?}"));
            let mut fixture = Self {
                source: directory.join("child.rs"),
                executable: directory.join("child.exe"),
                directory,
                hash: [0; 32],
            };
            assert_eq!(fixture.source.parent(), Some(fixture.directory.as_path()));
            assert_eq!(
                fixture.executable.parent(),
                Some(fixture.directory.as_path())
            );
            std::fs::write(&fixture.source, CHILD)
                .unwrap_or_else(|error| panic!("写 fixture: {error:?}"));
            let compiler = std::process::Command::new("rustc")
                .args([
                    "+1.98.1-x86_64-pc-windows-gnu",
                    "--edition=2024",
                    "--target",
                    "x86_64-pc-windows-gnu",
                ])
                .arg(&fixture.source)
                .arg("-o")
                .arg(&fixture.executable)
                .env("RUSTUP_AUTO_INSTALL", "0")
                .env_remove("GUOSH_GUI_FIXTURE")
                .status()
                .unwrap_or_else(|error| panic!("compiler: {error:?}"));
            assert!(compiler.success());
            use sha2::{Digest, Sha256};
            let image = std::fs::read(&fixture.executable)
                .unwrap_or_else(|error| panic!("读取独占 fixture 产物: {error:?}"));
            fixture.hash = Sha256::digest(&image).into();
            fixture
        }
        fn record<'a>(&'a self, arguments: &'a [&'a OsStr]) -> FixtureRecord<'a> {
            FixtureRecord {
                executable: self.executable.as_os_str(),
                arguments,
                hash: self.hash,
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            for file in [&self.source, &self.executable] {
                assert_eq!(file.parent(), Some(self.directory.as_path()));
                if file.exists() {
                    std::fs::remove_file(file)
                        .unwrap_or_else(|error| panic!("清理 created file: {error:?}"));
                }
            }
            std::fs::remove_dir(&self.directory)
                .unwrap_or_else(|error| panic!("清理本任务空目录: {error:?}"));
        }
    }

    #[test]
    fn helper_submission_classifies_stale_admission_and_releases_failed_reservations() {
        static CLASSIFICATION_ROOT: ProcessLedger =
            ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
        let root = &CLASSIFICATION_ROOT;
        let fixture = Fixture::new();
        let args = [OsStr::new("good")];
        let launch = unsafe { TrustedLaunch::borrowed(fixture.executable.as_os_str(), &args) }
            .unwrap_or_else(|e| panic!("launch: {e:?}"));
        let record = fixture.record(&args);
        let request =
            Request::parse(request_bytes(), MAGIC).unwrap_or_else(|e| panic!("request: {e:?}"));
        let plan = crate::windows::SpawnCostPlan::bound(&launch, &request, &record)
            .unwrap_or_else(|e| panic!("plan: {e:?}"));
        let mut pool = HelperPool::new(root, 1).unwrap_or_else(|e| panic!("pool: {e:?}"));
        let old_epoch = Admission {
            epoch: 1,
            source: None,
        };
        let epoch_reserved = plan
            .costs()
            .reserve(root, old_epoch)
            .unwrap_or_else(|e| panic!("epoch grants: {e:?}"));
        assert_eq!(root.advance_epoch(1), Ok(2));
        let source = root
            .open_source(2, 0)
            .unwrap_or_else(|e| panic!("source: {e:?}"));
        let old_source = Admission {
            epoch: 2,
            source: Some(source),
        };
        let source_reserved = plan
            .costs()
            .reserve(root, old_source)
            .unwrap_or_else(|e| panic!("source grants: {e:?}"));
        root.invalidate_source(source)
            .unwrap_or_else(|e| panic!("invalidate: {e:?}"));
        let current = Admission {
            epoch: 2,
            source: None,
        };
        // 真实充分额度，不以 snapshot 推断拒绝理由；返回值来自同次 reserve 的 NumericReject。
        let check = plan
            .costs()
            .reserve(root, current)
            .unwrap_or_else(|e| panic!("充分额度: {e:?}"));
        drop(check);
        let before = root.snapshot();
        let audit = audit::AuditScope::start(root);
        let epoch_result = pool
            .submit(&launch, request_bytes(), &record, old_epoch, 5000)
            .err();
        let source_result = pool
            .submit(&launch, request_bytes(), &record, old_source, 5000)
            .err();
        let report = audit.finish();
        let no_pending = pool.snapshot();
        let unchanged = root.snapshot() == before;
        let audit = audit::AuditScope::start(root);
        let reserved_results = [(old_epoch, epoch_reserved), (old_source, source_reserved)].map(
            |(admission, reservations)| {
                pool.submit_reserved(
                    crate::worker::Submission {
                        launch: &launch,
                        bytes: request_bytes(),
                        record: &record,
                        admission,
                        timeout_ms: 5000,
                    },
                    reservations,
                )
                .err()
            },
        );
        let reserved_report = audit.finish();
        let persistent = root.snapshot();
        let occupied = root
            .try_reserve(
                Amount {
                    rust_bytes: BYTE_LIMIT - persistent.rust_bytes - persistent.c_bytes,
                    c_bytes: 0,
                    slots: 0,
                },
                crate::ledger::ResourceClass::Runtime,
                current,
            )
            .unwrap_or_else(|e| panic!("占满额度: {e:?}"));
        let full = root.snapshot();
        let audit = audit::AuditScope::start(root);
        let budget_result = pool
            .submit(&launch, request_bytes(), &record, current, 5000)
            .err();
        let budget_report = audit.finish();
        assert_eq!(root.snapshot(), full);
        drop(occupied);
        let wrong_record = FixtureRecord {
            executable: fixture.source.as_os_str(),
            ..fixture.record(&args)
        };
        let audit = audit::AuditScope::start(root);
        let unsupported_result = pool
            .submit(&launch, request_bytes(), &wrong_record, current, 5000)
            .err();
        let overflow_result = pool
            .submit(&launch, request_bytes(), &record, current, u64::MAX)
            .err();
        let unsupported_report = audit.finish();
        let audit = audit::AuditScope::start(root);
        audit.fail_next_size(plan.costs().raw.rust_bytes);
        let allocation_result = pool
            .submit(&launch, request_bytes(), &record, current, 5000)
            .err();
        let allocation_report = audit.finish();
        assert_eq!(pool.snapshot(), [0; 4]);
        assert_eq!(root.snapshot(), persistent);
        // 失败均还原槽位；真实新提交可重新占用并由 wait/drain/消费完成释放。
        let ticket = pool
            .submit(&launch, request_bytes(), &record, current, 5000)
            .unwrap_or_else(|e| panic!("恢复提交: {e:?}"));
        let loaded = collect(&pool, ticket)
            .into_loaded()
            .unwrap_or_else(|e| panic!("恢复收割: {e:?}"));
        drop(loaded);
        assert_eq!(root.snapshot(), persistent);
        pool.shutdown()
            .unwrap_or_else(|e| panic!("shutdown: {e:?}"));
        assert_eq!(root.snapshot().rust_bytes, 0);
        assert_eq!(root.snapshot().c_bytes, 0);
        drop(fixture);
        assert_eq!([epoch_result, source_result], [Some(Failure::Cancelled); 2]);
        assert_eq!(reserved_results, [Some(Failure::Cancelled); 2]);
        assert_eq!(budget_result, Some(Failure::Budget));
        assert_eq!(unsupported_result, Some(Failure::Unsupported));
        assert_eq!(overflow_result, Some(Failure::Unsupported));
        assert_eq!(allocation_result, Some(Failure::Allocation));
        assert!(unchanged);
        assert_eq!(no_pending, [0; 4]);
        for report in [report, reserved_report, budget_report, unsupported_report] {
            assert!(!report.overflow);
            assert!(report.events().is_empty());
        }
        assert!(!allocation_report.overflow);
        assert!(
            allocation_report
                .events()
                .iter()
                .any(|event| event.kind == 4)
        );
    }

    fn request_bytes() -> [u8; 112] {
        let mut b = [0; 112];
        b[..8].copy_from_slice(MAGIC);
        b[8..10].copy_from_slice(&2u16.to_le_bytes());
        for (at, value) in [
            (12, 65536u64),
            (20, 1024),
            (28, 4096),
            (72, 64 + 24 * 6 + 4096 + 128),
            (80, 1048576),
        ] {
            b[at..at + 8].copy_from_slice(&value.to_le_bytes());
        }
        b[88..92].copy_from_slice(&6u32.to_le_bytes());
        b[96..112].copy_from_slice(&[7; 16]);
        b
    }
    fn submit(pool: &HelperPool, fixture: &Fixture, mode: &str, timeout: u64) -> Ticket {
        let args = [OsStr::new(mode)];
        // SAFETY: fixture 独占目录及 exe；参数只有固定测试场景，绝不接用户配置。
        let launch = unsafe { TrustedLaunch::borrowed(fixture.executable.as_os_str(), &args) }
            .unwrap_or_else(|error| panic!("trusted fixture: {error:?}"));
        pool.submit(
            &launch,
            request_bytes(),
            &fixture.record(&args),
            Admission {
                epoch: ROOT.snapshot().epoch,
                source: None,
            },
            timeout,
        )
        .unwrap_or_else(|error| panic!("child 准入: {error:?}"))
    }
    fn collect(pool: &HelperPool, ticket: Ticket) -> Completion {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(result) = pool
                .take_result(ticket)
                .unwrap_or_else(|error| panic!("收割: {error:?}"))
            {
                return result;
            }
            assert!(Instant::now() < until, "固定监督池挂起");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn await_hook() {
        let until = Instant::now() + Duration::from_secs(5);
        while !crate::worker::test_hooks::REACHED.load(std::sync::atomic::Ordering::Acquire) {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    struct HookRelease;
    impl Drop for HookRelease {
        fn drop(&mut self) {
            crate::worker::test_hooks::RELEASE.store(true, std::sync::atomic::Ordering::Release);
        }
    }

    struct ParentCanary {
        handle: crate::windows::Handle,
        name: String,
        _padding: Vec<crate::windows::Handle>,
    }
    impl ParentCanary {
        fn new(controlled_collision: bool) -> Self {
            use std::os::windows::ffi::OsStrExt;
            use windows_sys::Win32::{
                Foundation::{ERROR_ALREADY_EXISTS, GetLastError},
                System::Threading::CreateEventW,
            };
            let name = format!("Local\\GuoSSHell_w1c_canary_{}", uuid::Uuid::new_v4());
            let wide: Vec<u16> = OsStr::new(&name).encode_wide().chain(Some(0)).collect();
            let mut padding = Vec::new();
            if controlled_collision {
                for _ in 0..1024 {
                    let raw = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
                    assert!(!raw.is_null());
                    let handle = crate::windows::Handle(raw as usize);
                    let ready = handle.0 >= 2048;
                    padding.push(handle);
                    if ready {
                        break;
                    }
                }
                assert!(padding.last().is_some_and(|handle| handle.0 >= 2048));
            }
            let security = windows_sys::Win32::Security::SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<windows_sys::Win32::Security::SECURITY_ATTRIBUTES>()
                    as u32,
                lpSecurityDescriptor: std::ptr::null_mut(),
                bInheritHandle: 1,
            };
            let raw = unsafe { CreateEventW(&security, 0, 0, wide.as_ptr()) };
            assert!(!raw.is_null());
            let handle = crate::windows::Handle(raw as usize);
            assert_ne!(unsafe { GetLastError() }, ERROR_ALREADY_EXISTS);
            Self {
                handle,
                name,
                _padding: padding,
            }
        }
        fn mode(&self, collision: bool) -> String {
            format!(
                "{}:{}:{}",
                if collision {
                    "handle-collision"
                } else {
                    "handle"
                },
                self.handle.0,
                self.name
            )
        }
    }

    /// 仅测试的 alternate spawn：显式额外继承指定 canary，不调用或改变产品 launcher。
    struct AlternateChild {
        process: crate::windows::Handle,
        stdin: Option<crate::windows::Handle>,
        stdout: crate::windows::Handle,
        stderr: crate::windows::Handle,
    }
    impl AlternateChild {
        fn spawn(fixture: &Fixture, mode: &str, canary: &ParentCanary) -> Self {
            use std::os::windows::ffi::OsStrExt;
            use windows_sys::Win32::{
                Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation},
                Security::SECURITY_ATTRIBUTES,
                System::{
                    Pipes::CreatePipe,
                    Threading::{
                        CreateProcessW, DeleteProcThreadAttributeList,
                        EXTENDED_STARTUPINFO_PRESENT, InitializeProcThreadAttributeList,
                        LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
                        PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOEXW,
                        UpdateProcThreadAttribute,
                    },
                },
            };
            fn pipe(parent_reads: bool) -> (crate::windows::Handle, crate::windows::Handle) {
                let security = SECURITY_ATTRIBUTES {
                    nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                    lpSecurityDescriptor: std::ptr::null_mut(),
                    bInheritHandle: 1,
                };
                let mut read = std::ptr::null_mut();
                let mut write = std::ptr::null_mut();
                assert_ne!(
                    unsafe { CreatePipe(&mut read, &mut write, &security, 0) },
                    0
                );
                let read = crate::windows::Handle(read as usize);
                let write = crate::windows::Handle(write as usize);
                let parent = if parent_reads {
                    read.raw()
                } else {
                    write.raw()
                };
                assert_ne!(
                    unsafe { SetHandleInformation(parent, HANDLE_FLAG_INHERIT, 0) },
                    0
                );
                (read, write)
            }
            let (stdin_child, stdin) = pipe(false);
            let (stdout, stdout_child) = pipe(true);
            let (stderr, stderr_child) = pipe(true);
            let executable: Vec<u16> = fixture
                .executable
                .as_os_str()
                .encode_wide()
                .chain(Some(0))
                .collect();
            let mut command = vec![34u16];
            command.extend(fixture.executable.as_os_str().encode_wide());
            command.extend([34, 32, 34]);
            command.extend(mode.encode_utf16());
            command.extend([34, 0]);
            let mut size = 0;
            assert_eq!(
                unsafe { InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut size) },
                0
            );
            assert!(size > 0);
            let mut storage = vec![0usize; size.div_ceil(std::mem::size_of::<usize>())];
            let list = storage.as_mut_ptr().cast();
            assert_ne!(
                unsafe { InitializeProcThreadAttributeList(list, 1, 0, &mut size) },
                0
            );
            struct Attributes(LPPROC_THREAD_ATTRIBUTE_LIST);
            impl Drop for Attributes {
                fn drop(&mut self) {
                    unsafe {
                        DeleteProcThreadAttributeList(self.0);
                    }
                }
            }
            let _attributes = Attributes(list);
            let handles = [
                stdin_child.raw(),
                stdout_child.raw(),
                stderr_child.raw(),
                canary.handle.raw(),
            ];
            assert_ne!(
                unsafe {
                    UpdateProcThreadAttribute(
                        list,
                        0,
                        PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                        handles.as_ptr().cast(),
                        std::mem::size_of_val(&handles),
                        std::ptr::null_mut(),
                        std::ptr::null(),
                    )
                },
                0
            );
            let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
            startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
            startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
            startup.StartupInfo.hStdInput = stdin_child.raw();
            startup.StartupInfo.hStdOutput = stdout_child.raw();
            startup.StartupInfo.hStdError = stderr_child.raw();
            startup.lpAttributeList = list;
            let mut process: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
            assert_ne!(
                unsafe {
                    CreateProcessW(
                        executable.as_ptr(),
                        command.as_mut_ptr(),
                        std::ptr::null(),
                        std::ptr::null(),
                        1,
                        EXTENDED_STARTUPINFO_PRESENT,
                        std::ptr::null(),
                        std::ptr::null(),
                        &startup.StartupInfo,
                        &mut process,
                    )
                },
                0
            );
            drop(crate::windows::Handle(process.hThread as usize));
            Self {
                process: crate::windows::Handle(process.hProcess as usize),
                stdin: Some(stdin),
                stdout,
                stderr,
            }
        }
    }
    impl Drop for AlternateChild {
        fn drop(&mut self) {
            drop(self.stdin.take());
            if !crate::windows::exited(self.process.0) {
                crate::windows::kill(self.process.0)
                    .unwrap_or_else(|error| panic!("停止 owned alternate child: {error:?}"));
            }
            crate::windows::wait(self.process.0)
                .unwrap_or_else(|error| panic!("收割 owned alternate child: {error:?}"));
        }
    }

    #[test]
    fn helper_handle_object_isolation_accepts_child_event_collision_and_rejects_actual_inheritance()
    {
        static ISOLATION_ROOT: ProcessLedger =
            ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
        let fixture = Fixture::new();
        let canary = ParentCanary::new(true);
        let mode = canary.mode(true);
        let args = [OsStr::new(&mode)];
        let launch = unsafe { TrustedLaunch::borrowed(fixture.executable.as_os_str(), &args) }
            .unwrap_or_else(|error| panic!("受控碰撞 fixture: {error:?}"));
        let mut pool =
            HelperPool::new(&ISOLATION_ROOT, 1).unwrap_or_else(|error| panic!("pool: {error:?}"));
        let persistent = ISOLATION_ROOT.snapshot();
        let ticket = pool
            .submit(
                &launch,
                request_bytes(),
                &fixture.record(&args),
                Admission {
                    epoch: 1,
                    source: None,
                },
                5000,
            )
            .unwrap_or_else(|error| panic!("碰撞子进程准入: {error:?}"));
        let completion = collect(&pool, ticket);
        assert_eq!(completion.exit_code(), 0);
        assert_eq!(completion.stderr_bytes(), 0);
        let loaded = completion
            .into_loaded()
            .unwrap_or_else(|error| panic!("不同对象必须正常完成流: {error:?}"));
        assert_eq!(
            loaded.field(0).map(|field| field.value),
            Some(crate::process::BorrowedValue::Text("1,0"))
        );
        assert_eq!(loaded.storage().descriptors().len(), 6);
        assert_eq!(
            loaded.required_facts().group_reference(),
            crate::process::GroupReferenceFact::NoReference
        );
        assert_eq!(ISOLATION_ROOT.snapshot().c_bytes, 0);
        drop(loaded);
        assert_eq!(ISOLATION_ROOT.snapshot(), persistent);
        let ordinary_mode = canary.mode(false);
        let ordinary_args = [OsStr::new(&ordinary_mode)];
        let ordinary_launch =
            unsafe { TrustedLaunch::borrowed(fixture.executable.as_os_str(), &ordinary_args) }
                .unwrap_or_else(|error| panic!("无碰撞 fixture: {error:?}"));
        let ticket = pool
            .submit(
                &ordinary_launch,
                request_bytes(),
                &fixture.record(&ordinary_args),
                Admission {
                    epoch: 1,
                    source: None,
                },
                5000,
            )
            .unwrap_or_else(|error| panic!("无碰撞子进程准入: {error:?}"));
        let completion = collect(&pool, ticket);
        assert_eq!(completion.exit_code(), 0);
        assert_eq!(completion.stderr_bytes(), 0);
        let loaded = completion
            .into_loaded()
            .unwrap_or_else(|error| panic!("无候选对象的正常流: {error:?}"));
        assert_eq!(
            loaded.field(0).map(|field| field.value),
            Some(crate::process::BorrowedValue::Text("0,0"))
        );
        assert_eq!(loaded.storage().descriptors().len(), 6);
        drop(loaded);
        assert_eq!(ISOLATION_ROOT.snapshot(), persistent);
        pool.shutdown()
            .unwrap_or_else(|error| panic!("shutdown: {error:?}"));
        assert_eq!(ISOLATION_ROOT.snapshot().rust_bytes, 0);
        println!("GetHandleInformation=1 Compare_ObjectParentDifferent=0 child_exit=0 fields=6");
        println!("GetHandleInformation=0 incoming_candidate_absent=1 child_exit=0 fields=6");

        let mode = canary.mode(false);
        let mut child = AlternateChild::spawn(&fixture, &mode, &canary);
        child
            .stdin
            .as_ref()
            .unwrap_or_else(|| panic!("缺 stdin"))
            .write_all(&request_bytes())
            .unwrap_or_else(|error| panic!("写 alternate request: {error:?}"));
        drop(child.stdin.take());
        let request = golden_request(false);
        let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
        let mut raw = crate::process::RawStorage::allocate(
            &root,
            Admission {
                epoch: 1,
                source: None,
            },
            &request,
        )
        .unwrap_or_else(|error| panic!("alternate raw: {error:?}"));
        let footer = crate::process::read_stream(&mut child.stdout, &request, &mut raw)
            .unwrap_or_else(|error| panic!("隔离拒绝 footer/EOF: {error:?}"));
        let mut scratch = [0u8; 128];
        assert_eq!(
            child
                .stderr
                .read(&mut scratch)
                .unwrap_or_else(|error| panic!("stderr drain: {error:?}")),
            0
        );
        let code = crate::windows::wait(child.process.0)
            .unwrap_or_else(|error| panic!("actual exit: {error:?}"));
        assert_eq!(code, 101);
        assert_eq!(footer.outcome, 9);
        assert_eq!(footer.code, 1);
        assert_eq!(footer.phase, 1);
        assert_eq!(footer.reason, 1);
        assert!(raw.descriptors().is_empty());
        println!(
            "GetHandleInformation={} Compare_ObjectParentSame={} child_exit={} rejected_outcome={}",
            footer.phase, footer.code, code, footer.outcome
        );
        drop(raw);
        assert_eq!(root.snapshot().rust_bytes, 0);
        drop(child);
        // canary 的 parent owner 到两次真实 child exit 后仍在，最后释放全部测试事件。
        drop(canary);
    }

    #[test]
    fn helper_real_children_repeat_drain_truncate_extra_cancel_timeout_and_result_handoff() {
        let fixture = Fixture::new();
        let missing = fixture.directory.join("missing.exe");
        let canary = ParentCanary::new(false);
        let handle_mode = canary.mode(false);
        let source = ROOT
            .open_source(1, 0)
            .unwrap_or_else(|error| panic!("source: {error:?}"));
        let port_key = crate::inbox::Key {
            epoch: 1,
            ..Default::default()
        };
        let register_wire = crate::control_harness::wire(
            crate::frame::Command::RegisterReplyPort,
            port_key,
            0,
            &[],
        );
        let prepare_key = crate::inbox::Key {
            owner: 1,
            generation: source.generation,
            session: 1,
            ..port_key
        };
        let prepare_wire = crate::control_harness::wire(
            crate::frame::Command::Prepare,
            prepare_key,
            1,
            &["", "", "fixture", "fixture", ""],
        );
        let close_wire = crate::control_harness::wire(
            crate::frame::Command::CloseReplyPort,
            crate::inbox::Key {
                owner: 1,
                ..port_key
            },
            0,
            &[],
        );
        let audit = audit::PoolAuditScope::start(&ROOT);
        let mut pool = HelperPool::new(&ROOT, 1).unwrap_or_else(|error| panic!("pool: {error:?}"));
        let args = [OsStr::new("good")];
        let launch = unsafe { TrustedLaunch::borrowed(fixture.executable.as_os_str(), &args) }
            .unwrap_or_else(|error| panic!("launch: {error:?}"));
        let parsed_request = Request::parse(request_bytes(), MAGIC)
            .unwrap_or_else(|error| panic!("request: {error:?}"));
        let record = fixture.record(&args);
        let plan = crate::windows::SpawnCostPlan::bound(&launch, &parsed_request, &record)
            .unwrap_or_else(|error| panic!("parent bound: {error:?}"));
        let mut costs = crate::control_harness::costs();
        costs.loading = plan.costs();
        let runtime = crate::admission::ControlRuntime::new(&ROOT, 1, costs)
            .unwrap_or_else(|error| panic!("admission: {error:?}"));
        let persistent = ROOT.snapshot();
        {
            assert_eq!(
                runtime.submit(
                    crate::frame::ControlFrame::parse(&register_wire, &[])
                        .unwrap_or_else(|error| panic!("register: {error:?}"))
                )[0],
                0
            );
            assert_eq!(
                runtime.submit(
                    crate::frame::ControlFrame::parse(&prepare_wire, &[])
                        .unwrap_or_else(|error| panic!("prepare: {error:?}"))
                )[0],
                0
            );
            let Some(crate::inbox::NormalWork::Prepare(work)) = runtime.next_batch().normal else {
                panic!("缺已准入工作");
            };
            let key = work.key;
            let helper_before =
                ROOT.snapshot().classes[crate::ledger::ResourceClass::Helper as usize];
            let ticket = pool
                .submit_reserved(
                    crate::worker::Submission {
                        launch: &launch,
                        bytes: parsed_request.bytes,
                        record: &record,
                        admission: Admission {
                            epoch: 1,
                            source: Some(source),
                        },
                        timeout_ms: 5000,
                    },
                    work.loading,
                )
                .unwrap_or_else(|error| panic!("移动原 grants: {error:?}"));
            assert_eq!(
                ROOT.snapshot().classes[crate::ledger::ResourceClass::Helper as usize],
                helper_before
            );
            let completion = collect(&pool, ticket);
            let (footer, raw) = completion.into_result();
            assert_eq!(
                footer
                    .unwrap_or_else(|error| panic!("footer: {error:?}"))
                    .outcome,
                0
            );
            assert_eq!(ROOT.snapshot().c_bytes, 0);
            assert!(!raw.is_empty());
            drop(raw);
            drop(work.payload);
            drop(work.reply);
            runtime.finish_work(key, crate::frame::Status::Cancelled);
            drop(runtime.next_batch());
            assert_eq!(ROOT.snapshot(), persistent);
        }
        for _ in 0..8 {
            let ticket = submit(&pool, &fixture, "good", 5000);
            let completion = collect(&pool, ticket);
            assert_eq!(
                completion
                    .result()
                    .unwrap_or_else(|error| panic!("完整 frame: {error:?}"))
                    .outcome,
                0
            );
            assert_eq!(completion.raw().descriptors().len(), 6);
            assert!(ROOT.snapshot().rust_bytes > persistent.rust_bytes);
            assert_eq!(pool.snapshot(), [0; 4]);
            drop(completion);
            assert_eq!(ROOT.snapshot(), persistent);
        }
        let ticket = submit(&pool, &fixture, "stderr", 5000);
        let completion = collect(&pool, ticket);
        assert!(completion.result().is_ok());
        assert_eq!(completion.stderr_bytes(), 1048576);
        assert!(!completion.stderr_overflow());
        assert_eq!(completion.into_loaded().err(), Some(Failure::Protocol));
        assert_eq!(ROOT.snapshot(), persistent);
        let concurrent =
            std::array::from_fn::<_, 4, _>(|_| submit(&pool, &fixture, "closed_stdout", 5000));
        let args = [OsStr::new("good")];
        let launch = unsafe { TrustedLaunch::borrowed(fixture.executable.as_os_str(), &args) }
            .unwrap_or_else(|error| panic!("launch: {error:?}"));
        assert_eq!(
            pool.submit(
                &launch,
                request_bytes(),
                &fixture.record(&args),
                Admission {
                    epoch: ROOT.snapshot().epoch,
                    source: None
                },
                5000
            )
            .err(),
            Some(Failure::Budget)
        );
        for ticket in concurrent {
            let completion = collect(&pool, ticket);
            assert!(completion.result().is_ok());
            drop(completion);
        }
        assert_eq!(ROOT.snapshot(), persistent);
        // 完整 footer/EOF/exit 不代替接收者消费，迟到 Cancel 仍禁止 LoadedRaw。
        let ticket = submit(&pool, &fixture, "good", 5000);
        let until = Instant::now() + Duration::from_secs(5);
        while pool.snapshot()[ticket.slot] & 8 == 0 {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(ROOT.snapshot().c_bytes > 0);
        pool.cancel(ticket)
            .unwrap_or_else(|error| panic!("迟到取消: {error:?}"));
        assert!(ROOT.snapshot().c_bytes > 0);
        assert_eq!(
            collect(&pool, ticket).into_loaded().err(),
            Some(Failure::Cancelled)
        );
        assert_eq!(ROOT.snapshot(), persistent);

        let ticket = submit(&pool, &fixture, "canary", 5000);
        let loaded = collect(&pool, ticket)
            .into_loaded()
            .unwrap_or_else(|error| panic!("真实收割 LoadedRaw: {error:?}"));
        assert_eq!(loaded.nonce(), [7; 16]);
        assert_eq!(loaded.revision(), 7);
        assert_eq!(loaded.source(), 3);
        assert_eq!(loaded.selected_connection(), None);
        assert_eq!(loaded.artifact_sha256(), fixture.hash);
        assert_eq!(
            loaded.field(0).map(|field| field.value),
            Some(crate::process::BorrowedValue::Text(
                "$(not-a-command); SHSELFN2\0SHSELFD2"
            ))
        );
        assert!(loaded.field(6).is_none());
        assert_eq!(ROOT.snapshot().c_bytes, 0);
        assert!(ROOT.snapshot().rust_bytes > persistent.rust_bytes);
        drop(loaded);
        assert_eq!(ROOT.snapshot(), persistent);

        // 记录错配在 owned copy 前拒绝；正确记录但实际产物 hash 错配不得 spawn。
        let args = [OsStr::new("good")];
        let launch = unsafe { TrustedLaunch::borrowed(fixture.executable.as_os_str(), &args) }
            .unwrap_or_else(|error| panic!("launch: {error:?}"));
        let wrong_record = FixtureRecord {
            executable: missing.as_os_str(),
            arguments: &args,
            hash: fixture.hash,
        };
        assert_eq!(
            pool.submit(
                &launch,
                request_bytes(),
                &wrong_record,
                Admission {
                    epoch: 1,
                    source: None
                },
                5000
            )
            .err(),
            Some(Failure::Unsupported)
        );
        assert_eq!(ROOT.snapshot(), persistent);
        let wrong_hash = FixtureRecord {
            hash: [0; 32],
            ..fixture.record(&args)
        };
        let ticket = pool
            .submit(
                &launch,
                request_bytes(),
                &wrong_hash,
                Admission {
                    epoch: 1,
                    source: None,
                },
                5000,
            )
            .unwrap_or_else(|error| panic!("hash 场景准入: {error:?}"));
        assert_eq!(
            collect(&pool, ticket).into_loaded().err(),
            Some(Failure::Unsupported)
        );
        assert_eq!(ROOT.snapshot(), persistent);

        let plan =
            crate::windows::SpawnCostPlan::bound(&launch, &parsed_request, &fixture.record(&args))
                .unwrap_or_else(|error| panic!("source 测试 bound: {error:?}"));
        let reservations = plan
            .costs()
            .reserve(
                &ROOT,
                Admission {
                    epoch: 1,
                    source: Some(source),
                },
            )
            .unwrap_or_else(|error| panic!("原 source grant: {error:?}"));
        assert_eq!(
            pool.submit_reserved(
                crate::worker::Submission {
                    launch: &launch,
                    bytes: request_bytes(),
                    record: &fixture.record(&args),
                    admission: Admission {
                        epoch: 1,
                        source: None
                    },
                    timeout_ms: 5000,
                },
                reservations
            )
            .err(),
            Some(Failure::Unsupported)
        );
        assert_eq!(ROOT.snapshot(), persistent);
        for (mode, expected) in [("half", Failure::Truncated), ("extra", Failure::Protocol)] {
            let ticket = submit(&pool, &fixture, mode, 5000);
            let completion = collect(&pool, ticket);
            assert_eq!(completion.result().err(), Some(expected));
            drop(completion);
            assert_eq!(ROOT.snapshot(), persistent);
        }
        for (mode, expected) in [
            ("partial_field", Failure::Truncated),
            ("footer_half", Failure::Truncated),
            ("extra_pause", Failure::Protocol),
            ("crash", Failure::ChildCrashed),
            ("bad_nonce", Failure::Protocol),
            ("wide_field", Failure::Protocol),
            ("incomplete", Failure::Protocol),
            ("bad_flags", Failure::Protocol),
        ] {
            let ticket = submit(&pool, &fixture, mode, 5000);
            let completion = collect(&pool, ticket);
            assert_eq!(completion.result().err(), Some(expected));
            drop(completion);
            assert_eq!(ROOT.snapshot(), persistent);
        }
        let ticket = submit(&pool, &fixture, "footer_only", 5000);
        let completion = collect(&pool, ticket);
        assert_eq!(
            completion
                .result()
                .unwrap_or_else(|error| panic!("early footer: {error:?}"))
                .outcome,
            1
        );
        assert!(completion.raw().descriptors().is_empty());
        drop(completion);
        assert_eq!(ROOT.snapshot(), persistent);
        for mode in ["inherit", "quote space\\\"tail\\"] {
            let ticket = submit(&pool, &fixture, mode, 5000);
            let completion = collect(&pool, ticket);
            assert!(completion.result().is_ok());
            if mode.starts_with("quote") {
                let field = completion.raw().descriptors()[0];
                assert_eq!(
                    &completion.raw().bytes()[field.offset..field.offset + field.len],
                    mode.as_bytes()
                );
            }
            drop(completion);
            assert_eq!(ROOT.snapshot(), persistent);
        }
        let ticket = submit(&pool, &fixture, &handle_mode, 5000);
        let completion = collect(&pool, ticket);
        assert!(
            completion.result().is_ok(),
            "不继承 allowlist 之外的 app 句柄"
        );
        assert_eq!(completion.exit_code(), 0);
        assert_eq!(completion.stderr_bytes(), 0);
        let loaded = completion
            .into_loaded()
            .unwrap_or_else(|error| panic!("隔离场景正常流: {error:?}"));
        assert!(matches!(
            loaded.field(0).map(|field| field.value),
            Some(crate::process::BorrowedValue::Text("0,0" | "1,0"))
        ));
        drop(loaded);
        assert_eq!(ROOT.snapshot(), persistent);
        let ticket = submit(&pool, &fixture, "pause", 5000);
        pool.cancel(ticket)
            .unwrap_or_else(|error| panic!("cancel: {error:?}"));
        assert_eq!(
            collect(&pool, ticket).result().err(),
            Some(Failure::Cancelled)
        );
        assert_eq!(ROOT.snapshot(), persistent);
        let ticket = submit(&pool, &fixture, "pause", 100);
        assert_eq!(
            collect(&pool, ticket).result().err(),
            Some(Failure::TimedOut)
        );
        assert_eq!(ROOT.snapshot(), persistent);
        let ticket = submit(&pool, &fixture, "closed_stdout", 5000);
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            pool.take_result(ticket)
                .unwrap_or_else(|error| panic!("pending: {error:?}"))
                .is_none()
        );
        assert!(ROOT.snapshot().c_bytes > 0);
        let completion = collect(&pool, ticket);
        assert!(completion.result().is_ok());
        assert!(ROOT.snapshot().c_bytes > 0);
        drop(completion);
        assert_eq!(ROOT.snapshot(), persistent);
        // 真实 CreateProcess 已成功、尚未发布 handle 时取消；发布后必须立即 kill/wait。
        for point in [1, 2] {
            crate::worker::test_hooks::arm(point);
            let _release = HookRelease;
            let ticket = submit(&pool, &fixture, "pause", 5000);
            await_hook();
            pool.cancel(ticket)
                .unwrap_or_else(|error| panic!("paused cancel: {error:?}"));
            assert!(ROOT.snapshot().c_bytes > 0);
            assert!(
                pool.take_result(ticket)
                    .unwrap_or_else(|error| panic!("尚未完成: {error:?}"))
                    .is_none()
            );
            crate::worker::test_hooks::RELEASE.store(true, std::sync::atomic::Ordering::Release);
            assert_eq!(
                collect(&pool, ticket).result().err(),
                Some(Failure::Cancelled)
            );
            assert_eq!(ROOT.snapshot(), persistent);
        }
        // wait 观察失败保留 root，新的真实 wait 证明完成前不能复用。
        crate::worker::test_hooks::FAIL_WAIT.store(true, std::sync::atomic::Ordering::Release);
        let ticket = submit(&pool, &fixture, "good", 5000);
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            match pool.take_result(ticket) {
                Err(Failure::Wait) => break,
                Ok(None) => {}
                _ => panic!("必须 quarantine"),
            }
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(ROOT.snapshot().c_bytes > 0);
        while !pool
            .retry_wait(ticket)
            .unwrap_or_else(|error| panic!("retry wait: {error:?}"))
        {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(collect(&pool, ticket).result().err(), Some(Failure::Wait));
        assert_eq!(ROOT.snapshot(), persistent);
        let args = [OsStr::new("good")];
        let launch = unsafe { TrustedLaunch::borrowed(missing.as_os_str(), &args) }
            .unwrap_or_else(|error| panic!("缺失路径: {error:?}"));
        let ticket = pool
            .submit(
                &launch,
                request_bytes(),
                &FixtureRecord {
                    executable: missing.as_os_str(),
                    arguments: &args,
                    hash: fixture.hash,
                },
                Admission {
                    epoch: ROOT.snapshot().epoch,
                    source: None,
                },
                5000,
            )
            .unwrap_or_else(|error| panic!("spawn 准入: {error:?}"));
        assert_eq!(collect(&pool, ticket).result().err(), Some(Failure::Spawn));
        assert_eq!(ROOT.snapshot(), persistent);
        // Close 与写前暂停重叠；接收者丢弃后仍由 pool 收割 child 和全部 IO。
        crate::worker::test_hooks::arm(2);
        let release = HookRelease;
        let dropped_receiver = submit(&pool, &fixture, "pause", 5000);
        await_hook();
        assert_eq!(
            runtime.submit(
                crate::frame::ControlFrame::parse(&close_wire, &[])
                    .unwrap_or_else(|error| panic!("close: {error:?}"))
            )[0],
            0
        );
        assert_eq!(ROOT.snapshot().epoch, 2);
        assert_eq!(
            ROOT.snapshot().classes[crate::ledger::ResourceClass::Helper as usize].c_bytes,
            65_536
        );
        let until = Instant::now() + Duration::from_secs(5);
        while pool
            .cancellation(dropped_receiver)
            .unwrap_or_else(|error| panic!("cancel state: {error:?}"))
            != Some(Failure::Cancelled)
        {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(1));
        }
        drop(release);
        let until = Instant::now() + Duration::from_secs(5);
        while pool.snapshot()[dropped_receiver.slot] & 8 == 0 {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(pool.snapshot()[dropped_receiver.slot], 9);
        assert_eq!(
            ROOT.snapshot().classes[crate::ledger::ResourceClass::Helper as usize].c_bytes,
            65_536
        );
        pool.shutdown()
            .unwrap_or_else(|error| panic!("shutdown: {error:?}"));
        drop(runtime);
        assert_eq!(ROOT.snapshot().rust_bytes + ROOT.snapshot().c_bytes, 0);
        let report = audit.finish();
        assert!(
            report.every_live_layout_covered(),
            "跨线程每笔 Layout 到真 dealloc 持额"
        );
    }

    #[test]
    fn helper_reader_bad_headers_reject_before_payload_and_owns_exact_layout() {
        let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
        let request = Request::parse(request_bytes(), MAGIC)
            .unwrap_or_else(|error| panic!("request: {error:?}"));
        let altered = Request {
            wire_max: request.wire_max + 1,
            ..request
        };
        assert!(crate::process::RawStorage::layout(&altered).is_err());
        let mut bad = [0u8; 64];
        bad[..8].copy_from_slice(crate::process::PREAMBLE_MAGIC);
        bad[8..10].copy_from_slice(&2u16.to_le_bytes());
        bad[10..12].copy_from_slice(&1u16.to_le_bytes());
        bad[12..16].copy_from_slice(&64u32.to_le_bytes());
        bad[16..32].copy_from_slice(&request.nonce);
        bad[40..44].copy_from_slice(&3u32.to_le_bytes());
        bad[44..48].copy_from_slice(&u32::MAX.to_le_bytes());
        struct Reader<'a>(&'a [u8]);
        impl crate::process::PipeReader for Reader<'_> {
            fn read(&mut self, out: &mut [u8]) -> Result<usize, Failure> {
                let n = out.len().min(self.0.len());
                out[..n].copy_from_slice(&self.0[..n]);
                self.0 = &self.0[n..];
                Ok(n)
            }
        }
        let audit = audit::AuditScope::start(&root);
        let mut raw = crate::process::RawStorage::allocate(
            &root,
            Admission {
                epoch: 1,
                source: None,
            },
            &request,
        )
        .unwrap_or_else(|error| panic!("raw grant: {error:?}"));
        assert_eq!(
            crate::process::read_stream(&mut Reader(&bad), &request, &mut raw).err(),
            Some(Failure::Protocol)
        );
        assert_eq!(raw.len(), 0);
        drop(raw);
        assert!(audit.finish().every_live_layout_covered());
        assert_eq!(root.snapshot().rust_bytes, 0);
    }

    #[test]
    fn helper_aggregate_validation_stops_at_preamble_before_field_header_or_copy() {
        let request = Request::parse(request_bytes(), MAGIC)
            .unwrap_or_else(|error| panic!("request: {error:?}"));
        let mut preamble = [0u8; 64];
        preamble[..8].copy_from_slice(crate::process::PREAMBLE_MAGIC);
        preamble[8..10].copy_from_slice(&2u16.to_le_bytes());
        preamble[10..12].copy_from_slice(&1u16.to_le_bytes());
        preamble[12..16].copy_from_slice(&64u32.to_le_bytes());
        preamble[16..32].copy_from_slice(&request.nonce);
        preamble[40..44].copy_from_slice(&3u32.to_le_bytes());
        preamble[44..48].copy_from_slice(&6u32.to_le_bytes());
        preamble[48..56].copy_from_slice(&6u64.to_le_bytes());
        preamble[56..64].copy_from_slice(&342u64.to_le_bytes());
        struct Reader {
            preamble: [u8; 64],
            at: usize,
        }
        impl crate::process::PipeReader for Reader {
            fn read(&mut self, out: &mut [u8]) -> Result<usize, Failure> {
                assert!(self.at < 64, "不得在 aggregate 拒绝后读取 field header");
                let n = out.len().min(64 - self.at);
                out[..n].copy_from_slice(&self.preamble[self.at..self.at + n]);
                self.at += n;
                Ok(n)
            }
        }
        for (offset, width, value) in [
            (44, 4, 7u64),
            (48, 8, 4097),
            (56, 8, 343),
            (56, 8, u64::MAX),
            (40, 4, 1),
        ] {
            let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
            let mut changed = preamble;
            changed[offset..offset + width].copy_from_slice(&value.to_le_bytes()[..width]);
            let mut reader = Reader {
                preamble: changed,
                at: 0,
            };
            let audit = audit::AuditScope::start(&root);
            let mut raw = crate::process::RawStorage::allocate(
                &root,
                Admission {
                    epoch: 1,
                    source: None,
                },
                &request,
            )
            .unwrap_or_else(|error| panic!("actual raw grant: {error:?}"));
            let held = root.snapshot();
            assert_eq!(
                crate::process::read_stream(&mut reader, &request, &mut raw).err(),
                Some(Failure::Protocol)
            );
            assert_eq!(reader.at, 64);
            assert!(raw.is_empty());
            assert!(raw.descriptors().is_empty());
            assert_eq!(root.snapshot(), held);
            drop(raw);
            assert!(audit.finish().every_live_layout_covered());
            assert_eq!(root.snapshot().rust_bytes, 0);
        }
    }

    struct Reader<'a> {
        bytes: &'a [u8],
        at: usize,
        stop: Option<usize>,
    }
    impl crate::process::PipeReader for Reader<'_> {
        fn read(&mut self, out: &mut [u8]) -> Result<usize, Failure> {
            if let Some(stop) = self.stop {
                assert!(self.at < stop, "拒绝必须先于 payload 或下一 field 的读取");
            }
            let n = out.len().min(self.bytes.len() - self.at);
            out[..n].copy_from_slice(&self.bytes[self.at..self.at + n]);
            self.at += n;
            Ok(n)
        }
    }

    fn golden_request(connected: bool) -> Request {
        let mut bytes = request_bytes();
        if connected {
            bytes[10..12].copy_from_slice(&1u16.to_le_bytes());
            bytes[40..56].copy_from_slice(&[9; 16]);
            bytes[72..80].copy_from_slice(&(64 + 24 * 22 + 4096 + 10 + 128u64).to_le_bytes());
            bytes[88..92].copy_from_slice(&22u32.to_le_bytes());
        }
        Request::parse(bytes, MAGIC).unwrap_or_else(|error| panic!("golden request: {error:?}"))
    }

    struct Golden {
        bytes: Vec<u8>,
        group_payload: usize,
        footer: usize,
    }
    /// 独立手写线格式；不 include producer、解析 JSON 或替 domain 判 UUID。
    fn golden(
        request: &Request,
        fact: u16,
        null_group: bool,
        completed: usize,
        outcome: u16,
    ) -> Golden {
        let count = if request.connected { 22 } else { 6 };
        let first = if request.connected { 1 } else { 17 };
        let group = if fact == 4 {
            b"not-a-uuid".as_slice()
        } else {
            b"{00000000-0000-4000-8000-000000000002}".as_slice()
        };
        let mut fields = Vec::new();
        let mut text = 0u64;
        let mut integers = 0u64;
        let mut group_payload = 0;
        let mut text_upper = 0u64;
        for index in 0..count {
            let tag = first + index as u16;
            let scalar = if tag == 5 { 22u64 } else { (-7i64) as u64 }.to_le_bytes();
            let (kind, payload): (u8, &[u8]) = match tag {
                2 if null_group => (0, &[]),
                2 => (1, group),
                5 => (2, &scalar[..2]),
                14 => (3, &scalar),
                22 => (1, b"{not-json"),
                _ => (1, b"x"),
            };
            if kind == 1 {
                text_upper += payload.len() as u64;
            }
            if index >= completed {
                continue;
            }
            let mut header = [0u8; 24];
            header[..8].copy_from_slice(b"SHSELFD2");
            header[8..10].copy_from_slice(&tag.to_le_bytes());
            header[10] = kind;
            header[16..24].copy_from_slice(&(payload.len() as u64).to_le_bytes());
            fields.extend_from_slice(&header);
            if tag == 2 {
                group_payload = 64 + fields.len();
            }
            fields.extend_from_slice(payload);
            if kind == 1 {
                text += payload.len() as u64;
            } else {
                integers += payload.len() as u64;
            }
        }
        let mut preamble = [0u8; 64];
        preamble[..8].copy_from_slice(b"SHSELHD2");
        preamble[8..10].copy_from_slice(&2u16.to_le_bytes());
        preamble[10..12].copy_from_slice(&fact.to_le_bytes());
        preamble[12..16].copy_from_slice(&64u32.to_le_bytes());
        preamble[16..32].copy_from_slice(&request.nonce);
        preamble[32..40].copy_from_slice(&37u64.to_le_bytes());
        preamble[40..44]
            .copy_from_slice(&(if request.connected { 2u32 } else { 3u32 }).to_le_bytes());
        preamble[44..48].copy_from_slice(&(count as u32).to_le_bytes());
        preamble[48..56].copy_from_slice(&text_upper.to_le_bytes());
        let wire =
            64 + 24 * count as u64 + text_upper + if request.connected { 10 } else { 0 } + 128;
        preamble[56..64].copy_from_slice(&wire.to_le_bytes());
        let mut bytes = preamble.to_vec();
        bytes.extend_from_slice(&fields);
        let at = bytes.len();
        let mut footer = [0u8; 128];
        footer[..8].copy_from_slice(b"SHSELFN2");
        footer[8..10].copy_from_slice(&2u16.to_le_bytes());
        footer[10..12].copy_from_slice(&outcome.to_le_bytes());
        footer[12..16].copy_from_slice(
            &((if outcome == 0 { 95u32 } else { 7u32 }) | (u32::from(fact) << 8)).to_le_bytes(),
        );
        footer[16..32].copy_from_slice(&request.nonce);
        footer[32..40].copy_from_slice(&37u64.to_le_bytes());
        footer[40..44].copy_from_slice(&preamble[40..44]);
        footer[44..48].copy_from_slice(&(completed as u32).to_le_bytes());
        footer[48..56].copy_from_slice(&text.to_le_bytes());
        footer[56..64].copy_from_slice(&integers.to_le_bytes());
        footer[64..72].copy_from_slice(&(at as u64).to_le_bytes());
        bytes.extend_from_slice(&footer);
        Golden {
            bytes,
            group_payload,
            footer: at,
        }
    }

    fn read_golden(
        request: &Request,
        bytes: &[u8],
        expected: Option<Failure>,
    ) -> Option<crate::process::Footer> {
        let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
        let audit = audit::AuditScope::start(&root);
        let mut raw = crate::process::RawStorage::allocate(
            &root,
            Admission {
                epoch: 1,
                source: None,
            },
            request,
        )
        .unwrap_or_else(|error| panic!("golden raw grant: {error:?}"));
        let held = root.snapshot();
        let result = crate::process::read_stream(
            &mut Reader {
                bytes,
                at: 0,
                stop: None,
            },
            request,
            &mut raw,
        );
        assert_eq!(result.as_ref().err().copied(), expected);
        assert_eq!(root.snapshot(), held);
        drop(raw);
        assert!(audit.finish().every_live_layout_covered());
        assert_eq!(root.snapshot().rust_bytes, 0);
        result.ok()
    }

    #[test]
    fn helper_group_facts_golden_preserve_all_values_and_failure_prefixes() {
        let request = golden_request(true);
        for fact in 1..=5 {
            let complete = golden(&request, fact, fact == 1, 22, 0);
            let footer =
                read_golden(&request, &complete.bytes, None).unwrap_or_else(|| panic!("缺 footer"));
            assert_eq!(footer.flags, 95 | (u32::from(fact) << 8));
            for completed in [0, 1, 2, 21, 22] {
                let partial = golden(&request, fact, fact == 1, completed, 5);
                let footer = read_golden(&request, &partial.bytes, None)
                    .unwrap_or_else(|| panic!("缺失败 footer"));
                assert_eq!(footer.outcome, 5);
                assert_eq!(footer.flags, 7 | (u32::from(fact) << 8));
            }
        }
        let request = golden_request(false);
        let complete = golden(&request, 1, true, 6, 0);
        assert!(read_golden(&request, &complete.bytes, None).is_some());
        for fact in 2..=5 {
            let invalid = golden(&request, fact, false, 6, 0);
            read_golden(&request, &invalid.bytes, Some(Failure::Protocol));
        }
    }

    #[test]
    fn helper_group_facts_reserved_mismatch_missing_and_dangling_reject() {
        let request = golden_request(true);
        for fact in [0, 6, 7, 8, 0x8001] {
            let invalid = golden(&request, fact, true, 22, 0);
            read_golden(&request, &invalid.bytes, Some(Failure::Protocol));
        }
        for outcome in [0, 5] {
            let base = golden(&request, 2, false, 22, outcome);
            for bits in [0, 1, 3, 4, 5, 6, 7, 8, 0x8000] {
                let mut invalid = base.bytes.clone();
                invalid[base.footer + 12..base.footer + 16].copy_from_slice(
                    &((if outcome == 0 { 95u32 } else { 7u32 }) | (bits << 8)).to_le_bytes(),
                );
                read_golden(&request, &invalid, Some(Failure::Protocol));
            }
        }
        let absent = golden(&request, 3, false, 21, 0);
        read_golden(&request, &absent.bytes, Some(Failure::Protocol));
        let request = golden_request(false);
        let mut early = [0u8; 128];
        early[..8].copy_from_slice(b"SHSELFN2");
        early[8..10].copy_from_slice(&2u16.to_le_bytes());
        early[10..12].copy_from_slice(&1u16.to_le_bytes());
        early[12..16].copy_from_slice(&4u32.to_le_bytes());
        early[16..32].copy_from_slice(&request.nonce);
        assert_eq!(
            read_golden(&request, &early, None).map(|footer| footer.outcome),
            Some(1)
        );
        for fact in 1..=7 {
            early[12..16].copy_from_slice(&(4u32 | (fact << 8)).to_le_bytes());
            read_golden(&request, &early, Some(Failure::Protocol));
        }
    }

    #[test]
    fn helper_group_facts_header_conflicts_reject_before_payload_and_errors_do_not_commit_field() {
        let request = golden_request(true);
        for fact in 1..=5 {
            let invalid = golden(&request, fact, fact != 1, 22, 0);
            let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
            let mut raw = crate::process::RawStorage::allocate(
                &root,
                Admission {
                    epoch: 1,
                    source: None,
                },
                &request,
            )
            .unwrap_or_else(|error| panic!("raw: {error:?}"));
            let held = root.snapshot();
            let mut reader = Reader {
                bytes: &invalid.bytes,
                at: 0,
                stop: Some(invalid.group_payload),
            };
            assert_eq!(
                crate::process::read_stream(&mut reader, &request, &mut raw).err(),
                Some(Failure::Protocol)
            );
            assert_eq!(reader.at, invalid.group_payload);
            assert_eq!(raw.descriptors().len(), 1);
            assert_eq!(root.snapshot(), held);
            drop(raw);
            assert_eq!(root.snapshot().rust_bytes, 0);
        }
        let mut invalid = golden(&request, 2, false, 22, 0);
        invalid.bytes[64 + 24] = 0xff;
        let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
        let mut raw = crate::process::RawStorage::allocate(
            &root,
            Admission {
                epoch: 1,
                source: None,
            },
            &request,
        )
        .unwrap_or_else(|error| panic!("raw: {error:?}"));
        let held = root.snapshot();
        let mut reader = Reader {
            bytes: &invalid.bytes,
            at: 0,
            stop: None,
        };
        assert_eq!(
            crate::process::read_stream(&mut reader, &request, &mut raw).err(),
            Some(Failure::Protocol)
        );
        assert_eq!(raw.len(), 64);
        assert!(raw.descriptors().is_empty());
        let at = reader.at;
        assert_eq!(
            crate::process::read_stream(&mut reader, &request, &mut raw).err(),
            Some(Failure::Protocol)
        );
        assert_eq!(reader.at, at);
        assert_eq!(root.snapshot(), held);
        drop(raw);
        assert_eq!(root.snapshot().rust_bytes, 0);
    }

    #[test]
    fn helper_loaded_facts_and_stderr_gate_require_actual_pool_harvest() {
        static FACT_ROOT: ProcessLedger = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
        let fixture = Fixture::new();
        let mut pool =
            HelperPool::new(&FACT_ROOT, 1).unwrap_or_else(|error| panic!("pool: {error:?}"));
        let persistent = FACT_ROOT.snapshot();
        for mode in ["good", "stderr"] {
            let args = [OsStr::new(mode)];
            let launch = unsafe { TrustedLaunch::borrowed(fixture.executable.as_os_str(), &args) }
                .unwrap_or_else(|error| panic!("fixture launch: {error:?}"));
            let record = fixture.record(&args);
            let request = golden_request(false);
            let plan = crate::windows::SpawnCostPlan::bound(&launch, &request, &record)
                .unwrap_or_else(|error| panic!("fixture actual bound: {error:?}"));
            let admission = Admission {
                epoch: 1,
                source: None,
            };
            let reservations = plan
                .costs()
                .reserve(&FACT_ROOT, admission)
                .unwrap_or_else(|error| panic!("beforeSpawn grants: {error:?}"));
            let admitted = FACT_ROOT.snapshot();
            let ticket = pool
                .submit_reserved(
                    crate::worker::Submission {
                        launch: &launch,
                        bytes: request.bytes,
                        record: &record,
                        admission,
                        timeout_ms: 5000,
                    },
                    reservations,
                )
                .unwrap_or_else(|error| panic!("submit: {error:?}"));
            assert_eq!(FACT_ROOT.snapshot(), admitted);
            let completion = collect(&pool, ticket);
            assert!(completion.result().is_ok());
            assert_eq!(completion.exit_code(), 0);
            assert!(FACT_ROOT.snapshot().c_bytes > 0);
            if mode == "stderr" {
                assert_eq!(completion.stderr_bytes(), 1048576);
                assert_eq!(completion.into_loaded().err(), Some(Failure::Protocol));
            } else {
                assert_eq!(completion.stderr_bytes(), 0);
                let loaded = completion
                    .into_loaded()
                    .unwrap_or_else(|error| panic!("LoadedRaw: {error:?}"));
                let facts = loaded.required_facts();
                assert_eq!(
                    facts.group_reference(),
                    crate::process::GroupReferenceFact::NoReference
                );
                assert_eq!(facts.transaction_nonce(), request.nonce);
                assert_eq!(facts.revision(), 7);
                assert_eq!(facts.source(), 3);
                assert_eq!(facts.selected_connection(), None);
                assert_eq!(loaded.artifact_sha256(), fixture.hash);
                assert_eq!(FACT_ROOT.snapshot().c_bytes, 0);
                assert!(FACT_ROOT.snapshot().rust_bytes > persistent.rust_bytes);
                drop(loaded);
            }
            assert_eq!(FACT_ROOT.snapshot(), persistent);
        }
        pool.shutdown()
            .unwrap_or_else(|error| panic!("shutdown: {error:?}"));
        assert_eq!(FACT_ROOT.snapshot().rust_bytes, 0);
    }

    const CHILD: &str = r#"
use std::io::{Read, Write};
type RawHandle = *mut std::ffi::c_void;
#[link(name="kernel32")] unsafe extern "system" {
    fn GetStdHandle(kind:u32)->RawHandle;
    fn CloseHandle(handle:RawHandle)->i32;
    fn GetHandleInformation(handle:RawHandle, flags:*mut u32)->i32;
    fn GetCurrentProcess()->RawHandle;
    fn DuplicateHandle(source_process:RawHandle,source:RawHandle,target_process:RawHandle,target:*mut RawHandle,access:u32,inherit:i32,options:u32)->i32;
    fn GetLastError()->u32;
    fn CreateEventW(security:*const std::ffi::c_void,manual:i32,initial:i32,name:*const u16)->RawHandle;
    fn OpenEventW(access:u32,inherit:i32,name:*const u16)->RawHandle;
    fn GetModuleHandleW(name:*const u16)->RawHandle;
    fn GetProcAddress(module:RawHandle,name:*const u8)->*const std::ffi::c_void;
}
struct OwnedHandle(RawHandle);
impl Drop for OwnedHandle { fn drop(&mut self) { if !self.0.is_null() { assert_ne!(unsafe { CloseHandle(self.0) },0); } } }
struct CollisionEvents { handles:[usize;1024],len:usize }
impl CollisionEvents {
    fn new()->Self { Self { handles:[0;1024],len:0 } }
    fn occupy(&mut self,target:usize) {
        for _ in 0..self.handles.len() {
            let raw=unsafe { CreateEventW(std::ptr::null(),0,0,std::ptr::null()) };
            assert!(!raw.is_null()); self.handles[self.len]=raw as usize; self.len+=1;
            if raw as usize == target { return; }
        }
        panic!("受控子 Event 未占用指定数值");
    }
}
impl Drop for CollisionEvents { fn drop(&mut self) { for raw in &self.handles[..self.len] { assert_ne!(unsafe { CloseHandle(*raw as RawHandle) },0); } } }
fn observe_candidate(candidate:usize,name:&str)->[u8;3] {
    let mut flags=0;
    let present=unsafe { GetHandleInformation(candidate as RawHandle,&mut flags) } != 0;
    let current=unsafe { GetCurrentProcess() };
    let mut duplicate=std::ptr::null_mut();
    // 先固定原 candidate；诊断 reference 或 DLL 查询不能污染被测数值。
    let copied=unsafe { DuplicateHandle(current,candidate as RawHandle,current,&mut duplicate,0,0,2) };
    if present { assert_ne!(copied,0); } else { assert_eq!(copied,0); assert_eq!(unsafe { GetLastError() },6); }
    let snapshot=OwnedHandle(duplicate);
    let wide:Vec<u16>=name.encode_utf16().chain(Some(0)).collect();
    let reference=OwnedHandle(unsafe { OpenEventW(0x00100000,0,wide.as_ptr()) });
    assert!(!reference.0.is_null());
    let dll:Vec<u16>="kernelbase.dll".encode_utf16().chain(Some(0)).collect();
    let module=unsafe { GetModuleHandleW(dll.as_ptr()) }; assert!(!module.is_null());
    let address=unsafe { GetProcAddress(module,c"CompareObjectHandles".as_ptr().cast()) }; assert!(!address.is_null());
    let compare:unsafe extern "system" fn(RawHandle,RawHandle)->i32=unsafe { std::mem::transmute(address) };
    let same=!snapshot.0.is_null() && unsafe { compare(snapshot.0,reference.0) } != 0;
    [b'0'+u8::from(present),b',',b'0'+u8::from(same)]
}
fn main() {
    let mode = std::env::args().nth(1).unwrap();
    if mode == "inherit" { assert_eq!(std::env::var("RUSTUP_AUTO_INSTALL").unwrap(),"0"); assert!(std::path::Path::new("Cargo.toml").is_file()); }
    let mut collision_events=CollisionEvents::new();
    let candidate=mode.strip_prefix("handle-collision:").map(|value|(value,true)).or_else(||mode.strip_prefix("handle:").map(|value|(value,false)));
    let observation=if let Some((value,collision))=candidate {
        let (numeric,name)=value.split_once(':').unwrap(); let numeric=numeric.parse::<usize>().unwrap();
        // 这里建立的是待测子对象，不是身份诊断 reference。
        if collision { collision_events.occupy(numeric); }
        Some(observe_candidate(numeric,name))
    } else { None };
    if mode == "pause" { std::thread::sleep(std::time::Duration::from_secs(60)); return; }
    let mut request = [0;112]; std::io::stdin().read_exact(&mut request).unwrap();
    let mut eof=[0;1]; assert_eq!(std::io::stdin().read(&mut eof).unwrap(),0);
    if let Some(observed)=observation && observed[2]==b'1' {
        let mut footer=[0u8;128]; footer[..8].copy_from_slice(b"SHSELFN2"); footer[8..10].copy_from_slice(&2u16.to_le_bytes());
        footer[10..12].copy_from_slice(&9u16.to_le_bytes()); footer[12..16].copy_from_slice(&4u32.to_le_bytes()); footer[16..32].copy_from_slice(&request[96..112]);
        footer[72..74].copy_from_slice(&1u16.to_le_bytes()); footer[74]=observed[0]-b'0'; footer[75]=1;
        std::io::stdout().write_all(&footer).unwrap(); std::io::stdout().flush().unwrap(); std::process::exit(101);
    }
    if mode == "footer_only" || mode == "bad_nonce" { let mut footer=[0u8;128]; footer[..8].copy_from_slice(b"SHSELFN2"); footer[8..10].copy_from_slice(&2u16.to_le_bytes()); footer[10..12].copy_from_slice(&1u16.to_le_bytes()); footer[12..16].copy_from_slice(&4u32.to_le_bytes()); if mode == "footer_only" { footer[16..32].copy_from_slice(&request[96..112]); } std::io::stdout().write_all(&footer).unwrap(); return; }
    if mode == "stderr" { let scratch=[b'e';128]; let mut stderr=std::io::stderr().lock(); for _ in 0..8192 { stderr.write_all(&scratch).unwrap(); } }
    let mut header=[0u8;64]; header[..8].copy_from_slice(b"SHSELHD2"); header[8..10].copy_from_slice(&2u16.to_le_bytes()); header[10..12].copy_from_slice(&1u16.to_le_bytes()); header[12..16].copy_from_slice(&64u32.to_le_bytes());
    header[16..32].copy_from_slice(&request[96..112]); header[32..40].copy_from_slice(&7u64.to_le_bytes()); header[40..44].copy_from_slice(&3u32.to_le_bytes()); header[44..48].copy_from_slice(&6u32.to_le_bytes());
    let first: &[u8] = if let Some(ref observed)=observation { observed } else if mode.starts_with("quote") { mode.as_bytes() } else if mode == "canary" { b"$(not-a-command); SHSELFN2\0SHSELFD2" } else { b"x" }; let text=first.len() as u64+5;
    header[48..56].copy_from_slice(&text.to_le_bytes()); header[56..64].copy_from_slice(&(64+6*24+text+128u64).to_le_bytes());
    let mut stdout=std::io::stdout().lock();
    if mode == "half" { stdout.write_all(&header[..32]).unwrap(); return; }
    stdout.write_all(&header).unwrap();
    for tag in 17..=22u16 { if mode == "incomplete" && tag == 22 { break; } let value = if tag == 17 { first } else { b"x" }; let mut field=[0u8;24]; field[..8].copy_from_slice(b"SHSELFD2"); field[8..10].copy_from_slice(&tag.to_le_bytes()); field[10]=1; field[16..24].copy_from_slice(&(if mode == "wide_field" { 1025u64 } else { value.len() as u64 }).to_le_bytes()); stdout.write_all(&field).unwrap(); if mode == "partial_field" || mode == "wide_field" { return; } stdout.write_all(value).unwrap(); }
    let mut footer=[0u8;128]; footer[..8].copy_from_slice(b"SHSELFN2"); footer[8..10].copy_from_slice(&2u16.to_le_bytes()); footer[12..16].copy_from_slice(&351u32.to_le_bytes()); footer[16..32].copy_from_slice(&request[96..112]);
    footer[32..40].copy_from_slice(&7u64.to_le_bytes()); footer[40..44].copy_from_slice(&3u32.to_le_bytes()); footer[44..48].copy_from_slice(&6u32.to_le_bytes()); footer[48..56].copy_from_slice(&text.to_le_bytes()); footer[64..72].copy_from_slice(&(64+6*24+text).to_le_bytes());
    if mode == "bad_flags" { footer[12..16].copy_from_slice(&479u32.to_le_bytes()); }
    if mode == "footer_half" { stdout.write_all(&footer[..60]).unwrap(); return; }
    stdout.write_all(&footer).unwrap();
    if mode == "extra" || mode == "extra_pause" { stdout.write_all(b"x").unwrap(); }
    stdout.flush().unwrap(); drop(stdout);
    if mode == "crash" { std::process::exit(55); }
    if mode == "extra_pause" { std::thread::sleep(std::time::Duration::from_secs(60)); }
    if mode == "closed_stdout" { unsafe { CloseHandle(GetStdHandle((-11i32) as u32)); } std::thread::sleep(std::time::Duration::from_millis(500)); }
}
"#;
}
