import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

/// 缩放只作用于当前窗格；默认字号保持不变，重置后回到它。
class TerminalZoom extends ChangeNotifier {
  TerminalZoom({
    required this.defaultSize,
    required this.minSize,
    required this.maxSize,
  }) : _size = defaultSize.clamp(minSize, maxSize);
  final double defaultSize;
  final double minSize;
  final double maxSize;
  double _size;
  double? _startSize;
  double get size => _size;

  void _set(double value) {
    if (!value.isFinite) return;
    final next = (value.clamp(minSize, maxSize) * 4).round() / 4;
    if (next == _size) return;
    _size = next;
    notifyListeners();
  }

  void step(int direction) => _set(_size + direction);
  void reset() => _set(defaultSize);
  void begin() => _startSize = _size;
  void scale(double factor) {
    if (_startSize != null && factor.isFinite && factor > 0) {
      _set(_startSize! * factor);
    }
  }

  void end() => _startSize = null;

  /// ⌘= 同样放大，免于要求用户额外按 Shift；支持数字键盘。
  bool handleKey(KeyEvent event, {required bool meta}) {
    if (!meta || (event is! KeyDownEvent && event is! KeyRepeatEvent)) {
      return false;
    }
    final key = event.logicalKey;
    if (key == LogicalKeyboardKey.equal ||
        key == LogicalKeyboardKey.add ||
        key == LogicalKeyboardKey.numpadAdd) {
      step(1);
    } else if (key == LogicalKeyboardKey.minus ||
        key == LogicalKeyboardKey.numpadSubtract) {
      step(-1);
    } else if (key == LogicalKeyboardKey.digit0 ||
        key == LogicalKeyboardKey.numpad0) {
      reset();
    } else {
      return false;
    }
    return true;
  }
}

class TerminalZoomSurface extends StatelessWidget {
  final TerminalZoom zoom;
  final VoidCallback onStart;
  final Widget child;
  const TerminalZoomSurface({
    super.key,
    required this.zoom,
    required this.onStart,
    required this.child,
  });

  @override
  Widget build(BuildContext context) => RawGestureDetector(
    gestures: {
      _PinchRecognizer: GestureRecognizerFactoryWithHandlers<_PinchRecognizer>(
        _PinchRecognizer.new,
        (recognizer) => recognizer
          ..onStart = () {
            onStart();
            zoom.begin();
          }
          ..onScale = zoom.scale
          ..onEnd = zoom.end,
      ),
    },
    child: child,
  );
}

/// 单指不抢滚动；第二根手指落下后才竞争捏合。触控板只有比例发生变化才竞争，
/// 两指平移仍由终端滚动处理。
class _PinchRecognizer extends OneSequenceGestureRecognizer {
  VoidCallback? onStart;
  ValueChanged<double>? onScale;
  VoidCallback? onEnd;
  final Map<int, Offset> _points = {};
  final Map<int, Offset> _origins = {};
  final Set<int> _accepted = {};
  int? _trackpad;
  double _initialDistance = 0;
  bool _started = false;

  @override
  void addAllowedPointer(PointerDownEvent event) {
    if (event.kind != PointerDeviceKind.touch ||
        _points.length >= 2 ||
        _trackpad != null) {
      return;
    }
    startTrackingPointer(event.pointer, event.transform);
    _points[event.pointer] = event.position;
    _origins[event.pointer] = event.position;
    if (_points.length == 2) {
      _initialDistance = (_points.values.first - _points.values.last).distance;
      if (_initialDistance > 0) resolve(GestureDisposition.accepted);
    }
  }

  @override
  void addAllowedPointerPanZoom(PointerPanZoomStartEvent event) {
    if (_points.isNotEmpty || _trackpad != null) return;
    _trackpad = event.pointer;
    startTrackingPointer(event.pointer, event.transform);
  }

  @override
  void handleEvent(PointerEvent event) {
    if (event.pointer == _trackpad) {
      if (event is PointerPanZoomUpdateEvent) {
        if (!_started && (event.scale - 1).abs() > 0.01) {
          resolve(GestureDisposition.accepted);
        }
        if (_started) onScale?.call(event.scale);
      } else if (event is PointerPanZoomEndEvent) {
        stopTrackingPointer(event.pointer);
      }
      return;
    }
    if (!_points.containsKey(event.pointer)) return;
    if (event is PointerMoveEvent) {
      _points[event.pointer] = event.position;
      if (_points.length == 1 &&
          !_started &&
          (event.position - _origins[event.pointer]!).distance > kTouchSlop) {
        resolve(GestureDisposition.rejected);
      } else if (_started && _points.length == 2 && _initialDistance > 0) {
        onScale?.call(
          (_points.values.first - _points.values.last).distance /
              _initialDistance,
        );
      }
    } else if (event is PointerUpEvent || event is PointerCancelEvent) {
      _finish();
      _points.remove(event.pointer);
      _origins.remove(event.pointer);
      stopTrackingPointer(event.pointer);
      if (_points.isEmpty) resolve(GestureDisposition.rejected);
    }
  }

  @override
  void acceptGesture(int pointer) {
    _accepted.add(pointer);
    if (!_started &&
        (pointer == _trackpad ||
            (_points.length == 2 && _accepted.containsAll(_points.keys)))) {
      _started = true;
      onStart?.call();
    }
  }

  @override
  void rejectGesture(int pointer) {
    _points.remove(pointer);
    _origins.remove(pointer);
    _accepted.remove(pointer);
    stopTrackingPointer(pointer);
  }

  void _finish() {
    if (_started) onEnd?.call();
    _started = false;
  }

  @override
  void didStopTrackingLastPointer(int pointer) {
    _finish();
    _points.clear();
    _origins.clear();
    _accepted.clear();
    _trackpad = null;
  }

  @override
  void dispose() {
    _finish();
    super.dispose();
  }

  @override
  String get debugDescription => '终端双指缩放';
}
