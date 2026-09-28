import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

/// Android：有会话连着（或正在连）时开一个前台服务，App 切到后台后进程不被冻结，
/// 连接得以保持；会话都结束了就停掉。其他平台什么都不做。
class SessionKeeper {
  SessionKeeper({required this.enabled, MethodChannel? channel})
    : _channel = channel ?? const MethodChannel('guosshell/session_keeper');

  static final SessionKeeper instance = SessionKeeper(
    enabled: !kIsWeb && defaultTargetPlatform == TargetPlatform.android,
  );

  final bool enabled;
  final MethodChannel _channel;
  final Set<Object> _live = {};
  int _sent = 0;

  /// 一个窗格的会话是否还需要保活。
  void report(Object owner, {required bool live}) {
    final changed = live ? _live.add(owner) : _live.remove(owner);
    if (changed) _sync();
  }

  int get count => _live.length;

  void _sync() {
    if (!enabled || _live.length == _sent) return;
    _sent = _live.length;
    _channel.invokeMethod<void>('update', _sent).catchError((Object error) {
      debugPrint('session keeper: $error');
    });
  }
}
