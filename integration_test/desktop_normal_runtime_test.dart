import 'dart:async';
import 'dart:io';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:integration_test/integration_test.dart';
import 'package:rinf/rinf.dart';

// 验证正常 runner 的 Flutter V2 准备与私有原生输入隔离，不证明认证、输入或 TIP 行为。
const _native = MethodChannel('guosshell/desktop_native_input_v1');

Future<T> _response<T>(
  WidgetTester tester,
  Stream<RustSignalPack<T>> stream,
  bool Function(T) matches,
  void Function() send,
) async {
  T? result;
  final subscription = stream.listen((pack) {
    if (matches(pack.message)) result ??= pack.message;
  });
  try {
    send();
    final elapsed = Stopwatch()..start();
    while (result == null) {
      if (elapsed.elapsed > const Duration(seconds: 15)) {
        throw TestFailure('正常 hub 响应超时：$T');
      }
      await tester.pump(const Duration(milliseconds: 25));
      expect(tester.takeException(), isNull);
    }
    return result as T;
  } finally {
    await subscription.cancel();
  }
}

Uint64 _id(int value) => Uint64(BigInt.from(value));

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('正常 hub 开放 Flutter V2，私有原生输入仍关闭', (tester) async {
    if (!Platform.isWindows) {
      throw TestFailure('此接口验收需要真实 Windows runner');
    }
    final owned = await Directory.systemTemp.createTemp(
      'guosh-normal-runtime-',
    );
    var initialized = false;
    ServerSocket? server;
    final held = <Socket>[];
    StreamSubscription<RustSignalPack<DesktopNativeLifecycle>>?
    lifecycleSubscription;
    try {
      await initializeRust(assignRustSignal);
      initialized = true;
      await tester.pumpWidget(
        const Directionality(
          textDirection: TextDirection.ltr,
          child: Text('正常原生接口验证'),
        ),
      );
      final runtime = await _native.invokeMapMethod<String, Object?>(
        'attachRuntime',
      );
      expect(runtime, isNotNull);
      expect(runtime!['moduleAttached'], isTrue);
      expect(runtime['sameModule'], isTrue);
      expect(runtime['modulePinned'], isTrue);
      expect(runtime['inputSourceAvailable'], isFalse);
      expect(runtime['productionEnabled'], isFalse);
      // 重复关联不得建立第二份运行时或改变既有模块。
      final repeated = await _native.invokeMapMethod<String, Object?>(
        'attachRuntime',
      );
      expect(repeated!['moduleAttached'], isTrue);
      expect(repeated['sameModule'], isTrue);

      final ready = await _response(
        tester,
        AppReady.rustSignalStream,
        (_) => true,
        () => AppStart(supportDir: owned.path).sendSignalToRust(),
      );
      expect(ready.ok, isTrue, reason: ready.detail);
      final source = DesktopSource(sessionId: 0x5a4001, generation: _id(1));
      // 本用例自起的一次性本机监听端口：只接受连接、不读不写，SSH 握手停在等待服务端标识，
      // 直到 Disconnect 关闭会话；不依赖 runner 上是否有 22 端口服务。
      server = await ServerSocket.bind(InternetAddress.loopbackIPv4, 0);
      server.listen(held.add);
      // 在 Start 之前开始缓存生命周期事件：Ended 可能在测试订阅前就已发出，断言只查缓存。
      final lifecycles = <DesktopNativeLifecycle>[];
      lifecycleSubscription = DesktopNativeLifecycle.rustSignalStream.listen(
        (pack) => lifecycles.add(pack.message),
      );
      final preparation = await _response(
        tester,
        DesktopPreparationResult.rustSignalStream,
        (value) =>
            value.source == source &&
            value.requestId == 1 &&
            value.state != DesktopPreparationState.preparing,
        () => DesktopPrepare(
          version: 2,
          requestId: 1,
          source: source,
          target: DesktopTargetKind.nativeSsh,
          profileId: '',
          connectionId: '',
          host: '127.0.0.1',
          port: server!.port,
          username: 'normal-runtime-test',
          command: '',
        ).sendSignalToRust(),
      );
      expect(preparation.state, DesktopPreparationState.prepared);
      expect(preparation.failure, DesktopFailure.none);
      expect(preparation.identity, isNotEmpty);
      expect(preparation.snapshot, isNotNull);

      // Start 仍校验准备身份，非法请求不能启动连接。
      final start = await _response(
        tester,
        DesktopPreparationResult.rustSignalStream,
        (value) => value.source == source && value.requestId == 2,
        () => DesktopStart(
          version: 2,
          requestId: 2,
          prepareRequestId: 1,
          source: source,
          identity: '',
          geometry: const DesktopGeometry(
            cols: 80,
            rows: 24,
            pixelWidth: 640,
            pixelHeight: 384,
            dpi: 96,
          ),
          fontProjection: '',
          password: '',
        ).sendSignalToRust(),
      );
      expect(start.state, DesktopPreparationState.failed);
      expect(start.failure, DesktopFailure.identityMismatch);
      expect(start.snapshot, isNull);

      // 合法 Flutter V2 Start 被接纳；只连本用例的本机监听端口，不连接外部目标。
      final accepted = await _response(
        tester,
        DesktopPreparationResult.rustSignalStream,
        (value) => value.source == source && value.requestId == 3,
        () => DesktopStart(
          version: 2,
          requestId: 3,
          prepareRequestId: 1,
          source: source,
          identity: preparation.identity,
          geometry: const DesktopGeometry(
            cols: 80,
            rows: 24,
            pixelWidth: 640,
            pixelHeight: 384,
            dpi: 96,
          ),
          fontProjection: preparation.snapshot!.fallbackProjection,
          password: '',
        ).sendSignalToRust(),
      );
      expect(accepted.state, DesktopPreparationState.accepted);
      expect(accepted.failure, DesktopFailure.none);
      DesktopOrdered(
        version: 2,
        source: source,
        identity: preparation.identity,
        seq: _id(1),
        event: const DesktopEventDisconnect(),
      ).sendSignalToRust();
      final elapsed = Stopwatch()..start();
      bool isEnded(DesktopNativeLifecycle value) =>
          value.source == source &&
          value.identity == preparation.identity &&
          value.state == NativeLifecycleState.ended;
      while (!lifecycles.any(isEnded)) {
        if (elapsed.elapsed > const Duration(seconds: 15)) {
          throw TestFailure('正常 hub 响应超时：DesktopNativeLifecycle ended');
        }
        await tester.pump(const Duration(milliseconds: 25));
        expect(tester.takeException(), isNull);
      }
      expect(lifecycles.where(isEnded), hasLength(1));

      // 真实 Rinf 往返验证新 GUI 编码与完整回执；未登记来源不能取得输入权限。
      final nativeSource = DesktopSource(
        sessionId: 0x5a4002,
        generation: _id(1),
      );
      const inputs = <GuiInput>[
        GuiInputGuiKey(value: GuiKeyEnter()),
        GuiInputGuiKey(value: GuiKeyF(value: 12)),
        GuiInputGuiText(value: '检查点文本'),
      ];
      for (var index = 0; index < inputs.length; index++) {
        final request = DesktopNativeCommand(
          version: 1,
          source: nativeSource,
          identity: '未登记检查点',
          attempt: _id(7),
          focusEpoch: _id(9),
          requestId: _id(10 + index),
          command: NativeCommandGuiIntent(
            value: GuiIntent(
              input: inputs[index],
              modifiers: const GuiModifiers(button: 0, latched: 1, locked: 2),
              modifierRevision: _id(100 + index),
            ),
          ),
        );
        final result = await _response(
          tester,
          DesktopNativeCommandResult.rustSignalStream,
          (value) =>
              value.source == nativeSource &&
              value.requestId == request.requestId,
          request.sendSignalToRust,
        );
        expect(result.version, 1);
        expect(result.identity, request.identity);
        expect(result.attempt, request.attempt);
        expect(result.focusEpoch, request.focusEpoch);
        expect(result.admission, NativeCommandAdmission.rejected);
        expect(result.failure, NativeCommandFailure.staleSource);
        expect(result.effect, _id(0));
        expect(result.consumedLatches, 0);
        expect(result.guiModifierRevision, _id(100 + index));
      }
    } finally {
      await lifecycleSubscription?.cancel();
      for (final socket in held) {
        socket.destroy();
      }
      await server?.close();
      if (initialized) finalizeRust();
      // 该目录由本用例独占创建；运行时停止后才删除，不触及用户存储。
      await owned.delete(recursive: true);
    }
  });
}
