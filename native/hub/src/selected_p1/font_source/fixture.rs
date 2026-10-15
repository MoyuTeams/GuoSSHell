//! 可控的完整 sfnt：两个 glyph、三角形 glyf、真实 cmap，不靠 name-only 假 face。

use crate::succeeds;

pub fn put16(data: &mut [u8], offset: usize, value: u16) {
    data[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
}
pub fn put32(data: &mut [u8], offset: usize, value: u32) {
    data[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

pub fn utf16(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(u16::to_be_bytes).collect()
}

pub fn names(records: &[(u16, u16, u16, &[u8])]) -> Vec<u8> {
    let base = 6 + records.len() * 12;
    let mut table = vec![0; base];
    put16(&mut table, 2, succeeds(u16::try_from(records.len())));
    put16(&mut table, 4, succeeds(u16::try_from(base)));
    for (index, (platform, encoding, id, name)) in records.iter().enumerate() {
        let offset = 6 + index * 12;
        let start = table.len() - base;
        put16(&mut table, offset, *platform);
        put16(&mut table, offset + 2, *encoding);
        put16(&mut table, offset + 6, *id);
        put16(&mut table, offset + 8, succeeds(u16::try_from(name.len())));
        put16(&mut table, offset + 10, succeeds(u16::try_from(start)));
        table.extend_from_slice(name);
    }
    table
}

pub fn tables(name: Vec<u8>) -> Vec<([u8; 4], Vec<u8>)> {
    let mut head = vec![0; 54];
    put32(&mut head, 0, 0x10000);
    put32(&mut head, 12, 0x5f0f3cf5);
    put16(&mut head, 18, 1000);
    let mut hhea = vec![0; 36];
    put32(&mut hhea, 0, 0x10000);
    put16(&mut hhea, 34, 1);
    let mut maxp = vec![0; 32];
    put32(&mut maxp, 0, 0x10000);
    put16(&mut maxp, 4, 2);
    let mut cmap = vec![0; 12 + 262];
    put16(&mut cmap, 2, 1);
    put16(&mut cmap, 4, 3);
    put16(&mut cmap, 6, 1);
    put32(&mut cmap, 8, 12);
    put16(&mut cmap, 14, 262);
    cmap[12 + 6 + 65] = 1;
    vec![
        (*b"head", head),
        (*b"hhea", hhea),
        (*b"maxp", maxp),
        (*b"cmap", cmap),
        (*b"loca", vec![0, 0, 0, 0, 0, 10]),
        (
            *b"glyf",
            vec![
                0, 1, 0, 0, 0, 0, 0, 10, 0, 10, 0, 2, 0, 0, 0x31, 0x33, 0x27, 10, 10, 10, 0, 0,
            ],
        ),
        (*b"hmtx", vec![1, 244, 0, 0, 0, 0]),
        (*b"name", name),
    ]
}

pub fn font(mut tables: Vec<([u8; 4], Vec<u8>)>) -> Vec<u8> {
    tables.sort_by_key(|(tag, _)| *tag);
    let mut bytes = vec![0; 12 + tables.len() * 16];
    put32(&mut bytes, 0, 0x10000);
    put16(&mut bytes, 4, succeeds(u16::try_from(tables.len())));
    for (index, (tag, data)) in tables.into_iter().enumerate() {
        let record = 12 + index * 16;
        bytes[record..record + 4].copy_from_slice(&tag);
        let start = bytes.len();
        put32(&mut bytes, record + 8, succeeds(u32::try_from(start)));
        put32(&mut bytes, record + 12, succeeds(u32::try_from(data.len())));
        bytes.extend_from_slice(&data);
        while !bytes.len().is_multiple_of(4) {
            bytes.push(0);
        }
    }
    bytes
}

pub fn named(data: &[u8]) -> Vec<u8> {
    font(tables(names(&[(3, 1, 1, data)])))
}

fn variation_store() -> Vec<u8> {
    vec![0, 1, 0, 0, 0, 8, 0, 0, 0, 1, 0, 0]
}

/// 所有实际启用的 table parser 都得到合法的入口，惰性 get/outline 不冒充 parse 覆盖。
pub fn with_optional_tables() -> Vec<u8> {
    let mut tables = tables(names(&[(3, 1, 1, &utf16("完整解析闭包"))]));
    tables.push((
        *b"CFF ",
        vec![
            1, 0, 4, 4, 0, 0, 0, 1, 1, 1, 3, 156, 17, 0, 0, 0, 0, 0, 1, 1, 1, 2, 14,
        ],
    ));
    tables.push((
        *b"CFF2",
        vec![2, 0, 5, 0, 2, 150, 17, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 2, 14],
    ));
    let mut cpal = vec![0; 18];
    put16(&mut cpal, 4, 1);
    put16(&mut cpal, 6, 1);
    put32(&mut cpal, 8, 14);
    tables.push((*b"CPAL", cpal));
    let mut colr = vec![0; 47];
    put16(&mut colr, 0, 1);
    for offset in [4, 8, 14] {
        put32(&mut colr, offset, 34);
    }
    put32(&mut colr, 18, 38);
    put32(&mut colr, 22, 42);
    put32(&mut colr, 26, 47);
    put32(&mut colr, 30, 47);
    colr[42] = 1;
    colr.extend_from_slice(&variation_store());
    tables.push((*b"COLR", colr));
    for tag in [*b"bloc", *b"bdat", *b"CBLC", *b"CBDT", *b"EBLC", *b"EBDT"] {
        tables.push((tag, vec![]));
    }
    let mut os2 = vec![0; 100];
    put16(&mut os2, 0, 5);
    tables.push((*b"OS/2", os2));
    let mut post = vec![0; 38];
    put32(&mut post, 0, 0x20000);
    put16(&mut post, 32, 2);
    tables.push((*b"post", post));
    tables.push((*b"kern", vec![0; 4]));
    let mut sbix = vec![0; 12];
    put16(&mut sbix, 0, 1);
    put32(&mut sbix, 4, 1);
    put32(&mut sbix, 8, 12);
    tables.push((*b"sbix", sbix));
    let mut stat = vec![0; 18];
    put32(&mut stat, 0, 0x10000);
    put16(&mut stat, 4, 8);
    put32(&mut stat, 8, 18);
    put32(&mut stat, 14, 18);
    tables.push((*b"STAT", stat));
    let mut svg = vec![0; 8];
    put32(&mut svg, 2, 6);
    tables.push((*b"SVG ", svg));
    let mut vorg = vec![0; 8];
    put32(&mut vorg, 0, 0x10000);
    tables.push((*b"VORG", vorg));
    let mut vhea = vec![0; 36];
    put32(&mut vhea, 0, 0x10000);
    put16(&mut vhea, 34, 1);
    tables.push((*b"vhea", vhea));
    tables.push((*b"vmtx", vec![1, 244, 0, 0, 0, 0]));
    let mut gdef = vec![0; 18];
    put32(&mut gdef, 0, 0x10003);
    put32(&mut gdef, 14, 18);
    gdef.extend_from_slice(&variation_store());
    tables.push((*b"GDEF", gdef));
    for tag in [*b"GPOS", *b"GSUB"] {
        let mut layout = vec![0; 28];
        put16(&mut layout, 0, 1);
        put16(&mut layout, 2, 1);
        put16(&mut layout, 4, 14);
        put16(&mut layout, 6, 16);
        put16(&mut layout, 8, 18);
        put32(&mut layout, 10, 20);
        put16(&mut layout, 20, 1);
        tables.push((tag, layout));
    }
    let mut math = vec![0; 242];
    put16(&mut math, 0, 1);
    put16(&mut math, 4, 10);
    put16(&mut math, 6, 224);
    put16(&mut math, 8, 232);
    tables.push((*b"MATH", math));
    let mut ankr = vec![0; 18];
    put32(&mut ankr, 4, 12);
    put32(&mut ankr, 8, 18);
    tables.push((*b"ankr", ankr));
    let mut feat = vec![0; 12];
    put32(&mut feat, 0, 0x10000);
    tables.push((*b"feat", feat));
    tables.push((*b"kerx", vec![0; 8]));
    tables.push((*b"morx", vec![0; 8]));
    let mut trak = vec![0; 12];
    put32(&mut trak, 0, 0x10000);
    tables.push((*b"trak", trak));
    let mut avar = vec![0; 8];
    put32(&mut avar, 0, 0x10000);
    put16(&mut avar, 6, 1);
    tables.push((*b"avar", avar));
    let mut fvar = vec![0; 36];
    put32(&mut fvar, 0, 0x10000);
    put16(&mut fvar, 4, 16);
    put16(&mut fvar, 8, 1);
    tables.push((*b"fvar", fvar));
    let mut gvar = vec![0; 26];
    put32(&mut gvar, 0, 0x10000);
    put16(&mut gvar, 4, 1);
    put16(&mut gvar, 12, 2);
    put32(&mut gvar, 16, 26);
    tables.push((*b"gvar", gvar));
    for (tag, len) in [(*b"HVAR", 20), (*b"VVAR", 24)] {
        let mut data = vec![0; len];
        put32(&mut data, 0, 0x10000);
        put32(&mut data, 4, succeeds(u32::try_from(len)));
        data.extend_from_slice(&variation_store());
        tables.push((tag, data));
    }
    let mut mvar = vec![0; 20];
    put32(&mut mvar, 0, 0x10000);
    put16(&mut mvar, 6, 8);
    put16(&mut mvar, 8, 1);
    put16(&mut mvar, 10, 20);
    mvar.extend_from_slice(&variation_store());
    tables.push((*b"MVAR", mvar));
    font(tables)
}
