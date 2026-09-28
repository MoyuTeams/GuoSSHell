import 'dart:io';

import 'package:fluent_ui/fluent_ui.dart' as f;
import 'package:flutter/services.dart';
import 'package:flutter/foundation.dart' show ValueKey;
import 'package:flutter/material.dart' show Dialog, SizedBox, showDialog;
import 'package:file_selector/file_selector.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:path_provider_platform_interface/path_provider_platform_interface.dart';
import 'package:window_manager/window_manager.dart';
import 'package:guosh_shell/main.dart' as app;
import 'package:guosh_shell/src/desktop/windows_chrome.dart';
import 'package:guosh_shell/src/desktop/windows_shell.dart';
import 'package:guosh_shell/src/desktop/windows_settings.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/settings/interface_font.dart';
import 'package:guosh_shell/src/settings/interface_font_setting.dart';

/// 原生验收的数据完全放在调用方提供的构建目录，不使用个人连接与凭证。
class _IsolatedPaths extends PathProviderPlatform {
  _IsolatedPaths(this.directory);
  final String directory;
  @override
  Future<String?> getApplicationSupportPath() async => directory;
}

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  testWidgets('Windows 原生窗口、Fluent 设置与透明度持久化', (tester) async {
    final path = Platform.environment['GUOSH_DESKTOP_TEST_DATA'];
    expect(path, isNotNull, reason: '必须提供隔离的数据目录');
    await Directory(path!).create(recursive: true);
    PathProviderPlatform.instance = _IsolatedPaths(path);
    await app.main();
    final deadline = DateTime.now().add(const Duration(seconds: 30));
    while (find.byType(WindowsShell).evaluate().isEmpty &&
        DateTime.now().isBefore(deadline)) {
      await tester.pump(const Duration(milliseconds: 100));
    }
    expect(find.byType(WindowsShell), findsOneWidget);
    expect(WindowsAppearance.instance.materialAvailable, isTrue);
    await tester.pumpAndSettle();

    for (final size in [const Size(760, 520), const Size(1360, 860)]) {
      await windowManager.setSize(size);
      await tester.pumpAndSettle();
      expect(find.text('连接，开始工作。'), findsOneWidget);
      expect(tester.takeException(), isNull);
    }
    await windowManager.maximize();
    await tester.pumpAndSettle();
    expect(await windowManager.isMaximized(), isTrue);
    await windowManager.unmaximize();
    await tester.pumpAndSettle();
    expect(await windowManager.isMaximized(), isFalse);

    await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.comma);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
    await tester.pumpAndSettle();
    expect(find.byType(WindowsSettings), findsOneWidget);
    final fontBytes = (await rootBundle.load(
      'assets/fonts/MesloLGS-NF-Regular.ttf',
    )).buffer.asUint8List();
    for (final terminal in [false, true]) {
      final slot = terminal ? 'terminal' : 'interface';
      final controller = terminal
          ? InterfaceTypography.terminal
          : InterfaceTypography.instance;
      final modal = showDialog<void>(
        context: tester.element(find.byType(WindowsSettings)),
        builder: (_) => Dialog(
          child: SizedBox(
            width: 600,
            child: InterfaceFontSetting(
              terminal: terminal,
              controller: controller,
              pickFile: () async => XFile.fromData(
                fontBytes,
                name: 'native-test.ttf',
                path: 'native-test.ttf',
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(ValueKey('pick-$slot-font')).last);
      await tester.pumpAndSettle();
      expect(controller.custom, isFalse);
      await tester.tap(find.byKey(ValueKey('apply-$slot-font')));
      await tester.pumpAndSettle();
      expect(controller.custom, isTrue);
      final loaded = controller.family;
      await controller.refresh();
      expect(controller.family, loaded);
      await tester.tap(find.text(terminal ? '恢复内置等宽字体' : '恢复内置 MiSans').last);
      await tester.pumpAndSettle();
      expect(controller.family, controller.defaultFamily);
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
      await modal;
    }
    final stateFuture = WindowsAppearanceState.rustSignalStream.first;
    final slider = tester.widget<f.Slider>(find.byType(f.Slider).first);
    slider.onChanged!(35);
    slider.onChangeEnd!(35);
    final saved = (await stateFuture.timeout(const Duration(seconds: 10)))
        .message;
    expect(saved.opacity, closeTo(0.65, 0.001));
    expect(saved.error, isEmpty);
    await tester.pumpAndSettle();
    expect(find.text('35%'), findsOneWidget);
    await tester.tap(find.text('完成'));
    await tester.pumpAndSettle();
    await tester.tap(find.widgetWithText(f.FilledButton, '快速连接'));
    await tester.pumpAndSettle();
    expect(find.text('身份验证'), findsNothing);
    expect(find.text('主机'), findsOneWidget);
    await tester.tap(find.text('取消'));
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
    binding.reportData = {
      'windows_desktop': 'passed',
      'opacity': WindowsAppearance.instance.opacity,
    };
  }, skip: !Platform.isWindows);
}
