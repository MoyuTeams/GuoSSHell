import 'dart:ffi';
import 'dart:io';
import 'dart:convert';

import 'package:fluent_ui/fluent_ui.dart' as f;
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:path_provider_platform_interface/path_provider_platform_interface.dart';
import 'package:terminal_view/terminal_view.dart';
import 'package:window_manager/window_manager.dart';
import 'package:guosh_shell/main.dart' as app;
import 'package:guosh_shell/src/terminal/terminal_pane.dart';

class _IsolatedPaths extends PathProviderPlatform {
  _IsolatedPaths(this.directory);
  final String directory;
  @override
  Future<String?> getApplicationSupportPath() async => directory;
}

/// 向本测试窗口的 Flutter 子窗口发送原生键盘消息，经过 Windows 引擎文本输入插件。
class _NativeInput {
  final _user32 = DynamicLibrary.open('user32.dll');
  late final _child = _user32
      .lookupFunction<
        Pointer<Void> Function(Pointer<Void>, Uint32),
        Pointer<Void> Function(Pointer<Void>, int)
      >('GetWindow');
  late final _post = _user32
      .lookupFunction<
        Int32 Function(Pointer<Void>, Uint32, UintPtr, IntPtr),
        int Function(Pointer<Void>, int, int, int)
      >('PostMessageW');
  late final _scan = _user32
      .lookupFunction<Uint32 Function(Uint32, Uint32), int Function(int, int)>(
        'MapVirtualKeyW',
      );
  late final Pointer<Void> window;
  Future<void> initialize() async {
    window = _child(Pointer.fromAddress(await windowManager.getId()), 5);
    expect(window.address, isNonZero);
  }

  void key(int virtualKey, {bool extended = false}) {
    final flags = 1 | (_scan(virtualKey, 0) << 16) | (extended ? 1 << 24 : 0);
    expect(_post(window, 0x100, virtualKey, flags), isNonZero);
    // 消息循环的 TranslateMessage 负责生成 WM_CHAR，不能再手工发送一份。
    expect(_post(window, 0x101, virtualKey, flags | (3 << 30)), isNonZero);
  }

  void text(String value) {
    for (final unit in value.codeUnits) {
      key(unit >= 0x61 && unit <= 0x7a ? unit - 32 : unit);
    }
  }

  // Windows IME 完成提交后的字符消息；不绕过引擎调用 Dart 的文本接口。
  void commit(String value) {
    for (final unit in value.codeUnits) {
      expect(_post(window, 0x102, unit, 1), isNonZero);
    }
  }
}

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  testWidgets('Windows 原生文字输入经 SSH 回显', (tester) async {
    final data = Platform.environment['GUOSH_DESKTOP_TEST_DATA'];
    final log = Platform.environment['GUOSH_DEMO_INPUT_LOG'];
    expect(data, isNotNull);
    expect(log, isNotNull);
    await Directory(data!).create(recursive: true);
    // 测试服务器由本机启动，直接固定其公钥；不接受任意远端主机密钥。
    final publicKey = await File(
      '${Platform.environment['GUOSH_DEMO_DATA']}/hostkey.pub',
    ).readAsString();
    final fields = publicKey.trim().split(RegExp(r'\s+'));
    await File('$data/known_hosts').writeAsString(
      '[${Platform.environment['GUOSH_HOST']}]:${Platform.environment['GUOSH_PORT']} ${fields[0]} ${fields[1]}\n',
    );
    PathProviderPlatform.instance = _IsolatedPaths(data);
    await app.main();
    final native = _NativeInput();
    await native.initialize();
    Future<void> tick([int ms = 200]) =>
        tester.pump(Duration(milliseconds: ms));
    Future<void> shortcut(LogicalKeyboardKey key) async {
      await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
      await tester.sendKeyEvent(key);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
      await tick(350);
    }

    Future<void> passwordIfNeeded() async {
      if (find.text('输入密码').evaluate().isEmpty) return;
      await tick(300);
      native.text('probe');
      await tick(300);
      expect(
        tester
            .widget<EditableText>(find.byType(EditableText).last)
            .controller
            .text,
        'probe',
      );
      await tester.tap(find.widgetWithText(f.FilledButton, '连接').last);
      await tick(300);
    }

    final deadline = DateTime.now().add(const Duration(seconds: 35));
    while (true) {
      await tester.pump(const Duration(milliseconds: 100));
      if (find.text('信任并连接').evaluate().isNotEmpty) {
        await tester.tap(find.text('信任并连接'));
      }
      await passwordIfNeeded();
      final panes = tester.widgetList<TerminalPane>(find.byType(TerminalPane));
      if (panes.isNotEmpty && panes.first.controller.connected) break;
      if (DateTime.now().isAfter(deadline)) fail('连接测试服务器超时');
    }
    await tester.tap(
      find.byType(TerminalView).first,
      kind: PointerDeviceKind.mouse,
    );
    await tester.pump(const Duration(milliseconds: 300));
    final view = tester.state<TerminalViewState>(
      find.byType(TerminalView).first,
    );
    expect(view.hasInputConnection, isTrue, reason: '鼠标点击后建立文本输入连接');
    final inputLog = File(log!);
    Future<String> received() async => await inputLog.exists()
        ? (await inputLog.readAsString()).replaceAll(RegExp(r'\s'), '')
        : '';
    Future<void> checkInput(String expected, VoidCallback send) async {
      final before = await received();
      final hex = utf8
          .encode(expected)
          .map((v) => v.toRadixString(16).padLeft(2, '0'))
          .join();
      send();
      final until = DateTime.now().add(const Duration(seconds: 5));
      while (DateTime.now().isBefore(until)) {
        await tick(100);
        if ((await received()).length >= before.length + hex.length) break;
      }
      expect(
        (await received()).substring(before.length),
        hex,
        reason: '原生输入必须完整且不重复地到达 SSH 对端',
      );
    }

    await checkInput('abc123', () => native.text('abc123'));
    await checkInput('\r\x1b[D', () {
      native.key(0x0d);
      native.key(0x25, extended: true);
    });
    await checkInput('中文输入', () => native.commit('中文输入'));

    // 设置弹窗关闭后由工作区恢复焦点。
    await shortcut(LogicalKeyboardKey.comma);
    await tester.tap(find.text('完成'));
    await tick(350);
    await checkInput('afterdialog', () => native.text('afterdialog'));

    final originalPane = tester
        .widget<TerminalPane>(find.byType(TerminalPane).first)
        .controller;
    // 使用标签栏的新建入口打开第二条连接，再切回原标签。
    tester.widget<f.TabView>(find.byType(f.TabView)).onNewPressed!();
    await tester.pumpAndSettle();
    for (final entry in {
      '主机': Platform.environment['GUOSH_HOST']!,
      '端口': Platform.environment['GUOSH_PORT']!,
      '用户名': 'probe',
      '密码': 'probe',
    }.entries) {
      final label = find.byWidgetPredicate(
        (w) => w is f.InfoLabel && w.label.toPlainText() == entry.key,
      );
      final field = find.descendant(
        of: label,
        matching: find.byType(f.TextBox),
      );
      expect(field, findsOneWidget, reason: '快速连接表单：${entry.key}');
      tester.widget<f.TextBox>(field).controller!.text = entry.value;
    }
    await tester.tap(find.widgetWithText(f.FilledButton, '连接').last);
    final connectDeadline = DateTime.now().add(const Duration(seconds: 15));
    while (!tester
        .widget<TerminalPane>(find.byType(TerminalPane).first)
        .controller
        .connected) {
      if (DateTime.now().isAfter(connectDeadline)) fail('第二标签连接超时');
      await tick();
    }
    await tick(350);
    expect(tester.widget<f.TabView>(find.byType(f.TabView)).currentIndex, 1);
    await checkInput('tabtwo123', () => native.text('tabtwo123'));
    await shortcut(LogicalKeyboardKey.tab);
    expect(tester.widget<f.TabView>(find.byType(f.TabView)).currentIndex, 0);
    expect(
      tester.widget<TerminalPane>(find.byType(TerminalPane).first).controller,
      same(originalPane),
    );
    await checkInput('tabone456', () => native.text('tabone456'));

    // 鼠标清除选区以后，文本输入连接仍可用。
    final pane = tester
        .widget<TerminalPane>(find.byType(TerminalPane).first)
        .controller;
    pane.selection.setExternalSelection(
      const CellOffset(0, 0),
      const CellOffset(5, 0),
    );
    await tick();
    await tester.tap(
      find.byType(TerminalView).first,
      kind: PointerDeviceKind.mouse,
    );
    await tick(350);
    await checkInput('afterselection', () => native.text('afterselection'));
    expect(tester.takeException(), isNull);
  }, skip: !Platform.isWindows);
}
