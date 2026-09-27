// M6 集成测试的公共部分（docs/acceptance-m6-2026-09-26.md §2.6）：在真机 / 模拟器 / macOS 上驱动
// 真实 App，连验收服务器（scripts/sshd-test.sh up），读 App 自己的终端缓冲断言。
//
// 由 scripts/m6.sh 经 flutter drive 运行；--dart-define：
//   M6_HOST / M6_PORT   验收服务器（默认 127.0.0.1:2223）
//   M6_DEVICE           设备名，截图与报告按它分目录
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/terminal/perf_monitor.dart';
import 'package:guosh_shell/src/terminal/session_target.dart';
import 'package:guosh_shell/src/terminal/terminal_pane.dart';
import 'package:guosh_shell/src/workspace/workspace_page.dart';
import 'package:integration_test/integration_test.dart';
import 'package:path_provider/path_provider.dart';
import 'package:rinf/rinf.dart';
import 'package:terminal_view/terminal_view.dart' show BufferLine, TerminalKey;

const m6Host = String.fromEnvironment('M6_HOST', defaultValue: '127.0.0.1');
const m6Port = int.fromEnvironment('M6_PORT', defaultValue: 2223);
const m6Device = String.fromEnvironment('M6_DEVICE', defaultValue: 'device');
// iOS 测试插件会保留截图原图；内存与卡顿门槛用关闭截图的独立轮次判定。
const m6CaptureScreenshots = bool.fromEnvironment('M6_SCREENSHOTS', defaultValue: true);

/// 验收服务器上假 AI 上游的地址；默认与 SSH 使用同一主机，真机可以访问局域网测试台。
const m6LlmHost = String.fromEnvironment('M6_LLM_HOST', defaultValue: m6Host);
const m6LlmPort = int.fromEnvironment('M6_LLM_PORT', defaultValue: 2224);

Uri m6Api(String path, {Map<String, String>? query}) =>
    Uri(scheme: 'http', host: m6LlmHost, port: m6LlmPort, path: path, queryParameters: query);

bool _rustStarted = false;

/// 起 Rust 侧并等设置到位（与 App 启动时的 _StartupGate 相同的顺序）。
Future<void> startRust() async {
  if (_rustStarted) return;
  _rustStarted = true;
  await initializeRust(assignRustSignal);
  final ready = AppReady.rustSignalStream.first;
  final settings = SettingsState.rustSignalStream.first;
  final directory = await getApplicationSupportDirectory();
  AppStart(supportDir: directory.path).sendSignalToRust();
  final ok = (await ready).message;
  if (!ok.ok) throw StateError('AppReady: ${ok.detail}');
  SettingsQuery().sendSignalToRust();
  await settings;
}

/// 一次测试里的 App：一个工作区、若干窗格。
class M6App {
  M6App(this.tester, this.binding) {
    _frames = FrameUpdate.rustSignalStream.listen((pack) {
      _lastFrame[pack.message.sessionId] = DateTime.now();
    });
  }

  final WidgetTester tester;
  final IntegrationTestWidgetsFlutterBinding binding;
  late final StreamSubscription<RustSignalPack<FrameUpdate>> _frames;
  final Map<int, DateTime> _lastFrame = {};

  /// 打开工作区并连上验收服务器（首次连接自动信任主机密钥，密钥变了就替换）。
  Future<TerminalPaneController> open({String command = ''}) async {
    await tester.pumpWidget(MaterialApp(
      theme: ThemeData(brightness: Brightness.dark, colorScheme: ColorScheme.fromSeed(seedColor: Colors.teal, brightness: Brightness.dark)),
      home: WorkspacePage(
        initial: SessionTarget.quick(
          host: m6Host,
          port: m6Port,
          username: 'probe',
          password: 'probe',
          command: command,
        ),
      ),
    ));
    return waitConnected();
  }

  /// 活动窗格（第一个）。
  TerminalPaneController get pane => panes.first;

  List<TerminalPaneController> get panes =>
      tester.widgetList<TerminalPane>(find.byType(TerminalPane)).map((w) => w.controller).toList();

  /// 等窗格连上；路上遇到主机密钥确认就替用户点掉。
  Future<TerminalPaneController> waitConnected({TerminalPaneController? of}) async {
    final deadline = DateTime.now().add(const Duration(seconds: 60));
    var retries = 0;
    while (true) {
      await tester.pump(const Duration(milliseconds: 100));
      if (find.text('信任并连接').evaluate().isNotEmpty) {
        await tester.tap(find.text('信任并连接'));
        continue;
      }
      if (find.text('替换旧密钥并连接').evaluate().isNotEmpty) {
        final checkbox = find.byType(Checkbox);
        if (checkbox.evaluate().isNotEmpty) await tester.tap(checkbox.first);
        await tester.pump(const Duration(milliseconds: 300));
        final replace = find.text('替换旧密钥并连接');
        if (replace.evaluate().isNotEmpty) await tester.tap(replace.first);
        continue;
      }
      // 首次局域网授权完成后，系统可能已让原来的连接失败；从界面重新发起连接。
      final retry = find.text('重试');
      if (retry.evaluate().isNotEmpty && retries < 3) {
        retries++;
        await tester.tap(retry.first);
        await tester.pump(const Duration(milliseconds: 500));
        continue;
      }
      final panes = this.panes;
      final pane = of ?? (panes.isEmpty ? null : panes.first);
      if (pane != null && pane.connected) return pane;
      if (DateTime.now().isAfter(deadline)) {
        throw TestFailure('连接验收服务器超时（$m6Host:$m6Port，先 ./scripts/sshd-test.sh up）');
      }
    }
  }

  /// 送出 [text]（直接交给终端的文本输入，不先 pump），量到屏幕上出现 [expect]（默认就是
  /// [text]）的毫秒数；每帧查一次。按软换行接回的逻辑行找（窄窗格里一行会折开）。
  Future<int> echoMs(String text, {String? expect, TerminalPaneController? pane}) async {
    final controller = pane ?? this.pane;
    final target = expect ?? text;
    final watch = Stopwatch()..start();
    controller.terminal.textInput(text);
    while (!logicalLines(pane: controller).any((line) => line.contains(target))) {
      if (watch.elapsed > const Duration(seconds: 10)) {
        await screenshot('timeout-${DateTime.now().millisecondsSinceEpoch}');
        throw TestFailure('等回显「$target」超时。${describe(pane: controller)}\n当前屏幕：\n${screenText(pane: controller)}');
      }
      await tester.pump(const Duration(milliseconds: 1));
    }
    return watch.elapsedMilliseconds;
  }

  /// 让 Rust 交出当前的统计窗口（画面一致性自检的请求顺带交出，不等满 5 秒）：
  /// 量一段之前调一次丢掉旧窗口，量完再调一次收齐。
  Future<void> flushPerf({TerminalPaneController? pane}) async {
    final controller = pane ?? this.pane;
    final reply = ScreenCheck.rustSignalStream.firstWhere((p) => p.message.sessionId == controller.sessionId);
    ScreenCheckRequest(sessionId: controller.sessionId).sendSignalToRust();
    await reply.timeout(const Duration(seconds: 10));
    await tester.pump(const Duration(milliseconds: 50));
  }

  /// 在窗格里敲一段文本（经 App 的文本输入通道，等同软键盘提交）；`\r` 即回车。
  Future<void> type(String text, {TerminalPaneController? pane}) async {
    (pane ?? this.pane).terminal.textInput(text);
    await tester.pump(const Duration(milliseconds: 50));
  }

  /// 按一个功能键（经 App 的键编码通道）。
  Future<void> key(TerminalKey key, {bool ctrl = false, bool alt = false, bool shift = false, TerminalPaneController? pane}) async {
    (pane ?? this.pane).terminal.keyInput(key, ctrl: ctrl, alt: alt, shift: shift);
    await tester.pump(const Duration(milliseconds: 50));
  }

  /// 当前屏幕（不含滚回）的每一行文本，宽字符的第二列不重复。
  List<String> screen({TerminalPaneController? pane}) {
    final terminal = (pane ?? this.pane).terminal;
    final top = terminal.screenTopIndex;
    return [
      for (var row = 0; row < terminal.viewHeight; row++) _lineText(terminal.lineAt(top + row), terminal.viewWidth),
    ];
  }

  String screenText({TerminalPaneController? pane}) => screen(pane: pane).join('\n');

  /// 当前屏幕按软换行接回的逻辑行（一行输出比屏幕宽时跨几行）。
  List<String> logicalLines({TerminalPaneController? pane}) {
    final terminal = (pane ?? this.pane).terminal;
    final top = terminal.screenTopIndex;
    final lines = <String>[];
    var current = StringBuffer();
    for (var row = 0; row < terminal.viewHeight; row++) {
      final line = terminal.lineAt(top + row);
      final text = _lineText(line, terminal.viewWidth, trim: !line.isWrapped);
      current.write(text);
      if (!line.isWrapped) {
        lines.add(current.toString());
        current = StringBuffer();
      }
    }
    if (current.isNotEmpty) lines.add(current.toString());
    return lines;
  }

  /// 终端状态（等待超时时一并打出，便于判断是画面问题还是测试问题）。
  String describe({TerminalPaneController? pane}) {
    final controller = pane ?? this.pane;
    final t = controller.terminal;
    return '会话 ${controller.sessionId} ${controller.state} · ${t.viewWidth}x${t.viewHeight} · '
        '行数 ${t.height} · 屏幕首行 ${t.screenTopIndex} · 最早一行 ${t.firstStableRow} · '
        '窗口盖住屏幕 ${t.windowCovers(t.screenTopIndex, t.viewHeight)} · 备用屏 ${t.isUsingAltBuffer}';
  }

  /// 等屏幕上出现 [text]（或满足 [test]）。
  Future<Duration> waitScreen(String text, {Duration timeout = const Duration(seconds: 60), TerminalPaneController? pane}) async {
    final start = DateTime.now();
    final deadline = start.add(timeout);
    while (!screenText(pane: pane).contains(text)) {
      if (DateTime.now().isAfter(deadline)) {
        await screenshot('timeout-${DateTime.now().millisecondsSinceEpoch}');
        throw TestFailure('等「$text」超时。${describe(pane: pane)}\n当前屏幕：\n${screenText(pane: pane)}');
      }
      await tester.pump(const Duration(milliseconds: 50));
    }
    return DateTime.now().difference(start);
  }

  /// 等条件成立。
  Future<void> waitFor(bool Function() done, String what, {Duration timeout = const Duration(seconds: 30)}) async {
    final deadline = DateTime.now().add(timeout);
    while (!done()) {
      if (DateTime.now().isAfter(deadline)) {
        await screenshot('timeout-${DateTime.now().millisecondsSinceEpoch}');
        throw TestFailure('等「$what」超时。${describe()}\n当前屏幕：\n${screenText()}');
      }
      await tester.pump(const Duration(milliseconds: 50));
    }
  }

  /// 等假上游做出某个剧本决定（/m6/plans）：[since] 之后出现满足 [test] 的一条。
  /// 用它判断「agent 开始流式」「subagent 已派出」，不依赖各 agent 界面上的写法。
  Future<void> waitForPlan(bool Function(Map<String, dynamic> plan) test, String what,
      {required DateTime since, Duration timeout = const Duration(seconds: 90)}) async {
    final deadline = DateTime.now().add(timeout);
    final client = HttpClient();
    try {
      while (true) {
        try {
          final request = await client.getUrl(m6Api('/m6/plans', query: {'limit': '200'}));
          final response = await request.close();
          final plans = jsonDecode(await response.transform(utf8.decoder).join()) as List<dynamic>;
          final hit = plans.cast<Map<String, dynamic>>().any(
                (plan) => (plan['t'] as int) >= since.millisecondsSinceEpoch && test(plan),
              );
          if (hit) return;
        } on Object {
          // 服务器暂时连不上：下一轮再问。
        }
        if (DateTime.now().isAfter(deadline)) {
          await screenshot('timeout-${DateTime.now().millisecondsSinceEpoch}');
          throw TestFailure('等「$what」超时（假上游没有对应的剧本决定）。当前屏幕：\n${screenText()}');
        }
        await tester.pump(const Duration(milliseconds: 200));
      }
    } finally {
      client.close(force: true);
    }
  }

  /// 等输出静止：[quiet] 内没有新帧。
  Future<void> waitIdle({Duration quiet = const Duration(milliseconds: 800), TerminalPaneController? pane}) async {
    final sessionId = (pane ?? this.pane).sessionId;
    final deadline = DateTime.now().add(const Duration(seconds: 60));
    while (true) {
      await tester.pump(const Duration(milliseconds: 100));
      final last = _lastFrame[sessionId];
      if (last == null || DateTime.now().difference(last) >= quiet) return;
      if (DateTime.now().isAfter(deadline)) return;
    }
  }

  /// 画面一致性：Rust 最近发出的一帧（逐列）与 Dart 画完同一序号的帧后的行池逐列比对，
  /// 返回不一致之处（空 = 一致）。按帧序号对齐，所以画面还在刷新（TUI 定时重画）也能比。
  Future<List<String>> screenMismatches({TerminalPaneController? pane, int attempts = 5}) async {
    final controller = pane ?? this.pane;
    List<String>? problems;
    for (var attempt = 0; attempt < attempts; attempt++) {
      problems = await _compareOnce(controller);
      if (problems != null) return problems;
    }
    throw TestFailure('画面一致性：$attempts 次都没能与 Rust 发出的帧对齐（${describe(pane: controller)}）');
  }

  /// 对齐不上（Dart 已经画了更新的帧）返回 null，由调用方重试。
  Future<List<String>?> _compareOnce(TerminalPaneController controller) async {
    final reply = ScreenCheck.rustSignalStream.firstWhere((p) => p.message.sessionId == controller.sessionId);
    ScreenCheckRequest(sessionId: controller.sessionId).sendSignalToRust();
    final check = (await reply.timeout(const Duration(seconds: 10))).message;
    final deadline = DateTime.now().add(const Duration(seconds: 2));
    while (controller.lastFrameSeq != check.seq) {
      if (controller.lastFrameSeq > check.seq || DateTime.now().isAfter(deadline)) return null;
      await tester.pump(const Duration(milliseconds: 16));
    }
    // 同一轮事件里比对：此刻行池正是这一帧。
    final terminal = controller.terminal;
    final problems = <String>[];
    for (var row = 0; row < check.rows.length; row++) {
      final index = terminal.indexOfStable(check.stableRows[row]);
      if (index == null) continue;
      final engine = check.rows[row];
      final line = terminal.lineAt(index);
      for (var col = 0; col < check.cols; col++) {
        final expected = _blank(col < engine.length ? engine[col] : '');
        final actual = _blank(_cellText(line, col));
        if (expected != actual) {
          problems.add('第 $row 行第 $col 列：Rust「$expected」App「$actual」');
          if (problems.length > 20) return problems;
        }
      }
    }
    return problems;
  }

  /// 截图（flutter drive 的驱动端落盘到 build/m6/screenshots/<设备>/）。平台不支持截图时
  /// （macOS 的 integration_test）跳过，不影响断言。
  Future<void> screenshot(String name) async {
    binding.reportData ??= <String, dynamic>{};
    binding.reportData!['capture_screenshots'] = m6CaptureScreenshots;
    if (!m6CaptureScreenshots) return;
    await tester.pump();
    try {
      await binding.takeScreenshot('$m6Device/$name');
    } on Object catch (error) {
      debugPrint('[m6] 截图跳过（$name）：$error');
    }
  }

  /// 从 [since] 起收集到的性能窗口（PerfMonitor）。
  List<PerfRecord> perfSince(int since, {TerminalPaneController? pane}) {
    final sessionId = (pane ?? this.pane).sessionId;
    return PerfMonitor.instance.records.skip(since).where((r) => r.sessionId == sessionId).toList();
  }

  int get perfMark => PerfMonitor.instance.records.length;

  Future<void> dispose() async {
    await _frames.cancel();
    await tester.pumpWidget(const SizedBox.shrink());
    await tester.pump(const Duration(milliseconds: 300));
  }
}

/// 清空 m6-keyecho 的输入记录（验收服务器的 /m6/input）。
Future<void> resetInput() async {
  final client = HttpClient();
  try {
    final request = await client.postUrl(m6Api('/m6/input/reset'));
    await (await request.close()).drain<void>();
  } finally {
    client.close(force: true);
  }
}

/// m6-keyecho 记下的输入：每行一条 `{"t": 毫秒, "hex": "..."}`，按顺序拼成字节。
Future<List<int>> inputBytes() async {
  final client = HttpClient();
  try {
    final request = await client.getUrl(m6Api('/m6/input'));
    final body = await (await request.close()).transform(utf8.decoder).join();
    final bytes = <int>[];
    for (final line in const LineSplitter().convert(body)) {
      if (line.trim().isEmpty) continue;
      final hex = (jsonDecode(line) as Map<String, dynamic>)['hex'] as String;
      for (var i = 0; i + 1 < hex.length; i += 2) {
        bytes.add(int.parse(hex.substring(i, i + 2), radix: 16));
      }
    }
    return bytes;
  } finally {
    client.close(force: true);
  }
}

/// 字节里的 SGR 鼠标事件，每个写成「按钮码 + M/m @ 列,行」，如 `35M@12,5`。
List<String> sgrMouseEvents(List<int> bytes) => [
      for (final m in RegExp(r'\x1b\[<(\d+);(\d+);(\d+)([Mm])').allMatches(String.fromCharCodes(bytes)))
        '${m.group(1)}${m.group(4)}@${m.group(2)},${m.group(3)}',
    ];

/// 把几窗性能数据汇总成一条（写进报告）。
Map<String, Object> summarizePerf(List<PerfRecord> records) {
  if (records.isEmpty) return {'windows': 0};
  int maxOf(int Function(PerfRecord) f) => records.map(f).reduce((a, b) => a > b ? a : b);
  int sumOf(int Function(PerfRecord) f) => records.map(f).fold(0, (a, b) => a + b);
  final frames = sumOf((r) => r.frames);
  final windowMs = sumOf((r) => r.windowMs);
  return {
    'windows': records.length,
    'fps': windowMs == 0 ? 0 : double.parse((frames * 1000 / windowMs).toStringAsFixed(1)),
    'latency_us_p95_max': maxOf((r) => r.latencyUsP95),
    'latency_us_max': maxOf((r) => r.latencyUsMax),
    'ack_timeouts': sumOf((r) => r.ackTimeouts),
    'sync_timeouts': sumOf((r) => r.syncTimeouts),
    'render_us_max': maxOf((r) => r.renderUsMax),
    'apply_us_max': maxOf((r) => r.applyUsMax),
    'raster_us_p90_max': maxOf((r) => r.rasterUsP90),
    'build_us_p90_max': maxOf((r) => r.buildUsP90),
    'janky': sumOf((r) => r.janky),
    'flutter_frames': sumOf((r) => r.flutterFrames),
    'input_bytes': sumOf((r) => r.inputBytes),
    'rss_mb_max': maxOf((r) => r.rssBytes ~/ (1 << 20)),
    'rss_mb_first': records.first.rssBytes ~/ (1 << 20),
    'rss_mb_last': records.last.rssBytes ~/ (1 << 20),
  };
}

/// 屏幕上不该出现的东西：替换字符、泄漏成文字的转义序列。
List<String> screenGarbage(String screen) => [
      if (screen.contains('�')) '出现替换字符 U+FFFD',
      for (final m in RegExp(r'\[\?\d+[hl]|\]\d+;|\[\d+;\d+[Hr]').allMatches(screen).take(3)) '疑似转义序列泄漏：「${m.group(0)}」',
    ];

/// 旋转 / 改窗口的替身：改视图的物理尺寸（框架层；真实旋转在 XCUITest 里验）。系统键盘与安全区
/// 不跟着这个替身变——竖屏的软键盘还占着时宽高对调，工作区只剩几十像素，标签条与键位条放不下——
/// 所以先收起软键盘。
Future<void> resizeView(WidgetTester tester, Size logical) async {
  FocusManager.instance.primaryFocus?.unfocus();
  await tester.pump(const Duration(milliseconds: 600));
  final view = tester.view;
  view.physicalSize = logical * view.devicePixelRatio;
  await tester.pump(const Duration(milliseconds: 600));
}

Future<void> restoreView(WidgetTester tester) async {
  tester.view.resetPhysicalSize();
  await tester.pump(const Duration(milliseconds: 600));
}

Future<void> setOrientation(List<DeviceOrientation> orientations) =>
    SystemChrome.setPreferredOrientations(orientations);

String _cellText(BufferLine line, int col) {
  if (col >= line.length) return '';
  final code = line.getCodePoint(col);
  if (code == 0) return '';
  return String.fromCharCode(code) + (line.getCombined(col) ?? '');
}

String _lineText(BufferLine line, int cols, {bool trim = true}) {
  final out = StringBuffer();
  for (var col = 0; col < cols; col++) {
    final text = _cellText(line, col);
    if (text.isNotEmpty) {
      out.write(text);
    } else if (col == 0 || line.getCodePoint(col - 1) == 0 || _width(line, col - 1) == 1) {
      out.write(' ');
    }
  }
  return trim ? out.toString().trimRight() : out.toString();
}

int _width(BufferLine line, int col) => line.getWidth(col);

String _blank(String text) => text == ' ' ? '' : text;
