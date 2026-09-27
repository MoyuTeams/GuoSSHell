import 'dart:async';
import 'dart:io';

import 'package:fluent_ui/fluent_ui.dart' as fluent;
import 'package:flutter/material.dart' as material;
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/app.dart';
import 'package:guosh_shell/src/catalog/connection_list_page.dart';
import 'package:guosh_shell/src/desktop/desktop_shell.dart';
import 'package:guosh_shell/src/desktop/desktop_status.dart';
import 'package:guosh_shell/src/desktop/desktop_theme.dart';
import 'package:guosh_shell/src/desktop/legacy_material_surface.dart';
import 'package:guosh_shell/src/platform_target.dart';
import 'package:guosh_shell/src/terminal/session_target.dart';

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

Future<void> _advanceRoute(WidgetTester tester) async {
  await tester.pump();
  await tester.pump(DesktopMotion.page + DesktopMotion.control);
}

void main() {
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

  testWidgets('从桌面根进入原界面并返回，仍可再次进入', (tester) async {
    final fake = _FakeStartup();
    await tester.pumpWidget(
      GuoSSHellApp(
        targetOverride: AppTarget.desktop,
        startupOverride: fake,
        legacyHomeOverride: ConnectionListPage(queryCatalog: (_) {}),
      ),
    );
    await tester.pump();
    fake.ready.add((true, '', null));
    fake.settings.add(null);
    await tester.pumpAndSettle();
    await tester.tap(find.text('打开原界面（尚未迁移）'));
    await _advanceRoute(tester);
    expect(find.byType(LegacyMaterialSurface), findsOneWidget);
    expect(find.text('返回 Fluent 首页'), findsOneWidget);
    final banner = tester.widget<material.Text>(
      find.text('原界面 · 桌面 Fluent 页面尚未迁移'),
    );
    expect(banner.style?.color, DesktopColors.text);
    final back = tester.widget<material.TextButton>(
      find.ancestor(
        of: find.text('返回 Fluent 首页'),
        matching: find.byType(material.TextButton),
      ),
    );
    expect(back.style?.foregroundColor?.resolve({}), DesktopColors.text);
    await tester.tap(find.text('返回 Fluent 首页'));
    await _advanceRoute(tester);
    expect(find.byType(DesktopShell), findsOneWidget);
    expect(find.byType(LegacyMaterialSurface), findsNothing);
    await tester.tap(find.text('打开原界面（尚未迁移）'));
    await _advanceRoute(tester);
    expect(find.byType(LegacyMaterialSurface), findsOneWidget);
    await tester.pumpWidget(const material.SizedBox());
    await fake.close();
  });

  testWidgets('外层返回由内层处理，拒绝关闭时兼容页与会话路由都保留', (tester) async {
    final fake = _FakeStartup();
    await tester.pumpWidget(
      GuoSSHellApp(
        targetOverride: AppTarget.desktop,
        startupOverride: fake,
        legacyHomeOverride: ConnectionListPage(queryCatalog: (_) {}),
      ),
    );
    await tester.pump();
    fake.ready.add((true, '', null));
    fake.settings.add(null);
    await tester.pumpAndSettle();
    await tester.tap(find.text('打开原界面（尚未迁移）'));
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
    expect(find.byType(DesktopShell), findsOneWidget);
    await tester.pumpWidget(const material.SizedBox());
    await fake.close();
  });

  testWidgets('系统要求减少动画时兼容路由的进入与退出时长为零', (tester) async {
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
      ),
    );
    await tester.pump();
    fake.ready.add((true, '', null));
    fake.settings.add(null);
    await tester.pumpAndSettle();
    await tester.tap(find.text('打开原界面（尚未迁移）'));
    await _advanceRoute(tester);
    final route = material.ModalRoute.of(
      tester.element(find.byType(LegacyMaterialSurface)),
    )!;
    expect(route.transitionDuration, Duration.zero);
    expect(route.reverseTransitionDuration, Duration.zero);
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
