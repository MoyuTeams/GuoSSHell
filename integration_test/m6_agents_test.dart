// M6 A 类：coding agent 在 App 里跑剧本（docs/acceptance-m6-2026-09-26.md §3 A）。
//
// --dart-define=M6_AGENTS=claude,codex,opencode   跑哪些 agent（默认全部）
// --dart-define=M6_SCENARIOS=stream,burst,…       跑哪些场景（默认全部；inline- 前缀 = 行内模式；
//                                                 给空值则只跑多窗格与 S5）
import 'dart:convert';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:terminal_view/terminal_view.dart' show TerminalKey;

import 'm6_harness.dart';

const _agents = String.fromEnvironment('M6_AGENTS', defaultValue: 'claude,codex,opencode');
const _scenarios = String.fromEnvironment(
  'M6_SCENARIOS',
  defaultValue: 'stream,burst,tools,subagents,cjk,long,inline-stream,inline-cjk',
);

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.fullyLive;
  final report = <String, Object>{'device': m6Device};

  setUpAll(startRust);
  // 截图也存在 reportData 里（screenshots）：合并，不覆盖。
  tearDownAll(() => binding.reportData = {...?binding.reportData, 'agents': report});

  for (final agent in _agents.split(',').where((a) => a.isNotEmpty)) {
    for (final entry in _scenarios.split(',').where((e) => e.isNotEmpty)) {
      final inline = entry.startsWith('inline-');
      final scenario = inline ? entry.substring('inline-'.length) : entry;
      testWidgets('A：$agent × $entry', (tester) async {
        final app = M6App(tester, binding);
        final result = <String, Object>{};
        report['$agent/$entry'] = result;
        await app.open();
        await app.waitScreen('probe@');
        await app.type('clear\r');
        await app.waitIdle();

        await app.flushPerf();
        final mark = app.perfMark;
        final started = DateTime.now();
        await app.type('m6-agent $agent $scenario${inline ? ' --inline' : ''}\r');

        if (scenario == 'stream' || scenario == 'burst') {
          // 流式开始后在 agent 的输入框里打字：从敲下最后一个字符到它出现在屏幕上。
          await app.waitForPlan(
            (p) => p['kind'] == 'main' && p['dialect'] == agent && p['scenario'] == scenario,
            '$agent 开始流式输出',
            since: started,
          );
          await tester.pump(const Duration(milliseconds: 400));
          result['typing_echo_ms'] = await app.echoMs('zq6typ');
        }
        if (scenario == 'subagents') {
          await app.waitForPlan(
            (p) => p['kind'] == 'sub' && p['dialect'] == agent && p['scenario'] == 'subagents',
            '$agent 派出 subagent',
            since: started,
          );
          await tester.pump(const Duration(seconds: 1));
          await _switchViews(app, agent);
        }

        await app.waitScreen('M6-DONE', timeout: Duration(seconds: scenario == 'long' ? 240 : 120));
        result['duration_ms'] = DateTime.now().difference(started).inMilliseconds;
        await app.waitIdle(quiet: const Duration(milliseconds: 1500));
        // 自检顺带交出最后一个统计窗口，之后再汇总。
        final problems = await app.screenMismatches();
        await tester.pump(const Duration(milliseconds: 50));
        result['perf'] = summarizePerf(app.perfSince(mark));
        await app.screenshot('agent-$agent-$entry');
        final garbage = screenGarbage(app.screenText());
        result['mismatches'] = problems;
        result['garbage'] = garbage;

        // 退出 agent：回到 shell，终端模式都复位（S5 的正常退出）。
        await _exitAgent(app, agent);
        await app.type('clear; m6-probe\r');
        await app.waitScreen('M6-MODES', timeout: const Duration(seconds: 10));
        final line = app.logicalLines().firstWhere((l) => l.contains('M6-MODES'));
        final modes = jsonDecode(line.substring(line.indexOf('{'))) as Map<String, dynamic>;
        result['modes_after'] = modes;

        expect(problems, isEmpty, reason: problems.join('\n'));
        expect(garbage, isEmpty, reason: garbage.join('\n'));
        expect(modes['1049'], 2, reason: '退出 agent 后不在备用屏');
        for (final mouse in ['1000', '1002', '1003']) {
          expect(modes[mouse], 2, reason: '鼠标上报 $mouse 应已关闭');
        }
        expect(modes['25'], 1, reason: '光标可见');
        await app.dispose();
      });
    }
  }

  testWidgets('多窗格：一个窗格高速输出，另一个窗格打字的回显延迟', (tester) async {
    final app = M6App(tester, binding);
    final result = <String, Object>{};
    report['panes'] = result;
    await app.open();
    await app.waitScreen('probe@');
    // ⌘D 左右分屏，从选择器里再开一个同样的连接。
    app.pane.focusNode.requestFocus();
    await tester.pump(const Duration(milliseconds: 200));
    // 物理键显式给出：profile 构建里按逻辑键反查物理键会失败（键名只在 debug 里有）。
    await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft, physicalKey: PhysicalKeyboardKey.metaLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyD, physicalKey: PhysicalKeyboardKey.keyD);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft, physicalKey: PhysicalKeyboardKey.metaLeft);
    await app.waitFor(() => find.textContaining('再开一个').evaluate().isNotEmpty, '连接选择器');
    // 等选择器弹出的动画走完再点；没点中（选择器还在）就再点。
    for (var attempt = 0; attempt < 3 && find.textContaining('再开一个').evaluate().isNotEmpty; attempt++) {
      await tester.pump(const Duration(milliseconds: 600));
      await tester.tap(find.textContaining('再开一个'), warnIfMissed: false);
      await tester.pump(const Duration(milliseconds: 600));
    }
    await app.waitFor(() => app.panes.length == 2 && app.panes.every((p) => p.connected), '第二个窗格连上');
    final busy = app.panes[0];
    final quiet = app.panes[1];
    await app.waitScreen('probe@', pane: quiet);

    // 右边先量一组不受干扰的回显作对照。
    Future<List<int>> echoes(String prefix) async {
      final samples = <int>[];
      var typed = '';
      for (var i = 0; i < 12; i++) {
        typed += String.fromCharCode(0x61 + i);
        samples.add(await app.echoMs(typed.substring(typed.length - 1), expect: '$prefix$typed', pane: quiet));
        await tester.pump(const Duration(milliseconds: 300));
      }
      await app.type('\x15', pane: quiet); // Ctrl-U 清掉这一行
      return samples..sort();
    }

    int p95(List<int> sorted) => sorted[(sorted.length * 95 ~/ 100).clamp(0, sorted.length - 1)];
    await app.type('#', pane: quiet);
    final idle = await echoes('#');
    result['echo_idle_ms'] = idle;
    result['echo_idle_ms_p95'] = p95(idle);

    // 左边：2000 行 / 秒的彩色输出，持续 20 秒；右边逐个字符敲，量从送出到它出现在屏幕上。
    await app.flushPerf(pane: busy);
    final mark = app.perfMark;
    await app.type('m6-flood 2000 20\r', pane: busy);
    await tester.pump(const Duration(seconds: 2));
    await app.type('#', pane: quiet);
    final samples = await echoes('#');
    result['echo_ms'] = samples;
    result['echo_ms_p95'] = p95(samples);
    await app.waitScreen('M6-FLOOD-DONE', pane: busy, timeout: const Duration(seconds: 60));
    await app.flushPerf(pane: busy);
    result['busy_perf'] = summarizePerf(app.perfSince(mark, pane: busy));
    await app.screenshot('panes-flood');
    await app.dispose();
  });

  testWidgets('S5：流式中直接杀掉 agent，回到 shell 后画面不卡住、reset 能恢复', (tester) async {
    final app = M6App(tester, binding);
    await app.open();
    await app.waitScreen('probe@');
    // 杀的是 agent 本身（Codex 的原生进程）：只杀 npm 的启动器，原生进程会成孤儿接着跑。
    await app.type("clear; (sleep 4; pkill -KILL -f 'codex-linux-.*\\[m6:burst\\]') & m6-agent codex burst; "
        "echo M6-AGENT-KILLED-\$?\r");
    await app.waitScreen('M6-AGENT-KILLED-', timeout: const Duration(seconds: 20));
    report['killed_left_alt_screen'] = app.pane.terminal.isUsingAltBuffer;
    await app.screenshot('agent-codex-killed');
    await app.type('reset; echo M6-RESET-DONE\r');
    await app.waitScreen('M6-RESET-DONE', timeout: const Duration(seconds: 10));
    expect(app.pane.terminal.isUsingAltBuffer, isFalse);
    await app.dispose();
  });
}

/// S3：subagent 还在刷新时切换视图。
Future<void> _switchViews(M6App app, String agent) async {
  final tester = app.tester;
  Future<void> shot(String name) => app.screenshot('agent-$agent-switch-$name');
  switch (agent) {
    case 'claude':
      // ↓ 进后台 agent 列表，Enter 查看选中的 agent，Esc 回到主视图。
      await app.key(TerminalKey.arrowDown);
      await tester.pump(const Duration(milliseconds: 600));
      await app.key(TerminalKey.enter);
      await tester.pump(const Duration(seconds: 1));
      await shot('child');
      await app.key(TerminalKey.escape);
      await tester.pump(const Duration(seconds: 1));
      await app.key(TerminalKey.keyO, ctrl: true);
      await tester.pump(const Duration(seconds: 1));
      await shot('transcript');
      await app.key(TerminalKey.keyO, ctrl: true);
    case 'codex':
      // 输入框为空时 Alt+← / → 在 agent 线程之间切换。
      for (var i = 0; i < 2; i++) {
        await app.key(TerminalKey.arrowRight, alt: true);
        await tester.pump(const Duration(milliseconds: 800));
      }
      await shot('child');
      for (var i = 0; i < 2; i++) {
        await app.key(TerminalKey.arrowLeft, alt: true);
        await tester.pump(const Duration(milliseconds: 800));
      }
    case 'opencode':
      // Ctrl+X ↓ 进第一个子会话；子会话里 ← / → 在子会话之间切换，↑ 回父会话（不带引导键）。
      await app.key(TerminalKey.keyX, ctrl: true);
      await app.key(TerminalKey.arrowDown);
      await tester.pump(const Duration(seconds: 1));
      await shot('child');
      for (final key in [TerminalKey.arrowRight, TerminalKey.arrowRight, TerminalKey.arrowLeft]) {
        await app.key(key);
        await tester.pump(const Duration(milliseconds: 800));
      }
      await shot('child-cycled');
      await app.key(TerminalKey.arrowUp);
      await tester.pump(const Duration(seconds: 1));
  }
}

/// 退出 agent，等回到 shell 提示符。
Future<void> _exitAgent(M6App app, String agent) async {
  final tester = app.tester;
  bool atShell() => !app.pane.terminal.isUsingAltBuffer && app.screen().reversed.any((l) => l.startsWith('probe@'));
  for (var attempt = 0; attempt < 6 && !atShell(); attempt++) {
    if (agent == 'opencode' && attempt.isOdd) {
      await app.key(TerminalKey.keyD, ctrl: true);
    } else {
      await app.key(TerminalKey.keyC, ctrl: true);
    }
    await tester.pump(const Duration(milliseconds: 700));
  }
  await app.waitFor(atShell, '$agent 退回 shell', timeout: const Duration(seconds: 15));
}
