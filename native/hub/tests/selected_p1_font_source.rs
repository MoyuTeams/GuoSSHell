//! 独立 W1-F executable；原字体模块仅作真实 oracle，不注册正常 P1/lib 入口。

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

#[path = "../src/selected_p1/font_source/audit.rs"]
pub mod audit;
#[path = "../src/selected_p1/font_source/fixture.rs"]
pub mod fixture;
#[path = "../src/selected_p1/font_source/mod.rs"]
pub mod font_source;
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
#[cfg(all(
    target_os = "windows",
    target_env = "gnu",
    target_pointer_width = "64",
    target_endian = "little",
    panic = "unwind"
))]
use std::path::Path;
use std::path::PathBuf;
#[cfg(all(
    target_os = "windows",
    target_env = "gnu",
    target_pointer_width = "64",
    target_endian = "little",
    panic = "unwind"
))]
use std::sync::atomic::{AtomicBool, Ordering};

use font_source::closure::FontInspectionProof;
#[cfg(all(
    target_os = "windows",
    target_env = "gnu",
    target_pointer_width = "64",
    target_endian = "little",
    panic = "unwind"
))]
use font_source::{
    BorrowedCapture, FontAvailability, FontMode, ResourceFailure, SourceOwner, TrustedRoot,
    capture_bytes, capture_file,
};
use font_source::{FontBuildProof, FontReject, GNU_FONT_BUILD, SourceLayout, inspect};
use layout::NumericReject;
#[cfg(all(
    target_os = "windows",
    target_env = "gnu",
    target_pointer_width = "64",
    target_endian = "little",
    panic = "unwind"
))]
use ledger::{Admission, Amount, ResourceClass};
use ledger::{BYTE_LIMIT, CLASS_COUNT, ProcessLedger};
use rshell_m0::rshell_core::TerminalSettingsV1;

const REGULAR: &[u8] = include_bytes!("../../../assets/fonts/MesloLGS-NF-Regular.ttf");
const BOLD: &[u8] = include_bytes!("../../../assets/fonts/MesloLGS-NF-Bold.ttf");

pub fn succeeds<T, E: std::fmt::Debug>(value: Result<T, E>) -> T {
    match value {
        Ok(value) => value,
        Err(error) => panic!("预期成功：{error:?}"),
    }
}

#[cfg(all(
    target_os = "windows",
    target_env = "gnu",
    target_pointer_width = "64",
    target_endian = "little",
    panic = "unwind"
))]
fn certificate() -> FontBuildProof {
    succeeds(FontBuildProof::verify(GNU_FONT_BUILD))
}
fn root(limit: usize) -> ProcessLedger {
    ProcessLedger::new(limit, [u32::MAX; CLASS_COUNT])
}
#[cfg(all(
    target_os = "windows",
    target_env = "gnu",
    target_pointer_width = "64",
    target_endian = "little",
    panic = "unwind"
))]
fn admission() -> Admission {
    Admission {
        epoch: 1,
        source: None,
    }
}
#[cfg(all(
    target_os = "windows",
    target_env = "gnu",
    target_pointer_width = "64",
    target_endian = "little",
    panic = "unwind"
))]
fn input<'a>(
    settings: &'a TerminalSettingsV1,
    slot: Option<&'a fonts::ImportedFont>,
    path: &'a Path,
) -> BorrowedCapture<'a> {
    BorrowedCapture {
        settings,
        profile_id: *uuid::Uuid::nil().as_bytes(),
        revision: 17,
        legacy_slot_version: 3,
        captured_slot: slot,
        trusted_root: TrustedRoot::from_root_caller(path),
    }
}
fn heapless<T>(call: impl FnOnce() -> T) -> T {
    let ledger = root(0);
    let audit = audit::AuditScope::start(&ledger);
    let value = call();
    let report = audit.finish();
    assert!(!report.overflow);
    assert!(report.events().is_empty());
    value
}

fn differential(bytes: &[u8], name: &str) {
    let old = fonts::inspect(bytes, name);
    #[cfg(all(
        target_os = "windows",
        target_env = "gnu",
        target_pointer_width = "64",
        target_endian = "little",
        panic = "unwind"
    ))]
    let new = {
        let proof = succeeds(FontInspectionProof::verify(GNU_FONT_BUILD));
        heapless(|| proof.inspect(bytes, name))
    };
    #[cfg(not(all(
        target_os = "windows",
        target_env = "gnu",
        target_pointer_width = "64",
        target_endian = "little",
        panic = "unwind"
    )))]
    let new = heapless(|| inspect(bytes, name));
    match (old, new) {
        (Ok(old), Ok(new)) => {
            assert_eq!(old.hash, new.hash());
            assert_eq!(old.label, new.label());
        }
        (Err(_), Err(_)) => {}
        _ => panic!("真实旧 inspect 与借用检查不一致"),
    }
}

#[derive(Default)]
struct Outline(Vec<(u8, [f32; 6])>);

impl ttf_parser::OutlineBuilder for Outline {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.push((1, [x, y, 0., 0., 0., 0.]));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.push((2, [x, y, 0., 0., 0., 0.]));
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.0.push((3, [x1, y1, x, y, 0., 0.]));
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.0.push((4, [x1, y1, x2, y2, x, y]));
    }
    fn close(&mut self) {
        self.0.push((5, [0.; 6]));
    }
}

// 只审计与生产相同的初始构造；glyph/bbox/outline 的 Vec 与惰性解析是审计外的语义 QA。
fn face_differential(bytes: &[u8]) {
    let old = succeeds(ttf_parser::Face::parse(bytes, 0));
    let new = heapless(|| succeeds(font_source::inspect::face_for_qa(bytes)));
    assert_eq!(old.number_of_glyphs(), new.number_of_glyphs());
    assert_eq!(old.global_bounding_box(), new.global_bounding_box());
    assert_eq!(old.variation_coordinates(), new.variation_coordinates());
    let glyph = succeeds(old.glyph_index('A').ok_or("glyph A"));
    assert_eq!(new.glyph_index('A'), Some(glyph));
    assert_eq!(glyph_formats(&old), glyph_formats(&new));
    for id in [ttf_parser::GlyphId(0), glyph] {
        assert_eq!(
            old.tables().glyf.and_then(|glyf| glyf.bbox(id)),
            new.tables().glyf.and_then(|glyf| glyf.bbox(id))
        );
        let mut before = Outline::default();
        let mut after = Outline::default();
        assert_eq!(
            old.outline_glyph(id, &mut before),
            new.outline_glyph(id, &mut after)
        );
        assert_eq!(before.0, after.0);
        assert_eq!(old.glyph_bounding_box(id), new.glyph_bounding_box(id));
    }
}

fn glyph_formats(face: &ttf_parser::Face<'_>) -> u32 {
    use ttf_parser::cmap::Format;
    let mut formats = 0;
    for subtable in succeeds(face.tables().cmap.ok_or("cmap")).subtables {
        let format = match subtable.format {
            Format::ByteEncodingTable(_) => 0,
            Format::HighByteMappingThroughTable(_) => 2,
            Format::SegmentMappingToDeltaValues(_) => 4,
            Format::TrimmedTableMapping(_) => 6,
            Format::MixedCoverage => 8,
            Format::TrimmedArray(_) => 10,
            Format::SegmentedCoverage(_) => 12,
            Format::ManyToOneRangeMappings(_) => 13,
            Format::UnicodeVariationSequences(_) => 14,
        };
        formats |= 1 << format;
        if subtable.is_unicode() && subtable.glyph_index(u32::from('A')).is_some() {
            break;
        }
    }
    formats
}

fn whole_hash(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(all(
    target_os = "windows",
    target_env = "gnu",
    target_pointer_width = "64",
    target_endian = "little",
    panic = "unwind"
))]
fn assert_source_bytes_and_metadata(bytes: &[u8], font: &fonts::ImportedFont) {
    let fixture = Fixture::new();
    let settings = TerminalSettingsV1::default();
    let certificate = certificate();
    let ledger = root(BYTE_LIMIT);
    let cancel = AtomicBool::new(false);
    let audit = audit::AuditScope::start(&ledger);
    let source = succeeds(capture_bytes(
        &certificate,
        &ledger,
        admission(),
        input(&settings, Some(font), &fixture.0),
        FontAvailability::Bytes(bytes),
        &cancel,
    ));
    let report = audit.finish();
    assert_eq!(source.bytes(), Some(bytes));
    let imported = succeeds(source.body().imported_font.as_ref().ok_or("ImportedFont"));
    assert_eq!(imported.hash, font.hash);
    assert_eq!(imported.label, font.label);
    let metadata = &source.body().metadata;
    assert_eq!(metadata.profile_id, [0; 16]);
    assert_eq!(metadata.revision, 17);
    assert_eq!(metadata.legacy_slot_version, 3);
    assert_eq!(metadata.version, 1);
    assert_eq!(metadata.mode, FontMode::LegacyUndetermined);
    assert!(metadata.uneditable_reason.is_none());
    assert!(metadata.saved_hash.is_none());
    assert_eq!(metadata.raw_family, settings.font_family);
    assert_eq!(metadata.actual_family, format!("GuoshFont_{}", font.hash));
    assert_eq!(
        metadata.runtime_source,
        font_source::source::RuntimeSource::LegacyTerminalSlot
    );
    assert!(metadata.fallback_reason.is_none());
    assert_eq!(metadata.resource_hash(), Some(font.hash.as_str()));
    assert_eq!(metadata.captured_resource_hash(), Some(font.hash.as_str()));
    let captured = succeeds(
        metadata
            .captured_terminal_slot
            .as_ref()
            .ok_or("captured slot"),
    );
    assert_eq!(captured.hash, font.hash);
    assert_eq!(captured.label, font.label);
    assert!(
        report
            .events()
            .iter()
            .all(|event| event.held_rust >= event.size && event.source_bytes > 0)
    );
    drop(source);
    assert_eq!(ledger.snapshot().rust_bytes, 0);
}

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let parent = std::env::temp_dir();
        assert!(parent.is_dir(), "测试只使用预先存在的标准临时根");
        let path = parent.join(format!("font-source-{}", uuid::Uuid::new_v4()));
        succeeds(std::fs::create_dir(&path));
        let fixture = Self(path);
        succeeds(std::fs::create_dir(fixture.0.join("fonts")));
        fixture
    }
    fn publish(&self, bytes: &[u8]) -> fonts::ImportedFont {
        let font = succeeds(fonts::inspect(bytes, "cached.ttf"));
        succeeds(std::fs::write(
            self.0.join("fonts").join(format!("{}.ttf", font.hash)),
            bytes,
        ));
        font
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        succeeds(std::fs::remove_dir_all(&self.0));
    }
}

#[test]
fn font_source_real_fonts_and_complete_synthetic_glyph_route_are_heapless() {
    for bytes in [REGULAR, BOLD] {
        differential(bytes, "目录/字体.TtF");
        face_differential(bytes);
    }
    let font = fixture::named(&fixture::utf16("测试😀字体"));
    let face = succeeds(ttf_parser::Face::parse(&font, 0));
    assert_eq!(face.number_of_glyphs(), 2);
    assert_eq!(face.glyph_index('A'), Some(ttf_parser::GlyphId(1)));
    assert_eq!(glyph_formats(&face), 1);
    let mut outline = Outline::default();
    assert!(
        face.outline_glyph(ttf_parser::GlyphId(1), &mut outline)
            .is_some()
    );
    assert!(outline.0.len() >= 4);
    face_differential(&font);
    differential(&font, "chosen.ttf");
}

#[test]
fn font_source_optional_bytes_preserve_business_values_and_variable_defaults() {
    let bytes = fixture::with_optional_tables();
    differential(&bytes, "complete.ttf");
    face_differential(&bytes);
    let face = succeeds(font_source::inspect::face_for_qa(&bytes));
    assert_eq!(face.variation_coordinates().len(), 1);
    assert!(face.tables().gvar.is_some());
    assert!(
        face.outline_glyph(ttf_parser::GlyphId(1), &mut Outline::default())
            .is_some()
    );
    #[cfg(all(
        target_os = "windows",
        target_env = "gnu",
        target_pointer_width = "64",
        target_endian = "little",
        panic = "unwind"
    ))]
    assert_source_bytes_and_metadata(&bytes, &succeeds(fonts::inspect(&bytes, "complete.ttf")));
}

#[test]
fn font_source_utf16_all_units_and_name_candidates_match_the_actual_old_route() {
    for unit in 0..=u16::MAX {
        let data = unit.to_be_bytes();
        let name = ttf_parser::name::Name {
            platform_id: ttf_parser::PlatformId::Unicode,
            encoding_id: 0,
            language_id: 0,
            name_id: 1,
            name: &data,
        };
        let old = name.to_string().map(|text| {
            text.chars()
                .filter(|ch| !ch.is_control())
                .take(128)
                .collect::<String>()
        });
        let new = heapless(|| font_source::inspect::name_label(name));
        let new = new
            .as_ref()
            .map(|(buffer, len)| succeeds(std::str::from_utf8(&buffer[..*len])));
        assert_eq!(old.as_deref(), new);
    }
    let prefix = fixture::utf16(&"😀".repeat(128));
    let mut invalid_tail = prefix.clone();
    invalid_tail.extend_from_slice(&[0xd8, 0]);
    let good = fixture::utf16("后备");
    for first in [
        &[][..],
        &[0, 0, 0, 10][..],
        &prefix[..],
        &invalid_tail[..],
        &[0, 65, 0xff][..],
    ] {
        let font = fixture::font(fixture::tables(fixture::names(&[
            (3, 1, 1, first),
            (0, 5, 1, &good),
        ])));
        differential(&font, "fallback.ttf");
    }
    let maximum = vec![0; usize::from(u16::MAX)];
    differential(&fixture::named(&maximum), "maximum.ttf");
    let mut bad_record = fixture::names(&[(9, 1, 1, &good), (3, 1, 1, &good)]);
    let font = fixture::font(fixture::tables(bad_record.clone()));
    differential(&font, "目录\\原名.TTF");
    assert_eq!(
        succeeds(inspect(&font, "目录\\原名.TTF")).label(),
        "原名.TTF"
    );
    fixture::put16(&mut bad_record, 6, 0);
    fixture::put16(&mut bad_record, 16, u16::MAX);
    differential(
        &fixture::font(fixture::tables(bad_record)),
        "a/b/fallback.ttf",
    );
}

#[test]
fn font_source_reject_classes_and_order_preserve_the_real_glyph_checks() {
    let valid = fixture::named(&fixture::utf16("valid"));
    let large = vec![0; font_source::MAX_FONT_BYTES + 1];
    for (bytes, name, expected) in [
        (&large[..], "bad.otf", FontReject::TooLarge),
        (b"bad".as_slice(), "bad.otf", FontReject::Extension),
        (b"bad".as_slice(), "good.ttf", FontReject::Signature),
        (&[0, 1, 0, 0][..], "good.ttf", FontReject::Face),
    ] {
        assert_eq!(heapless(|| inspect(bytes, name)).err(), Some(expected));
        differential(bytes, name);
    }
    for tag in [*b"head", *b"hhea", *b"maxp", *b"glyf", *b"loca", *b"cmap"] {
        let mut tables = fixture::tables(fixture::names(&[]));
        tables.retain(|(name, _)| name != &tag);
        let bytes = fixture::font(tables);
        let expected = if [*b"head", *b"hhea", *b"maxp"].contains(&tag) {
            FontReject::Face
        } else {
            FontReject::GlyphTables
        };
        assert_eq!(
            heapless(|| inspect(&bytes, "cached.ttf")).err(),
            Some(expected)
        );
        differential(&bytes, "cached.ttf");
    }
    for len in 0..valid.len() {
        differential(&valid[..len], "truncated.ttf");
    }
    let mut tables = fixture::tables(fixture::names(&[]));
    for (tag, data) in &mut tables {
        if tag == b"maxp" {
            fixture::put16(data, 4, 0);
        }
    }
    let bytes = fixture::font(tables);
    assert_eq!(
        heapless(|| inspect(&bytes, "cached.ttf")).err(),
        Some(FontReject::Face)
    );
    differential(&bytes, "cached.ttf");
}

fn table_window(bytes: &mut [u8], tag: [u8; 4], occurrence: usize) {
    let count = usize::from(u16::from_be_bytes([bytes[4], bytes[5]]));
    let mut seen = 0;
    for record in bytes[12..12 + count * 16].as_chunks_mut::<16>().0 {
        if record[..4] == tag {
            if seen == occurrence {
                fixture::put32(record, 8, u32::MAX);
                fixture::put32(record, 12, u32::MAX);
                return;
            }
            seen += 1;
        }
    }
    panic!("fixture 缺少 table occurrence");
}

fn variable_tables(axes: u16, axes_offset: u16) -> Vec<([u8; 4], Vec<u8>)> {
    let mut tables = fixture::tables(fixture::names(&[(
        3,
        1,
        1,
        &fixture::utf16("默认变量字体"),
    )]));
    let mut fvar = vec![0; usize::from(axes_offset) + usize::from(axes) * 20];
    fixture::put32(&mut fvar, 0, 0x10000);
    fixture::put16(&mut fvar, 4, axes_offset);
    fixture::put16(&mut fvar, 6, 2);
    fixture::put16(&mut fvar, 8, axes);
    fixture::put16(&mut fvar, 10, 20);
    for record in fvar[usize::from(axes_offset)..].as_chunks_mut::<20>().0 {
        record[..4].copy_from_slice(b"wght");
        fixture::put32(record, 12, 0x10000);
        fixture::put16(record, 18, 1);
    }
    let mut gvar = vec![0; 26];
    fixture::put32(&mut gvar, 0, 0x10000);
    fixture::put16(&mut gvar, 4, axes);
    fixture::put32(&mut gvar, 8, 26);
    fixture::put16(&mut gvar, 12, 2);
    fixture::put32(&mut gvar, 16, 26);
    tables.push((*b"fvar", fvar));
    tables.push((*b"gvar", gvar));
    tables
}

#[test]
fn font_source_directory_order_and_last_valid_empty_or_invalid_windows_match_old() {
    let base = variable_tables(1, 16);
    for tag in [
        *b"head", *b"hhea", *b"maxp", *b"loca", *b"glyf", *b"cmap", *b"name", *b"fvar", *b"gvar",
    ] {
        let data = succeeds(
            base.iter()
                .find(|(name, _)| name == &tag)
                .map(|(_, data)| data)
                .ok_or("table"),
        );
        for empty in [false, true] {
            let mut tables = base.clone();
            tables.push((tag, if empty { vec![] } else { data.clone() }));
            let bytes = fixture::font(tables);
            differential(&bytes, "duplicate.ttf");
            assert_eq!(
                ttf_parser::Face::parse(&bytes, 0).err(),
                heapless(|| font_source::inspect::face_for_qa(&bytes)).err()
            );
            if empty && tag == *b"glyf" {
                assert!(heapless(|| inspect(&bytes, "empty.ttf")).is_ok());
            }
        }
        let mut tables = base.clone();
        tables.push((tag, data.clone()));
        let mut bytes = fixture::font(tables);
        table_window(&mut bytes, tag, 1);
        differential(&bytes, "last-invalid.ttf");
        if tag == *b"glyf" {
            assert_eq!(
                heapless(|| inspect(&bytes, "invalid.ttf")).err(),
                Some(FontReject::GlyphTables)
            );
        }
        let mut tables = base.clone();
        tables.push((tag, data.clone()));
        let mut bytes = fixture::font(tables);
        table_window(&mut bytes, tag, 0);
        differential(&bytes, "last-valid.ttf");
        assert!(heapless(|| inspect(&bytes, "last-valid.ttf")).is_ok());
        let count = usize::from(u16::from_be_bytes([bytes[4], bytes[5]]));
        for index in 0..count / 2 {
            for field in 0..16 {
                bytes.swap(
                    12 + index * 16 + field,
                    12 + (count - index - 1) * 16 + field,
                );
            }
        }
        differential(&bytes, "unsorted.ttf");
    }
}

#[test]
fn font_source_full_u16_directory_preserves_font_hash_label_and_metadata() {
    let base = fixture::named(&fixture::utf16("完整目录"));
    let original = succeeds(fonts::inspect(&base, "actual.ttf"));
    let mut tables = fixture::tables(fixture::names(&[(3, 1, 1, &fixture::utf16("完整目录"))]));
    tables.resize_with(usize::from(u16::MAX), || (*b"zzzz", vec![]));
    let bytes = fixture::font(tables);
    let expected = fonts::ImportedFont {
        hash: whole_hash(&bytes),
        label: original.label,
    };
    let checked = heapless(|| succeeds(inspect(&bytes, "actual.ttf")));
    assert_eq!(checked.hash(), expected.hash);
    assert_eq!(checked.label(), expected.label);
    #[cfg(all(
        target_os = "windows",
        target_env = "gnu",
        target_pointer_width = "64",
        target_endian = "little",
        panic = "unwind"
    ))]
    assert_source_bytes_and_metadata(&bytes, &expected);
    differential(
        &bytes[..12 + usize::from(u16::MAX) * 16 - 1],
        "incomplete.ttf",
    );
    let face = succeeds(font_source::inspect::face_for_qa(&bytes));
    assert_eq!(face.number_of_glyphs(), 2);
    assert_eq!(face.glyph_index('A'), Some(ttf_parser::GlyphId(1)));
    assert!(
        face.outline_glyph(ttf_parser::GlyphId(1), &mut Outline::default())
            .is_some()
    );
}

#[test]
fn font_source_ignored_optional_counts_keep_full_font_bytes_hash_and_metadata() {
    let name = fixture::utf16("保留完整字形");
    let base = fixture::named(&name);
    let original = succeeds(fonts::inspect(&base, "actual.ttf"));
    for strikes in [0, 1, u32::from(u16::MAX), u32::MAX] {
        let mut tables = fixture::tables(fixture::names(&[(3, 1, 1, &name)]));
        let mut sbix = vec![0; 8];
        fixture::put16(&mut sbix, 0, 1);
        fixture::put32(&mut sbix, 4, strikes);
        tables.push((*b"sbix", sbix));
        let bytes = fixture::font(tables);
        let expected = fonts::ImportedFont {
            hash: whole_hash(&bytes),
            label: original.label.clone(),
        };
        let checked = heapless(|| succeeds(inspect(&bytes, "actual.ttf")));
        assert_eq!(checked.hash(), expected.hash);
        assert_eq!(checked.label(), expected.label);
        assert_ne!(checked.hash(), original.hash);
        #[cfg(all(
            target_os = "windows",
            target_env = "gnu",
            target_pointer_width = "64",
            target_endian = "little",
            panic = "unwind"
        ))]
        assert_source_bytes_and_metadata(&bytes, &expected);
        let face = succeeds(font_source::inspect::face_for_qa(&bytes));
        assert_eq!(face.glyph_index('A'), Some(ttf_parser::GlyphId(1)));
        assert!(
            face.outline_glyph(ttf_parser::GlyphId(1), &mut Outline::default())
                .is_some()
        );
    }
    for tag in [*b"CFF ", *b"CFF2", *b"COLR", *b"GPOS", *b"GSUB", *b"CBDT"] {
        let mut tables = fixture::tables(fixture::names(&[(3, 1, 1, &name)]));
        tables.push((tag, vec![0xff; 8]));
        let bytes = fixture::font(tables);
        differential(&bytes, "irrelevant.ttf");
    }
}

#[test]
fn font_source_name_v1_maximum_records_storage_and_incomplete_windows_match_old() {
    let lang_bytes = 16383 * 4;
    let record_start = 8 + lang_bytes;
    let storage_start = record_start + usize::from(u16::MAX) * 12;
    let mut name = vec![0; storage_start];
    fixture::put16(&mut name, 0, 1);
    fixture::put16(&mut name, 2, u16::MAX);
    fixture::put16(&mut name, 6, 16383);
    for record in name[record_start..].as_chunks_mut::<12>().0 {
        fixture::put16(record, 0, 1);
        fixture::put16(record, 6, 1);
    }
    let last = storage_start - 12;
    fixture::put16(&mut name, last, 0);
    fixture::put16(&mut name, last + 8, 2);
    name.extend_from_slice(&fixture::utf16("末"));
    let bytes = fixture::font(fixture::tables(name.clone()));
    differential(&bytes, "maximum.ttf");
    assert_eq!(succeeds(inspect(&bytes, "maximum.ttf")).label(), "末");
    for len in [
        0,
        6,
        7,
        8,
        record_start - 1,
        record_start,
        storage_start - 1,
    ] {
        differential(
            &fixture::font(fixture::tables(name[..len].to_vec())),
            "incomplete.ttf",
        );
    }
    fixture::put16(&mut name, 6, 16384);
    differential(
        &fixture::font(fixture::tables(name)),
        "checked-multiply.ttf",
    );
    let mut name = vec![0; 30 + 2 * usize::from(u16::MAX)];
    fixture::put16(&mut name, 2, 2);
    fixture::put16(&mut name, 4, 30);
    for record in [6, 18] {
        fixture::put16(&mut name, record, 3);
        fixture::put16(&mut name, record + 2, 1);
        fixture::put16(&mut name, record + 6, 1);
    }
    fixture::put16(&mut name, 14, u16::MAX);
    fixture::put16(&mut name, 16, u16::MAX);
    fixture::put16(&mut name, 26, 2);
    let storage = &mut name[30..];
    storage[..2].copy_from_slice(&fixture::utf16("好"));
    let prefix = fixture::utf16(&"😀".repeat(128));
    let start = usize::from(u16::MAX);
    storage[start..start + prefix.len()].copy_from_slice(&prefix);
    let invalid = storage.len() - 3;
    storage[invalid..invalid + 2].copy_from_slice(&[0xd8, 0]);
    let bytes = fixture::font(fixture::tables(name));
    differential(&bytes, "full-tail.ttf");
    assert_eq!(succeeds(inspect(&bytes, "full-tail.ttf")).label(), "好");
}

#[test]
fn font_source_variable_default_coordinates_and_actual_read_windows_match_old() {
    for (axes, offset) in [(1, 16), (63, 16), (64, 16), (65, 16), (u16::MAX, u16::MAX)] {
        let bytes = fixture::font(variable_tables(axes, offset));
        differential(&bytes, "variable.ttf");
        let old = succeeds(ttf_parser::Face::parse(&bytes, 0));
        let new = heapless(|| succeeds(font_source::inspect::face_for_qa(&bytes)));
        assert_eq!(new.variation_coordinates(), old.variation_coordinates());
        assert_eq!(new.variation_coordinates().len(), usize::from(axes.min(64)));
        face_differential(&bytes);
    }
    for (axis_count, shared_count, offset) in [
        (1, u16::MAX, 20),
        (u16::MAX, u16::MAX, 20),
        (1, 1, u32::MAX),
    ] {
        let mut tables = variable_tables(1, 16);
        let gvar = succeeds(
            tables
                .iter_mut()
                .find(|(tag, _)| tag == b"gvar")
                .map(|(_, data)| data)
                .ok_or("gvar"),
        );
        fixture::put16(gvar, 4, axis_count);
        fixture::put16(gvar, 6, shared_count);
        fixture::put32(gvar, 8, offset);
        let bytes = fixture::font(tables);
        differential(&bytes, "gvar-window.ttf");
    }
    let mut tables = variable_tables(1, 16);
    let gvar = succeeds(
        tables
            .iter_mut()
            .find(|(tag, _)| tag == b"gvar")
            .map(|(_, data)| data)
            .ok_or("gvar"),
    );
    gvar.resize(font_source::MAX_FONT_BYTES - 2048, 0);
    let end = succeeds(u32::try_from(gvar.len()));
    fixture::put16(gvar, 6, u16::MAX);
    fixture::put32(gvar, 8, end);
    let bytes = fixture::font(tables);
    assert!(bytes.len() <= font_source::MAX_FONT_BYTES);
    differential(&bytes, "actual-table-window.ttf");
}

#[test]
fn font_source_required_parser_rules_and_numeric_error_order_match_old() {
    use ttf_parser::FaceParsingError;
    for (head_bad, hhea_bad, maxp_bad, expected) in [
        (true, true, true, FaceParsingError::NoHeadTable),
        (false, true, true, FaceParsingError::NoHheaTable),
        (false, false, true, FaceParsingError::NoMaxpTable),
    ] {
        let mut tables = fixture::tables(fixture::names(&[]));
        for (tag, data) in &mut tables {
            if (head_bad && tag == b"head")
                || (hhea_bad && tag == b"hhea")
                || (maxp_bad && tag == b"maxp")
                || tag == b"cmap"
            {
                data.clear();
            }
        }
        let bytes = fixture::font(tables);
        assert_eq!(ttf_parser::Face::parse(&bytes, 0).err(), Some(expected));
        assert_eq!(
            heapless(|| font_source::inspect::face_for_qa(&bytes)).err(),
            Some(expected)
        );
        assert_eq!(
            heapless(|| inspect(&bytes, "invalid.ttf")).err(),
            Some(FontReject::Face)
        );
        differential(&bytes, "invalid.ttf");
    }
    for (tag, offset, value, expected) in [
        (*b"head", 18, 15, FontReject::Face),
        (*b"head", 50, 2, FontReject::Face),
        (*b"maxp", 4, 0, FontReject::Face),
        (*b"cmap", 2, u16::MAX, FontReject::GlyphTables),
    ] {
        let mut tables = fixture::tables(fixture::names(&[]));
        let data = succeeds(
            tables
                .iter_mut()
                .find(|(name, _)| name == &tag)
                .map(|(_, data)| data)
                .ok_or("table"),
        );
        fixture::put16(data, offset, value);
        let bytes = fixture::font(tables);
        assert_eq!(
            heapless(|| inspect(&bytes, "invalid.ttf")).err(),
            Some(expected)
        );
        differential(&bytes, "invalid.ttf");
    }
    for (long, count, expected) in [
        (false, 65535, None),
        (false, 65536, Some(FontReject::GlyphTables)),
        (true, 65535, None),
        (true, 65536, Some(FontReject::GlyphTables)),
    ] {
        let mut tables = fixture::tables(fixture::names(&[]));
        for (tag, data) in &mut tables {
            if tag == b"head" {
                fixture::put16(data, 50, u16::from(long));
            }
            if tag == b"loca" {
                data.resize(count * if long { 4 } else { 2 }, 0);
            }
        }
        let bytes = fixture::font(tables);
        assert_eq!(heapless(|| inspect(&bytes, "loca.ttf")).err(), expected);
        differential(&bytes, "loca.ttf");
    }
    let mut true_font = fixture::named(&fixture::utf16("TrueType"));
    true_font[..4].copy_from_slice(b"true");
    differential(&true_font, "true.TTF");
    for name in ["", "ttf", "font.otf", "font.ＴＴＦ", "font.ttf/"] {
        differential(&true_font, name);
    }
    let mut collection = vec![0; 12];
    collection[..4].copy_from_slice(b"ttcf");
    fixture::put32(&mut collection, 8, u32::MAX);
    assert_eq!(
        heapless(|| inspect(&collection, "collection.ttf")).err(),
        Some(FontReject::Signature)
    );
    differential(&collection, "collection.ttf");
    let mut bytes = fixture::named(&fixture::utf16("全量 hash"));
    bytes.resize(font_source::MAX_FONT_BYTES, 0x5a);
    differential(&bytes, "limit.ttf");
    bytes.push(0);
    assert_eq!(
        heapless(|| inspect(&bytes, "bad.otf")).err(),
        Some(FontReject::TooLarge)
    );
}

#[test]
#[cfg(all(
    target_os = "windows",
    target_env = "gnu",
    target_pointer_width = "64",
    target_endian = "little",
    panic = "unwind"
))]
fn font_source_certified_inspection_matches_the_narrow_real_route() {
    let bytes = fixture::named(&fixture::utf16("检查证明"));
    let proof = heapless(|| succeeds(FontInspectionProof::verify(GNU_FONT_BUILD)));
    let checked = heapless(|| succeeds(proof.inspect(&bytes, "proof.ttf")));
    let old = succeeds(fonts::inspect(&bytes, "proof.ttf"));
    assert_eq!(checked.hash(), old.hash);
    assert_eq!(checked.label(), old.label);
}

#[test]
fn font_source_inspection_proof_checks_build_before_the_narrow_real_route() {
    let mut build = GNU_FONT_BUILD;
    build.ttf_features |= 64;
    assert_eq!(
        heapless(|| FontInspectionProof::verify(build)).err(),
        Some(NumericReject::UnsupportedEnvelope)
    );
    build = GNU_FONT_BUILD;
    build.target = "unverified";
    assert_eq!(
        heapless(|| FontInspectionProof::verify(build)).err(),
        Some(NumericReject::UnsupportedEnvelope)
    );
    #[cfg(not(all(
        target_os = "windows",
        target_env = "gnu",
        target_pointer_width = "64",
        target_endian = "little",
        panic = "unwind"
    )))]
    assert_eq!(
        heapless(|| FontInspectionProof::verify(GNU_FONT_BUILD)).err(),
        Some(NumericReject::UnsupportedEnvelope)
    );
}

#[test]
#[cfg(all(
    target_os = "windows",
    target_env = "gnu",
    target_pointer_width = "64",
    target_endian = "little",
    panic = "unwind"
))]
fn font_source_actual_layout_and_every_release_order_return_after_dealloc() {
    let fixture = Fixture::new();
    let slot = fixture.publish(REGULAR);
    let settings = TerminalSettingsV1::default();
    let cancel = AtomicBool::new(false);
    let certificate = certificate();
    for order in (0..4).flat_map(|a| {
        (0..4).filter(move |b| *b != a).flat_map(move |b| {
            (0..4)
                .filter(move |c| *c != a && *c != b)
                .map(move |c| [a, b, c, 6 - a - b - c])
        })
    }) {
        let ledger = root(BYTE_LIMIT);
        let audit = audit::AuditScope::start(&ledger);
        let source = succeeds(capture_bytes(
            &certificate,
            &ledger,
            admission(),
            input(&settings, Some(&slot), &fixture.0),
            FontAvailability::Bytes(REGULAR),
            &cancel,
        ));
        assert_eq!(source.bytes(), Some(REGULAR));
        assert_eq!(source.trusted_root(), fixture.0);
        assert_eq!(source.body().metadata.legacy_slot_version, 3);
        assert_eq!(source.body().metadata.mode, FontMode::LegacyUndetermined);
        let held = ledger.snapshot().rust_bytes;
        let frozen = succeeds(source.try_clone());
        assert_eq!(ledger.snapshot().rust_bytes, held);
        let metadata = succeeds(source.copy_metadata(&ledger, admission(), &cancel));
        let send = succeeds(source.copy_font_bytes(&ledger, admission(), &cancel));
        let mut source = Some(source);
        let mut frozen = Some(frozen);
        let mut metadata = Some(metadata);
        let mut send = Some(send);
        for index in order {
            match index {
                0 => drop(source.take()),
                1 => drop(frozen.take()),
                2 => drop(metadata.take()),
                _ => drop(send.take()),
            }
        }
        assert_eq!(ledger.snapshot().rust_bytes, 0);
        audit.finish().assert_balanced_and_pregranted();
    }
}

#[test]
fn font_source_actual_layout_matches_current_target() {
    let slot = succeeds(fonts::inspect(REGULAR, "cached.ttf"));
    let model = succeeds(SourceLayout::actual(REGULAR.len(), 64, slot.label.len(), 0));
    assert_eq!(model.body, Layout::new::<font_source::SourceBody>());
    assert_eq!(model.imported_font, Layout::new::<fonts::ImportedFont>());
    println!(
        "SourceBody={}/{}, SourceMetadata={}/{}, ImportedFont={}/{}, owner_base={}",
        model.body.size(),
        model.body.align(),
        model.metadata.size(),
        model.metadata.align(),
        model.imported_font.size(),
        model.imported_font.align(),
        succeeds(owner::OwnerLayout::actual::<font_source::SourceBody>(0))
            .backing
            .size()
    );
}

#[cfg(all(
    target_os = "windows",
    target_env = "gnu",
    target_pointer_width = "64",
    target_endian = "little",
    panic = "unwind"
))]
fn assert_projection(source: &SourceOwner<'_>, old: &settings_signals::ProfileFontSnapshot) {
    use settings_signals::{
        ProfileFontFailure as Failure, ProfileFontMetadataError as Metadata,
        ProfileFontMode as Mode, ProfileFontRuntimeSource as Runtime,
    };
    let value = &source.body().metadata;
    let mode = match value.mode {
        FontMode::LegacyUndetermined => Mode::LegacyUndetermined,
        FontMode::InheritTerminalSlot => Mode::InheritTerminalSlot,
        FontMode::ExplicitFamily => Mode::ExplicitFamily,
        FontMode::ExplicitCachedTtf => Mode::ExplicitCachedTtf,
        FontMode::Uneditable => Mode::Uneditable,
    };
    let reason = value.uneditable_reason.map(|reason| match reason {
        font_source::MetadataError::InvalidMetadata => Metadata::InvalidMetadata,
        font_source::MetadataError::UnsupportedVersion => Metadata::UnsupportedVersion,
    });
    let failure = value.fallback_reason.map(|reason| match reason {
        ResourceFailure::Missing => Failure::Missing,
        ResourceFailure::Corrupt => Failure::Corrupt,
        ResourceFailure::Unavailable => Failure::Unavailable,
        ResourceFailure::UneditableMetadata => Failure::UneditableMetadata,
    });
    let runtime = match value.runtime_source {
        font_source::source::RuntimeSource::LegacyTerminalSlot => Runtime::LegacyTerminalSlot,
        font_source::source::RuntimeSource::ProfileBinding => Runtime::ProfileBinding,
        font_source::source::RuntimeSource::Uneditable => Runtime::Uneditable,
    };
    assert_eq!(old.version, value.version);
    assert_eq!(
        old.profile_id,
        uuid::Uuid::from_bytes(value.profile_id).to_string()
    );
    assert_eq!(old.configuration_revision, value.revision);
    assert_eq!(old.saved.mode, mode);
    assert_eq!(old.saved.version, value.version);
    assert_eq!(old.saved.hash, value.saved_hash);
    assert_eq!(old.saved.uneditable_reason, reason);
    assert_eq!(old.raw_family, value.raw_family);
    assert_eq!(old.actual_family, value.actual_family);
    assert_eq!(old.runtime_source, runtime);
    assert_eq!(old.fallback_reason, failure);
    assert_eq!(
        old.resource.as_ref().map(|r| r.hash.as_str()),
        value.resource_hash()
    );
    assert_eq!(
        old.captured_terminal_slot.as_ref().map(|r| r.hash.as_str()),
        value.captured_resource_hash()
    );
    for reference in [&old.resource, &old.captured_terminal_slot]
        .into_iter()
        .flatten()
    {
        assert_eq!(reference.version, value.version);
        assert!(font_source::metadata::family_matches(
            &reference.family,
            &reference.hash
        ));
    }
}

#[cfg(all(
    target_os = "windows",
    target_env = "gnu",
    target_pointer_width = "64",
    target_endian = "little",
    panic = "unwind"
))]
fn assert_source_projection(input: BorrowedCapture<'_>, old: &profile_fonts::ResolvedProfileFont) {
    let certificate = certificate();
    let ledger = root(BYTE_LIMIT);
    let cancel = AtomicBool::new(false);
    let audit = audit::AuditScope::start(&ledger);
    let source = succeeds(capture_bytes(
        &certificate,
        &ledger,
        admission(),
        input,
        FontAvailability::Bytes(REGULAR),
        &cancel,
    ));
    let report = audit.finish();
    assert_projection(&source, &old.projection);
    assert_eq!(
        source.bytes(),
        old.resource.as_ref().map(|r| r.bytes().as_ref())
    );
    if let (Some(font), Some(resource)) = (&source.body().imported_font, &old.resource) {
        assert_eq!(font.hash, resource.hash());
        assert_eq!(font.label, resource.label());
    }
    assert!(
        report
            .events()
            .iter()
            .all(|e| e.held_rust >= e.size && e.source_bytes > 0)
    );
    drop(source);
    assert_eq!(ledger.snapshot().rust_bytes, 0);
}

#[test]
fn font_source_metadata_and_resources_match_actual_old_profile_resolution() {
    use serde_json::json;
    let fixture = Fixture::new();
    let slot = fixture.publish(REGULAR);
    let invalid_slot = fonts::ImportedFont {
        hash: "不规范的标识".into(),
        label: "捕获标签\0".into(),
    };
    let repository = succeeds(rshell_m0::rshell_storage::SqliteRepository::open(
        fixture.0.join("oracle.sqlite3"),
    ));
    succeeds(repository.migrate());
    let mut snapshot = succeeds(repository.load_configuration());
    let id = snapshot.profiles[0].id;
    let cases = [
        None,
        Some(json!({"version":1,"mode":"inherit_terminal_slot"})),
        Some(json!({"version":1,"mode":"explicit_family"})),
        Some(json!({"version":1,"mode":"explicit_cached_ttf","hash":slot.hash})),
        Some(json!({"version":2,"mode":"future","unknown":["扩展",1]})),
        Some(json!({"version":"1","mode":"explicit_family"})),
        Some(json!({"version":1,"mode":"inherit_terminal_slot","extra":null})),
        Some(json!({"version":1,"mode":"explicit_cached_ttf","hash":"A".repeat(64)})),
        Some(json!(null)),
    ];
    for case in cases {
        for captured in [Some(&slot), Some(&invalid_slot), None] {
            let mut settings = TerminalSettingsV1 {
                font_family: if case.as_ref().is_some_and(|v| {
                    v.get("mode").and_then(serde_json::Value::as_str) == Some("explicit_cached_ttf")
                }) {
                    format!("GuoshFont_{}", slot.hash)
                } else {
                    "  借用原 family  ".into()
                },
                ..TerminalSettingsV1::default()
            };
            if let Some(case) = &case {
                settings
                    .extensions
                    .insert(font_source::metadata::NAMESPACE.into(), case.clone());
            }
            let old_binding = profile_fonts::saved_binding(&settings);
            let new_binding = heapless(|| font_source::saved_binding(&settings));
            assert_eq!(old_binding.hash.as_deref(), new_binding.hash);
            snapshot.profiles[0].settings = settings.clone();
            let old = succeeds(
                profile_fonts::resolve_profile_font(&snapshot, id, captured.cloned(), &fixture.0)
                    .ok_or("oracle profile"),
            );
            let profile = settings_signals::DesktopTerminalProfile {
                id: id.0.to_string(),
                name: "oracle".into(),
                version: 1,
                terminal_type: "xterm".into(),
                initial_cols: 80,
                initial_rows: 24,
                scrollback_lines: 10,
                font_family: settings.font_family.clone(),
                font_size: 14.0,
                color_scheme: "dark".into(),
                key_bindings: vec![],
                left_alt_as_meta: false,
                right_alt_as_meta: false,
                enable_csi_u: false,
                enable_kitty_keyboard: false,
                mouse_reporting: false,
                scroll_on_output: false,
                scroll_on_keypress: false,
                answerback: String::new(),
            };
            let projection = profile_fonts::resolve_projection(
                &profile,
                old_binding,
                snapshot.revision,
                captured.cloned(),
                &fixture.0,
            );
            assert_eq!(old.projection, projection);
            #[cfg(all(
                target_os = "windows",
                target_env = "gnu",
                target_pointer_width = "64",
                target_endian = "little",
                panic = "unwind"
            ))]
            assert_source_projection(
                BorrowedCapture {
                    profile_id: *id.0.as_bytes(),
                    revision: snapshot.revision,
                    ..input(&settings, captured, &fixture.0)
                },
                &old,
            );
        }
    }
}

#[test]
#[cfg(all(
    target_os = "windows",
    target_env = "gnu",
    target_pointer_width = "64",
    target_endian = "little",
    panic = "unwind"
))]
fn font_source_budget_and_cancel_precede_owned_copy_and_file_read() {
    let fixture = Fixture::new();
    let slot = fixture.publish(REGULAR);
    let settings = TerminalSettingsV1::default();
    let certificate = certificate();
    let cancel = AtomicBool::new(false);
    let ledger = root(1);
    let audit = audit::AuditScope::start(&ledger);
    let error = capture_bytes(
        &certificate,
        &ledger,
        admission(),
        input(&settings, Some(&slot), &fixture.0),
        FontAvailability::Bytes(REGULAR),
        &cancel,
    )
    .err();
    assert_eq!(
        error,
        Some(FontReject::Budget(NumericReject::RustBudgetExceeded))
    );
    assert!(audit.finish().events().is_empty());
    let path = fixture.0.join("fonts").join(format!("{}.ttf", slot.hash));
    let mut file = succeeds(std::fs::File::open(&path));
    let audit = audit::AuditScope::start(&ledger);
    let error = capture_file(
        &certificate,
        &ledger,
        admission(),
        input(&settings, Some(&slot), &fixture.0),
        &mut file,
        REGULAR.len() as u64,
        &cancel,
    )
    .err();
    assert_eq!(
        error,
        Some(FontReject::Budget(NumericReject::RustBudgetExceeded))
    );
    assert!(audit.finish().events().is_empty());
    assert_eq!(succeeds(std::io::Seek::stream_position(&mut file)), 0);
    cancel.store(true, Ordering::Release);
    let ledger = root(BYTE_LIMIT);
    let audit = audit::AuditScope::start(&ledger);
    let error = capture_bytes(
        &certificate,
        &ledger,
        admission(),
        input(&settings, Some(&slot), &fixture.0),
        FontAvailability::Bytes(REGULAR),
        &cancel,
    )
    .err();
    assert_eq!(error, Some(FontReject::Budget(NumericReject::Cancelled)));
    assert!(audit.finish().events().is_empty());
    cancel.store(false, Ordering::Release);
    let audit = audit::AuditScope::start(&ledger);
    audit.cancel_on_allocation(&cancel, 2);
    let error = capture_bytes(
        &certificate,
        &ledger,
        admission(),
        input(&settings, Some(&slot), &fixture.0),
        FontAvailability::Bytes(REGULAR),
        &cancel,
    )
    .err();
    assert_eq!(error, Some(FontReject::Budget(NumericReject::Cancelled)));
    audit.finish().assert_balanced_and_pregranted();
    assert_eq!(ledger.snapshot().rust_bytes, 0);
}

#[test]
#[cfg(all(
    target_os = "windows",
    target_env = "gnu",
    target_pointer_width = "64",
    target_endian = "little",
    panic = "unwind"
))]
fn font_source_one_granted_buffer_read_exact_hash_and_failure_paths() {
    let fixture = Fixture::new();
    let slot = fixture.publish(REGULAR);
    let settings = TerminalSettingsV1::default();
    let certificate = certificate();
    let cancel = AtomicBool::new(false);
    let path = fixture.0.join("fonts").join(format!("{}.ttf", slot.hash));
    for length in [
        REGULAR.len() as u64,
        REGULAR.len() as u64 + 1,
        font_source::MAX_FONT_BYTES as u64 + 1,
    ] {
        let mut file = succeeds(std::fs::File::open(&path));
        let ledger = root(BYTE_LIMIT);
        let audit = audit::AuditScope::start(&ledger);
        let source = succeeds(capture_file(
            &certificate,
            &ledger,
            admission(),
            input(&settings, Some(&slot), &fixture.0),
            &mut file,
            length,
            &cancel,
        ));
        if length == REGULAR.len() as u64 {
            assert_eq!(source.bytes(), Some(REGULAR));
        } else {
            assert!(source.bytes().is_none());
            assert_eq!(
                source.body().metadata.fallback_reason,
                Some(ResourceFailure::Corrupt)
            );
        }
        drop(source);
        let report = audit.finish();
        report.assert_balanced_and_pregranted();
        assert!(report.events().iter().all(|e| e.kind != 3));
        assert_eq!(ledger.snapshot().rust_bytes, 0);
    }
    succeeds(std::fs::write(&path, BOLD));
    let ledger = root(BYTE_LIMIT);
    let mut file = succeeds(std::fs::File::open(&path));
    let audit = audit::AuditScope::start(&ledger);
    let source = succeeds(capture_file(
        &certificate,
        &ledger,
        admission(),
        input(&settings, Some(&slot), &fixture.0),
        &mut file,
        BOLD.len() as u64,
        &cancel,
    ));
    assert_eq!(
        source.body().metadata.fallback_reason,
        Some(ResourceFailure::Corrupt)
    );
    drop(source);
    audit.finish().assert_balanced_and_pregranted();
    assert_eq!(ledger.snapshot().rust_bytes, 0);
    let mut grown = REGULAR.to_vec();
    grown.push(1);
    succeeds(std::fs::write(&path, grown));
    let mut file = succeeds(std::fs::File::open(&path));
    let audit = audit::AuditScope::start(&ledger);
    let source = succeeds(capture_file(
        &certificate,
        &ledger,
        admission(),
        input(&settings, Some(&slot), &fixture.0),
        &mut file,
        REGULAR.len() as u64,
        &cancel,
    ));
    assert_eq!(
        source.body().metadata.fallback_reason,
        Some(ResourceFailure::Corrupt)
    );
    drop(source);
    audit.finish().assert_balanced_and_pregranted();
    let mut file = succeeds(std::fs::OpenOptions::new().write(true).open(&path));
    let audit = audit::AuditScope::start(&ledger);
    let source = succeeds(capture_file(
        &certificate,
        &ledger,
        admission(),
        input(&settings, Some(&slot), &fixture.0),
        &mut file,
        REGULAR.len() as u64,
        &cancel,
    ));
    assert_eq!(
        source.body().metadata.fallback_reason,
        Some(ResourceFailure::Unavailable)
    );
    drop(source);
    audit.finish().assert_balanced_and_pregranted();
    assert_eq!(ledger.snapshot().rust_bytes, 0);
}

#[test]
fn font_source_real_cache_rejects_growth_beyond_saved_hash() {
    let fixture = Fixture::new();
    let slot = fixture.publish(REGULAR);
    let path = fixture.0.join("fonts").join(format!("{}.ttf", slot.hash));
    let mut grown = REGULAR.to_vec();
    grown.push(1);
    succeeds(std::fs::write(&path, grown));
    assert_eq!(
        font_resources::load_by_hash(&fixture.0, &slot.hash).err(),
        Some(font_resources::FontResourceError::Corrupt)
    );
}

#[test]
#[cfg(all(
    target_os = "windows",
    target_env = "gnu",
    target_pointer_width = "64",
    target_endian = "little",
    panic = "unwind"
))]
fn font_source_real_shortfall_does_not_reject_other_normal_selected_or_release_old_epoch() {
    let fixture = Fixture::new();
    let slot = fixture.publish(REGULAR);
    let mut settings = TerminalSettingsV1::default();
    let certificate = certificate();
    let cancel = AtomicBool::new(false);
    let ledger = root(BYTE_LIMIT);
    let held = succeeds(ledger.try_reserve(
        Amount {
            rust_bytes: BYTE_LIMIT - 1024,
            c_bytes: 0,
            slots: 0,
        },
        ResourceClass::Runtime,
        admission(),
    ));
    let audit = audit::AuditScope::start(&ledger);
    assert_eq!(
        capture_bytes(
            &certificate,
            &ledger,
            admission(),
            input(&settings, Some(&slot), &fixture.0),
            FontAvailability::Bytes(REGULAR),
            &cancel
        )
        .err(),
        Some(FontReject::Budget(NumericReject::RustBudgetExceeded))
    );
    assert!(audit.finish().events().is_empty());
    settings.extensions.insert(
        font_source::metadata::NAMESPACE.into(),
        serde_json::json!({"version":1,"mode":"explicit_family"}),
    );
    settings.font_family = "small".into();
    let source = succeeds(capture_bytes(
        &certificate,
        &ledger,
        admission(),
        input(&settings, Some(&slot), &fixture.0),
        FontAvailability::Bytes(REGULAR),
        &cancel,
    ));
    assert_eq!(source.body().metadata.actual_family, "small");
    assert!(source.bytes().is_none());
    let shared = succeeds(source.try_clone());
    drop(source);
    let before = ledger.snapshot().rust_bytes;
    assert_eq!(succeeds(ledger.advance_epoch(1)), 2);
    assert_eq!(ledger.snapshot().rust_bytes, before);
    assert_eq!(
        shared.copy_metadata(&ledger, admission(), &cancel).err(),
        Some(NumericReject::EpochMismatch)
    );
    drop(shared);
    drop(held);
    assert_eq!(ledger.snapshot().rust_bytes, 0);
}

#[test]
#[cfg(all(
    target_os = "windows",
    target_env = "gnu",
    target_pointer_width = "64",
    target_endian = "little",
    panic = "unwind"
))]
fn font_source_null_allocator_does_not_publish_or_retain_quota() {
    let fixture = Fixture::new();
    let slot = fixture.publish(REGULAR);
    let settings = TerminalSettingsV1::default();
    let certificate = certificate();
    let cancel = AtomicBool::new(false);
    let ledger = root(BYTE_LIMIT);
    let source = succeeds(capture_bytes(
        &certificate,
        &ledger,
        admission(),
        input(&settings, Some(&slot), &fixture.0),
        FontAvailability::Bytes(REGULAR),
        &cancel,
    ));
    drop(source);
    let audit = audit::AuditScope::start(&ledger);
    audit.fail_next_size(64);
    let error = capture_bytes(
        &certificate,
        &ledger,
        admission(),
        input(&settings, Some(&slot), &fixture.0),
        FontAvailability::Bytes(REGULAR),
        &cancel,
    )
    .err();
    assert_eq!(
        error,
        Some(FontReject::Budget(NumericReject::AllocationFailure))
    );
    audit.finish().assert_balanced_and_pregranted();
    assert_eq!(ledger.snapshot().rust_bytes, 0);
}

#[test]
fn font_source_legacy_vec_arc_shape_is_explicit() {
    let slot = succeeds(fonts::inspect(REGULAR, "cached.ttf"));
    let vec = REGULAR.to_vec();
    let capacity = vec.capacity();
    let arc: std::sync::Arc<[u8]> = vec.into();
    let shape = succeeds(font_source::source::LegacyBackingLayout::actual(
        &slot,
        &arc,
        Some(capacity),
    ));
    assert_eq!(shape.hash.size(), slot.hash.capacity());
    assert_eq!(shape.label.size(), slot.label.capacity());
    assert_eq!(shape.previous_vec.map(|l| l.size()), Some(capacity));
    let header = succeeds(Layout::array::<std::sync::atomic::AtomicUsize>(2));
    let data = succeeds(Layout::array::<u8>(arc.len()));
    assert_eq!(shape.arc, succeeds(layout::extend(header, data)).0);
}

#[test]
fn font_source_build_mismatch_and_overflow_fail_before_allocation() {
    let mut build = GNU_FONT_BUILD;
    build.ttf_features |= 64;
    assert_eq!(
        FontBuildProof::verify(build).err(),
        Some(NumericReject::UnsupportedEnvelope)
    );
    build = GNU_FONT_BUILD;
    build.compiler = "unverified";
    assert_eq!(
        FontBuildProof::verify(build).err(),
        Some(NumericReject::UnsupportedEnvelope)
    );
    assert_eq!(
        SourceLayout::actual(usize::MAX, 0, 0, 0).err(),
        Some(NumericReject::InvalidLayout)
    );
    assert_eq!(
        SourceLayout::actual(0, usize::MAX, 1, 0).err(),
        Some(NumericReject::Overflow)
    );
}

#[test]
#[cfg(not(all(
    target_os = "windows",
    target_env = "gnu",
    target_pointer_width = "64",
    target_endian = "little",
    panic = "unwind"
)))]
fn font_source_uncertified_target_rejects_before_quota_and_allocation() {
    for bytes in [0, BYTE_LIMIT] {
        let ledger = root(bytes);
        let audit = audit::AuditScope::start(&ledger);
        assert_eq!(
            FontBuildProof::verify(GNU_FONT_BUILD).err(),
            Some(NumericReject::UnsupportedEnvelope)
        );
        assert_eq!(
            FontInspectionProof::verify(GNU_FONT_BUILD).err(),
            Some(NumericReject::UnsupportedEnvelope)
        );
        assert!(audit.finish().events().is_empty());
        assert_eq!(ledger.snapshot().rust_bytes, 0);
    }
}
