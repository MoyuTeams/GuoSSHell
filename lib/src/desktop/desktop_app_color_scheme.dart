import 'dart:async';

import 'package:flutter/material.dart';
import 'package:terminal_view/terminal_view.dart';

import '../bindings/bindings.dart';
import '../terminal/desktop_terminal_theme.dart';
import 'desktop_theme.dart';
import 'settings/desktop_settings_gateway.dart';

/// 由应用配色派生的桌面壳语义色；`default` 不派生，沿用设计 token。
@immutable
class DesktopAppPalette {
  const DesktopAppPalette({
    required this.brightness,
    required this.canvas,
    required this.surface,
    required this.text,
    required this.accent,
  });
  final Brightness brightness;

  /// 窗口画布：终端背景。
  final Color canvas;

  /// 面板与对话框表面：终端背景向前景微调。
  final Color surface;
  final Color text;
  final Color accent;
}

double _contrast(Color a, Color b) {
  final la = a.computeLuminance();
  final lb = b.computeLuminance();
  return la > lb ? (la + 0.05) / (lb + 0.05) : (lb + 0.05) / (la + 0.05);
}

/// `default` 或未知配色返回 null。强调色依次取 cyan、blue、brightCyan、brightBlue
/// 中第一个与画布对比度 ≥3:1 的颜色，都不满足时回退 [DesktopColors.accent]。
DesktopAppPalette? desktopAppPalette(String scheme) {
  if (scheme == 'default') return null;
  final TerminalTheme theme;
  try {
    theme = themeForProfile(scheme);
  } on ArgumentError {
    return null;
  }
  final canvas = theme.background;
  return DesktopAppPalette(
    brightness: ThemeData.estimateBrightnessForColor(canvas),
    canvas: canvas,
    surface: Color.lerp(canvas, theme.foreground, 0.06)!,
    text: theme.foreground,
    accent: [theme.cyan, theme.blue, theme.brightCyan, theme.brightBlue]
        .firstWhere(
          (color) => _contrast(color, canvas) >= 3,
          orElse: () => DesktopColors.accent,
        ),
  );
}

/// 应用配色的单一来源：设置 gateway 的任何成功结果更新快照后读取 app.colorScheme，
/// 设置页保存后壳主题立即生效。未知值按 `default` 处理。
class DesktopAppColorScheme extends ValueNotifier<String> {
  DesktopAppColorScheme() : super('default');
  static final instance = DesktopAppColorScheme();
  static const _owner = 'desktop-app-color-scheme';
  DesktopSettingsGateway? _gateway;
  StreamSubscription<DesktopSettingsGatewayEvent>? _events;
  int _sequence = 0;

  /// 首次调用订阅 gateway；每次调用以固定 owner 发一次查询。
  void connect([DesktopSettingsGateway? gateway]) {
    final source = _gateway ??= gateway ?? DesktopSettingsGateway.instance;
    _events ??= source.events.listen((_) => _sync());
    _sync();
    source.send(
      DesktopConfigurationCommand(
        DesktopSettingsRequest(
          protocolVersion: 1,
          requestId: '$_owner-query-${++_sequence}',
          baseRevision: Uint64(BigInt.zero),
          operation: 'query',
          targetId: '',
          name: '',
        ),
      ),
      const DesktopSettingsRequestContext(
        ownerId: _owner,
        generation: 0,
        editVersion: 0,
        unit: 'query',
      ),
    );
  }

  void _sync() {
    final scheme = _gateway?.snapshot?.app.colorScheme;
    if (scheme == null) return;
    value = desktopAppPalette(scheme) == null ? 'default' : scheme;
  }

  @override
  void dispose() {
    _events?.cancel();
    super.dispose();
  }
}
