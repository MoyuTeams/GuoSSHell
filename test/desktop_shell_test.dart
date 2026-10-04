import 'package:fluent_ui/fluent_ui.dart' as fluent;
import 'package:flutter/material.dart' as material;
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/desktop/desktop_shell.dart';
import 'package:guosh_shell/src/desktop/catalog/desktop_catalog_gateway.dart';
import 'package:guosh_shell/src/desktop/catalog/desktop_catalog_page.dart';
import 'package:guosh_shell/src/desktop/desktop_status.dart';
import 'package:guosh_shell/src/desktop/desktop_theme.dart';
import 'package:guosh_shell/src/desktop/keys/desktop_keys_page.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';

class _CatalogWithoutRust extends RustDesktopCatalogGateway {
  const _CatalogWithoutRust();

  @override
  void query(String text) {}

  @override
  Stream<CatalogState> get catalogStates => const Stream.empty();
}

void main() {
  test('暗色正文与说明在实际字阶满足文字对比度', () {
    double ratio(material.Color a, material.Color b) {
      final light = a.computeLuminance();
      final dark = b.computeLuminance();
      return (light + 0.05) / (dark + 0.05);
    }

    expect(
      ratio(DesktopColors.text, DesktopColors.surface),
      greaterThanOrEqualTo(7),
    );
    expect(
      ratio(DesktopColors.muted, DesktopColors.surface),
      greaterThanOrEqualTo(4.5),
    );
    expect(
      ratio(DesktopColors.accent, DesktopColors.surface),
      greaterThanOrEqualTo(3),
    );
  });

  for (final width in [375.0, 768.0, 1280.0]) {
    testWidgets('Fluent 工作台在 $width 宽度从概览进入目录', (tester) async {
      tester.view.physicalSize = material.Size(width, 900);
      tester.view.devicePixelRatio = 1;
      addTearDown(() {
        tester.view.resetPhysicalSize();
        tester.view.resetDevicePixelRatio();
      });
      var settingsOpened = 0;
      await tester.pumpWidget(
        fluent.FluentApp(
          theme: desktopTheme(highContrast: false, reduceMotion: true),
          home: DesktopShell(
            catalogGateway: const _CatalogWithoutRust(),
            openSettings: () => settingsOpened++,
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.byType(fluent.NavigationView), findsOneWidget);
      expect(find.text('从连接目录开始'), findsOneWidget);
      expect(tester.takeException(), isNull);
      await tester.sendKeyEvent(LogicalKeyboardKey.tab);
      await tester.pump();
      expect(material.FocusManager.instance.primaryFocus, isNotNull);
      final settingsButton = find.widgetWithText(fluent.Button, '打开字体设置（兼容）');
      await tester.tap(settingsButton);
      await tester.pump();
      expect(settingsOpened, 1);
      final focus = material.Focus.of(tester.element(find.text('打开字体设置（兼容）')));
      focus.requestFocus();
      await tester.pump();
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pump();
      expect(settingsOpened, 2);
      expect(tester.takeException(), isNull);
      await tester.tap(find.text('打开连接目录'));
      await tester.pump(const Duration(milliseconds: 250));
      expect(find.byType(DesktopCatalogPage), findsOneWidget);
      expect(find.byType(material.MaterialApp), findsNothing);
      expect(find.text('打开原界面（尚未迁移）'), findsNothing);
      final pane = tester
          .widget<fluent.NavigationView>(find.byType(fluent.NavigationView))
          .pane!;
      expect((pane.items[3] as fluent.PaneItem).body, isA<DesktopKeysPage>());
      expect(
        pane.displayMode,
        width < DesktopMetrics.narrowWidth
            ? fluent.PaneDisplayMode.minimal
            : width < DesktopMetrics.wideWidth
            ? fluent.PaneDisplayMode.compact
            : fluent.PaneDisplayMode.expanded,
      );
      pane.onChanged!(1);
      // 组件状态页包含持续旋转的加载指示器，不能等待帧队列清空。
      await tester.pump(const Duration(milliseconds: 250));
      expect(find.text('组件状态'), findsWidgets);
      expect(find.byType(DesktopStatus), findsNWidgets(3));
      await tester.enterText(find.byType(fluent.TextBox).first, '示例');
      await tester.pump();
      expect(find.text('已输入示例内容；未保存。'), findsOneWidget);
      final disabled = tester.widget<fluent.Button>(
        find.widgetWithText(fluent.Button, '不可用操作'),
      );
      expect(disabled.onPressed, isNull);
      await tester.tap(find.text('示例操作'));
      await tester.pump(const Duration(milliseconds: 250));
      expect(find.text('已触发示例操作；未发送请求。'), findsOneWidget);
      expect(tester.takeException(), isNull);
    });
  }

  testWidgets('减少动画时等待状态使用静态图标，失败仍可读', (tester) async {
    await tester.pumpWidget(
      fluent.FluentApp(
        theme: desktopTheme(highContrast: true, reduceMotion: true),
        home: const material.MediaQuery(
          data: material.MediaQueryData(
            disableAnimations: true,
            highContrast: true,
          ),
          child: fluent.NavigationView(
            content: DesktopStatus(
              kind: DesktopStatusKind.loading,
              title: '等待中',
              detail: '设置尚未到达',
            ),
          ),
        ),
      ),
    );
    expect(find.byType(fluent.ProgressRing), findsNothing);
    expect(find.text('设置尚未到达'), findsOneWidget);
    expect(
      fluent.FluentTheme.of(tester.element(find.text('等待中')))
          .focusTheme
          .primaryBorder!
          .color,
      DesktopColors.highContrastText,
    );
  });

  testWidgets('高对比 TextBox 的实际资源与控件边界为白色', (tester) async {
    await tester.pumpWidget(
      fluent.FluentApp(
        theme: desktopTheme(highContrast: true, reduceMotion: true),
        home: const fluent.NavigationView(
          content: fluent.TextBox(placeholder: '高对比输入'),
        ),
      ),
    );
    final input = find.byType(fluent.TextBox);
    final resources = fluent.FluentTheme.of(tester.element(input)).resources;
    expect(resources.controlStrokeColorDefault, DesktopColors.highContrastText);
    final decoration = tester.widgetList<fluent.DecoratedBox>(
      find.descendant(of: input, matching: find.byType(fluent.DecoratedBox)),
    );
    expect(
      decoration.any(
        (box) => switch (box.decoration) {
          final fluent.BoxDecoration value =>
            value.border?.top.color == DesktopColors.highContrastText,
          _ => false,
        },
      ),
      isTrue,
    );
  });

  testWidgets('宽桌面页内容真实限制到 880', (tester) async {
    tester.view.physicalSize = const material.Size(1600, 900);
    tester.view.devicePixelRatio = 1;
    addTearDown(() {
      tester.view.resetPhysicalSize();
      tester.view.resetDevicePixelRatio();
    });
    await tester.pumpWidget(
      fluent.FluentApp(
        theme: desktopTheme(highContrast: false, reduceMotion: true),
        home: const fluent.NavigationView(
          content: DesktopPage(
            title: '宽页面',
            description: '内容宽度应受限',
            children: [],
          ),
        ),
      ),
    );
    expect(
      tester
          .getSize(find.byKey(const material.Key('desktop-page-content')))
          .width,
      lessThanOrEqualTo(DesktopMetrics.contentWidth),
    );
  });
}
