import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/terminal/desktop_font.dart';
import 'package:guosh_shell/src/terminal/desktop_ordered_session.dart';
import 'package:guosh_shell/src/terminal/session_target.dart';
import 'package:rinf/rinf.dart';

import 'desktop_session_controller_test.dart'
    show DesktopSessionTestGateway, prepared, source;

const _geometry = DesktopGeometry(
  cols: 80,
  rows: 24,
  pixelWidth: 640,
  pixelHeight: 480,
  dpi: 96,
);

class _Fixture {
  _Fixture() {
    session
      ..performAction = ((action) async {
        actions.add(action);
      })
      ..onClipboard = clipboard.add
      ..begin();
  }

  final gateway = DesktopSessionTestGateway();
  late final session = DesktopOrderedSession(
    source: source,
    target: const SessionTarget.quick(
      host: 'example.test',
      port: 22,
      username: 'test',
      profileId: 'selected',
    ),
    gateway: gateway,
    font: DesktopFontController(
      cache: DesktopFontCache(register: (_, _) async {}),
    ),
  );
  final actions = <DesktopAction>[];
  final clipboard = <String>[];
  final held = <DesktopOrdered>[];

  /// 暂不回执，由用例逐个放行。
  void hold() => gateway.onOrdered = (request) {
    held.add(request);
    return null;
  };

  void reply(
    DesktopOrdered request, {
    DesktopFailure failure = DesktopFailure.none,
    DesktopAction? action,
  }) => gateway.eventResultBus.add(
    RustSignalPack(
      DesktopSessionTestGateway.accept(
        request,
        failure: failure,
        action: action,
      ),
      Uint8List(0),
    ),
  );

  Future<void> start({
    bool accept = true,
    Future<void> Function()? tick,
  }) async {
    final prepare = gateway.prepares.single;
    gateway.emit(prepared(prepare.requestId));
    await (tick ?? () => Future<void>.delayed(Duration.zero))();
    session.measure(_geometry);
    expect(gateway.starts, hasLength(1));
    if (accept) {
      gateway.emit(
        prepared(prepare.requestId).copyWith(
          requestId: gateway.starts.single.requestId,
          state: DesktopPreparationState.accepted,
          snapshot: () => null,
        ),
      );
    }
  }

  List<int> get seqs => [
    for (final request in gateway.orderedEvents) request.seq.toBigInt().toInt(),
  ];
  List<DesktopEvent> get events => [
    for (final request in gateway.orderedEvents) request.event,
  ];

  Future<void> dispose() async {
    session.dispose();
    await gateway.dispose();
  }
}

DesktopEventKey _key(String key) => desktopKeyEvent(
  key: key,
  phase: DesktopKeyPhase.down,
  source: null,
  shift: false,
  control: false,
  alt: false,
  leftAlt: false,
  rightAlt: false,
  superKey: false,
);

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  test('Accepted 前本地排队；之后单个在途，seq 从 1 严格递增', () async {
    final f = _Fixture();
    f.hold();
    f.session.send(const DesktopEventText(text: 'a'));
    await f.start(accept: false);
    f.session.send(const DesktopEventText(text: 'b'));
    expect(f.gateway.orderedEvents, isEmpty);
    f.gateway.emit(
      prepared(f.gateway.prepares.single.requestId).copyWith(
        requestId: f.gateway.starts.single.requestId,
        state: DesktopPreparationState.accepted,
        snapshot: () => null,
      ),
    );
    expect(f.seqs, [1]);
    expect(f.events.single, const DesktopEventText(text: 'a'));
    f.reply(f.held.removeAt(0));
    expect(f.seqs, [1, 2]);
    expect(f.events.last, const DesktopEventText(text: 'b'));
    f.reply(f.held.removeAt(0));
    f.session.send(_key('enter'));
    expect(f.seqs, [1, 2, 3]);
    await f.dispose();
  });

  testWidgets('Busy 不推进 seq，同 seq 重试，键盘/文本/粘贴不丢不乱序', (tester) async {
    final f = _Fixture();
    await f.start(tick: tester.pump);
    var busy = 2;
    f.gateway.onOrdered = (request) => DesktopSessionTestGateway.accept(
      request,
      failure: busy-- > 0 ? DesktopFailure.busy : DesktopFailure.none,
    );
    f.session
      ..send(_key('arrow_up'))
      ..send(const DesktopEventText(text: 'x'))
      ..send(const DesktopEventPaste(text: 'y'));
    expect(f.seqs, [1]);
    await tester.pump(const Duration(milliseconds: 25));
    expect(f.seqs, [1, 1]);
    await tester.pump(const Duration(milliseconds: 25));
    await tester.pump();
    expect(f.seqs, [1, 1, 1, 2, 3]);
    expect(f.events.skip(2), [
      _key('arrow_up'),
      const DesktopEventText(text: 'x'),
      const DesktopEventPaste(text: 'y'),
    ]);
    await f.dispose();
  });

  test('InvalidEvent/PolicyUnavailable 已消费 seq，继续下一个', () async {
    final f = _Fixture();
    await f.start();
    f.hold();
    f.session
      ..send(const DesktopEventText(text: '1'))
      ..send(const DesktopEventText(text: '2'))
      ..send(const DesktopEventText(text: '3'));
    f.reply(f.held.removeAt(0), failure: DesktopFailure.invalidEvent);
    f.reply(f.held.removeAt(0), failure: DesktopFailure.policyUnavailable);
    expect(f.seqs, [1, 2, 3]);
    expect(f.session.error, isEmpty);
    await f.dispose();
  });

  test('InputTooLarge 把粘贴对半拆分，用同一 seq 重发，内容完整', () async {
    final f = _Fixture();
    await f.start();
    f.hold();
    f.session.send(const DesktopEventPaste(text: '粘贴🙂内容'));
    f.reply(f.held.removeAt(0), failure: DesktopFailure.inputTooLarge);
    f.reply(f.held.removeAt(0));
    f.reply(f.held.removeAt(0));
    expect(f.seqs, [1, 1, 2]);
    final parts = [
      for (final event in f.events.skip(1)) (event as DesktopEventPaste).text,
    ];
    expect(parts.join(), '粘贴🙂内容');
    expect(parts, hasLength(2));
    await f.dispose();
  });

  test('鼠标移动与视口只保留队尾最新；Busy 时鼠标移动直接丢弃', () async {
    final f = _Fixture();
    await f.start();
    f.hold();
    DesktopEventMouse move(int col) => DesktopEventMouse(
      kind: 'move',
      button: '',
      col: col,
      row: 0,
      shift: false,
      control: false,
      alt: false,
    );
    f.session
      ..send(move(1))
      ..send(move(2))
      ..send(move(3))
      ..send(_key('tab'))
      ..send(
        const DesktopEventViewport(
          followBottom: false,
          topStableRow: 1,
          rows: 9,
        ),
      )
      ..send(
        const DesktopEventViewport(
          followBottom: true,
          topStableRow: 0,
          rows: 0,
        ),
      );
    f.reply(f.held.removeAt(0), failure: DesktopFailure.busy);
    f.reply(f.held.removeAt(0));
    f.reply(f.held.removeAt(0));
    f.reply(f.held.removeAt(0));
    expect(f.seqs, [1, 1, 2, 3]);
    expect(f.events, [
      move(1),
      move(3),
      _key('tab'),
      const DesktopEventViewport(followBottom: true, topStableRow: 0, rows: 0),
    ]);
    await f.dispose();
  });

  test('按键回报的壳动作交窗格；Copy 结果回调剪贴板', () async {
    final f = _Fixture();
    await f.start();
    f.gateway.onOrdered = (request) => DesktopSessionTestGateway.accept(
      request,
      action: request.event is DesktopEventKey ? DesktopAction.newTab : null,
    );
    f.session.send(_key('character:t'));
    await Future<void>.delayed(Duration.zero);
    expect(f.actions, [DesktopAction.newTab]);
    f.session.send(const DesktopEventCopy());
    f.gateway.clipboardBus.add(
      RustSignalPack(
        DesktopClipboard(source: source, identity: 'frozen', text: '选区'),
        Uint8List(0),
      ),
    );
    expect(f.clipboard, ['选区']);
    await f.dispose();
  });

  test('Start 前关闭只发 Cancel，取消确认后结束', () async {
    final f = _Fixture();
    final closed = f.session.close();
    expect(f.gateway.cancels, hasLength(1));
    f.gateway.emit(
      prepared(f.gateway.prepares.single.requestId).copyWith(
        requestId: f.gateway.cancels.single.requestId,
        state: DesktopPreparationState.cancelled,
        snapshot: () => null,
      ),
    );
    expect(await closed, isTrue);
    expect(f.gateway.orderedEvents, isEmpty);
    await f.dispose();
  });

  test('Start 后关闭丢弃未发输入，发有序 Disconnect，等 lifecycle ended', () async {
    final f = _Fixture();
    await f.start();
    f.gateway.lifecycle(NativeLifecycleState.authenticated);
    f.hold();
    f.session
      ..send(const DesktopEventText(text: '在途'))
      ..send(const DesktopEventText(text: '未发'));
    var done = false;
    final closed = f.session.close().then((value) => done = value);
    f.reply(f.held.removeAt(0));
    expect(f.events.last, const DesktopEventDisconnect());
    expect(f.events.whereType<DesktopEventText>().map((event) => event.text), [
      '在途',
    ]);
    f.reply(f.held.removeAt(0));
    await Future<void>.delayed(Duration.zero);
    expect(done, isFalse);
    f.gateway.lifecycle(NativeLifecycleState.ended, revision: 2);
    await closed;
    expect(done, isTrue);
    await f.dispose();
  });

  test('远端已 closed 的会话关闭时仍发 Disconnect，等 lifecycle ended', () async {
    final f = _Fixture();
    await f.start();
    f.gateway.lifecycle(NativeLifecycleState.authenticated);
    f.gateway.statusBus.add(
      RustSignalPack(
        DesktopSessionStatus(
          source: source,
          identity: 'frozen',
          status: const SessionStatus(
            sessionId: 61,
            state: SessionState.closed,
            failure: FailureKind.none,
            detail: '',
            localNetworkSettingsUrl: '',
            hint: ConnectHint.none,
          ),
        ),
        Uint8List(0),
      ),
    );
    var done = false;
    final closed = f.session.close().then((value) => done = value);
    await Future<void>.delayed(Duration.zero);
    expect(f.events.last, const DesktopEventDisconnect());
    expect(done, isFalse);
    f.gateway.lifecycle(NativeLifecycleState.ended, revision: 2);
    await closed;
    expect(done, isTrue);
    await f.dispose();
  });

  test('Key 字段：左右 Alt 来源、物理键、Down 才带默认动作', () {
    const right = KeyDownEvent(
      physicalKey: PhysicalKeyboardKey.arrowLeft,
      logicalKey: LogicalKeyboardKey.arrowLeft,
      timeStamp: Duration.zero,
    );
    final rightAlt = desktopKeyEvent(
      key: 'arrow_left',
      phase: DesktopKeyPhase.down,
      source: right,
      shift: false,
      control: false,
      alt: true,
      leftAlt: false,
      rightAlt: true,
      superKey: false,
    );
    expect(rightAlt.altSource, DesktopAltSource.known);
    expect([rightAlt.leftAlt, rightAlt.rightAlt], [false, true]);
    expect(
      rightAlt.physical.toBigInt(),
      BigInt.from(PhysicalKeyboardKey.arrowLeft.usbHidUsage),
    );
    final leftAlt = desktopKeyEvent(
      key: 'arrow_left',
      phase: DesktopKeyPhase.down,
      source: right,
      shift: false,
      control: false,
      alt: true,
      leftAlt: true,
      rightAlt: false,
      superKey: false,
    );
    expect(leftAlt.altSource, DesktopAltSource.known);
    expect([leftAlt.leftAlt, leftAlt.rightAlt], [true, false]);
    // 键位条挂住的 Alt 没有物理来源：不得声称 Known。
    final latched = desktopKeyEvent(
      key: 'arrow_left',
      phase: DesktopKeyPhase.down,
      source: right,
      shift: false,
      control: false,
      alt: true,
      leftAlt: false,
      rightAlt: false,
      superKey: false,
    );
    expect(latched.altSource, DesktopAltSource.unknown);
    final synthetic = _key('enter');
    expect(synthetic.altSource, DesktopAltSource.unknown);
    expect(synthetic.physical.toBigInt(), BigInt.zero);

    DesktopEventKey chord(
      LogicalKeyboardKey logical, {
      DesktopKeyPhase phase = DesktopKeyPhase.down,
      bool shift = false,
    }) => desktopKeyEvent(
      key: 'character:x',
      phase: phase,
      source: KeyDownEvent(
        physicalKey: PhysicalKeyboardKey.keyT,
        logicalKey: logical,
        timeStamp: Duration.zero,
      ),
      shift: shift,
      control: true,
      alt: false,
      leftAlt: false,
      rightAlt: false,
      superKey: false,
    );
    expect(
      chord(LogicalKeyboardKey.keyT, shift: true).defaultAction,
      DesktopAction.newTab,
    );
    expect(
      chord(LogicalKeyboardKey.keyD, shift: true).defaultAction,
      DesktopAction.splitRight,
    );
    expect(
      chord(LogicalKeyboardKey.keyC, shift: true).defaultAction,
      DesktopAction.copy,
    );
    expect(chord(LogicalKeyboardKey.keyV).defaultAction, DesktopAction.paste);
    expect(chord(LogicalKeyboardKey.equal).defaultAction, DesktopAction.zoomIn);
    expect(chord(LogicalKeyboardKey.keyC).defaultAction, isNull);
    expect(
      chord(
        LogicalKeyboardKey.keyT,
        shift: true,
        phase: DesktopKeyPhase.repeat,
      ).defaultAction,
      isNull,
    );
  });
}
