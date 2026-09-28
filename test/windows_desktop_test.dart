import 'package:fluent_ui/fluent_ui.dart' as f;
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/app.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/desktop/desktop_shortcuts.dart';
import 'package:guosh_shell/src/desktop/windows_chrome.dart';
import 'package:guosh_shell/src/desktop/windows_shell.dart';
import 'package:guosh_shell/src/desktop/windows_prompt.dart';
import 'package:guosh_shell/src/terminal/session_target.dart';
import 'package:guosh_shell/src/workspace/workspace.dart';

Widget host(Widget child) => MaterialApp(
  theme: desktopMaterialTheme(),
  localizationsDelegates: const [f.FluentLocalizations.delegate],
  builder: (_, navigator) => f.FluentTheme(
    data: f.FluentThemeData(brightness: Brightness.dark),
    child: navigator!,
  ),
  home: child,
);

void main() {
  tearDown(() {
    debugDefaultTargetPlatformOverride = null;
  });

  test('桌面快捷键保留远端控制键', () {
    for (final key in [
      LogicalKeyboardKey.keyC,
      LogicalKeyboardKey.keyD,
      LogicalKeyboardKey.keyW,
      LogicalKeyboardKey.keyT,
    ]) {
      expect(desktopCommand(key, control: true, shift: false), isNull);
    }
    expect(
      desktopCommand(LogicalKeyboardKey.keyD, control: true, shift: true),
      DesktopCommand.splitRight,
    );
    expect(
      desktopCommand(LogicalKeyboardKey.tab, control: true, shift: true),
      DesktopCommand.previousTab,
    );
    expect(
      desktopCommand(
        LogicalKeyboardKey.keyD,
        control: true,
        shift: true,
        alt: true,
      ),
      isNull,
    );
  });

  testWidgets('非 Windows 平台维持原入口且不调用窗口插件', (tester) async {
    late BuildContext context;
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (value) {
            context = value;
            return const SizedBox();
          },
        ),
      ),
    );
    for (final platform in [
      TargetPlatform.android,
      TargetPlatform.iOS,
      TargetPlatform.macOS,
      TargetPlatform.linux,
    ]) {
      debugDefaultTargetPlatformOverride = platform;
      await WindowsDesktopWindow.initialize();
      expect(WindowsDesktopWindow.initialized, isFalse);
      final app = const GuoSSHellApp().build(context) as MaterialApp;
      expect(app.builder, isNull);
      expect(app.localizationsDelegates, isNull);
      expect(app.theme!.visualDensity, ThemeData().visualDensity);
    }
    debugDefaultTargetPlatformOverride = TargetPlatform.windows;
    expect(
      (const GuoSSHellApp().build(context) as MaterialApp).builder,
      isNotNull,
    );
    debugDefaultTargetPlatformOverride = null;
  });

  testWidgets('标签重排保留活动会话与嵌套分屏', (tester) async {
    final workspace = Workspace();
    final a = workspace.openTab(
      const SessionTarget.saved(connectionId: 'a', title: 'a'),
    );
    final b = workspace.split(
      a,
      Axis.vertical,
      const SessionTarget.saved(connectionId: 'b', title: 'b'),
    );
    final first = workspace.activeTab;
    workspace.openTab(const SessionTarget.saved(connectionId: 'c', title: 'c'));
    workspace.selectTab(0);
    workspace.reorderTab(0, 2);
    expect(workspace.activeTab, same(first));
    expect(workspace.activePane, same(b));
    expect(workspace.activeIndex, 1);
    expect(workspace.activeTab!.panes, [a, b]);
    workspace.dispose();
    await tester.pump();
  });

  testWidgets('自绘标题栏可在应用导航器外显示并正常销毁', (tester) async {
    await tester.pumpWidget(
      MaterialApp(
        localizationsDelegates: const [f.FluentLocalizations.delegate],
        builder: (_, child) => f.FluentTheme(
          data: f.FluentThemeData(brightness: Brightness.dark),
          child: WindowsFrame(child: child!),
        ),
        home: const Scaffold(body: Text('工作区')),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('SSH 工作区'), findsOneWidget);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox());
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
  });

  for (final size in [const Size(760, 520), const Size(1360, 860)]) {
    testWidgets('Windows 空工作区在 $size 无溢出并保留搜索关联', (tester) async {
      tester.view.reset();
      tester.view.devicePixelRatio = 1;
      tester.view.physicalSize = size;
      addTearDown(tester.view.reset);
      void publish(String query) => assignRustSignal['CatalogState']!(
        CatalogState(query: query, connections: const []).bincodeSerialize(),
        Uint8List(0),
      );
      await tester.pumpWidget(host(WindowsShell(queryCatalog: publish)));
      await tester.pumpAndSettle();
      expect(find.text('连接，开始工作。'), findsOneWidget);
      expect(find.byType(VerticalDivider), findsNothing);
      expect(tester.takeException(), isNull);
      await tester.enterText(find.byType(f.TextBox), '不存在');
      await tester.pumpAndSettle();
      publish('');
      await tester.pumpAndSettle();
      expect(find.text('没有匹配的连接'), findsOneWidget);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
    });
  }

  testWidgets('Fluent 主机密钥变更保留二次确认门禁', (tester) async {
    const prompt = InteractionPrompt(
      sessionId: 1,
      promptId: 1,
      kind: PromptKind.hostKey,
      username: 'fixture',
      host: 'fixture.invalid',
      port: 22,
      address: 'loopback',
      algorithm: 'ssh-ed25519',
      fingerprint: 'SHA256:fixture',
      changed: true,
      name: '',
      instruction: '',
      fields: [],
      canRemember: false,
      retry: false,
      triesLeft: -1,
    );
    await tester.pumpWidget(host(const WindowsPrompt(prompt: prompt)));
    await tester.pumpAndSettle();
    final button = find.widgetWithText(f.FilledButton, '替换旧密钥并连接');
    expect(tester.widget<f.FilledButton>(button).onPressed, isNull);
    await tester.tap(find.text('我已通过其他途径核对新指纹'));
    await tester.pumpAndSettle();
    expect(tester.widget<f.FilledButton>(button).onPressed, isNotNull);
  });
}
