import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:terminal_view/terminal_view.dart' show TerminalTheme;

import '../bindings/bindings.dart';
import '../desktop/native_input/desktop_native_host.dart';
import '../desktop/workspace/desktop_workspace_scope.dart';
import 'desktop_font.dart';
import 'desktop_session_gateway.dart';
import 'desktop_terminal_theme.dart';
import 'session_target.dart';

enum DesktopPanePhase {
  preparing,
  loadingFont,
  measuring,
  starting,
  running,
  closing,
  ended,
  failed,
}

class DesktopPresentedFrame {
  const DesktopPresentedFrame(this.message, this.binary, this.projection);
  final DesktopFrame message;
  final Uint8List binary;
  final DesktopNativeProjection projection;
}

/// 存续到原会话与 claim 结算完成，不依赖 TerminalPane widget 的寿命。
/// 唯一 Dart 控制与 GUI 意图 writer；物理 Key/Text/Mouse 只由宿主发送。
class DesktopSessionController extends ChangeNotifier {
  DesktopSessionController({
    required this.source,
    required this.target,
    required this.gateway,
    required this.host,
    required this.font,
  });

  final DesktopSource source;
  final SessionTarget target;
  final DesktopSessionGateway gateway;
  final DesktopNativeHost host;
  final DesktopFontController font;
  final dismissed = ValueNotifier(false);
  final _subscriptions = <StreamSubscription>[];
  final _frames = <int, (DesktopFrame, Uint8List)>{};
  final _projections = <int, DesktopNativeProjection>{};
  final _roots = <String, _PaneRoot>{};
  final _pendingActions = <String>{};
  final _commands = <Uint64, Completer<bool>>{};
  final _commandOrigins = <Uint64, DesktopNativeCommand>{};
  final _commandTimers = <Uint64, Timer>{};
  final _dataRequests = <Uint64>{};
  Uint64? _frameAckRequest;
  Uint64? _disconnectRequest;
  DesktopPanePhase phase = DesktopPanePhase.preparing;
  DesktopPreparationResult? prepared;
  TerminalTheme? _theme;
  TerminalTheme? get theme => _theme;
  DesktopNativeLifecycle? lifecycle;
  DesktopSessionStatus? status;
  DesktopSelection? selection;
  DesktopPresentedFrame? frame;
  Map<Object?, Object?>? composition;
  String error = '';
  String _identity = '';
  int _request = 0;
  int _queryRequest = 0;
  int _lastFrame = 0;
  int _acknowledgedFrame = 0;
  int _prepareRequest = 0;
  int _startRequest = 0;
  int _cancelRequest = 0;
  bool _begun = false;
  bool _closed = false;
  bool _disposed = false;
  bool _detached = false;
  bool _startSent = false;
  bool _accepted = false;
  bool _disconnectSent = false;
  bool _active = false;
  bool _focusRequested = true;
  int _focusIntent = 0;
  _PaneRoot? _root;
  Future<bool>? _quietFuture;
  Future<bool>? _closeFuture;
  Timer? _closeTimer;
  Completer<bool>? _ended;
  Map<String, Object?>? _geometry;
  DesktopGeometry? _measured;
  bool _presenting = false;
  _NativePresentation? _pendingPresentation;
  _PendingFocus? _pendingFocus;

  Future<DesktopActionOutcome> Function(DesktopAction)? executeAction;
  Future<void> Function(DesktopPrompt)? showPrompt;
  bool Function()? canFocus;
  Future<void> Function(DesktopNativeContextMenuReady)? showContextMenu;
  bool _menuShowing = false;
  Uint64? _lastMenuEffect;
  int _latched = 0;
  int _locked = 0;
  int _modifierRevision = 0;
  int _guiInteractionRevision = 0;
  _GuiPending? _guiPending;
  bool _guiUnknown = false;
  NativeCommandAdmission? _guiAdmission;
  NativeCommandAdmission? get guiAdmission => _guiAdmission;
  int get guiInteractionRevision => _guiInteractionRevision;
  int get modifierRevision => _modifierRevision;
  bool get guiBusy => _guiPending != null;
  bool get guiUnknown => _guiUnknown;
  bool get guiAvailable =>
      active && !_disposed && !_guiUnknown && canFocus?.call() == true;
  bool isModifierLatched(String id) => (_latched & _modifierBit(id)) != 0;
  bool isModifierLocked(String id) => (_locked & _modifierBit(id)) != 0;
  static int _modifierBit(String id) => switch (id) {
    'ctrl' => 1,
    'alt' => 2,
    _ => throw ArgumentError.value(id),
  };
  void tapModifier(String id) {
    if (!guiAvailable) return;
    final bit = _modifierBit(id);
    if ((_locked & bit) != 0) {
      _locked &= ~bit;
    } else if ((_latched & bit) != 0) {
      _latched &= ~bit;
      _locked |= bit;
    } else {
      _latched |= bit;
    }
    ++_modifierRevision;
    notifyListeners();
  }

  void lockModifier(String id) {
    if (!guiAvailable) return;
    final bit = _modifierBit(id);
    _latched &= ~bit;
    _locked |= bit;
    ++_modifierRevision;
    notifyListeners();
  }

  void clearGuiModifiers() {
    _guiAdmission = null;
    _latched = _locked = 0;
    ++_modifierRevision;
    ++_guiInteractionRevision;
    if (!_disposed) notifyListeners();
  }

  Future<NativeCommandAdmission> guiInput(
    GuiInput input, {
    int button = 0,
  }) async {
    final root = _root;
    if (!guiAvailable ||
        guiBusy ||
        root == null ||
        root.retired ||
        canFocus?.call() != true ||
        (button & ~3) != 0) {
      return NativeCommandAdmission.rejected;
    }
    if (input is GuiInputGuiText) {
      final scalars = input.value.runes.toList();
      if (scalars.isEmpty ||
          scalars.length > 32 ||
          scalars.any((value) => value >= 0xd800 && value <= 0xdfff)) {
        return NativeCommandAdmission.rejected;
      }
    }
    final intent = GuiIntent(
      input: input,
      modifiers: GuiModifiers(
        button: button,
        latched: _latched,
        locked: _locked,
      ),
      modifierRevision: _u64(_modifierRevision),
    );
    final command = DesktopNativeCommand(
      version: 1,
      source: source,
      identity: _identity,
      attempt: root.attempt,
      focusEpoch: root.focusEpoch,
      requestId: _u64(++_request),
      command: NativeCommandGuiIntent(value: intent),
    );
    final pending = _GuiPending(command, root, _focusIntent, intent);
    _guiAdmission = null;
    _guiPending = pending;
    pending.timer = Timer(
      const Duration(seconds: 5),
      () => _isolateGui(pending),
    );
    notifyListeners();
    try {
      gateway.command(command);
    } catch (_) {
      _isolateGui(pending);
    }
    return pending.completion.future;
  }

  void _isolateGui(_GuiPending pending) {
    if (_guiPending != pending) return;
    pending.timer?.cancel();
    _guiUnknown = true;
    ++_guiInteractionRevision;
    if (!pending.completion.isCompleted) {
      pending.completion.complete(NativeCommandAdmission.unknown);
    }
    notifyListeners();
  }

  static bool _commandMatches(
    DesktopNativeCommand command,
    DesktopNativeCommandResult result,
    Uint64? revision,
  ) =>
      result.version == command.version &&
      result.source == command.source &&
      result.identity == command.identity &&
      result.attempt == command.attempt &&
      result.focusEpoch == command.focusEpoch &&
      result.requestId == command.requestId &&
      result.guiModifierRevision == revision;

  void _commandResult(DesktopNativeCommandResult message) {
    if (_disposed) return;
    final pending = _guiPending;
    if (pending != null &&
        _commandMatches(
          pending.command,
          message,
          pending.intent.modifierRevision,
        )) {
      if (message.admission == NativeCommandAdmission.unknown ||
          message.failure == NativeCommandFailure.duplicate) {
        _isolateGui(pending);
        return;
      }
      final modifiers = pending.intent.modifiers;
      final eligible =
          pending.intent.input is GuiInputGuiText &&
              (pending.intent.input as GuiInputGuiText).value.runes.length != 1
          ? 0
          : modifiers.latched & ~(modifiers.button | modifiers.locked);
      if ((message.consumedLatches & ~eligible) != 0 ||
          (message.admission == NativeCommandAdmission.accepted &&
              message.failure != NativeCommandFailure.none) ||
          (message.admission == NativeCommandAdmission.rejected &&
              message.consumedLatches != 0)) {
        _isolateGui(pending);
        return;
      }
      pending.timer?.cancel();
      _guiPending = null;
      _guiUnknown = false;
      final currentOwner =
          !_closed &&
          active &&
          _root == pending.root &&
          !pending.root.retired &&
          pending.command.focusEpoch == pending.root.focusEpoch &&
          pending.focusIntent == _focusIntent &&
          pending.intent.modifierRevision == _u64(_modifierRevision);
      if (currentOwner) _guiAdmission = message.admission;
      if (message.admission == NativeCommandAdmission.accepted &&
          currentOwner) {
        if (message.consumedLatches != 0) {
          _latched &= ~message.consumedLatches;
          ++_modifierRevision;
        }
      }
      if (!pending.completion.isCompleted) {
        pending.completion.complete(message.admission);
      }
      notifyListeners();
      return;
    }
    final command = _commandOrigins[message.requestId];
    if (command == null ||
        !_commandMatches(command, message, null) ||
        message.consumedLatches != 0) {
      return;
    }
    final waiting = _commands.remove(message.requestId);
    if (waiting == null) return;
    _commandOrigins.remove(message.requestId);
    _dataRequests.remove(message.requestId);
    _commandTimers.remove(message.requestId)?.cancel();
    final accepted =
        message.admission == NativeCommandAdmission.accepted &&
        message.failure == NativeCommandFailure.none;
    waiting.complete(accepted);
    if (!accepted && !_closed) _fail('原控制命令未受理：${message.failure.name}');
  }

  bool get active => _active && !_closed;
  bool get editableSafe =>
      !_attaching && _root == null && _roots.values.every((root) => root.quiet);
  bool get closing => _closed;
  bool get started => _startSent;
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
      host.notifications.listen(_hostNotification),
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
        if (_closed || !_origin(pack.message.source, pack.message.identity)) {
          return;
        }
        status = pack.message;
        if (status!.status.state == SessionState.failed ||
            status!.status.state == SessionState.closed) {
          dismissed.value = true;
        }
        notifyListeners();
      }),
      gateway.selections.listen((pack) {
        if (_closed || !_origin(pack.message.source, pack.message.identity)) {
          return;
        }
        selection = pack.message;
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
        selection = null;
        frame = null;
        unawaited(invalidateGeometry());
        notifyListeners();
      }),
      gateway.actions.listen((pack) => unawaited(_action(pack.message))),
      gateway.contextMenus.listen(
        (pack) => unawaited(_contextMenu(pack.message)),
      ),
      gateway.commandResults.listen((pack) => _commandResult(pack.message)),
      gateway.textEnds.listen((pack) {
        final message = pack.message;
        if (message.version != 1 ||
            !_origin(message.source, message.identity)) {
          return;
        }
        final root = _roots[_binding(message.attempt, message.focusEpoch)];
        if (root == null) {
          if (_root?.attempt == message.attempt &&
              _root?.focusEpoch == message.focusEpoch &&
              active) {
            _fail('原文本终结通知没有对应宿主');
          }
          return;
        }
        unawaited(
          host
              .textEndRequired(root.token, {
                'attempt': message.attempt.toString(),
                'focusEpoch': message.focusEpoch.toString(),
                'contextId': message.contextId.toString(),
                'transactionId': message.transactionId.toString(),
                'endFence': message.endFence.toString(),
              })
              .catchError((Object _) => _fail('原生文本终结未确认')),
        );
      }),
    ]);
    _prepareRequest = ++_request;
    gateway.prepare(
      DesktopPrepare(
        version: 2,
        requestId: _prepareRequest,
        source: source,
        target: switch (target.kind) {
          SessionTargetKind.local => DesktopTargetKind.localShell,
          SessionTargetKind.systemSsh => DesktopTargetKind.systemOpenSsh,
          SessionTargetKind.ssh => DesktopTargetKind.nativeSsh,
        },
        profileId: target.profileId,
        connectionId: target.connectionId,
        host: target.host,
        port: target.port,
        username: target.username,
        command: target.command,
      ),
    );
  }

  Future<void> _contextMenu(DesktopNativeContextMenuReady message) async {
    final root = _root;
    if (_closed ||
        !active ||
        _menuShowing ||
        message.version != 1 ||
        !_origin(message.source, message.identity) ||
        root == null ||
        root.retired ||
        root.attempt != message.attempt ||
        root.focusEpoch != message.focusEpoch ||
        canFocus?.call() != true ||
        (_lastMenuEffect != null &&
            message.effect.toBigInt() <= _lastMenuEffect!.toBigInt())) {
      return;
    }
    _lastMenuEffect = message.effect;
    _menuShowing = true;
    try {
      await showContextMenu?.call(message);
    } catch (_) {
      _fail('终端右键菜单未完成');
    } finally {
      _menuShowing = false;
    }
  }

  void _hostNotification(Map<Object?, Object?> message) {
    if (_disposed) return;
    final matching = _roots.values.where(
      (root) => root.token.value == message['paneToken'],
    );
    if (matching.isEmpty) return;
    final root = matching.first;
    if (message['method'] == 'paneState') {
      if (message['state'] == 'nativequiet') {
        root.quiet = true;
        root.retired = true;
        if (root == _root) {
          clearGuiModifiers();
          _active = false;
          _focusRequested = false;
          _root = null;
          ++_focusIntent;
          composition = null;
          _quietFuture = Future.value(true);
        }
        if (root.claims == 0) unawaited(_release(root));
      } else if (message['state'] == 'inactive') {
        if (root == _root) {
          clearGuiModifiers();
          _active = false;
          _focusRequested = false;
          ++_focusIntent;
          composition = null;
        }
      }
      notifyListeners();
    } else if (message['method'] == 'compositionLayout') {
      final context = message['contextId'];
      final contextId = BigInt.tryParse('$context');
      final transaction = BigInt.tryParse('${message['transactionId']}');
      final edit = BigInt.tryParse('${message['editRevision']}');
      if (context is! String ||
          contextId == null ||
          contextId <= BigInt.zero ||
          transaction == null ||
          edit == null ||
          transaction <= BigInt.zero ||
          edit < BigInt.zero) {
        return;
      }
      final ended = message['state'] == 'ended';
      if (ended &&
          composition?['paneToken'] == root.token.value &&
          composition?['contextId'] == context &&
          composition?['transactionId'] == message['transactionId'] &&
          edit >= BigInt.parse(composition!['editRevision'] as String)) {
        composition = null;
        notifyListeners();
      }
      if (contextId < root.compositionContext) return;
      final previous = contextId == root.compositionContext
          ? root.compositionTransaction
          : null;
      if (previous != null &&
          (transaction < previous.$1 ||
              (transaction == previous.$1 &&
                  (previous.$3 || edit < previous.$2)))) {
        return;
      }
      if (ended) {
        root.compositionContext = contextId;
        root.compositionTransaction = (transaction, edit, true);
        return;
      }
      if (_closed ||
          root.retired ||
          root != _root ||
          !active ||
          frame?.projection.frameRevision != message['frameRevision'] ||
          message['text'] is! String ||
          message['contextId'] is! String ||
          message['transactionId'] is! String ||
          message['editRevision'] is! String ||
          message['acpStart'] is! int) {
        return;
      }
      root.compositionContext = contextId;
      root.compositionTransaction = (transaction, edit, false);
      composition = message;
      notifyListeners();
    }
  }

  Future<void> compositionPainted(
    Map<Object?, Object?> original,
    Map<String, Object?> layout,
  ) async {
    final root = _root;
    final owner = _focusIntent;
    final epoch = root?.focusEpoch;
    final originalFrame = frame;
    final context = original['contextId'];
    final transaction = original['transactionId'];
    final edit = original['editRevision'];
    bool current() =>
        !_disposed &&
        !_closed &&
        active &&
        _root == root &&
        root != null &&
        !root.retired &&
        root.focusEpoch == epoch &&
        _focusIntent == owner &&
        identical(composition, original) &&
        composition?['contextId'] == context &&
        composition?['transactionId'] == transaction &&
        composition?['editRevision'] == edit &&
        frame == originalFrame &&
        frame?.projection.frameRevision == original['frameRevision'];
    if (_closed ||
        root == null ||
        root.retired ||
        composition != original ||
        frame?.projection.frameRevision != original['frameRevision']) {
      return;
    }
    try {
      final accepted = await host.textLayout(root.token, {
        'contextId': original['contextId'],
        'transactionId': original['transactionId'],
        'editRevision': original['editRevision'],
        'acpStart': original['acpStart'],
        'frameRevision': original['frameRevision'],
        ...layout,
      });
      if (current() && !accepted) _fail('原 composition 文字布局未被接受');
    } catch (_) {
      if (current()) _fail('原 composition 文字布局未被接受');
    }
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
        _accepted = true;
        _queryLifecycle();
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
          _fail('原准备快照缺少终端 profile');
          return;
        }
        try {
          _theme = themeForProfile(profile.colorScheme);
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
        _accepted = true;
        phase = DesktopPanePhase.running;
        _queryLifecycle();
        notifyListeners();
      case DesktopPreparationState.cancelled:
      case DesktopPreparationState.failed:
        _fail('桌面会话准备失败：${message.failure.name}');
    }
  }

  void measure(
    DesktopGeometry geometry,
    Map<String, Object?> presentedGeometry,
  ) {
    if (_closed || _disposed) return;
    final changed = _measured != geometry;
    if (changed) clearGuiModifiers();
    _geometry = presentedGeometry;
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
    } else if (changed && lifecycle != null && lifecycle!.attempt != _u64(0)) {
      event(DesktopEventResize(geometry: geometry), requiresFocus: false);
    }
    unawaited(_attach());
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
      clearGuiModifiers();
      _active = false;
      composition = null;
      _lastMenuEffect = null;
      _pendingPresentation = null;
      ++_focusIntent;
      final old = _root;
      _root = null;
      if (old != null) unawaited(_retireRoot(old));
      _frames.clear();
      _projections.clear();
      frame = null;
      _lastFrame = 0;
      _acknowledgedFrame = 0;
    } else if (_root != null) {
      final root = _root!;
      final pending = _pendingFocus;
      // 生命周期不授予焦点；待决宿主 RPC 的回执必须再核对最新 epoch。
      final verifying =
          pending != null &&
          pending.root == root &&
          pending.intent == _focusIntent &&
          root.attempt == message.attempt &&
          !root.retired &&
          message.state == NativeLifecycleState.authenticated &&
          message.focusEpoch.toBigInt() > root.focusEpoch.toBigInt();
      if (root.focusEpoch == message.focusEpoch) {
        _roots[_binding(message.attempt, message.focusEpoch)] = root;
      }
      if (previous?.focusEpoch != message.focusEpoch &&
          root.focusEpoch != message.focusEpoch &&
          !verifying) {
        unawaited(quiet());
      }
    }
    if (message.state == NativeLifecycleState.ended) {
      dismissed.value = true;
      _active = false;
      _focusRequested = false;
      unawaited(quiet());
      if (_closed) _finishClose(true);
    } else if (_closed) {
      _disconnect();
    } else if (message.state == NativeLifecycleState.authenticated) {
      unawaited(_attach());
    } else if (message.state == NativeLifecycleState.inputRevoked) {
      _active = false;
      _focusRequested = false;
      unawaited(quiet());
    } else if (message.state == NativeLifecycleState.disconnected ||
        message.state == NativeLifecycleState.authenticationFailed ||
        message.state == NativeLifecycleState.attemptFailed) {
      _active = false;
      _focusRequested = false;
      dismissed.value = true;
      unawaited(quiet());
    }
    notifyListeners();
  }

  bool _attaching = false;
  Completer<void>? _attachCompletion;
  Future<void> _attach({bool focusAfterAttach = true}) async {
    final current = lifecycle;
    final geometry = _geometry;
    if (_attaching ||
        _closed ||
        _root != null ||
        _roots.values.any((root) => !root.quiet) ||
        geometry == null ||
        !_focusRequested ||
        canFocus?.call() != true ||
        (current?.state != NativeLifecycleState.authenticated &&
            current?.state != NativeLifecycleState.inputRevoked)) {
      return;
    }
    if (_roots.values.toSet().length >= 64) {
      _fail('原生输入资源待结算额度已满');
      return;
    }
    _attaching = true;
    notifyListeners();
    _attachCompletion = Completer<void>();
    final intent = _focusIntent;
    try {
      final token = await host.attachPane(
        origin: source,
        identity: _identity,
        attempt: current!.attempt,
        focusEpoch: current.focusEpoch,
        lifecycleRevision: current.lifecycleRevision,
        geometry: geometry,
      );
      final root = _PaneRoot(token, current.attempt, current.focusEpoch);
      _roots[_binding(current.attempt, current.focusEpoch)] = root;
      if (_closed ||
          intent != _focusIntent ||
          canFocus?.call() != true ||
          lifecycle?.attempt != current.attempt) {
        await _retireRoot(root);
        return;
      }
      _root = root;
      if (focusAfterAttach && canFocus?.call() == true) await requestFocus();
    } catch (_) {
      _fail('原生输入宿主登记失败');
    } finally {
      _attaching = false;
      _attachCompletion!.complete();
      _disposeDetachedIfFinished();
    }
  }

  Future<bool> requestFocus() {
    final pending = _pendingFocus;
    if (!_closed &&
        !_disposed &&
        canFocus?.call() == true &&
        pending != null &&
        pending.root == _root &&
        pending.intent == _focusIntent &&
        !pending.root.retired) {
      return pending.completion.future;
    }
    return _requestFocus();
  }

  Future<bool> _requestFocus() async {
    if (_closed || _disposed || canFocus?.call() != true) return false;
    _focusRequested = true;
    if (_root == null && !_closed && canFocus?.call() == true) {
      await _attach(focusAfterAttach: false);
    }
    final root = _root;
    if (_closed || _disposed || root == null || canFocus?.call() != true) {
      return false;
    }
    final intent = ++_focusIntent;
    final original = lifecycle;
    if (original == null || original.attempt != root.attempt || root.retired) {
      return false;
    }
    final pending = _PendingFocus(root, intent, original.lifecycleRevision);
    _pendingFocus = pending;
    _active = false;
    composition = null;
    clearGuiModifiers();
    try {
      final result = await _verifyFocus(pending);
      pending.completion.complete(result);
      return result;
    } finally {
      if (_pendingFocus == pending) _pendingFocus = null;
      if (!pending.completion.isCompleted) pending.completion.complete(false);
    }
  }

  Future<bool> _verifyFocus(_PendingFocus pending) async {
    final root = pending.root;
    DesktopNativeFocus focus;
    try {
      focus = await host.requestFocus(root.token);
    } catch (_) {
      if (!_disposed &&
          !_closed &&
          _root == root &&
          pending.intent == _focusIntent) {
        _fail('原生终端焦点未确认');
      }
      await _abandonFocus(pending);
      return false;
    }
    final latest = lifecycle;
    if (_disposed ||
        _closed ||
        pending.intent != _focusIntent ||
        _root != root ||
        root.retired ||
        canFocus?.call() != true ||
        !focus.active ||
        focus.attempt != root.attempt ||
        focus.focusEpoch == null ||
        focus.focusEpoch!.toBigInt() < root.focusEpoch.toBigInt() ||
        latest?.attempt != root.attempt ||
        (latest!.lifecycleRevision.toBigInt() >
                pending.lifecycleRevision.toBigInt() &&
            (latest.state != NativeLifecycleState.authenticated ||
                latest.focusEpoch != focus.focusEpoch))) {
      await _abandonFocus(pending);
      return false;
    }
    root.focusEpoch = focus.focusEpoch!;
    _roots[_binding(root.attempt, root.focusEpoch)] = root;
    _quietFuture = null;
    _active = true;
    _queryLifecycle();
    notifyListeners();
    return true;
  }

  Future<void> _abandonFocus(_PendingFocus pending) async {
    final current = _root == pending.root && _focusIntent == pending.intent;
    if (current) {
      _root = null;
      _active = false;
      _focusRequested = false;
      composition = null;
      ++_focusIntent;
      clearGuiModifiers();
    }
    final retiring = _retireRoot(pending.root);
    if (current) _quietFuture = retiring;
    await retiring;
  }

  Future<bool> quiet() async {
    clearGuiModifiers();
    _active = false;
    _focusRequested = false;
    composition = null;
    ++_focusIntent;
    notifyListeners();
    if (_attaching) await _attachCompletion!.future;
    final root = _root;
    _root = null;
    if (root == null) return _quietFuture ?? Future.value(!_attaching);
    final future = _retireRoot(root);
    _quietFuture = future;
    return future;
  }

  Future<bool> _retireRoot(_PaneRoot root) async {
    root.retired = true;
    try {
      root.quiet = await (root.quietFuture ??= host.retirePane(
        root.token,
        release: false,
      ));
      if (!_disposed) notifyListeners();
      if (root.quiet && root.claims == 0) unawaited(_release(root));
      return root.quiet;
    } catch (_) {
      _fail('原生输入尚未静默，不能打开新的输入框');
      return false;
    }
  }

  Future<void> _release(_PaneRoot root) async {
    if (root.released || !root.quiet || root.claims != 0) return;
    try {
      root.released = await (root.releaseFuture ??= host.retirePane(
        root.token,
        release: true,
      ));
      if (!root.released) _fail('原生输入资源尚未释放');
      if (root.released) {
        _roots.removeWhere((_, value) => identical(value, root));
        if (!root.releaseCompleted.isCompleted) {
          root.releaseCompleted.complete(true);
        }
        _disposeDetachedIfFinished();
      }
    } catch (_) {
      _fail('原生输入资源释放未确认');
      if (!root.releaseCompleted.isCompleted) {
        root.releaseCompleted.complete(false);
      }
    }
  }

  Future<void> _prompt(DesktopPrompt message) async {
    if (_closed || !_origin(message.source, message.identity)) return;
    final original = lifecycle?.attempt;
    if (!await quiet() || _closed || lifecycle?.attempt != original) return;
    final intent = _focusIntent;
    dismissed.value = false;
    await showPrompt?.call(message);
    if (!_closed &&
        _focusIntent == intent &&
        lifecycle?.attempt == original &&
        canFocus?.call() == true) {
      _focusRequested = true;
      unawaited(_attach());
    }
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

  void event(DesktopEvent event, {bool requiresFocus = true}) {
    if (event is DesktopEventKey ||
        event is DesktopEventText ||
        event is DesktopEventMouse) {
      throw ArgumentError('正常物理输入仅由原生宿主发送');
    }
    if (_closed &&
        event is! DesktopEventDisconnect &&
        event is! DesktopEventFrameAck) {
      return;
    }
    unawaited(
      _command(NativeCommandEvent(value: event), requiresFocus: requiresFocus),
    );
  }

  Future<bool> action(DesktopAction action) async {
    if (!active && !await requestFocus()) return false;
    if (!active) return false;
    final pending = _command(
      NativeCommandAction(value: action),
      requiresFocus: true,
    );
    if (pending == null) return false;
    return pending;
  }

  Future<bool> reconnect() async {
    if (_closed || !_startSent || !await quiet()) return false;
    _focusRequested = canFocus?.call() == true;
    error = '';
    final accepted = _command(
      const NativeCommandEvent(value: DesktopEventReconnect()),
      requiresFocus: false,
    );
    return accepted == null ? false : await accepted;
  }

  Future<bool>? _command(NativeCommand command, {required bool requiresFocus}) {
    final origin = lifecycle;
    if (_disposed ||
        origin == null ||
        origin.attempt == _u64(0) ||
        (requiresFocus && !active)) {
      return null;
    }
    final frameAck =
        command is NativeCommandEvent && command.value is DesktopEventFrameAck;
    final disconnect =
        command is NativeCommandEvent &&
        command.value is DesktopEventDisconnect;
    if (!frameAck && !disconnect && _dataRequests.length >= 64) {
      _fail('非键命令待决额度已满');
      return null;
    }
    if (frameAck && _frameAckRequest != null) {
      // 新帧已绘制；旧 ACK 的未决关联不占数据额度，也不伪造其准入结果。
      final previous = _commands.remove(_frameAckRequest);
      _commandOrigins.remove(_frameAckRequest);
      _commandTimers.remove(_frameAckRequest)?.cancel();
      if (previous?.isCompleted == false) previous!.complete(false);
    }
    if (disconnect &&
        _disconnectRequest != null &&
        _commands.containsKey(_disconnectRequest)) {
      return _commands[_disconnectRequest]!.future;
    }
    final requestId = _u64(++_request);
    final waiting = Completer<bool>();
    _commands[requestId] = waiting;
    if (frameAck) {
      _frameAckRequest = requestId;
    } else if (disconnect) {
      _disconnectRequest = requestId;
    } else {
      _dataRequests.add(requestId);
    }
    _commandTimers[requestId] = Timer(const Duration(seconds: 10), () {
      _commandTimers.remove(requestId);
      _commands.remove(requestId);
      _commandOrigins.remove(requestId);
      _dataRequests.remove(requestId);
      if (!waiting.isCompleted) waiting.complete(false);
      _fail('原非键命令受理未确认');
    });
    final request = DesktopNativeCommand(
      version: 1,
      source: source,
      identity: _identity,
      attempt: origin.attempt,
      focusEpoch: requiresFocus ? _root!.focusEpoch : origin.focusEpoch,
      requestId: requestId,
      command: command,
    );
    _commandOrigins[requestId] = request;
    gateway.command(request);
    return waiting.future;
  }

  Future<void> _action(DesktopNativeActionReady ready) async {
    if (ready.version != 1 ||
        _closed ||
        !_origin(ready.source, ready.identity)) {
      return;
    }
    final root = _roots[_binding(ready.attempt, ready.focusEpoch)];
    final id = '${_binding(ready.attempt, ready.focusEpoch)}:${ready.effect}';
    if (root == null || root.retired || _pendingActions.contains(id)) return;
    if (_pendingActions.length >= 64) {
      _fail('原生动作待决额度已满');
      return;
    }
    _pendingActions.add(id);
    ++root.claims;
    DesktopActionClaim? claim;
    bool rejected = false;
    bool settled = false;
    try {
      claim = await host.claimAction(root.token, ready.effect);
      if (claim == null) {
        rejected = true;
        return;
      }
      if (_closed || root.retired) {
        settled = await host.completeAction(claim, kind: 'abort');
        return;
      }
      switch (claim.action) {
        case DesktopAction.copy:
          await Clipboard.setData(ClipboardData(text: claim.payload));
          settled = await host.completeAction(claim, kind: 'completed');
        case DesktopAction.paste:
          final text = (await Clipboard.getData('text/plain'))?.text;
          final authorized =
              !_closed &&
              active &&
              _root == root &&
              !root.retired &&
              root.attempt == ready.attempt &&
              root.focusEpoch == ready.focusEpoch;
          settled = await host.completeAction(
            claim,
            kind: authorized && text != null ? 'paste' : 'abort',
            text: authorized ? text : null,
          );
        case DesktopAction.selectAll:
          // 原引擎动作不应成为 GUI claim，不以空执行冒充成功。
          settled = await host.completeAction(claim, kind: 'abort');
        default:
          final outcome = await executeAction?.call(claim.action);
          settled = await host.completeAction(
            claim,
            kind: outcome == DesktopActionOutcome.executed
                ? 'completed'
                : 'abort',
          );
      }
      if (!settled) settled = await host.completeAction(claim, kind: 'abort');
    } catch (_) {
      if (claim != null) {
        try {
          settled = await host.completeAction(claim, kind: 'abort');
        } catch (_) {
          /* 原 root 保留待结算。 */
        }
      }
      _fail('原生动作未完成');
    } finally {
      if (rejected || settled) {
        --root.claims;
        _pendingActions.remove(id);
        if (root.retired && root.quiet) await _release(root);
      } else {
        _fail('原生动作未结算，保留原资源');
      }
    }
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

  Future<void> presented(
    DesktopPresentedFrame original,
    Map<String, Object?> geometry,
  ) async {
    if (_closed ||
        _disposed ||
        !_origin(original.message.source, original.message.identity) ||
        !_origin(original.projection.source, original.projection.identity) ||
        lifecycle?.attempt != original.projection.attempt) {
      return;
    }
    final root = _root;
    // FrameAck 证明对应帧已真正经过绘制，不等待 native hit-test 的完成。
    acknowledgePainted(original);
    if (frame != original) return;
    if (root == null ||
        root.attempt != original.projection.attempt ||
        !active) {
      return;
    }
    final request = _NativePresentation(
      original,
      geometry,
      root,
      root.focusEpoch,
      _focusIntent,
    );
    await _present(request);
  }

  bool _presentationCurrent(_NativePresentation request) =>
      !_disposed &&
      !_closed &&
      active &&
      _root == request.root &&
      !request.root.retired &&
      request.root.focusEpoch == request.epoch &&
      _focusIntent == request.intent &&
      frame == request.frame;

  Future<void> _present(_NativePresentation request) async {
    if (!_presentationCurrent(request)) return;
    if (_presenting) {
      _pendingPresentation = request;
      return;
    }
    _presenting = true;
    final projection = request.frame.projection;
    try {
      final accepted = await host.presentedFrame(request.root.token, {
        ...request.geometry,
        'frameRevision': projection.frameRevision,
        'rowRevision': projection.rowRevision.toString(),
        'attempt': projection.attempt.toString(),
        'focusEpoch': request.epoch.toString(),
        'viewportTop': projection.viewportTop,
        'caret': projection.caret == null
            ? null
            : {
                'x': projection.caret!.x,
                'y': projection.caret!.y,
                'width': projection.caret!.width,
                'height': projection.caret!.height,
                'dpi': projection.caret!.dpi,
              },
      });
      if (!accepted && _presentationCurrent(request)) _fail('原帧呈现几何未被宿主接受');
    } catch (_) {
      if (_presentationCurrent(request)) _fail('原帧呈现几何未被宿主接受');
    } finally {
      _presenting = false;
      final pending = _pendingPresentation;
      _pendingPresentation = null;
      if (pending != null && !_closed && !_disposed) {
        unawaited(_present(pending));
      }
    }
  }

  Future<void> invalidateGeometry() async {
    final root = _root;
    if (root != null) await host.invalidateGeometry(root.token);
  }

  void acknowledgePainted(DesktopPresentedFrame original) {
    if (_closed ||
        _disposed ||
        lifecycle?.attempt != original.projection.attempt ||
        _acknowledgedFrame == original.message.frame.seq) {
      return;
    }
    _acknowledgedFrame = original.message.frame.seq;
    event(
      DesktopEventFrameAck(frameSeq: original.message.frame.seq),
      requiresFocus: false,
    );
  }

  /// 未确认（含超时）时清除缓存结果并允许重发 Disconnect，下一次调用重新发起关闭。
  Future<bool> close() => _closeFuture ??= _close().then((closed) {
    if (!closed) {
      _closeFuture = null;
      _disconnectSent = false;
    }
    return closed;
  });
  Future<bool> _close() async {
    if (_disposed) return false;
    _closed = true;
    phase = DesktopPanePhase.closing;
    dismissed.value = true;
    _ended = Completer<bool>();
    _closeTimer = Timer(const Duration(seconds: 10), () {
      _fail('关闭尚未确认，原会话资源继续保留');
      _finishClose(false);
    });
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
      _queryLifecycle();
      _disconnect();
    }
    final quieted = await quiet();
    final ended = await _ended!.future;
    if (!quieted || !ended) return false;
    try {
      final releases = await Future.wait([
        for (final root in _roots.values.toSet()) root.releaseCompleted.future,
      ]).timeout(const Duration(seconds: 10));
      return releases.every((released) => released);
    } on TimeoutException {
      _fail('原动作资源尚未最终释放');
      return false;
    }
  }

  void _disconnect() {
    if (_disconnectSent || lifecycle == null || lifecycle!.attempt == _u64(0)) {
      return;
    }
    _disconnectSent = true;
    event(const DesktopEventDisconnect(), requiresFocus: false);
  }

  void _finishClose(bool success) {
    _closeTimer?.cancel();
    if (success) phase = DesktopPanePhase.ended;
    if (_ended?.isCompleted == false) _ended!.complete(success);
    if (success) _disposeDetachedIfFinished();
  }

  void _fail(String detail) {
    if (_disposed) return;
    _focusRequested = false;
    error = detail;
    if (!_closed) phase = DesktopPanePhase.failed;
    notifyListeners();
  }

  /// widget 脱离后仍由关闭 Future/原 claim 持有。本方法不伪造释放。
  Future<void> detach() async {
    _detached = true;
    final success = await close();
    if (!success || _roots.values.any((root) => !root.released)) return;
    dispose();
  }

  void _disposeDetachedIfFinished() {
    if (_detached &&
        phase == DesktopPanePhase.ended &&
        !_attaching &&
        _roots.isEmpty) {
      // 生命周期通知先返回，避免在 notifier 重入栈中释放。
      scheduleMicrotask(() {
        if (!_disposed) dispose();
      });
    }
  }

  @override
  void dispose() {
    if (_disposed) return;
    _disposed = true;
    for (final subscription in _subscriptions) {
      subscription.cancel();
    }
    for (final waiting in _commands.values) {
      if (!waiting.isCompleted) waiting.complete(false);
    }
    _commands.clear();
    _commandOrigins.clear();
    _guiPending?.timer?.cancel();
    if (_guiPending?.completion.isCompleted == false) {
      _guiPending!.completion.complete(NativeCommandAdmission.unknown);
    }
    _dataRequests.clear();
    for (final timer in _commandTimers.values) {
      timer.cancel();
    }
    _commandTimers.clear();
    _closeTimer?.cancel();
    font.dispose();
    dismissed.dispose();
    super.dispose();
  }

  static Uint64 _u64(int value) => Uint64(BigInt.from(value));
  static String _binding(Uint64 attempt, Uint64 focus) => '$attempt:$focus';
}

class _PaneRoot {
  _PaneRoot(this.token, this.attempt, this.focusEpoch);
  final DesktopPaneToken token;
  final Uint64 attempt;
  Uint64 focusEpoch;
  int claims = 0;
  bool retired = false;
  bool quiet = false;
  bool released = false;
  Future<bool>? quietFuture;
  Future<bool>? releaseFuture;
  final releaseCompleted = Completer<bool>();
  // 宿主 context ID 随退休单调递增，仅保留当前 context 的事务水位。
  BigInt compositionContext = BigInt.zero;
  (BigInt, BigInt, bool)? compositionTransaction;
}

class _PendingFocus {
  _PendingFocus(this.root, this.intent, this.lifecycleRevision);
  final _PaneRoot root;
  final int intent;
  final Uint64 lifecycleRevision;
  final completion = Completer<bool>();
}

class _NativePresentation {
  _NativePresentation(
    this.frame,
    this.geometry,
    this.root,
    this.epoch,
    this.intent,
  );
  final DesktopPresentedFrame frame;
  final Map<String, Object?> geometry;
  final _PaneRoot root;
  final Uint64 epoch;
  final int intent;
}

class _GuiPending {
  _GuiPending(this.command, this.root, this.focusIntent, this.intent);
  final DesktopNativeCommand command;
  final _PaneRoot root;
  final int focusIntent;
  final GuiIntent intent;
  final completion = Completer<NativeCommandAdmission>();
  Timer? timer;
}
