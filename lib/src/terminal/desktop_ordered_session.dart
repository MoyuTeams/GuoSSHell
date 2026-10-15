import 'dart:async';
import 'dart:collection';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart'
    show
        CopySelectionTextIntent,
        PasteTextIntent,
        SelectAllTextIntent,
        SingleActivator;
import 'package:terminal_view/terminal_view.dart'
    show TerminalTheme, defaultTerminalShortcuts;

import '../bindings/bindings.dart';
import '../desktop/desktop_shortcuts.dart';
import 'desktop_font.dart';
import 'desktop_session_controller.dart'
    show DesktopPanePhase, DesktopPresentedFrame;
import 'desktop_session_gateway.dart';
import 'desktop_terminal_theme.dart';
import 'frame_terminal.dart' show KeyInputEvent;
import 'session_target.dart';

/// 桌面 V2 的 Flutter 输入会话：Prepare → 字体 → 度量 → Start，之后键盘、文本、
/// 鼠标与控制全部经单一有序通道（DesktopOrdered）送 Rust。
///
/// 同一时刻只有一个事件在途，收到 DesktopEventResult 再发下一个：
/// - None / InvalidEvent / PolicyUnavailable：seq 已消费，继续下一个；
/// - Busy：seq 未推进，原事件用同一 seq 稍后重试（仅鼠标移动可丢弃）；
/// - InputTooLarge：文本/粘贴对半拆分后用同一 seq 重发。
/// Start 被接受前的输入在本地按序排队（type-ahead），最多 1024 条、512 Ki UTF-16 单元，
/// 超限丢弃整段并阻断输入直到重连。受理后超大粘贴按 [pasteChunkUnits] 分片全部入队；
/// 待发条数超过上限时只丢弃最新一条并经 [onNotice] 提示，不阻断。
/// 关闭可用 Disconnect 替换明确收到 Busy 的输入，不等待数据预算。
class DesktopOrderedSession extends ChangeNotifier {
  DesktopOrderedSession({
    required this.source,
    required this.target,
    required this.gateway,
    required this.font,
  });

  final DesktopSource source;
  final SessionTarget target;
  final DesktopSessionGateway gateway;
  final DesktopFontController font;
  final dismissed = ValueNotifier(false);

  DesktopPanePhase phase = DesktopPanePhase.preparing;
  DesktopPreparationResult? prepared;
  TerminalTheme? theme;
  DesktopNativeLifecycle? lifecycle;
  DesktopSessionStatus? status;
  DesktopPresentedFrame? frame;
  String error = '';

  /// Rust 按 profile 仲裁后回报的壳动作，由窗格执行。
  Future<void> Function(DesktopAction action)? performAction;
  Future<String?> Function()? readClipboard;
  Future<void> Function(DesktopPrompt prompt)? showPrompt;

  /// Copy 事件取到的选区文字。
  void Function(String text)? onClipboard;

  /// 不改变会话状态的瞬时提示（如本次粘贴被丢弃），由窗格短暂显示。
  void Function(String text)? onNotice;

  static const _busyRetry = Duration(milliseconds: 20);
  static const _resultTimeout = Duration(seconds: 10);
  static const _clipboardTimeout = Duration(seconds: 2);
  static const maxQueuedEvents = 1024;
  static const maxQueuedTextUnits = 512 * 1024;

  /// 粘贴分片上限（UTF-16 单元）。每单元最多 3 字节 UTF-8，Rust 按 3 倍字节加 512
  /// 预约，64 Ki 单元最坏约 576 KiB，低于 1 MiB 的单条准入预算。
  static const pasteChunkUnits = 64 * 1024;

  final _subscriptions = <StreamSubscription>[];
  final _frames = <int, (DesktopFrame, Uint8List)>{};
  final _projections = <int, DesktopNativeProjection>{};
  final _queue = ListQueue<DesktopEvent>();
  DesktopEvent? _inflight;
  int _seq = 0;
  Timer? _retry;
  Timer? _resultTimer;
  String _identity = '';
  int _request = 0;
  int _queryRequest = 0;
  int _prepareRequest = 0;
  int _startRequest = 0;
  int _cancelRequest = 0;
  int _lastFrame = 0;
  int _acknowledgedFrame = 0;
  DesktopGeometry? _measured;
  bool _begun = false;
  bool _startSent = false;
  bool _accepted = false;
  bool _closed = false;
  bool _disposed = false;
  bool _inputBlocked = false;
  bool _readingClipboard = false;
  DesktopEvent? _clipboardMarker;
  int _inputGeneration = 0;
  Future<bool>? _closeFuture;
  Completer<bool>? _ended;
  Timer? _closeTimer;

  bool get started => _startSent;
  bool get closing => _closed;
  String get identity => _identity;

  bool _origin(DesktopSource origin, String identity) =>
      !_disposed &&
      origin == source &&
      _identity.isNotEmpty &&
      identity == _identity;

  void begin() {
    if (_begun) throw StateError('会话已准备');
    _begun = true;
    _subscriptions.addAll([
      gateway.preparations.listen(
        (pack) => _preparation(pack.message, pack.binary),
      ),
      gateway.lifecycles.listen((pack) => _lifecycle(pack.message)),
      gateway.lifecycleQueries.listen((pack) {
        final message = pack.message;
        if (_origin(message.source, message.identity) &&
            message.requestId == _u64(_queryRequest) &&
            message.lifecycle != null) {
          _lifecycle(message.lifecycle!);
        }
      }),
      gateway.frames.listen((pack) {
        final message = pack.message;
        if (_closed ||
            !_origin(message.source, message.identity) ||
            message.frame.seq <= _lastFrame) {
          return;
        }
        _frames[message.frame.seq] = (message, Uint8List.fromList(pack.binary));
        _bound(_frames);
        _pair(message.frame.seq);
      }),
      gateway.projections.listen((pack) {
        final message = pack.message;
        if (_closed ||
            !_origin(message.source, message.identity) ||
            message.frameRevision <= _lastFrame ||
            lifecycle?.attempt != message.attempt) {
          return;
        }
        _projections[message.frameRevision] = message;
        _bound(_projections);
        _pair(message.frameRevision);
      }),
      gateway.statuses.listen((pack) {
        if (!_origin(pack.message.source, pack.message.identity)) return;
        status = pack.message;
        final state = pack.message.status.state;
        if (state == SessionState.failed || state == SessionState.closed) {
          dismissed.value = true;
        }
        if (_closed && state == SessionState.closed) _finishClose(true);
        notifyListeners();
      }),
      gateway.prompts.listen((pack) => unawaited(_prompt(pack.message))),
      gateway.invalidations.listen((pack) {
        if (!_origin(pack.message.source, pack.message.identity)) return;
        final current = frame?.projection;
        if (current != null &&
            pack.message.rowRevision.toBigInt() <
                current.rowRevision.toBigInt()) {
          return;
        }
        _frames.clear();
        _projections.clear();
        frame = null;
        notifyListeners();
      }),
      gateway.eventResults.listen((pack) => _result(pack.message)),
      gateway.clipboards.listen((pack) {
        if (_closed || !_origin(pack.message.source, pack.message.identity)) {
          return;
        }
        onClipboard?.call(pack.message.text);
      }),
    ]);
    _prepareRequest = ++_request;
    gateway.prepare(
      DesktopPrepare(
        version: 2,
        requestId: _prepareRequest,
        source: source,
        target: desktopTargetKind(target),
        profileId: target.profileId,
        connectionId: target.connectionId,
        host: target.host,
        port: target.port,
        username: target.username,
        command: target.command,
      ),
    );
  }

  Future<void> _preparation(
    DesktopPreparationResult message,
    Uint8List binary,
  ) async {
    if (_disposed ||
        message.source != source ||
        message.prepareRequestId != _prepareRequest) {
      return;
    }
    if (message.requestId != _prepareRequest &&
        message.requestId != _startRequest &&
        message.requestId != _cancelRequest) {
      return;
    }
    if (_identity.isNotEmpty &&
        message.identity.isNotEmpty &&
        _identity != message.identity) {
      return;
    }
    if (_closed) {
      if (message.state == DesktopPreparationState.accepted &&
          message.requestId == _startRequest) {
        _identity = message.identity;
        _accept();
      } else if (message.state == DesktopPreparationState.cancelled ||
          message.state == DesktopPreparationState.failed) {
        if (!_accepted) _finishClose(true);
      }
      return;
    }
    switch (message.state) {
      case DesktopPreparationState.preparing:
        return;
      case DesktopPreparationState.prepared:
        if (message.requestId != _prepareRequest ||
            prepared != null ||
            message.failure != DesktopFailure.none) {
          return;
        }
        _identity = message.identity;
        prepared = message;
        final profile = message.snapshot?.profile;
        if (profile == null) {
          _fail('准备快照缺少终端 profile');
          return;
        }
        try {
          theme = themeForProfile(profile.colorScheme);
        } on ArgumentError {
          _fail('所选 profile 的终端静态配色不受支持');
          return;
        }
        phase = DesktopPanePhase.loadingFont;
        notifyListeners();
        try {
          final material = await font.selectPrepared(message, binary);
          if (_closed || _disposed || material == null || prepared != message) {
            return;
          }
          // 字体变化之前的几何不能作为首次 PTY 度量。
          _measured = null;
          phase = DesktopPanePhase.measuring;
          notifyListeners();
        } catch (_) {
          _fail('准备的字体材料无效');
        }
      case DesktopPreparationState.accepted:
        if (!_startSent || message.requestId != _startRequest) return;
        phase = DesktopPanePhase.running;
        _accept();
        notifyListeners();
      case DesktopPreparationState.cancelled:
      case DesktopPreparationState.failed:
        _fail('桌面会话准备失败：${message.failure.name}');
    }
  }

  void _accept() {
    _accepted = true;
    _queryLifecycle();
    _pump();
  }

  /// 首次度量后发 Start；之后几何变化经有序通道 Resize。
  void measure(DesktopGeometry geometry) {
    if (_closed || _disposed) return;
    final changed = _measured != geometry;
    _measured = geometry;
    if (phase == DesktopPanePhase.measuring &&
        !_startSent &&
        font.current?.projection != null) {
      _startSent = true;
      phase = DesktopPanePhase.starting;
      _startRequest = ++_request;
      gateway.start(
        DesktopStart(
          version: 2,
          requestId: _startRequest,
          prepareRequestId: _prepareRequest,
          source: source,
          identity: _identity,
          geometry: geometry,
          fontProjection: font.current!.projection!,
          password: target.password,
        ),
      );
    } else if (changed && _startSent) {
      send(DesktopEventResize(geometry: geometry));
    }
  }

  void _queryLifecycle() {
    if (_identity.isEmpty) return;
    gateway.queryLifecycle(
      DesktopNativeLifecycleQuery(
        version: 1,
        source: source,
        identity: _identity,
        requestId: _u64(++_queryRequest),
      ),
    );
  }

  void _lifecycle(DesktopNativeLifecycle message) {
    if (message.version != 1 ||
        !_origin(message.source, message.identity) ||
        (lifecycle != null &&
            message.lifecycleRevision.toBigInt() <=
                lifecycle!.lifecycleRevision.toBigInt())) {
      return;
    }
    final previous = lifecycle;
    lifecycle = message;
    if (previous?.attempt != message.attempt) {
      _frames.clear();
      _projections.clear();
      frame = null;
      _lastFrame = 0;
      _acknowledgedFrame = 0;
    }
    switch (message.state) {
      case NativeLifecycleState.ended:
        dismissed.value = true;
        _queue.clear();
        if (_closed) _finishClose(true);
      case NativeLifecycleState.disconnected:
      case NativeLifecycleState.authenticationFailed:
      case NativeLifecycleState.attemptFailed:
        dismissed.value = true;
      default:
        break;
    }
    notifyListeners();
  }

  Future<void> _prompt(DesktopPrompt message) async {
    if (_closed || !_origin(message.source, message.identity)) return;
    dismissed.value = false;
    await showPrompt?.call(message);
  }

  void answer(
    DesktopPrompt prompt, {
    required bool accept,
    required List<String> answers,
    required bool remember,
  }) {
    if (_closed ||
        dismissed.value ||
        !_origin(prompt.source, prompt.identity)) {
      return;
    }
    gateway.reply(
      DesktopReply(
        version: 2,
        source: source,
        identity: _identity,
        promptId: prompt.prompt.promptId,
        accept: accept,
        answers: answers,
        remember: remember,
      ),
    );
  }

  /// 按序发送；鼠标移动、视口、尺寸与帧确认只保留队尾最新一条。
  void send(DesktopEvent event) {
    if (_disposed || (_closed && event is! DesktopEventDisconnect)) return;
    if (_inputBlocked && !_control(event)) return;
    if (_coalescible(event) &&
        _queue.isNotEmpty &&
        _coalescible(_queue.last) &&
        _queue.last.runtimeType == event.runtimeType) {
      _queue.removeLast();
    }
    final parts = _chunks(event);
    if (_accepted &&
        !_control(event) &&
        _queue.length + parts.length > maxQueuedEvents) {
      onNotice?.call('待发送输入过多，已丢弃本次输入');
      return;
    }
    if (!_enqueue(parts)) return;
    _pump();
  }

  /// 断开、重连与协议心跳/几何（帧确认、尺寸、视口）不是用户输入，不受输入阻断与条数上限影响。
  static bool _control(DesktopEvent event) =>
      event is DesktopEventDisconnect ||
      event is DesktopEventReconnect ||
      event is DesktopEventFrameAck ||
      event is DesktopEventResize ||
      event is DesktopEventViewport;

  /// 超过 [pasteChunkUnits] 的粘贴反复对半拆分（保持代理对完整）；其他事件原样返回。
  static List<DesktopEvent> _chunks(DesktopEvent event) {
    if (event is! DesktopEventPaste || event.text.length <= pasteChunkUnits) {
      return [event];
    }
    final halves = _split(event);
    if (halves == null) return [event];
    return [..._chunks(halves.$1), ..._chunks(halves.$2)];
  }

  static int _textUnits(DesktopEvent? event) => switch (event) {
    DesktopEventText(:final text) ||
    DesktopEventPaste(:final text) => text.length,
    DesktopEventKey(:final key) ||
    DesktopEventSoftKey(:final key) => key.length,
    _ => 0,
  };

  /// Start 受理前有总量上限，超限丢弃整段并阻断；受理后总是入队。
  bool _enqueue(List<DesktopEvent> events, {int? at}) {
    if (!_accepted && events.any((event) => event is! DesktopEventDisconnect)) {
      final units =
          events.fold<int>(0, (total, item) => total + _textUnits(item)) +
          _textUnits(_inflight) +
          _queue.fold<int>(0, (total, item) => total + _textUnits(item));
      if (_queue.length + events.length > maxQueuedEvents ||
          units > maxQueuedTextUnits) {
        // 丢弃整段未发送输入，避免仅丢文本后仍执行排队的 Enter。
        _queue.clear();
        _inputBlocked = true;
        _fail('待发送输入超过上限，请重新连接');
        return false;
      }
    }
    if (at == null) {
      _queue.addAll(events);
    } else {
      final queued = _queue.toList()..insertAll(at, events);
      _queue
        ..clear()
        ..addAll(queued);
    }
    return true;
  }

  static bool _coalescible(DesktopEvent event) =>
      event is DesktopEventViewport ||
      event is DesktopEventResize ||
      event is DesktopEventFrameAck ||
      (event is DesktopEventMouse && event.kind == 'move');

  void _pump() {
    if (_disposed ||
        !_accepted ||
        _inflight != null ||
        _retry != null ||
        _queue.isEmpty ||
        identical(_queue.first, _clipboardMarker)) {
      return;
    }
    _inflight = _queue.removeFirst();
    _transmit();
  }

  void _transmit() {
    _resultTimer?.cancel();
    _resultTimer = Timer(_resultTimeout, () {
      _inflight = null;
      _queue.clear();
      _fail('有序输入回执超时');
      if (_closed) _finishClose(false);
    });
    gateway.ordered(
      DesktopOrdered(
        version: 2,
        source: source,
        identity: _identity,
        seq: _u64(_seq + 1),
        event: _inflight!,
      ),
    );
  }

  void _result(DesktopEventResult message) {
    final event = _inflight;
    if (_disposed ||
        event == null ||
        _retry != null ||
        message.source != source ||
        message.identity != _identity ||
        message.seq.toBigInt() != BigInt.from(_seq + 1)) {
      return;
    }
    _resultTimer?.cancel();
    switch (message.failure) {
      case DesktopFailure.none:
      case DesktopFailure.invalidEvent:
      case DesktopFailure.policyUnavailable:
        ++_seq;
        _inflight = null;
        final action = message.action;
        if (message.failure == DesktopFailure.none && action != null) {
          if (action == DesktopAction.paste) {
            unawaited(pasteClipboard(beforeQueued: true));
          } else {
            unawaited(performAction?.call(action));
          }
        }
      case DesktopFailure.busy:
        if (_closed && event is! DesktopEventDisconnect) {
          _queue.clear();
          _inflight = const DesktopEventDisconnect();
          _transmit();
          return;
        } else if (event is DesktopEventMouse && event.kind == 'move') {
          _inflight = null;
        } else {
          _retry = Timer(_busyRetry, () {
            _retry = null;
            if (!_disposed && _inflight != null) _transmit();
          });
          return;
        }
      case DesktopFailure.inputTooLarge:
        final halves = _split(event);
        if (halves != null) {
          // 一条换两条，不受排队上限约束，保证已受理的粘贴完整送达。
          _inflight = halves.$1;
          _queue.addFirst(halves.$2);
          _transmit();
          return;
        }
        _inflight = null;
        _fail('单个输入过大，无法发送');
      default:
        // 来源或顺序失配：会话已结束或协议错位，seq 未被消费。
        _inflight = null;
        _queue.clear();
        if (_closed) {
          _finishClose(true);
        } else if (lifecycle?.state != NativeLifecycleState.ended) {
          _fail('有序输入被拒绝：${message.failure.name}');
        }
    }
    _pump();
  }

  static (DesktopEvent, DesktopEvent)? _split(DesktopEvent event) {
    final text = switch (event) {
      DesktopEventText(:final text) || DesktopEventPaste(:final text) => text,
      _ => null,
    };
    if (text == null) return null;
    if (text.length < 2) return null;
    var middle = text.length ~/ 2;
    if (text.codeUnitAt(middle) >= 0xdc00 &&
        text.codeUnitAt(middle) <= 0xdfff &&
        text.codeUnitAt(middle - 1) >= 0xd800 &&
        text.codeUnitAt(middle - 1) <= 0xdbff) {
      middle--;
    }
    if (middle == 0) middle = 2;
    if (middle >= text.length) return null;
    final head = text.substring(0, middle);
    final tail = text.substring(middle);
    return event is DesktopEventPaste
        ? (DesktopEventPaste(text: head), DesktopEventPaste(text: tail))
        : (DesktopEventText(text: head), DesktopEventText(text: tail));
  }

  void _pair(int revision) {
    final bytes = _frames[revision];
    final projection = _projections[revision];
    if (bytes == null || projection == null) return;
    _lastFrame = revision;
    frame = DesktopPresentedFrame(bytes.$1, bytes.$2, projection);
    _frames.removeWhere((key, _) => key <= revision);
    _projections.removeWhere((key, _) => key <= revision);
    notifyListeners();
  }

  static void _bound<T>(Map<int, T> values) {
    while (values.length > 2) {
      values.remove(values.keys.reduce((a, b) => a < b ? a : b));
    }
  }

  /// 帧实际绘制后确认，Rust 据此放行下一帧。
  void acknowledgePainted(DesktopPresentedFrame original) {
    if (_closed ||
        _disposed ||
        lifecycle?.attempt != original.projection.attempt ||
        _acknowledgedFrame == original.message.frame.seq) {
      return;
    }
    _acknowledgedFrame = original.message.frame.seq;
    send(DesktopEventFrameAck(frameSeq: original.message.frame.seq));
  }

  Future<bool> reconnect() async {
    if (_closed || !_startSent) return false;
    _inputBlocked = false;
    ++_inputGeneration;
    error = '';
    if (phase == DesktopPanePhase.failed && _accepted) {
      phase = DesktopPanePhase.running;
    }
    send(const DesktopEventReconnect());
    notifyListeners();
    return true;
  }

  /// 剪贴板读取属于当前输入顺序；完成前后续按键不能越过粘贴。
  Future<void> pasteClipboard({bool beforeQueued = false}) async {
    if (_closed || _disposed || _inputBlocked) return;
    if (_readingClipboard) {
      onNotice?.call('正在读取剪贴板，请稍后再粘贴');
      return;
    }
    _readingClipboard = true;
    final attempt = lifecycle?.attempt;
    final generation = _inputGeneration;
    // 占住输入位置：菜单粘贴排在已有输入后，按键裁决的粘贴排在后续按键前。
    final marker = DesktopEventPaste(text: '');
    _clipboardMarker = marker;
    if (!_enqueue([marker], at: beforeQueued ? 0 : null)) {
      _clipboardMarker = null;
      _readingClipboard = false;
      return;
    }
    _pump();
    try {
      final read = readClipboard?.call();
      // 读取挂住按失败处理，不能让占位一直挡住后续输入。
      final text = read == null ? null : await read.timeout(_clipboardTimeout);
      final index = _queue.toList().indexWhere(
        (event) => identical(event, marker),
      );
      if (index >= 0) _queue.removeWhere((event) => identical(event, marker));
      if (!_closed &&
          !_disposed &&
          !_inputBlocked &&
          generation == _inputGeneration &&
          lifecycle?.attempt == attempt &&
          index >= 0 &&
          text != null &&
          text.isNotEmpty) {
        _enqueue(_chunks(DesktopEventPaste(text: text)), at: index);
      }
    } catch (_) {
      // 只丢弃本次粘贴，已排队的其他输入照常发送。
      if (!_closed && !_disposed) onNotice?.call('无法读取剪贴板，本次粘贴已取消');
    } finally {
      _queue.removeWhere((event) => identical(event, marker));
      _clipboardMarker = null;
      _readingClipboard = false;
      _pump();
    }
  }

  /// Start 之前取消准备；之后有序 Disconnect，等 lifecycle ended 或 status closed。
  /// 未确认（含超时）时清除缓存结果，下一次调用重新发起关闭。
  Future<bool> close() => _closeFuture ??= _close().then((closed) {
    if (!closed) _closeFuture = null;
    return closed;
  });
  Future<bool> _close() async {
    if (_disposed) return false;
    _closed = true;
    phase = DesktopPanePhase.closing;
    dismissed.value = true;
    _ended = Completer<bool>();
    _closeTimer = Timer(const Duration(seconds: 10), () {
      _fail('关闭尚未确认');
      _finishClose(false);
    });
    // closed 的会话仍可重连，Rust 侧任务还在；只有 lifecycle ended 才无需断开。
    if (lifecycle?.state == NativeLifecycleState.ended) {
      _finishClose(true);
    } else if (!_startSent) {
      _cancelRequest = ++_request;
      gateway.cancel(
        DesktopCancel(
          version: 2,
          requestId: _cancelRequest,
          prepareRequestId: _prepareRequest,
          source: source,
          identity: _identity,
        ),
      );
    } else {
      _queue.clear();
      // Busy 已明确拒绝该序号，允许以 Disconnect 替换，不等待数据预算。
      if (_retry != null) {
        _retry!.cancel();
        _retry = null;
        _inflight = null;
      }
      send(const DesktopEventDisconnect());
    }
    return _ended!.future;
  }

  void _finishClose(bool success) {
    _closeTimer?.cancel();
    _retry?.cancel();
    _retry = null;
    _resultTimer?.cancel();
    _inflight = null;
    _queue.clear();
    if (success) phase = DesktopPanePhase.ended;
    if (_ended?.isCompleted == false) _ended!.complete(success);
  }

  /// widget 脱离后仍等关闭结论，再释放订阅。
  Future<void> detach() async {
    await close();
    dispose();
  }

  void _fail(String detail) {
    if (_disposed) return;
    error = detail;
    if (!_closed) phase = DesktopPanePhase.failed;
    notifyListeners();
  }

  @override
  void dispose() {
    if (_disposed) return;
    _disposed = true;
    for (final subscription in _subscriptions) {
      subscription.cancel();
    }
    _retry?.cancel();
    _resultTimer?.cancel();
    _closeTimer?.cancel();
    _queue.clear();
    font.dispose();
    dismissed.dispose();
    super.dispose();
  }

  static Uint64 _u64(int value) => Uint64(BigInt.from(value));
}

/// Prepare 的目标种类：本地终端走 LocalShell，System 连接走 SystemOpenSsh，其余走 Native SSH。
DesktopTargetKind desktopTargetKind(SessionTarget target) =>
    switch (target.kind) {
      SessionTargetKind.local => DesktopTargetKind.localShell,
      SessionTargetKind.systemSsh => DesktopTargetKind.systemOpenSsh,
      SessionTargetKind.ssh => DesktopTargetKind.nativeSsh,
    };

final _localExit = RegExp(
  r'^exit code=Some\((-?\d+)\) success=(?:true|false)$',
);

/// 本地 shell 或系统 ssh 退出后 Rust 状态 detail 的固定格式 `exit code=Some(N) success=…`
/// 中的退出码；其他格式（含 `None`）返回 null。
int? localShellExitCode(String detail) {
  final match = _localExit.firstMatch(detail);
  return match == null ? null : int.parse(match.group(1)!);
}

/// 适配器转来的按键 → 有序事件：带硬件 [KeyInputEvent.source] 的走物理 Key；
/// 没有来源的是软件按键（键位条、软键盘合成、挂住修饰的单字符），走 SoftKey。
DesktopEvent desktopOrderedKey(
  KeyInputEvent event, {
  required bool metaPressed,
}) {
  final source = event.source;
  if (source == null) {
    return desktopSoftKeyEvent(
      key: event.key,
      shift: event.shift,
      control: event.control,
      alt: event.alt,
    );
  }
  return desktopKeyEvent(
    key: event.key,
    phase: source is KeyRepeatEvent
        ? DesktopKeyPhase.repeat
        : DesktopKeyPhase.down,
    source: source,
    shift: event.shift,
    control: event.control,
    alt: event.alt,
    leftAlt: event.leftAlt,
    rightAlt: event.rightAlt,
    superKey: metaPressed,
  );
}

/// 软件按键：不声称物理键身份，Rust 不做物理锁存、绑定匹配或壳默认动作，按键名与
/// 按钮给出的修饰直接编码；挂住的 Alt 作为显式 Meta（ESC 前缀）。
DesktopEventSoftKey desktopSoftKeyEvent({
  required String key,
  required bool shift,
  required bool control,
  required bool alt,
}) => DesktopEventSoftKey(key: key, shift: shift, control: control, alt: alt);

/// 由 fork 转来的硬件按键（或窗格收到的 KeyUp）构造有序 Key 事件。
/// Rust 要求物理键身份；[source] 为空的事件会被拒绝，软件按键用 [desktopSoftKeyEvent]。
DesktopEventKey desktopKeyEvent({
  required String key,
  required DesktopKeyPhase phase,
  required KeyEvent? source,
  required bool shift,
  required bool control,
  required bool alt,
  required bool leftAlt,
  required bool rightAlt,
  required bool superKey,
}) {
  final known = source != null && alt == (leftAlt || rightAlt);
  return DesktopEventKey(
    key: key,
    phase: phase,
    physical: Uint64(BigInt.from(source?.physicalKey.usbHidUsage ?? 0)),
    logical: Uint64(BigInt.zero),
    shift: shift,
    control: control,
    alt: alt,
    leftAlt: leftAlt,
    rightAlt: rightAlt,
    altSource: known ? DesktopAltSource.known : DesktopAltSource.unknown,
    superKey: superKey,
    defaultAction: phase == DesktopKeyPhase.down && source != null
        ? desktopDefaultAction(
            source.logicalKey,
            control: control,
            shift: shift,
            alt: alt,
            meta: superKey,
          )
        : null,
  );
}

/// 壳快捷键、缩放与终端复制/粘贴/全选的默认候选；Rust 按 profile 绑定优先裁决。
/// macOS 上 ⌘ 与壳一致地参与壳快捷键（⌘⇧L）和缩放（⌘+/⌘−/⌘0）；其他平台 meta
/// 只参与终端复制/粘贴/全选的匹配。
DesktopAction? desktopDefaultAction(
  LogicalKeyboardKey key, {
  required bool control,
  required bool shift,
  required bool alt,
  required bool meta,
}) {
  final shellMeta = meta && defaultTargetPlatform == TargetPlatform.macOS;
  final command = desktopCommand(
    key,
    control: control,
    shift: shift,
    alt: alt,
    meta: shellMeta,
  );
  if (command != null) {
    return switch (command) {
      DesktopCommand.newTab => DesktopAction.newTab,
      DesktopCommand.closePane => DesktopAction.closePane,
      DesktopCommand.splitRight => DesktopAction.splitRight,
      DesktopCommand.splitDown => DesktopAction.splitDown,
      DesktopCommand.nextTab => DesktopAction.nextTab,
      DesktopCommand.previousTab => DesktopAction.previousTab,
      DesktopCommand.nextPane => DesktopAction.nextPane,
      DesktopCommand.search => DesktopAction.search,
      DesktopCommand.settings => DesktopAction.settings,
      DesktopCommand.toggleSidebar => DesktopAction.toggleSidebar,
      DesktopCommand.localTerminal => DesktopAction.newLocalTerminal,
    };
  }
  // 与 TerminalZoom.handleKey 的调用方相同：Windows 为 Ctrl（不含 Alt），
  // Apple 平台为 ⌘，加 =/+/-/0。
  if ((control && !alt) || shellMeta) {
    if (key == LogicalKeyboardKey.equal ||
        key == LogicalKeyboardKey.add ||
        key == LogicalKeyboardKey.numpadAdd) {
      return DesktopAction.zoomIn;
    }
    if (key == LogicalKeyboardKey.minus ||
        key == LogicalKeyboardKey.numpadSubtract) {
      return DesktopAction.zoomOut;
    }
    if (key == LogicalKeyboardKey.digit0 || key == LogicalKeyboardKey.numpad0) {
      return DesktopAction.zoomReset;
    }
  }
  for (final entry in defaultTerminalShortcuts.entries) {
    final activator = entry.key;
    if (activator is SingleActivator &&
        activator.trigger == key &&
        activator.control == control &&
        activator.shift == shift &&
        activator.alt == alt &&
        activator.meta == meta) {
      return switch (entry.value) {
        CopySelectionTextIntent() => DesktopAction.copy,
        PasteTextIntent() => DesktopAction.paste,
        SelectAllTextIntent() => DesktopAction.selectAll,
        _ => null,
      };
    }
  }
  return null;
}
