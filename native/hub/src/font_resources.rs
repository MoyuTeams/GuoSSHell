//! 独立于双槽的 TTF 缓存资源；本模块不读取或发布偏好、profile 及配置选择。
// 设置与运行快照的消费端独立接入，内部接缝在此由专属测试锁定。

use std::fmt;
use std::path::Path;
use std::sync::Arc;

use crate::fonts::{self, CacheError, ImportedFont};

/// 对外仅提供固定原因，不泄露缓存路径或系统 I/O 信息。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FontResourceError {
    Missing,
    Corrupt,
    Unavailable,
}

impl FontResourceError {
    pub(crate) fn reason(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Corrupt => "corrupt",
            Self::Unavailable => "unavailable",
        }
    }
}

impl fmt::Display for FontResourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Missing => "字体缓存缺失",
            Self::Corrupt => "字体缓存损坏",
            Self::Unavailable => "字体缓存不可用",
        })
    }
}

impl std::error::Error for FontResourceError {}

impl From<CacheError> for FontResourceError {
    fn from(error: CacheError) -> Self {
        match error {
            CacheError::InvalidHash | CacheError::Corrupt => Self::Corrupt,
            CacheError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => Self::Missing,
            CacheError::Io(_) => Self::Unavailable,
        }
    }
}

/// 通过 TTF 结构和内容摘要校验的不可变资源，字段不可由调用方拼装或改写。
#[derive(Debug, Clone)]
pub(crate) struct FontResource {
    font: ImportedFont,
    bytes: Arc<[u8]>,
}

impl FontResource {
    pub(crate) fn hash(&self) -> &str {
        &self.font.hash
    }

    pub(crate) fn family(&self) -> String {
        format!("GuoshFont_{}", self.hash())
    }

    #[cfg(test)]
    pub(crate) fn label(&self) -> &str {
        &self.font.label
    }

    pub(crate) fn bytes(&self) -> &Arc<[u8]> {
        &self.bytes
    }
}

/// 仅校验内存中的真实 TTF，不持久化。无字体名称时使用固定标签以保证重载一致。
pub(crate) fn verify_ttf(bytes: Vec<u8>) -> Result<FontResource, FontResourceError> {
    let font = fonts::inspect(&bytes, "cached.ttf").map_err(|_| FontResourceError::Corrupt)?;
    Ok(FontResource {
        font,
        bytes: bytes.into(),
    })
}

/// 资源先就绪，选择随后由调用方通过配置 CAS 发布；失败或 CAS 冲突均不回收 blob。
pub(crate) fn publish(root: &Path, resource: &FontResource) -> Result<(), FontResourceError> {
    match check_cache_directory(root) {
        Ok(()) | Err(FontResourceError::Missing) => {}
        Err(error) => return Err(error),
    }
    let path = fonts::font_path(root, resource.hash()).map_err(|_| FontResourceError::Corrupt)?;
    match check_cache_file(&path) {
        Ok(()) | Err(FontResourceError::Missing) => {}
        Err(error) => return Err(error),
    }
    fonts::publish_cached(root, &resource.font, &resource.bytes).map_err(|error| match error {
        CacheError::Io(_) => FontResourceError::Unavailable,
        error => FontResourceError::from(error),
    })
}

fn check_cache_directory(root: &Path) -> Result<(), FontResourceError> {
    let metadata = std::fs::symlink_metadata(root.join("fonts"))
        .map_err(CacheError::from)
        .map_err(FontResourceError::from)?;
    if !metadata.file_type().is_dir() {
        return Err(FontResourceError::Unavailable);
    }
    Ok(())
}

fn check_cache_file(path: &Path) -> Result<(), FontResourceError> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(CacheError::from)
        .map_err(FontResourceError::from)?;
    if !metadata.file_type().is_file() {
        return Err(FontResourceError::Unavailable);
    }
    Ok(())
}

/// 只加载既有 fonts/<hash>.ttf；不按 family 推断身份，也不查询当前槽。
/// 继承/旧槽的调用方须先取得一次 ImportedFont 快照，再传入快照的 hash。
pub(crate) fn load_by_hash(root: &Path, hash: &str) -> Result<FontResource, FontResourceError> {
    // 内容摘要统一为小写，避免大小写敏感文件系统给同一输入不同的身份语义。
    if hash.bytes().any(|value| value.is_ascii_uppercase()) {
        return Err(FontResourceError::Corrupt);
    }
    let path = fonts::font_path(root, hash).map_err(|_| FontResourceError::Corrupt)?;
    // Windows 对非目录中间分量也会返回 NotFound，不能误报为 blob 缺失。
    check_cache_directory(root)?;
    check_cache_file(&path)?;
    let bytes = fonts::read_cached(&path).map_err(FontResourceError::from)?;
    let resource = verify_ttf(bytes)?;
    if resource.hash() != hash {
        return Err(FontResourceError::Corrupt);
    }
    Ok(resource)
}

#[cfg(test)]
#[path = "font_resources_tests.rs"]
mod tests;
