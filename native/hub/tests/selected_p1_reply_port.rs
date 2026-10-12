//! 独立 W3 native peer/FFI 协议测试；fake post 不是真实 Flutter/GC 证据。
//! 链接真实 W1-L/F，不注册 hub 生产模块、不调用正常 Prepare/Start。

#[path = "../src/selected_p1/font_source/mod.rs"]
pub mod font_source;
#[path = "../src/selected_p1/model/layout.rs"]
pub mod layout;
#[path = "../src/selected_p1/resource/ledger.rs"]
pub mod ledger;
#[path = "../src/selected_p1/resource/owner.rs"]
pub mod owner;
#[path = "../src/selected_p1/reply_port/mod.rs"]
pub mod reply_port;
#[path = "../src/selected_p1/resource/secret.rs"]
pub mod secret;

pub mod selected_p1 {
    pub use crate::font_source;
    pub mod model {
        pub use crate::layout;
    }
    pub mod resource {
        pub use crate::{ledger, owner, secret};
    }
}

#[path = "../src/selected_p1/font_source/audit.rs"]
pub mod audit;
#[path = "../src/selected_p1/font_source/oracle_fixture.rs"]
pub mod oracle_fixture;
pub use oracle_fixture::{keys, settings, settings_signals};
pub mod signals {
    pub use crate::settings_signals as settings;
}
#[path = "../src/font_resources.rs"]
pub mod font_resources;
#[path = "../src/fonts.rs"]
pub mod fonts;
#[path = "../src/profile_fonts.rs"]
pub mod profile_fonts;

#[global_allocator]
static ALLOCATOR: audit::Allocator = audit::Allocator;

use std::cell::Cell;
use std::ffi::c_void;
use std::path::Path;
use std::sync::Once;
use std::sync::atomic::AtomicBool;

use allo_isolate::ffi::{
    DartCObject, DartCObjectType, DartNativeExternalTypedData, DartTypedDataType,
};
use layout::NumericReject;
use ledger::{Admission, Amount, BYTE_LIMIT, CLASS_COUNT, ProcessLedger, ResourceClass};
use reply_port::{
    ByteSink, FiniteMetadata, FrameSpans, LibraryPinProof, MetadataHeaderBuilder, PortOwner,
    PostOutcome, ReplyPortReject, ReplyPortState,
};
use rshell_m0::rshell_core::TerminalSettingsV1;

const HEADER: &[u8] = b"private-frame\0";
const FONT: &[u8] = include_bytes!("../../../assets/fonts/MesloLGS-NF-Regular.ttf");

pub fn succeeds<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("预期成功：{error:?}"),
    }
}

fn root(limit: usize) -> ProcessLedger {
    ProcessLedger::new(limit, [u32::MAX; CLASS_COUNT])
}
fn admission() -> Admission {
    Admission {
        epoch: 1,
        source: None,
    }
}

/// 有限测试 DTO，不冒充父的实际 bincode/Flutter DTO 编码证明。
struct Dto<'a> {
    key: &'a str,
    fields: &'a [&'a str],
}

// SAFETY: 最多八个借用字段，先验证数量；encode 只有 checked 长度、栈整数与 writer。
// 不创建 String/CString/Vec，不 clone root/投影；所有测试输入都是借用的静态文字。
unsafe impl FiniteMetadata for Dto<'_> {
    fn encode(&self, writer: &mut dyn ByteSink) -> Result<(), NumericReject> {
        if self.fields.len() > 8 {
            return Err(NumericReject::ConstructionFailure);
        }
        write_text(writer, self.key)?;
        writer.write(&(self.fields.len() as u32).to_le_bytes())?;
        for field in self.fields {
            write_text(writer, field)?;
        }
        Ok(())
    }
}

fn write_text(writer: &mut dyn ByteSink, text: &str) -> Result<(), NumericReject> {
    let len = u32::try_from(text.len()).map_err(|_| NumericReject::Overflow)?;
    writer.write(&len.to_le_bytes())?;
    writer.write(text.as_bytes())
}

struct EmptyMetadata;
// SAFETY: 空 DTO 两次编码都无分配、无字段、无输出。
unsafe impl FiniteMetadata for EmptyMetadata {
    fn encode(&self, _writer: &mut dyn ByteSink) -> Result<(), NumericReject> {
        Ok(())
    }
}

#[test]
fn reply_port_single_backing_has_actual_offsets_zero_spans_and_no_encoding_heap()
-> Result<(), NumericReject> {
    for fields in 0..=8 {
        let strings = ["", "中\0文🙂", "a", "键", "", "μ", "tail", "last"];
        let dto = Dto {
            key: "root\0键",
            fields: &strings[..fields],
        };
        let ledger = root(BYTE_LIMIT);
        let builder = MetadataHeaderBuilder::new(HEADER, &dto, &[]);
        let scope = audit::AuditScope::start(&ledger);
        let model = builder.layout::<()>()?;
        assert!(scope.finish().events().is_empty());
        let scope = audit::AuditScope::start(&ledger);
        let reply = builder.try_build(&ledger, admission(), ())?;
        let spans = reply.spans();
        let metadata = &reply.bytes()[spans.metadata.offset as usize..spans.binary.offset as usize];
        let key_len = u32::from_le_bytes(succeeds(metadata[..4].try_into())) as usize;
        assert_eq!(&metadata[4..4 + key_len], dto.key.as_bytes());
        assert_eq!(&reply.bytes()[..HEADER.len()], HEADER);
        assert_eq!(reply.bytes().len(), spans.total as usize);
        assert_eq!(spans.binary.len, 0);
        assert_eq!(reply.admitted(), model.amount());
        assert_eq!(
            (reply.bytes().as_ptr() as usize) % model.owner.backing.align(),
            0
        );
        drop(reply);
        let report = scope.finish();
        assert!(report.symmetric_single_backing());
        report.assert_balanced_and_pregranted();
        assert_eq!(ledger.snapshot().rust_bytes, 0);
    }
    let ledger = root(BYTE_LIMIT);
    let scope = audit::AuditScope::start(&ledger);
    let reply = MetadataHeaderBuilder::new(HEADER, &EmptyMetadata, &[]).try_build(
        &ledger,
        admission(),
        (),
    )?;
    assert_eq!(reply.spans().metadata.len, 0);
    assert_eq!(reply.spans().binary.len, 0);
    assert_eq!(reply.bytes(), HEADER);
    drop(reply);
    scope.finish().assert_balanced_and_pregranted();
    Ok(())
}

#[test]
fn reply_port_checked_frame_rejects_invalid_offsets_overflow_and_empty_header() {
    assert_eq!(
        FrameSpans::checked(0, 0, 0),
        Err(NumericReject::ConstructionFailure)
    );
    assert!(FrameSpans::checked(1, usize::MAX, 0).is_err());
    assert!(FrameSpans::checked(1, 0, u32::MAX as usize).is_err());
    let mut spans = succeeds(FrameSpans::checked(1, 0, 0));
    spans.binary.offset = 0;
    assert_eq!(
        reply_port::ReplyLayout::actual::<()>(spans).err(),
        Some(NumericReject::ConstructionFailure)
    );
}

#[test]
fn reply_port_grant_shortfall_and_allocator_failure_do_not_publish_peer()
-> Result<(), NumericReject> {
    let builder = MetadataHeaderBuilder::new(HEADER, &EmptyMetadata, &[1, 2, 3]);
    let model = builder.layout::<()>()?;
    let ledger = root(model.amount().rust_bytes - 1);
    let scope = audit::AuditScope::start(&ledger);
    assert_eq!(
        builder.try_build(&ledger, admission(), ()).err(),
        Some(NumericReject::RustBudgetExceeded)
    );
    assert!(scope.finish().events().is_empty());
    let ledger = root(BYTE_LIMIT);
    let scope = audit::AuditScope::start(&ledger);
    scope.fail_next_size(model.owner.backing.size());
    assert_eq!(
        builder.try_build(&ledger, admission(), ()).err(),
        Some(NumericReject::AllocationFailure)
    );
    let report = scope.finish();
    assert_eq!(report.events().len(), 1);
    assert_eq!(report.events()[0].kind, 4);
    report.assert_balanced_and_pregranted();
    assert_eq!(ledger.snapshot().rust_bytes, 0);
    Ok(())
}

#[test]
fn reply_port_reserved_result_grant_is_consumed_without_second_reservation()
-> Result<(), NumericReject> {
    let builder = MetadataHeaderBuilder::new(HEADER, &EmptyMetadata, &[]);
    let model = builder.layout::<()>()?;
    let ledger = root(model.amount().rust_bytes);
    let grant = ledger.try_reserve(model.amount(), ResourceClass::Reply, admission())?;
    let scope = audit::AuditScope::start(&ledger);
    let reply = builder.try_build_reserved(grant, ())?;
    assert_eq!(ledger.snapshot().rust_bytes, model.amount().rust_bytes);
    drop(reply);
    scope.finish().assert_balanced_and_pregranted();
    assert_eq!(ledger.snapshot().rust_bytes, 0);
    let grant = ledger.try_reserve(model.amount(), ResourceClass::Source, admission())?;
    let scope = audit::AuditScope::start(&ledger);
    assert_eq!(
        builder.try_build_reserved(grant, ()).err(),
        Some(NumericReject::ConstructionFailure)
    );
    assert!(scope.finish().events().is_empty());
    Ok(())
}

#[test]
fn reply_port_control_is_numeric_heapless_closed_and_epoch_scoped() {
    let ledger = root(0);
    let scope = audit::AuditScope::start(&ledger);
    let mut state = ReplyPortState::new(1, 2);
    assert_eq!(
        state.register(1, 2, 0, 0),
        Err(ReplyPortReject::InvalidPort)
    );
    assert_eq!(
        state.register(1, 2, -1, 0),
        Err(ReplyPortReject::InvalidPort)
    );
    assert_eq!(
        state.register(1, 3, 71, 0),
        Err(ReplyPortReject::ProtocolMismatch)
    );
    assert_eq!(
        state.register(2, 2, 71, 0),
        Err(ReplyPortReject::EpochMismatch)
    );
    assert_eq!(
        state.register(1, 2, 71, 9),
        Err(ReplyPortReject::OwnerMismatch)
    );
    let first = succeeds(state.register(1, 2, 71, 0));
    assert_eq!(state.register(1, 2, 71, 0), Ok(first));
    assert_eq!(state.register(1, 2, 71, first.token), Ok(first));
    assert_eq!(
        state.register(1, 2, 72, 0),
        Err(ReplyPortReject::AlreadyRegistered)
    );
    assert_eq!(
        state.close(PortOwner {
            token: first.token + 1,
            ..first
        }),
        Err(ReplyPortReject::OwnerMismatch)
    );
    assert_eq!(
        state.begin_epoch(2),
        Err(ReplyPortReject::AlreadyRegistered)
    );
    succeeds(state.close(first));
    succeeds(state.close(first));
    assert_eq!(state.register(1, 2, 71, 0), Err(ReplyPortReject::Closed));
    assert_eq!(state.begin_epoch(1), Err(ReplyPortReject::EpochMismatch));
    succeeds(state.begin_epoch(2));
    let next = succeeds(state.register(2, 2, 72, 0));
    assert!(next.token > first.token);
    assert_eq!(state.authorize(first), Err(ReplyPortReject::EpochMismatch));
    assert_eq!(state.close(first), Err(ReplyPortReject::OwnerMismatch));
    assert_eq!(succeeds(state.authorize(next)).value(), 72);
    state.shutdown();
    assert_eq!(state.register(2, 2, 72, 0), Err(ReplyPortReject::Closed));
    assert!(scope.finish().events().is_empty());
}

// 独立 test binary 的 SDK 初始化只做一次，且总是在安装 fake 前检查未注册分支。
static SDK_INIT: Once = Once::new();
static SDK_NO_POST_ROOT: ProcessLedger = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
static VM_WRITE_ROOT: ProcessLedger = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
static PROTOCOL_ROOTS: [ProcessLedger; 4] = [
    ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]),
    ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]),
    ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]),
    ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]),
];

#[derive(Clone, Copy)]
struct FakePost {
    accept: bool,
    calls: u32,
    valid: bool,
    captured: Option<DartNativeExternalTypedData>,
}
thread_local! {
    static FAKE: Cell<FakePost> = const { Cell::new(FakePost {
        accept: false, calls: 0, valid: true, captured: None,
    }) };
}

unsafe extern "C" fn fake_post(port: i64, object: *mut DartCObject) -> bool {
    // SAFETY: allo 在 post 调用期间提供一个真实 initialized CObject；不访问所有权根。
    let object = unsafe { &*object };
    let mut state = FAKE.with(Cell::get);
    state.calls += 1;
    state.valid &= port > 0 && object.ty == DartCObjectType::DartExternalTypedData;
    if object.ty == DartCObjectType::DartExternalTypedData {
        let external = unsafe { object.value.as_external_typed_data };
        state.valid &= external.ty == DartTypedDataType::Uint8
            && external.length > 0
            && !external.data.is_null()
            && !external.peer.is_null()
            && external.peer.cast::<u8>() != external.data
            && state.captured.is_none();
        if state.accept {
            state.captured = Some(external);
        }
    }
    FAKE.with(|cell| cell.set(state));
    state.accept
}

/// 一份 VM 所有权的测试替身；只能从 fake post 的一次成功 capture 取得，不能 Clone。
struct CapturedPeer(Option<DartNativeExternalTypedData>);
impl CapturedPeer {
    fn take() -> Self {
        let data = FAKE.with(|cell| {
            let mut state = cell.get();
            let data = state.captured.take();
            cell.set(state);
            data
        });
        assert!(data.is_some());
        Self(data)
    }
    fn bytes(&self) -> &[u8] {
        let data = succeeds(self.0.as_ref().ok_or("已析构 peer"));
        // SAFETY: 当前替身独占未消费 finalizer；data 仍活，测试不改 external bytes。
        unsafe { std::slice::from_raw_parts(data.data, data.length as usize) }
    }
    fn bytes_mut(&mut self) -> &mut [u8] {
        let data = succeeds(self.0.as_ref().ok_or("已析构 peer"));
        // SAFETY: 唯一 capture 已从 FAKE 取走，CapturedPeer 不可 Clone；&mut self
        // 排除并存的 bytes()/bytes_mut() 借用。data 来自已消费 reply 原 allocation
        // 的 writable raw pointer；这里模拟 VM 独占写 tail，不写 header/body/source。
        unsafe { std::slice::from_raw_parts_mut(data.data, data.length as usize) }
    }
    fn finalize(mut self) {
        self.release();
    }
    fn release(&mut self) {
        if let Some(data) = self.0.take() {
            // SAFETY: 唯一 capture 的 callback 只消费一次；故意传不同首参数验证不用它。
            unsafe {
                (data.callback)(std::ptr::dangling_mut::<u8>().cast::<c_void>(), data.peer);
            }
        }
    }
}
impl Drop for CapturedPeer {
    fn drop(&mut self) {
        self.release();
    }
}

fn set_accept(accept: bool) {
    FAKE.with(|cell| {
        let mut state = cell.get();
        state.accept = accept;
        cell.set(state);
    });
}

fn begin_fake_post_test(accept: bool) {
    FAKE.with(|cell| {
        assert!(cell.get().captured.is_none());
        cell.set(FakePost {
            accept,
            calls: 0,
            valid: true,
            captured: None,
        });
    });
}

fn initialize_fake_sdk() {
    SDK_INIT.call_once(|| {
        let ledger = &SDK_NO_POST_ROOT;
        let mut state = ReplyPortState::new(1, 2);
        let owner = succeeds(state.register(1, 2, 71, 0));
        // SAFETY: 静态 test binary/ledger/callback 活到进程结束，不是真实 Flutter pin。
        let pin = unsafe { LibraryPinProof::from_process_library_pin() };
        let scope = audit::AuditScope::start(ledger);
        let reply = succeeds(
            MetadataHeaderBuilder::new(HEADER, &EmptyMetadata, &[]).try_build(
                ledger,
                admission(),
                (),
            ),
        );
        assert_eq!(state.post(owner, reply, &pin), PostOutcome::NotAccepted);
        assert_eq!(FAKE.with(|cell| cell.get().calls), 0);
        let report = scope.finish();
        assert!(report.symmetric_single_backing());
        report.assert_balanced_and_pregranted();
        assert_eq!(ledger.snapshot().rust_bytes, 0);
        // SAFETY: isolated test process 的唯一 SDK 初始化；生产不调用此 setter。
        unsafe {
            allo_isolate::store_dart_post_cobject(fake_post);
        }
    });
}

fn source(
    ledger: &'static ProcessLedger,
    admission: Admission,
    legacy: Option<&fonts::ImportedFont>,
    settings: &TerminalSettingsV1,
) -> font_source::SourceOwner<'static> {
    let proof = succeeds(font_source::FontBuildProof::verify(
        font_source::GNU_FONT_BUILD,
    ));
    succeeds(font_source::capture_bytes(
        &proof,
        ledger,
        admission,
        font_source::BorrowedCapture {
            settings,
            profile_id: [1; 16],
            revision: 17,
            legacy_slot_version: 4,
            captured_slot: legacy,
            trusted_root: font_source::TrustedRoot::from_root_caller(Path::new("fixture")),
        },
        font_source::FontAvailability::Bytes(FONT),
        &AtomicBool::new(false),
    ))
}

#[test]
fn reply_port_public_allo_protocol_holds_actual_peers_until_unique_finalizer()
-> Result<(), NumericReject> {
    initialize_fake_sdk();
    begin_fake_post_test(false);
    let ledger = &PROTOCOL_ROOTS[0];
    let settings = TerminalSettingsV1::default();
    let mut state = ReplyPortState::new(1, 2);
    let owner = succeeds(state.register(1, 2, 71, 0));
    // SAFETY: 此静态 test binary 不会卸载；根表与回调活到进程结束，不声称真实 Flutter pin。
    let pin = unsafe { LibraryPinProof::from_process_library_pin() };
    let builder = MetadataHeaderBuilder::new(HEADER, &EmptyMetadata, &[]);
    let unregistered = ReplyPortState::new(1, 2);
    let scope = audit::AuditScope::start(ledger);
    let reply = builder.try_build(ledger, admission(), ())?;
    assert_eq!(
        unregistered.post(PortOwner { epoch: 1, token: 0 }, reply, &pin),
        PostOutcome::RegistrationRejected(ReplyPortReject::OwnerMismatch)
    );
    scope.finish().assert_balanced_and_pregranted();
    set_accept(false);
    let scope = audit::AuditScope::start(ledger);
    let reply = builder.try_build(ledger, admission(), ())?;
    assert_eq!(state.post(owner, reply, &pin), PostOutcome::NotAccepted);
    let report = scope.finish();
    assert!(report.symmetric_single_backing());
    report.assert_balanced_and_pregranted();
    assert_eq!(ledger.snapshot().rust_bytes, 0);
    assert_eq!(FAKE.with(|cell| cell.get().calls), 1);

    let scope = audit::AuditScope::start(ledger);
    let source = source(ledger, admission(), None, &settings);
    let reply = builder.try_build(ledger, admission(), source)?;
    assert_eq!(state.post(owner, reply, &pin), PostOutcome::NotAccepted);
    scope.finish().assert_balanced_and_pregranted();
    assert_eq!(ledger.snapshot().rust_bytes, 0);

    set_accept(true);
    let scope = audit::AuditScope::start(ledger);
    let reply = builder.try_build(ledger, admission(), ())?;
    let held = reply.admitted().rust_bytes;
    assert_eq!(state.post(owner, reply, &pin), PostOutcome::AcceptedByVm);
    assert_eq!(ledger.snapshot().rust_bytes, held); // send-return/ACK 不是释放事件。
    assert_eq!(
        ledger
            .try_reserve(
                Amount {
                    rust_bytes: BYTE_LIMIT,
                    c_bytes: 0,
                    slots: 1
                },
                ResourceClass::Reply,
                admission()
            )
            .err(),
        Some(NumericReject::RustBudgetExceeded)
    );
    let peer = CapturedPeer::take();
    assert_eq!(peer.bytes(), HEADER); // 空 binary 仍是具体非空 external frame。
    succeeds(state.close(owner));
    assert_eq!(ledger.snapshot().rust_bytes, held);
    let closure = move || {
        assert_eq!(peer.bytes(), HEADER);
        peer.finalize();
    };
    assert_eq!(ledger.snapshot().rust_bytes, held); // closure 的真实引用仍持有 peer。
    closure();
    scope.finish().assert_balanced_and_pregranted();
    assert_eq!(ledger.snapshot().rust_bytes, 0);

    let calls = FAKE.with(|cell| cell.get().calls);
    let scope = audit::AuditScope::start(ledger);
    let reply = builder.try_build(ledger, admission(), ())?;
    assert_eq!(
        state.post(owner, reply, &pin),
        PostOutcome::RegistrationRejected(ReplyPortReject::Closed)
    );
    assert_eq!(FAKE.with(|cell| cell.get().calls), calls);
    scope.finish().assert_balanced_and_pregranted();

    reply_port_source_and_peer_order(&pin, true)?;
    reply_port_source_and_peer_order(&pin, false)?;
    reply_port_metadata_root_remains_until_last_owner(&pin)?;
    reply_port_old_epoch_peer_releases_only_its_actual_root(&pin)?;
    assert!(FAKE.with(|cell| cell.get().valid));
    Ok(())
}

#[test]
fn reply_port_public_allo_vm_mutable_tail_keeps_source_owners_and_late_epoch_budget()
-> Result<(), NumericReject> {
    initialize_fake_sdk();
    begin_fake_post_test(true);
    let ledger = &VM_WRITE_ROOT;
    let settings = TerminalSettingsV1::default();
    let checked = succeeds(font_source::inspect(FONT, "fixture.ttf"));
    let legacy = fonts::ImportedFont {
        hash: checked.hash().to_owned(),
        label: checked.label().to_owned(),
    };
    let mut state = ReplyPortState::new(1, 2);
    let owner = succeeds(state.register(1, 2, 71, 0));
    let token = ledger.open_source(1, 0)?;
    let old_admission = Admission {
        epoch: 1,
        source: Some(token),
    };
    // SAFETY: 此测试的 module/根表/callback 均静态存活，不证明实际 hub library pin。
    let pin = unsafe { LibraryPinProof::from_process_library_pin() };
    let scope = audit::AuditScope::start(ledger);
    let captured_source = source(ledger, old_admission, Some(&legacy), &settings);
    let source_bytes = captured_source.admitted().rust_bytes;
    let prepared = captured_source.try_clone()?;
    let runtime = captured_source.try_clone()?;
    let dto = Dto {
        key: "mutable-frame",
        fields: &["键\0🙂"],
    };
    let builder =
        MetadataHeaderBuilder::new(HEADER, &dto, captured_source.bytes().unwrap_or_default());
    let layout = builder.layout::<font_source::SourceOwner<'static>>()?;
    let reply = builder.try_build(ledger, old_admission, captured_source.try_clone()?)?;
    let spans = reply.spans();
    // 此 shared 借用只观察移交前的 bytes；作用域结束后才消费 reply。
    {
        let bytes = reply.bytes();
        assert!(bytes.starts_with(HEADER));
        assert!(bytes.ends_with(FONT));
        assert_eq!(bytes.len(), spans.total as usize);
    }
    assert_eq!(state.post(owner, reply, &pin), PostOutcome::AcceptedByVm);
    let mut peer = CapturedPeer::take();
    let external = succeeds(peer.0.ok_or("未捕获 external data"));
    assert_eq!(
        external.data,
        external
            .peer
            .cast::<u8>()
            .wrapping_add(layout.owner.payload_offset)
    );
    assert_ne!(
        external.data.cast_const(),
        captured_source.bytes().unwrap_or_default().as_ptr()
    );
    let held = source_bytes + layout.amount().rust_bytes;
    assert_eq!(ledger.snapshot().rust_bytes, held);
    // 模拟 Uint8List 对完整 external tail 的合法写入，既不接管 peer 又不产生副本。
    for byte in peer.bytes_mut() {
        *byte ^= 0xa5;
    }
    for (actual, original) in peer.bytes()[..HEADER.len()].iter().zip(HEADER) {
        assert_eq!(*actual, *original ^ 0xa5);
    }
    for (actual, original) in peer.bytes()[spans.binary.offset as usize..]
        .iter()
        .zip(FONT)
    {
        assert_eq!(*actual, *original ^ 0xa5);
    }
    assert_eq!(prepared.bytes(), Some(FONT));
    assert_eq!(runtime.bytes(), Some(FONT));
    assert_eq!(ledger.snapshot().rust_bytes, held);
    ledger.invalidate_source(token)?;
    state.shutdown();
    let epoch = ledger.advance_epoch(1)?;
    succeeds(state.begin_epoch(epoch));
    let new_owner = succeeds(state.register(epoch, 2, 72, 0));
    let new_token = ledger.open_source(epoch, 0)?;
    let next_source = source(
        ledger,
        Admission {
            epoch,
            source: Some(new_token),
        },
        None,
        &settings,
    );
    let next_bytes = next_source.admitted().rust_bytes;
    assert_eq!(ledger.snapshot().rust_bytes, held + next_bytes);
    drop(captured_source);
    drop(prepared);
    // old epoch 的 VM 仍独占可写 tail，runtime 保留的原 Source 不受此写入影响。
    peer.bytes_mut()[spans.binary.offset as usize] = 0x5a;
    assert_eq!(runtime.bytes(), Some(FONT));
    peer.finalize();
    assert_eq!(ledger.snapshot().rust_bytes, source_bytes + next_bytes);
    assert_eq!(succeeds(state.authorize(new_owner)).value(), 72);
    drop(runtime);
    assert_eq!(ledger.snapshot().rust_bytes, next_bytes);
    drop(next_source);
    assert_eq!(ledger.snapshot().rust_bytes, 0);
    scope.finish().assert_balanced_and_pregranted();
    assert!(FAKE.with(|cell| cell.get().valid));
    Ok(())
}

fn reply_port_metadata_root_remains_until_last_owner(
    pin: &LibraryPinProof,
) -> Result<(), NumericReject> {
    let ledger = &PROTOCOL_ROOTS[0];
    let settings = TerminalSettingsV1::default();
    // oracle 的借用输入/结果在审计外构造，不属于被声明为已收费的 P1 backing。
    let profile = settings_signals::DesktopTerminalProfile {
        id: uuid::Uuid::from_bytes([1; 16]).to_string(),
        name: String::new(),
        version: 1,
        terminal_type: String::from("xterm-256color"),
        initial_cols: 80,
        initial_rows: 24,
        scrollback_lines: 10_000,
        font_family: settings.font_family.clone(),
        font_size: 14.0,
        color_scheme: String::new(),
        key_bindings: Vec::new(),
        left_alt_as_meta: false,
        right_alt_as_meta: false,
        enable_csi_u: false,
        enable_kitty_keyboard: false,
        mouse_reporting: false,
        scroll_on_output: false,
        scroll_on_keypress: false,
        answerback: String::new(),
    };
    let oracle = profile_fonts::resolve_projection(
        &profile,
        profile_fonts::saved_binding(&settings),
        17,
        None,
        Path::new("fixture"),
    );
    let scope = audit::AuditScope::start(ledger);
    let source = source(ledger, admission(), None, &settings);
    let metadata = source.copy_metadata(ledger, admission(), &AtomicBool::new(false))?;
    let second = metadata.try_clone()?;
    let metadata_bytes = metadata.admitted().rust_bytes;
    let builder = MetadataHeaderBuilder::new(HEADER, &EmptyMetadata, &[]);
    let reply = builder.try_build(ledger, admission(), metadata)?;
    let reply_bytes = reply.admitted().rust_bytes;
    let mut state = ReplyPortState::new(1, 2);
    let port_owner = succeeds(state.register(1, 2, 71, 0));
    assert_eq!(
        state.post(port_owner, reply, pin),
        PostOutcome::AcceptedByVm
    );
    let peer = CapturedPeer::take();
    drop(source);
    assert_eq!(ledger.snapshot().rust_bytes, metadata_bytes + reply_bytes);
    peer.finalize();
    assert_eq!(ledger.snapshot().rust_bytes, metadata_bytes);
    assert_eq!(second.value().metadata.profile_id, [1; 16]);
    assert_eq!(
        second.value().metadata.revision,
        oracle.configuration_revision
    );
    assert_eq!(second.value().metadata.raw_family, oracle.raw_family);
    assert_eq!(second.value().metadata.actual_family, oracle.actual_family);
    drop(second);
    assert_eq!(ledger.snapshot().rust_bytes, 0);
    scope.finish().assert_balanced_and_pregranted();
    Ok(())
}

fn reply_port_source_and_peer_order(
    pin: &LibraryPinProof,
    source_first: bool,
) -> Result<(), NumericReject> {
    let ledger = &PROTOCOL_ROOTS[if source_first { 1 } else { 2 }];
    // 原 borrowed legacy metadata 在审计之前构造；P1 自己的 captured 字段在 grant 后复制。
    let checked = succeeds(font_source::inspect(FONT, "fixture.ttf"));
    let reference_face = succeeds(font_source::inspect::face_for_qa(FONT));
    let reference_glyphs = reference_face.number_of_glyphs();
    assert!(reference_glyphs > 0);
    let legacy = fonts::ImportedFont {
        hash: checked.hash().to_owned(),
        label: checked.label().to_owned(),
    };
    let settings = TerminalSettingsV1::default();
    let scope = audit::AuditScope::start(ledger);
    let source = source(ledger, admission(), Some(&legacy), &settings);
    let source_bytes = source.admitted().rust_bytes;
    let prepared = source.try_clone()?;
    let runtime = source.try_clone()?;
    let attempt = source.try_clone()?;
    assert_eq!(
        source.bytes().map(|bytes| bytes.as_ptr()),
        runtime.bytes().map(|bytes| bytes.as_ptr())
    );
    let mut state = ReplyPortState::new(1, 2);
    let port_owner = succeeds(state.register(1, 2, 71, 0));
    let dto = Dto {
        key: "finite-projection",
        fields: &["family", "键\0🙂"],
    };
    let builder = MetadataHeaderBuilder::new(HEADER, &dto, source.bytes().unwrap_or_default());
    let reply = builder.try_build(ledger, admission(), source.try_clone()?)?;
    let reply_bytes = reply.admitted().rust_bytes;
    assert_eq!(
        state.post(port_owner, reply, pin),
        PostOutcome::AcceptedByVm
    );
    let peer = CapturedPeer::take();
    assert!(peer.bytes().ends_with(FONT));
    assert_eq!(ledger.snapshot().rust_bytes, source_bytes + reply_bytes);
    drop(prepared);
    assert_eq!(ledger.snapshot().rust_bytes, source_bytes + reply_bytes);
    state.shutdown();
    if source_first {
        drop(source);
        drop(runtime);
        drop(attempt);
        assert_eq!(ledger.snapshot().rust_bytes, source_bytes + reply_bytes);
        peer.finalize();
        assert_eq!(ledger.snapshot().rust_bytes, 0);
    } else {
        peer.finalize();
        assert_eq!(ledger.snapshot().rust_bytes, source_bytes);
        assert_eq!(runtime.bytes(), Some(FONT));
        let retained_face = succeeds(font_source::inspect::face_for_qa(
            runtime.bytes().unwrap_or_default(),
        ));
        assert_eq!(retained_face.number_of_glyphs(), reference_glyphs);
        drop(source);
        drop(runtime);
        assert_eq!(ledger.snapshot().rust_bytes, source_bytes);
        drop(attempt);
        assert_eq!(ledger.snapshot().rust_bytes, 0);
    }
    scope.finish().assert_balanced_and_pregranted();
    Ok(())
}

fn reply_port_old_epoch_peer_releases_only_its_actual_root(
    pin: &LibraryPinProof,
) -> Result<(), NumericReject> {
    let ledger = &PROTOCOL_ROOTS[3];
    let mut state = ReplyPortState::new(1, 2);
    let port_owner = succeeds(state.register(1, 2, 71, 0));
    let old_token = ledger.open_source(1, 0)?;
    let old_admission = Admission {
        epoch: 1,
        source: Some(old_token),
    };
    let settings = TerminalSettingsV1::default();
    let scope = audit::AuditScope::start(ledger);
    let old_source = source(ledger, old_admission, None, &settings);
    let old_bytes = old_source.admitted().rust_bytes;
    let builder = MetadataHeaderBuilder::new(HEADER, &EmptyMetadata, &[]);
    let reply = builder.try_build(ledger, old_admission, old_source)?;
    let reply_bytes = reply.admitted().rust_bytes;
    assert_eq!(
        state.post(port_owner, reply, pin),
        PostOutcome::AcceptedByVm
    );
    let old_peer = CapturedPeer::take();
    let old_unposted = builder.try_build(ledger, old_admission, ())?;
    let old_unposted_bytes = old_unposted.admitted().rust_bytes;
    ledger.invalidate_source(old_token)?;
    assert_eq!(
        ledger.snapshot().rust_bytes,
        old_bytes + reply_bytes + old_unposted_bytes
    );
    state.shutdown();
    let epoch = ledger.advance_epoch(1)?;
    succeeds(state.begin_epoch(epoch));
    let next_owner = succeeds(state.register(epoch, 2, 72, 0));
    let token = ledger.open_source(epoch, 0)?;
    let new_admission = Admission {
        epoch,
        source: Some(token),
    };
    let new_source = source(ledger, new_admission, None, &settings);
    let new_bytes = new_source.admitted().rust_bytes;
    assert_eq!(
        ledger.snapshot().rust_bytes,
        old_bytes + reply_bytes + new_bytes + old_unposted_bytes
    );
    let calls = FAKE.with(|cell| cell.get().calls);
    assert_eq!(
        state.post(next_owner, old_unposted, pin),
        PostOutcome::RegistrationRejected(ReplyPortReject::EpochMismatch)
    );
    assert_eq!(FAKE.with(|cell| cell.get().calls), calls);
    assert_eq!(
        ledger.snapshot().rust_bytes,
        old_bytes + reply_bytes + new_bytes
    );
    assert_eq!(
        ledger
            .try_reserve(Amount::default(), ResourceClass::Reply, old_admission)
            .err(),
        Some(NumericReject::EpochMismatch)
    );
    old_peer.finalize();
    assert_eq!(ledger.snapshot().rust_bytes, new_bytes);
    assert_eq!(succeeds(state.authorize(next_owner)).value(), 72);
    drop(new_source);
    assert_eq!(ledger.snapshot().rust_bytes, 0);
    scope.finish().assert_balanced_and_pregranted();
    Ok(())
}
