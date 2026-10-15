import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:fluent_ui/fluent_ui.dart' as f;
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/main.dart' as app;
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/desktop/desktop_app_color_scheme.dart';
import 'package:guosh_shell/src/desktop/settings/desktop_settings_controller.dart';
import 'package:guosh_shell/src/desktop/settings/desktop_settings_gateway.dart';
import 'package:guosh_shell/src/desktop/settings/desktop_settings_page.dart';
import 'package:guosh_shell/src/desktop/windows_chrome.dart';
import 'package:guosh_shell/src/desktop/windows_shell.dart';
import 'package:integration_test/integration_test.dart';
import 'package:path_provider_platform_interface/path_provider_platform_interface.dart';
import 'package:rinf/rinf.dart';

// GUOSH_DESKTOP_TEST_DATA=<隔离目录> flutter test integration_test/desktop_settings_e2e_test.dart -d windows --no-pub
// 经真实设置界面编辑 profile 与应用配置，再用独立协议查询逐字段读回；数据库为本轮 createTemp 新建。

class _IsolatedPaths extends PathProviderPlatform {
  _IsolatedPaths(this.directory);
  final String directory;
  @override
  Future<String?> getApplicationSupportPath() async => directory;
}

const _timeout = Duration(seconds: 20);

Future<void> _wait(
  WidgetTester tester,
  String description,
  bool Function() ready,
) async {
  final elapsed = Stopwatch()..start();
  while (!ready()) {
    if (elapsed.elapsed >= _timeout) throw TestFailure('等待超时：$description');
    // 真实时钟下泵帧，让 Rust 信号与布局继续处理。
    await tester.pump(const Duration(milliseconds: 50));
    expect(tester.takeException(), isNull, reason: description);
  }
}

Future<RustSignalPack<T>> _exchange<T>(
  WidgetTester tester,
  Stream<RustSignalPack<T>> stream,
  bool Function(T) matches,
  void Function() send,
  String description,
) async {
  RustSignalPack<T>? response;
  // 先订阅再发送；只接收本轮响应。
  final listener = stream.listen((pack) {
    if (matches(pack.message)) response ??= pack;
  });
  try {
    send();
    await _wait(tester, description, () => response != null);
    return response!;
  } finally {
    await listener.cancel();
  }
}

/// 不经设置 gateway 的独立查询，读回 Rust 实际保存的配置。
Future<DesktopConfiguration> _query(WidgetTester tester, String id) async {
  final result = (await _exchange(
    tester,
    DesktopSettingsResult.rustSignalStream,
    (message) => message.requestId == id,
    () => DesktopSettingsRequest(
      protocolVersion: 1,
      requestId: id,
      baseRevision: Uint64(BigInt.zero),
      operation: 'query',
      targetId: '',
      name: '',
    ).sendSignalToRust(),
    id,
  )).message;
  expect(result.status, 'ok', reason: '${result.detail} ${result.fieldErrors}');
  return result.snapshot!;
}

Future<Map<String, Object?>> _preferences(Directory support) async =>
    jsonDecode(
      await File('${support.path}${Platform.pathSeparator}preferences.json')
          .readAsString(),
    ) as Map<String, Object?>;

DesktopSettingsController _controller(WidgetTester tester) => tester
    .widget<DesktopSettingsPage>(find.byType(DesktopSettingsPage))
    .controller;

Finder _field(String field) => find.byKey(ValueKey('field-$field'));

f.ComboBox<String> _choice(WidgetTester tester, String key) =>
    tester.widget<f.ComboBox<String>>(
      find.descendant(
        of: find.byKey(ValueKey(key)),
        matching: find.byType(f.ComboBox<String>),
      ),
    );

Future<void> _tap(WidgetTester tester, Finder finder) async {
  await tester.ensureVisible(finder);
  await tester.pump();
  await tester.tap(finder);
  await tester.pump();
}

Future<void> _enter(WidgetTester tester, String field, String text) async {
  await tester.ensureVisible(_field(field));
  await tester.enterText(_field(field), text);
  await tester.pump();
}

Future<void> _openSettings(WidgetTester tester) async {
  await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
  await tester.sendKeyEvent(LogicalKeyboardKey.comma);
  await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
  await _wait(
    tester,
    '设置页打开并载入配置',
    () =>
        find.byType(DesktopSettingsPage).evaluate().isNotEmpty &&
        _controller(tester).editable,
  );
}

Future<void> _section(WidgetTester tester, String name) async {
  await _tap(tester, find.byKey(ValueKey('settings-nav-$name')));
  await tester.pumpAndSettle();
}

Future<void> _saved(WidgetTester tester, String unit) =>
    _wait(tester, '$unit 写入已应用', () {
      final c = _controller(tester);
      return c.blockingWrite == null &&
          c.stateFor(unit).state == DesktopSettingsWriteState.applied;
    });

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('设置页编辑 profile 与应用配置，独立协议逐字段读回，应用配色即时生效', (tester) async {
    if (!Platform.isWindows) {
      throw TestFailure('此 e2e 只允许在 Windows 桌面设备上运行');
    }
    final root = Platform.environment['GUOSH_DESKTOP_TEST_DATA'];
    expect(root, isNotNull, reason: '必须提供隔离的数据目录');
    await Directory(root!).create(recursive: true);
    // 每轮新库：只在调用方目录下新建并删除本轮独占目录。
    final owned = await Directory(root).createTemp('settings-e2e-');
    try {
      PathProviderPlatform.instance = _IsolatedPaths(owned.path);
      await app.main();
      await _wait(
        tester,
        '工作区出现',
        () => find.byType(WindowsShell).evaluate().isNotEmpty,
      );
      // 启动查询经 gateway 载入应用配色。
      await _wait(
        tester,
        '启动查询应用配色',
        () => DesktopSettingsGateway.instance.snapshot != null,
      );
      expect(DesktopAppColorScheme.instance.value, 'default');
      await tester.pumpAndSettle();

      // —— 配置文件页：新建 profile ——
      await _openSettings(tester);
      await _section(tester, 'profile');
      final before = _controller(tester).profiles.keys.toSet();
      await _tap(tester, find.byKey(const ValueKey('profile-create')));
      await tester.pumpAndSettle();
      await tester.enterText(
        find.descendant(
          of: find.byKey(const ValueKey('lifecycle-name')),
          matching: find.byType(f.TextBox),
        ),
        'E2E 配置',
      );
      await tester.pump();
      await _tap(tester, find.byKey(const ValueKey('lifecycle-confirm')));
      await _wait(
        tester,
        '新建 profile 出现在草稿中',
        () =>
            _controller(tester).blockingWrite == null &&
            _controller(tester).profiles.length == before.length + 1,
      );
      final id = _controller(tester).profiles.keys
          .singleWhere((key) => !before.contains(key));
      _choice(tester, 'selected-profile').onChanged!(id);
      await tester.pumpAndSettle();
      expect(find.byKey(ValueKey('profile-form-$id')), findsOneWidget);

      // —— 改若干字段并保存 ——
      final mouseBefore = _controller(tester)
          .profiles[id]!
          .value
          .mouseReporting;
      await _enter(tester, 'name', 'E2E 配置改');
      await _enter(tester, 'initial_cols', '101');
      await _enter(tester, 'initial_rows', '31');
      await _enter(tester, 'answerback', 'e2e-answer');
      await _enter(tester, 'font_size', '17');
      await _enter(tester, 'scrollback_lines', '4321');
      await _tap(tester, find.byKey(const ValueKey('mouse_reporting')));
      _choice(tester, 'profile-color_scheme').onChanged!('nord');
      await tester.pumpAndSettle();
      await _tap(tester, find.byKey(const ValueKey('save-profile')));
      await _saved(tester, id);

      // —— 设为默认，在应用页保存 ——
      await _tap(tester, find.byKey(const ValueKey('profile-default')));
      await tester.pumpAndSettle();
      expect(_controller(tester).app!.value.defaultTerminalProfile, id);
      await _tap(tester, find.byKey(const ValueKey('save-app')));
      await _saved(tester, 'app');

      // —— 独立协议逐字段读回 ——
      final saved = await _query(tester, 'settings-e2e-query-1');
      expect(saved.app.defaultTerminalProfile, id);
      final profile = saved.profiles.singleWhere((p) => p.id == id);
      expect(profile.name, 'E2E 配置改');
      expect(profile.initialCols, 101);
      expect(profile.initialRows, 31);
      expect(profile.answerback, 'e2e-answer');
      expect(profile.fontSize, 17.0);
      expect(profile.scrollbackLines, 4321);
      expect(profile.mouseReporting, !mouseBefore);
      expect(profile.colorScheme, 'nord');

      // —— 关闭设置重开，表单显示已保存值 ——
      await _tap(tester, find.byKey(const ValueKey('close-settings')));
      await tester.pumpAndSettle();
      expect(find.byType(DesktopSettingsPage), findsNothing);
      await _openSettings(tester);
      await _section(tester, 'profile');
      expect(_controller(tester).selectedProfileId, id);
      String text(String field) =>
          tester.widget<f.TextBox>(_field(field)).controller!.text;
      expect(text('name'), 'E2E 配置改');
      expect(text('initial_cols'), '101');
      expect(text('initial_rows'), '31');
      expect(text('answerback'), 'e2e-answer');
      expect(double.parse(text('font_size')), 17.0);
      expect(text('scrollback_lines'), '4321');
      expect(
        tester
            .widget<f.ToggleSwitch>(
              find.byKey(const ValueKey('mouse_reporting')),
            )
            .checked,
        !mouseBefore,
      );
      expect(_choice(tester, 'profile-color_scheme').value, 'nord');

      // —— 应用配色选德古拉，保存后壳主题立即变化 ——
      await _section(tester, 'app');
      Color background() =>
          f.FluentTheme.of(tester.element(find.byType(DesktopSettingsPage)))
              .scaffoldBackgroundColor;
      final themeBefore = background();
      _choice(tester, 'app-color_scheme').onChanged!('dracula');
      await tester.pumpAndSettle();
      expect(background(), themeBefore, reason: '未保存的草稿不改主题');
      await _tap(tester, find.byKey(const ValueKey('save-app')));
      await _saved(tester, 'app');
      await tester.pump();
      final dracula = desktopAppPalette('dracula')!;
      expect(DesktopAppColorScheme.instance.value, 'dracula');
      expect(background(), isNot(themeBefore));
      expect(background(), dracula.surface);
      expect(
        tester.widget<WindowsFrame>(find.byType(WindowsFrame)).background,
        dracula.canvas,
      );
      expect(
        (await _query(tester, 'settings-e2e-query-2')).app.colorScheme,
        'dracula',
      );

      // —— 键位条页拨动显示开关，读回偏好文件 ——
      await _section(tester, 'keybar');
      final toggle = find.byKey(const ValueKey('settings-show-keybar'));
      final shown = tester.widget<f.ToggleSwitch>(toggle).checked;
      SettingsState? state;
      // 先订阅再拨动；Rust 写入偏好后回报新状态。
      final states = SettingsState.rustSignalStream.listen((pack) {
        if (pack.message.showKeyBar == !shown) state ??= pack.message;
      });
      try {
        await tester.tap(toggle);
        await _wait(tester, '键位条显示开关保存', () => state != null);
      } finally {
        await states.cancel();
      }
      await tester.pumpAndSettle();
      expect(tester.widget<f.ToggleSwitch>(toggle).checked, !shown);
      expect((await _preferences(owned))['show_key_bar'], !shown);

      await _tap(tester, find.byKey(const ValueKey('close-settings')));
      await tester.pumpAndSettle();
      expect(find.byType(DesktopSettingsPage), findsNothing);
      expect(tester.takeException(), isNull);
    } finally {
      try {
        await tester.pumpWidget(const SizedBox.shrink());
      } finally {
        // 同步退出 Rust 并 join SQLite worker 后，只删除本轮 createTemp 目录。
        finalizeRust();
        await owned.delete(recursive: true);
        expect(await owned.exists(), isFalse);
      }
    }
  });
}
