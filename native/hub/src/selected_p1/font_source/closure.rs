//! ttf-parser 0.25.1 的私有九表 inspection 源闭包；不覆盖通用 Face 或完整 Source。
//! features 取 metadata.resolve.nodes 的实际列表，不能取 package.features 定义。

use crate::selected_p1::model::layout::NumericReject;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FontBuild {
    pub compiler: &'static str,
    pub target: &'static str,
    pub ttf_parser: &'static str,
    pub sha2: &'static str,
    /// apple-layout/default/glyph-names/opentype-layout/std/variable-fonts/gvar-alloc 按位。
    pub ttf_features: u32,
    /// alloc/default/oid 按位；固定输出入口不调用 alloc API。
    pub sha2_features: u32,
    pub layout_flags: &'static str,
}

/// 父构建入口核对 actual resolve/compiler/cfg 后传入；常量不自行发现 SDK。
pub const GNU_FONT_BUILD: FontBuild = FontBuild {
    compiler: "48a229ceaefd4985c50990b14116b6d856af0985",
    target: "x86_64-pc-windows-gnu",
    ttf_parser: "0.25.1",
    sha2: "0.11.0",
    ttf_features: 63,
    sha2_features: 7,
    layout_flags: "",
};

pub struct FontBuildProof(FontBuild);

impl FontBuildProof {
    pub fn verify(actual: FontBuild) -> Result<Self, NumericReject> {
        if actual != GNU_FONT_BUILD
            || !cfg!(all(
                target_os = "windows",
                target_env = "gnu",
                target_pointer_width = "64",
                target_endian = "little",
                panic = "unwind"
            ))
        {
            return Err(NumericReject::UnsupportedEnvelope);
        }
        Ok(Self(actual))
    }

    pub fn require(&self) -> Result<(), NumericReject> {
        if self.0 == GNU_FONT_BUILD {
            Ok(())
        } else {
            Err(NumericReject::UnsupportedEnvelope)
        }
    }
}

/// 只覆盖 inspect 的 size/suffix/signature、九表初始解析、名字与 SHA256。
/// 不签发 provider/open/path/LegacySlot/最后根/Start/VM 或其他 target 的 Source 证书。
pub struct FontInspectionProof(FontBuildProof);

impl FontInspectionProof {
    pub fn verify(actual: FontBuild) -> Result<Self, NumericReject> {
        FontBuildProof::verify(actual).map(Self)
    }

    pub fn inspect(
        &self,
        bytes: &[u8],
        file_name: &str,
    ) -> Result<super::inspect::CheckedFont, super::inspect::FontReject> {
        self.0.require()?;
        super::inspect::inspect(bytes, file_name)
    }
}

/// 生产调用的辅助 heap Layout 集合为空：标量、借用 slice/LazyArray 和固定栈数组。
/// 各窗口在 read 发生前证明，不按成功 slice 长度反推，也不作为额外输入 cap。
pub struct ParseCall {
    pub table: &'static str,
    pub source: &'static str,
    pub mechanism: &'static str,
}

pub const PARSE_CALLS: &[ParseCall] = &[
    ParseCall {
        table: "入口与 sfnt",
        source: "inspect.rs inspect/inspection_face; lib.rs:837 RawFace::parse; parser.rs:272 LazyArray16",
        mechanism: "size≤33554432、借用 ASCII suffix、00010000/true 排除 TTC；目录 read 为12+16*65535=1048572；0..len/get 最大 index65534，不调用 LazyArrayIter16::next",
    },
    ParseCall {
        table: "目录 window 与九表",
        source: "lib.rs:712 TableRecord::parse/:918 RawFaceTables/:1130 collect_tables（覆盖语义）; inspect.rs inspection_face",
        mechanism: "record 固定16 bytes；u32 offset/length 转 GNU usize，checked_add 后原 bytes.get；顺序最后 tag 覆盖 None/空，求和失败跳过；无排序、binary search 或 owned index",
    },
    ParseCall {
        table: "Face 构造",
        source: "lib.rs:1208 Face::from_raw_tables/:1230 parse_tables/:738 VarCoords::default",
        mechanism: "head→hhea→maxp；仅九表给 Some，其余分支不调用 parser；fvar 决定 min(axes.len(),64) 的默认零坐标长度，固定数组；glyf/cmap 判定和名字不触达其他 Face API",
    },
    ParseCall {
        table: "head/hhea/maxp",
        source: "tables/head.rs:32; tables/hhea.rs:21; tables/maxp.rs:17",
        mechanism: "窗口≤54/36/6；保 units-per-em/index format、version/NonZeroU16 与必需表错误顺序；标量 FromData/Stream read",
    },
    ParseCall {
        table: "loca→glyf 与 glyph 判定",
        source: "tables/loca.rs:25; tables/glyf.rs:590; lib.rs:1827 number_of_glyphs/:1367 tables",
        mechanism: "原 count+1（u16MAX 特例）、actual_total 的 u16 转换和 min；Short/Long read≤2*65535/4*65535=262140；glyf 初始 parse 只存借用，不 outline；已解析 NonZeroU16 与 Option 判定",
    },
    ParseCall {
        table: "cmap 初始 parse",
        source: "tables/cmap/mod.rs:275",
        mechanism: "4+8*65535=524284；EncodingRecord LazyArray；不调用 Subtables::get 或任意 format parser/getter",
    },
    ParseCall {
        table: "name 初始 parse 与 storage",
        source: "tables/name.rs:303; parser.rs Stream/read_array16/tail",
        mechanism: "v1 lang_tag_count.checked_mul(4)≤65532，read 终点≤8+65532+12*65535=851960；v0 更小；storage_offset≤65535，定位只推进到 max(当前offset,storage_offset)，tail 取 slice",
    },
    ParseCall {
        table: "名字消费",
        source: "lib.rs Face::names; tables/name.rs NamesIter::next/Names::get/NameRecord::parse/PlatformId::parse/Name::is_unicode",
        mechanism: "NamesIter 先 index<len 再递增，最大65535安全；record固定12 bytes，offset+length≤131070；坏 record 的 None 停止 find_map；全部标量和借用",
    },
    ParseCall {
        table: "fvar",
        source: "tables/fvar.rs:75; lib.rs Face::from_raw_tables/VarCoords",
        mechanism: "NonZeroU16；new_at 先检查 actual bounds；u16 axes offset+20*u16 count≤65535+20*65535=1376235；默认零坐标数组不分配",
    },
    ParseCall {
        table: "gvar",
        source: "tables/gvar.rs:1651; parser.rs Stream::new_at/read_array16",
        mechanism: "new_at 将 shared tuple offset 限制到实际 table≤33554432；shared_tuple_count.checked_mul(axis_count)仍为u16，额外F2DOT14≤131070，终点≤33685502<u32MAX；主头20，checked glyph_count+1 后 offsets终点≤262160；variation data用slice.get；不调用 outline/VariationTuples",
    },
    ParseCall {
        table: "共同底层",
        source: "parser.rs Stream::{read,read_bytes,read_array16,new_at,skip,advance,tail}/LazyArray16::{len,get}; 各标量 FromData",
        mechanism: "上述 offset+read 窗口在 u32 内，也在 GNU usize 内；advance 的固定/checked u16 长度与 new_at bounds 另有前件；LazyArray get≤已记录u16长度；正常/Option/Result 返回无辅助 heap，无输入触发的算术 panic",
    },
    ParseCall {
        table: "名字适配与 basename",
        source: "inspect.rs name_label/Label::push/inspect",
        mechanism: "as_chunks<2>/decode_utf16 完整 tail；odd尾忽略，invalid候选继续，合法空/control为Some；noncontrol≤128 scalar≤512 UTF8 bytes；basename rsplit借用；不调用 Name::to_string/中文 owned 错误",
    },
    ParseCall {
        table: "SHA256",
        source: "sha2 0.11.0 lib.rs buffer_fixed; digest 0.11.3 固定输出入口; inspect.rs hex",
        mechanism: "全原始 bytes；固定 core/block/output 与 CPU dispatch，不调用 DynDigest/alloc API；64-byte ASCII hex 栈数组",
    },
];

/// 仅列真实 fixture QA 的调用图，不给任意 malformed glyph/format 输入零堆或无 panic 保证。
/// 生产 inspect 不执行这些 API；新增生产调用必须另补闭包。
pub const QA_CALLS: &[ParseCall] = &[
    ParseCall {
        table: "字符映射",
        source: "lib.rs Face::glyph_index; tables/cmap/mod.rs SubtablesIter::next/Subtables::get/Subtable::glyph_index",
        mechanism: "项目 Meslo glyph A 的 format4 parser/getter；合成 glyph A 的 format0 parser/getter；仅借用有效 fixture，非任意 cmap format 的源证",
    },
    ParseCall {
        table: "glyph/header bbox",
        source: "tables/glyf.rs Table::bbox/get; tables/loca.rs glyph_range; lib.rs global_bounding_box",
        mechanism: "loca LazyArray get→glyf window→固定10-byte glyph头；全局 bbox 来自 head；仅比较 fixture 的旧新完整 Rect",
    },
    ParseCall {
        table: "默认 outline 与 tight bbox",
        source: "lib.rs outline_glyph/glyph_bounding_box/variation_coordinates; tables/glyf.rs outline/outline_impl/parse_simple_outline; tables/gvar.rs outline/outline_var_impl/parse_variation_data/VariationTuples",
        mechanism: "静态有效 glyph 走 glyf/simple points/flags/坐标；variable 优先 gvar，默认 coords 与空 variation window、固定 VariationTuples 保留；tight bbox 复用 outline；不调用 set_variation，不外推到任意损坏 outline",
    },
];
