import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/settings/key_bar_editor.dart';
import 'package:guosh_shell/src/terminal/key_bar_layout.dart';
import 'package:guosh_shell/src/terminal/key_bar_visuals.dart';

Finder cell(int row, int col) => find.byKey(ValueKey('preview-$row-$col'));
Finder palette(String id) => find.byKey(ValueKey('palette-$id'));

Future<void> dragTo(
  WidgetTester tester,
  Finder from,
  Finder to, {
  double dx = 0,
}) async {
  final gesture = await tester.startGesture(tester.getCenter(from));
  await gesture.moveBy(const Offset(0, 12));
  await tester.pump();
  await gesture.moveTo(tester.getCenter(to) + Offset(dx, 0));
  await tester.pump();
  await gesture.up();
  await tester.pumpAndSettle();
}

void main() {
  testWidgets('预览可跨排拖动、从目录拖入和拖出删除，保存不修改原数据', (tester) async {
    const initial = [
      ['escape', 'tab'],
      ['copy'],
    ];
    List<List<String>>? saved;
    await tester.pumpWidget(
      MaterialApp(
        home: KeyBarEditor(
          initialRows: initial,
          onSave: (rows) async => saved = rows,
        ),
      ),
    );
    await dragTo(tester, cell(0, 1), cell(1, 1));
    await dragTo(tester, palette('f1'), cell(0, 1));
    await dragTo(
      tester,
      cell(0, 0),
      find.byKey(const ValueKey('keybar-delete-target')),
    );
    await tester.tap(find.text('保存'));
    await tester.pumpAndSettle();
    expect(saved, [
      ['f1'],
      ['copy', 'tab'],
    ]);
    expect(initial, [
      ['escape', 'tab'],
      ['copy'],
    ]);
  });

  testWidgets('同排拖动可放到前后半区，重复按钮有独立身份', (tester) async {
    List<List<String>>? saved;
    await tester.pumpWidget(
      MaterialApp(
        home: KeyBarEditor(
          initialRows: const [
            ['escape', 'tab', 'tab'],
            [],
          ],
          onSave: (rows) async => saved = rows,
        ),
      ),
    );
    await dragTo(tester, cell(0, 0), cell(0, 2), dx: 12);
    await tester.tap(find.text('保存'));
    await tester.pumpAndSettle();
    expect(saved, [
      ['tab', 'tab', 'escape'],
      [],
    ]);
  });

  testWidgets('两排空时可选择第二排添加，点击选中后删除', (tester) async {
    List<List<String>>? saved;
    await tester.pumpWidget(
      MaterialApp(
        home: KeyBarEditor(
          initialRows: const [[], []],
          onSave: (rows) async => saved = rows,
        ),
      ),
    );
    await tester.tap(cell(1, 0));
    await tester.tap(palette('f1'));
    await tester.pump();
    await tester.tap(palette('f2'));
    await tester.pump();
    await tester.tap(cell(1, 0));
    await tester.pump();
    await tester.tap(find.text('删除所选按钮'));
    await tester.pump();
    await tester.tap(find.text('保存'));
    await tester.pumpAndSettle();
    expect(saved, [
      [],
      ['f2'],
    ]);
  });

  testWidgets('拖到空列保留列位置，空位随排布一起保存', (tester) async {
    List<List<String>>? saved;
    await tester.pumpWidget(
      MaterialApp(
        home: KeyBarEditor(
          initialRows: const [
            ['escape', 'tab', 'up'],
            [],
          ],
          onSave: (rows) async => saved = rows,
        ),
      ),
    );
    await dragTo(tester, palette('f1'), cell(1, 2));
    await tester.tap(find.text('保存'));
    await tester.pumpAndSettle();
    expect(saved![1], ['spacer', 'spacer', 'f1']);
  });

  testWidgets('空排布可恢复默认，保存失败保留草稿', (tester) async {
    await tester.pumpWidget(
      MaterialApp(
        home: KeyBarEditor(
          initialRows: const [[], []],
          onSave: (_) async => throw StateError('disk'),
        ),
      ),
    );
    await tester.tap(find.text('恢复默认'));
    await tester.pump();
    expect(
      find.descendant(of: cell(0, 4), matching: find.text('↑')),
      findsOneWidget,
    );
    await tester.tap(find.text('保存'));
    await tester.pumpAndSettle();
    expect(find.text('保存失败，请重试。'), findsOneWidget);
    expect(cell(1, 7), findsOneWidget);
  });

  testWidgets('满排仍能内部排序，但不能加入第 25 个按钮', (tester) async {
    await tester.binding.setSurfaceSize(const Size(1200, 700));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    List<List<String>>? saved;
    await tester.pumpWidget(
      MaterialApp(
        home: KeyBarEditor(
          initialRows: [
            ['escape', ...List.filled(23, 'tab')],
            [],
          ],
          onSave: (rows) async => saved = rows,
        ),
      ),
    );
    await dragTo(tester, palette('f1'), cell(0, 1));
    await dragTo(tester, cell(0, 0), cell(0, 2), dx: 12);
    await tester.tap(find.text('保存'));
    await tester.pumpAndSettle();
    expect(saved![0].length, 24);
    expect(saved![0].take(3), ['tab', 'tab', 'escape']);
    expect(saved![0].contains('f1'), isFalse);
  });

  testWidgets('窄屏拖动到预览边缘会滚动，可移到长排末尾', (tester) async {
    await tester.binding.setSurfaceSize(const Size(320, 700));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    List<List<String>>? saved;
    await tester.pumpWidget(
      MaterialApp(
        home: KeyBarEditor(
          initialRows: [
            ['escape', ...List.filled(10, 'tab')],
            [],
          ],
          onSave: (rows) async => saved = rows,
        ),
      ),
    );
    final gesture = await tester.startGesture(tester.getCenter(cell(0, 0)));
    await gesture.moveBy(const Offset(0, 12));
    await tester.pump();
    final preview = tester.getRect(find.byType(KeyBarGrid));
    await gesture.moveTo(
      Offset(preview.right - 8, tester.getCenter(cell(0, 0)).dy),
    );
    await tester.pump();
    for (var i = 0; i < 40; i++) {
      await tester.pump(const Duration(milliseconds: 40));
    }
    expect(tester.getCenter(cell(0, 11)).dx, lessThan(preview.right));
    await gesture.moveTo(tester.getCenter(cell(0, 11)));
    await tester.pump();
    await gesture.up();
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
    await tester.tap(find.text('保存'));
    await tester.pumpAndSettle();
    expect(saved![0].last, 'escape');
  });

  test('默认方向键呈倒 T 形，所有按钮均有定义', () {
    final col = defaultKeyBarRows[0].indexOf('up');
    expect(defaultKeyBarRows[1].sublist(col - 1, col + 2), [
      'left',
      'down',
      'right',
    ]);
    for (final id in defaultKeyBarRows.expand((row) => row)) {
      expect(keyBarButton(id), isNotNull);
    }
  });
}
