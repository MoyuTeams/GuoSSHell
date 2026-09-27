import 'dart:convert';
import 'dart:typed_data';

import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/terminal/frame.dart';
import 'package:guosh_shell/src/terminal/frame_terminal.dart';
import 'package:terminal_view/terminal_view.dart';

FrameRun run(
  int start,
  int len,
  String text, {
  int attrs = 0,
  List<int>? layout,
}) => FrameRun(
  start: start,
  len: len,
  attrs: layout == null ? attrs : attrs | Attr.layout,
  fg: const DefaultColor(),
  bg: const DefaultColor(),
  text: text,
  layout: layout,
);

/// 一行的每一列：有字形的列给出字形（含组合字符），宽字符的第二列与空白列为空串。
List<String> columnsOf(BufferLine line, int cols) => [
  for (var col = 0; col < cols; col++)
    line.getCodePoint(col) == 0
        ? ''
        : String.fromCharCode(line.getCodePoint(col)) +
              (line.getCombined(col) ?? ''),
];

/// 没有滚回的帧：最早一行 = 屏幕首行 = 帧的第一行。
TerminalFrame frame({
  int cols = 20,
  int rows = 3,
  required List<FrameRow> lines,
  int cursorCol = -1,
  int cursorRow = -1,
  int? firstStableRow,
  int? screenTopStableRow,
}) {
  final top = lines.isEmpty ? 0 : lines.first.stableRow;
  return TerminalFrame(
    cols: cols,
    rows: rows,
    cursorCol: cursorCol,
    cursorRow: cursorRow,
    firstStableRow: firstStableRow ?? top,
    screenTopStableRow: screenTopStableRow ?? top,
    lines: lines,
  );
}

void main() {
  FrameRow rowAt(int stableRow, String text) => FrameRow(
    stableRow: stableRow,
    wrapped: false,
    runs: [run(0, text.length, text)],
  );

  test('宽字符按引擎给的布局铺格：之后的着色段落在引擎的列上', () {
    final terminal = FrameTerminal();
    // 引擎：中(0-1) 文(2-3) X(4) " tail "(5-10) 🚀(11-12) Y(13)
    terminal.applyFrame(
      frame(
        lines: [
          FrameRow(
            stableRow: 0,
            wrapped: false,
            runs: [
              run(0, 4, '中文', layout: [0x12, 0x12]),
              run(4, 1, 'X'),
              run(
                5,
                8,
                ' tail 🚀',
                layout: [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x12],
              ),
              run(13, 1, 'Y'),
            ],
          ),
        ],
      ),
    );
    final columns = columnsOf(terminal.lineAt(0), 14);
    expect(columns, [
      '中',
      '',
      '文',
      '',
      'X',
      ' ',
      't',
      'a',
      'i',
      'l',
      ' ',
      '🚀',
      '',
      'Y',
    ]);
  });

  test('一整行宽字符铺满所有列，不丢后半行', () {
    final terminal = FrameTerminal();
    terminal.applyFrame(
      frame(
        cols: 20,
        lines: [
          FrameRow(
            stableRow: 0,
            wrapped: false,
            runs: [run(0, 20, '中' * 10, layout: List.filled(10, 0x12))],
          ),
        ],
      ),
    );
    final columns = columnsOf(terminal.lineAt(0), 20);
    expect(columns.where((c) => c == '中').length, 10);
    expect(columns[18], '中');
  });

  test('组合字符跟随它的基字符占一格', () {
    final terminal = FrameTerminal();
    terminal.applyFrame(
      frame(
        lines: [
          FrameRow(
            stableRow: 0,
            wrapped: false,
            runs: [
              run(0, 1, 'e\u0301', layout: [0x21]),
              run(1, 1, '!'),
            ],
          ),
        ],
      ),
    );
    final columns = columnsOf(terminal.lineAt(0), 2);
    expect(columns, ['e\u0301', '!']);
  });

  test('扩展布局保留超过 15 码点的整格，后续字符仍位于引擎指定的列', () {
    for (final (base, width) in [('a', 1), ('中', 2)]) {
      for (final count in [15, 16, 32, 256]) {
        final cluster = '$base${'\u0301' * (count - 1)}';
        final text = utf8.encode('${cluster}B');
        final bytes = BytesBuilder();
        void u16(int value) => bytes.add(
          (ByteData(
            2,
          )..setUint16(0, value, Endian.little)).buffer.asUint8List(),
        );
        void u32(int value) => bytes.add(
          (ByteData(
            4,
          )..setUint32(0, value, Endian.little)).buffer.asUint8List(),
        );
        u16(1); // 一行。
        u16(1); // 一个 run。
        bytes.add(List.filled(9, 0)); // stable_row 与 wrapped。
        u16(0);
        u16(width + 1);
        bytes.add([0, 0, Attr.layout]);
        u32(text.length);
        bytes.add(text);
        u16(2);
        if (count <= 15) {
          bytes.addByte((count << 4) | width);
        } else {
          bytes.addByte(width); // 高半字节为 0，后接完整码点数。
          u32(count);
        }
        bytes.addByte(0x11);
        final decoded = decodeFrame(
          bytes.takeBytes(),
          cols: width + 1,
          rows: 1,
          cursorCol: -1,
          cursorRow: -1,
          firstStableRow: 0,
          screenTopStableRow: 0,
        );
        final terminal = FrameTerminal()..applyFrame(decoded);
        expect(columnsOf(terminal.lineAt(0), width + 1), [
          cluster,
          if (width == 2) '',
          'B',
        ]);
        expect(decoded.lines.single.runs.single.layout, [
          (count << 4) | width,
          0x11,
        ]);
      }
    }
  });

  test('布局变了的行不复用对象（只比 text 会漏掉宽度变化）', () {
    final terminal = FrameTerminal();
    FrameRow wide(List<int> layout, int len) => FrameRow(
      stableRow: 0,
      wrapped: false,
      runs: [run(0, len, 'ab', layout: layout)],
    );
    terminal.applyFrame(
      frame(
        lines: [
          wide([0x11, 0x11], 2),
        ],
      ),
    );
    final before = terminal.lineAt(0);
    terminal.applyFrame(
      frame(
        lines: [
          wide([0x12, 0x11], 3),
        ],
      ),
    );
    expect(identical(before, terminal.lineAt(0)), isFalse);
  });

  test('同一行的内容跨帧不变则复用同一对象（锚点与 Picture 缓存的地基）', () {
    final terminal = FrameTerminal();
    terminal.applyFrame(
      frame(lines: [rowAt(10, 'aaaa'), rowAt(11, 'bbbb'), rowAt(12, 'cccc')]),
    );
    final first = [0, 1, 2].map(terminal.lineAt).toList();

    terminal.applyFrame(
      frame(lines: [rowAt(10, 'aaaa'), rowAt(11, 'bbbb'), rowAt(12, 'cccc')]),
    );
    for (var i = 0; i < 3; i++) {
      expect(
        identical(first[i], terminal.lineAt(i)),
        isTrue,
        reason: '同 stable_row 同内容的行必须复用对象',
      );
    }
  });

  test('内容滚走后行对象跟着内容走（选区锚点才不会停在原位）', () {
    final terminal = FrameTerminal();
    terminal.applyFrame(
      frame(lines: [rowAt(10, 'top'), rowAt(11, 'mid'), rowAt(12, 'bot')]),
    );
    final midLine = terminal.lineAt(1);

    // 内容整体上移一行：mid（stable_row 11）应出现在位置 0。
    terminal.applyFrame(
      frame(lines: [rowAt(11, 'mid'), rowAt(12, 'bot'), rowAt(13, 'new')]),
    );
    expect(identical(terminal.lineAt(0), midLine), isTrue);
  });

  test('stableRow 换算：视口行 ↔ 引擎绝对行', () {
    final terminal = FrameTerminal();
    terminal.applyFrame(
      frame(lines: [rowAt(10, 'aaaa'), rowAt(11, 'bbbb'), rowAt(12, 'cccc')]),
    );
    expect(terminal.stableRowAt(0), 10);
    expect(terminal.stableRowAt(2), 12);
    expect(terminal.stableRowAt(3), isNull);
    expect(terminal.indexOfStable(11), 1);
    expect(terminal.indexOfStable(99), isNull);

    // 内容整体上移：换算表跟着新帧更新。
    terminal.applyFrame(
      frame(lines: [rowAt(11, 'bbbb'), rowAt(12, 'cccc'), rowAt(13, 'dddd')]),
    );
    expect(terminal.stableRowAt(0), 11);
    expect(terminal.indexOfStable(11), 0);
    expect(terminal.indexOfStable(10), isNull);
  });

  test('滚回：行数 = 滚回 + 屏幕，窗口外是空白占位，光标在屏幕里', () {
    final terminal = FrameTerminal();
    terminal.applyFrame(
      frame(
        rows: 3,
        cursorCol: 2,
        cursorRow: 1,
        firstStableRow: 0,
        screenTopStableRow: 100,
        lines: [rowAt(50, 'x50'), rowAt(51, 'x51'), rowAt(52, 'x52')],
      ),
    );
    expect(terminal.height, 103);
    expect(terminal.screenTopIndex, 100);
    expect(terminal.lineAt(51).getText(), 'x51');
    expect(terminal.lineAt(0).getText(), isEmpty);
    expect(terminal.lineAt(102).getText(), isEmpty);
    expect(terminal.absoluteCursorY, 101);
    expect(terminal.cursorX, 2);
    expect(terminal.stableRowAt(102), 102);
    expect(terminal.stableRowAt(103), isNull);
    expect(terminal.windowCovers(50, 3), isTrue);
    expect(terminal.windowCovers(49, 3), isFalse);
    expect(terminal.windowCovers(51, 3), isFalse);
  });

  test('最早一行后移时报告平移的行数（滚回满了裁掉旧行）', () {
    final terminal = FrameTerminal();
    terminal.applyFrame(
      frame(
        firstStableRow: 0,
        screenTopStableRow: 100,
        lines: [rowAt(100, 'a'), rowAt(101, 'b'), rowAt(102, 'c')],
      ),
    );
    terminal.applyFrame(
      frame(
        firstStableRow: 5,
        screenTopStableRow: 105,
        lines: [rowAt(105, 'd'), rowAt(106, 'e'), rowAt(107, 'f')],
      ),
    );
    expect(terminal.originShift, 5);
    expect(terminal.height, 103);
    expect(terminal.indexOfStable(105), 100);
    expect(terminal.indexOfStable(4), isNull);
  });

  test('锚点已收养（attached），选区依赖这个性质', () {
    final terminal = FrameTerminal();
    terminal.applyFrame(
      frame(
        lines: [
          FrameRow(stableRow: 0, wrapped: false, runs: [run(0, 4, 'abcd')]),
        ],
      ),
    );
    final anchor = terminal.createAnchor(2, 0);
    expect(anchor.attached, isTrue);
    expect(anchor.offset, const CellOffset(2, 0));
  });

  test('getWordBoundary 按分隔符切词', () {
    final terminal = FrameTerminal();
    // "ls -la /tmp"：'-la' 的 '-' 在 x=3。
    terminal.applyFrame(
      frame(
        lines: [
          FrameRow(
            stableRow: 0,
            wrapped: false,
            runs: [run(0, 11, 'ls -la /tmp')],
          ),
        ],
      ),
    );
    // 词边界端点排他（fork 约定）：x=4 的 'l' → 词是 'la'。
    final word = terminal.getWordBoundary(const CellOffset(4, 0));
    expect(word, isNotNull);
    expect(terminal.getText(word), 'la');
    // 分隔符上没有词：返回 null，页面侧回退「单格选区」。
    expect(terminal.getWordBoundary(const CellOffset(3, 0)), isNull);
  });

  test('getText 尊重 wrapped 标志：软换行的物理行不插换行符', () {
    final terminal = FrameTerminal();
    terminal.applyFrame(
      frame(
        lines: [
          FrameRow(stableRow: 0, wrapped: false, runs: [run(0, 20, 'a' * 20)]),
          FrameRow(stableRow: 1, wrapped: true, runs: [run(0, 3, 'bbb')]),
          FrameRow(stableRow: 2, wrapped: false, runs: [run(0, 3, 'end')]),
        ],
      ),
    );
    // 端点排他：取满 'end' 要指到 x=3。
    final text = terminal.getText(
      BufferRangeLine(const CellOffset(0, 0), const CellOffset(3, 2)),
    );
    expect(text, '${'a' * 20}bbb\nend');
  });

  test('帧变矮后高度跟着收敛（旧行尾巴不算进缓冲）', () {
    FrameRow row(int i) =>
        FrameRow(stableRow: i, wrapped: false, runs: [run(0, 3, 'r$i')]);
    final terminal = FrameTerminal();
    terminal.applyFrame(
      frame(cols: 20, rows: 46, lines: List.generate(46, row)),
    );
    expect(terminal.height, 46);

    // 键盘弹出：帧变 25 行。缓冲物理长度仍是 46（fork 没有删行 API），
    // 但 height 必须收敛到 25，否则旧行会被当成缓冲内容（实机重影 bug）。
    terminal.applyFrame(
      frame(cols: 20, rows: 25, lines: List.generate(25, row)),
    );
    expect(terminal.height, 25);
    expect(terminal.lineAt(24), isNotNull);
  });

  test('复制时裁掉每行行尾空格（上游把空白格发成字面空格）', () {
    final terminal = FrameTerminal();
    terminal.applyFrame(
      frame(
        cols: 20,
        rows: 2,
        lines: [
          FrameRow(
            stableRow: 0,
            wrapped: false,
            runs: [run(0, 20, 'abc${' ' * 17}')],
          ),
          FrameRow(
            stableRow: 1,
            wrapped: false,
            runs: [run(0, 20, 'de${' ' * 18}')],
          ),
        ],
      ),
    );
    final text = terminal.getText(
      BufferRangeLine(const CellOffset(0, 0), const CellOffset(20, 1)),
    );
    expect(text, 'abc\nde');
  });

  test('getText 裁掉行尾空白', () {
    final terminal = FrameTerminal();
    terminal.applyFrame(
      frame(
        lines: [
          FrameRow(stableRow: 0, wrapped: false, runs: [run(0, 4, 'abc')]),
        ],
      ),
    );
    final text = terminal.getText(
      BufferRangeLine(const CellOffset(0, 0), const CellOffset(19, 0)),
    );
    expect(text, 'abc');
  });
}
