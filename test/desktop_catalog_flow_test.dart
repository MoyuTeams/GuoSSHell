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
import 'package:guosh_shell/src/desktop/workspace/desktop_target_options.dart';
import 'package:guosh_shell/src/terminal/desktop_ordered_session.dart'
    show desktopTargetKind;
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
  transport: DesktopTransport.nativeSsh,
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
  List<DesktopProfileOption> profiles = const [];
  bool profilesFail = false;
  String currentProfileId = '';
  bool profileIdFail = false;
  DesktopTransport currentTransport = DesktopTransport.nativeSsh;
  String currentIdentityPath = '';
  bool systemAvailable = true;

  @override
  Future<DesktopConnectionSummary> connection(String id) async {
    if (profileIdFail) throw StateError('读取失败');
    return DesktopConnectionSummary(
      id: id,
      name: '',
      targetKind: DesktopCatalogTargetKind.ssh,
      transport: currentTransport,
      host: '',
      port: 22,
      username: '',
      auth: DesktopAuthMethod.agent,
      passwordSaved: false,
      keyId: '',
      systemIdentityPath: currentIdentityPath,
      command: '',
      profileId: currentProfileId,
    );
  }

  @override
  Future<DesktopTargetOptions> targetOptions() async {
    if (profilesFail) throw StateError('读取失败');
    return DesktopTargetOptions(
      profiles: profiles,
      nativeAvailable: true,
      systemAvailable: systemAvailable,
      systemReason: '未找到系统 ssh（测试）',
    );
  }

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

/// 先让输入框失焦并稳定，再滚动到目标；否则焦点会把滚动位置拉回去，命中测试落空。
Future<void> reveal(WidgetTester tester, Finder finder) async {
  tester.binding.focusManager.primaryFocus?.unfocus();
  await tester.pumpAndSettle();
  await tester.ensureVisible(finder);
  await tester.pumpAndSettle();
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
    // 已有连接须等当前 profile 引用读回后才可保存。
    await tester.pump();
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
    await reveal(tester, find.byKey(const fluent.Key('catalog-auth')));
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

  const profiles = [
    DesktopProfileOption('p1', '终端甲'),
    DesktopProfileOption('p2', '终端乙'),
  ];

  Future<void> pumpEditor(
    WidgetTester tester,
    FakeCatalog fake, {
    ConnectionSummary? existing,
  }) async {
    addTearDown(fake.close);
    await tester.pumpWidget(
      app(
        fluent.NavigationView(
          content: DesktopConnectionEditor(gateway: fake, existing: existing),
        ),
      ),
    );
    await tester.pumpAndSettle();
  }

  Future<void> pickProfile(WidgetTester tester, String label) async {
    final combo = find.byKey(const fluent.Key('catalog-profile'));
    await reveal(tester, combo);
    await tester.tap(combo);
    await tester.pumpAndSettle();
    await tester.tap(find.text(label).last);
    await tester.pumpAndSettle();
  }

  Future<void> save(WidgetTester tester) async {
    await tester.ensureVisible(find.text('保存'));
    await tester.tap(find.text('保存'));
    await tester.pumpAndSettle();
  }

  testWidgets('新建连接默认不引用 profile（提交空串）', (tester) async {
    final fake = FakeCatalog()..profiles = profiles;
    await pumpEditor(tester, fake);
    expect(find.text('终端配置'), findsOneWidget);
    expect(find.text('使用默认配置'), findsOneWidget);
    await field(tester, '主机', 'example.test');
    await field(tester, '用户名', 'tester');
    await field(tester, '密码', 'secret');
    await save(tester);
    expect(fake.drafts.single.profileId, isEmpty);
  });

  testWidgets('新建连接选择 profile 后保存请求携带该 profile id', (tester) async {
    final fake = FakeCatalog()
      ..profiles = profiles
      ..saveError = CatalogError.storage;
    await pumpEditor(tester, fake);
    await field(tester, '主机', 'example.test');
    await field(tester, '用户名', 'tester');
    await field(tester, '密码', 'secret');
    await pickProfile(tester, '终端乙');
    await save(tester);
    expect(fake.drafts.last.profileId, 'p2');
  });

  testWidgets('编辑已有连接预填当前 profile，改回默认时提交空串', (tester) async {
    final fake = FakeCatalog()
      ..profiles = profiles
      ..currentProfileId = 'p1'
      ..saveError = CatalogError.storage;
    await pumpEditor(tester, fake, existing: saved);
    expect(find.text('终端甲'), findsOneWidget);
    await save(tester);
    expect(fake.drafts.last.id, 'saved-1');
    expect(fake.drafts.last.profileId, 'p1');
    await pickProfile(tester, '使用默认配置');
    await save(tester);
    expect(fake.drafts.last.profileId, isEmpty);
  });

  testWidgets('profile 列表读取失败时提示且保留原值保存', (tester) async {
    final fake = FakeCatalog()
      ..profilesFail = true
      ..currentProfileId = 'p2'
      ..saveError = CatalogError.storage;
    await pumpEditor(tester, fake, existing: saved);
    expect(find.textContaining('无法读取终端配置列表'), findsOneWidget);
    await save(tester);
    expect(fake.drafts.single.profileId, 'p2');
  });

  testWidgets('读不到已有连接的 profile 时不允许直接保存，显式选择后才可保存', (tester) async {
    final fake = FakeCatalog()
      ..profiles = profiles
      ..profileIdFail = true
      ..saveError = CatalogError.storage;
    await pumpEditor(tester, fake, existing: saved);
    expect(find.textContaining('无法读取该连接当前的终端配置'), findsOneWidget);
    await tester.ensureVisible(find.text('保存'));
    expect(
      tester
          .widget<fluent.FilledButton>(
            find.widgetWithText(fluent.FilledButton, '保存'),
          )
          .onPressed,
      isNull,
    );
    await pickProfile(tester, '终端甲');
    await save(tester);
    expect(fake.drafts.single.profileId, 'p1');
  });

  Future<void> pickTransport(WidgetTester tester, String label) async {
    final combo = find.byKey(const fluent.Key('catalog-transport'));
    await reveal(tester, combo);
    await tester.tap(combo);
    await tester.pumpAndSettle();
    await tester.tap(find.text(label).last);
    await tester.pumpAndSettle();
  }

  testWidgets('选 System OpenSSH 后隐藏密码与认证方式，保存携带连接方式与私钥文件', (tester) async {
    final fake = FakeCatalog()..saveError = CatalogError.storage;
    await pumpEditor(tester, fake);
    await field(tester, '主机', 'example.test');
    await field(tester, '用户名', 'tester');
    await field(tester, '密码', 'must-not-leak');
    await pickTransport(tester, 'System OpenSSH');
    expect(find.byKey(const fluent.Key('catalog-auth')), findsNothing);
    expect(find.byKey(const fluent.ValueKey('catalog-field-密码')), findsNothing);
    expect(find.byKey(const fluent.Key('catalog-save-password')), findsNothing);
    expect(find.textContaining('由系统 ssh 处理认证'), findsOneWidget);
    await save(tester);
    var draft = fake.drafts.last;
    expect(draft.transport, DesktopTransport.systemOpenSsh);
    expect(draft.systemIdentityPath, isEmpty);
    var request = desktopSaveRequest(draft, 1);
    expect(request.transport, DesktopTransport.systemOpenSsh);
    expect(request.auth, DesktopAuthMethod.agent);
    expect(request.password, isEmpty);
    expect(request.passwordAction, PasswordAction.clear);
    expect(request.keyId, isEmpty);

    await field(tester, '私钥文件（可选）', ' C:/keys/id_test ');
    await save(tester);
    draft = fake.drafts.last;
    request = desktopSaveRequest(draft, 2);
    expect(request.auth, DesktopAuthMethod.publicKey);
    expect(request.systemIdentityPath, 'C:/keys/id_test');
    expect(request.password, isEmpty);

    // 切回 Native：密码与认证方式恢复，System 路径不随请求发送。
    await pickTransport(tester, 'Native SSH');
    expect(find.byKey(const fluent.Key('catalog-auth')), findsOneWidget);
    await save(tester);
    request = desktopSaveRequest(fake.drafts.last, 3);
    expect(request.transport, DesktopTransport.nativeSsh);
    expect(request.auth, DesktopAuthMethod.password);
    expect(request.systemIdentityPath, isEmpty);
    expect(request.password, 'must-not-leak');
  });

  testWidgets('系统 ssh 不可用时 System 选项禁用并提示原因', (tester) async {
    final fake = FakeCatalog()..systemAvailable = false;
    await pumpEditor(tester, fake);
    expect(find.text('System OpenSSH 不可用：未找到系统 ssh（测试）'), findsOneWidget);
    final combo = find.byKey(const fluent.Key('catalog-transport'));
    await reveal(tester, combo);
    await tester.tap(combo);
    await tester.pumpAndSettle();
    await tester.tap(find.text('System OpenSSH').last, warnIfMissed: false);
    await tester.pumpAndSettle();
    expect(find.byKey(const fluent.Key('catalog-auth')), findsOneWidget);
  });

  testWidgets('编辑已有 System 连接读回私钥文件路径，打开时 Prepare 走 SystemOpenSsh', (
    tester,
  ) async {
    const system = ConnectionSummary(
      id: 'system-1',
      name: '系统连接',
      host: 'example.test',
      port: 22,
      username: 'tester',
      auth: AuthMethod.password,
      passwordSaved: false,
      keyId: '',
      command: '',
      transport: DesktopTransport.systemOpenSsh,
    );
    final fake = FakeCatalog()
      ..currentTransport = DesktopTransport.systemOpenSsh
      ..currentIdentityPath = '/home/tester/.ssh/id_work'
      ..saveError = CatalogError.storage;
    await pumpEditor(tester, fake, existing: system);
    expect(find.byKey(const fluent.Key('catalog-auth')), findsNothing);
    expect(
      tester
          .widget<fluent.TextBox>(
            find.byKey(const fluent.ValueKey('catalog-field-私钥文件（可选）')),
          )
          .controller!
          .text,
      '/home/tester/.ssh/id_work',
    );
    await save(tester);
    expect(fake.drafts.last.transport, DesktopTransport.systemOpenSsh);
    expect(fake.drafts.last.systemIdentityPath, '/home/tester/.ssh/id_work');

    final target = desktopSavedTarget(system, profileId: 'p1');
    expect(target.isSystemSsh, isTrue);
    expect(target.runsLocalProcess, isTrue);
    expect(target.connectionId, 'system-1');
    expect(desktopTargetKind(target), DesktopTargetKind.systemOpenSsh);
    expect(
      desktopTargetKind(desktopTargetWithProfile(target, 'p2')),
      DesktopTargetKind.systemOpenSsh,
    );
    expect(
      desktopTargetKind(desktopSavedTarget(saved)),
      DesktopTargetKind.nativeSsh,
    );
  });
}
