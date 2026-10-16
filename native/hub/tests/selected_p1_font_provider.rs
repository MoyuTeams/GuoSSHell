//! 独立 provider executable；真实缓存 I/O 与 allocator 审计，不注册正常 Prepare。

#![cfg(windows)]

#[path = "../src/selected_p1/font_provider/mod.rs"]
pub mod font_provider;
#[path = "../src/selected_p1/font_source/mod.rs"]
pub mod font_source;
#[path = "../src/selected_p1/model/layout.rs"]
pub mod layout;
#[path = "../src/selected_p1/resource/ledger.rs"]
pub mod ledger;
#[path = "../src/selected_p1/resource/owner.rs"]
pub mod owner;
#[path = "../src/selected_p1/resource/secret.rs"]
pub mod secret;
pub mod selected_p1 {
    pub use crate::{font_provider, font_source};
    pub mod model {
        pub use crate::layout;
    }
    pub mod resource {
        pub use crate::{ledger, owner, secret};
    }
}
#[path = "../src/selected_p1/font_source/audit.rs"]
pub mod audit;
#[path = "../src/selected_p1/font_source/fixture.rs"]
pub mod font_fixture;
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

use std::alloc::Layout;
use std::ffi::c_void;
use std::fs::File;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::FromRawHandle;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use font_provider::model::ResourceContract;
use font_provider::{BorrowedProfile, BudgetedCacheRoot, BudgetedLegacySlot, capture, probe};
use font_source::{FontBuildProof, FontMode, FontReject, GNU_FONT_BUILD, ResourceFailure};
use layout::NumericReject;
use ledger::{Admission, Amount, BYTE_LIMIT, CLASS_COUNT, ProcessLedger, ResourceClass};
use rshell_m0::rshell_core::TerminalSettingsV1;
use rshell_m0::rshell_storage::SqliteRepository;

const REGULAR: &[u8] = include_bytes!("../../../assets/fonts/MesloLGS-NF-Regular.ttf");
const BOLD: &[u8] = include_bytes!("../../../assets/fonts/MesloLGS-NF-Bold.ttf");

pub fn succeeds<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("预期成功：{error:?}"),
    }
}

fn proof() -> FontBuildProof {
    succeeds(FontBuildProof::verify(GNU_FONT_BUILD))
}
fn ledger(limit: usize) -> ProcessLedger {
    ProcessLedger::new(limit, [u32::MAX; CLASS_COUNT])
}
fn admission() -> Admission {
    Admission {
        epoch: 1,
        source: None,
    }
}
fn borrowed(settings: &TerminalSettingsV1) -> BorrowedProfile<'_> {
    BorrowedProfile {
        settings,
        profile_id: [7; 16],
        revision: 42,
    }
}
fn saved(mode: &str, family: &str, hash: Option<&str>) -> TerminalSettingsV1 {
    let mut settings = TerminalSettingsV1 {
        font_family: family.to_owned(),
        ..TerminalSettingsV1::default()
    };
    let mut value = serde_json::json!({"version":1,"mode":mode});
    if let Some(hash) = hash {
        value["hash"] = hash.into();
    }
    settings.extensions.insert("guosshell.font".into(), value);
    settings
}

struct FontsRepo {
    run: PathBuf,
    directory: PathBuf,
}
impl FontsRepo {
    fn new() -> Self {
        let parent = std::env::temp_dir().join("opencode");
        assert!(parent.is_dir());
        let run = parent.join(format!("guosh-p1-font-provider-{}", uuid::Uuid::new_v4()));
        succeeds(std::fs::create_dir(&run));
        let directory = run.join("fonts");
        succeeds(std::fs::create_dir(&directory));
        Self { run, directory }
    }
    fn publish(&self, bytes: &[u8]) -> fonts::ImportedFont {
        let font = succeeds(fonts::inspect(bytes, "cached.ttf"));
        succeeds(std::fs::write(
            self.directory.join(format!("{}.ttf", font.hash)),
            bytes,
        ));
        font
    }
    fn database(&self) -> SqliteRepository {
        let repository = succeeds(SqliteRepository::open(self.run.join("catalog.sqlite3")));
        succeeds(repository.migrate());
        repository
    }
    fn root<'a>(&self, ledger: &'a ProcessLedger) -> BudgetedCacheRoot<'a> {
        let path = succeeds(std::fs::canonicalize(&self.run));
        let wide_units = path.as_os_str().encode_wide().count() + 1;
        let scratch = succeeds(Layout::array::<u16>(wide_units)).size();
        let required = succeeds(BudgetedCacheRoot::required_rust(&path, scratch));
        let grant = succeeds(ledger.try_reserve(
            Amount {
                rust_bytes: required,
                c_bytes: 0,
                slots: 1,
            },
            ResourceClass::Source,
            admission(),
        ));
        succeeds(BudgetedCacheRoot::try_init_for_caller(
            grant,
            &path,
            scratch,
            || open_directory(&path),
        ))
    }
}
impl Drop for FontsRepo {
    fn drop(&mut self) {
        // 仅递归清理本轮 create_dir 成功的唯一 run 目录；没有外部资源或配置。
        succeeds(std::fs::remove_dir_all(&self.run));
    }
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateFileW(
        path: *const u16,
        access: u32,
        share: u32,
        security: *const c_void,
        disposition: u32,
        flags: u32,
        template: *mut c_void,
    ) -> *mut c_void;
    fn DeviceIoControl(
        handle: *mut c_void,
        code: u32,
        input: *const c_void,
        input_bytes: u32,
        output: *mut c_void,
        output_bytes: u32,
        returned: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
}

/// fixture 的 root-open 闭包：一个 exact UTF16 scratch，除此以外只有栈 FFI 参数/File。
/// 这不是 normal 平台 root/provider 闭包的证书。
fn open_directory(path: &Path) -> Result<File, NumericReject> {
    let mut wide = Vec::new();
    wide.try_reserve_exact(path.as_os_str().encode_wide().count() + 1)
        .map_err(|_| NumericReject::AllocationFailure)?;
    wide.extend(path.as_os_str().encode_wide());
    wide.push(0);
    // SAFETY: 有效完整 NUL UTF16；同步只读打开目录，无继承或写权限。
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            0x0012_0089,
            1,
            std::ptr::null(),
            3,
            0x0200_0000 | 0x0020_0000,
            std::ptr::null_mut(),
        )
    };
    if handle as isize == -1 {
        return Err(NumericReject::InvalidSource);
    }
    // SAFETY: 成功返回唯一句柄；所有路径由真实 File owner 关闭。
    Ok(unsafe { File::from_raw_handle(handle) })
}

#[test]
fn provider_real_fonts_and_repository_profile_use_same_full_metadata_and_bytes() {
    for bytes in [REGULAR, BOLD] {
        let fixture = FontsRepo::new();
        let font = fixture.publish(bytes);
        let repository = fixture.database();
        let configuration = succeeds(repository.load_configuration());
        let profile = &configuration.profiles[0];
        let ledger = ledger(BYTE_LIMIT);
        let root = fixture.root(&ledger);
        let slot = succeeds(BudgetedLegacySlot::try_initial(
            &ledger,
            admission(),
            9,
            Some(&font),
        ));
        let old = succeeds(
            profile_fonts::resolve_profile_font(
                &configuration,
                profile.id,
                Some(font.clone()),
                &fixture.run,
            )
            .ok_or("profile"),
        );
        // 此 oracle 只消费字体相关 DTO 字段，其余值是同一只读 fixture 的有限投影。
        let projected: settings_signals::DesktopTerminalProfile =
            succeeds(serde_json::from_value(serde_json::json!({
                "id": profile.id.0.to_string(), "name": profile.name, "version": 1,
                "terminal_type": "xterm-256color", "initial_cols": 80, "initial_rows": 24,
                "scrollback_lines": 10000, "font_family": profile.settings.font_family,
                "font_size": 14.0, "color_scheme": "default", "key_bindings": [],
                "left_alt_as_meta": false, "right_alt_as_meta": false,
                "enable_csi_u": false, "enable_kitty_keyboard": false, "mouse_reporting": true,
                "scroll_on_output": true, "scroll_on_keypress": true, "answerback": ""
            })));
        let old_projection = profile_fonts::resolve_projection(
            &projected,
            profile_fonts::saved_binding(&profile.settings),
            configuration.revision,
            Some(font.clone()),
            &fixture.run,
        );
        assert_eq!(old_projection, old.projection);
        let face = succeeds(font_source::inspect::face_for_qa(bytes));
        let old_face = succeeds(ttf_parser::Face::parse(bytes, 0));
        assert_eq!(face.number_of_glyphs(), old_face.number_of_glyphs());
        assert_eq!(face.glyph_index('A'), old_face.glyph_index('A'));
        let audit = audit::AuditScope::start(&ledger);
        probe::reset();
        let source = succeeds(capture(
            &proof(),
            &ledger,
            admission(),
            BorrowedProfile {
                settings: &profile.settings,
                profile_id: *profile.id.0.as_bytes(),
                revision: configuration.revision,
            },
            Some(&slot),
            root.borrowed(),
            &AtomicBool::new(false),
        ));
        assert_eq!(source.bytes(), Some(bytes));
        assert_eq!(
            source
                .body()
                .imported_font
                .as_ref()
                .map(|font| (&font.hash, &font.label)),
            Some((&font.hash, &font.label))
        );
        let metadata = &source.body().metadata;
        assert_eq!(metadata.profile_id, *profile.id.0.as_bytes());
        assert_eq!(metadata.revision, configuration.revision);
        assert_eq!(metadata.legacy_slot_version, 9);
        assert_eq!(metadata.actual_family, old.projection.actual_family);
        assert_eq!(metadata.raw_family, old.projection.raw_family);
        assert_eq!(
            metadata.resource_hash(),
            old.projection
                .resource
                .as_ref()
                .map(|resource| resource.hash.as_str())
        );
        assert_eq!(
            metadata
                .captured_terminal_slot
                .as_ref()
                .map(|font| (&font.hash, &font.label)),
            Some((&font.hash, &font.label))
        );
        assert_eq!(
            probe::counters(),
            probe::Counters {
                opens: 1,
                path_copies: 1,
                cleanups: 1
            }
        );
        drop(source);
        audit.finish().assert_balanced_and_pregranted();
        drop(slot);
        drop(root);
        assert_eq!(ledger.snapshot().rust_bytes, 0);
    }
}

#[test]
fn provider_explicit_family_and_unknown_metadata_do_not_open_or_read_a_slot() {
    let fixture = FontsRepo::new();
    succeeds(std::fs::remove_dir(&fixture.directory));
    let ledger = ledger(BYTE_LIMIT);
    let root = fixture.root(&ledger);
    let settings = saved("explicit_family", "已有系统字体", None);
    let mut unknown = settings.clone();
    unknown
        .extensions
        .get_mut("guosshell.font")
        .unwrap_or_else(|| panic!("namespace"))["version"] = 17.into();
    for (settings, mode, failure, actual) in [
        (&settings, FontMode::ExplicitFamily, None, "已有系统字体"),
        (
            &unknown,
            FontMode::Uneditable,
            Some(ResourceFailure::UneditableMetadata),
            "MesloLGS NF",
        ),
    ] {
        let audit = audit::AuditScope::start(&ledger);
        probe::reset();
        let source = succeeds(capture(
            &proof(),
            &ledger,
            admission(),
            borrowed(settings),
            None,
            root.borrowed(),
            &AtomicBool::new(false),
        ));
        assert_eq!(source.body().metadata.mode, mode);
        assert_eq!(source.body().metadata.fallback_reason, failure);
        assert_eq!(source.body().metadata.actual_family, actual);
        assert_eq!(source.body().metadata.raw_family, settings.font_family);
        assert_eq!(source.bytes(), None);
        assert_eq!(probe::counters(), probe::Counters::default());
        drop(source);
        audit.finish().assert_balanced_and_pregranted();
    }
}

#[test]
fn provider_legacy_inherit_and_cached_take_only_the_captured_identity() {
    let fixture = FontsRepo::new();
    let regular = fixture.publish(REGULAR);
    let bold = fixture.publish(BOLD);
    let ledger = ledger(BYTE_LIMIT);
    let root = fixture.root(&ledger);
    let slot = succeeds(BudgetedLegacySlot::try_initial(
        &ledger,
        admission(),
        23,
        Some(&regular),
    ));
    for settings in [
        TerminalSettingsV1::default(),
        saved("inherit_terminal_slot", "原始 family", None),
    ] {
        let source = succeeds(capture(
            &proof(),
            &ledger,
            admission(),
            borrowed(&settings),
            Some(&slot),
            root.borrowed(),
            &AtomicBool::new(false),
        ));
        let replacement = succeeds(slot.try_replacement(&ledger, admission(), Some(&bold)));
        assert_eq!(replacement.version(), 24);
        assert_eq!(source.body().metadata.legacy_slot_version, 23);
        assert_eq!(
            source.body().metadata.captured_resource_hash(),
            Some(regular.hash.as_str())
        );
        assert_eq!(source.bytes(), Some(REGULAR));
        drop(replacement);
    }
    let family = format!("GuoshFont_{}", bold.hash);
    let explicit = saved("explicit_cached_ttf", &family, Some(&bold.hash));
    probe::reset();
    let source = succeeds(capture(
        &proof(),
        &ledger,
        admission(),
        borrowed(&explicit),
        None,
        root.borrowed(),
        &AtomicBool::new(false),
    ));
    assert_eq!(source.bytes(), Some(BOLD));
    assert_eq!(
        source
            .body()
            .metadata
            .captured_terminal_slot
            .as_ref()
            .map(|font| &font.hash),
        None
    );
    assert_eq!(source.body().metadata.legacy_slot_version, 0);
    assert_eq!(probe::counters().opens, 1);
    let uppercase = bold.hash.to_ascii_uppercase();
    let mismatch = saved("explicit_cached_ttf", &family, Some(&uppercase));
    probe::reset();
    let failed = succeeds(capture(
        &proof(),
        &ledger,
        admission(),
        borrowed(&mismatch),
        None,
        root.borrowed(),
        &AtomicBool::new(false),
    ));
    assert_eq!(failed.body().metadata.mode, FontMode::Uneditable);
    assert_eq!(failed.body().metadata.raw_family, family);
    assert_eq!(probe::counters().opens, 0);
}

#[test]
fn provider_missing_corrupt_oversized_and_invalid_slot_preserve_configuration() {
    let fixture = FontsRepo::new();
    let font = succeeds(fonts::inspect(REGULAR, "cached.ttf"));
    let family = format!("GuoshFont_{}", font.hash);
    let settings = saved("explicit_cached_ttf", &family, Some(&font.hash));
    let ledger = ledger(BYTE_LIMIT);
    let root = fixture.root(&ledger);
    let path = fixture.directory.join(format!("{}.ttf", font.hash));
    for failure in [
        ResourceFailure::Missing,
        ResourceFailure::Corrupt,
        ResourceFailure::Corrupt,
    ] {
        let source = succeeds(capture(
            &proof(),
            &ledger,
            admission(),
            borrowed(&settings),
            None,
            root.borrowed(),
            &AtomicBool::new(false),
        ));
        assert_eq!(source.body().metadata.fallback_reason, Some(failure));
        assert_eq!(
            source.body().metadata.saved_hash.as_deref(),
            Some(font.hash.as_str())
        );
        assert_eq!(source.body().metadata.raw_family, family);
        assert_eq!(source.bytes(), None);
        if !path.exists() {
            succeeds(std::fs::write(&path, "损坏的字体".as_bytes()));
        } else {
            let file = succeeds(std::fs::OpenOptions::new().write(true).open(&path));
            succeeds(file.set_len(font_source::MAX_FONT_BYTES as u64 + 1));
        }
    }
    let invalid = fonts::ImportedFont {
        hash: "../outside".into(),
        label: "原槽".into(),
    };
    let slot = succeeds(BudgetedLegacySlot::try_initial(
        &ledger,
        admission(),
        11,
        Some(&invalid),
    ));
    probe::reset();
    let legacy = TerminalSettingsV1::default();
    let source = succeeds(capture(
        &proof(),
        &ledger,
        admission(),
        borrowed(&legacy),
        Some(&slot),
        root.borrowed(),
        &AtomicBool::new(false),
    ));
    assert_eq!(
        source.body().metadata.fallback_reason,
        Some(ResourceFailure::Corrupt)
    );
    assert_eq!(
        source
            .body()
            .metadata
            .captured_terminal_slot
            .as_ref()
            .map(|font| &font.hash),
        Some(&invalid.hash)
    );
    assert_eq!(probe::counters().opens, 0);
}

#[test]
fn provider_quota_before_open_has_no_owned_path_or_copy() {
    let fixture = FontsRepo::new();
    let font = fixture.publish(REGULAR);
    let ledger = ledger(BYTE_LIMIT);
    let root = fixture.root(&ledger);
    let settings = saved(
        "explicit_cached_ttf",
        &format!("GuoshFont_{}", font.hash),
        Some(&font.hash),
    );
    let occupied = ledger.snapshot().rust_bytes;
    let wrapper = succeeds(ResourceContract::actual(&proof())).open_rust_bytes;
    let blocker = succeeds(ledger.try_reserve(
        Amount {
            rust_bytes: BYTE_LIMIT - occupied - wrapper + 1,
            c_bytes: 0,
            slots: 0,
        },
        ResourceClass::Source,
        admission(),
    ));
    let audit = audit::AuditScope::start(&ledger);
    probe::reset();
    let result = capture(
        &proof(),
        &ledger,
        admission(),
        borrowed(&settings),
        None,
        root.borrowed(),
        &AtomicBool::new(false),
    );
    assert!(matches!(
        result,
        Err(FontReject::Budget(NumericReject::RustBudgetExceeded))
    ));
    assert_eq!(probe::counters(), probe::Counters::default());
    assert!(audit.finish().events().is_empty());
    drop(blocker);
}

#[test]
fn provider_tail_quota_failure_closes_already_admitted_open_without_read_or_source_copy() {
    let fixture = FontsRepo::new();
    let font = fixture.publish(REGULAR);
    let ledger = ledger(BYTE_LIMIT);
    let root = fixture.root(&ledger);
    let settings = saved(
        "explicit_cached_ttf",
        &format!("GuoshFont_{}", font.hash),
        Some(&font.hash),
    );
    let occupied = ledger.snapshot().rust_bytes;
    let wrapper = succeeds(ResourceContract::actual(&proof()));
    let blocker = succeeds(ledger.try_reserve(
        Amount {
            rust_bytes: BYTE_LIMIT - occupied - wrapper.open_rust_bytes,
            c_bytes: 0,
            slots: 0,
        },
        ResourceClass::Source,
        admission(),
    ));
    let audit = audit::AuditScope::start(&ledger);
    probe::reset();
    let result = capture(
        &proof(),
        &ledger,
        admission(),
        borrowed(&settings),
        None,
        root.borrowed(),
        &AtomicBool::new(false),
    );
    assert!(matches!(
        result,
        Err(FontReject::Budget(NumericReject::RustBudgetExceeded))
    ));
    assert_eq!(
        probe::counters(),
        probe::Counters {
            opens: 1,
            path_copies: 1,
            cleanups: 1
        }
    );
    let report = audit.finish();
    assert!(report.symmetric_single_backing());
    assert_eq!(report.events()[0].size, wrapper.read_wrapper.size());
    report.assert_balanced_and_pregranted();
    drop(blocker);
    assert_eq!(ledger.snapshot().rust_bytes, occupied);
}

#[test]
fn provider_short_read_and_growth_preserve_fallback() {
    let fixture = FontsRepo::new();
    let font = fixture.publish(REGULAR);
    let ledger = ledger(BYTE_LIMIT);
    let root = fixture.root(&ledger);
    let settings = saved(
        "explicit_cached_ttf",
        &format!("GuoshFont_{}", font.hash),
        Some(&font.hash),
    );
    for captured_length in [REGULAR.len() as u64 - 1, REGULAR.len() as u64 + 1] {
        probe::reset();
        probe::captured_length(captured_length);
        let audit = audit::AuditScope::start(&ledger);
        let source = succeeds(capture(
            &proof(),
            &ledger,
            admission(),
            borrowed(&settings),
            None,
            root.borrowed(),
            &AtomicBool::new(false),
        ));
        assert_eq!(
            source.body().metadata.fallback_reason,
            Some(ResourceFailure::Corrupt)
        );
        assert_eq!(
            source.body().metadata.saved_hash.as_deref(),
            Some(font.hash.as_str())
        );
        assert_eq!(source.bytes(), None);
        drop(source);
        audit.finish().assert_balanced_and_pregranted();
    }
}

#[test]
fn provider_safe_synthetic_font_checks_whole_input_name_and_hash() {
    let fixture = FontsRepo::new();
    let bytes = font_fixture::named(&font_fixture::utf16("合成字体完整名称"));
    let font = fixture.publish(&bytes);
    let ledger = ledger(BYTE_LIMIT);
    let root = fixture.root(&ledger);
    let slot = succeeds(BudgetedLegacySlot::try_initial(
        &ledger,
        admission(),
        6,
        Some(&font),
    ));
    let settings = TerminalSettingsV1::default();
    let source = succeeds(capture(
        &proof(),
        &ledger,
        admission(),
        borrowed(&settings),
        Some(&slot),
        root.borrowed(),
        &AtomicBool::new(false),
    ));
    assert_eq!(source.bytes(), Some(bytes.as_slice()));
    assert_eq!(
        source
            .body()
            .imported_font
            .as_ref()
            .map(|font| (&font.hash, &font.label)),
        Some((&font.hash, &font.label))
    );
    assert_eq!(font.label, "合成字体完整名称");
}

#[test]
fn provider_actual_layout_and_all_source_reply_release_orders_remain_charged() {
    let contract = succeeds(ResourceContract::actual(&proof()));
    assert_eq!(contract.filename_units, 74);
    assert_eq!((contract.file.size(), contract.file.align()), (8, 8));
    assert_eq!(
        (
            contract.file_identity.size(),
            contract.file_identity.align()
        ),
        (32, 8)
    );
    assert_eq!(
        (contract.read_wrapper.size(), contract.read_wrapper.align()),
        (200, 8)
    );
    assert_eq!(Layout::new::<font_source::SourceMetadata>().size(), 184);
    assert_eq!(Layout::new::<font_source::SourceBody>().size(), 264);
    assert_eq!(
        succeeds(owner::OwnerLayout::actual::<font_source::SourceBody>(0))
            .backing
            .size(),
        344
    );
    let fixture = FontsRepo::new();
    let font = fixture.publish(REGULAR);
    let settings = TerminalSettingsV1::default();
    let ledger = ledger(BYTE_LIMIT);
    let root = fixture.root(&ledger);
    let slot = succeeds(BudgetedLegacySlot::try_initial(
        &ledger,
        admission(),
        77,
        Some(&font),
    ));
    let baseline = ledger.snapshot();
    let owned_deallocations =
        |imported: Option<&fonts::ImportedFont>, metadata: &font_source::SourceMetadata| {
            let font_strings = |font: &fonts::ImportedFont| {
                usize::from(font.hash.capacity() != 0) + usize::from(font.label.capacity() != 0)
            };
            // tail 在同一 header backing 中；仅按真实非空 String backing 定位 drop 事件。
            1 + imported.map_or(0, font_strings)
                + metadata
                    .saved_hash
                    .as_ref()
                    .map_or(0, |hash| usize::from(hash.capacity() != 0))
                + usize::from(metadata.raw_family.capacity() != 0)
                + usize::from(metadata.actual_family.capacity() != 0)
                + metadata
                    .captured_terminal_slot
                    .as_ref()
                    .map_or(0, font_strings)
        };
    for a in 0..4 {
        for b in 0..4 {
            for c in 0..4 {
                if a == b || a == c || b == c {
                    continue;
                }
                let d = 6 - a - b - c;
                let audit = audit::AuditScope::start(&ledger);
                let source = succeeds(capture(
                    &proof(),
                    &ledger,
                    admission(),
                    borrowed(&settings),
                    Some(&slot),
                    root.borrowed(),
                    &AtomicBool::new(false),
                ));
                let copy = succeeds(source.try_clone());
                let metadata =
                    succeeds(source.copy_metadata(&ledger, admission(), &AtomicBool::new(false)));
                let send =
                    succeeds(source.copy_font_bytes(&ledger, admission(), &AtomicBool::new(false)));
                let source_amount = source.admitted();
                let metadata_amount = metadata.admitted();
                let send_amount = send.admitted();
                assert_eq!(copy.admitted(), source_amount);
                let source_deallocations = owned_deallocations(
                    source.body().imported_font.as_ref(),
                    &source.body().metadata,
                );
                let metadata_deallocations = owned_deallocations(
                    metadata.value().imported_font.as_ref(),
                    &metadata.value().metadata,
                );
                let expected = |source_alive, metadata_alive, send_alive| {
                    let mut expected = baseline;
                    for (class, amount, alive) in [
                        (ResourceClass::Source, source_amount, source_alive),
                        (ResourceClass::Reply, metadata_amount, metadata_alive),
                        (ResourceClass::Reply, send_amount, send_alive),
                    ] {
                        if alive {
                            expected.rust_bytes += amount.rust_bytes;
                            expected.c_bytes += amount.c_bytes;
                            let usage = &mut expected.classes[class as usize];
                            usage.rust_bytes += amount.rust_bytes;
                            usage.c_bytes += amount.c_bytes;
                            usage.slots += amount.slots;
                        }
                    }
                    expected
                };
                let mut source = Some(source);
                let mut copy = Some(copy);
                let mut metadata = Some(metadata);
                let mut send = Some(send);
                let mut drops = [(baseline, baseline, 0); 4];
                for (step, index) in [a, b, c, d].into_iter().enumerate() {
                    let before = expected(
                        source.is_some() || copy.is_some(),
                        metadata.is_some(),
                        send.is_some(),
                    );
                    assert_eq!(ledger.snapshot(), before);
                    match index {
                        0 => drop(source.take()),
                        1 => drop(copy.take()),
                        2 => drop(metadata.take()),
                        3 => drop(send.take()),
                        _ => panic!("release order"),
                    }
                    let after = expected(
                        source.is_some() || copy.is_some(),
                        metadata.is_some(),
                        send.is_some(),
                    );
                    assert_eq!(ledger.snapshot(), after);
                    let deallocations = match index {
                        0 | 1 if source.is_none() && copy.is_none() => source_deallocations,
                        0 | 1 => 0,
                        2 => metadata_deallocations,
                        3 => 1,
                        _ => panic!("release order"),
                    };
                    drops[step] = (before, after, deallocations);
                }
                assert_eq!(ledger.snapshot(), baseline);
                let report = audit.finish();
                report.assert_balanced_and_pregranted();
                let drop_events: usize = drops.iter().map(|(_, _, count)| count).sum();
                let mut cursor = succeeds(
                    report
                        .events()
                        .len()
                        .checked_sub(drop_events)
                        .ok_or("drop events"),
                );
                for (before, _, count) in drops {
                    for event in &report.events()[cursor..cursor + count] {
                        assert_eq!(event.kind, 2);
                        assert_eq!(event.held_rust, before.rust_bytes);
                        assert_eq!(
                            event.source_bytes,
                            before.classes[ResourceClass::Source as usize].rust_bytes,
                        );
                        assert_eq!(
                            event.reply_bytes,
                            before.classes[ResourceClass::Reply as usize].rust_bytes,
                        );
                    }
                    cursor += count;
                }
                assert_eq!(cursor, report.events().len());
                if [a, b, c, d] == [0, 1, 2, 3] {
                    eprintln!(
                        "实际持额：baseline={baseline:?} Source={source_amount:?} metadata={metadata_amount:?} send={send_amount:?}"
                    );
                    for (index, (before, after, count)) in drops.into_iter().enumerate() {
                        eprintln!(
                            "drop={index} deallocs={count} before=({}, {:?}, {:?}) after=({}, {:?}, {:?})",
                            before.rust_bytes,
                            before.classes[ResourceClass::Source as usize],
                            before.classes[ResourceClass::Reply as usize],
                            after.rust_bytes,
                            after.classes[ResourceClass::Source as usize],
                            after.classes[ResourceClass::Reply as usize],
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn provider_invalid_root_and_directory_blob_are_refused_without_outside_read() {
    let fixture = FontsRepo::new();
    let ledger = ledger(BYTE_LIMIT);
    for path in [
        Path::new("relative/fonts"),
        Path::new("C:\\owned\\..\\outside"),
        Path::new("C:\\owned\0\\fonts"),
    ] {
        let grant = succeeds(ledger.try_reserve(
            Amount {
                rust_bytes: 4096,
                c_bytes: 0,
                slots: 1,
            },
            ResourceClass::Source,
            admission(),
        ));
        let called = std::cell::Cell::new(false);
        let result = BudgetedCacheRoot::try_init_for_caller(grant, path, 0, || {
            called.set(true);
            Err(NumericReject::ConstructionFailure)
        });
        assert!(matches!(result, Err(NumericReject::InvalidSource)));
        assert!(!called.get());
        assert_eq!(ledger.snapshot().rust_bytes, 0);
    }
    let font = succeeds(fonts::inspect(REGULAR, "cached.ttf"));
    succeeds(std::fs::create_dir(
        fixture.directory.join(format!("{}.ttf", font.hash)),
    ));
    let root = fixture.root(&ledger);
    let settings = saved(
        "explicit_cached_ttf",
        &format!("GuoshFont_{}", font.hash),
        Some(&font.hash),
    );
    let source = succeeds(capture(
        &proof(),
        &ledger,
        admission(),
        borrowed(&settings),
        None,
        root.borrowed(),
        &AtomicBool::new(false),
    ));
    assert_eq!(
        source.body().metadata.fallback_reason,
        Some(ResourceFailure::Unavailable)
    );
    assert_eq!(source.bytes(), None);
}

#[test]
fn provider_pins_real_file_against_write_and_delete_until_cleanup() {
    let fixture = FontsRepo::new();
    let font = fixture.publish(REGULAR);
    let ledger = ledger(BYTE_LIMIT);
    let root = fixture.root(&ledger);
    let settings = saved(
        "explicit_cached_ttf",
        &format!("GuoshFont_{}", font.hash),
        Some(&font.hash),
    );
    probe::reset();
    probe::check_write_and_delete_pinning();
    let audit = audit::AuditScope::start(&ledger);
    let source = succeeds(capture(
        &proof(),
        &ledger,
        admission(),
        borrowed(&settings),
        None,
        root.borrowed(),
        &AtomicBool::new(false),
    ));
    assert_eq!(probe::pin_status(), Some((0xc000_0043, 0xc000_0043)));
    assert_eq!(probe::counters().cleanups, 1);
    assert_eq!(source.bytes(), Some(REGULAR));
    drop(source);
    audit.finish().assert_balanced_and_pregranted();
    let writer = succeeds(
        std::fs::OpenOptions::new()
            .write(true)
            .open(fixture.directory.join(format!("{}.ttf", font.hash))),
    );
    drop(writer);
}

#[test]
fn provider_allocation_failure_cancel_and_missing_slot_do_not_open() {
    let fixture = FontsRepo::new();
    let font = fixture.publish(REGULAR);
    let ledger = ledger(BYTE_LIMIT);
    let root = fixture.root(&ledger);
    let settings = saved(
        "explicit_cached_ttf",
        &format!("GuoshFont_{}", font.hash),
        Some(&font.hash),
    );
    let wrapper = succeeds(ResourceContract::actual(&proof()));
    let baseline = ledger.snapshot().rust_bytes;
    probe::reset();
    let audit = audit::AuditScope::start(&ledger);
    audit.fail_next_size(wrapper.read_wrapper.size());
    assert!(matches!(
        capture(
            &proof(),
            &ledger,
            admission(),
            borrowed(&settings),
            None,
            root.borrowed(),
            &AtomicBool::new(false)
        ),
        Err(FontReject::Budget(NumericReject::AllocationFailure))
    ));
    assert_eq!(probe::counters(), probe::Counters::default());
    audit.finish().assert_balanced_and_pregranted();
    assert_eq!(ledger.snapshot().rust_bytes, baseline);

    let cancel = AtomicBool::new(false);
    let audit = audit::AuditScope::start(&ledger);
    audit.cancel_on_allocation(&cancel, 1);
    assert!(matches!(
        capture(
            &proof(),
            &ledger,
            admission(),
            borrowed(&settings),
            None,
            root.borrowed(),
            &cancel
        ),
        Err(FontReject::Budget(NumericReject::Cancelled))
    ));
    assert_eq!(
        probe::counters(),
        probe::Counters {
            opens: 0,
            path_copies: 1,
            cleanups: 1
        }
    );
    audit.finish().assert_balanced_and_pregranted();
    assert_eq!(ledger.snapshot().rust_bytes, baseline);

    let settings = TerminalSettingsV1::default();
    let audit = audit::AuditScope::start(&ledger);
    probe::reset();
    assert!(matches!(
        capture(
            &proof(),
            &ledger,
            admission(),
            borrowed(&settings),
            None,
            root.borrowed(),
            &AtomicBool::new(false)
        ),
        Err(FontReject::Budget(NumericReject::InvalidSource))
    ));
    assert!(audit.finish().events().is_empty());
    assert_eq!(probe::counters(), probe::Counters::default());
}

#[test]
fn provider_empty_slot_reset_and_version_overflow_keep_exact_identity() {
    let fixture = FontsRepo::new();
    let font = fixture.publish(REGULAR);
    let ledger = ledger(BYTE_LIMIT);
    let root = fixture.root(&ledger);
    let settings = TerminalSettingsV1::default();
    let first = succeeds(BudgetedLegacySlot::try_initial(
        &ledger,
        admission(),
        8,
        Some(&font),
    ));
    let source = succeeds(capture(
        &proof(),
        &ledger,
        admission(),
        borrowed(&settings),
        Some(&first),
        root.borrowed(),
        &AtomicBool::new(false),
    ));
    let reset = succeeds(first.try_replacement(&ledger, admission(), None));
    assert_eq!(reset.version(), 9);
    drop(first);
    probe::reset();
    let empty = succeeds(capture(
        &proof(),
        &ledger,
        admission(),
        borrowed(&settings),
        Some(&reset),
        root.borrowed(),
        &AtomicBool::new(false),
    ));
    assert_eq!(empty.body().metadata.legacy_slot_version, 9);
    assert_eq!(empty.body().metadata.fallback_reason, None);
    assert_eq!(empty.bytes(), None);
    assert_eq!(probe::counters(), probe::Counters::default());
    assert_eq!(source.body().metadata.legacy_slot_version, 8);
    assert_eq!(source.bytes(), Some(REGULAR));
    let terminal = succeeds(BudgetedLegacySlot::try_initial(
        &ledger,
        admission(),
        u64::MAX,
        Some(&font),
    ));
    let audit = audit::AuditScope::start(&ledger);
    assert!(matches!(
        terminal.try_replacement(&ledger, admission(), None),
        Err(NumericReject::Overflow)
    ));
    assert!(audit.finish().events().is_empty());
    assert_eq!(terminal.font().map(|font| &font.hash), Some(&font.hash));
}

struct Junction {
    path: PathBuf,
}
impl Junction {
    fn create(path: PathBuf, target: &Path) -> Self {
        succeeds(std::fs::create_dir(&path));
        let junction = Self { path };
        let canonical = succeeds(std::fs::canonicalize(target));
        let target_units: Vec<u16> = canonical.as_os_str().encode_wide().collect();
        assert!(target_units.starts_with(&[92, 92, 63, 92]));
        let mut substitute: Vec<u16> = r"\??\".encode_utf16().collect();
        substitute.extend_from_slice(&target_units[4..]);
        let print = &target_units[4..];
        let substitute_bytes = substitute.len() * 2;
        let print_bytes = print.len() * 2;
        let mut buffer = vec![0; 16];
        buffer[..4].copy_from_slice(&0xa000_0003_u32.to_le_bytes());
        buffer[8..10].copy_from_slice(&0_u16.to_le_bytes());
        buffer[10..12].copy_from_slice(&succeeds(u16::try_from(substitute_bytes)).to_le_bytes());
        buffer[12..14]
            .copy_from_slice(&succeeds(u16::try_from(substitute_bytes + 2)).to_le_bytes());
        buffer[14..16].copy_from_slice(&succeeds(u16::try_from(print_bytes)).to_le_bytes());
        for unit in substitute
            .iter()
            .chain(std::iter::once(&0))
            .chain(print)
            .chain(std::iter::once(&0))
        {
            buffer.extend_from_slice(&unit.to_le_bytes());
        }
        let data_bytes = succeeds(u16::try_from(buffer.len() - 8));
        buffer[4..6].copy_from_slice(&data_bytes.to_le_bytes());
        let mut wide: Vec<u16> = junction.path.as_os_str().encode_wide().collect();
        wide.push(0);
        // SAFETY: 仅打开刚 create_dir 成功的本轮目录，用于设置测试 mount-point reparse。
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                0x4000_0000,
                7,
                std::ptr::null(),
                3,
                0x0200_0000 | 0x0020_0000,
                std::ptr::null_mut(),
            )
        };
        assert_ne!(handle as isize, -1);
        // SAFETY: 唯一有效 owned 目录句柄，File 负责关闭。
        let file = unsafe { File::from_raw_handle(handle) };
        let mut returned = 0;
        use std::os::windows::io::AsRawHandle;
        // SAFETY: 完整 mount-point buffer 与同步输出；不启用 symlink 权限或修改外部路径。
        let result = unsafe {
            DeviceIoControl(
                file.as_raw_handle(),
                0x0009_00a4,
                buffer.as_ptr().cast(),
                succeeds(u32::try_from(buffer.len())),
                std::ptr::null_mut(),
                0,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        assert_ne!(result, 0, "本轮 junction fixture 无法建立");
        drop(file);
        junction
    }
}
impl Drop for Junction {
    fn drop(&mut self) {
        // 精确删除本轮 junction 本身，先于 run 的递归清理；不遍历其 target。
        succeeds(std::fs::remove_dir(&self.path));
    }
}

#[test]
fn provider_reparse_root_and_blob_do_not_follow_outside_the_trusted_directory() {
    let fixture = FontsRepo::new();
    let outside = fixture.run.join("outside-cache");
    succeeds(std::fs::create_dir(&outside));
    let font = succeeds(fonts::inspect(REGULAR, "cached.ttf"));
    succeeds(std::fs::write(
        outside.join(format!("{}.ttf", font.hash)),
        REGULAR,
    ));
    let redirected_root = Junction::create(fixture.run.join("redirected-cache"), &outside);
    let ledger = ledger(BYTE_LIMIT);
    let scratch = succeeds(Layout::array::<u16>(
        redirected_root.path.as_os_str().encode_wide().count() + 1,
    ))
    .size();
    let required = succeeds(BudgetedCacheRoot::required_rust(
        &redirected_root.path,
        scratch,
    ));
    let grant = succeeds(ledger.try_reserve(
        Amount {
            rust_bytes: required,
            c_bytes: 0,
            slots: 1,
        },
        ResourceClass::Source,
        admission(),
    ));
    let result =
        BudgetedCacheRoot::try_init_for_caller(grant, &redirected_root.path, scratch, || {
            open_directory(&redirected_root.path)
        });
    assert!(matches!(result, Err(NumericReject::InvalidSource)));
    assert_eq!(ledger.snapshot().rust_bytes, 0);

    let redirected_blob = Junction::create(
        fixture.directory.join(format!("{}.ttf", font.hash)),
        &outside,
    );
    let root = fixture.root(&ledger);
    let settings = saved(
        "explicit_cached_ttf",
        &format!("GuoshFont_{}", font.hash),
        Some(&font.hash),
    );
    probe::reset();
    let audit = audit::AuditScope::start(&ledger);
    let source = succeeds(capture(
        &proof(),
        &ledger,
        admission(),
        borrowed(&settings),
        None,
        root.borrowed(),
        &AtomicBool::new(false),
    ));
    assert_eq!(
        source.body().metadata.fallback_reason,
        Some(ResourceFailure::Unavailable)
    );
    assert_eq!(source.bytes(), None);
    assert_eq!(probe::counters().cleanups, 1);
    drop(source);
    audit.finish().assert_balanced_and_pregranted();
    drop(root);
    drop(redirected_blob);
    drop(redirected_root);
    assert_eq!(
        succeeds(std::fs::read(outside.join(format!("{}.ttf", font.hash)))),
        REGULAR
    );
}

#[test]
fn provider_reparse_fonts_component_cannot_escape_a_valid_root_handle() {
    let fixture = FontsRepo::new();
    succeeds(std::fs::remove_dir(&fixture.directory));
    let outside = fixture.run.join("outside-cache");
    succeeds(std::fs::create_dir(&outside));
    let font = succeeds(fonts::inspect(REGULAR, "cached.ttf"));
    succeeds(std::fs::write(
        outside.join(format!("{}.ttf", font.hash)),
        REGULAR,
    ));
    let junction = Junction::create(fixture.directory.clone(), &outside);
    let ledger = ledger(BYTE_LIMIT);
    let root = fixture.root(&ledger);
    let settings = saved(
        "explicit_cached_ttf",
        &format!("GuoshFont_{}", font.hash),
        Some(&font.hash),
    );
    let audit = audit::AuditScope::start(&ledger);
    let source = succeeds(capture(
        &proof(),
        &ledger,
        admission(),
        borrowed(&settings),
        None,
        root.borrowed(),
        &AtomicBool::new(false),
    ));
    assert_eq!(
        source.body().metadata.fallback_reason,
        Some(ResourceFailure::Unavailable)
    );
    assert_eq!(source.bytes(), None);
    drop(source);
    audit.finish().assert_balanced_and_pregranted();
    drop(root);
    drop(junction);
    assert_eq!(
        succeeds(std::fs::read(outside.join(format!("{}.ttf", font.hash)))),
        REGULAR
    );
}
