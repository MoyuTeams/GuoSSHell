//! 受信数据目录句柄上的 fonts/<hash>.ttf；不调用 std 的无界路径规范化链。

use std::alloc::{alloc, dealloc};
use std::ffi::c_void;
use std::fs::File;
use std::mem::ManuallyDrop;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::selected_p1::font_source::{
    BorrowedCapture, FontAvailability, FontBuildProof, FontReject, ResourceFailure, SourceOwner,
    capture_bytes, capture_file,
};
use crate::selected_p1::model::layout::NumericReject;
use crate::selected_p1::resource::ledger::{
    Admission, Amount, Grant, ProcessLedger, ResourceClass,
};

use super::model::{FileIdentity, ReadWrapper, ResourceContract};
use super::root::TrustedCacheRoot;

const DIRECTORY: u32 = 0x10;
const REPARSE_POINT: u32 = 0x400;

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *const u16,
}

#[repr(C)]
struct ObjectAttributes {
    length: u32,
    root_directory: *mut c_void,
    object_name: *const UnicodeString,
    attributes: u32,
    security_descriptor: *mut c_void,
    security_quality_of_service: *mut c_void,
}

#[repr(C)]
struct IoStatusBlock {
    status_or_pointer: usize,
    information: usize,
}

#[repr(C)]
#[derive(Default)]
struct ByHandleInformation {
    attributes: u32,
    creation: [u32; 2],
    last_access: [u32; 2],
    last_write: [u32; 2],
    volume: u32,
    size_high: u32,
    size_low: u32,
    links: u32,
    index_high: u32,
    index_low: u32,
}

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtCreateFile(
        handle: *mut *mut c_void,
        desired_access: u32,
        object_attributes: *const ObjectAttributes,
        io_status: *mut IoStatusBlock,
        allocation_size: *const i64,
        file_attributes: u32,
        share_access: u32,
        create_disposition: u32,
        create_options: u32,
        ea_buffer: *const c_void,
        ea_length: u32,
    ) -> i32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetFileInformationByHandle(
        handle: *mut c_void,
        information: *mut ByHandleInformation,
    ) -> i32;
}

fn identity(file: &File) -> Result<FileIdentity, ResourceFailure> {
    let mut information = ByHandleInformation::default();
    // SAFETY: File 保持句柄存活，information 是正确 repr(C) 的独占完整输出。
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
        return Err(ResourceFailure::Unavailable);
    }
    Ok(FileIdentity {
        volume: information.volume,
        index_high: information.index_high,
        index_low: information.index_low,
        attributes: information.attributes,
        length: (u64::from(information.size_high) << 32) | u64::from(information.size_low),
        last_write: (u64::from(information.last_write[1]) << 32)
            | u64::from(information.last_write[0]),
    })
}

pub(crate) fn directory_identity(file: &File) -> Result<FileIdentity, NumericReject> {
    let identity = identity(file).map_err(|_| NumericReject::InvalidSource)?;
    if identity.attributes & (DIRECTORY | REPARSE_POINT) != DIRECTORY {
        return Err(NumericReject::InvalidSource);
    }
    Ok(identity)
}

/// wrapper 的唯一 owner；关闭 File → free 原 Layout → return Grant。
struct OpenLoan<'root> {
    wrapper: NonNull<ReadWrapper>,
    contract: ResourceContract,
    grant: ManuallyDrop<Grant<'root>>,
}

impl<'root> OpenLoan<'root> {
    fn before_open(
        proof: &FontBuildProof,
        ledger: &'root ProcessLedger,
        admission: Admission,
        hash: &str,
    ) -> Result<Self, NumericReject> {
        let contract = ResourceContract::actual(proof)?;
        let grant = ledger.try_reserve(
            Amount {
                rust_bytes: contract.open_rust_bytes,
                c_bytes: 0,
                slots: 1,
            },
            ResourceClass::Source,
            admission,
        )?;
        // SAFETY: 固定非零 actual Layout；析构使用同 allocator 与原 Layout。
        let wrapper = NonNull::new(unsafe { alloc(contract.read_wrapper) })
            .ok_or(NumericReject::AllocationFailure)?
            .cast::<ReadWrapper>();
        let mut filename = [0; super::model::FILE_NAME_UNITS];
        for (output, input) in filename.iter_mut().zip(
            b"fonts\\"
                .iter()
                .copied()
                .chain(hash.bytes())
                .chain(b".ttf".iter().copied()),
        ) {
            *output = u16::from(input);
        }
        // SAFETY: 唯一已分配且对齐的 wrapper；初始化失败前尚无需要析构的 File。
        unsafe {
            wrapper.as_ptr().write(ReadWrapper {
                file: None,
                before: FileIdentity::default(),
                filename,
            });
        }
        #[cfg(test)]
        probe::path_copy();
        Ok(Self {
            wrapper,
            contract,
            grant: ManuallyDrop::new(grant),
        })
    }

    fn value_mut(&mut self) -> &mut ReadWrapper {
        // SAFETY: OpenLoan 是唯一 holder；可变借用不会逃出同步 capture。
        unsafe { self.wrapper.as_mut() }
    }

    fn open(&mut self, root: TrustedCacheRoot<'_, '_>) -> Result<(), ResourceFailure> {
        let wrapper = self.value_mut();
        let name_bytes =
            u16::try_from(wrapper.filename.len() * 2).map_err(|_| ResourceFailure::Unavailable)?;
        let name = UnicodeString {
            length: name_bytes,
            maximum_length: name_bytes,
            buffer: wrapper.filename.as_ptr(),
        };
        let attributes = ObjectAttributes {
            length: std::mem::size_of::<ObjectAttributes>() as u32,
            root_directory: root.directory().as_raw_handle(),
            object_name: &name,
            // OBJ_CASE_INSENSITIVE | OBJ_DONT_REPARSE；fonts 或文件均不得重解析。
            attributes: 0x40 | 0x1000,
            security_descriptor: std::ptr::null_mut(),
            security_quality_of_service: std::ptr::null_mut(),
        };
        let mut io_status = IoStatusBlock {
            status_or_pointer: 0,
            information: 0,
        };
        let mut handle = std::ptr::null_mut();
        #[cfg(test)]
        probe::file_open();
        // SAFETY: 借用 root 句柄、固定宽字串及所有 FFI 栈参数保持活到同步调用返回。
        // FILE_GENERIC_READ；仅 FILE_SHARE_READ，禁止写入/删除共享。FILE_OPEN 只打开既有文件；
        // FILE_NON_DIRECTORY_FILE | FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_REPARSE_POINT。
        let status = unsafe {
            NtCreateFile(
                &mut handle,
                0x0012_0089,
                &attributes,
                &mut io_status,
                std::ptr::null(),
                0,
                1,
                1,
                0x40 | 0x20 | 0x0020_0000,
                std::ptr::null(),
                0,
            )
        };
        if status < 0 {
            return Err(match status as u32 {
                0xc000_0034 | 0xc000_003a | 0xc000_000f => ResourceFailure::Missing,
                _ => ResourceFailure::Unavailable,
            });
        }
        // SAFETY: NtCreateFile 成功交付唯一同步 File 句柄，由 wrapper 正常 CloseHandle。
        wrapper.file = Some(unsafe { File::from_raw_handle(handle) });
        wrapper.before = identity(wrapper.file.as_ref().ok_or(ResourceFailure::Unavailable)?)?;
        if wrapper.before.attributes & (DIRECTORY | REPARSE_POINT) != 0 {
            return Err(ResourceFailure::Unavailable);
        }
        #[cfg(test)]
        probe::check_pinning(&attributes);
        Ok(())
    }
}

impl Drop for OpenLoan<'_> {
    fn drop(&mut self) {
        // SAFETY: 唯一 initialized wrapper，取走额度后关闭句柄、free，最后返额。
        let grant = unsafe { ManuallyDrop::take(&mut self.grant) };
        unsafe {
            std::ptr::drop_in_place(self.wrapper.as_ptr());
            dealloc(self.wrapper.as_ptr().cast(), self.contract.read_wrapper);
        }
        #[cfg(test)]
        probe::cleanup();
        drop(grant);
    }
}

pub(crate) fn capture_cached<'root>(
    proof: &FontBuildProof,
    ledger: &'root ProcessLedger,
    admission: Admission,
    input: BorrowedCapture<'_>,
    root: TrustedCacheRoot<'_, '_>,
    hash: &str,
    cancel: &AtomicBool,
) -> Result<SourceOwner<'root>, FontReject> {
    let mut loan = OpenLoan::before_open(proof, ledger, admission, hash)?;
    if cancel.load(Ordering::Acquire) {
        return Err(NumericReject::Cancelled.into());
    }
    let opened = loan.open(root);
    #[cfg(test)]
    probe::after_open(loan.value_mut());
    let source = match opened {
        Err(failure) => capture_bytes(
            proof,
            ledger,
            admission,
            input,
            FontAvailability::Failed(failure),
            cancel,
        )?,
        Ok(()) => {
            let wrapper = loan.value_mut();
            let file = wrapper
                .file
                .as_mut()
                .ok_or(NumericReject::ConstructionFailure)?;
            let source = capture_file(
                proof,
                ledger,
                admission,
                input,
                file,
                wrapper.before.length,
                cancel,
            )?;
            let after = identity(file);
            let changed = match after {
                Ok(after) if after == wrapper.before => None,
                Ok(_) => Some(ResourceFailure::Corrupt),
                Err(failure) => Some(failure),
            };
            if let Some(failure) = changed {
                // 不能发布捕获中变化的 bytes；旧配置与槽 metadata 仍原样保留。
                drop(source);
                capture_bytes(
                    proof,
                    ledger,
                    admission,
                    input,
                    FontAvailability::Failed(failure),
                    cancel,
                )?
            } else {
                source
            }
        }
    };
    drop(loan);
    if cancel.load(Ordering::Acquire) {
        return Err(NumericReject::Cancelled.into());
    }
    ledger.try_reserve(Amount::default(), ResourceClass::Source, admission)?;
    Ok(source)
}

#[cfg(test)]
pub mod probe {
    use std::cell::Cell;

    use std::os::windows::io::FromRawHandle;

    use super::{IoStatusBlock, NtCreateFile, ObjectAttributes, ReadWrapper};

    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    pub struct Counters {
        pub opens: usize,
        pub path_copies: usize,
        pub cleanups: usize,
    }
    thread_local! {
        static COUNTERS: Cell<Counters> = const { Cell::new(Counters { opens: 0, path_copies: 0, cleanups: 0 }) };
        static CAPTURED_LENGTH: Cell<Option<u64>> = const { Cell::new(None) };
        static PIN_CHECK: Cell<bool> = const { Cell::new(false) };
        static PIN_STATUS: Cell<Option<(u32, u32)>> = const { Cell::new(None) };
    }
    pub fn reset() {
        COUNTERS.set(Counters::default());
        CAPTURED_LENGTH.set(None);
        PIN_CHECK.set(false);
        PIN_STATUS.set(None);
    }
    pub fn counters() -> Counters {
        COUNTERS.get()
    }
    /// 仅测试改变捕获长度；真实读仍走同一个 provider 打开的真实 File。
    pub fn captured_length(length: u64) {
        CAPTURED_LENGTH.set(Some(length));
    }
    pub fn check_write_and_delete_pinning() {
        PIN_CHECK.set(true);
    }
    pub fn pin_status() -> Option<(u32, u32)> {
        PIN_STATUS.get()
    }
    pub(super) fn check_pinning(attributes: &ObjectAttributes) {
        if !PIN_CHECK.take() {
            return;
        }
        let attempt = |access| {
            let mut handle = std::ptr::null_mut();
            let mut io_status = IoStatusBlock {
                status_or_pointer: 0,
                information: 0,
            };
            // SAFETY: 仅对本轮真实借用的同一固定相对名请求写/删访问，不执行写/删。
            let status = unsafe {
                NtCreateFile(
                    &mut handle,
                    access,
                    attributes,
                    &mut io_status,
                    std::ptr::null(),
                    0,
                    7,
                    1,
                    0x40 | 0x20 | 0x0020_0000,
                    std::ptr::null(),
                    0,
                )
            };
            if status >= 0 {
                // SAFETY: 意外成功的测试句柄唯一接管并立即关闭，不修改测试文件。
                drop(unsafe { std::fs::File::from_raw_handle(handle) });
            }
            status as u32
        };
        PIN_STATUS.set(Some((attempt(0x0012_0116), attempt(0x0011_0000))));
    }
    pub(super) fn file_open() {
        let mut counters = COUNTERS.get();
        counters.opens += 1;
        COUNTERS.set(counters);
    }
    pub(super) fn path_copy() {
        let mut counters = COUNTERS.get();
        counters.path_copies += 1;
        COUNTERS.set(counters);
    }
    pub(super) fn cleanup() {
        let mut counters = COUNTERS.get();
        counters.cleanups += 1;
        COUNTERS.set(counters);
    }
    pub(super) fn after_open(wrapper: &mut ReadWrapper) {
        if let Some(length) = CAPTURED_LENGTH.take() {
            wrapper.before.length = length;
        }
    }
}
