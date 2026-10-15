import 'dart:async';

import 'package:fluent_ui/fluent_ui.dart' as f;
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/desktop/catalog/desktop_connection_editor.dart';
import 'package:guosh_shell/src/desktop/keys/desktop_keys_page.dart';
import 'package:guosh_shell/src/desktop/desktop_widgets.dart';
import 'package:guosh_shell/src/desktop/windows_chrome.dart';
import 'package:guosh_shell/src/desktop/windows_sidebar.dart';
import 'package:guosh_shell/src/desktop/windows_shell.dart';
import 'package:guosh_shell/src/desktop/workspace/desktop_target_options.dart';
import 'package:guosh_shell/src/desktop/workspace/desktop_workspace_scope.dart';
import 'package:guosh_shell/src/terminal/session_target.dart';
import 'package:guosh_shell/src/workspace/workspace.dart';
import 'package:multi_split_view/multi_split_view.dart';

import 'desktop_catalog_flow_test.dart' show FakeCatalog, saved, emptyKeyState;

class _Options implements DesktopTargetOptionsGateway {
  bool available = true;
  bool system = false;
  Completer<DesktopTargetOptions>? pending;
  int queries = 0;
  DesktopTargetOptions get value => DesktopTargetOptions(
    profiles: const [
      DesktopProfileOption('a', '配置 A'),
      DesktopProfileOption('b', '配置 B'),
    ],
    nativeAvailable: available,
    systemAvailable: system,
    nativeReason: 'Native 当前不可用',
    systemReason: 'System 固定不可用原因',
    localReason: 'Local 固定不可用原因',
  );
  @override
  Future<DesktopTargetOptions> query() {
    queries++;
    return pending?.future ?? Future.value(value);
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

Finder iconButton(String label) => find.byWidgetPredicate(
  (widget) => widget is DesktopIconButton && widget.label == label,
);

class _Harness {
  final catalog = FakeCatalog();
  final options = _Options();
  final panes = <WorkspacePane>[];
  DesktopWorkspaceScope scope(WidgetTester tester) =>
      tester.widget<DesktopWorkspaceScope>(find.byType(DesktopWorkspaceScope));
  Future<void> mount(
    WidgetTester tester, {
    double width = 1280,
    SessionTarget? initial,
    bool empty = false,
  }) async {
    tester.view.physicalSize = Size(width, 900);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    addTearDown(catalog.close);
    await tester.pumpWidget(
      _host(
        WindowsShell(
          initial: empty
              ? null
              : initial ??
                    const SessionTarget.saved(
                      connectionId: 'origin',
                      title: 'origin',
                      profileId: 'a',
                    ),
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
    catalog.catalog.add(const CatalogState(query: '', connections: [saved]));
    catalog.keys.add(emptyKeyState);
    await tester.pumpAndSettle();
    if (panes.isNotEmpty) panes.first.controller.focusNode.requestFocus();
    await tester.pump();
  }

  Future<DesktopActionOutcome> action(
    WidgetTester tester,
    DesktopAction action,
    WorkspacePane pane,
  ) async {
    pane.controller.focusNode.requestFocus();
    await tester.pump();
    return scope(tester).execute(action, origin: pane.controller);
  }
}

Future<void> shortcut(
  WidgetTester tester,
  LogicalKeyboardKey key, {
  bool settle = true,
}) async {
  await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
  await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
  await tester.sendKeyEvent(key);
  await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
  await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
  if (settle) await tester.pumpAndSettle();
}

void main() {
  tearDown(() => debugDefaultTargetPlatformOverride = null);
  for (final destination in ['search', 'pane', 'unmount']) {
    testWidgets('真实快捷键 NewTab 查询中转到 $destination 不打开陈旧面板', (tester) async {
      final h = _Harness();
      await h.mount(tester);
      if (destination == 'pane') {
        await shortcut(tester, LogicalKeyboardKey.keyD);
        expect(h.panes, hasLength(2));
        expect(h.panes.last.controller.focusNode.hasFocus, isTrue);
      }
      h.options.pending = Completer<DesktopTargetOptions>();
      await shortcut(tester, LogicalKeyboardKey.keyT);
      expect(h.options.queries, 1);
      expect(find.byType(DesktopConnectionEditor), findsNothing);
      if (destination == 'search') {
        tester
            .widget<WindowsSidebar>(find.byType(WindowsSidebar))
            .focusNode
            .requestFocus();
        await tester.pump();
      } else if (destination == 'pane') {
        h.panes.first.controller.focusNode.requestFocus();
        await tester.pump();
      } else {
        await tester.pumpWidget(const SizedBox());
      }
      h.options.pending!.complete(h.options.value);
      await tester.pumpAndSettle();
      expect(find.byType(DesktopConnectionEditor), findsNothing);
      if (destination == 'search') {
        expect(
          tester
              .widget<WindowsSidebar>(find.byType(WindowsSidebar))
              .focusNode
              .hasFocus,
          isTrue,
        );
      } else if (destination == 'pane') {
        expect(h.panes.first.controller.focusNode.hasFocus, isTrue);
      }
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
    });
  }

  testWidgets('真实快捷键 Search 的延迟回调不抢另一 pane 焦点', (tester) async {
    final h = _Harness();
    await h.mount(tester);
    await shortcut(tester, LogicalKeyboardKey.keyD);
    await shortcut(tester, LogicalKeyboardKey.keyF, settle: false);
    h.panes.first.controller.focusNode.requestFocus();
    await tester.idle();
    await tester.pump();
    await tester.pump();
    expect(h.panes.first.controller.focusNode.hasFocus, isTrue);
    expect(
      tester
          .widget<WindowsSidebar>(find.byType(WindowsSidebar))
          .focusNode
          .hasFocus,
      isFalse,
    );
    await tester.pumpWidget(const SizedBox());
  });

  for (final confirmed in [false, true]) {
    testWidgets('真实快捷键关闭保护捕获 origin，确认期间换 tab 不关闭/夺焦：$confirmed', (
      tester,
    ) async {
      final h = _Harness();
      await h.mount(tester);
      final first = h.panes.single;
      tester.widget<WindowsSidebar>(find.byType(WindowsSidebar)).onOpen(saved);
      await tester.pumpAndSettle();
      final second = h.panes.last;
      tester.widget<f.TabView>(find.byType(f.TabView)).onChanged!(0);
      await tester.pumpAndSettle();
      expect(first.controller.focusNode.hasFocus, isTrue);
      final element = first.key.currentContext;
      await shortcut(tester, LogicalKeyboardKey.keyW);
      expect(find.text('关闭窗格？'), findsOneWidget);
      tester.widget<f.TabView>(find.byType(f.TabView)).onChanged!(1);
      await tester.pump();
      await tester.tap(find.text(confirmed ? '断开' : '取消'));
      // 异步关闭结果不能再把焦点派发给切换后的终端；它由用户显式持焦。
      final search = tester
          .widget<WindowsSidebar>(find.byType(WindowsSidebar))
          .focusNode;
      search.requestFocus();
      await tester.pumpAndSettle();
      expect(search.hasFocus, isTrue);
      expect(first.key.currentContext, same(element));
      expect(
        tester.widget<f.TabView>(find.byType(f.TabView)).tabs,
        hasLength(2),
      );
      expect(second.controller.focusNode.context, isNotNull);
      await tester.pumpWidget(const SizedBox());
    });
  }

  for (final initialEmpty in [true, false]) {
    testWidgets('空工作区欢迎命令仅壳自身焦点有效，初始为空：$initialEmpty', (tester) async {
      final h = _Harness();
      await h.mount(tester, empty: initialEmpty);
      final root = Focus.of(tester.element(find.byType(WindowsSidebar)));
      if (!initialEmpty) {
        await shortcut(tester, LogicalKeyboardKey.keyW);
        await tester.tap(find.text('断开'));
        await tester.pumpAndSettle();
      }
      expect(root.hasPrimaryFocus, isTrue);
      expect(find.text('连接，开始工作。'), findsOneWidget);
      await shortcut(tester, LogicalKeyboardKey.keyT);
      expect(h.options.queries, 1);
      expect(find.byType(DesktopConnectionEditor), findsOneWidget);
      await shortcut(tester, LogicalKeyboardKey.keyT);
      expect(h.options.queries, 1);
      await tester.tap(find.text('返回工作区'));
      await tester.pumpAndSettle();
      expect(root.hasPrimaryFocus, isTrue);
      await shortcut(tester, LogicalKeyboardKey.keyF);
      final search = tester
          .widget<WindowsSidebar>(find.byType(WindowsSidebar))
          .focusNode;
      expect(search.hasFocus, isTrue);
      await shortcut(tester, LogicalKeyboardKey.keyT);
      expect(h.options.queries, 1);
      await tester.tap(find.text(saved.name));
      await tester.pumpAndSettle();
      expect(root.hasPrimaryFocus, isFalse);
      await shortcut(tester, LogicalKeyboardKey.keyT);
      expect(h.options.queries, 1);
      final authFocus = FocusNode();
      final auth = f.showDialog<void>(
        context: tester.element(find.byType(WindowsShell)),
        builder: (_) => f.ContentDialog(
          title: const Text('认证'),
          content: f.TextBox(autofocus: true, focusNode: authFocus),
        ),
      );
      await tester.pumpAndSettle();
      expect(authFocus.hasFocus, isTrue);
      await shortcut(tester, LogicalKeyboardKey.keyT);
      expect(h.options.queries, 1);
      tester.state<NavigatorState>(find.byType(Navigator)).pop();
      await auth;
      await tester.pumpAndSettle();
      root.requestFocus();
      await tester.pump();
      // 新查询必须再次验证空壳焦点，不能凭此前的欢迎命令权限打开面板。
      h.options.pending = Completer<DesktopTargetOptions>();
      await shortcut(tester, LogicalKeyboardKey.keyT);
      expect(h.options.queries, 2);
      search.requestFocus();
      await tester.pump();
      h.options.pending!.complete(h.options.value);
      await tester.pumpAndSettle();
      expect(find.byType(DesktopConnectionEditor), findsNothing);
      expect(search.hasFocus, isTrue);
      expect(root.hasPrimaryFocus, isFalse);
      await tester.pumpWidget(const SizedBox());
      authFocus.dispose();
    });
  }
  test('显式 profile 不改保存引用/默认，也不丢快速目标字段', () {
    const target = SessionTarget.quick(
      host: 'fixture.test',
      port: 2222,
      username: 'fixture',
      password: 'fixture-only-secret',
      command: 'fixture-command',
    );
    final selected = desktopTargetWithProfile(target, 'b');
    expect(
      [
        selected.host,
        selected.port,
        selected.username,
        selected.password,
        selected.command,
        selected.profileId,
      ],
      [
        target.host,
        target.port,
        target.username,
        target.password,
        target.command,
        'b',
      ],
    );
    final savedTarget = desktopTargetWithProfile(
      const SessionTarget.saved(connectionId: 'saved', title: '标签'),
      'a',
    );
    expect(
      [savedTarget.connectionId, savedTarget.title, savedTarget.profileId],
      ['saved', '标签', 'a'],
    );
    expect(desktopTargetWithProfile(savedTarget, '').profileId, isEmpty);
  });

  for (final width in [375.0, 768.0, 1280.0]) {
    testWidgets('$width 两向分屏/比例/标签保留 controller 与元素，高对比和字体缩放不丢树', (
      tester,
    ) async {
      final h = _Harness();
      await h.mount(tester, width: width);
      final original = h.panes.single;
      final element = original.key.currentContext;
      if (width < 640) {
        await tester.tap(iconButton('连接侧栏 · Ctrl+Shift+B'));
        await tester.pumpAndSettle();
      }
      expect(
        await h.action(tester, DesktopAction.splitRight, original),
        DesktopActionOutcome.executed,
      );
      await tester.pumpAndSettle();
      expect(h.panes, hasLength(2));
      expect(original.key.currentContext, same(element));
      final right = h.panes.last;
      expect(right.controller.target.profileId, 'a');
      var views = tester
          .widgetList<MultiSplitView>(find.byType(MultiSplitView))
          .toList();
      expect(views.single.axis, Axis.horizontal);
      final layout = views.single.controller!;
      layout.areas[0].flex = 2;
      layout.areas[1].flex = 1;
      expect(
        await h.action(tester, DesktopAction.splitDown, right),
        DesktopActionOutcome.executed,
      );
      await tester.pumpAndSettle();
      views = tester
          .widgetList<MultiSplitView>(find.byType(MultiSplitView))
          .toList();
      expect(
        views.map((view) => view.axis),
        containsAll([Axis.horizontal, Axis.vertical]),
      );
      expect(original.key.currentContext, same(element));
      expect(layout.areas.first.flex, 2);
      expect(h.panes, hasLength(3));
      final down = h.panes.last;
      final downElement = down.key.currentContext;
      expect(
        await h.action(tester, DesktopAction.nextPane, down),
        DesktopActionOutcome.executed,
      );
      await tester.pumpAndSettle();
      expect(original.controller.focusNode.hasFocus, isTrue);
      final navigator = tester.state<NavigatorState>(find.byType(Navigator));
      tester.binding.platformDispatcher.accessibilityFeaturesTestValue =
          const FakeAccessibilityFeatures(
            highContrast: true,
            disableAnimations: true,
          );
      addTearDown(
        tester.binding.platformDispatcher.clearAccessibilityFeaturesTestValue,
      );
      tester.platformDispatcher.textScaleFactorTestValue = 1.5;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      await tester.pumpAndSettle();
      expect(
        tester.state<NavigatorState>(find.byType(Navigator)),
        same(navigator),
      );
      expect(original.key.currentContext, same(element));
      expect(down.key.currentContext, same(downElement));
      expect(layout.areas.first.flex, 2);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
    });
  }

  testWidgets('Rust 裁决后动作只作用当前终端，旧路径不抢跑新路径且无假终端成功', (tester) async {
    final h = _Harness();
    await h.mount(tester);
    final first = h.panes.single;
    final scope = h.scope(tester);
    scope.ownsTerminalInput(first.controller, true);
    await shortcut(tester, LogicalKeyboardKey.keyT);
    expect(h.options.queries, 0);
    expect(find.byType(DesktopConnectionEditor), findsNothing);
    await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
    await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyD);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
    await tester.pumpAndSettle();
    expect(h.panes, hasLength(1));
    expect(
      await scope.execute(DesktopAction.splitRight, origin: first.controller),
      DesktopActionOutcome.executed,
    );
    await tester.pumpAndSettle();
    final second = h.panes.last;
    expect(
      await scope.execute(DesktopAction.splitDown, origin: first.controller),
      DesktopActionOutcome.staleOrigin,
    );
    expect(
      await h.action(tester, DesktopAction.selectAll, second),
      DesktopActionOutcome.needsPaneApi,
    );
    expect(
      await h.action(tester, DesktopAction.paste, second),
      DesktopActionOutcome.needsPaneApi,
    );
    expect(
      await h.action(tester, DesktopAction.copy, second),
      DesktopActionOutcome.unavailable,
    );
    expect(
      await h.action(tester, DesktopAction.zoomIn, second),
      DesktopActionOutcome.unavailable,
    );
    await h.action(tester, DesktopAction.search, second);
    await tester.pumpAndSettle();
    expect(
      await scope.execute(DesktopAction.splitDown, origin: second.controller),
      DesktopActionOutcome.staleOrigin,
    );
    await tester.enterText(find.byType(f.TextBox), '草稿');
    await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
    await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyD);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
    expect(h.panes, hasLength(2));
    second.controller.focusNode.requestFocus();
    await tester.pump();
    final auth = f.showDialog<void>(
      context: tester.element(find.byType(WindowsShell)),
      builder: (_) => const f.ContentDialog(
        title: Text('认证'),
        content: f.TextBox(obscureText: true, autofocus: true),
      ),
    );
    await tester.pumpAndSettle();
    expect(
      await scope.execute(DesktopAction.splitDown, origin: second.controller),
      DesktopActionOutcome.staleOrigin,
    );
    tester.state<NavigatorState>(find.byType(Navigator)).pop();
    await auth;
    await tester.pumpAndSettle();
    scope.ownsTerminalInput(first.controller, false);
    first.controller.focusNode.requestFocus();
    await tester.pump();
    // 当前活动窗格仍是 second；先用 nextPane 返回 first 再验旧路径。
    second.controller.focusNode.requestFocus();
    await tester.pump();
    await scope.execute(DesktopAction.nextPane, origin: second.controller);
    await tester.pumpAndSettle();
    await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
    await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyD);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
    await tester.pumpAndSettle();
    expect(h.panes, hasLength(3));
    await tester.pumpWidget(const SizedBox());
  });

  testWidgets('快速连接选 profile、管理往返/标签切换不销毁 pane，搜索草稿保留', (tester) async {
    final h = _Harness();
    await h.mount(tester);
    final first = h.panes.single;
    final element = first.key.currentContext;
    await tester.enterText(find.byType(f.TextBox), '保留搜索');
    await tester.tap(iconButton('快速连接 · Ctrl+Shift+T'));
    await tester.pumpAndSettle();
    final editor = tester.widget<DesktopConnectionEditor>(
      find.byType(DesktopConnectionEditor),
    );
    final sync = editor.keySyncState;
    final picker = tester.widget<f.ComboBox<String>>(
      find.byKey(const ValueKey('workspace-profile')),
    );
    picker.onChanged!('b');
    await tester.pump();
    await tester.enterText(
      find.byKey(const ValueKey('catalog-field-主机')),
      'fixture.test',
    );
    await tester.enterText(
      find.byKey(const ValueKey('catalog-field-用户名')),
      'fixture',
    );
    await tester.enterText(
      find.byKey(const ValueKey('catalog-field-密码')),
      'fixture-only-secret',
    );
    final scope = h.scope(tester);
    expect(
      await scope.execute(DesktopAction.splitRight, origin: first.controller),
      DesktopActionOutcome.staleOrigin,
    );
    await tester.ensureVisible(find.widgetWithText(f.FilledButton, '连接'));
    await tester.tap(find.widgetWithText(f.FilledButton, '连接'));
    await tester.pumpAndSettle();
    expect(h.panes, hasLength(2));
    final quick = h.panes.last;
    expect(quick.controller.target.profileId, 'b');
    expect(quick.controller.target.password, 'fixture-only-secret');
    expect(first.key.currentContext, same(element));
    final quickElement = quick.key.currentContext;
    await tester.tap(iconButton('私钥管理'));
    await tester.pumpAndSettle();
    expect(
      tester.widget<DesktopKeysPage>(find.byType(DesktopKeysPage)).syncState,
      same(sync),
    );
    await tester.tap(find.text('返回工作区'));
    await tester.pumpAndSettle();
    expect(first.key.currentContext, same(element));
    expect(quick.key.currentContext, same(quickElement));
    expect(
      tester.widget<f.TextBox>(find.byType(f.TextBox)).controller!.text,
      '保留搜索',
    );
    await h.action(tester, DesktopAction.previousTab, quick);
    await tester.pumpAndSettle();
    expect(first.controller.focusNode.hasFocus, isTrue);
    expect(first.key.currentContext, same(element));
    await h.action(tester, DesktopAction.nextTab, first);
    await tester.pumpAndSettle();
    expect(quick.controller.focusNode.hasFocus, isTrue);
    expect(quick.key.currentContext, same(quickElement));
    await tester.pumpWidget(const SizedBox());
  });

  testWidgets('快速连接可选 System OpenSSH：不可用时禁用，选中后不要求密码', (tester) async {
    final h = _Harness();
    await h.mount(tester);
    final transport = find.byKey(const Key('catalog-transport'));
    await tester.tap(iconButton('快速连接 · Ctrl+Shift+T'));
    await tester.pumpAndSettle();
    // 快速连接只保留「连接方式」一个传输控件。
    expect(find.byKey(const ValueKey('workspace-transport')), findsNothing);
    var combo = tester.widget<f.ComboBox<DesktopTransport>>(transport);
    expect(combo.value, DesktopTransport.nativeSsh);
    expect(combo.items![1].enabled, isFalse);
    expect(find.text('System OpenSSH 不可用：System 固定不可用原因'), findsOneWidget);
    expect(find.byKey(const ValueKey('catalog-field-密码')), findsOneWidget);
    await tester.tap(find.text('取消'));
    await tester.pumpAndSettle();

    h.options.system = true;
    await tester.tap(iconButton('快速连接 · Ctrl+Shift+T'));
    await tester.pumpAndSettle();
    combo = tester.widget<f.ComboBox<DesktopTransport>>(transport);
    expect(combo.items![1].enabled, isTrue);
    tester
        .widget<f.ComboBox<String>>(
          find.byKey(const ValueKey('workspace-profile')),
        )
        .onChanged!('b');
    await tester.enterText(
      find.byKey(const ValueKey('catalog-field-密码')),
      'fixture-only-secret',
    );
    combo.onChanged!(DesktopTransport.systemOpenSsh);
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('catalog-field-密码')), findsNothing);
    expect(find.textContaining('由系统 ssh 处理认证'), findsOneWidget);
    await tester.enterText(
      find.byKey(const ValueKey('catalog-field-主机')),
      'fixture.test',
    );
    await tester.enterText(
      find.byKey(const ValueKey('catalog-field-用户名')),
      'fixture',
    );
    await tester.ensureVisible(find.widgetWithText(f.FilledButton, '连接'));
    await tester.tap(find.widgetWithText(f.FilledButton, '连接'));
    await tester.pumpAndSettle();
    final target = h.panes.last.controller.target;
    expect(target.isSystemSsh, isTrue);
    expect(target.connectionId, isEmpty);
    expect(
      [target.host, target.port, target.username],
      ['fixture.test', 22, 'fixture'],
    );
    expect(target.password, isEmpty, reason: '先输入的密码不随 System 目标发出');
    expect(target.profileId, 'b');
    await tester.pumpWidget(const SizedBox());
  });

  testWidgets('保存连接选 profile 可取消，默认保持空引用；不可用 Native 不打开表单', (tester) async {
    final h = _Harness();
    await h.mount(tester);
    var side = tester.widget<WindowsSidebar>(find.byType(WindowsSidebar));
    side.onAction(saved, 'profile');
    await tester.pumpAndSettle();
    expect(find.text('System OpenSSH：System 固定不可用原因'), findsOneWidget);
    expect(find.text('本地 shell：Local 固定不可用原因'), findsOneWidget);
    expect(find.widgetWithText(f.Button, 'System OpenSSH'), findsNothing);
    final transportMenu = tester.widget<f.DropDownButton>(
      find.byKey(const ValueKey('workspace-transport')),
    );
    expect(
      transportMenu.items.whereType<f.MenuFlyoutItem>().map(
        (item) => item.onPressed,
      ),
      everyElement(isNull),
    );
    expect(
      transportMenu.items.whereType<f.MenuFlyoutItem>().map(
        (item) => (item.text as Text).data,
      ),
      containsAll(['System OpenSSH：System 固定不可用原因', '本地 shell：Local 固定不可用原因']),
    );
    expect(
      tester
          .widget<f.ComboBox<String>>(
            find.byKey(const ValueKey('workspace-profile')),
          )
          .value,
      '',
    );
    await tester.tap(find.text('取消'));
    await tester.pumpAndSettle();
    expect(h.panes, hasLength(1));
    side.onAction(saved, 'profile');
    await tester.pumpAndSettle();
    tester
        .widget<f.ComboBox<String>>(
          find.byKey(const ValueKey('workspace-profile')),
        )
        .onChanged!('a');
    await tester.pump();
    await tester.tap(find.widgetWithText(f.FilledButton, '连接'));
    await tester.pumpAndSettle();
    expect(h.panes.last.controller.target.connectionId, saved.id);
    expect(h.panes.last.controller.target.profileId, 'a');
    h.options.available = false;
    await tester.tap(iconButton('快速连接 · Ctrl+Shift+T'));
    await tester.pumpAndSettle();
    expect(find.byType(DesktopConnectionEditor), findsNothing);
    expect(find.text('Native 当前不可用'), findsOneWidget);
    expect(h.panes, hasLength(2));
    await tester.pumpWidget(const SizedBox());
  });

  testWidgets('来源在异步配置查询期间失焦时不会打开目标面板', (tester) async {
    final h = _Harness();
    await h.mount(tester);
    h.options.pending = Completer<DesktopTargetOptions>();
    final pending = h
        .scope(tester)
        .execute(DesktopAction.newTab, origin: h.panes.first.controller);
    await tester.pump();
    tester
        .widget<WindowsSidebar>(find.byType(WindowsSidebar))
        .focusNode
        .requestFocus();
    await tester.pump();
    h.options.pending!.complete(h.options.value);
    await pending;
    await tester.pumpAndSettle();
    expect(find.byType(DesktopConnectionEditor), findsNothing);
    expect(h.panes, hasLength(1));
    await tester.pumpWidget(const SizedBox());
  });

  testWidgets('同一工作区跨窄宽/宽侧栏重排与快速表单保留树，延迟搜索不抢输入框焦点', (tester) async {
    final h = _Harness();
    await h.mount(tester);
    final pane = h.panes.single;
    final element = pane.key.currentContext;
    final navigator = tester.state<NavigatorState>(find.byType(Navigator));
    for (final width in [375.0, 768.0, 1280.0]) {
      tester.view.physicalSize = Size(width, 900);
      await tester.pumpAndSettle();
      expect(pane.key.currentContext, same(element));
      expect(
        tester.state<NavigatorState>(find.byType(Navigator)),
        same(navigator),
      );
      expect(h.panes, [pane]);
      if (width == 375) {
        await tester.tap(iconButton('快速连接 · Ctrl+Shift+T'));
        await tester.pumpAndSettle();
        expect(find.byType(DesktopProfilePicker), findsOneWidget);
        expect(find.byType(DesktopConnectionEditor), findsOneWidget);
        await tester.tap(find.text('返回工作区'));
        await tester.pumpAndSettle();
        expect(pane.key.currentContext, same(element));
      }
      expect(tester.takeException(), isNull);
    }
    final scope = h.scope(tester);
    pane.controller.focusNode.requestFocus();
    await tester.pump();
    await scope.execute(DesktopAction.search, origin: pane.controller);
    final other = FocusNode();
    final dialog = f.showDialog<void>(
      context: tester.element(find.byType(WindowsShell)),
      builder: (_) => f.ContentDialog(
        title: const Text('输入'),
        content: f.TextBox(focusNode: other, autofocus: true),
      ),
    );
    await tester.pumpAndSettle();
    expect(other.hasFocus, isTrue);
    expect(
      tester
          .widget<WindowsSidebar>(
            find.byType(WindowsSidebar, skipOffstage: false),
          )
          .focusNode
          .hasFocus,
      isFalse,
    );
    navigator.pop();
    await dialog;
    await tester.pumpAndSettle();
    await tester.pumpWidget(const SizedBox());
    other.dispose();
  });

  for (final platform in [TargetPlatform.windows, TargetPlatform.macOS]) {
    testWidgets('$platform 关闭 pane/tab/window 均确认，取消保留会话，最后窗格回到目录', (
      tester,
    ) async {
      debugDefaultTargetPlatformOverride = platform;
      final h = _Harness();
      await h.mount(tester);
      final pane = h.panes.single;
      final element = pane.key.currentContext;
      final scope = h.scope(tester);
      final closeWindow = WindowsDesktopWindow.confirmClose!();
      await tester.pumpAndSettle();
      expect(find.text('关闭 GuoSSHell？'), findsOneWidget);
      await tester.tap(find.text('取消'));
      await tester.pumpAndSettle();
      expect(await closeWindow, isFalse);
      expect(pane.key.currentContext, same(element));
      final closePane = h
          .scope(tester)
          .execute(DesktopAction.closePane, origin: pane.controller);
      await tester.pumpAndSettle();
      expect(find.text('关闭窗格？'), findsOneWidget);
      await tester.tap(find.text('取消'));
      await closePane;
      await tester.pumpAndSettle();
      expect(pane.key.currentContext, same(element));
      tester.widget<f.TabView>(find.byType(f.TabView)).tabs.single.onClosed!();
      await tester.pumpAndSettle();
      expect(find.text('关闭标签？'), findsOneWidget);
      await tester.tap(find.text('取消'));
      await tester.pumpAndSettle();
      expect(pane.key.currentContext, same(element));
      final finalClose = h
          .scope(tester)
          .execute(DesktopAction.closePane, origin: pane.controller);
      await tester.pumpAndSettle();
      await tester.tap(find.text('断开'));
      await finalClose;
      await tester.pumpAndSettle();
      expect(pane.key.currentContext, isNull);
      expect(find.text('连接，开始工作。'), findsOneWidget);
      expect(await WindowsDesktopWindow.confirmClose!(), isTrue);
      expect(
        await scope.execute(DesktopAction.splitRight, origin: pane.controller),
        DesktopActionOutcome.staleOrigin,
      );
      await tester.pumpWidget(const SizedBox());
      expect(WindowsDesktopWindow.confirmClose, isNull);
      debugDefaultTargetPlatformOverride = null;
    });
  }
}
