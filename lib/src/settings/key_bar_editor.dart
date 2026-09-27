import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';

import '../terminal/key_bar_layout.dart';
import '../terminal/key_bar_visuals.dart';

/// 直接在两排预览中编辑；保存成功后才应用，退出即放弃草稿。
class KeyBarEditor extends StatefulWidget {
  final List<List<String>> initialRows;
  final Future<void> Function(List<List<String>>) onSave;

  const KeyBarEditor({
    super.key,
    required this.initialRows,
    required this.onSave,
  });

  @override
  State<KeyBarEditor> createState() => _KeyBarEditorState();
}

/// 每个副本有独立身份，重复按钮与跨排拖动不会相互混淆。
class _ButtonEntry {
  _ButtonEntry(this.id);
  final String id;
}

class _DraggedButton {
  const _DraggedButton(this.id, [this.entry]);
  final String id;
  final _ButtonEntry? entry;
}

class _KeyBarEditorState extends State<KeyBarEditor> {
  late List<List<_ButtonEntry>> _rows = [
    for (var i = 0; i < 2; i++)
      [
        for (final id in widget.initialRows.elementAtOrNull(i) ?? <String>[])
          _ButtonEntry(id),
      ],
  ];
  final _scroll = ScrollController();
  final _previewKey = GlobalKey();
  int _row = 0;
  bool _saving = false;
  String? _error;
  _ButtonEntry? _selected;
  _DraggedButton? _dragging;
  Offset? _dragPosition;
  Timer? _scrollTimer;

  @override
  void dispose() {
    _scrollTimer?.cancel();
    _scroll.dispose();
    super.dispose();
  }

  Future<void> _save() async {
    setState(() {
      _saving = true;
      _error = null;
    });
    try {
      await widget.onSave([
        for (final row in _rows) [for (final entry in row) entry.id],
      ]);
      if (mounted) Navigator.pop(context);
    } catch (_) {
      if (mounted) {
        setState(() {
          _saving = false;
          _error = '保存失败，请重试。';
        });
      }
    }
  }

  bool _canDrop(_DraggedButton data, int row) =>
      !_saving && (_rows[row].length < 24 || _rows[row].contains(data.entry));

  void _drop(_DraggedButton data, int row, int index) {
    if (!_canDrop(data, row)) return;
    setState(() {
      final entry = data.entry ?? _ButtonEntry(data.id);
      final oldIndex = _rows[row].indexOf(entry);
      if (oldIndex >= 0 && oldIndex < index) index--;
      for (final source in _rows) {
        source.remove(entry);
      }
      // 空列是可保存的占位，不把用户指定的列位置收缩成行尾。
      while (_rows[row].length < index) {
        _rows[row].add(_ButtonEntry('spacer'));
      }
      _rows[row].insert(index.clamp(0, _rows[row].length), entry);
      _selected = entry;
      _row = row;
    });
  }

  void _delete(_ButtonEntry entry) {
    setState(() {
      for (final row in _rows) {
        row.remove(entry);
      }
      if (_selected == entry) _selected = null;
    });
  }

  void _startDrag(_DraggedButton data) {
    setState(() => _dragging = data);
    _scrollTimer?.cancel();
    // 长排在拖动靠近边缘时自动滚动，两排共用一个滚动位置。
    _scrollTimer = Timer.periodic(const Duration(milliseconds: 40), (_) {
      final box = _previewKey.currentContext?.findRenderObject() as RenderBox?;
      final position = _dragPosition;
      if (box == null || position == null || !_scroll.hasClients) return;
      final local = box.globalToLocal(position);
      if (local.dy < -20 || local.dy > box.size.height + 20) return;
      final delta = local.dx < 28
          ? -12.0
          : local.dx > box.size.width - 28
          ? 12.0
          : 0.0;
      if (delta == 0) return;
      _scroll.jumpTo(
        (_scroll.offset + delta).clamp(0.0, _scroll.position.maxScrollExtent),
      );
    });
  }

  void _endDrag() {
    _scrollTimer?.cancel();
    _dragPosition = null;
    if (mounted) setState(() => _dragging = null);
  }

  Widget _draggable(_DraggedButton data, Widget child) => Listener(
    onPointerMove: (event) => _dragPosition = event.position,
    child: Draggable<_DraggedButton>(
      data: data,
      maxSimultaneousDrags: _saving || _dragging != null ? 0 : 1,
      dragAnchorStrategy: pointerDragAnchorStrategy,
      feedback: Transform.translate(
        offset: const Offset(-keyBarCellWidth / 2, -keyBarCellHeight / 2),
        child: Material(
          color: Theme.of(context).colorScheme.surface,
          elevation: 6,
          child: _face(data.id, active: true),
        ),
      ),
      childWhenDragging: Opacity(opacity: 0.2, child: child),
      onDragStarted: () => _startDrag(data),
      onDragEnd: (_) => _endDrag(),
      child: child,
    ),
  );

  Widget _face(String id, {bool active = false}) {
    final button = keyBarButton(id);
    return KeyBarKeyFace(
      label: button?.label ?? id,
      displayLabel: button?.compactLabel ?? id,
      active: active,
    );
  }

  Future<void> _customText() async {
    final text = await showDialog<String>(
      context: context,
      builder: (_) => const _CustomTextDialog(),
    );
    if (!mounted || text == null || _rows[_row].length >= 24) return;
    _drop(_DraggedButton('text:$text'), _row, _rows[_row].length);
  }

  Widget _preview() {
    final columns = math.min(
      24,
      math.max(_rows[0].length, _rows[1].length) + 1,
    );
    return ColoredBox(
      color: Theme.of(context).colorScheme.surfaceContainer,
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Column(
            children: [
              for (var row = 0; row < 2; row++)
                GestureDetector(
                  onTap: _saving ? null : () => setState(() => _row = row),
                  child: KeyBarKeyFace(
                    label: row == 0 ? '第一排' : '第二排',
                    displayLabel: (row + 1).toString(),
                    active: _row == row,
                  ),
                ),
            ],
          ),
          Expanded(
            child: KeyBarGrid(
              key: _previewKey,
              controller: _scroll,
              rows: [
                for (var row = 0; row < 2; row++)
                  [
                    for (var col = 0; col < columns; col++)
                      _previewCell(row, col),
                  ],
              ],
            ),
          ),
        ],
      ),
    );
  }

  Widget _previewCell(int row, int col) {
    final entries = _rows[row];
    final entry = col < entries.length ? entries[col] : null;
    return _DropCell(
      key: ValueKey('preview-$row-$col'),
      canAccept: (data) => _canDrop(data, row),
      onDrop: (data, after) =>
          _drop(data, row, entry == null ? col : col + (after ? 1 : 0)),
      empty: entry == null,
      child: entry == null
          ? GestureDetector(
              behavior: HitTestBehavior.opaque,
              onTap: _saving ? null : () => setState(() => _row = row),
              child: KeyBarKeyFace(
                label: '添加到第${row + 1}排',
                icon: Icons.add,
                enabled: !_saving,
              ),
            )
          : _draggable(
              _DraggedButton(entry.id, entry),
              GestureDetector(
                behavior: HitTestBehavior.opaque,
                onTap: _saving
                    ? null
                    : () => setState(() {
                        _selected = entry;
                        _row = row;
                      }),
                child: _face(entry.id, active: _selected == entry),
              ),
            ),
    );
  }

  Widget _deleteTarget() => DragTarget<_DraggedButton>(
    onWillAcceptWithDetails: (details) =>
        !_saving && details.data.entry != null,
    onAcceptWithDetails: (details) => _delete(details.data.entry!),
    builder: (context, candidates, _) {
      final scheme = Theme.of(context).colorScheme;
      final hovering = candidates.isNotEmpty;
      return ColoredBox(
        key: const ValueKey('keybar-delete-target'),
        color: hovering ? scheme.errorContainer : scheme.surfaceContainer,
        child: SizedBox(
          height: 48,
          width: double.infinity,
          child: _selected != null && _dragging == null
              ? TextButton.icon(
                  onPressed: _saving ? null : () => _delete(_selected!),
                  icon: const Icon(Icons.delete_outline),
                  label: const Text('删除所选按钮'),
                )
              : Row(
                  mainAxisAlignment: MainAxisAlignment.center,
                  children: [
                    Icon(Icons.delete_outline, color: scheme.onSurfaceVariant),
                    const SizedBox(width: 8),
                    Text(hovering ? '松开删除' : '拖到这里删除'),
                  ],
                ),
        ),
      );
    },
  );

  @override
  Widget build(BuildContext context) => PopScope(
    canPop: !_saving,
    child: Scaffold(
      appBar: AppBar(
        title: const Text('编辑功能按钮'),
        actions: [
          TextButton(
            onPressed: _saving || _dragging != null ? null : _save,
            child: Text(_saving ? '保存中…' : '保存'),
          ),
        ],
      ),
      body: SafeArea(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(16, 8, 16, 12),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  const Text('直接拖动按钮排序或移到另一排，排布与终端一致。'),
                  const SizedBox(height: 12),
                  _preview(),
                  const SizedBox(height: 8),
                  Row(
                    children: [
                      Expanded(
                        child: Text(
                          '点击下方按钮加到第${_row + 1}排，也可直接拖入。',
                          style: Theme.of(context).textTheme.bodySmall,
                        ),
                      ),
                      TextButton(
                        onPressed: _saving || _dragging != null
                            ? null
                            : () => setState(() {
                                _rows = [
                                  for (final row in defaultKeyBarRows)
                                    [for (final id in row) _ButtonEntry(id)],
                                ];
                                _selected = null;
                              }),
                        child: const Text('恢复默认'),
                      ),
                    ],
                  ),
                  if (_rows[_row].length >= 24)
                    const Text('这一排已满（24 个），可移到另一排或先删除按钮。'),
                  if (_error != null)
                    Text(
                      _error!,
                      style: TextStyle(
                        color: Theme.of(context).colorScheme.error,
                      ),
                    ),
                ],
              ),
            ),
            Expanded(
              child: SingleChildScrollView(
                padding: const EdgeInsets.fromLTRB(16, 0, 16, 16),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Row(
                      children: [
                        Expanded(
                          child: Text(
                            '添加按钮',
                            style: Theme.of(context).textTheme.titleSmall,
                          ),
                        ),
                        TextButton.icon(
                          onPressed: _saving || _rows[_row].length >= 24
                              ? null
                              : _customText,
                          icon: const Icon(Icons.text_fields, size: 18),
                          label: const Text('自定义文本'),
                        ),
                      ],
                    ),
                    Wrap(
                      spacing: 8,
                      runSpacing: 8,
                      children: [
                        for (final button in keyBarButtons)
                          _draggable(
                            _DraggedButton(button.id),
                            GestureDetector(
                              key: ValueKey('palette-${button.id}'),
                              behavior: HitTestBehavior.opaque,
                              onTap: _saving
                                  ? null
                                  : () => _drop(
                                      _DraggedButton(button.id),
                                      _row,
                                      _rows[_row].length,
                                    ),
                              child: _face(button.id),
                            ),
                          ),
                      ],
                    ),
                  ],
                ),
              ),
            ),
            _deleteTarget(),
          ],
        ),
      ),
    ),
  );
}

/// 鼠标落在键帽前后半区时分别插到前面或后面，细线标出实际插入位置。
class _DropCell extends StatefulWidget {
  final bool Function(_DraggedButton) canAccept;
  final void Function(_DraggedButton, bool after) onDrop;
  final bool empty;
  final Widget child;

  const _DropCell({
    super.key,
    required this.canAccept,
    required this.onDrop,
    required this.empty,
    required this.child,
  });

  @override
  State<_DropCell> createState() => _DropCellState();
}

class _DropCellState extends State<_DropCell> {
  bool _after = false;

  bool _isAfter(Offset global) {
    final box = context.findRenderObject() as RenderBox;
    return !widget.empty && box.globalToLocal(global).dx > box.size.width / 2;
  }

  @override
  Widget build(BuildContext context) => DragTarget<_DraggedButton>(
    onWillAcceptWithDetails: (details) => widget.canAccept(details.data),
    onMove: (details) {
      final after = _isAfter(details.offset);
      if (after != _after) setState(() => _after = after);
    },
    onAcceptWithDetails: (details) =>
        widget.onDrop(details.data, _isAfter(details.offset)),
    builder: (context, candidates, _) => SizedBox(
      width: keyBarCellWidth,
      height: keyBarCellHeight,
      child: Stack(
        children: [
          widget.child,
          if (candidates.isNotEmpty)
            Positioned(
              left: _after ? null : 0,
              right: _after ? 0 : null,
              top: 3,
              bottom: 3,
              child: ColoredBox(
                color: Theme.of(context).colorScheme.primary,
                child: const SizedBox(width: 2),
              ),
            ),
        ],
      ),
    ),
  );
}

class _CustomTextDialog extends StatefulWidget {
  const _CustomTextDialog();
  @override
  State<_CustomTextDialog> createState() => _CustomTextDialogState();
}

class _CustomTextDialogState extends State<_CustomTextDialog> {
  final _text = TextEditingController();
  @override
  void dispose() {
    _text.dispose();
    super.dispose();
  }

  bool get _valid =>
      _text.text.trim().isNotEmpty &&
      _text.text.runes.length <= 32 &&
      !_text.text.contains(RegExp(r'[\x00-\x1f\x7f-\x9f]'));
  @override
  Widget build(BuildContext context) => AlertDialog(
    title: const Text('自定义文本按钮'),
    content: TextField(
      controller: _text,
      autofocus: true,
      maxLength: 32,
      decoration: const InputDecoration(
        labelText: '输入文本',
        helperText: '不自动发送回车',
      ),
      onChanged: (_) => setState(() {}),
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.pop(context),
        child: const Text('取消'),
      ),
      FilledButton(
        onPressed: _valid ? () => Navigator.pop(context, _text.text) : null,
        child: const Text('添加'),
      ),
    ],
  );
}
