import 'dart:math' as math;

import 'package:flutter/cupertino.dart' show cupertinoTextSelectionHandleControls;
import 'package:flutter/foundation.dart' show defaultTargetPlatform;
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart'
    show
        TextSelectionControls,
        kMinInteractiveDimension,
        materialTextSelectionHandleControls;
import 'package:flutter/widgets.dart';

import 'package:terminal_view/src/core/buffer/cell_offset.dart';
import 'package:terminal_view/src/terminal_surface.dart';
import 'package:terminal_view/src/ui/controller.dart';
import 'package:terminal_view/src/ui/render.dart';

/// 两个选区手柄，画在终端上方（widget 层）。
///
/// 手柄本身复用 Flutter 官方控件（`materialTextSelectionHandleControls` /
/// `cupertinoTextSelectionHandleControls` 的 `buildHandle`），位置由
/// [RenderTerminal] 的格子几何算出——和选区高亮同一个坐标来源。
///
/// 拖动只改「含头含尾」的选区端点，交给 [RenderTerminal.selectInclusiveRange]
/// 上报；越过对端不特殊处理（引擎会规范化）。手柄是覆盖在上层的独立手势，
/// 不和终端自己的长按/拖动手势抢。
class TerminalSelectionHandles extends StatefulWidget {
  const TerminalSelectionHandles({
    super.key,
    required this.controller,
    required this.terminal,
    required this.renderTerminal,
    this.controls,
  });

  final TerminalController controller;

  final TerminalSurface terminal;

  /// 当前的 [RenderTerminal]（还没布局出来时为 null）。
  final RenderTerminal? Function() renderTerminal;

  /// 覆盖默认的平台手柄控件（测试用）。
  final TextSelectionControls? controls;

  @override
  State<TerminalSelectionHandles> createState() =>
      _TerminalSelectionHandlesState();
}

class _TerminalSelectionHandlesState extends State<TerminalSelectionHandles> {
  // 拖动中的「含头含尾」选区，在下按时定格：只有被拖的那一端跟着走。
  CellOffset? _dragFirst;
  CellOffset? _dragLast;

  @override
  void initState() {
    super.initState();
    widget.controller.addListener(_onChanged);
    widget.terminal.addListener(_onChanged);
  }

  @override
  void didUpdateWidget(TerminalSelectionHandles oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.controller != widget.controller) {
      oldWidget.controller.removeListener(_onChanged);
      widget.controller.addListener(_onChanged);
    }
    if (oldWidget.terminal != widget.terminal) {
      oldWidget.terminal.removeListener(_onChanged);
      widget.terminal.addListener(_onChanged);
    }
  }

  @override
  void dispose() {
    widget.controller.removeListener(_onChanged);
    widget.terminal.removeListener(_onChanged);
    super.dispose();
  }

  void _onChanged() {
    if (mounted) setState(() {});
  }

  TextSelectionControls get _controls =>
      widget.controls ??
      (defaultTargetPlatform == TargetPlatform.iOS
          ? cupertinoTextSelectionHandleControls
          : materialTextSelectionHandleControls);

  /// 末端是排他的；换成「含头含尾」里最后一个格子。跨行（末列 0）时退到
  /// 上一行最后一列——软换行的行一定是整行满的，所以那是正确的前一格。
  CellOffset _lastInclusive(CellOffset end) {
    if (end.x > 0) return CellOffset(end.x - 1, end.y);
    return CellOffset(widget.terminal.viewWidth - 1, end.y - 1);
  }

  CellOffset _cellAt(RenderTerminal render, Offset globalPosition) {
    return render.getCellOffset(render.globalToLocal(globalPosition));
  }

  void _startDrag({required bool isStart, required Offset globalPosition}) {
    final selection = widget.controller.selection;
    final render = widget.renderTerminal();
    if (selection == null || render == null) return;
    final normalized = selection.normalized;
    _dragFirst = normalized.begin;
    _dragLast = _lastInclusive(normalized.end);
    _dragIsStart = isStart;
  }

  bool _dragIsStart = false;

  void _updateDrag(Offset globalPosition) {
    final render = widget.renderTerminal();
    final first = _dragFirst;
    final last = _dragLast;
    if (render == null || first == null || last == null) return;
    final cell = _cellAt(render, globalPosition);
    if (_dragIsStart) {
      _dragFirst = cell;
    } else {
      _dragLast = cell;
    }
    render.selectInclusiveRange(_dragFirst!, _dragLast!);
  }

  void _endDrag() {
    _dragFirst = null;
    _dragLast = null;
  }

  Offset? _lastStartAnchor;
  Offset? _lastEndAnchor;

  /// 两个手柄的锚点（本 widget 坐标系）：起点挂在首个选中格的下边、终点挂在
  /// 最后一个选中格的下边——与 Flutter 文本选区同款（`getEndpointsForSelection`
  /// 的 start=首格左下、end=末格右下）。画不出来时返回 null。
  (Offset, Offset)? _computeAnchors() {
    final selection = widget.controller.selection;
    if (selection == null) return null;
    final normalized = selection.normalized;
    if (normalized.isCollapsed) return null;
    final render = widget.renderTerminal();
    final self = context.findRenderObject() as RenderBox?;
    if (render == null || self == null) return null;

    final cellHeight = render.cellSize.height;
    final startAnchor = self.globalToLocal(
      render.localToGlobal(render.getOffset(normalized.begin) + Offset(0, cellHeight)),
    );
    final endAnchor = self.globalToLocal(
      render.localToGlobal(render.getOffset(normalized.end) + Offset(0, cellHeight)),
    );
    return (startAnchor, endAnchor);
  }

  @override
  Widget build(BuildContext context) {
    final anchors = _computeAnchors();
    if (anchors == null) return const SizedBox.shrink();
    final (startAnchor, endAnchor) = anchors;
    _lastStartAnchor = startAnchor;
    _lastEndAnchor = endAnchor;

    final lineHeight = widget.renderTerminal()!.lineHeight;

    // build 读到的是上一次布局的几何；布局走完再校一遍，锚点变了就重排，
    // 保证手柄贴住最新格子（键盘弹出那一帧就靠这个追上）。
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      final fresh = _computeAnchors();
      if (fresh == null) return;
      if (fresh.$1 != _lastStartAnchor || fresh.$2 != _lastEndAnchor) {
        setState(() {});
      }
    });

    return Stack(
      clipBehavior: Clip.none,
      children: [
        _buildHandle(
          context,
          TextSelectionHandleType.left,
          startAnchor,
          lineHeight,
          onDragStart: (details) =>
              _startDrag(isStart: true, globalPosition: details.globalPosition),
        ),
        _buildHandle(
          context,
          TextSelectionHandleType.right,
          endAnchor,
          lineHeight,
          onDragStart: (details) =>
              _startDrag(isStart: false, globalPosition: details.globalPosition),
        ),
      ],
    );
  }

  Widget _buildHandle(
    BuildContext context,
    TextSelectionHandleType type,
    Offset anchor,
    double lineHeight, {
    required GestureDragStartCallback onDragStart,
  }) {
    final controls = _controls;
    final handleSize = controls.getHandleSize(lineHeight);
    final handleAnchor = controls.getHandleAnchor(type, lineHeight);

    // 命中区域至少一个最小交互尺寸（官方也是这么放宽的）。
    final interactiveSize = Size(
      math.max(handleSize.width, kMinInteractiveDimension),
      math.max(handleSize.height, kMinInteractiveDimension),
    );
    final padding = Offset(
      (interactiveSize.width - handleSize.width) / 2,
      (interactiveSize.height - handleSize.height) / 2,
    );

    return Positioned(
      left: anchor.dx - handleAnchor.dx - padding.dx,
      top: anchor.dy - handleAnchor.dy - padding.dy,
      width: interactiveSize.width,
      height: interactiveSize.height,
      child: RawGestureDetector(
        // opaque：手柄这块小区域独占触摸，下面的终端手势不再参与——
        // 否则 tap-down 会把正在拖的选区清掉（官方场景没有这层冲突，
        // 所以它用 translucent，我们不能照抄）。
        behavior: HitTestBehavior.opaque,
        gestures: <Type, GestureRecognizerFactory>{
          PanGestureRecognizer:
              GestureRecognizerFactoryWithHandlers<PanGestureRecognizer>(
            () => PanGestureRecognizer(
              debugOwner: this,
              supportedDevices: const {
                PointerDeviceKind.touch,
                PointerDeviceKind.stylus,
                PointerDeviceKind.unknown,
              },
            ),
            (PanGestureRecognizer instance) {
              instance
                ..dragStartBehavior = DragStartBehavior.down
                ..onStart = onDragStart
                ..onUpdate = (details) {
                  _updateDrag(details.globalPosition);
                }
                ..onEnd = (details) {
                  _endDrag();
                }
                ..onCancel = _endDrag;
            },
          ),
        },
        child: Align(
          alignment: Alignment.topLeft,
          child: Padding(
            // 外层 Positioned 减掉的 padding 在这里加回来，手柄的锚点
            // 才正好落在选区端点上（官方 _SelectionHandleOverlay 同款）。
            padding: EdgeInsets.fromLTRB(padding.dx, padding.dy, padding.dx, padding.dy),
            child: controls.buildHandle(context, type, lineHeight),
          ),
        ),
      ),
    );
  }
}
