import 'package:fluent_ui/fluent_ui.dart' as fluent;
import 'package:flutter/material.dart' as material;

import '../settings/interface_font.dart';
import '../settings/interface_font_setting.dart';
import '../terminal/session_target.dart';
import '../workspace/workspace_page.dart';
import 'desktop_theme.dart';

/// 兼容路径有独立 Material Navigator；从 Fluent 壳退出不会卸载启动订阅。
class LegacyMaterialSurface extends material.StatefulWidget {
  final SessionTarget? initialTarget;
  final material.Widget? homeOverride;
  final material.Widget Function(SessionTarget)? workspaceBuilder;
  final bool _settings;

  const LegacyMaterialSurface({
    super.key,
    this.initialTarget,
    this.homeOverride,
    this.workspaceBuilder,
  }) : _settings = false,
       assert(initialTarget != null || homeOverride != null);

  /// 设置独立于受目标约束的工作区，不暴露旧 Material 目录或密钥入口。
  const LegacyMaterialSurface.settings({super.key})
    : initialTarget = null,
      homeOverride = null,
      workspaceBuilder = null,
      _settings = true;

  @override
  material.State<LegacyMaterialSurface> createState() =>
      _LegacyMaterialSurfaceState();
}

class _LegacyMaterialSurfaceState
    extends material.State<LegacyMaterialSurface> {
  final _navigatorKey = material.GlobalKey<material.NavigatorState>();
  material.Route<void>? _workspaceRoute;
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
        _workspaceRoute = material.MaterialPageRoute<void>(
          builder: (_) =>
              widget.workspaceBuilder?.call(target) ??
              WorkspacePage(initial: target),
        );
        _navigatorKey.currentState?.push(_workspaceRoute!);
      });
    }
  }

  void _onInnerPop(material.Route<dynamic> route) {
    if (!identical(route, _workspaceRoute) || !mounted) return;
    // 工作区自身的受保护关闭已通过后才会弹出此路由；不再露出旧 Material 目录。
    material.WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) fluent.Navigator.of(context).pop();
    });
  }

  @override
  material.Widget build(material.BuildContext context) {
    final outerNavigator = fluent.Navigator.of(context);
    final theme = material.ThemeData(
      fontFamily: InterfaceFontScope.fontFamilyOf(context),
      brightness: material.Brightness.dark,
      colorScheme: material.ColorScheme.fromSeed(
        seedColor: material.Colors.teal,
        brightness: material.Brightness.dark,
      ),
    );
    return material.Theme(
      data: theme,
      child: material.PopScope(
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
                          _innerCanPop
                              ? widget.initialTarget == null
                                    ? '返回原界面目录'
                                    : '返回 Fluent 工作台'
                              : '返回 Fluent 首页',
                        ),
                      ),
                      const material.SizedBox(width: DesktopSpacing.lg),
                      material.Expanded(
                        child: material.Text(
                          widget._settings
                              ? '字体设置 · Material 兼容界面（尚未迁移）'
                              : '工作区 · Material 兼容界面（尚未迁移）',
                          style: const material.TextStyle(
                            color: DesktopColors.text,
                          ),
                          maxLines: 1,
                          overflow: material.TextOverflow.ellipsis,
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            ),
            if (widget.initialTarget != null)
              material.Material(
                color: DesktopColors.canvas,
                child: const material.Padding(
                  padding: material.EdgeInsets.symmetric(
                    horizontal: DesktopSpacing.lg,
                    vertical: DesktopSpacing.sm,
                  ),
                  child: material.Text(
                    '兼容工作区：新标签、分屏的连接选择器与快速连接仍使用原界面；密钥管理请返回 Fluent 密钥页。',
                    style: material.TextStyle(color: DesktopColors.text),
                  ),
                ),
              ),
            material.Expanded(
              child: material.MaterialApp(
                navigatorKey: _navigatorKey,
                navigatorObservers: [
                  _LegacyNavigationObserver(_syncNavigation, _onInnerPop),
                ],
                title: 'GuoSSHell',
                theme: theme,
                home: widget._settings
                    ? const _LegacyFontSettingsPage()
                    : widget.homeOverride ?? const material.SizedBox.shrink(),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// 仅复用现有双字体槽；完整 Fluent 设置由后续迁移替代。
class _LegacyFontSettingsPage extends material.StatelessWidget {
  const _LegacyFontSettingsPage();

  @override
  material.Widget build(material.BuildContext context) => material.Scaffold(
    appBar: material.AppBar(title: const material.Text('字体设置（兼容）')),
    body: const material.SafeArea(
      child: material.SingleChildScrollView(
        child: material.Column(
          children: [
            InterfaceFontSetting(),
            InterfaceFontSetting(terminal: true),
          ],
        ),
      ),
    ),
  );
}

class _LegacyNavigationObserver extends material.NavigatorObserver {
  final material.VoidCallback onChange;
  final void Function(material.Route<dynamic>) onPop;

  _LegacyNavigationObserver(this.onChange, this.onPop);

  @override
  void didPush(
    material.Route<dynamic> route,
    material.Route<dynamic>? previousRoute,
  ) => onChange();

  @override
  void didPop(
    material.Route<dynamic> route,
    material.Route<dynamic>? previousRoute,
  ) {
    onChange();
    onPop(route);
  }
}
