//! 父平台 caller 的受信数据目录 root；字体 leaf 只打开其 fonts 缓存。

use std::fs::File;
use std::path::{Component, Path};

use crate::selected_p1::font_source::source::TailRange;
use crate::selected_p1::model::layout::{NumericReject, add};
use crate::selected_p1::resource::ledger::Grant;
use crate::selected_p1::resource::owner::{OwnerLayout, SharedOwner};

use super::model::FileIdentity;

#[repr(C)]
struct RootBody {
    directory: File,
    identity: FileIdentity,
    encoded_path: TailRange,
}

pub struct BudgetedCacheRoot<'root>(SharedOwner<'root, RootBody>);

impl<'root> BudgetedCacheRoot<'root> {
    /// path 是已受信的数据目录身份，不能来自 Dart；fonts 子目录允许尚不存在。
    /// 父先取得 required_rust(path, root_open_upper) 的真实 Grant，再调用本函数。
    /// open 只能产生一个目录 File；root_open_upper 必须来自父实际平台打开闭包的源模型，
    /// 覆盖编码、临时量及失败路径，不能填测量峰值或猜测常数。
    /// 借用 path 的原 backing 由父原 root 独立持额；不能用此新 owner 给旧分配补签。
    /// 该数值不被本 leaf 认作完整 normal root 证书；不会接管预先打开的无预算句柄。
    pub fn try_init_for_caller(
        grant: Grant<'root>,
        path: &Path,
        root_open_upper: usize,
        open: impl FnOnce() -> Result<File, NumericReject>,
    ) -> Result<Self, NumericReject> {
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (grant, path, root_open_upper, open);
            Err(NumericReject::UnsupportedEnvelope)
        }
        #[cfg(target_os = "windows")]
        {
            validate_path(path)?;
            let bytes = path.as_os_str().as_encoded_bytes();
            let required = Self::required_rust(path, root_open_upper)?;
            if grant.amount().rust_bytes < required {
                return Err(NumericReject::RustBudgetExceeded);
            }
            SharedOwner::try_init(grant, bytes.len(), root_open_upper, |tail| {
                tail.copy_from_slice(bytes);
                let directory = open()?;
                let identity = super::windows::directory_identity(&directory)?;
                Ok(RootBody {
                    directory,
                    identity,
                    encoded_path: TailRange {
                        offset: 0,
                        len: bytes.len(),
                    },
                })
            })
            .map(Self)
        }
    }

    pub fn required_rust(path: &Path, root_open_upper: usize) -> Result<usize, NumericReject> {
        validate_path(path)?;
        add(
            OwnerLayout::actual::<RootBody>(path.as_os_str().as_encoded_bytes().len())?
                .backing
                .size(),
            root_open_upper,
        )
    }

    pub fn borrowed(&self) -> TrustedCacheRoot<'_, 'root> {
        TrustedCacheRoot { root: self }
    }
}

/// 借用保持父 root 的 File、路径 tail 及真实 Grant；不暴露给 Dart。
#[derive(Clone, Copy)]
pub struct TrustedCacheRoot<'a, 'root> {
    root: &'a BudgetedCacheRoot<'root>,
}

impl<'a, 'root> TrustedCacheRoot<'a, 'root> {
    pub fn path(self) -> &'a Path {
        let range = self.root.0.value().encoded_path;
        let bytes = &self.root.0.payload()[range.offset..range.offset + range.len];
        // SAFETY: 同平台完整 Path 编码仅复制一次，未修改或拆分字符串编码。
        Path::new(unsafe { std::ffi::OsStr::from_encoded_bytes_unchecked(bytes) })
    }

    pub(crate) fn directory(self) -> &'a File {
        &self.root.0.value().directory
    }

    pub fn identity(self) -> FileIdentity {
        self.root.0.value().identity
    }
}

fn validate_path(path: &Path) -> Result<(), NumericReject> {
    let bytes = path.as_os_str().as_encoded_bytes();
    if !path.is_absolute()
        || bytes.contains(&0)
        || bytes
            .split(|byte| matches!(byte, b'/' | b'\\'))
            .any(|part| matches!(part, b"." | b".."))
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err(NumericReject::InvalidSource);
    }
    Ok(())
}
