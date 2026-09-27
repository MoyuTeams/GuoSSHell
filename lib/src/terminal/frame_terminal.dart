import 'package:flutter/foundation.dart';
// 我们的 fork 没把这个类放进公开导出面，但锚点机制依赖它（行身份），
// 属于有意的实现级依赖，fork 收紧导出面前先直接引。
// ignore: implementation_imports
import 'package:terminal_view/src/utils/circular_buffer.dart';
import 'package:terminal_view/terminal_view.dart';

import 'frame.dart';

/// 输入事件（适配器 → 页面 → rinf → Rust）。
sealed class TerminalInputEvent {
  const TerminalInputEvent();
}

class KeyInputEvent extends TerminalInputEvent {
  /// "character:x" 或命名键（见 session.rs 的 parse_key_code）。
  final String key;
  final bool shift;
  final bool control;
  final bool alt;
  const KeyInputEvent(this.key, {this.shift = false, this.control = false, this.alt = false});
}

class TextInputEvent extends TerminalInputEvent {
  final String text;
  const TextInputEvent(this.text);
}

/// 粘贴（剪贴板原文）。换行规范化与 bracketed paste 由 Rust 按远端模式处理。
class PasteInputEvent extends TerminalInputEvent {
  final String text;
  const PasteInputEvent(this.text);
}

enum MouseAction { press, release, move }

/// 鼠标事件（M2a）。仅在远端开启鼠标上报时产生。[button] 为 null 是无键移动
/// （悬停）；远端是否要这类事件、同格移动是否重复，都由 Rust 判断。
class MouseInputEvent extends TerminalInputEvent {
  final TerminalMouseButton? button;
  final MouseAction action;
  final CellOffset position;
  final bool shift;
  final bool alt;
  final bool ctrl;
  const MouseInputEvent(
    this.button,
    this.action,
    this.position, {
    this.shift = false,
    this.alt = false,
    this.ctrl = false,
  });
}

/// 终端视口几何（度量权威在 Flutter，PLAN §8）。像素是整个终端的尺寸。
@immutable
class TerminalGeometry {
  final int cols;
  final int rows;
  final int pixelWidth;
  final int pixelHeight;

  const TerminalGeometry({
    required this.cols,
    required this.rows,
    required this.pixelWidth,
    required this.pixelHeight,
  });

  @override
  bool operator ==(Object other) =>
      other is TerminalGeometry &&
      other.cols == cols &&
      other.rows == rows &&
      other.pixelWidth == pixelWidth &&
      other.pixelHeight == pixelHeight;

  @override
  int get hashCode => Object.hash(cols, rows, pixelWidth, pixelHeight);
}

/// 一条池化行：BufferLine 对象 + 它当初被填充时的 run 快照。
/// 快照逐字段比对相等 → 复用对象、不动 version → 行 Picture 缓存命中。
class _PooledLine {
  final BufferLine line;
  final List<FrameRun> runs;
  final bool wrapped;
  _PooledLine(this.line, this.runs, this.wrapped);
}

/// `TerminalSurface` 的帧驱动实现（PLAN §6.1 M2 fork 决定，方案 A）。
///
/// Rust 每帧推来的压缩 run 解码后填进池化的 `BufferLine`：
/// * 相邻同属性单元格的合并已在 Rust 侧完成，这里只负责「展开」回格子；
/// * 行内容逐 run 比对，相同则复用同一对象、不碰 `version`——
///   fork 的行 Picture 重放靠这个命中，60fps 下的主要收益来源；
/// * 终端状态（网格/滚动/模式）全部在 Rust，这里没有任何状态机，
///   只有最新一帧的投影（铁律 4）。
///
/// 滚回：对 fork 而言缓冲的行数 = 滚回 + 屏幕（行号 = 绝对行 − 最早一行），fork 的
/// Scrollable 照常滚动；内容只有 Rust 渲染过来的那一段窗口，窗口外的行是空白占位，
/// 滚到那里时页面按滚动位置向 Rust 要新的窗口。
class FrameTerminal with ChangeNotifier implements TerminalSurface, TerminalBufferSurface {
  @override
  TerminalBufferSurface get buffer => this;

  FrameTerminal({this.onInput, this.onResize}) {
    // 首帧前也要可画：fork 的 paint 会对 height-1 做 clamp，height 为 0
    // 直接越界（上游 Terminal 永远至少一行，从没暴露过这个边界）。
    _buffer.push(BufferLine(_cols));
  }

  /// 键盘/IME/粘贴的出口。返回值表示事件是否被接受。
  bool Function(TerminalInputEvent event)? onInput;

  /// 视口几何变化，由 fork 的 render 在布局期回调（只在变化时）。
  void Function(TerminalGeometry geometry)? onResize;

  /// 窗口里的行（按帧里的顺序）。fork 的 CellAnchor 挂在 BufferLine 对象上，
  /// 锚点的行号 = owner.index，而 index 只有经 IndexAwareCircularBuffer 收养
  /// （attached）才有效。容量固定富余（1024 行，够三屏的滚回窗口）；
  /// _length 只能靠 push 长大，帧变高时补齐、变矮时留尾巴（不再读到）。
  final IndexAwareCircularBuffer<BufferLine> _buffer =
      IndexAwareCircularBuffer<BufferLine>(1024);

  /// 上一帧的行对象表：key 是帧里的 `stable_row`（引擎的绝对行号）。
  /// 用行身份而不是屏幕位置，内容滚动时对象跟着内容走。
  final Map<int, _PooledLine> _pool = {};

  /// 滚回范围与窗口（绝对行）：最早一行、屏幕首行、窗口首行与行数。
  int _firstStable = 0;
  int _screenTopStable = 0;
  int _windowTopStable = 0;
  int _windowLength = 0;

  /// 上一帧以来最早一行后移的行数（滚回满了裁掉旧行、清空滚回…）。
  /// 停在滚回里时，页面据此把滚动位置上移同样的行数，内容才不会在眼前漂走。
  int _originShift = 0;
  int get originShift => _originShift;

  /// 窗口外的行：空白占位（按列宽缓存一份）。
  BufferLine? _placeholder;

  String _title = '';

  /// 远端设置的窗口标题（OSC 0 / 2）。
  String get title => _title;

  bool _mouseReporting = false;
  bool _alternateScreen = false;

  /// 远端鼠标上报（随帧下发）。开启后触摸点击/滚轮转发远端。
  set mouseReporting(bool value) {
    if (_mouseReporting == value) return;
    _mouseReporting = value;
    notifyListeners();
  }

  /// 远端备用屏（随帧下发）。fork 的滚动归属判定要用。
  set alternateScreen(bool value) {
    if (_alternateScreen == value) return;
    _alternateScreen = value;
    notifyListeners();
  }

  int _cols = 80;
  int _rows = 24;
  /// fork 看到的行数：滚回 + 屏幕。与 `_buffer.length`（窗口的物理行数，帧变矮时
  /// 旧行还挂在尾部）无关。
  int _height = 1;
  int _cursorX = 0;
  int _cursorY = 0;
  bool _cursorVisible = false;
  TerminalGeometry? _measured;

  /// 最近一次布局量得的几何；尚未布局过为 null。
  /// 重连时视图尺寸往往没变、fork 不会再回调 [resize]，连接请求要靠它带上几何。
  TerminalGeometry? get measuredGeometry => _measured;

  // ── 帧摄入 ──

  void applyFrame(TerminalFrame frame) {
    if (frame.cols != _cols) _placeholder = null;
    _cols = frame.cols;
    _rows = frame.rows;
    _title = frame.title;
    _originShift = frame.firstStableRow - _firstStable;
    _firstStable = frame.firstStableRow;
    _screenTopStable = frame.screenTopStableRow;
    final screenIndex = _screenTopStable - _firstStable;
    _cursorX = frame.cursorCol.clamp(0, frame.cols - 1);
    _cursorY = screenIndex + frame.cursorRow.clamp(0, frame.rows - 1);
    _cursorVisible = frame.cursorCol >= 0 && frame.cursorRow >= 0;

    // 行池：key = stable_row。同一行的内容没变就复用同一对象（Picture
    // 缓存与选区锚点都靠对象身份）；内容变了才换新对象。
    final next = <int, _PooledLine>{};
    final lines = <BufferLine>[];
    for (final row in frame.lines) {
      final pooled = _reusable(row) ??
          _PooledLine(_buildLine(row), row.runs, row.wrapped);
      next[row.stableRow] = pooled;
      lines.add(pooled.line);
    }
    _pool
      ..clear()
      ..addAll(next);

    // 行池按位置铺开：新行补进缓冲、位置上的旧对象换成新对象。选区不再挂
    // 锚点（权威在引擎），所以不做锚点迁移。帧里的行是连续的绝对行。
    _windowTopStable = frame.lines.isEmpty ? _screenTopStable : frame.lines.first.stableRow;
    _windowLength = lines.length;
    while (_buffer.length < lines.length) {
      _buffer.push(lines[_buffer.length]);
    }
    for (var i = 0; i < lines.length; i++) {
      _buffer[i] = lines[i];
    }
    _height = (screenIndex + _rows).clamp(1, 1 << 40);
    notifyListeners();
  }

  /// 最早一行的绝对行号（行号 0 对应它）。
  int get firstStableRow => _firstStable;

  /// 屏幕首行的行号（滚到底时它在视口顶端）。
  int get screenTopIndex => _screenTopStable - _firstStable;

  /// 行号 → 引擎绝对行。越界返回 null。
  int? stableRowAt(int index) {
    if (index < 0 || index >= _height) return null;
    return _firstStable + index;
  }

  /// 引擎绝对行 → 行号。不在滚回范围内返回 null。
  int? indexOfStable(int stableRow) {
    final index = stableRow - _firstStable;
    if (index < 0 || index >= _height) return null;
    return index;
  }

  /// 窗口是否盖住了行号 [first] 起的 [count] 行。
  bool windowCovers(int first, int count) {
    final windowFirst = _windowTopStable - _firstStable;
    return first >= windowFirst && first + count <= windowFirst + _windowLength;
  }

  /// 该 stable_row 上一帧的行对象在内容仍相同时可以复用。
  _PooledLine? _reusable(FrameRow row) {
    final pooled = _pool[row.stableRow];
    if (pooled == null) return null;
    if (pooled.wrapped != row.wrapped) return null;
    if (!_runsEqual(pooled.runs, row.runs)) return null;
    return pooled;
  }

  static bool _runsEqual(List<FrameRun> a, List<FrameRun> b) {
    if (a.length != b.length) return false;
    for (var i = 0; i < a.length; i++) {
      final x = a[i];
      final y = b[i];
      if (x.start != y.start ||
          x.len != y.len ||
          x.attrs != y.attrs ||
          x.text != y.text ||
          !listEquals(x.layout, y.layout) ||
          x.fg.runtimeType != y.fg.runtimeType ||
          x.bg.runtimeType != y.bg.runtimeType) {
        return false;
      }
      if (x.fg is AnsiColor && (x.fg as AnsiColor).index != (y.fg as AnsiColor).index) {
        return false;
      }
      if (x.bg is AnsiColor && (x.bg as AnsiColor).index != (y.bg as AnsiColor).index) {
        return false;
      }
      if (x.fg is RgbColor &&
          ((x.fg as RgbColor).r != (y.fg as RgbColor).r ||
              (x.fg as RgbColor).g != (y.fg as RgbColor).g ||
              (x.fg as RgbColor).b != (y.fg as RgbColor).b)) {
        return false;
      }
      if (x.bg is RgbColor &&
          ((x.bg as RgbColor).r != (y.bg as RgbColor).r ||
              (x.bg as RgbColor).g != (y.bg as RgbColor).g ||
              (x.bg as RgbColor).b != (y.bg as RgbColor).b)) {
        return false;
      }
    }
    return true;
  }

  /// 把一行 run 展开回 BufferLine：每格的列与宽度都按引擎给的布局（Dart 不算字符宽度）。
  /// 宽字符占 2 列（第二列只带颜色不带字形），run 覆盖范围内没有字形的列留空白、保留颜色。
  BufferLine _buildLine(FrameRow row) {
    final line = BufferLine(_cols, isWrapped: row.wrapped);
    for (final run in row.runs) {
      final fg = _encodeColor(run.fg, foreground: true);
      final bg = _encodeColor(run.bg, foreground: false);
      final attrs = _encodeAttrs(run);
      final end = (run.start + run.len).clamp(0, _cols);
      final runes = run.text.runes.toList(growable: false);
      final layout = run.layout;

      var col = run.start;
      if (layout == null) {
        // 简单 run：一个码点一格、宽 1。
        for (final rune in runes) {
          if (col >= end) break;
          _fillCluster(line, col, [rune], 1, fg, bg, attrs);
          col += 1;
        }
      } else {
        var next = 0;
        for (final cell in layout) {
          final count = cell >> 4;
          final width = cell & 0x0f;
          if (col >= end || next + count > runes.length) break;
          _fillCluster(line, col, runes.sublist(next, next + count), width, fg, bg, attrs);
          next += count;
          col += width;
        }
      }
      // run 内剩余列：空白，但保留 run 的底色（反显/色块场景需要）。
      while (col < end) {
        _setColors(line, col, fg, bg, attrs);
        col += 1;
      }
    }
    return line;
  }

  void _fillCluster(
    BufferLine line,
    int col,
    List<int> codepoints,
    int width,
    int fg,
    int bg,
    int attrs,
  ) {
    if (col >= _cols) return;
    _setColors(line, col, fg, bg, attrs);
    line.setContent(col, codepoints.first | (width << CellContent.widthShift));
    if (codepoints.length > 1) {
      line.setCombined(col, String.fromCharCodes(codepoints.sublist(1)));
    }
    // 宽字符的第二列：只有颜色（painter 跳过 codepoint 0 的字形）。
    if (width == 2 && col + 1 < _cols) {
      _setColors(line, col + 1, fg, bg, attrs);
    }
  }

  void _setColors(BufferLine line, int col, int fg, int bg, int attrs) {
    if (col >= _cols) return;
    line.setForeground(col, fg);
    line.setBackground(col, bg);
    line.setAttributes(col, attrs);
  }

  /// 我们的 Color（Default/Ansi/Rgb）→ fork 的 CellColor 编码。
  static int _encodeColor(TermColor color, {required bool foreground}) {
    switch (color) {
      case DefaultColor():
        return CellColor.normal;
      case AnsiColor(:final index):
        return index < 16
            ? (CellColor.named | index)
            : (CellColor.palette | index);
      case RgbColor(:final r, :final g, :final b):
        return CellColor.rgb | (r << 16) | (g << 8) | b;
    }
  }

  static int _encodeAttrs(FrameRun run) {
    var flags = 0;
    if (run.bold) flags |= CellFlags.bold;
    if (run.italic) flags |= CellFlags.italic;
    if (run.underline) flags |= CellFlags.underline;
    if (run.strike) flags |= CellFlags.strikethrough;
    if (run.reverse) flags |= CellFlags.inverse;
    return flags;
  }

  // ── TerminalBufferSurface（adapter 自身兼任）──

  @override
  int get height => _height;

  @override
  BufferLine lineAt(int index) {
    final offset = _firstStable + index - _windowTopStable;
    if (offset >= 0 && offset < _windowLength) return _buffer[offset];
    return _placeholder ??= BufferLine(_cols);
  }

  @override
  int get cursorX => _cursorX;

  /// 光标所在行号（屏幕首行的行号 + 光标在屏幕上的行）。
  @override
  int get absoluteCursorY => _cursorY;

  /// 切词（长按选词的边界规则）：与 fork 的 Buffer 同一套分隔符
  /// （NUL/空白/`.`/`:`/`-`/`\`/`"`/`*`/`+`/`/`）。CJK 连续段成一个词。
  static const Set<int> _wordSeparators = {
    0,
    0x20,
    0x2e,
    0x3a,
    0x2d,
    0x5c,
    0x22,
    0x2a,
    0x2b,
    0x2f,
  };

  @override
  BufferRangeLine? getWordBoundary(CellOffset position) {
    if (position.y < 0 || position.y >= _height) return null;
    final line = lineAt(position.y);
    var start = position.x;
    var end = position.x;
    while (start > 0 && !_wordSeparators.contains(line.getCodePoint(start - 1))) {
      start--;
    }
    while (end < _cols && !_wordSeparators.contains(line.getCodePoint(end))) {
      end++;
    }
    if (start == end) return null;
    return BufferRangeLine(CellOffset(start, position.y), CellOffset(end, position.y));
  }

  @override
  CellAnchor createAnchor(int x, int y) {
    final line = lineAt(y.clamp(0, _height - 1));
    return line.createAnchor(x.clamp(0, _cols - 1));
  }

  @override
  CellAnchor createAnchorFromOffset(CellOffset offset) =>
      createAnchor(offset.x, offset.y);

  @override
  String getText([BufferRange? range]) {
    range ??= BufferRangeLine(
      CellOffset(0, 0),
      CellOffset(_cols - 1, _height - 1),
    );
    range = range.normalized;
    final builder = StringBuffer();
    for (final segment in range.toSegments()) {
      if (segment.line < 0 || segment.line >= _height) continue;
      final line = lineAt(segment.line);
      // 换行规则与 fork 的 Buffer.getText 一致：起始行/首行/wrapped
      // 行前不插换行（软换行的物理行拼回一段）。
      if (!(segment.line == range.begin.y || segment.line == 0 || line.isWrapped)) {
        builder.write('\n');
      }
      // 行尾空格要裁掉：上游在 wire 上把空白格发成**字面空格**
      // （rshell-session render.rs 的 blank_cell 用 " "），fork 的
      // 裁尾逻辑只认内容 0，不裁的话每行都拖着一串到行宽的空格。
      builder.write(line.getText(segment.start, segment.end).trimRight());
    }
    return builder.toString();
  }

  // ── TerminalSurface ──

  @override
  int get viewWidth => _cols;

  @override
  int get viewHeight => _rows;

  @override
  bool get cursorVisibleMode => _cursorVisible;

  final CursorStyle _cursorStyle = CursorStyle();

  /// 引擎的光标样式随帧走（FrameUpdate 暂未携带），M2 固定块状。
  @override
  CursorStyle get cursor => _cursorStyle;

  @override
  MouseMode get mouseMode =>
      _mouseReporting ? MouseMode.clickOnly : MouseMode.none;

  @override
  bool get isUsingAltBuffer => _alternateScreen;

  @override
  void resize(int newWidth, int newHeight, [int? pixelWidth, int? pixelHeight]) {
    // fork 的 render 传来的是**单格**像素；SSH 的 pty-req / window-change
    // 要整个终端的像素尺寸，在这里换算。
    final geometry = TerminalGeometry(
      cols: newWidth,
      rows: newHeight,
      pixelWidth: newWidth * (pixelWidth ?? 0),
      pixelHeight: newHeight * (pixelHeight ?? 0),
    );
    // 布局期每帧都会调用；只在几何真正变化时转发，否则每帧一次 window-change。
    if (geometry == _measured) return;
    _measured = geometry;
    onResize?.call(geometry);
  }

  // ── 修饰键挂住/锁定（Termux 式，键位条与软键盘输入共享状态）──
  // 状态必须放在适配器里：软键盘的字母经 TerminalView → onInsert →
  // keyInput 进来，不经过键位条——挂在键位条上的 Ctrl 只有在这里
  // 消费才能作用到任何来源的按键。
  static const String _modCtrl = 'ctrl';
  static const String _modAlt = 'alt';
  final Set<String> _latchedModifiers = {};
  final Set<String> _lockedModifiers = {};

  /// 点击修饰键：未挂 → 挂住一次；挂住 → 锁定；锁定 → 解除。
  void tapModifier(String modifier) {
    if (_lockedModifiers.remove(modifier)) {
      _latchedModifiers.remove(modifier);
    } else if (_latchedModifiers.remove(modifier)) {
      _lockedModifiers.add(modifier);
    } else {
      _latchedModifiers.add(modifier);
    }
    notifyListeners();
  }

  /// 长按修饰键：直接锁定。
  void lockModifier(String modifier) {
    _latchedModifiers.remove(modifier);
    _lockedModifiers.add(modifier);
    notifyListeners();
  }

  bool isModifierLatched(String modifier) => _latchedModifiers.contains(modifier);
  bool isModifierLocked(String modifier) => _lockedModifiers.contains(modifier);

  /// 离开键位条去编辑配置时解除挂住状态，避免移除按钮后修饰键仍生效。
  void clearModifiers() {
    if (_latchedModifiers.isEmpty && _lockedModifiers.isEmpty) return;
    _latchedModifiers.clear();
    _lockedModifiers.clear();
    notifyListeners();
  }

  /// 下一个输入是否带着该修饰键；挂住态在此消耗（锁定态保留）。
  bool _consumeModifier(String modifier) {
    if (_lockedModifiers.contains(modifier)) return true;
    final had = _latchedModifiers.remove(modifier);
    if (had) notifyListeners();
    return had;
  }

  @override
  bool keyInput(
    TerminalKey key, {
    bool shift = false,
    bool alt = false,
    bool ctrl = false,
  }) {
    var name = keyName(key);
    if (name == null) return false;
    // 可打印字符（字母/数字/标点/空格）不带硬件 Ctrl/Alt 时交回文本通道：
    // fork 的软键盘路径先把单字符映射成键再试 keyInput，而 'A' 与 'a'
    // 映射到同一个 keyA——这里拿不到大小写。返回 false 后 fork 改走
    // textInput(原字符)，大小写原样保留，挂住的 Ctrl/Alt 也在那里消费。
    if (name.startsWith(_characterPrefix) && !ctrl && !alt) return false;
    // 带 Ctrl/Alt 的 Shift+数字 / 标点：键只给出基准字符，按 US 布局换成上档字符
    // （Ctrl+_ = 0x1f、Ctrl+^ = 0x1e、Alt+> = ESC >）。
    if (shift && name.startsWith(_characterPrefix)) {
      final shifted = _usShifted[name.substring(_characterPrefix.length)];
      if (shifted != null) name = '$_characterPrefix$shifted';
    }
    return _emit(
      KeyInputEvent(
        name,
        shift: shift,
        control: ctrl || _consumeModifier(_modCtrl),
        alt: alt || _consumeModifier(_modAlt),
      ),
    );
  }

  @override
  void textInput(String text) {
    // 挂住的 Ctrl/Alt 作用于下一个输入；单个字符时升级成 Key 事件
    // （软键盘 C + 挂住 Ctrl = Ctrl+C，Rust 侧编码成 ETX）。
    // 多字符文本不消耗挂住态——那是输入法的整段提交，不是「下一个按键」。
    if (text.runes.length == 1) {
      final ctrl = _consumeModifier(_modCtrl);
      final alt = _consumeModifier(_modAlt);
      if (ctrl || alt) {
        _emit(KeyInputEvent('character:$text', control: ctrl, alt: alt));
        return;
      }
    }
    _emit(TextInputEvent(text));
  }

  @override
  void paste(String text) {
    _emit(PasteInputEvent(text));
  }

  @override
  bool mouseInput(
    TerminalMouseButton button,
    TerminalMouseButtonState buttonState,
    CellOffset position, {
    bool shift = false,
    bool alt = false,
    bool ctrl = false,
  }) {
    // 鼠标上报没开：不消费，fork 回退到本地行为（聚焦/选区/滚动/菜单）。
    if (!_mouseReporting) return false;
    _emit(MouseInputEvent(
      button,
      switch (buttonState) {
        TerminalMouseButtonState.down => MouseAction.press,
        TerminalMouseButtonState.up => MouseAction.release,
      },
      _clampToGrid(position),
      shift: shift,
      alt: alt,
      ctrl: ctrl,
    ));
    return true;
  }

  @override
  bool mouseMotion(
    TerminalMouseButton? button,
    CellOffset position, {
    bool shift = false,
    bool alt = false,
    bool ctrl = false,
  }) {
    if (!_mouseReporting) return false;
    _emit(MouseInputEvent(
      button,
      MouseAction.move,
      _clampToGrid(position),
      shift: shift,
      alt: alt,
      ctrl: ctrl,
    ));
    return true;
  }

  CellOffset _clampToGrid(CellOffset position) =>
      CellOffset(position.x.clamp(0, _cols - 1), position.y.clamp(0, _rows - 1));

  bool _emit(TerminalInputEvent event) {
    final callback = onInput;
    if (callback == null) return false;
    return callback(event);
  }

  static const String _characterPrefix = 'character:';

  /// TerminalKey → 边界键名（见 session.rs 的 parse_key_code）。
  /// 字母/数字只在带 Ctrl/Alt 时以键的形式出现（见 [keyInput]），所以
  /// 这里的字母一律小写——大小写不影响 Ctrl/Alt 组合的编码。
  @visibleForTesting
  static String? keyName(TerminalKey key) {
    final named = _namedKeyNames[key];
    if (named != null) return named;
    final fKey = _fKeyNames[key];
    if (fKey != null) return fKey;
    // 字母段在枚举里连续（keyA..keyZ），直接按序号换算。
    if (key.index >= TerminalKey.keyA.index &&
        key.index <= TerminalKey.keyZ.index) {
      return '$_characterPrefix${String.fromCharCode(0x61 + key.index - TerminalKey.keyA.index)}';
    }
    // 数字段按 HID 用法码顺序：digit1..digit9 连续，digit0 排在最后。
    if (key.index >= TerminalKey.digit1.index &&
        key.index <= TerminalKey.digit9.index) {
      return '$_characterPrefix${String.fromCharCode(0x31 + key.index - TerminalKey.digit1.index)}';
    }
    if (key == TerminalKey.digit0) return '${_characterPrefix}0';
    final punctuation = _punctuation[key];
    if (punctuation != null) return '$_characterPrefix$punctuation';
    return null;
  }

  /// 标点键：US 布局的基准字符（与数字、字母一样只在带 Ctrl/Alt 时以键的形式发出）。
  static const Map<TerminalKey, String> _punctuation = {
    TerminalKey.minus: '-',
    TerminalKey.equal: '=',
    TerminalKey.bracketLeft: '[',
    TerminalKey.bracketRight: ']',
    TerminalKey.backslash: r'\',
    TerminalKey.semicolon: ';',
    TerminalKey.quote: "'",
    TerminalKey.backquote: '`',
    TerminalKey.comma: ',',
    TerminalKey.period: '.',
    TerminalKey.slash: '/',
  };

  /// US 布局下数字与标点的上档字符。
  static const Map<String, String> _usShifted = {
    '1': '!', '2': '@', '3': '#', '4': r'$', '5': '%', '6': '^', '7': '&', '8': '*', '9': '(', '0': ')',
    '-': '_', '=': '+', '[': '{', ']': '}', r'\': '|', ';': ':', "'": '"', '`': '~', ',': '<', '.': '>',
    '/': '?',
  };


  static const Map<TerminalKey, String> _namedKeyNames = {
    TerminalKey.enter: 'enter',
    TerminalKey.escape: 'escape',
    TerminalKey.tab: 'tab',
    TerminalKey.backspace: 'backspace',
    TerminalKey.delete: 'delete',
    TerminalKey.insert: 'insert',
    TerminalKey.home: 'home',
    TerminalKey.end: 'end',
    TerminalKey.pageUp: 'page_up',
    TerminalKey.pageDown: 'page_down',
    TerminalKey.arrowUp: 'arrow_up',
    TerminalKey.arrowDown: 'arrow_down',
    TerminalKey.arrowLeft: 'arrow_left',
    TerminalKey.arrowRight: 'arrow_right',
    TerminalKey.space: 'character: ',
  };

  static const Map<TerminalKey, String> _fKeyNames = {
    TerminalKey.f1: 'f1',
    TerminalKey.f2: 'f2',
    TerminalKey.f3: 'f3',
    TerminalKey.f4: 'f4',
    TerminalKey.f5: 'f5',
    TerminalKey.f6: 'f6',
    TerminalKey.f7: 'f7',
    TerminalKey.f8: 'f8',
    TerminalKey.f9: 'f9',
    TerminalKey.f10: 'f10',
    TerminalKey.f11: 'f11',
    TerminalKey.f12: 'f12',
  };
}
