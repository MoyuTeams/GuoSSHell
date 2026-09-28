import 'package:fluent_ui/fluent_ui.dart' as f;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../bindings/bindings.dart';
import '../catalog/catalog_requests.dart';
import 'desktop_widgets.dart';
import 'windows_chrome.dart';

class WindowsSidebar extends StatelessWidget {
  const WindowsSidebar({
    super.key,
    required this.search,
    required this.focusNode,
    required this.connections,
    required this.selectedId,
    required this.onSelect,
    required this.onOpen,
    required this.onNew,
    required this.onAction,
  });
  final TextEditingController search;
  final FocusNode focusNode;
  final List<ConnectionSummary>? connections;
  final String? selectedId;
  final ValueChanged<ConnectionSummary> onSelect;
  final ValueChanged<ConnectionSummary> onOpen;
  final VoidCallback onNew;
  final void Function(ConnectionSummary, String) onAction;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.fromLTRB(10, 12, 12, 12),
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            const Expanded(
              child: Text(
                '连接',
                style: TextStyle(fontSize: 18, fontWeight: FontWeight.w600),
              ),
            ),
            DesktopIconButton(
              icon: f.FluentIcons.add,
              label: '添加连接',
              onPressed: onNew,
            ),
          ],
        ),
        const SizedBox(height: 12),
        f.TextBox(
          controller: search,
          focusNode: focusNode,
          placeholder: '搜索连接',
          prefix: const Padding(
            padding: EdgeInsets.only(left: 10),
            child: Icon(f.FluentIcons.search, size: 14, color: desktopMuted),
          ),
          suffix: DesktopIconButton(
            icon: f.FluentIcons.clear,
            label: '清除搜索',
            onPressed: search.clear,
          ),
        ),
        const SizedBox(height: 20),
        Row(
          children: [
            const Expanded(
              child: Text(
                '已保存',
                style: TextStyle(
                  fontSize: 11,
                  color: desktopMuted,
                  letterSpacing: 1,
                ),
              ),
            ),
            Text(
              '${connections?.length ?? 0}',
              style: const TextStyle(fontSize: 11, color: desktopMuted),
            ),
          ],
        ),
        const SizedBox(height: 8),
        Expanded(
          child: connections == null
              ? const Center(
                  child: SizedBox(
                    width: 22,
                    height: 22,
                    child: f.ProgressRing(strokeWidth: 2),
                  ),
                )
              : connections!.isEmpty
              ? Padding(
                  padding: const EdgeInsets.only(top: 30),
                  child: Text(
                    search.text.isEmpty
                        ? '还没有保存的连接。\n添加常用服务器，随时回来。'
                        : '没有匹配的连接',
                    style: const TextStyle(
                      fontSize: 12,
                      color: desktopMuted,
                      height: 1.7,
                    ),
                  ),
                )
              : ListView.builder(
                  itemCount: connections!.length,
                  itemBuilder: (_, index) {
                    final item = connections![index];
                    return _ConnectionRow(
                      item: item,
                      selected: selectedId == item.id,
                      onSelect: () => onSelect(item),
                      onOpen: () => onOpen(item),
                      onAction: (action) => onAction(item, action),
                    );
                  },
                ),
        ),
        const Padding(
          padding: EdgeInsets.only(top: 12),
          child: Text(
            '双击或 Enter 打开连接',
            style: TextStyle(fontSize: 11, color: desktopMuted),
          ),
        ),
      ],
    ),
  );
}

class _ConnectionRow extends StatefulWidget {
  const _ConnectionRow({
    required this.item,
    required this.selected,
    required this.onSelect,
    required this.onOpen,
    required this.onAction,
  });
  final ConnectionSummary item;
  final bool selected;
  final VoidCallback onSelect;
  final VoidCallback onOpen;
  final ValueChanged<String> onAction;
  @override
  State<_ConnectionRow> createState() => _ConnectionRowState();
}

class _ConnectionRowState extends State<_ConnectionRow> {
  final _flyout = f.FlyoutController();
  final _focus = FocusNode();

  @override
  void dispose() {
    _flyout.dispose();
    _focus.dispose();
    super.dispose();
  }

  void _menu() {
    widget.onSelect();
    _flyout.showFlyout(
      builder: (context) => f.MenuFlyout(
        items: [
          for (final entry in [
            ('open', '连接', f.FluentIcons.plug_connected),
            ('edit', '编辑', f.FluentIcons.edit),
            ('duplicate', '复制', f.FluentIcons.copy),
            ('delete', '删除', f.FluentIcons.delete),
          ])
            f.MenuFlyoutItem(
              leading: Icon(entry.$3, size: 14),
              text: Text(entry.$2),
              onPressed: () {
                Navigator.of(context).pop();
                widget.onAction(entry.$1);
              },
            ),
        ],
      ),
    );
  }

  @override
  Widget build(BuildContext context) => f.FlyoutTarget(
    controller: _flyout,
    child: Focus(
      onKeyEvent: (_, event) {
        if (event is! KeyDownEvent) return KeyEventResult.ignored;
        if (event.logicalKey == LogicalKeyboardKey.enter) {
          widget.onOpen();
          return KeyEventResult.handled;
        }
        if (event.logicalKey == LogicalKeyboardKey.f2) {
          widget.onAction('edit');
          return KeyEventResult.handled;
        }
        if (event.logicalKey == LogicalKeyboardKey.delete) {
          widget.onAction('delete');
          return KeyEventResult.handled;
        }
        if (event.logicalKey == LogicalKeyboardKey.contextMenu) {
          _menu();
          return KeyEventResult.handled;
        }
        return KeyEventResult.ignored;
      },
      child: GestureDetector(
        onDoubleTap: widget.onOpen,
        onSecondaryTap: _menu,
        child: f.ListTile.selectable(
          focusNode: _focus,
          selected: widget.selected,
          onPressed: () {
            widget.onSelect();
            _focus.requestFocus();
          },
          onSelectionChange: (_) {
            widget.onSelect();
            _focus.requestFocus();
          },
          margin: const EdgeInsets.symmetric(vertical: 2),
          contentPadding: const EdgeInsets.fromLTRB(10, 10, 4, 10),
          leading: const Icon(
            f.FluentIcons.server,
            size: 18,
            color: desktopAccent,
          ),
          title: Text(
            connectionTitle(widget.item),
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: const TextStyle(fontSize: 13),
          ),
          subtitle: Text(
            '${widget.item.username}@${widget.item.host}',
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: const TextStyle(fontSize: 11, color: desktopMuted),
          ),
          trailing: DesktopIconButton(
            icon: f.FluentIcons.more,
            label: '连接操作',
            onPressed: _menu,
          ),
        ),
      ),
    ),
  );
}
