//! W1-F 的借用字体检查、元数据解析与受额度约束的不可变源。
//! 父入口须先以借用方式捕获 settings/槽/root；本单元不读取偏好或扫描目录。

pub mod closure;
pub mod inspect;
pub mod metadata;
pub mod source;

pub use closure::{FontBuild, FontBuildProof, GNU_FONT_BUILD};
pub use inspect::{CheckedFont, FontReject, MAX_FONT_BYTES, inspect};
pub use metadata::{BindingRef, FontMode, MetadataError, ResourceFailure, saved_binding};
pub use source::{
    BorrowedCapture, FontAvailability, SourceBody, SourceLayout, SourceMetadata, SourceOwner,
    TrustedRoot, capture_bytes, capture_file,
};
