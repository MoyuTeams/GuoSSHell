import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:terminal_view/src/core/mouse/button.dart';
import 'package:terminal_view/src/core/mouse/button_state.dart';
import 'package:terminal_view/src/terminal_view.dart';
import 'package:terminal_view/src/ui/controller.dart';
import 'package:terminal_view/src/ui/gesture/gesture_detector.dart';
import 'package:terminal_view/src/ui/pointer_input.dart';
import 'package:terminal_view/src/ui/render.dart';

class TerminalGestureHandler extends StatefulWidget {
  const TerminalGestureHandler({
    super.key,
    required this.terminalView,
    required this.terminalController,
    this.child,
    this.onTapUp,
    this.onSingleTapUp,
    this.onTapDown,
    this.onSecondaryTapDown,
    this.onSecondaryTapUp,
    this.onTertiaryTapDown,
    this.onTertiaryTapUp,
    this.readOnly = false,
  });

  final TerminalViewState terminalView;

  final TerminalController terminalController;

  final Widget? child;

  final GestureTapUpCallback? onTapUp;

  final GestureTapUpCallback? onSingleTapUp;

  final GestureTapDownCallback? onTapDown;

  final GestureTapDownCallback? onSecondaryTapDown;

  final GestureTapUpCallback? onSecondaryTapUp;

  final GestureTapDownCallback? onTertiaryTapDown;

  final GestureTapUpCallback? onTertiaryTapUp;

  final bool readOnly;

  @override
  State<TerminalGestureHandler> createState() => _TerminalGestureHandlerState();
}

class _TerminalGestureHandlerState extends State<TerminalGestureHandler> {
  TerminalViewState get terminalView => widget.terminalView;

  RenderTerminal get renderTerminal => terminalView.renderTerminal;

  DragStartDetails? _lastDragStartDetails;

  LongPressStartDetails? _lastLongPressStartDetails;

  /// The mouse pointer whose press went to the terminal, and the button it
  /// pressed: its moves are reported as drags and its release ends them.
  int? _mousePointer;

  TerminalMouseButton? _mouseButton;

  /// Whether the last mouse press went to the terminal. Mouse presses are
  /// reported by the [Listener] below; the gesture recognizers see the same
  /// press after it and must neither report it again nor treat it as their
  /// own tap, double tap or selection drag.
  bool _mousePressReported = false;

  @override
  Widget build(BuildContext context) {
    return Listener(
      onPointerDown: _onPointerDown,
      onPointerMove: _onPointerMove,
      onPointerUp: _onPointerUp,
      onPointerCancel: _onPointerCancel,
      onPointerHover: _onPointerHover,
      child: _buildGestureDetector(),
    );
  }

  Widget _buildGestureDetector() {
    return TerminalGestureDetector(
      child: widget.child,
      onTapUp: widget.onTapUp,
      onSingleTapUp: onSingleTapUp,
      onTapDown: onTapDown,
      onSecondaryTapDown: onSecondaryTapDown,
      onSecondaryTapUp: onSecondaryTapUp,
      onTertiaryTapDown: onTertiaryTapDown,
      onTertiaryTapUp: onTertiaryTapUp,
      onLongPressStart: onLongPressStart,
      onLongPressMoveUpdate: onLongPressMoveUpdate,
      // onLongPressUp: onLongPressUp,
      onDragStart: onDragStart,
      onDragUpdate: onDragUpdate,
      onDoubleTapDown: onDoubleTapDown,
    );
  }

  bool get _shouldSendTapEvent =>
      !widget.readOnly &&
      widget.terminalController.shouldSendPointerInput(PointerInput.tap);

  static TerminalMouseButton? _mouseButtonOf(int buttons) {
    if (buttons & kPrimaryMouseButton != 0) return TerminalMouseButton.left;
    if (buttons & kSecondaryMouseButton != 0) return TerminalMouseButton.right;
    if (buttons & kMiddleMouseButton != 0) return TerminalMouseButton.middle;
    return null;
  }

  void _onPointerDown(PointerDownEvent event) {
    if (event.kind != PointerDeviceKind.mouse) return;
    final button = _mouseButtonOf(event.buttons);
    final keyboard = HardwareKeyboard.instance;
    // Shift keeps the mouse for local selection even while the application
    // tracks it, as in other terminals.
    _mousePressReported = button != null &&
        _shouldSendTapEvent &&
        !keyboard.isShiftPressed &&
        renderTerminal.mouseEvent(
          button,
          TerminalMouseButtonState.down,
          event.localPosition,
          alt: keyboard.isAltPressed,
          ctrl: keyboard.isControlPressed,
        );
    if (_mousePressReported) {
      _mousePointer = event.pointer;
      _mouseButton = button;
    }
  }

  void _onPointerMove(PointerMoveEvent event) {
    if (event.pointer != _mousePointer ||
        !widget.terminalController.shouldSendPointerInput(PointerInput.drag)) {
      return;
    }
    final keyboard = HardwareKeyboard.instance;
    renderTerminal.mouseMotion(
      _mouseButton,
      event.localPosition,
      shift: keyboard.isShiftPressed,
      alt: keyboard.isAltPressed,
      ctrl: keyboard.isControlPressed,
    );
  }

  void _onPointerUp(PointerUpEvent event) {
    if (event.pointer == _mousePointer) _releaseMouse(event.localPosition);
  }

  void _onPointerCancel(PointerCancelEvent event) {
    if (event.pointer == _mousePointer) _releaseMouse(event.localPosition);
  }

  void _releaseMouse(Offset position) {
    final button = _mouseButton!;
    _mousePointer = null;
    _mouseButton = null;
    final keyboard = HardwareKeyboard.instance;
    renderTerminal.mouseEvent(
      button,
      TerminalMouseButtonState.up,
      position,
      shift: keyboard.isShiftPressed,
      alt: keyboard.isAltPressed,
      ctrl: keyboard.isControlPressed,
    );
  }

  void _onPointerHover(PointerHoverEvent event) {
    if (event.kind != PointerDeviceKind.mouse ||
        widget.readOnly ||
        !widget.terminalController.shouldSendPointerInput(PointerInput.move)) {
      return;
    }
    final keyboard = HardwareKeyboard.instance;
    renderTerminal.mouseMotion(
      null,
      event.localPosition,
      shift: keyboard.isShiftPressed,
      alt: keyboard.isAltPressed,
      ctrl: keyboard.isControlPressed,
    );
  }

  void _tapDown(
    GestureTapDownCallback? callback,
    TapDownDetails details,
    TerminalMouseButton button, {
    bool forceCallback = false,
  }) {
    // Check if the terminal should and can handle the tap down event.
    var handled = false;
    if (details.kind == PointerDeviceKind.mouse) {
      handled = _mousePressReported;
    } else if (_shouldSendTapEvent) {
      final keyboard = HardwareKeyboard.instance;
      handled = renderTerminal.mouseEvent(
        button,
        TerminalMouseButtonState.down,
        details.localPosition,
        shift: keyboard.isShiftPressed,
        alt: keyboard.isAltPressed,
        ctrl: keyboard.isControlPressed,
      );
    }
    // If the event was not handled by the terminal, use the supplied callback.
    if (!handled || forceCallback) {
      callback?.call(details);
    }
  }

  void _tapUp(
    GestureTapUpCallback? callback,
    TapUpDetails details,
    TerminalMouseButton button, {
    bool forceCallback = false,
  }) {
    // Check if the terminal should and can handle the tap up event.
    var handled = false;
    if (details.kind == PointerDeviceKind.mouse) {
      handled = _mousePressReported;
    } else if (_shouldSendTapEvent) {
      final keyboard = HardwareKeyboard.instance;
      handled = renderTerminal.mouseEvent(
        button,
        TerminalMouseButtonState.up,
        details.localPosition,
        shift: keyboard.isShiftPressed,
        alt: keyboard.isAltPressed,
        ctrl: keyboard.isControlPressed,
      );
    }
    // If the event was not handled by the terminal, use the supplied callback.
    if (!handled || forceCallback) {
      callback?.call(details);
    }
  }

  void onTapDown(TapDownDetails details) {
    // onTapDown is special, as it will always call the supplied callback.
    // The TerminalView depends on it to bring the terminal into focus.
    _tapDown(
      widget.onTapDown,
      details,
      TerminalMouseButton.left,
      forceCallback: true,
    );
  }

  void onSingleTapUp(TapUpDetails details) {
    // Also forced, to match onTapDown above: an app that turned on mouse
    // reporting (most full-screen TUIs do) would otherwise swallow every
    // tap as a mouse click, and the terminal would never bring itself into
    // focus - the click still reaches the app either way.
    _tapUp(
      widget.onSingleTapUp,
      details,
      TerminalMouseButton.left,
      forceCallback: true,
    );
  }

  void onSecondaryTapDown(TapDownDetails details) {
    _tapDown(widget.onSecondaryTapDown, details, TerminalMouseButton.right);
  }

  void onSecondaryTapUp(TapUpDetails details) {
    _tapUp(widget.onSecondaryTapUp, details, TerminalMouseButton.right);
  }

  void onTertiaryTapDown(TapDownDetails details) {
    _tapDown(widget.onTertiaryTapDown, details, TerminalMouseButton.middle);
  }

  void onTertiaryTapUp(TapUpDetails details) {
    _tapUp(widget.onTertiaryTapUp, details, TerminalMouseButton.middle);
  }

  void onDoubleTapDown(TapDownDetails details) {
    if (details.kind == PointerDeviceKind.mouse && _mousePressReported) return;
    renderTerminal.selectWord(details.localPosition);
  }

  void onLongPressStart(LongPressStartDetails details) {
    _lastLongPressStartDetails = details;
    renderTerminal.selectWord(details.localPosition);
  }

  void onLongPressMoveUpdate(LongPressMoveUpdateDetails details) {
    renderTerminal.selectWord(
      _lastLongPressStartDetails!.localPosition,
      details.localPosition,
    );
  }

  // void onLongPressUp() {}

  void onDragStart(DragStartDetails details) {
    // The drag of a press that went to the application is the application's.
    if (_mousePressReported) {
      _lastDragStartDetails = null;
      return;
    }
    _lastDragStartDetails = details;

    details.kind == PointerDeviceKind.mouse
        ? renderTerminal.selectCharacters(details.localPosition)
        : renderTerminal.selectWord(details.localPosition);
  }

  void onDragUpdate(DragUpdateDetails details) {
    final start = _lastDragStartDetails;
    if (start == null) return;
    renderTerminal.selectCharacters(start.localPosition, details.localPosition);
  }
}
