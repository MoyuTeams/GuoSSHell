import 'dart:async';
import 'dart:math' as math;

import 'package:fluent_ui/fluent_ui.dart' as f;
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';

import '../bindings/bindings.dart' hide listEquals;
import '../desktop/desktop_theme.dart';
import 'desktop_session_controller.dart';
import 'key_bar_layout.dart';
import 'key_bar_visuals.dart'
    show KeyBarGrid, keyBarCellWidth, keyBarCellHeight;

/// normal 键位条只提交 GUI 意图，修饰与待决权威在原会话 controller。
class DesktopTerminalKeyBar extends StatefulWidget {
  const DesktopTerminalKeyBar({
    super.key,
    required this.session,
    required this.rows,
    required this.onAction,
    required this.onEdit,
    required this.canCopy,
    this.extraListen,
  });
  final DesktopSessionController session;
  final List<List<String>> rows;
  final ValueChanged<String> onAction;
  final VoidCallback onEdit;
  final bool Function() canCopy;
  final Listenable? extraListen;

  @override
  State<DesktopTerminalKeyBar> createState() => _DesktopTerminalKeyBarState();
}

class _DesktopTerminalKeyBarState extends State<DesktopTerminalKeyBar> {
  Timer? _repeat;
  int _interaction = 0;
  @override
  void initState() {
    super.initState();
    _interaction = widget.session.guiInteractionRevision;
    widget.session.addListener(_sessionChanged);
  }

  void _stop() {
    _repeat?.cancel();
    _repeat = null;
  }

  void _sessionChanged() {
    if (_interaction != widget.session.guiInteractionRevision ||
        !widget.session.guiAvailable) {
      _stop();
    }
    _interaction = widget.session.guiInteractionRevision;
  }

  @override
  void didUpdateWidget(DesktopTerminalKeyBar oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.session != widget.session ||
        !listEquals(
          oldWidget.rows.expand((row) => row).toList(),
          widget.rows.expand((row) => row).toList(),
        ) ||
        !listEquals(
          oldWidget.rows.map((row) => row.length).toList(),
          widget.rows.map((row) => row.length).toList(),
        )) {
      _stop();
      oldWidget.session.removeListener(_sessionChanged);
      widget.session.addListener(_sessionChanged);
      final session = widget.session;
      scheduleMicrotask(() {
        if (mounted && widget.session == session) {
          session.clearGuiModifiers();
        }
      });
      _interaction = widget.session.guiInteractionRevision;
    }
  }

  @override
  void dispose() {
    _stop();
    widget.session.removeListener(_sessionChanged);
    super.dispose();
  }

  void _send(KeyBarButton button) {
    if (!widget.session.guiAvailable) {
      _stop();
      return;
    }
    final input = guiInputForButton(button);
    if (input == null || widget.session.guiBusy) return;
    unawaited(widget.session.guiInput(input, button: button.ctrl ? 1 : 0));
  }

  double _rowHeight(BuildContext context) {
    final caption = f.FluentTheme.of(context).typography.caption!;
    return math.max(
      keyBarCellHeight,
      MediaQuery.textScalerOf(context).scale(caption.fontSize!) *
              caption.height! *
              2 +
          DesktopSpacing.xs,
    );
  }

  Widget _button(KeyBarButton button, double rowHeight) {
    if (button.id == 'spacer') {
      return SizedBox(width: keyBarCellWidth, height: rowHeight);
    }
    final modifier = button.id == 'ctrl' || button.id == 'alt';
    final input = guiInputForButton(button);
    final action = !modifier && input == null;
    final available = action
        ? (button.id != 'copy' || widget.canCopy()) && !widget.session.closing
        : widget.session.guiAvailable;
    final status = modifier
        ? (widget.session.isModifierLocked(button.id)
              ? '锁定'
              : widget.session.isModifierLatched(button.id)
              ? '单次'
              : '关闭')
        : null;
    void activate() {
      if (modifier) {
        widget.session.tapModifier(button.id);
      } else if (action) {
        _stop();
        widget.onAction(button.id);
      } else {
        _send(button);
      }
    }

    final cap = SizedBox(
      width: keyBarCellWidth,
      height: rowHeight,
      child: f.Tooltip(
        message: status == null ? button.label : '${button.label}，$status',
        triggerMode: f.TooltipTriggerMode.manual,
        child: Semantics(
          button: true,
          enabled: available,
          label: status == null ? button.label : '${button.label}，$status',
          selected: modifier && status != '关闭',
          child: f.Button(
            onPressed: available ? activate : null,
            onLongPress: !available || action
                ? null
                : () {
                    if (modifier) {
                      widget.session.lockModifier(button.id);
                      return;
                    }
                    _send(button);
                    if (widget.session.guiAvailable) {
                      _repeat = Timer.periodic(
                        const Duration(milliseconds: 200),
                        (_) => _send(button),
                      );
                    }
                  },
            style: const f.ButtonStyle(
              padding: WidgetStatePropertyAll(EdgeInsets.zero),
            ),
            child: ExcludeSemantics(
              child: Text(
                status == null
                    ? button.compactLabel
                    : '${button.compactLabel}\n$status',
                maxLines: 2,
                style: f.FluentTheme.of(context).typography.caption,
              ),
            ),
          ),
        ),
      ),
    );
    return Listener(
      onPointerDown: (_) => _stop(),
      onPointerUp: (_) => _stop(),
      onPointerCancel: (_) => _stop(),
      child: cap,
    );
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: Listenable.merge([widget.session, widget.extraListen]),
    builder: (context, _) => ColoredBox(
      color: f.FluentTheme.of(context).scaffoldBackgroundColor,
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Expanded(
                child: KeyBarGrid(
                  rowHeight: _rowHeight(context),
                  rows: [
                    for (final row in widget.rows)
                      [
                        for (final id in row)
                          if (keyBarButton(id) case final button?)
                            _button(button, _rowHeight(context)),
                      ],
                  ],
                ),
              ),
              f.Tooltip(
                message: '编辑功能按钮',
                child: f.Button(
                  onPressed: () {
                    _stop();
                    widget.session.clearGuiModifiers();
                    widget.onEdit();
                  },
                  child: const Padding(
                    padding: EdgeInsets.all(DesktopSpacing.sm),
                    child: Icon(f.FluentIcons.edit),
                  ),
                ),
              ),
            ],
          ),
          Padding(
            padding: const EdgeInsets.all(DesktopSpacing.sm),
            child: Semantics(
              liveRegion: true,
              child: Text(
                widget.session.guiUnknown
                    ? '受理未知，已停重复，等待原请求'
                    : widget.session.guiBusy
                    ? '正在确认输入受理'
                    : widget.session.guiAdmission ==
                              NativeCommandAdmission.rejected &&
                          widget.session.guiAvailable
                    ? '输入未受理，修饰状态保留'
                    : widget.session.guiAvailable
                    ? '终端输入就绪'
                    : '终端输入未激活',
                style: f.FluentTheme.of(context).typography.body,
              ),
            ),
          ),
        ],
      ),
    ),
  );
}

GuiInput? guiInputForButton(KeyBarButton button) {
  if (button.text case final text?) return GuiInputGuiText(value: text);
  final GuiKey? key = switch (button.id) {
    'enter' => const GuiKeyEnter(),
    'escape' => const GuiKeyEscape(),
    'tab' => const GuiKeyTab(),
    'backspace' => const GuiKeyBackspace(),
    'up' => const GuiKeyArrowUp(),
    'down' => const GuiKeyArrowDown(),
    'left' => const GuiKeyArrowLeft(),
    'right' => const GuiKeyArrowRight(),
    'delete' => const GuiKeyDelete(),
    'home' => const GuiKeyHome(),
    'end' => const GuiKeyEnd(),
    'pageUp' => const GuiKeyPageUp(),
    'pageDown' => const GuiKeyPageDown(),
    'ctrlC' => const GuiKeyCtrlC(),
    'ctrlD' => const GuiKeyCtrlD(),
    'ctrlZ' => const GuiKeyCtrlZ(),
    'ctrlL' => const GuiKeyCtrlL(),
    _ => null,
  };
  if (key != null) return GuiInputGuiKey(value: key);
  final number = button.id.startsWith('f')
      ? int.tryParse(button.id.substring(1))
      : null;
  if (number != null && number >= 1 && number <= 12) {
    return GuiInputGuiKey(value: GuiKeyF(value: number));
  }
  return null;
}
