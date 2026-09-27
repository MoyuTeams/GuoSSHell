import 'package:fluent_ui/fluent_ui.dart' as fluent;
import 'package:flutter/material.dart' as material;

import '../catalog/connection_list_page.dart';
import '../terminal/session_target.dart';
import '../workspace/workspace_page.dart';
import 'desktop_theme.dart';

/// 兼容路径有独立 Material Navigator；从 Fluent 壳退出不会卸载启动订阅。
class LegacyMaterialSurface extends material.StatefulWidget {
  final SessionTarget? initialTarget;
  final material.Widget? homeOverride;

  const LegacyMaterialSurface({
    super.key,
    this.initialTarget,
    this.homeOverride,
  });

  @override
  material.State<LegacyMaterialSurface> createState() =>
      _LegacyMaterialSurfaceState();
}

class _LegacyMaterialSurfaceState
    extends material.State<LegacyMaterialSurface> {
  final _navigatorKey = material.GlobalKey<material.NavigatorState>();
  bool _innerCanPop = false;

  void _syncNavigation() {
    material.WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      final canPop = _navigatorKey.currentState?.canPop() ?? false;
      if (canPop != _innerCanPop) setState(() => _innerCanPop = canPop);
    });
  }

  @override
  void initState() {
    super.initState();
    if (widget.initialTarget case final target?) {
      material.WidgetsBinding.instance.addPostFrameCallback((_) {
        if (!mounted) return;
        _navigatorKey.currentState?.push(
          material.MaterialPageRoute<void>(
            builder: (_) => WorkspacePage(initial: target),
          ),
        );
      });
    }
  }

  @override
  material.Widget build(material.BuildContext context) {
    final outerNavigator = fluent.Navigator.of(context);
    return material.PopScope(
      canPop: !_innerCanPop,
      onPopInvokedWithResult: (didPop, _) {
        if (!didPop && _innerCanPop) _navigatorKey.currentState?.maybePop();
      },
      child: material.Column(
        children: [
          material.Material(
            color: DesktopColors.canvas,
            child: material.SafeArea(
              bottom: false,
              child: material.Padding(
                padding: const material.EdgeInsets.symmetric(
                  horizontal: DesktopSpacing.lg,
                  vertical: DesktopSpacing.sm,
                ),
                child: material.Row(
                  children: [
                    material.TextButton.icon(
                      style: material.TextButton.styleFrom(
                        foregroundColor: DesktopColors.text,
                      ),
                      onPressed: () {
                        if (_innerCanPop) {
                          _navigatorKey.currentState?.maybePop();
                        } else {
                          outerNavigator.maybePop();
                        }
                      },
                      icon: const material.Icon(material.Icons.arrow_back),
                      label: material.Text(
                        _innerCanPop ? '返回原界面目录' : '返回 Fluent 首页',
                      ),
                    ),
                    const material.SizedBox(width: DesktopSpacing.lg),
                    const material.Expanded(
                      child: material.Text(
                        '原界面 · 桌面 Fluent 页面尚未迁移',
                        style: material.TextStyle(color: DesktopColors.text),
                        maxLines: 1,
                        overflow: material.TextOverflow.ellipsis,
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ),
          material.Expanded(
            child: material.MaterialApp(
              navigatorKey: _navigatorKey,
              navigatorObservers: [_LegacyNavigationObserver(_syncNavigation)],
              title: 'GuoSSHell',
              theme: material.ThemeData(
                brightness: material.Brightness.dark,
                colorScheme: material.ColorScheme.fromSeed(
                  seedColor: material.Colors.teal,
                  brightness: material.Brightness.dark,
                ),
              ),
              home: widget.homeOverride ?? const ConnectionListPage(),
            ),
          ),
        ],
      ),
    );
  }
}

class _LegacyNavigationObserver extends material.NavigatorObserver {
  final material.VoidCallback onChange;

  _LegacyNavigationObserver(this.onChange);

  @override
  void didPush(
    material.Route<dynamic> route,
    material.Route<dynamic>? previousRoute,
  ) => onChange();

  @override
  void didPop(
    material.Route<dynamic> route,
    material.Route<dynamic>? previousRoute,
  ) => onChange();
}
