import 'dart:async';
import 'dart:convert';

import 'package:file_selector/file_selector.dart';
import 'package:fluent_ui/fluent_ui.dart' as fluent;
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/desktop/desktop_theme.dart';
import 'package:guosh_shell/src/desktop/keys/desktop_key_actions.dart';
import 'package:guosh_shell/src/desktop/keys/desktop_key_sync_state.dart';
import 'package:guosh_shell/src/desktop/keys/desktop_keys_page.dart';

const _sample = KeySummary(
  id: 'one',
  name: '工作凭据',
  algorithm: 'Ed25519',
  fingerprint: 'SHA256:fixture',
  publicKey: 'ssh-ed25519 fixture',
  encrypted: true,
  passphraseSaved: true,
  synchronized: false,
  usedBy: 0,
  cardIdent: '',
  securityKey: false,
);
const _empty = KeyListState(
  keys: [],
  listError: KeyError.none,
  syncPending: false,
  syncTargetEnabled: false,
  syncEnabled: false,
  syncAvailable: true,
  securityKeysAvailable: false,
);

void _publish(KeyListState state) =>
    assignRustSignal['KeyListState']!(state.bincodeSerialize(), Uint8List(0));

const _ok = KeyResult(requestId: 1, error: KeyError.none, keyId: 'one');

Future<void> _show(
  WidgetTester tester,
  DesktopKeyActions actions, {
  DesktopKeySyncState? syncState,
}) async {
  final state = syncState ?? DesktopKeySyncState();
  if (syncState == null) addTearDown(state.dispose);
  await tester.pumpWidget(
    fluent.FluentApp(
      theme: desktopTheme(highContrast: false, reduceMotion: true),
      home: fluent.NavigationView(
        content: DesktopKeysPage(actions: actions, syncState: state),
      ),
    ),
  );
  await tester.pumpAndSettle();
}

void main() {
  tearDown(() => KeyListState.latestRustSignal = null);

  for (final width in [375.0, 768.0, 1280.0]) {
    testWidgets('$width 宽工作区能显示列表、Fluent 详情和操作', (tester) async {
      tester.view.physicalSize = Size(width, 900);
      tester.view.devicePixelRatio = 1;
      addTearDown(() {
        tester.view.resetPhysicalSize();
        tester.view.resetDevicePixelRatio();
      });
      _publish(_empty.copyWith(keys: [_sample]));
      await _show(tester, DesktopKeyActions(query: () {}));
      expect(find.text('工作凭据'), findsOneWidget);
      await tester.tap(find.text('工作凭据'));
      await tester.pumpAndSettle();
      expect(find.text('ssh-ed25519 fixture'), findsOneWidget);
      expect(find.text('SHA256:fixture'), findsWidgets);
      expect(find.textContaining('0 个连接引用'), findsOneWidget);
      expect(find.byType(fluent.ContentDialog), findsNothing);
      expect(tester.takeException(), isNull);
    });
  }

  testWidgets('列表错误单独展示并重新查询，不伪装空态', (tester) async {
    _publish(_empty.copyWith(listError: KeyError.keychain));
    var queries = 0;
    await _show(tester, DesktopKeyActions(query: () => queries++));
    expect(find.text('无法读取私钥列表'), findsOneWidget);
    expect(find.text('还没有私钥'), findsNothing);
    await tester.tap(find.text('重试读取'));
    expect(queries, 2);
    await tester.pump(const Duration(milliseconds: 200));
  });

  testWidgets('文件导入成功后只显示状态与登记 id，不回显私钥', (tester) async {
    _publish(_empty);
    var queries = 0;
    String? receivedKey;
    await _show(
      tester,
      DesktopKeyActions(
        query: () => queries++,
        pickFile: () async => XFile.fromData(
          Uint8List.fromList(utf8.encode('测试私钥原文')),
          name: 'fixture.pem',
        ),
        import: (_, key, _) async {
          receivedKey = key;
          return _ok;
        },
      ),
    );
    await tester.tap(find.text('导入私钥'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('从文件选择'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('导入').last);
    await tester.pumpAndSettle();
    expect(receivedKey, '测试私钥原文');
    expect(find.text('私钥已导入'), findsOneWidget);
    expect(find.text('测试私钥原文'), findsNothing);
    expect(queries, 2);
  });

  testWidgets('卡片认证槽登记后查询列表，不以扫码本身作为登记成功', (tester) async {
    _publish(_empty);
    var registered = 0;
    var queries = 0;
    await _show(
      tester,
      DesktopKeyActions(
        query: () => queries++,
        scan: (_) async => const CardScanResult(
          requestId: 1,
          error: KeyError.none,
          nfcAvailable: false,
          cards: [
            CardSummary(
              ident: 'card-one',
              cardholder: '测试卡',
              algorithm: 'Ed25519',
              publicKey: 'ssh-ed25519 fixture',
              fingerprint: 'SHA256:fixture',
              pinTriesLeft: 3,
              touch: false,
              added: false,
            ),
          ],
        ),
        addCard: (ident, _) async {
          expect(ident, 'card-one');
          registered++;
          return _ok;
        },
      ),
    );
    await tester.tap(find.text('添加 OpenPGP 卡'));
    await tester.pumpAndSettle();
    expect(find.text('卡片公钥已登记'), findsNothing);
    await tester.tap(find.text('添加'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('添加').last);
    await tester.pumpAndSettle();
    expect(registered, 1);
    expect(queries, 2);
    expect(find.text('卡片公钥已登记'), findsOneWidget);
  });

  testWidgets('引用保护删除，确认后也不向请求层发送删除', (tester) async {
    _publish(_empty.copyWith(keys: [_sample.copyWith(usedBy: 2)]));
    var deletes = 0;
    await _show(
      tester,
      DesktopKeyActions(
        query: () {},
        delete: (_) async {
          deletes++;
          return _ok;
        },
      ),
    );
    await tester.tap(find.text('工作凭据'));
    await tester.pumpAndSettle();
    await tester.ensureVisible(find.text('删除'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('删除'));
    await tester.pumpAndSettle();
    expect(find.textContaining('2 个连接引用它'), findsOneWidget);
    expect(deletes, 0);
  });

  testWidgets('忘记口令经 Fluent 确认，不展示口令原文', (tester) async {
    _publish(_empty.copyWith(keys: [_sample]));
    var forgotten = 0;
    await _show(
      tester,
      DesktopKeyActions(
        query: () {},
        forgetPassphrase: (_) async {
          forgotten++;
          return _ok;
        },
      ),
    );
    await tester.tap(find.text('工作凭据'));
    await tester.pumpAndSettle();
    await tester.ensureVisible(find.text('忘记口令'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('忘记口令'));
    await tester.pumpAndSettle();
    expect(find.byType(fluent.ContentDialog), findsOneWidget);
    await tester.tap(find.text('忘记口令').last);
    await tester.pumpAndSettle();
    expect(forgotten, 1);
  });

  testWidgets('同步超时保留未知状态，仅按原目标重试确认迁移', (tester) async {
    _publish(_empty);
    final targets = <bool>[];
    await _show(
      tester,
      DesktopKeyActions(
        query: () {},
        setSync: (target) async {
          targets.add(target);
          if (targets.length == 1) throw TimeoutException('fixture');
          return _ok;
        },
      ),
    );
    await tester.tap(find.byType(fluent.ToggleSwitch));
    await tester.pumpAndSettle();
    await tester.tap(find.text('上传并同步'));
    await tester.pumpAndSettle();
    _publish(_empty);
    await tester.pumpAndSettle();
    expect(find.text('私钥同步位置待确认'), findsOneWidget);
    expect(
      tester
          .widget<fluent.ToggleSwitch>(find.byType(fluent.ToggleSwitch))
          .onChanged,
      isNull,
    );
    // 无请求号的旧列表即使与目标一致，也不能替代本次超时请求的确认。
    _publish(_empty.copyWith(syncEnabled: true, syncTargetEnabled: true));
    await tester.pumpAndSettle();
    expect(find.text('私钥同步位置待确认'), findsOneWidget);
    expect(find.textContaining('私钥保存在 iCloud 钥匙串'), findsNothing);
    await tester.tap(find.text('按原目标重试'));
    await tester.pumpAndSettle();
    expect(targets, [true, true]);
    expect(find.text('私钥同步位置待确认'), findsOneWidget);
    expect(find.textContaining('私钥保存在 iCloud 钥匙串'), findsNothing);
    _publish(_empty);
    await tester.pumpAndSettle();
    expect(find.text('私钥同步位置待确认'), findsOneWidget);
    _publish(_empty.copyWith(syncEnabled: true, syncTargetEnabled: true));
    await tester.pumpAndSettle();
    expect(find.textContaining('私钥保存在 iCloud 钥匙串'), findsOneWidget);
  });

  testWidgets('同步关闭但本机与 iCloud 均有钥匙时逐项标注并允许导入和切换', (tester) async {
    final cloud = _sample.copyWith(
      id: 'cloud',
      name: '云端凭据',
      synchronized: true,
    );
    _publish(_empty.copyWith(keys: [_sample, cloud]));
    final sync = DesktopKeySyncState();
    addTearDown(sync.dispose);
    await _show(tester, DesktopKeyActions(query: () {}), syncState: sync);
    expect(sync.locationUnknown, isFalse);
    expect(sync.canMutate, isTrue);
    expect(find.text('私钥只保存在这台设备上'), findsNothing);
    expect(find.textContaining('列表中有私钥位于 iCloud 钥匙串'), findsOneWidget);
    expect(find.text('iCloud'), findsOneWidget);
    expect(
      tester
          .widget<fluent.FilledButton>(
            find.widgetWithText(fluent.FilledButton, '导入私钥'),
          )
          .onPressed,
      isNotNull,
    );
    expect(
      tester
          .widget<fluent.ToggleSwitch>(find.byType(fluent.ToggleSwitch))
          .onChanged,
      isNotNull,
    );
    await tester.tap(find.text('工作凭据'));
    await tester.pumpAndSettle();
    expect(find.textContaining('0 个连接引用 · 本机凭据'), findsOneWidget);
    await tester.tap(find.text('云端凭据'));
    await tester.pumpAndSettle();
    expect(find.textContaining('0 个连接引用 · iCloud 钥匙串'), findsOneWidget);
    await tester.ensureVisible(find.text('删除'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('删除'));
    await tester.pumpAndSettle();
    expect(find.textContaining('从 iCloud 钥匙串删除'), findsOneWidget);
    await tester.tap(find.text('取消'));
    await tester.pumpAndSettle();
  });

  testWidgets('正常混合允许发起同步，但回包与混合旧列表之间必须禁写', (tester) async {
    final cloud = _sample.copyWith(
      id: 'cloud',
      name: '云端凭据',
      synchronized: true,
    );
    final mixed = _empty.copyWith(keys: [_sample, cloud]);
    _publish(mixed);
    final sync = DesktopKeySyncState();
    addTearDown(sync.dispose);
    await _show(
      tester,
      DesktopKeyActions(query: () {}, setSync: (_) async => _ok),
      syncState: sync,
    );
    await tester.tap(find.byType(fluent.ToggleSwitch));
    await tester.pumpAndSettle();
    await tester.tap(find.text('上传并同步'));
    await tester.pumpAndSettle();
    expect(sync.canMutate, isFalse);
    _publish(mixed.copyWith(syncEnabled: true, syncTargetEnabled: true));
    await tester.pumpAndSettle();
    expect(sync.canMutate, isFalse);
    expect(find.textContaining('私钥保存在 iCloud 钥匙串'), findsNothing);
    expect(
      tester
          .widget<fluent.FilledButton>(
            find.widgetWithText(fluent.FilledButton, '导入私钥'),
          )
          .onPressed,
      isNull,
    );
    _publish(
      mixed.copyWith(
        syncEnabled: true,
        syncTargetEnabled: true,
        keys: [_sample.copyWith(synchronized: true), cloud],
      ),
    );
    await tester.pumpAndSettle();
    expect(sync.canMutate, isTrue);
    expect(find.textContaining('私钥保存在 iCloud 钥匙串'), findsOneWidget);
  });

  testWidgets('导航页销毁重建仍保留超时锁，旧 latestRustSignal 不能宣称仅本机', (tester) async {
    final old = _empty.copyWith(keys: [_sample]);
    _publish(old);
    final sync = DesktopKeySyncState();
    addTearDown(sync.dispose);
    var attempts = 0;
    final actions = DesktopKeyActions(
      query: () {},
      setSync: (_) async {
        if (attempts++ == 0) throw TimeoutException('测试超时');
        return _ok;
      },
    );
    await _show(tester, actions, syncState: sync);
    await tester.tap(find.byType(fluent.ToggleSwitch));
    await tester.pumpAndSettle();
    await tester.tap(find.text('上传并同步'));
    await tester.pumpAndSettle();
    expect(sync.canMutate, isFalse);
    await tester.pumpWidget(const fluent.SizedBox());
    _publish(old);
    await tester.pump();
    await _show(tester, actions, syncState: sync);
    expect(find.text('私钥只保存在这台设备上'), findsNothing);
    expect(find.text('私钥同步位置待确认'), findsOneWidget);
    expect(
      tester
          .widget<fluent.FilledButton>(
            find.widgetWithText(fluent.FilledButton, '导入私钥'),
          )
          .onPressed,
      isNull,
    );
    await tester.tap(find.text('工作凭据'));
    await tester.pumpAndSettle();
    expect(find.textContaining('存储位置待确认'), findsWidgets);
    expect(
      tester
          .widget<fluent.Button>(find.widgetWithText(fluent.Button, '删除'))
          .onPressed,
      isNull,
    );
    _publish(
      old.copyWith(
        syncEnabled: true,
        syncTargetEnabled: true,
        keys: [_sample.copyWith(synchronized: true)],
      ),
    );
    await tester.pumpAndSettle();
    expect(sync.canMutate, isFalse);
    await tester.tap(find.text('按原目标重试'));
    await tester.pumpAndSettle();
    expect(sync.canMutate, isFalse);
    _publish(
      old.copyWith(
        syncEnabled: true,
        syncTargetEnabled: true,
        keys: [_sample.copyWith(synchronized: true)],
      ),
    );
    await tester.pumpAndSettle();
    expect(sync.canMutate, isTrue);
    expect(find.textContaining('私钥保存在 iCloud 钥匙串'), findsOneWidget);
  });

  testWidgets('成功回包早于全量列表、列表错误或逐项位置不符时禁改凭据', (tester) async {
    final old = _empty.copyWith(keys: [_sample]);
    _publish(old);
    final sync = DesktopKeySyncState();
    addTearDown(sync.dispose);
    await _show(
      tester,
      DesktopKeyActions(query: () {}, setSync: (_) async => _ok),
      syncState: sync,
    );
    await tester.tap(find.text('工作凭据'));
    await tester.pumpAndSettle();
    await tester.tap(find.byType(fluent.ToggleSwitch));
    await tester.pumpAndSettle();
    await tester.tap(find.text('上传并同步'));
    await tester.pumpAndSettle();
    expect(sync.canMutate, isFalse);
    expect(
      tester
          .widget<fluent.FilledButton>(
            find.widgetWithText(fluent.FilledButton, '导入私钥'),
          )
          .onPressed,
      isNull,
    );
    expect(
      tester
          .widget<fluent.Button>(
            find.widgetWithText(fluent.Button, '添加 OpenPGP 卡'),
          )
          .onPressed,
      isNull,
    );
    for (final action in ['重命名', '忘记口令', '删除']) {
      expect(
        tester
            .widget<fluent.Button>(find.widgetWithText(fluent.Button, action))
            .onPressed,
        isNull,
      );
    }
    expect(find.text('钥匙串迁移已由完整列表确认'), findsNothing);
    expect(find.textContaining('本机凭据'), findsNothing);
    expect(find.textContaining('私钥保存在 iCloud 钥匙串'), findsNothing);
    _publish(old.copyWith(syncEnabled: true, syncTargetEnabled: true));
    await tester.pumpAndSettle();
    expect(sync.canMutate, isFalse);
    _publish(
      old.copyWith(
        syncEnabled: true,
        syncTargetEnabled: true,
        listError: KeyError.keychain,
        keys: [],
      ),
    );
    await tester.pumpAndSettle();
    expect(sync.canMutate, isFalse);
    _publish(
      old.copyWith(
        syncEnabled: true,
        syncTargetEnabled: true,
        keys: [_sample.copyWith(synchronized: true)],
      ),
    );
    await tester.pumpAndSettle();
    expect(sync.canMutate, isTrue);
    expect(find.textContaining('私钥保存在 iCloud 钥匙串'), findsOneWidget);
  });

  testWidgets('页面卸载时请求仍在进行，回包由父级状态接收而非丢失', (tester) async {
    final old = _empty.copyWith(keys: [_sample]);
    _publish(old);
    final sync = DesktopKeySyncState();
    addTearDown(sync.dispose);
    final reply = Completer<KeyResult>();
    final actions = DesktopKeyActions(
      query: () {},
      setSync: (_) => reply.future,
    );
    await _show(tester, actions, syncState: sync);
    await tester.tap(find.byType(fluent.ToggleSwitch));
    await tester.pumpAndSettle();
    await tester.tap(find.text('上传并同步'));
    await tester.pumpAndSettle();
    expect(sync.busy, isTrue);
    await tester.pumpWidget(const fluent.SizedBox());
    reply.complete(_ok);
    await tester.pump(const Duration(milliseconds: 200));
    expect(sync.busy, isFalse);
    expect(sync.canMutate, isFalse);
    await _show(tester, actions, syncState: sync);
    expect(find.text('私钥只保存在这台设备上'), findsNothing);
    _publish(
      old.copyWith(
        keys: [_sample.copyWith(synchronized: true)],
        syncEnabled: true,
        syncTargetEnabled: true,
      ),
    );
    await tester.pumpAndSettle();
    expect(sync.canMutate, isTrue);
    expect(tester.takeException(), isNull);
  });

  testWidgets('失败回包及旧原位置列表不能解锁，重试成功后等目标列表', (tester) async {
    final old = _empty.copyWith(keys: [_sample]);
    _publish(old);
    final sync = DesktopKeySyncState();
    addTearDown(sync.dispose);
    var attempts = 0;
    await _show(
      tester,
      DesktopKeyActions(
        query: () {},
        setSync: (_) async => attempts++ == 0
            ? const KeyResult(
                requestId: 1,
                error: KeyError.syncUnavailable,
                keyId: '',
              )
            : _ok,
      ),
      syncState: sync,
    );
    await tester.tap(find.byType(fluent.ToggleSwitch));
    await tester.pumpAndSettle();
    await tester.tap(find.text('上传并同步'));
    await tester.pumpAndSettle();
    expect(sync.canMutate, isFalse);
    expect(find.text('私钥只保存在这台设备上'), findsNothing);
    _publish(old.copyWith(listError: KeyError.keychain, keys: []));
    await tester.pumpAndSettle();
    expect(sync.canMutate, isFalse);
    _publish(old);
    await tester.pumpAndSettle();
    expect(sync.canMutate, isFalse);
    expect(find.text('私钥只保存在这台设备上'), findsNothing);
    await tester.tap(find.text('按原目标重试'));
    await tester.pumpAndSettle();
    expect(sync.canMutate, isFalse);
    _publish(
      old.copyWith(
        syncEnabled: true,
        syncTargetEnabled: true,
        keys: [_sample.copyWith(synchronized: true)],
      ),
    );
    await tester.pumpAndSettle();
    expect(sync.canMutate, isTrue);
    expect(attempts, 2);
  });

  testWidgets('删除确认打开期间位置变化会中止旧警告下的删除', (tester) async {
    _publish(_empty.copyWith(keys: [_sample]));
    var deletes = 0;
    await _show(
      tester,
      DesktopKeyActions(
        query: () {},
        delete: (_) async {
          deletes++;
          return _ok;
        },
      ),
    );
    await tester.tap(find.text('工作凭据'));
    await tester.pumpAndSettle();
    await tester.ensureVisible(find.text('删除'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('删除'));
    await tester.pumpAndSettle();
    expect(find.textContaining('从钥匙串删除'), findsOneWidget);
    _publish(
      _empty.copyWith(
        syncEnabled: true,
        syncTargetEnabled: true,
        keys: [_sample.copyWith(synchronized: true)],
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('删除').last);
    await tester.pumpAndSettle();
    expect(deletes, 0);
    expect(find.textContaining('存储位置已变化'), findsOneWidget);
  });

  testWidgets('安全密钥能力不可用时无入口，取消登记不显示成功', (tester) async {
    _publish(_empty);
    var calls = 0;
    await _show(
      tester,
      DesktopKeyActions(
        query: () {},
        registerSecurityKey: (_) async {
          calls++;
          return const KeyResult(
            requestId: 1,
            error: KeyError.securityKeyCancelled,
            keyId: '',
          );
        },
      ),
    );
    expect(find.text('添加安全密钥（FIDO2）'), findsNothing);
    _publish(_empty.copyWith(securityKeysAvailable: true));
    await tester.pumpAndSettle();
    expect(find.text('添加安全密钥（FIDO2）'), findsOneWidget);
    await tester.tap(find.text('添加安全密钥（FIDO2）'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('继续'));
    await tester.pumpAndSettle();
    expect(calls, 1);
    expect(find.textContaining('安全密钥已登记'), findsNothing);
  });

  testWidgets('卸载后异步请求回包不更新界面', (tester) async {
    _publish(_empty.copyWith(keys: [_sample]));
    final pending = Completer<KeyResult>();
    await _show(
      tester,
      DesktopKeyActions(query: () {}, rename: (_, _) => pending.future),
    );
    await tester.tap(find.text('工作凭据'));
    await tester.pumpAndSettle();
    await tester.ensureVisible(find.text('重命名'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('重命名'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('保存'));
    await tester.pump(const Duration(milliseconds: 200));
    await tester.pumpWidget(const fluent.SizedBox());
    pending.complete(_ok);
    await tester.pump(const Duration(milliseconds: 200));
    expect(tester.takeException(), isNull);
  });

  testWidgets('旧壳尚未注入父级状态时保底对象也跨导航保留锁', (tester) async {
    _publish(_empty.copyWith(keys: [_sample]));
    final actions = DesktopKeyActions(
      query: () {},
      setSync: (_) async => throw TimeoutException('测试超时'),
    );
    Future<void> showLegacy() => tester.pumpWidget(
      fluent.FluentApp(
        theme: desktopTheme(highContrast: false, reduceMotion: true),
        home: fluent.NavigationView(content: DesktopKeysPage(actions: actions)),
      ),
    );
    await showLegacy();
    await tester.pumpAndSettle();
    await tester.tap(find.byType(fluent.ToggleSwitch));
    await tester.pumpAndSettle();
    await tester.tap(find.text('上传并同步'));
    await tester.pumpAndSettle();
    await tester.pumpWidget(const fluent.SizedBox());
    await showLegacy();
    await tester.pumpAndSettle();
    expect(find.text('私钥只保存在这台设备上'), findsNothing);
    expect(find.text('私钥同步位置待确认'), findsOneWidget);
    expect(
      tester
          .widget<fluent.FilledButton>(
            find.widgetWithText(fluent.FilledButton, '导入私钥'),
          )
          .onPressed,
      isNull,
    );
  });
}
