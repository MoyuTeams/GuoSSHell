import 'dart:async';

import 'package:fluent_ui/fluent_ui.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/desktop/native_input_spike_sdk/bootstrap.dart';
import 'package:guosh_shell/src/desktop/native_input_spike_sdk/fixture_app.dart';
import 'package:guosh_shell/src/desktop/native_input_spike_sdk/protocol.dart';
import 'package:guosh_shell/src/desktop/native_input_spike_sdk/sdk.dart';

class UnavailableNative implements FixtureNativeAdapter {
  int calls = 0;
  Completer<FixturePreflight>? pending;
  @override
  Future<FixturePreflight> preflight({
    required String controlledPath,
    required bool explicitOptIn,
    required Uint8List nonce,
  }) async {
    calls++;
    if (pending != null) return pending!.future;
    throw MissingPluginException();
  }

  @override
  Future<FixtureDiagnostic> verify(Uint8List actualReply) =>
      throw StateError('测试不应 verify');
}

class UnstartedSdk implements FixtureSdk {
  int starts = 0;
  @override
  Future<void> initialize(
    Map<String, FixtureSignalHandler> handlers,
    String path,
  ) async {
    starts++;
  }

  @override
  void sendProbe(Uint8List request) => throw StateError('测试不应发送');
  @override
  void finalize() => throw StateError('测试不应 finalize');
}

void main() {
  for (final width in [375.0, 768.0, 1280.0]) {
    testWidgets('只读面板宽度 $width，文字缩放／高对比度／减少动画', (tester) async {
      tester.view.physicalSize = Size(width, 900);
      tester.view.devicePixelRatio = 1;
      tester.platformDispatcher.textScaleFactorTestValue = 1.4;
      tester.platformDispatcher.accessibilityFeaturesTestValue =
          const FakeAccessibilityFeatures(
            highContrast: true,
            disableAnimations: true,
          );
      addTearDown(tester.view.reset);
      addTearDown(tester.platformDispatcher.clearAllTestValues);
      final native = UnavailableNative();
      final sdk = UnstartedSdk();
      final bootstrap = SdkBootstrap(
        native: native,
        sdk: sdk,
        normalHandlers: const {},
        controlledPath: '',
        explicitOptIn: false,
      );
      addTearDown(bootstrap.dispose);
      var closes = 0;
      await tester.pumpWidget(
        SdkFixtureApp(
          bootstrap: bootstrap,
          onClose: () {
            closes++;
          },
        ),
      );
      expect(find.byType(TextBox), findsNothing);
      expect(find.text('默认关闭；SDK 未启动。'), findsOneWidget);
      expect(
        tester
            .widget<FilledButton>(find.byKey(const Key('sdk-fixture-run')))
            .onPressed,
        isNull,
      );
      await tester.tap(find.byKey(const Key('sdk-fixture-close')));
      await tester.pumpAndSettle();
      expect(closes, 1);
      expect(native.calls, 0);
      expect(sdk.starts, 0);
      expect(tester.takeException(), isNull);
    });
  }

  testWidgets('Tab/Enter 单次激活，缺宿主时失败且无重试', (tester) async {
    final native = UnavailableNative();
    final sdk = UnstartedSdk();
    final bootstrap = SdkBootstrap(
      native: native,
      sdk: sdk,
      normalHandlers: const {},
      controlledPath: 'controlled-fixture.dll',
      explicitOptIn: true,
    );
    addTearDown(bootstrap.dispose);
    await tester.pumpWidget(
      SdkFixtureApp(bootstrap: bootstrap, onClose: bootstrap.close),
    );
    await tester.sendKeyEvent(LogicalKeyboardKey.tab);
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pumpAndSettle();
    expect(native.calls, 1);
    expect(sdk.starts, 0);
    expect(bootstrap.phase, BootstrapPhase.failed);
    expect(
      tester
          .widget<FilledButton>(find.byKey(const Key('sdk-fixture-run')))
          .onPressed,
      isNull,
    );
    expect(find.textContaining('bootstrap 不可用'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('预检等待仍可关闭，迟到结果不启动 SDK', (tester) async {
    final native = UnavailableNative()..pending = Completer<FixturePreflight>();
    final sdk = UnstartedSdk();
    final bootstrap = SdkBootstrap(
      native: native,
      sdk: sdk,
      normalHandlers: const {},
      controlledPath: 'controlled-fixture.dll',
      explicitOptIn: true,
    );
    addTearDown(bootstrap.dispose);
    await tester.pumpWidget(
      SdkFixtureApp(bootstrap: bootstrap, onClose: bootstrap.close),
    );
    await tester.tap(find.byKey(const Key('sdk-fixture-run')));
    await tester.pump();
    expect(find.textContaining('SDK 尚未启动'), findsOneWidget);
    await tester.tap(find.byKey(const Key('sdk-fixture-close')));
    native.pending!.completeError(MissingPluginException());
    await tester.pumpAndSettle();
    expect(bootstrap.phase, BootstrapPhase.closed);
    expect(sdk.starts, 0);
    expect(tester.takeException(), isNull);
  });
}
