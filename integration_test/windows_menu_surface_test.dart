import 'dart:io';

import 'package:fluent_ui/fluent_ui.dart' as f;
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/main.dart' as app;
import 'package:guosh_shell/src/desktop/catalog/desktop_connection_editor.dart';
import 'package:guosh_shell/src/desktop/desktop_widgets.dart';
import 'package:guosh_shell/src/desktop/settings/desktop_settings_page.dart';
import 'package:guosh_shell/src/desktop/windows_chrome.dart';
import 'package:guosh_shell/src/desktop/windows_shell.dart';
import 'package:integration_test/integration_test.dart';
import 'package:path_provider_platform_interface/path_provider_platform_interface.dart';
import 'package:window_manager/window_manager.dart';

// GUOSH_DESKTOP_TEST_DATA=<隔离目录> [GUOSH_MENU_SHOT_DIR=<截图握手目录>]
// flutter test integration_test/windows_menu_surface_test.dart -d windows --no-pub
// 亚克力开启、默认透明度下打开侧栏连接菜单、设置页与设置页下拉，弹层不得模糊混合
// 后方内容，设置对话框背景不透明。
// 提供握手目录时，菜单（menu）、设置页（settings）、下拉（combo）打开后依次写入
// <名称>.ready（内容为窗口句柄），等待外部截图后的
// <名称>.done 再继续；未提供时仅停留数秒供人工观察。

class _IsolatedPaths extends PathProviderPlatform {
  _IsolatedPaths(this.directory);
  final String directory;
  @override
  Future<String?> getApplicationSupportPath() async => directory;
}

Future<void> _appears(WidgetTester tester, Finder finder) async {
  final deadline = DateTime.now().add(const Duration(seconds: 20));
  while (finder.evaluate().isEmpty) {
    if (DateTime.now().isAfter(deadline)) {
      throw TestFailure('等待界面更新超时：$finder');
    }
    await tester.pump(const Duration(milliseconds: 50));
  }
}

Finder _iconButton(String label) => find.byWidgetPredicate(
  (widget) => widget is DesktopIconButton && widget.label == label,
);

/// 弹层保持打开期间交由外部截图；真实时钟下继续泵帧。
Future<void> _hold(WidgetTester tester, String name) async {
  final shots = Platform.environment['GUOSH_MENU_SHOT_DIR'];
  if (shots == null || shots.isEmpty) {
    await tester.pump(const Duration(seconds: 3));
    return;
  }
  final done = File('$shots${Platform.pathSeparator}$name.done');
  if (done.existsSync()) done.deleteSync();
  await File('$shots${Platform.pathSeparator}$name.ready')
      .writeAsString('${await windowManager.getId()}');
  final deadline = DateTime.now().add(const Duration(seconds: 60));
  while (!done.existsSync()) {
    if (DateTime.now().isAfter(deadline)) throw TestFailure('等待截图超时：$name');
    await tester.pump(const Duration(milliseconds: 100));
  }
}

/// 弹层位于 Acrylic 内，且 DisableAcrylic 生效（无背景模糊，以不透明色调绘制）。
bool _solid(String item) {
  final surface = find.ancestor(
    of: find.text(item).last,
    matching: find.byType(f.Acrylic),
  );
  return surface.evaluate().isNotEmpty &&
      find
          .descendant(of: surface, matching: find.byType(BackdropFilter))
          .evaluate()
          .isEmpty;
}

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  testWidgets('Windows 弹出菜单与下拉在亚克力窗口上不透明', (tester) async {
    final path = Platform.environment['GUOSH_DESKTOP_TEST_DATA'];
    expect(path, isNotNull, reason: '必须提供隔离的数据目录');
    await Directory(path!).create(recursive: true);
    PathProviderPlatform.instance = _IsolatedPaths(path);
    await app.main();
    await _appears(tester, find.byType(WindowsShell));
    final appearance = WindowsAppearance.instance;
    expect(appearance.materialAvailable, isTrue);
    expect(appearance.acrylicEnabled, isTrue);
    expect(appearance.opacity, closeTo(0.78, 0.001));
    await windowManager.setSize(const Size(1000, 700));
    await windowManager.center();
    await tester.pumpAndSettle();

    // 侧栏需要一条保存的连接才有省略号菜单；不保存密码。
    await tester.tap(_iconButton('添加连接'));
    await _appears(tester, find.byType(DesktopConnectionEditor));
    for (final (label, value) in [
      ('名称', 'menu-surface'),
      ('主机', 'localhost'),
      ('用户名', 'menu'),
    ]) {
      final input = find.byKey(ValueKey('catalog-field-$label'));
      await tester.ensureVisible(input);
      await tester.enterText(input, value);
    }
    final savePassword = find.byKey(const Key('catalog-save-password'));
    await tester.ensureVisible(savePassword);
    await tester.pumpAndSettle();
    if (tester.widget<f.ToggleSwitch>(savePassword).checked) {
      await tester.tapAt(
        tester.getTopLeft(savePassword) +
            Offset(20, tester.getSize(savePassword).height / 2),
      );
      await tester.pumpAndSettle();
    }
    await tester.ensureVisible(find.text('保存'));
    await tester.tap(find.text('保存'));
    await _appears(tester, find.text('menu-surface'));
    await tester.pumpAndSettle();

    await tester.tap(_iconButton('连接操作').first);
    await _appears(
      tester,
      find.descendant(of: find.byType(f.MenuFlyout), matching: find.text('编辑')),
    );
    await tester.pumpAndSettle();
    await _hold(tester, 'menu');
    final menuSolid = _solid('编辑');
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(find.byType(f.MenuFlyout), findsNothing);

    await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.comma);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
    await _appears(tester, find.byType(DesktopSettingsPage));
    await tester.pumpAndSettle();
    await _hold(tester, 'settings');
    // 设置对话框自带不透明底：窗口画布与后方主页不得透入。
    final settingsMenu = f.FluentTheme.of(
      tester.element(find.byType(DesktopSettingsPage)),
    ).menuColor;
    final settingsSolid =
        tester
                .widget<ColoredBox>(
                  find
                      .ancestor(
                        of: find.text('设置').first,
                        matching: find.byType(ColoredBox),
                      )
                      .first,
                )
                .color ==
            settingsMenu &&
        settingsMenu.a == 1.0;
    await tester.tap(find.byKey(const ValueKey('settings-nav-app')));
    await tester.pumpAndSettle();
    final combo = find.descendant(
      of: find.byKey(const ValueKey('app-color_scheme')),
      matching: find.byType(f.ComboBox<String>),
    );
    await _appears(tester, combo);
    await tester.ensureVisible(combo);
    await tester.pumpAndSettle();
    final before = find.text('德古拉').evaluate().length;
    await tester.tap(combo);
    await tester.pumpAndSettle();
    // 下拉列表出现后，配色名多出一份（列表项）。
    expect(find.text('德古拉').evaluate().length, greaterThan(before));
    await _hold(tester, 'combo');
    final comboSolid = _solid('德古拉');
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('close-settings')));
    await tester.pumpAndSettle();
    // 截图之后再断言，修复前的对照运行也能拿到两张截图。
    expect(find.byType(f.DisableAcrylic), findsOneWidget);
    expect(menuSolid, isTrue, reason: '侧栏连接菜单不应模糊并混合后方内容');
    expect(comboSolid, isTrue, reason: '设置页下拉不应模糊并混合后方内容');
    expect(settingsSolid, isTrue, reason: '设置对话框背景必须不透明');
    expect(tester.takeException(), isNull);
  }, skip: !Platform.isWindows);
}
