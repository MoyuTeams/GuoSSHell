import 'dart:async';

import 'package:flutter/services.dart';

import '../../bindings/bindings.dart';

/// 通道局部身份；只接受宿主签发的 token，不携带 C ABI handle。
class DesktopPaneToken {
  DesktopPaneToken._(this.value);
  final String value;
}

class DesktopActionClaim {
  DesktopActionClaim._(this.pane, this.token, this.action, this.payload);
  final DesktopPaneToken pane;
  final String token;
  final DesktopAction action;
  final String payload;
}

class DesktopNativeCapability {
  const DesktopNativeCapability({
    required this.moduleAttached,
    required this.inputSourceAvailable,
    required this.productionEnabled,
  });
  final bool moduleAttached;
  final bool inputSourceAvailable;
  final bool productionEnabled;
  bool get available =>
      moduleAttached && inputSourceAvailable && productionEnabled;
}

class DesktopNativeFocus {
  const DesktopNativeFocus(this.active, this.attempt, this.focusEpoch);
  final bool active;
  final Uint64? attempt;
  final Uint64? focusEpoch;
}

/// 正常 MethodChannel 不加载 DLL，不向 Dart 暴露受信登记材料。
class DesktopNativeHost {
  DesktopNativeHost({
    MethodChannel? channel,
    this.timeout = const Duration(seconds: 10),
  }) : _channel =
           channel ?? const MethodChannel('guosshell/desktop_native_input_v1') {
    _channel.setMethodCallHandler((call) async {
      if (call.method != 'paneState' && call.method != 'compositionLayout') {
        throw MissingPluginException('未实现的原生输入通知');
      }
      final args = call.arguments;
      if (args is! Map ||
          args['version'] != 1 ||
          args['paneToken'] is! String) {
        throw const FormatException('无效原生窗格通知');
      }
      final message = Map<Object?, Object?>.from(args)
        ..['method'] = call.method;
      final pending = _transitions['${args['paneToken']}:${args['requestId']}'];
      if (call.method == 'paneState' &&
          args['state'] != 'pending' &&
          pending != null &&
          !pending.isCompleted) {
        pending.complete(message);
      }
      _notifications.add(message);
      return {'version': 1, 'state': 'queued'};
    });
  }

  static final shared = DesktopNativeHost();
  final MethodChannel _channel;
  final _notifications = StreamController<Map<Object?, Object?>>.broadcast(
    sync: true,
  );
  Stream<Map<Object?, Object?>> get notifications => _notifications.stream;
  final _transitions = <String, Completer<Map<Object?, Object?>>>{};
  int _request = 0;
  Future<DesktopNativeCapability>? _runtime;
  final Duration timeout;

  Future<DesktopNativeCapability> attachRuntime() =>
      _runtime ??= _attachRuntime();

  Future<DesktopNativeCapability> _attachRuntime() async {
    try {
      final reply = await _call('attachRuntime', null);
      return DesktopNativeCapability(
        moduleAttached: reply['moduleAttached'] == true,
        inputSourceAvailable: reply['inputSourceAvailable'] == true,
        productionEnabled: reply['productionEnabled'] == true,
      );
    } on MissingPluginException {
      return const DesktopNativeCapability(
        moduleAttached: false,
        inputSourceAvailable: false,
        productionEnabled: false,
      );
    } on PlatformException {
      return const DesktopNativeCapability(
        moduleAttached: false,
        inputSourceAvailable: false,
        productionEnabled: false,
      );
    } on TimeoutException {
      return const DesktopNativeCapability(
        moduleAttached: false,
        inputSourceAvailable: false,
        productionEnabled: false,
      );
    }
  }

  Future<Map<Object?, Object?>> _call(
    String method,
    Map<String, Object?>? args,
  ) async {
    final reply = await _channel
        .invokeMapMethod<Object?, Object?>(
          method,
          args == null ? null : {'version': 1, ...args},
        )
        .timeout(timeout);
    if (reply == null || reply['version'] != 1) {
      throw const FormatException('原生输入通道返回无效版本');
    }
    return reply;
  }

  static Map<String, Object?> source(DesktopSource source) => {
    'sessionId': source.sessionId,
    'generation': source.generation.toString(),
  };

  Future<DesktopPaneToken> attachPane({
    required DesktopSource origin,
    required String identity,
    required Uint64 attempt,
    required Uint64 focusEpoch,
    required Uint64 lifecycleRevision,
    required Map<String, Object?> geometry,
  }) async {
    final reply = await _call('attachPane', {
      'source': source(origin),
      'identity': identity,
      'attempt': attempt.toString(),
      'focusEpoch': focusEpoch.toString(),
      'lifecycleRevision': lifecycleRevision.toString(),
      'geometry': geometry,
    });
    final token = reply['paneToken'];
    if (reply['state'] != 'attached' || token is! String || token.isEmpty) {
      throw StateError('原生输入窗格未登记');
    }
    return DesktopPaneToken._(token);
  }

  /// 宿主 Future 只在原请求达到 active/nativequiet/released 或明确失败时完成。
  /// pending 不是成功，不能据此允许另一个 editable。
  Future<Map<Object?, Object?>> _transition(
    String method,
    DesktopPaneToken pane, {
    String? phase,
  }) async {
    if (_transitions.length >= 64) throw StateError('原生输入转换待决额度已满');
    final request = (++_request).toString();
    final key = '${pane.value}:$request';
    final pending = Completer<Map<Object?, Object?>>();
    _transitions[key] = pending;
    try {
      final reply = await _call(method, {
        'paneToken': pane.value,
        'requestId': request,
        'phase': ?phase,
      });
      return reply['state'] == 'pending'
          ? await pending.future.timeout(timeout)
          : reply;
    } finally {
      _transitions.remove(key);
    }
  }

  Future<DesktopNativeFocus> requestFocus(DesktopPaneToken pane) async {
    final reply = await _transition('requestFocus', pane);
    if (reply['state'] != 'active') {
      return const DesktopNativeFocus(false, null, null);
    }
    return DesktopNativeFocus(
      true,
      Uint64.parse(reply['attempt'] as String),
      Uint64.parse(reply['focusEpoch'] as String),
    );
  }

  Future<bool> retirePane(
    DesktopPaneToken pane, {
    required bool release,
  }) async {
    final reply = await _transition(
      'retirePane',
      pane,
      phase: release ? 'release' : 'quiet',
    );
    return release
        ? reply['state'] == 'released'
        : reply['state'] == 'nativequiet' || reply['state'] == 'released';
  }

  Future<void> invalidateGeometry(DesktopPaneToken pane) async {
    final reply = await _call('invalidateGeometry', {'paneToken': pane.value});
    if (reply['state'] != 'invalidated') throw StateError('原生几何未失效');
  }

  Future<bool> presentedFrame(
    DesktopPaneToken pane,
    Map<String, Object?> frame,
  ) async {
    final reply = await _call('presentedFrame', {
      'paneToken': pane.value,
      ...frame,
    });
    return reply['state'] == 'presented';
  }

  Future<void> textEndRequired(
    DesktopPaneToken pane,
    Map<String, Object?> fence,
  ) async {
    final reply = await _call('textEndRequired', {
      'paneToken': pane.value,
      ...fence,
    });
    if (reply['state'] != 'accepted' && reply['state'] != 'nativequiet') {
      throw StateError('原文本终结请求未受理');
    }
  }

  Future<bool> textLayout(
    DesktopPaneToken pane,
    Map<String, Object?> layout,
  ) async {
    final reply = await _call('textLayout', {
      'paneToken': pane.value,
      ...layout,
    });
    return reply['state'] == 'laidOut';
  }

  Future<DesktopActionClaim?> claimAction(
    DesktopPaneToken pane,
    Uint64 effect,
  ) async {
    final reply = await _call('claimAction', {
      'paneToken': pane.value,
      'effect': effect.toString(),
    });
    if (reply['state'] != 'claimed') return null;
    final token = reply['claimToken'];
    final action = reply['action'];
    final payload = reply['payload'];
    if (token is! String ||
        token.isEmpty ||
        action is! String ||
        payload is! String) {
      throw const FormatException('原生动作缺少完整 claim 材料');
    }
    return DesktopActionClaim._(
      pane,
      token,
      DesktopAction.values.firstWhere((value) => value.name == action),
      payload,
    );
  }

  Future<bool> completeAction(
    DesktopActionClaim claim, {
    required String kind,
    String? text,
  }) async {
    final reply = await _call('completeAction', {
      'paneToken': claim.pane.value,
      'claimToken': claim.token,
      'kind': kind,
      'text': ?text,
    });
    return reply['state'] == 'completed' ||
        (kind == 'abort' && reply['state'] == 'aborted');
  }
}
