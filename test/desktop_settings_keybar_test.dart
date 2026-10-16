import 'dart:async';
import 'dart:math' as math;

import 'package:fluent_ui/fluent_ui.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/desktop/desktop_theme.dart';
import 'package:guosh_shell/src/desktop/settings/keybar/desktop_keybar.dart';
import 'package:guosh_shell/src/terminal/key_bar_layout.dart';

class _Transport implements KeyBarTransport {
  final stream = StreamController<KeyBarLayoutResult>.broadcast(sync: true);
  final sent = <List<List<String>>>[];
  VoidCallback? onSend;
  @override
  Stream<KeyBarLayoutResult> get results => stream.stream;
  @override
  void send(List<List<String>> rows) {
    expect(stream.hasListener, isTrue);
    sent.add(
      SaveKeyBarLayout.bincodeDeserialize(
        SaveKeyBarLayout(rows: rows).bincodeSerialize(),
      ).rows,
    );
    onSend?.call();
  }

  void reply(bool ok) => stream.add(
    KeyBarLayoutResult.bincodeDeserialize(
      KeyBarLayoutResult(
        ok: ok,
        detail: ok ? '' : 'fixture',
      ).bincodeSerialize(),
    ),
  );
}

class _Verifier implements KeyBarVerifier {
  final calls = <KeyBarSettlementToken>[];
  final completion = Completer<KeyBarVerificationProof?>();
  @override
  Future<KeyBarVerificationProof?> verify(KeyBarSettlementToken token) {
    calls.add(token);
    return completion.future;
  }
}

class _Fixture {
  _Fixture({List<List<String>>? rows, KeyBarVerifier? verifier}) {
    writer = KeyBarWriter(
      transport: transport,
      timeout: const Duration(seconds: 10),
      verifier: verifier,
      applyRows: applied.add,
    );
    draft = KeyBarDraft(rows ?? [[], []]);
    controller = KeyBarEditorController(draft: draft, writer: writer);
  }
  final transport = _Transport();
  final applied = <List<List<String>>>[];
  late final KeyBarWriter writer;
  late final KeyBarDraft draft;
  late final KeyBarEditorController controller;
  Future<void> close() async {
    controller.dispose();
    writer.dispose();
    draft.dispose();
    await transport.stream.close();
  }
}

Future<void> _mount(
  WidgetTester tester,
  _Fixture fixture, {
  double width = 1280,
  double scale = 1,
  bool highContrast = false,
  bool reduced = false,
  Future<void> Function(bool)? onEditingFocus,
}) async {
  tester.view.reset();
  tester.view.devicePixelRatio = 1;
  tester.view.physicalSize = Size(width, 900);
  await tester.pumpWidget(
    FluentApp(
      theme: desktopTheme(highContrast: highContrast, reduceMotion: reduced),
      builder: (context, child) => MediaQuery(
        data: MediaQuery.of(context).copyWith(
          textScaler: TextScaler.linear(scale),
          highContrast: highContrast,
          disableAnimations: reduced,
          accessibleNavigation: reduced,
        ),
        child: child!,
      ),
      home: DesktopKeyBarEditor(
        controller: fixture.controller,
        onEditingFocus: onEditingFocus,
      ),
    ),
  );
  await tester.pumpAndSettle();
}

Finder _key(String key) => find.byKey(ValueKey(key));
Finder _button(String key, {bool filled = false}) => find.descendant(
  of: _key(key),
  matching: find.byType(filled ? FilledButton : Button),
);

Future<void> _tap(WidgetTester tester, String key) async {
  await tester.ensureVisible(_key(key));
  await tester.tap(_key(key));
  await tester.pumpAndSettle();
}

Future<void> _dragTo(
  WidgetTester tester,
  Finder source,
  String target, {
  bool after = false,
  bool revealTarget = false,
}) async {
  await tester.ensureVisible(source);
  final start = tester.getCenter(source);
  final gesture = await tester.startGesture(
    start,
    kind: PointerDeviceKind.mouse,
  );
  await gesture.moveTo(start + const Offset(DesktopSpacing.sm, 0));
  await tester.pump();
  if (revealTarget) await tester.ensureVisible(_key(target));
  final rect = tester.getRect(_key(target));
  final end = Offset(
    after ? rect.right - DesktopSpacing.md : rect.left + DesktopSpacing.md,
    rect.center.dy,
  );
  await gesture.moveTo(end);
  await tester.pump();
  await gesture.up();
  await tester.pumpAndSettle();
}

void main() {
  group('编码与草稿', () {
    test('同排 gap 保留指定最终列，补 spacer 后的越界 move 不改变源排', () {
      final draft = KeyBarDraft([
        ['tab'],
        [],
      ]);
      addTearDown(draft.dispose);
      final entry = draft.entries[0].single;
      expect(draft.drop(KeyBarDrag.move(entry), 0, 23), isTrue);
      expect(draft.rows[0], [...List.filled(23, 'spacer'), 'tab']);
      expect(draft.entries[0][23], same(entry));
      expect(draft.selected, same(entry));
      final single = KeyBarDraft([
        ['tab'],
        [],
      ]);
      addTearDown(single.dispose);
      final source = single.entries[0].single;
      expect(single.canDrop(KeyBarDrag.move(source), 0, 24), isFalse);
      expect(single.drop(KeyBarDrag.move(source), 0, 24), isFalse);
      expect(single.rows, [
        ['tab'],
        [],
      ]);
      expect(single.entries[0].single, same(source));
    });
    test('稳定编码完整保留，空排及 spacer 可保存', () {
      final rows = [
        keyBarButtons.map((button) => button.id).take(24).toList(),
        <String>[],
      ];
      final draft = KeyBarDraft(rows);
      expect(draft.rows, rows);
      expect(draft.dirty, isFalse);
      expect(
        validKeyBarRows([
          ['spacer', r'text:$HOME'],
          [],
        ]),
        isTrue,
      );
      expect(
        keyBarButtons.every((button) => keyBarButton(button.id) != null),
        isTrue,
      );
      draft.dispose();
    });

    test('文本精确匹配 Rust Unicode 长度、空白与控制字符规则，不解析命令', () {
      for (final text in [
        '',
        ' \t',
        '\u2000\u3000',
        'a\n',
        'a\u007f',
        'a\u0085',
        'a\u001b',
        '界' * 33,
      ]) {
        expect(validKeyBarText(text), isFalse);
      }
      for (final text in [
        '界' * 32,
        '😀' * 32,
        r'$HOME; rm -rf /',
        ' a ',
        '\u200b',
      ]) {
        expect(validKeyBarText(text), isTrue);
        expect(
          KeyBarDraft([
            ['text:$text'],
            [],
          ]).rows[0].single,
          'text:$text',
        );
      }
    });

    test('非法形状或按钮不制造可保存草稿', () {
      for (final rows in <List<List<String>>>[
        <List<String>>[],
        [[]],
        [[], [], []],
        [
          ['unknown'],
          [],
        ],
        [List.filled(25, 'tab'), []],
      ]) {
        expect(() => KeyBarDraft(rows), throwsArgumentError);
      }
    });

    test('每次 copy 有独立身份，重复按钮跨排只移动指定副本', () {
      final draft = KeyBarDraft([[], []]);
      draft.drop(const KeyBarDrag.copy('tab'), 0, 0);
      final first = draft.selected!;
      draft.drop(const KeyBarDrag.copy('tab'), 0, 1);
      final second = draft.selected!;
      expect(identical(first, second), isFalse);
      draft.drop(KeyBarDrag.move(first), 1, 0);
      expect(draft.entries[0].single, same(second));
      expect(draft.entries[1].single, same(first));
      draft.dispose();
    });

    test('gap 之后真正 copy/move 才检查 24 上限，拒绝不改变源排', () {
      final draft = KeyBarDraft([
        ['tab'],
        [],
      ]);
      final entry = draft.entries[0].single;
      expect(draft.drop(const KeyBarDrag.copy('tab'), 1, 24), isFalse);
      expect(draft.drop(KeyBarDrag.move(entry), 1, 24), isFalse);
      expect(draft.rows, [
        ['tab'],
        [],
      ]);
      expect(draft.drop(KeyBarDrag.move(entry), 1, 23), isTrue);
      expect(draft.rows[0], isEmpty);
      expect(draft.rows[1], [...List.filled(23, 'spacer'), 'tab']);
      expect(draft.drop(const KeyBarDrag.copy('escape'), 1, 0), isFalse);
      draft.dispose();
    });

    test('满排同排排序合法，包括尾部插入与自身前后', () {
      final codes = keyBarButtons.take(24).map((button) => button.id).toList();
      final draft = KeyBarDraft([codes, []]);
      final entry = draft.entries[0].first;
      expect(draft.drop(KeyBarDrag.move(entry), 0, 24), isTrue);
      expect(draft.entries[0].last, same(entry));
      expect(draft.rows[0].length, 24);
      expect(draft.moveSelected(-1, 0), isTrue);
      expect(draft.entries[0][22], same(entry));
      expect(draft.drop(KeyBarDrag.move(entry), 0, 22), isTrue);
      expect(draft.drop(KeyBarDrag.move(entry), 0, 23), isTrue);
      expect(draft.rows[0].length, 24);
      draft.dispose();
    });

    test('恢复默认、删除与确认基线不覆盖保存期间的修改', () {
      final draft = KeyBarDraft([[], []]);
      draft.restoreDefaults();
      expect(draft.rows, defaultKeyBarRows);
      final submitted = draft.rows;
      draft.select(draft.entries[0].first, 0);
      draft.deleteSelected();
      draft.confirm(submitted);
      expect(draft.dirty, isTrue);
      expect(draft.rows[0].first, 'slash');
      expect(() => submitted[0].add('tab'), throwsUnsupportedError);
      draft.dispose();
    });
  });

  group('单写者与隔离', () {
    testWidgets('proof 同步所有连接 scope 与重新 attach 的基线，保留 identity/选择/顺序', (
      tester,
    ) async {
      final fixture = _Fixture();
      addTearDown(fixture.close);
      final connectedDraft = KeyBarDraft([
        ['escape', r'text:$keep'],
        ['spacer'],
      ]);
      final connected = KeyBarEditorController(
        draft: connectedDraft,
        writer: fixture.writer,
      );
      addTearDown(connectedDraft.dispose);
      addTearDown(connected.dispose);
      final selected = connectedDraft.entries[0][1];
      connectedDraft.select(selected, 0);
      final originalEntries = connectedDraft.entries;
      final retainedDraft = KeyBarDraft([
        ['copy', 'paste'],
        [],
      ]);
      final closed = KeyBarEditorController(
        draft: retainedDraft,
        writer: fixture.writer,
      );
      final retainedSelected = retainedDraft.entries[0].last;
      retainedDraft.select(retainedSelected, 0);
      final retainedEntries = retainedDraft.entries;
      closed.dispose();
      addTearDown(retainedDraft.dispose);
      final foreign = _Fixture(
        rows: [
          ['escape'],
          [],
        ],
      );
      addTearDown(foreign.close);
      fixture.draft.drop(const KeyBarDrag.copy('tab'), 0, 0);
      final save = fixture.controller.save();
      await tester.pump(const Duration(seconds: 11));
      await save;
      fixture.transport.reply(true);
      expect(connectedDraft.dirty, isFalse);
      expect(retainedDraft.dirty, isFalse);
      expect(connected.outcome, isNull);
      expect(
        fixture.writer.acceptProof(
          KeyBarVerificationProof.afterSettlement(
            token: fixture.writer.settlement!,
            rows: [
              ['tab'],
              [],
            ],
            evidence: Object(),
          ),
        ),
        isTrue,
      );
      expect(connectedDraft.dirty, isTrue);
      expect(connected.canSave, isTrue);
      expect(connectedDraft.entries, originalEntries);
      expect(connectedDraft.selected, same(selected));
      expect(connectedDraft.activeRow, 0);
      expect(connected.outcome, isNull);
      expect(retainedDraft.dirty, isFalse);
      final reopened = KeyBarEditorController(
        draft: retainedDraft,
        writer: fixture.writer,
      );
      addTearDown(reopened.dispose);
      expect(retainedDraft.dirty, isTrue);
      expect(reopened.canSave, isTrue);
      expect(retainedDraft.entries, retainedEntries);
      expect(retainedDraft.selected, same(retainedSelected));
      expect(foreign.draft.dirty, isFalse);
      connectedDraft.select(null, 1);
      expect(connectedDraft.entries, originalEntries);
      expect(fixture.draft.dirty, isFalse);
    });

    testWidgets('普通保存仅确认自身提交，重新 attach 不使用过期 proof 覆盖真实 query 起点', (
      tester,
    ) async {
      final fixture = _Fixture();
      addTearDown(fixture.close);
      fixture.draft.drop(const KeyBarDrag.copy('tab'), 0, 0);
      final uncertain = fixture.controller.save();
      fixture.draft.deleteSelected();
      await tester.pump(const Duration(seconds: 11));
      await uncertain;
      fixture.transport.reply(true);
      final proof = KeyBarVerificationProof.afterSettlement(
        token: fixture.writer.settlement!,
        rows: [
          ['tab'],
          [],
        ],
        evidence: Object(),
      );
      expect(fixture.writer.acceptProof(proof), isTrue);
      final saveEmpty = fixture.controller.save();
      fixture.transport.reply(true);
      fixture.draft.drop(const KeyBarDrag.copy('escape'), 1, 0);
      await saveEmpty;
      expect(fixture.draft.dirty, isTrue);
      expect(fixture.draft.rows, [
        [],
        ['escape'],
      ]);
      fixture.controller.dispose();
      final reopened = KeyBarEditorController(
        draft: fixture.draft,
        writer: fixture.writer,
      );
      addTearDown(reopened.dispose);
      expect(fixture.draft.dirty, isTrue);
      fixture.draft.deleteSelected();
      expect(fixture.draft.dirty, isFalse);
      final queried = KeyBarDraft([
        ['copy'],
        [],
      ]);
      final queriedController = KeyBarEditorController(
        draft: queried,
        writer: fixture.writer,
      );
      addTearDown(queried.dispose);
      addTearDown(queriedController.dispose);
      expect(queried.dirty, isFalse);
      expect(fixture.writer.acceptProof(proof), isFalse);
      expect(queried.dirty, isFalse);
    });
    testWidgets('核实实际 tab 基线后，保存期间删回空排仍 dirty 且可保存', (tester) async {
      final verifier = _Verifier();
      final fixture = _Fixture(verifier: verifier);
      addTearDown(fixture.close);
      fixture.draft.drop(const KeyBarDrag.copy('tab'), 0, 0);
      final save = fixture.controller.save();
      fixture.draft.deleteSelected();
      expect(fixture.draft.rows, [[], []]);
      expect(fixture.draft.dirty, isFalse);
      await tester.pump(const Duration(seconds: 11));
      expect((await save).status, KeyBarSaveStatus.uncertain);
      fixture.transport.reply(true);
      expect(fixture.draft.dirty, isFalse);
      expect(fixture.controller.canSave, isFalse);
      final verification = fixture.writer.verify();
      verifier.completion.complete(
        KeyBarVerificationProof.afterSettlement(
          token: verifier.calls.single,
          rows: [
            ['tab'],
            [],
          ],
          evidence: Object(),
        ),
      );
      expect(await verification, isTrue);
      expect(fixture.draft.rows, [[], []]);
      expect(fixture.draft.dirty, isTrue);
      expect(fixture.controller.canSave, isTrue);
      final emptySave = fixture.controller.save();
      expect(fixture.transport.sent.last, [[], []]);
      fixture.transport.reply(true);
      expect((await emptySave).status, KeyBarSaveStatus.saved);
      expect(fixture.draft.dirty, isFalse);
    });
    test('发送异常保留未决归属，不将异常冒充明确 backend error', () async {
      final fixture = _Fixture();
      fixture.draft.drop(const KeyBarDrag.copy('tab'), 0, 0);
      fixture.transport.onSend = () => throw StateError('fixture');
      expect(
        (await fixture.controller.save()).status,
        KeyBarSaveStatus.uncertain,
      );
      expect(fixture.writer.pending, isNotNull);
      expect(fixture.writer.state, KeyBarWriteState.quarantined);
      fixture.transport.reply(false);
      expect(fixture.writer.state, KeyBarWriteState.needsVerify);
      expect(fixture.draft.dirty, isTrue);
      await fixture.close();
    });
    test('先订阅后 beforeSend，同步结果安全，发送快照不可变', () async {
      final fixture = _Fixture();
      fixture.draft.drop(const KeyBarDrag.copy('tab'), 0, 0);
      fixture.writer.beforeSend = () =>
          expect(fixture.transport.stream.hasListener, isTrue);
      fixture.transport.onSend = () => fixture.transport.reply(true);
      final save = fixture.controller.save();
      fixture.draft.drop(const KeyBarDrag.copy('escape'), 1, 0);
      expect((await save).status, KeyBarSaveStatus.saved);
      expect(fixture.applied.single, [
        ['tab'],
        [],
      ]);
      expect(fixture.draft.rows, [
        ['tab'],
        ['escape'],
      ]);
      expect(fixture.draft.dirty, isTrue);
      expect(
        () => fixture.writer.confirmedRows![0].clear(),
        throwsUnsupportedError,
      );
      await fixture.close();
    });

    test('相同 transport 不允许 competing listener', () async {
      final fixture = _Fixture();
      expect(
        () => KeyBarWriter(transport: fixture.transport),
        throwsStateError,
      );
      await fixture.close();
    });

    test('明确失败已 settled，可重试，精确草稿与 identity 保留', () async {
      final fixture = _Fixture();
      fixture.draft.drop(const KeyBarDrag.copy(r'text:$ a  '), 0, 0);
      final entry = fixture.draft.entries[0].single;
      final first = fixture.controller.save();
      final identity = fixture.writer.pending;
      fixture.transport.reply(false);
      expect((await first).status, KeyBarSaveStatus.rejected);
      expect(fixture.writer.state, KeyBarWriteState.ready);
      expect(fixture.writer.quarantine, isNull);
      expect(fixture.draft.entries[0].single, same(entry));
      expect(fixture.draft.rows, [
        [r'text:$ a  '],
        [],
      ]);
      final retry = fixture.controller.save();
      expect(fixture.writer.pending, isNot(same(identity)));
      fixture.transport.reply(true);
      expect((await retry).status, KeyBarSaveStatus.saved);
      expect(fixture.draft.dirty, isFalse);
      await fixture.close();
    });

    test('全应用最多一 write，多个草稿确认不串 scope', () async {
      final fixture = _Fixture();
      final secondDraft = KeyBarDraft([
        ['escape'],
        [],
      ]);
      secondDraft.drop(const KeyBarDrag.copy('tab'), 1, 0);
      final second = KeyBarEditorController(
        draft: secondDraft,
        writer: fixture.writer,
      );
      fixture.draft.drop(const KeyBarDrag.copy('tab'), 0, 0);
      final firstSave = fixture.controller.save();
      expect((await second.save()).status, KeyBarSaveStatus.blocked);
      expect(fixture.transport.sent.length, 1);
      fixture.transport.reply(true);
      await firstSave;
      expect(secondDraft.dirty, isTrue);
      final secondSave = second.save();
      fixture.transport.reply(true);
      await secondSave;
      expect(fixture.applied.last, [
        ['escape'],
        ['tab'],
      ]);
      second.dispose();
      secondDraft.dispose();
      await fixture.close();
    });

    testWidgets('timeout 保留 pending，迟到 ok 只归旧 write，无 proof 不重发', (
      tester,
    ) async {
      final fixture = _Fixture();
      fixture.draft.drop(const KeyBarDrag.copy('tab'), 0, 0);
      final save = fixture.controller.save();
      final identity = fixture.writer.pending;
      await tester.pump(const Duration(seconds: 11));
      expect((await save).status, KeyBarSaveStatus.uncertain);
      expect(fixture.writer.pending, same(identity));
      fixture.draft.drop(const KeyBarDrag.copy('escape'), 1, 0);
      expect(
        (await fixture.controller.save()).status,
        KeyBarSaveStatus.blocked,
      );
      expect(await fixture.writer.verify(), isFalse);
      fixture.transport.reply(true);
      expect(fixture.writer.settlement!.quarantine.write, same(identity));
      expect(fixture.writer.state, KeyBarWriteState.needsVerify);
      expect(await fixture.writer.verify(), isFalse);
      expect(fixture.applied, isEmpty);
      expect(fixture.draft.dirty, isTrue);
      expect(fixture.transport.sent.length, 1);
      await fixture.close();
    });

    testWidgets('关闭只 detach，重开/新草稿不清隔离，晚回包不确认新编辑', (tester) async {
      final fixture = _Fixture();
      fixture.draft.drop(const KeyBarDrag.copy('tab'), 0, 0);
      final save = fixture.controller.save();
      fixture.controller.dispose();
      expect((await save).status, KeyBarSaveStatus.uncertain);
      final reopened = KeyBarEditorController(
        draft: fixture.draft,
        writer: fixture.writer,
      );
      fixture.draft.drop(const KeyBarDrag.copy('escape'), 0, 1);
      fixture.transport.reply(true);
      expect(fixture.draft.dirty, isTrue);
      expect(fixture.draft.rows, [
        ['tab', 'escape'],
        [],
      ]);
      expect((await reopened.save()).status, KeyBarSaveStatus.blocked);
      expect(fixture.applied, isEmpty);
      reopened.dispose();
      await fixture.close();
    });

    testWidgets('verifier 必须晚于旧回包消耗，proof 只更新实际排布不覆盖草稿', (tester) async {
      final verifier = _Verifier();
      final fixture = _Fixture(verifier: verifier);
      fixture.draft.drop(const KeyBarDrag.copy('tab'), 0, 0);
      final save = fixture.controller.save();
      await tester.pump(const Duration(seconds: 11));
      await save;
      expect(await fixture.writer.verify(), isFalse);
      expect(verifier.calls, isEmpty);
      fixture.transport.reply(false);
      final verification = fixture.writer.verify();
      final token = verifier.calls.single;
      fixture.draft.drop(const KeyBarDrag.copy('escape'), 1, 0);
      expect(fixture.writer.canSave, isFalse);
      verifier.completion.complete(
        KeyBarVerificationProof.afterSettlement(
          token: token,
          rows: [[], []],
          evidence: Object(),
        ),
      );
      expect(await verification, isTrue);
      expect(fixture.applied.single, [[], []]);
      expect(fixture.draft.rows, [
        ['tab'],
        ['escape'],
      ]);
      expect(fixture.draft.dirty, isTrue);
      expect(fixture.controller.canSave, isTrue);
      final next = fixture.controller.save();
      fixture.transport.reply(true);
      expect((await next).status, KeyBarSaveStatus.saved);
      await fixture.close();
    });

    testWidgets('核实失败可再核实，不制造成功或清除隔离', (tester) async {
      final verifier = _Verifier();
      final fixture = _Fixture(verifier: verifier);
      fixture.draft.drop(const KeyBarDrag.copy('tab'), 0, 0);
      final save = fixture.controller.save();
      await tester.pump(const Duration(seconds: 11));
      await save;
      fixture.transport.reply(true);
      final verify = fixture.writer.verify();
      verifier.completion.complete(null);
      expect(await verify, isFalse);
      expect(fixture.writer.state, KeyBarWriteState.needsVerify);
      expect(fixture.writer.settlement, same(verifier.calls.single));
      expect(fixture.applied, isEmpty);
      await fixture.close();
    });

    testWidgets('只有归属匹配的已确认 restart+query proof 可清未回包隔离', (tester) async {
      final fixture = _Fixture();
      final foreign = _Fixture();
      for (final value in [fixture, foreign]) {
        value.draft.drop(const KeyBarDrag.copy('tab'), 0, 0);
        unawaited(value.controller.save());
      }
      await tester.pump(const Duration(seconds: 11));
      expect(
        fixture.writer.acceptProof(
          KeyBarVerificationProof.afterBackendRestart(
            token: foreign.writer.quarantine!,
            rows: [[], []],
            evidence: Object(),
          ),
        ),
        isFalse,
      );
      expect(
        fixture.writer.acceptProof(
          KeyBarVerificationProof.afterBackendRestart(
            token: fixture.writer.quarantine!,
            rows: [[], []],
            evidence: Object(),
          ),
        ),
        isTrue,
      );
      expect(fixture.writer.pending, isNull);
      expect(fixture.controller.canSave, isTrue);
      expect(fixture.draft.dirty, isTrue);
      await foreign.close();
      await fixture.close();
    });

    test('实际 Rinf receive hook 与生成 DTO adapter；不宣称 Rust/SQLite 往返', () async {
      final applied = <List<List<String>>>[];
      final writer = KeyBarWriter(
        transport: RinfKeyBarTransport.instance,
        applyRows: applied.add,
      );
      // 使用生成通道接收；发送委托由 controlled transport 单独覆盖。
      final signal = Completer<KeyBarLayoutResult>();
      final listener = RinfKeyBarTransport.instance.results.listen(
        signal.complete,
      );
      assignRustSignal['KeyBarLayoutResult']!(
        const KeyBarLayoutResult(ok: true, detail: '').bincodeSerialize(),
        Uint8List(0),
      );
      expect((await signal.future).ok, isTrue);
      expect(writer.state, KeyBarWriteState.ready);
      expect(applied, isEmpty);
      await listener.cancel();
      writer.dispose();
    });
  });

  group('Fluent 实际交互', () {
    testWidgets('Widget 空草稿在 tab proof 后可保存，不回填已删除按钮', (tester) async {
      final verifier = _Verifier();
      final fixture = _Fixture(verifier: verifier);
      addTearDown(fixture.close);
      await _mount(tester, fixture);
      await _tap(tester, 'keybar-palette-tab');
      await _tap(tester, 'keybar-save');
      await _tap(tester, 'keybar-delete');
      expect(fixture.draft.dirty, isFalse);
      await tester.pump(const Duration(seconds: 11));
      fixture.transport.reply(true);
      await tester.pumpAndSettle();
      expect(
        tester
            .widget<FilledButton>(_button('keybar-save', filled: true))
            .onPressed,
        isNull,
      );
      await _tap(tester, 'keybar-verify');
      verifier.completion.complete(
        KeyBarVerificationProof.afterSettlement(
          token: verifier.calls.single,
          rows: [
            ['tab'],
            [],
          ],
          evidence: Object(),
        ),
      );
      await tester.pumpAndSettle();
      expect(fixture.draft.rows, [[], []]);
      expect(fixture.draft.dirty, isTrue);
      expect(
        tester
            .widget<FilledButton>(_button('keybar-save', filled: true))
            .onPressed,
        isNotNull,
      );
      await _tap(tester, 'keybar-save');
      expect(fixture.transport.sent.last, [[], []]);
      fixture.transport.reply(true);
      await tester.pumpAndSettle();
      expect(fixture.draft.dirty, isFalse);
      await tester.pumpWidget(const SizedBox.shrink());
    });

    testWidgets('Widget 同排拖到空列 24 精确保留 23 spacer 与原按钮身份', (tester) async {
      final fixture = _Fixture(
        rows: [
          ['tab'],
          List.filled(24, 'spacer'),
        ],
      );
      addTearDown(fixture.close);
      await _mount(tester, fixture);
      final entry = fixture.draft.entries[0].single;
      final otherRow = fixture.draft.entries[1];
      await _dragTo(
        tester,
        find.byKey(ObjectKey(entry)),
        'keybar-cell-0-23',
        revealTarget: true,
      );
      expect(fixture.draft.rows[0], [...List.filled(23, 'spacer'), 'tab']);
      expect(fixture.draft.entries[0][23], same(entry));
      expect(fixture.draft.selected, same(entry));
      expect(fixture.draft.entries[1], otherRow);
      expect(fixture.applied, isEmpty);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    });
    testWidgets('Widget 拆装与 controller 关闭保留草稿，reopen 不清 pending quarantine', (
      tester,
    ) async {
      final fixture = _Fixture();
      await _mount(tester, fixture);
      await _tap(tester, 'keybar-palette-tab');
      await _tap(tester, 'keybar-save');
      final identity = fixture.writer.pending;
      await tester.pumpWidget(const SizedBox.shrink());
      fixture.controller.dispose();
      final reopened = KeyBarEditorController(
        draft: fixture.draft,
        writer: fixture.writer,
      );
      await tester.pumpWidget(
        FluentApp(
          theme: desktopTheme(highContrast: false, reduceMotion: false),
          home: DesktopKeyBarEditor(controller: reopened),
        ),
      );
      await tester.pumpAndSettle();
      expect(fixture.writer.pending, same(identity));
      await _tap(tester, 'keybar-palette-escape');
      fixture.transport.reply(true);
      await tester.pumpAndSettle();
      expect(fixture.draft.rows, [
        ['tab', 'escape'],
        [],
      ]);
      expect(fixture.draft.dirty, isTrue);
      expect(
        tester
            .widget<FilledButton>(_button('keybar-save', filled: true))
            .onPressed,
        isNull,
      );
      expect(fixture.applied, isEmpty);
      await tester.pumpWidget(const SizedBox.shrink());
      reopened.dispose();
      await fixture.close();
    });
    testWidgets('仅 Tab/Enter 选择文本按钮并打开输入窗口，方向键不被按钮吞掉', (tester) async {
      final fixture = _Fixture(
        rows: [
          ['text:原文', 'tab'],
          [],
        ],
      );
      await _mount(tester, fixture);
      final entry = fixture.draft.entries[0].first;
      for (
        var i = 0;
        i < 12 && !identical(fixture.draft.selected, entry);
        i++
      ) {
        await tester.sendKeyEvent(LogicalKeyboardKey.tab);
        await tester.pumpAndSettle();
      }
      expect(fixture.draft.selected, same(entry));
      await tester.sendKeyEvent(LogicalKeyboardKey.arrowRight);
      await tester.pumpAndSettle();
      expect(fixture.draft.rows[0], ['tab', 'text:原文']);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pumpAndSettle();
      expect(_key('keybar-text-input'), findsOneWidget);
      await tester.enterText(_key('keybar-text-input'), r'$literal');
      await tester.testTextInput.receiveAction(TextInputAction.done);
      await tester.pumpAndSettle();
      expect(fixture.draft.rows[0], ['tab', r'text:$literal']);
      await tester.pumpWidget(const SizedBox.shrink());
      await fixture.close();
    });

    testWidgets('真实 Widget 超时可继续编辑但禁重发，late result 后只有 explicit verifier 解除', (
      tester,
    ) async {
      final verifier = _Verifier();
      final fixture = _Fixture(verifier: verifier);
      await _mount(tester, fixture);
      await _tap(tester, 'keybar-palette-tab');
      await _tap(tester, 'keybar-save');
      await tester.pump(const Duration(seconds: 11));
      expect(fixture.writer.state, KeyBarWriteState.quarantined);
      expect(
        tester
            .widget<FilledButton>(_button('keybar-save', filled: true))
            .onPressed,
        isNull,
      );
      await _tap(tester, 'keybar-palette-escape');
      expect(fixture.draft.rows, [
        ['tab', 'escape'],
        [],
      ]);
      fixture.transport.reply(true);
      await tester.pumpAndSettle();
      expect(fixture.writer.state, KeyBarWriteState.needsVerify);
      expect(fixture.applied, isEmpty);
      await _tap(tester, 'keybar-verify');
      expect(verifier.calls.length, 1);
      verifier.completion.complete(
        KeyBarVerificationProof.afterSettlement(
          token: verifier.calls.single,
          rows: [
            ['tab'],
            [],
          ],
          evidence: Object(),
        ),
      );
      await tester.pumpAndSettle();
      expect(fixture.draft.rows, [
        ['tab', 'escape'],
        [],
      ]);
      expect(fixture.controller.canSave, isTrue);
      await _tap(tester, 'keybar-save');
      fixture.transport.reply(true);
      await tester.pumpAndSettle();
      expect(fixture.draft.dirty, isFalse);
      expect(fixture.applied.last, [
        ['tab', 'escape'],
        [],
      ]);
      await tester.pumpWidget(const SizedBox.shrink());
      await fixture.close();
    });

    testWidgets('实际跨排拖动/复制拒绝满排，gap 到最后一列后精确计数 24，拖入删除', (tester) async {
      final fixture = _Fixture(
        rows: [
          List.filled(24, 'tab'),
          ['escape'],
        ],
      );
      await _mount(tester, fixture);
      final source = fixture.draft.entries[1].single;
      await _dragTo(tester, find.byKey(ObjectKey(source)), 'keybar-cell-0-1');
      expect(fixture.draft.entries[1].single, same(source));
      expect(fixture.draft.rows[0].length, 24);
      await _dragTo(tester, _key('keybar-palette-up'), 'keybar-cell-0-1');
      expect(fixture.draft.rows[0].length, 24);
      final moving = fixture.draft.entries[0].first;
      await _dragTo(
        tester,
        find.byKey(ObjectKey(moving)),
        'keybar-cell-1-23',
        revealTarget: true,
      );
      expect(fixture.draft.rows[0].length, 23);
      expect(fixture.draft.rows[1], [
        'escape',
        ...List.filled(22, 'spacer'),
        'tab',
      ]);
      await _dragTo(tester, find.byKey(ObjectKey(moving)), 'keybar-delete');
      expect(fixture.draft.rows[1].length, 23);
      expect(fixture.applied, isEmpty);
      await tester.pumpWidget(const SizedBox.shrink());
      await fixture.close();
    });
    testWidgets('按钮 palette/两排/重复/删除/default 与明确保存结果', (tester) async {
      final fixture = _Fixture();
      await _mount(tester, fixture);
      await _tap(tester, 'keybar-palette-tab');
      final first = fixture.draft.entries[0].single;
      await _tap(tester, 'keybar-palette-tab');
      expect(fixture.draft.entries[0].last, isNot(same(first)));
      await _tap(tester, 'keybar-row-1');
      await _tap(tester, 'keybar-palette-spacer');
      expect(fixture.draft.rows, [
        ['tab', 'tab'],
        ['spacer'],
      ]);
      await _tap(tester, 'keybar-delete');
      expect(fixture.draft.rows[1], isEmpty);
      await _tap(tester, 'keybar-defaults');
      expect(fixture.draft.rows, defaultKeyBarRows);
      await _tap(tester, 'keybar-save');
      expect(fixture.writer.state, KeyBarWriteState.pending);
      expect(fixture.applied, isEmpty);
      fixture.transport.reply(false);
      await tester.pumpAndSettle();
      expect(fixture.controller.canSave, isTrue);
      await _tap(tester, 'keybar-save');
      fixture.transport.reply(true);
      await tester.pumpAndSettle();
      expect(fixture.applied.single, defaultKeyBarRows);
      expect(fixture.draft.dirty, isFalse);
      await tester.pumpWidget(const SizedBox.shrink());
      await fixture.close();
    });

    testWidgets('实际拖放 copy 到 gap，重复跨排与同排排序，不提前 apply', (tester) async {
      final fixture = _Fixture(
        rows: [
          ['tab', 'tab', 'escape'],
          [],
        ],
      );
      await _mount(tester, fixture);
      final first = fixture.draft.entries[0].first;
      final second = fixture.draft.entries[0][1];
      await _dragTo(tester, find.byKey(ObjectKey(first)), 'keybar-cell-1-2');
      expect(fixture.draft.rows, [
        ['tab', 'escape'],
        ['spacer', 'spacer', 'tab'],
      ]);
      expect(fixture.draft.entries[0].first, same(second));
      await _dragTo(
        tester,
        find.byKey(ObjectKey(second)),
        'keybar-cell-0-1',
        after: true,
      );
      expect(fixture.draft.rows[0], ['escape', 'tab']);
      await _dragTo(tester, _key('keybar-palette-up'), 'keybar-cell-0-2');
      expect(fixture.draft.rows[0], ['escape', 'tab', 'up']);
      expect(fixture.applied, isEmpty);
      await tester.pumpWidget(const SizedBox.shrink());
      await fixture.close();
    });

    testWidgets('键盘选择/四向移动/删除/escape/Tab 可用', (tester) async {
      final fixture = _Fixture(
        rows: [
          ['escape', 'tab'],
          [],
        ],
      );
      await _mount(tester, fixture);
      await tester.tap(find.byKey(ObjectKey(fixture.draft.entries[0].first)));
      await tester.pumpAndSettle();
      await tester.sendKeyEvent(LogicalKeyboardKey.arrowRight);
      await tester.pumpAndSettle();
      expect(fixture.draft.rows[0], ['tab', 'escape']);
      await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
      await tester.pumpAndSettle();
      expect(fixture.draft.rows[1], ['spacer', 'escape']);
      await tester.sendKeyEvent(LogicalKeyboardKey.arrowLeft);
      await tester.sendKeyEvent(LogicalKeyboardKey.arrowUp);
      await tester.pumpAndSettle();
      expect(fixture.draft.rows, [
        ['escape', 'tab'],
        ['spacer'],
      ]);
      await tester.sendKeyEvent(LogicalKeyboardKey.delete);
      await tester.pumpAndSettle();
      expect(fixture.draft.rows[0], ['tab']);
      await tester.tap(find.byKey(ObjectKey(fixture.draft.entries[0].first)));
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
      expect(fixture.draft.selected, isNull);
      await tester.sendKeyEvent(LogicalKeyboardKey.tab);
      await tester.pumpAndSettle();
      expect(FocusManager.instance.primaryFocus, isNotNull);
      await tester.pumpWidget(const SizedBox.shrink());
      await fixture.close();
    });

    testWidgets('独立输入边界先完成才创建 TextBox，Enter 文本编辑不解析数据', (tester) async {
      final fixture = _Fixture(
        rows: [
          [r'text:$OLD'],
          [],
        ],
      );
      final entered = Completer<void>();
      final events = <bool>[];
      await _mount(
        tester,
        fixture,
        onEditingFocus: (active) {
          events.add(active);
          return active ? entered.future : Future.value();
        },
      );
      await tester.tap(find.byKey(ObjectKey(fixture.draft.entries[0].single)));
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pump();
      expect(events, [true]);
      expect(_key('keybar-text-input'), findsNothing);
      entered.complete();
      await tester.pumpAndSettle();
      await tester.enterText(_key('keybar-text-input'), r'$HOME; literal  ');
      await tester.testTextInput.receiveAction(TextInputAction.done);
      await tester.pumpAndSettle();
      expect(fixture.draft.rows[0], [r'text:$HOME; literal  ']);
      expect(events, [true, false]);
      expect(fixture.applied, isEmpty);
      await tester.pumpWidget(const SizedBox.shrink());
      await fixture.close();
    });

    testWidgets('文本取消保留身份，超长 Unicode 不截断，不允许确认', (tester) async {
      final fixture = _Fixture(
        rows: [
          ['text:原文'],
          [],
        ],
      );
      await _mount(tester, fixture);
      final entry = fixture.draft.entries[0].single;
      await tester.tap(find.byKey(ObjectKey(entry)));
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pumpAndSettle();
      await tester.enterText(_key('keybar-text-input'), '界' * 33);
      await tester.pumpAndSettle();
      expect(
        tester
            .widget<FilledButton>(_button('keybar-text-confirm', filled: true))
            .onPressed,
        isNull,
      );
      expect(
        tester
            .widget<TextBox>(_key('keybar-text-input'))
            .controller!
            .text
            .runes
            .length,
        33,
      );
      await _tap(tester, 'keybar-text-cancel');
      expect(fixture.draft.entries[0].single, same(entry));
      expect(fixture.draft.dirty, isFalse);
      await _tap(tester, 'keybar-custom-text');
      await tester.enterText(_key('keybar-text-input'), '😀' * 32);
      await tester.pumpAndSettle();
      expect(
        tester
            .widget<FilledButton>(_button('keybar-text-confirm', filled: true))
            .onPressed,
        isNotNull,
      );
      await _tap(tester, 'keybar-text-confirm');
      expect(fixture.draft.rows[0].last.runes.length, 37);
      await tester.pumpWidget(const SizedBox.shrink());
      await fixture.close();
    });

    testWidgets('满排 Widget palette 禁用但实际内部拖动排序可用', (tester) async {
      final codes = keyBarButtons.take(24).map((button) => button.id).toList();
      final fixture = _Fixture(rows: [codes, []]);
      await _mount(tester, fixture);
      expect(
        tester.widget<Button>(_button('keybar-palette-tab')).onPressed,
        isNull,
      );
      final first = fixture.draft.entries[0].first;
      await _dragTo(
        tester,
        find.byKey(ObjectKey(first)),
        'keybar-cell-0-2',
        after: true,
      );
      expect(fixture.draft.entries[0][2], same(first));
      expect(fixture.draft.rows[0].length, 24);
      expect(fixture.applied, isEmpty);
      await tester.pumpWidget(const SizedBox.shrink());
      await fixture.close();
    });

    for (final width in [375.0, 768.0, 1280.0]) {
      for (final scale in [1.0, 2.0]) {
        testWidgets('$width 宽 $scale 文字缩放/高对比/减动画，两排同步滚动与语义无溢出', (
          tester,
        ) async {
          final fixture = _Fixture(
            rows: [List.filled(24, 'tab'), List.filled(24, 'escape')],
          );
          final semantics = tester.ensureSemantics();
          await _mount(
            tester,
            fixture,
            width: width,
            scale: scale,
            highContrast: scale > 1,
            reduced: scale > 1,
          );
          final top = _key('keybar-cell-0-0');
          final bottom = _key('keybar-cell-1-0');
          final beforeTop = tester.getTopLeft(top).dx;
          final beforeBottom = tester.getTopLeft(bottom).dx;
          final horizontal = find.byWidgetPredicate(
            (widget) =>
                widget is SingleChildScrollView &&
                widget.scrollDirection == Axis.horizontal,
          );
          final viewport = tester.getRect(horizontal);
          await tester.dragFrom(
            Offset(viewport.center.dx, viewport.bottom - DesktopSpacing.xs),
            const Offset(-300, 0),
          );
          await tester.pumpAndSettle();
          final deltaTop = tester.getTopLeft(top).dx - beforeTop;
          final deltaBottom = tester.getTopLeft(bottom).dx - beforeBottom;
          expect(deltaTop, lessThan(0));
          expect(deltaTop, closeTo(deltaBottom, 0.01));
          expect(tester.takeException(), isNull);
          expect(find.byType(ToggleButton), findsNWidgets(2));
          await _tap(tester, 'keybar-row-1');
          await _tap(tester, 'keybar-defaults');
          expect(fixture.draft.rows, defaultKeyBarRows);
          await _tap(tester, 'keybar-save');
          expect(
            tester.getSemantics(_key('keybar-save')).flagsCollection.isButton,
            isTrue,
          );
          fixture.transport.reply(false);
          await tester.pumpAndSettle();
          expect(fixture.controller.canSave, isTrue);
          await _tap(tester, 'keybar-custom-text');
          expect(_key('keybar-text-input'), findsOneWidget);
          await tester.enterText(_key('keybar-text-input'), '界' * 32);
          await tester.pumpAndSettle();
          expect(tester.takeException(), isNull);
          await _tap(tester, 'keybar-text-confirm');
          expect(fixture.draft.rows[1].last, 'text:${'界' * 32}');
          expect(tester.takeException(), isNull);
          semantics.dispose();
          await tester.pumpWidget(const SizedBox.shrink());
          await fixture.close();
          tester.view.resetPhysicalSize();
          tester.view.resetDevicePixelRatio();
        });
      }
    }

    test('实际桌面 token 对比：正文 AAA，focus 与边界 AA', () {
      double channel(double value) => value <= 0.04045
          ? value / 12.92
          : math.pow((value + 0.055) / 1.055, 2.4).toDouble();
      double luminance(Color color) =>
          channel(color.r) * 0.2126 +
          channel(color.g) * 0.7152 +
          channel(color.b) * 0.0722;
      double contrast(Color a, Color b) {
        final x = luminance(a), y = luminance(b);
        return (math.max(x, y) + 0.05) / (math.min(x, y) + 0.05);
      }

      expect(
        contrast(DesktopColors.text, DesktopColors.surface),
        greaterThanOrEqualTo(7),
      );
      expect(
        contrast(DesktopColors.muted, DesktopColors.surface),
        greaterThanOrEqualTo(7),
      );
      expect(
        contrast(DesktopColors.accent, DesktopColors.surface),
        greaterThanOrEqualTo(3),
      );
      expect(
        contrast(DesktopColors.border, DesktopColors.surface),
        greaterThanOrEqualTo(3),
      );
      expect(
        contrast(DesktopColors.onAccent, DesktopColors.accent),
        greaterThanOrEqualTo(7),
      );
    });
  });
}
