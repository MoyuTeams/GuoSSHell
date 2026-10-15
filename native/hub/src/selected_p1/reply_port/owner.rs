//! 真实 W1-L header/body/tail；peer 只移交一个 strong ref，不另分配 Box/Vec。

use crate::selected_p1::font_source::source::{MetadataBody, SourceOwner};
use crate::selected_p1::model::layout::NumericReject;
use crate::selected_p1::resource::ledger::{Amount, Grant, ResourceClass};
use crate::selected_p1::resource::owner::{OwnerLayout, RcMemberLayout, SharedOwner};

use super::frame::FrameSpans;

/// 只用于 VM peer 保留的实际资源 root；不宣称任意 T 的 Drop 安全。
///
/// # Safety
/// 所有可达析构只销毁已建模的 bytes/metadata/shared resource 与真实 grant；
/// 不 unwind、不调用 actor/runtime/GUI/Dart，不执行 opaque 第三方 destructor。
/// 每个被持有的物理 backing 都已在首次创建前准入；共享根须保持 no-Weak、
/// 最后 inner/body/header 原 Layout dealloc 后返额，且可在任意 VM 线程析构。
pub unsafe trait NoUnwindRoot: Send + Sync {}

// SAFETY: 空 root 没有析构/分配，用于不再保留源的独立 owned 回复。
unsafe impl NoUnwindRoot for () {}

// SAFETY: SourceBody 仅含 ImportedFont(String/String)、SourceMetadata 的数字/枚举/
// Option/String/ImportedFont 与 tail spans；均无自定义/opaque destructor。
// SourceOwner 只包装 W1-L SharedOwner；Grant::drop 只执行账本 checked 守恒释放，
// 不依赖 runtime 或当前 epoch。字体/path 在同一 tail，Strings 已在 Source 准入。
unsafe impl NoUnwindRoot for SourceOwner<'_> {}

// SAFETY: MetadataBody 只含上述有限 ImportedFont/SourceMetadata；实际 SharedOwner
// 负责 inner String free、original backing dealloc、最后 return，未泛化到其他 T。
unsafe impl NoUnwindRoot for SharedOwner<'_, MetadataBody> {}

/// 父的完整 SourceRecord 可通过其受限新类型实现 NoUnwindRoot；不复制其 opaque 图。
#[repr(C)]
pub struct OwnedReplyBody<R: NoUnwindRoot> {
    pub root: R,
    pub epoch: u64,
    pub spans: FrameSpans,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReplyLayout {
    pub owner: OwnerLayout,
    pub spans: FrameSpans,
}

impl ReplyLayout {
    pub fn actual<R: NoUnwindRoot>(spans: FrameSpans) -> Result<Self, NumericReject> {
        // 公共 spans 可由 caller 构造，必须验证为单个连续完整 frame。
        let checked = FrameSpans::checked(
            spans.header.len as usize,
            spans.metadata.len as usize,
            spans.binary.len as usize,
        )?;
        if checked != spans {
            return Err(NumericReject::ConstructionFailure);
        }
        Ok(Self {
            owner: OwnerLayout::actual::<OwnedReplyBody<R>>(spans.total as usize)?,
            spans,
        })
    }

    pub fn amount(self) -> Amount {
        Amount {
            rust_bytes: self.owner.backing.size(),
            c_bytes: 0,
            slots: 1,
        }
    }
}

pub struct UnpostedReply<'root, R: NoUnwindRoot> {
    owner: SharedOwner<'root, OwnedReplyBody<R>>,
}

impl<'root, R: NoUnwindRoot> UnpostedReply<'root, R> {
    pub(super) fn try_init(
        grant: Grant<'root>,
        root: R,
        layout: ReplyLayout,
        fill: impl FnOnce(&mut [u8]) -> Result<(), NumericReject>,
    ) -> Result<Self, NumericReject> {
        if grant.class() != ResourceClass::Reply
            || grant.amount().slots != 1
            || grant.amount().c_bytes != 0
        {
            return Err(NumericReject::ConstructionFailure);
        }
        let epoch = grant.epoch();
        let owner = SharedOwner::try_init(grant, layout.spans.total as usize, 0, |tail| {
            fill(tail)?;
            Ok(OwnedReplyBody {
                root,
                epoch,
                spans: layout.spans,
            })
        })?;
        Ok(Self { owner })
    }

    pub fn bytes(&self) -> &[u8] {
        self.owner.payload()
    }
    pub fn spans(&self) -> FrameSpans {
        self.owner.value().spans
    }
    pub fn epoch(&self) -> u64 {
        self.owner.value().epoch
    }
    pub fn admitted(&self) -> Amount {
        self.owner.admitted()
    }

    /// 消费唯一 reply holder；返回原 allocation 的 writable tail，不从 shared slice 派生。
    pub(super) fn into_raw_parts(self) -> (*mut RcMemberLayout<Grant<'root>>, *mut u8, isize) {
        // 只复制 body 标量；total 在 FrameSpans/同 T 的构造 Layout 中已校验。
        let total = self.owner.value().spans.total as usize;
        let Ok(layout) = OwnerLayout::actual::<OwnedReplyBody<R>>(total) else {
            std::process::abort();
        };
        // owner 私有且只由 try_init 的 fresh SharedOwner(refs=1) 构造；没有 clone、
        // shared owner/raw getter 或从既有 holder 包装的入口。bytes() 借用依赖 self，
        // 安全调用方不能把它跨过本次消费；root 的其他 holder 只共享另一个源 allocation。
        let peer = self.owner.into_raw();
        // SAFETY: W1-L header 直接来自 alloc(backing).cast，不是从引用重建的指针。
        // 同 OwnedReplyBody<R>/total 的 actual Layout 给出原 tail offset；完整 tail
        // 在该 allocation 内、已初始化且非空。此前借用已结束，唯一 strong ref 原样
        // 移交 peer；不读私有 header/body 布局、不固定 prefix、不调整 refcount。
        let data = unsafe { peer.cast::<u8>().add(layout.payload_offset) };
        (peer, data, total as isize)
    }
}

#[cfg(test)]
mod tests {
    use super::{ReplyLayout, UnpostedReply};
    use crate::audit::AuditScope;
    use crate::selected_p1::model::layout::NumericReject;
    use crate::selected_p1::resource::ledger::{
        Admission, BYTE_LIMIT, CLASS_COUNT, ProcessLedger, ResourceClass,
    };

    #[test]
    fn reply_port_fill_error_frees_exact_backing_before_grant_return() -> Result<(), NumericReject>
    {
        let ledger = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
        let layout = ReplyLayout::actual::<()>(super::FrameSpans::checked(8, 3, 0)?)?;
        let grant = ledger.try_reserve(
            layout.amount(),
            ResourceClass::Reply,
            Admission {
                epoch: 1,
                source: None,
            },
        )?;
        let scope = AuditScope::start(&ledger);
        let result = UnpostedReply::try_init(grant, (), layout, |tail| {
            tail[..8].copy_from_slice(b"header\0\0");
            Err(NumericReject::ConstructionFailure)
        });
        assert_eq!(result.err(), Some(NumericReject::ConstructionFailure));
        let report = scope.finish();
        assert!(report.symmetric_single_backing());
        report.assert_balanced_and_pregranted();
        assert_eq!(ledger.snapshot().rust_bytes, 0);
        Ok(())
    }
}
