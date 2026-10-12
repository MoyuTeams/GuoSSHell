//! 证书前件与 actual target 布局。FoundationReady 不授权任何生产 P1 路径。

use std::alloc::Layout;

use super::layout::{
    NodeLayouts, NumericReject, ProductLayouts, PublicLayouts, btree_nodes, struct_upper,
};

pub const GNU_COMPILER: &str = "48a229ceaefd4985c50990b14116b6d856af0985";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BuildFingerprint {
    pub compiler: &'static str,
    pub target: &'static str,
    pub serde: &'static str,
    pub serde_core: &'static str,
    pub serde_derive: &'static str,
    pub serde_json: &'static str,
    pub secrecy: &'static str,
    pub zeroize: &'static str,
    pub core_revision: &'static str,
    pub serde_feature_bits: u32,
    pub serde_core_feature_bits: u32,
    pub serde_derive_feature_bits: u32,
    pub json_feature_bits: u32,
    pub secrecy_feature_bits: u32,
    pub zeroize_feature_bits: u32,
    pub layout_flags: &'static str,
    pub cfg_bits: u32,
}

/// features/cfg 必须由父的实际构建图核对；此常量不是 runtime SDK 检测。
pub const GNU_1981: BuildFingerprint = BuildFingerprint {
    compiler: GNU_COMPILER,
    target: "x86_64-pc-windows-gnu",
    serde: "1.0.229",
    serde_core: "1.0.229",
    serde_derive: "1.0.229",
    serde_json: "1.0.151",
    secrecy: "0.10.3",
    zeroize: "1.9.0",
    core_revision: "b72a83bf337c959b3e40c6ef5a255413fc09c0f9",
    // 各组按下列实际 feature 列表的顺序逐 bit 编码；未列出的 feature 不受此源模型支持。
    serde_feature_bits: 63,       // alloc/default/derive/rc/serde_derive/std。
    serde_core_feature_bits: 15,  // alloc/rc/result/std。
    serde_derive_feature_bits: 1, // default。
    json_feature_bits: 3,         // default/std。
    secrecy_feature_bits: 0,
    zeroize_feature_bits: 3, // alloc/default。
    layout_flags: "",        // 本 foundation 实际构建无额外布局 flags。
    cfg_bits: 1,             // little endian、64-bit GNU Windows、panic=unwind。
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ProofGate {
    BuildFingerprint = 1,
    ActualPublicLayouts = 2,
    FinalSchemas = 3,
    DecodeClosure = 4,
    SourceClosure = 5,
    ControlClosure = 6,
    AuthOperationRoot = 7,
    ReplyVm = 8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CertificateState {
    FoundationReady,
    Pending(ProofGate),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PrivateBounds {
    pub content: Layout,
    pub content_pair: Layout,
    pub flatten_entry: Layout,
    pub error_code: Layout,
    pub error_impl_actual: Layout,
    pub error_impl_upper: Layout,
    pub string_value_nodes: NodeLayouts,
    pub string_content_nodes: NodeLayouts,
    pub string_set_nodes: NodeLayouts,
}

/// private enum 用 exact compiler 的真实 monomorphization 诊断；struct 用源码字段排列互核。
/// Content/Pair/Flatten 来自已交付 note 的真实 serde_core 诊断，不复制 private struct。
fn gnu_private(public: PublicLayouts) -> Result<PrivateBounds, NumericReject> {
    let l = |size, align| {
        Layout::from_size_align(size, align).map_err(|_| NumericReject::InvalidLayout)
    };
    let content = l(32, 8)?;
    // serde_json 1.0.151 的 ErrorCode 包含 Io(io::Error)；真实诊断 24/8，ErrorImpl 40/8。
    let error_code = l(24, 8)?;
    let error_impl_actual = l(40, 8)?;
    let error_impl_upper = struct_upper(
        [error_code, Layout::new::<usize>(), Layout::new::<usize>()],
        1,
    )?;
    if error_impl_actual.size() > error_impl_upper.size() {
        return Err(NumericReject::UnsupportedEnvelope);
    }
    Ok(PrivateBounds {
        content,
        content_pair: l(64, 8)?,
        flatten_entry: l(64, 8)?,
        error_code,
        error_impl_actual,
        error_impl_upper,
        string_value_nodes: btree_nodes(public.string, public.value, 1)?,
        string_content_nodes: btree_nodes(public.string, content, 1)?,
        string_set_nodes: btree_nodes(public.string, Layout::new::<()>(), 1)?,
    })
}

pub trait ActualLayoutProvider {
    fn product_layouts(&self) -> Result<ProductLayouts, ProofGate>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TargetLayouts {
    pub build: BuildFingerprint,
    pub public: PublicLayouts,
    private: Option<PrivateBounds>,
    state: CertificateState,
}

impl TargetLayouts {
    pub fn foundation(actual_build: BuildFingerprint) -> Result<Self, NumericReject> {
        let public = PublicLayouts::actual();
        let state = if actual_build != GNU_1981 {
            CertificateState::Pending(ProofGate::BuildFingerprint)
        } else if !cfg!(all(
            target_os = "windows",
            target_env = "gnu",
            target_pointer_width = "64",
            target_endian = "little",
            panic = "unwind"
        )) || (public.string.size(), public.string.align()) != (24, 8)
            || (public.path.size(), public.path.align()) != (32, 8)
            || (public.value.size(), public.value.align()) != (32, 8)
        {
            CertificateState::Pending(ProofGate::ActualPublicLayouts)
        } else {
            CertificateState::FoundationReady
        };
        let private = if state == CertificateState::FoundationReady {
            Some(gnu_private(public)?)
        } else {
            None
        };
        Ok(Self {
            build: actual_build,
            public,
            private,
            state,
        })
    }

    pub fn state(&self) -> CertificateState {
        self.state
    }

    pub fn private_bounds(&self) -> Result<PrivateBounds, NumericReject> {
        self.private.ok_or(NumericReject::UnsupportedEnvelope)
    }

    /// 读取最终 actual schema 不等于其 decode/source/operation/VM 闭包已获证。
    pub fn product_state(
        &self,
        provider: &impl ActualLayoutProvider,
        closure: ProofGate,
    ) -> CertificateState {
        if self.state != CertificateState::FoundationReady {
            return self.state;
        }
        match provider.product_layouts() {
            Err(gate) => CertificateState::Pending(gate),
            Ok(_) => CertificateState::Pending(closure),
        }
    }
}

#[cfg(test)]
#[path = "certificate_tests.rs"]
mod tests;
