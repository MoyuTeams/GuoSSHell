import 'package:flutter/material.dart';

/// 两个汉字的文字区域加留白；各类按键与编辑入口占相同列宽。
const keyBarCellWidth = 40.0;
const keyBarCellHeight = 40.0;

/// 编辑预览和终端共用列宽与滚动方式，两排始终对齐，不自动换行。
class KeyBarGrid extends StatelessWidget {
  final List<List<Widget>> rows;
  final ScrollController? controller;

  const KeyBarGrid({super.key, required this.rows, this.controller});

  @override
  Widget build(BuildContext context) => SingleChildScrollView(
    controller: controller,
    scrollDirection: Axis.horizontal,
    child: Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        for (final row in rows)
          SizedBox(
            height: keyBarCellHeight,
            child: Row(mainAxisSize: MainAxisSize.min, children: row),
          ),
      ],
    ),
  );
}

/// 平直无边框的键帽。长标签截短显示，完整名称保留在提示与语义中。
class KeyBarKeyFace extends StatelessWidget {
  final String label;
  final String? displayLabel;
  final IconData? icon;
  final bool active;
  final bool locked;
  final bool enabled;

  const KeyBarKeyFace({
    super.key,
    required this.label,
    this.displayLabel,
    this.icon,
    this.active = false,
    this.locked = false,
    this.enabled = true,
  });

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final foreground = !enabled
        ? scheme.onSurface.withValues(alpha: 0.35)
        : locked
        ? scheme.onPrimary
        : scheme.onSurface;
    return Tooltip(
      message: label,
      // 触摸长按留给终端重复键和修饰键锁定；指针悬停仍可查看名称。
      triggerMode: TooltipTriggerMode.manual,
      child: Semantics(
        label: locked ? '$label，已锁定' : label,
        button: true,
        enabled: enabled,
        selected: active || locked,
        child: ExcludeSemantics(
          child: Container(
            width: keyBarCellWidth,
            height: keyBarCellHeight,
            color: locked
                ? scheme.primary
                : active
                ? scheme.primaryContainer
                : Colors.transparent,
            child: Stack(
              alignment: Alignment.center,
              children: [
                if (icon != null)
                  Icon(icon, size: 18, color: foreground)
                else
                  SizedBox(
                    width: 26,
                    child: Text(
                      displayLabel ?? label,
                      textAlign: TextAlign.center,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 13, color: foreground),
                    ),
                  ),
                if (locked)
                  Positioned(
                    right: 2,
                    top: 2,
                    child: Icon(Icons.lock, size: 8, color: foreground),
                  ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}
