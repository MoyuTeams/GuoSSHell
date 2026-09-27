import 'dart:async';

import 'package:flutter/material.dart';

import 'frame_terminal.dart';
import 'key_bar_layout.dart';
import 'key_bar_visuals.dart';

/// 可编辑的双排键位条。
///
/// * 修饰键（Ctrl/Alt）：点按挂住一次、长按锁定、锁定后再点解除——
///   状态存于 [FrameTerminal]，软键盘的下一个按键同样带得上修饰键。
/// * 其余按键：短按抬起发一次；长按（Flutter 手势识别器默认阈值）触发时
///   先发一个、之后按 [_keyRepeatInterval] 自动重复，抬起即停。
/// * 复制/粘贴：动作键，不参与自动重复；复制键在无选区时置灰
///   （[canCopy] 由页面的 TerminalController 驱动，[extraListen] 带它重建）。
/// * 断开：关掉活动窗格（连着时先确认）。
/// 编辑入口始终保留，即使用户移除了所有按钮。
class TerminalKeyBar extends StatefulWidget {
  final FrameTerminal terminal;
  final Listenable? extraListen;
  final bool Function() canCopy;
  final VoidCallback onCopy;
  final VoidCallback onPaste;
  final VoidCallback onToggleKeyboard;
  final VoidCallback onDisconnect;
  final List<List<String>> rows;
  final VoidCallback onEdit;
  final VoidCallback onZoomIn;
  final VoidCallback onZoomOut;
  final VoidCallback onZoomReset;

  const TerminalKeyBar({
    super.key,
    required this.terminal,
    required this.canCopy,
    required this.onCopy,
    required this.onPaste,
    required this.onToggleKeyboard,
    required this.onDisconnect,
    required this.rows,
    required this.onEdit,
    required this.onZoomIn,
    required this.onZoomOut,
    required this.onZoomReset,
    this.extraListen,
  });

  @override
  State<TerminalKeyBar> createState() => _TerminalKeyBarState();
}

class _TerminalKeyBarState extends State<TerminalKeyBar> {
  /// 长按自动重复的间隔。长按的触发阈值不在这里写死——
  /// 用 GestureDetector 长按识别器的默认时长。
  static const Duration _keyRepeatInterval = Duration(milliseconds: 200);

  Timer? _repeatTimer;
  String? _pressedId; // 手指按下的键帽（按下即高亮）
  String? _repeatingId; // 正在自动重复的键帽

  bool _isLit(String id) => _pressedId == id || _repeatingId == id;

  void _down(String id) {
    setState(() => _pressedId = id);
  }

  /// 短按抬起：发一次。长按获胜时 onTapUp 不会触发，走 [_endRepeat]。
  void _up(String id, void Function() send) {
    if (_pressedId != id) return;
    _pressedId = null;
    setState(() {});
    send();
  }

  /// 长按触发：立即发一个，之后按 [_keyRepeatInterval] 重复。
  void _longPress(String id, void Function() send) {
    if (_repeatingId == id) return;
    _repeatingId = id;
    setState(() {});
    send();
    _repeatTimer = Timer.periodic(_keyRepeatInterval, (_) => send());
  }

  /// 抬起 / 取消：停止重复。
  void _endRepeat(String id) {
    if (_repeatingId != id && _pressedId != id) return;
    _repeatTimer?.cancel();
    _repeatTimer = null;
    _repeatingId = null;
    _pressedId = null;
    setState(() {});
  }

  @override
  void didUpdateWidget(covariant TerminalKeyBar oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.rows != widget.rows ||
        oldWidget.terminal != widget.terminal) {
      _repeatTimer?.cancel();
      _repeatTimer = null;
      _pressedId = null;
      _repeatingId = null;
    }
  }

  @override
  void dispose() {
    _repeatTimer?.cancel();
    super.dispose();
  }

  void _edit() {
    _repeatTimer?.cancel();
    _repeatTimer = null;
    _pressedId = null;
    _repeatingId = null;
    widget.terminal.clearModifiers();
    widget.onEdit();
  }

  @override
  Widget build(BuildContext context) => ColoredBox(
    color: Theme.of(context).colorScheme.surface,
    child: SafeArea(
      top: false,
      child: ListenableBuilder(
        listenable: Listenable.merge([widget.terminal, widget.extraListen]),
        builder: (context, _) => Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Expanded(
              child: KeyBarGrid(
                rows: [
                  for (var row = 0; row < widget.rows.length; row++)
                    [
                      for (var col = 0; col < widget.rows[row].length; col++)
                        if (keyBarButton(widget.rows[row][col])
                            case final button?)
                          _buttonCap(button, '$row:$col'),
                    ],
                ],
              ),
            ),
            GestureDetector(
              behavior: HitTestBehavior.opaque,
              onTap: _edit,
              child: const KeyBarKeyFace(label: '编辑功能按钮', icon: Icons.tune),
            ),
          ],
        ),
      ),
    ),
  );

  Widget _buttonCap(KeyBarButton button, String position) {
    if (button.id == 'spacer') {
      return const SizedBox(width: keyBarCellWidth, height: keyBarCellHeight);
    }
    final modifier = button.id == 'ctrl' || button.id == 'alt';
    if (modifier) {
      return GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: () => widget.terminal.tapModifier(button.id),
        onLongPressStart: (_) => widget.terminal.lockModifier(button.id),
        child: KeyBarKeyFace(
          label: button.label,
          displayLabel: button.compactLabel,
          active: widget.terminal.isModifierLatched(button.id),
          locked: widget.terminal.isModifierLocked(button.id),
        ),
      );
    }
    final action = switch (button.id) {
      'keyboard' => widget.onToggleKeyboard,
      'disconnect' => widget.onDisconnect,
      'copy' => widget.canCopy() ? widget.onCopy : null,
      'paste' => widget.onPaste,
      'zoomIn' => widget.onZoomIn,
      'zoomOut' => widget.onZoomOut,
      'zoomReset' => widget.onZoomReset,
      _ => null,
    };
    if (button.key == null && button.text == null) {
      return GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: action,
        child: KeyBarKeyFace(
          label: button.label,
          displayLabel: button.compactLabel,
          enabled: action != null,
        ),
      );
    }
    void send() {
      if (button.key case final key?) {
        widget.terminal.keyInput(key, ctrl: button.ctrl);
      } else {
        widget.terminal.textInput(button.text!);
      }
    }

    // 位置标识区分同名按钮，按住一个时不会点亮其他副本。
    return GestureDetector(
      behavior: HitTestBehavior.opaque,
      onTapDown: (_) => _down(position),
      onTapUp: (_) => _up(position, send),
      onTapCancel: () => _endRepeat(position),
      onLongPressStart: (_) => _longPress(position, send),
      onLongPressEnd: (_) => _endRepeat(position),
      onLongPressCancel: () => _endRepeat(position),
      child: KeyBarKeyFace(
        label: button.label,
        displayLabel: button.compactLabel,
        active: _isLit(position),
      ),
    );
  }
}
