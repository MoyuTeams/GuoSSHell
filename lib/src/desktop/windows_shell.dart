import 'dart:async';

import 'package:fluent_ui/fluent_ui.dart' as f;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:multi_split_view/multi_split_view.dart';

import '../bindings/bindings.dart';
import '../catalog/catalog_requests.dart';
import '../catalog/connection_editor_page.dart';
import '../settings/key_bar_editor.dart';
import '../terminal/key_bar_layout.dart';
import '../terminal/session_target.dart';
import '../terminal/terminal_key_bar.dart';
import '../terminal/terminal_pane.dart';
import '../workspace/workspace.dart';
import 'desktop_shortcuts.dart';
import 'desktop_widgets.dart';
import 'windows_chrome.dart';
import 'windows_settings.dart';
import 'windows_sidebar.dart';
import 'windows_keys.dart';

/// Windows 常驻工作区。目录、设置和编辑器的切换不销毁终端会话。
class WindowsShell extends StatefulWidget {
  const WindowsShell({super.key, this.initial, this.queryCatalog});
  final SessionTarget? initial;
  final ValueChanged<String>? queryCatalog;
  @override
  State<WindowsShell> createState() => _WindowsShellState();
}

class _WindowsShellState extends State<WindowsShell> {
  final _workspace = Workspace();
  final _search = TextEditingController();
  final _searchFocus = FocusNode();
  final _tabs = <WorkspaceTab, f.Tab>{};
  final _subscriptions = <StreamSubscription>[];
  List<ConnectionSummary>? _connections;
  SettingsState? _settings = SettingsState.latestRustSignal?.message;
  String? _selectedId;
  String? _error;
  bool _sidebar = true;
  double _sidebarWidth = 256;
  bool _dialogOpen = false;

  @override
  void initState() {
    super.initState();
    _workspace.addListener(_changed);
    if (widget.initial case final initial?) _workspace.openTab(initial);
    _subscriptions.add(
      CatalogState.rustSignalStream.listen((pack) {
        if (!mounted || pack.message.query != _search.text) return;
        setState(() {
          _connections = pack.message.connections;
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
    WindowsDesktopWindow.confirmClose = _confirmClose;
  }

  void _query() => widget.queryCatalog != null
      ? widget.queryCatalog!(_search.text)
      : CatalogQuery(query: _search.text).sendSignalToRust();

  void _changed() {
    if (!mounted) return;
    setState(
      () => _tabs.removeWhere((tab, _) => !_workspace.tabs.contains(tab)),
    );
  }

  @override
  void dispose() {
    WindowsDesktopWindow.confirmClose = null;
    for (final subscription in _subscriptions) {
      subscription.cancel();
    }
    _workspace.removeListener(_changed);
    _workspace.dispose();
    _search.dispose();
    _searchFocus.dispose();
    super.dispose();
  }

  void _focusActive() => WidgetsBinding.instance.addPostFrameCallback((_) {
    if (mounted && !_dialogOpen) {
      _workspace.activePane?.controller.focusNode.requestFocus();
    }
  });

  void _open(ConnectionSummary item) {
    _workspace.openTab(
      SessionTarget.saved(connectionId: item.id, title: connectionTitle(item)),
    );
    _focusActive();
  }

  Future<void> _editor({
    ConnectionSummary? existing,
    bool quick = false,
  }) async {
    if (_dialogOpen) return;
    _dialogOpen = true;
    final target = await f.showDialog<SessionTarget>(
      context: context,
      builder: (_) => ConnectionEditorPage(
        existing: existing,
        quick: quick,
        returnTarget: true,
      ),
    );
    _dialogOpen = false;
    if (!mounted) return;
    if (target != null) _workspace.openTab(target);
    _query();
    _focusActive();
  }

  Future<void> _settingsDialog() async {
    if (_dialogOpen) return;
    _dialogOpen = true;
    await f.showDialog<void>(
      context: context,
      builder: (_) => const WindowsSettings(),
    );
    _dialogOpen = false;
    _focusActive();
  }

  Future<void> _keys() async {
    if (_dialogOpen) return;
    _dialogOpen = true;
    await f.showDialog<void>(
      context: context,
      builder: (_) => const WindowsKeys(),
    );
    _dialogOpen = false;
    _focusActive();
  }

  Future<bool> _confirmClose() async {
    final count = _workspace.panes
        .where(
          (pane) =>
              pane.controller.state != SessionState.closed &&
              pane.controller.state != SessionState.failed,
        )
        .length;
    if (count == 0) return true;
    return desktopConfirm(
      context,
      '关闭 GuoSSHell？',
      '这将断开工作区中的 $count 个会话。',
      action: '断开并关闭',
    );
  }

  Future<void> _closeTab(WorkspaceTab tab) async {
    if (tab.panes.any(
      (pane) =>
          pane.controller.state != SessionState.closed &&
          pane.controller.state != SessionState.failed,
    )) {
      if (!await desktopConfirm(
        context,
        '关闭标签？',
        '这将断开「${tab.active.controller.title}」中的会话。',
        action: '断开',
      )) {
        return;
      }
    }
    if (!mounted || !_workspace.tabs.contains(tab)) return;
    _workspace.closeTab(tab);
    _focusActive();
  }

  Future<void> _closePane() async {
    final pane = _workspace.activePane;
    if (pane == null) return;
    if (pane.controller.state != SessionState.closed &&
        pane.controller.state != SessionState.failed) {
      if (!await desktopConfirm(
        context,
        '关闭窗格？',
        '断开「${pane.controller.title}」的会话。',
        action: '断开',
      )) {
        return;
      }
    }
    if (mounted && _workspace.panes.contains(pane)) _workspace.close(pane);
    _focusActive();
  }

  void _split(Axis axis) {
    final pane = _workspace.activePane;
    if (pane == null) return;
    _workspace.split(pane, axis, pane.controller.target);
    _focusActive();
  }

  Future<void> _catalogAction(ConnectionSummary item, String action) async {
    if (action == 'edit') return _editor(existing: item);
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
          ? duplicateConnection(item.id)
          : deleteConnection(item.id));
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
    final keyboard = HardwareKeyboard.instance;
    final key = event.logicalKey;
    final command = desktopCommand(
      key,
      control: keyboard.isControlPressed,
      shift: keyboard.isShiftPressed,
      alt: keyboard.isAltPressed,
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
    if (event is! KeyDownEvent) return KeyEventResult.handled;
    switch (command) {
      case DesktopCommand.newTab:
        _editor(quick: true);
      case DesktopCommand.closePane:
        _closePane();
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
          if (mounted) _searchFocus.requestFocus();
        });
      case DesktopCommand.settings:
        _settingsDialog();
      case DesktopCommand.toggleSidebar:
        setState(() => _sidebar = !_sidebar);
    }
    return KeyEventResult.handled;
  }

  @override
  Widget build(BuildContext context) {
    final active = _workspace.activePane;
    return Focus(
      autofocus: true,
      onKeyEvent: _key,
      child: Material(
        type: MaterialType.transparency,
        child: Column(
          children: [
            Expanded(
              child: Row(
                children: [
                  SizedBox(
                    width: 52,
                    child: Column(
                      children: [
                        const SizedBox(height: 10),
                        DesktopIconButton(
                          icon: f.FluentIcons.global_nav_button,
                          label: '连接侧栏 · Ctrl+Shift+B',
                          onPressed: () => setState(() => _sidebar = !_sidebar),
                        ),
                        const SizedBox(height: 14),
                        DesktopIconButton(
                          icon: f.FluentIcons.add,
                          label: '快速连接 · Ctrl+Shift+T',
                          onPressed: () => _editor(quick: true),
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
                  if (_sidebar) ...[
                    SizedBox(
                      width: _sidebarWidth,
                      child: WindowsSidebar(
                        search: _search,
                        focusNode: _searchFocus,
                        connections: _connections,
                        selectedId: _selectedId,
                        onSelect: (item) =>
                            setState(() => _selectedId = item.id),
                        onOpen: _open,
                        onNew: () => _editor(),
                        onAction: _catalogAction,
                      ),
                    ),
                    MouseRegion(
                      cursor: SystemMouseCursors.resizeColumn,
                      child: GestureDetector(
                        behavior: HitTestBehavior.opaque,
                        onHorizontalDragUpdate: (details) => setState(
                          () =>
                              _sidebarWidth = (_sidebarWidth + details.delta.dx)
                                  .clamp(210, 320),
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
                          color: const Color(0xff10141b).withValues(alpha: 0.3),
                          borderRadius: BorderRadius.circular(8),
                          border: Border.all(color: Colors.white10),
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
                                  onClose: () => setState(() => _error = null),
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
                                    onNewPressed: () => _editor(quick: true),
                                    onReorder: _workspace.reorderTab,
                                    tabs: [
                                      for (final tab in _workspace.tabs)
                                        _tabs.putIfAbsent(
                                          tab,
                                          () => f.Tab(
                                            text: ListenableBuilder(
                                              listenable: _workspace,
                                              builder: (_, _) => Text(
                                                tab.active.controller.title,
                                                overflow: TextOverflow.ellipsis,
                                              ),
                                            ),
                                            icon: const Icon(
                                              f.FluentIcons.command_prompt,
                                              size: 14,
                                            ),
                                            body: const SizedBox.shrink(),
                                            onClosed: () => _closeTab(tab),
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
                                            color: const Color(0xff323b4b),
                                            highlightedColor: desktopAccent,
                                          ),
                                    ),
                                    child: IndexedStack(
                                      index: _workspace.activeIndex,
                                      children: [
                                        for (final tab in _workspace.tabs)
                                          KeyedSubtree(
                                            key: ObjectKey(tab),
                                            child: _paneNode(tab.root, tab),
                                          ),
                                      ],
                                    ),
                                  ),
                                ),
                                if (active != null &&
                                    (_settings?.showKeyBar ?? false))
                                  TerminalKeyBar(
                                    key: ObjectKey(active),
                                    terminal: active.controller.terminal,
                                    extraListen: active.controller.selection,
                                    canCopy: () => active.controller.canCopy,
                                    onCopy: active.controller.copy,
                                    onPaste: active.controller.paste,
                                    onToggleKeyboard:
                                        active.controller.toggleKeyboard,
                                    onDisconnect: _closePane,
                                    rows:
                                        _settings?.keyBarRows ??
                                        defaultKeyBarRows,
                                    onEdit: () => showDesktopPage<void>(
                                      context,
                                      KeyBarEditor(
                                        initialRows:
                                            _settings?.keyBarRows ??
                                            defaultKeyBarRows,
                                        onSave: saveKeyBarLayout,
                                      ),
                                    ),
                                    onZoomIn: () =>
                                        active.controller.zoom?.step(1),
                                    onZoomOut: () =>
                                        active.controller.zoom?.step(-1),
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
            ),
            _statusBar(),
          ],
        ),
      ),
    );
  }

  Widget _paneNode(PaneNode node, WorkspaceTab tab) => switch (node) {
    PaneLeaf(:final pane) => TerminalPane(
      key: pane.key,
      controller: pane.controller,
      backgroundOpacity: 0.12,
      highlighted: tab.root is PaneSplit && tab.active == pane,
      onActivate: () => _workspace.activate(pane),
      onClose: () => _workspace.close(pane),
      onKeyEvent: _key,
    ),
    PaneSplit(:final axis, :final layout) => MultiSplitView(
      key: ObjectKey(node),
      axis: axis,
      controller: layout,
      builder: (_, area) => _paneNode(area.data as PaneNode, tab),
    ),
  };

  Widget _toolbar() => Container(
    height: 40,
    padding: const EdgeInsets.symmetric(horizontal: 10),
    decoration: const BoxDecoration(
      border: Border(bottom: BorderSide(color: Colors.white10)),
    ),
    child: Row(
      children: [
        const Icon(f.FluentIcons.plug_connected, size: 13, color: desktopMuted),
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
  );

  Widget _welcome() => Center(
    child: SingleChildScrollView(
      padding: const EdgeInsets.all(32),
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
              ],
            ),
            const Padding(
              padding: EdgeInsets.symmetric(vertical: 28),
              child: Divider(color: Colors.white10),
            ),
            const Wrap(
              spacing: 28,
              children: [
                ShortcutHint('新建会话', 'Ctrl + Shift + T'),
                ShortcutHint('切换标签', 'Ctrl + Tab'),
                ShortcutHint('左右分屏', 'Ctrl + Shift + D'),
                ShortcutHint('搜索连接', 'Ctrl + Shift + F'),
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
          Text(
            '${_workspace.tabs.length} 个标签  ·  ${_workspace.panes.length} 个窗格',
            style: const TextStyle(fontSize: 11, color: desktopMuted),
          ),
          const SizedBox(width: 20),
          const Text(
            'SSH',
            style: TextStyle(fontSize: 11, color: desktopMuted),
          ),
        ],
      ),
    );
  }
}
