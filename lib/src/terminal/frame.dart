import 'dart:convert';
import 'dart:typed_data';

/// 终端颜色（边界上的三种形态，对应 `rshell_core::Color`）。
sealed class TermColor {
  const TermColor();
}

class DefaultColor extends TermColor {
  const DefaultColor();
}

class AnsiColor extends TermColor {
  final int index;
  const AnsiColor(this.index);
}

class RgbColor extends TermColor {
  final int r, g, b;
  const RgbColor(this.r, this.g, this.b);
}

/// 属性位（与 frame_codec.rs 的 wire 格式一致）。
class Attr {
  static const int bold = 1;
  static const int italic = 2;
  static const int underline = 4;
  static const int strike = 8;
  static const int reverse = 16;
  static const int selected = 32;

  /// run 后面带逐格布局（有宽字符或多码点的格子）。
  static const int layout = 64;
}

/// 一段连续同属性单元格。`start`/`len` 以列计。
///
/// 每格的宽度与字形由引擎给出，这里不算宽度：[layout] 为空时 [text] 的每个码点就是一格、
/// 宽 1；否则 [layout] 逐格给出 `码点数 << 4 | 宽度`（宽字符的第二列不单独成格）。
class FrameRun {
  final int start, len, attrs;
  final TermColor fg, bg;
  final String text;
  final List<int>? layout;
  const FrameRun({
    required this.start,
    required this.len,
    required this.attrs,
    required this.fg,
    required this.bg,
    required this.text,
    this.layout,
  });

  bool get bold => attrs & Attr.bold != 0;
  bool get italic => attrs & Attr.italic != 0;
  bool get underline => attrs & Attr.underline != 0;
  bool get strike => attrs & Attr.strike != 0;
  bool get reverse => attrs & Attr.reverse != 0;
  bool get selected => attrs & Attr.selected != 0;
}

class FrameRow {
  final int stableRow;
  final bool wrapped;
  final List<FrameRun> runs;
  const FrameRow({
    required this.stableRow,
    required this.wrapped,
    required this.runs,
  });
}

/// 一帧画面：几何（屏幕的 cols/rows、光标）、滚回范围 + 按行的 run 列表。
/// 这是**纯展示数据**——Dart 不持有终端状态机（PLAN.md 铁律 4），
/// 每一帧都是 Rust 权威状态的一段快照投影：[lines] 是 Dart 请求的窗口
/// （跟着屏幕时就是屏幕），不一定从 [firstStableRow] 开始。
class TerminalFrame {
  final int cols, rows;

  /// 光标在屏幕上的列与行；`-1` = 隐藏。
  final int cursorCol, cursorRow;

  /// 滚回范围：最早一行与屏幕首行的绝对行号。
  final int firstStableRow, screenTopStableRow;

  /// 远端设置的窗口标题（OSC 0 / 2）。
  final String title;
  final List<FrameRow> lines;
  const TerminalFrame({
    required this.cols,
    required this.rows,
    required this.cursorCol,
    required this.cursorRow,
    required this.firstStableRow,
    required this.screenTopStableRow,
    this.title = '',
    required this.lines,
  });
}

/// 解码 `frame_codec::pack_runs` 的字节流。
/// wire 格式（全部小端）：
/// row_count:u16 → 每行 { run_count:u16 · stable_row:i64 · wrapped:u8 →
///   每个 run { start:u16 · len:u16 · fg · bg · attrs:u8 · text_len:u32 · text ·
///     [attrs 带 Attr.layout 时] cell_count:u16 · 每格 { descriptor:u8 · [扩展时] code_points:u32 } } }
/// descriptor 高半字节非 0 时为码点数；为 0 时读取后续 u32，完整展开为 `码点数 << 4 | 宽度`。
/// fg/bg：0=Default；1+u8=Ansi；2+r,g,b=Rgb。start / len 以列计。
TerminalFrame decodeFrame(
  Uint8List binary, {
  required int cols,
  required int rows,
  required int cursorCol,
  required int cursorRow,
  required int firstStableRow,
  required int screenTopStableRow,
  String title = '',
}) {
  final data = ByteData.sublistView(binary);
  var o = 0;
  void need(int n) {
    if (o + n > binary.length) {
      throw FormatException('truncated frame at byte $o (+$n)');
    }
  }

  int u8() {
    need(1);
    return data.getUint8(o++);
  }

  int u16() {
    need(2);
    final v = data.getUint16(o, Endian.little);
    o += 2;
    return v;
  }

  int u32() {
    need(4);
    final v = data.getUint32(o, Endian.little);
    o += 4;
    return v;
  }

  int i64() {
    need(8);
    final v = data.getInt64(o, Endian.little);
    o += 8;
    return v;
  }

  TermColor color() {
    final marker = u8();
    switch (marker) {
      case 0:
        return const DefaultColor();
      case 1:
        return AnsiColor(u8());
      case 2:
        final r = u8();
        final g = u8();
        final b = u8();
        return RgbColor(r, g, b);
      default:
        throw FormatException('bad color marker $marker at byte ${o - 1}');
    }
  }

  final rowCount = u16();
  final lines = <FrameRow>[];
  for (var r = 0; r < rowCount; r++) {
    final runCount = u16();
    final stableRow = i64();
    final wrapped = u8() != 0;
    final runs = <FrameRun>[];
    for (var i = 0; i < runCount; i++) {
      final start = u16();
      final len = u16();
      final fg = color();
      final bg = color();
      final attrs = u8();
      final textLen = u32();
      need(textLen);
      final text = utf8.decode(
        binary.sublist(o, o + textLen),
        allowMalformed: true,
      );
      o += textLen;
      List<int>? layout;
      if (attrs & Attr.layout != 0) {
        final cells = u16();
        layout = List<int>.generate(cells, (_) {
          final descriptor = u8();
          final width = descriptor & 0x0f;
          final count = descriptor >> 4 == 0 ? u32() : descriptor >> 4;
          if (count == 0 || count > text.length || (width != 1 && width != 2)) {
            throw FormatException('invalid cell layout at byte $o');
          }
          return (count << 4) | width;
        }, growable: false);
      }
      runs.add(
        FrameRun(
          start: start,
          len: len,
          attrs: attrs,
          fg: fg,
          bg: bg,
          text: text,
          layout: layout,
        ),
      );
    }
    lines.add(FrameRow(stableRow: stableRow, wrapped: wrapped, runs: runs));
  }
  if (o != binary.length) {
    throw FormatException('trailing bytes: stream=$binary.length consumed=$o');
  }
  return TerminalFrame(
    cols: cols,
    rows: rows,
    cursorCol: cursorCol,
    cursorRow: cursorRow,
    firstStableRow: firstStableRow,
    screenTopStableRow: screenTopStableRow,
    title: title,
    lines: lines,
  );
}
