import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/desktop/native_input_spike_sdk/bootstrap.dart';
import 'package:guosh_shell/src/desktop/native_input_spike_sdk/sdk.dart';
import 'package:guosh_shell/src/desktop/native_input_spike_source/adapter.dart';
import 'package:guosh_shell/src/desktop/native_input_spike_source/fixture_app.dart';

import 'native_input_spike_source_progress.dart';

class _UnusedSdk implements FixtureSdk {
  @override
  Future<void> initialize(
    Map<String, void Function(Uint8List, Uint8List)> handlers,
    String path,
  ) async => throw StateError('夹具未授权启动');
  @override
  void sendProbe(Uint8List request) => throw StateError('夹具未授权启动');
  @override
  void finalize() {}
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  const adapter = MethodChannelSourceAdapter();
  test('Unknown Repeat／Up 必须匹配新事件计数及各自 phase', () {
    final baseline = SourceNumbers.fromMap({
      'lastEvent': 11,
      'observations': 7,
      'gate': 6,
      'loopHandled': 8,
    });
    SourceNumbers observation(int ordinal, int phase) => SourceNumbers.fromMap({
      for (final field in ['lastEvent', 'observations', 'gate', 'loopHandled'])
        field: baseline.number(field) + ordinal,
      'lastPhase': phase,
    });
    for (final (ordinal, phase) in [(8, 0), (9, 1), (10, 2)]) {
      expectSourceProbeProgress(
        baseline: baseline,
        current: observation(ordinal, phase),
        ordinal: ordinal,
        phase: phase,
        postStatus: 0,
      );
    }
    expect(
      () => expectSourceProbeProgress(
        baseline: baseline,
        current: observation(8, 0),
        ordinal: 9,
        phase: 1,
        postStatus: 0,
      ),
      throwsA(isA<TestFailure>()),
    );
    expect(
      () => expectSourceProbeProgress(
        baseline: baseline,
        current: observation(10, 0),
        ordinal: 10,
        phase: 2,
        postStatus: 0,
      ),
      throwsA(isA<TestFailure>()),
    );
    expect(
      () => expectSourceProbeProgress(
        baseline: baseline,
        current: observation(9, 1),
        ordinal: 9,
        phase: 1,
        postStatus: 2,
      ),
      throwsA(isA<TestFailure>()),
    );
  });
  test('夹具通道只传固定探针名与模式，不传 native 数值身份', () async {
    final calls = <MethodCall>[];
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(MethodChannelSourceAdapter.channel, (
          call,
        ) async {
          calls.add(call);
          if (call.method == 'sourceRead') {
            return {
              'status': 0,
              'bytes': Uint8List.fromList([27, 91, 51, 126]),
            };
          }
          return {'status': 0};
        });
    addTearDown(
      () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(MethodChannelSourceAdapter.channel, null),
    );
    expect(await adapter.probe(SourceProbe.f4Down), 0);
    expect(calls.last.arguments, {'probe': 'f4Down'});
    expect(await adapter.mode(SourceMode.mode3), 0);
    expect(calls.last.arguments, {'command': 'mode3'});
    expect((await adapter.read()).bytes, [27, 91, 51, 126]);
  });

  test('只有真实完整关闭数值同时成立才允许确认释放', () {
    final values = <String, Object>{
      'closeComplete': true,
      'status': 0,
      'coreRevoked': 1,
      'actualShutdownConfirmed': 1,
      'managementCompleted': 1,
      'joinStatus': 0,
      'owners': 0,
      'stopped': 1,
      'workerDone': 1,
      'workerJoined': 1,
      'chargedSlots': 0,
      'chargedBytes': 0,
      'inflight': 0,
    };
    expect(SourceNumbers.fromMap(values).joinedAndEmpty, isTrue);
    for (final key in ['managementCompleted', 'workerDone', 'workerJoined']) {
      expect(
        SourceNumbers.fromMap({...values, key: 0}).joinedAndEmpty,
        isFalse,
      );
    }
    expect(
      SourceNumbers.fromMap({...values, 'chargedSlots': 1}).joinedAndEmpty,
      isFalse,
    );
    expect(
      () => SourceNumbers.fromMap({'address': '不可回传'}),
      throwsFormatException,
    );
  });

  for (final width in [375.0, 768.0, 1280.0]) {
    testWidgets('source 默认关闭布局 $width 不启动 SDK 或 source', (tester) async {
      tester.view.physicalSize = Size(width, 720);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final bootstrap = SdkBootstrap(
        native: const MethodChannelFixtureAdapter(),
        sdk: _UnusedSdk(),
        normalHandlers: const {},
        controlledPath: '',
        explicitOptIn: false,
      );
      await tester.pumpWidget(
        SourceFixtureApp(
          bootstrap: bootstrap,
          source: adapter,
          explicitOptIn: false,
          onClosed: () {},
        ),
      );
      await tester.pumpAndSettle();
      expect(bootstrap.phase, BootstrapPhase.idle);
      expect(tester.takeException(), isNull);
      expect(find.byKey(const Key('source-fixture-run')), findsOneWidget);
      await tester.pumpWidget(const SizedBox.shrink());
      bootstrap.dispose();
    });
  }
}
