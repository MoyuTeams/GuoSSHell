import 'dart:async';
import 'dart:typed_data';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/desktop/native_input_spike_sdk/bootstrap.dart';
import 'package:guosh_shell/src/desktop/native_input_spike_sdk/protocol.dart';
import 'package:guosh_shell/src/desktop/native_input_spike_sdk/sdk.dart';

const safeDiagnostic = FixtureDiagnostic(
  status: 0,
  coreAbi: 2,
  pointerBits: 64,
  owners: 0,
  rinfExports: 4,
  coreExports: 8,
  fixtureOnly: true,
  sameModule: false,
);
const provenDiagnostic = FixtureDiagnostic(
  status: 0,
  coreAbi: 2,
  pointerBits: 64,
  owners: 0,
  rinfExports: 4,
  coreExports: 8,
  fixtureOnly: true,
  sameModule: true,
);

Uint8List fakeReply(Uint8List nonce) {
  final bytes = Uint8List(sdkResponseBytes);
  bytes.setRange(0, 4, const [71, 78, 83, 82]);
  final view = ByteData.sublistView(bytes);
  view.setUint16(4, 1, Endian.little);
  view.setUint32(8, 2, Endian.little);
  view.setUint32(12, 64, Endian.little);
  bytes.setRange(16, 32, nonce);
  view.setUint64(32, 0x1000, Endian.little);
  view.setUint32(40, 1, Endian.little);
  view.setUint32(44, 1, Endian.little);
  return bytes;
}

class FakeNative implements FixtureNativeAdapter {
  int preflights = 0;
  int verifications = 0;
  Uint8List? nonce;
  Uint8List? actualReply;
  FixtureDiagnostic preflightDiagnostic = safeDiagnostic;
  FixtureDiagnostic verifyDiagnostic = provenDiagnostic;
  Completer<FixturePreflight>? pending;

  @override
  Future<FixturePreflight> preflight({
    required String controlledPath,
    required bool explicitOptIn,
    required Uint8List nonce,
  }) async {
    preflights++;
    expect(explicitOptIn, isTrue);
    this.nonce = nonce;
    return pending != null
        ? pending!.future
        : FixturePreflight(preflightDiagnostic, 'fixture.dll');
  }

  @override
  Future<FixtureDiagnostic> verify(Uint8List actualReply) async {
    verifications++;
    this.actualReply = actualReply;
    return verifyDiagnostic;
  }
}

class FakeSdk implements FixtureSdk {
  int initializations = 0;
  int sends = 0;
  int finalizations = 0;
  bool failInitialization = false;
  Map<String, FixtureSignalHandler>? handlers;
  void Function(Uint8List)? onSend;
  Completer<void>? startup;

  @override
  Future<void> initialize(
    Map<String, FixtureSignalHandler> handlers,
    String path,
  ) async {
    initializations++;
    this.handlers = handlers;
    expect(path, 'fixture.dll');
    if (failInitialization) throw StateError('不得显示本机信息的模拟异常');
    if (startup != null) await startup!.future;
  }

  @override
  void sendProbe(Uint8List request) {
    sends++;
    expect(request.length, sdkRequestBytes);
    onSend?.call(request);
  }

  @override
  void finalize() {
    finalizations++;
  }
}

SdkBootstrap createBootstrap(
  FakeNative native,
  FakeSdk sdk, {
  bool optIn = true,
  Duration timeout = const Duration(seconds: 1),
  Map<String, FixtureSignalHandler> handlers = const {},
}) => SdkBootstrap(
  native: native,
  sdk: sdk,
  normalHandlers: handlers,
  controlledPath: 'controlled-fixture.dll',
  explicitOptIn: optIn,
  replyLimit: timeout,
);

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  test('默认关闭不触及 native 或 SDK', () async {
    final native = FakeNative();
    final sdk = FakeSdk();
    final bootstrap = createBootstrap(native, sdk, optIn: false);
    await bootstrap.run();
    expect(native.preflights, 0);
    expect(sdk.initializations, 0);
    expect(bootstrap.phase, BootstrapPhase.failed);
    bootstrap.dispose();
  });

  test('预检未证明隔离时禁止初始化', () async {
    final native = FakeNative()
      ..preflightDiagnostic = const FixtureDiagnostic(
        status: 6,
        coreAbi: 2,
        pointerBits: 64,
        owners: 0,
        rinfExports: 4,
        coreExports: 8,
        fixtureOnly: false,
        sameModule: false,
      );
    final sdk = FakeSdk();
    final bootstrap = createBootstrap(native, sdk);
    await bootstrap.run();
    expect(sdk.initializations, 0);
    expect(native.verifications, 0);
    bootstrap.dispose();
  });

  test('实际 matching 回应先到 Dart，再交 native 核验；最多一次', () async {
    final native = FakeNative();
    final sdk = FakeSdk();
    final bootstrap = createBootstrap(native, sdk);
    Uint8List? reply;
    sdk.onSend = (request) {
      reply = fakeReply(Uint8List.sublistView(request, 16));
      sdk.handlers![sdkProbeReplySignal]!(reply!, Uint8List(0));
    };
    await bootstrap.run();
    expect(bootstrap.phase, BootstrapPhase.proven);
    expect(native.actualReply, same(reply));
    expect(native.verifications, 1);
    await bootstrap.run();
    expect(native.preflights, 1);
    expect(sdk.initializations, 1);
    expect(sdk.sends, 1);
    bootstrap.close();
    bootstrap.close();
    expect(sdk.finalizations, 1);
    bootstrap.dispose();
  });

  test('SDK send 返回不代表 delivery；错误 nonce 和迟到回应不绑定', () async {
    final native = FakeNative();
    final sdk = FakeSdk();
    final bootstrap = createBootstrap(
      native,
      sdk,
      timeout: const Duration(milliseconds: 10),
    );
    sdk.onSend = (request) {
      final reply = fakeReply(Uint8List.sublistView(request, 16));
      reply[16] ^= 1;
      sdk.handlers![sdkProbeReplySignal]!(reply, Uint8List(0));
    };
    await bootstrap.run();
    expect(bootstrap.phase, BootstrapPhase.failed);
    expect(native.verifications, 0);
    sdk.handlers![sdkProbeReplySignal]!(fakeReply(native.nonce!), Uint8List(0));
    expect(native.verifications, 0);
    expect(bootstrap.phase, BootstrapPhase.failed);
    bootstrap.dispose();
  });

  test('native 拒绝同 module 时不会转成功', () async {
    final native = FakeNative()..verifyDiagnostic = safeDiagnostic;
    final sdk = FakeSdk();
    sdk.onSend = (request) => sdk.handlers![sdkProbeReplySignal]!(
      fakeReply(Uint8List.sublistView(request, 16)),
      Uint8List(0),
    );
    final bootstrap = createBootstrap(native, sdk);
    await bootstrap.run();
    expect(bootstrap.phase, BootstrapPhase.failed);
    bootstrap.dispose();
  });

  test('初始化错误不显示异常内容，也不发送 probe', () async {
    final native = FakeNative();
    final sdk = FakeSdk()..failInitialization = true;
    final bootstrap = createBootstrap(native, sdk);
    await bootstrap.run();
    expect(bootstrap.phase, BootstrapPhase.failed);
    expect(bootstrap.failure, isNot(contains('模拟异常')));
    expect(sdk.sends, 0);
    expect(sdk.finalizations, 0);
    bootstrap.dispose();
  });

  test('关闭预检不允许迟到结果启动 SDK', () async {
    final native = FakeNative()..pending = Completer<FixturePreflight>();
    final sdk = FakeSdk();
    final bootstrap = createBootstrap(native, sdk);
    final running = bootstrap.run();
    bootstrap.close();
    native.pending!.complete(
      const FixturePreflight(safeDiagnostic, 'fixture.dll'),
    );
    await running;
    expect(sdk.initializations, 0);
    bootstrap.dispose();
  });

  test('启动期间关闭后不发送 probe，迟到初始化只 finalize 一次', () async {
    final native = FakeNative();
    final sdk = FakeSdk()..startup = Completer<void>();
    final bootstrap = createBootstrap(native, sdk);
    final running = bootstrap.run();
    await Future<void>.delayed(Duration.zero);
    expect(sdk.initializations, 1);
    bootstrap.close();
    sdk.startup!.complete();
    await running;
    expect(sdk.sends, 0);
    expect(sdk.finalizations, 1);
    bootstrap.dispose();
  });

  test('原有 callback 保留委派，不新订阅正常 signals', () async {
    final native = FakeNative();
    final sdk = FakeSdk();
    var delegated = 0;
    final bootstrap = createBootstrap(
      native,
      sdk,
      handlers: {
        'Existing': (_, _) {
          delegated++;
        },
      },
    );
    sdk.onSend = (request) {
      sdk.handlers!['Existing']!(Uint8List(0), Uint8List(0));
      sdk.handlers![sdkProbeReplySignal]!(
        fakeReply(native.nonce!),
        Uint8List(0),
      );
      sdk.handlers!['Existing']!(Uint8List(0), Uint8List(0));
    };
    await bootstrap.run();
    expect(delegated, 2);
    bootstrap.dispose();
  });

  test('固定协议拒绝截断、binary、版本、状态、ABI、预算和零 nonce', () {
    final nonce = Uint8List(16)..fillRange(0, 16, 7);
    final reply = fakeReply(nonce);
    expect(matchingSdkReply(reply, Uint8List(0), nonce), isTrue);
    expect(matchingSdkReply(Uint8List(47), Uint8List(0), nonce), isFalse);
    expect(matchingSdkReply(reply, Uint8List(1), nonce), isFalse);
    for (final offset in [0, 4, 6, 8, 12, 32, 40, 44]) {
      final malformed = Uint8List.fromList(reply)..[offset] ^= 1;
      expect(matchingSdkReply(malformed, Uint8List(0), nonce), isFalse);
    }
    expect(() => encodeSdkProbe(Uint8List(16)), throwsArgumentError);
    expect(encodeSdkProbe(nonce).sublist(0, 16), [
      71,
      78,
      83,
      81,
      1,
      0,
      1,
      0,
      1,
      0,
      0,
      0,
      0,
      0,
      0,
      0,
    ]);
  });

  test('MethodChannel 仅发送 preflight 和实际 reply 两个固定控制调用', () async {
    final calls = <MethodCall>[];
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    messenger.setMockMethodCallHandler(MethodChannelFixtureAdapter.channel, (
      call,
    ) async {
      calls.add(call);
      return <String, Object>{
        'status': 0,
        'coreAbi': 2,
        'pointerBits': 64,
        'owners': 0,
        'rinfExports': 4,
        'coreExports': 8,
        'fixtureOnly': true,
        'sameModule': call.method == 'verify',
        'compiledLibPath': 'fixture.dll',
      };
    });
    addTearDown(
      () => messenger.setMockMethodCallHandler(
        MethodChannelFixtureAdapter.channel,
        null,
      ),
    );
    const adapter = MethodChannelFixtureAdapter();
    final nonce = Uint8List(16)..fillRange(0, 16, 7);
    final preflight = await adapter.preflight(
      controlledPath: 'controlled-fixture.dll',
      explicitOptIn: true,
      nonce: nonce,
    );
    expect(preflight.compiledLibPath, 'fixture.dll');
    expect((await adapter.verify(fakeReply(nonce))).sameModule, isTrue);
    expect(calls.map((call) => call.method), ['preflight', 'verify']);
    expect((calls[0].arguments as Map)['nonce'], nonce);
    expect((calls[1].arguments as Map)['reply'], hasLength(48));
  });
}
