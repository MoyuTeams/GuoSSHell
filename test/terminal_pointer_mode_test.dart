import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/terminal/frame.dart';
import 'package:guosh_shell/src/terminal/frame_terminal.dart';
import 'package:guosh_shell/src/terminal/terminal_input_mode.dart';
import 'package:guosh_shell/src/terminal/terminal_pane.dart';
import 'package:terminal_view/terminal_view.dart';
// 核对实际手柄组件是否被构建，避免只测试配置值。
// ignore: implementation_imports
import 'package:terminal_view/src/ui/selection_handles.dart';

class RecordingFrame extends FrameTerminal {
  bool tracking = false;
  final reports = <TerminalMouseButtonState>[];
  @override
  bool mouseInput(
    TerminalMouseButton button,
    TerminalMouseButtonState state,
    CellOffset position, {
    bool shift = false,
    bool alt = false,
    bool ctrl = false,
  }) {
    if (!tracking) return false;
    reports.add(state);
    return true;
  }
}

void main() {
  for (final platform in [
    TargetPlatform.windows,
    TargetPlatform.macOS,
    TargetPlatform.linux,
    TargetPlatform.iOS,
    TargetPlatform.android,
  ]) {
    testWidgets('$platform 按触摸、鼠标、键盘切换手柄', (tester) async {
      debugDefaultTargetPlatformOverride = platform;
      final mode = TerminalInputMode();
      final terminal = RecordingFrame();
      final controller = TerminalController(
        pointerInputs: const PointerInputs.all(),
      );
      final scroll = ScrollController();
      final focus = FocusNode();
      var secondary = 0;
      terminal.applyFrame(
        const TerminalFrame(
          cols: 80,
          rows: 20,
          cursorCol: 0,
          cursorRow: 0,
          firstStableRow: 0,
          screenTopStableRow: 0,
          lines: [
            FrameRow(
              stableRow: 0,
              wrapped: false,
              runs: [
                FrameRun(
                  start: 0,
                  len: 16,
                  attrs: 0,
                  fg: DefaultColor(),
                  bg: DefaultColor(),
                  text: 'alpha beta gamma',
                ),
              ],
            ),
          ],
        ),
      );
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: SizedBox(
              width: 600,
              height: 320,
              child: TerminalViewport(
                terminal: terminal,
                controller: controller,
                scrollController: scroll,
                focusNode: focus,
                style: const TerminalStyle(fontSize: 14),
                backgroundOpacity: 1,
                inputMode: mode,
                onSecondaryTap: () => secondary++,
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      final render = tester
          .state<TerminalViewState>(find.byType(TerminalView))
          .renderTerminal;
      final point = render.localToGlobal(
        Offset(render.cellSize.width * 3, render.cellSize.height / 2),
      );
      controller.setExternalSelection(
        const CellOffset(0, 0),
        const CellOffset(5, 0),
      );
      await tester.pump();
      expect(find.byType(TerminalSelectionHandles), findsNothing);
      final touch = await tester.startGesture(
        point,
        kind: PointerDeviceKind.touch,
      );
      await tester.pump();
      expect(mode.touch, isTrue);
      await touch.up();
      await tester.pumpAndSettle();
      controller.setExternalSelection(
        const CellOffset(0, 0),
        const CellOffset(5, 0),
      );
      await tester.pump();
      expect(find.byType(TerminalSelectionHandles), findsOneWidget);
      final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
      await mouse.addPointer(location: point);
      await mouse.moveTo(point + const Offset(4, 0));
      await tester.pump();
      expect(mode.touch, isFalse);
      expect(find.byType(TerminalSelectionHandles), findsNothing);
      await tester.tapAt(
        point,
        buttons: kSecondaryMouseButton,
        kind: PointerDeviceKind.mouse,
      );
      await tester.pumpAndSettle();
      expect(secondary, 1);
      expect(controller.selection, isNotNull, reason: '右键不先清除选区');
      terminal.tracking = true;
      await tester.tapAt(
        point,
        buttons: kSecondaryMouseButton,
        kind: PointerDeviceKind.mouse,
      );
      await tester.pumpAndSettle();
      expect(secondary, 1);
      expect(terminal.reports, [
        TerminalMouseButtonState.down,
        TerminalMouseButtonState.up,
      ]);
      await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
      await tester.tapAt(
        point,
        buttons: kSecondaryMouseButton,
        kind: PointerDeviceKind.mouse,
      );
      await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
      await tester.pumpAndSettle();
      expect(secondary, 2);
      mode.pointer(PointerDeviceKind.touch);
      await tester.pump();
      focus.requestFocus();
      await tester.pump();
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pump();
      expect(
        tester.testTextInput.setClientArgs?['viewId'],
        tester.view.viewId,
        reason: '原生文本客户端必须关联终端所属视图',
      );
      expect(find.byType(TerminalSelectionHandles), findsNothing);
      mode.pointer(PointerDeviceKind.trackpad);
      expect(mode.touch, isFalse);
      mode.pointer(PointerDeviceKind.stylus);
      expect(mode.touch, isTrue);
      await mouse.removePointer();
      await tester.pumpWidget(const SizedBox());
      await tester.pumpAndSettle();
      controller.dispose();
      scroll.dispose();
      focus.dispose();
      terminal.dispose();
      mode.dispose();
      debugDefaultTargetPlatformOverride = null;
    });
  }

  testWidgets('工作区外的鼠标和键盘输入也退出触屏模式', (tester) async {
    final mode = TerminalInputMode()..bind();
    await tester.pumpWidget(
      const MaterialApp(home: Scaffold(body: Text('页面'))),
    );
    mode.pointer(PointerDeviceKind.touch);
    tester.binding.handlePointerEvent(
      const PointerHoverEvent(
        kind: PointerDeviceKind.mouse,
        position: Offset(40, 40),
        synthesized: true,
      ),
    );
    expect(mode.touch, isTrue, reason: '合成鼠标事件不应覆盖实际触摸操作');
    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: const Offset(40, 40));
    await mouse.moveTo(const Offset(60, 40));
    await tester.pump();
    expect(mode.touch, isFalse);
    mode.pointer(PointerDeviceKind.touch);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    expect(mode.touch, isFalse);
    await mouse.removePointer();
    mode.dispose();
  });
}
