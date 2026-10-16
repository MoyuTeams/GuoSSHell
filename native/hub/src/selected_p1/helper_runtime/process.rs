//! helper v2 的最小独立 reader；协议常量来自生产计划 §3.2，不复制上游源码。

use crate::selected_p1::{
    model::layout::{NumericReject, add, extend, mul},
    resource::ledger::{Admission, Amount, Grant, ProcessLedger, ResourceClass},
};
use std::alloc::{Layout, alloc, dealloc};
use std::ptr::NonNull;

pub const REQUEST_LEN: usize = 112;
pub const PREAMBLE_LEN: usize = 64;
pub const FIELD_LEN: usize = 24;
pub const FOOTER_LEN: usize = 128;
pub const PREAMBLE_MAGIC: &[u8; 8] = b"SHSELHD2";
pub const FIELD_MAGIC: &[u8; 8] = b"SHSELFD2";
pub const FOOTER_MAGIC: &[u8; 8] = b"SHSELFN2";
pub const GROUP_FACT_SHIFT: u32 = 8;
pub const GROUP_FACT_MASK: u32 = 7 << GROUP_FACT_SHIFT;
pub const SUCCESS_FLAGS: u32 = 95;

/// 私有 wire 的数字事实；只有收割后的 RequiredFacts 才关联受信事务身份。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum GroupReferenceFact {
    NoReference = 1,
    Exists = 2,
    Absent = 3,
    InvalidUuid = 4,
    Unproven = 5,
}

impl GroupReferenceFact {
    pub(crate) fn decode(value: u16) -> Result<Self, Failure> {
        match value {
            1 => Ok(Self::NoReference),
            2 => Ok(Self::Exists),
            3 => Ok(Self::Absent),
            4 => Ok(Self::InvalidUuid),
            5 => Ok(Self::Unproven),
            _ => Err(Failure::Protocol),
        }
    }
}

/// borrowed Prepare 已取得的三类额度，后续移动到真实 helper owner，不重复预约。
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct LoadCosts {
    pub helper: Amount,
    pub spawn: Amount,
    pub raw: Amount,
}

#[repr(C)]
pub struct LoadReservations<'r> {
    root: &'r ProcessLedger,
    admission: Admission,
    helper: Grant<'r>,
    spawn: Grant<'r>,
    raw: Grant<'r>,
}
impl<'r> LoadReservations<'r> {
    pub fn belongs_to(&self, root: &ProcessLedger) -> bool {
        std::ptr::eq(self.root, root)
    }
    pub fn admission(&self) -> Admission {
        self.admission
    }
    pub fn into_parts(self) -> (Grant<'r>, Grant<'r>, Grant<'r>) {
        (self.helper, self.spawn, self.raw)
    }
}

impl LoadCosts {
    pub fn reserve(
        self,
        root: &ProcessLedger,
        admission: Admission,
    ) -> Result<LoadReservations<'_>, NumericReject> {
        let helper = root.try_reserve(self.helper, ResourceClass::Helper, admission)?;
        let spawn = root.try_reserve(self.spawn, ResourceClass::Runtime, admission)?;
        let raw = root.try_reserve(self.raw, ResourceClass::Raw, admission)?;
        Ok(LoadReservations {
            root,
            admission,
            helper,
            spawn,
            raw,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Failure {
    Spawn = 1,
    Io = 2,
    Truncated = 3,
    Protocol = 4,
    Cancelled = 5,
    TimedOut = 6,
    ChildCrashed = 7,
    Wait = 8,
    Unsupported = 9,
    Budget = 10,
    Allocation = 11,
    Helper = 12,
}

#[derive(Clone, Copy, Eq, PartialEq)]
#[repr(C)]
pub struct Request {
    pub bytes: [u8; REQUEST_LEN],
    pub c_max: usize,
    pub helper_rust: usize,
    pub field_max: usize,
    pub total_max: usize,
    pub wire_max: usize,
    pub frames: u32,
    pub connected: bool,
    pub nonce: [u8; 16],
}

fn u16_at(b: &[u8], n: usize) -> u16 {
    u16::from_le_bytes([b[n], b[n + 1]])
}
fn u32_at(b: &[u8], n: usize) -> u32 {
    u32::from_le_bytes([b[n], b[n + 1], b[n + 2], b[n + 3]])
}
fn u64_at(b: &[u8], n: usize) -> u64 {
    u64::from_le_bytes([
        b[n],
        b[n + 1],
        b[n + 2],
        b[n + 3],
        b[n + 4],
        b[n + 5],
        b[n + 6],
        b[n + 7],
    ])
}
fn usize_at(b: &[u8], n: usize) -> Result<usize, Failure> {
    usize::try_from(u64_at(b, n)).map_err(|_| Failure::Protocol)
}

impl Request {
    fn canonical(&self) -> bool {
        let magic = [
            self.bytes[0],
            self.bytes[1],
            self.bytes[2],
            self.bytes[3],
            self.bytes[4],
            self.bytes[5],
            self.bytes[6],
            self.bytes[7],
        ];
        Self::parse(self.bytes, &magic).is_ok_and(|request| request == *self)
    }
    /// request magic 由父的上游 codec 投影提供，不猜上游 v1 魔数。
    pub fn parse(bytes: [u8; REQUEST_LEN], request_magic: &[u8; 8]) -> Result<Self, Failure> {
        let flags = u16_at(&bytes, 10);
        if &bytes[..8] != request_magic
            || u16_at(&bytes, 8) != 2
            || flags & !3 != 0
            || bytes[92..96] != [0; 4]
        {
            return Err(Failure::Protocol);
        }
        let c_max = usize_at(&bytes, 12)?;
        let field_max = usize_at(&bytes, 20)?;
        let total_max = usize_at(&bytes, 28)?;
        let tags = u32_at(&bytes, 36);
        let connected = flags & 1 != 0;
        if !(65536..=67108864).contains(&c_max)
            || !c_max.is_multiple_of(16)
            || !(1..=1048576).contains(&field_max)
            || !(field_max..=4194304).contains(&total_max)
            || tags > 4096
            || (flags & 1 == 0 && bytes[40..56] != [0; 16])
            || (flags & 2 == 0 && bytes[56..72] != [0; 16])
        {
            return Err(Failure::Protocol);
        }
        let frames = u32_at(&bytes, 88);
        let expected_frames = if connected { 22 + tags } else { 6 };
        let expected_wire = add(
            add(
                add(
                    64,
                    mul(24, expected_frames as usize).map_err(|_| Failure::Protocol)?,
                )
                .map_err(|_| Failure::Protocol)?,
                total_max,
            )
            .map_err(|_| Failure::Protocol)?,
            if connected { 138 } else { 128 },
        )
        .map_err(|_| Failure::Protocol)?;
        let wire_max = usize_at(&bytes, 72)?;
        let helper_rust = usize_at(&bytes, 80)?;
        if frames != expected_frames || wire_max != expected_wire || helper_rust == 0 {
            return Err(Failure::Protocol);
        }
        let mut nonce = [0; 16];
        nonce.copy_from_slice(&bytes[96..112]);
        Ok(Self {
            bytes,
            c_max,
            helper_rust,
            field_max,
            total_max,
            wire_max,
            frames,
            connected,
            nonce,
        })
    }
}

#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct LaunchShape {
    pub argument_units: [usize; 8],
    pub arguments: usize,
    pub executable_units: usize,
    pub command_units: usize,
    pub inherits_environment: bool,
    pub inherits_cwd: bool,
}

#[derive(Clone, Copy)]
pub struct LaunchIdentity<'a> {
    pub executable: &'a std::ffi::OsStr,
    pub arguments: &'a [&'a std::ffi::OsStr],
}

#[derive(Clone, Copy, Debug)]
pub struct CertifiedHelper {
    pub amount: Amount,
    pub artifact_sha256: [u8; 32],
}

/// # Safety
/// 实现者是父构建所有者：必须验证实际 helper/build/cfg/codec/source 闭包与 LaunchShape，
/// 返回的 H 必须覆盖启动前及退出前全部 helper Rust allocation。wire flags 不构成证书。
/// 必须把完整路径、实际参数、请求及产物 SHA256 绑定到同一受证记录；长度相同不算匹配。
/// 正常 helper 自报数字或 from_trusted 类构造器不构成 source/readonly 事务证明。
/// 方法不得 owned-copy、格式化或读取用户配置；调用者授权只适用于该次 immutable launch。
pub unsafe trait HelperBuildRecord {
    fn request_magic(&self) -> &[u8; 8];
    fn authorize(
        &self,
        identity: LaunchIdentity<'_>,
        shape: &LaunchShape,
        request: &Request,
    ) -> Result<CertifiedHelper, Failure>;
}

#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct FieldDescriptor {
    pub offset: usize,
    pub len: usize,
    pub tag: u16,
    pub kind: u8,
    pub ordinal: u32,
}

#[repr(C)]
pub struct RawStorage<'r> {
    pointer: NonNull<u8>,
    layout: Layout,
    bytes_offset: usize,
    len: usize,
    count: usize,
    descriptor_capacity: usize,
    wire_capacity: usize,
    permit: Grant<'r>,
}
// SAFETY: 唯一 owner；读写线程只移动，不共享可变 slice。
unsafe impl Send for RawStorage<'_> {}

impl<'r> RawStorage<'r> {
    pub fn layout(request: &Request) -> Result<(Layout, usize), NumericReject> {
        if !request.canonical() {
            return Err(NumericReject::UnsupportedEnvelope);
        }
        extend(
            Layout::array::<FieldDescriptor>(request.frames as usize)
                .map_err(|_| NumericReject::InvalidLayout)?,
            Layout::array::<u8>(request.wire_max).map_err(|_| NumericReject::InvalidLayout)?,
        )
    }
    pub fn allocate(
        ledger: &'r ProcessLedger,
        admission: Admission,
        request: &Request,
    ) -> Result<Self, NumericReject> {
        let (layout, _) = Self::layout(request)?;
        let permit = ledger.try_reserve(
            Amount {
                rust_bytes: layout.size(),
                c_bytes: 0,
                slots: 1,
            },
            ResourceClass::Raw,
            admission,
        )?;
        Self::from_grant(permit, request)
    }
    pub fn from_grant(permit: Grant<'r>, request: &Request) -> Result<Self, NumericReject> {
        let (layout, bytes_offset) = Self::layout(request)?;
        if permit.class() != ResourceClass::Raw
            || permit.amount().rust_bytes < layout.size()
            || permit.amount().slots != 1
        {
            return Err(NumericReject::RustBudgetExceeded);
        }
        let pointer =
            NonNull::new(unsafe { alloc(layout) }).ok_or(NumericReject::AllocationFailure)?;
        unsafe {
            pointer.as_ptr().write_bytes(0, layout.size());
        }
        Ok(Self {
            pointer,
            layout,
            bytes_offset,
            len: 0,
            count: 0,
            descriptor_capacity: request.frames as usize,
            wire_capacity: request.wire_max,
            permit,
        })
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn bytes(&self) -> &[u8] {
        unsafe {
            std::slice::from_raw_parts(self.pointer.as_ptr().add(self.bytes_offset), self.len)
        }
    }
    pub fn descriptors(&self) -> &[FieldDescriptor] {
        unsafe { std::slice::from_raw_parts(self.pointer.as_ptr().cast(), self.count) }
    }
    fn buffer(&mut self) -> &mut [u8] {
        unsafe {
            std::slice::from_raw_parts_mut(
                self.pointer.as_ptr().add(self.bytes_offset),
                self.layout.size() - self.bytes_offset,
            )
        }
    }
    fn descriptor(&mut self, descriptor: FieldDescriptor) {
        unsafe {
            self.pointer
                .as_ptr()
                .cast::<FieldDescriptor>()
                .add(self.count)
                .write(descriptor);
        }
        self.count += 1;
    }
    pub fn admitted(&self) -> Amount {
        self.permit.amount()
    }
}

impl Drop for RawStorage<'_> {
    fn drop(&mut self) {
        unsafe {
            dealloc(self.pointer.as_ptr(), self.layout);
        }
    }
}

pub trait PipeReader {
    fn read(&mut self, bytes: &mut [u8]) -> Result<usize, Failure>;
}
fn exact(reader: &mut impl PipeReader, mut out: &mut [u8]) -> Result<(), Failure> {
    while !out.is_empty() {
        let n = reader.read(out)?;
        if n == 0 {
            return Err(Failure::Truncated);
        }
        if n > out.len() {
            return Err(Failure::Protocol);
        }
        out = &mut out[n..];
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct Footer {
    pub outcome: u16,
    pub flags: u32,
    pub revision: u64,
    pub source: u32,
    pub code: u16,
    pub phase: u8,
    pub reason: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BorrowedValue<'a> {
    Null,
    Text(&'a str),
    Port(u16),
    Position(i64),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BorrowedField<'a> {
    pub tag: u16,
    pub ordinal: u32,
    pub value: BorrowedValue<'a>,
}

/// stdout 只使用固定 header/scratch；payload 直接写入预先准入的 raw backing。
pub fn read_stream(
    reader: &mut impl PipeReader,
    request: &Request,
    raw: &mut RawStorage<'_>,
) -> Result<Footer, Failure> {
    if !request.canonical()
        || raw.descriptor_capacity != request.frames as usize
        || raw.wire_capacity != request.wire_max
        || raw.count != 0
        || raw.len != 0
    {
        return Err(Failure::Protocol);
    }
    let mut header = [0u8; FOOTER_LEN];
    exact(reader, &mut header[..8])?;
    let mut preamble = false;
    let mut group_reference = None;
    let mut revision = 0;
    let mut source = 0;
    let mut expected_fields = 0;
    let mut text_bound = 0;
    let mut text = 0usize;
    let mut integers = 0usize;
    if &header[..8] == PREAMBLE_MAGIC {
        exact(reader, &mut header[8..64])?;
        if u16_at(&header, 8) != 2
            || u16_at(&header, 10) & !7 != 0
            || u32_at(&header, 12) != 64
            || header[16..32] != request.nonce
        {
            return Err(Failure::Protocol);
        }
        let fact = GroupReferenceFact::decode(u16_at(&header, 10))?;
        if !request.connected && fact != GroupReferenceFact::NoReference {
            return Err(Failure::Protocol);
        }
        group_reference = Some(fact);
        revision = u64_at(&header, 32);
        source = u32_at(&header, 40);
        expected_fields = u32_at(&header, 44);
        text_bound = usize_at(&header, 48)?;
        let w = usize_at(&header, 56)?;
        let minimum = if request.connected { 22 } else { 6 };
        let wire_upper = add(
            add(
                add(
                    PREAMBLE_LEN,
                    mul(FIELD_LEN, expected_fields as usize).map_err(|_| Failure::Protocol)?,
                )
                .map_err(|_| Failure::Protocol)?,
                text_bound,
            )
            .map_err(|_| Failure::Protocol)?,
            if request.connected {
                FOOTER_LEN + 10
            } else {
                FOOTER_LEN
            },
        )
        .map_err(|_| Failure::Protocol)?;
        let explicit = u16_at(&request.bytes, 10) & 2 != 0;
        if !(minimum..=request.frames).contains(&expected_fields)
            || (!request.connected && expected_fields != 6)
            || text_bound > request.total_max
            || !(1..=3).contains(&source)
            || (source == 1) != explicit
            || (!request.connected && !explicit && source != 3)
            || w != wire_upper
            || w > request.wire_max
        {
            return Err(Failure::Protocol);
        }
        raw.buffer()[..64].copy_from_slice(&header[..64]);
        raw.len = 64;
        preamble = true;
        exact(reader, &mut header[..8])?;
    }
    loop {
        if &header[..8] == FOOTER_MAGIC {
            break;
        }
        if !preamble || &header[..8] != FIELD_MAGIC || raw.count >= expected_fields as usize {
            return Err(Failure::Protocol);
        }
        exact(reader, &mut header[8..24])?;
        let tag = u16_at(&header, 8);
        let kind = header[10];
        let ordinal = u32_at(&header, 12);
        let len = usize_at(&header, 16)?;
        let expected_tag = if request.connected {
            (raw.count + 1).min(23) as u16
        } else {
            (raw.count + 17) as u16
        };
        let nullable = matches!(tag, 2 | 9 | 10 | 12 | 15);
        let expected_kind = if tag == 5 {
            2
        } else if tag == 14 {
            3
        } else {
            1
        };
        if tag != expected_tag
            || header[11] != 0
            || ordinal
                != if tag == 23 {
                    (raw.count - 21) as u32
                } else {
                    0
                }
            || !(kind == expected_kind || (nullable && kind == 0))
            || (kind == 0 && len != 0)
            || (kind == 2 && len != 2)
            || (kind == 3 && len != 8)
            || (tag == 2
                && (kind == 0) != (group_reference == Some(GroupReferenceFact::NoReference)))
        {
            return Err(Failure::Protocol);
        }
        if kind == 1 {
            text = text.checked_add(len).ok_or(Failure::Protocol)?;
            if len > request.field_max || text > text_bound {
                return Err(Failure::Protocol);
            }
        } else {
            integers = integers.checked_add(len).ok_or(Failure::Protocol)?;
        }
        let end = raw
            .len
            .checked_add(24)
            .and_then(|n| n.checked_add(len))
            .ok_or(Failure::Protocol)?;
        if end
            > request
                .wire_max
                .checked_sub(FOOTER_LEN)
                .ok_or(Failure::Protocol)?
        {
            return Err(Failure::Protocol);
        }
        let at = raw.len;
        raw.buffer()[at..at + 24].copy_from_slice(&header[..24]);
        exact(reader, &mut raw.buffer()[at + 24..end])?;
        if kind == 1 && std::str::from_utf8(&raw.buffer()[at + 24..end]).is_err() {
            return Err(Failure::Protocol);
        }
        raw.descriptor(FieldDescriptor {
            offset: at + 24,
            len,
            tag,
            kind,
            ordinal,
        });
        raw.len = end;
        exact(reader, &mut header[..8])?;
    }
    exact(reader, &mut header[8..128])?;
    let flags = u32_at(&header, 12);
    let outcome = u16_at(&header, 10);
    let fact_bits = (flags & GROUP_FACT_MASK) >> GROUP_FACT_SHIFT;
    let footer_fact = if preamble {
        Some(GroupReferenceFact::decode(fact_bits as u16)?)
    } else {
        if fact_bits != 0 {
            return Err(Failure::Protocol);
        }
        None
    };
    if u16_at(&header, 8) != 2
        || outcome > 10
        || flags & !(255 | GROUP_FACT_MASK) != 0
        || flags & 4 == 0
        || header[16..32] != request.nonce
        || (flags & 1 != 0) != preamble
        || footer_fact != group_reference
        || u32_at(&header, 44) as usize != raw.count
        || usize_at(&header, 48)? != text
        || usize_at(&header, 56)? != integers
        || usize_at(&header, 64)? != raw.len
        || (preamble
            && (flags & 2 == 0 || u64_at(&header, 32) != revision || u32_at(&header, 40) != source))
        || (flags & 2 == 0 && (u64_at(&header, 32) != 0 || u32_at(&header, 40) != 0))
    {
        return Err(Failure::Protocol);
    }
    if outcome == 0
        && (!preamble
            || raw.count != expected_fields as usize
            || flags & !GROUP_FACT_MASK != SUCCESS_FLAGS
            || integers != if request.connected { 10 } else { 0 }
            || u64_at(&header, 120) != 0
            || header[72..120].iter().any(|b| *b != 0))
    {
        return Err(Failure::Protocol);
    }
    let mut extra = [0u8; 1];
    if reader.read(&mut extra)? != 0 {
        return Err(Failure::Protocol);
    }
    let at = raw.len;
    let end = at.checked_add(FOOTER_LEN).ok_or(Failure::Protocol)?;
    if end > request.wire_max {
        return Err(Failure::Protocol);
    }
    raw.buffer()[at..end].copy_from_slice(&header);
    raw.len = end;
    Ok(Footer {
        outcome,
        flags,
        revision: u64_at(&header, 32),
        source: u32_at(&header, 40),
        code: u16_at(&header, 72),
        phase: header[74],
        reason: header[75],
    })
}
