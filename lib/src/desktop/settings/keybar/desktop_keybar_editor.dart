import 'dart:async';
import 'dart:math' as math;

import 'package:fluent_ui/fluent_ui.dart';
import 'package:flutter/services.dart';

import '../../../terminal/key_bar_layout.dart';
import '../../desktop_theme.dart';
import 'keybar_draft.dart';
import 'keybar_writer.dart';

/// 父级持有 controller/draft/writer，页面拆装不承担保存生命周期。
class DesktopKeyBarEditor extends StatefulWidget {
  const DesktopKeyBarEditor({
    super.key,
    required this.controller,
    this.onEditingFocus,
    this.onEditorFocusChanged,
  });
  final KeyBarEditorController controller;

  /// true 完成后才打开输入窗口；父级可先撤销 native 输入 lease。
  /// false 表示输入窗口关闭，不等同于真实终端焦点已恢复。
  final Future<void> Function(bool active)? onEditingFocus;
  final ValueChanged<bool>? onEditorFocusChanged;

  @override
  State<DesktopKeyBarEditor> createState() => _DesktopKeyBarEditorState();
}

class _DesktopKeyBarEditorState extends State<DesktopKeyBarEditor> {
  final _scroll = ScrollController();
  final _preview = GlobalKey();
  final _editorFocus = FocusNode();
  KeyBarDrag? _dragging;
  Offset? _pointer;
  Timer? _edgeTimer;
  bool _dialogOpen = false;
  String? _inputIssue;
  KeyBarDraft get _draft => widget.controller.draft;

  @override
  void dispose() {
    _edgeTimer?.cancel();
    _scroll.dispose();
    _editorFocus.dispose();
    super.dispose();
  }

  void _startDrag(KeyBarDrag drag) {
    setState(() => _dragging = drag);
    _edgeTimer?.cancel();
    _edgeTimer = Timer.periodic(DesktopMotion.control, (_) {
      final box = _preview.currentContext?.findRenderObject() as RenderBox?;
      if (box == null || _pointer == null || !_scroll.hasClients) return;
      final point = box.globalToLocal(_pointer!);
      if (point.dy < 0 || point.dy > box.size.height) return;
      final delta = point.dx < DesktopSpacing.xxl
          ? -DesktopSpacing.md
          : point.dx > box.size.width - DesktopSpacing.xxl
          ? DesktopSpacing.md
          : 0.0;
      _scroll.jumpTo(
        (_scroll.offset + delta).clamp(0.0, _scroll.position.maxScrollExtent),
      );
    });
  }

  void _endDrag() {
    _edgeTimer?.cancel();
    _pointer = null;
    if (mounted) setState(() => _dragging = null);
  }

  Future<void> _textDialog([KeyBarEntry? entry]) async {
    if (_dialogOpen) return;
    _dialogOpen = true;
    var editingBoundary = false;
    try {
      await widget.onEditingFocus?.call(true);
      editingBoundary = true;
      if (!mounted) return;
      final text = await showDialog<String>(
        context: context,
        builder: (_) => _KeyBarTextDialog(
          initial: entry?.code.startsWith('text:') == true
              ? entry!.code.substring(5)
              : '',
        ),
      );
      if (!mounted || text == null) return;
      if (entry == null) {
        _draft.drop(
          KeyBarDrag.copy('text:$text'),
          _draft.activeRow,
          _draft.entries[_draft.activeRow].length,
        );
      } else {
        _draft.replaceText(entry, text);
      }
    } catch (_) {
      if (mounted) setState(() => _inputIssue = '无法进入独立输入焦点域。');
    } finally {
      _dialogOpen = false;
      if (editingBoundary) await widget.onEditingFocus?.call(false);
      if (mounted) _editorFocus.requestFocus();
    }
  }

  KeyEventResult _key(FocusNode node, KeyEvent event) {
    if (event is! KeyDownEvent || _dialogOpen) return KeyEventResult.ignored;
    final key = event.logicalKey;
    if (key == LogicalKeyboardKey.escape) {
      _draft.select(null, _draft.activeRow);
      return KeyEventResult.handled;
    }
    if (_draft.selected == null) return KeyEventResult.ignored;
    final movement = switch (key) {
      LogicalKeyboardKey.arrowLeft => (-1, 0),
      LogicalKeyboardKey.arrowRight => (1, 0),
      LogicalKeyboardKey.arrowUp => (0, -1),
      LogicalKeyboardKey.arrowDown => (0, 1),
      _ => null,
    };
    if (movement != null) {
      _draft.moveSelected(movement.$1, movement.$2);
      _revealSelected();
      return KeyEventResult.handled;
    }
    if (key == LogicalKeyboardKey.delete ||
        key == LogicalKeyboardKey.backspace) {
      _draft.deleteSelected();
      return KeyEventResult.handled;
    }
    if (key == LogicalKeyboardKey.enter &&
        _draft.selected!.code.startsWith('text:')) {
      unawaited(_textDialog(_draft.selected));
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  double _cellWidth(BuildContext context) =>
      DesktopSpacing.xxl *
      3 *
      MediaQuery.textScalerOf(context).scale(1).clamp(1.0, 2.0);

  void _revealSelected() {
    if (!_scroll.hasClients || _draft.selected == null) return;
    final index = _draft.entries[_draft.activeRow].indexOf(_draft.selected!);
    final x = index * (_cellWidth(context) + DesktopSpacing.xs);
    final end = x + _cellWidth(context);
    final viewport = _scroll.position.viewportDimension;
    if (x < _scroll.offset || end > _scroll.offset + viewport) {
      _scroll.jumpTo(
        (x < _scroll.offset ? x : end - viewport).clamp(
          0.0,
          _scroll.position.maxScrollExtent,
        ),
      );
    }
  }

  Widget _drag(KeyBarDrag drag, Widget child) => Listener(
    onPointerMove: (event) => _pointer = event.position,
    child: Draggable<KeyBarDrag>(
      data: drag,
      maxSimultaneousDrags: _dragging == null ? 1 : 0,
      dragAnchorStrategy: pointerDragAnchorStrategy,
      feedback: FluentTheme(
        data: FluentTheme.of(context),
        child: SizedBox(width: _cellWidth(context), child: _face(drag.code)),
      ),
      childWhenDragging: Opacity(opacity: 0.5, child: child),
      onDragStarted: () => _startDrag(drag),
      onDragEnd: (_) => _endDrag(),
      child: child,
    ),
  );

  Widget _face(String code) => Text(
    keyBarButton(code)?.label ?? code,
    maxLines: 1,
    overflow: TextOverflow.ellipsis,
    style: FluentTheme.of(context).typography.body,
  );

  Widget _cell(int row, int col) {
    final entries = _draft.entries[row];
    final entry = col < entries.length ? entries[col] : null;
    return _KeyBarDropCell(
      key: ValueKey('keybar-cell-$row-$col'),
      empty: entry == null,
      width: _cellWidth(context),
      canDrop: (drag, after) =>
          _draft.canDrop(drag, row, col + (after && entry != null ? 1 : 0)),
      onDrop: (drag, after) =>
          _draft.drop(drag, row, col + (after && entry != null ? 1 : 0)),
      child: entry == null
          ? _KeyBarButton(
              key: ValueKey('keybar-empty-$row-$col'),
              onPressed: () {
                _draft.select(null, row);
                _editorFocus.requestFocus();
              },
              child: Text('空列 ${col + 1}'),
            )
          : _drag(
              KeyBarDrag.move(entry),
              Focus(
                canRequestFocus: false,
                onFocusChange: (focused) {
                  if (focused && !identical(_draft.selected, entry)) {
                    _draft.select(entry, row);
                  }
                },
                child: Semantics(
                  selected: identical(_draft.selected, entry),
                  label: '第${row + 1}排第${col + 1}列',
                  child: Tooltip(
                    message: keyBarButton(entry.code)?.label ?? entry.code,
                    child: _KeyBarButton(
                      key: ObjectKey(entry),
                      style: ButtonStyle(
                        shape: WidgetStateProperty.resolveWith(
                          (states) => RoundedRectangleBorder(
                            borderRadius: BorderRadius.circular(
                              DesktopMetrics.radius,
                            ),
                            side: BorderSide(
                              width: identical(_draft.selected, entry)
                                  ? DesktopMetrics.focus
                                  : DesktopMetrics.border,
                              color: identical(_draft.selected, entry)
                                  ? FluentTheme.of(context).accentColor
                                  : MediaQuery.highContrastOf(context)
                                  ? DesktopColors.highContrastText
                                  : DesktopColors.border,
                            ),
                          ),
                        ),
                      ),
                      onPressed: () {
                        _draft.select(entry, row);
                        _editorFocus.requestFocus();
                        if (entry.code.startsWith('text:')) {
                          unawaited(_textDialog(entry));
                        }
                      },
                      child: _face(entry.code),
                    ),
                  ),
                ),
              ),
            ),
    );
  }

  Widget _previewRows() {
    final columns = math.min(
      24,
      math.max(_draft.entries[0].length, _draft.entries[1].length) + 1,
    );
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Wrap(
          spacing: DesktopSpacing.sm,
          runSpacing: DesktopSpacing.sm,
          children: [
            for (var row = 0; row < 2; row++)
              Semantics(
                button: true,
                toggled: _draft.activeRow == row,
                child: ToggleButton(
                  key: ValueKey('keybar-row-$row'),
                  checked: _draft.activeRow == row,
                  onChanged: (_) => _draft.select(null, row),
                  child: Text('第${row + 1}排（${_draft.entries[row].length}/24）'),
                ),
              ),
          ],
        ),
        const SizedBox(height: DesktopSpacing.sm),
        Scrollbar(
          controller: _scroll,
          child: SingleChildScrollView(
            key: _preview,
            controller: _scroll,
            scrollDirection: Axis.horizontal,
            padding: const EdgeInsets.only(bottom: DesktopSpacing.lg),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                for (var row = 0; row < 2; row++) ...[
                  if (row > 0) const SizedBox(height: DesktopSpacing.sm),
                  Row(
                    children: [
                      for (var col = 0; col < columns; col++) ...[
                        if (col > 0) const SizedBox(width: DesktopSpacing.xs),
                        _cell(row, col),
                      ],
                    ],
                  ),
                ],
              ],
            ),
          ),
        ),
      ],
    );
  }

  String _status() => switch (widget.controller.writer.state) {
    KeyBarWriteState.pending => '保存中；继续编辑不会改变已提交的排布。',
    KeyBarWriteState.quarantined => '保存结果尚未确定。草稿保留，等待旧请求结算后核实；请勿重发。',
    KeyBarWriteState.needsVerify => '旧回包已消耗，仍需真实来源核实。草稿可编辑，暂不可保存。',
    KeyBarWriteState.verifying => '正在核实后端排布，草稿保留。',
    KeyBarWriteState.ready => switch (widget.controller.outcome?.status) {
      KeyBarSaveStatus.rejected => '保存失败，完整草稿保留，可重试。',
      KeyBarSaveStatus.saved => _draft.dirty ? '已保存提交的排布；还有未保存修改。' : '键位条已保存。',
      _ => _draft.dirty ? '排布尚未保存。' : '排布没有未保存修改。',
    },
  };

  @override
  Widget build(BuildContext context) => AnimatedBuilder(
    animation: widget.controller,
    builder: (context, _) => Focus(
      focusNode: _editorFocus,
      skipTraversal: true,
      onFocusChange: widget.onEditorFocusChanged,
      onKeyEvent: _key,
      child: SingleChildScrollView(
        padding: EdgeInsets.all(
          MediaQuery.sizeOf(context).width < DesktopMetrics.narrowWidth
              ? DesktopSpacing.lg
              : DesktopSpacing.xl,
        ),
        child: Align(
          alignment: Alignment.topLeft,
          child: ConstrainedBox(
            constraints: const BoxConstraints(
              maxWidth: DesktopMetrics.contentWidth,
            ),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  '键位条',
                  style: FluentTheme.of(context).typography.titleLarge,
                ),
                const SizedBox(height: DesktopSpacing.sm),
                const Text(
                  '拖动排列或跨排移动。Tab 选择按钮，方向键移动，Delete 删除，Esc 取消选择，Enter 编辑文本。',
                ),
                const SizedBox(height: DesktopSpacing.lg),
                _previewRows(),
                const SizedBox(height: DesktopSpacing.sm),
                Wrap(
                  spacing: DesktopSpacing.sm,
                  runSpacing: DesktopSpacing.sm,
                  children: [
                    for (final (label, dx, dy) in [
                      ('向左移动', -1, 0),
                      ('向右移动', 1, 0),
                      ('向上移动', 0, -1),
                      ('向下移动', 0, 1),
                    ])
                      _KeyBarButton(
                        key: ValueKey('keybar-move-$dx-$dy'),
                        onPressed: _draft.selected == null
                            ? null
                            : () {
                                _draft.moveSelected(dx, dy);
                                _revealSelected();
                              },
                        child: Text(label),
                      ),
                    _KeyBarButton(
                      key: const ValueKey('keybar-edit-text'),
                      onPressed:
                          _draft.selected?.code.startsWith('text:') == true
                          ? () => _textDialog(_draft.selected)
                          : null,
                      child: const Text('编辑所选文本'),
                    ),
                  ],
                ),
                const SizedBox(height: DesktopSpacing.sm),
                DragTarget<KeyBarDrag>(
                  onWillAcceptWithDetails: (details) =>
                      details.data.entry != null,
                  onAcceptWithDetails: (details) {
                    _draft.select(details.data.entry, _draft.activeRow);
                    _draft.deleteSelected();
                  },
                  builder: (context, candidates, _) => _KeyBarButton(
                    key: const ValueKey('keybar-delete'),
                    onPressed: _draft.selected == null
                        ? null
                        : _draft.deleteSelected,
                    child: Text(
                      candidates.isNotEmpty ? '松开删除' : '删除所选按钮（也可拖入）',
                    ),
                  ),
                ),
                const SizedBox(height: DesktopSpacing.lg),
                Wrap(
                  spacing: DesktopSpacing.sm,
                  runSpacing: DesktopSpacing.sm,
                  children: [
                    _KeyBarButton(
                      key: const ValueKey('keybar-custom-text'),
                      onPressed: _draft.entries[_draft.activeRow].length >= 24
                          ? null
                          : () => _textDialog(),
                      child: const Text('自定义文本'),
                    ),
                    _KeyBarButton(
                      key: const ValueKey('keybar-defaults'),
                      onPressed: _draft.restoreDefaults,
                      child: const Text('恢复默认'),
                    ),
                  ],
                ),
                const SizedBox(height: DesktopSpacing.md),
                Wrap(
                  spacing: DesktopSpacing.sm,
                  runSpacing: DesktopSpacing.sm,
                  children: [
                    for (final button in keyBarButtons)
                      _drag(
                        KeyBarDrag.copy(button.id),
                        _KeyBarButton(
                          key: ValueKey('keybar-palette-${button.id}'),
                          onPressed:
                              _draft.entries[_draft.activeRow].length >= 24
                              ? null
                              : () => _draft.drop(
                                  KeyBarDrag.copy(button.id),
                                  _draft.activeRow,
                                  _draft.entries[_draft.activeRow].length,
                                ),
                          child: _face(button.id),
                        ),
                      ),
                  ],
                ),
                const SizedBox(height: DesktopSpacing.lg),
                Semantics(liveRegion: true, child: Text(_status())),
                if (_inputIssue != null) Text(_inputIssue!),
                const SizedBox(height: DesktopSpacing.sm),
                Wrap(
                  spacing: DesktopSpacing.sm,
                  runSpacing: DesktopSpacing.sm,
                  children: [
                    _KeyBarButton(
                      filled: true,
                      key: const ValueKey('keybar-save'),
                      onPressed: widget.controller.canSave && _dragging == null
                          ? () => widget.controller.save()
                          : null,
                      child: const Text('保存键位条'),
                    ),
                    _KeyBarButton(
                      key: const ValueKey('keybar-verify'),
                      onPressed:
                          widget.controller.writer.state ==
                                  KeyBarWriteState.needsVerify &&
                              widget.controller.writer.verifier != null
                          ? () => widget.controller.writer.verify()
                          : null,
                      child: const Text('核实已结算排布'),
                    ),
                  ],
                ),
              ],
            ),
          ),
        ),
      ),
    ),
  );
}

class _KeyBarDropCell extends StatefulWidget {
  const _KeyBarDropCell({
    super.key,
    required this.empty,
    required this.width,
    required this.canDrop,
    required this.onDrop,
    required this.child,
  });
  final bool empty;
  final double width;
  final bool Function(KeyBarDrag drag, bool after) canDrop;
  final void Function(KeyBarDrag drag, bool after) onDrop;
  final Widget child;

  @override
  State<_KeyBarDropCell> createState() => _KeyBarDropCellState();
}

class _KeyBarDropCellState extends State<_KeyBarDropCell> {
  bool _after = false;
  bool _isAfter(Offset point) {
    final box = context.findRenderObject() as RenderBox;
    return !widget.empty && box.globalToLocal(point).dx > box.size.width / 2;
  }

  @override
  Widget build(BuildContext context) => DragTarget<KeyBarDrag>(
    onWillAcceptWithDetails: (details) =>
        widget.canDrop(details.data, _isAfter(details.offset)),
    onMove: (details) {
      final after = _isAfter(details.offset);
      if (_after != after) setState(() => _after = after);
    },
    onAcceptWithDetails: (details) =>
        widget.onDrop(details.data, _isAfter(details.offset)),
    builder: (context, candidates, _) => SizedBox(
      width: widget.width,
      height:
          MediaQuery.textScalerOf(context).scale(DesktopType.body) * 1.5 +
          DesktopSpacing.lg * 2,
      child: Stack(
        fit: StackFit.expand,
        children: [
          widget.child,
          if (candidates.isNotEmpty)
            Positioned(
              left: _after ? null : 0,
              right: _after ? 0 : null,
              top: DesktopSpacing.xs,
              bottom: DesktopSpacing.xs,
              child: ColoredBox(
                color: FluentTheme.of(context).accentColor,
                child: const SizedBox(width: DesktopMetrics.focus),
              ),
            ),
        ],
      ),
    ),
  );
}

class _KeyBarTextDialog extends StatefulWidget {
  const _KeyBarTextDialog({required this.initial});
  final String initial;
  @override
  State<_KeyBarTextDialog> createState() => _KeyBarTextDialogState();
}

class _KeyBarTextDialogState extends State<_KeyBarTextDialog> {
  late final _text = TextEditingController(text: widget.initial);
  @override
  void dispose() {
    _text.dispose();
    super.dispose();
  }

  void _confirm() {
    if (validKeyBarText(_text.text)) Navigator.pop(context, _text.text);
  }

  @override
  Widget build(BuildContext context) => ContentDialog(
    title: const Text('自定义文本按钮'),
    content: SingleChildScrollView(
      child: InfoLabel(
        label: '文本（最多 32 个 Unicode 字符，不允许控制字符）',
        child: TextBox(
          key: const ValueKey('keybar-text-input'),
          controller: _text,
          autofocus: true,
          onChanged: (_) => setState(() {}),
          onSubmitted: (_) => _confirm(),
        ),
      ),
    ),
    actions: [
      _KeyBarButton(
        key: const ValueKey('keybar-text-cancel'),
        onPressed: () => Navigator.pop(context),
        child: const Text('取消'),
      ),
      _KeyBarButton(
        filled: true,
        key: const ValueKey('keybar-text-confirm'),
        onPressed: validKeyBarText(_text.text) ? _confirm : null,
        child: const Text('确定'),
      ),
    ],
  );
}

/// Fluent 的操作控件补充 button role，保留原生焦点、状态与激活动作。
class _KeyBarButton extends StatelessWidget {
  const _KeyBarButton({
    super.key,
    required this.onPressed,
    required this.child,
    this.style,
    this.filled = false,
  });
  final VoidCallback? onPressed;
  final Widget child;
  final ButtonStyle? style;
  final bool filled;

  @override
  Widget build(BuildContext context) => MergeSemantics(
    child: Semantics(
      button: true,
      enabled: onPressed != null,
      child: filled
          ? FilledButton(onPressed: onPressed, style: style, child: child)
          : Button(onPressed: onPressed, style: style, child: child),
    ),
  );
}
