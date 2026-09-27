import 'dart:async';
import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/keys/keys_page.dart';

const _local = KeyListState(
  keys: [],
  listError: KeyError.none,
  syncPending: false,
  syncTargetEnabled: false,
  syncEnabled: false,
  syncAvailable: true,
  securityKeysAvailable: false,
);

void _publish(KeyListState state) {
  assignRustSignal['KeyListState']!(state.bincodeSerialize(), Uint8List(0));
}

Future<void> _enableSync(WidgetTester tester) async {
  await tester.tap(find.byType(SwitchListTile));
  await tester.pumpAndSettle();
  await tester.tap(find.text('上传并同步'));
  await tester.pumpAndSettle();
}

void main() {
  tearDown(() => KeyListState.latestRustSignal = null);

  testWidgets('停止同步清理未完成时明确提示云端残留并按原目标重试', (tester) async {
    _publish(_local.copyWith(syncPending: true));
    final targets = <bool>[];
    await tester.pumpWidget(
      MaterialApp(
        home: KeysPage(
          queryKeys: () {},
          setSync: (target) async {
            targets.add(target);
            return const KeyResult(
              requestId: 1,
              error: KeyError.none,
              keyId: '',
            );
          },
        ),
      ),
    );
    expect(find.textContaining('iCloud 钥匙串仍可能保留私钥与口令'), findsOneWidget);
    expect(find.text('私钥只保存在这台设备上'), findsNothing);
    expect(
      tester.widget<SwitchListTile>(find.byType(SwitchListTile)).onChanged,
      isNull,
    );
    await tester.tap(find.text('重试'));
    await tester.pumpAndSettle();
    expect(targets, [false]);
    expect(find.text('私钥同步迁移待完成'), findsNothing);
    expect(find.text('私钥只保存在这台设备上'), findsOneWidget);
  });

  testWidgets('列表失败显示错误与重试而不显示没有私钥', (tester) async {
    _publish(_local.copyWith(listError: KeyError.keychain));
    var queries = 0;
    await tester.pumpWidget(
      MaterialApp(home: KeysPage(queryKeys: () => queries++)),
    );
    expect(find.text('无法读取私钥列表'), findsOneWidget);
    expect(find.text('还没有私钥'), findsNothing);
    await tester.tap(find.text('重试'));
    expect(queries, 2);
  });

  testWidgets('同步超时后旧快照不能恢复仅本机说明，成功重试解除未知状态', (tester) async {
    _publish(_local);
    final targets = <bool>[];
    await tester.pumpWidget(
      MaterialApp(
        home: KeysPage(
          queryKeys: () {},
          setSync: (target) async {
            targets.add(target);
            if (targets.length == 1) throw TimeoutException('模拟等待超时');
            return const KeyResult(
              requestId: 2,
              error: KeyError.none,
              keyId: '',
            );
          },
        ),
      ),
    );
    await _enableSync(tester);
    _publish(_local);
    await tester.pumpAndSettle();
    expect(find.text('私钥同步请求尚未确认'), findsOneWidget);
    expect(find.text('私钥只保存在这台设备上'), findsNothing);
    expect(
      tester.widget<SwitchListTile>(find.byType(SwitchListTile)).onChanged,
      isNull,
    );
    await tester.tap(find.text('重试'));
    await tester.pumpAndSettle();
    expect(targets, [true, true]);
    expect(find.text('私钥同步请求尚未确认'), findsNothing);
    expect(find.textContaining('私钥保存在 iCloud 钥匙串'), findsOneWidget);
    expect(
      tester.widget<SwitchListTile>(find.byType(SwitchListTile)).onChanged,
      isNotNull,
    );
  });

  testWidgets('超时后重试失败由新快照解除未知状态，同时保留列表错误', (tester) async {
    _publish(_local);
    var attempts = 0;
    await tester.pumpWidget(
      MaterialApp(
        home: KeysPage(
          queryKeys: () {},
          setSync: (_) async {
            if (attempts++ == 0) throw TimeoutException('模拟等待超时');
            return const KeyResult(
              requestId: 2,
              error: KeyError.keychain,
              keyId: '',
            );
          },
        ),
      ),
    );
    await _enableSync(tester);
    await tester.tap(find.text('重试'));
    await tester.pumpAndSettle();
    expect(find.text('私钥同步请求尚未确认'), findsNothing);
    _publish(_local.copyWith(listError: KeyError.keychain));
    await tester.pumpAndSettle();
    expect(find.text('私钥同步迁移待完成'), findsNothing);
    expect(find.text('无法读取私钥列表'), findsOneWidget);
    expect(
      tester.widget<SwitchListTile>(find.byType(SwitchListTile)).onChanged,
      isNotNull,
    );
  });
}
