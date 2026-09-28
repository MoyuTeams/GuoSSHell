import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:terminal_view/terminal_view.dart';

void main() {
  group('TerminalController', () {
    testWidgets('setSelectionRange works', (tester) async {
      final terminal = Terminal();
      final terminalView = TerminalController();

      await tester.pumpWidget(MaterialApp(
        home: Scaffold(
          body: TerminalView(
            terminal,
            controller: terminalView,
          ),
        ),
      ));

      terminalView.setSelection(
        terminal.buffer.createAnchor(0, 0),
        terminal.buffer.createAnchor(2, 2),
      );

      await tester.pump();

      expect(terminalView.selection, isNotNull);
    });

    testWidgets('setSelectionMode changes BufferRange type', (tester) async {
      final terminal = Terminal();
      final terminalView = TerminalController();

      await tester.pumpWidget(MaterialApp(
        home: Scaffold(
          body: TerminalView(
            terminal,
            controller: terminalView,
          ),
        ),
      ));

      terminalView.setSelection(
        terminal.buffer.createAnchor(0, 0),
        terminal.buffer.createAnchor(2, 2),
      );

      expect(terminalView.selection, isA<BufferRangeLine>());

      terminalView.setSelectionMode(SelectionMode.block);

      expect(terminalView.selection, isA<BufferRangeBlock>());
    });

    testWidgets('clearSelection works', (tester) async {
      final terminal = Terminal();
      final terminalView = TerminalController();

      await tester.pumpWidget(MaterialApp(
        home: Scaffold(
          body: TerminalView(
            terminal,
            controller: terminalView,
          ),
        ),
      ));

      terminalView.setSelection(
        terminal.buffer.createAnchor(0, 0),
        terminal.buffer.createAnchor(2, 2),
      );

      expect(terminalView.selection, isNotNull);

      terminalView.clearSelection();

      expect(terminalView.selection, isNull);
    });
  });

  group('TerminalController.highlight', () {
    test('works', () {
      final terminal = Terminal();
      final controller = TerminalController();

      final highlight = controller.highlight(
        p1: terminal.buffer.createAnchor(5, 5),
        p2: terminal.buffer.createAnchor(5, 10),
        color: Colors.yellow,
      );
      assert(controller.highlights.length == 1);

      highlight.dispose();
      assert(controller.highlights.isEmpty);
    });
  });

  group('TerminalController external selection', () {
    test('setExternalSelection drives selection without anchors', () {
      final controller = TerminalController();

      controller.setExternalSelection(
        const CellOffset(1, 0),
        const CellOffset(4, 2),
      );

      final selection = controller.selection;
      expect(selection, isNotNull);
      expect(selection, isA<BufferRangeLine>());
      expect(selection!.begin, const CellOffset(1, 0));
      expect(selection.end, const CellOffset(4, 2));

      controller.setExternalSelection(null, null);
      expect(controller.selection, isNull);
    });

    test('requestSelection reports the intent and updates optimistically', () {
      final controller = TerminalController();
      final reported = <(CellOffset?, CellOffset?)>[];
      controller.onSelectionIntent =
          (begin, end) => reported.add((begin, end));

      controller.requestSelection(
        const CellOffset(1, 1),
        const CellOffset(3, 1),
      );

      expect(reported, [(const CellOffset(1, 1), const CellOffset(3, 1))]);
      expect(controller.selection!.begin, const CellOffset(1, 1));
    });

    test('clearSelection reports a clear intent', () {
      final controller = TerminalController();
      controller.setExternalSelection(
        const CellOffset(1, 0),
        const CellOffset(4, 2),
      );

      final reported = <(CellOffset?, CellOffset?)>[];
      controller.onSelectionIntent =
          (begin, end) => reported.add((begin, end));

      controller.clearSelection();

      expect(controller.selection, isNull);
      expect(reported, [(null, null)]);
    });
  });
}
