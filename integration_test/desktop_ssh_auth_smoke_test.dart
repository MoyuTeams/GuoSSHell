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
import 'package:guosh_shell/src/desktop/catalog/desktop_catalog_gateway.dart';
import 'package:guosh_shell/src/desktop/catalog/desktop_connection_editor.dart';
import 'package:guosh_shell/src/desktop/desktop_widgets.dart';
import 'package:guosh_shell/src/desktop/keys/desktop_keys_page.dart';
import 'package:guosh_shell/src/desktop/windows_chrome.dart';
import 'package:guosh_shell/src/desktop/windows_shell.dart';
import 'package:guosh_shell/src/platform_target.dart';
import 'package:guosh_shell/src/terminal/frame_terminal.dart';
import 'package:guosh_shell/src/terminal/session_target.dart';
import 'package:guosh_shell/src/terminal/terminal_pane.dart';
import 'package:integration_test/integration_test.dart';
import 'package:rinf/rinf.dart';
import 'package:terminal_view/terminal_view.dart';
import 'package:window_manager/window_manager.dart';

// 由 scripts/ci/windows_auth_check.ps1 启动；不预写 known_hosts、不替代传输或认证提示。
// 接缝仅隔离 support 与调试自动连接；AppReady、设置、AppStart 均走真实 Rust。
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

// 原生 WM_CHAR 提交到本测试窗口的 Flutter 子窗口，不直接发 Rust 输入信号。
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
  Duration timeout = const Duration(seconds: 20),
}) async {
  final elapsed = Stopwatch()..start();
  while (!await ready()) {
    if (elapsed.elapsed >= timeout) {
      throw TestFailure('等待超时：$description');
    }
    // integration_test 的真实时钟下泵帧，让 native 信号、布局和输入消息继续处理。
    // 每轮重新检查状态，不依赖固定延迟或持续 demo 流永远无法完成的 pumpAndSettle。
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
    throw TestFailure('请通过 Windows 认证 runner 提供 $name');
  }
  return value;
}

// fixture 只将输入回显染为 SGR 36；读取真实 TerminalView 使用的展示单元格，
// 不让并行日志流把完整有序的输入回显误判为丢字，也不接受缺字、重字或错序。
String _fixtureEcho(FrameTerminal terminal) {
  final text = StringBuffer();
  for (var row = 0; row < terminal.height; row++) {
    final line = terminal.lineAt(row);
    for (var col = 0; col < terminal.viewWidth; col++) {
      final codePoint = line.getCodePoint(col);
      if (line.getForeground(col) == (CellColor.named | 6) && codePoint != 0) {
        text.writeCharCode(codePoint);
        text.write(line.getCombined(col) ?? '');
      }
    }
  }
  return text.toString();
}

int _fixtureLogSequence(FrameTerminal terminal) =>
    RegExp(r'日志行 (\d+)：')
        .allMatches(terminal.getText())
        .map((match) => int.parse(match.group(1)!))
        .fold(-1, (latest, seq) => seq > latest ? seq : latest);

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('Windows 真实目录、Fluent 首见认证、Native SSH 回显与返回管理', (tester) async {
    if (!Platform.isWindows) {
      throw TestFailure('此 smoke 只允许在 Windows 桌面设备上运行');
    }
    final run = Directory(_requiredEnv('GUOSH_AUTH_RUN'));
    final runId = _requiredEnv('GUOSH_AUTH_RUN_ID');
    final owned = Directory(_requiredEnv('GUOSH_DESKTOP_TEST_DATA'));
    final fixture = Directory(_requiredEnv('GUOSH_DEMO_DATA'));
    final inputLog = File(_requiredEnv('GUOSH_DEMO_INPUT_LOG'));
    final port = int.parse(_requiredEnv('GUOSH_AUTH_PORT'));
    final fingerprint = _requiredEnv('GUOSH_AUTH_FINGERPRINT');
    final separator = Platform.pathSeparator;
    final runPath = await run.resolveSymbolicLinks();
    final ownedPath = await owned.resolveSymbolicLinks();
    expect(
      run.uri.pathSegments.where((part) => part.isNotEmpty).last,
      'guossh-auth-$runId',
    );
    expect(await File('$runPath$separator.owner').readAsString(), runId);
    expect(ownedPath, '$runPath${separator}app');
    expect(await fixture.resolveSymbolicLinks(), '$runPath${separator}server');
    expect(inputLog.absolute.path, '$runPath${separator}received.txt');
    expect(await owned.list().isEmpty, isTrue, reason: 'support 必须是本轮全新目录');
    expect(await inputLog.exists(), isFalse);
    expect(port, inInclusiveRange(1, 65535));
    expect(fingerprint, matches(RegExp(r'^SHA256:[A-Za-z0-9+/]+$')));
    final publicKey = (await File(
      '${fixture.path}${separator}hostkey.pub',
    ).readAsString()).trim().split(RegExp(r'\s+'));
    expect(publicKey[0], 'ssh-ed25519');
    final knownHosts = File('${owned.path}${separator}known_hosts');
    expect(await knownHosts.exists(), isFalse);

    var rustStarted = false;
    final listeners = <StreamSubscription>[];
    final prompts = <InteractionPrompt>[];
    final statuses = <SessionStatus>[];
    final frames = <FrameUpdate>[];
    final keyResults = <KeyResult>[];
    final keyStates = <KeyListState>[];
    try {
      await WindowsDesktopWindow.initialize();
      await initializeRust(assignRustSignal);
      rustStarted = true;
      // 桌面窗格走 V2：提示、状态与帧经带来源的 Desktop* 信号到达。
      listeners.addAll([
        DesktopPrompt.rustSignalStream.listen(
          (pack) => prompts.add(pack.message.prompt),
        ),
        DesktopSessionStatus.rustSignalStream.listen(
          (pack) => statuses.add(pack.message.status),
        ),
        DesktopFrame.rustSignalStream.listen(
          (pack) => frames.add(pack.message.frame),
        ),
        KeyResult.rustSignalStream.listen(
          (pack) => keyResults.add(pack.message),
        ),
        KeyListState.rustSignalStream.listen(
          (pack) => keyStates.add(pack.message),
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
      expect(find.byType(WindowsFrame), findsOneWidget);
      expect(WindowsDesktopWindow.initialized, isTrue);
      expect(
        await File('${owned.path}${separator}catalog.sqlite3').exists(),
        isTrue,
      );
      await _appears(tester, find.textContaining('还没有保存的连接'));
      expect((await _catalog(tester, '$runId-initial')).connections, isEmpty);
      const gateway = RustDesktopCatalogGateway();
      gateway.query('');

      // 真实 Fluent 编辑器保存元数据；密码保持空且不保存，不触发私钥或同步操作。
      const name = 'auth-smoke-fixture';
      await tester.tap(_iconButton('添加连接'));
      await _appears(tester, find.byType(DesktopConnectionEditor));
      await _field(tester, '名称', name);
      await _field(tester, '主机', '127.0.0.1');
      await _field(tester, '端口', '$port');
      await _field(tester, '用户名', 'probe');
      final savePassword = find.byKey(
        const fluent.Key('catalog-save-password'),
      );
      await tester.ensureVisible(savePassword);
      // 滚动要落帧后命中测试才用新位置。
      await tester.pumpAndSettle();
      expect(tester.widget<fluent.ToggleSwitch>(savePassword).checked, isTrue);
      await tester.tapAt(
        tester.getTopLeft(savePassword) +
            fluent.Offset(20, tester.getSize(savePassword).height / 2),
      );
      await tester.pump();
      expect(tester.widget<fluent.ToggleSwitch>(savePassword).checked, isFalse);
      expect(
        tester
            .widget<fluent.TextBox>(
              find.byKey(const fluent.ValueKey('catalog-field-命令（可选）')),
            )
            .controller!
            .text,
        isEmpty,
        reason: 'demo fixture 必须走 shell_request，绝不执行本机命令',
      );
      await tester.ensureVisible(find.text('保存'));
      await tester.tap(find.text('保存'));
      await _appears(tester, find.text(name));
      final entry = (await _catalog(tester, name)).connections.single;
      expect(entry.host, '127.0.0.1');
      expect(entry.port, port);
      expect(entry.username, 'probe');
      expect(entry.auth, AuthMethod.password);
      expect(entry.passwordSaved, isFalse);
      expect(entry.keyId, isEmpty);
      expect(entry.command, isEmpty);
      gateway.query('');
      expect(await knownHosts.exists(), isFalse);
      debugPrint('[auth-smoke] 真实 Rust 目录保存成功；密码未保存，命令为空');

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
      expect(pane.target.connectionId, entry.id);
      final sessionId = pane.sessionId;
      expect(sessionId, isPositive);

      // 生产路径先问空密码，再握手；只通过 Fluent 掩码输入回答真实 prompt。
      await _appears(tester, find.text('输入密码'));
      final passwordPrompt = prompts.single;
      expect(passwordPrompt.kind, PromptKind.password);
      expect(passwordPrompt.sessionId, sessionId);
      expect(passwordPrompt.retry, isFalse);
      expect(find.byType(fluent.ContentDialog), findsOneWidget);
      final passwordBox = find.descendant(
        of: find.byType(fluent.ContentDialog),
        matching: find.byType(fluent.TextBox),
      );
      expect(tester.widget<fluent.TextBox>(passwordBox).obscureText, isTrue);
      for (final box in tester.widgetList<fluent.Checkbox>(
        find.descendant(
          of: find.byType(fluent.ContentDialog),
          matching: find.byType(fluent.Checkbox),
        ),
      )) {
        expect(box.checked, isFalse, reason: '本轮不得保存凭证');
      }
      await tester.enterText(passwordBox, 'probe');
      await tester.tap(find.widgetWithText(fluent.FilledButton, '连接'));

      await _appears(tester, find.text('确认主机密钥'));
      final hostPrompt = prompts.last;
      expect(prompts, hasLength(2));
      expect(hostPrompt.kind, PromptKind.hostKey);
      expect(hostPrompt.sessionId, sessionId);
      expect(hostPrompt.changed, isFalse, reason: '变更密钥绝不自动接受');
      expect(hostPrompt.host, '127.0.0.1');
      expect(hostPrompt.port, port);
      expect(hostPrompt.algorithm, publicKey[0]);
      expect(hostPrompt.fingerprint, fingerprint);
      expect(
        find.textContaining('${publicKey[0]}\n$fingerprint'),
        findsOneWidget,
      );
      expect(find.text('替换旧密钥并连接'), findsNothing);
      expect(await knownHosts.exists(), isFalse);
      await tester.tap(find.widgetWithText(fluent.FilledButton, '信任并连接'));
      debugPrint('[auth-smoke] Fluent 密码及首见 host-key 已核对；仅接受本轮 fixture');

      await _wait(
        tester,
        'Native SSH connected 与 demo banner',
        () =>
            pane.connected &&
            pane.lastFrameSeq > 0 &&
            pane.terminal.getText().contains('GuoSSHell M1'),
      );
      expect(
        statuses.any(
          (status) =>
              status.sessionId == sessionId &&
              status.state == SessionState.connected,
        ),
        isTrue,
      );
      expect(
        frames.any(
          (frame) =>
              frame.sessionId == sessionId && frame.cols > 0 && frame.rows > 0,
        ),
        isTrue,
      );
      expect(find.text('已连接'), findsOneWidget);
      await _wait(tester, 'TOFU 在隔离 support 落盘', knownHosts.exists);
      expect(
        await knownHosts.readAsString(),
        contains('${publicKey[0]} ${publicKey[1]}'),
      );

      final native = _NativeText();
      await native.initialize();
      await tester.tap(
        find.byType(TerminalView),
        kind: PointerDeviceKind.mouse,
      );
      await _wait(
        tester,
        '原生终端文本输入连接',
        () => tester
            .state<TerminalViewState>(find.byType(TerminalView))
            .hasInputConnection,
      );
      const echo = 'authsmokeecho123';
      final expectedHex = utf8
          .encode(echo)
          .map((byte) => byte.toRadixString(16).padLeft(2, '0'))
          .join();
      native.commit(echo);
      await _wait(
        tester,
        'fixture 收到原生输入且终端帧出现回显',
        () async =>
            await inputLog.exists() &&
            (await inputLog.readAsString()).replaceAll(RegExp(r'\s'), '') ==
                expectedHex &&
            _fixtureEcho(pane.terminal) == echo,
      );
      expect(prompts, hasLength(2), reason: '不得悄悄接受额外认证请求');
      debugPrint('[auth-smoke] Native connected/frame/banner 与原生输入对端日志+终端回显通过');

      // 不断开真实 Native 会话，仅往返管理 UI；回来仍由同一 pane/session 继续原生输入。
      final paneElement = tester.element(find.byType(TerminalPane));
      final frameBeforeManagement = pane.lastFrameSeq;
      await tester.tap(_iconButton('添加连接'));
      await _appears(tester, find.byType(DesktopConnectionEditor));
      await _field(tester, '名称', 'live-session-management');
      await tester.ensureVisible(find.text('取消'));
      await tester.tap(find.text('取消'));
      await _wait(
        tester,
        '存活会话编辑器取消',
        () => find.byType(DesktopConnectionEditor).evaluate().isEmpty,
      );
      await tester.tap(_iconButton('私钥管理'));
      await _appears(tester, find.byType(DesktopKeysPage));
      expect(find.text('还没有私钥'), findsOneWidget);
      expect(pane.connected, isTrue);
      expect(pane.sessionId, sessionId);
      await tester.tap(find.text('返回工作区'));
      await _wait(
        tester,
        '管理面板退出且真实终端已可命中',
        () =>
            find.byType(DesktopKeysPage).evaluate().isEmpty &&
            find.byType(TerminalView).hitTestable().evaluate().isNotEmpty,
      );
      expect(tester.element(find.byType(TerminalPane)), same(paneElement));
      expect(
        tester.widget<TerminalPane>(find.byType(TerminalPane)).controller,
        same(pane),
      );
      expect(tester.state(find.byType(WindowsShell)), same(shell));
      expect(pane.connected, isTrue);
      expect(pane.sessionId, sessionId);
      expect(
        statuses.where((status) => status.sessionId == sessionId).last.state,
        SessionState.connected,
      );
      await tester.tap(
        find.byType(TerminalView),
        kind: PointerDeviceKind.mouse,
      );
      await _wait(
        tester,
        '管理往返后原生输入连接',
        () => tester
            .state<TerminalViewState>(find.byType(TerminalView))
            .hasInputConnection,
      );
      const afterManagement = 'authaftermanagement456';
      final completeHex = utf8
          .encode('$echo$afterManagement')
          .map((byte) => byte.toRadixString(16).padLeft(2, '0'))
          .join();
      // 第二段原生输入必须实际穿插 fixture 日志，验证回显无缺字、重字或错序。
      // 不额外发任何输入字节，也不暂停 fixture；全部输入仍经同一原生文本连接。
      final prefix = afterManagement.substring(0, 3);
      final prefixHex = utf8
          .encode('$echo$prefix')
          .map((byte) => byte.toRadixString(16).padLeft(2, '0'))
          .join();
      native.commit(prefix);
      await _wait(
        tester,
        '第二段前缀已精确收到并显示',
        () async =>
            (await inputLog.readAsString()).replaceAll(RegExp(r'\s'), '') ==
                prefixHex &&
            _fixtureEcho(pane.terminal) == '$echo$prefix',
      );
      final logBeforeSuffix = _fixtureLogSequence(pane.terminal);
      await _wait(
        tester,
        'fixture 日志已在第二段前缀之后显示',
        () => _fixtureLogSequence(pane.terminal) > logBeforeSuffix,
      );
      native.commit(afterManagement.substring(prefix.length));
      try {
        await _wait(
          tester,
          '同一会话管理往返后继续原生输入及精确对端/回显',
          () async =>
              pane.connected &&
              pane.sessionId == sessionId &&
              pane.lastFrameSeq > frameBeforeManagement &&
              (await inputLog.readAsString()).replaceAll(RegExp(r'\s'), '') ==
                  completeHex &&
              _fixtureEcho(pane.terminal) == '$echo$afterManagement',
        );
      } on TestFailure {
        // 仅输出受控输入证据及状态，不输出主机密钥、凭证或应用目录。
        final actualHex = (await inputLog.readAsString()).replaceAll(
          RegExp(r'\s'),
          '',
        );
        final terminalText = pane.terminal.getText();
        debugPrint(
          '[auth-smoke] 第二段诊断：connected=${pane.connected} '
          'sameSession=${pane.sessionId == sessionId} '
          'newFrame=${pane.lastFrameSeq > frameBeforeManagement} '
          'exactInput=${actualHex == completeHex} '
          'receivedBytes=${actualHex.length ~/ 2} '
          'exactEcho=${_fixtureEcho(pane.terminal) == '$echo$afterManagement'} '
          'projectedEcho=${_fixtureEcho(pane.terminal)}',
        );
        debugPrint(
          '[auth-smoke] 受控回显/resize 行：${terminalText.split('\n').where((line) => line.contains('aut') || line.contains('management') || line.contains('456') || line.contains('RESIZE-ACK')).join(' | ')}',
        );
        rethrow;
      }
      expect(
        pane.terminal.getText(),
        isNot(contains(afterManagement)),
        reason: '受控 fixture 日志必须实际打断第二段连续子串，不能退化为未交错的通过',
      );
      expect(prompts, hasLength(2));
      final liveStatuses = statuses.where(
        (status) => status.sessionId == sessionId,
      );
      expect(
        liveStatuses.where((status) => status.state == SessionState.connected),
        hasLength(1),
        reason: '管理往返不得断开后重连来替代原会话',
      );
      expect(
        liveStatuses.any(
          (status) =>
              status.state == SessionState.failed ||
              status.state == SessionState.closed,
        ),
        isFalse,
      );
      debugPrint(
        '[auth-smoke] 同一存活 pane/session 管理往返后，fixture 日志确已插入第二段原生输入；对端与展示回显完整 38 字符均精确匹配',
      );

      await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
      await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
      await tester.sendKeyEvent(LogicalKeyboardKey.keyW);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
      await _appears(tester, find.text('关闭窗格？'));
      await tester.tap(find.widgetWithText(fluent.FilledButton, '断开'));
      await _wait(
        tester,
        '关闭会话后返回常驻管理目录',
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
      expect(find.text('就绪'), findsOneWidget);

      // 管理入口仍然真实可操作；不再发起第二条连接。
      await tester.tap(_iconButton('添加连接'));
      await _appears(tester, find.byType(DesktopConnectionEditor));
      await _field(tester, '名称', 'management-still-operable');
      await tester.ensureVisible(find.text('取消'));
      await tester.tap(find.text('取消'));
      await _wait(
        tester,
        '管理编辑器取消',
        () => find.byType(DesktopConnectionEditor).evaluate().isEmpty,
      );
      final saved = (await _catalog(tester, name)).connections.single;
      expect(saved.id, entry.id);
      expect(saved.passwordSaved, isFalse);
      expect(saved.command, isEmpty);
      expect(saved.keyId, isEmpty);
      gateway.queryKeys();
      await _wait(tester, '本轮私钥只读快照', () => keyStates.isNotEmpty);
      expect(
        keyStates.every(
          (state) =>
              state.keys.isEmpty &&
              !state.syncEnabled &&
              !state.syncPending &&
              !state.syncTargetEnabled,
        ),
        isTrue,
      );
      expect(keyResults, isEmpty);
      expect(tester.takeException(), isNull);
      debugPrint('[auth-smoke] Rust closed、常驻 Windows 管理入口与再次编辑通过；无私钥/同步变更');
    } finally {
      try {
        await tester.pumpWidget(const fluent.SizedBox.shrink());
      } finally {
        for (final listener in listeners) {
          await listener.cancel();
        }
        // 同现有真实目录 smoke：同步退出 Rust 并 join SQLite worker 后才删除 support。
        if (rustStarted) finalizeRust();
        expect(await owned.resolveSymbolicLinks(), ownedPath);
        expect(await run.resolveSymbolicLinks(), runPath);
        expect(await File('$runPath$separator.owner').readAsString(), runId);
        await owned.delete(recursive: true);
        expect(await owned.exists(), isFalse);
        debugPrint(
          '[auth-smoke] widget/listener/Rust 已结束；SQLite support 精确删除成功',
        );
      }
    }
  });
}
