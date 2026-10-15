import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/terminal/desktop_font.dart';
import 'package:guosh_shell/src/terminal/desktop_ordered_session.dart';
import 'package:guosh_shell/src/terminal/frame_terminal.dart';
import 'package:guosh_shell/src/terminal/session_target.dart';
import 'package:guosh_shell/src/terminal/terminal_key_bar.dart';
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

  test('InputTooLarge 拆分起始代理对时保持完整字符', () async {
    final f = _Fixture();
    await f.start();
    f.hold();
    f.session.send(const DesktopEventPaste(text: '🙂a'));
    f.reply(f.held.removeAt(0), failure: DesktopFailure.inputTooLarge);
    expect(f.events.last, const DesktopEventPaste(text: '🙂'));
    f.reply(f.held.removeAt(0));
    expect(f.events.last, const DesktopEventPaste(text: 'a'));
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

  test('粘贴读取完成前 Enter 不得越过，迟到剪贴板不能跨 attempt', () async {
    final f = _Fixture();
    await f.start();
    f.gateway.lifecycle(NativeLifecycleState.authenticated);
    final clipboard = Completer<String?>();
    f.session.readClipboard = () => clipboard.future;
    f.hold();
    f.session.send(_key('character:v'));
    f.reply(f.held.removeAt(0), action: DesktopAction.paste);
    f.session.send(_key('enter'));
    expect(f.events, hasLength(1));
    clipboard.complete('command');
    await Future<void>.delayed(Duration.zero);
    expect(f.events.last, const DesktopEventPaste(text: 'command'));
    f.reply(f.held.removeAt(0));
    expect(f.events.last, _key('enter'));
    f.reply(f.held.removeAt(0));

    final stale = Completer<String?>();
    f.session.readClipboard = () => stale.future;
    final paste = f.session.pasteClipboard();
    f.gateway.lifecycle(
      NativeLifecycleState.authenticated,
      revision: 2,
      attempt: 2,
    );
    stale.complete('old secret');
    await paste;
    expect(f.events.whereType<DesktopEventPaste>(), hasLength(1));
    await f.dispose();
  });

  test('认证前排队有总量上限，超限不发送残余 Enter', () async {
    final f = _Fixture();
    f.session.send(
      DesktopEventPaste(
        text: 'x' * (DesktopOrderedSession.maxQueuedTextUnits + 1),
      ),
    );
    f.session.send(_key('enter'));
    expect(f.session.error, isNotEmpty);
    await f.start();
    expect(f.events, isEmpty);
    await f.dispose();
  });

  test('受理后超大粘贴按分片全部发出，不阻断后续输入', () async {
    final f = _Fixture();
    await f.start();
    final notices = <String>[];
    f.session.onNotice = notices.add;
    // 与 Rust 准入一致：UTF-8 字节 ×3 + 512 超过 1 MiB 即 InputTooLarge。
    f.gateway.onOrdered = (request) => DesktopSessionTestGateway.accept(
      request,
      failure: switch (request.event) {
        DesktopEventPaste(:final text)
            when utf8.encode(text).length * 3 + 512 > 1024 * 1024 =>
          DesktopFailure.inputTooLarge,
        _ => DesktopFailure.none,
      },
    );
    final text = 'x${'中🙂' * (200 * 1024)}';
    expect(text.length, greaterThan(600 * 1024));
    f.session
      ..send(DesktopEventPaste(text: text))
      ..send(_key('enter'));
    for (var i = 0; i < 64; i++) {
      await Future<void>.delayed(Duration.zero);
    }
    final parts = f.events.whereType<DesktopEventPaste>().toList();
    expect(parts.length, greaterThan(1));
    for (final part in parts) {
      expect(
        part.text.length,
        lessThanOrEqualTo(DesktopOrderedSession.pasteChunkUnits),
      );
      final first = part.text.codeUnitAt(0);
      expect(first >= 0xdc00 && first <= 0xdfff, isFalse);
    }
    expect(parts.map((part) => part.text).join(), text);
    expect(f.events.last, _key('enter'));
    // 每个分片一次即受理，seq 严格递增。
    expect(f.seqs, [for (var i = 1; i <= parts.length + 1; i++) i]);
    expect(f.session.error, isEmpty);
    expect(notices, isEmpty);
    f.session.send(const DesktopEventText(text: '继续'));
    expect(f.events.last, const DesktopEventText(text: '继续'));
    await f.dispose();
  });

  test('受理后待发条数超限只丢弃最新一条，不阻断，帧确认仍入队', () async {
    final f = _Fixture();
    await f.start();
    final notices = <String>[];
    f.session.onNotice = notices.add;
    f.hold();
    // 第一条在途，其余排队直到上限。
    for (var i = 0; i <= DesktopOrderedSession.maxQueuedEvents; i++) {
      f.session.send(_key('enter'));
    }
    f.session
      ..send(const DesktopEventText(text: '丢弃'))
      ..send(const DesktopEventFrameAck(frameSeq: 1));
    expect(notices, hasLength(1));
    expect(f.session.error, isEmpty);
    while (f.held.isNotEmpty) {
      f.reply(f.held.removeAt(0));
    }
    expect(f.events, hasLength(DesktopOrderedSession.maxQueuedEvents + 2));
    expect(f.events.last, const DesktopEventFrameAck(frameSeq: 1));
    expect(f.events.whereType<DesktopEventText>(), isEmpty);
    await f.dispose();
  });

  test('受理前超限阻断期间，帧确认、尺寸与视口仍发送', () async {
    final f = _Fixture();
    f.session.send(
      DesktopEventPaste(
        text: 'x' * (DesktopOrderedSession.maxQueuedTextUnits + 1),
      ),
    );
    expect(f.session.error, isNotEmpty);
    await f.start();
    f.hold();
    f.session
      ..send(const DesktopEventText(text: '丢弃'))
      ..send(const DesktopEventFrameAck(frameSeq: 1))
      ..send(const DesktopEventResize(geometry: _geometry))
      ..send(
        const DesktopEventViewport(
          followBottom: true,
          topStableRow: 0,
          rows: 0,
        ),
      );
    while (f.held.isNotEmpty) {
      f.reply(f.held.removeAt(0));
    }
    expect(f.events, const [
      DesktopEventFrameAck(frameSeq: 1),
      DesktopEventResize(geometry: _geometry),
      DesktopEventViewport(followBottom: true, topStableRow: 0, rows: 0),
    ]);
    await f.dispose();
  });

  test('剪贴板读取失败只丢弃本次粘贴，排队输入照常发送', () async {
    final f = _Fixture();
    await f.start();
    final notices = <String>[];
    f.session.onNotice = notices.add;
    f.hold();
    f.session.send(const DesktopEventText(text: 'one'));
    f.session.readClipboard = () async =>
        throw PlatformException(code: 'unavailable');
    final paste = f.session.pasteClipboard();
    f.session.send(_key('enter'));
    await paste;
    expect(notices, hasLength(1));
    f.reply(f.held.removeAt(0));
    expect(f.events.last, _key('enter'));
    f.reply(f.held.removeAt(0));
    expect(f.events.whereType<DesktopEventPaste>(), isEmpty);
    expect(f.session.error, isEmpty);
    f.session.send(const DesktopEventText(text: 'two'));
    expect(f.events.last, const DesktopEventText(text: 'two'));
    await f.dispose();
  });

  testWidgets('剪贴板读取超时按失败处理；读取期间再次粘贴给出提示', (tester) async {
    final f = _Fixture();
    await f.start(tick: tester.pump);
    final notices = <String>[];
    f.session.onNotice = notices.add;
    f.session.readClipboard = () => Completer<String?>().future;
    final paste = f.session.pasteClipboard();
    f.session.send(_key('enter'));
    await f.session.pasteClipboard();
    expect(notices, hasLength(1));
    expect(f.events, isEmpty);
    await tester.pump(const Duration(seconds: 2));
    await paste;
    await tester.pump();
    expect(notices, hasLength(2));
    expect(f.events, [_key('enter')]);
    expect(f.session.error, isEmpty);
    await f.dispose();
  });

  testWidgets('关闭未确认后再次 close 重新发起 Disconnect', (tester) async {
    final f = _Fixture();
    await f.start(tick: tester.pump);
    f.gateway.lifecycle(NativeLifecycleState.authenticated);
    f.hold();
    final first = f.session.close();
    expect(f.events.last, const DesktopEventDisconnect());
    await tester.pump(const Duration(seconds: 10));
    expect(await first, isFalse);
    final count = f.events.length;
    final second = f.session.close();
    expect(f.events, hasLength(count + 1));
    expect(f.events.last, const DesktopEventDisconnect());
    f.gateway.lifecycle(NativeLifecycleState.ended, revision: 2);
    expect(await second, isTrue);
    expect(identical(f.session.close(), second), isTrue);
    await f.dispose();
  });

  test('菜单粘贴保持之前已排队的文本顺序', () async {
    final f = _Fixture();
    await f.start();
    f.hold();
    f.session
      ..send(const DesktopEventText(text: 'one'))
      ..send(const DesktopEventText(text: 'two'));
    final clipboard = Completer<String?>();
    f.session.readClipboard = () => clipboard.future;
    final paste = f.session.pasteClipboard();
    f.session.send(_key('enter'));
    f.reply(f.held.removeAt(0));
    expect(f.events.last, const DesktopEventText(text: 'two'));
    f.reply(f.held.removeAt(0));
    expect(f.events, hasLength(2));
    clipboard.complete('three');
    await paste;
    expect(f.events.last, const DesktopEventPaste(text: 'three'));
    f.reply(f.held.removeAt(0));
    expect(f.events.last, _key('enter'));
    await f.dispose();
  });

  test('认证前离散输入条目有上限', () async {
    final f = _Fixture();
    for (var i = 0; i <= DesktopOrderedSession.maxQueuedEvents; i++) {
      f.session.send(_key('enter'));
    }
    expect(f.session.error, isNotEmpty);
    await f.start();
    expect(f.events, isEmpty);
    await f.dispose();
  });

  testWidgets('Busy 输入不阻塞关闭，未消费的序号用于 Disconnect', (tester) async {
    final f = _Fixture();
    await f.start(tick: tester.pump);
    f.gateway.onOrdered = (request) => DesktopSessionTestGateway.accept(
      request,
      failure: request.event is DesktopEventDisconnect
          ? DesktopFailure.none
          : DesktopFailure.busy,
    );
    f.session.send(const DesktopEventText(text: 'pending'));
    final closed = f.session.close();
    await tester.pump(const Duration(milliseconds: 25));
    expect(f.events.last, const DesktopEventDisconnect());
    expect(f.seqs, [1, 1]);
    f.gateway.lifecycle(NativeLifecycleState.ended);
    expect(await closed, isTrue);
    await f.dispose();
  });

  testWidgets('关闭等待中的数据 Busy 回执会替换为 Disconnect，关闭超时后停止重试', (tester) async {
    final f = _Fixture();
    await f.start(tick: tester.pump);
    f.hold();
    f.session.send(const DesktopEventText(text: 'pending'));
    final closed = f.session.close();
    f.reply(f.held.removeAt(0), failure: DesktopFailure.busy);
    expect(f.events.last, const DesktopEventDisconnect());
    expect(f.seqs, [1, 1]);
    f.reply(f.held.removeAt(0), failure: DesktopFailure.busy);
    await tester.pump(const Duration(seconds: 10));
    expect(await closed, isFalse);
    final count = f.events.length;
    await tester.pump(const Duration(seconds: 1));
    expect(f.events, hasLength(count));
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
    // 没有硬件来源的按键不伪造物理键，改走 SoftKey，不带默认动作。
    expect(
      desktopOrderedKey(
        KeyInputEvent('character:t', shift: true, control: true),
        metaPressed: false,
      ),
      const DesktopEventSoftKey(
        key: 'character:t',
        shift: true,
        control: true,
        alt: false,
      ),
    );
    expect(
      desktopOrderedKey(
        KeyInputEvent('arrow_left', alt: true, source: right),
        metaPressed: false,
      ),
      isA<DesktopEventKey>()
          .having(
            (key) => key.physical.toBigInt(),
            'physical',
            isNot(BigInt.zero),
          )
          .having(
            (key) => key.altSource,
            'altSource',
            DesktopAltSource.unknown,
          ),
    );

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

  test('非法 Resize 由 Rust 按已消费回报，下一条有效事件用下一个 seq 继续', () async {
    final f = _Fixture();
    await f.start();
    f.hold();
    f.session
      ..send(
        const DesktopEventResize(
          geometry: DesktopGeometry(
            cols: 0,
            rows: 24,
            pixelWidth: 640,
            pixelHeight: 480,
            dpi: 96,
          ),
        ),
      )
      ..send(const DesktopEventText(text: 'ok'));
    f.reply(f.held.removeAt(0), failure: DesktopFailure.invalidEvent);
    f.reply(f.held.removeAt(0));
    expect(f.seqs, [1, 2]);
    expect(f.events.last, const DesktopEventText(text: 'ok'));
    expect(f.session.error, isEmpty);
    f.session.send(const DesktopEventText(text: '再来'));
    expect(f.seqs, [1, 2, 3]);
    await f.dispose();
  });

  testWidgets('键位条 Esc/Tab/方向/Ctrl+C 与挂住 Ctrl 的按钮经有序通道发 SoftKey', (
    tester,
  ) async {
    final f = _Fixture();
    await f.start(tick: tester.pump);
    // 与 V2 窗格 _sendOrdered 相同的接线：适配器事件 → desktopOrderedKey → 有序通道。
    final terminal = FrameTerminal(
      onInput: (event) {
        f.session.send(switch (event) {
          KeyInputEvent() => desktopOrderedKey(event, metaPressed: false),
          TextInputEvent(:final text) => DesktopEventText(text: text),
          _ => throw StateError('键位条不产生 $event'),
        });
        return true;
      },
    );
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: TerminalKeyBar(
            terminal: terminal,
            canCopy: () => false,
            onCopy: () {},
            onPaste: () {},
            onToggleKeyboard: () {},
            onDisconnect: () {},
            onEdit: () {},
            onZoomIn: () {},
            onZoomOut: () {},
            onZoomReset: () {},
            rows: const [
              ['escape', 'tab', 'up', 'left', 'ctrlC', 'ctrl', 'alt'],
            ],
          ),
        ),
      ),
    );
    for (final label in ['Esc', 'Tab', '↑', '←', 'Ctrl+C', 'Ctrl', 'Tab']) {
      await tester.tap(find.byTooltip(label));
      await tester.pump();
    }
    await tester.tap(find.byTooltip('Alt'));
    await tester.pump();
    await tester.tap(find.byTooltip('Esc'));
    await tester.pump();
    DesktopEventSoftKey soft(
      String key, {
      bool control = false,
      bool alt = false,
    }) =>
        DesktopEventSoftKey(key: key, shift: false, control: control, alt: alt);
    // 键名与 Rust ordered_soft_keys_* 单测编码的相同（Esc→ESC、Tab→HT、↑→CSI A、^C→ETX）。
    expect(f.events, [
      soft('escape'),
      soft('tab'),
      soft('arrow_up'),
      soft('arrow_left'),
      soft('character:c', control: true),
      soft('tab', control: true),
      soft('escape', alt: true),
    ]);
    expect(f.seqs, [1, 2, 3, 4, 5, 6, 7]);
    expect(f.session.error, isEmpty);
    await f.dispose();
  });

  group('macOS ⌘ 默认候选', () {
    tearDown(() => debugDefaultTargetPlatformOverride = null);

    DesktopAction? meta(LogicalKeyboardKey key, {bool shift = false}) =>
        desktopDefaultAction(
          key,
          control: false,
          shift: shift,
          alt: false,
          meta: true,
        );

    test('macOS：⌘⇧L 与 ⌘+/⌘−/⌘0 交壳', () {
      debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
      expect(
        meta(LogicalKeyboardKey.keyL, shift: true),
        DesktopAction.newLocalTerminal,
      );
      expect(meta(LogicalKeyboardKey.equal), DesktopAction.zoomIn);
      expect(meta(LogicalKeyboardKey.minus), DesktopAction.zoomOut);
      expect(meta(LogicalKeyboardKey.digit0), DesktopAction.zoomReset);
      final event = desktopKeyEvent(
        key: 'character:=',
        phase: DesktopKeyPhase.down,
        source: const KeyDownEvent(
          physicalKey: PhysicalKeyboardKey.equal,
          logicalKey: LogicalKeyboardKey.equal,
          timeStamp: Duration.zero,
        ),
        shift: false,
        control: false,
        alt: false,
        leftAlt: false,
        rightAlt: false,
        superKey: true,
      );
      expect(event.defaultAction, DesktopAction.zoomIn);
    });

    test('其他平台：Win/Super 不触发壳快捷键与缩放，Ctrl 行为不变', () {
      debugDefaultTargetPlatformOverride = TargetPlatform.windows;
      expect(meta(LogicalKeyboardKey.keyL, shift: true), isNull);
      expect(meta(LogicalKeyboardKey.equal), isNull);
      expect(meta(LogicalKeyboardKey.digit0), isNull);
      expect(
        desktopDefaultAction(
          LogicalKeyboardKey.equal,
          control: true,
          shift: false,
          alt: false,
          meta: false,
        ),
        DesktopAction.zoomIn,
      );
      expect(
        desktopDefaultAction(
          LogicalKeyboardKey.keyL,
          control: true,
          shift: true,
          alt: false,
          meta: false,
        ),
        DesktopAction.newLocalTerminal,
      );
    });
  });
}
