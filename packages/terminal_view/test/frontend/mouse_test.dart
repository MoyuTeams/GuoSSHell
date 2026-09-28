import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:terminal_view/terminal_view.dart';

/// A terminal that records the mouse reports it receives. [tracking] plays
/// the application's mouse mode: while it is on, every report is consumed.
class _RecordingTerminal extends Terminal {
  _RecordingTerminal({required this.tracking});

  final bool tracking;

  final reports = <String>[];

  @override
  bool mouseInput(
    TerminalMouseButton button,
    TerminalMouseButtonState buttonState,
    CellOffset position, {
    bool shift = false,
    bool alt = false,
    bool ctrl = false,
  }) {
    if (!tracking) return false;
    reports.add('${button.name} ${buttonState.name} '
        '${_cell(position)}${_modifiers(shift, alt, ctrl)}');
    return true;
  }

  @override
  bool mouseMotion(
    TerminalMouseButton? button,
    CellOffset position, {
    bool shift = false,
    bool alt = false,
    bool ctrl = false,
  }) {
    if (!tracking) return false;
    reports.add('${button?.name ?? 'hover'} move '
        '${_cell(position)}${_modifiers(shift, alt, ctrl)}');
    return true;
  }

  static String _cell(CellOffset position) => '${position.x},${position.y}';

  static String _modifiers(bool shift, bool alt, bool ctrl) => [
        if (shift) ' shift',
        if (alt) ' alt',
        if (ctrl) ' ctrl',
      ].join();
}

void main() {
  late _RecordingTerminal terminal;
  late TerminalController controller;
  late List<TapDownDetails> secondaryTaps;

  Future<void> pumpTerminal(
    WidgetTester tester, {
    required bool tracking,
    PointerInputs pointerInputs = const PointerInputs.all(),
  }) async {
    terminal = _RecordingTerminal(tracking: tracking);
    controller = TerminalController(pointerInputs: pointerInputs);
    secondaryTaps = [];
    await tester.pumpWidget(MaterialApp(
      home: Scaffold(
        body: TerminalView(
          terminal,
          controller: controller,
          onSecondaryTapDown: (details, offset) => secondaryTaps.add(details),
        ),
      ),
    ));
  }

  /// The global position of the middle of cell ([col], [row]).
  Offset cellCenter(WidgetTester tester, int col, int row) {
    final render = tester.state<TerminalViewState>(find.byType(TerminalView)).renderTerminal;
    final cell = render.cellSize;
    return render.localToGlobal(Offset((col + 0.5) * cell.width, (row + 0.5) * cell.height));
  }

  Future<void> drag(
    WidgetTester tester,
    List<Offset> path, {
    int buttons = kPrimaryMouseButton,
  }) async {
    final gesture = await tester.startGesture(
      path.first,
      kind: PointerDeviceKind.mouse,
      buttons: buttons,
    );
    for (final position in path.skip(1)) {
      await gesture.moveTo(position);
    }
    await gesture.up();
    await gesture.removePointer();
    // Let the double-tap window of a click close.
    await tester.pump(kDoubleTapTimeout);
  }

  group('while the application tracks the mouse', () {
    testWidgets('a mouse drag is reported as press, moves and release', (tester) async {
      await pumpTerminal(tester, tracking: true);

      await drag(tester, [
        cellCenter(tester, 1, 1),
        cellCenter(tester, 3, 1),
        cellCenter(tester, 3, 2),
      ]);

      expect(terminal.reports, [
        'left down 1,1',
        'left move 3,1',
        'left move 3,2',
        'left up 3,2',
      ]);
      expect(controller.selection, isNull);
    });

    testWidgets('a click is reported once', (tester) async {
      await pumpTerminal(tester, tracking: true);

      await drag(tester, [cellCenter(tester, 2, 0)]);

      expect(terminal.reports, ['left down 2,0', 'left up 2,0']);
    });

    testWidgets('hovering is reported as buttonless moves', (tester) async {
      await pumpTerminal(tester, tracking: true);

      final gesture = await tester.createGesture(kind: PointerDeviceKind.mouse);
      await gesture.addPointer(location: cellCenter(tester, 0, 0));
      await gesture.moveTo(cellCenter(tester, 4, 3));
      await tester.pump();

      expect(terminal.reports, ['hover move 4,3']);
      await gesture.removePointer();
    });

    testWidgets('a right click goes to the application, not to the context menu', (tester) async {
      await pumpTerminal(tester, tracking: true);

      await drag(tester, [cellCenter(tester, 5, 2)], buttons: kSecondaryMouseButton);

      expect(terminal.reports, ['right down 5,2', 'right up 5,2']);
      expect(secondaryTaps, isEmpty);
    });

    testWidgets('control and alt are reported with the press', (tester) async {
      await pumpTerminal(tester, tracking: true);

      await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
      await tester.sendKeyDownEvent(LogicalKeyboardKey.altLeft);
      await drag(tester, [cellCenter(tester, 1, 1)]);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.altLeft);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);

      expect(terminal.reports, ['left down 1,1 alt ctrl', 'left up 1,1 alt ctrl']);
    });

    testWidgets('shift keeps the mouse for local selection', (tester) async {
      await pumpTerminal(tester, tracking: true);

      await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
      await drag(tester, [cellCenter(tester, 1, 1), cellCenter(tester, 6, 1)]);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);

      expect(terminal.reports, isEmpty);
      expect(controller.selection, isNotNull);
    });

    testWidgets('drags and hovering stay off unless enabled on the controller', (tester) async {
      await pumpTerminal(
        tester,
        tracking: true,
        pointerInputs: const PointerInputs({PointerInput.tap}),
      );

      await drag(tester, [cellCenter(tester, 1, 1), cellCenter(tester, 3, 1)]);
      final hover = await tester.createGesture(kind: PointerDeviceKind.mouse);
      await hover.addPointer(location: cellCenter(tester, 0, 0));
      await hover.moveTo(cellCenter(tester, 2, 2));
      await tester.pump();
      await hover.removePointer();

      expect(terminal.reports, ['left down 1,1', 'left up 3,1']);
    });
  });

  group('while the application does not track the mouse', () {
    testWidgets('a mouse drag selects locally', (tester) async {
      await pumpTerminal(tester, tracking: false);

      await drag(tester, [cellCenter(tester, 1, 1), cellCenter(tester, 6, 1)]);

      expect(terminal.reports, isEmpty);
      expect(controller.selection, isNotNull);
    });

    testWidgets('a right click opens the context menu', (tester) async {
      await pumpTerminal(tester, tracking: false);

      await drag(tester, [cellCenter(tester, 5, 2)], buttons: kSecondaryMouseButton);

      expect(secondaryTaps, hasLength(1));
    });
  });

  testWidgets('touch taps are still reported as clicks', (tester) async {
    await pumpTerminal(tester, tracking: true);

    await tester.tapAt(cellCenter(tester, 2, 1));
    await tester.pump(kDoubleTapTimeout);

    expect(terminal.reports, ['left down 2,1', 'left up 2,1']);
  });
}
