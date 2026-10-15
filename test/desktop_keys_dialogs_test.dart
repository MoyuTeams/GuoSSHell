import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';

import 'package:file_selector/file_selector.dart';
import 'package:fluent_ui/fluent_ui.dart' as fluent;
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/desktop/desktop_theme.dart';
import 'package:guosh_shell/src/desktop/keys/desktop_key_actions.dart';
import 'package:guosh_shell/src/desktop/keys/desktop_key_dialogs.dart';
import 'package:guosh_shell/src/desktop/keys/desktop_key_sync_state.dart';

const _empty = KeyListState(
  keys: [],
  listError: KeyError.none,
  syncPending: false,
  syncTargetEnabled: false,
  syncEnabled: false,
  syncAvailable: true,
  securityKeysAvailable: false,
);

class _Unreadable extends XFile {
  _Unreadable() : super('unreadable.pem');
  @override
  Future<int> length() async => 10;
  @override
  Future<String> readAsString({Encoding encoding = utf8}) async =>
      throw StateError('fixture');
}

Future<void> _open(
  WidgetTester tester,
  fluent.Widget Function(DesktopKeySyncState) buildDialog,
) async {
  assignRustSignal['KeyListState']!(_empty.bincodeSerialize(), Uint8List(0));
  final syncState = DesktopKeySyncState();
  addTearDown(syncState.dispose);
  final dialog = buildDialog(syncState);
  await tester.pumpWidget(
    fluent.FluentApp(
      theme: desktopTheme(highContrast: true, reduceMotion: true),
      home: fluent.NavigationView(
        content: fluent.Builder(
          builder: (context) => fluent.Button(
            onPressed: () => fluent.showDialog<void>(
              context: context,
              builder: (_) => dialog,
            ),
            child: const fluent.Text('打开'),
          ),
        ),
      ),
    ),
  );
  await tester.tap(find.text('打开'));
  await tester.pumpAndSettle();
}

void main() {
  tearDown(() => KeyListState.latestRustSignal = null);
  testWidgets('375 宽导入对话框可操作且内容不溢出', (tester) async {
    tester.view.physicalSize = const fluent.Size(375, 900);
    tester.view.devicePixelRatio = 1;
    addTearDown(() {
      tester.view.resetPhysicalSize();
      tester.view.resetDevicePixelRatio();
    });
    await _open(
      tester,
      (sync) =>
          DesktopKeyImportDialog(actions: DesktopKeyActions(), syncState: sync),
    );
    expect(find.text('从文件选择'), findsOneWidget);
    expect(
      tester.getSize(find.byType(fluent.ContentDialog)).width,
      lessThanOrEqualTo(375),
    );
    expect(tester.takeException(), isNull);
  });

  testWidgets('导入：无效编码、读取失败保留输入、口令始终掩码', (tester) async {
    var count = 0;
    await _open(
      tester,
      (sync) => DesktopKeyImportDialog(
        syncState: sync,
        actions: DesktopKeyActions(
          pickFile: () async {
            if (count++ == 0) {
              return XFile.fromData(Uint8List.fromList([0xff, 0xfe]));
            }
            return _Unreadable();
          },
        ),
      ),
    );
    expect(
      tester
          .widgetList<fluent.TextBox>(find.byType(fluent.TextBox))
          .last
          .obscureText,
      isTrue,
    );
    await tester.enterText(find.byType(fluent.TextBox).at(1), '原有输入');
    await tester.tap(find.text('从文件选择'));
    await tester.pumpAndSettle();
    expect(find.textContaining('UTF-8'), findsOneWidget);
    await tester.tap(find.text('从文件选择'));
    await tester.pumpAndSettle();
    expect(find.textContaining('无法读取文件'), findsOneWidget);
    expect(find.text('原有输入'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('导入口令错误聚焦口令字段，不展示输入的口令', (tester) async {
    await _open(
      tester,
      (sync) => DesktopKeyImportDialog(
        syncState: sync,
        actions: DesktopKeyActions(
          import: (_, _, _) async => const KeyResult(
            requestId: 1,
            error: KeyError.passphraseWrong,
            keyId: '',
          ),
        ),
      ),
    );
    await tester.enterText(find.byType(fluent.TextBox).last, 'fixture-secret');
    await tester.tap(find.text('导入').last);
    await tester.pumpAndSettle();
    expect(find.text('口令不正确'), findsOneWidget);
    expect(
      find.byWidgetPredicate(
        (widget) => widget is fluent.Text && widget.data == 'fixture-secret',
      ),
      findsNothing,
    );
    expect(
      tester
          .widget<fluent.TextBox>(find.byType(fluent.TextBox).last)
          .focusNode
          ?.hasFocus,
      isTrue,
    );
  });

  testWidgets('导入框打开后迁移变为未知，提交禁用且不能越过页面锁', (tester) async {
    late DesktopKeySyncState sync;
    var imports = 0;
    await _open(tester, (state) {
      sync = state;
      return DesktopKeyImportDialog(
        syncState: state,
        actions: DesktopKeyActions(
          import: (_, _, _) async {
            imports++;
            return const KeyResult(
              requestId: 1,
              error: KeyError.none,
              keyId: 'one',
            );
          },
        ),
      );
    });
    await sync.migrate(
      true,
      setSync: (_) async =>
          const KeyResult(requestId: 2, error: KeyError.none, keyId: ''),
      query: () {},
    );
    await tester.pumpAndSettle();
    expect(sync.canMutate, isFalse);
    expect(
      tester
          .widget<fluent.FilledButton>(
            find.widgetWithText(fluent.FilledButton, '导入'),
          )
          .onPressed,
      isNull,
    );
    expect(find.text('存储位置尚未确认，暂不能导入私钥。'), findsOneWidget);
    expect(imports, 0);
  });

  testWidgets('读卡根据回包决定 NFC 与认证槽可用性', (tester) async {
    final calls = <bool>[];
    await _open(
      tester,
      (sync) => DesktopCardScanDialog(
        syncState: sync,
        actions: DesktopKeyActions(
          scan: (nfc) async {
            calls.add(nfc);
            return const CardScanResult(
              requestId: 1,
              error: KeyError.none,
              nfcAvailable: true,
              cards: [
                CardSummary(
                  ident: 'fixture',
                  cardholder: '',
                  algorithm: 'Ed25519',
                  publicKey: '',
                  fingerprint: '',
                  pinTriesLeft: 2,
                  touch: true,
                  added: false,
                ),
              ],
            );
          },
        ),
      ),
    );
    expect(find.text('认证槽不可用'), findsOneWidget);
    expect(find.text('通过 NFC 读取'), findsOneWidget);
    await tester.tap(find.text('通过 NFC 读取'));
    await tester.pumpAndSettle();
    expect(calls, [false, true]);
    expect(tester.takeException(), isNull);
  });

  testWidgets('读卡失败不展示旧卡，更不会把不可用能力标成成功', (tester) async {
    var count = 0;
    await _open(
      tester,
      (sync) => DesktopCardScanDialog(
        syncState: sync,
        actions: DesktopKeyActions(
          scan: (_) async {
            if (count++ == 0) {
              return const CardScanResult(
                requestId: 1,
                error: KeyError.none,
                nfcAvailable: false,
                cards: [
                  CardSummary(
                    ident: '旧卡',
                    cardholder: '',
                    algorithm: 'RSA',
                    publicKey: 'ssh-rsa fixture',
                    fingerprint: '',
                    pinTriesLeft: 3,
                    touch: false,
                    added: false,
                  ),
                ],
              );
            }
            return const CardScanResult(
              requestId: 2,
              error: KeyError.cardNotFound,
              nfcAvailable: false,
              cards: [],
            );
          },
        ),
      ),
    );
    expect(find.text('卡号 旧卡 · RSA'), findsOneWidget);
    expect(find.text('通过 NFC 读取'), findsNothing);
    await tester.tap(find.text('重新读取'));
    await tester.pumpAndSettle();
    expect(find.text('卡号 旧卡 · RSA'), findsNothing);
    expect(find.text('没有找到 OpenPGP 卡'), findsOneWidget);
  });

  testWidgets('读卡框打开后同步待确认，只允许读卡不允许登记', (tester) async {
    late DesktopKeySyncState sync;
    var registrations = 0;
    await _open(tester, (state) {
      sync = state;
      return DesktopCardScanDialog(
        syncState: state,
        actions: DesktopKeyActions(
          scan: (_) async => const CardScanResult(
            requestId: 1,
            error: KeyError.none,
            nfcAvailable: false,
            cards: [
              CardSummary(
                ident: 'card',
                cardholder: '',
                algorithm: 'RSA',
                publicKey: 'ssh-rsa fixture',
                fingerprint: '',
                pinTriesLeft: 2,
                touch: false,
                added: false,
              ),
            ],
          ),
          addCard: (_, _) async {
            registrations++;
            return const KeyResult(
              requestId: 2,
              error: KeyError.none,
              keyId: 'one',
            );
          },
        ),
      );
    });
    await sync.migrate(
      true,
      setSync: (_) async =>
          const KeyResult(requestId: 3, error: KeyError.none, keyId: ''),
      query: () {},
    );
    await tester.pumpAndSettle();
    expect(
      tester
          .widget<fluent.Button>(find.widgetWithText(fluent.Button, '添加'))
          .onPressed,
      isNull,
    );
    expect(find.text('存储位置尚未确认，暂不能登记卡片。'), findsOneWidget);
    expect(registrations, 0);
  });

  testWidgets('文件读取期间关闭界面后不调用已卸载控制器', (tester) async {
    final pending = Completer<XFile?>();
    await _open(
      tester,
      (sync) => DesktopKeyImportDialog(
        syncState: sync,
        actions: DesktopKeyActions(pickFile: () => pending.future),
      ),
    );
    await tester.tap(find.text('从文件选择'));
    await tester.pumpWidget(const fluent.SizedBox());
    pending.complete(XFile.fromData(Uint8List(65537)));
    await tester.pump(const Duration(milliseconds: 200));
    expect(tester.takeException(), isNull);
  });
}
