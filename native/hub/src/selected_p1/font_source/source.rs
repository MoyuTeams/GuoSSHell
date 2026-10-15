//! 首次 owned copy/read 前预约；font/path 是 owner tail，普通 clone 只增 header ref。

use std::alloc::Layout;
use std::fs::File;
use std::io::{ErrorKind, Read};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use rshell_m0::rshell_core::TerminalSettingsV1;

use crate::fonts::ImportedFont;
use crate::selected_p1::model::layout::{NumericReject, add, extend};
use crate::selected_p1::resource::ledger::{Admission, Amount, ProcessLedger, ResourceClass};
use crate::selected_p1::resource::owner::{OwnerLayout, SharedOwner};

use super::closure::FontBuildProof;
use super::inspect::{CheckedFont, FontReject, LABEL_BYTES, MAX_FONT_BYTES, inspect};
use super::metadata::{
    BindingRef, DEFAULT_FAMILY, FAMILY_PREFIX, FontMode, MetadataError, ResourceFailure,
    saved_binding, valid_hash,
};

/// 信任来自父平台 provider；不是 Dart 文件路径的校验器。
#[derive(Clone, Copy)]
pub struct TrustedRoot<'a>(&'a Path);

impl<'a> TrustedRoot<'a> {
    pub fn from_root_caller(root: &'a Path) -> Self {
        Self(root)
    }

    pub fn path(self) -> &'a Path {
        self.0
    }
}

#[derive(Clone, Copy)]
pub struct BorrowedCapture<'a> {
    pub settings: &'a TerminalSettingsV1,
    pub profile_id: [u8; 16],
    pub revision: u64,
    pub legacy_slot_version: u64,
    pub captured_slot: Option<&'a ImportedFont>,
    pub trusted_root: TrustedRoot<'a>,
}

#[derive(Clone, Copy)]
pub enum FontAvailability<'a> {
    Bytes(&'a [u8]),
    Failed(ResourceFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum RuntimeSource {
    LegacyTerminalSlot,
    ProfileBinding,
    Uneditable,
}

#[repr(C)]
pub struct SourceMetadata {
    pub profile_id: [u8; 16],
    pub revision: u64,
    pub legacy_slot_version: u64,
    pub version: u32,
    pub mode: FontMode,
    pub uneditable_reason: Option<MetadataError>,
    pub saved_hash: Option<String>,
    pub raw_family: String,
    pub actual_family: String,
    pub runtime_source: RuntimeSource,
    pub captured_terminal_slot: Option<ImportedFont>,
    pub fallback_reason: Option<ResourceFailure>,
}

impl SourceMetadata {
    pub fn resource_hash(&self) -> Option<&str> {
        let hash = if matches!(
            self.mode,
            FontMode::LegacyUndetermined | FontMode::InheritTerminalSlot
        ) {
            self.captured_terminal_slot
                .as_ref()
                .map(|font| font.hash.as_str())
        } else {
            self.saved_hash.as_deref()
        };
        hash.filter(|hash| valid_hash(hash))
    }

    pub fn captured_resource_hash(&self) -> Option<&str> {
        self.captured_terminal_slot
            .as_ref()
            .map(|font| font.hash.as_str())
            .filter(|hash| valid_hash(hash))
    }
}

#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct TailRange {
    pub offset: usize,
    pub len: usize,
}

#[repr(C)]
pub struct SourceBody {
    pub imported_font: Option<ImportedFont>,
    pub metadata: SourceMetadata,
    pub trusted_root: TailRange,
    pub font_buffer: TailRange,
}

#[repr(C)]
pub struct MetadataBody {
    pub imported_font: Option<ImportedFont>,
    pub metadata: SourceMetadata,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceLayout {
    pub owner: OwnerLayout,
    pub body: Layout,
    pub imported_font: Layout,
    pub metadata: Layout,
    pub hash_backing: usize,
    pub label_backing: usize,
    pub metadata_backing: usize,
    pub tail_bytes: usize,
    pub required_rust: usize,
}

impl SourceLayout {
    pub fn actual(
        tail_bytes: usize,
        hash_backing: usize,
        label_backing: usize,
        metadata_backing: usize,
    ) -> Result<Self, NumericReject> {
        let owner = OwnerLayout::actual::<SourceBody>(tail_bytes)?;
        let out_of_line = add(add(hash_backing, label_backing)?, metadata_backing)?;
        Ok(Self {
            owner,
            body: Layout::new::<SourceBody>(),
            imported_font: Layout::new::<ImportedFont>(),
            metadata: Layout::new::<SourceMetadata>(),
            hash_backing,
            label_backing,
            metadata_backing,
            tail_bytes,
            required_rust: owner.required_rust(out_of_line)?,
        })
    }
}

/// 只量旧输入的真实 backing；不接管它、不事后给旧分配贴 permit。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyBackingLayout {
    pub hash: Layout,
    pub label: Layout,
    pub arc: Layout,
    pub previous_vec: Option<Layout>,
    pub coexistence_bytes: usize,
}

impl LegacyBackingLayout {
    pub fn actual(
        font: &ImportedFont,
        bytes: &std::sync::Arc<[u8]>,
        previous_vec_capacity: Option<usize>,
    ) -> Result<Self, NumericReject> {
        let array = |n| Layout::array::<u8>(n).map_err(|_| NumericReject::InvalidLayout);
        // std ArcInner: strong/weak 两个 AtomicUsize；Arc<[u8]> 与旧 Vec cap 同时存活的转换另列。
        let arc = extend(
            Layout::array::<AtomicUsize>(2).map_err(|_| NumericReject::InvalidLayout)?,
            array(bytes.len())?,
        )?
        .0;
        let hash = array(font.hash.capacity())?;
        let label = array(font.label.capacity())?;
        let previous_vec = previous_vec_capacity.map(array).transpose()?;
        let coexistence_bytes = add(
            add(hash.size(), label.size())?,
            add(arc.size(), previous_vec.map_or(0, |l| l.size()))?,
        )?;
        Ok(Self {
            hash,
            label,
            arc,
            previous_vec,
            coexistence_bytes,
        })
    }
}

struct CaptureShape<'a> {
    input: BorrowedCapture<'a>,
    binding: BindingRef<'a>,
    captured: Option<&'a ImportedFont>,
    hash: Option<&'a str>,
}

impl<'a> CaptureShape<'a> {
    fn new(input: BorrowedCapture<'a>) -> Self {
        let binding = saved_binding(input.settings);
        let uses_slot = matches!(
            binding.mode,
            FontMode::LegacyUndetermined | FontMode::InheritTerminalSlot
        );
        let captured = if uses_slot { input.captured_slot } else { None };
        let hash = if uses_slot {
            captured.map(|font| font.hash.as_str())
        } else {
            binding.hash
        };
        Self {
            input,
            binding,
            captured,
            hash,
        }
    }

    fn metadata_backing(&self, loaded: bool) -> Result<usize, NumericReject> {
        let captured = self
            .captured
            .map_or(Ok(0), |font| add(font.hash.len(), font.label.len()))?;
        let family = if loaded {
            add(FAMILY_PREFIX.len(), 64)?
        } else if self.binding.mode == FontMode::ExplicitFamily {
            self.input.settings.font_family.len()
        } else {
            DEFAULT_FAMILY.len()
        };
        add(
            add(self.binding.hash.map_or(0, str::len), captured)?,
            add(self.input.settings.font_family.len(), family)?,
        )
    }

    fn initial_failure(&self) -> Option<ResourceFailure> {
        if self.binding.mode == FontMode::Uneditable {
            Some(ResourceFailure::UneditableMetadata)
        } else if self.hash.is_some_and(|hash| !valid_hash(hash)) {
            Some(ResourceFailure::Corrupt)
        } else {
            None
        }
    }

    fn needs_font(&self) -> bool {
        self.initial_failure().is_none() && self.hash.is_some()
    }

    fn fill_metadata(
        &self,
        loaded: bool,
        failure: Option<ResourceFailure>,
    ) -> Result<SourceMetadata, NumericReject> {
        let actual_family = if loaded {
            let mut family = exact_string_capacity(add(FAMILY_PREFIX.len(), 64)?)?;
            family.push_str(FAMILY_PREFIX);
            // needs_font 已确认 Some 且 valid_hash；无 format/临时 hash copy。
            family.push_str(self.hash.ok_or(NumericReject::ConstructionFailure)?);
            family
        } else if self.binding.mode == FontMode::ExplicitFamily {
            copy_string(&self.input.settings.font_family)?
        } else {
            copy_string(DEFAULT_FAMILY)?
        };
        Ok(SourceMetadata {
            profile_id: self.input.profile_id,
            revision: self.input.revision,
            legacy_slot_version: self.input.legacy_slot_version,
            version: self.binding.version,
            mode: self.binding.mode,
            uneditable_reason: self.binding.uneditable_reason,
            saved_hash: self.binding.hash.map(copy_string).transpose()?,
            raw_family: copy_string(&self.input.settings.font_family)?,
            actual_family,
            runtime_source: match self.binding.mode {
                FontMode::LegacyUndetermined => RuntimeSource::LegacyTerminalSlot,
                FontMode::Uneditable => RuntimeSource::Uneditable,
                _ => RuntimeSource::ProfileBinding,
            },
            captured_terminal_slot: self.captured.map(copy_imported).transpose()?,
            fallback_reason: failure,
        })
    }
}

fn exact_string_capacity(len: usize) -> Result<String, NumericReject> {
    Layout::array::<u8>(len).map_err(|_| NumericReject::InvalidLayout)?;
    let mut string = String::new();
    string
        .try_reserve_exact(len)
        .map_err(|_| NumericReject::AllocationFailure)?;
    Ok(string)
}

fn copy_string(text: &str) -> Result<String, NumericReject> {
    let mut string = exact_string_capacity(text.len())?;
    string.push_str(text);
    Ok(string)
}

fn copy_imported(font: &ImportedFont) -> Result<ImportedFont, NumericReject> {
    Ok(ImportedFont {
        hash: copy_string(&font.hash)?,
        label: copy_string(&font.label)?,
    })
}

fn imported(checked: &CheckedFont) -> Result<ImportedFont, NumericReject> {
    Ok(ImportedFont {
        hash: copy_string(checked.hash())?,
        label: copy_string(checked.label())?,
    })
}

fn cancelled(cancel: &AtomicBool) -> Result<(), NumericReject> {
    if cancel.load(Ordering::Acquire) {
        Err(NumericReject::Cancelled)
    } else {
        Ok(())
    }
}

pub struct SourceOwner<'root>(SharedOwner<'root, SourceBody>);

impl<'root> SourceOwner<'root> {
    pub fn body(&self) -> &SourceBody {
        self.0.value()
    }
    pub fn admitted(&self) -> Amount {
        self.0.admitted()
    }
    pub fn try_clone(&self) -> Result<Self, NumericReject> {
        self.0.try_clone().map(Self)
    }

    pub fn bytes(&self) -> Option<&[u8]> {
        self.body().imported_font.as_ref()?;
        let range = self.body().font_buffer;
        Some(&self.0.payload()[range.offset..range.offset + range.len])
    }

    pub fn trusted_root(&self) -> &Path {
        let range = self.body().trusted_root;
        let bytes = &self.0.payload()[range.offset..range.offset + range.len];
        // SAFETY: 完整复制本进程 Path::as_encoded_bytes，未跨平台/修改/切分编码。
        Path::new(unsafe { std::ffi::OsStr::from_encoded_bytes_unchecked(bytes) })
    }

    /// metadata/reply 真 copy 有自己的 owner/grant，不保留原 Source 字节或 root。
    pub fn copy_metadata(
        &self,
        ledger: &'root ProcessLedger,
        admission: Admission,
        cancel: &AtomicBool,
    ) -> Result<SharedOwner<'root, MetadataBody>, NumericReject> {
        cancelled(cancel)?;
        let body = self.body();
        let metadata = &body.metadata;
        let imported_bytes = body
            .imported_font
            .as_ref()
            .map_or(Ok(0), |font| add(font.hash.len(), font.label.len()))?;
        let captured_bytes = metadata
            .captured_terminal_slot
            .as_ref()
            .map_or(Ok(0), |font| add(font.hash.len(), font.label.len()))?;
        let strings = add(
            add(imported_bytes, captured_bytes)?,
            add(
                metadata.saved_hash.as_ref().map_or(0, String::len),
                add(metadata.raw_family.len(), metadata.actual_family.len())?,
            )?,
        )?;
        let layout = OwnerLayout::actual::<MetadataBody>(0)?;
        let grant = ledger.try_reserve(
            Amount {
                rust_bytes: layout.required_rust(strings)?,
                c_bytes: 0,
                slots: 1,
            },
            ResourceClass::Reply,
            admission,
        )?;
        let owner = SharedOwner::try_init(grant, 0, strings, |_| {
            let metadata = SourceMetadata {
                profile_id: metadata.profile_id,
                revision: metadata.revision,
                legacy_slot_version: metadata.legacy_slot_version,
                version: metadata.version,
                mode: metadata.mode,
                uneditable_reason: metadata.uneditable_reason,
                saved_hash: metadata
                    .saved_hash
                    .as_deref()
                    .map(copy_string)
                    .transpose()?,
                raw_family: copy_string(&metadata.raw_family)?,
                actual_family: copy_string(&metadata.actual_family)?,
                runtime_source: metadata.runtime_source,
                captured_terminal_slot: metadata
                    .captured_terminal_slot
                    .as_ref()
                    .map(copy_imported)
                    .transpose()?,
                fallback_reason: metadata.fallback_reason,
            };
            let imported_font = body.imported_font.as_ref().map(copy_imported).transpose()?;
            cancelled(cancel)?;
            Ok(MetadataBody {
                imported_font,
                metadata,
            })
        })?;
        ledger.try_reserve(Amount::default(), ResourceClass::Reply, admission)?;
        Ok(owner)
    }

    /// 发送字节的独立 tail：额度不足时不复制，原 Source 仍可继续使用。
    pub fn copy_font_bytes(
        &self,
        ledger: &'root ProcessLedger,
        admission: Admission,
        cancel: &AtomicBool,
    ) -> Result<SharedOwner<'root, TailRange>, NumericReject> {
        cancelled(cancel)?;
        let bytes = self.bytes().unwrap_or_default();
        let layout = OwnerLayout::actual::<TailRange>(bytes.len())?;
        let grant = ledger.try_reserve(
            Amount {
                rust_bytes: layout.backing.size(),
                c_bytes: 0,
                slots: 1,
            },
            ResourceClass::Reply,
            admission,
        )?;
        let owner = SharedOwner::try_init(grant, bytes.len(), 0, |tail| {
            tail.copy_from_slice(bytes);
            cancelled(cancel)?;
            Ok(TailRange {
                offset: 0,
                len: bytes.len(),
            })
        })?;
        ledger.try_reserve(Amount::default(), ResourceClass::Reply, admission)?;
        Ok(owner)
    }
}

pub fn capture_bytes<'root>(
    certificate: &FontBuildProof,
    ledger: &'root ProcessLedger,
    admission: Admission,
    input: BorrowedCapture<'_>,
    available: FontAvailability<'_>,
    cancel: &AtomicBool,
) -> Result<SourceOwner<'root>, FontReject> {
    certificate.require()?;
    cancelled(cancel)?;
    let shape = CaptureShape::new(input);
    let mut failure = shape.initial_failure();
    let mut selected = None;
    let mut bytes = &[][..];
    if shape.needs_font() {
        match available {
            FontAvailability::Failed(reason) => failure = Some(reason),
            FontAvailability::Bytes(data) => match inspect(data, "cached.ttf") {
                Ok(checked) if Some(checked.hash()) == shape.hash => {
                    selected = Some(checked);
                    bytes = data;
                }
                _ => failure = Some(ResourceFailure::Corrupt),
            },
        }
    }
    cancelled(cancel)?;
    let root = input.trusted_root.path().as_os_str().as_encoded_bytes();
    let metadata_backing = shape.metadata_backing(selected.is_some())?;
    let model = SourceLayout::actual(
        add(root.len(), bytes.len())?,
        selected.as_ref().map_or(0, |_| 64),
        selected.as_ref().map_or(0, |c| c.label().len()),
        metadata_backing,
    )?;
    let grant = ledger.try_reserve(
        Amount {
            rust_bytes: model.required_rust,
            c_bytes: 0,
            slots: 1,
        },
        ResourceClass::Source,
        admission,
    )?;
    let owner = SharedOwner::try_init(
        grant,
        model.tail_bytes,
        add(
            add(model.hash_backing, model.label_backing)?,
            metadata_backing,
        )?,
        |tail| {
            tail[..root.len()].copy_from_slice(root);
            tail[root.len()..].copy_from_slice(bytes);
            let imported_font = selected.as_ref().map(imported).transpose()?;
            let metadata = shape.fill_metadata(selected.is_some(), failure)?;
            cancelled(cancel)?;
            Ok(SourceBody {
                imported_font,
                metadata,
                trusted_root: TailRange {
                    offset: 0,
                    len: root.len(),
                },
                font_buffer: TailRange {
                    offset: root.len(),
                    len: bytes.len(),
                },
            })
        },
    )?;
    ledger.try_reserve(Amount::default(), ResourceClass::Source, admission)?;
    Ok(SourceOwner(owner))
}

/// File 已由受信 root caller 按规范 hash 打开并检查目录/常规文件；句柄仍由 caller 持有。
/// captured_len 来自该句柄的已捕获 metadata。一次 read_exact 直接写入已预约的 owner tail，
/// 随后的单字节栈缓冲验证 EOF，不能只校验增长文件的已保存前缀。
/// 文件打开/路径编码与并发文件变更的捕获策略仍在父接缝，不冒称 normalPrepare 已接通。
pub fn capture_file<'root>(
    certificate: &FontBuildProof,
    ledger: &'root ProcessLedger,
    admission: Admission,
    input: BorrowedCapture<'_>,
    file: &mut File,
    captured_len: u64,
    cancel: &AtomicBool,
) -> Result<SourceOwner<'root>, FontReject> {
    certificate.require()?;
    cancelled(cancel)?;
    let shape = CaptureShape::new(input);
    if !shape.needs_font() {
        return capture_bytes(
            certificate,
            ledger,
            admission,
            input,
            FontAvailability::Failed(ResourceFailure::Missing),
            cancel,
        );
    }
    if captured_len > MAX_FONT_BYTES as u64 {
        return capture_bytes(
            certificate,
            ledger,
            admission,
            input,
            FontAvailability::Failed(ResourceFailure::Corrupt),
            cancel,
        );
    }
    let len = usize::try_from(captured_len).map_err(|_| NumericReject::Overflow)?;
    let root = input.trusted_root.path().as_os_str().as_encoded_bytes();
    // read 前不知道 label；其实际 upper 为固定 512。失败后的默认 family 也在此上界内。
    let metadata_backing = shape
        .metadata_backing(true)?
        .max(shape.metadata_backing(false)?);
    let model = SourceLayout::actual(add(root.len(), len)?, 64, LABEL_BYTES, metadata_backing)?;
    let out_of_line = add(64 + LABEL_BYTES, metadata_backing)?;
    let grant = ledger.try_reserve(
        Amount {
            rust_bytes: model.required_rust,
            c_bytes: 0,
            slots: 1,
        },
        ResourceClass::Source,
        admission,
    )?;
    let owner = SharedOwner::try_init(grant, model.tail_bytes, out_of_line, |tail| {
        tail[..root.len()].copy_from_slice(root);
        cancelled(cancel)?;
        let data = &mut tail[root.len()..];
        let read = file.read_exact(data).and_then(|()| {
            let mut extra = [0; 1];
            match file.read(&mut extra) {
                Ok(0) => Ok(()),
                Ok(_) => Err(ErrorKind::InvalidData.into()),
                Err(error) => Err(error),
            }
        });
        let (checked, failure) = match read {
            Err(error) => (
                None,
                Some(
                    if matches!(
                        error.kind(),
                        ErrorKind::UnexpectedEof | ErrorKind::InvalidData
                    ) {
                        ResourceFailure::Corrupt
                    } else {
                        ResourceFailure::Unavailable
                    },
                ),
            ),
            Ok(()) => match inspect(data, "cached.ttf") {
                Ok(checked) if Some(checked.hash()) == shape.hash => (Some(checked), None),
                _ => (None, Some(ResourceFailure::Corrupt)),
            },
        };
        cancelled(cancel)?;
        let imported_font = checked.as_ref().map(imported).transpose()?;
        let metadata = shape.fill_metadata(checked.is_some(), failure)?;
        Ok(SourceBody {
            imported_font,
            metadata,
            trusted_root: TailRange {
                offset: 0,
                len: root.len(),
            },
            font_buffer: TailRange {
                offset: root.len(),
                len: if checked.is_some() { len } else { 0 },
            },
        })
    })?;
    cancelled(cancel)?;
    ledger.try_reserve(Amount::default(), ResourceClass::Source, admission)?;
    Ok(SourceOwner(owner))
}
