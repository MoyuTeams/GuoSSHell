import 'package:flutter/services.dart';

/// 固定合成探针；不接受任意键、正文、capability 或 native 地址。
enum SourceProbe {
  f1Down,
  f1Up,
  f2Down,
  f2Repeat,
  f2Up,
  f3Down,
  f3Up,
  f4Down,
  f4Repeat,
  f4Up,
  f5Down,
  f5Up,
  f6Down,
  f6Up,
  unknownDown,
  unknownRepeat,
  unknownUp,
}

enum SourceMode { query, mode1, mode2, mode3 }

class SourceNumbers {
  SourceNumbers.fromMap(Object? raw) : values = _decode(raw);

  final Map<String, Object> values;
  static Map<String, Object> _decode(Object? raw) {
    if (raw is! Map) throw const FormatException('source 数值报告不可用');
    final values = <String, Object>{};
    for (final entry in raw.entries) {
      if (entry.key is! String ||
          (entry.value is! int && entry.value is! bool)) {
        throw const FormatException('source 报告包含非数值字段');
      }
      values[entry.key as String] = entry.value as Object;
    }
    return Map.unmodifiable(values);
  }

  int number(String name) {
    final value = values[name];
    if (value is! int) throw const FormatException('source 数值字段缺失');
    return value;
  }

  bool flag(String name) {
    final value = values[name];
    if (value is! bool) throw const FormatException('source 标志字段缺失');
    return value;
  }

  bool get joinedAndEmpty =>
      flag('closeComplete') &&
      number('status') == 0 &&
      number('coreRevoked') == 1 &&
      number('actualShutdownConfirmed') == 1 &&
      number('managementCompleted') == 1 &&
      number('joinStatus') == 0 &&
      number('owners') == 0 &&
      number('stopped') == 1 &&
      number('workerDone') == 1 &&
      number('workerJoined') == 1 &&
      number('chargedSlots') == 0 &&
      number('chargedBytes') == 0 &&
      number('inflight') == 0;
}

class SourcePeerBytes {
  SourcePeerBytes.fromMap(Object? raw) {
    if (raw is! Map || raw['status'] is! int || raw['bytes'] is! Uint8List) {
      throw const FormatException('source peer 结果不可用');
    }
    status = raw['status'] as int;
    bytes = raw['bytes'] as Uint8List;
    if (bytes.length > 128) throw const FormatException('source peer 结果超界');
  }
  late final int status;
  late final Uint8List bytes;
}

abstract interface class SourceFixtureAdapter {
  Future<SourceNumbers> open({required bool explicitOptIn});
  Future<SourceNumbers> inspect();
  Future<int> probe(SourceProbe probe);
  Future<int> mode(SourceMode mode);
  Future<SourcePeerBytes> read();
  Future<SourceNumbers> closeStatus({required bool begin});
}

class MethodChannelSourceAdapter implements SourceFixtureAdapter {
  const MethodChannelSourceAdapter();
  static const channel = MethodChannel('guosshell/native_input_spike_sdk');
  static const callLimit = Duration(seconds: 3);

  /// 另须原生固定 exit-test flag；消费自有 WM_QUIT，不实际卸载 COM。
  Future<SourceNumbers> testDirectExit() async =>
      SourceNumbers.fromMap(await _call('sourceTestDirectExit'));

  Future<Object?> _call(String method, [Map<String, Object>? arguments]) =>
      channel.invokeMethod<Object?>(method, arguments).timeout(callLimit);

  Future<int> _status(String method, Map<String, Object> arguments) async {
    final raw = await _call(method, arguments);
    if (raw is! Map || raw['status'] is! int) {
      throw const FormatException('source 状态不可用');
    }
    return raw['status'] as int;
  }

  @override
  Future<SourceNumbers> open({required bool explicitOptIn}) async =>
      SourceNumbers.fromMap(
        await _call('sourceOpen', {'explicitOptIn': explicitOptIn}),
      );

  @override
  Future<SourceNumbers> inspect() async =>
      SourceNumbers.fromMap(await _call('sourceInspect'));

  @override
  Future<int> probe(SourceProbe probe) =>
      _status('sourceProbe', {'probe': probe.name});

  @override
  Future<int> mode(SourceMode mode) =>
      _status('sourceMode', {'command': mode.name});

  @override
  Future<SourcePeerBytes> read() async =>
      SourcePeerBytes.fromMap(await _call('sourceRead'));

  @override
  Future<SourceNumbers> closeStatus({required bool begin}) async =>
      SourceNumbers.fromMap(await _call('sourceCloseStatus', {'begin': begin}));
}

/// 有界 UI poll；Rust join 只由原生专用管理线程执行。
Future<SourceNumbers> closeSourceFixture(SourceFixtureAdapter source) async {
  var result = await source.closeStatus(begin: true);
  final deadline = DateTime.now().add(const Duration(seconds: 3));
  while (result.number('closeHr') == -2147483638 &&
      DateTime.now().isBefore(deadline)) {
    await Future<void>.delayed(const Duration(milliseconds: 25));
    result = await source.closeStatus(begin: false);
  }
  return result;
}
