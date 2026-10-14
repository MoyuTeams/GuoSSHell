import 'dart:async';

import 'package:fluent_ui/fluent_ui.dart' as f;
import 'package:flutter/foundation.dart'
    show debugDefaultTargetPlatformOverride;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/desktop/desktop_shortcuts.dart';
import 'package:guosh_shell/src/desktop/desktop_widgets.dart';
import 'package:guosh_shell/src/desktop/windows_chrome.dart';
import 'package:guosh_shell/src/desktop/windows_shell.dart';
import 'package:guosh_shell/src/desktop/workspace/desktop_target_options.dart';
import 'package:guosh_shell/src/desktop/workspace/desktop_workspace_scope.dart';
import 'package:guosh_shell/src/terminal/desktop_ordered_session.dart';
import 'package:guosh_shell/src/terminal/session_target.dart';
import 'package:guosh_shell/src/terminal/terminal_pane.dart';
import 'package:guosh_shell/src/workspace/workspace.dart';
import 'package:rinf/rinf.dart';

import 'desktop_catalog_flow_test.dart' show FakeCatalog, emptyKeyState;
import 'desktop_session_controller_test.dart'
    show DesktopSessionTestGateway, prepared, u64;

class _Options implements DesktopTargetOptionsGateway {
  _Options({this.local = true});
  final bool local;
  int queries = 0;
  @override
  Future<DesktopTargetOptions> query() async {
    queries++;
    return DesktopTargetOptions(
      profiles: const [],
      nativeAvailable: true,
      localAvailable: local,
      localReason: '本机不支持本地 shell',
    );
  }
}

Widget _host(Widget child) => MaterialApp(
  theme: desktopMaterialTheme(),
  localizationsDelegates: const [f.FluentLocalizations.delegate],
  builder: (_, child) => f.FluentTheme(
    data: desktopFluentTheme(reduceMotion: true),
    child: child!,
  ),
  home: child,
);

Finder get _sidebarLocal => find.byKey(const ValueKey('workspace-local'));
Finder get _welcomeLocal => find.byKey(const ValueKey('welcome-local'));

/// 空工作区的真实壳；窗格用替身渲染，只记录打开的目标。
Future<List<WorkspacePane>> _mountShell(
  WidgetTester tester,
  _Options options, {
  TargetPlatform platform = TargetPlatform.windows,
}) async {
  tester.view.physicalSize = const Size(1280, 900);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  debugDefaultTargetPlatformOverride = platform;
  addTearDown(() => debugDefaultTargetPlatformOverride = null);
  final catalog = FakeCatalog();
  addTearDown(catalog.close);
  final panes = <WorkspacePane>[];
  await tester.pumpWidget(
    _host(
      WindowsShell(
        gateway: catalog,
        targetOptions: options,
        paneBuilder: (pane) {
          if (!panes.contains(pane)) panes.add(pane);
          return Focus(
            focusNode: pane.controller.focusNode,
            child: Text('pane-${pane.controller.target.title}'),
          );
        },
      ),
    ),
  );
  catalog.catalog.add(const CatalogState(query: '', connections: []));
  catalog.keys.add(emptyKeyState);
  await tester.pumpAndSettle();
  return panes;
}

/// 本地终端窗格：测试网关代替 Rust；Disconnect 后报 lifecycle ended。
class _LocalPane {
  final gateway = DesktopSessionTestGateway();
  final controller = TerminalPaneController(const SessionTarget.local());
  int closes = 0;
  int revision = 0;
  DesktopSource get source => gateway.prepares.single.source;

  void lifecycle(NativeLifecycleState state) => gateway.lifecycleBus.add(
    RustSignalPack(
      DesktopNativeLifecycle(
        version: 1,
        source: source,
        identity: 'frozen',
        attempt: u64(12),
        focusEpoch: u64(1),
        lifecycleRevision: u64(++revision),
        state: state,
      ),
      Uint8List(0),
    ),
  );

  void status(SessionState state, [String detail = '']) =>
      gateway.statusBus.add(
        RustSignalPack(
          DesktopSessionStatus(
            source: source,
            identity: 'frozen',
            status: SessionStatus(
              sessionId: source.sessionId,
              state: state,
              failure: FailureKind.none,
              detail: detail,
              localNetworkSettingsUrl: '',
              hint: ConnectHint.none,
            ),
          ),
          Uint8List(0),
        ),
      );

  Future<void> mount(WidgetTester tester) async {
    tester.view.physicalSize = const Size(1280, 900);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    debugDefaultTargetPlatformOverride = TargetPlatform.windows;
    final oldSettings = SettingsState.latestRustSignal;
    SettingsState.latestRustSignal = RustSignalPack(
      const SettingsState(
        fontSize: 14,
        minFontSize: 8,
        maxFontSize: 64,
        scrollbackLines: 1000,
        maxScrollbackLines: 10000,
        showKeyBar: false,
        keyBarRows: [],
      ),
      Uint8List(0),
    );
    addTearDown(() {
      SettingsState.latestRustSignal = oldSettings;
      debugDefaultTargetPlatformOverride = null;
    });
    gateway.onOrdered = (request) {
      if (request.event is DesktopEventDisconnect) {
        scheduleMicrotask(() => lifecycle(NativeLifecycleState.ended));
      }
      return DesktopSessionTestGateway.accept(request);
    };
    await tester.pumpWidget(
      _host(
        DesktopWorkspaceScope(
          execute: (_, {required origin}) async =>
              DesktopActionOutcome.executed,
          ownsTerminalInput: (_, _) {},
          isCurrentOrigin: (origin) => identical(origin, controller),
          child: TerminalPane(
            controller: controller,
            desktopGateway: gateway,
            onActivate: () {},
            onClose: () => closes++,
          ),
        ),
      ),
    );
    await tester.pump();
    final snapshot = prepared(gateway.prepares.single.requestId);
    gateway.emit(snapshot.copyWith(source: source));
    await tester.pumpAndSettle();
    expect(gateway.starts, hasLength(1));
    gateway.emit(
      snapshot.copyWith(
        source: source,
        requestId: gateway.starts.single.requestId,
        state: DesktopPreparationState.accepted,
        snapshot: () => null,
      ),
    );
    lifecycle(NativeLifecycleState.authenticated);
    status(SessionState.connected, '本地 shell');
    await tester.pumpAndSettle();
  }

  Future<void> dispose(WidgetTester tester) async {
    await tester.pumpWidget(const SizedBox.shrink());
    await tester.pumpAndSettle();
    controller.dispose();
    await gateway.dispose();
  }
}

/// 平台覆盖须在测试体结束前复位（框架在 tearDown 之前校验 debug 变量）。
void _widgets(String name, Future<void> Function(WidgetTester) body) =>
    testWidgets(name, (tester) async {
      try {
        await body(tester);
      } finally {
        debugDefaultTargetPlatformOverride = null;
      }
    });

void main() {
  test('本地目标：种类、标题与空的 SSH 字段；profile 显式选择保留', () {
    const local = SessionTarget.local();
    expect(local.isLocal, isTrue);
    expect(local.kind, SessionTargetKind.local);
    expect(local.title, '本地终端');
    expect([
      local.connectionId,
      local.host,
      local.username,
      local.password,
    ], everyElement(isEmpty));
    expect(local.port, 0);
    expect(local.profileId, isEmpty);
    expect(const SessionTarget.local(profileId: 'p').profileId, 'p');
    final chosen = desktopTargetWithProfile(local, 'q');
    expect(chosen.isLocal, isTrue);
    expect(chosen.profileId, 'q');
    expect(
      const SessionTarget.quick(host: 'h', port: 22, username: 'u').isLocal,
      isFalse,
    );
    expect(desktopTargetKind(local), DesktopTargetKind.localShell);
    expect(
      desktopTargetKind(
        const SessionTarget.saved(connectionId: 'a', title: 'a'),
      ),
      DesktopTargetKind.nativeSsh,
    );
  });

  test('只解析固定退出格式；Ctrl/Cmd+Shift+L 映射本地终端且不交 Rust 动作', () {
    expect(localShellExitCode('exit code=Some(0) success=true'), 0);
    expect(localShellExitCode('exit code=Some(3) success=false'), 3);
    expect(localShellExitCode('exit code=None success=false'), isNull);
    expect(localShellExitCode('eof'), isNull);
    expect(localShellExitCode('本地 shell'), isNull);
    expect(
      desktopCommand(LogicalKeyboardKey.keyL, control: true, shift: true),
      DesktopCommand.localTerminal,
    );
    expect(
      desktopCommand(
        LogicalKeyboardKey.keyL,
        control: false,
        shift: true,
        meta: true,
      ),
      DesktopCommand.localTerminal,
    );
    expect(
      desktopCommand(LogicalKeyboardKey.keyL, control: true, shift: false),
      isNull,
    );
    expect(
      desktopDefaultAction(
        LogicalKeyboardKey.keyL,
        control: true,
        shift: true,
        alt: false,
        meta: false,
      ),
      isNull,
    );
  });

  _widgets('桌面侧栏、欢迎页与快捷键打开本地终端标签', (tester) async {
    final options = _Options();
    final panes = await _mountShell(tester, options);
    expect(_sidebarLocal, findsOneWidget);
    expect(_welcomeLocal, findsOneWidget);
    expect(find.text('Ctrl + Shift + L'), findsOneWidget);

    await tester.tap(_welcomeLocal);
    await tester.pumpAndSettle();
    expect(panes, hasLength(1));
    expect(panes.single.controller.target.isLocal, isTrue);
    expect(panes.single.controller.target.profileId, isEmpty);
    expect(find.text('本地终端'), findsWidgets);

    await tester.tap(_sidebarLocal);
    await tester.pumpAndSettle();
    expect(panes, hasLength(2));
    expect(panes.last.controller.target.isLocal, isTrue);
    // 替身窗格持焦时壳截获快捷键（真实 V2 窗格的按键交 Rust 仲裁）。
    await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
    await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyL);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
    await tester.pumpAndSettle();
    expect(panes, hasLength(3));
    expect(options.queries, 3);
    expect(tester.takeException(), isNull);
  });

  _widgets('空工作区快捷键 Ctrl+Shift+L 打开本地终端', (tester) async {
    final panes = await _mountShell(tester, _Options());
    await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
    await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyL);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
    await tester.pumpAndSettle();
    expect(panes, hasLength(1));
    expect(panes.single.controller.target.isLocal, isTrue);
  });

  _widgets('能力报告本地 shell 不可用：不开标签，入口禁用并提示原因', (tester) async {
    final options = _Options(local: false);
    final panes = await _mountShell(tester, options);
    await tester.tap(_sidebarLocal);
    await tester.pumpAndSettle();
    expect(panes, isEmpty);
    expect(find.text('本机不支持本地 shell'), findsOneWidget);
    final button = tester.widget<DesktopIconButton>(_sidebarLocal);
    expect(button.onPressed, isNull);
    expect(button.label, contains('本机不支持本地 shell'));
    expect(tester.widget<f.Button>(_welcomeLocal).onPressed, isNull);
    await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
    await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyL);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
    await tester.pumpAndSettle();
    expect(panes, isEmpty);
    expect(options.queries, 1);
  });

  _widgets('移动端不显示任何本地入口，快捷键也不打开', (tester) async {
    final options = _Options();
    final panes = await _mountShell(
      tester,
      options,
      platform: TargetPlatform.android,
    );
    expect(_sidebarLocal, findsNothing);
    expect(_welcomeLocal, findsNothing);
    expect(find.text('本地终端'), findsNothing);
    await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
    await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyL);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
    await tester.pumpAndSettle();
    expect(panes, isEmpty);
    expect(options.queries, 0);
  });

  _widgets('Prepare 目标为 LocalShell、SSH 字段为空，Start 不带密码且不弹认证', (tester) async {
    final h = _LocalPane();
    await h.mount(tester);
    final prepare = h.gateway.prepares.single;
    expect(prepare.target, DesktopTargetKind.localShell);
    expect(prepare.connectionId, isEmpty);
    expect(prepare.host, isEmpty);
    expect(prepare.username, isEmpty);
    expect(prepare.command, isEmpty);
    expect(prepare.port, 0);
    expect(prepare.profileId, isEmpty);
    expect(h.gateway.starts.single.password, isEmpty);
    expect(find.byType(f.ContentDialog), findsNothing);
    expect(find.textContaining('进程已退出'), findsNothing);
    await h.dispose(tester);
    expect(tester.takeException(), isNull);
  });

  _widgets('非 0 退出保留窗格显示退出码与重新启动；之后 exit 0 关闭窗格一次', (tester) async {
    final h = _LocalPane();
    await h.mount(tester);
    h.status(SessionState.closed, 'exit code=Some(3) success=false');
    await tester.pumpAndSettle();
    expect(find.text('进程已退出（代码 3）'), findsOneWidget);
    expect(h.closes, 0);
    await tester.tap(find.widgetWithText(TextButton, '重新启动'));
    await tester.pumpAndSettle();
    expect(
      h.gateway.orderedEvents.where((r) => r.event is DesktopEventReconnect),
      hasLength(1),
    );
    h.status(SessionState.connecting, '本地 shell');
    h.status(SessionState.connected, '本地 shell');
    await tester.pumpAndSettle();
    expect(find.textContaining('进程已退出'), findsNothing);

    h.status(SessionState.closed, 'exit code=Some(0) success=true');
    await tester.pumpAndSettle();
    expect(h.closes, 1);
    // 状态重发不重复关窗格。
    h.status(SessionState.closed, 'exit code=Some(0) success=true');
    await tester.pumpAndSettle();
    expect(h.closes, 1);
    // 关闭未生效（如壳有对话框）时重新启动，下一次 exit 0 仍会关窗格。
    h.status(SessionState.connected, '本地 shell');
    await tester.pumpAndSettle();
    h.status(SessionState.closed, 'exit code=Some(0) success=true');
    await tester.pumpAndSettle();
    expect(h.closes, 2);
    await h.dispose(tester);
    expect(tester.takeException(), isNull);
  });

  _widgets('无退出码的结束显示「进程已退出」，不自动关窗格', (tester) async {
    final h = _LocalPane();
    await h.mount(tester);
    h.status(SessionState.closed, 'exit code=None success=false');
    await tester.pumpAndSettle();
    expect(find.text('进程已退出'), findsOneWidget);
    expect(find.widgetWithText(TextButton, '重新启动'), findsOneWidget);
    expect(h.closes, 0);
    await h.dispose(tester);
  });
}
