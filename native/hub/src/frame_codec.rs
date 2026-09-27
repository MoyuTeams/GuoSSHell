//! `RenderFrame` → 紧凑 run 字节流（run 压缩 + 打包）。
//!
//! 相邻同属性的单元格合并成一个 run（§4 实测：4,800 格 → 73–200 run → 5.8–7.2 KB/帧）。
//! 列位置与每格的宽度都以引擎为准：Dart 只按这里给出的布局铺格子，不自己算字符宽度
//! （宽度的权威在引擎，铁律 4）。
//!
//! 脏行增量尚未实现：`TerminalEngine::advance` 只给帧级 `dirty: bool`，
//! 行级脏标记上游没有暴露；先发整帧压缩（典型 5.8–7.2 KB，TUI 满屏最坏 61 KB）。

use rshell_m0::rshell_core::{CellAttributes, Color, RenderCell, RenderFrame};

/// attrs 的第 6 位：run 后面跟着逐格布局（有宽字符或多码点的格子）。
const ATTR_LAYOUT: u8 = 1 << 6;

/// 每格都是「一个码点、宽 1」时的布局字节（码点数 << 4 | 宽度）。
const SIMPLE_CELL: u64 = (1 << 4) | 1;

/// wire 格式（全部**小端**）：
///
/// ```text
/// row_count: u16
/// 每行:
///   run_count: u16 · stable_row: i64 · wrapped: u8
///   每个 run:
///     start: u16 · len: u16 · fg · bg · attrs: u8 · text_len: u32 · text: utf8
///     [attrs 第 6 位为 1 时] cell_count: u16 · 每格 { descriptor: u8 · [扩展时] code_points: u32 }
/// ```
///
/// `start` / `len` 以**列**计。run 里每格都是「一个码点、宽 1」时不带布局，text 的每个码点
/// 就是一格；否则 layout 逐格给出 `码点数 << 4 | 宽度`（宽度 1 或 2，宽字符的第二列不单独成格，
/// 码点数含组合字符与零宽连接符），按它从 text 里切出每一格。
/// 超过 15 个码点的格子把 descriptor 的高半字节置 0，再用后续 u32 给出完整码点数。
/// `fg` / `bg`：`0` = Default；`1` + `u8` = Ansi(index)；`2` + r,g,b = Rgb。
/// `attrs` 位：0 bold · 1 italic · 2 underline · 3 strike · 4 reverse · 5 selected · 6 布局。
pub fn pack_runs(frame: &RenderFrame) -> Vec<u8> {
    let mut out = Vec::with_capacity(64 * 1024);
    let rows = &frame.rows[..];
    out.extend_from_slice(&(rows.len() as u16).to_le_bytes());
    for row in rows {
        let mut runs: Vec<Run<'_>> = Vec::new();
        let mut column: u16 = 0;
        for cell in row.cells.iter() {
            let text = if cell.text.is_empty() {
                " "
            } else {
                cell.text.as_str()
            };
            let width = cell.width.clamp(1, 2);
            let code_points = text.chars().count() as u64;
            let layout = (code_points << 4) | u64::from(width);
            match runs.last_mut() {
                Some(run) if same_run(run.head, cell) => run.push(text, width, layout),
                _ => runs.push(Run::new(column, cell, text, width, layout)),
            }
            column = column.saturating_add(u16::from(width));
        }
        out.extend_from_slice(&(runs.len() as u16).to_le_bytes());
        out.extend_from_slice(&row.stable_row.to_le_bytes());
        out.push(u8::from(row.wrapped));
        for run in runs {
            out.extend_from_slice(&run.start.to_le_bytes());
            out.extend_from_slice(&run.columns.to_le_bytes());
            write_color(run.head.foreground, &mut out);
            write_color(run.head.background, &mut out);
            let layout = run.layout.as_deref();
            write_attrs(
                run.head.attributes,
                run.head.selected,
                layout.is_some(),
                &mut out,
            );
            out.extend_from_slice(&(run.text.len() as u32).to_le_bytes());
            out.extend_from_slice(run.text.as_bytes());
            if let Some(layout) = layout {
                out.extend_from_slice(&(layout.len() as u16).to_le_bytes());
                for &cell in layout {
                    let code_points = cell >> 4;
                    if code_points <= 15 {
                        out.push(cell as u8);
                    } else {
                        out.push((cell & 0x0f) as u8);
                        out.extend_from_slice(&(code_points as u32).to_le_bytes());
                    }
                }
            }
        }
    }
    out
}

/// 打包中的一个 run。布局只在出现非简单格子时才建（此前的格子补成简单格）。
struct Run<'a> {
    start: u16,
    columns: u16,
    cells: u16,
    head: &'a RenderCell,
    text: String,
    layout: Option<Vec<u64>>,
}

impl<'a> Run<'a> {
    fn new(start: u16, head: &'a RenderCell, text: &str, width: u8, layout: u64) -> Self {
        let mut run = Self {
            start,
            columns: 0,
            cells: 0,
            head,
            text: String::with_capacity(text.len() * 8),
            layout: None,
        };
        run.push(text, width, layout);
        run
    }

    fn push(&mut self, text: &str, width: u8, layout: u64) {
        if layout != SIMPLE_CELL && self.layout.is_none() {
            self.layout = Some(vec![SIMPLE_CELL; usize::from(self.cells)]);
        }
        if let Some(bytes) = self.layout.as_mut() {
            bytes.push(layout);
        }
        self.text.push_str(text);
        self.columns = self.columns.saturating_add(u16::from(width));
        self.cells = self.cells.saturating_add(1);
    }
}

/// 相邻单元格在 (fg, bg, attrs, selected) 上完全相同才属于同一个 run。
fn same_run(left: &RenderCell, right: &RenderCell) -> bool {
    left.foreground == right.foreground
        && left.background == right.background
        && left.attributes == right.attributes
        && left.selected == right.selected
}

fn write_color(color: Color, out: &mut Vec<u8>) {
    match color {
        Color::Default => out.push(0),
        Color::Ansi(index) => {
            out.push(1);
            out.push(index);
        }
        Color::Rgb(r, g, b) => {
            out.push(2);
            out.extend_from_slice(&[r, g, b]);
        }
    }
}

fn write_attrs(attributes: CellAttributes, selected: bool, layout: bool, out: &mut Vec<u8>) {
    let mut bits = 0u8;
    bits |= u8::from(attributes.bold);
    bits |= u8::from(attributes.italic) << 1;
    bits |= u8::from(attributes.underline) << 2;
    bits |= u8::from(attributes.strike) << 3;
    bits |= u8::from(attributes.reverse) << 4;
    bits |= u8::from(selected) << 5;
    if layout {
        bits |= ATTR_LAYOUT;
    }
    out.push(bits);
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::pack_runs;
    use rshell_m0::rshell_core::{
        ResolvedTerminalProfile, TerminalOverrides, TerminalSettingsV1, TerminalSize, Viewport,
    };
    use rshell_m0::rshell_session::{DefaultTerminalEngine, TerminalEngine};

    fn size() -> TerminalSize {
        TerminalSize {
            cols: 80,
            rows: 24,
            pixel_width: 0,
            pixel_height: 0,
            dpi: 96,
        }
    }

    /// 解出来的一个 run（测试用的最小解码器，与 Dart 侧 decodeFrame 同一格式）。
    #[derive(Debug)]
    struct DecodedRun {
        start: u16,
        len: u16,
        fg: Vec<u8>,
        text: String,
        layout: Option<Vec<u64>>,
    }

    fn decode(packed: &[u8]) -> Vec<Vec<DecodedRun>> {
        let mut cursor = 0usize;
        let mut take = |n: usize| {
            let slice = &packed[cursor..cursor + n];
            cursor += n;
            slice
        };
        let u16_of = |bytes: &[u8]| u16::from_le_bytes([bytes[0], bytes[1]]);
        let color = |take: &mut dyn FnMut(usize) -> Vec<u8>| {
            let marker = take(1)[0];
            let mut bytes = vec![marker];
            match marker {
                1 => bytes.extend(take(1)),
                2 => bytes.extend(take(3)),
                _ => {}
            }
            bytes
        };
        let mut take_vec = |n: usize| take(n).to_vec();
        let row_count = u16_of(&take_vec(2));
        let mut rows = Vec::new();
        for _ in 0..row_count {
            let run_count = u16_of(&take_vec(2));
            take_vec(8 + 1);
            let mut runs = Vec::new();
            for _ in 0..run_count {
                let start = u16_of(&take_vec(2));
                let len = u16_of(&take_vec(2));
                let fg = color(&mut take_vec);
                color(&mut take_vec);
                let attrs = take_vec(1)[0];
                let text_len = u32::from_le_bytes(take_vec(4).try_into().expect("u32")) as usize;
                let text = String::from_utf8(take_vec(text_len)).expect("utf8");
                let layout = (attrs & (1 << 6) != 0).then(|| {
                    let cells = u16_of(&take_vec(2));
                    (0..cells)
                        .map(|_| {
                            let descriptor = u64::from(take_vec(1)[0]);
                            if descriptor >> 4 != 0 {
                                descriptor
                            } else {
                                let count =
                                    u32::from_le_bytes(take_vec(4).try_into().expect("u32"));
                                (u64::from(count) << 4) | descriptor
                            }
                        })
                        .collect()
                });
                runs.push(DecodedRun {
                    start,
                    len,
                    fg,
                    text,
                    layout,
                });
            }
            rows.push(runs);
        }
        assert_eq!(
            cursor,
            packed.len(),
            "decoder must consume exactly all bytes"
        );
        rows
    }

    fn screen(output: &str) -> Vec<Vec<DecodedRun>> {
        let profile: ResolvedTerminalProfile =
            TerminalSettingsV1::default().resolve(&TerminalOverrides::default());
        let mut engine = DefaultTerminalEngine::new(&profile, size()).expect("engine");
        engine.advance(output.as_bytes()).expect("advance");
        let frame = engine
            .render(
                Viewport {
                    top_stable_row: i64::MAX,
                    rows: 24,
                },
                None,
            )
            .expect("render");
        decode(&pack_runs(&frame))
    }

    #[test]
    fn packed_frame_matches_wire_format() {
        let rows = screen("\x1b[31mred\x1b[0m plain\r\n\u{4e2d}\u{6587} wide\r\n");
        assert_eq!(rows.len(), 24);
        let red = &rows[0][0];
        assert_eq!((red.start, red.len, red.text.as_str()), (0, 3, "red"));
        assert_eq!(red.fg, vec![1, 1], "SGR 31 is Ansi(1)");
        assert!(red.layout.is_none(), "plain ASCII needs no layout");
    }

    /// 位置与长度按列：宽字符后面的着色段落在引擎给的列上，整行中文不缺字。
    #[test]
    fn runs_are_placed_in_columns_after_wide_characters() {
        let rows = screen(
            "\u{4e2d}\u{6587}\x1b[31mX\x1b[0m tail \u{1f680}\x1b[32mY\x1b[0m\r\n\
             e\u{301}\x1b[31m!\x1b[0m\r\n",
        );
        let row = &rows[0];
        assert_eq!(
            (row[0].start, row[0].len, row[0].text.as_str()),
            (0, 4, "\u{4e2d}\u{6587}")
        );
        assert_eq!(
            row[0].layout.as_deref(),
            Some(&[0x12, 0x12][..]),
            "two wide cells"
        );
        assert_eq!(
            (row[1].start, row[1].len, row[1].text.as_str()),
            (4, 1, "X")
        );
        assert_eq!((row[2].start, row[2].text.as_str()), (5, " tail \u{1f680}"));
        assert_eq!(row[2].len, 8, "six narrow cells and one wide emoji");
        let layout = row[2].layout.as_deref().expect("layout for the emoji");
        assert_eq!(layout.len(), 7);
        assert_eq!(layout[6], 0x12);
        assert_eq!((row[3].start, row[3].text.as_str()), (13, "Y"));
        let combined = &rows[1];
        assert_eq!(
            combined[0].layout.as_deref(),
            Some(&[0x21][..]),
            "e + combining acute"
        );
        assert_eq!((combined[1].start, combined[1].text.as_str()), (1, "!"));
    }

    #[test]
    fn a_full_row_of_wide_characters_covers_every_column() {
        let rows = screen(&"\u{4e2d}".repeat(40));
        let run = &rows[0][0];
        assert_eq!((run.start, run.len), (0, 80));
        assert_eq!(run.text.chars().count(), 40);
    }

    #[test]
    fn long_combining_clusters_keep_the_following_cells_aligned() {
        for (base, width) in [("a", 1), ("中", 2)] {
            for count in [15, 16, 32, 256] {
                let cluster = format!("{base}{}", "\u{301}".repeat(count - 1));
                let rows = screen(&format!("{cluster}B"));
                let run = &rows[0][0];
                let layout = run.layout.as_ref().expect("组合字符布局");
                assert_eq!(layout[0], ((count as u64) << 4) | width);
                assert_eq!(layout[1], 0x11);
                let mut chars = run.text.chars();
                assert_eq!(
                    chars
                        .by_ref()
                        .take((layout[0] >> 4) as usize)
                        .collect::<String>(),
                    cluster
                );
                assert_eq!(chars.next(), Some('B'));
                assert_eq!(
                    layout.iter().map(|cell| cell >> 4).sum::<u64>(),
                    run.text.chars().count() as u64
                );
            }
        }
    }
}
