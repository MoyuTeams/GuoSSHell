import 'dart:async';
import 'dart:ffi';
import 'dart:io';

import 'package:fluent_ui/fluent_ui.dart' as fluent;
import 'package:flutter/gestures.dart';
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

// flutter test integration_test/desktop_local_shell_dim_test.dart -d windows --no-pub
// 经真实 Fluent 壳打开本地终端（平台默认 shell：pwsh → powershell），输出 SGR 2 文本，
// 验证 dim 经 ConPTY、内核渲染帧与帧编码到达渲染层：DIMTEXT 单元格带 faint，NORMAL 不带。
// 可选 --dart-define=GUOSSH_DIM_SHOT_DIR=<目录>：断言通过后写入 ready，等外部截图写入 done
// （最多 30 秒）再继续，便于截取真实窗口。未定义时不等待。
// support 目录为本轮 createTemp 的独占目录；不连接任何外部目标。
const _shotDir = String.fromEnvironment('GUOSSH_DIM_SHOT_DIR');

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
    WindowsAppearanceQuery(requestId: 0).sendSignalToRust();
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

/// 缓冲中文本恰为 [text] 的行（命令回显行也含 DIMTEXT，按整行精确匹配排除）。
BufferLine? _lineExactly(TerminalPaneController pane, String text) {
  final terminal = pane.terminal;
  for (var i = terminal.height - 1; i >= 0; i--) {
    final line = terminal.lineAt(i);
    if (line.getText().trim() == text) return line;
  }
  return null;
}

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('Windows 本地终端：SGR 2 dim 文本在渲染层带 faint（隔离存储）', (tester) async {
    if (!Platform.isWindows) {
      throw TestFailure('此 smoke 只允许在 Windows 桌面设备上运行');
    }
    final owned = await Directory.systemTemp.createTemp('guossh-dim-smoke-');
    var rustStarted = false;
    try {
      await WindowsDesktopWindow.initialize();
      await initializeRust(assignRustSignal);
      rustStarted = true;
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
      final welcome = find.byKey(const fluent.ValueKey('welcome-local'));
      await _wait(tester, '欢迎页本地终端入口', () => welcome.evaluate().isNotEmpty);
      await tester.tap(welcome);
      await _wait(
        tester,
        '本地终端窗格',
        () => find.byType(TerminalPane).evaluate().isNotEmpty,
      );
      final pane = tester
          .widget<TerminalPane>(find.byType(TerminalPane))
          .controller;
      await _wait(
        tester,
        '本地 shell 已连接且提示符就绪',
        () =>
            pane.connected &&
            pane.lastFrameSeq > 0 &&
            _lines(pane).isNotEmpty &&
            _lines(pane).last.endsWith('>'),
      );

      final native = _NativeText();
      await native.initialize();
      await tester.tap(
        find.byType(TerminalView),
        kind: PointerDeviceKind.mouse,
      );
      await _wait(
        tester,
        '终端文本输入连接',
        () => tester
            .state<TerminalViewState>(find.byType(TerminalView))
            .hasInputConnection,
      );
      // 直接写控制台：Write-Host 在设置了 NO_COLOR 的环境里会剥掉转义序列。
      const command =
          r'[Console]::WriteLine("$([char]27)[2mDIMTEXT$([char]27)[0m NORMAL")';
      native.commit(command);
      // 先等 shell 回显出整行命令再按回车：WM_CHAR 经消息循环异步到达，回车不能抢先。
      await _wait(
        tester,
        'shell 回显命令',
        () => _lines(pane).isNotEmpty && _lines(pane).last.contains(command),
      );
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);

      BufferLine? line;
      await _wait(tester, '终端出现 dim 输出行', () {
        line = _lineExactly(pane, 'DIMTEXT NORMAL');
        return line != null;
      });
      final output = line!;
      final start = output.getText().indexOf('DIMTEXT');
      expect(output.getText(start, start + 14), 'DIMTEXT NORMAL');
      for (var col = start; col < start + 7; col++) {
        expect(
          output.getAttributes(col) & CellFlags.faint,
          CellFlags.faint,
          reason: 'DIMTEXT 第 ${col - start} 格应带 faint',
        );
      }
      for (var col = start + 8; col < start + 14; col++) {
        expect(
          output.getAttributes(col) & CellFlags.faint,
          0,
          reason: 'NORMAL 第 ${col - start - 8} 格不应带 faint',
        );
      }

      if (_shotDir.isNotEmpty) {
        final done = File('$_shotDir${Platform.pathSeparator}done');
        await File('$_shotDir${Platform.pathSeparator}ready').writeAsString('');
        await _wait(
          tester,
          '外部截图完成',
          () => done.existsSync(),
          timeout: const Duration(seconds: 30),
        );
      }
      expect(find.byType(WindowsFrame), findsOneWidget);
      expect(tester.takeException(), isNull);
    } finally {
      try {
        await tester.pumpWidget(const fluent.SizedBox.shrink());
      } finally {
        // 同步退出 Rust 并 join SQLite worker 后才删除 support。
        if (rustStarted) finalizeRust();
        // 只删除本轮 createTemp 返回的精确目录。
        await owned.delete(recursive: true);
        expect(await owned.exists(), isFalse);
      }
    }
  });
}
