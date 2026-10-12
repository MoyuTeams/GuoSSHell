//! 路径发现与运行证据分开；读取的只是候选程序，不读取用户 SSH 文件。

use std::fs::File;
use std::io::Read;
#[cfg(windows)]
use std::io::{Seek, SeekFrom};
use std::path::Path;

use super::{ScopeReject, launch::absolute_path};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidateOrigin {
    ExplicitRshellSsh,
    AbsoluteSearchEntry,
}

/// 父从已有环境的路径 metadata 构造；不在 leaf 中枚举/clone 全环境或查注册表。
pub struct LocatorPaths<'a> {
    pub rshell_ssh: Option<&'a str>,
    pub absolute_search_candidates: &'a [&'a str],
}

#[derive(Clone, Copy)]
pub struct LocatorCandidate<'a> {
    path: &'a str,
    origin: CandidateOrigin,
}

impl<'a> LocatorCandidate<'a> {
    pub fn locate(paths: LocatorPaths<'a>) -> Result<Self, ScopeReject> {
        if let Some(path) = paths.rshell_ssh {
            // 包含空值的显式 override 也不允许回落。
            absolute_path(path).map_err(|_| ScopeReject::InvalidOverride)?;
            if open_image(path)
                .and_then(|file| image_stamp(&file))
                .is_err()
            {
                return Err(ScopeReject::InvalidOverride);
            }
            return Ok(Self {
                path,
                origin: CandidateOrigin::ExplicitRshellSsh,
            });
        }
        for &path in paths.absolute_search_candidates {
            if absolute_path(path).is_err() || native_name(path).is_err() {
                continue;
            }
            if open_image(path).and_then(|file| image_stamp(&file)).is_ok() {
                return Ok(Self {
                    path,
                    origin: CandidateOrigin::AbsoluteSearchEntry,
                });
            }
        }
        Err(ScopeReject::NotFound)
    }
    pub fn path(self) -> &'a str {
        self.path
    }
    pub fn origin(self) -> CandidateOrigin {
        self.origin
    }
}

fn native_name(path: &str) -> Result<(), ScopeReject> {
    let name = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(ScopeReject::NotExecutable)?;
    if if cfg!(windows) {
        !name.eq_ignore_ascii_case("ssh.exe")
    } else {
        name != "ssh"
    } {
        return Err(ScopeReject::NotExecutable);
    }
    Ok(())
}

#[derive(Clone, Copy, Default, Debug, Eq, PartialEq)]
pub struct ImageStamp {
    pub volume: u64,
    pub file_id: u64,
    pub size: u64,
    pub modified: u64,
}

/// 父的只读程序来源/完整性证明，不是运行 feature 证书。
///
/// # Safety
/// 必须证明本候选为可信已存在的本机 OpenSSH 映像，ACL/owner/签名/目录来源可信，
/// 并在借用期间保证映像与路径不可被不受信主体改写。Unix 普通 read fd 本身不锁写，
/// 因而需要额外不可变来源保证；Windows 只读 share 锁也不替代签名/ACL 检查。
pub unsafe trait ReadOnlyParentImageSource {
    fn trusted_readonly_image(&self, path: &str, stamp: ImageStamp) -> bool;
}

pub struct ExeIdentity<'a> {
    path: &'a str,
    origin: CandidateOrigin,
    file: Option<File>,
    stamp: ImageStamp,
    source: Option<&'a dyn ReadOnlyParentImageSource>,
    #[cfg(test)]
    fixture_only: bool,
}

impl<'a> ExeIdentity<'a> {
    pub fn read_locked(
        candidate: LocatorCandidate<'a>,
        source: &'a dyn ReadOnlyParentImageSource,
    ) -> Result<Self, ScopeReject> {
        let mut file = open_image(candidate.path)?;
        let stamp = image_stamp(&file)?;
        let mut header = [0; 64];
        file.read_exact(&mut header)
            .map_err(|_| ScopeReject::NotExecutable)?;
        #[cfg(windows)]
        {
            if &header[..2] != b"MZ" {
                return Err(ScopeReject::NotExecutable);
            }
            let offset =
                u32::from_le_bytes([header[60], header[61], header[62], header[63]]) as u64;
            if offset > stamp.size.saturating_sub(4) {
                return Err(ScopeReject::NotExecutable);
            }
            file.seek(SeekFrom::Start(offset))
                .map_err(|_| ScopeReject::Io)?;
            let mut signature = [0; 4];
            file.read_exact(&mut signature)
                .map_err(|_| ScopeReject::NotExecutable)?;
            if signature != *b"PE\0\0" {
                return Err(ScopeReject::NotExecutable);
            }
        }
        #[cfg(target_os = "linux")]
        if &header[..4] != b"\x7fELF" {
            return Err(ScopeReject::NotExecutable);
        }
        #[cfg(target_os = "macos")]
        if ![
            0xfeedfaceu32,
            0xfeedfacf,
            0xcefaedfe,
            0xcffaedfe,
            0xcafebabe,
            0xbebafeca,
        ]
        .contains(&u32::from_ne_bytes([
            header[0], header[1], header[2], header[3],
        ])) {
            return Err(ScopeReject::NotExecutable);
        }
        if !source.trusted_readonly_image(candidate.path, stamp) {
            return Err(ScopeReject::UntrustedImage);
        }
        Ok(Self {
            path: candidate.path,
            origin: candidate.origin,
            file: Some(file),
            stamp,
            source: Some(source),
            #[cfg(test)]
            fixture_only: false,
        })
    }

    pub fn path(&self) -> &'a str {
        self.path
    }
    pub fn stamp(&self) -> ImageStamp {
        self.stamp
    }
    pub fn origin(&self) -> CandidateOrigin {
        self.origin
    }

    /// 每次运行前重验：缓存不能仅由路径/版本文字维持。
    pub fn revalidate(&self) -> Result<(), ScopeReject> {
        #[cfg(test)]
        if self.fixture_only {
            return Ok(());
        }
        let file = self.file.as_ref().ok_or(ScopeReject::ImageNotLocked)?;
        let source = self.source.ok_or(ScopeReject::ImageNotLocked)?;
        let current = open_image(self.path)?;
        if image_stamp(file)? != self.stamp || image_stamp(&current)? != self.stamp {
            return Err(ScopeReject::ImageChanged);
        }
        if !source.trusted_readonly_image(self.path, self.stamp) {
            return Err(ScopeReject::UntrustedImage);
        }
        Ok(())
    }

    /// 仅编译到独立测试；不创建生产任意 exe override 或运行证明。
    #[cfg(test)]
    pub fn test_only_trusted_codec(path: &'a str) -> Result<Self, ScopeReject> {
        absolute_path(path)?;
        Ok(Self {
            path,
            origin: CandidateOrigin::AbsoluteSearchEntry,
            file: None,
            stamp: ImageStamp::default(),
            source: None,
            fixture_only: true,
        })
    }
}

#[cfg(windows)]
fn open_image(path: &str) -> Result<File, ScopeReject> {
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::{GENERIC_READ, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, OPEN_EXISTING,
    };
    let mut wide = [0u16; 32_768];
    for (len, unit) in path.encode_utf16().enumerate() {
        if len == wide.len() - 1 {
            return Err(ScopeReject::Encoding);
        }
        wide[len] = unit;
    }
    // SAFETY: 绝对、已验证无 NUL 的路径，栈上 NUL 终结；只读，不允许共享写/删除。
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(ScopeReject::Io);
    }
    // SAFETY: 成功的唯一 owned handle 转给 File，错误/正常路径均 CloseHandle。
    Ok(unsafe { File::from_raw_handle(handle.cast()) })
}

#[cfg(windows)]
fn image_stamp(file: &File) -> Result<ImageStamp, ScopeReject> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle().cast(), &mut info) } == 0 {
        return Err(ScopeReject::Io);
    }
    Ok(ImageStamp {
        volume: info.dwVolumeSerialNumber as u64,
        file_id: ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64,
        size: ((info.nFileSizeHigh as u64) << 32) | info.nFileSizeLow as u64,
        modified: ((info.ftLastWriteTime.dwHighDateTime as u64) << 32)
            | info.ftLastWriteTime.dwLowDateTime as u64,
    })
}

#[cfg(unix)]
fn open_image(path: &str) -> Result<File, ScopeReject> {
    use std::os::fd::FromRawFd;
    use std::os::unix::fs::MetadataExt;
    unsafe extern "C" {
        fn open(path: *const std::ffi::c_char, flags: std::ffi::c_int) -> std::ffi::c_int;
    }
    let mut bytes = [0u8; 32_768];
    if path.len() >= bytes.len() {
        return Err(ScopeReject::Encoding);
    }
    bytes[..path.len()].copy_from_slice(path.as_bytes());
    #[cfg(target_os = "linux")]
    let close_on_exec = 0o2000000;
    #[cfg(target_os = "macos")]
    let close_on_exec = 0x01000000;
    // SAFETY: 栈上的已验证 UTF-8/NUL 路径；O_RDONLY | O_CLOEXEC，不依赖环境/PATH。
    let fd = unsafe { open(bytes.as_ptr().cast(), close_on_exec) };
    if fd < 0 {
        return Err(ScopeReject::Io);
    }
    let file = unsafe { File::from_raw_fd(fd) };
    let metadata = file.metadata().map_err(|_| ScopeReject::Io)?;
    if !metadata.is_file() || metadata.mode() & 0o111 == 0 || metadata.mode() & 0o022 != 0 {
        return Err(ScopeReject::NotExecutable);
    }
    Ok(file)
}

#[cfg(unix)]
fn image_stamp(file: &File) -> Result<ImageStamp, ScopeReject> {
    use std::os::unix::fs::MetadataExt;
    let info = file.metadata().map_err(|_| ScopeReject::Io)?;
    Ok(ImageStamp {
        volume: info.dev(),
        file_id: info.ino(),
        size: info.size(),
        modified: (info.mtime() as u64)
            .wrapping_mul(1_000_000_000)
            .wrapping_add(info.mtime_nsec() as u64),
    })
}
