// M6 E 类：终端协议与宽字符（docs/acceptance-m6-2026-09-26.md §3 E、S7）；
// D 类里 XCUITest 在模拟器上合成不出来的部分，从 Flutter 的按键 / 指针事件注入：
// 回车、退格、Esc、编辑键、F 键与 Ctrl / Alt 组合的编码（D2–D5），鼠标悬停（D10）。
import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/terminal/terminal_pane.dart';
import 'package:integration_test/integration_test.dart';

import 'm6_harness.dart';

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.fullyLive;
  final report = <String, Object>{'device': m6Device};

  setUpAll(startRust);
  // 截图也存在 reportData 里（screenshots）：合并，不覆盖。
  tearDownAll(() => binding.reportData = {...?binding.reportData, 'protocol': report});

  testWidgets('S7 / E：宽字符按列排布，画面与引擎一致', (tester) async {
    final app = M6App(tester, binding);
    await app.open();
    await app.waitScreen('probe@');
    await app.type('clear; m6-cjk\r');
    await app.waitScreen('M6-CJK-END');
    await app.waitIdle();

    final problems = await app.screenMismatches();
    report['cjk_mismatches'] = problems;
    final cols = app.pane.terminal.viewWidth;
    final lines = app.screen();
    expect(lines, contains('中' * (cols ~/ 2)), reason: '一整行中文要铺满');
    expect(lines.any((l) => l.startsWith('中文X tail 🚀Y end')), isTrue, reason: lines.join('\n'));
    expect(screenGarbage(app.screenText()), isEmpty);
    await app.screenshot('protocol-cjk');
    expect(problems, isEmpty, reason: problems.join('\n'));
    await app.dispose();
  });

  testWidgets('E1：同步输出——成对整帧出现；没有结束序列或被杀也照常显示', (tester) async {
    final app = M6App(tester, binding);
    await app.open();
    await app.waitScreen('probe@');

    await app.type('clear; m6-sync stall\r');
    final visible = await app.waitScreen('M6-SYNC-VISIBLE', timeout: const Duration(seconds: 3));
    report['sync_stall_visible_ms'] = visible.inMilliseconds;
    expect(visible, lessThan(const Duration(milliseconds: 1500)), reason: 'BSU 后 150 ms 应照常显示，不该等到 2 秒后');
    await app.waitScreen('M6-SYNC-STALL-DONE');

    await app.type('clear; m6-sync kill\r');
    await app.waitScreen('M6-SYNC-KILL-DONE', timeout: const Duration(seconds: 5));
    await app.type('echo after-kill-$m6Device\r');
    await app.waitScreen('after-kill-$m6Device', timeout: const Duration(seconds: 5));

    await app.type('clear; m6-sync pair\r');
    await app.waitScreen('M6-SYNC-PAIR-DONE', timeout: const Duration(seconds: 20));
    await app.waitIdle();
    final problems = await app.screenMismatches();
    expect(problems, isEmpty, reason: problems.join('\n'));
    await app.dispose();
  });

  testWidgets('D2–D5：硬件按键到远端的字节（xterm 默认编码；应用光标键下方向键与 Home / End 为 SS3）', (tester) async {
    const ctrl = LogicalKeyboardKey.controlLeft;
    const alt = LogicalKeyboardKey.altLeft;
    const shift = LogicalKeyboardKey.shiftLeft;
    // 名称 → (按住的修饰键, 按键, 期望字节)。
    final normal = <String, (List<LogicalKeyboardKey>, LogicalKeyboardKey, List<int>)>{
      'enter': (const [], LogicalKeyboardKey.enter, [0x0d]),
      'backspace': (const [], LogicalKeyboardKey.backspace, [0x7f]),
      'escape': (const [], LogicalKeyboardKey.escape, [0x1b]),
      'tab': (const [], LogicalKeyboardKey.tab, [0x09]),
      'shift+tab': (const [shift], LogicalKeyboardKey.tab, _esc('[Z')),
      'up': (const [], LogicalKeyboardKey.arrowUp, _esc('[A')),
      'shift+up': (const [shift], LogicalKeyboardKey.arrowUp, _esc('[1;2A')),
      'ctrl+right': (const [ctrl], LogicalKeyboardKey.arrowRight, _esc('[1;5C')),
      'alt+left': (const [alt], LogicalKeyboardKey.arrowLeft, _esc('[1;3D')),
      'home': (const [], LogicalKeyboardKey.home, _esc('[H')),
      'end': (const [], LogicalKeyboardKey.end, _esc('[F')),
      'page_up': (const [], LogicalKeyboardKey.pageUp, _esc('[5~')),
      'page_down': (const [], LogicalKeyboardKey.pageDown, _esc('[6~')),
      'delete': (const [], LogicalKeyboardKey.delete, _esc('[3~')),
      'insert': (const [], LogicalKeyboardKey.insert, _esc('[2~')),
      for (final (i, seq) in ['OP', 'OQ', 'OR', 'OS', '[15~', '[17~', '[18~', '[19~', '[20~', '[21~', '[23~', '[24~'].indexed)
        'f${i + 1}': (const [], _fKeys[i], _esc(seq)),
      'ctrl+c': (const [ctrl], LogicalKeyboardKey.keyC, [0x03]),
      'ctrl+[': (const [ctrl], LogicalKeyboardKey.bracketLeft, [0x1b]),
      r'ctrl+\': (const [ctrl], LogicalKeyboardKey.backslash, [0x1c]),
      'ctrl+]': (const [ctrl], LogicalKeyboardKey.bracketRight, [0x1d]),
      'ctrl+_': (const [ctrl, shift], LogicalKeyboardKey.minus, [0x1f]),
      'ctrl+space': (const [ctrl], LogicalKeyboardKey.space, [0x00]),
      'alt+b': (const [alt], LogicalKeyboardKey.keyB, _esc('b')),
      'alt+.': (const [alt], LogicalKeyboardKey.period, _esc('.')),
      'alt+>': (const [alt, shift], LogicalKeyboardKey.period, _esc('>')),
    };
    final appCursor = <String, (List<LogicalKeyboardKey>, LogicalKeyboardKey, List<int>)>{
      'app:up': (const [], LogicalKeyboardKey.arrowUp, _esc('OA')),
      'app:left': (const [], LogicalKeyboardKey.arrowLeft, _esc('OD')),
      'app:home': (const [], LogicalKeyboardKey.home, _esc('OH')),
      'app:end': (const [], LogicalKeyboardKey.end, _esc('OF')),
      'app:shift+up': (const [shift], LogicalKeyboardKey.arrowUp, _esc('[1;2A')),
    };
    // 先检查全部用例，避免连接后才因漏写键位映射中断字节断言。
    for (final MapEntry(key: name, value: (modifiers, key, _))
        in [...normal.entries, ...appCursor.entries]) {
      for (final logical in [...modifiers, key]) {
        expect(_usPhysicalKeys.containsKey(logical), isTrue,
            reason: '$name 缺少 US 物理键映射：0x${logical.keyId.toRadixString(16)}');
      }
    }

    final received = <String, String>{};
    final wrong = <String>[];
    Future<void> run(String modes, Map<String, (List<LogicalKeyboardKey>, LogicalKeyboardKey, List<int>)> cases) async {
      final app = M6App(tester, binding);
      await app.open(command: 'm6-keyecho $modes --seconds 180');
      await app.waitScreen('M6-KEYECHO-READY');
      app.pane.focusNode.requestFocus();
      await tester.pump(const Duration(milliseconds: 300));
      for (final MapEntry(key: name, value: (modifiers, key, expected)) in cases.entries) {
        await resetInput();
        for (final modifier in modifiers) {
          await tester.sendKeyDownEvent(modifier, physicalKey: _physicalKey(modifier));
        }
        await tester.sendKeyEvent(key, physicalKey: _physicalKey(key));
        for (final modifier in modifiers.reversed) {
          await tester.sendKeyUpEvent(modifier, physicalKey: _physicalKey(modifier));
        }
        // 等字节到齐：与期望一致，或 1.5 秒内不再变化。
        var bytes = <int>[];
        final deadline = DateTime.now().add(const Duration(milliseconds: 1500));
        while (DateTime.now().isBefore(deadline)) {
          await tester.pump(const Duration(milliseconds: 100));
          bytes = await inputBytes();
          if (listEquals(bytes, expected)) break;
        }
        received[name] = _hex(bytes);
        if (!listEquals(bytes, expected)) wrong.add('$name：期望 ${_hex(expected)}，收到 ${_hex(bytes)}');
      }
      await app.dispose();
    }

    await run('', normal);
    await run('appcursor', appCursor);
    report['keys'] = received;
    report['keys_wrong'] = wrong;
    expect(wrong, isEmpty, reason: wrong.join('\n'));
  });

  testWidgets('D10：鼠标悬停——1003 下逐格上报移动、同一格不重复；1002 下不上报', (tester) async {
    Future<List<String>> hover(String modes) async {
      final app = M6App(tester, binding);
      await app.open(command: 'm6-keyecho $modes --seconds 120');
      await app.waitScreen('M6-KEYECHO-READY');
      await app.waitIdle();
      await resetInput();
      // 从终端中部向右，每步半格：12 步经过 6 格。
      final rect = tester.getRect(find.byType(TerminalPane).first);
      final cell = rect.width / app.pane.terminal.viewWidth;
      final start = rect.center;
      final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
      await mouse.addPointer(location: start);
      for (var step = 1; step <= 12; step++) {
        await mouse.moveTo(start + Offset(cell * step / 2, 0));
        await tester.pump(const Duration(milliseconds: 60));
      }
      await mouse.removePointer();
      await tester.pump(const Duration(seconds: 1));
      final events = sgrMouseEvents(await inputBytes());
      await app.dispose();
      return events;
    }

    final motion = await hover('motion');
    report['hover_motion'] = motion;
    expect(motion, isNotEmpty, reason: '1003 下悬停要上报');
    expect(motion.every((e) => e.startsWith('35M@')), isTrue, reason: '悬停是按钮 3 + 32：$motion');
    final cells = motion.map((e) => e.substring(e.indexOf('@') + 1)).toList();
    expect(cells.toSet().length, cells.length, reason: '同一格不重复上报：$cells');
    expect(cells.length, inInclusiveRange(5, 7), reason: '经过约 6 格：$cells');

    final drag = await hover('drag');
    report['hover_drag'] = drag;
    expect(drag, isEmpty, reason: '1002 只报按住拖动，悬停不上报：$drag');
  });

  testWidgets('E2：终端查询按顺序应答，TUI 常用的模式都认得', (tester) async {
    final app = M6App(tester, binding);
    await app.open();
    await app.waitScreen('probe@');
    await app.type('clear; m6-probe\r');
    await app.waitScreen('M6-MODES');
    final line = app.logicalLines().firstWhere((l) => l.contains('M6-MODES'));
    final modes = jsonDecode(line.substring(line.indexOf('{'))) as Map<String, dynamic>;
    report['probe_modes'] = modes;
    expect(modes['da1'], isTrue);
    expect(modes['cpr'], isA<List>());
    for (final mode in ['1', '25', '1000', '1002', '1003', '1004', '1006', '1049', '2004', '2026']) {
      expect(modes[mode], isNot(0), reason: '模式 $mode 应被识别（DECRQM）');
    }
    expect(modes['1049'], 2, reason: '不在备用屏');
    expect(modes['25'], 1, reason: '光标可见');
    await app.dispose();
  });
}

const _fKeys = [
  LogicalKeyboardKey.f1,
  LogicalKeyboardKey.f2,
  LogicalKeyboardKey.f3,
  LogicalKeyboardKey.f4,
  LogicalKeyboardKey.f5,
  LogicalKeyboardKey.f6,
  LogicalKeyboardKey.f7,
  LogicalKeyboardKey.f8,
  LogicalKeyboardKey.f9,
  LogicalKeyboardKey.f10,
  LogicalKeyboardKey.f11,
  LogicalKeyboardKey.f12,
];

// 按 US 键盘的实际键位注入，Shift 组合仍使用原键位（如减号、句号）。
// Flutter 的自动映射依赖仅 debug 构建可用的 debugName，profile 测试必须显式给出物理键。
final _usPhysicalKeys = <LogicalKeyboardKey, PhysicalKeyboardKey>{
  LogicalKeyboardKey.controlLeft: PhysicalKeyboardKey.controlLeft,
  LogicalKeyboardKey.altLeft: PhysicalKeyboardKey.altLeft,
  LogicalKeyboardKey.shiftLeft: PhysicalKeyboardKey.shiftLeft,
  LogicalKeyboardKey.enter: PhysicalKeyboardKey.enter,
  LogicalKeyboardKey.backspace: PhysicalKeyboardKey.backspace,
  LogicalKeyboardKey.escape: PhysicalKeyboardKey.escape,
  LogicalKeyboardKey.tab: PhysicalKeyboardKey.tab,
  LogicalKeyboardKey.arrowUp: PhysicalKeyboardKey.arrowUp,
  LogicalKeyboardKey.arrowRight: PhysicalKeyboardKey.arrowRight,
  LogicalKeyboardKey.arrowLeft: PhysicalKeyboardKey.arrowLeft,
  LogicalKeyboardKey.home: PhysicalKeyboardKey.home,
  LogicalKeyboardKey.end: PhysicalKeyboardKey.end,
  LogicalKeyboardKey.pageUp: PhysicalKeyboardKey.pageUp,
  LogicalKeyboardKey.pageDown: PhysicalKeyboardKey.pageDown,
  LogicalKeyboardKey.delete: PhysicalKeyboardKey.delete,
  LogicalKeyboardKey.insert: PhysicalKeyboardKey.insert,
  LogicalKeyboardKey.f1: PhysicalKeyboardKey.f1,
  LogicalKeyboardKey.f2: PhysicalKeyboardKey.f2,
  LogicalKeyboardKey.f3: PhysicalKeyboardKey.f3,
  LogicalKeyboardKey.f4: PhysicalKeyboardKey.f4,
  LogicalKeyboardKey.f5: PhysicalKeyboardKey.f5,
  LogicalKeyboardKey.f6: PhysicalKeyboardKey.f6,
  LogicalKeyboardKey.f7: PhysicalKeyboardKey.f7,
  LogicalKeyboardKey.f8: PhysicalKeyboardKey.f8,
  LogicalKeyboardKey.f9: PhysicalKeyboardKey.f9,
  LogicalKeyboardKey.f10: PhysicalKeyboardKey.f10,
  LogicalKeyboardKey.f11: PhysicalKeyboardKey.f11,
  LogicalKeyboardKey.f12: PhysicalKeyboardKey.f12,
  LogicalKeyboardKey.keyC: PhysicalKeyboardKey.keyC,
  LogicalKeyboardKey.bracketLeft: PhysicalKeyboardKey.bracketLeft,
  LogicalKeyboardKey.backslash: PhysicalKeyboardKey.backslash,
  LogicalKeyboardKey.bracketRight: PhysicalKeyboardKey.bracketRight,
  LogicalKeyboardKey.minus: PhysicalKeyboardKey.minus,
  LogicalKeyboardKey.space: PhysicalKeyboardKey.space,
  LogicalKeyboardKey.keyB: PhysicalKeyboardKey.keyB,
  LogicalKeyboardKey.period: PhysicalKeyboardKey.period,
};

PhysicalKeyboardKey _physicalKey(LogicalKeyboardKey key) =>
    _usPhysicalKeys[key] ??
    (throw StateError('缺少 US 物理键映射：0x${key.keyId.toRadixString(16)}'));

List<int> _esc(String sequence) => [0x1b, ...sequence.codeUnits];

String _hex(List<int> bytes) => bytes.map((b) => b.toRadixString(16).padLeft(2, '0')).join(' ');
