// 缩放与键位条配置在真机上的集成回归；按键与手势在 Flutter 层注入。
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart' show ValueKey;
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/terminal/key_bar_layout.dart';
import 'package:guosh_shell/src/terminal/terminal_key_bar.dart';
import 'package:guosh_shell/src/terminal/terminal_zoom.dart';
import 'package:integration_test/integration_test.dart';

import 'm6_harness.dart';

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.fullyLive;
  setUpAll(startRust);

  testWidgets('缩放重排且不发送快捷键；编辑排布保存后立即生效', (tester) async {
    final app = M6App(tester, binding);
    final original = SettingsState.latestRustSignal!.message.keyBarRows;
    try {
      await saveKeyBarLayout(defaultKeyBarRows);
      await app.open(command: 'm6-keyecho --seconds 180');
      await app.waitScreen('M6-KEYECHO-READY');
      final pane = app.pane;
      final session = pane.sessionId;
      pane.focusNode.requestFocus();
      await tester.pump(const Duration(milliseconds: 300));
      final initialSize = pane.zoom!.size;
      final initialCols = pane.terminal.viewWidth;
      await resetInput();
      await tester.sendKeyDownEvent(
        LogicalKeyboardKey.metaLeft,
        physicalKey: PhysicalKeyboardKey.metaLeft,
      );
      await tester.sendKeyEvent(
        LogicalKeyboardKey.equal,
        physicalKey: PhysicalKeyboardKey.equal,
      );
      await tester.sendKeyUpEvent(
        LogicalKeyboardKey.metaLeft,
        physicalKey: PhysicalKeyboardKey.metaLeft,
      );
      await tester.pump(const Duration(milliseconds: 500));
      expect(pane.zoom!.size, initialSize + 1);
      expect(pane.terminal.viewWidth, lessThan(initialCols));
      expect(await inputBytes(), isEmpty);
      await tester.sendKeyDownEvent(
        LogicalKeyboardKey.metaLeft,
        physicalKey: PhysicalKeyboardKey.metaLeft,
      );
      await tester.sendKeyEvent(
        LogicalKeyboardKey.digit0,
        physicalKey: PhysicalKeyboardKey.digit0,
      );
      await tester.sendKeyUpEvent(
        LogicalKeyboardKey.metaLeft,
        physicalKey: PhysicalKeyboardKey.metaLeft,
      );
      await tester.pump(const Duration(milliseconds: 500));
      expect(pane.zoom!.size, initialSize);
      expect(pane.terminal.viewWidth, initialCols);

      final rect = tester.getRect(find.byType(TerminalZoomSurface));
      final center = Offset(rect.center.dx, rect.top + rect.height * 0.25);
      final a = await tester.startGesture(
        center - const Offset(40, 0),
        pointer: 20,
      );
      final b = await tester.startGesture(
        center + const Offset(40, 0),
        pointer: 21,
      );
      await a.moveTo(center - const Offset(65, 0));
      await b.moveTo(center + const Offset(65, 0));
      await a.up();
      await b.up();
      await tester.pump(const Duration(milliseconds: 500));
      expect(pane.zoom!.size, greaterThan(initialSize));
      pane.zoom!.reset();
      await tester.pump(const Duration(milliseconds: 300));

      await tester.tap(find.byTooltip('编辑功能按钮'));
      await tester.pumpAndSettle();
      await app.screenshot('keybar-visual-editor');
      final drag = await tester.startGesture(
        tester.getCenter(find.byKey(const ValueKey('preview-0-0'))),
      );
      await drag.moveBy(const Offset(0, 12));
      await tester.pump();
      await drag.moveTo(
        tester.getCenter(find.byKey(const ValueKey('preview-1-8'))),
      );
      await tester.pump();
      await drag.up();
      await tester.pumpAndSettle();
      expect(
        find.descendant(
          of: find.byKey(const ValueKey('preview-1-8')),
          matching: find.text('Esc'),
        ),
        findsOneWidget,
      );
      await app.screenshot('keybar-visual-editor-dragged');
      await tester.pump();
      await tester.tap(find.text('删除所选按钮'));
      await tester.pump();
      await tester.tap(find.text('保存'));
      await tester.pumpAndSettle();
      final rows = tester
          .widget<TerminalKeyBar>(find.byType(TerminalKeyBar))
          .rows;
      expect(rows.first, defaultKeyBarRows.first.skip(1).toList());
      expect(rows.last, defaultKeyBarRows.last);
      expect(pane.sessionId, session);
      final query = SettingsState.rustSignalStream.first;
      SettingsQuery().sendSignalToRust();
      await tester.runAsync(
        () async => expect((await query).message.keyBarRows, rows),
      );
      await app.screenshot('input-features');
      binding.reportData = {
        ...?binding.reportData,
        'input_features': {
          'zoom_initial_size': initialSize,
          'zoom_initial_cols': initialCols,
          'keyboard_zoom': true,
          'pinch_zoom': true,
          'key_bar_persistence': true,
          'key_bar_visual_drag': true,
          'session_preserved': pane.sessionId == session,
        },
      };
    } finally {
      await saveKeyBarLayout(original);
      await app.dispose();
    }
  });
}
