import 'dart:async';

import 'package:fluent_ui/fluent_ui.dart';

import '../../bindings/bindings.dart';
import '../../keys/key_requests.dart' show keyErrorText;

/// 由桌面壳持有，生命周期长于导航页；dispose 时取消 Rust 列表订阅。
/// 请求回包不是完整钥匙串快照，不能单凭回包解除凭据变更锁。
class DesktopKeySyncState extends ChangeNotifier {
  DesktopKeySyncState({Stream<KeyListState>? states, KeyListState? initial})
    : _snapshot =
          initial ??
          (states == null ? KeyListState.latestRustSignal?.message : null) {
    _subscription =
        (states ?? KeyListState.rustSignalStream.map((pack) => pack.message))
            .listen((snapshot) {
              if (_disposed) return;
              _receive(snapshot);
            });
  }

  late final StreamSubscription _subscription;
  KeyListState? _snapshot;
  bool? _target;
  bool _requestAcknowledged = false;
  bool _busy = false;
  bool _disposed = false;
  String? _statusMessage;

  KeyListState? get snapshot => _snapshot;
  bool get busy => _busy;
  bool? get pendingTarget =>
      _target ??
      (_snapshot?.syncPending == true ? _snapshot!.syncTargetEnabled : null);
  String? get statusMessage => _statusMessage;

  bool get locationUnknown {
    final snapshot = _snapshot;
    return _busy ||
        _target != null ||
        snapshot == null ||
        snapshot.listError != KeyError.none ||
        snapshot.syncPending;
  }

  bool get canMutate => !locationUnknown;

  // 正常稳定列表允许本机与云端并存；仅迁移确认才要求全部钥匙到达目标。
  static bool _allAtTarget(KeyListState snapshot, bool target) =>
      snapshot.keys.every((key) => key.synchronized == target);

  void _receive(KeyListState snapshot) {
    _snapshot = snapshot;
    final target = _target;
    if (target != null &&
        !_busy &&
        _requestAcknowledged &&
        snapshot.listError == KeyError.none &&
        !snapshot.syncPending &&
        snapshot.syncEnabled == target &&
        _allAtTarget(snapshot, target)) {
      _target = null;
      _statusMessage = '钥匙串迁移已由完整列表确认';
    }
    notifyListeners();
  }

  /// 请求仍可在页面卸载后返回，状态始终由此对象而非页面 State 接收。
  Future<void> migrate(
    bool target, {
    required Future<KeyResult> Function(bool) setSync,
    required void Function() query,
  }) async {
    if (_disposed || _busy) return;
    _target = target;
    _busy = true;
    _requestAcknowledged = false;
    _statusMessage = null;
    notifyListeners();
    try {
      final result = await setSync(target);
      if (_disposed) return;
      if (result.error == KeyError.none) {
        _requestAcknowledged = true;
        _statusMessage = '迁移请求已返回，等待完整列表确认私钥位置';
      } else {
        _statusMessage = keyErrorText(result.error);
      }
    } on TimeoutException {
      if (_disposed) return;
      _statusMessage = '同步请求尚未确认，可等待完整列表或按原目标重试';
    } catch (_) {
      if (_disposed) return;
      _statusMessage = '同步状态尚未确认，请按原目标重试';
    } finally {
      if (!_disposed) {
        _busy = false;
        notifyListeners();
        query();
      }
    }
  }

  @override
  void dispose() {
    _disposed = true;
    _subscription.cancel();
    super.dispose();
  }
}
