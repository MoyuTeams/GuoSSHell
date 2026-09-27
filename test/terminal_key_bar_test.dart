import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/terminal/frame_terminal.dart';
import 'package:guosh_shell/src/terminal/key_bar_layout.dart';
import 'package:guosh_shell/src/terminal/key_bar_visuals.dart';
import 'package:guosh_shell/src/terminal/terminal_key_bar.dart';

void main() {
  testWidgets('不同类型按钮等宽，默认方向键对齐，锁定不改变尺寸', (tester) async {
    final terminal = FrameTerminal();
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: TerminalKeyBar(
            terminal: terminal,
            canCopy: () => false,
            onCopy: () {},
            onPaste: () {},
            onToggleKeyboard: () {},
            onDisconnect: () {},
            onEdit: () {},
            onZoomIn: () {},
            onZoomOut: () {},
            onZoomReset: () {},
            rows: [
              defaultKeyBarRows[0],
              [...defaultKeyBarRows[1], 'text:最多两个汉字宽度'],
            ],
          ),
        ),
      ),
    );
    Finder face(String label) => find.widgetWithText(KeyBarKeyFace, label);
    final widths = tester
        .widgetList<KeyBarKeyFace>(find.byType(KeyBarKeyFace))
        .map((widget) => tester.getSize(find.byWidget(widget)).width)
        .toSet();
    expect(widths, {40.0});
    final up = tester.getCenter(face('↑'));
    final down = tester.getCenter(face('↓'));
    expect(up.dx, down.dx);
    expect(down.dy - up.dy, 40);
    expect(tester.getCenter(face('←')).dx, down.dx - 40);
    expect(tester.getCenter(face('→')).dx, down.dx + 40);
    final before = tester.getSize(face('Ctrl'));
    await tester.longPress(face('Ctrl'));
    await tester.pump();
    expect(terminal.isModifierLocked('ctrl'), isTrue);
    expect(tester.getSize(face('Ctrl')), before);
    expect(tester.takeException(), isNull);
  });

  testWidgets('窄屏两排一起滚动，键位条输入与编辑动作正常', (tester) async {
    await tester.binding.setSurfaceSize(const Size(320, 640));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    final events = <TerminalInputEvent>[];
    final terminal = FrameTerminal(
      onInput: (event) {
        events.add(event);
        return true;
      },
    );
    var edited = false;
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Align(
            alignment: Alignment.bottomCenter,
            child: TerminalKeyBar(
              terminal: terminal,
              canCopy: () => true,
              onCopy: () {},
              onPaste: () {},
              onToggleKeyboard: () {},
              onDisconnect: () {},
              onEdit: () => edited = true,
              onZoomIn: () {},
              onZoomOut: () {},
              onZoomReset: () {},
              rows: defaultKeyBarRows,
            ),
          ),
        ),
      ),
    );
    expect(tester.takeException(), isNull);
    await tester.tap(find.text('Ctrl'));
    await tester.tap(find.text('Tab'));
    await tester.pump();
    expect(
      events.single,
      isA<KeyInputEvent>().having((event) => event.control, 'control', isTrue),
    );
    final upBefore = tester.getCenter(find.text('↑'));
    final downBefore = tester.getCenter(find.text('↓'));
    await tester.drag(find.byType(KeyBarGrid), const Offset(-100, 0));
    await tester.pumpAndSettle();
    final upAfter = tester.getCenter(find.text('↑'));
    final downAfter = tester.getCenter(find.text('↓'));
    expect(upAfter.dx, lessThan(upBefore.dx));
    expect(upAfter.dx - upBefore.dx, downAfter.dx - downBefore.dx);
    terminal.lockModifier('alt');
    await tester.tap(find.byTooltip('编辑功能按钮'));
    await tester.pump();
    expect(edited, isTrue);
    expect(terminal.isModifierLocked('alt'), isFalse);
    expect(tester.takeException(), isNull);
  });
}
