import 'dart:async';
import 'dart:io';

import 'package:fluent_ui/fluent_ui.dart' as fluent;
import 'package:flutter/material.dart' as material;
import 'package:flutter/foundation.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/app.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/catalog/connection_list_page.dart';
import 'package:guosh_shell/src/desktop/catalog/desktop_catalog_page.dart';
import 'package:guosh_shell/src/desktop/catalog/desktop_catalog_gateway.dart';
import 'package:guosh_shell/src/desktop/desktop_shell.dart';
import 'package:guosh_shell/src/desktop/desktop_status.dart';
import 'package:guosh_shell/src/desktop/desktop_theme.dart';
import 'package:guosh_shell/src/desktop/legacy_material_surface.dart';
import 'package:guosh_shell/src/desktop/windows_chrome.dart';
import 'package:guosh_shell/src/desktop/windows_shell.dart';
import 'package:guosh_shell/src/keys/keys_page.dart';
import 'package:guosh_shell/src/platform_target.dart';
import 'package:guosh_shell/src/settings/interface_font.dart';
import 'package:guosh_shell/src/settings/interface_font_setting.dart';
import 'package:guosh_shell/src/terminal/session_target.dart';
import 'package:guosh_shell/src/workspace/workspace.dart';

class _FakeStartup implements StartupPort {
  final ready = StreamController<(bool, String, SessionTarget?)>.broadcast(
    sync: true,
  );
  final settings = StreamController<void>.broadcast(sync: true);
  int starts = 0;
  int queries = 0;
  int lifecycle = 0;
  bool failDirectory = false;

  @override
  StreamSubscription<dynamic> listenReady(
    void Function(bool, String, SessionTarget?) onReady,
  ) => ready.stream.listen((event) => onReady(event.$1, event.$2, event.$3));

  @override
  StreamSubscription<dynamic> listenSettings(void Function() onSettings) =>
      settings.stream.listen((_) => onSettings());

  @override
  Future<String> supportDirectory() async {
    if (failDirectory) {
      throw const FileSystemException('拒绝访问', 'private-sentinel');
    }
    return 'fixture';
  }

  @override
  void start(String directory) {
    starts++;
  }

  @override
  void querySettings() {
    queries++;
  }

  @override
  void foreground(bool foreground) {
    lifecycle++;
  }

  Future<void> close() async {
    await ready.close();
    await settings.close();
  }
}

class _CatalogWithoutRust extends RustDesktopCatalogGateway {
  const _CatalogWithoutRust();

  @override
  void query(String text) {}

  @override
  void queryKeys() {}

  @override
  Stream<CatalogState> get catalogStates => const Stream.empty();
}

Future<void> _advanceRoute(WidgetTester tester) async {
  await tester.pump();
  await tester.pump(DesktopMotion.page + DesktopMotion.control);
}

/// 使用真实 Workspace 模型验证主题更新的挂载生命周期，不连接真实终端。
class _WorkspaceProbe extends material.StatefulWidget {
  const _WorkspaceProbe({super.key, required this.target});
  final SessionTarget target;

  @override
  material.State<_WorkspaceProbe> createState() => _WorkspaceProbeState();
}

class _WorkspaceProbeState extends material.State<_WorkspaceProbe> {
  final workspace = Workspace();

  @override
  void initState() {
    super.initState();
    workspace.openTab(widget.target);
  }

  @override
  void dispose() {
    workspace.dispose();
    super.dispose();
  }

  @override
  material.Widget build(material.BuildContext context) =>
      const material.Scaffold(body: material.Text('已挂载的工作区'));
}

void main() {
  for (final platform in [TargetPlatform.macOS, TargetPlatform.linux]) {
    for (final width in [375.0, 768.0, 1280.0]) {
      testWidgets('$platform / $width 概览的明确兼容设置入口直达双字体槽且不回退目录密钥', (
        tester,
      ) async {
        debugDefaultTargetPlatformOverride = platform;
        addTearDown(() => debugDefaultTargetPlatformOverride = null);
        tester.view.physicalSize = material.Size(width, 900);
        tester.view.devicePixelRatio = 1;
        addTearDown(() {
          tester.view.resetPhysicalSize();
          tester.view.resetDevicePixelRatio();
        });
        tester.binding.platformDispatcher.accessibilityFeaturesTestValue =
            const FakeAccessibilityFeatures(disableAnimations: true);
        addTearDown(
          tester.binding.platformDispatcher.clearAccessibilityFeaturesTestValue,
        );
        final fake = _FakeStartup();
        await tester.pumpWidget(GuoSSHellApp(startupOverride: fake));
        fake.ready.add((true, '', null));
        fake.settings.add(null);
        await tester.pumpAndSettle();
        final outer = tester.state<material.NavigatorState>(
          find.byType(material.Navigator).first,
        );
        await tester.tap(find.text('打开字体设置（兼容）'));
        await tester.pump();
        expect(outer.canPop(), isTrue);
        expect(find.byType(LegacyMaterialSurface), findsOneWidget);
        expect(find.text('字体设置 · Material 兼容界面（尚未迁移）'), findsOneWidget);
        expect(find.byType(InterfaceFontSetting), findsNWidgets(2));
        expect(
          find.byKey(const material.ValueKey('pick-interface-font')),
          findsOneWidget,
        );
        expect(
          find.byKey(const material.ValueKey('interface-font-preview')),
          findsOneWidget,
        );
        expect(
          find.byKey(const material.ValueKey('pick-terminal-font')),
          findsOneWidget,
        );
        expect(
          find.byKey(const material.ValueKey('terminal-font-preview')),
          findsOneWidget,
        );
        for (final label in ['恢复内置 MiSans', '恢复内置等宽字体']) {
          await tester.ensureVisible(find.text(label));
          await tester.pump();
          expect(
            tester
                .widget<material.TextButton>(
                  find.widgetWithText(material.TextButton, label),
                )
                .onPressed,
            isNotNull,
          );
        }
        expect(
          find.byType(ConnectionListPage, skipOffstage: false),
          findsNothing,
        );
        expect(find.byType(KeysPage, skipOffstage: false), findsNothing);
        expect(tester.takeException(), isNull);
        expect(find.textContaining('兼容工作区：'), findsNothing);
        final inner = tester
            .widget<material.MaterialApp>(find.byType(material.MaterialApp))
            .navigatorKey!
            .currentState!;
        expect(inner.canPop(), isFalse, reason: '设置为独立根，不先进入工作区或旧目录');
        await tester.tap(find.text('返回 Fluent 首页'));
        await tester.pumpAndSettle();
        expect(find.byType(DesktopShell), findsOneWidget);
        expect(find.byType(LegacyMaterialSurface), findsNothing);
        expect(
          tester.state<material.NavigatorState>(
            find.byType(material.Navigator).first,
          ),
          same(outer),
        );
        expect(fake.starts, 1);
        await tester.pumpWidget(const material.SizedBox());
        await fake.close();
        debugDefaultTargetPlatformOverride = null;
      });
    }

    testWidgets('$platform 界面字体进入实际 Fluent 与兼容主题并保留工作区导航', (tester) async {
      debugDefaultTargetPlatformOverride = platform;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      final fake = _FakeStartup();
      var requestId = 0;
      final font = InterfaceTypography(
        transport: (slot, action, name, bytes) async => FontReply(
          FontFileState(
            requestId: ++requestId,
            slot: slot,
            family: action == 'apply' ? 'GuoshFont_fixture' : 'MiSans',
            label: '测试字体',
            error: '',
            applied: true,
          ),
          bytes,
        ),
        register: (_, _) async {},
      );
      await tester.pumpWidget(
        InterfaceFontScope(
          controller: font,
          child: GuoSSHellApp(startupOverride: fake),
        ),
      );
      fake.ready.add((true, '', null));
      fake.settings.add(null);
      await tester.pumpAndSettle();
      final shell = find.byType(DesktopShell);
      final fluentFamily = fluent.FluentTheme.of(tester.element(shell))
          .typography
          .body!
          .fontFamily;
      final outer = tester.state<material.NavigatorState>(
        find.byType(material.Navigator).first,
      );
      final workspaceKey = material.GlobalKey<_WorkspaceProbeState>();
      outer.push(
        fluent.FluentPageRoute<void>(
          builder: (_) => LegacyMaterialSurface(
            initialTarget: const SessionTarget.saved(
              connectionId: 'fixture',
              title: '测试连接',
            ),
            workspaceBuilder: (target) =>
                _WorkspaceProbe(key: workspaceKey, target: target),
          ),
        ),
      );
      await tester.pumpAndSettle();
      final inner = tester
          .widget<material.MaterialApp>(find.byType(material.MaterialApp))
          .navigatorKey!
          .currentState!;
      final workspace = workspaceKey.currentState;
      expect(workspace, isNotNull);
      final model = workspace!.workspace;
      final pane = model.activePane;
      expect(
        [
          fluentFamily,
          material.Theme.of(tester.element(find.text('已挂载的工作区')))
              .textTheme
              .bodyMedium!
              .fontFamily,
        ],
        ['MiSans', 'MiSans'],
        reason: 'Fluent 与兼容 Material 都必须消费界面字体槽',
      );
      for (final action in ['apply', 'reset']) {
        if (action == 'apply') {
          await font.apply(
            FontDraft('fixture.ttf', 'GuoshFont_fixture', '测试字体', Uint8List(0)),
          );
        } else {
          await font.reset();
        }
        await tester.pumpAndSettle();
        final expected = action == 'apply' ? 'GuoshFont_fixture' : 'MiSans';
        expect(
          fluent.FluentTheme.of(
            tester.element(find.byType(LegacyMaterialSurface)),
          ).typography.body!.fontFamily,
          expected,
        );
        expect(
          material.Theme.of(tester.element(find.text('已挂载的工作区')))
              .textTheme
              .bodyMedium!
              .fontFamily,
          expected,
        );
        expect(
          tester.state<material.NavigatorState>(
            find.byType(material.Navigator).first,
          ),
          same(outer),
        );
        expect(
          tester
              .widget<material.MaterialApp>(find.byType(material.MaterialApp))
              .navigatorKey!
              .currentState,
          same(inner),
        );
        expect(workspaceKey.currentState, same(workspace));
        expect(workspaceKey.currentState!.workspace, same(model));
        expect(model.activePane, same(pane));
        expect(model.panes, hasLength(1));
        expect(inner.canPop(), isTrue);
        expect(fake.starts, 1);
        expect(fake.queries, 1);
      }
      await tester.pumpWidget(const material.SizedBox());
      font.dispose();
      await fake.close();
      debugDefaultTargetPlatformOverride = null;
    });
  }

  testWidgets('Windows 仍由 Material 导航经 WindowsFrame 到 WindowsShell', (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.windows;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final fake = _FakeStartup();
    await tester.pumpWidget(
      GuoSSHellApp(
        startupOverride: fake,
        desktopCatalogGatewayOverride: const _CatalogWithoutRust(),
      ),
    );
    expect(find.byType(material.MaterialApp), findsOneWidget);
    expect(find.byType(WindowsFrame), findsOneWidget);
    fake.ready.add((true, '', null));
    fake.settings.add(null);
    // 没有目录回包时 Windows 侧加载指示器持续运转。
    await tester.pump();
    await tester.pump(DesktopMotion.page);
    expect(find.byType(WindowsShell), findsOneWidget);
    expect(
      find.ancestor(
        of: find.byType(WindowsShell),
        matching: find.byType(WindowsFrame),
      ),
      findsOneWidget,
    );
    expect(find.byType(DesktopShell), findsNothing);
    expect(find.byType(fluent.FluentApp), findsNothing);
    await tester.pumpWidget(const material.SizedBox());
    await fake.close();
    debugDefaultTargetPlatformOverride = null;
  });

  testWidgets('桌面等待 Rust 设置后切换 Fluent 工作台并释放订阅', (tester) async {
    final fake = _FakeStartup();
    await tester.pumpWidget(
      GuoSSHellApp(
        targetOverride: AppTarget.desktop,
        startupOverride: fake,
        legacyHomeOverride: ConnectionListPage(queryCatalog: (_) {}),
      ),
    );
    await tester.pump();
    expect(fake.starts, 1);
    expect(find.byType(fluent.FluentApp), findsOneWidget);
    expect(find.byType(DesktopStatus), findsOneWidget);
    fake.ready.add((true, '', null));
    await tester.pump();
    expect(fake.queries, 1);
    expect(find.byType(DesktopShell), findsNothing);
    fake.settings.add(null);
    await tester.pumpAndSettle();
    expect(find.byType(DesktopShell), findsOneWidget);
    await tester.pumpWidget(const material.SizedBox());
    fake.settings.add(null);
    expect(fake.ready.hasListener, isFalse);
    expect(fake.settings.hasListener, isFalse);
    await fake.close();
  });

  testWidgets('桌面启动失败明确告知且不进入工作台', (tester) async {
    final fake = _FakeStartup();
    await tester.pumpWidget(
      GuoSSHellApp(targetOverride: AppTarget.desktop, startupOverride: fake),
    );
    fake.ready.add((false, 'private-sentinel', null));
    await tester.pump();
    expect(find.text('无法打开数据存储'), findsOneWidget);
    expect(find.textContaining('private-sentinel'), findsNothing);
    expect(find.byType(DesktopShell), findsNothing);
    expect(fake.queries, 0);
    await tester.pumpWidget(const material.SizedBox());
    await fake.close();
  });

  testWidgets('桌面本地存储异常不在 Fluent 页面显示系统位置', (tester) async {
    final fake = _FakeStartup()..failDirectory = true;
    await tester.pumpWidget(
      GuoSSHellApp(targetOverride: AppTarget.desktop, startupOverride: fake),
    );
    await tester.pump();
    expect(find.text('无法打开数据存储'), findsOneWidget);
    expect(find.textContaining('private-sentinel'), findsNothing);
    await tester.pumpWidget(const material.SizedBox());
    await fake.close();
  });

  testWidgets('生产桌面概览只进入 Fluent 目录，不打开无目标 Material 目录', (tester) async {
    final fake = _FakeStartup();
    await tester.pumpWidget(
      GuoSSHellApp(
        targetOverride: AppTarget.desktop,
        startupOverride: fake,
        legacyHomeOverride: ConnectionListPage(queryCatalog: (_) {}),
        desktopCatalogGatewayOverride: const _CatalogWithoutRust(),
      ),
    );
    await tester.pump();
    fake.ready.add((true, '', null));
    fake.settings.add(null);
    await tester.pumpAndSettle();
    expect(
      tester.widget<DesktopShell>(find.byType(DesktopShell)).openTarget,
      isNotNull,
    );
    await tester.tap(find.text('打开连接目录'));
    await tester.pump();
    expect(find.byType(DesktopCatalogPage), findsOneWidget);
    expect(find.byType(LegacyMaterialSurface), findsNothing);
    expect(find.byType(ConnectionListPage), findsNothing);
    final pane = tester
        .widget<fluent.NavigationView>(find.byType(fluent.NavigationView))
        .pane!;
    pane.onChanged!(0);
    await tester.pumpAndSettle();
    await tester.tap(find.text('打开连接目录'));
    await tester.pump(const Duration(milliseconds: 250));
    expect(find.byType(DesktopCatalogPage), findsOneWidget);
    expect(find.byType(material.MaterialApp), findsNothing);
    await tester.pumpWidget(const material.SizedBox());
    await fake.close();
  });

  testWidgets('homeOverride 测试兼容路由仍由内层处理受保护关闭', (tester) async {
    expect(() => LegacyMaterialSurface(), throwsAssertionError);
    await tester.pumpWidget(
      fluent.FluentApp(
        theme: desktopTheme(highContrast: false, reduceMotion: true),
        home: fluent.Builder(
          builder: (context) => fluent.Button(
            onPressed: () => fluent.Navigator.of(context).push(
              fluent.FluentPageRoute<void>(
                builder: (_) => LegacyMaterialSurface(
                  homeOverride: ConnectionListPage(queryCatalog: (_) {}),
                ),
              ),
            ),
            child: const fluent.Text('打开测试兼容路由'),
          ),
        ),
      ),
    );
    await tester.tap(find.text('打开测试兼容路由'));
    await _advanceRoute(tester);
    final inner = tester
        .widget<material.MaterialApp>(find.byType(material.MaterialApp))
        .navigatorKey!
        .currentState!;
    final outer = material.Navigator.of(
      tester.element(find.byType(LegacyMaterialSurface)),
    );
    var confirmations = 0;
    inner.push(
      material.MaterialPageRoute<void>(
        builder: (_) => material.PopScope(
          canPop: false,
          onPopInvokedWithResult: (didPop, _) {
            if (!didPop) confirmations++;
          },
          child: const material.Scaffold(
            body: material.Center(child: material.Text('受保护的会话')),
          ),
        ),
      ),
    );
    await _advanceRoute(tester);
    await outer.maybePop();
    await tester.pump();
    expect(confirmations, 1);
    expect(find.text('受保护的会话'), findsOneWidget);
    expect(find.byType(LegacyMaterialSurface), findsOneWidget);
    await tester.tap(find.text('返回原界面目录'));
    await tester.pump(const Duration(milliseconds: 250));
    expect(confirmations, 2);
    inner.pop(); // 模拟用户在原有确认中允许退出。
    await _advanceRoute(tester);
    expect(find.text('返回 Fluent 首页'), findsOneWidget);
    await tester.tap(find.text('返回 Fluent 首页'));
    await _advanceRoute(tester);
    expect(find.text('打开测试兼容路由'), findsOneWidget);
    await tester.pumpWidget(const material.SizedBox());
  });

  testWidgets('系统要求减少动画时概览进入 Fluent 目录且不启动兼容路由', (tester) async {
    tester.binding.platformDispatcher.accessibilityFeaturesTestValue =
        const FakeAccessibilityFeatures(disableAnimations: true);
    addTearDown(
      tester.binding.platformDispatcher.clearAccessibilityFeaturesTestValue,
    );
    final fake = _FakeStartup();
    await tester.pumpWidget(
      GuoSSHellApp(
        targetOverride: AppTarget.desktop,
        startupOverride: fake,
        legacyHomeOverride: ConnectionListPage(queryCatalog: (_) {}),
        desktopCatalogGatewayOverride: const _CatalogWithoutRust(),
      ),
    );
    await tester.pump();
    fake.ready.add((true, '', null));
    fake.settings.add(null);
    await tester.pumpAndSettle();
    await tester.tap(find.text('打开连接目录'));
    await tester.pumpAndSettle();
    expect(find.byType(DesktopCatalogPage), findsOneWidget);
    expect(find.byType(LegacyMaterialSurface), findsNothing);
    await tester.pumpWidget(const material.SizedBox());
    await fake.close();
  });

  testWidgets('移动根保持 MaterialApp 的等待与错误状态', (tester) async {
    final fake = _FakeStartup();
    await tester.pumpWidget(
      GuoSSHellApp(targetOverride: AppTarget.mobile, startupOverride: fake),
    );
    expect(find.byType(material.MaterialApp), findsOneWidget);
    expect(find.byType(fluent.FluentApp), findsNothing);
    expect(find.byType(material.CircularProgressIndicator), findsOneWidget);
    fake.ready.add((false, '测试错误', null));
    await tester.pump();
    expect(find.text('无法打开数据存储：测试错误'), findsOneWidget);
    await tester.pumpWidget(const material.SizedBox());
    await fake.close();
  });

  testWidgets('移动端本地存储异常仍沿用原错误文案', (tester) async {
    final fake = _FakeStartup()..failDirectory = true;
    await tester.pumpWidget(
      GuoSSHellApp(targetOverride: AppTarget.mobile, startupOverride: fake),
    );
    await tester.pump();
    expect(find.textContaining('无法打开数据存储：FileSystemException'), findsOneWidget);
    expect(find.textContaining('private-sentinel'), findsOneWidget);
    await tester.pumpWidget(const material.SizedBox());
    await fake.close();
  });
}
