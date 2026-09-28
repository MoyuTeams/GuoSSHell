import 'package:flutter/foundation.dart';

/// App 的前后台。进后台后连接可能被系统挂起或冻结（iOS 回收挂起 App 的 socket，Android
/// 冻结进程），窗格据此判断一次断开是不是后台造成的，回到前台时自动重连。
class AppVisibility extends ChangeNotifier {
  AppVisibility({DateTime Function()? clock}) : _clock = clock ?? DateTime.now;

  static final AppVisibility instance = AppVisibility();

  /// 回到前台后这段时间内报出的断开也算后台造成的：被回收的 socket 要等进程恢复运行才报错，
  /// 冻结期间悄悄没了的连接靠 keepalive 发现（15 秒空闲后探测，3 次无回音，最长约一分钟）。
  static const resumeWindow = Duration(seconds: 75);

  final DateTime Function() _clock;
  bool _foreground = true;
  DateTime? _shownAt;
  int _resumes = 0;

  bool get foreground => _foreground;

  /// 回到前台的次数。每次回到前台，每个窗格至多自动重连一次。
  int get resumes => _resumes;

  /// 此刻报出的断开是否可归因于进后台：还在后台，或刚回到前台不久。
  bool get lossFromBackground {
    if (!_foreground) return true;
    final shownAt = _shownAt;
    return shownAt != null && _clock().difference(shownAt) < resumeWindow;
  }

  void update({required bool foreground}) {
    if (foreground == _foreground) return;
    _foreground = foreground;
    if (foreground) {
      _shownAt = _clock();
      _resumes++;
    }
    notifyListeners();
  }
}

/// 一个窗格的自动重连判断：连接在后台（或刚回前台时）断开，回到前台就重连一次；
/// 重连失败留给用户手动重试，不会反复重连。
class ResumeReconnect {
  ResumeReconnect(this.visibility);

  final AppVisibility visibility;

  /// 在后台断开，等回到前台再重连。
  bool _pending = false;

  /// 已经为哪一次回到前台自动重连过。
  int? _usedFor;

  /// 连接断了（连上之后的网络中断或 keepalive 超时）。返回是否立刻重连；在后台时记下，
  /// 等 [onShown]。
  bool onLost() {
    if (!visibility.lossFromBackground) return false;
    if (!visibility.foreground) {
      _pending = true;
      return false;
    }
    return _take();
  }

  /// 回到前台。返回是否重连。
  bool onShown() {
    if (!_pending || !visibility.foreground) return false;
    _pending = false;
    return _take();
  }

  /// 用户自己重连或会话换了结局：不再等回前台重连。
  void cancel() => _pending = false;

  bool _take() {
    if (_usedFor == visibility.resumes) return false;
    _usedFor = visibility.resumes;
    return true;
  }
}
