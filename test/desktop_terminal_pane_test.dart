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
import 'package:guosh_shell/src/desktop/native_input/desktop_native_host.dart';
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
  1,
  0,
  3,
  0,
  0,
  0,
  0,
  0,
  0,
  0,
  0,
  0,
  0,
  0,
  0,
  1,
  0,
  0,
  0,
  0,
  1,
  0,
  0,
  0,
  32,
  1,
  0,
  1,
  0,
  1,
  1,
  1,
  1,
  0,
  1,
  0,
  0,
  0,
  32,
  2,
  0,
  1,
  0,
  2,
  19,
  87,
  143,
  2,
  19,
  87,
  143,
  0,
  1,
  0,
  0,
  0,
  32,
]);

Future<List<Color>> _cellPixels(
  WidgetTester tester,
  GlobalKey boundaryKey,
  GlobalKey<TerminalViewState> viewKey,
) async {
  final boundary =
      boundaryKey.currentContext!.findRenderObject()! as RenderRepaintBoundary;
  final image = await tester.runAsync(() => boundary.toImage());
  try {
    final bytes = (await tester.runAsync(
      () => image!.toByteData(format: ui.ImageByteFormat.rawRgba),
    ))!;
    final render = viewKey.currentState!.renderTerminal;
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
  final calls = <MethodCall>[];
  final clipboardWrites = <String>[];
  bool allowed = true;
  bool ownsInput = false;
  int panes = 0;
  int epoch = 3;
  int lifecycleRevision = 0;
  int attempt = 12;
  final paintKey = GlobalKey();
  String claimAction = 'copy';
  final paneVisible = ValueNotifier(true);
  Future<DesktopActionOutcome> Function(DesktopAction)? execute;
  Completer<Object?>? clipboardWrite;
  TargetPlatform? _oldPlatform;
  DesktopSource get source => gateway.prepares.single.source;

  void lifecycle(NativeLifecycleState state) => gateway.lifecycleBus.add(
    RustSignalPack(
      DesktopNativeLifecycle(
        version: 1,
        source: source,
        identity: 'frozen',
        attempt: u64(attempt),
        focusEpoch: u64(epoch),
        lifecycleRevision: u64(++lifecycleRevision),
        state: state,
      ),
      Uint8List(0),
    ),
  );

  Future<void> mount(
    WidgetTester tester, {
    double width = 1280,
    String scheme = 'default',
  }) async {
    tester.view.physicalSize = Size(width, 900);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    _oldPlatform = debugDefaultTargetPlatformOverride;
    debugDefaultTargetPlatformOverride = TargetPlatform.windows;
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
    const channel = MethodChannel('test/normal-pane-host');
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    messenger.setMockMethodCallHandler(channel, (call) async {
      calls.add(call);
      return switch (call.method) {
        'attachRuntime' => {
          'version': 1,
          'moduleAttached': true,
          'inputSourceAvailable': true,
          'productionEnabled': true,
        },
        'attachPane' => {
          'version': 1,
          'state': 'attached',
          'paneToken': 'pane-${++panes}',
        },
        'requestFocus' => {
          'version': 1,
          'state': 'active',
          'attempt': '$attempt',
          'focusEpoch': '${++epoch}',
        },
        'retirePane' => {
          'version': 1,
          'state': (call.arguments as Map)['phase'] == 'quiet'
              ? 'nativequiet'
              : 'released',
        },
        'invalidateGeometry' => {'version': 1, 'state': 'invalidated'},
        'presentedFrame' => {
          'version': 1,
          'state': (call.arguments as Map)['focusEpoch'] == '$epoch'
              ? 'presented'
              : 'rejected',
        },
        'claimAction' => {
          'version': 1,
          'state': 'claimed',
          'claimToken': 'copy-claim',
          'action': claimAction,
          'payload': '完整🙂\n复制内容',
        },
        'completeAction' => {
          'version': 1,
          'state': (call.arguments as Map)['kind'] == 'abort'
              ? 'aborted'
              : 'completed',
        },
        _ => throw MissingPluginException(call.method),
      };
    });
    messenger.setMockMethodCallHandler(SystemChannels.platform, (call) async {
      if (call.method == 'Clipboard.setData') {
        clipboardWrites.add((call.arguments as Map)['text'] as String);
        return clipboardWrite?.future;
      }
      return null;
    });
    addTearDown(() {
      messenger.setMockMethodCallHandler(channel, null);
      messenger.setMockMethodCallHandler(SystemChannels.platform, null);
    });
    gateway.onCommand = (command) {
      gateway.commandBus.add(
        RustSignalPack(
          DesktopNativeCommandResult(
            version: 1,
            source: command.source,
            identity: command.identity,
            attempt: command.attempt,
            focusEpoch: command.focusEpoch,
            admission: NativeCommandAdmission.accepted,
            consumedLatches: 0,
            requestId: command.requestId,
            effect: command.requestId,
            failure: NativeCommandFailure.none,
          ),
          Uint8List(0),
        ),
      );
      if (command.command is NativeCommandEvent &&
          (command.command as NativeCommandEvent).value
              is DesktopEventDisconnect) {
        lifecycle(NativeLifecycleState.ended);
      }
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
            execute: (action, {required origin}) async => execute == null
                ? DesktopActionOutcome.unavailable
                : await execute!(action),
            ownsTerminalInput: (_, enabled) => ownsInput = enabled,
            isCurrentOrigin: (origin) =>
                identical(origin, controller) && allowed && origin.nativeActive,
            canRequestFocus: (origin) =>
                identical(origin, controller) &&
                allowed &&
                (origin.focusNode.context == null ||
                    (ModalRoute.of(origin.focusNode.context!)?.isCurrent ??
                        true)),
            child: ValueListenableBuilder<bool>(
              valueListenable: paneVisible,
              child: TerminalPane(
                controller: controller,
                desktopGateway: gateway,
                desktopHost: DesktopNativeHost(channel: channel),
                onActivate: () {},
                onClose: () {},
              ),
              builder: (_, visible, child) =>
                  visible ? child! : const SizedBox.shrink(),
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
    lifecycle(NativeLifecycleState.authenticated);
    gateway.statusBus.add(
      RustSignalPack(
        DesktopSessionStatus(
          source: source,
          identity: 'frozen',
          status: SessionStatus(
            sessionId: source.sessionId,
            state: SessionState.connected,
            failure: FailureKind.none,
            detail: '',
            localNetworkSettingsUrl: '',
            hint: ConnectHint.none,
          ),
        ),
        Uint8List(0),
      ),
    );
    await tester.pumpAndSettle();
    expect(controller.nativeActive, isTrue);
    expect(ownsInput, isTrue);
  }

  void frame({Uint8List? binary, bool selection = true}) {
    final geometry = gateway.starts.single.geometry;
    gateway.projectionBus.add(
      RustSignalPack(
        DesktopNativeProjection(
          source: source,
          identity: 'frozen',
          attempt: u64(attempt),
          focusEpoch: u64(epoch),
          frameRevision: 1,
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
            seq: 1,
            topStableRow: 0,
            firstStableRow: 0,
            screenTopStableRow: 0,
            cursorCol: selection ? 0 : -1,
            cursorRow: selection ? 0 : -1,
            title: '终端',
            mouseReporting: false,
            alternateScreen: false,
          ),
        ),
        binary ??
            Uint8List.fromList([
              1,
              0,
              1,
              0,
              0,
              0,
              0,
              0,
              0,
              0,
              0,
              0,
              0,
              0,
              0,
              2,
              0,
              0,
              0,
              0,
              2,
              0,
              0,
              0,
              104,
              105,
            ]),
      ),
    );
  }

  void menu() => gateway.menuBus.add(
    RustSignalPack(
      DesktopNativeContextMenuReady(
        version: 1,
        source: source,
        identity: 'frozen',
        attempt: u64(12),
        focusEpoch: u64(epoch),
        effect: u64(31),
        stableRow: 0,
        column: 1,
      ),
      Uint8List(0),
    ),
  );

  Future<void> close(WidgetTester tester) async {
    await tester.pumpWidget(const SizedBox.shrink());
    await tester.pump();
    expect(ownsInput, isFalse);
    controller.dispose();
    await gateway.dispose();
    paneVisible.dispose();
    debugDefaultTargetPlatformOverride = _oldPlatform;
  }
}

void main() {
  testWidgets('静态帧重新激活升级 epoch 后实际重绘呈现，不等待远端输出', (tester) async {
    final h = _PaneHarness();
    await h.mount(tester);
    h.frame();
    await tester.pumpAndSettle();
    final original = h.controller.normalSession!.frame;
    final before = h.calls
        .where((call) => call.method == 'presentedFrame')
        .length;
    final epoch = h.epoch;
    expect(await h.controller.requestTerminalFocus(), isTrue);
    h.lifecycle(NativeLifecycleState.authenticated);
    await tester.pumpAndSettle();
    final calls = h.calls
        .where((call) => call.method == 'presentedFrame')
        .toList();
    expect(h.epoch, greaterThan(epoch));
    expect(calls.length, greaterThan(before));
    expect((calls.last.arguments as Map)['focusEpoch'], '${h.epoch}');
    expect((calls.last.arguments as Map)['frameRevision'], 1);
    expect(h.controller.normalSession!.frame, same(original));
    expect(h.controller.lastFrameSeq, 1);
    expect(h.controller.normalSession!.error, isEmpty);
    expect(h.controller.nativeActive, isTrue);
    await h.close(tester);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    '静态配色 Prepared 实际进入 normal renderer，Default/Ansi/RGB 与重连清缓存保持原 profile',
    (tester) async {
      final h = _PaneHarness();
      await h.mount(tester, scheme: 'one_dark');
      h.frame(binary: _colorBinary(), selection: false);
      await tester.pumpAndSettle();
      final view = tester.widget<TerminalView>(find.byType(TerminalView));
      final key = view.key! as GlobalKey<TerminalViewState>;
      expect(view.theme, same(themeForProfile('one_dark')));
      final expected = [
        themeForProfile('one_dark').background,
        themeForProfile('one_dark').red,
        const Color.fromARGB(255, 19, 87, 143),
      ];
      expect(await _cellPixels(tester, h.paintKey, key), expected);
      final old = h.controller.terminal.lineAt(0);
      final late = prepared(h.gateway.prepares.single.requestId);
      h.gateway.emit(
        late.copyWith(
          source: h.source.copyWith(generation: u64(77)),
          snapshot: () => late.snapshot!.copyWith(
            profile: late.snapshot!.profile.copyWith(
              colorScheme: 'solarized_light',
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(
        tester.widget<TerminalView>(find.byType(TerminalView)).theme,
        same(themeForProfile('one_dark')),
      );
      h.attempt = 13;
      h.lifecycle(NativeLifecycleState.connecting);
      await tester.pumpAndSettle();
      expect(h.controller.lastFrameSeq, 0);
      expect(h.controller.canCopy, isFalse);
      expect(h.controller.terminal.windowCovers(0, 1), isFalse);
      expect(h.controller.terminal.lineAt(0), isNot(same(old)));
      h.lifecycle(NativeLifecycleState.authenticated);
      await tester.pumpAndSettle();
      h.frame(binary: _colorBinary(), selection: false);
      await tester.pumpAndSettle();
      expect(h.controller.terminal.lineAt(0), isNot(same(old)));
      expect(await _cellPixels(tester, h.paintKey, key), expected);
      await h.close(tester);
      expect(tester.takeException(), isNull);
    },
  );

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
      expect(await _cellPixels(tester, boundary, keys[0]), [
        themeForProfile('one_dark').background,
        themeForProfile('one_dark').red,
        const Color.fromARGB(255, 19, 87, 143),
      ]);
      expect(await _cellPixels(tester, boundary, keys[1]), [
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
      expect(await _cellPixels(tester, boundary, keys[0]), [
        themeForProfile('one_dark').background,
        themeForProfile('one_dark').red,
        const Color.fromARGB(255, 19, 87, 143),
      ]);
      expect(await _cellPixels(tester, boundary, keys[1]), [
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

  testWidgets('normal ClosePane 实际移除 widget 后原 claim ACK/release 独立结算，不自等', (
    tester,
  ) async {
    final h = _PaneHarness();
    await h.mount(tester);
    h.claimAction = 'closePane';
    h.execute = (action) async {
      expect(action, DesktopAction.closePane);
      h.paneVisible.value = false;
      await WidgetsBinding.instance.endOfFrame;
      expect(find.byType(TerminalPane), findsNothing);
      return DesktopActionOutcome.executed;
    };
    h.gateway.actionBus.add(
      RustSignalPack(
        DesktopNativeActionReady(
          version: 1,
          source: h.source,
          identity: 'frozen',
          attempt: u64(12),
          focusEpoch: u64(h.epoch),
          effect: u64(31),
        ),
        Uint8List(0),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.byType(TerminalPane), findsNothing);
    final completed = h.calls.indexWhere(
      (call) => call.method == 'completeAction',
    );
    final released = h.calls.indexWhere(
      (call) =>
          call.method == 'retirePane' &&
          (call.arguments as Map)['phase'] == 'release',
    );
    expect(completed, greaterThan(0));
    expect(released, greaterThan(completed));
    expect((h.calls[completed].arguments as Map)['kind'], 'completed');
    expect((h.calls[completed].arguments as Map)['paneToken'], 'pane-1');
    await h.close(tester);
    expect(tester.takeException(), isNull);
  });

  for (final width in [375.0, 768.0, 1280.0]) {
    testWidgets(
      '$width normal 原帧实际 paint 后 ACK，右键 quiet 后显示真实 Fluent 菜单并完成完整 Copy',
      (tester) async {
        final h = _PaneHarness();
        await h.mount(tester, width: width);
        h.frame();
        expect(
          h.gateway.commands.where(
            (request) =>
                request.command is NativeCommandEvent &&
                (request.command as NativeCommandEvent).value
                    is DesktopEventFrameAck,
          ),
          isEmpty,
        );
        await tester.pumpAndSettle();
        expect(h.controller.canCopy, isTrue);
        expect(
          h.calls.where((call) => call.method == 'presentedFrame'),
          isNotEmpty,
        );
        final ack = h.gateway.commands.singleWhere(
          (request) =>
              request.command is NativeCommandEvent &&
              (request.command as NativeCommandEvent).value
                  is DesktopEventFrameAck,
        );
        expect(ack.attempt, u64(12));
        h.menu();
        await tester.pumpAndSettle();
        expect(find.byType(f.MenuFlyout), findsOneWidget);
        expect(find.text('复制'), findsOneWidget);
        expect(h.controller.nativeEditableSafe, isTrue);
        h.lifecycle(NativeLifecycleState.inputRevoked);
        await tester.pumpAndSettle();
        expect(find.byType(f.MenuFlyout), findsOneWidget);
        h.clipboardWrite = Completer<Object?>();
        await tester.tap(find.text('复制'));
        await tester.pumpAndSettle();
        final action = h.gateway.commands.last;
        expect(
          (action.command as NativeCommandAction).value,
          DesktopAction.copy,
        );
        h.gateway.actionBus.add(
          RustSignalPack(
            DesktopNativeActionReady(
              version: 1,
              source: h.source,
              identity: 'frozen',
              attempt: u64(12),
              focusEpoch: u64(h.epoch),
              effect: action.requestId,
            ),
            Uint8List(0),
          ),
        );
        await tester.pump();
        expect(h.clipboardWrites, ['完整🙂\n复制内容']);
        expect(
          h.calls.where((call) => call.method == 'completeAction'),
          isEmpty,
        );
        h.clipboardWrite!.complete(null);
        await tester.pumpAndSettle();
        expect(
          (h.calls
                  .singleWhere((call) => call.method == 'completeAction')
                  .arguments
              as Map)['kind'],
          'completed',
        );
        expect(
          h.gateway.commands.any(
            (request) =>
                request.command is NativeCommandEvent &&
                ((request.command as NativeCommandEvent).value
                        is DesktopEventMouse ||
                    (request.command as NativeCommandEvent).value
                        is DesktopEventKey ||
                    (request.command as NativeCommandEvent).value
                        is DesktopEventText),
          ),
          isFalse,
        );
        await h.close(tester);
        expect(tester.takeException(), isNull);
      },
    );
  }
  testWidgets('normal 菜单打开后原 pane 意图失效，迟到选项不请求新焦点或发 Paste', (tester) async {
    final h = _PaneHarness();
    await h.mount(tester);
    h.frame();
    await tester.pumpAndSettle();
    h.menu();
    await tester.pumpAndSettle();
    expect(find.byType(f.MenuFlyout), findsOneWidget);
    final focused = h.calls
        .where((call) => call.method == 'requestFocus')
        .length;
    h.allowed = false;
    await tester.tap(find.text('粘贴'));
    await tester.pumpAndSettle();
    expect(
      h.calls.where((call) => call.method == 'requestFocus').length,
      focused,
    );
    expect(
      h.gateway.commands.where(
        (request) => request.command is NativeCommandAction,
      ),
      isEmpty,
    );
    await h.close(tester);
    expect(tester.takeException(), isNull);
  });
}
