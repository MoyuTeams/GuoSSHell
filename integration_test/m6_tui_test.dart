// M6 B 类：全屏 TUI 的进入、运行、改尺寸、退出（docs/acceptance-m6-2026-09-26.md §3 B）。
import 'dart:convert';
import 'dart:ui';

import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

import 'm6_harness.dart';

/// 程序 → 进入全屏后屏幕上必有的字样（含窗格小于程序最小尺寸时它自己的提示）、退出按键。
const _tuis = <String, ({List<String> signature, List<String> quit})>{
  'btop': (signature: ['cpu', 'too small'], quit: ['q']),
  'nvtop': (signature: ['NVIDIA L40S'], quit: ['q']),
  'nload': (signature: ['Incoming'], quit: ['q']),
  'bmon': (signature: ['Interfaces', 'at least 48 columns'], quit: ['q', 'y']),
  'iftop': (signature: ['TX:', 'RX:'], quit: ['q']),
  'htop': (signature: ['PID', 'Load average'], quit: ['q']),
};

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.fullyLive;
  final report = <String, Object>{'device': m6Device};

  setUpAll(startRust);
  // 截图也存在 reportData 里（screenshots）：合并，不覆盖。
  tearDownAll(() => binding.reportData = {...?binding.reportData, 'tui': report});

  for (final entry in _tuis.entries) {
    final name = entry.key;
    final tui = entry.value;
    testWidgets('B1–B4：$name 进入、运行、改尺寸、退出', (tester) async {
      final app = M6App(tester, binding);
      final result = <String, Object>{};
      report[name] = result;
      await app.open();
      await app.waitScreen('probe@');
      await app.type('clear\r');
      await app.waitIdle();
      final heightBefore = app.pane.terminal.height;

      // B1 进入：备用屏、铺满、签名字样。
      await app.type('m6-tui $name\r');
      await app.waitFor(() => app.pane.terminal.isUsingAltBuffer, '$name 进入备用屏', timeout: const Duration(seconds: 15));
      await app.waitFor(() => tui.signature.any(app.screenText().contains), '$name 画出界面', timeout: const Duration(seconds: 15));
      expect(app.screenText(), isNot(contains('M6-BEFORE')), reason: '备用屏与主屏隔离');

      // B2 运行：6 秒的度量（高刷新下的帧率与显示延迟）。
      await app.flushPerf();
      final mark = app.perfMark;
      await tester.pump(const Duration(seconds: 6));
      await app.flushPerf();
      result['running'] = summarizePerf(app.perfSince(mark));
      await app.screenshot('tui-$name-running');

      // B3 改尺寸：宽高对调（旋转的替身），重排后画面与引擎一致、没有旧画面残留。
      final size = tester.view.physicalSize / tester.view.devicePixelRatio;
      final colsBefore = app.pane.terminal.viewWidth;
      await resizeView(tester, Size(size.height, size.width));
      await app.waitFor(() => app.pane.terminal.viewWidth != colsBefore, '$name 收到新尺寸', timeout: const Duration(seconds: 10));
      await tester.pump(const Duration(seconds: 2));
      result['resized_to'] = '${app.pane.terminal.viewWidth}x${app.pane.terminal.viewHeight}';
      // 这里改的是视图的物理尺寸（框架层的替身），系统截图不反映它；真实旋转的截图在 XCUITest 里。
      final resized = await app.screenMismatches();
      result['resized_mismatches'] = resized.length;
      await restoreView(tester);
      await tester.pump(const Duration(seconds: 2));

      // B4 退出：主屏原样回来，模式都复位，滚回里没有 TUI 画面。滚回只量退出这一段：窄屏上
      // 改尺寸时主屏的长行会按新宽度重新折行、挤进滚回，那不是 TUI 的画面。
      final heightBeforeQuit = app.pane.terminal.height;
      for (final key in tui.quit) {
        await app.type(key);
        await tester.pump(const Duration(milliseconds: 300));
      }
      await app.waitScreen('M6-MODES', timeout: const Duration(seconds: 15));
      await app.waitIdle();
      final terminal = app.pane.terminal;
      final line = app.logicalLines().firstWhere((l) => l.contains('M6-MODES'));
      final modes = jsonDecode(line.substring(line.indexOf('{'))) as Map<String, dynamic>;
      result['modes_after'] = modes;
      result['scrollback_growth'] = terminal.height - heightBeforeQuit;
      result['scrollback_growth_total'] = terminal.height - heightBefore;
      await app.screenshot('tui-$name-exited');
      expect(terminal.isUsingAltBuffer, isFalse);
      expect(app.screenText(), contains('M6-BEFORE $name'), reason: '进入前的画面要回来');
      expect(modes['1049'], 2, reason: '退出后不在备用屏');
      for (final mouse in ['1000', '1002', '1003', '1006']) {
        expect(modes[mouse], 2, reason: '鼠标上报 $mouse 应已关闭');
      }
      expect(modes['25'], 1, reason: '光标可见');
      expect(terminal.height - heightBeforeQuit, lessThan(12), reason: 'TUI 的画面不该进滚回');
      final problems = await app.screenMismatches();
      expect(problems, isEmpty, reason: problems.join('\n'));
      expect(resized, isEmpty, reason: '改尺寸后：${resized.join('\n')}');
      await app.dispose();
    });
  }

  testWidgets('B5：btop 被 SIGKILL 后画面不卡住，reset 能恢复', (tester) async {
    final app = M6App(tester, binding);
    await app.open();
    await app.waitScreen('probe@');
    await app.type('clear; timeout --foreground -s KILL 3 btop; echo M6-KILLED-\$?\r');
    await app.waitFor(() => app.pane.terminal.isUsingAltBuffer, 'btop 进入备用屏', timeout: const Duration(seconds: 10));
    // btop 每帧都包在同步输出里：被杀在一帧中途也要显示后面的输出。
    await app.waitScreen('M6-KILLED-137', timeout: const Duration(seconds: 10));
    await app.screenshot('tui-btop-killed');
    report['killed_left_alt_screen'] = app.pane.terminal.isUsingAltBuffer;
    await app.type('reset; echo M6-RESET-DONE\r');
    await app.waitScreen('M6-RESET-DONE', timeout: const Duration(seconds: 10));
    await app.waitIdle();
    expect(app.pane.terminal.isUsingAltBuffer, isFalse, reason: 'reset 之后回到主屏');
    await app.dispose();
  });
}
