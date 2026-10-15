import 'package:fluent_ui/fluent_ui.dart' as f;
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart' show RenderRepaintBoundary;
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/app.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/desktop/desktop_app_color_scheme.dart';
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

  testWidgets('非 Windows 桌面使用 Fluent，移动保持原入口且均不调用窗口插件', (tester) async {
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
      final app = const GuoSSHellApp().build(context);
      if (platform == TargetPlatform.android ||
          platform == TargetPlatform.iOS) {
        expect(app, isA<MaterialApp>());
        final mobile = app as MaterialApp;
        expect(mobile.builder, isNull);
        expect(mobile.localizationsDelegates, isNull);
        expect(mobile.theme!.visualDensity, ThemeData().visualDensity);
      } else {
        expect(app, isA<f.FluentApp>());
        expect((app as f.FluentApp).builder, isNotNull);
      }
    }
    debugDefaultTargetPlatformOverride = TargetPlatform.windows;
    expect(
      (const GuoSSHellApp().build(context) as MaterialApp).builder,
      isNotNull,
    );
    debugDefaultTargetPlatformOverride = null;
  });

  testWidgets('Windows 入口的菜单与下拉弹层在半透明窗口上保持不透明', (tester) async {
    late BuildContext context;
    await tester.pumpWidget(
      Builder(
        builder: (value) {
          context = value;
          return const SizedBox();
        },
      ),
    );
    debugDefaultTargetPlatformOverride = TargetPlatform.windows;
    final builder =
        (const GuoSSHellApp().build(context) as MaterialApp).builder!;
    // 模拟亚克力开启、默认透明度：窗口画布本身半透明。
    final appearance = WindowsAppearance.instance;
    appearance.materialAvailable = true;
    addTearDown(() => appearance.materialAvailable = false);
    final flyout = f.FlyoutController();
    addTearDown(flyout.dispose);
    final boundary = GlobalKey();
    await tester.pumpWidget(
      RepaintBoundary(
        key: boundary,
        child: MaterialApp(
          localizationsDelegates: const [f.FluentLocalizations.delegate],
          builder: builder,
          home: Center(
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                f.FlyoutTarget(
                  controller: flyout,
                  child: f.Button(
                    child: const Text('更多'),
                    onPressed: () => flyout.showFlyout(
                      builder: (_) => f.MenuFlyout(
                        items: [
                          f.MenuFlyoutItem(
                            text: const Text('菜单项甲'),
                            onPressed: () {},
                          ),
                          f.MenuFlyoutItem(
                            text: const Text('菜单项乙'),
                            onPressed: () {},
                          ),
                        ],
                      ),
                    ),
                  ),
                ),
                const SizedBox(height: 120),
                f.ComboBox<String>(
                  value: '选项甲',
                  items: const [
                    f.ComboBoxItem(value: '选项甲', child: Text('选项甲')),
                    f.ComboBoxItem(value: '选项乙', child: Text('选项乙')),
                  ],
                  onChanged: (_) {},
                ),
              ],
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.byType(f.DisableAcrylic), findsOneWidget);

    Future<int> alphaAt(Offset point) async {
      final render =
          boundary.currentContext!.findRenderObject()! as RenderRepaintBoundary;
      final bytes = await tester.runAsync(() async {
        final image = await render.toImage();
        final data = await image.toByteData();
        final width = image.width;
        image.dispose();
        return (data!, width);
      });
      final (data, width) = bytes!;
      return data.getUint8(
        (point.dy.round() * width + point.dx.round()) * 4 + 3,
      );
    }

    // 对照：窗口画布确为半透明。
    expect(await alphaAt(const Offset(4, 300)), lessThan(255));

    Future<void> expectOpaque(String item) async {
      final surface = find.ancestor(
        of: find.text(item),
        matching: find.byType(f.Acrylic),
      );
      // DisableAcrylic 生效时弹层不做背景模糊，以不透明色调绘制。
      expect(
        find.descendant(of: surface, matching: find.byType(BackdropFilter)),
        findsNothing,
        reason: '$item 所在弹层不应模糊并混合后方内容',
      );
      final rect = tester.getRect(surface.first);
      final row = tester.getCenter(find.text(item));
      expect(
        await alphaAt(Offset(rect.left + 3, row.dy)),
        255,
        reason: '$item 所在弹层背景必须不透明',
      );
    }

    await tester.tap(find.text('更多'));
    await tester.pumpAndSettle();
    await expectOpaque('菜单项乙');
    await tester.tapAt(const Offset(4, 300));
    await tester.pumpAndSettle();

    await tester.tap(find.text('选项甲'));
    await tester.pumpAndSettle();
    await expectOpaque('选项乙');
    expect(tester.takeException(), isNull);
    debugDefaultTargetPlatformOverride = null;
  });

  test('弹层底色不透明并跟随应用配色', () {
    for (final theme in [
      desktopFluentTheme(),
      desktopFluentTheme(colorScheme: 'dracula'),
      desktopFluentTheme(colorScheme: 'solarized_light'),
      desktopFluentTheme(highContrast: true),
    ]) {
      expect(theme.menuColor.a, 1);
      expect(theme.acrylicBackgroundColor, theme.menuColor);
    }
    expect(desktopFluentTheme().menuColor, desktopMenu);
    expect(
      desktopFluentTheme(colorScheme: 'dracula').menuColor,
      desktopAppPalette('dracula')!.surface,
    );
  });

  // 其余桌面弹窗都经 ContentDialog：主体与按钮区底色在各配色下都必须不透明。
  for (final scheme in ['default', 'dracula', 'solarized_light', 'high']) {
    testWidgets('ContentDialog 背景不透明：$scheme', (tester) async {
      await tester.pumpWidget(
        MaterialApp(
          localizationsDelegates: const [f.FluentLocalizations.delegate],
          builder: (_, navigator) => f.FluentTheme(
            data: desktopFluentTheme(
              colorScheme: scheme,
              highContrast: scheme == 'high',
            ),
            child: navigator!,
          ),
          home: Builder(
            builder: (context) => TextButton(
              onPressed: () => f.showDialog<void>(
                context: context,
                builder: (_) => f.ContentDialog(
                  title: const Text('标题'),
                  content: const Text('正文'),
                  actions: [f.Button(onPressed: () {}, child: const Text('好'))],
                ),
              ),
              child: const Text('打开'),
            ),
          ),
        ),
      );
      await tester.tap(find.text('打开'));
      await tester.pumpAndSettle();
      final style = f.ContentDialogTheme.of(
        tester.element(find.byType(f.ContentDialog)),
      );
      expect((style.decoration! as BoxDecoration).color!.a, 1.0);
      expect((style.actionsDecoration! as BoxDecoration).color!.a, 1.0);
    });
  }

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
