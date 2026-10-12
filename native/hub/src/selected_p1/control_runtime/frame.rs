//! 私有控制帧；只借用可信 FFI 映射内存，不保存调用者指针。

use crate::selected_p1::resource::ledger::SourceToken;

pub const MAGIC: &[u8; 8] = b"GUOP1CT2";
pub const HEADER: usize = 240;
pub const FIELDS: usize = 8;
pub const RESULT_WORDS: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Status {
    Admitted = 0,
    Invalid = 1,
    Stale = 2,
    Duplicate = 3,
    Full = 4,
    Budget = 5,
    Allocation = 6,
    Cancelled = 7,
    AlreadyStarted = 8,
    NotPrepared = 9,
    Identity = 10,
    Geometry = 11,
    Unavailable = 12,
    ShutdownFailure = 13,
    Expired = 14,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum Command {
    Prepare = 1,
    Start = 2,
    Cancel = 3,
    RegisterReplyPort = 4,
    CloseReplyPort = 5,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(C)]
pub struct Geometry {
    pub cols: u16,
    pub rows: u16,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub dpi: u32,
}

impl Geometry {
    pub fn valid(self) -> bool {
        (2..=999).contains(&self.cols)
            && (2..=999).contains(&self.rows)
            && (u32::from(self.cols)..=65535).contains(&self.pixel_width)
            && (u32::from(self.rows)..=65535).contains(&self.pixel_height)
            && (48..=960).contains(&self.dpi)
    }
}

#[derive(Clone, Copy)]
pub struct ControlFrame<'a> {
    pub(super) command: Command,
    pub(super) epoch: u64,
    pub(super) owner: u64,
    pub(super) source: SourceToken,
    pub(super) session: u32,
    pub(super) request: u32,
    pub(super) prepare: u32,
    pub(super) entry: u64,
    pub(super) port: i64,
    pub(super) target: u32,
    pub(super) ssh_port: u16,
    pub(super) geometry: Geometry,
    pub(super) fields: [&'a str; FIELDS],
    pub(super) secret: &'a str,
}

/// 父 FFI 薄壳只交此 checked borrowed 值；不暴露构造 key 或保留输入指针的入口。
pub type BorrowEntry<'a> = ControlFrame<'a>;

fn n<const N: usize>(bytes: &[u8], at: usize) -> Result<[u8; N], Status> {
    bytes
        .get(at..at + N)
        .ok_or(Status::Invalid)?
        .try_into()
        .map_err(|_| Status::Invalid)
}

impl<'a> ControlFrame<'a> {
    pub fn parse(bytes: &'a [u8], secret: &'a [u8]) -> Result<Self, Status> {
        let frame_end = (bytes.as_ptr() as usize)
            .checked_add(bytes.len())
            .ok_or(Status::Invalid)?;
        let secret_end = (secret.as_ptr() as usize)
            .checked_add(secret.len())
            .ok_or(Status::Invalid)?;
        if !secret.is_empty()
            && (secret.as_ptr() as usize) < frame_end
            && (bytes.as_ptr() as usize) < secret_end
        {
            return Err(Status::Invalid);
        }
        if bytes.len() < HEADER
            || bytes.get(..8) != Some(MAGIC)
            || u16::from_le_bytes(n(bytes, 8)?) != 2
        {
            return Err(Status::Invalid);
        }
        let command = match u16::from_le_bytes(n(bytes, 10)?) {
            1 => Command::Prepare,
            2 => Command::Start,
            3 => Command::Cancel,
            4 => Command::RegisterReplyPort,
            5 => Command::CloseReplyPort,
            _ => return Err(Status::Invalid),
        };
        if bytes[12..16]
            .iter()
            .chain(bytes[98..112].iter())
            .any(|b| *b != 0)
        {
            return Err(Status::Invalid);
        }
        let count = match command {
            Command::Prepare => 5,
            Command::Start => 2,
            Command::Cancel => 1,
            _ => 0,
        };
        let mut fields = [""; FIELDS];
        let mut end = HEADER;
        for (i, field) in fields.iter_mut().enumerate() {
            let offset = usize::try_from(u64::from_le_bytes(n(bytes, 112 + i * 16)?))
                .map_err(|_| Status::Invalid)?;
            let len = usize::try_from(u64::from_le_bytes(n(bytes, 120 + i * 16)?))
                .map_err(|_| Status::Invalid)?;
            if i >= count {
                if offset != 0 || len != 0 {
                    return Err(Status::Invalid);
                }
                continue;
            }
            if offset != end {
                return Err(Status::Invalid);
            }
            end = offset.checked_add(len).ok_or(Status::Invalid)?;
            *field = std::str::from_utf8(bytes.get(offset..end).ok_or(Status::Invalid)?)
                .map_err(|_| Status::Invalid)?;
        }
        if end != bytes.len() || (command != Command::Start && !secret.is_empty()) {
            return Err(Status::Invalid);
        }
        let epoch = u64::from_le_bytes(n(bytes, 16)?);
        let result = Self {
            command,
            epoch,
            owner: u64::from_le_bytes(n(bytes, 24)?),
            source: SourceToken {
                epoch,
                index: u32::from_le_bytes(n(bytes, 32)?),
                generation: u64::from_le_bytes(n(bytes, 40)?),
            },
            session: u32::from_le_bytes(n(bytes, 36)?),
            request: u32::from_le_bytes(n(bytes, 48)?),
            prepare: u32::from_le_bytes(n(bytes, 52)?),
            entry: u64::from_le_bytes(n(bytes, 56)?),
            port: i64::from_le_bytes(n(bytes, 64)?),
            target: u32::from_le_bytes(n(bytes, 72)?),
            ssh_port: u16::from_le_bytes(n(bytes, 76)?),
            geometry: Geometry {
                cols: u16::from_le_bytes(n(bytes, 78)?),
                rows: u16::from_le_bytes(n(bytes, 80)?),
                pixel_width: u32::from_le_bytes(n(bytes, 84)?),
                pixel_height: u32::from_le_bytes(n(bytes, 88)?),
                dpi: u32::from_le_bytes(n(bytes, 92)?),
            },
            fields,
            secret: std::str::from_utf8(secret).map_err(|_| Status::Invalid)?,
        };
        if bytes[82..84] != [0, 0] || bytes[96..98] != [0, 0] || epoch == 0 {
            return Err(Status::Invalid);
        }
        let controls_zero =
            result.target == 0 && result.ssh_port == 0 && result.geometry == Geometry::default();
        match command {
            Command::Prepare
                if result.request != 0
                    && result.prepare == 0
                    && result.entry == 0
                    && result.port == 0
                    && result.geometry == Geometry::default()
                    && result.target <= 2 => {}
            Command::Start
                if result.request != 0
                    && result.prepare != 0
                    && result.request != result.prepare
                    && result.entry != 0
                    && result.port == 0
                    && result.target == 0
                    && result.ssh_port == 0 => {}
            Command::Cancel
                if result.request != 0
                    && result.prepare != 0
                    && result.entry != 0
                    && result.port == 0
                    && controls_zero => {}
            Command::RegisterReplyPort | Command::CloseReplyPort
                if result.port > 0
                    && result.entry == 0
                    && result.request == 0
                    && result.prepare == 0
                    && result.session == 0
                    && result.source.index == 0
                    && result.source.generation == 0
                    && controls_zero => {}
            _ => return Err(Status::Invalid),
        }
        Ok(result)
    }
}

/// # Safety
/// 非空输入及结果区必须是调用期间有效、固定映射；输入不可被并发修改。
/// 此函数只验证范围、对齐和不重叠，不声称能识别任意不可映射地址。
pub unsafe fn with_ffi_frame(
    bytes: *const u8,
    len: usize,
    secret: *const u8,
    secret_len: usize,
    output: *mut u64,
    words: usize,
    consume: impl FnOnce(ControlFrame<'_>) -> [u64; RESULT_WORDS],
) -> Status {
    fn range(pointer: usize, len: usize) -> Option<(usize, usize)> {
        if len > isize::MAX as usize || (len != 0 && pointer == 0) {
            return None;
        }
        Some((pointer, pointer.checked_add(len)?))
    }
    if words != RESULT_WORDS
        || output.is_null()
        || !(output as usize).is_multiple_of(std::mem::align_of::<u64>())
    {
        return Status::Invalid;
    }
    let Some(o) = range(output as usize, RESULT_WORDS * 8) else {
        return Status::Invalid;
    };
    let Some(b) = range(bytes as usize, len) else {
        return Status::Invalid;
    };
    let Some(s) = range(secret as usize, secret_len) else {
        return Status::Invalid;
    };
    if [b, s].iter().any(|r| r.0 != r.1 && r.0 < o.1 && o.0 < r.1) {
        return Status::Invalid;
    }
    if b.0 != b.1 && s.0 != s.1 && b.0 < s.1 && s.0 < b.1 {
        return Status::Invalid;
    }
    // SAFETY: 有效映射是私有调用契约；空输入从不解引用 NULL。
    let input = if len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(bytes, len) }
    };
    let password = if secret_len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(secret, secret_len) }
    };
    let result = match ControlFrame::parse(input, password) {
        Ok(frame) => consume(frame),
        Err(status) => [status as u64, 0, 0, 0, 0, 0, 0, 0],
    };
    unsafe {
        std::ptr::copy_nonoverlapping(result.as_ptr(), output, RESULT_WORDS);
    }
    match result[0] {
        0 => Status::Admitted,
        1 => Status::Invalid,
        2 => Status::Stale,
        3 => Status::Duplicate,
        4 => Status::Full,
        5 => Status::Budget,
        6 => Status::Allocation,
        7 => Status::Cancelled,
        8 => Status::AlreadyStarted,
        9 => Status::NotPrepared,
        10 => Status::Identity,
        11 => Status::Geometry,
        13 => Status::ShutdownFailure,
        14 => Status::Expired,
        _ => Status::Unavailable,
    }
}
