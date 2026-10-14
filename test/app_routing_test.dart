import 'dart:async';
import 'dart:io';

import 'package:fluent_ui/fluent_ui.dart' as f;
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/app.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/catalog/connection_list_page.dart';
import 'package:guosh_shell/src/desktop/catalog/desktop_catalog_gateway.dart';
import 'package:guosh_shell/src/desktop/desktop_shell.dart';
import 'package:guosh_shell/src/desktop/desktop_status.dart';
import 'package:guosh_shell/src/desktop/desktop_theme.dart';
import 'package:guosh_shell/src/desktop/desktop_widgets.dart';
import 'package:guosh_shell/src/desktop/legacy_material_surface.dart';
import 'package:guosh_shell/src/desktop/settings/desktop_settings_page.dart';
import 'package:guosh_shell/src/desktop/windows_chrome.dart';
import 'package:guosh_shell/src/desktop/windows_shell.dart';
import 'package:guosh_shell/src/desktop/windows_sidebar.dart';
import 'package:guosh_shell/src/keys/keys_page.dart';
import 'package:guosh_shell/src/platform_target.dart';
import 'package:guosh_shell/src/settings/interface_font.dart';
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
  @override
  Stream<KeyListState> get keyStates => const Stream.empty();
}

Widget probe(WorkspacePane pane) => Focus(
  focusNode: pane.controller.focusNode,
  child: Text('窗格：${pane.controller.target.title}'),
);

void main() {
  for (final platform in [
    TargetPlatform.windows,
    TargetPlatform.macOS,
    TargetPlatform.linux,
  ]) {
    for (final width in [375.0, 768.0, 1280.0]) {
      testWidgets('$platform / $width 生产路由为常驻 Fluent 工作区，字体与导航不重建', (
        tester,
      ) async {
        debugDefaultTargetPlatformOverride = platform;
        addTearDown(() => debugDefaultTargetPlatformOverride = null);
        tester.view.physicalSize = Size(width, 900);
        tester.view.devicePixelRatio = 1;
        addTearDown(tester.view.reset);
        tester.binding.platformDispatcher.accessibilityFeaturesTestValue =
            const FakeAccessibilityFeatures(
              disableAnimations: true,
              highContrast: true,
            );
        addTearDown(
          tester.binding.platformDispatcher.clearAccessibilityFeaturesTestValue,
        );
        final hostCalls = <MethodCall>[];
        for (final channel in ['window_manager', 'guosshell/window_material']) {
          tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
            MethodChannel(channel),
            (call) async {
              hostCalls.add(call);
              return null;
            },
          );
          addTearDown(
            () => tester.binding.defaultBinaryMessenger
                .setMockMethodCallHandler(MethodChannel(channel), null),
          );
        }
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
        final panes = <WorkspacePane>[];
        await tester.pumpWidget(
          InterfaceFontScope(
            controller: font,
            child: GuoSSHellApp(
              startupOverride: fake,
              desktopCatalogGatewayOverride: const _CatalogWithoutRust(),
              desktopPaneBuilderOverride: (pane) {
                if (!panes.contains(pane)) panes.add(pane);
                return probe(pane);
              },
            ),
          ),
        );
        expect(find.byType(DesktopStatus), findsOneWidget);
        fake.ready.add((
          true,
          '',
          const SessionTarget.saved(connectionId: 'fixture', title: '测试连接'),
        ));
        await tester.pump();
        expect(find.byType(WindowsShell), findsNothing);
        fake.settings.add(null);
        await tester.pumpAndSettle();
        expect(find.byType(WindowsShell), findsOneWidget);
        expect(find.byType(DesktopShell), findsNothing);
        expect(find.byType(LegacyMaterialSurface), findsNothing);
        expect(
          find.byType(ConnectionListPage, skipOffstage: false),
          findsNothing,
        );
        expect(find.byType(KeysPage, skipOffstage: false), findsNothing);
        expect(
          find.byType(WindowsFrame),
          platform == TargetPlatform.windows ? findsOneWidget : findsNothing,
        );
        expect(find.byType(WindowsSidebar), findsOneWidget);
        final shell = tester.state(find.byType(WindowsShell));
        final navigator = tester.state<NavigatorState>(
          find.byType(Navigator).first,
        );
        final pane = panes.single;
        final element = pane.key.currentContext;
        for (final action in ['apply', 'reset']) {
          if (action == 'apply') {
            await font.apply(
              FontDraft(
                'fixture.ttf',
                'GuoshFont_fixture',
                '测试字体',
                Uint8List(0),
              ),
            );
          } else {
            await font.reset();
          }
          await tester.pumpAndSettle();
          final expected = action == 'apply' ? 'GuoshFont_fixture' : 'MiSans';
          expect(
            f.FluentTheme.of(tester.element(find.byType(WindowsShell)))
                .typography
                .body!
                .fontFamily,
            expected,
          );
          expect(
            Theme.of(tester.element(find.text('窗格：测试连接')))
                .textTheme
                .bodyMedium!
                .fontFamily,
            expected,
          );
          expect(tester.state(find.byType(WindowsShell)), same(shell));
          expect(
            tester.state<NavigatorState>(find.byType(Navigator).first),
            same(navigator),
          );
          expect(pane.key.currentContext, same(element));
          expect(panes, [pane]);
        }
        if (platform != TargetPlatform.windows) {
          await tester.tap(
            find.byWidgetPredicate(
              (widget) =>
                  widget is DesktopIconButton && widget.label == '设置 · Ctrl+,',
            ),
          );
          await tester.pumpAndSettle();
          expect(navigator.canPop(), isTrue);
          expect(find.byType(DesktopSettingsPage), findsOneWidget);
          await tester.tap(
            find.byKey(const ValueKey('settings-nav-appearance')),
          );
          await tester.pumpAndSettle();
          for (final slot in ['interface', 'terminal']) {
            expect(find.byKey(ValueKey('pick-$slot-resource')), findsOneWidget);
            expect(
              find.byKey(ValueKey('$slot-resource-preview')),
              findsOneWidget,
            );
            final reset = find.byKey(ValueKey('reset-$slot-resource'));
            await tester.ensureVisible(reset);
            expect(tester.widget<f.Button>(reset).onPressed, isNotNull);
          }
          expect(find.text('亚克力半透明'), findsNothing);
          await tester.tap(find.byKey(const ValueKey('close-settings')));
          await tester.pumpAndSettle();
          expect(find.byType(DesktopSettingsPage), findsNothing);
          expect(pane.key.currentContext, same(element));
          expect(hostCalls, isEmpty);
          await WindowsDesktopWindow.initialize();
          expect(hostCalls, isEmpty);
        }
        expect(fake.starts, 1);
        expect(fake.queries, 1);
        for (final state in [
          AppLifecycleState.inactive,
          AppLifecycleState.hidden,
          AppLifecycleState.paused,
          AppLifecycleState.hidden,
          AppLifecycleState.inactive,
          AppLifecycleState.resumed,
        ]) {
          tester.binding.handleAppLifecycleStateChanged(state);
        }
        expect(fake.lifecycle, greaterThan(0));
        expect(tester.takeException(), isNull);
        await tester.pumpWidget(const SizedBox());
        expect(fake.ready.hasListener, isFalse);
        expect(fake.settings.hasListener, isFalse);
        font.dispose();
        await fake.close();
        debugDefaultTargetPlatformOverride = null;
      });
    }
  }

  for (final localError in [false, true]) {
    testWidgets('桌面启动失败/本地异常脱敏且不进入工作区：$localError', (tester) async {
      final fake = _FakeStartup()..failDirectory = localError;
      await tester.pumpWidget(
        GuoSSHellApp(targetOverride: AppTarget.desktop, startupOverride: fake),
      );
      if (!localError) fake.ready.add((false, 'private-sentinel', null));
      await tester.pump();
      expect(find.text('无法打开数据存储'), findsOneWidget);
      expect(find.textContaining('private-sentinel'), findsNothing);
      expect(find.byType(WindowsShell), findsNothing);
      expect(fake.queries, 0);
      await tester.pumpWidget(const SizedBox());
      await fake.close();
    });
  }

  for (final platform in [TargetPlatform.android, TargetPlatform.iOS]) {
    testWidgets('$platform 移动旧目录、等待/错误文案与生命周期保持', (tester) async {
      debugDefaultTargetPlatformOverride = platform;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      final fake = _FakeStartup();
      await tester.pumpWidget(GuoSSHellApp(startupOverride: fake));
      expect(find.byType(MaterialApp), findsOneWidget);
      expect(find.byType(f.FluentApp), findsNothing);
      expect(find.byType(CircularProgressIndicator), findsOneWidget);
      fake.ready.add((false, '测试错误', null));
      await tester.pump();
      expect(find.text('无法打开数据存储：测试错误'), findsOneWidget);
      await tester.pumpWidget(const SizedBox());
      await fake.close();
      final success = _FakeStartup();
      await tester.pumpWidget(
        GuoSSHellApp(
          startupOverride: success,
          legacyHomeOverride: ConnectionListPage(queryCatalog: (_) {}),
        ),
      );
      success.ready.add((true, '', null));
      success.settings.add(null);
      await tester.pump();
      expect(find.byType(ConnectionListPage), findsOneWidget);
      expect(find.byType(WindowsShell), findsNothing);
      await tester.pumpWidget(const SizedBox());
      await success.close();
      debugDefaultTargetPlatformOverride = null;
    });
  }
  testWidgets('移动端本地存储异常沿用原错误文案', (tester) async {
    final fake = _FakeStartup()..failDirectory = true;
    await tester.pumpWidget(
      GuoSSHellApp(targetOverride: AppTarget.mobile, startupOverride: fake),
    );
    await tester.pump();
    expect(find.textContaining('无法打开数据存储：FileSystemException'), findsOneWidget);
    expect(find.textContaining('private-sentinel'), findsOneWidget);
    await tester.pumpWidget(const SizedBox());
    await fake.close();
  });

  // 兼容组件虽不再是生产入口，保留此前的内层关闭保护断言。
  testWidgets('homeOverride 测试兼容路由仍由内层处理受保护关闭', (tester) async {
    expect(() => LegacyMaterialSurface(), throwsAssertionError);
    await tester.pumpWidget(
      f.FluentApp(
        theme: desktopTheme(highContrast: false, reduceMotion: true),
        home: Builder(
          builder: (context) => f.Button(
            onPressed: () => Navigator.of(context).push(
              f.FluentPageRoute<void>(
                builder: (_) => LegacyMaterialSurface(
                  homeOverride: ConnectionListPage(queryCatalog: (_) {}),
                ),
              ),
            ),
            child: const Text('打开测试兼容路由'),
          ),
        ),
      ),
    );
    await tester.tap(find.text('打开测试兼容路由'));
    await tester.pump();
    assignRustSignal['CatalogState']!(
      const CatalogState(query: '', connections: []).bincodeSerialize(),
      Uint8List(0),
    );
    await tester.pumpAndSettle();
    final inner = tester
        .widget<MaterialApp>(find.byType(MaterialApp))
        .navigatorKey!
        .currentState!;
    final outer = Navigator.of(
      tester.element(find.byType(LegacyMaterialSurface)),
    );
    var confirmations = 0;
    inner.push(
      MaterialPageRoute<void>(
        builder: (_) => PopScope(
          canPop: false,
          onPopInvokedWithResult: (didPop, _) {
            if (!didPop) confirmations++;
          },
          child: const Scaffold(body: Text('受保护的会话')),
        ),
      ),
    );
    await tester.pumpAndSettle();
    await outer.maybePop();
    await tester.pump();
    expect(confirmations, 1);
    expect(find.text('受保护的会话'), findsOneWidget);
    expect(find.byType(LegacyMaterialSurface), findsOneWidget);
    await tester.tap(find.text('返回原界面目录'));
    await tester.pumpAndSettle();
    expect(confirmations, 2);
    inner.pop();
    await tester.pumpAndSettle();
    await tester.tap(find.text('返回 Fluent 首页'));
    await tester.pumpAndSettle();
    expect(find.text('打开测试兼容路由'), findsOneWidget);
    await tester.pumpWidget(const SizedBox());
  });
}
