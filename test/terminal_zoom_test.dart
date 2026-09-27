import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/terminal/terminal_zoom.dart';

void main() {
  test('缩放遵守上下界，快捷键不修改默认字号', () {
    final zoom = TerminalZoom(defaultSize: 14, minSize: 8, maxSize: 32);
    const plus = KeyDownEvent(
      physicalKey: PhysicalKeyboardKey.equal,
      logicalKey: LogicalKeyboardKey.equal,
      timeStamp: Duration.zero,
    );
    expect(zoom.handleKey(plus, meta: false), isFalse);
    expect(zoom.handleKey(plus, meta: true), isTrue);
    expect(zoom.size, 15);
    zoom.begin();
    zoom.scale(99);
    zoom.end();
    expect(zoom.size, 32);
    zoom.begin();
    zoom.scale(0.01);
    zoom.end();
    expect(zoom.size, 8);
    zoom.reset();
    expect(zoom.size, 14);
    zoom.scale(double.nan);
    expect(zoom.size, 14);
  });

  testWidgets('双指捏合改变字号，单指仍可滚动', (tester) async {
    final zoom = TerminalZoom(defaultSize: 14, minSize: 8, maxSize: 32);
    final scroll = ScrollController();
    await tester.pumpWidget(
      MaterialApp(
        home: TerminalZoomSurface(
          zoom: zoom,
          onStart: () {},
          child: ListView(
            controller: scroll,
            children: [
              for (var i = 0; i < 40; i++)
                SizedBox(height: 60, child: Text('行 $i')),
            ],
          ),
        ),
      ),
    );
    final first = await tester.startGesture(const Offset(250, 220), pointer: 1);
    final second = await tester.startGesture(
      const Offset(350, 220),
      pointer: 2,
    );
    await first.moveTo(const Offset(220, 220));
    await second.moveTo(const Offset(380, 220));
    await tester.pump();
    expect(zoom.size, greaterThan(14));
    expect(scroll.offset, 0, reason: '捏合不能同时滚动终端');
    await first.up();
    await second.up();
    final scaled = zoom.size;
    await tester.drag(find.byType(ListView), const Offset(0, -200));
    await tester.pumpAndSettle();
    expect(scroll.offset, greaterThan(0));
    expect(zoom.size, scaled);
    await tester.pumpWidget(const SizedBox());
    zoom.dispose();
    scroll.dispose();
  });

  testWidgets('触控板缩放按比例变化', (tester) async {
    final zoom = TerminalZoom(defaultSize: 14, minSize: 8, maxSize: 32);
    await tester.pumpWidget(
      MaterialApp(
        home: TerminalZoomSurface(
          zoom: zoom,
          onStart: () {},
          child: const ColoredBox(color: Colors.black),
        ),
      ),
    );
    final gesture = await tester.createGesture(kind: PointerDeviceKind.trackpad);
    await gesture.panZoomStart(const Offset(250, 250));
    await gesture.panZoomUpdate(const Offset(250, 250), scale: 1.5);
    await tester.pump();
    expect(zoom.size, 21);
    await gesture.panZoomEnd();
    await tester.pumpWidget(const SizedBox());
    zoom.dispose();
  });
}
