//! 父 wire 的借用 header、有限 DTO 编码与 binary 共用一份 tail，不生成中间 Vec。

use crate::selected_p1::model::layout::{NumericReject, add};
use crate::selected_p1::resource::ledger::{Admission, Grant, ProcessLedger, ResourceClass};

use super::owner::{NoUnwindRoot, ReplyLayout, UnpostedReply};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(C)]
pub struct Span {
    pub offset: u32,
    pub len: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct FrameSpans {
    pub header: Span,
    pub metadata: Span,
    pub binary: Span,
    pub total: u32,
}

impl FrameSpans {
    pub fn checked(header: usize, metadata: usize, binary: usize) -> Result<Self, NumericReject> {
        if header == 0 {
            return Err(NumericReject::ConstructionFailure);
        }
        let metadata_end = add(header, metadata)?;
        let total = add(metadata_end, binary)?;
        isize::try_from(total).map_err(|_| NumericReject::InvalidLayout)?;
        let narrow = |n| u32::try_from(n).map_err(|_| NumericReject::Overflow);
        Ok(Self {
            header: Span {
                offset: 0,
                len: narrow(header)?,
            },
            metadata: Span {
                offset: narrow(header)?,
                len: narrow(metadata)?,
            },
            binary: Span {
                offset: narrow(metadata_end)?,
                len: narrow(binary)?,
            },
            total: narrow(total)?,
        })
    }
}

/// writer 只借用栈或已预约的 tail；允许任意 UTF-8/NUL bytes，不走 CString。
pub trait ByteSink {
    fn write(&mut self, bytes: &[u8]) -> Result<(), NumericReject>;
}

/// 父必须为真实有限 DTO 的具体编码路径提供源模型，不能为任意 serializer 泛化实现。
///
/// # Safety
/// 两次 encode 只借用同一不可变 DTO，成功时输出相同字节长度；count 和 fill 均不分配、
/// 不 clone owned 字段、不调用 String/CString 构造或不受证的第三方代码。
/// DTO 本身及它借用的投影、strings/bindings、上游暂存均已在首次构造前取得真实 grant。
/// 格式/选项、最大字段集合与字节计算必须与父的实际 Dart DTO 解码器一致。
/// 只对 writer 写最终编码；超限或编码失败用 NumericReject，不能先 serialize 再贴额。
pub unsafe trait FiniteMetadata {
    fn encode(&self, writer: &mut dyn ByteSink) -> Result<(), NumericReject>;
}

struct Counter(usize);

impl ByteSink for Counter {
    fn write(&mut self, bytes: &[u8]) -> Result<(), NumericReject> {
        self.0 = add(self.0, bytes.len())?;
        u32::try_from(self.0).map_err(|_| NumericReject::Overflow)?;
        Ok(())
    }
}

struct TailWriter<'a> {
    tail: &'a mut [u8],
    used: usize,
}

impl ByteSink for TailWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> Result<(), NumericReject> {
        let end = add(self.used, bytes.len())?;
        let destination = self
            .tail
            .get_mut(self.used..end)
            .ok_or(NumericReject::ConstructionFailure)?;
        destination.copy_from_slice(bytes);
        self.used = end;
        Ok(())
    }
}

pub struct MetadataHeaderBuilder<'a, M> {
    header: &'a [u8],
    metadata: &'a M,
    binary: &'a [u8],
}

impl<'a, M: FiniteMetadata> MetadataHeaderBuilder<'a, M> {
    /// header 来自父的实际私有 wire，binary 可为空；不在本单元另造 DTO/wire schema。
    pub fn new(header: &'a [u8], metadata: &'a M, binary: &'a [u8]) -> Self {
        Self {
            header,
            metadata,
            binary,
        }
    }

    pub fn layout<R: NoUnwindRoot>(&self) -> Result<ReplyLayout, NumericReject> {
        let mut count = Counter(0);
        self.metadata.encode(&mut count)?;
        ReplyLayout::actual::<R>(FrameSpans::checked(
            self.header.len(),
            count.0,
            self.binary.len(),
        )?)
    }

    pub fn try_build<'root, R: NoUnwindRoot>(
        &self,
        ledger: &'root ProcessLedger,
        admission: Admission,
        root: R,
    ) -> Result<UnpostedReply<'root, R>, NumericReject> {
        let layout = self.layout::<R>()?;
        let grant = ledger.try_reserve(layout.amount(), ResourceClass::Reply, admission)?;
        self.fill(grant, root, layout)
    }

    /// 消费父在 admitted Prepare 的 terminal/result lane 中预先保留的真实 Grant。
    /// 不能从 owner 已绑定的额度收紧或拆分；本方法不生成额外 error peer。
    pub fn try_build_reserved<'root, R: NoUnwindRoot>(
        &self,
        grant: Grant<'root>,
        root: R,
    ) -> Result<UnpostedReply<'root, R>, NumericReject> {
        let layout = self.layout::<R>()?;
        self.fill(grant, root, layout)
    }

    fn fill<'root, R: NoUnwindRoot>(
        &self,
        grant: Grant<'root>,
        root: R,
        layout: ReplyLayout,
    ) -> Result<UnpostedReply<'root, R>, NumericReject> {
        UnpostedReply::try_init(grant, root, layout, |tail| {
            let header_end = layout.spans.metadata.offset as usize;
            let metadata_end = layout.spans.binary.offset as usize;
            tail[..header_end].copy_from_slice(self.header);
            let mut writer = TailWriter {
                tail: &mut tail[header_end..metadata_end],
                used: 0,
            };
            self.metadata.encode(&mut writer)?;
            if writer.used != writer.tail.len() {
                return Err(NumericReject::ConstructionFailure);
            }
            tail[metadata_end..].copy_from_slice(self.binary);
            Ok(())
        })
    }
}
