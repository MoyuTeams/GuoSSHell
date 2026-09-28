import 'package:fluent_ui/fluent_ui.dart' as f;
import 'package:flutter/material.dart';

import 'windows_chrome.dart';

class DesktopIconButton extends StatelessWidget {
  const DesktopIconButton({
    super.key,
    required this.icon,
    required this.label,
    required this.onPressed,
  });
  final IconData icon;
  final String label;
  final VoidCallback? onPressed;

  @override
  Widget build(BuildContext context) => f.Tooltip(
    message: label,
    child: Semantics(
      label: label,
      button: true,
      child: f.IconButton(
        icon: Icon(icon, size: 16),
        onPressed: onPressed,
        style: const f.ButtonStyle(
          padding: WidgetStatePropertyAll(EdgeInsets.all(10)),
        ),
      ),
    ),
  );
}

Future<bool> desktopConfirm(
  BuildContext context,
  String title,
  String detail, {
  String action = '确定',
}) async =>
    await f.showDialog<bool>(
      context: context,
      builder: (context) => f.ContentDialog(
        title: Text(title),
        content: Text(detail),
        actions: [
          f.Button(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('取消'),
          ),
          f.FilledButton(
            onPressed: () => Navigator.pop(context, true),
            child: Text(action),
          ),
        ],
      ),
    ) ??
    false;

/// 辅助页面使用有界的独立导航栈，不卸载背后的会话与分屏。
Future<T?> showDesktopPage<T>(BuildContext context, Widget page) =>
    showDialog<T>(
      context: context,
      builder: (context) => Dialog(
        clipBehavior: Clip.antiAlias,
        child: SizedBox(
          width: 760,
          height: MediaQuery.sizeOf(context).height * 0.82,
          child: Navigator(
            observers: [
              _PanelNavigatorObserver(() => Navigator.of(context).pop()),
            ],
            onGenerateInitialRoutes: (_, _) => [
              MaterialPageRoute<void>(
                settings: const RouteSettings(name: 'panel-sentinel'),
                builder: (_) => const SizedBox.shrink(),
              ),
              MaterialPageRoute<void>(
                builder: (inner) => Column(
                  children: [
                    Align(
                      alignment: Alignment.centerRight,
                      child: DesktopIconButton(
                        icon: f.FluentIcons.chrome_close,
                        label: '关闭面板',
                        onPressed: () => Navigator.of(context).pop(),
                      ),
                    ),
                    Expanded(child: page),
                  ],
                ),
              ),
            ],
            onGenerateRoute: (_) => null,
          ),
        ),
      ),
    );

class _PanelNavigatorObserver extends NavigatorObserver {
  _PanelNavigatorObserver(this.close);
  final VoidCallback close;
  @override
  void didPop(Route<dynamic> route, Route<dynamic>? previousRoute) {
    if (previousRoute?.settings.name == 'panel-sentinel') {
      WidgetsBinding.instance.addPostFrameCallback((_) => close());
    }
  }
}

class ShortcutHint extends StatelessWidget {
  const ShortcutHint(this.label, this.keys, {super.key});
  final String label;
  final String keys;
  @override
  Widget build(BuildContext context) => SizedBox(
    width: 260,
    child: Padding(
      padding: const EdgeInsets.symmetric(vertical: 10),
      child: Row(
        children: [
          Expanded(
            child: Text(
              label,
              style: const TextStyle(color: desktopMuted, fontSize: 12),
            ),
          ),
          DecoratedBox(
            decoration: BoxDecoration(
              color: Colors.white.withValues(alpha: 0.04),
              borderRadius: BorderRadius.circular(4),
              border: Border.all(color: Colors.white12),
            ),
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
              child: Text(keys, style: const TextStyle(fontSize: 11)),
            ),
          ),
        ],
      ),
    ),
  );
}
