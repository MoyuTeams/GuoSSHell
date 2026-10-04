import 'dart:async';
import 'dart:typed_data';

import 'package:fluent_ui/fluent_ui.dart' as fluent;
import 'package:flutter/material.dart' as material;
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/desktop/catalog/desktop_catalog_gateway.dart';
import 'package:guosh_shell/src/desktop/catalog/desktop_catalog_page.dart';
import 'package:guosh_shell/src/desktop/catalog/desktop_connection_editor.dart';
import 'package:guosh_shell/src/desktop/desktop_theme.dart';
import 'package:guosh_shell/src/desktop/keys/desktop_key_actions.dart';
import 'package:guosh_shell/src/desktop/keys/desktop_key_sync_state.dart';
import 'package:guosh_shell/src/desktop/legacy_material_surface.dart';
import 'package:guosh_shell/src/terminal/session_target.dart';

const saved = ConnectionSummary(
  id: 'saved-1',
  name: '示例连接',
  host: 'example.test',
  port: 22,
  username: 'tester',
  auth: AuthMethod.password,
  passwordSaved: true,
  keyId: '',
  command: '',
);

const emptyKeyState = KeyListState(
  keys: [],
  listError: KeyError.none,
  syncPending: false,
  syncTargetEnabled: false,
  syncEnabled: false,
  syncAvailable: true,
  securityKeysAvailable: false,
);

DesktopKeySyncState readyKeySync() {
  assignRustSignal['KeyListState']!(
    emptyKeyState.bincodeSerialize(),
    Uint8List(0),
  );
  final state = DesktopKeySyncState();
  addTearDown(state.dispose);
  return state;
}

class FakeCatalog implements DesktopCatalogGateway {
  final catalog = StreamController<CatalogState>.broadcast();
  final keys = StreamController<KeyListState>.broadcast();
  final queries = <String>[];
  final drafts = <DesktopConnectionDraft>[];
  final duplicates = <String>[];
  final deletes = <String>[];
  CatalogError saveError = CatalogError.none;
  CatalogError actionError = CatalogError.none;
  bool holdSave = false;
  Completer<CatalogResult>? pendingSave;
  int keyQueries = 0;

  CatalogResult result(CatalogError error) =>
      CatalogResult(requestId: 1, error: error, connectionId: '');

  @override
  Stream<CatalogState> get catalogStates => catalog.stream;
  @override
  Stream<KeyListState> get keyStates => keys.stream;
  @override
  void query(String text) => queries.add(text);
  @override
  void queryKeys() => keyQueries++;
  @override
  Future<CatalogResult> save(DesktopConnectionDraft draft) {
    drafts.add(draft);
    if (holdSave) return (pendingSave = Completer<CatalogResult>()).future;
    return Future.value(result(saveError));
  }

  @override
  Future<CatalogResult> duplicate(String id) async {
    duplicates.add(id);
    return result(actionError);
  }

  @override
  Future<CatalogResult> delete(String id) async {
    deletes.add(id);
    return result(actionError);
  }

  void emit(String query, List<ConnectionSummary> connections) =>
      catalog.add(CatalogState(query: query, connections: connections));

  Future<void> close() async {
    await catalog.close();
    await keys.close();
  }
}

fluent.Widget app(fluent.Widget child) => fluent.FluentApp(
  theme: desktopTheme(highContrast: false, reduceMotion: true),
  home: material.MediaQuery(
    data: const material.MediaQueryData(disableAnimations: true),
    child: child,
  ),
);

Future<void> field(WidgetTester tester, String label, String text) async {
  final finder = find.byKey(fluent.ValueKey('catalog-field-$label'));
  await tester.ensureVisible(finder);
  await tester.enterText(finder, text);
}

void main() {
  tearDown(() => KeyListState.latestRustSignal = null);
  testWidgets('搜索仅接纳当前查询，编辑返回后重新查询且保留原文', (tester) async {
    final fake = FakeCatalog();
    addTearDown(fake.close);
    await tester.pumpWidget(
      app(DesktopCatalogPage(gateway: fake, openTarget: (_) {})),
    );
    fake.emit('', [saved]);
    await tester.pump();
    await tester.enterText(
      find.byKey(const fluent.Key('catalog-search')),
      'test',
    );
    await tester.pump();
    expect(fake.queries.last, 'test');
    fake.emit('', [saved]);
    await tester.pump();
    expect(find.text('示例连接'), findsNothing);
    fake.emit('test', [saved]);
    await tester.pumpAndSettle();
    expect(fake.catalog.hasListener, isTrue);
    expect(
      tester
          .widget<fluent.TextBox>(
            find.byKey(const fluent.Key('catalog-search')),
          )
          .controller!
          .text,
      'test',
    );
    expect(find.text('正在读取目录'), findsNothing);
    expect(find.text('示例连接'), findsOneWidget);
    await tester.tap(find.text('编辑'));
    await tester.pumpAndSettle();
    await tester.ensureVisible(find.text('取消'));
    await tester.tap(find.text('取消'));
    await tester.pumpAndSettle();
    expect(fake.queries.last, 'test');
    expect(find.text('示例连接'), findsOneWidget);
  });

  testWidgets('复制失败可读，删除须确认并明确提示钥匙串密码', (tester) async {
    final fake = FakeCatalog()..actionError = CatalogError.storage;
    addTearDown(fake.close);
    await tester.pumpWidget(
      app(DesktopCatalogPage(gateway: fake, openTarget: (_) {})),
    );
    fake.emit('', [saved]);
    await tester.pump();
    await tester.tap(find.text('复制'));
    await tester.pump();
    expect(fake.duplicates, ['saved-1']);
    expect(find.text('保存失败'), findsOneWidget);
    await tester.tap(find.text('删除'));
    await tester.pumpAndSettle();
    expect(find.textContaining('钥匙串里的密码也会一起删除'), findsOneWidget);
    await tester.tap(find.text('取消'));
    await tester.pumpAndSettle();
    expect(fake.deletes, isEmpty);
    await tester.tap(find.text('删除').first);
    await tester.pumpAndSettle();
    await tester.tap(find.text('删除').last);
    await tester.pumpAndSettle();
    expect(fake.deletes, ['saved-1']);
  });

  testWidgets('新建失败保留草稿，重试提交同一字段并成功返回', (tester) async {
    final fake = FakeCatalog()..saveError = CatalogError.hostInvalid;
    addTearDown(fake.close);
    await tester.pumpWidget(
      app(DesktopCatalogPage(gateway: fake, openTarget: (_) {})),
    );
    await tester.tap(find.text('添加连接'));
    await tester.pumpAndSettle();
    await field(tester, '主机', 'bad host');
    await field(tester, '用户名', 'user');
    await field(tester, '密码', 'hidden-secret');
    expect(
      tester
          .widget<fluent.TextBox>(
            find.byKey(const fluent.ValueKey('catalog-field-密码')),
          )
          .obscureText,
      isTrue,
    );
    expect(
      tester
          .widget<fluent.EditableText>(
            find.descendant(
              of: find.byKey(const fluent.ValueKey('catalog-field-密码')),
              matching: find.byType(fluent.EditableText),
            ),
          )
          .obscureText,
      isTrue,
    );
    await tester.ensureVisible(find.text('保存'));
    await tester.tap(find.text('保存'));
    await tester.pumpAndSettle();
    expect(find.text('主机地址无效'), findsOneWidget);
    expect(
      tester
          .widget<fluent.TextBox>(
            find.byKey(const fluent.ValueKey('catalog-field-主机')),
          )
          .controller!
          .text,
      'bad host',
    );
    expect(fake.drafts.single.passwordAction, PasswordAction.set);
    fake.saveError = CatalogError.none;
    await tester.tap(find.text('保存'));
    await tester.pumpAndSettle();
    expect(fake.drafts, hasLength(2));
    expect(find.byType(DesktopConnectionEditor), findsNothing);
  });

  testWidgets('既有密码留空保留、改密覆盖、关闭保存清除；等待期间禁用提交', (tester) async {
    final fake = FakeCatalog()..holdSave = true;
    addTearDown(fake.close);
    await tester.pumpWidget(
      app(
        fluent.NavigationView(
          content: DesktopConnectionEditor(gateway: fake, existing: saved),
        ),
      ),
    );
    await tester.ensureVisible(find.text('保存'));
    await tester.tap(find.text('保存'));
    await tester.pump();
    expect(fake.drafts.single.passwordAction, PasswordAction.keep);
    expect(
      tester
          .widget<fluent.FilledButton>(
            find.widgetWithText(fluent.FilledButton, '保存'),
          )
          .onPressed,
      isNull,
    );
    fake.pendingSave!.complete(fake.result(CatalogError.storage));
    await tester.pump();
    await field(tester, '密码', 'replacement-secret');
    await tester.ensureVisible(find.text('保存'));
    fake.holdSave = false;
    fake.saveError = CatalogError.storage;
    await tester.tap(find.text('保存'));
    await tester.pump();
    expect(fake.drafts.last.passwordAction, PasswordAction.set);
    expect(
      tester
          .widget<fluent.TextBox>(
            find.byKey(const fluent.ValueKey('catalog-field-密码')),
          )
          .obscureText,
      isTrue,
    );
    await tester.ensureVisible(
      find.byKey(const fluent.Key('catalog-save-password')),
    );
    await tester.tap(find.byKey(const fluent.Key('catalog-save-password')));
    await tester.pump();
    await tester.ensureVisible(find.text('保存'));
    await tester.tap(find.text('保存'));
    await tester.pumpAndSettle();
    expect(fake.drafts.last.passwordAction, PasswordAction.clear);
  });

  testWidgets('快速连接不保存且密码只在目标内，取消不产生目标', (tester) async {
    final fake = FakeCatalog();
    addTearDown(fake.close);
    final targets = <SessionTarget>[];
    await tester.pumpWidget(
      app(DesktopCatalogPage(gateway: fake, openTarget: targets.add)),
    );
    await tester.tap(find.text('快速连接'));
    await tester.pumpAndSettle();
    expect(find.byKey(const fluent.Key('catalog-auth')), findsNothing);
    await field(tester, '主机', 'example.test');
    await field(tester, '用户名', 'tester');
    await field(tester, '密码', 'once-only');
    await tester.ensureVisible(find.text('连接'));
    await tester.tap(find.text('连接'));
    await tester.pumpAndSettle();
    expect(fake.drafts, isEmpty);
    expect(targets.single.password, 'once-only');
    expect(targets.single.connectionId, isEmpty);
    await tester.tap(find.text('快速连接'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('取消'));
    await tester.pumpAndSettle();
    expect(targets, hasLength(1));
  });

  testWidgets('私钥和键盘交互认证仅提交现有字段，不夹带先前密码', (tester) async {
    final fake = FakeCatalog()..saveError = CatalogError.keyRequired;
    addTearDown(fake.close);
    await tester.pumpWidget(
      app(
        fluent.NavigationView(content: DesktopConnectionEditor(gateway: fake)),
      ),
    );
    expect(fake.keyQueries, 1);
    await field(tester, '密码', 'not-for-key');
    await tester.ensureVisible(find.byKey(const fluent.Key('catalog-auth')));
    await tester.tap(find.byKey(const fluent.Key('catalog-auth')));
    await tester.pumpAndSettle();
    await tester.tap(find.text('私钥').last);
    await tester.pumpAndSettle();
    expect(find.byKey(const fluent.Key('catalog-key')), findsOneWidget);
    expect(find.text('私钥存储位置尚未确认，暂不能导入或登记卡片。'), findsOneWidget);
    expect(
      tester
          .widget<fluent.Button>(find.widgetWithText(fluent.Button, '导入私钥…'))
          .onPressed,
      isNull,
    );
    await tester.ensureVisible(find.text('保存'));
    await tester.tap(find.text('保存'));
    await tester.pumpAndSettle();
    expect(fake.drafts.last.auth, AuthMethod.publicKey);
    expect(fake.drafts.last.passwordAction, PasswordAction.clear);
    expect(fake.drafts.last.password, isEmpty);
    expect(find.text('请选择私钥'), findsOneWidget);
    await tester.ensureVisible(find.byKey(const fluent.Key('catalog-auth')));
    await tester.tap(find.byKey(const fluent.Key('catalog-auth')));
    await tester.pumpAndSettle();
    await tester.tap(find.text('键盘交互').last);
    await tester.pumpAndSettle();
    expect(find.textContaining('由服务器逐项提问'), findsOneWidget);
    await tester.ensureVisible(find.text('保存'));
    await tester.tap(find.text('保存'));
    await tester.pumpAndSettle();
    expect(fake.drafts.last.auth, AuthMethod.keyboardInteractive);
    expect(fake.drafts.last.passwordAction, PasswordAction.clear);
  });

  testWidgets('保存的目标使用 id 而非摘要密码，进入既有工作区回调', (tester) async {
    final fake = FakeCatalog();
    addTearDown(fake.close);
    final targets = <SessionTarget>[];
    await tester.pumpWidget(
      app(DesktopCatalogPage(gateway: fake, openTarget: targets.add)),
    );
    fake.emit('', [saved]);
    await tester.pumpAndSettle();
    await tester.tap(find.widgetWithText(fluent.FilledButton, '连接'));
    await tester.pumpAndSettle();
    expect(targets.single.connectionId, saved.id);
    expect(targets.single.password, isEmpty);
    expect(targets.single.title, saved.name);
  });

  testWidgets('Fluent 目录进入兼容工作区后，受保护关闭确认通过才回 Fluent 目录', (tester) async {
    final fake = FakeCatalog();
    addTearDown(fake.close);
    var confirmations = 0;
    var allowClose = false;
    await tester.pumpWidget(
      app(
        DesktopCatalogPage(
          gateway: fake,
          openTarget: (target) {
            fluent.Navigator.of(tester.element(find.byType(DesktopCatalogPage)))
                .push(
                  fluent.FluentPageRoute<void>(
                    builder: (_) => LegacyMaterialSurface(
                      initialTarget: target,
                      homeOverride: const material.Scaffold(
                        body: material.Text('测试兼容首页'),
                      ),
                      workspaceBuilder: (_) => material.PopScope(
                        canPop: false,
                        onPopInvokedWithResult: (didPop, _) {
                          if (!didPop) {
                            confirmations++;
                            if (allowClose) {
                              material.Navigator.of(
                                tester.element(find.text('受保护工作区')),
                              ).pop();
                            }
                          }
                        },
                        child: const material.Scaffold(
                          body: material.Text('受保护工作区'),
                        ),
                      ),
                    ),
                  ),
                );
          },
        ),
      ),
    );
    fake.emit('', [saved]);
    await tester.pumpAndSettle();
    await tester.tap(find.widgetWithText(fluent.FilledButton, '连接'));
    await tester.pumpAndSettle();
    expect(find.text('受保护工作区'), findsOneWidget);
    expect(find.textContaining('新标签、分屏的连接选择器与快速连接仍使用原界面'), findsOneWidget);
    expect(find.text('返回 Fluent 工作台'), findsOneWidget);
    expect(find.text('测试兼容首页'), findsNothing);
    await tester.tap(find.text('返回 Fluent 工作台'));
    await tester.pumpAndSettle();
    expect(confirmations, 1);
    expect(find.byType(LegacyMaterialSurface), findsOneWidget);
    allowClose = true;
    await tester.tap(find.text('返回 Fluent 工作台'));
    await tester.pumpAndSettle();
    expect(confirmations, 2);
    expect(find.byType(LegacyMaterialSurface), findsNothing);
    expect(find.text('连接目录'), findsOneWidget);
    expect(find.text('测试兼容首页'), findsNothing);
    expect(fake.queries.length, greaterThan(1));
  });

  testWidgets('编辑表单里导入 Fluent 私钥后选中新 ID，草稿保留且可保存', (tester) async {
    final fake = FakeCatalog()..saveError = CatalogError.hostInvalid;
    addTearDown(fake.close);
    final syncState = readyKeySync();
    final actions = DesktopKeyActions(
      import: (name, key, passphrase) async => const KeyResult(
        requestId: 1,
        error: KeyError.none,
        keyId: 'imported-key',
      ),
    );
    await tester.pumpWidget(
      app(
        fluent.NavigationView(
          content: DesktopConnectionEditor(
            gateway: fake,
            keyActions: actions,
            keySyncState: syncState,
          ),
        ),
      ),
    );
    await field(tester, '主机', 'draft.example');
    await tester.tap(find.byKey(const fluent.Key('catalog-auth')));
    await tester.pumpAndSettle();
    await tester.tap(find.text('私钥').last);
    await tester.pumpAndSettle();
    await tester.ensureVisible(find.text('导入私钥…'));
    await tester.tap(find.text('导入私钥…'));
    await tester.pumpAndSettle();
    expect(find.byType(fluent.ContentDialog), findsOneWidget);
    await tester.tap(find.widgetWithText(fluent.FilledButton, '导入'));
    await tester.pumpAndSettle();
    expect(find.byType(fluent.ContentDialog), findsNothing);
    expect(find.text('新登记的私钥'), findsOneWidget);
    expect(fake.keyQueries, 2);
    expect(
      tester
          .widget<fluent.TextBox>(
            find.byKey(const fluent.ValueKey('catalog-field-主机')),
          )
          .controller!
          .text,
      'draft.example',
    );
    await tester.ensureVisible(find.text('保存'));
    await tester.tap(find.text('保存'));
    await tester.pumpAndSettle();
    expect(fake.drafts.last.keyId, 'imported-key');
    expect(fake.drafts.last.host, 'draft.example');
  });

  testWidgets('编辑表单里 Fluent 读卡登记后选中新卡 ID，不清除其他字段', (tester) async {
    final fake = FakeCatalog()..saveError = CatalogError.hostInvalid;
    addTearDown(fake.close);
    final syncState = readyKeySync();
    const card = CardSummary(
      ident: 'card-fixture',
      cardholder: '测试卡',
      algorithm: 'RSA',
      publicKey: 'fixture-public-key',
      fingerprint: '',
      pinTriesLeft: 3,
      touch: false,
      added: false,
    );
    final actions = DesktopKeyActions(
      scan: (_) async => const CardScanResult(
        requestId: 1,
        error: KeyError.none,
        cards: [card],
        nfcAvailable: false,
      ),
      addCard: (ident, name) async => const KeyResult(
        requestId: 2,
        error: KeyError.none,
        keyId: 'card-key',
      ),
    );
    await tester.pumpWidget(
      app(
        fluent.NavigationView(
          content: DesktopConnectionEditor(
            gateway: fake,
            keyActions: actions,
            keySyncState: syncState,
          ),
        ),
      ),
    );
    await field(tester, '用户名', 'draft-user');
    await tester.tap(find.byKey(const fluent.Key('catalog-auth')));
    await tester.pumpAndSettle();
    await tester.tap(find.text('私钥').last);
    await tester.pumpAndSettle();
    await tester.ensureVisible(find.text('添加 OpenPGP 卡…'));
    await tester.tap(find.text('添加 OpenPGP 卡…'));
    await tester.pumpAndSettle();
    await tester.tap(find.widgetWithText(fluent.Button, '添加'));
    await tester.pumpAndSettle();
    await tester.tap(find.widgetWithText(fluent.FilledButton, '添加'));
    await tester.pumpAndSettle();
    expect(find.byType(fluent.ContentDialog), findsNothing);
    expect(find.text('新登记的私钥'), findsOneWidget);
    await tester.ensureVisible(find.text('保存'));
    await tester.tap(find.text('保存'));
    await tester.pumpAndSettle();
    expect(fake.drafts.last.keyId, 'card-key');
    expect(fake.drafts.last.username, 'draft-user');
  });
}
