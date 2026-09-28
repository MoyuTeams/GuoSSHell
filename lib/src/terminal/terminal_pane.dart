import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart' show PointerDeviceKind;
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart' show ScrollDirection;
import 'package:flutter/scheduler.dart';
import 'package:flutter/services.dart';
import 'package:rinf/rinf.dart';
import 'package:terminal_view/terminal_view.dart'
    show
        CellOffset,
        defaultTerminalShortcuts,
        MouseMode,
        PointerInputs,
        TerminalController,
        TerminalStyle,
        TerminalView,
        TerminalThemes,
        TerminalMouseButton;
// 字符度量是 fork 的内部工具，但选区菜单的锚点定位要用它
// （与行身份缓冲同一类实现级依赖）。
// ignore: implementation_imports
import 'package:terminal_view/src/ui/char_metrics.dart';
import 'package:url_launcher/url_launcher.dart';

import 'package:guosh_shell/src/bindings/bindings.dart';

import '../settings/terminal_font.dart';
import '../settings/interface_font.dart';
import 'terminal_clipboard_actions.dart';
import 'terminal_input_mode.dart';
import 'terminal_zoom.dart';
import 'frame.dart';
import 'frame_terminal.dart';
import 'perf_monitor.dart';
import 'prompt_dialogs.dart';
import 'session_target.dart';

/// 本进程内会话编号（rinf 信号按类型全局广播，靠它分流）。
int _nextSessionId = 1;

/// 复制 / 全选的快捷键（⌘C / ⌘A 等，按平台由 fork 的默认表给出）改走引擎：
/// 选区权威在引擎（PLAN §6.1 M2a 选区决定），fork 自带的这两个动作用的是
/// Dart 侧缓冲，拿不到滚出视口的内容，全选也不会告诉引擎。
class _EngineCopyIntent extends Intent {
  const _EngineCopyIntent();
}

class _EngineSelectAllIntent extends Intent {
  const _EngineSelectAllIntent();
}

final Map<ShortcutActivator, Intent> _terminalShortcuts = {
  for (final entry in defaultTerminalShortcuts.entries)
    entry.key: switch (entry.value) {
      CopySelectionTextIntent() => const _EngineCopyIntent(),
      SelectAllTextIntent() => const _EngineSelectAllIntent(),
      final other => other,
    },
};

/// 窗格对外的一面：工作区（标签条、键位条、快捷键）经它操作窗格里的会话。终端适配器、
/// 选区控制器与焦点归它所有，窗格在分屏、换标签时重新布局也不受影响。
class TerminalPaneController extends ChangeNotifier {
  TerminalPaneController(this.target);

  final SessionTarget target;
  final FrameTerminal terminal = FrameTerminal();
  final TerminalController selection = TerminalController(
    pointerInputs: const PointerInputs.all(),
  );

  /// 软键盘的开关靠它：焦点在终端上 = 键盘起，unfocus = 收起。
  final FocusNode focusNode = FocusNode();

  _TerminalPaneState? _pane;
  SessionState? _state;
  String _remoteTitle = '';

  /// 标签上的名字：远端设置的窗口标题，没有就用连接的名字。
  String get title => _remoteTitle.isNotEmpty ? _remoteTitle : target.title;

  SessionState? get state => _state;
  bool get connected => _state == SessionState.connected;

  /// 当前会话编号（信号按它分流；重连换新编号）。窗格还没布局时为 0。
  int get sessionId => _pane?._sessionId ?? 0;

  /// 最近画上的一帧的序号（画面一致性自检按它与 Rust 发出的帧对齐）。
  int get lastFrameSeq => _lastFrameSeq;
  int _lastFrameSeq = 0;
  bool get canCopy => selection.selection?.normalized.isCollapsed == false;

  /// 复制选区（取文在引擎里）。
  void copy() => _pane?._copySelection();

  TerminalZoom? get zoom => _pane?._zoom;

  /// 系统剪贴板 → 远端。
  Future<void> paste() async => _pane?._pasteClipboard();

  /// 软键盘开关：焦点在终端 = 键盘起，否则收起。
  void toggleKeyboard() {
    if (focusNode.hasFocus) {
      focusNode.unfocus();
    } else {
      focusNode.requestFocus();
    }
  }

  void _report({SessionState? state, String? title}) {
    final changed =
        (state != null && state != _state) ||
        (title != null && title != _remoteTitle);
    if (state != null) _state = state;
    if (title != null) _remoteTitle = title;
    if (changed) notifyListeners();
  }

  @override
  void dispose() {
    selection.dispose();
    focusNode.dispose();
    super.dispose();
  }
}

/// 一个窗格：一条 SSH 会话的终端（fork 渲染 + 帧驱动适配器）与连接提示。会话在窗格首次
/// 布局完成时发起（几何随连接请求一起带上），窗格从界面上移除即断开。
class TerminalPane extends StatefulWidget {
  final TerminalPaneController controller;

  /// 一个标签里有多个窗格时，活动窗格描一圈边。
  final bool highlighted;

  /// Windows 材质背景可透过画布；其他平台默认保持原来的不透明终端。
  final double backgroundOpacity;

  /// 在窗格里按下（点、拖、选）：它成为活动窗格。
  final VoidCallback onActivate;

  /// 关掉这个窗格：会话结束后点「关闭」，或连接前取消了。
  final VoidCallback onClose;

  /// 硬件键盘的按键先交给工作区（标签、分屏的快捷键）。
  final KeyEventResult Function(FocusNode node, KeyEvent event)? onKeyEvent;

  const TerminalPane({
    super.key,
    required this.controller,
    this.highlighted = false,
    this.backgroundOpacity = 1,
    required this.onActivate,
    required this.onClose,
    this.onKeyEvent,
  });

  @override
  State<TerminalPane> createState() => _TerminalPaneState();
}

class _TerminalPaneState extends State<TerminalPane> {
  TerminalPaneController get _controller => widget.controller;
  FrameTerminal get _terminal => widget.controller.terminal;
  TerminalController get _terminalController => widget.controller.selection;
  FocusNode get _terminalFocus => widget.controller.focusNode;

  /// 滚回的滚动位置（fork 的 Scrollable 用它）；滚动时按位置向 Rust 要窗口。
  final ScrollController _scroll = ScrollController();

  /// 跟着屏幕（随输出滚动）；滚进滚回后为 false，[_window] 是已请求的窗口。
  bool _followBottom = true;
  ({int top, int rows})? _window;

  /// 选区菜单锚点定位用（终端渲染区的屏幕坐标）。
  final GlobalKey _terminalSurfaceKey = GlobalKey();

  /// Flutter 自带的选区菜单（iOS 上是系统风格气垫）。
  final ContextMenuController _selectionMenu = ContextMenuController();
  final _inputMode = TerminalInputMode.shared;
  late final _pointerClipboard = TerminalClipboardActions(
    selection: () {
      final selected = _terminalController.selection?.normalized;
      return selected == null || selected.isCollapsed
          ? null
          : (selected.begin, selected.end);
    },
    requestCopy: () => CopyRequest(sessionId: _sessionId).sendSignalToRust(),
    writeClipboard: (text) => Clipboard.setData(ClipboardData(text: text)),
    clearSelection: () {
      if (!mounted) return;
      _selectionEcho = null;
      _terminalController.setExternalSelection(null, null);
      _sendSelectionRequest(clear: true);
    },
    paste: _pasteClipboard,
    onError: (error) => debugPrint('终端剪贴板操作失败：$error'),
  );

  /// 样式与选区菜单锚点共用缩放后的度量，实际尺寸照常传给远端 PTY。
  late final _settings = SettingsState.latestRustSignal!.message;
  TerminalStyle get _baseStyle => terminalStyle(_settings);
  late final TerminalZoom _zoom = TerminalZoom(
    defaultSize: _settings.fontSize,
    minSize: _settings.minFontSize,
    maxSize: _settings.maxFontSize,
  );
  TerminalStyle get _style => _baseStyle.copyWith(fontSize: _zoom.size);

  void _onZoomChanged() {
    _selectionMenu.remove();
    if (mounted) setState(() {});
  }

  void _onTerminalFontChanged() {
    _selectionMenu.remove();
    if (mounted) setState(() {});
  }

  StreamSubscription? _statusSub;
  StreamSubscription? _frameSub;
  StreamSubscription? _selectionSub;
  StreamSubscription? _clipboardSub;
  StreamSubscription? _perfSub;
  StreamSubscription? _promptSub;

  /// 当前会话编号；重连换新编号，旧会话迟到的信号不再生效。
  int _sessionId = 0;

  /// 会话已失败 / 结束：还开着的交互对话框据此自行关闭。
  final ValueNotifier<bool> _promptsDismissed = ValueNotifier(false);

  /// 引擎最近一次选区回显（绝对行坐标）。行号映射一变就要拿它重新投影。
  SelectionState? _selectionEcho;

  /// 上次投影时的行号起点（最早一行的绝对行）。变了才重新投影——
  /// 拖动中的乐观更新不会被迟到的回显顶掉。
  int? _projectedFirstStable;

  SessionState? _state;
  FailureKind _failure = FailureKind.none;
  String _detail = '';
  String _localNetworkSettingsUrl = '';
  ConnectHint _hint = ConnectHint.none;

  @override
  void initState() {
    super.initState();
    _statusSub = SessionStatus.rustSignalStream.listen(_onStatus);
    _frameSub = FrameUpdate.rustSignalStream.listen(_onFrame);
    _selectionSub = SelectionState.rustSignalStream.listen(_onSelectionState);
    _clipboardSub = ClipboardText.rustSignalStream.listen(_onClipboardText);
    _promptSub = InteractionPrompt.rustSignalStream.listen(_onPrompt);
    if (!kReleaseMode) {
      _perfSub = PerfStats.rustSignalStream.listen(_onPerfStats);
    }
    _terminal
      ..onInput = _onTerminalInput
      ..onResize = _onTerminalResize;
    _terminalController
      ..addListener(_onSelectionChanged)
      ..onSelectionIntent = _onSelectionIntent;
    _scroll.addListener(_onScroll);
    _controller._pane = this;
    _zoom.addListener(_onZoomChanged);
    _inputMode.addListener(_onInputModeChanged);
    InterfaceTypography.terminal.addListener(_onTerminalFontChanged);
    _startSession();
  }

  @override
  void dispose() {
    // 窗格从界面上移除（关窗格、关标签、离开工作区）即结束会话：连接中的一并取消，断开后
    // 还在等重连的也结束；只有连接前被取消的会话 Rust 那边已经收掉了。
    if (_sessionId != 0 && _state != SessionState.cancelled) {
      DisconnectRequest(sessionId: _sessionId).sendSignalToRust();
    }
    if (_controller._pane == this) _controller._pane = null;
    _statusSub?.cancel();
    _frameSub?.cancel();
    _selectionSub?.cancel();
    _clipboardSub?.cancel();
    _perfSub?.cancel();
    _promptSub?.cancel();
    _promptsDismissed.dispose();
    _pointerClipboard.dispose();
    _inputMode.removeListener(_onInputModeChanged);
    _terminalController
      ..removeListener(_onSelectionChanged)
      ..onSelectionIntent = null;
    _selectionMenu.remove();
    _resizeTimer?.cancel();
    _zoom.dispose();
    InterfaceTypography.terminal.removeListener(_onTerminalFontChanged);
    _scroll.dispose();
    super.dispose();
  }

  void _onStatus(RustSignalPack<SessionStatus> pack) {
    final msg = pack.message;
    if (!mounted || msg.sessionId != _sessionId) return;
    final state = msg.state;
    _controller._report(state: state);
    // 连接前被取消（关了密码框等）：关掉这个窗格。
    if (state == SessionState.cancelled) {
      _state = state;
      widget.onClose();
      return;
    }
    // 会话结束（失败/断开）：引擎那边选区没了，别再留着高亮/耳朵；
    // 还开着的交互对话框也收起。
    if (state == SessionState.closed || state == SessionState.failed) {
      _selectionEcho = null;
      _projectedFirstStable = null;
      _terminalController.setExternalSelection(null, null);
      _promptsDismissed.value = true;
    }
    setState(() {
      _state = state;
      _failure = msg.failure;
      _detail = msg.detail;
      _localNetworkSettingsUrl = msg.localNetworkSettingsUrl;
      _hint = msg.hint;
    });
    // 连接中视图几何可能又变了（键盘弹出、旋转）：连上即补发最新尺寸。
    if (state == SessionState.connected) _flushResize();
  }

  /// 连接过程中的问题（密码、主机密钥、keyboard-interactive）→ 对话框 → 回答。
  Future<void> _onPrompt(RustSignalPack<InteractionPrompt> pack) async {
    final prompt = pack.message;
    if (!mounted || prompt.sessionId != _sessionId) return;
    final answer = await showPromptDialog(context, prompt, _promptsDismissed);
    if (prompt.sessionId != _sessionId) return;
    InteractionReply(
      sessionId: prompt.sessionId,
      promptId: prompt.promptId,
      accept: answer.accept,
      answers: answer.answers,
      remember: answer.remember,
    ).sendSignalToRust();
  }

  void _onFrame(RustSignalPack<FrameUpdate> pack) {
    final msg = pack.message;
    if (!mounted || msg.sessionId != _sessionId) return;
    final watch = Stopwatch()..start();
    try {
      final frame = decodeFrame(
        pack.binary,
        cols: msg.cols,
        rows: msg.rows,
        cursorCol: msg.cursorCol,
        cursorRow: msg.cursorRow,
        firstStableRow: msg.firstStableRow,
        screenTopStableRow: msg.screenTopStableRow,
        title: msg.title,
      );
      // 显示模式随帧走（RenderFrame 已带，M2a 起过边界）：
      // 决定触摸点击/滚轮是转发远端还是保持本地行为。
      _terminal
        ..mouseReporting = msg.mouseReporting
        ..alternateScreen = msg.alternateScreen;
      _terminal.applyFrame(frame);
      _controller._lastFrameSeq = msg.seq;
      _controller._report(title: _terminal.title);
      _keepScrollPosition();
      // 最早一行变了（滚回裁掉旧行、清空、重排）行号就整体平移；重新投影选区，
      // 高亮和耳朵才会跟着内容走（拖动进行中映射不变，不会被顶掉）。
      if (_terminal.firstStableRow != _projectedFirstStable) {
        _projectedFirstStable = _terminal.firstStableRow;
        _projectSelection();
      }
      // 重排可能让选区端点失效：这时把还挂着的菜单收回，别留个孤儿气泡。
      if (_selectionMenu.isShown && _terminalController.selection == null) {
        _selectionMenu.remove();
      }
    } catch (error) {
      debugPrint('[frame] decode failed: $error');
    } finally {
      // 流控：Rust 等到这一帧的 ACK 才发下一帧（解码失败也要回，免得它空等）。
      FrameAck(sessionId: msg.sessionId, seq: msg.seq).sendSignalToRust();
      if (!kReleaseMode) {
        PerfMonitor.instance.recordApply(
          msg.sessionId,
          watch.elapsedMicroseconds,
        );
      }
    }
  }

  /// 性能汇总（debug 与 profile 构建，只打日志、不画浮层）：Rust 每 5 秒一条，
  /// PerfMonitor 补上 Dart 与 Flutter 这边的数据。
  void _onPerfStats(RustSignalPack<PerfStats> pack) {
    if (pack.message.sessionId != _sessionId) return;
    PerfMonitor.instance.report(pack.message);
  }

  /// 引擎回显选区 → 记住（绝对行坐标）→ 投影到当前视口。
  void _onSelectionState(RustSignalPack<SelectionState> pack) {
    if (!mounted || pack.message.sessionId != _sessionId) return;
    _selectionEcho = pack.message;
    _projectSelection();
  }

  /// 把引擎回显的选区（绝对行）换成行号喂给 fork（行号覆盖整个滚回，窗口外的部分
  /// 滚到时照样有高亮）。端点被裁出滚回时贴边，整体都被裁掉才清空。
  void _projectSelection() {
    final echo = _selectionEcho;
    if (echo == null || !echo.hasSelection) {
      _terminalController.setExternalSelection(null, null);
      return;
    }
    final first = _terminal.firstStableRow;
    final last = first + _terminal.height - 1;

    // 排序出首端/尾端（首端 = 早的那个），角色保留给 begin/end。
    final anchorIsFirst =
        echo.anchorRow < echo.focusRow ||
        (echo.anchorRow == echo.focusRow && echo.anchorCol <= echo.focusCol);
    final firstStableRow = anchorIsFirst ? echo.anchorRow : echo.focusRow;
    final lastStableRow = anchorIsFirst ? echo.focusRow : echo.anchorRow;
    final firstCol = anchorIsFirst ? echo.anchorCol : echo.focusCol;
    final lastCol = anchorIsFirst ? echo.focusCol : echo.anchorCol;

    if (firstStableRow > last || lastStableRow < first) {
      _terminalController.setExternalSelection(null, null);
      return;
    }
    final firstOffset = firstStableRow >= first
        ? CellOffset(firstCol, firstStableRow - first)
        : const CellOffset(0, 0); // 首端已被裁掉：贴到最早一行行首
    final lastOffset = lastStableRow <= last
        ? CellOffset(lastCol, lastStableRow - first)
        : CellOffset(0, _terminal.height); // 不会出现（尾端总在屏幕以内），兜底贴尾

    _terminalController.setExternalSelection(
      anchorIsFirst ? firstOffset : lastOffset,
      anchorIsFirst ? lastOffset : firstOffset,
    );
  }

  double get _lineHeight =>
      calcCharSize(_style, MediaQuery.textScalerOf(context)).height;

  /// 滚动位置 → 要显示的窗口。在底部就跟着屏幕；滚进滚回后要一段上下各多一屏的窗口，
  /// 已有的窗口还盖得住可见区（各留半屏余量）就不重发。只有用户的滚动才离开底部：
  /// 内容变矮后的回弹、尺寸变化的校正这些没人要的位移只会回到底部（回弹动画还瞄着旧的
  /// 底部时输出继续增长，按位置判断就会误以为用户滚上去了，从此不再跟着屏幕）。
  void _onScroll() {
    if (!mounted || !_scroll.hasClients || _state != SessionState.connected) {
      return;
    }
    final position = _scroll.position;
    final lineHeight = _lineHeight;
    if (lineHeight <= 0 || !position.hasContentDimensions) return;
    if (_followBottom && position.userScrollDirection == ScrollDirection.idle) {
      return;
    }
    if (position.pixels >= position.maxScrollExtent - lineHeight / 2) {
      if (!_followBottom) {
        _followBottom = true;
        _window = null;
        ViewportRequest(
          sessionId: _sessionId,
          followBottom: true,
          topStableRow: 0,
          rows: 0,
        ).sendSignalToRust();
      }
      return;
    }
    final visibleTop = (position.pixels / lineHeight).floor();
    final visibleRows = (position.viewportDimension / lineHeight).ceil() + 1;
    final slack = visibleRows ~/ 2;
    if (!_followBottom &&
        _window != null &&
        _terminal.windowCovers(visibleTop - slack, visibleRows + 2 * slack)) {
      return;
    }
    final topIndex = math.max(0, visibleTop - visibleRows);
    final window = (
      top: _terminal.firstStableRow + topIndex,
      rows: visibleRows * 3,
    );
    if (!_followBottom && window == _window) return;
    _followBottom = false;
    _window = window;
    ViewportRequest(
      sessionId: _sessionId,
      followBottom: false,
      topStableRow: window.top,
      rows: window.rows,
    ).sendSignalToRust();
  }

  /// 新帧之后校正滚动位置：停在滚回里时，最早一行后移了几行就把位置上移几行（内容
  /// 不在眼前漂走）；远端接管滚动（备用屏、鼠标上报）时回到底部。
  void _keepScrollPosition() {
    if (!_scroll.hasClients) return;
    final position = _scroll.position;
    if (_terminal.isUsingAltBuffer || _terminal.mouseMode != MouseMode.none) {
      if (!_followBottom) _scrollToBottom();
      return;
    }
    final shift = _terminal.originShift;
    if (shift != 0 && !_followBottom && position.hasContentDimensions) {
      position.correctBy(-shift * _lineHeight);
    }
  }

  /// 回到底部（跟着屏幕）。
  void _scrollToBottom() {
    if (_scroll.hasClients && _scroll.position.hasContentDimensions) {
      _scroll.jumpTo(_scroll.position.maxScrollExtent);
    }
    if (!_followBottom) {
      _followBottom = true;
      _window = null;
      ViewportRequest(
        sessionId: _sessionId,
        followBottom: true,
        topStableRow: 0,
        rows: 0,
      ).sendSignalToRust();
    }
  }

  /// 引擎取文回来 → 进剪贴板 + 清选区（对齐 Termux）。
  void _onClipboardText(RustSignalPack<ClipboardText> pack) {
    if (!mounted || pack.message.sessionId != _sessionId) return;
    unawaited(_pointerClipboard.receiveCopy(pack.message.text));
  }

  /// fork 上报的选区意图 → 换算成引擎绝对行 → 发 SelectionRequest。
  void _onSelectionIntent(CellOffset? begin, CellOffset? end) {
    if (_state != SessionState.connected) return;
    if (begin == null || end == null) {
      _sendSelectionRequest(clear: true);
      return;
    }
    final anchorRow = _terminal.stableRowAt(begin.y);
    final focusRow = _terminal.stableRowAt(end.y);
    if (anchorRow == null || focusRow == null) return;
    SelectionRequest(
      sessionId: _sessionId,
      clear: false,
      anchorRow: anchorRow,
      anchorCol: begin.x,
      focusRow: focusRow,
      focusCol: end.x,
      rectangular: false,
    ).sendSignalToRust();
  }

  void _sendSelectionRequest({required bool clear}) {
    SelectionRequest(
      sessionId: _sessionId,
      clear: clear,
      anchorRow: 0,
      anchorCol: 0,
      focusRow: 0,
      focusCol: 0,
      rectangular: false,
    ).sendSignalToRust();
  }

  /// fork 的输入口 → rinf → Rust（键编码权威在 encode_input）。
  /// 连接中的输入也照发：Rust 连上后按顺序补上（type-ahead）。
  bool _onTerminalInput(TerminalInputEvent event) {
    if (_state != SessionState.connected && _state != SessionState.connecting) {
      return false;
    }
    if (event is! MouseInputEvent) _scrollToBottom();
    switch (event) {
      case KeyInputEvent(:final key, :final shift, :final control, :final alt):
        InputRequest(
          sessionId: _sessionId,
          paste: false,
          text: '',
          key: key,
          shift: shift,
          control: control,
          alt: alt,
        ).sendSignalToRust();
      case TextInputEvent(:final text):
        InputRequest(
          sessionId: _sessionId,
          paste: false,
          text: text,
          key: '',
          shift: false,
          control: false,
          alt: false,
        ).sendSignalToRust();
      case PasteInputEvent(:final text):
        InputRequest(
          sessionId: _sessionId,
          paste: true,
          text: text,
          key: '',
          shift: false,
          control: false,
          alt: false,
        ).sendSignalToRust();
      case MouseInputEvent(
        :final button,
        :final action,
        :final position,
        :final shift,
        :final alt,
        :final ctrl,
      ):
        // 滚轮走 Scroll（上游 validate 拒绝「滚轮走 press」）。
        MouseRequest(
          sessionId: _sessionId,
          kind: button != null && button.isWheel
              ? 'scroll'
              : switch (action) {
                  MouseAction.press => 'press',
                  MouseAction.release => 'release',
                  MouseAction.move => 'move',
                },
          button: switch (button) {
            TerminalMouseButton.left => 'left',
            TerminalMouseButton.middle => 'middle',
            TerminalMouseButton.right => 'right',
            TerminalMouseButton.wheelUp => 'wheel_up',
            TerminalMouseButton.wheelDown => 'wheel_down',
            TerminalMouseButton.wheelLeft ||
            TerminalMouseButton.wheelRight ||
            null => '',
          },
          col: position.x,
          row: position.y,
          shift: shift,
          control: ctrl,
          alt: alt,
        ).sendSignalToRust();
    }
    return true;
  }

  /// fork 的 render 在布局期报几何（只在变化时）→ Rust（度量的权威在 Flutter）。
  /// 有待发的连接请求时带着真实几何发 ConnectRequest——否则远端 PTY 只能按
  /// 缺省 2×2 建立，exec 输出会在换行历史里塞满垃圾。
  void _onTerminalResize(TerminalGeometry geometry) {
    if (_connectPending) {
      _flushPendingConnect();
      return;
    }
    // 连接中的几何变化先不发：连上时由 _onStatus 补发（见 _flushResize）。
    if (_state != SessionState.connected || geometry == _sentGeometry) return;
    // 软键盘/旋转动画期间 fork 会**逐帧**报新几何；每一步都转发的话，
    // 远端 shell 会为每一步重画一次提示符（实机实测：一次键盘弹出刷了
    // 8 行提示符）。去抖后只发最终尺寸。
    _resizeTimer?.cancel();
    _resizeTimer = Timer(const Duration(milliseconds: 150), _flushResize);
  }

  /// 把最新几何发给会话（与已发的相同则不发）。
  void _flushResize() {
    _resizeTimer?.cancel();
    _resizeTimer = null;
    if (!mounted || _state != SessionState.connected) return;
    final geometry = _terminal.measuredGeometry;
    if (geometry == null || geometry == _sentGeometry) return;
    _sentGeometry = geometry;
    ResizeRequest(
      sessionId: _sessionId,
      cols: geometry.cols,
      rows: geometry.rows,
      pixelWidth: geometry.pixelWidth,
      pixelHeight: geometry.pixelHeight,
      dpi: _dpi,
    ).sendSignalToRust();
  }

  /// 有待发的连接请求且已量到几何 → 发出 ConnectRequest。
  /// 还没有几何（首次布局前）就等 fork 的 resize 回调再发。
  void _flushPendingConnect() {
    final geometry = _terminal.measuredGeometry;
    if (!_connectPending || geometry == null || !mounted) return;
    _connectPending = false;
    _sentGeometry = geometry;
    final target = _controller.target;
    ConnectRequest(
      sessionId: _sessionId,
      connectionId: target.connectionId,
      host: target.host,
      port: target.port,
      username: target.username,
      password: target.password,
      command: target.command,
      cols: geometry.cols,
      rows: geometry.rows,
      pixelWidth: geometry.pixelWidth,
      pixelHeight: geometry.pixelHeight,
      dpi: _dpi,
    ).sendSignalToRust();
  }

  int get _dpi => (96 * MediaQuery.devicePixelRatioOf(context)).round();

  /// 已随 ConnectRequest / ResizeRequest 发给当前会话的几何。
  TerminalGeometry? _sentGeometry;
  Timer? _resizeTimer;

  void _onInputModeChanged() {
    if (!_inputMode.touch) _selectionMenu.remove();
  }

  /// 只有触屏选区显示浮动菜单；鼠标选区仅高亮，由右键直接复制或粘贴。
  void _onSelectionChanged() {
    if (!mounted) return;
    if (!_inputMode.touch) {
      _selectionMenu.remove();
      return;
    }
    if (_terminalController.selection?.normalized.isCollapsed != false) {
      _selectionMenu.remove();
      return;
    }
    _selectionMenu.show(
      context: context,
      contextMenuBuilder: (context) => AdaptiveTextSelectionToolbar.buttonItems(
        anchors: _selectionAnchors(),
        buttonItems: [
          // 用官方的按钮类型，文案/样式由 Flutter 按平台给（不再硬编码中文）。
          ContextMenuButtonItem(
            type: ContextMenuButtonType.copy,
            onPressed: () {
              _selectionMenu.remove();
              _copySelection();
            },
          ),
          ContextMenuButtonItem(
            type: ContextMenuButtonType.paste,
            onPressed: () {
              _selectionMenu.remove();
              _pasteClipboard();
            },
          ),
        ],
      ),
    );
  }

  /// 选区端点 → 屏幕锚点。单元格尺寸用 fork 同一套字符度量算，
  /// 终端区原点取渲染盒的全局坐标，减去滚动位置；端点滚出视口时贴到可见的边上。
  TextSelectionToolbarAnchors _selectionAnchors() {
    final selection = _terminalController.selection!.normalized;
    final box =
        _terminalSurfaceKey.currentContext?.findRenderObject() as RenderBox?;
    if (box == null) {
      return const TextSelectionToolbarAnchors(primaryAnchor: Offset.zero);
    }
    final scroll = _scroll.hasClients ? _scroll.offset : 0.0;
    final origin = box.localToGlobal(Offset(0, -scroll));
    final cell = calcCharSize(_style, MediaQuery.textScalerOf(context));
    final begin = selection.begin;
    final end = selection.end;
    double clampY(double y) => y.clamp(scroll, scroll + box.size.height);
    return TextSelectionToolbarAnchors(
      primaryAnchor:
          origin + Offset(begin.x * cell.width, clampY(begin.y * cell.height)),
      secondaryAnchor:
          origin +
          Offset((end.x + 1) * cell.width, clampY((end.y + 1) * cell.height)),
    );
  }

  /// 复制选区：取文在引擎里（Dart 不碰 BufferLine），发请求等 ClipboardText。
  void _copySelection() {
    if (_state != SessionState.connected) return;
    _pointerClipboard.copy();
  }

  /// 全选：滚回与屏幕的全部内容（引擎的选区终点列是排除式的，所以终点取列数）。
  void _selectAll() {
    if (_state != SessionState.connected || _terminal.height == 0) return;
    final first = _terminal.stableRowAt(0);
    final last = _terminal.stableRowAt(_terminal.height - 1);
    if (first == null || last == null) return;
    SelectionRequest(
      sessionId: _sessionId,
      clear: false,
      anchorRow: first,
      anchorCol: 0,
      focusRow: last,
      focusCol: _terminal.viewWidth,
      rectangular: false,
    ).sendSignalToRust();
  }

  /// 系统剪贴板 → 远端（bracketed paste 等由 Rust 按远端模式处理）。
  Future<void> _pasteClipboard() async {
    final text = (await Clipboard.getData('text/plain'))?.text;
    if (text == null || text.isEmpty) return;
    _terminal.paste(text);
  }

  /// 断开之后再连：同一会话，画面与滚回保留，新的输出接在后面。
  void _reconnect() {
    _promptsDismissed.value = false;
    _controller._report(state: SessionState.connecting);
    setState(() {
      _state = SessionState.connecting;
      _failure = FailureKind.none;
      _detail = '';
      _localNetworkSettingsUrl = '';
      _hint = ConnectHint.none;
    });
    ReconnectRequest(sessionId: _sessionId).sendSignalToRust();
  }

  /// 发起会话：分配编号，等终端量好几何后发 ConnectRequest。
  void _startSession() {
    _pointerClipboard.cancelCopy();
    _sessionId = _nextSessionId++;
    _promptsDismissed.value = false;
    _selectionEcho = null;
    _projectedFirstStable = null;
    _followBottom = true;
    _window = null;
    _terminalController.setExternalSelection(null, null);
    setState(() {
      _state = SessionState.connecting;
      _failure = FailureKind.none;
      _detail = '';
      _localNetworkSettingsUrl = '';
      // 不立刻发请求：等会话视图完成布局、拿到真实几何再发。
      _connectPending = true;
    });
    // 布局后几何变了 → fork 回调 _onTerminalResize 已经发出；没变（重连时
    // 视图尺寸通常不变，fork 不再回调）→ 这里用已量到的几何发出。
    SchedulerBinding.instance.addPostFrameCallback(
      (_) => _flushPendingConnect(),
    );
  }

  bool _connectPending = false;

  Future<void> _openSettings(String url) async {
    await launchUrl(Uri.parse(url));
  }

  @override
  Widget build(BuildContext context) {
    final title = _controller.target.title;
    final banner = switch (_state) {
      SessionState.connecting => switch (_hint) {
        ConnectHint.touchCard => _Banner(
          icon: Icons.touch_app_outlined,
          text: '请按一下 OpenPGP 卡上的按键',
          detail: '正在连接 $title',
        ),
        ConnectHint.tapCard => _Banner(
          icon: Icons.contactless_outlined,
          text: '请把 OpenPGP 卡靠近设备',
          detail: '正在连接 $title',
        ),
        ConnectHint.securityKey => _Banner(
          icon: Icons.usb,
          text: '请按系统提示插上（或靠近）安全密钥，并触摸它',
          detail: '正在连接 $title',
        ),
        ConnectHint.none => _Banner(icon: Icons.sync, text: '正在连接 $title…'),
      },
      SessionState.failed => _Banner(
        icon: Icons.error_outline,
        text: _failureText(_failure),
        detail: _detail,
        error: true,
        actions: [
          if (_localNetworkSettingsUrl.isNotEmpty)
            TextButton(
              onPressed: () => _openSettings(_localNetworkSettingsUrl),
              child: const Text('打开设置'),
            ),
          TextButton(onPressed: _reconnect, child: const Text('重试')),
          TextButton(onPressed: widget.onClose, child: const Text('关闭')),
        ],
        hint: _localNetworkSettingsUrl.isEmpty
            ? null
            : '服务器在局域网内时，需要允许 GuoSSHell 访问本地网络。',
      ),
      SessionState.closed => _Banner(
        icon: Icons.link_off,
        text: '会话已结束',
        detail: _detail,
        actions: [
          TextButton(onPressed: _reconnect, child: const Text('重新连接')),
          TextButton(onPressed: widget.onClose, child: const Text('关闭')),
        ],
      ),
      _ => null,
    };

    // 按下即成为活动窗格（不参与手势竞争，选区、滚动照常）。
    return Listener(
      onPointerDown: (_) => widget.onActivate(),
      child: DecoratedBox(
        position: DecorationPosition.foreground,
        decoration: BoxDecoration(
          border: widget.highlighted
              ? Border.all(
                  color: Theme.of(context).colorScheme.primary,
                  width: 1.5,
                )
              : null,
        ),
        child: ColoredBox(
          color: widget.backgroundOpacity == 1
              ? Colors.black
              : Colors.transparent,
          child: Stack(
            children: [
              Positioned.fill(
                child: Actions(
                  actions: {
                    _EngineCopyIntent: CallbackAction<_EngineCopyIntent>(
                      onInvoke: (_) => _copySelection(),
                    ),
                    _EngineSelectAllIntent:
                        CallbackAction<_EngineSelectAllIntent>(
                          onInvoke: (_) => _selectAll(),
                        ),
                  },
                  child: TerminalZoomSurface(
                    zoom: _zoom,
                    onStart: widget.onActivate,
                    child: TerminalViewport(
                      key: _terminalSurfaceKey,
                      terminal: _terminal,
                      controller: _terminalController,
                      scrollController: _scroll,
                      focusNode: _terminalFocus,
                      style: _style,
                      onKeyEvent: widget.onKeyEvent,
                      inputMode: _inputMode,
                      onSecondaryTap: () {
                        if (_state != SessionState.connected) return;
                        _terminalFocus.requestFocus();
                        unawaited(_pointerClipboard.rightClick());
                      },
                      backgroundOpacity: widget.backgroundOpacity,
                    ),
                  ),
                ),
              ),
              // 不在终端上方悬浮控件：拖选区（尤其长选区）经过时会干扰触摸。
              if (banner != null)
                Positioned(top: 0, left: 0, right: 0, child: banner),
            ],
          ),
        ),
      ),
    );
  }
}

/// fork 的 TerminalView 需要有界高度（内部是 Scrollable）。
/// 会话期常驻（连接前也要完成首次布局，几何才能随 ConnectRequest 发出）。
class TerminalViewport extends StatelessWidget {
  final FrameTerminal terminal;
  final TerminalController controller;
  final ScrollController scrollController;
  final FocusNode focusNode;
  final TerminalStyle style;
  final double backgroundOpacity;
  final TerminalInputMode inputMode;
  final VoidCallback? onSecondaryTap;
  final KeyEventResult Function(FocusNode node, KeyEvent event)? onKeyEvent;

  const TerminalViewport({
    super.key,
    required this.terminal,
    required this.controller,
    required this.scrollController,
    required this.focusNode,
    required this.style,
    required this.backgroundOpacity,
    required this.inputMode,
    this.onSecondaryTap,
    this.onKeyEvent,
  });

  @override
  Widget build(BuildContext context) {
    // 滚动条跟着 fork 的 Scrollable（滚回）；远端接管滚动时 fork 不滚，也就不显示。
    return ListenableBuilder(
      listenable: inputMode,
      builder: (context, _) => Listener(
        onPointerDown: (event) => inputMode.pointer(event.kind),
        onPointerHover: (event) {
          if (!event.synthesized && event.kind == PointerDeviceKind.mouse) {
            inputMode.pointer(event.kind);
          }
        },
        onPointerSignal: (event) => inputMode.pointer(event.kind),
        onPointerPanZoomStart: (event) => inputMode.pointer(event.kind),
        child: Scrollbar(controller: scrollController, child: _terminalView()),
      ),
    );
  }

  Widget _terminalView() {
    return TerminalView(
      terminal,
      controller: controller,
      scrollController: scrollController,
      focusNode: focusNode,
      onKeyEvent: (node, event) {
        if (!event.synthesized &&
            (event is KeyDownEvent || event is KeyRepeatEvent)) {
          inputMode.keyboard();
        }
        return onKeyEvent?.call(node, event) ?? KeyEventResult.ignored;
      },
      autoResize: true,
      shortcuts: _terminalShortcuts,
      // iOS 软键盘的退格不产生硬件按键事件，必须靠编辑增量探测
      // （fork 的 onDelete → keyInput(backspace)）。
      deleteDetection: true,
      textStyle: style,
      theme: TerminalThemes.defaultTheme,
      backgroundOpacity: backgroundOpacity,
      showSelectionHandles: inputMode.touch,
      // 渲染库仅在事件没有交给远端鼠标模式时调用此回调；Shift 可保留本地操作。
      onSecondaryTapUp: onSecondaryTap == null
          ? null
          : (_, _) => onSecondaryTap!(),
      keyboardType: TextInputType.emailAddress,
      keyboardAppearance: Brightness.dark,
    );
  }
}

/// 失败分类 → 文案。
String _failureText(FailureKind failure) => switch (failure) {
  FailureKind.none || FailureKind.other => '连接出错',
  FailureKind.notFound => '这条连接已不存在',
  FailureKind.invalidTarget => '连接目标无效：请检查主机、端口和用户名',
  FailureKind.keyNotFound => '连接用的私钥不在钥匙串里了，请重新选择',
  FailureKind.authentication => '认证失败：服务器不接受这个用户名的密码或密钥',
  FailureKind.hostKeyRejected => '已拒绝服务器的主机密钥',
  FailureKind.hostKeyChanged => '主机密钥已变更，连接已中止',
  FailureKind.network => '无法连接到服务器',
  FailureKind.timeout => '连接超时',
  FailureKind.connectionLost => '连接已断开：网络中断，或服务器不再响应',
  FailureKind.keychain => '读写钥匙串失败',
  FailureKind.cardNotFound => '没有找到 OpenPGP 卡：请插上（或靠近）登记的那张卡后重试',
  FailureKind.cardKeyMismatch => '卡上的密钥与登记时的不同，请检查是不是插错了卡',
  FailureKind.cardUnsupported =>
    '这张卡的认证密钥 SSH 用不了（支持 Ed25519、RSA 与 NIST P-256/384/521）',
  FailureKind.cardPinBlocked => 'OpenPGP 卡的 PIN 已锁定，需要用管理 PIN 解锁',
  FailureKind.cardTouchTimeout => '没有等到卡上的按键确认',
  FailureKind.cardError => '读卡失败',
  FailureKind.securityKeyFailed => '安全密钥没有完成签名：请确认用的是登记时的那把安全密钥',
};

class _Banner extends StatelessWidget {
  final IconData icon;
  final String text;
  final String detail;
  final String? hint;
  final bool error;
  final List<Widget> actions;

  const _Banner({
    required this.icon,
    required this.text,
    this.detail = '',
    this.hint,
    this.error = false,
    this.actions = const [],
  });

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final foreground = error ? scheme.onErrorContainer : scheme.onSurface;
    return Material(
      color: error
          ? scheme.errorContainer
          : scheme.surface.withValues(alpha: 0.92),
      child: SafeArea(
        bottom: false,
        child: Padding(
          padding: const EdgeInsets.fromLTRB(16, 8, 8, 4),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            mainAxisSize: MainAxisSize.min,
            children: [
              Row(
                children: [
                  Icon(icon, color: foreground, size: 20),
                  const SizedBox(width: 12),
                  Expanded(
                    child: Text(text, style: TextStyle(color: foreground)),
                  ),
                ],
              ),
              if (hint != null)
                Padding(
                  padding: const EdgeInsets.only(left: 32, top: 4),
                  child: Text(
                    hint!,
                    style: TextStyle(color: foreground, fontSize: 13),
                  ),
                ),
              if (detail.isNotEmpty)
                Padding(
                  padding: const EdgeInsets.only(left: 32, top: 2),
                  child: Text(
                    detail,
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: foreground.withValues(alpha: 0.7),
                      fontSize: 11,
                    ),
                  ),
                ),
              if (actions.isNotEmpty)
                TextButtonTheme(
                  data: TextButtonThemeData(
                    style: TextButton.styleFrom(foregroundColor: foreground),
                  ),
                  child: Row(
                    mainAxisAlignment: MainAxisAlignment.end,
                    children: actions,
                  ),
                ),
            ],
          ),
        ),
      ),
    );
  }
}
