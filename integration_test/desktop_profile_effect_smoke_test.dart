import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:fluent_ui/fluent_ui.dart' as fluent;
import 'package:flutter/foundation.dart' show debugPrint;
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/app.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/desktop/catalog/desktop_catalog_gateway.dart';
import 'package:guosh_shell/src/desktop/desktop_widgets.dart';
import 'package:guosh_shell/src/desktop/windows_chrome.dart';
import 'package:guosh_shell/src/desktop/windows_shell.dart';
import 'package:guosh_shell/src/platform_target.dart';
import 'package:guosh_shell/src/terminal/session_target.dart';
import 'package:guosh_shell/src/terminal/terminal_pane.dart';
import 'package:integration_test/integration_test.dart';
import 'package:rinf/rinf.dart';
import 'package:terminal_view/terminal_view.dart';

// 由 scripts/ci/windows_auth_check.ps1 -Target profile 启动（demo_server 带 GUOSH_DEMO_ENQ=1）。
// 经设置协议建一个非默认终端 profile，用它与默认 profile 各连一次环回 fixture，
// 以对端收到的 PTY TERM、answerback 与按键字节证明 profile 在 Rust 侧真实生效。
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
    WindowsAppearanceQuery(requestId: 0).sendSignalToRust();
    SettingsQuery().sendSignalToRust();
  }

  @override
  void foreground(bool value) =>
      AppLifecycle(foreground: value).sendSignalToRust();
}

Future<void> _wait(
  WidgetTester tester,
  String description,
  FutureOr<bool> Function() ready, {
  Duration timeout = const Duration(seconds: 20),
}) async {
  final elapsed = Stopwatch()..start();
  while (!await ready()) {
    if (elapsed.elapsed >= timeout) {
      throw TestFailure('等待超时：$description');
    }
    // 真实时钟下泵帧，让 native 信号与布局继续处理；demo 日志流不停，不用 pumpAndSettle。
    await tester.pump(const Duration(milliseconds: 50));
    expect(tester.takeException(), isNull, reason: description);
  }
}

Future<void> _appears(WidgetTester tester, Finder finder) =>
    _wait(tester, '界面出现', () => finder.evaluate().isNotEmpty);

/// 边泵帧边等待异步结果；Rust 回包要靠事件循环送达。
Future<T> _settle<T>(
  WidgetTester tester,
  String description,
  Future<T> future,
) async {
  late T value;
  Object? error;
  var done = false;
  future.then(
    (result) {
      value = result;
      done = true;
    },
    onError: (Object failure) {
      error = failure;
      done = true;
    },
  );
  await _wait(tester, description, () => done);
  if (error != null) throw TestFailure('$description：$error');
  return value;
}

Finder _iconButton(String label) => find.byWidgetPredicate(
  (widget) => widget is DesktopIconButton && widget.label == label,
);

String _requiredEnv(String name) {
  final value = Platform.environment[name];
  if (value == null || value.isEmpty) {
    throw TestFailure('请通过 Windows 认证 runner 提供 $name');
  }
  return value;
}

String _hex(List<int> bytes) =>
    bytes.map((byte) => byte.toRadixString(16).padLeft(2, '0')).join();

Future<DesktopSettingsResult> _settings(
  WidgetTester tester,
  String requestId,
  void Function() send,
) async {
  DesktopSettingsResult? reply;
  // 先订阅再发送，只收本轮 requestId 的回包。
  final listener = DesktopSettingsResult.rustSignalStream.listen((pack) {
    if (pack.message.requestId == requestId) reply ??= pack.message;
  });
  try {
    send();
    await _wait(tester, '设置协议回包 $requestId', () => reply != null);
    return reply!;
  } finally {
    await listener.cancel();
  }
}

DesktopConfiguration _snapshot(DesktopSettingsResult result, String status) {
  expect(
    result.status,
    status,
    reason: '桌面配置协议返回：${result.detail} ${result.fieldErrors}',
  );
  expect(result.snapshot, isNotNull);
  return result.snapshot!;
}

DesktopTerminalProfile _profile(DesktopConfiguration snapshot, String id) =>
    snapshot.profiles.singleWhere((profile) => profile.id == id);

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('Windows 非默认终端 profile 经 V2 会话对真实 SSH 对端生效', (tester) async {
    if (!Platform.isWindows) {
      throw TestFailure('此 smoke 只允许在 Windows 桌面设备上运行');
    }
    final run = Directory(_requiredEnv('GUOSH_AUTH_RUN'));
    final runId = _requiredEnv('GUOSH_AUTH_RUN_ID');
    final owned = Directory(_requiredEnv('GUOSH_DESKTOP_TEST_DATA'));
    final inputLog = File(_requiredEnv('GUOSH_DEMO_INPUT_LOG'));
    final port = int.parse(_requiredEnv('GUOSH_AUTH_PORT'));
    expect(_requiredEnv('GUOSH_DEMO_ENQ'), '1', reason: 'fixture 必须发 ENQ');
    final separator = Platform.pathSeparator;
    final runPath = await run.resolveSymbolicLinks();
    final ownedPath = await owned.resolveSymbolicLinks();
    expect(await File('$runPath$separator.owner').readAsString(), runId);
    expect(ownedPath, '$runPath${separator}app');
    expect(inputLog.absolute.path, '$runPath${separator}received.txt');
    expect(await owned.list().isEmpty, isTrue, reason: 'support 必须是本轮全新目录');
    expect(await inputLog.exists(), isFalse);

    Future<String> received() async => await inputLog.exists()
        ? (await inputLog.readAsString()).replaceAll(RegExp(r'\s'), '')
        : '';

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
      expect(find.byType(WindowsFrame), findsOneWidget);
      await _appears(tester, find.textContaining('还没有保存的连接'));

      // 1. 设置协议：查询 → 新建 → 写入非默认字段。
      final query = await _settings(
        tester,
        'profile-e2e-query',
        () => DesktopSettingsRequest(
          protocolVersion: 1,
          requestId: 'profile-e2e-query',
          baseRevision: Uint64(BigInt.zero),
          operation: 'query',
          targetId: '',
          name: '',
        ).sendSignalToRust(),
      );
      final initial = _snapshot(query, 'ok');
      // 动作名以协议能力表为准。
      expect(query.capabilities.keyActions, contains('send:\u007f'));
      final defaults = _profile(initial, initial.app.defaultTerminalProfile);
      expect(defaults.terminalType, 'xterm-256color');
      final created = await _settings(
        tester,
        'profile-e2e-create',
        () => DesktopSettingsRequest(
          protocolVersion: 1,
          requestId: 'profile-e2e-create',
          baseRevision: initial.revision,
          operation: 'create_profile',
          targetId: '',
          name: 'profile-e2e-vt220',
        ).sendSignalToRust(),
      );
      final afterCreate = _snapshot(created, 'applied');
      final profileId = created.targetId;
      expect(profileId, isNot(initial.app.defaultTerminalProfile));
      final draft = _profile(afterCreate, profileId).copyWith(
        terminalType: 'vt220',
        answerback: 'GUOSH-AB',
        leftAltAsMeta: true,
        rightAltAsMeta: false,
        keyBindings: const [
          DesktopKeyBinding(
            key: 'F5',
            shift: false,
            control: false,
            alt: false,
            superKey: false,
            action: 'send:\u007f',
          ),
        ],
      );
      final saved = _snapshot(
        await _settings(
          tester,
          'profile-e2e-save',
          () => DesktopSettingsRequest(
            protocolVersion: 1,
            requestId: 'profile-e2e-save',
            baseRevision: afterCreate.revision,
            operation: 'save_profile',
            targetId: profileId,
            name: '',
            profile: draft,
          ).sendSignalToRust(),
        ),
        'applied',
      );
      expect(_profile(saved, profileId), draft);
      expect(saved.app.defaultTerminalProfile, defaults.id);
      debugPrint('[profile-e2e] 设置协议已建并保存非默认 profile');

      // 2. 两条目录连接：一条引用新 profile，一条跟随默认；均不保存密码。
      const gateway = RustDesktopCatalogGateway();
      Future<String> save(String name, String profile) async {
        final result = await _settle(
          tester,
          '保存连接 $name',
          gateway.save(
            DesktopConnectionDraft(
              id: '',
              name: name,
              host: '127.0.0.1',
              port: port,
              username: 'probe',
              auth: AuthMethod.password,
              passwordAction: PasswordAction.clear,
              password: '',
              keyId: '',
              command: '',
              profileId: profile,
            ),
          ),
        );
        expect(result.error, CatalogError.none);
        expect(
          (await _settle(
            tester,
            '读取连接 profile',
            gateway.connection(result.connectionId),
          )).profileId,
          profile,
        );
        return result.connectionId;
      }

      const profileName = 'profile-e2e-custom';
      const defaultName = 'profile-e2e-default';
      await save(profileName, profileId);
      await save(defaultName, '');
      gateway.query('');
      await _appears(tester, find.text(profileName));
      await _appears(tester, find.text(defaultName));

      Future<TerminalPaneController> connect(String name) async {
        final row = find.ancestor(
          of: find.text(name),
          matching: find.byType(fluent.ListTile),
        );
        await tester.tap(
          find.descendant(of: row, matching: _iconButton('连接操作')),
        );
        final item = find.descendant(
          of: find.byType(fluent.MenuFlyout),
          matching: find.text('连接'),
        );
        await _appears(tester, item);
        await tester.tap(item);
        await _appears(tester, find.byType(TerminalPane));
        final pane = tester
            .widget<TerminalPane>(find.byType(TerminalPane))
            .controller;
        await _appears(tester, find.text('输入密码'));
        expect(prompts.last.kind, PromptKind.password);
        expect(prompts.last.sessionId, pane.sessionId);
        await tester.enterText(
          find.descendant(
            of: find.byType(fluent.ContentDialog),
            matching: find.byType(fluent.TextBox),
          ),
          'probe',
        );
        await tester.tap(find.widgetWithText(fluent.FilledButton, '连接'));
        return pane;
      }

      Future<void> focus() async {
        await tester.tap(
          find.byType(TerminalView),
          kind: PointerDeviceKind.mouse,
        );
        await _wait(
          tester,
          '终端获得输入焦点',
          () => tester
              .state<TerminalViewState>(find.byType(TerminalView))
              .hasInputConnection,
        );
      }

      Future<void> expectReceived(String description, String expected) async {
        try {
          await _wait(
            tester,
            description,
            () async => await received() == expected,
          );
        } on TestFailure {
          debugPrint(
            '[profile-e2e] $description：期望 $expected，实际 ${await received()}',
          );
          rethrow;
        }
      }

      Future<void> close(TerminalPaneController pane) async {
        final sessionId = pane.sessionId;
        await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
        await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
        await tester.sendKeyEvent(LogicalKeyboardKey.keyW);
        await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
        await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
        await _appears(tester, find.text('关闭窗格？'));
        await tester.tap(find.widgetWithText(fluent.FilledButton, '断开'));
        await _wait(
          tester,
          '会话关闭并返回管理目录',
          () =>
              find.byType(TerminalPane).evaluate().isEmpty &&
              statuses.any(
                (status) =>
                    status.sessionId == sessionId &&
                    status.state == SessionState.closed,
              ),
        );
      }

      // 3. 非默认 profile 会话：首见 host-key 后验证 TERM、answerback 与按键。
      final custom = await connect(profileName);
      await _appears(tester, find.text('确认主机密钥'));
      expect(prompts.last.kind, PromptKind.hostKey);
      expect(prompts.last.changed, isFalse);
      await tester.tap(find.widgetWithText(fluent.FilledButton, '信任并连接'));
      await _wait(
        tester,
        'vt220 会话连接并显示 PTY-ACK',
        () =>
            custom.connected &&
            custom.terminal.getText().contains('PTY-ACK term=vt220 ') &&
            custom.terminal.getText().contains('GuoSSHell M1'),
      );
      var expected = _hex(utf8.encode('GUOSH-AB'));
      await expectReceived('ENQ 触发 profile answerback', expected);
      await focus();

      await tester.sendKeyEvent(LogicalKeyboardKey.f5);
      expected += '7f';
      await expectReceived('F5 走 profile 绑定 send:DEL', expected);

      await tester.sendKeyDownEvent(LogicalKeyboardKey.altLeft);
      await tester.sendKeyEvent(LogicalKeyboardKey.keyX);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.altLeft);
      expected += '1b78';
      await expectReceived('左 Alt 作 Meta：ESC x', expected);

      // 右 Alt 非 Meta 时 Rust 清掉 Alt 修饰，编码为裸 x（见 session_input_tests 的左右 Alt 用例）。
      await tester.sendKeyDownEvent(LogicalKeyboardKey.altRight);
      await tester.sendKeyEvent(LogicalKeyboardKey.keyX);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.altRight);
      expected += '78';
      await expectReceived('右 Alt 非 Meta：无 ESC 前缀', expected);
      expect(custom.connected, isTrue);
      debugPrint('[profile-e2e] 非默认 profile：TERM/answerback/F5/左右 Alt 均按配置生效');
      await close(custom);

      // 4. 默认 profile 对照：host-key 已信任，只问密码。
      final promptsBefore = prompts.length;
      final control = await connect(defaultName);
      await _wait(
        tester,
        '默认会话连接并显示 PTY-ACK 与首条日志',
        () =>
            control.connected &&
            control.terminal.getText().contains(
              'PTY-ACK term=xterm-256color ',
            ) &&
            // 日志行在 ENQ 之后发出：出现即说明 ENQ 已被引擎处理。
            control.terminal.getText().contains('日志行 0：'),
      );
      expect(
        prompts,
        hasLength(promptsBefore + 1),
        reason: '已信任主机不应再问 host-key',
      );
      expected += _hex(utf8.encode(defaults.answerback));
      await expectReceived('默认 profile 的 answerback', expected);
      await focus();
      await tester.sendKeyEvent(LogicalKeyboardKey.f5);
      expected += '1b5b31357e';
      await expectReceived('默认 profile 的 F5 为 CSI 15~', expected);
      debugPrint(
        '[profile-e2e] 默认 profile 对照：TERM=xterm-256color，'
        'answerback=${defaults.answerback}，F5=CSI 15~',
      );
      await close(control);
      expect(tester.takeException(), isNull);
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
        expect(await File('$runPath$separator.owner').readAsString(), runId);
        await owned.delete(recursive: true);
        expect(await owned.exists(), isFalse);
        debugPrint(
          '[profile-e2e] widget/listener/Rust 已结束；SQLite support 精确删除成功',
        );
      }
    }
  });
}
