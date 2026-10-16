import 'package:flutter/gestures.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

/// 依据当前输入设备切换选区交互，不以操作系统或连接了哪些硬件推断。
class TerminalInputMode extends ChangeNotifier {
  /// 一个工作区内的窗格共享输入方式，改用鼠标时同时收起触屏控件。
  static final shared = TerminalInputMode();
  bool _touch = false;
  bool _bound = false;
  bool get touch => _touch;

  /// 仅观察输入方式，不消费事件，也不维护或修改修饰键状态。
  void bind() {
    if (_bound) return;
    _bound = true;
    GestureBinding.instance.pointerRouter.addGlobalRoute(_onPointer);
    HardwareKeyboard.instance.addHandler(_onKey);
  }

  void _onPointer(PointerEvent event) {
    if (event.synthesized) return;
    if (event is PointerDownEvent ||
        event is PointerSignalEvent ||
        event is PointerPanZoomStartEvent ||
        (event is PointerHoverEvent && event.kind == PointerDeviceKind.mouse)) {
      pointer(event.kind);
    }
  }

  bool _onKey(KeyEvent event) {
    if (!event.synthesized &&
        (event is KeyDownEvent || event is KeyRepeatEvent)) {
      keyboard();
    }
    return false;
  }

  void pointer(PointerDeviceKind kind) => _set(
    kind == PointerDeviceKind.touch ||
        kind == PointerDeviceKind.stylus ||
        kind == PointerDeviceKind.invertedStylus,
  );
  void keyboard() => _set(false);
  void _set(bool touch) {
    if (_touch == touch) return;
    _touch = touch;
    notifyListeners();
  }

  @override
  void dispose() {
    if (_bound) {
      GestureBinding.instance.pointerRouter.removeGlobalRoute(_onPointer);
      HardwareKeyboard.instance.removeHandler(_onKey);
    }
    super.dispose();
  }
}
