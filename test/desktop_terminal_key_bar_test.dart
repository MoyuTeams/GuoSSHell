import 'package:fluent_ui/fluent_ui.dart' as f;
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/desktop/desktop_theme.dart';
import 'package:guosh_shell/src/desktop/native_input/desktop_native_host.dart';
import 'package:guosh_shell/src/terminal/desktop_font.dart';
import 'package:guosh_shell/src/terminal/desktop_session_controller.dart';
import 'package:guosh_shell/src/terminal/desktop_terminal_key_bar.dart';
import 'package:guosh_shell/src/terminal/key_bar_layout.dart';
import 'package:guosh_shell/src/terminal/session_target.dart';
import 'package:rinf/rinf.dart';

import 'desktop_session_controller_test.dart'
    show DesktopSessionTestGateway, source, u64, prepared;

class _Harness {
  final gateway = DesktopSessionTestGateway();
  late final DesktopSessionController session;
  final actions = <String>[];
  int edits = 0;
  final rows = ValueNotifier<List<List<String>>>([
    ['ctrl', 'alt', 'up', 'ctrlC', 'text:🙂'],
    ['copy', 'paste', 'disconnect', 'zoomIn', 'zoomOut', 'zoomReset'],
  ]);
  Future<void> finish(WidgetTester tester) async {
    await tester.pumpWidget(const SizedBox.shrink());
    session.dispose();
    await tester.pump(const Duration(milliseconds: 200));
  }

  void settle({
    NativeCommandAdmission admission = NativeCommandAdmission.accepted,
  }) {
    final command = gateway.commands.last;
    gateway.commandBus.add(
      RustSignalPack(
        DesktopNativeCommandResult(
          version: command.version,
          source: command.source,
          identity: command.identity,
          attempt: command.attempt,
          focusEpoch: command.focusEpoch,
          requestId: command.requestId,
          admission: admission,
          effect: u64(1),
          consumedLatches: 0,
          failure: NativeCommandFailure.none,
          guiModifierRevision: (command.command as NativeCommandGuiIntent)
              .value
              .modifierRevision,
        ),
        Uint8List(0),
      ),
    );
  }

  Future<void> mount(
    WidgetTester tester, {
    double width = 1280,
    bool contrast = false,
    double scale = 1,
  }) async {
    tester.view.physicalSize = Size(width, 720);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    const channel = MethodChannel('test/gui-keybar');
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    messenger.setMockMethodCallHandler(
      channel,
      (call) async => switch (call.method) {
        'attachPane' => {
          'version': 1,
          'state': 'attached',
          'paneToken': 'bar-pane',
        },
        'requestFocus' => {
          'version': 1,
          'state': 'active',
          'attempt': '12',
          'focusEpoch': '4',
        },
        'retirePane' => {
          'version': 1,
          'state': (call.arguments as Map)['phase'] == 'quiet'
              ? 'nativequiet'
              : 'released',
        },
        _ => throw MissingPluginException(),
      },
    );
    session = DesktopSessionController(
      source: source,
      target: const SessionTarget.quick(
        host: 'example.test',
        port: 22,
        username: 'test',
      ),
      gateway: gateway,
      host: DesktopNativeHost(channel: channel),
      font: DesktopFontController(
        cache: DesktopFontCache(register: (_, _) async {}),
      ),
    )..canFocus = () => true;
    await tester.runAsync(() async {
      session.begin();
      gateway.emit(prepared(gateway.prepares.single.requestId));
      await Future<void>.delayed(Duration.zero);
      session.measure(
        const DesktopGeometry(
          cols: 90,
          rows: 30,
          pixelWidth: 720,
          pixelHeight: 600,
          dpi: 96,
        ),
        {'width': 720.0},
      );
      gateway.lifecycle(NativeLifecycleState.authenticated);
      await Future<void>.delayed(Duration.zero);
    });
    await tester.pumpWidget(
      f.FluentApp(
        theme: desktopTheme(highContrast: contrast, reduceMotion: true),
        home: MediaQuery(
          data: MediaQueryData(
            size: Size(width, 720),
            textScaler: TextScaler.linear(scale),
            highContrast: contrast,
            disableAnimations: true,
          ),
          child: Align(
            alignment: Alignment.bottomLeft,
            child: ValueListenableBuilder<List<List<String>>>(
              valueListenable: rows,
              builder: (_, value, _) => DesktopTerminalKeyBar(
                session: session,
                rows: value,
                canCopy: () => false,
                onAction: actions.add,
                onEdit: () => edits++,
              ),
            ),
          ),
        ),
      ),
    );
    addTearDown(() async {
      await tester.pumpWidget(const SizedBox.shrink());
      session.dispose();
      rows.dispose();
      await gateway.dispose();
      messenger.setMockMethodCallHandler(channel, null);
    });
  }
}

void main() {
  test('normal 按钮全集映射 Rust GUI key 或原样 scalar text', () {
    for (final button in keyBarButtons.where(
      (button) => button.key != null || button.text != null,
    )) {
      expect(guiInputForButton(button), isNotNull, reason: button.id);
    }
    expect(
      guiInputForButton(keyBarButton('text:中🙂\r\n')!),
      const GuiInputGuiText(value: '中🙂\r\n'),
    );
    expect(
      guiInputForButton(keyBarButton('ctrlC')!),
      const GuiInputGuiKey(value: GuiKeyCtrlC()),
    );
    expect(
      guiInputForButton(keyBarButton('f12')!),
      const GuiInputGuiKey(value: GuiKeyF(value: 12)),
    );
  });

  for (final width in [375.0, 768.0, 1280.0]) {
    testWidgets('normal Fluent 双排键位条 $width 宽修饰状态语义、操作、焦点与无溢出', (tester) async {
      final harness = _Harness();
      await harness.mount(
        tester,
        width: width,
        contrast: width == 768,
        scale: width == 375 ? 1.3 : 1,
      );
      final semantics = tester.ensureSemantics();
      await tester.tap(find.text('Ctrl\n关闭'));
      await tester.pump();
      expect(harness.session.isModifierLatched('ctrl'), isTrue);
      expect(find.text('Ctrl\n单次'), findsOneWidget);
      expect(find.bySemanticsLabel(RegExp('Ctrl，单次')), findsWidgets);
      await tester.tap(find.text('Ctrl\n单次'));
      await tester.pump();
      expect(harness.session.isModifierLocked('ctrl'), isTrue);
      await tester.tap(find.text('Ctrl\n锁定'));
      await tester.pump();
      expect(harness.session.isModifierLocked('ctrl'), isFalse);
      final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
      await mouse.addPointer(location: tester.getCenter(find.text('↑')));
      await tester.pump();
      await tester.sendKeyEvent(LogicalKeyboardKey.tab);
      await tester.pump();
      final ctrlButton = find.ancestor(
        of: find.text('Ctrl\n关闭'),
        matching: find.byType(f.Button),
      );
      final controlContext = tester.element(ctrlButton);
      final theme = f.FluentTheme.of(controlContext);
      final foreground = theme.typography.caption!.color!;
      for (final states in [
        <WidgetState>{},
        {WidgetState.hovered},
        {WidgetState.pressed},
        {WidgetState.disabled},
      ]) {
        final background = Color.alphaBlend(
          f.ButtonThemeData.buttonColor(controlContext, states),
          theme.scaffoldBackgroundColor,
        );
        final light = foreground.computeLuminance();
        final dark = background.computeLuminance();
        expect((light + 0.05) / (dark + 0.05), greaterThanOrEqualTo(4.5));
      }
      final focus = tester.widget<FocusableActionDetector>(
        find.descendant(
          of: ctrlButton,
          matching: find.byType(FocusableActionDetector),
        ),
      );
      focus.focusNode!.requestFocus();
      await tester.pump();
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pump();
      expect(harness.session.isModifierLatched('ctrl'), isTrue);
      expect(tester.takeException(), isNull);
      await tester.tap(find.text('复制'));
      expect(harness.actions, isEmpty);
      await tester.tap(find.text('粘贴'));
      expect(harness.actions, ['paste']);
      final settledSize = tester.getSize(find.byType(DesktopTerminalKeyBar));
      await tester.tap(find.text('↑'));
      await tester.pump();
      expect(harness.gateway.commands.length, 1);
      expect(find.text('正在确认输入受理'), findsOneWidget);
      expect(tester.getSize(find.byType(DesktopTerminalKeyBar)), settledSize);
      harness.settle(admission: NativeCommandAdmission.rejected);
      await tester.pump();
      expect(find.text('输入未受理，修饰状态保留'), findsOneWidget);
      expect(tester.getSize(find.byType(DesktopTerminalKeyBar)), settledSize);
      await tester.tap(find.text('↑'));
      await tester.pump();
      expect(harness.gateway.commands.length, 2);
      harness.settle(admission: NativeCommandAdmission.unknown);
      await tester.pump();
      expect(harness.session.guiUnknown, isTrue);
      expect(tester.getSize(find.byType(DesktopTerminalKeyBar)), settledSize);
      await tester.tap(find.text('↑'));
      expect(harness.gateway.commands.length, 2);
      expect(tester.takeException(), isNull);
      await mouse.removePointer();
      semantics.dispose();
      await harness.finish(tester);
    });
  }

  testWidgets(
    'normal 长按立即单次，200ms 重复不积压，release/布局/quiet/Unknown/dispose 均停止',
    (tester) async {
      final harness = _Harness();
      await harness.mount(tester);
      Future<TestGesture> hold() async {
        final gesture = await tester.startGesture(
          tester.getCenter(find.text('↑')),
        );
        await tester.pump(const Duration(milliseconds: 600));
        return gesture;
      }

      final first = await hold();
      expect(harness.gateway.commands.length, 1);
      await tester.pump(const Duration(milliseconds: 600));
      expect(harness.gateway.commands.length, 1);
      harness.settle();
      await tester.pump(const Duration(milliseconds: 200));
      expect(harness.gateway.commands.length, 2);
      harness.settle();
      await first.up();
      await tester.pump(const Duration(milliseconds: 600));
      expect(harness.gateway.commands.length, 2);
      final second = await hold();
      expect(harness.gateway.commands.length, 3);
      harness.settle();
      harness.rows.value = [
        ['ctrl', 'up'],
        ['paste'],
      ];
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 600));
      expect(harness.gateway.commands.length, 3);
      await second.cancel();
      final third = await hold();
      expect(harness.gateway.commands.length, 4);
      harness.settle(admission: NativeCommandAdmission.unknown);
      await tester.pump(const Duration(milliseconds: 600));
      harness.settle();
      await tester.pump(const Duration(milliseconds: 600));
      expect(harness.gateway.commands.length, 4);
      await third.up();
      final fourth = await hold();
      expect(harness.gateway.commands.length, 5);
      harness.settle();
      await tester.runAsync(harness.session.quiet);
      await tester.pump(const Duration(milliseconds: 600));
      expect(harness.gateway.commands.length, 5);
      await fourth.up();
      await tester.runAsync(harness.session.requestFocus);
      await tester.pump();
      final fifth = await hold();
      expect(harness.gateway.commands.length, 6);
      harness.settle();
      await tester.pumpWidget(const SizedBox.shrink());
      await tester.pump(const Duration(milliseconds: 600));
      expect(harness.gateway.commands.length, 6);
      await fifth.up();
      await harness.finish(tester);
    },
  );

  testWidgets('normal 修饰长按直接锁定，编辑先停 repeat 清状态', (tester) async {
    final harness = _Harness();
    await harness.mount(tester);
    await tester.longPress(find.text('Alt\n关闭'));
    await tester.pump();
    expect(harness.session.isModifierLocked('alt'), isTrue);
    await tester.tap(find.byIcon(f.FluentIcons.edit));
    await tester.pump();
    expect(harness.edits, 1);
    expect(harness.session.isModifierLocked('alt'), isFalse);
    await harness.finish(tester);
  });
}
