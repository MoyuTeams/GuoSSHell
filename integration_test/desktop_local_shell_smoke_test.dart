import 'dart:async';
import 'dart:ffi';
import 'dart:io';

import 'package:fluent_ui/fluent_ui.dart' as fluent;
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart' show TextButton;
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/app.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/desktop/windows_chrome.dart';
import 'package:guosh_shell/src/desktop/windows_shell.dart';
import 'package:guosh_shell/src/platform_target.dart';
import 'package:guosh_shell/src/terminal/session_target.dart';
import 'package:guosh_shell/src/terminal/terminal_pane.dart';
import 'package:integration_test/integration_test.dart';
import 'package:rinf/rinf.dart';
import 'package:terminal_view/terminal_view.dart';
import 'package:window_manager/window_manager.dart';

// flutter test integration_test/desktop_local_shell_smoke_test.dart -d windows --no-pub
// 经真实 Fluent 壳打开本地终端（平台默认 shell），验证回显、非 0 退出后重新启动与 exit 0 关窗格。
// support 目录为本轮 createTemp 的独占目录；不连接任何外部目标。
class _OwnedStartup implements StartupPort {
  _OwnedStartup(this.directory);

  final Directory directory;

  @override
  StreamSubscription<dynamic> listenReady(
    void Function(bool, String, SessionTarget?) onReady,
  ) => AppReady.rustSignalStream.listen(
    // 忽略调试环境中的自动连接参数。
    (pack) => onReady(pack.message.ok, pack.message.detail, null),
  );

  @override
  StreamSubscription<dynamic> listenSettings(void Function() onSettings) =>
      SettingsState.rustSignalStream.listen((_) => onSettings());

  @override
  Future<String> supportDirectory() async => directory.path;

  @override
  void start(String path) => AppStart(supportDir: path).sendSignalToRust();

  @override
  void querySettings() {
    WindowsAppearanceQuery().sendSignalToRust();
    SettingsQuery().sendSignalToRust();
  }

  @override
  void foreground(bool value) =>
      AppLifecycle(foreground: value).sendSignalToRust();
}

// 原生 WM_CHAR 提交到本测试窗口的 Flutter 子窗口，与真实键入同走 TextInput。
class _NativeText {
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
  late final Pointer<Void> window;

  Future<void> initialize() async {
    window = _child(Pointer.fromAddress(await windowManager.getId()), 5);
    expect(window.address, isNonZero);
  }

  void commit(String text) {
    for (final unit in text.codeUnits) {
      expect(_post(window, 0x102, unit, 1), isNonZero);
    }
  }
}

Future<void> _wait(
  WidgetTester tester,
  String description,
  FutureOr<bool> Function() ready, {
  Duration timeout = const Duration(seconds: 30),
}) async {
  final elapsed = Stopwatch()..start();
  while (!await ready()) {
    if (elapsed.elapsed >= timeout) {
      throw TestFailure('等待超时：$description');
    }
    await tester.pump(const Duration(milliseconds: 50));
    expect(tester.takeException(), isNull, reason: description);
  }
}

List<String> _lines(TerminalPaneController pane) => [
  for (final line in pane.terminal.getText().split('\n'))
    if (line.trim().isNotEmpty) line.trimRight(),
];

/// shell 提示符已就绪：最后一个非空行以 `>` 结尾（pwsh / powershell / cmd 均如此）。
bool _prompt(TerminalPaneController pane) {
  final lines = _lines(pane);
  return lines.isNotEmpty && lines.last.endsWith('>');
}

Future<void> _focusTerminal(WidgetTester tester) async {
  await tester.tap(find.byType(TerminalView), kind: PointerDeviceKind.mouse);
  await _wait(
    tester,
    '终端文本输入连接',
    () => tester
        .state<TerminalViewState>(find.byType(TerminalView))
        .hasInputConnection,
  );
}

/// 先等 shell 回显出整行命令再按回车：WM_CHAR 经消息循环异步到达，回车不能抢先。
Future<void> _run(
  WidgetTester tester,
  _NativeText native,
  TerminalPaneController pane,
  String command,
) async {
  native.commit(command);
  await _wait(
    tester,
    'shell 回显命令「$command」',
    () => _lines(pane).isNotEmpty && _lines(pane).last.contains(command),
  );
  await tester.sendKeyEvent(LogicalKeyboardKey.enter);
}

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('Windows 本地终端：回显、非 0 退出重新启动、exit 0 关窗格（隔离存储）', (tester) async {
    if (!Platform.isWindows) {
      throw TestFailure('此 smoke 只允许在 Windows 桌面设备上运行');
    }
    final owned = await Directory.systemTemp.createTemp('guossh-local-smoke-');
    var rustStarted = false;
    final statuses = <SessionStatus>[];
    StreamSubscription? listener;
    try {
      await WindowsDesktopWindow.initialize();
      await initializeRust(assignRustSignal);
      rustStarted = true;
      listener = DesktopSessionStatus.rustSignalStream.listen(
        (pack) => statuses.add(pack.message.status),
      );
      await tester.pumpWidget(
        GuoSSHellApp(
          targetOverride: AppTarget.desktop,
          startupOverride: _OwnedStartup(owned),
        ),
      );
      await _wait(
        tester,
        '工作区出现',
        () => find.byType(WindowsShell).evaluate().isNotEmpty,
      );
      final shell = tester.state(find.byType(WindowsShell));
      final welcome = find.byKey(const fluent.ValueKey('welcome-local'));
      await _wait(tester, '欢迎页本地终端入口', () => welcome.evaluate().isNotEmpty);
      expect(
        find.byKey(const fluent.ValueKey('workspace-local')),
        findsOneWidget,
      );

      await tester.tap(welcome);
      await _wait(
        tester,
        '本地终端窗格',
        () => find.byType(TerminalPane).evaluate().isNotEmpty,
      );
      final pane = tester
          .widget<TerminalPane>(find.byType(TerminalPane))
          .controller;
      expect(pane.target.isLocal, isTrue);
      expect(find.text('1 个标签  ·  1 个窗格'), findsOneWidget);
      await _wait(
        tester,
        '本地 shell 已连接且提示符就绪',
        () => pane.connected && pane.lastFrameSeq > 0 && _prompt(pane),
      );
      final sessionId = pane.sessionId;
      expect(
        statuses.any(
          (status) =>
              status.sessionId == sessionId &&
              status.state == SessionState.connected &&
              status.detail == '本地 shell',
        ),
        isTrue,
      );
      // 本地 shell 不认证：不应出现任何认证对话框。
      expect(find.byType(fluent.ContentDialog), findsNothing);

      final native = _NativeText();
      await native.initialize();
      await _focusTerminal(tester);
      await _run(tester, native, pane, 'echo GUOSH_LOCAL_OK');
      await _wait(
        tester,
        '终端出现 echo 输出行',
        () => _lines(pane).any((line) => line.trim() == 'GUOSH_LOCAL_OK'),
      );

      await _run(tester, native, pane, 'exit 3');
      await _wait(
        tester,
        '非 0 退出保留窗格并显示退出码',
        () =>
            pane.state == SessionState.closed &&
            find.text('进程已退出（代码 3）').evaluate().isNotEmpty,
      );
      expect(
        statuses.last.detail,
        'exit code=Some(3) success=false',
        reason: 'Rust 退出状态固定格式',
      );
      expect(find.byType(TerminalPane), findsOneWidget);

      await tester.tap(find.widgetWithText(TextButton, '重新启动'));
      await _wait(
        tester,
        '重新启动后 shell 已连接且提示符就绪',
        () =>
            pane.connected &&
            pane.lastFrameSeq > 0 &&
            find.textContaining('进程已退出').evaluate().isEmpty &&
            _prompt(pane),
      );
      expect(pane.sessionId, sessionId, reason: '同一窗格同一会话重新启动');

      await _focusTerminal(tester);
      await _run(tester, native, pane, 'exit 0');
      await _wait(
        tester,
        'exit 0 后窗格关闭',
        () =>
            find.byType(TerminalPane).evaluate().isEmpty &&
            find.text('0 个标签  ·  0 个窗格').evaluate().isNotEmpty,
      );
      expect(
        statuses
            .where((status) => status.sessionId == sessionId)
            .any((status) => status.detail == 'exit code=Some(0) success=true'),
        isTrue,
      );
      expect(tester.state(find.byType(WindowsShell)), same(shell));
      expect(find.byType(WindowsFrame), findsOneWidget);
      expect(tester.takeException(), isNull);
    } finally {
      try {
        await tester.pumpWidget(const fluent.SizedBox.shrink());
      } finally {
        await listener?.cancel();
        // 同步退出 Rust 并 join SQLite worker 后才删除 support。
        if (rustStarted) finalizeRust();
        // 只删除本轮 createTemp 返回的精确目录。
        await owned.delete(recursive: true);
        expect(await owned.exists(), isFalse);
      }
    }
  });
}
