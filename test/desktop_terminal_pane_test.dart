import 'dart:async';
import 'dart:ui' as ui;

import 'package:fluent_ui/fluent_ui.dart' as f;
import 'package:flutter/foundation.dart'
    show debugDefaultTargetPlatformOverride;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter/rendering.dart' show RenderRepaintBoundary;
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/desktop/windows_chrome.dart';
import 'package:guosh_shell/src/desktop/workspace/desktop_workspace_scope.dart';
import 'package:guosh_shell/src/terminal/session_target.dart';
import 'package:guosh_shell/src/terminal/terminal_pane.dart';
import 'package:guosh_shell/src/terminal/desktop_terminal_theme.dart';
import 'package:guosh_shell/src/terminal/frame_terminal.dart';
import 'package:guosh_shell/src/terminal/frame.dart';
import 'package:guosh_shell/src/terminal/terminal_input_mode.dart';
import 'package:terminal_view/terminal_view.dart'
    show
        CellOffset,
        TerminalView,
        TerminalViewState,
        TerminalStyle,
        TerminalController;
import 'package:rinf/rinf.dart';

import 'desktop_session_controller_test.dart'
    show DesktopSessionTestGateway, prepared, u64;

Uint8List _colorBinary() => Uint8List.fromList([
  1, 0, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, //
  32, 1, 0, 1, 0, 1, 1, 1, 1, 0, 1, 0, 0, 0, 32, 2, 0, 1, 0, 2, 19, 87, //
  143, 2, 19, 87, 143, 0, 1, 0, 0, 0, 32,
]);

Uint8List _textBinary() => Uint8List.fromList([
  1, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 2, 0, 0, 0, //
  104, 105,
]);

Future<List<Color>> _cellPixels(
  WidgetTester tester,
  GlobalKey boundaryKey,
  TerminalViewState view,
) async {
  final boundary =
      boundaryKey.currentContext!.findRenderObject()! as RenderRepaintBoundary;
  final image = await tester.runAsync(() => boundary.toImage());
  try {
    final bytes = (await tester.runAsync(
      () => image!.toByteData(format: ui.ImageByteFormat.rawRgba),
    ))!;
    final render = view.renderTerminal;
    return [
      for (int column = 0; column < 3; column++)
        (() {
          final local =
              render.getOffset(CellOffset(column, 0)) +
              Offset(render.cellSize.width / 2, render.cellSize.height / 2);
          final point = boundary.globalToLocal(render.localToGlobal(local));
          final offset =
              (point.dy.floor() * image!.width + point.dx.floor()) * 4;
          return Color.fromARGB(
            bytes.getUint8(offset + 3),
            bytes.getUint8(offset),
            bytes.getUint8(offset + 1),
            bytes.getUint8(offset + 2),
          );
        })(),
    ];
  } finally {
    image!.dispose();
  }
}

/// 桌面 V2 窗格：测试网关代替 Rust，回执默认受理；Disconnect 后报 lifecycle ended。
class _PaneHarness {
  final gateway = DesktopSessionTestGateway();
  final controller = TerminalPaneController(
    const SessionTarget.quick(
      host: 'example.test',
      port: 22,
      username: 'test',
      profileId: 'selected',
    ),
  );
  final clipboardWrites = <String>[];
  final executed = <DesktopAction>[];
  bool ownsInput = false;
  int lifecycleRevision = 0;
  int attempt = 12;
  final paintKey = GlobalKey();

  /// 单条有序事件的回执；返回 null 用默认受理。
  DesktopEventResult? Function(DesktopOrdered request)? reply;
  DesktopSource get source => gateway.prepares.single.source;

  List<DesktopOrdered> where<T extends DesktopEvent>() => [
    for (final request in gateway.orderedEvents)
      if (request.event is T) request,
  ];

  void lifecycle(NativeLifecycleState state) => gateway.lifecycleBus.add(
    RustSignalPack(
      DesktopNativeLifecycle(
        version: 1,
        source: source,
        identity: 'frozen',
        attempt: u64(attempt),
        focusEpoch: u64(1),
        lifecycleRevision: u64(++lifecycleRevision),
        state: state,
      ),
      Uint8List(0),
    ),
  );

  void status(SessionState state) => gateway.statusBus.add(
    RustSignalPack(
      DesktopSessionStatus(
        source: source,
        identity: 'frozen',
        status: SessionStatus(
          sessionId: source.sessionId,
          state: state,
          failure: FailureKind.none,
          detail: '',
          localNetworkSettingsUrl: '',
          hint: ConnectHint.none,
        ),
      ),
      Uint8List(0),
    ),
  );

  Future<void> mount(
    WidgetTester tester, {
    double width = 1280,
    String scheme = 'default',
    TargetPlatform platform = TargetPlatform.windows,
  }) async {
    tester.view.physicalSize = Size(width, 900);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    debugDefaultTargetPlatformOverride = platform;
    final oldSettings = SettingsState.latestRustSignal;
    SettingsState.latestRustSignal = RustSignalPack(
      const SettingsState(
        fontSize: 14,
        minFontSize: 8,
        maxFontSize: 64,
        scrollbackLines: 1000,
        maxScrollbackLines: 10000,
        showKeyBar: false,
        keyBarRows: [],
      ),
      Uint8List(0),
    );
    addTearDown(() {
      SettingsState.latestRustSignal = oldSettings;
      debugDefaultTargetPlatformOverride = null;
    });
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    messenger.setMockMethodCallHandler(SystemChannels.platform, (call) async {
      if (call.method == 'Clipboard.setData') {
        clipboardWrites.add((call.arguments as Map)['text'] as String);
      }
      if (call.method == 'Clipboard.getData') {
        return {'text': '剪贴板'};
      }
      return null;
    });
    addTearDown(
      () => messenger.setMockMethodCallHandler(SystemChannels.platform, null),
    );
    gateway.onOrdered = (request) {
      if (request.event is DesktopEventDisconnect) {
        scheduleMicrotask(() => lifecycle(NativeLifecycleState.ended));
      }
      return reply?.call(request) ?? DesktopSessionTestGateway.accept(request);
    };
    await tester.pumpWidget(
      MaterialApp(
        theme: desktopMaterialTheme(),
        localizationsDelegates: const [f.FluentLocalizations.delegate],
        builder: (_, child) => f.FluentTheme(
          data: desktopFluentTheme(reduceMotion: true),
          child: child!,
        ),
        home: RepaintBoundary(
          key: paintKey,
          child: DesktopWorkspaceScope(
            execute: (action, {required origin}) async {
              expect(origin, same(controller));
              executed.add(action);
              return DesktopActionOutcome.executed;
            },
            ownsTerminalInput: (_, enabled) => ownsInput = enabled,
            isCurrentOrigin: (origin) => identical(origin, controller),
            child: TerminalPane(
              controller: controller,
              desktopGateway: gateway,
              onActivate: () {},
              onClose: () {},
            ),
          ),
        ),
      ),
    );
    await tester.pump();
    final snapshot = prepared(gateway.prepares.single.requestId);
    gateway.emit(
      snapshot.copyWith(
        source: source,
        snapshot: () => snapshot.snapshot!.copyWith(
          profile: snapshot.snapshot!.profile.copyWith(colorScheme: scheme),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(gateway.starts.length, 1);
    gateway.emit(
      snapshot.copyWith(
        source: source,
        requestId: gateway.starts.single.requestId,
        state: DesktopPreparationState.accepted,
        snapshot: () => null,
      ),
    );
    lifecycle(NativeLifecycleState.authenticated);
    status(SessionState.connected);
    await tester.pumpAndSettle();
    expect(ownsInput, isTrue);
    controller.focusNode.requestFocus();
    await tester.pumpAndSettle();
    expect(controller.focusNode.hasFocus, isTrue);
  }

  void frame({Uint8List? binary, bool selection = false, int seq = 1}) {
    final geometry = gateway.starts.single.geometry;
    gateway.projectionBus.add(
      RustSignalPack(
        DesktopNativeProjection(
          source: source,
          identity: 'frozen',
          attempt: u64(attempt),
          focusEpoch: u64(1),
          frameRevision: seq,
          appliedEffect: u64(0),
          rowRevision: u64(1),
          followBottom: true,
          firstStableRow: 0,
          screenTopStableRow: 0,
          viewportTop: 0,
          mouseReporting: false,
          bracketedPaste: false,
          selection: selection
              ? const NativeSelection(
                  startRow: 0,
                  startCol: 0,
                  endRow: 0,
                  endCol: 2,
                  rectangular: false,
                )
              : null,
        ),
        Uint8List(0),
      ),
    );
    gateway.frameBus.add(
      RustSignalPack(
        DesktopFrame(
          source: source,
          identity: 'frozen',
          frame: FrameUpdate(
            sessionId: source.sessionId,
            cols: geometry.cols,
            rows: geometry.rows,
            seq: seq,
            topStableRow: 0,
            firstStableRow: 0,
            screenTopStableRow: 0,
            cursorCol: -1,
            cursorRow: -1,
            title: '终端',
            mouseReporting: false,
            alternateScreen: false,
          ),
        ),
        binary ?? _textBinary(),
      ),
    );
  }

  Future<void> close(WidgetTester tester) async {
    await tester.pumpWidget(const SizedBox.shrink());
    await tester.pumpAndSettle();
    expect(ownsInput, isFalse);
    expect(where<DesktopEventDisconnect>(), hasLength(1));
    controller.dispose();
    await gateway.dispose();
    debugDefaultTargetPlatformOverride = null;
  }
}

void main() {
  testWidgets('桌面 V2：按键经有序通道发 Down/Up，seq 递增，本地快捷键交 Rust', (tester) async {
    final h = _PaneHarness();
    await h.mount(tester);
    expect(
      tester.widget<TerminalView>(find.byType(TerminalView)).shortcuts,
      isEmpty,
    );
    expect(h.controller.ownsNativeInput, isFalse);
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowUp);
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
    await tester.pumpAndSettle();
    final keys = [
      for (final request in h.where<DesktopEventKey>())
        request.event as DesktopEventKey,
    ];
    expect(
      [for (final key in keys) (key.key, key.phase)],
      [
        ('arrow_up', DesktopKeyPhase.down),
        ('arrow_up', DesktopKeyPhase.up),
        ('arrow_down', DesktopKeyPhase.down),
        ('arrow_down', DesktopKeyPhase.up),
      ],
    );
    expect(keys[0].physical, keys[1].physical);
    expect(
      keys[0].physical.toBigInt(),
      BigInt.from(PhysicalKeyboardKey.arrowUp.usbHidUsage),
    );
    final seqs = [
      for (final request in h.gateway.orderedEvents)
        request.seq.toBigInt().toInt(),
    ];
    expect(seqs, [for (var i = 1; i <= seqs.length; i++) i]);
    await h.close(tester);
    expect(tester.takeException(), isNull);
  });

  testWidgets('左右 Alt 分别上报 Known 来源', (tester) async {
    final h = _PaneHarness();
    await h.mount(tester);
    for (final alt in [
      LogicalKeyboardKey.altLeft,
      LogicalKeyboardKey.altRight,
    ]) {
      await tester.sendKeyDownEvent(alt);
      await tester.sendKeyEvent(LogicalKeyboardKey.arrowLeft);
      await tester.sendKeyUpEvent(alt);
    }
    await tester.pumpAndSettle();
    final downs = [
      for (final request in h.where<DesktopEventKey>())
        if ((request.event as DesktopEventKey).phase == DesktopKeyPhase.down)
          request.event as DesktopEventKey,
    ];
    expect(downs, hasLength(2));
    expect(
      [downs[0].alt, downs[0].leftAlt, downs[0].rightAlt],
      [true, true, false],
    );
    expect(
      [downs[1].alt, downs[1].leftAlt, downs[1].rightAlt],
      [true, false, true],
    );
    expect(
      downs.map((key) => key.altSource),
      everyElement(DesktopAltSource.known),
    );
    await h.close(tester);
  });

  testWidgets('Rust 回报 new_tab/split/本地终端动作经工作区执行入口，壳不重复执行', (tester) async {
    final h = _PaneHarness();
    await h.mount(tester);
    h.reply = (request) {
      final event = request.event;
      return event is DesktopEventKey && event.defaultAction != null
          ? DesktopSessionTestGateway.accept(
              request,
              action: event.defaultAction,
            )
          : null;
    };
    for (final key in [
      LogicalKeyboardKey.keyT,
      LogicalKeyboardKey.keyD,
      LogicalKeyboardKey.keyL,
    ]) {
      await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
      await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
      await tester.sendKeyEvent(key);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
    }
    await tester.pumpAndSettle();
    expect(h.executed, [
      DesktopAction.newTab,
      DesktopAction.splitRight,
      DesktopAction.newLocalTerminal,
    ]);
    final down = h
        .where<DesktopEventKey>()
        .map((request) => request.event as DesktopEventKey)
        .firstWhere((key) => key.phase == DesktopKeyPhase.down);
    expect(down.defaultAction, DesktopAction.newTab);
    expect([down.control, down.shift], [true, true]);
    await h.close(tester);
  });

  testWidgets('Busy 时按键按序排队、同 seq 重试，不丢键', (tester) async {
    final h = _PaneHarness();
    await h.mount(tester);
    var busy = 3;
    h.reply = (request) => request.event is DesktopEventKey && busy-- > 0
        ? DesktopSessionTestGateway.accept(
            request,
            failure: DesktopFailure.busy,
          )
        : null;
    final before = h.gateway.orderedEvents.length;
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowUp);
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowRight);
    await tester.pump(const Duration(milliseconds: 200));
    await tester.pumpAndSettle();
    final sent = h.gateway.orderedEvents.skip(before).toList();
    final keys = [
      for (final request in sent)
        if (request.event is DesktopEventKey)
          (
            request.seq.toBigInt().toInt(),
            (request.event as DesktopEventKey).key,
            (request.event as DesktopEventKey).phase,
          ),
    ];
    // 前三次为 Busy 的同一 seq，之后每个 seq 只出现一次并保持键序。
    expect(keys.take(4).map((key) => key.$1).toSet(), hasLength(1));
    final accepted = keys.skip(3).toList();
    expect(
      [for (final key in accepted) (key.$2, key.$3)],
      [
        ('arrow_up', DesktopKeyPhase.down),
        ('arrow_up', DesktopKeyPhase.up),
        ('arrow_right', DesktopKeyPhase.down),
        ('arrow_right', DesktopKeyPhase.up),
      ],
    );
    final seqs = [for (final key in accepted) key.$1];
    expect(seqs, [for (var i = 0; i < 4; i++) seqs.first + i]);
    await h.close(tester);
  });

  testWidgets('Copy 动作发有序 Copy 并写入剪贴板；Paste 读剪贴板后发有序 Paste', (tester) async {
    final h = _PaneHarness();
    await h.mount(tester);
    h.frame(selection: true);
    await tester.pumpAndSettle();
    expect(h.controller.canCopy, isTrue);
    expect(h.where<DesktopEventFrameAck>(), isNotEmpty);
    h.reply = (request) {
      final event = request.event;
      if (event is DesktopEventCopy) {
        scheduleMicrotask(
          () => h.gateway.clipboardBus.add(
            RustSignalPack(
              DesktopClipboard(
                source: h.source,
                identity: 'frozen',
                text: '完整🙂\n复制内容',
              ),
              Uint8List(0),
            ),
          ),
        );
      }
      return event is DesktopEventKey && event.defaultAction != null
          ? DesktopSessionTestGateway.accept(
              request,
              action: event.defaultAction,
            )
          : null;
    };
    await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
    await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyC);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyV);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
    await tester.pumpAndSettle();
    expect(h.where<DesktopEventCopy>(), hasLength(1));
    expect(h.clipboardWrites, ['完整🙂\n复制内容']);
    expect(
      h.where<DesktopEventPaste>().map(
        (request) => (request.event as DesktopEventPaste).text,
      ),
      ['剪贴板'],
    );
    expect(h.executed, isEmpty);
    await h.close(tester);
  });

  testWidgets('V2 损坏帧仍确认该序号并保留错误提示，下一正常帧恢复', (tester) async {
    final h = _PaneHarness();
    await h.mount(tester);
    List<int> acked() => [
      for (final request in h.where<DesktopEventFrameAck>())
        (request.event as DesktopEventFrameAck).frameSeq,
    ];
    h.frame(seq: 1);
    await tester.pumpAndSettle();
    expect(acked(), [1]);
    expect(h.controller.lastFrameSeq, 1);

    h.frame(binary: Uint8List.fromList([1, 0]), seq: 2);
    await tester.pumpAndSettle();
    expect(acked(), [1, 2]);
    expect(h.controller.lastFrameSeq, 1);
    expect(find.text('终端画面更新失败'), findsOneWidget);
    expect(find.text('画面帧 2 解码失败，等待下一帧'), findsOneWidget);
    // 状态重发不会覆盖解码错误。
    h.status(SessionState.connected);
    await tester.pumpAndSettle();
    expect(find.text('终端画面更新失败'), findsOneWidget);
    expect(acked(), [1, 2]);

    h.frame(seq: 3);
    await tester.pumpAndSettle();
    expect(acked(), [1, 2, 3]);
    expect(h.controller.lastFrameSeq, 3);
    expect(find.text('终端画面更新失败'), findsNothing);
    await h.close(tester);
    expect(tester.takeException(), isNull);
  });

  testWidgets('macOS ⌘C 交 Rust 仲裁后复制，Up 同名补发', (tester) async {
    final h = _PaneHarness();
    await h.mount(tester, platform: TargetPlatform.macOS);
    h.frame(selection: true);
    await tester.pumpAndSettle();
    h.reply = (request) {
      final event = request.event;
      if (event is DesktopEventCopy) {
        scheduleMicrotask(
          () => h.gateway.clipboardBus.add(
            RustSignalPack(
              DesktopClipboard(
                source: h.source,
                identity: 'frozen',
                text: 'hi',
              ),
              Uint8List(0),
            ),
          ),
        );
      }
      return event is DesktopEventKey && event.defaultAction != null
          ? DesktopSessionTestGateway.accept(
              request,
              action: event.defaultAction,
            )
          : null;
    };
    await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyC);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
    await tester.pumpAndSettle();
    final keys = [
      for (final request in h.where<DesktopEventKey>())
        request.event as DesktopEventKey,
    ];
    expect(
      [for (final key in keys) (key.key, key.phase, key.superKey)],
      [
        ('character:c', DesktopKeyPhase.down, true),
        ('character:c', DesktopKeyPhase.up, true),
      ],
    );
    expect(keys.first.defaultAction, DesktopAction.copy);
    expect(h.where<DesktopEventCopy>(), hasLength(1));
    expect(h.clipboardWrites, ['hi']);
    await h.close(tester);
  });

  testWidgets('静态配色 Prepared 实际进入 V2 renderer，重连换 attempt 清缓存保持原 profile', (
    tester,
  ) async {
    final h = _PaneHarness();
    await h.mount(tester, scheme: 'one_dark');
    h.frame(binary: _colorBinary());
    await tester.pumpAndSettle();
    final view = tester.widget<TerminalView>(find.byType(TerminalView));
    final key = tester.state<TerminalViewState>(find.byType(TerminalView));
    expect(view.theme, same(themeForProfile('one_dark')));
    final expected = [
      themeForProfile('one_dark').background,
      themeForProfile('one_dark').red,
      const Color.fromARGB(255, 19, 87, 143),
    ];
    expect(await _cellPixels(tester, h.paintKey, key), expected);
    final old = h.controller.terminal.lineAt(0);
    h.attempt = 13;
    h.lifecycle(NativeLifecycleState.connecting);
    await tester.pumpAndSettle();
    expect(h.controller.lastFrameSeq, 0);
    expect(h.controller.terminal.lineAt(0), isNot(same(old)));
    h.lifecycle(NativeLifecycleState.authenticated);
    h.frame(binary: _colorBinary());
    await tester.pumpAndSettle();
    expect(h.controller.lastFrameSeq, 1);
    expect(
      tester.widget<TerminalView>(find.byType(TerminalView)).theme,
      same(themeForProfile('one_dark')),
    );
    expect(await _cellPixels(tester, h.paintKey, key), expected);
    await h.close(tester);
    expect(tester.takeException(), isNull);
  });

  testWidgets('移动端仍走旧路径：不发 Prepare，不登记有序输入，保留本地快捷键', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.android;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final oldSettings = SettingsState.latestRustSignal;
    SettingsState.latestRustSignal = RustSignalPack(
      const SettingsState(
        fontSize: 14,
        minFontSize: 8,
        maxFontSize: 64,
        scrollbackLines: 1000,
        maxScrollbackLines: 10000,
        showKeyBar: false,
        keyBarRows: [],
      ),
      Uint8List(0),
    );
    addTearDown(() => SettingsState.latestRustSignal = oldSettings);
    final gateway = DesktopSessionTestGateway();
    final controller = TerminalPaneController(
      const SessionTarget.quick(host: 'nas.lan', port: 22, username: 'probe'),
    );
    var owns = false;
    await tester.pumpWidget(
      MaterialApp(
        home: DesktopWorkspaceScope(
          execute: (_, {required origin}) async =>
              DesktopActionOutcome.unavailable,
          ownsTerminalInput: (_, enabled) => owns = enabled,
          isCurrentOrigin: (_) => true,
          // 1×1 不产生几何，旧路径不会发 ConnectRequest（测试无 Native 动态库）。
          child: Align(
            alignment: Alignment.topLeft,
            child: SizedBox(
              width: 1,
              height: 1,
              child: TerminalPane(
                controller: controller,
                desktopGateway: gateway,
                onActivate: () {},
                onClose: () {},
              ),
            ),
          ),
        ),
      ),
    );
    await tester.pump();
    expect(gateway.prepares, isEmpty);
    expect(owns, isFalse);
    expect(controller.sessionId, greaterThan(0));
    expect(
      tester.widget<TerminalView>(find.byType(TerminalView)).shortcuts,
      isNotEmpty,
    );
    // 已取消的旧会话卸载时不发 DisconnectRequest。
    assignRustSignal['SessionStatus']!(
      SessionStatus(
        sessionId: controller.sessionId,
        state: SessionState.cancelled,
        failure: FailureKind.none,
        detail: '',
        localNetworkSettingsUrl: '',
        hint: ConnectHint.none,
      ).bincodeSerialize(),
      Uint8List(0),
    );
    await tester.pump();
    await tester.pumpWidget(const SizedBox.shrink());
    SessionStatus.latestRustSignal = null;
    controller.dispose();
    await gateway.dispose();
    expect(tester.takeException(), isNull);
    debugDefaultTargetPlatformOverride = null;
  });

  testWidgets(
    '静态配色多 viewport 同帧各自着色，新来源 reset/theme 更新不重放旧 Picture 或覆盖邻 pane',
    (tester) async {
      final terminal = [FrameTerminal(), FrameTerminal()];
      final selection = [TerminalController(), TerminalController()];
      final scroll = [ScrollController(), ScrollController()];
      final focus = [FocusNode(), FocusNode()];
      final keys = [
        GlobalKey<TerminalViewState>(),
        GlobalKey<TerminalViewState>(),
      ];
      final boundary = GlobalKey();
      final schemes = ValueNotifier(('one_dark', 'solarized_light'));
      final frame = decodeFrame(
        _colorBinary(),
        cols: 20,
        rows: 1,
        cursorCol: -1,
        cursorRow: -1,
        firstStableRow: 0,
        screenTopStableRow: 0,
      );
      for (final pane in terminal) {
        pane.applyFrame(frame);
      }
      await tester.pumpWidget(
        MaterialApp(
          home: RepaintBoundary(
            key: boundary,
            child: ValueListenableBuilder(
              valueListenable: schemes,
              builder: (_, chosen, _) => Row(
                children: [
                  for (int i = 0; i < 2; i++)
                    Expanded(
                      child: TerminalViewport(
                        viewKey: keys[i],
                        terminal: terminal[i],
                        controller: selection[i],
                        scrollController: scroll[i],
                        focusNode: focus[i],
                        style: const TerminalStyle(),
                        theme: themeForProfile(i == 0 ? chosen.$1 : chosen.$2),
                        backgroundOpacity: 1,
                        inputMode: TerminalInputMode.shared,
                        nativeInput: true,
                      ),
                    ),
                ],
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(await _cellPixels(tester, boundary, keys[0].currentState!), [
        themeForProfile('one_dark').background,
        themeForProfile('one_dark').red,
        const Color.fromARGB(255, 19, 87, 143),
      ]);
      expect(await _cellPixels(tester, boundary, keys[1].currentState!), [
        themeForProfile('solarized_light').background,
        themeForProfile('solarized_light').red,
        const Color.fromARGB(255, 19, 87, 143),
      ]);
      final old = terminal[1].lineAt(0);
      terminal[1].resetDisplay();
      terminal[1].applyFrame(frame);
      schemes.value = ('one_dark', 'dracula');
      await tester.pumpAndSettle();
      expect(terminal[1].lineAt(0), isNot(same(old)));
      expect(await _cellPixels(tester, boundary, keys[0].currentState!), [
        themeForProfile('one_dark').background,
        themeForProfile('one_dark').red,
        const Color.fromARGB(255, 19, 87, 143),
      ]);
      expect(await _cellPixels(tester, boundary, keys[1].currentState!), [
        themeForProfile('dracula').background,
        themeForProfile('dracula').red,
        const Color.fromARGB(255, 19, 87, 143),
      ]);
      await tester.pumpWidget(const SizedBox.shrink());
      schemes.dispose();
      for (int i = 0; i < 2; i++) {
        terminal[i].dispose();
        selection[i].dispose();
        scroll[i].dispose();
        focus[i].dispose();
      }
      expect(tester.takeException(), isNull);
    },
  );
}
