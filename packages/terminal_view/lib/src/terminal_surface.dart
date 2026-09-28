import 'package:terminal_view/src/core/buffer/cell_offset.dart';
import 'package:terminal_view/src/core/buffer/line.dart';
import 'package:terminal_view/src/core/buffer/range.dart';
import 'package:terminal_view/src/core/buffer/range_line.dart';
import 'package:terminal_view/src/core/cursor.dart';
import 'package:terminal_view/src/core/input/keys.dart';
import 'package:terminal_view/src/core/mouse/button.dart';
import 'package:terminal_view/src/core/mouse/button_state.dart';
import 'package:terminal_view/src/core/mouse/mode.dart';

/// The slice of a terminal's buffer that the rendering layer reads.
///
/// Implemented by the full [Terminal] state machine, and can be implemented
/// by other backends that keep their state elsewhere - for example a remote
/// terminal whose grid lives on another machine or process. Line [version]
/// numbers on [BufferLine] drive the line picture cache; a backend that
/// refills lines in place must keep unchanged lines bit-identical (or at
/// least object-identical with an untouched version) for caching to pay off.
abstract class TerminalBufferSurface {
  /// The total number of lines, including everything above the viewport.
  int get height;

  /// The line at [index], from the top of the scrollback.
  BufferLine lineAt(int index);

  int get cursorX;

  int get absoluteCursorY;

  CellAnchor createAnchor(int x, int y);

  CellAnchor createAnchorFromOffset(CellOffset offset);

  BufferRangeLine? getWordBoundary(CellOffset position);

  String getText([BufferRange? range]);
}

/// Everything [RenderTerminal] and [TerminalView] need from a terminal.
///
/// This is deliberately narrower than the [Terminal] state machine: a backend
/// only has to answer questions about what to draw and where the cursor is,
/// and to forward input. It does not parse anything.
abstract class TerminalSurface {
  TerminalBufferSurface get buffer;

  int get viewWidth;

  int get viewHeight;

  bool get cursorVisibleMode;

  CursorStyle get cursor;

  MouseMode get mouseMode;

  bool get isUsingAltBuffer;

  void resize(int newWidth, int newHeight, [int? pixelWidth, int? pixelHeight]);

  bool keyInput(
    TerminalKey key, {
    bool shift = false,
    bool alt = false,
    bool ctrl = false,
  });

  void textInput(String text);

  void paste(String text);

  /// A mouse [button] went [buttonState] at [position]. Returns whether the
  /// terminal consumed the event (the application tracks the mouse).
  bool mouseInput(
    TerminalMouseButton button,
    TerminalMouseButtonState buttonState,
    CellOffset position, {
    bool shift = false,
    bool alt = false,
    bool ctrl = false,
  });

  /// A mouse pointer moved to [position], dragging with [button] held, or
  /// hovering when [button] is null. Returns whether the terminal consumed
  /// the event.
  bool mouseMotion(
    TerminalMouseButton? button,
    CellOffset position, {
    bool shift = false,
    bool alt = false,
    bool ctrl = false,
  });

  void addListener(void Function() listener);

  void removeListener(void Function() listener);
}
