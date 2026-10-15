import 'dart:async';
import 'dart:io';

import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/desktop/native_input_spike_sdk/bootstrap.dart';
import 'package:guosh_shell/src/desktop/native_input_spike_sdk/fixture_app.dart';
import 'package:guosh_shell/src/desktop/native_input_spike_sdk/sdk.dart';

// 专用原生 fixture；不启动正常 app、配置、会话或输入 owner。
void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('公共 SDK 实际回应通过同 module 核验且没有输入 owner', (tester) async {
    expect(Platform.isWindows, isTrue);
    expect(const bool.fromEnvironment('GUOSSH_NATIVE_INPUT_SPIKE_SDK'), isTrue);
    final bootstrap = SdkBootstrap(
      native: const MethodChannelFixtureAdapter(),
      sdk: RinfFixtureSdk(),
      normalHandlers: assignRustSignal,
      controlledPath: const String.fromEnvironment(
        'GUOSSH_NATIVE_INPUT_SPIKE_DLL',
      ),
      explicitOptIn: true,
    );
    final finished = Completer<void>();
    void changed() {
      if (!finished.isCompleted &&
          (bootstrap.phase == BootstrapPhase.proven ||
              bootstrap.phase == BootstrapPhase.failed ||
              bootstrap.phase == BootstrapPhase.closed)) {
        finished.complete();
      }
    }

    bootstrap.addListener(changed);
    try {
      await tester.pumpWidget(
        SdkFixtureApp(bootstrap: bootstrap, onClose: bootstrap.close),
      );
      await tester.pumpAndSettle();
      expect(bootstrap.phase, BootstrapPhase.idle);
      await tester.tap(find.byKey(const Key('sdk-fixture-run')));
      await tester.runAsync(
        () => finished.future.timeout(const Duration(seconds: 20)),
      );
      await tester.pump();
      expect(bootstrap.phase, BootstrapPhase.proven, reason: bootstrap.failure);
      final diagnostic = bootstrap.diagnostic!;
      expect(diagnostic.status, 0);
      expect(diagnostic.coreAbi, 2);
      expect(diagnostic.pointerBits, 64);
      expect(diagnostic.owners, 0);
      expect(diagnostic.rinfExports, 4);
      expect(diagnostic.coreExports, 8);
      expect(diagnostic.fixtureOnly, isTrue);
      expect(diagnostic.sameModule, isTrue);
      binding.reportData = {
        'sdkActualReply': true,
        'sameModule': diagnostic.sameModule,
        'fixtureOnly': diagnostic.fixtureOnly,
        'owners': diagnostic.owners,
        'coreAbi': diagnostic.coreAbi,
        'pointerBits': diagnostic.pointerBits,
      };
    } finally {
      bootstrap.removeListener(changed);
      bootstrap.close();
      await tester.pumpWidget(const SizedBox.shrink());
      bootstrap.dispose();
    }
  });
}
