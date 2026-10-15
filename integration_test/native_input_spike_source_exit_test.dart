import 'dart:io';

import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/desktop/native_input_spike_sdk/bootstrap.dart';
import 'package:guosh_shell/src/desktop/native_input_spike_sdk/sdk.dart';
import 'package:guosh_shell/src/desktop/native_input_spike_source/adapter.dart';
import 'package:guosh_shell/src/desktop/native_input_spike_source/fixture_app.dart';

/// 真实存活 source 的安全 WM_QUIT 接缝；不执行 COM teardown 或输入泵。
void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  testWidgets('自有 STA 的 direct WM_QUIT 在 COM 存活时开始退休 active source', (
    tester,
  ) async {
    expect(Platform.isWindows, isTrue);
    expect(const bool.fromEnvironment('GUOSSH_NATIVE_INPUT_SPIKE_SDK'), isTrue);
    expect(
      const bool.fromEnvironment('GUOSSH_NATIVE_INPUT_SPIKE_SOURCE'),
      isTrue,
    );
    expect(
      const bool.fromEnvironment('GUOSSH_NATIVE_INPUT_SPIKE_SOURCE_EXIT_TEST'),
      isTrue,
    );
    final bootstrap = SdkBootstrap(
      native: const MethodChannelFixtureAdapter(),
      sdk: RinfFixtureSdk(),
      normalHandlers: assignRustSignal,
      controlledPath: const String.fromEnvironment(
        'GUOSSH_NATIVE_INPUT_SPIKE_DLL',
      ),
      explicitOptIn: true,
    );
    const source = MethodChannelSourceAdapter();
    final report = <String, Object>{
      'syntheticOnly': true,
      'physical': false,
      'originUnknown': true,
      'integrationNativePluginCoverage': false,
    };
    SourceNumbers? exit;
    SourceNumbers? close;
    SourceNumbers? restart;
    int? stoppedProbe;
    var opened = false;
    try {
      await tester.pumpWidget(
        SourceFixtureApp(
          bootstrap: bootstrap,
          source: source,
          explicitOptIn: true,
          onClosed: () {},
        ),
      );
      await tester.pumpAndSettle();
      await tester.runAsync(() async {
        await bootstrap.run();
        expect(bootstrap.phase, BootstrapPhase.proven);
        report['sdkActualSameModule'] = bootstrap.diagnostic!.sameModule;
        final before = await source.open(explicitOptIn: true);
        report['before'] = before.values;
        opened = true;
        expect(before.number('openHr'), 0);
        expect(before.flag('sourceActualSameSdk'), isTrue);
        expect(before.number('accepting'), 1);
        expect(before.number('actualShutdownConfirmed'), 0);
        exit = await source.testDirectExit();
        report['exit'] = exit!.values;
        restart = await source.open(explicitOptIn: true);
        stoppedProbe = await source.probe(SourceProbe.f4Down);
        report['restartHr'] = restart!.number('openHr');
        report['stoppedProbeStatus'] = stoppedProbe!;
      });
    } finally {
      await tester.runAsync(() async {
        if (opened) {
          close = await closeSourceFixture(source);
          report['close'] = close!.values;
        }
        if (close?.joinedAndEmpty ?? !opened) bootstrap.close();
      });
      report['sdkClosed'] = bootstrap.phase == BootstrapPhase.closed;
      binding.reportData = report;
      await tester.pumpWidget(const SizedBox.shrink());
      if (bootstrap.phase == BootstrapPhase.closed) bootstrap.dispose();
    }
    expect(exit!.flag('exitLoopQuit'), isTrue);
    expect(exit!.flag('comStillSta'), isTrue);
    expect(exit!.number('accepting'), 0);
    expect(exit!.number('coreRevoked'), 1);
    expect(exit!.number('actualShutdownConfirmed'), 1);
    expect(exit!.number('managementStarted'), 1);
    expect(exit!.flag('exitPrepared'), isTrue);
    expect(exit!.flag('normalTeardownAllowed'), exit!.number('finishHr') == 0);
    if (exit!.flag('normalTeardownAllowed')) {
      expect(exit!.number('managementCompleted'), 1);
      expect(exit!.number('workerJoined'), 1);
      expect(exit!.number('joinStatus'), 0);
      expect(exit!.number('chargedSlots'), 0);
      expect(exit!.number('chargedBytes'), 0);
      expect(exit!.number('inflight'), 0);
    }
    expect(restart!.number('openHr'), -2147024891);
    expect(stoppedProbe, 7);
    expect(close!.joinedAndEmpty, isTrue);
    expect(report['sdkClosed'], isTrue);
  });
}
