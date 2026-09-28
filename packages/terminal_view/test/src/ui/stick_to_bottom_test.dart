import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:terminal_view/terminal_view.dart';

void main() {
  testWidgets(
      'output after a clear keeps the view at the bottom despite the spring back',
      (tester) async {
    // iOS scroll physics spring back when the content shrinks below the scroll
    // offset; that animation must not leave the view behind while output grows.
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    final terminal = Terminal(maxLines: 10000);
    final scroll = ScrollController();
    await tester.pumpWidget(MaterialApp(
      home: Scaffold(
        body: SizedBox(
          width: 800,
          height: 600,
          child: TerminalView(terminal, scrollController: scroll),
        ),
      ),
    ));
    terminal.write(List.generate(300, (i) => 'line $i').join('\r\n'));
    await tester.pump(const Duration(milliseconds: 16));

    // `clear`: drop the scrollback and the screen.
    terminal.write('\x1b[H\x1b[2J\x1b[3J');
    await tester.pump(const Duration(milliseconds: 16));
    for (var frame = 0; frame < 40; frame++) {
      terminal
          .write(List.generate(80, (i) => '\r\nframe $frame row $i').join());
      await tester.pump(const Duration(milliseconds: 16));
    }
    await tester.pump(const Duration(seconds: 1));
    debugDefaultTargetPlatformOverride = null;

    expect(scroll.position.pixels, scroll.position.maxScrollExtent);
  });

  testWidgets('the user scrolling up still leaves the bottom', (tester) async {
    final terminal = Terminal(maxLines: 10000);
    final scroll = ScrollController();
    await tester.pumpWidget(MaterialApp(
      home: Scaffold(
        body: SizedBox(
          width: 800,
          height: 600,
          child: TerminalView(terminal, scrollController: scroll),
        ),
      ),
    ));
    terminal.write(List.generate(300, (i) => 'line $i').join('\r\n'));
    await tester.pump();
    await tester.drag(find.byType(TerminalView), const Offset(0, 300));
    await tester.pumpAndSettle();
    final detached = scroll.position.pixels;
    expect(detached, lessThan(scroll.position.maxScrollExtent));

    terminal.write('\r\nmore output');
    await tester.pump();
    expect(scroll.position.pixels, detached,
        reason: 'new output does not pull the user back down');
  });
}
