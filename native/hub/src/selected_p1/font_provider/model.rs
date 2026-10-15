//! GNU provider 的实际 layout 与有限调用契约；不为父 root-open 闭包签证。

use std::alloc::Layout;

use crate::selected_p1::font_source::FontBuildProof;
use crate::selected_p1::model::layout::NumericReject;

pub const HASH_BYTES: usize = 64;
pub const FILE_NAME_UNITS: usize = 6 + HASH_BYTES + 4;

/// GetFileInformationByHandle 的固定栈输出；不是 Rust allocator backing。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(C)]
pub struct FileIdentity {
    pub volume: u32,
    pub index_high: u32,
    pub index_low: u32,
    pub attributes: u32,
    pub length: u64,
    pub last_write: u64,
}

impl FileIdentity {
    pub fn same_file(self, other: Self) -> bool {
        (self.volume, self.index_high, self.index_low)
            == (other.volume, other.index_high, other.index_low)
    }
}

/// Heap 上唯一的 open/read wrapper；filename 无 OsString/PathBuf/Vec 中间副本。
#[repr(C)]
pub(crate) struct ReadWrapper {
    pub file: Option<std::fs::File>,
    pub before: FileIdentity,
    pub filename: [u16; FILE_NAME_UNITS],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResourceContract {
    pub read_wrapper: Layout,
    pub file: Layout,
    pub file_identity: Layout,
    pub filename_units: usize,
    pub open_rust_bytes: usize,
}

impl ResourceContract {
    pub fn actual(proof: &FontBuildProof) -> Result<Self, NumericReject> {
        proof.require()?;
        if !cfg!(all(target_os = "windows", target_env = "gnu")) {
            return Err(NumericReject::UnsupportedEnvelope);
        }
        let read_wrapper = Layout::new::<ReadWrapper>();
        Ok(Self {
            read_wrapper,
            file: Layout::new::<std::fs::File>(),
            file_identity: Layout::new::<FileIdentity>(),
            filename_units: FILE_NAME_UNITS,
            open_rust_bytes: read_wrapper.size(),
        })
    }
}
