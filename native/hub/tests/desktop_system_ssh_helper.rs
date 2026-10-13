//! 独立 wire fixture、固定源图及受控子进程 loopback；不注册 app 或伪造 live auth。
//!
//! 通过单次源码装配复用真实资源与保护接口，无额外 Cargo 依赖或私有编译 flags。

#[path = "../src/system_ssh/helper/fixture_library.rs"]
mod test_assembly;

pub use test_assembly::foundation::{
    audit, capacity, certificate, layout, ledger, owner, secret, selected_p1,
};
pub(crate) use test_assembly::protected_reply;
pub use test_assembly::{helper, interaction, signals};

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use helper::Reject;
use helper::buffer::{ProtectedBuffer, audit as buffer_audit};
use helper::pipeline::{FrozenAuth, Pipeline};
use helper::protocol::{
    Binding, HEADER_BYTES, Header, HostReason, Policy, RawFrame, Request, Role, SequenceGate,
};
use helper::transport::{
    CancellableIo, Cancellation, NotReadyPipe, PipePurpose, Resources, TrustedPipe, fixture,
};
use ledger::{
    Admission, Amount, BYTE_LIMIT, CLASS_COUNT, ProcessLedger, ResourceClass, SourceToken,
};
use secret::{CopyValidity, LockedSecretString};

fn binding() -> Binding {
    Binding {
        source: SourceToken {
            epoch: 1,
            index: 0,
            generation: 1,
        },
        attempt: [0x41; 16],
        process_link: [0x52; 16],
    }
}

fn policy() -> Policy {
    Policy::new(256, 64, 4096, 128).unwrap_or_else(|error| panic!("policy: {error:?}"))
}

fn root() -> ProcessLedger {
    ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT])
}

fn resources(root: &ProcessLedger) -> Resources<'_> {
    let source = root
        .open_source(1, 0)
        .unwrap_or_else(|error| panic!("source: {error:?}"));
    assert_eq!(source, binding().source);
    Resources {
        root,
        admission: Admission {
            epoch: 1,
            source: Some(source),
        },
    }
}

/// 手工字段偏移和字节，不调用生产 encoder 生成输入。
fn wire(role: u8, kind: u8, sequence: u64, prompt: u64, fields: &[&[u8]]) -> Vec<u8> {
    let mut bytes = vec![0; 128];
    bytes[..8].copy_from_slice(b"GSSHHLPR");
    bytes[8..10].copy_from_slice(&1u16.to_le_bytes());
    bytes[10] = role;
    bytes[11] = kind;
    bytes[16..24].copy_from_slice(&1u64.to_le_bytes());
    bytes[32..40].copy_from_slice(&1u64.to_le_bytes());
    bytes[40..56].fill(0x41);
    bytes[56..72].fill(0x52);
    bytes[72..80].copy_from_slice(&sequence.to_le_bytes());
    bytes[80..88].copy_from_slice(&prompt.to_le_bytes());
    for (i, field) in fields.iter().enumerate() {
        bytes[96 + i * 8..104 + i * 8].copy_from_slice(&(field.len() as u64).to_le_bytes());
        bytes.extend_from_slice(field);
    }
    bytes
}

#[test]
fn independent_wire_has_actual_hostkey_and_order_has_no_confirmation_key() {
    let hostname = wire(
        1,
        1,
        1,
        0,
        &[b"HOSTNAME", b"[fixture]:2200", b"ssh-ed25519", b"AAAA"],
    );
    let parsed = RawFrame::parse(&hostname, policy()).unwrap_or_else(|e| panic!("frame: {e:?}"));
    match parsed.request {
        Request::KnownHosts(query) => {
            assert_eq!(query.reason, HostReason::Hostname);
            assert_eq!(query.host, "[fixture]:2200");
            assert_eq!(query.algorithm, "ssh-ed25519");
            assert_eq!(query.key_base64, "AAAA");
        }
        _ => panic!("不是 host 请求"),
    }
    assert_eq!(parsed.header.encode(), hostname[..128]);
    let order = wire(1, 1, 1, 0, &[b"ORDER", b"fixture", b"", b""]);
    assert!(
        matches!(RawFrame::parse(&order, policy()).map(|f| f.request),
        Ok(Request::KnownHosts(query)) if query.reason == HostReason::Order && query.key_base64.is_empty())
    );
    for fields in [
        [b"ORDER".as_slice(), b"fixture", b"ssh-ed25519", b"AAAA"],
        [b"HOSTNAME".as_slice(), b"fixture", b"", b""],
        [b"ADDRESS".as_slice(), b"fixture", b"ssh-ed25519", b"AAAA"],
        [
            b"HOSTNAME".as_slice(),
            b"fixture",
            b"ssh-ed25519-cert-v01@openssh.com",
            b"AAAA",
        ],
        [b"HOSTNAME".as_slice(), b"fixture", b"ssh-ed25519", b"A=AA"],
    ] {
        assert_eq!(
            RawFrame::parse(&wire(1, 1, 1, 0, &fields), policy()).err(),
            Some(Reject::Malformed)
        );
    }
}

#[test]
fn checked_lengths_and_foreign_interactions_reject_without_helper_backing() {
    let root = root();
    let _resources = resources(&root);
    let audit = buffer_audit::Capture::start(&root);
    for kind in [2, 3, 4, 6, 255] {
        assert_eq!(
            RawFrame::parse(&wire(2, kind, 1, 9, &[b"password:"]), policy()).err(),
            Some(Reject::UnsupportedInteraction)
        );
    }
    for (at, value, expected) in [
        (96, u64::MAX, Reject::Limit),
        (104, 1, Reject::Limit),
        (72, 0, Reject::Sequence),
    ] {
        let mut bytes = wire(2, 1, 1, 9, &[b"prompt"]);
        bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
        assert_eq!(RawFrame::parse(&bytes, policy()).err(), Some(expected));
    }
    let mut truncated = wire(2, 1, 1, 9, &[b"prompt"]);
    truncated.pop();
    assert_eq!(
        RawFrame::parse(&truncated, policy()).err(),
        Some(Reject::Malformed)
    );
    let mut trailing = wire(3, 1, 1, 0, &[]);
    trailing.push(0);
    assert_eq!(
        RawFrame::parse(&trailing, policy()).err(),
        Some(Reject::Malformed)
    );
    assert_eq!(audit.events().2, 0);
    assert_eq!(root.snapshot().rust_bytes, 0);
}

struct RejectingPeer;
impl CancellableIo for RejectingPeer {
    fn read_some(&mut self, _out: &mut [u8], _cancel: &Cancellation) -> Result<usize, Reject> {
        panic!("伪 peer 不得读取")
    }
    fn write_some(&mut self, _bytes: &[u8], _cancel: &Cancellation) -> Result<usize, Reject> {
        panic!("伪 peer 不得发送")
    }
}
// SAFETY: 本 fixture 始终拒绝，不伪造任何实际身份前件。
unsafe impl TrustedPipe for RejectingPeer {
    fn verify(&self, _binding: Binding, _role: Role, _purpose: PipePurpose) -> Result<(), Reject> {
        Err(Reject::Peer)
    }
}

#[test]
fn fixed_roles_stale_links_cancel_closed_and_unready_never_allocate_or_send() {
    for (role, bytes) in [
        (
            Role::KnownHosts,
            wire(1, 1, 1, 0, &[b"ORDER", b"fixture", b"", b""]),
        ),
        (Role::Askpass, wire(2, 1, 1, 9, &[b"untrusted prompt"])),
        (Role::PostAuth, wire(3, 1, 1, 0, &[])),
    ] {
        let root = root();
        let resources = resources(&root);
        let pipeline = Pipeline::new(
            resources,
            binding(),
            role,
            FrozenAuth::SystemPassword,
            1,
            policy(),
        )
        .unwrap_or_else(|error| panic!("pipeline: {error:?}"));
        let held = root.snapshot().rust_bytes;
        let audit = buffer_audit::Capture::start(&root);
        for _ in 0..2 {
            assert_eq!(
                pipeline.admit(&bytes, &NotReadyPipe).err(),
                Some(Reject::NotReady)
            );
            assert_eq!(
                pipeline.admit(&bytes, &RejectingPeer).err(),
                Some(Reject::Peer)
            );
        }
        assert_eq!(
            pipeline.read(&mut NotReadyPipe).err(),
            Some(Reject::NotReady)
        );
        let foreign_role = if role == Role::PostAuth {
            wire(2, 1, 1, 9, &[b""])
        } else {
            wire(3, 1, 1, 0, &[])
        };
        assert_eq!(
            pipeline.admit(&foreign_role, &NotReadyPipe).err(),
            Some(Reject::WrongRole)
        );
        for (at, expected) in [
            (32, Reject::Source),
            (40, Reject::Attempt),
            (56, Reject::ProcessLink),
        ] {
            let mut stale = bytes.clone();
            stale[at] ^= 1;
            assert_eq!(pipeline.admit(&stale, &NotReadyPipe).err(), Some(expected));
        }
        let mut gap = bytes.clone();
        gap[72..80].copy_from_slice(&2u64.to_le_bytes());
        assert_eq!(
            pipeline.admit(&gap, &NotReadyPipe).err(),
            Some(Reject::Sequence)
        );
        pipeline.cancel();
        assert_eq!(
            pipeline.admit(&bytes, &NotReadyPipe).err(),
            Some(Reject::Cancelled)
        );
        assert_eq!(pipeline.terminal(), Some(Reject::Cancelled));
        pipeline.close();
        assert_eq!(
            pipeline.admit(&bytes, &NotReadyPipe).err(),
            Some(Reject::Closed)
        );
        assert_eq!(audit.events().2, 0);
        assert_eq!(root.snapshot().rust_bytes, held);
        drop(pipeline);
        assert_eq!(root.snapshot().rust_bytes, 0);
    }
}

#[test]
fn terminal_capacity_survives_a_completely_occupied_byte_budget() {
    let root = root();
    let resources = resources(&root);
    let admission = resources.admission;
    let pipeline = Pipeline::new(
        resources,
        binding(),
        Role::Askpass,
        FrozenAuth::FileIdentity,
        1,
        policy(),
    )
    .unwrap_or_else(|error| panic!("pipeline: {error:?}"));
    let control = root.snapshot();
    assert_eq!(control.classes[ResourceClass::Control as usize].slots, 3);
    let occupied = root
        .try_reserve(
            Amount {
                rust_bytes: BYTE_LIMIT - control.rust_bytes,
                c_bytes: 0,
                slots: 1,
            },
            ResourceClass::Runtime,
            admission,
        )
        .unwrap_or_else(|error| panic!("quota: {error:?}"));
    let audit = buffer_audit::Capture::start(&root);
    pipeline.cancel();
    assert_eq!(pipeline.terminal(), Some(Reject::Cancelled));
    assert_eq!(audit.events().2, 0);
    drop(occupied);
    drop(pipeline);
    assert_eq!(root.snapshot().rust_bytes, 0);
}

#[test]
fn independent_sequence_fixture_rejects_duplicates_and_gaps_before_owned_data() {
    let root = root();
    let audit = buffer_audit::Capture::start(&root);
    let mut sequence = SequenceGate::default();
    for value in [1, 2, 3] {
        let bytes = wire(2, 1, value, 9, &[b"display"]);
        let header = RawFrame::parse(&bytes, policy())
            .unwrap_or_else(|e| panic!("frame: {e:?}"))
            .header;
        assert!(sequence.accept(header.sequence()).is_ok());
        assert_eq!(sequence.accept(header.sequence()), Err(Reject::Sequence));
        assert_eq!(
            sequence.accept(header.sequence() + 2),
            Err(Reject::Sequence)
        );
    }
    assert_eq!(audit.events().2, 0);
    assert_eq!(root.snapshot().rust_bytes, 0);
}

struct BorrowedIo<'a> {
    input: &'a [u8],
    read_at: usize,
    written: usize,
    cancel_after: Option<usize>,
}

impl CancellableIo for BorrowedIo<'_> {
    fn read_some(&mut self, out: &mut [u8], cancellation: &Cancellation) -> Result<usize, Reject> {
        cancellation.check()?;
        let count = out.len().min(self.input.len() - self.read_at).min(3);
        out[..count].copy_from_slice(&self.input[self.read_at..self.read_at + count]);
        self.read_at += count;
        Ok(count)
    }
    fn write_some(&mut self, bytes: &[u8], cancellation: &Cancellation) -> Result<usize, Reject> {
        cancellation.check()?;
        let count = bytes.len().min(3);
        self.written += count;
        if self.cancel_after.is_some_and(|after| self.written >= after) {
            cancellation.cancel();
        }
        Ok(count)
    }
}

fn locked<'root>(input: &str, resources: &Resources<'root>) -> LockedSecretString<'root> {
    let grant = resources
        .root
        .try_reserve(
            Amount {
                rust_bytes: input.len(),
                c_bytes: 0,
                slots: 1,
            },
            ResourceClass::Secret,
            resources.admission,
        )
        .unwrap_or_else(|error| panic!("secret grant: {error:?}"));
    LockedSecretString::try_copy(input, grant, &CopyValidity::default())
        .unwrap_or_else(|error| panic!("protected source: {error:?}"))
}

#[test]
fn real_locked_adapter_exact_layout_last_clone_wipe_and_refund() {
    let root = root();
    let resources = resources(&root);
    let input = "固定 canary 🙂";
    let secret = locked(input, &resources);
    let audit = buffer_audit::Capture::start(&root);
    let buffer = fixture::copy_locked(&secret, &resources, &Cancellation::default())
        .unwrap_or_else(|error| panic!("adapter: {error:?}"));
    let layout = ProtectedBuffer::layout(input.len()).unwrap_or_else(|e| panic!("layout: {e:?}"));
    assert_eq!(buffer.admitted().rust_bytes, layout.backing.size());
    assert!(fixture::equals(&buffer, input.as_bytes()));
    let late = buffer
        .try_clone()
        .unwrap_or_else(|e| panic!("clone: {e:?}"));
    let held = root.snapshot().rust_bytes;
    root.invalidate_source(binding().source)
        .unwrap_or_else(|e| panic!("invalidate: {e:?}"));
    drop(buffer);
    assert_eq!(root.snapshot().rust_bytes, held);
    assert_eq!(audit.events().1, 0);
    drop(secret);
    assert_eq!(root.snapshot().rust_bytes, layout.backing.size());
    drop(late);
    let (events, len, allocations) = audit.events();
    assert_eq!((len, allocations), (1, 1));
    assert!(events[0].wiped);
    assert_eq!(events[0].capacity, input.len());
    assert_eq!(events[0].held, layout.backing.size());
    assert_eq!(root.snapshot().rust_bytes, 0);
}

#[test]
fn eof_wipes_written_and_unwritten_full_capacity_and_zero_budget_precedes_read() {
    let root = root();
    let resources = resources(&root);
    let audit = buffer_audit::Capture::start(&root);
    let mut io = BorrowedIo {
        input: b"canary",
        read_at: 0,
        written: 0,
        cancel_after: None,
    };
    assert_eq!(
        fixture::read(&mut io, 4097, &resources, &Cancellation::default()).err(),
        Some(Reject::UnexpectedEof)
    );
    let (events, len, allocations) = audit.events();
    assert_eq!((len, allocations), (1, 1));
    assert_eq!(events[0].capacity, 4097);
    assert!(events[0].wiped);
    assert!(
        events[0].held
            >= ProtectedBuffer::layout(4097)
                .unwrap_or_else(|e| panic!("layout: {e:?}"))
                .backing
                .size()
    );
    assert_eq!(root.snapshot().rust_bytes, 0);
    drop(audit);
    let empty = ProcessLedger::new(0, [u32::MAX; CLASS_COUNT]);
    let resources = crate::resources(&empty);
    let audit = buffer_audit::Capture::start(&empty);
    let mut io = BorrowedIo {
        input: b"canary",
        read_at: 0,
        written: 0,
        cancel_after: None,
    };
    assert!(fixture::read(&mut io, 6, &resources, &Cancellation::default()).is_err());
    assert_eq!(io.read_at, 0);
    assert_eq!(audit.events().2, 0);
}

#[test]
fn borrowed_header_reader_eof_and_overlarge_length_do_not_allocate_payload() {
    let root = root();
    let audit = buffer_audit::Capture::start(&root);
    let header = wire(2, 1, 1, 9, &[b""]);
    let mut short = BorrowedIo {
        input: &header[..120],
        read_at: 0,
        written: 0,
        cancel_after: None,
    };
    assert_eq!(
        fixture::checked_header(&mut short, policy(), &Cancellation::default()),
        Err(Reject::UnexpectedEof)
    );
    let mut overlarge = header;
    overlarge[96..104].copy_from_slice(&u64::MAX.to_le_bytes());
    let mut reader = BorrowedIo {
        input: &overlarge,
        read_at: 0,
        written: 0,
        cancel_after: None,
    };
    assert_eq!(
        fixture::checked_header(&mut reader, policy(), &Cancellation::default()),
        Err(Reject::Limit)
    );
    assert_eq!(audit.events().2, 0);
    assert_eq!(root.snapshot().rust_bytes, 0);
}

#[test]
fn system_line_limits_do_not_truncate_or_impose_a_business_secret_max() {
    let root = root();
    let resources = resources(&root);
    for input in ["a\0b", "a\nb", "a\rb"] {
        let secret = locked(input, &resources);
        let audit = buffer_audit::Capture::start(&root);
        assert_eq!(
            fixture::copy_locked(&secret, &resources, &Cancellation::default()).err(),
            Some(Reject::LineUnrepresentable)
        );
        assert_eq!(audit.events().2, 0);
        drop(audit);
        assert!(secret.expose_secret() == input);
    }
    let input = [b'x'; 8193];
    let text = std::str::from_utf8(&input).unwrap_or_else(|_| panic!("fixture UTF-8"));
    let secret = locked(text, &resources);
    let buffer = fixture::copy_locked(&secret, &resources, &Cancellation::default())
        .unwrap_or_else(|e| panic!("长秘密: {e:?}"));
    assert!(buffer.len() > policy().nonsecret_max());
    assert!(fixture::equals(&buffer, &input));
}

#[test]
fn empty_system_secret_is_preserved_and_only_the_reply_line_ending_is_written() {
    let root = root();
    let resources = resources(&root);
    let secret = locked("", &resources);
    let buffer = fixture::copy_locked(&secret, &resources, &Cancellation::default())
        .unwrap_or_else(|e| panic!("empty adapter: {e:?}"));
    assert!(buffer.is_empty());
    let mut writer = BorrowedIo {
        input: &[],
        read_at: 0,
        written: 0,
        cancel_after: None,
    };
    fixture::line(&mut writer, &buffer, &Cancellation::default())
        .unwrap_or_else(|e| panic!("empty line: {e:?}"));
    assert_eq!(writer.written, 1);
    drop(buffer);
    drop(secret);
    assert_eq!(root.snapshot().rust_bytes, 0);
}

#[test]
fn partial_write_cancel_reports_prefix_never_replays_and_wipes_adapter() {
    let root = root();
    let resources = resources(&root);
    let secret = locked("fixed-canary", &resources);
    let audit = buffer_audit::Capture::start(&root);
    let buffer = fixture::copy_locked(&secret, &resources, &Cancellation::default())
        .unwrap_or_else(|e| panic!("adapter: {e:?}"));
    let cancellation = Cancellation::default();
    let mut io = BorrowedIo {
        input: &[],
        read_at: 0,
        written: 0,
        cancel_after: Some(3),
    };
    let failure = fixture::line(&mut io, &buffer, &cancellation)
        .err()
        .unwrap_or_else(|| panic!("应取消"));
    assert_eq!(failure.cause, Reject::Cancelled);
    assert_eq!(
        (
            failure.header_prefix,
            failure.payload_prefix,
            failure.line_ending_prefix
        ),
        (0, 3, 0)
    );
    assert!(fixture::line(&mut io, &buffer, &cancellation).is_err());
    assert_eq!(io.written, 3);
    drop(buffer);
    assert!(audit.events().0[0].wiped);
    drop(secret);
    assert_eq!(root.snapshot().rust_bytes, 0);
}

#[test]
fn protected_reply_adapter_checks_prompt_and_copies_into_protected_backing() {
    let root = root();
    let resources = resources(&root);
    for (prompt, expected) in [(9, None), (8, Some(Reject::InvalidReply))] {
        let reply = interaction::InteractionReply {
            session_id: 1,
            prompt_id: prompt,
            accept: true,
            answers: vec![String::from("fixed-fixture-canary")],
            remember: false,
        };
        let drop_audit = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let reply = protected_reply::ProtectedReply::audited(reply, drop_audit.clone());
        let grant = resources
            .reserve_buffer(reply.answers[0].len(), ResourceClass::Secret)
            .unwrap_or_else(|e| panic!("reply grant: {e:?}"));
        let audit = buffer_audit::Capture::start(&root);
        let result = ProtectedBuffer::from_reply(&reply, 1, 9, grant, &Cancellation::default());
        if let Some(expected) = expected {
            assert_eq!(result.err(), Some(expected));
            assert_eq!(audit.events().2, 0);
        } else {
            let buffer = result.unwrap_or_else(|e| panic!("reply: {e:?}"));
            assert!(fixture::equals(&buffer, b"fixed-fixture-canary"));
            drop(buffer);
            assert!(audit.events().0[0].wiped);
        }
        drop(reply);
        assert_eq!(
            *drop_audit
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            [1]
        );
        assert_eq!(root.snapshot().rust_bytes, 0);
    }
    use secrecy::ExposeSecret;
    let mut reply = protected_reply::ProtectedReply::from(interaction::InteractionReply {
        session_id: 1,
        prompt_id: 9,
        accept: true,
        answers: vec![
            String::from("first-fixture-canary"),
            String::from("remaining-fixture-canary"),
        ],
        remember: false,
    });
    let first = reply
        .first_secret()
        .unwrap_or_else(|| panic!("first secret"));
    assert!(first.expose_secret() == "first-fixture-canary");
    let remaining = reply.all_secrets();
    assert_eq!(remaining.len(), 2);
    assert!(remaining[0].expose_secret().is_empty());
    assert!(remaining[1].expose_secret() == "remaining-fixture-canary");
}

struct LoopbackIo(TcpStream);
impl CancellableIo for LoopbackIo {
    fn read_some(&mut self, out: &mut [u8], cancellation: &Cancellation) -> Result<usize, Reject> {
        cancellation.check()?;
        self.0.read(out).map_err(|_| Reject::Io)
    }
    fn write_some(&mut self, bytes: &[u8], cancellation: &Cancellation) -> Result<usize, Reject> {
        cancellation.check()?;
        self.0.write(bytes).map_err(|_| Reject::Io)
    }
}

struct OwnedFixture {
    directory: std::path::PathBuf,
    child: Option<Child>,
}

impl OwnedFixture {
    fn new() -> Self {
        let parent = std::env::temp_dir();
        assert!(parent.is_dir(), "标准临时父目录不存在");
        let directory = parent.join(format!("system-helper-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap_or_else(|e| panic!("创建 fixture: {e}"));
        Self {
            directory,
            child: None,
        }
    }

    fn cleanup(&mut self) -> std::io::Result<()> {
        if let Some(child) = self.child.as_mut() {
            if child.try_wait()?.is_none() {
                child.kill()?;
                child.wait()?;
            }
            self.child = None;
        }
        std::fs::remove_dir(&self.directory)
    }
}

impl Drop for OwnedFixture {
    fn drop(&mut self) {
        if let Err(error) = self.cleanup() {
            eprintln!("fixture cleanup failure: {:?}", error.raw_os_error());
            assert!(std::thread::panicking(), "fixture 清理失败");
        }
    }
}

#[cfg(windows)]
fn fixture_windows_directory() -> std::ffi::OsString {
    use std::os::windows::ffi::OsStringExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetWindowsDirectoryW(buffer: *mut u16, size: u32) -> u32;
    }
    let mut buffer = [0u16; 32768];
    // SAFETY: 本地非秘密固定缓冲有效；OS 返回的路径只用于清空环境后的系统必需变量。
    let length = unsafe { GetWindowsDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
    assert!(length > 0 && length < buffer.len(), "OS 目录查询失败");
    std::ffi::OsString::from_wide(&buffer[..length])
}

#[test]
fn controlled_child_loopback_streams_protected_canary_and_reaps_exact_child() {
    let mut owned = OwnedFixture::new();
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .unwrap_or_else(|e| panic!("listener: {e}"));
    listener
        .set_nonblocking(true)
        .unwrap_or_else(|e| panic!("listener mode: {e}"));
    let endpoint = listener
        .local_addr()
        .unwrap_or_else(|e| panic!("endpoint: {e}"));
    let mut command =
        Command::new(std::env::current_exe().unwrap_or_else(|e| panic!("test exe: {e}")));
    command
        .args([
            "--exact",
            "controlled_helper_fixture_entry",
            "--ignored",
            "--nocapture",
        ])
        .env_clear()
        .env("SYSTEM_HELPER_FIXTURE_ENDPOINT", endpoint.to_string())
        .env("SYSTEM_HELPER_FIXTURE_DIRECTORY", &owned.directory)
        .current_dir(&owned.directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    command.env("SystemRoot", fixture_windows_directory());
    owned.child = Some(
        command
            .spawn()
            .unwrap_or_else(|e| panic!("spawn fixture: {e}")),
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    let stream = loop {
        match listener.accept() {
            Ok((stream, peer)) => {
                assert!(peer.ip().is_loopback());
                break stream;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "等待 fixture 超时");
                let exited = owned
                    .child
                    .as_mut()
                    .unwrap_or_else(|| panic!("child"))
                    .try_wait()
                    .unwrap_or_else(|e| panic!("child status: {e}"));
                assert!(exited.is_none(), "fixture 提前结束: {exited:?}");
                std::thread::yield_now();
            }
            Err(e) => panic!("accept: {e}"),
        }
    };
    drop(listener);
    stream
        .set_nonblocking(false)
        .unwrap_or_else(|e| panic!("stream mode: {e}"));
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap_or_else(|e| panic!("read timeout: {e}"));
    stream
        .set_write_timeout(Some(Duration::from_secs(20)))
        .unwrap_or_else(|e| panic!("write timeout: {e}"));
    let mut pipe = LoopbackIo(stream);
    let root = root();
    let resources = resources(&root);
    let secret = locked("controlled-system-helper-canary", &resources);
    let audit = buffer_audit::Capture::start(&root);
    let buffer = fixture::copy_locked(&secret, &resources, &Cancellation::default())
        .unwrap_or_else(|e| panic!("protected adapter: {e:?}"));
    fixture::line(&mut pipe, &buffer, &Cancellation::default())
        .unwrap_or_else(|e| panic!("write: {e:?}"));
    let reply = fixture::read(
        &mut pipe,
        b"controlled-system-helper-canary\n".len(),
        &resources,
        &Cancellation::default(),
    )
    .unwrap_or_else(|e| panic!("read: {e:?}"));
    assert!(fixture::equals(
        &reply,
        b"controlled-system-helper-canary\n"
    ));
    drop(reply);
    drop(buffer);
    drop(secret);
    assert_eq!(root.snapshot().rust_bytes, 0);
    let (events, len, allocations) = audit.events();
    assert_eq!((len, allocations), (2, 2));
    assert!(events[..len].iter().all(|e| e.wiped && e.held > 0));
    pipe.0
        .shutdown(Shutdown::Both)
        .unwrap_or_else(|e| panic!("shutdown: {e}"));
    drop(pipe);
    let status = owned
        .child
        .as_mut()
        .unwrap_or_else(|| panic!("child"))
        .wait()
        .unwrap_or_else(|e| panic!("wait: {e}"));
    assert!(status.success());
    owned.child = None;
    // Drop 只删除登记的空 fixture 目录；已显式关闭 listener、pipe 并 wait 本次 child。
}

#[test]
#[ignore = "仅由受控父 fixture 以精确入口启动"]
fn controlled_helper_fixture_entry() {
    let directory = std::env::var_os("SYSTEM_HELPER_FIXTURE_DIRECTORY")
        .unwrap_or_else(|| panic!("缺 fixture cwd"));
    assert_eq!(
        std::env::current_dir().unwrap_or_else(|e| panic!("cwd: {e}")),
        std::path::PathBuf::from(directory)
    );
    let endpoint: std::net::SocketAddr = std::env::var("SYSTEM_HELPER_FIXTURE_ENDPOINT")
        .unwrap_or_else(|_| panic!("缺 endpoint"))
        .parse()
        .unwrap_or_else(|_| panic!("endpoint"));
    assert!(endpoint.ip().is_loopback());
    let stream = TcpStream::connect_timeout(&endpoint, Duration::from_secs(20))
        .unwrap_or_else(|e| panic!("fixture connect: {e}"));
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap_or_else(|e| panic!("timeout: {e}"));
    stream
        .set_write_timeout(Some(Duration::from_secs(20)))
        .unwrap_or_else(|e| panic!("timeout: {e}"));
    let mut pipe = LoopbackIo(stream);
    let root = root();
    let resources = resources(&root);
    let audit = buffer_audit::Capture::start(&root);
    let buffer = fixture::read(
        &mut pipe,
        b"controlled-system-helper-canary".len(),
        &resources,
        &Cancellation::default(),
    )
    .unwrap_or_else(|e| panic!("fixture read: {e:?}"));
    assert!(fixture::equals(&buffer, b"controlled-system-helper-canary"));
    let mut delimiter = [0u8; 1];
    pipe.0
        .read_exact(&mut delimiter)
        .unwrap_or_else(|e| panic!("fixture line ending: {e}"));
    assert_eq!(delimiter, [b'\n']);
    fixture::line(&mut pipe, &buffer, &Cancellation::default())
        .unwrap_or_else(|e| panic!("fixture reply: {e:?}"));
    drop(buffer);
    assert_eq!(root.snapshot().rust_bytes, 0);
    assert!(audit.events().0[0].wiped);
    pipe.0
        .shutdown(Shutdown::Both)
        .unwrap_or_else(|e| panic!("fixture shutdown: {e}"));
}

#[test]
fn numeric_layout_reports_actual_types_not_observed_peak_or_system_h() {
    let control = Pipeline::layout().unwrap_or_else(|e| panic!("control layout: {e:?}"));
    let buffer = ProtectedBuffer::layout(4097).unwrap_or_else(|e| panic!("buffer layout: {e:?}"));
    println!(
        "control={} align={}; protected4097={} align={} payload={}; request={} header={}; pipeline={} auth_event={}",
        control.backing.size(),
        control.backing.align(),
        buffer.backing.size(),
        buffer.backing.align(),
        buffer.payload_offset,
        helper::pipeline::OwnedRequest::outer_layout().size(),
        HEADER_BYTES,
        std::alloc::Layout::new::<Pipeline<'_>>().size(),
        std::alloc::Layout::new::<helper::pipeline::AuthenticationCompleted<'_>>().size()
    );
    assert!(buffer.backing.size() >= 4097 + buffer.payload_offset);
    let secret_header = wire(2, 5, 1, 9, &[b""]);
    assert!(Header::parse(&secret_header, policy()).is_ok());
}
