import 'dart:async';
import 'dart:math';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import 'protocol.dart';
import 'sdk.dart';

abstract interface class FixtureNativeAdapter {
  Future<FixturePreflight> preflight({
    required String controlledPath,
    required bool explicitOptIn,
    required Uint8List nonce,
  });
  Future<FixtureDiagnostic> verify(Uint8List actualReply);
}

/// 父级待接的两个有界控制调用，不注册正常 signal listener 或输入 source。
class MethodChannelFixtureAdapter implements FixtureNativeAdapter {
  const MethodChannelFixtureAdapter();
  static const channel = MethodChannel('guosshell/native_input_spike_sdk');
  static const callLimit = Duration(seconds: 3);

  @override
  Future<FixturePreflight> preflight({
    required String controlledPath,
    required bool explicitOptIn,
    required Uint8List nonce,
  }) async {
    final raw = await channel
        .invokeMethod<Object?>('preflight', {
          'path': controlledPath,
          'explicitOptIn': explicitOptIn,
          'nonce': nonce,
        })
        .timeout(callLimit);
    final diagnostic = FixtureDiagnostic.fromMap(raw);
    final path = raw is Map ? raw['compiledLibPath'] : null;
    if (diagnostic.safePreflight && (path is! String || path.isEmpty)) {
      throw const FormatException('native 未返回受控加载路径');
    }
    return FixturePreflight(diagnostic, path is String ? path : '');
  }

  @override
  Future<FixtureDiagnostic> verify(Uint8List actualReply) async =>
      FixtureDiagnostic.fromMap(
        await channel
            .invokeMethod<Object?>('verify', {'reply': actualReply})
            .timeout(callLimit),
      );
}

enum BootstrapPhase {
  idle,
  preflight,
  initializing,
  waiting,
  verifying,
  proven,
  failed,
  closed,
}

class SdkBootstrap extends ChangeNotifier {
  SdkBootstrap({
    required this.native,
    required this.sdk,
    required this.normalHandlers,
    required this.controlledPath,
    required this.explicitOptIn,
    this.replyLimit = const Duration(seconds: 5),
  });

  final FixtureNativeAdapter native;
  final FixtureSdk sdk;
  final Map<String, FixtureSignalHandler> normalHandlers;
  final String controlledPath;
  final bool explicitOptIn;
  final Duration replyLimit;

  BootstrapPhase phase = BootstrapPhase.idle;
  FixtureDiagnostic? diagnostic;
  String failure = '';
  bool _attempted = false;
  bool _closed = false;
  bool _sdkReady = false;
  bool _finalized = false;
  Uint8List? _nonce;
  Completer<Uint8List?>? _reply;
  Timer? _timer;

  bool get canRun => explicitOptIn && !_attempted && !_closed;
  bool get busy => switch (phase) {
    BootstrapPhase.preflight ||
    BootstrapPhase.initializing ||
    BootstrapPhase.waiting ||
    BootstrapPhase.verifying => true,
    _ => false,
  };

  void _phase(BootstrapPhase next) {
    if (_closed) return;
    phase = next;
    notifyListeners();
  }

  void _finishWait() {
    _timer?.cancel();
    _timer = null;
    final pending = _reply;
    if (pending != null && !pending.isCompleted) pending.complete(null);
  }

  void _fail(String message) {
    if (_closed) return;
    failure = message;
    _finishWait();
    _phase(BootstrapPhase.failed);
  }

  void _receive(Uint8List message, Uint8List binary) {
    final nonce = _nonce;
    final pending = _reply;
    if (_closed ||
        phase == BootstrapPhase.failed ||
        nonce == null ||
        pending == null ||
        pending.isCompleted ||
        !matchingSdkReply(message, binary, nonce)) {
      return;
    }
    // 保留实际 SDK 回应交由 native 核验，不构造 Dart 身份 token。
    pending.complete(message);
  }

  Future<void> run() async {
    if (_attempted || _closed) return;
    _attempted = true;
    if (!explicitOptIn ||
        controlledPath.isEmpty ||
        normalHandlers.containsKey(sdkProbeReplySignal)) {
      _fail('未显式开启受控 fixture，SDK 未启动。');
      return;
    }
    _phase(BootstrapPhase.preflight);
    try {
      final random = Random.secure();
      final nonce = Uint8List.fromList(
        List.generate(16, (_) => random.nextInt(256)),
      );
      if (nonce.every((byte) => byte == 0)) nonce[0] = 1;
      _nonce = nonce;
      final preflight = await native
          .preflight(
            controlledPath: controlledPath,
            explicitOptIn: true,
            nonce: nonce,
          )
          .timeout(MethodChannelFixtureAdapter.callLimit);
      if (_closed) return;
      diagnostic = preflight.diagnostic;
      if (!preflight.diagnostic.safePreflight ||
          preflight.compiledLibPath.isEmpty) {
        _fail('native 隔离预检未通过，SDK 未启动。');
        return;
      }
      _reply = Completer<Uint8List?>();
      _timer = Timer(replyLimit, () => _fail('SDK 回应超时；未绑定，不重试。'));
      final handlers = <String, FixtureSignalHandler>{
        sdkProbeReplySignal: _receive,
        ...normalHandlers,
      };
      _phase(BootstrapPhase.initializing);
      final initialized = sdk
          .initialize(handlers, preflight.compiledLibPath)
          .then((_) {
            _sdkReady = true;
            if (_closed) _finalize();
          });
      await initialized.timeout(replyLimit);
      if (_closed) {
        _finalize();
        return;
      }
      if (phase == BootstrapPhase.failed) return;
      _phase(BootstrapPhase.waiting);
      sdk.sendProbe(encodeSdkProbe(nonce));
      final reply = await _reply!.future;
      if (_closed || reply == null || phase == BootstrapPhase.failed) return;
      _timer?.cancel();
      _timer = null;
      _phase(BootstrapPhase.verifying);
      final verified = await native
          .verify(reply)
          .timeout(MethodChannelFixtureAdapter.callLimit);
      if (_closed) return;
      diagnostic = verified;
      if (!verified.safePreflight || !verified.sameModule) {
        _fail('native 同 module 核验未通过；未绑定，不重试。');
        return;
      }
      _phase(BootstrapPhase.proven);
    } catch (_) {
      // 绝不显示 SDK／平台异常中可能携带的本机路径或地址。
      _fail('bootstrap 不可用；未绑定，请关闭后检查受控构建与宿主接线。');
    }
  }

  void _finalize() {
    if (!_sdkReady || _finalized) return;
    _finalized = true;
    sdk.finalize();
  }

  void close() {
    if (_closed) return;
    _closed = true;
    phase = BootstrapPhase.closed;
    _finishWait();
    _nonce = null;
    _finalize();
  }

  @override
  void dispose() {
    close();
    super.dispose();
  }
}
