import 'dart:async';

import 'package:fluent_ui/fluent_ui.dart' as f;
import 'package:flutter/material.dart';
import 'package:flutter/gestures.dart'
    show kDoubleTapTimeout, kSecondaryMouseButton;
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/catalog/connection_editor_page.dart';
import 'package:guosh_shell/src/desktop/catalog/desktop_connection_editor.dart';
import 'package:guosh_shell/src/desktop/desktop_widgets.dart';
import 'package:guosh_shell/src/desktop/keys/desktop_key_actions.dart';
import 'package:guosh_shell/src/desktop/keys/desktop_keys_page.dart';
import 'package:guosh_shell/src/desktop/windows_chrome.dart';
import 'package:guosh_shell/src/desktop/windows_shell.dart';
import 'package:guosh_shell/src/terminal/session_target.dart';
import 'package:guosh_shell/src/workspace/workspace.dart';

import 'desktop_catalog_flow_test.dart'
    show FakeCatalog, saved, emptyKeyState, field;

const _ok = KeyResult(requestId: 1, error: KeyError.none, keyId: 'imported');
const _privateFixture =
    '-----BEGIN PRIVATE KEY-----\nfixture-private-key\n-----END PRIVATE KEY-----\n';

/// 只观测生产壳拥有的窗格树和控制器生命周期，不模拟 SSH 状态或真实网络。
class _PaneProbe extends StatefulWidget {
  const _PaneProbe(this.pane, this.onMount, this.onDispose);
  final WorkspacePane pane;
  final ValueChanged<WorkspacePane> onMount;
  final VoidCallback onDispose;

  @override
  State<_PaneProbe> createState() => _PaneProbeState();
}

class _PaneProbeState extends State<_PaneProbe> {
  @override
  void initState() {
    super.initState();
    widget.onMount(widget.pane);
  }

  @override
  void dispose() {
    widget.onDispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => Focus(
    focusNode: widget.pane.controller.focusNode,
    child: Text('生命周期探针：${widget.pane.controller.target.title}'),
  );
}

class _Harness {
  final gateway = FakeCatalog();
  final mountedPanes = <WorkspacePane>[];
  int disposals = 0;

  Future<void> show(WidgetTester tester, {DesktopKeyActions? actions}) async {
    addTearDown(gateway.close);
    await tester.pumpWidget(
      MaterialApp(
        theme: ThemeData.dark(),
        localizationsDelegates: const [f.FluentLocalizations.delegate],
        builder: (_, child) =>
            f.FluentTheme(data: desktopFluentTheme(), child: child!),
        home: WindowsShell(
          initial: const SessionTarget.saved(
            connectionId: 'existing',
            title: '已有标签',
          ),
          gateway: gateway,
          keyActions: actions ?? DesktopKeyActions(query: () {}),
          paneBuilder: (pane) =>
              _PaneProbe(pane, mountedPanes.add, () => disposals++),
        ),
      ),
    );
    gateway.emit('', [saved]);
    gateway.keys.add(emptyKeyState);
    await tester.pumpAndSettle();
  }
}

Future<void> _icon(WidgetTester tester, String label) async {
  await tester.tap(
    find.byWidgetPredicate(
      (widget) => widget is DesktopIconButton && widget.label == label,
    ),
  );
  await tester.pump(kDoubleTapTimeout);
  await tester.pumpAndSettle();
}

Future<void> _click(WidgetTester tester, String text) async {
  tester.binding.focusManager.primaryFocus?.unfocus();
  await tester.pumpAndSettle();
  await tester.ensureVisible(find.text(text).last);
  await tester.pumpAndSettle();
  await tester.tap(find.text(text).last);
  await tester.pumpAndSettle();
}

Future<void> _shortcut(WidgetTester tester, LogicalKeyboardKey key) async {
  await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
  await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
  await tester.sendKeyEvent(key);
  await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
  await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
  await tester.pumpAndSettle();
}

void main() {
  for (final size in [
    const Size(760, 520),
    const Size(1000, 768),
    const Size(1360, 860),
  ]) {
    testWidgets('$size Windows 实际入口添加、编辑、密钥往返不卸载窗格', (tester) async {
      tester.view.devicePixelRatio = 1;
      tester.view.physicalSize = size;
      addTearDown(tester.view.reset);
      final harness = _Harness();
      await harness.show(tester);
      final firstPane = harness.mountedPanes.single;
      final firstElement = tester.element(find.byType(_PaneProbe));
      final firstController = firstPane.controller;
      await _icon(tester, '添加连接');
      expect(find.byType(DesktopConnectionEditor), findsOneWidget);
      expect(find.byType(ConnectionEditorPage), findsNothing);
      expect(find.byType(TextField), findsNothing);
      await field(tester, '主机', 'fixture.invalid');
      await field(tester, '用户名', 'fixture');
      await _click(tester, '保存');
      expect(harness.gateway.drafts.single.host, 'fixture.invalid');
      expect(harness.gateway.queries.last, '');
      await tester.enterText(find.byType(f.TextBox), '示例');
      harness.gateway.emit('示例', [saved]);
      await tester.pumpAndSettle();
      // 陈旧查询不覆盖当前搜索投影。
      harness.gateway.emit('', []);
      await tester.pumpAndSettle();
      expect(find.text(saved.name), findsOneWidget);
      await _icon(tester, '连接操作');
      expect(find.byType(f.MenuFlyout, skipOffstage: false), findsOneWidget);
      await _click(tester, '编辑');
      expect(
        tester
            .widget<DesktopConnectionEditor>(
              find.byType(DesktopConnectionEditor),
            )
            .existing,
        saved,
      );
      await _click(tester, '取消');
      expect(harness.gateway.queries.last, '示例');
      expect(
        tester.widget<f.TextBox>(find.byType(f.TextBox)).controller!.text,
        '示例',
      );
      await _icon(tester, '私钥管理');
      expect(find.byType(DesktopKeysPage), findsOneWidget);
      expect(find.text('还没有私钥'), findsOneWidget);
      await _click(tester, '返回工作区');
      expect(tester.element(find.byType(_PaneProbe)), same(firstElement));
      expect(harness.mountedPanes, [firstPane]);
      expect(firstPane.controller, same(firstController));
      expect(firstController.focusNode.hasFocus, isTrue);
      expect(harness.disposals, 0);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
      await tester.pump();
      expect(harness.disposals, 1);
    });
  }

  testWidgets('Windows 快捷键及标签新增均进入 Fluent 快速连接，目标进入原工作区', (tester) async {
    final harness = _Harness();
    await harness.show(tester);
    await _shortcut(tester, LogicalKeyboardKey.keyT);
    expect(
      tester
          .widget<DesktopConnectionEditor>(find.byType(DesktopConnectionEditor))
          .quick,
      isTrue,
    );
    // 编辑框焦点期间再次按工作区快捷键不能叠第二个编辑器。
    await field(tester, '主机', 'fixture.invalid');
    await _shortcut(tester, LogicalKeyboardKey.keyT);
    expect(find.byType(DesktopConnectionEditor), findsOneWidget);
    await field(tester, '用户名', 'fixture');
    await _click(tester, '连接');
    expect(harness.mountedPanes, hasLength(2));
    final target = harness.mountedPanes.last.controller.target;
    expect(target.host, 'fixture.invalid');
    expect(target.username, 'fixture');
    expect(target.connectionId, isEmpty);
    expect(harness.gateway.drafts, isEmpty);
    expect(harness.disposals, 0);
    // 这是生产 TabView 的新增回调，不另造导航或会话树。
    tester.widget<f.TabView>(find.byType(f.TabView)).onNewPressed!();
    await tester.pumpAndSettle();
    expect(
      tester
          .widget<DesktopConnectionEditor>(find.byType(DesktopConnectionEditor))
          .quick,
      isTrue,
    );
    await _click(tester, '取消');
    await _icon(tester, '快速连接 · Ctrl+Shift+T');
    expect(find.byType(DesktopConnectionEditor), findsOneWidget);
    await _click(tester, '取消');
    expect(harness.disposals, 0);
    expect(tester.takeException(), isNull);
  });

  testWidgets('Windows 侧栏复制和确认删除经过同一目录 gateway，取消不写入', (tester) async {
    final harness = _Harness();
    await harness.show(tester);
    await _icon(tester, '连接操作');
    await _click(tester, '复制');
    expect(harness.gateway.duplicates, [saved.id]);
    await _icon(tester, '连接操作');
    await _click(tester, '删除');
    expect(find.byType(f.ContentDialog), findsOneWidget);
    await _click(tester, '取消');
    expect(harness.gateway.deletes, isEmpty);
    await _icon(tester, '连接操作');
    await _click(tester, '删除');
    await _click(tester, '删除');
    expect(harness.gateway.deletes, [saved.id]);
    expect(harness.gateway.queries, ['', '', '']);
    expect(harness.disposals, 0);
    expect(tester.takeException(), isNull);
  });

  testWidgets('Windows 分屏树及活动焦点跨管理面板保留，关闭窗格仍使用原工作区', (tester) async {
    final harness = _Harness();
    await harness.show(tester);
    await _shortcut(tester, LogicalKeyboardKey.keyD);
    expect(harness.mountedPanes, hasLength(2));
    final panes = List<WorkspacePane>.of(harness.mountedPanes);
    final elements = tester.elementList(find.byType(_PaneProbe)).toList();
    await _icon(tester, '私钥管理');
    await _click(tester, '返回工作区');
    expect(
      tester.elementList(find.byType(_PaneProbe)),
      orderedEquals(elements),
    );
    expect(harness.mountedPanes, panes);
    expect(harness.disposals, 0);
    expect(panes.last.controller.focusNode.hasFocus, isTrue);
    await _shortcut(tester, LogicalKeyboardKey.keyW);
    expect(find.text('关闭窗格？'), findsOneWidget);
    await _click(tester, '取消');
    expect(harness.disposals, 0);
    await _shortcut(tester, LogicalKeyboardKey.keyW);
    await _click(tester, '断开');
    expect(find.byType(_PaneProbe), findsOneWidget);
    expect(harness.disposals, 1);
    expect(panes.first.controller.focusNode.hasFocus, isTrue);
    expect(tester.takeException(), isNull);
  });

  for (final card in [false, true]) {
    testWidgets('Windows 编辑草稿通过 Fluent ${card ? '读卡' : '私钥导入'}登记并保留失败草稿', (
      tester,
    ) async {
      final harness = _Harness();
      harness.gateway.saveError = CatalogError.hostInvalid;
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        (call) async =>
            call.method == 'Clipboard.hasStrings' ? {'value': true} : null,
      );
      addTearDown(
        () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          SystemChannels.platform,
          null,
        ),
      );
      var calls = 0;
      var clipboardReads = 0;
      final actions = DesktopKeyActions(
        query: () {},
        readClipboard: () async {
          clipboardReads++;
          return _privateFixture;
        },
        import: (_, key, _) async {
          expect(key, _privateFixture);
          calls++;
          return _ok;
        },
        scan: (_) async => const CardScanResult(
          requestId: 1,
          error: KeyError.none,
          nfcAvailable: false,
          cards: [
            CardSummary(
              ident: 'fixture-card',
              cardholder: '测试卡',
              algorithm: 'RSA',
              publicKey: 'fixture-public-key',
              fingerprint: '',
              pinTriesLeft: 3,
              touch: false,
              added: false,
            ),
          ],
        ),
        addCard: (ident, _) async {
          expect(ident, 'fixture-card');
          calls++;
          return _ok;
        },
      );
      await harness.show(tester, actions: actions);
      await _icon(tester, '添加连接');
      await field(tester, '主机', 'draft.invalid');
      await tester.ensureVisible(find.byKey(const Key('catalog-auth')));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('catalog-auth')));
      await tester.pumpAndSettle();
      await _click(tester, '私钥');
      await _click(tester, card ? '添加 OpenPGP 卡…' : '导入私钥…');
      expect(find.byType(f.ContentDialog), findsOneWidget);
      if (card) {
        await _click(tester, '添加');
        await _click(tester, '添加');
      } else {
        final privateField = find.byWidgetPredicate(
          (widget) =>
              widget is f.TextBox &&
              widget.placeholder == '粘贴 OpenSSH、PEM 或 PKCS#8 私钥',
        );
        expect(tester.widget<f.TextBox>(privateField).obscureText, isTrue);
        await _click(tester, '粘贴私钥');
        expect(tester.widget<f.TextBox>(privateField).readOnly, isTrue);
        await tester.tap(privateField);
        await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
        await tester.sendKeyEvent(LogicalKeyboardKey.keyA);
        await tester.sendKeyEvent(LogicalKeyboardKey.keyV);
        await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
        await tester.pumpAndSettle();
        expect(clipboardReads, 2);
        await tester.tap(privateField, buttons: kSecondaryMouseButton);
        await tester.pumpAndSettle();
        final toolbar = tester.widget<f.WindowsTextSelectionToolbar>(
          find.byType(f.WindowsTextSelectionToolbar),
        );
        tester.widget<f.TextBox>(privateField).controller!.selection =
            TextSelection(baseOffset: 0, extentOffset: _privateFixture.length);
        toolbar.buttonItems
            .singleWhere((item) => item.type == ContextMenuButtonType.paste)
            .onPressed!();
        await tester.pumpAndSettle();
        expect(clipboardReads, 3);
        expect(
          tester.widget<f.TextBox>(privateField).controller!.text,
          _privateFixture,
        );
        await _click(tester, '导入');
      }
      expect(calls, 1);
      expect(find.byType(f.ContentDialog), findsNothing);
      expect(find.text('新登记的私钥'), findsOneWidget);
      await _click(tester, '保存');
      expect(harness.gateway.drafts.last.keyId, 'imported');
      expect(harness.gateway.drafts.last.host, 'draft.invalid');
      expect(find.text('主机地址无效'), findsOneWidget);
      harness.gateway.saveError = CatalogError.none;
      await _click(tester, '保存');
      expect(find.byType(DesktopConnectionEditor), findsNothing);
      expect(harness.disposals, 0);
      expect(tester.takeException(), isNull);
    });
  }

  for (final error in [KeyError.none, KeyError.syncUnavailable, null]) {
    testWidgets('Windows 壳持有同步锁：$error 回包未确认，重开密钥和编辑仍禁写', (tester) async {
      final harness = _Harness();
      final reply = Completer<KeyResult>();
      var migrations = 0;
      await harness.show(
        tester,
        actions: DesktopKeyActions(
          query: () {},
          setSync: (_) {
            migrations++;
            return reply.future;
          },
        ),
      );
      await _icon(tester, '私钥管理');
      final sync = tester
          .widget<DesktopKeysPage>(find.byType(DesktopKeysPage))
          .syncState;
      await tester.tap(find.byType(f.ToggleSwitch));
      await tester.pumpAndSettle();
      await _click(tester, '上传并同步');
      expect(sync.busy, isTrue);
      await _click(tester, '返回工作区');
      if (error == null) {
        reply.completeError(TimeoutException('测试超时'));
      } else {
        reply.complete(_ok.copyWith(error: error));
      }
      await tester.pumpAndSettle();
      harness.gateway.keys.add(emptyKeyState);
      await tester.pumpAndSettle();
      // 成功必须等待回包之后的完整目标列表；失败和超时不能被旧列表解锁。
      expect(sync.canMutate, isFalse);
      await _icon(tester, '私钥管理');
      expect(
        tester.widget<DesktopKeysPage>(find.byType(DesktopKeysPage)).syncState,
        same(sync),
      );
      expect(migrations, 1);
      expect(
        tester
            .widget<f.FilledButton>(find.widgetWithText(f.FilledButton, '导入私钥'))
            .onPressed,
        isNull,
      );
      expect(find.text('私钥同步位置待确认'), findsOneWidget);
      await _click(tester, '返回工作区');
      await _icon(tester, '添加连接');
      final editor = tester.widget<DesktopConnectionEditor>(
        find.byType(DesktopConnectionEditor),
      );
      expect(editor.keySyncState, same(sync));
      await tester.ensureVisible(find.byKey(const Key('catalog-auth')));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('catalog-auth')));
      await tester.pumpAndSettle();
      await _click(tester, '私钥');
      expect(
        tester
            .widget<f.Button>(find.widgetWithText(f.Button, '导入私钥…'))
            .onPressed,
        isNull,
      );
      await _click(tester, '取消');
      harness.gateway.keys.add(emptyKeyState.copyWith(syncEnabled: true));
      await tester.pumpAndSettle();
      expect(sync.canMutate, error == KeyError.none);
      expect(harness.mountedPanes, hasLength(1));
      expect(harness.disposals, 0);
      expect(tester.takeException(), isNull);
    });
  }
}
