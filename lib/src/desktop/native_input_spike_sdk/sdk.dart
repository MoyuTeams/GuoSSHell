import 'dart:typed_data';

import 'package:rinf/rinf.dart' as rinf;

import 'protocol.dart';

abstract interface class FixtureSdk {
  Future<void> initialize(
    Map<String, FixtureSignalHandler> handlers,
    String path,
  );
  void sendProbe(Uint8List request);
  void finalize();
}

/// 仅使用 Rinf 的公共 API，不读取 SDK private library 或修改 upstream ABI。
class RinfFixtureSdk implements FixtureSdk {
  @override
  Future<void> initialize(
    Map<String, FixtureSignalHandler> handlers,
    String path,
  ) => rinf.initializeRust(handlers, compiledLibPath: path);

  @override
  void sendProbe(Uint8List request) =>
      rinf.sendDartSignal(sdkProbeEndpoint, request, Uint8List(0));

  @override
  void finalize() => rinf.finalizeRust();
}
