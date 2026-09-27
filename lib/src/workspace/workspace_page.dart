import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:multi_split_view/multi_split_view.dart';

import '../bindings/bindings.dart';
import '../settings/key_bar_editor.dart';
import '../terminal/key_bar_layout.dart';
import '../terminal/session_target.dart';
import '../terminal/terminal_key_bar.dart';
import '../terminal/terminal_pane.dart';
import 'connection_picker.dart';
import 'workspace.dart';

/// 会话工作区：标签条、每个标签的分屏，以及跟着活动窗格走的键位条。
/// 从连接列表打开时带着第一个连接；最后一个窗格关掉后回到列表。
class WorkspacePage extends StatefulWidget {
  final SessionTarget initial;

  const WorkspacePage({super.key, required this.initial});

  @override
  State<WorkspacePage> createState() => _WorkspacePageState();
}

class _WorkspacePageState extends State<WorkspacePage> {
  final Workspace _workspace = Workspace();
  StreamSubscription? _settingsSub;
  SettingsState? _settings = SettingsState.latestRustSignal?.message;

  @override
  void initState() {
    super.initState();
    _workspace.openTab(widget.initial);
    _workspace.addListener(_onWorkspaceChanged);
    _settingsSub = SettingsState.rustSignalStream.listen((pack) {
      if (mounted) setState(() => _settings = pack.message);
    });
  }

  @override
  void dispose() {
    _settingsSub?.cancel();
    _workspace.removeListener(_onWorkspaceChanged);
    _workspace.dispose();
    super.dispose();
  }

  void _onWorkspaceChanged() {
    if (!mounted) return;
    if (_workspace.tabs.isEmpty) {
      Navigator.of(context).pop();
      return;
    }
    setState(() {});
  }

  int get _connectedCount =>
      _workspace.panes.where((pane) => pane.controller.connected).length;

  // ── 标签与分屏 ──

  Future<void> _newTab() async {
    final target = await pickConnection(context);
    if (target != null && mounted) {
      _focus(_workspace.openTab(target));
    }
  }

  Future<void> _split(Axis axis) async {
    final pane = _workspace.activePane;
    if (pane == null) return;
    final target = await pickConnection(
      context,
      current: pane.controller.target,
    );
    if (target != null && mounted && _workspace.panes.contains(pane)) {
      _focus(_workspace.split(pane, axis, target));
    }
  }

  /// 新窗格布局完成后接过键盘焦点（软键盘开着时跟过去）。
  void _focus(WorkspacePane pane) {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted && _workspace.panes.contains(pane)) {
        pane.controller.focusNode.requestFocus();
      }
    });
  }

  /// 关窗格：连着的先确认。
  Future<void> _closePane(WorkspacePane pane) async {
    if (pane.controller.connected) {
      final confirmed = await _confirm(
        '断开与「${pane.controller.target.title}」的连接？',
        action: '断开',
      );
      if (!confirmed || !mounted) return;
    }
    if (_workspace.panes.contains(pane)) _workspace.close(pane);
  }

  Future<void> _closeTab(WorkspaceTab tab) async {
    final connected = tab.panes
        .where((pane) => pane.controller.connected)
        .length;
    if (connected > 0) {
      final confirmed = await _confirm(
        connected == 1 ? '断开这个标签里的会话？' : '断开这个标签里的 $connected 个会话？',
        action: '断开',
      );
      if (!confirmed || !mounted) return;
    }
    _workspace.closeTab(tab);
  }

  /// 离开工作区：还有连着的会话先确认。
  Future<void> _leave() async {
    final connected = _connectedCount;
    if (connected > 0) {
      final confirmed = await _confirm(
        connected == 1 ? '断开会话并返回？' : '断开全部 $connected 个会话并返回？',
        action: '断开',
      );
      if (!confirmed || !mounted) return;
    }
    Navigator.of(context).pop();
  }

  Future<bool> _confirm(String title, {required String action}) async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text(title),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('取消'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, true),
            child: Text(action),
          ),
        ],
      ),
    );
    return confirmed == true;
  }

  Future<void> _editKeyBar() async {
    await Navigator.of(context).push(
      MaterialPageRoute<void>(
        builder: (_) => KeyBarEditor(
          initialRows: _settings?.keyBarRows ?? defaultKeyBarRows,
          onSave: saveKeyBarLayout,
        ),
      ),
    );
    if (!mounted) return;
    final pane = _workspace.activePane;
    if (pane != null) _focus(pane);
  }

  // ── 硬件键盘快捷键 ──

  /// ⌘T 新标签、⌘W 关窗格、⌘D 左右分屏、⌘⇧D 上下分屏、⌘1…9 切标签、
  /// ⌘⇧[ / ⌘⇧] 前后标签、⌘[ / ⌘] 前后窗格。其余按键照常交给终端。
  KeyEventResult _onKeyEvent(FocusNode node, KeyEvent event) {
    final keyboard = HardwareKeyboard.instance;
    if (_workspace.activePane?.controller.zoom?.handleKey(
          event,
          meta: keyboard.isMetaPressed,
        ) ??
        false) {
      return KeyEventResult.handled;
    }
    if (event is! KeyDownEvent || !keyboard.isMetaPressed) {
      return KeyEventResult.ignored;
    }
    final key = event.logicalKey;
    final shift = keyboard.isShiftPressed;
    void run(void Function() action) =>
        WidgetsBinding.instance.addPostFrameCallback((_) {
          if (mounted) action();
        });
    if (key == LogicalKeyboardKey.keyT) {
      run(_newTab);
    } else if (key == LogicalKeyboardKey.keyW) {
      final pane = _workspace.activePane;
      if (pane != null) run(() => _closePane(pane));
    } else if (key == LogicalKeyboardKey.keyD) {
      run(() => _split(shift ? Axis.vertical : Axis.horizontal));
    } else if (key == LogicalKeyboardKey.bracketLeft ||
        key == LogicalKeyboardKey.bracketRight) {
      final step = key == LogicalKeyboardKey.bracketRight ? 1 : -1;
      if (shift) {
        final count = _workspace.tabs.length;
        _workspace.selectTab((_workspace.activeIndex + step + count) % count);
      } else {
        _workspace.cyclePane(step);
      }
      _focus(_workspace.activePane!);
    } else if (_digitKeys.contains(key)) {
      final index = _digitKeys.indexOf(key);
      _workspace.selectTab(index == 8 ? _workspace.tabs.length - 1 : index);
      _focus(_workspace.activePane!);
    } else {
      return KeyEventResult.ignored;
    }
    return KeyEventResult.handled;
  }

  static const List<LogicalKeyboardKey> _digitKeys = [
    LogicalKeyboardKey.digit1,
    LogicalKeyboardKey.digit2,
    LogicalKeyboardKey.digit3,
    LogicalKeyboardKey.digit4,
    LogicalKeyboardKey.digit5,
    LogicalKeyboardKey.digit6,
    LogicalKeyboardKey.digit7,
    LogicalKeyboardKey.digit8,
    LogicalKeyboardKey.digit9,
  ];

  // ── 布局 ──

  @override
  Widget build(BuildContext context) {
    final active = _workspace.activePane;
    return PopScope(
      canPop: _connectedCount == 0,
      onPopInvokedWithResult: (didPop, _) {
        if (!didPop) _leave();
      },
      child: Scaffold(
        backgroundColor: Colors.black,
        body: SafeArea(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              _TabStrip(
                workspace: _workspace,
                onBack: _leave,
                onNewTab: _newTab,
                onCloseTab: _closeTab,
                onSplit: _split,
              ),
              Expanded(
                child: MultiSplitViewTheme(
                  data: MultiSplitViewThemeData(
                    dividerThickness: 6,
                    dividerPainter: DividerPainters.background(
                      color: Colors.grey.shade900,
                      highlightedColor: Theme.of(context).colorScheme.primary,
                    ),
                  ),
                  // 所有标签常驻（会话不断），只画当前的。
                  child: IndexedStack(
                    index: _workspace.activeIndex,
                    children: [
                      for (final tab in _workspace.tabs)
                        _buildNode(
                          tab.root,
                          tab,
                          multiple: tab.root is PaneSplit,
                        ),
                    ],
                  ),
                ),
              ),
              if (active != null && (_settings?.showKeyBar ?? true))
                TerminalKeyBar(
                  key: ObjectKey(active),
                  terminal: active.controller.terminal,
                  extraListen: active.controller.selection,
                  canCopy: () => active.controller.canCopy,
                  onCopy: active.controller.copy,
                  onPaste: active.controller.paste,
                  onToggleKeyboard: active.controller.toggleKeyboard,
                  onDisconnect: () => _closePane(active),
                  rows: _settings?.keyBarRows ?? defaultKeyBarRows,
                  onEdit: _editKeyBar,
                  onZoomIn: () => active.controller.zoom?.step(1),
                  onZoomOut: () => active.controller.zoom?.step(-1),
                  onZoomReset: () => active.controller.zoom?.reset(),
                ),
            ],
          ),
        ),
      ),
    );
  }

  Widget _buildNode(PaneNode node, WorkspaceTab tab, {required bool multiple}) {
    return switch (node) {
      PaneLeaf(:final pane) => TerminalPane(
        key: pane.key,
        controller: pane.controller,
        highlighted: multiple && tab.active == pane,
        onActivate: () => _workspace.activate(pane),
        onClose: () => _workspace.close(pane),
        onKeyEvent: _onKeyEvent,
      ),
      PaneSplit(:final axis, :final layout) => MultiSplitView(
        key: ObjectKey(node),
        axis: axis,
        controller: layout,
        builder: (context, area) =>
            _buildNode(area.data as PaneNode, tab, multiple: multiple),
      ),
    };
  }
}

/// 顶部的标签条：返回、标签（标题取活动窗格）、新标签与分屏。
class _TabStrip extends StatelessWidget {
  final Workspace workspace;
  final VoidCallback onBack;
  final VoidCallback onNewTab;
  final void Function(WorkspaceTab tab) onCloseTab;
  final void Function(Axis axis) onSplit;

  const _TabStrip({
    required this.workspace,
    required this.onBack,
    required this.onNewTab,
    required this.onCloseTab,
    required this.onSplit,
  });

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Container(
      height: 40,
      color: Colors.grey.shade900,
      child: Row(
        children: [
          IconButton(
            tooltip: '返回',
            iconSize: 20,
            icon: const Icon(Icons.arrow_back_ios_new),
            onPressed: onBack,
          ),
          Expanded(
            child: ListView.builder(
              scrollDirection: Axis.horizontal,
              itemCount: workspace.tabs.length,
              itemBuilder: (context, index) {
                final tab = workspace.tabs[index];
                final selected = index == workspace.activeIndex;
                return _TabChip(
                  title: tab.active.controller.title,
                  state: tab.active.controller.state,
                  panes: tab.panes.length,
                  selected: selected,
                  selectedColor: scheme.primary,
                  onTap: () {
                    workspace.selectTab(index);
                    tab.active.controller.focusNode.requestFocus();
                  },
                  onClose: () => onCloseTab(tab),
                );
              },
            ),
          ),
          IconButton(
            tooltip: '新标签',
            iconSize: 20,
            icon: const Icon(Icons.add),
            onPressed: onNewTab,
          ),
          PopupMenuButton<Axis>(
            tooltip: '分屏',
            icon: const Icon(Icons.vertical_split_outlined, size: 20),
            onSelected: onSplit,
            itemBuilder: (_) => const [
              PopupMenuItem(value: Axis.horizontal, child: Text('左右分屏')),
              PopupMenuItem(value: Axis.vertical, child: Text('上下分屏')),
            ],
          ),
        ],
      ),
    );
  }
}

class _TabChip extends StatelessWidget {
  final String title;
  final SessionState? state;
  final int panes;
  final bool selected;
  final Color selectedColor;
  final VoidCallback onTap;
  final VoidCallback onClose;

  const _TabChip({
    required this.title,
    required this.state,
    required this.panes,
    required this.selected,
    required this.selectedColor,
    required this.onTap,
    required this.onClose,
  });

  @override
  Widget build(BuildContext context) {
    final ended = state == SessionState.failed || state == SessionState.closed;
    return InkWell(
      onTap: onTap,
      child: Container(
        constraints: const BoxConstraints(minWidth: 80, maxWidth: 220),
        padding: const EdgeInsets.only(left: 12),
        decoration: BoxDecoration(
          color: selected ? Colors.black : null,
          border: Border(
            top: BorderSide(
              color: selected ? selectedColor : Colors.transparent,
              width: 2,
            ),
          ),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            if (ended)
              const Padding(
                padding: EdgeInsets.only(right: 6),
                child: Icon(Icons.link_off, size: 14, color: Colors.white54),
              ),
            Flexible(
              child: Text(
                title,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                  fontSize: 13,
                  color: selected ? Colors.white : Colors.white60,
                ),
              ),
            ),
            if (panes > 1)
              Padding(
                padding: const EdgeInsets.only(left: 6),
                child: Text(
                  '$panes',
                  style: const TextStyle(fontSize: 11, color: Colors.white54),
                ),
              ),
            IconButton(
              tooltip: '关闭标签',
              iconSize: 14,
              visualDensity: VisualDensity.compact,
              icon: const Icon(Icons.close, color: Colors.white54),
              onPressed: onClose,
            ),
          ],
        ),
      ),
    );
  }
}
