//! 固定栈空间保留旧 inspect 的检查顺序；只截取输出，不截断 UTF16 验证。

use sha2::{Digest, Sha256};

use crate::selected_p1::model::layout::NumericReject;

pub const MAX_FONT_BYTES: usize = 32 * 1024 * 1024;
pub const LABEL_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum FontReject {
    TooLarge = 1,
    Extension = 2,
    Signature = 3,
    Face = 4,
    GlyphTables = 5,
    Budget(NumericReject) = 6,
    Read = 7,
}

impl From<NumericReject> for FontReject {
    fn from(value: NumericReject) -> Self {
        Self::Budget(value)
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct CheckedFont {
    label: [u8; LABEL_BYTES],
    label_len: usize,
    hash: [u8; 64],
}

impl CheckedFont {
    pub fn label(&self) -> &str {
        // SAFETY: 唯一写入者使用 char::encode_utf8。
        unsafe { std::str::from_utf8_unchecked(&self.label[..self.label_len]) }
    }

    pub fn hash(&self) -> &str {
        // SAFETY: SHA256 输出只写入 ASCII 小写十六进制。
        unsafe { std::str::from_utf8_unchecked(&self.hash) }
    }
}

struct Label {
    bytes: [u8; LABEL_BYTES],
    len: usize,
    scalars: usize,
}

impl Default for Label {
    fn default() -> Self {
        Self {
            bytes: [0; LABEL_BYTES],
            len: 0,
            scalars: 0,
        }
    }
}

impl Label {
    fn push(&mut self, ch: char) {
        if self.scalars == 128 || ch.is_control() {
            return;
        }
        let mut utf8 = [0; 4];
        let encoded = ch.encode_utf8(&mut utf8).as_bytes();
        let end = self.len + encoded.len();
        self.bytes[self.len..end].copy_from_slice(encoded);
        self.len = end;
        self.scalars += 1;
    }
}

/// 非 Unicode/非法候选是 None；合法空或全 control 是 Some(空)。
pub fn name_label(name: ttf_parser::name::Name<'_>) -> Option<([u8; LABEL_BYTES], usize)> {
    if !name.is_unicode() {
        return None;
    }
    let units = name
        .name
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_be_bytes([pair[0], pair[1]]));
    let mut label = Label::default();
    for decoded in char::decode_utf16(units) {
        // 达到 128 个输出 scalar 后仍解码整个 tail；odd 最后一字节同旧 LazyArray 忽略。
        label.push(decoded.ok()?);
    }
    Some((label.bytes, label.len))
}

// 调用前由 inspect 排除 TTC 并限制输入大小；Face 只活在检查期间，不作为通用解析对象交接。
fn inspection_face(bytes: &[u8]) -> Result<ttf_parser::Face<'_>, ttf_parser::FaceParsingError> {
    let raw = ttf_parser::RawFace::parse(bytes, 0)?;
    let mut tables = ttf_parser::RawFaceTables::default();
    for index in 0..raw.table_records.len() {
        let record = raw
            .table_records
            .get(index)
            .ok_or(ttf_parser::FaceParsingError::MalformedFont)?;
        let start = record.offset as usize;
        let Some(end) = start.checked_add(record.length as usize) else {
            continue;
        };
        let data = bytes.get(start..end);
        // 同 tag 的后项覆盖前项，包括越界 None 和有效的 Some(空)；求和溢出则跳过该项。
        match &record.tag.to_bytes() {
            b"head" => tables.head = data.unwrap_or_default(),
            b"hhea" => tables.hhea = data.unwrap_or_default(),
            b"maxp" => tables.maxp = data.unwrap_or_default(),
            b"loca" => tables.loca = data,
            b"glyf" => tables.glyf = data,
            b"cmap" => tables.cmap = data,
            b"name" => tables.name = data,
            b"fvar" => tables.fvar = data,
            b"gvar" => tables.gvar = data,
            _ => {}
        }
    }
    ttf_parser::Face::from_raw_tables(tables)
}

/// 仅语义 QA 复用同一构造函数；glyph/outline API 不属于生产 inspection 的源闭包。
#[cfg(test)]
pub(crate) fn face_for_qa(
    bytes: &[u8],
) -> Result<ttf_parser::Face<'_>, ttf_parser::FaceParsingError> {
    inspection_face(bytes)
}

pub fn inspect(bytes: &[u8], file_name: &str) -> Result<CheckedFont, FontReject> {
    if bytes.len() > MAX_FONT_BYTES {
        return Err(FontReject::TooLarge);
    }
    if !file_name
        .as_bytes()
        .get(file_name.len().saturating_sub(4)..)
        .is_some_and(|suffix| suffix.eq_ignore_ascii_case(b".ttf"))
    {
        return Err(FontReject::Extension);
    }
    if bytes.get(..4) != Some(&[0, 1, 0, 0]) && bytes.get(..4) != Some(b"true") {
        return Err(FontReject::Signature);
    }
    let face = inspection_face(bytes).map_err(|_| FontReject::Face)?;
    if face.number_of_glyphs() == 0 || face.tables().glyf.is_none() || face.tables().cmap.is_none()
    {
        return Err(FontReject::GlyphTables);
    }
    let label = face
        .names()
        .into_iter()
        .filter(|name| name.name_id == ttf_parser::name_id::FAMILY)
        .find_map(name_label);
    let (label, label_len) = match label {
        Some(value) => value,
        None => {
            let basename = file_name.rsplit(['/', '\\']).next().unwrap_or("自定义字体");
            let mut label = Label::default();
            for ch in basename.chars() {
                label.push(ch);
            }
            (label.bytes, label.len)
        }
    };
    let digest = Sha256::digest(bytes);
    let mut hash = [0; 64];
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for (index, byte) in digest.iter().enumerate() {
        hash[index * 2] = HEX[usize::from(byte >> 4)];
        hash[index * 2 + 1] = HEX[usize::from(byte & 15)];
    }
    Ok(CheckedFont {
        label,
        label_len,
        hash,
    })
}
