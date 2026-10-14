import 'dart:async';

import 'package:fluent_ui/fluent_ui.dart' as f;
import 'package:flutter/foundation.dart' show defaultTargetPlatform;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:multi_split_view/multi_split_view.dart';

import '../bindings/bindings.dart';
import '../catalog/catalog_requests.dart';
import '../platform_target.dart';
import '../terminal/key_bar_layout.dart';
import '../terminal/session_target.dart';
import '../terminal/terminal_key_bar.dart';
import '../terminal/desktop_terminal_key_bar.dart';
import '../terminal/terminal_pane.dart';
import '../workspace/workspace.dart';
import 'catalog/desktop_catalog_gateway.dart';
import 'catalog/desktop_connection_editor.dart';
import 'desktop_shortcuts.dart';
import 'desktop_theme.dart';
import 'desktop_widgets.dart';
import 'keys/desktop_key_actions.dart';
import 'keys/desktop_key_sync_state.dart';
import 'keys/desktop_keys_page.dart';
import 'settings/desktop_settings_page.dart';
import 'windows_chrome.dart';
import 'windows_sidebar.dart';
import 'workspace/desktop_target_options.dart';
import 'workspace/desktop_workspace_scope.dart';

/// 三平台共用常驻工作区。保留 WindowsShell 身份供既有原生验收定位。
/// Windows 宿主仍由 WindowsFrame 管理；目录、设置和编辑器不销毁会话。
class WindowsShell extends StatefulWidget {
  const WindowsShell({
    super.key,
    this.initial,
    this.queryCatalog,
    this.gateway,
    this.keyActions,
    this.paneBuilder,
    this.targetOptions,
  });
  final SessionTarget? initial;
  final ValueChanged<String>? queryCatalog;
  final DesktopCatalogGateway? gateway;
  final DesktopKeyActions? keyActions;
  final DesktopTargetOptionsGateway? targetOptions;

  /// 生命周期测试可替换叶子渲染，工作区和会话树仍由本壳创建和持有。
  /// 生产入口不传入此项，继续使用 TerminalPane，不据替身声明真实 SSH 已连接。
  @visibleForTesting
  final Widget Function(WorkspacePane)? paneBuilder;
  @override
  State<WindowsShell> createState() => _WindowsShellState();
}

class _WindowsShellState extends State<WindowsShell> {
  final _workspace = Workspace();
  final _search = TextEditingController();
  final _searchFocus = FocusNode();
  final _shellFocus = FocusNode();
  final _tabs = <WorkspaceTab, f.Tab>{};
  final _subscriptions = <StreamSubscription>[];
  late final DesktopCatalogGateway _gateway;
  late final DesktopKeyActions _keyActions;
  late final DesktopKeySyncState _keySyncState;
  List<ConnectionSummary>? _connections;
  SettingsState? _settings = SettingsState.latestRustSignal?.message;
  String? _selectedId;
  String? _error;
  bool _sidebar = true;
  double _sidebarWidth = 256;
  bool _dialogOpen = false;
  bool _terminalFocusAllowed = true;
  int _focusIntent = 0;
  final _orderedInput = <TerminalPaneController>{};
  late final DesktopTargetOptionsGateway _targetOptions;

  /// 能力查询报告本地 shell 不可用后入口禁用，并以此原因提示。
  String? _localUnavailable;
  bool _localQuerying = false;

  @override
  void initState() {
    super.initState();
    _gateway = widget.gateway ?? const RustDesktopCatalogGateway();
    _targetOptions =
        widget.targetOptions ?? const RustDesktopTargetOptionsGateway();
    _keyActions =
        widget.keyActions ?? DesktopKeyActions(query: _gateway.queryKeys);
    _keySyncState = DesktopKeySyncState(
      states: _gateway.keyStates,
      initial: widget.gateway == null
          ? KeyListState.latestRustSignal?.message
          : null,
    );
    _workspace.addListener(_changed);
    if (widget.initial case final initial?) _workspace.openTab(initial);
    _subscriptions.add(
      _gateway.catalogStates.listen((state) {
        if (!mounted || state.query != _search.text) return;
        setState(() {
          _connections = state.connections;
          if (!_connections!.any((item) => item.id == _selectedId)) {
            _selectedId = null;
          }
        });
      }),
    );
    _subscriptions.add(
      SettingsState.rustSignalStream.listen((pack) {
        if (mounted) setState(() => _settings = pack.message);
      }),
    );
    _search.addListener(_query);
    _query();
    if (isWindowsDesktop) WindowsDesktopWindow.confirmClose = _confirmClose;
  }

  void _query() => widget.queryCatalog != null
      ? widget.queryCatalog!(_search.text)
      : _gateway.query(_search.text);

  void _changed() {
    if (!mounted) return;
    setState(
      () => _tabs.removeWhere((tab, _) => !_workspace.tabs.contains(tab)),
    );
    _orderedInput.removeWhere(
      (controller) => !_workspace.panes.any(
        (pane) => identical(pane.controller, controller),
      ),
    );
    for (final pane in _workspace.panes) {
      if (pane != _workspace.activePane && pane.controller.ownsNativeInput) {
        unawaited(pane.controller.quietNativeInput());
      }
    }
  }

  @override
  void dispose() {
    if (isWindowsDesktop) WindowsDesktopWindow.confirmClose = null;
    for (final subscription in _subscriptions) {
      subscription.cancel();
    }
    _workspace.removeListener(_changed);
    _workspace.dispose();
    _keySyncState.dispose();
    _search.dispose();
    _searchFocus.dispose();
    _shellFocus.dispose();
    super.dispose();
  }

  Future<bool> _focusActive({TerminalPaneController? origin}) async {
    final intent = ++_focusIntent;
    _terminalFocusAllowed = false;
    final expected = origin ?? _workspace.activePane?.controller;
    for (final pane in _workspace.panes.toList()) {
      if (pane.controller != expected &&
          !await pane.controller.quietNativeInput()) {
        return false;
      }
    }
    if (!mounted || intent != _focusIntent || _dialogOpen) return false;
    _terminalFocusAllowed = true;
    final focused = Completer<bool>();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted ||
          _dialogOpen ||
          intent != _focusIntent ||
          !(ModalRoute.of(context)?.isCurrent ?? true)) {
        focused.complete(false);
        return;
      }
      if (expected != null) {
        if (_originAttachedAndActive(expected)) {
          focused.complete(expected.requestTerminalFocus());
        } else {
          focused.complete(false);
        }
      } else if (_workspace.panes.isEmpty) {
        _shellFocus.requestFocus();
        focused.complete(true);
      } else {
        focused.complete(false);
      }
    });
    WidgetsBinding.instance.scheduleFrame();
    return focused.future;
  }

  Future<bool> _quietAll() async {
    ++_focusIntent;
    _terminalFocusAllowed = false;
    final results = await Future.wait([
      for (final pane in _workspace.panes) pane.controller.quietNativeInput(),
    ]);
    if (results.any((quiet) => !quiet)) {
      if (mounted) setState(() => _error = '终端输入尚未静默，操作未完成。');
      return false;
    }
    return mounted;
  }

  /// 本地终端只在桌面目标提供；移动端不显示任何本地入口。
  bool get _localEntry =>
      appTargetFor(defaultTargetPlatform) == AppTarget.desktop;

  /// 入口按钮的回调：不可用时为 null（禁用）。
  VoidCallback? get _localPressed =>
      _localUnavailable == null ? () => unawaited(_openLocal()) : null;

  String get _localLabel => _localUnavailable == null
      ? '本地终端 · Ctrl+Shift+L'
      : '本地终端不可用：$_localUnavailable';

  // 必须是壳本身的主焦点，不能把搜索、目录行或按钮的子焦点当欢迎命令权限。
  bool _isEmptyShellOrigin() =>
      mounted &&
      _workspace.panes.isEmpty &&
      _shellFocus.hasPrimaryFocus &&
      (ModalRoute.of(context)?.isCurrent ?? true);

  void _open(ConnectionSummary item) {
    _workspace.openTab(desktopSavedTarget(item));
    _focusActive();
  }

  Future<DesktopTargetOptions?> _queryTargetOptions() async {
    try {
      final options = await _targetOptions.query();
      if (!mounted) return null;
      if (!options.nativeAvailable) {
        setState(() => _error = options.nativeReason);
        return null;
      }
      return options;
    } catch (_) {
      if (mounted) setState(() => _error = '无法读取 profile 或传输能力，请重试。');
      return null;
    }
  }

  /// 本地终端用应用默认 profile；先查能力，不可用时禁用入口并提示原因。
  /// 返回是否打开了标签。
  Future<bool> _openLocal() async {
    if (_dialogOpen || _localQuerying || _localUnavailable != null) {
      return false;
    }
    _localQuerying = true;
    try {
      final options = await _targetOptions.query();
      if (!mounted) return false;
      if (!options.localAvailable) {
        setState(() => _localUnavailable = _error = options.localReason);
        return false;
      }
    } catch (_) {
      if (mounted) setState(() => _error = '无法读取本地终端能力，请重试。');
      return false;
    } finally {
      _localQuerying = false;
    }
    _workspace.openTab(const SessionTarget.local());
    _focusActive();
    return true;
  }

  Future<void> _openWithProfile(ConnectionSummary item) async {
    if (_dialogOpen) return;
    _dialogOpen = true;
    try {
      if (!await _quietAll()) return;
      final options = await _queryTargetOptions();
      if (!mounted || options == null) return;
      var profile = '';
      final selected = await f.showDialog<String>(
        context: context,
        builder: (dialogContext) => StatefulBuilder(
          builder: (_, update) => f.ContentDialog(
            title: Text('连接「${connectionTitle(item)}」'),
            content: SingleChildScrollView(
              child: DesktopProfilePicker(
                options: options,
                value: profile,
                saved: true,
                systemSsh: item.transport == DesktopTransport.systemOpenSsh,
                onChanged: (id) => update(() => profile = id),
              ),
            ),
            actions: [
              f.Button(
                onPressed: () => Navigator.of(dialogContext).pop(),
                child: const Text('取消'),
              ),
              f.FilledButton(
                onPressed: () => Navigator.of(dialogContext).pop(profile),
                child: const Text('连接'),
              ),
            ],
          ),
        ),
      );
      if (!mounted || selected == null) return;
      _workspace.openTab(desktopSavedTarget(item, profileId: selected));
    } finally {
      _dialogOpen = false;
      _focusActive();
    }
  }

  Future<DesktopActionOutcome> _editor({
    ConnectionSummary? existing,
    bool quick = false,
    TerminalPaneController? origin,
    bool emptyShellOrigin = false,
  }) async {
    if (_dialogOpen) return DesktopActionOutcome.unavailable;
    _dialogOpen = true;
    SessionTarget? target;
    var profile = '';
    try {
      final options = quick ? await _queryTargetOptions() : null;
      if (!mounted || (quick && options == null)) {
        return DesktopActionOutcome.unavailable;
      }
      if (origin != null &&
          (!_originAttachedAndActive(origin) ||
              !(origin.ownsNativeInput
                  ? origin.nativeActive
                  : origin.focusNode.hasFocus) ||
              !(ModalRoute.of(context)?.isCurrent ?? true))) {
        return DesktopActionOutcome.staleOrigin;
      }
      if (emptyShellOrigin && !_isEmptyShellOrigin()) {
        return DesktopActionOutcome.staleOrigin;
      }
      if (!await _quietAll() || !mounted) {
        return DesktopActionOutcome.unavailable;
      }
      target = await f.showDialog<SessionTarget>(
        context: context,
        builder: (dialogContext) => _managementPanel(
          dialogContext,
          Column(
            children: [
              if (options != null)
                Padding(
                  padding: const EdgeInsets.all(DesktopSpacing.lg),
                  child: StatefulBuilder(
                    builder: (_, update) => DesktopProfilePicker(
                      options: options,
                      value: profile,
                      saved: false,
                      onChanged: (id) => update(() => profile = id),
                    ),
                  ),
                ),
              Expanded(
                child: DesktopConnectionEditor(
                  gateway: _gateway,
                  existing: existing,
                  quick: quick,
                  keyActions: _keyActions,
                  keySyncState: _keySyncState,
                  targetOptions: options,
                ),
              ),
            ],
          ),
        ),
      );
    } finally {
      _dialogOpen = false;
    }
    if (!mounted) return DesktopActionOutcome.unavailable;
    if (origin != null && !_originAttachedAndActive(origin)) {
      return DesktopActionOutcome.staleOrigin;
    }
    if (emptyShellOrigin && _workspace.panes.isNotEmpty) {
      return DesktopActionOutcome.staleOrigin;
    }
    if (target != null) {
      _workspace.openTab(desktopTargetWithProfile(target, profile));
    }
    _query();
    _focusActive();
    if (origin?.ownsNativeInput == true) {
      await WidgetsBinding.instance.endOfFrame;
    }
    return target != null
        ? DesktopActionOutcome.executed
        : DesktopActionOutcome.unavailable;
  }

  Future<DesktopActionOutcome> _settingsDialog({
    TerminalPaneController? origin,
    bool emptyShellOrigin = false,
    DesktopSettingsSection section = DesktopSettingsSection.app,
  }) async {
    if (origin != null && !_isCurrentOrigin(origin)) {
      return DesktopActionOutcome.staleOrigin;
    }
    if (_dialogOpen) return DesktopActionOutcome.unavailable;
    _dialogOpen = true;
    try {
      if (!await _quietAll() || !mounted) {
        return DesktopActionOutcome.unavailable;
      }
      await showDesktopSettings(context, section: section, settings: _settings);
    } finally {
      _dialogOpen = false;
    }
    if (emptyShellOrigin && _workspace.panes.isNotEmpty) {
      return DesktopActionOutcome.staleOrigin;
    }
    final focused = _focusActive(origin: origin);
    if (origin?.ownsNativeInput == true) await focused;
    return DesktopActionOutcome.executed;
  }

  // 键位条按钮本身持焦，不能当终端来源；关闭后回到活动窗格。
  void _editKeyBar() =>
      unawaited(_settingsDialog(section: DesktopSettingsSection.keybar));

  Future<void> _keys() async {
    if (_dialogOpen) return;
    _dialogOpen = true;
    try {
      if (!await _quietAll() || !mounted) return;
      await f.showDialog<void>(
        context: context,
        builder: (dialogContext) => _managementPanel(
          dialogContext,
          DesktopKeysPage(actions: _keyActions, syncState: _keySyncState),
        ),
      );
    } finally {
      _dialogOpen = false;
    }
    _focusActive();
  }

  /// 只在现有 Navigator 上覆盖 Fluent 面板，不建立第二工作区或迁移会话树。
  Widget _managementPanel(BuildContext dialogContext, Widget page) => Padding(
    padding: const EdgeInsets.all(DesktopSpacing.xl),
    child: Center(
      child: ConstrainedBox(
        constraints: const BoxConstraints(
          maxWidth: DesktopMetrics.contentWidth,
        ),
        child: ClipRRect(
          borderRadius: BorderRadius.circular(DesktopMetrics.radius),
          child: ColoredBox(
            color: f.FluentTheme.of(dialogContext).scaffoldBackgroundColor,
            child: Column(
              children: [
                Padding(
                  padding: const EdgeInsets.all(DesktopSpacing.sm),
                  child: Align(
                    alignment: Alignment.centerRight,
                    child: f.Button(
                      onPressed: () => Navigator.of(dialogContext).pop(),
                      child: const Text('返回工作区'),
                    ),
                  ),
                ),
                Expanded(child: page),
              ],
            ),
          ),
        ),
      ),
    ),
  );

  Future<bool> _confirmClose() async {
    if (_dialogOpen) return false;
    final count = _workspace.panes
        .where(
          (pane) =>
              pane.controller.state != SessionState.closed &&
              pane.controller.state != SessionState.failed,
        )
        .length;
    if (count == 0) {
      if (!await _quietAll() || !mounted) return false;
      return _closeAllSessions();
    }
    _dialogOpen = true;
    try {
      if (!await _quietAll() || !mounted) return false;
      final confirmed = await desktopConfirm(
        context,
        '关闭 GuoSSHell？',
        '这将断开工作区中的 $count 个会话。',
        action: '断开并关闭',
      );
      return confirmed && await _closeAllSessions();
    } finally {
      _dialogOpen = false;
      _focusActive();
    }
  }

  Future<bool> _closeAllSessions() async {
    final results = await Future.wait([
      for (final pane in _workspace.panes) pane.controller.closeSession(),
    ]);
    return results.every((closed) => closed);
  }

  Future<void> _closeTab(WorkspaceTab tab) async {
    if (_dialogOpen) return;
    if (!await _quietAll() || !mounted) return;
    if (tab.panes.any(
      (pane) =>
          pane.controller.state != SessionState.closed &&
          pane.controller.state != SessionState.failed,
    )) {
      _dialogOpen = true;
      bool confirmed;
      try {
        confirmed = await desktopConfirm(
          context,
          '关闭标签？',
          '这将断开「${tab.active.controller.title}」中的会话。',
          action: '断开',
        );
      } finally {
        _dialogOpen = false;
      }
      if (!confirmed) {
        _focusActive();
        return;
      }
    }
    if (!mounted || !_workspace.tabs.contains(tab)) return;
    final closed = await Future.wait([
      for (final pane in tab.panes) pane.controller.closeSession(),
    ]);
    if (closed.any((value) => !value) || !mounted) return;
    _workspace.closeTab(tab);
    _focusActive();
  }

  Future<bool> _closePane([
    WorkspacePane? requested,
    TerminalPaneController? origin,
  ]) async {
    if (_dialogOpen) return false;
    final pane = requested ?? _workspace.activePane;
    if (pane == null) return false;
    if (!await _quietAll() || !mounted) return false;
    if (pane.controller.state != SessionState.closed &&
        pane.controller.state != SessionState.failed) {
      _dialogOpen = true;
      bool confirmed;
      try {
        confirmed = await desktopConfirm(
          context,
          '关闭窗格？',
          '断开「${pane.controller.title}」的会话。',
          action: '断开',
        );
      } finally {
        _dialogOpen = false;
      }
      if (!confirmed) {
        if (origin == null || _originAttachedAndActive(origin)) {
          _focusActive(origin: origin);
        }
        return false;
      }
    }
    if (mounted &&
        _workspace.panes.contains(pane) &&
        (origin == null || _originAttachedAndActive(origin))) {
      // claim 的 root 独立于被移除 widget；原 ClosePane 不等自己的最终 ACK。
      if (origin == null && !await pane.controller.closeSession()) return false;
      if (!mounted || !_workspace.panes.contains(pane)) return false;
      _workspace.close(pane);
      _focusActive();
      return true;
    }
    return false;
  }

  void _split(Axis axis) {
    final pane = _workspace.activePane;
    if (pane == null) return;
    _workspace.split(pane, axis, pane.controller.target);
    _focusActive();
  }

  Future<void> _catalogAction(ConnectionSummary item, String action) async {
    if (action == 'profile') return _openWithProfile(item);
    if (action == 'edit') {
      await _editor(existing: item);
      return;
    }
    if (action == 'open') {
      _open(item);
      return;
    }
    if (action == 'delete' &&
        !await desktopConfirm(
          context,
          '删除连接？',
          '删除「${connectionTitle(item)}」及其保存的密码。',
          action: '删除',
        )) {
      return;
    }
    try {
      final result = await (action == 'duplicate'
          ? _gateway.duplicate(item.id)
          : _gateway.delete(item.id));
      if (mounted) {
        setState(
          () => _error = result.error == CatalogError.none
              ? null
              : catalogErrorText(result.error),
        );
      }
    } on TimeoutException {
      if (mounted) setState(() => _error = '操作超时，请重试');
    }
    if (mounted) _query();
  }

  KeyEventResult _key(FocusNode node, KeyEvent event) {
    if (_dialogOpen || !(ModalRoute.of(context)?.isCurrent ?? true)) {
      return KeyEventResult.ignored;
    }
    final origin = _workspace.activePane?.controller;
    // 终端 V2 由 4C 同步门控；本壳不发送输入或替 Rust 作绑定判断。
    if (origin != null && _orderedInput.contains(origin)) {
      return KeyEventResult.ignored;
    }
    // 非空工作区仍须真实终端来源；空工作区只开放壳自身持焦的欢迎命令。
    final emptyShellOrigin = origin == null && _isEmptyShellOrigin();
    if (!emptyShellOrigin && (origin == null || !_isCurrentOrigin(origin))) {
      return KeyEventResult.ignored;
    }
    final keyboard = HardwareKeyboard.instance;
    final key = event.logicalKey;
    final command = desktopCommand(
      key,
      control: keyboard.isControlPressed,
      shift: keyboard.isShiftPressed,
      alt: keyboard.isAltPressed,
      meta:
          defaultTargetPlatform == TargetPlatform.macOS &&
          keyboard.isMetaPressed,
    );
    if (command == null) {
      if (_workspace.activePane?.controller.focusNode.hasFocus ?? false) {
        if (_workspace.activePane?.controller.zoom?.handleKey(
              event,
              meta: keyboard.isControlPressed && !keyboard.isAltPressed,
            ) ??
            false) {
          return KeyEventResult.handled;
        }
      }
      return KeyEventResult.ignored;
    }
    if (emptyShellOrigin &&
        command != DesktopCommand.newTab &&
        command != DesktopCommand.search &&
        command != DesktopCommand.settings &&
        command != DesktopCommand.toggleSidebar &&
        command != DesktopCommand.localTerminal) {
      return KeyEventResult.ignored;
    }
    if (command == DesktopCommand.localTerminal && !_localEntry) {
      return KeyEventResult.ignored;
    }
    if (event is! KeyDownEvent) return KeyEventResult.handled;
    switch (command) {
      case DesktopCommand.newTab:
        _editor(
          quick: true,
          origin: origin,
          emptyShellOrigin: emptyShellOrigin,
        );
      case DesktopCommand.closePane:
        _closePane(_workspace.activePane, origin);
      case DesktopCommand.splitRight:
        _split(Axis.horizontal);
      case DesktopCommand.splitDown:
        _split(Axis.vertical);
      case DesktopCommand.nextTab:
      case DesktopCommand.previousTab:
        if (_workspace.tabs.isNotEmpty) {
          _workspace.selectTab(
            (_workspace.activeIndex +
                    (command == DesktopCommand.nextTab ? 1 : -1)) %
                _workspace.tabs.length,
          );
          _focusActive();
        }
      case DesktopCommand.nextPane:
        _workspace.cyclePane(1);
        _focusActive();
      case DesktopCommand.search:
        setState(() => _sidebar = true);
        WidgetsBinding.instance.addPostFrameCallback((_) {
          if (!_dialogOpen &&
              (origin != null
                  ? _isCurrentOrigin(origin)
                  : _isEmptyShellOrigin())) {
            _searchFocus.requestFocus();
          }
        });
      case DesktopCommand.settings:
        _settingsDialog(origin: origin, emptyShellOrigin: emptyShellOrigin);
      case DesktopCommand.toggleSidebar:
        setState(() => _sidebar = !_sidebar);
      case DesktopCommand.localTerminal:
        unawaited(_openLocal());
    }
    return KeyEventResult.handled;
  }

  @override
  Widget build(BuildContext context) {
    final active = _workspace.activePane;
    final narrow =
        MediaQuery.sizeOf(context).width < DesktopMetrics.narrowWidth;
    final highContrast = MediaQuery.highContrastOf(context);
    return DesktopWorkspaceScope(
      execute: _executeAction,
      ownsTerminalInput: (controller, enabled) {
        if (enabled) {
          _orderedInput.add(controller);
        } else {
          _orderedInput.remove(controller);
        }
      },
      isCurrentOrigin: _isCurrentOrigin,
      canRequestFocus: (origin) =>
          _originAttachedAndActive(origin) &&
          _terminalFocusAllowed &&
          !_dialogOpen &&
          (ModalRoute.of(context)?.isCurrent ?? true),
      child: Focus(
        focusNode: _shellFocus,
        autofocus: true,
        onKeyEvent: _key,
        child: Material(
          type: MaterialType.transparency,
          child: Column(
            children: [
              Expanded(
                child: Stack(
                  fit: StackFit.expand,
                  children: [
                    Row(
                      children: [
                        SizedBox(
                          width: 52,
                          child: Column(
                            children: [
                              const SizedBox(height: 10),
                              DesktopIconButton(
                                icon: f.FluentIcons.global_nav_button,
                                label: '连接侧栏 · Ctrl+Shift+B',
                                onPressed: () =>
                                    setState(() => _sidebar = !_sidebar),
                              ),
                              const SizedBox(height: 14),
                              DesktopIconButton(
                                icon: f.FluentIcons.add,
                                label: '快速连接 · Ctrl+Shift+T',
                                onPressed: () => _editor(quick: true),
                              ),
                              if (_localEntry)
                                DesktopIconButton(
                                  key: const ValueKey('workspace-local'),
                                  icon: f.FluentIcons.command_prompt,
                                  label: _localLabel,
                                  onPressed: _localPressed,
                                ),
                              const Spacer(),
                              DesktopIconButton(
                                icon: f.FluentIcons.permissions,
                                label: '私钥管理',
                                onPressed: _keys,
                              ),
                              DesktopIconButton(
                                icon: f.FluentIcons.settings,
                                label: '设置 · Ctrl+,',
                                onPressed: _settingsDialog,
                              ),
                              const SizedBox(height: 14),
                            ],
                          ),
                        ),
                        if (_sidebar && !narrow) ...[
                          SizedBox(width: _sidebarWidth, child: _sidebarView()),
                          MouseRegion(
                            cursor: SystemMouseCursors.resizeColumn,
                            child: GestureDetector(
                              behavior: HitTestBehavior.opaque,
                              onHorizontalDragUpdate: (details) => setState(
                                () => _sidebarWidth =
                                    (_sidebarWidth + details.delta.dx).clamp(
                                      210,
                                      320,
                                    ),
                              ),
                              child: const SizedBox(width: 5),
                            ),
                          ),
                        ],
                        Expanded(
                          child: Padding(
                            padding: const EdgeInsets.only(
                              right: 10,
                              bottom: 8,
                              top: 4,
                            ),
                            child: DecoratedBox(
                              decoration: BoxDecoration(
                                color: highContrast
                                    ? DesktopColors.highContrastCanvas
                                    : DesktopWorkspaceColors.surface,
                                borderRadius: BorderRadius.circular(8),
                                border: Border.all(
                                  color: highContrast
                                      ? DesktopColors.highContrastText
                                      : DesktopWorkspaceColors.border,
                                ),
                              ),
                              child: ClipRRect(
                                borderRadius: BorderRadius.circular(8),
                                child: Column(
                                  children: [
                                    if (_error != null)
                                      f.InfoBar(
                                        title: const Text('操作未完成'),
                                        content: Text(_error!),
                                        severity: f.InfoBarSeverity.error,
                                        onClose: () =>
                                            setState(() => _error = null),
                                      ),
                                    if (_workspace.tabs.isEmpty)
                                      Expanded(child: _welcome())
                                    else ...[
                                      SizedBox(
                                        height: 42,
                                        child: f.TabView(
                                          currentIndex: _workspace.activeIndex,
                                          shortcutsEnabled: false,
                                          minTabWidth: 100,
                                          maxTabWidth: 220,
                                          tabWidthBehavior:
                                              f.TabWidthBehavior.sizeToContent,
                                          onChanged: (index) {
                                            _workspace.selectTab(index);
                                            _focusActive();
                                          },
                                          onNewPressed: () =>
                                              _editor(quick: true),
                                          onReorder: _workspace.reorderTab,
                                          tabs: [
                                            for (final tab in _workspace.tabs)
                                              _tabs.putIfAbsent(
                                                tab,
                                                () => f.Tab(
                                                  text: ListenableBuilder(
                                                    listenable: _workspace,
                                                    builder: (_, _) => Text(
                                                      tab
                                                          .active
                                                          .controller
                                                          .title,
                                                      overflow:
                                                          TextOverflow.ellipsis,
                                                    ),
                                                  ),
                                                  icon: const Icon(
                                                    f
                                                        .FluentIcons
                                                        .command_prompt,
                                                    size: 14,
                                                  ),
                                                  body: const SizedBox.shrink(),
                                                  onClosed: () =>
                                                      _closeTab(tab),
                                                ),
                                              ),
                                          ],
                                        ),
                                      ),
                                      _toolbar(),
                                      Expanded(
                                        child: MultiSplitViewTheme(
                                          data: MultiSplitViewThemeData(
                                            dividerThickness: 5,
                                            dividerPainter:
                                                DividerPainters.background(
                                                  color: highContrast
                                                      ? DesktopColors
                                                            .highContrastText
                                                      : DesktopWorkspaceColors
                                                            .divider,
                                                  highlightedColor: highContrast
                                                      ? DesktopColors
                                                            .highContrastText
                                                      : desktopAccent,
                                                ),
                                          ),
                                          child: IndexedStack(
                                            index: _workspace.activeIndex,
                                            children: [
                                              for (final tab in _workspace.tabs)
                                                KeyedSubtree(
                                                  key: ObjectKey(tab),
                                                  child: _paneNode(
                                                    tab.root,
                                                    tab,
                                                  ),
                                                ),
                                            ],
                                          ),
                                        ),
                                      ),
                                      if (active != null &&
                                          (_settings?.showKeyBar ?? false))
                                        if (active.controller.normalSession
                                            case final normal?)
                                          DesktopTerminalKeyBar(
                                            key: ObjectKey(active),
                                            session: normal,
                                            rows:
                                                _settings?.keyBarRows ??
                                                defaultKeyBarRows,
                                            extraListen:
                                                active.controller.selection,
                                            canCopy: () =>
                                                active.controller.canCopy,
                                            onEdit: _editKeyBar,
                                            onAction: (id) {
                                              switch (id) {
                                                case 'copy':
                                                  active.controller.copy();
                                                case 'paste':
                                                  unawaited(
                                                    active.controller.paste(),
                                                  );
                                                case 'keyboard':
                                                  active.controller
                                                      .toggleKeyboard();
                                                case 'disconnect':
                                                  unawaited(_closePane());
                                                case 'zoomIn':
                                                  active.controller.zoom?.step(
                                                    1,
                                                  );
                                                case 'zoomOut':
                                                  active.controller.zoom?.step(
                                                    -1,
                                                  );
                                                case 'zoomReset':
                                                  active.controller.zoom
                                                      ?.reset();
                                              }
                                            },
                                          )
                                        else
                                          TerminalKeyBar(
                                            key: ObjectKey(active),
                                            terminal:
                                                active.controller.terminal,
                                            extraListen:
                                                active.controller.selection,
                                            canCopy: () =>
                                                active.controller.canCopy,
                                            onCopy: active.controller.copy,
                                            onPaste: active.controller.paste,
                                            onToggleKeyboard: active
                                                .controller
                                                .toggleKeyboard,
                                            onDisconnect: _closePane,
                                            rows:
                                                _settings?.keyBarRows ??
                                                defaultKeyBarRows,
                                            onEdit: _editKeyBar,
                                            onZoomIn: () =>
                                                active.controller.zoom?.step(1),
                                            onZoomOut: () => active
                                                .controller
                                                .zoom
                                                ?.step(-1),
                                            onZoomReset: () =>
                                                active.controller.zoom?.reset(),
                                          ),
                                    ],
                                  ],
                                ),
                              ),
                            ),
                          ),
                        ),
                      ],
                    ),
                    if (narrow && _sidebar)
                      Positioned(
                        left: 52,
                        top: 0,
                        bottom: 0,
                        right: DesktopSpacing.sm,
                        child: ColoredBox(
                          color: highContrast
                              ? DesktopColors.highContrastCanvas
                              : desktopBackground,
                          child: _sidebarView(),
                        ),
                      ),
                  ],
                ),
              ),
              _statusBar(),
            ],
          ),
        ),
      ),
    );
  }

  Widget _sidebarView() {
    final sidebar = WindowsSidebar(
      search: _search,
      focusNode: _searchFocus,
      connections: _connections,
      selectedId: _selectedId,
      onSelect: (item) => setState(() => _selectedId = item.id),
      onOpen: (item) {
        _open(item);
        if (MediaQuery.sizeOf(context).width < DesktopMetrics.narrowWidth) {
          setState(() => _sidebar = false);
        }
      },
      onNew: () => _editor(),
      onAction: _catalogAction,
    );
    if (!_workspace.panes.any((pane) => pane.controller.ownsNativeInput)) {
      return sidebar;
    }
    return FocusScope(
      canRequestFocus: !_workspace.panes.any(
        (pane) => !pane.controller.nativeEditableSafe,
      ),
      child: Listener(
        onPointerDown: (event) async {
          if (!_workspace.panes.any(
            (pane) => !pane.controller.nativeEditableSafe,
          )) {
            return;
          }
          final searchBox = _searchFocus.context?.findRenderObject();
          final searchHit =
              searchBox is RenderBox &&
              searchBox.hasSize &&
              (searchBox.localToGlobal(Offset.zero) & searchBox.size).contains(
                event.position,
              );
          final origin = _workspace.activePane?.controller;
          if (!await _quietAll() || !mounted) return;
          final intent = _focusIntent;
          setState(() {});
          WidgetsBinding.instance.addPostFrameCallback((_) {
            if (searchHit &&
                mounted &&
                !_dialogOpen &&
                intent == _focusIntent &&
                _workspace.activePane?.controller == origin &&
                (ModalRoute.of(context)?.isCurrent ?? true)) {
              _searchFocus.requestFocus();
            }
          });
        },
        child: sidebar,
      ),
    );
  }

  bool _originAttachedAndActive(TerminalPaneController origin) =>
      mounted &&
      _workspace.activePane?.controller == origin &&
      _workspace.panes.any((pane) => identical(pane.controller, origin)) &&
      origin.focusNode.context != null;

  bool _isCurrentOrigin(TerminalPaneController origin) =>
      _originAttachedAndActive(origin) &&
      (origin.ownsNativeInput
          ? origin.nativeActive
          : origin.focusNode.hasFocus) &&
      !_dialogOpen &&
      (ModalRoute.of(context)?.isCurrent ?? true);

  Future<DesktopActionOutcome> _executeAction(
    DesktopAction action, {
    required TerminalPaneController origin,
  }) async {
    if (!_isCurrentOrigin(origin)) return DesktopActionOutcome.staleOrigin;
    final nativeAction = origin.ownsNativeInput;
    switch (action) {
      case DesktopAction.newTab:
        return _editor(quick: true, origin: origin);
      case DesktopAction.splitRight:
        _split(Axis.horizontal);
        if (nativeAction) await WidgetsBinding.instance.endOfFrame;
      case DesktopAction.splitDown:
        _split(Axis.vertical);
        if (nativeAction) await WidgetsBinding.instance.endOfFrame;
      case DesktopAction.closePane:
        final closed = await _closePane(_workspace.activePane, origin);
        if (closed && nativeAction) {
          await WidgetsBinding.instance.endOfFrame;
        }
        return closed
            ? DesktopActionOutcome.executed
            : DesktopActionOutcome.unavailable;
      case DesktopAction.nextTab:
      case DesktopAction.previousTab:
        if (!await _quietAll() ||
            !mounted ||
            !_originAttachedAndActive(origin)) {
          return DesktopActionOutcome.staleOrigin;
        }
        _workspace.selectTab(
          (_workspace.activeIndex +
                  (action == DesktopAction.nextTab ? 1 : -1)) %
              _workspace.tabs.length,
        );
        final focused = _focusActive();
        if (nativeAction && !await focused) {
          return DesktopActionOutcome.unavailable;
        }
      case DesktopAction.nextPane:
        if (!await _quietAll() ||
            !mounted ||
            !_originAttachedAndActive(origin)) {
          return DesktopActionOutcome.staleOrigin;
        }
        _workspace.cyclePane(1);
        final focused = _focusActive();
        if (nativeAction && !await focused) {
          return DesktopActionOutcome.unavailable;
        }
      case DesktopAction.search:
        if (!await _quietAll() ||
            !mounted ||
            !_originAttachedAndActive(origin)) {
          return DesktopActionOutcome.staleOrigin;
        }
        setState(() => _sidebar = true);
        final focused = Completer<bool>();
        WidgetsBinding.instance.addPostFrameCallback((_) {
          if (mounted &&
              !_dialogOpen &&
              _originAttachedAndActive(origin) &&
              (ModalRoute.of(context)?.isCurrent ?? true)) {
            _searchFocus.requestFocus();
            WidgetsBinding.instance.endOfFrame.then((_) {
              focused.complete(
                mounted && !_dialogOpen && _searchFocus.hasFocus,
              );
            });
          } else {
            focused.complete(false);
          }
        });
        if (nativeAction && !await focused.future) {
          return DesktopActionOutcome.unavailable;
        }
      case DesktopAction.settings:
        return _settingsDialog(origin: origin);
      case DesktopAction.toggleSidebar:
        setState(() => _sidebar = !_sidebar);
      case DesktopAction.copy:
        if (nativeAction) return DesktopActionOutcome.unavailable;
        if (origin.sessionId == 0 || !origin.canCopy) {
          return DesktopActionOutcome.unavailable;
        }
        origin.copy();
      case DesktopAction.paste:
      case DesktopAction.selectAll:
        // paste 缺少读取剪贴板后再校验来源的接缝，selectAll 当前没有公开 API。
        return DesktopActionOutcome.needsPaneApi;
      case DesktopAction.zoomIn:
      case DesktopAction.zoomOut:
      case DesktopAction.zoomReset:
        final zoom = origin.zoom;
        if (zoom == null) return DesktopActionOutcome.unavailable;
        if (action == DesktopAction.zoomReset) {
          zoom.reset();
        } else {
          zoom.step(action == DesktopAction.zoomIn ? 1 : -1);
        }
        if (nativeAction) await WidgetsBinding.instance.endOfFrame;
        if (!mounted || !_originAttachedAndActive(origin)) {
          return DesktopActionOutcome.staleOrigin;
        }
      case DesktopAction.newLocalTerminal:
        if (!_localEntry || !await _openLocal()) {
          return DesktopActionOutcome.unavailable;
        }
    }
    return DesktopActionOutcome.executed;
  }

  Widget _paneNode(PaneNode node, WorkspaceTab tab) => switch (node) {
    PaneLeaf(:final pane) =>
      widget.paneBuilder != null
          ? KeyedSubtree(key: pane.key, child: widget.paneBuilder!(pane))
          : TerminalPane(
              key: pane.key,
              controller: pane.controller,
              backgroundOpacity: MediaQuery.highContrastOf(context)
                  ? 1
                  : DesktopWorkspaceMetrics.terminalBackgroundOpacity,
              highlighted: tab.root is PaneSplit && tab.active == pane,
              onActivate: () => _workspace.activate(pane),
              onClose: () => _closePane(pane),
              onKeyEvent: _key,
            ),
    PaneSplit(:final axis, :final layout) => MultiSplitView(
      key: ObjectKey(node),
      axis: axis,
      controller: layout,
      builder: (_, area) => _paneNode(area.data as PaneNode, tab),
    ),
  };

  Widget _toolbar() => LayoutBuilder(
    builder: (context, constraints) => SingleChildScrollView(
      scrollDirection: Axis.horizontal,
      child: SizedBox(
        width: constraints.maxWidth < DesktopWorkspaceMetrics.toolbarMinWidth
            ? DesktopWorkspaceMetrics.toolbarMinWidth
            : constraints.maxWidth,
        child: Container(
          height: DesktopWorkspaceMetrics.toolbarHeight,
          padding: const EdgeInsets.symmetric(horizontal: 10),
          decoration: const BoxDecoration(
            border: Border(bottom: BorderSide(color: Colors.white10)),
          ),
          child: Row(
            children: [
              const Icon(
                f.FluentIcons.plug_connected,
                size: 13,
                color: desktopMuted,
              ),
              const SizedBox(width: 8),
              Expanded(
                child: Text(
                  _workspace.activePane?.controller.target.title ?? '',
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(fontSize: 12, color: desktopMuted),
                ),
              ),
              DesktopIconButton(
                icon: f.FluentIcons.copy,
                label: '复制选区 · Ctrl+Shift+C',
                onPressed: () => _workspace.activePane?.controller.copy(),
              ),
              DesktopIconButton(
                icon: f.FluentIcons.paste,
                label: '粘贴 · Ctrl+Shift+V',
                onPressed: () => _workspace.activePane?.controller.paste(),
              ),
              const SizedBox(width: 8),
              DesktopIconButton(
                icon: f.FluentIcons.split,
                label: '左右分屏 · Ctrl+Shift+D',
                onPressed: () => _split(Axis.horizontal),
              ),
              DesktopIconButton(
                icon: f.FluentIcons.rows_group,
                label: '上下分屏 · Ctrl+Shift+E',
                onPressed: () => _split(Axis.vertical),
              ),
            ],
          ),
        ),
      ),
    ),
  );

  Widget _welcome() => Center(
    child: SingleChildScrollView(
      padding: const EdgeInsets.all(DesktopSpacing.lg),
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 620),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Container(
              width: 64,
              height: 64,
              decoration: BoxDecoration(
                color: desktopAccent.withValues(alpha: 0.1),
                borderRadius: BorderRadius.circular(14),
                border: Border.all(color: desktopAccent.withValues(alpha: 0.2)),
              ),
              child: const Icon(
                f.FluentIcons.command_prompt,
                size: 28,
                color: desktopAccent,
              ),
            ),
            const SizedBox(height: 26),
            const Text(
              '连接，开始工作。',
              style: TextStyle(
                fontSize: 30,
                fontWeight: FontWeight.w600,
                letterSpacing: -0.6,
              ),
            ),
            const SizedBox(height: 12),
            const Text(
              '你的服务器，你的工作区。\n从左侧打开连接，或创建一个新的 SSH 会话。',
              style: TextStyle(height: 1.7, color: desktopMuted),
            ),
            const SizedBox(height: 28),
            Wrap(
              spacing: 12,
              runSpacing: 12,
              children: [
                f.FilledButton(
                  onPressed: () => _editor(quick: true),
                  child: const Padding(
                    padding: EdgeInsets.symmetric(horizontal: 10, vertical: 5),
                    child: Text('快速连接'),
                  ),
                ),
                f.Button(
                  onPressed: () => _editor(),
                  child: const Padding(
                    padding: EdgeInsets.symmetric(horizontal: 10, vertical: 5),
                    child: Text('添加连接'),
                  ),
                ),
                if (_localEntry)
                  f.Tooltip(
                    message: _localLabel,
                    child: f.Button(
                      key: const ValueKey('welcome-local'),
                      onPressed: _localPressed,
                      child: const Padding(
                        padding: EdgeInsets.symmetric(
                          horizontal: 10,
                          vertical: 5,
                        ),
                        child: Text('本地终端'),
                      ),
                    ),
                  ),
              ],
            ),
            const Padding(
              padding: EdgeInsets.symmetric(vertical: 28),
              child: Divider(color: Colors.white10),
            ),
            if (MediaQuery.sizeOf(context).width >= DesktopMetrics.narrowWidth)
              Wrap(
                spacing: 28,
                children: [
                  const ShortcutHint('新建会话', 'Ctrl + Shift + T'),
                  const ShortcutHint('切换标签', 'Ctrl + Tab'),
                  const ShortcutHint('左右分屏', 'Ctrl + Shift + D'),
                  const ShortcutHint('搜索连接', 'Ctrl + Shift + F'),
                  if (_localEntry)
                    const ShortcutHint('本地终端', 'Ctrl + Shift + L'),
                ],
              ),
          ],
        ),
      ),
    ),
  );

  Widget _statusBar() {
    final controller = _workspace.activePane?.controller;
    final status = switch (controller?.state) {
      SessionState.connected => '已连接',
      SessionState.failed => '连接失败',
      SessionState.closed => '已断开',
      null when controller == null => '就绪',
      _ => '正在连接',
    };
    return Container(
      height: 28,
      padding: const EdgeInsets.symmetric(horizontal: 18),
      child: Row(
        children: [
          Icon(
            f.FluentIcons.circle_fill,
            size: 7,
            color: controller?.connected == true
                ? const Color(0xff82d6a1)
                : desktopMuted,
          ),
          const SizedBox(width: 8),
          Text(
            status,
            style: const TextStyle(fontSize: 11, color: desktopMuted),
          ),
          const Spacer(),
          Flexible(
            child: Text(
              '${_workspace.tabs.length} 个标签  ·  ${_workspace.panes.length} 个窗格',
              style: const TextStyle(fontSize: 11, color: desktopMuted),
              overflow: TextOverflow.ellipsis,
            ),
          ),
        ],
      ),
    );
  }
}
