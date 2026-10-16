import 'dart:async';

import 'package:fluent_ui/fluent_ui.dart' as fluent;
import 'package:flutter/material.dart' as material;
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/desktop/catalog/desktop_catalog_gateway.dart';
import 'package:guosh_shell/src/desktop/catalog/desktop_catalog_page.dart';
import 'package:guosh_shell/src/desktop/catalog/desktop_connection_editor.dart';
import 'package:guosh_shell/src/desktop/desktop_shell.dart';
import 'package:guosh_shell/src/desktop/desktop_theme.dart';
import 'package:guosh_shell/src/desktop/keys/desktop_keys_page.dart';
import 'package:guosh_shell/src/desktop/keys/desktop_key_actions.dart';

class _Gateway extends RustDesktopCatalogGateway {
  _Gateway();

  final controller = StreamController<CatalogState>.broadcast();
  @override
  Stream<CatalogState> get catalogStates => controller.stream;
  @override
  void query(String _) {}
  @override
  void queryKeys() {}
  @override
  Stream<KeyListState> get keyStates => const Stream.empty();
}

void main() {
  for (final width in [375.0, 768.0, 1280.0]) {
    testWidgets('桌面 $width 宽度的导航进入真实 Fluent 目录和编辑器', (tester) async {
      tester.view.physicalSize = material.Size(width, 900);
      tester.view.devicePixelRatio = 1;
      addTearDown(() {
        tester.view.resetPhysicalSize();
        tester.view.resetDevicePixelRatio();
      });
      final gateway = _Gateway();
      addTearDown(gateway.controller.close);
      await tester.pumpWidget(
        fluent.FluentApp(
          theme: desktopTheme(highContrast: false, reduceMotion: true),
          home: material.MediaQuery(
            data: const material.MediaQueryData(disableAnimations: true),
            child: DesktopShell(catalogGateway: gateway, openTarget: (_) {}),
          ),
        ),
      );
      final pane = tester
          .widget<fluent.NavigationView>(find.byType(fluent.NavigationView))
          .pane!;
      pane.onChanged!(2);
      await tester.pump();
      expect(find.byType(DesktopCatalogPage), findsOneWidget);
      gateway.controller.add(
        const CatalogState(
          query: '',
          connections: [
            ConnectionSummary(
              id: 'test',
              name: '示例连接',
              host: 'example.test',
              port: 22,
              username: 'tester',
              auth: AuthMethod.password,
              passwordSaved: false,
              keyId: '',
              command: '',
              transport: DesktopTransport.nativeSsh,
            ),
          ],
        ),
      );
      await tester.pumpAndSettle();
      expect(find.text('示例连接'), findsOneWidget);
      await tester.tap(find.text('添加连接'));
      await tester.pumpAndSettle();
      expect(find.byType(DesktopConnectionEditor), findsOneWidget);
      expect(find.byType(material.Scaffold), findsNothing);
      expect(tester.takeException(), isNull);
    });
  }

  testWidgets('目录与密钥页共享同一同步状态，切换导航不会重建它', (tester) async {
    final gateway = _Gateway();
    addTearDown(gateway.controller.close);
    await tester.pumpWidget(
      fluent.FluentApp(
        theme: desktopTheme(highContrast: false, reduceMotion: true),
        home: DesktopShell(
          catalogGateway: gateway,
          keyActions: DesktopKeyActions(query: () {}),
        ),
      ),
    );
    final pane = tester
        .widget<fluent.NavigationView>(find.byType(fluent.NavigationView))
        .pane!;
    pane.onChanged!(2);
    await tester.pump();
    final shared = tester
        .widget<DesktopCatalogPage>(find.byType(DesktopCatalogPage))
        .keySyncState;
    expect(shared, isNotNull);
    pane.onChanged!(3);
    await tester.pump();
    expect(
      tester.widget<DesktopKeysPage>(find.byType(DesktopKeysPage)).syncState,
      same(shared),
    );
    pane.onChanged!(2);
    await tester.pump();
    expect(
      tester
          .widget<DesktopCatalogPage>(find.byType(DesktopCatalogPage))
          .keySyncState,
      same(shared),
    );
  });
}
