import 'dart:async';
import 'dart:convert';
import 'dart:ffi';
import 'dart:io';

import 'package:fluent_ui/fluent_ui.dart' as fluent;
import 'package:flutter/foundation.dart' show debugPrint;
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/app.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/desktop/catalog/desktop_connection_editor.dart';
import 'package:guosh_shell/src/desktop/desktop_widgets.dart';
import 'package:guosh_shell/src/desktop/windows_chrome.dart';
import 'package:guosh_shell/src/desktop/windows_shell.dart';
import 'package:guosh_shell/src/platform_target.dart';
import 'package:guosh_shell/src/terminal/session_target.dart';
import 'package:guosh_shell/src/terminal/terminal_pane.dart';
import 'package:integration_test/integration_test.dart';
import 'package:rinf/rinf.dart';
import 'package:terminal_view/terminal_view.dart';
import 'package:window_manager/window_manager.dart';

// 由 scripts/ci/windows_auth_check.ps1 -Target system 启动。
// 经真实 Fluent 编辑器保存 System OpenSSH 连接并打开：系统 ssh 在本地 PTY 中运行，
// 首见主机密钥与密码都由 ssh 在终端里提问，应用不弹任何对话框。
// 隔离：runner 用 GUOSH_SYSTEM_SSH_CONFIG（仅 debug 构建读取）让 ssh 以 -F 使用本轮配置；
// 连接主机名是只在该配置里定义的别名，配置未生效时 ssh 无法解析主机、连不到任何地方。
const _alias = 'guosh-system-smoke';

class _OwnedStartup implements StartupPort {
  _OwnedStartup(this.directory);

  final Directory directory;

  @override
  StreamSubscription<dynamic> listenReady(
    void Function(bool, String, SessionTarget?) onReady,
  ) => AppReady.rustSignalStream.listen(
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

/// 向本测试窗口的 Flutter 子窗口投递原生消息：WM_CHAR 为提交的文字，回车为
/// WM_KEYDOWN/WM_KEYUP（由消息循环生成 WM_CHAR）。同一队列，按投递顺序处理。
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

  void commit(String text) {
    for (final unit in text.codeUnits) {
      expect(_post(window, 0x102, unit, 1), isNonZero);
    }
  }

  void enter() {
    const virtualKey = 0x0D;
    final flags = 1 | (_scan(virtualKey, 0) << 16);
    expect(_post(window, 0x100, virtualKey, flags), isNonZero);
    expect(_post(window, 0x101, virtualKey, flags | (3 << 30)), isNonZero);
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

Future<void> _appears(WidgetTester tester, Finder finder) =>
    _wait(tester, '界面出现', () => finder.evaluate().isNotEmpty);

Finder _iconButton(String label) => find.byWidgetPredicate(
  (widget) => widget is DesktopIconButton && widget.label == label,
);

Future<void> _field(WidgetTester tester, String label, String value) async {
  final input = find.byKey(fluent.ValueKey('catalog-field-$label'));
  await tester.ensureVisible(input);
  await tester.enterText(input, value);
}

Future<CatalogState> _catalog(WidgetTester tester, String query) async {
  CatalogState? result;
  final listener = CatalogState.rustSignalStream.listen((pack) {
    if (pack.message.query == query) result = pack.message;
  });
  try {
    CatalogQuery(query: query).sendSignalToRust();
    await _wait(tester, '本轮 Rust 目录查询', () => result != null);
    return result!;
  } finally {
    await listener.cancel();
  }
}

String _requiredEnv(String name) {
  final value = Platform.environment[name];
  if (value == null || value.isEmpty) {
    throw TestFailure('请通过 Windows 认证 runner（-Target system）提供 $name');
  }
  return value;
}

/// 终端全文，去掉折行，便于匹配 ssh 的长提示。
String _flat(TerminalPaneController pane) =>
    pane.terminal.getText().replaceAll('\n', '');

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('Windows System OpenSSH：编辑器保存、终端内首见主机密钥与密码、回显', (tester) async {
    if (!Platform.isWindows) {
      throw TestFailure('此 smoke 只允许在 Windows 桌面设备上运行');
    }
    final run = Directory(_requiredEnv('GUOSH_AUTH_RUN'));
    final runId = _requiredEnv('GUOSH_AUTH_RUN_ID');
    final owned = Directory(_requiredEnv('GUOSH_DESKTOP_TEST_DATA'));
    final inputLog = File(_requiredEnv('GUOSH_DEMO_INPUT_LOG'));
    final port = int.parse(_requiredEnv('GUOSH_AUTH_PORT'));
    final fingerprint = _requiredEnv('GUOSH_AUTH_FINGERPRINT');
    final config = File(_requiredEnv('GUOSH_SYSTEM_SSH_CONFIG'));
    final separator = Platform.pathSeparator;
    final runPath = await run.resolveSymbolicLinks();
    final ownedPath = await owned.resolveSymbolicLinks();
    expect(await File('$runPath$separator.owner').readAsString(), runId);
    expect(ownedPath, '$runPath${separator}app');
    expect(
      await config.resolveSymbolicLinks(),
      '$runPath${separator}ssh${separator}config',
    );
    final sshKnownHosts = File(
      '$runPath${separator}ssh${separator}known_hosts',
    );
    expect(await sshKnownHosts.exists(), isFalse);
    expect(await owned.list().isEmpty, isTrue, reason: 'support 必须是本轮全新目录');
    expect(await inputLog.exists(), isFalse);
    // 应用自己的 known_hosts：System 连接不经应用核验，始终不应出现。
    final appKnownHosts = File('${owned.path}${separator}known_hosts');

    var rustStarted = false;
    final listeners = <StreamSubscription>[];
    final prompts = <InteractionPrompt>[];
    final statuses = <SessionStatus>[];
    try {
      await WindowsDesktopWindow.initialize();
      await initializeRust(assignRustSignal);
      rustStarted = true;
      listeners.addAll([
        DesktopPrompt.rustSignalStream.listen(
          (pack) => prompts.add(pack.message.prompt),
        ),
        DesktopSessionStatus.rustSignalStream.listen(
          (pack) => statuses.add(pack.message.status),
        ),
      ]);
      await tester.pumpWidget(
        GuoSSHellApp(
          targetOverride: AppTarget.desktop,
          startupOverride: _OwnedStartup(owned),
        ),
      );
      await _appears(tester, find.byType(WindowsShell));
      final shell = tester.state(find.byType(WindowsShell));
      await _appears(tester, find.textContaining('还没有保存的连接'));

      // 真实 Fluent 编辑器：选 System OpenSSH，密码与认证方式随之隐藏。
      const name = 'system-smoke-fixture';
      await tester.tap(_iconButton('添加连接'));
      await _appears(tester, find.byType(DesktopConnectionEditor));
      await _field(tester, '名称', name);
      await _field(tester, '主机', _alias);
      await _field(tester, '端口', '$port');
      await _field(tester, '用户名', 'probe');
      final auth = find.byKey(const fluent.Key('catalog-auth'));
      final transport = find.byKey(const fluent.Key('catalog-transport'));
      // 系统 ssh 可用性异步查询；查到前 System 选项禁用，选择不生效则重试。
      await _wait(tester, '选择 System OpenSSH', () async {
        if (auth.evaluate().isEmpty) return true;
        await tester.ensureVisible(transport);
        await tester.pumpAndSettle();
        await tester.tap(transport);
        await tester.pumpAndSettle();
        await tester.tap(find.text('System OpenSSH').last, warnIfMissed: false);
        await tester.pumpAndSettle();
        return auth.evaluate().isEmpty;
      });
      expect(
        find.byKey(const fluent.ValueKey('catalog-field-密码')),
        findsNothing,
      );
      expect(find.textContaining('由系统 ssh 处理认证'), findsOneWidget);
      await tester.ensureVisible(find.text('保存'));
      await tester.tap(find.text('保存'));
      await _appears(tester, find.text(name));
      final entry = (await _catalog(tester, name)).connections.single;
      expect(entry.transport, DesktopTransport.systemOpenSsh);
      expect(entry.host, _alias);
      expect(entry.port, port);
      expect(entry.username, 'probe');
      expect(entry.passwordSaved, isFalse);
      expect(find.textContaining('系统 ssh'), findsWidgets);
      debugPrint('[system-smoke] 编辑器保存 System 连接成功');

      await tester.tap(_iconButton('连接操作'));
      final connect = find.descendant(
        of: find.byType(fluent.MenuFlyout),
        matching: find.text('连接'),
      );
      await _appears(tester, connect);
      await tester.tap(connect);
      await _appears(tester, find.byType(TerminalPane));
      final pane = tester
          .widget<TerminalPane>(find.byType(TerminalPane))
          .controller;
      expect(pane.target.isSystemSsh, isTrue);
      expect(pane.target.connectionId, entry.id);
      final sessionId = pane.sessionId;

      // ssh 在终端里问首见主机密钥；只接受本轮 fixture 的指纹。
      await _wait(
        tester,
        'ssh 首见主机密钥提问',
        () => pane.connected && _flat(pane).contains('(yes/no'),
      );
      expect(_flat(pane), contains(fingerprint));
      expect(find.byType(fluent.ContentDialog), findsNothing);
      expect(prompts, isEmpty, reason: '应用不代为提问');
      expect(await sshKnownHosts.exists(), isFalse);

      final native = _NativeInput();
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
      final beforeYes = _flat(pane).length;
      native.commit('yes');
      await _wait(
        tester,
        'ssh 回显 yes',
        () => _flat(pane).substring(beforeYes).contains('yes'),
      );
      native.enter();
      await _wait(
        tester,
        'ssh 密码提问',
        () =>
            _flat(pane).contains("probe@$_alias's password:") ||
            _flat(pane).contains("probe@127.0.0.1's password:"),
      );
      expect(await sshKnownHosts.readAsString(), contains('ssh-ed25519'));
      // 密码不回显：文字与回车同队列按序投递。
      native.commit('probe');
      native.enter();
      await _wait(
        tester,
        'demo banner',
        () => _flat(pane).contains('GuoSSHell M1'),
      );
      expect(_flat(pane), isNot(contains("password: probe")));

      const echo = 'systemsmoke42';
      final expectedHex = utf8
          .encode(echo)
          .map((byte) => byte.toRadixString(16).padLeft(2, '0'))
          .join();
      native.commit(echo);
      await _wait(
        tester,
        'fixture 收到输入且终端出现回显',
        () async =>
            await inputLog.exists() &&
            (await inputLog.readAsString()).replaceAll(RegExp(r'\s'), '') ==
                expectedHex &&
            _flat(pane).contains(echo),
      );
      expect(prompts, isEmpty);
      expect(find.byType(fluent.ContentDialog), findsNothing);
      expect(await appKnownHosts.exists(), isFalse);
      expect(
        statuses.where(
          (status) =>
              status.sessionId == sessionId &&
              status.state == SessionState.connected,
        ),
        hasLength(1),
      );
      debugPrint('[system-smoke] 终端内首见主机密钥/密码、banner 与回显通过；无应用对话框');

      // demo fixture 不支持 exit：关闭窗格断开（ssh 随 PTY 结束）。
      await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
      await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
      await tester.sendKeyEvent(LogicalKeyboardKey.keyW);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
      await _appears(tester, find.text('关闭窗格？'));
      await tester.tap(find.widgetWithText(fluent.FilledButton, '断开'));
      await _wait(
        tester,
        '关闭窗格后回到管理目录',
        () =>
            find.byType(TerminalPane).evaluate().isEmpty &&
            statuses.any(
              (status) =>
                  status.sessionId == sessionId &&
                  status.state == SessionState.closed,
            ) &&
            find.text('0 个标签  ·  0 个窗格').evaluate().isNotEmpty,
      );
      expect(tester.state(find.byType(WindowsShell)), same(shell));
      expect(find.byType(WindowsFrame), findsOneWidget);
      expect(prompts, isEmpty);
      expect(tester.takeException(), isNull);
      debugPrint('[system-smoke] 关闭窗格断开、返回管理目录通过');
    } finally {
      try {
        await tester.pumpWidget(const fluent.SizedBox.shrink());
      } finally {
        for (final listener in listeners) {
          await listener.cancel();
        }
        // 同步退出 Rust 并 join SQLite worker 后才删除 support。
        if (rustStarted) finalizeRust();
        expect(await owned.resolveSymbolicLinks(), ownedPath);
        expect(await run.resolveSymbolicLinks(), runPath);
        expect(await File('$runPath$separator.owner').readAsString(), runId);
        await owned.delete(recursive: true);
        expect(await owned.exists(), isFalse);
      }
    }
  });
}
