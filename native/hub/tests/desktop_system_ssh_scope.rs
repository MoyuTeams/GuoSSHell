//! 独立消费 System scope；只查询已存在工具的无网络版本/配置，不注册生产入口。

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

#[path = "../src/system_ssh/scope/mod.rs"]
pub mod scope;

#[global_allocator]
static ALLOCATOR: audit::Allocator = audit::Allocator;

use ledger::{BYTE_LIMIT, CLASS_COUNT, ProcessLedger};
use scope::{
    ActualProcessLayout, ActualProcessSourceClosure, ActualVersion, AuthObservation, BorrowedShape,
    BorrowedSystem, DesktopPlatform, EnvClearExplicitBuilder, EvidenceLevel, ExeIdentity,
    ExternalIdentity, HookAttributes, LaunchPurpose, ParentProcessProof, PrivateAttemptFiles,
    PrivatePaths, ScopeReject, SelectedFileAccess, SourceBinding, SystemAuth, SystemProgress,
    TrustedOsPaths, admit_actual_process, require_session_hooks, windows_crt_argument,
};

fn succeeds<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("预期成功：{error:?}"),
    }
}

struct FixtureOs;
// SAFETY: 仅 codec fixture，不提供真实 spawn 的 OS 来源证明。
unsafe impl TrustedOsPaths for FixtureOs {
    fn system_root(&self) -> Option<&str> {
        Some("C:\\Windows")
    }
}
// SAFETY: 仅 synthetic codec，不创建真实进程或文件访问。
unsafe impl PrivateAttemptFiles for FixtureOs {
    fn owns_private_attempt(&self, paths: PrivatePaths<'_>, _: SourceBinding) -> bool {
        paths.cwd == PRIVATE.cwd
    }
}

#[cfg(windows)]
const PRIVATE: PrivatePaths<'static> = PrivatePaths {
    cwd: "C:\\scope-fixture",
    home: "C:\\scope-fixture\\home",
    config: "C:\\scope-fixture\\config",
    empty_certificate: "C:\\scope-fixture\\empty-cert",
    empty_provider: "C:\\scope-fixture\\empty-provider",
    helper: "C:\\scope-fixture\\工具 % $ helper.exe",
    endpoint: "synthetic-endpoint",
};
#[cfg(not(windows))]
const PRIVATE: PrivatePaths<'static> = PrivatePaths {
    cwd: "/scope-fixture",
    home: "/scope-fixture/home",
    config: "/scope-fixture/config",
    empty_certificate: "/scope-fixture/empty-cert",
    empty_provider: "/scope-fixture/empty-provider",
    helper: "/scope-fixture/工具 % $ helper",
    endpoint: "synthetic-endpoint",
};

fn input(source: ledger::SourceToken) -> BorrowedSystem<'static> {
    BorrowedSystem {
        host: "2001:db8::5",
        port: 2222,
        username: "用户 名%${X}",
        remote_command: Some("printf '%s' \"远端 $HOME; ~ ' \\\"\""),
        term: "xterm-256color",
        auth: SystemAuth::Password,
        paths: PRIVATE,
        platform: DesktopPlatform::current(),
        os: &FixtureOs,
        private_files: &FixtureOs,
        binding: SourceBinding {
            source,
            attempt: 7,
            target_identity: 11,
        },
    }
}

#[test]
fn system_borrowed_shape_and_exact_grant_precede_all_owned_encoding() {
    let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
    let source = succeeds(root.open_source(1, 0));
    let image = succeeds(ExeIdentity::test_only_trusted_codec(PRIVATE.helper));
    let audit = audit::AuditScope::start(&root);
    let shape = succeeds(BorrowedShape::checked(
        input(source),
        &image,
        LaunchPurpose::Session,
    ));
    let model = shape.cost_model();
    let owned = succeeds(shape.admit(&root));
    assert_eq!(owned.held_bytes(), model.backing.backing.size());
    assert_eq!(owned.config().len(), model.config_bytes);
    assert_eq!(owned.program(), PRIVATE.helper);
    assert_eq!(owned.cwd(), PRIVATE.cwd);
    assert_eq!(owned.args().last(), input(source).remote_command);
    assert!(owned.args().eq([
        "-F",
        PRIVATE.config,
        "-l",
        "用户 名%${X}",
        "-p",
        "2222",
        "-tt",
        "--",
        "2001:db8::5",
        input(source).remote_command.unwrap_or("")
    ]));
    assert_eq!(
        owned
            .windows_units(owned.record().windows_command)
            .map(|v| v.len()),
        Ok(model.windows_command_units)
    );
    assert_eq!(
        owned
            .windows_units(owned.record().windows_environment)
            .map(|v| v.len()),
        Ok(model.windows_environment_units)
    );
    assert_eq!(owned.record().binding.source, source);
    let process = succeeds(owned.retain());
    let helper = succeeds(owned.retain());
    let pipe = succeeds(owned.retain());
    let late_root = succeeds(owned.retain());
    succeeds(root.invalidate_source(source));
    drop(owned);
    drop(process);
    drop(helper);
    drop(pipe);
    assert_eq!(root.snapshot().rust_bytes, model.backing.backing.size());
    drop(late_root);
    let report = audit.finish();
    assert!(report.symmetric_single_backing());
    report.assert_balanced_and_pregranted();
    assert_eq!(root.snapshot().rust_bytes, 0);
    eprintln!(
        "SYSTEM_CODEC H={} payload={} config={}：单 backing，最后 holder free 后返额；OS/OpenSSH 不在该界内",
        model.backing.backing.size(),
        model.payload_bytes,
        model.config_bytes
    );
}

#[test]
fn system_real_shortfall_and_allocation_failure_do_not_create_backing() {
    let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
    let source = succeeds(root.open_source(1, 0));
    let image = succeeds(ExeIdentity::test_only_trusted_codec(PRIVATE.helper));
    let model = succeeds(BorrowedShape::checked(
        input(source),
        &image,
        LaunchPurpose::Session,
    ))
    .cost_model();
    let short = ProcessLedger::new(model.backing.backing.size() - 1, [u32::MAX; CLASS_COUNT]);
    let source = succeeds(short.open_source(1, 0));
    let audit = audit::AuditScope::start(&short);
    assert_eq!(
        succeeds(BorrowedShape::checked(
            input(source),
            &image,
            LaunchPurpose::Session
        ))
        .admit(&short)
        .err(),
        Some(ScopeReject::Quota {
            requested: model.backing.backing.size(),
            current: 0,
            limit: model.backing.backing.size() - 1
        })
    );
    assert!(audit.finish().events().is_empty());
    let source = succeeds(root.open_source(1, 1));
    let audit = audit::AuditScope::start(&root);
    audit.fail_next_size(model.backing.backing.size());
    assert_eq!(
        succeeds(BorrowedShape::checked(
            input(source),
            &image,
            LaunchPurpose::Session
        ))
        .admit(&root)
        .err(),
        Some(ScopeReject::Resource(
            layout::NumericReject::AllocationFailure
        ))
    );
    assert_eq!(audit.finish().events()[0].kind, 4);
    assert_eq!(root.snapshot().rust_bytes, 0);
}

#[test]
fn system_environment_is_explicit_not_inherited_and_probe_has_no_callback() {
    let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
    let source = succeeds(root.open_source(1, 0));
    let env = succeeds(EnvClearExplicitBuilder::for_system(
        input(source),
        LaunchPurpose::Session,
    ));
    for forbidden in [
        "PATH",
        "SSH_AUTH_SOCK",
        "SSH_AGENT_PID",
        "SSH_SK_PROVIDER",
        "LD_PRELOAD",
        "DYLD_INSERT_LIBRARIES",
        "BASH_ENV",
        "ENV",
        "KRB5_CONFIG",
        "GUO_SCOPE_FIXTURE_RUSTC",
        "GUO_SCOPE_FIXTURE_LINKER",
    ] {
        assert!(
            !env.entries()
                .any(|entry| entry.key.eq_ignore_ascii_case(forbidden))
        );
    }
    assert!(
        env.entries()
            .any(|e| e.key == "SSH_ASKPASS_REQUIRE" && e.value == "force")
    );
    assert!(
        env.entries()
            .any(|e| e.key == "HOME" && e.value == PRIVATE.home)
    );
    assert!(
        env.entries()
            .any(|e| matches!(e.key, "SHELL" | "COMSPEC") && e.value == PRIVATE.helper)
    );
    let probe = succeeds(EnvClearExplicitBuilder::for_system(
        input(source),
        LaunchPurpose::EffectiveConfig,
    ));
    assert!(!probe.entries().any(|e| matches!(
        e.key,
        "SSH_ASKPASS" | "SSH_ASKPASS_REQUIRE" | "SHELL" | "COMSPEC" | "GUO_SYSTEM_ENDPOINT"
    )));
}

struct ExactSelection<'a> {
    paths: [&'a str; 2],
    binding: SourceBinding,
}
// SAFETY: 仅 synthetic 显选集合，测试不允许对这些路径启动真实进程。
unsafe impl SelectedFileAccess for ExactSelection<'_> {
    fn permits_ssh_reopen(&self, path: &str, binding: SourceBinding) -> bool {
        self.binding == binding && self.paths.contains(&path)
    }
}

#[test]
fn system_identity_is_exact_selected_pair_not_saved_path_or_directory_permission() {
    let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
    let source = succeeds(root.open_source(1, 0));
    let mut value = input(source);
    #[cfg(windows)]
    let files = [
        "C:\\selected\\%h${HOME}~钥匙 key",
        "C:\\selected\\%h${HOME}~钥匙 key.pub",
    ];
    #[cfg(not(windows))]
    let files = [
        "/selected/%h${HOME}~钥匙 key",
        "/selected/%h${HOME}~钥匙 key.pub",
    ];
    let selection = ExactSelection {
        paths: files,
        binding: value.binding,
    };
    let identity = succeeds(ExternalIdentity::selected(files[0], files[1], &selection));
    assert_eq!(identity.exact_files(), files);
    value.auth = SystemAuth::ExternalFile(identity);
    let image = succeeds(ExeIdentity::test_only_trusted_codec(PRIVATE.helper));
    let owned = succeeds(
        succeeds(BorrowedShape::checked(
            value,
            &image,
            LaunchPurpose::Session,
        ))
        .admit(&root),
    );
    assert!(
        owned
            .environment()
            .any(|(name, path)| name == "GUO_SYSTEM_IDENTITY" && path == files[0])
    );
    assert!(!owned.args().any(|arg| arg == files[0]));
    value.binding.attempt += 1;
    assert_eq!(value.validate(), Err(ScopeReject::FilePermissionNotProven));
    assert_eq!(
        ExternalIdentity::selected(files[0], PRIVATE.empty_certificate, &selection).err(),
        Some(ScopeReject::ExactFileSelectionRequired)
    );
}

#[test]
fn system_target_boundaries_are_not_local_shell_parsing() {
    let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
    let source = succeeds(root.open_source(1, 0));
    let mut value = input(source);
    for host in [
        "-oProxyCommand=x",
        "user@host",
        "ssh://host",
        "host\nInclude x",
        "host;echo",
        "[::1]",
        "a/b",
        "${HOME}",
        "a..b",
    ] {
        value.host = host;
        assert!(value.validate().is_err());
    }
    for host in [
        "example.invalid",
        "127.0.0.1",
        "::1",
        "fe80::1%eth0",
        "主机.invalid",
    ] {
        value.host = host;
        succeeds(value.validate());
    }
    value.host = "example.invalid";
    for bad in ["", "-lroot", "user\0name", "user\rname"] {
        value.username = bad;
        assert!(value.validate().is_err());
    }
    value.username = "用户名%${HOME} ' \"";
    value.remote_command = Some("-leading -- $HOME %h ~ ' \"; 远端命令");
    succeeds(value.validate());
    value.remote_command = Some("line\ncommand");
    assert_eq!(value.validate(), Err(ScopeReject::ControlCharacter));
    value.remote_command = None;
    value.port = 0;
    assert_eq!(value.validate(), Err(ScopeReject::InvalidPort));
}

#[test]
fn system_helper_path_controls_reject_before_any_owned_encoding() {
    let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
    let source = succeeds(root.open_source(1, 0));
    let image = succeeds(ExeIdentity::test_only_trusted_codec(PRIVATE.helper));
    let mut value = input(source);
    let audit = audit::AuditScope::start(&root);
    for helper in ["helper\rpath", "helper\npath", "helper\0path"] {
        value.paths.helper = helper;
        assert_eq!(
            BorrowedShape::checked(value, &image, LaunchPurpose::Session).err(),
            Some(ScopeReject::ControlCharacter)
        );
    }
    assert!(audit.finish().events().is_empty());
    assert_eq!(root.snapshot().rust_bytes, 0);
}

#[test]
fn system_crt_encoding_roundtrips_exact_tokens() {
    // 独立 CRT 反向模型；真实 -G 子进程还验证目标/username 的接收边界。
    fn decode(units: &[u16]) -> String {
        let mut result = Vec::new();
        let mut i = 1;
        while i + 1 < units.len() {
            let mut n = 0;
            while units[i] == b'\\' as u16 {
                n += 1;
                i += 1;
            }
            if units[i] == b'"' as u16 {
                result.extend(std::iter::repeat_n(b'\\' as u16, n / 2));
                if n % 2 == 1 {
                    result.push(b'"' as u16);
                }
            } else {
                result.extend(std::iter::repeat_n(b'\\' as u16, n));
                result.push(units[i]);
            }
            i += 1;
        }
        succeeds(String::from_utf16(&result))
    }
    for value in [
        "",
        "plain",
        "空格 Unicode 😃",
        "%h${HOME}~",
        "quote\"x",
        "end\\",
        "\\\\\"tail\\\\",
        "-oProxyCommand=x",
    ] {
        let mut units = Vec::new();
        succeeds(windows_crt_argument(value, |u| {
            units.push(u);
            Ok(())
        }));
        assert_eq!(decode(&units), value);
    }
}

#[test]
fn system_query_and_none_probe_do_not_claim_actual_hook_or_pty() {
    assert_eq!(
        ActualVersion::parse(b"wrapper\n").err(),
        Some(ScopeReject::VersionNotOpenSsh)
    );
    assert_eq!(
        succeeds(ActualVersion::parse(
            b"OpenSSH_for_Windows_9.5p1, LibreSSL 3.8.2\r\n"
        ))
        .line(),
        "OpenSSH_for_Windows_9.5p1, LibreSSL 3.8.2"
    );
    assert_eq!(
        HookAttributes::default().post_auth_shell_chain,
        EvidenceLevel::StaticDeclared
    );
    assert_eq!(
        SystemProgress::Handshake.observe(AuthObservation::NoneMethodProbe),
        SystemProgress::Handshake
    );
    assert_eq!(
        SystemProgress::Handshake.observe(AuthObservation::CredentialRequested),
        SystemProgress::AwaitingCredential
    );
    assert_eq!(
        SystemProgress::AwaitingCredential.observe(AuthObservation::PostAuthCallback),
        SystemProgress::AuthenticatedRequestingTerminal
    );
    assert_eq!(
        SystemProgress::Ended.observe(AuthObservation::PostAuthCallback),
        SystemProgress::Ended
    );
}

#[test]
fn system_unclosed_parent_source_and_hook_evidence_cannot_get_a_spawn_grant() {
    struct Unclosed;
    // SAFETY: 永不声明闭合/匹配；不能构造任何正向生产证书。
    unsafe impl ActualProcessSourceClosure for Unclosed {
        fn matches(&self, _: &ExeIdentity<'_>, _: SourceBinding) -> bool {
            false
        }
        fn actual_layout(&self) -> ActualProcessLayout {
            unreachable!("未闭合的来源不得消费布局")
        }
    }
    // SAFETY: 无实际进程来源；matches 总为 false，不能发布认证能力。
    unsafe impl ParentProcessProof for Unclosed {
        fn matches(&self, _: &ExeIdentity<'_>, _: SourceBinding) -> bool {
            false
        }
        fn hooks(&self) -> HookAttributes {
            HookAttributes::default()
        }
    }
    let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
    let source = succeeds(root.open_source(1, 0));
    let value = input(source);
    let image = succeeds(ExeIdentity::test_only_trusted_codec(PRIVATE.helper));
    let audit = audit::AuditScope::start(&root);
    assert_eq!(
        admit_actual_process(&root, &image, value.binding, &Unclosed).err(),
        Some(ScopeReject::SourceClosureRequired)
    );
    assert_eq!(
        require_session_hooks(&image, value.binding, value.auth, &Unclosed),
        Err(ScopeReject::ImageChanged)
    );
    assert!(audit.finish().events().is_empty());
    assert_eq!(root.snapshot().rust_bytes, 0);
    for state in [
        SystemProgress::Starting,
        SystemProgress::AwaitingHostDecision,
    ] {
        assert_eq!(state.observe(AuthObservation::NoneMethodProbe), state);
    }
}

#[cfg(windows)]
mod actual_windows {
    use std::fs;
    use std::io::{Read, Write};
    use std::net::{Shutdown, TcpListener, TcpStream};
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::{
        DUPLICATE_SAME_ACCESS, DuplicateHandle, ERROR_BROKEN_PIPE, GetLastError, HANDLE,
        HANDLE_FLAG_INHERIT, SetHandleInformation, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::Storage::FileSystem::ReadFile;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
        QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
    };
    use windows_sys::Win32::System::Pipes::{CreatePipe, PeekNamedPipe};
    use windows_sys::Win32::System::SystemInformation::{
        GetSystemDirectoryW, GetSystemWindowsDirectoryW,
    };
    use windows_sys::Win32::System::Threading::{
        CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
        DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess,
        GetExitCodeProcess, InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
        PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROCESS_INFORMATION, ResumeThread, STARTF_USESTDHANDLES,
        STARTUPINFOEXW, UpdateProcThreadAttribute, WaitForSingleObject,
    };

    use super::{
        ActualVersion, BYTE_LIMIT, BorrowedShape, BorrowedSystem, CLASS_COUNT, DesktopPlatform,
        EvidenceLevel, ExactSelection, ExeIdentity, ExternalIdentity, LaunchPurpose,
        PrivateAttemptFiles, PrivatePaths, ProcessLedger, ScopeReject, SourceBinding, SystemAuth,
        TrustedOsPaths, audit, ledger, succeeds,
    };
    use crate::scope::{
        CandidateOrigin, EffectiveQuery, ImageStamp, LocatorCandidate, LocatorPaths, OwnedLaunch,
        ReadOnlyParentImageSource,
    };

    const OUTPUT_LIMIT: usize = 32 * 1024;

    // 仅在本轮 Temp 根编译的无秘密 helper；没有网络、认证、信任记录或任意命令入口。
    // 其 Rust runtime/SDK 编译过程和 OS/OpenSSH backing 不属于 app codec H，也未取得生产证书。
    const ORDER_HELPER_SOURCE: &str = r#"
use std::io::Write;
fn field(file: &mut std::fs::File, bytes: &[u8]) -> std::io::Result<()> {
    let len = u32::try_from(bytes.len()).map_err(|_| std::io::ErrorKind::InvalidInput)?;
    file.write_all(&len.to_le_bytes())?;
    file.write_all(bytes)
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 6 || args[1] != "host-key" || args[2] != "ORDER" {
        return Err("unsupported role".into());
    }
    if std::env::var("GUO_SYSTEM_ENDPOINT")?.as_str() != "synthetic-query-endpoint" {
        return Err("wrong fixture endpoint".into());
    }
    let mut file = std::fs::OpenOptions::new().create_new(true).write(true)
        .open("order-callback.bin")?;
    file.write_all(b"ORDERCB1")?;
    file.write_all(&std::process::id().to_le_bytes())?;
    field(&mut file, std::env::current_exe()?.to_str().ok_or("non unicode image")?.as_bytes())?;
    for arg in &args { field(&mut file, arg.as_bytes())?; }
    file.flush()?;
    Ok(())
}
fn main() {
    if run().is_err() { std::process::exit(71); }
}
"#;

    /// SDK 仅用于合成 fixture，不是 ssh 的发现/环境旁路。调用者给出已安装工具的精确路径。
    fn compile_order_helper(fixture: &Fixture, os: &OsSource) {
        let rustc = std::env::var_os("GUO_SCOPE_FIXTURE_RUSTC")
            .unwrap_or_else(|| panic!("UNVERIFIED：需既有 GNU 1.98 rustc 的精确 fixture 工具路径"));
        let linker = std::env::var_os("GUO_SCOPE_FIXTURE_LINKER")
            .unwrap_or_else(|| panic!("UNVERIFIED：需既有 GNU linker 的精确 fixture 工具路径"));
        assert!(Path::new(&rustc).is_absolute() && Path::new(&rustc).is_file());
        assert!(Path::new(&linker).is_absolute() && Path::new(&linker).is_file());
        let tool_bin = Path::new(&linker)
            .parent()
            .unwrap_or_else(|| panic!("linker 必须有绝对父目录"));
        let source = fixture.directory.join("order-helper.rs");
        succeeds(fs::write(&source, ORDER_HELPER_SOURCE));
        let status = succeeds(
            std::process::Command::new(rustc)
                .env_clear()
                .env("RUSTUP_AUTO_INSTALL", "0")
                .env("SystemRoot", &os.windows)
                .env("PATH", tool_bin)
                .env("HOME", &fixture.home)
                .env("USERPROFILE", &fixture.home)
                .env("TEMP", &fixture.directory)
                .env("TMP", &fixture.directory)
                .current_dir(&fixture.directory)
                .arg("--edition=2024")
                .args([
                    "--crate-name",
                    "owned_order_helper",
                    "--target",
                    "x86_64-pc-windows-gnu",
                ])
                .arg("-C")
                .arg(format!("linker={}", linker.to_string_lossy()))
                .args(["-C", "link-self-contained=yes"])
                .arg(&source)
                .arg("-o")
                .arg(&fixture.helper)
                .status(),
        );
        assert!(
            status.success(),
            "本轮 synthetic helper 编译失败：{:?}",
            status.code()
        );
    }

    const SERVER_IO_LIMIT: Duration = Duration::from_secs(3);

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum ServerPhase {
        Starting,
        Accept,
        Identification,
        Kex,
        Finished,
    }

    struct ServerControl {
        stop: AtomicBool,
        wake: AtomicBool,
        socket: Mutex<Option<TcpStream>>,
        phase: Mutex<ServerPhase>,
        changed: Condvar,
    }
    impl ServerControl {
        fn phase(&self, phase: ServerPhase) {
            *self
                .phase
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = phase;
            self.changed.notify_all();
        }
        fn wait_phase(&self, expected: ServerPhase) {
            let phase = self
                .phase
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let (phase, _) = self
                .changed
                .wait_timeout_while(phase, SERVER_IO_LIMIT, |phase| *phase != expected)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(*phase, expected, "fixture 阶段屏障未到达");
        }
        fn request_stop(&self, thread: &std::thread::Thread) -> Result<(), std::io::ErrorKind> {
            self.stop.store(true, Ordering::Release);
            self.wake.store(true, Ordering::Release);
            thread.unpark();
            let socket = self
                .socket
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            if let Some(socket) = socket {
                match socket.shutdown(Shutdown::Both) {
                    Ok(()) => (),
                    Err(error) if error.kind() == std::io::ErrorKind::NotConnected => (),
                    Err(error) => return Err(error.kind()),
                }
            }
            Ok(())
        }
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum ServerFailure {
        Io(std::io::ErrorKind),
        ThreadPanicked,
    }

    struct IdentificationServer {
        control: Arc<ServerControl>,
        thread: Option<std::thread::JoinHandle<Result<(bool, bool), std::io::ErrorKind>>>,
    }
    impl IdentificationServer {
        fn start() -> (Self, TcpListener) {
            let listener = succeeds(TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)));
            let for_thread = succeeds(listener.try_clone());
            succeeds(for_thread.set_nonblocking(true));
            let control = Arc::new(ServerControl {
                stop: AtomicBool::new(false),
                wake: AtomicBool::new(false),
                socket: Mutex::new(None),
                phase: Mutex::new(ServerPhase::Starting),
                changed: Condvar::new(),
            });
            let for_thread_control = Arc::clone(&control);
            let thread = std::thread::spawn(move || {
                let result = Self::serve(for_thread, &for_thread_control);
                drop(
                    for_thread_control
                        .socket
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .take(),
                );
                for_thread_control.phase(ServerPhase::Finished);
                result
            });
            (
                Self {
                    control,
                    thread: Some(thread),
                },
                listener,
            )
        }

        fn serve(
            listener: TcpListener,
            control: &ServerControl,
        ) -> Result<(bool, bool), std::io::ErrorKind> {
            let deadline = Instant::now() + SERVER_IO_LIMIT;
            control.phase(ServerPhase::Accept);
            let (mut stream, peer) = loop {
                if control.stop.load(Ordering::Acquire) {
                    return Ok((false, false));
                }
                match listener.accept() {
                    Ok(peer) => break peer,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            return Err(std::io::ErrorKind::TimedOut);
                        }
                        // park 可被 stop 立即唤醒；短轮询只消费 socket readiness，不延长既有 I/O 上限。
                        std::thread::park_timeout(Duration::from_millis(10));
                    }
                    Err(error) => return Err(error.kind()),
                }
            };
            if !peer.ip().is_loopback() {
                return Err(std::io::ErrorKind::PermissionDenied);
            }
            stream
                .set_nonblocking(false)
                .map_err(|error| error.kind())?;
            stream
                .set_read_timeout(Some(SERVER_IO_LIMIT))
                .map_err(|error| error.kind())?;
            stream
                .set_write_timeout(Some(SERVER_IO_LIMIT))
                .map_err(|error| error.kind())?;
            *control
                .socket
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(stream.try_clone().map_err(|error| error.kind())?);
            if control.stop.load(Ordering::Acquire) {
                return Ok((false, false));
            }
            control.phase(ServerPhase::Identification);
            let mut ident = [0; 1024];
            let mut count = 0;
            let ident_deadline = Instant::now() + SERVER_IO_LIMIT;
            loop {
                if control.stop.load(Ordering::Acquire) {
                    return Ok((false, false));
                }
                let remaining = ident_deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(std::io::ErrorKind::TimedOut);
                }
                stream
                    .set_read_timeout(Some(remaining))
                    .map_err(|error| error.kind())?;
                match stream.read(&mut ident[count..count + 1]) {
                    Ok(0) => return Ok((false, false)),
                    Ok(_) => {
                        count += 1;
                        if ident[count - 1] == b'\n' {
                            break;
                        }
                        if count == ident.len() {
                            return Err(std::io::ErrorKind::InvalidData);
                        }
                    }
                    Err(_) if control.stop.load(Ordering::Acquire) => return Ok((false, false)),
                    Err(error) => return Err(error.kind()),
                }
            }
            if !ident[..count].starts_with(b"SSH-2.0-OpenSSH_for_Windows_9.5") {
                return Err(std::io::ErrorKind::InvalidData);
            }
            if let Err(error) = stream.write_all(b"SSH-2.0-GuoOwnedIdentificationFixture\r\n") {
                return if control.stop.load(Ordering::Acquire) {
                    Ok((true, false))
                } else {
                    Err(error.kind())
                };
            }
            control.phase(ServerPhase::Kex);
            let mut prefix = [0; 6];
            let mut read = 0;
            let kex_deadline = Instant::now() + SERVER_IO_LIMIT;
            while read < prefix.len() {
                if control.stop.load(Ordering::Acquire) {
                    return Ok((true, false));
                }
                let remaining = kex_deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(std::io::ErrorKind::TimedOut);
                }
                stream
                    .set_read_timeout(Some(remaining))
                    .map_err(|error| error.kind())?;
                match stream.read(&mut prefix[read..]) {
                    Ok(0) => return Ok((true, false)),
                    Ok(count) => read += count,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::ConnectionReset
                            || control.stop.load(Ordering::Acquire) =>
                    {
                        return Ok((true, false));
                    }
                    Err(error) => return Err(error.kind()),
                }
            }
            let packet_len = u32::from_be_bytes([prefix[0], prefix[1], prefix[2], prefix[3]]);
            if !(2..=65_536).contains(&packet_len) || prefix[5] != 20 {
                return Err(std::io::ErrorKind::InvalidData);
            }
            // 不发送服务器 KEXINIT/host key/signature，也不实现 userauth；关闭此自有 socket。
            Ok((true, true))
        }
        fn finish(
            &mut self,
            before_wait: impl FnOnce(&ServerControl, &std::thread::Thread),
        ) -> Result<(bool, bool), ServerFailure> {
            let close = self.control.request_stop(
                self.thread
                    .as_ref()
                    .unwrap_or_else(|| panic!("fixture thread 只能收割一次"))
                    .thread(),
            );
            let thread = self
                .thread
                .take()
                .unwrap_or_else(|| panic!("fixture thread 只能收割一次"));
            before_wait(&self.control, thread.thread());
            let result = thread.join();
            drop(
                self.control
                    .socket
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take(),
            );
            close.map_err(ServerFailure::Io)?;
            result
                .map_err(|_| ServerFailure::ThreadPanicked)?
                .map_err(ServerFailure::Io)
        }
        fn join(mut self) -> Result<(bool, bool), ServerFailure> {
            self.finish(|_, _| {})
        }
    }
    impl Drop for IdentificationServer {
        fn drop(&mut self) {
            if self.thread.is_some()
                && let Err(error) = self.finish(|_, _| {})
            {
                eprintln!("fixture server 清理失败：{error:?}");
            }
        }
    }

    struct FixtureCompletion {
        primary: Result<(u32, usize), ScopeReject>,
        server: Result<(bool, bool), ServerFailure>,
    }
    fn complete_fixture_run(
        primary: Result<(u32, usize), ScopeReject>,
        listener: TcpListener,
        server: IdentificationServer,
    ) -> FixtureCompletion {
        drop(listener);
        FixtureCompletion {
            primary,
            server: server.join(),
        }
    }

    #[test]
    fn system_identification_explicit_join_requests_stop_before_wait_without_peer() {
        let (mut server, listener) = IdentificationServer::start();
        server.control.wait_phase(ServerPhase::Accept);
        let mut requested_before_wait = false;
        let result = server.finish(|control, thread| {
            requested_before_wait =
                control.stop.load(Ordering::Acquire) && control.wake.load(Ordering::Acquire);
            // 安全 observer 只在前件缺失时发出自有 stop，绝不真的无限 join 或杀线程。
            if !requested_before_wait {
                succeeds(control.request_stop(thread));
            }
        });
        drop(listener);
        drop(server);
        assert!(
            requested_before_wait,
            "explicit join 必须先请求 stop/wake，再等待 accept 中的 thread"
        );
        assert_eq!(result, Ok((false, false)));
    }

    #[test]
    fn system_identification_explicit_join_closes_blocked_ident_and_kex() {
        for phase in [ServerPhase::Identification, ServerPhase::Kex] {
            let (server, listener) = IdentificationServer::start();
            let control = Arc::clone(&server.control);
            let mut peer = succeeds(TcpStream::connect(succeeds(listener.local_addr())));
            if phase == ServerPhase::Kex {
                succeeds(peer.write_all(b"SSH-2.0-OpenSSH_for_Windows_9.5p2\r\n"));
            }
            control.wait_phase(phase);
            let joined = server.join();
            assert_eq!(joined, Ok((phase == ServerPhase::Kex, false)));
            control.wait_phase(ServerPhase::Finished);
            assert!(
                control
                    .socket
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .is_none()
            );
            drop(peer);
            drop(listener);
            drop(control);
        }
    }

    #[test]
    fn system_identification_drop_uses_same_stop_wake_and_joins_without_peer() {
        let (server, listener) = IdentificationServer::start();
        let control = Arc::clone(&server.control);
        control.wait_phase(ServerPhase::Accept);
        drop(server);
        assert!(control.stop.load(Ordering::Acquire) && control.wake.load(Ordering::Acquire));
        control.wait_phase(ServerPhase::Finished);
        assert!(
            control
                .socket
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_none()
        );
        drop(listener);
        drop(control);
    }

    #[test]
    fn system_identification_cleanup_error_keeps_original_run_failure() {
        let (server, listener) = IdentificationServer::start();
        let control = Arc::clone(&server.control);
        let mut peer = succeeds(TcpStream::connect(succeeds(listener.local_addr())));
        succeeds(peer.write_all(b"invalid-owned-identification\r\n"));
        control.wait_phase(ServerPhase::Finished);
        let completed = complete_fixture_run(Err(ScopeReject::Io), listener, server);
        assert_eq!(completed.primary, Err(ScopeReject::Io));
        assert_eq!(
            completed.server,
            Err(ServerFailure::Io(std::io::ErrorKind::InvalidData))
        );
        assert!(
            control
                .socket
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_none()
        );
        drop(peer);
        drop(control);
    }

    #[test]
    fn system_identification_no_connection_run_error_and_nonzero_complete_full_cleanup() {
        let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
        let source = succeeds(root.open_source(1, 0));
        let mother = succeeds(root.try_reserve(
            ledger::Amount {
                rust_bytes: 512 * 1024,
                c_bytes: 0,
                slots: 1,
            },
            ledger::ResourceClass::Source,
            ledger::Admission {
                epoch: 1,
                source: Some(source),
            },
        ));
        let fixture = Fixture::create();
        let os = OsSource {
            windows: os_path(GetSystemWindowsDirectoryW),
        };
        let program = format!("{}\\OpenSSH\\ssh.exe", os_path(GetSystemDirectoryW));
        let search_paths = [program.as_str()];
        let candidate = succeeds(LocatorCandidate::locate(LocatorPaths {
            rshell_ssh: None,
            absolute_search_candidates: &search_paths,
        }));
        let image_source = SystemImageSource {
            path: &program,
            active: AtomicBool::new(true),
        };
        let image = succeeds(ExeIdentity::read_locked(candidate, &image_source));
        // 未建立 config 的自有 cold fixture 用于 ssh -F 的 pre-connect 非零退出，不回落全局配置。
        assert!(!Path::new(&fixture.config).exists());
        for prelaunch_error in [true, false] {
            let (server, listener) = IdentificationServer::start();
            let control = Arc::clone(&server.control);
            control.wait_phase(ServerPhase::Accept);
            let value = BorrowedSystem {
                host: "127.0.0.1",
                port: succeeds(listener.local_addr()).port(),
                username: "order-probe",
                remote_command: None,
                term: "xterm-256color",
                auth: SystemAuth::Password,
                paths: fixture.paths(),
                platform: DesktopPlatform::Windows,
                os: &os,
                private_files: &fixture,
                binding: SourceBinding {
                    source,
                    attempt: 31,
                    target_identity: 37,
                },
            };
            let launch = succeeds(
                succeeds(BorrowedShape::checked(
                    value,
                    &image,
                    LaunchPurpose::Session,
                ))
                .admit(&root),
            );
            let mut output = [0; OUTPUT_LIMIT];
            let result = run_fixture_process(
                &launch,
                &image,
                if prelaunch_error {
                    None
                } else {
                    Some(&listener)
                },
                &mut output,
            );
            let completed = complete_fixture_run(result, listener, server);
            if prelaunch_error {
                assert_eq!(completed.primary, Err(ScopeReject::SourceClosureRequired));
            } else {
                assert!(matches!(completed.primary, Ok((code, _)) if code != 0));
            }
            assert_eq!(
                completed.server,
                Ok((false, false)),
                "无连接／wake 不得冒充 peer identification 或 KEX"
            );
            control.wait_phase(ServerPhase::Finished);
            assert!(control.stop.load(Ordering::Acquire) && control.wake.load(Ordering::Acquire));
            assert!(
                control
                    .socket
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .is_none()
            );
            drop(launch);
            drop(control);
        }
        drop(image);
        fixture.cleanup();
        drop(os);
        drop(program);
        drop(mother);
        assert_eq!(root.snapshot().rust_bytes, 0);
        eprintln!(
            "NO_CONNECT_COMPLETED：prelaunch Err 与实际 ssh -F 非零两流程均已 stop/wake、真实 join、无 peer identification，私有资源已 exact delete；不取得生产 source 证书"
        );
    }

    struct OsSource {
        windows: String,
    }
    // SAFETY: GetSystemWindowsDirectoryW 的只读返回值，不读取用户 SystemRoot 环境。
    unsafe impl TrustedOsPaths for OsSource {
        fn system_root(&self) -> Option<&str> {
            Some(&self.windows)
        }
    }

    fn os_path(query: unsafe extern "system" fn(*mut u16, u32) -> u32) -> String {
        let mut wide = [0; 32_768];
        let count = unsafe { query(wide.as_mut_ptr(), wide.len() as u32) } as usize;
        assert!(count > 0 && count < wide.len());
        succeeds(String::from_utf16(&wide[..count]))
    }

    struct SystemImageSource<'a> {
        path: &'a str,
        active: std::sync::atomic::AtomicBool,
    }
    // SAFETY: fixture 只使用 OS API 系统目录内现有 ssh；Windows read handle 禁止并发写/替换。
    // 本构造不提供签名/沙箱/认证证据，不能被生产引用。
    unsafe impl ReadOnlyParentImageSource for SystemImageSource<'_> {
        fn trusted_readonly_image(&self, path: &str, _: ImageStamp) -> bool {
            path == self.path && self.active.load(std::sync::atomic::Ordering::Acquire)
        }
    }

    struct Fixture {
        directory: PathBuf,
        home: String,
        config: String,
        certificate: String,
        provider: String,
        helper: String,
        selected: String,
        selected_pub: String,
        cleaned: bool,
    }

    impl Fixture {
        fn create() -> Self {
            let parent = std::env::temp_dir();
            assert!(parent.is_dir());
            let directory = parent.join(format!(
                "guo-system-scope-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
            succeeds(fs::create_dir(&directory));
            fn string(path: PathBuf) -> String {
                succeeds(path.into_os_string().into_string())
            }
            let home = string(directory.join("home"));
            let config = string(directory.join("应用 config % $.conf"));
            let certificate = string(directory.join("empty-cert"));
            let provider = string(directory.join("empty-provider"));
            let helper = string(directory.join("固定 %64 % ${HOME} 工具.exe"));
            let selected = string(directory.join("selected %h${HOME}~ key"));
            let selected_pub = format!("{selected}.pub");
            let fixture = Self {
                directory,
                home,
                config,
                certificate,
                provider,
                helper,
                selected,
                selected_pub,
                cleaned: false,
            };
            succeeds(fs::create_dir(&fixture.home));
            succeeds(fs::create_dir(Path::new(&fixture.home).join(".ssh")));
            // 全部是本轮合成文件；没有触碰真实 ~/.ssh 或系统配置。
            succeeds(fs::write(
                Path::new(&fixture.home).join(".ssh/config"),
                b"Host *\n User decoy-user\n ProxyCommand decoy-never-run\n",
            ));
            for path in [
                &fixture.certificate,
                &fixture.provider,
                &fixture.selected,
                &fixture.selected_pub,
            ] {
                succeeds(fs::write(path, b""));
            }
            fixture
        }
        fn paths(&self) -> PrivatePaths<'_> {
            PrivatePaths {
                cwd: self.directory.to_str().unwrap_or(""),
                home: &self.home,
                config: &self.config,
                empty_certificate: &self.certificate,
                empty_provider: &self.provider,
                helper: &self.helper,
                endpoint: "synthetic-query-endpoint",
            }
        }
        fn cleanup(mut self) {
            // 只删除本轮登记的 exact directory；句柄/进程在调用前已全部收割。
            succeeds(fs::remove_dir_all(&self.directory));
            self.cleaned = true;
        }
    }
    // SAFETY: 该 fixture 只拥有本轮唯一目录及合成文件；不用于生产来源/权限证书。
    unsafe impl PrivateAttemptFiles for Fixture {
        fn owns_private_attempt(&self, paths: PrivatePaths<'_>, _: SourceBinding) -> bool {
            !self.cleaned
                && Some(paths.cwd) == self.directory.to_str()
                && paths.config == self.config
                && paths.home == self.home
                && paths.empty_certificate == self.certificate
                && paths.empty_provider == self.provider
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            if !self.cleaned
                && let Err(error) = fs::remove_dir_all(&self.directory)
            {
                panic!("本轮 fixture 清理失败：{:?}", error.kind());
            }
        }
    }

    struct Handles {
        process: Option<OwnedHandle>,
        thread: Option<OwnedHandle>,
        job: OwnedHandle,
        reaped: bool,
    }
    impl Handles {
        fn wait(&mut self, timeout: u32) -> bool {
            let Some(process) = self.process.as_ref() else {
                return true;
            };
            self.reaped = unsafe { WaitForSingleObject(process.as_raw_handle().cast(), timeout) }
                == WAIT_OBJECT_0;
            self.reaped
        }
        fn wait_tree_empty(&self) -> bool {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION =
                    unsafe { std::mem::zeroed() };
                assert_ne!(
                    unsafe {
                        QueryInformationJobObject(
                            self.job.as_raw_handle().cast(),
                            JobObjectBasicAccountingInformation,
                            (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                            std::mem::size_of_val(&info) as u32,
                            std::ptr::null_mut(),
                        )
                    },
                    0
                );
                if info.ActiveProcesses == 0 {
                    return true;
                }
                if Instant::now() >= deadline {
                    return false;
                }
                std::thread::yield_now();
            }
        }
    }
    impl Drop for Handles {
        fn drop(&mut self) {
            if self.process.is_some() && (!self.reaped || !self.wait_tree_empty()) {
                let terminated =
                    unsafe { TerminateJobObject(self.job.as_raw_handle().cast(), 1) } != 0;
                let waited = self.wait(5000);
                assert!(
                    terminated && waited && self.wait_tree_empty(),
                    "本轮 fixture 树终止/收割失败"
                );
            }
            drop(self.thread.take());
            drop(self.process.take());
        }
    }

    fn owned(handle: HANDLE) -> OwnedHandle {
        assert!(!handle.is_null());
        unsafe { OwnedHandle::from_raw_handle(handle.cast()) }
    }

    fn pipe() -> (OwnedHandle, OwnedHandle) {
        let mut read = std::ptr::null_mut();
        let mut write = std::ptr::null_mut();
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: std::ptr::null_mut(),
            bInheritHandle: 1,
        };
        assert_ne!(
            unsafe { CreatePipe(&mut read, &mut write, &attributes, 0) },
            0
        );
        (owned(read), owned(write))
    }

    /// 实际应用侧调用链：固定栈缓冲 + windows-sys 直接 FFI，无 std Command/env map/
    /// portable CommandBuilder/线程/异步 future。CreateProcess/CRT/OpenSSH/OS 内部不计作零。
    /// Rust H 只有先准入的 OwnedLaunch backing；固定栈和内核对象属于明确排除的 OS 界。
    fn query(
        launch: &OwnedLaunch<'_>,
        image: &ExeIdentity<'_>,
        output: &mut [u8; OUTPUT_LIMIT],
    ) -> Result<(u32, usize), ScopeReject> {
        run_fixture_process(launch, image, None, output)
    }

    /// 只接自有 identification listener 的候选许可，不实现 ActualProcessSourceClosure。
    /// 配置查询与 ORDER fixture 均不认证、不提交信任；这不是生产 Session spawn API。
    fn run_fixture_process(
        launch: &OwnedLaunch<'_>,
        image: &ExeIdentity<'_>,
        identification: Option<&TcpListener>,
        output: &mut [u8; OUTPUT_LIMIT],
    ) -> Result<(u32, usize), ScopeReject> {
        image.revalidate()?;
        if launch.record().purpose == LaunchPurpose::Session {
            let listener = identification.ok_or(ScopeReject::SourceClosureRequired)?;
            let address = listener.local_addr().map_err(|_| ScopeReject::Io)?;
            if address.ip() != std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
                || launch
                    .args()
                    .nth(5)
                    .and_then(|port| port.parse::<u16>().ok())
                    != Some(address.port())
                || launch.args().nth(8) != Some("127.0.0.1")
                || launch.args().count() != 9
            {
                return Err(ScopeReject::SourceClosureRequired);
            }
        }
        let record = launch.record();
        let application = launch.windows_units(record.windows_application)?;
        let cwd = launch.windows_units(record.windows_cwd)?;
        let environment = launch.windows_units(record.windows_environment)?;
        let command = launch.windows_units(record.windows_command)?;
        let mut mutable_command = [0; 32_768];
        mutable_command[..command.len()].copy_from_slice(command);
        let (reader, writer) = pipe();
        let mut stderr_handle = std::ptr::null_mut();
        assert_ne!(
            unsafe {
                DuplicateHandle(
                    GetCurrentProcess(),
                    writer.as_raw_handle().cast(),
                    GetCurrentProcess(),
                    &mut stderr_handle,
                    0,
                    1,
                    DUPLICATE_SAME_ACCESS,
                )
            },
            0
        );
        let stderr_writer = owned(stderr_handle);
        let (stdin_reader, stdin_writer) = pipe();
        assert_ne!(
            unsafe { SetHandleInformation(reader.as_raw_handle().cast(), HANDLE_FLAG_INHERIT, 0) },
            0
        );
        assert_ne!(
            unsafe {
                SetHandleInformation(stdin_writer.as_raw_handle().cast(), HANDLE_FLAG_INHERIT, 0)
            },
            0
        );
        // 预先固定 attribute backing；不足就拒绝，不回退继承全部 handles。
        #[repr(align(16))]
        struct AttributeBacking([u8; 512]);
        let mut backing = AttributeBacking([0; 512]);
        let mut needed = 0;
        unsafe { InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut needed) };
        if needed > backing.0.len() {
            return Err(ScopeReject::SourceClosureRequired);
        }
        let attributes = backing.0.as_mut_ptr().cast() as LPPROC_THREAD_ATTRIBUTE_LIST;
        if unsafe { InitializeProcThreadAttributeList(attributes, 1, 0, &mut needed) } == 0 {
            return Err(ScopeReject::Io);
        }
        struct AttributeGuard(LPPROC_THREAD_ATTRIBUTE_LIST);
        impl Drop for AttributeGuard {
            fn drop(&mut self) {
                unsafe { DeleteProcThreadAttributeList(self.0) };
            }
        }
        let _attribute_guard = AttributeGuard(attributes);
        let mut inherited: [HANDLE; 3] = [
            stdin_reader.as_raw_handle().cast(),
            writer.as_raw_handle().cast(),
            stderr_writer.as_raw_handle().cast(),
        ];
        if unsafe {
            UpdateProcThreadAttribute(
                attributes,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                inherited.as_mut_ptr().cast(),
                std::mem::size_of_val(&inherited),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        } == 0
        {
            return Err(ScopeReject::Io);
        }
        let job = owned(unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) });
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if unsafe {
            SetInformationJobObject(
                job.as_raw_handle().cast(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
            )
        } == 0
        {
            return Err(ScopeReject::Io);
        }
        let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
        startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = inherited[0];
        startup.StartupInfo.hStdOutput = inherited[1];
        startup.StartupInfo.hStdError = inherited[2];
        startup.lpAttributeList = attributes;
        let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe {
            CreateProcessW(
                application.as_ptr(),
                mutable_command.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1,
                CREATE_NO_WINDOW
                    | CREATE_SUSPENDED
                    | CREATE_UNICODE_ENVIRONMENT
                    | EXTENDED_STARTUPINFO_PRESENT,
                environment.as_ptr().cast(),
                cwd.as_ptr(),
                &startup.StartupInfo,
                &mut info,
            )
        } == 0
        {
            return Err(ScopeReject::Io);
        }
        let mut child = Handles {
            process: Some(owned(info.hProcess)),
            thread: Some(owned(info.hThread)),
            job,
            reaped: false,
        };
        if unsafe { AssignProcessToJobObject(child.job.as_raw_handle().cast(), info.hProcess) } == 0
        {
            // 尚未 resume 的唯一 child，不杀同名用户进程；assignment 失败直接回收它。
            let stopped = unsafe {
                windows_sys::Win32::System::Threading::TerminateProcess(info.hProcess, 1)
            } != 0;
            assert!(stopped && child.wait(5000));
            return Err(ScopeReject::Io);
        }
        drop(writer);
        drop(stderr_writer);
        drop(stdin_reader);
        drop(stdin_writer);
        if unsafe { ResumeThread(info.hThread) } == u32::MAX {
            return Err(ScopeReject::Io);
        }
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut used = 0;
        let mut eof = false;
        while !eof || !child.reaped {
            let mut available = 0;
            if unsafe {
                PeekNamedPipe(
                    reader.as_raw_handle().cast(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    &mut available,
                    std::ptr::null_mut(),
                )
            } == 0
            {
                if unsafe { GetLastError() } != ERROR_BROKEN_PIPE {
                    return Err(ScopeReject::Io);
                }
                eof = true;
            }
            if available != 0 {
                let take = (available as usize).min(output.len() - used);
                if take == 0 {
                    return Err(ScopeReject::QueryTooLarge);
                }
                let mut read = 0;
                if unsafe {
                    ReadFile(
                        reader.as_raw_handle().cast(),
                        output[used..].as_mut_ptr(),
                        take as u32,
                        &mut read,
                        std::ptr::null_mut(),
                    )
                } == 0
                {
                    return Err(ScopeReject::Io);
                }
                used += read as usize;
            }
            child.wait(0);
            if Instant::now() >= deadline {
                return Err(ScopeReject::CleanupPending);
            }
            if !eof && available == 0 {
                child.wait(1);
            }
        }
        let mut code = 0;
        if unsafe { GetExitCodeProcess(info.hProcess, &mut code) } == 0 {
            return Err(ScopeReject::Io);
        }
        if !child.wait_tree_empty() {
            return Err(ScopeReject::CleanupPending);
        }
        drop(child);
        drop(reader);
        Ok((code, used))
    }

    #[test]
    fn system_current_windows_order_callback_executes_exact_percent_unicode_helper_path() {
        let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
        let source = succeeds(root.open_source(1, 0));
        // mother 保留 codec/fixture 生命周期；SDK/helper/thread/OS/OpenSSH 没有全集源证书。
        let mother = succeeds(root.try_reserve(
            ledger::Amount {
                rust_bytes: 512 * 1024,
                c_bytes: 0,
                slots: 1,
            },
            ledger::ResourceClass::Source,
            ledger::Admission {
                epoch: 1,
                source: Some(source),
            },
        ));
        let fixture = Fixture::create();
        let os = OsSource {
            windows: os_path(GetSystemWindowsDirectoryW),
        };
        let program = format!("{}\\OpenSSH\\ssh.exe", os_path(GetSystemDirectoryW));
        let search_paths = [program.as_str()];
        let candidate = succeeds(LocatorCandidate::locate(LocatorPaths {
            rshell_ssh: None,
            absolute_search_candidates: &search_paths,
        }));
        let image_source = SystemImageSource {
            path: &program,
            active: std::sync::atomic::AtomicBool::new(true),
        };
        let image = succeeds(ExeIdentity::read_locked(candidate, &image_source));
        let mut value = BorrowedSystem {
            host: "127.0.0.1",
            port: 22,
            username: "order-probe",
            remote_command: None,
            term: "xterm-256color",
            auth: SystemAuth::Password,
            paths: fixture.paths(),
            platform: DesktopPlatform::Windows,
            os: &os,
            private_files: &fixture,
            binding: SourceBinding {
                source,
                attempt: 23,
                target_identity: 29,
            },
        };
        let mut output = [0; OUTPUT_LIMIT];
        let version_launch = succeeds(
            succeeds(BorrowedShape::checked(
                value,
                &image,
                LaunchPurpose::Version,
            ))
            .admit(&root),
        );
        let (exit, count) = succeeds(query(&version_launch, &image, &mut output));
        assert_eq!(exit, 0);
        let version = succeeds(ActualVersion::parse(&output[..count]));
        assert!(
            version.line().starts_with("OpenSSH_for_Windows_9.5p2,"),
            "该回归需要当前实际 Windows 9.5p2"
        );
        eprintln!("ACTUAL_ORDER_VERSION {}", version.line());
        drop(version_launch);
        compile_order_helper(&fixture, &os);
        let (server, listener) = IdentificationServer::start();
        value.port = succeeds(listener.local_addr()).port();
        let shape = succeeds(BorrowedShape::checked(
            value,
            &image,
            LaunchPurpose::Session,
        ));
        let cost = shape.cost_model();
        let launch = succeeds(shape.admit(&root));
        succeeds(fs::write(&fixture.config, launch.config()));
        let result = run_fixture_process(&launch, &image, Some(&listener), &mut output);
        let completed = complete_fixture_run(result, listener, server);
        let (exit, _) = match completed.primary {
            Ok(value) => value,
            Err(primary) => panic!(
                "run_fixture_process primary={primary:?}；server cleanup={:?}",
                completed.server
            ),
        };
        let (identification, client_kex) = match completed.server {
            Ok(value) => value,
            Err(cleanup) => panic!(
                "run_fixture_process primary={:?}；server cleanup={cleanup:?}",
                completed.primary
            ),
        };
        assert_ne!(exit, 0, "fixture 在 KEX 完成前结束，不是认证或 PTY 成功");
        assert!(identification);
        let witness = fixture.directory.join("order-callback.bin");
        assert!(
            witness.is_file(),
            "实际 OpenSSH ORDER 必须执行精确含 %/空格/Unicode 的自有 helper av0"
        );
        let bytes = succeeds(fs::read(&witness));
        assert!(bytes.len() <= OUTPUT_LIMIT);
        assert_eq!(&bytes[..8], b"ORDERCB1");
        assert_ne!(u32::from_le_bytes(succeeds(bytes[8..12].try_into())), 0);
        let mut remaining = &bytes[12..];
        fn field<'a>(remaining: &mut &'a [u8]) -> &'a str {
            assert!(remaining.len() >= 4);
            let len = u32::from_le_bytes(succeeds(remaining[..4].try_into())) as usize;
            assert!(len <= remaining.len() - 4);
            let text = succeeds(std::str::from_utf8(&remaining[4..4 + len]));
            *remaining = &remaining[4 + len..];
            text
        }
        let observed_image = field(&mut remaining);
        let argv0 = field(&mut remaining);
        let expected_image = succeeds(fs::canonicalize(&fixture.helper));
        assert_eq!(succeeds(fs::canonicalize(observed_image)), expected_image);
        assert_eq!(succeeds(fs::canonicalize(argv0)), expected_image);
        assert_eq!(field(&mut remaining), "host-key");
        assert_eq!(field(&mut remaining), "ORDER");
        assert_eq!(field(&mut remaining), "127.0.0.1");
        assert_eq!(field(&mut remaining), "NONE");
        assert_eq!(field(&mut remaining), "NONE");
        assert!(remaining.is_empty() && client_kex);
        eprintln!(
            "ACTUAL_ORDER：精确 helper 映像/av0 与 %I/%H/%t/%K 已观察；codec H={} payload={} config={}；无认证、信任提交、PTY ACK 或生产源证书",
            cost.backing.backing.size(),
            cost.payload_bytes,
            cost.config_bytes
        );
        drop(bytes);
        drop(witness);
        drop(expected_image);
        drop(launch);
        drop(image);
        fixture.cleanup();
        drop(program);
        drop(os);
        drop(mother);
        assert_eq!(root.snapshot().rust_bytes, 0);
        eprintln!(
            "ORDER_TEMP_CLEAN：本轮 SDK 源/合成 exe/witness、socket、线程与 job 树已释放，随后 exact delete"
        );
    }

    #[test]
    fn system_existing_windows_ssh_version_and_closed_g_use_granted_explicit_blocks() {
        let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
        let source = succeeds(root.open_source(1, 0));
        // fixture 构造独立于产品 H；先保留 mother 额度，再建立自己拥有的临时资源。
        let mother = succeeds(root.try_reserve(
            ledger::Amount {
                rust_bytes: 512 * 1024,
                c_bytes: 0,
                slots: 1,
            },
            ledger::ResourceClass::Source,
            ledger::Admission {
                epoch: 1,
                source: Some(source),
            },
        ));
        let fixture = Fixture::create();
        let os = OsSource {
            windows: os_path(GetSystemWindowsDirectoryW),
        };
        let program = format!("{}\\OpenSSH\\ssh.exe", os_path(GetSystemDirectoryW));
        let search_paths = [program.as_str()];
        let candidate = match LocatorCandidate::locate(LocatorPaths {
            rshell_ssh: None,
            absolute_search_candidates: &search_paths,
        }) {
            Ok(candidate) => candidate,
            Err(ScopeReject::NotFound) => {
                eprintln!("UNVERIFIED：本机缺现有系统 ssh；未安装、未执行替代工具");
                fixture.cleanup();
                drop(os);
                drop(program);
                drop(mother);
                return;
            }
            Err(error) => panic!("定位失败：{error:?}"),
        };
        assert_eq!(candidate.path(), program);
        assert_eq!(candidate.origin(), CandidateOrigin::AbsoluteSearchEntry);
        assert_eq!(
            LocatorCandidate::locate(LocatorPaths {
                rshell_ssh: Some("ssh.exe"),
                absolute_search_candidates: &search_paths
            })
            .err(),
            Some(ScopeReject::InvalidOverride)
        );
        assert_eq!(
            LocatorCandidate::locate(LocatorPaths {
                rshell_ssh: Some(&fixture.helper),
                absolute_search_candidates: &search_paths
            })
            .err(),
            Some(ScopeReject::InvalidOverride)
        );
        let image_source = SystemImageSource {
            path: &program,
            active: std::sync::atomic::AtomicBool::new(true),
        };
        let image = succeeds(ExeIdentity::read_locked(candidate, &image_source));
        assert_eq!(image.origin(), candidate.origin());
        assert!(image.stamp().file_id != 0 && image.stamp().size > 64);
        let binding = SourceBinding {
            source,
            attempt: 13,
            target_identity: 17,
        };
        let value = BorrowedSystem {
            host: "example.invalid",
            port: 2222,
            username: "probe-user",
            remote_command: None,
            term: "xterm-256color",
            auth: SystemAuth::Password,
            paths: fixture.paths(),
            platform: DesktopPlatform::Windows,
            os: &os,
            private_files: &fixture,
            binding,
        };
        let mut output = [0; OUTPUT_LIMIT];
        let version_launch = succeeds(
            succeeds(BorrowedShape::checked(
                value,
                &image,
                LaunchPurpose::Version,
            ))
            .admit(&root),
        );
        let audit = audit::AuditScope::start(&root);
        let (code, count) = succeeds(query(&version_launch, &image, &mut output));
        assert!(
            audit.finish().events().is_empty(),
            "实际 -V 直接 OS 调用路径不能隐藏 Rust 分配"
        );
        assert_eq!(code, 0);
        let version = succeeds(ActualVersion::parse(&output[..count]));
        eprintln!("ACTUAL_VERSION {}；仅版本查询", version.line());
        drop(version_launch);
        for auth_mode in [false, true] {
            let mut value = value;
            if auth_mode {
                value.host = "::1";
                value.username = "用户名 空%${HOME}'\"";
            }
            let selection = ExactSelection {
                paths: [&fixture.selected, &fixture.selected_pub],
                binding,
            };
            value.auth = if auth_mode {
                SystemAuth::ExternalFile(succeeds(ExternalIdentity::selected(
                    &fixture.selected,
                    &fixture.selected_pub,
                    &selection,
                )))
            } else {
                SystemAuth::Password
            };
            let launch = succeeds(
                succeeds(BorrowedShape::checked(
                    value,
                    &image,
                    LaunchPurpose::EffectiveConfig,
                ))
                .admit(&root),
            );
            // 文件写入属于 fixture scaffold，不冒充生产 trust/config owner 证明。
            succeeds(fs::write(&fixture.config, launch.config()));
            let audit = audit::AuditScope::start(&root);
            let (code, count) = succeeds(query(&launch, &image, &mut output));
            assert!(
                audit.finish().events().is_empty(),
                "实际 -G 直接 OS 调用路径不能隐藏 Rust 分配"
            );
            if code != 0 {
                panic!("实际 -G 拒绝封闭配置，exit={code}，输出字节={count}");
            }
            let parsed = succeeds(EffectiveQuery::parse(&output[..count]));
            let hooks = succeeds(parsed.verify_closed_policy(value.auth));
            succeeds(parsed.exactly("hostname", value.host));
            succeeds(parsed.exactly("user", value.username));
            succeeds(parsed.exactly("port", "2222"));
            assert_eq!(
                parsed.exactly("securitykeyprovider", "${GUO_SYSTEM_PROVIDER}"),
                Ok(()),
                "provider 有效值"
            );
            let expected_callback = format!(
                "\"{}\" host-key \"%I\" \"%H\" \"%t\" \"%K\"",
                fixture.helper.replace('\\', "\\\\")
            );
            assert_eq!(
                parsed.exactly("knownhostscommand", &expected_callback),
                Ok(()),
                "KnownHostsCommand 精确 token"
            );
            assert_eq!(hooks.known_hosts_command, EvidenceLevel::Queried);
            assert_eq!(hooks.force_askpass_with_pty, EvidenceLevel::StaticDeclared);
            assert_eq!(hooks.post_auth_shell_chain, EvidenceLevel::StaticDeclared);
            eprintln!(
                "ACTUAL_G {}：封闭策略/目标精确值；环境块显式，未观测实际文件 open/agent/callback，不声明零访问或认证成功",
                if auth_mode {
                    "ExternalFile"
                } else {
                    "Password"
                }
            );
            drop(launch);
        }
        image_source
            .active
            .store(false, std::sync::atomic::Ordering::Release);
        assert_eq!(image.revalidate(), Err(ScopeReject::UntrustedImage));
        drop(image);
        fixture.cleanup();
        drop(os);
        drop(program);
        drop(mother);
        assert_eq!(root.snapshot().rust_bytes, 0);
        eprintln!(
            "TEMP_CLEAN：本轮目录已 exact delete；query wait+pipe EOF/drain 后返额；无 trust 写入、无秘密、无网络"
        );
    }
}
