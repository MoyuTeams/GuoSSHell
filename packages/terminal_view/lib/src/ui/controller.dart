import 'package:flutter/material.dart';
import 'package:meta/meta.dart';
import 'package:terminal_view/src/base/disposable.dart';
import 'package:terminal_view/src/core/buffer/cell_offset.dart';
import 'package:terminal_view/src/core/buffer/line.dart';
import 'package:terminal_view/src/core/buffer/range.dart';
import 'package:terminal_view/src/core/buffer/range_block.dart';
import 'package:terminal_view/src/core/buffer/range_line.dart';
import 'package:terminal_view/src/ui/pointer_input.dart';
import 'package:terminal_view/src/ui/selection_mode.dart';

class TerminalController with ChangeNotifier {
  TerminalController({
    SelectionMode selectionMode = SelectionMode.line,
    PointerInputs pointerInputs = const PointerInputs({PointerInput.tap}),
    bool suspendPointerInput = false,
  })  : _selectionMode = selectionMode,
        _pointerInputs = pointerInputs,
        _suspendPointerInputs = suspendPointerInput;

  CellAnchor? _selectionBase;
  CellAnchor? _selectionExtent;

  /// 纯坐标选区（M2a 起用）。选区权威在上层（我们的引擎），fork 只拿坐标画。
  /// 和 [_selectionBase]/[_selectionExtent] 二选一：有外部坐标就用它，
  /// 否则退回锚点（fork 单独用时没有引擎）。
  CellOffset? _externalBegin;
  CellOffset? _externalEnd;

  /// 选区变化时上报「意图」（begin/end；清除时两者为 null）。上层把它转给
  /// 自己的选区权威，再把规范化后的结果喂回 [setExternalSelection]。
  /// 没装这个回调 = 单独使用 fork，走原来的锚点路径。
  void Function(CellOffset? begin, CellOffset? end)? onSelectionIntent;

  SelectionMode get selectionMode => _selectionMode;
  SelectionMode _selectionMode;

  /// The set of pointer events which will be used as mouse input for the terminal.
  PointerInputs get pointerInput => _pointerInputs;
  PointerInputs _pointerInputs;

  /// True if sending pointer events to the terminal is suspended.
  bool get suspendedPointerInputs => _suspendPointerInputs;
  bool _suspendPointerInputs;

  List<TerminalHighlight> get highlights => _highlights;
  final _highlights = <TerminalHighlight>[];

  BufferRange? get selection {
    final externalBegin = _externalBegin;
    final externalEnd = _externalEnd;
    if (externalBegin != null && externalEnd != null) {
      return _createRange(externalBegin, externalEnd);
    }

    final base = _selectionBase;
    final extent = _selectionExtent;

    if (base == null || extent == null) {
      return null;
    }

    if (!base.attached || !extent.attached) {
      return null;
    }

    return _createRange(base.offset, extent.offset);
  }

  /// 喂入纯坐标选区（上层选区权威的回显）。端点角色（begin/end）原样保留：
  /// 拖耳朵越过对端时，靠这个角色不排序才不会乱。两者都 null = 清除。
  void setExternalSelection(CellOffset? begin, CellOffset? end) {
    final unchanged = _externalBegin == begin &&
        _externalEnd == end &&
        !_anchorsPresent;
    if (unchanged) {
      return;
    }
    _disposeAnchors();
    _externalBegin = begin;
    _externalEnd = end;
    notifyListeners();
  }

  /// 乐观更新纯坐标选区 + 上报意图：上层据此去问它自己的选区权威。
  /// 装上 [onSelectionIntent] 后，fork 的选词/拖选/清除都走这里。
  void requestSelection(CellOffset? begin, CellOffset? end) {
    setExternalSelection(begin, end);
    onSelectionIntent?.call(begin, end);
  }

  bool get _anchorsPresent => _selectionBase != null || _selectionExtent != null;

  void _disposeAnchors() {
    _selectionBase?.dispose();
    _selectionBase = null;
    _selectionExtent?.dispose();
    _selectionExtent = null;
  }

  /// Set selection on the terminal from [base] to [extent]. This method takes
  /// the ownership of [base] and [extent] and will dispose them when the
  /// selection is cleared or changed.
  void setSelection(CellAnchor base, CellAnchor extent, {SelectionMode? mode}) {
    _selectionBase?.dispose();
    _selectionBase = base;

    _selectionExtent?.dispose();
    _selectionExtent = extent;

    _externalBegin = null;
    _externalEnd = null;

    if (mode != null) {
      _selectionMode = mode;
    }

    notifyListeners();
  }

  BufferRange _createRange(CellOffset begin, CellOffset end) {
    switch (selectionMode) {
      case SelectionMode.line:
        return BufferRangeLine(begin, end);
      case SelectionMode.block:
        return BufferRangeBlock(begin, end);
    }
  }

  /// Controls how the terminal behaves when the user selects a range of text.
  /// The default is [SelectionMode.line]. Setting this to [SelectionMode.block]
  /// enables block selection mode.
  void setSelectionMode(SelectionMode newSelectionMode) {
    // If the new mode is the same as the old mode,
    // nothing has to be changed.
    if (_selectionMode == newSelectionMode) {
      return;
    }
    // Set the new mode.
    _selectionMode = newSelectionMode;
    notifyListeners();
  }

  /// Clears the current selection. 有意图回调时同时上报清除（上层要通知
  /// 它的选区权威，否则引擎那边的高亮不会被清掉）。
  void clearSelection() {
    final had = selection != null;
    _disposeAnchors();
    _externalBegin = null;
    _externalEnd = null;
    notifyListeners();
    if (had) {
      onSelectionIntent?.call(null, null);
    }
  }

  // Select which type of pointer events are send to the terminal.
  void setPointerInputs(PointerInputs pointerInput) {
    _pointerInputs = pointerInput;
    notifyListeners();
  }

  // Toggle sending pointer events to the terminal.
  void setSuspendPointerInput(bool suspend) {
    _suspendPointerInputs = suspend;
    notifyListeners();
  }

  // Returns true if this type of PointerInput should be send to the Terminal.
  @internal
  bool shouldSendPointerInput(PointerInput pointerInput) {
    // Always return false if pointer input is suspended.
    return _suspendPointerInputs
        ? false
        : _pointerInputs.inputs.contains(pointerInput);
  }

  /// Creates a new highlight on the terminal from [p1] to [p2] with the given
  /// [color]. The highlight will be removed when the returned object is
  /// disposed.
  TerminalHighlight highlight({
    required CellAnchor p1,
    required CellAnchor p2,
    required Color color,
  }) {
    final highlight = TerminalHighlight(
      this,
      p1: p1,
      p2: p2,
      color: color,
    );

    _highlights.add(highlight);
    notifyListeners();

    highlight.registerCallback(() {
      _highlights.remove(highlight);
      notifyListeners();
    });

    return highlight;
  }
}

class TerminalHighlight with Disposable {
  final TerminalController owner;

  final CellAnchor p1;

  final CellAnchor p2;

  final Color color;

  TerminalHighlight(
    this.owner, {
    required this.p1,
    required this.p2,
    required this.color,
  });

  /// Returns the range of the highlight. May be null if the anchors that
  /// define the highlight are not attached to the terminal.
  BufferRange? get range {
    if (!p1.attached || !p2.attached) {
      return null;
    }
    return BufferRangeLine(p1.offset, p2.offset);
  }
}
