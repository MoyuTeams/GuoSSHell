import 'package:fluent_ui/fluent_ui.dart';

/// 桌面壳的语义色；终端配色仍由终端配置管理。
abstract final class DesktopColors {
  static const canvas = Color(0xFF202020);
  static const surface = Color(0xFF292929);
  static const text = Color(0xFFF5F5F5);
  static const muted = Color(0xFFC2C2C2);
  static const accent = Color(0xFF80CBC4);
  static const onAccent = Color(0xFF102523);
  static const border = Color(0xFF858585);
  static const highContrastCanvas = Color(0xFF000000);
  static const highContrastText = Color(0xFFFFFFFF);
}

abstract final class DesktopSpacing {
  static const xs = 4.0;
  static const sm = 8.0;
  static const md = 12.0;
  static const lg = 16.0;
  static const xl = 24.0;
  static const xxl = 32.0;
}

abstract final class DesktopMetrics {
  static const radius = 4.0;
  static const border = 1.0;
  static const focus = 2.0;
  static const icon = 24.0;
  static const paneWidth = 232.0;
  static const contentWidth = 880.0;
  static const narrowWidth = 640.0;
  static const wideWidth = 1000.0;
}

abstract final class DesktopType {
  static const body = 14.0;
  static const caption = 12.0;
  static const subtitle = 20.0;
  static const title = 28.0;
  static const family = 'Microsoft YaHei UI';
  static const fallback = ['PingFang SC', 'Noto Sans CJK SC', 'Segoe UI'];
}

abstract final class DesktopMotion {
  static const control = Duration(milliseconds: 100);
  static const page = Duration(milliseconds: 200);
}

FluentThemeData desktopTheme({
  required bool highContrast,
  required bool reduceMotion,
  String fontFamily = DesktopType.family,
}) {
  final foreground = highContrast
      ? DesktopColors.highContrastText
      : DesktopColors.text;
  final accent = highContrast
      ? DesktopColors.highContrastText
      : DesktopColors.accent;
  final duration = reduceMotion ? Duration.zero : DesktopMotion.control;
  return FluentThemeData(
    brightness: Brightness.dark,
    accentColor: AccentColor.swatch({
      'darkest': accent,
      'darker': accent,
      'dark': accent,
      'normal': accent,
      'light': accent,
      'lighter': accent,
      'lightest': accent,
    }),
    scaffoldBackgroundColor: highContrast
        ? DesktopColors.highContrastCanvas
        : DesktopColors.surface,
    inactiveBackgroundColor: highContrast
        ? DesktopColors.highContrastCanvas
        : DesktopColors.canvas,
    resources: highContrast
        ? const ResourceDictionary.dark(
            controlStrokeColorDefault: DesktopColors.highContrastText,
            controlStrokeColorSecondary: DesktopColors.highContrastText,
            controlStrongStrokeColorDefault: DesktopColors.highContrastText,
            focusStrokeColorOuter: DesktopColors.highContrastText,
          )
        : null,
    fontFamily: fontFamily,
    typography: Typography.raw(
      display: TextStyle(
        fontSize: DesktopType.title,
        fontWeight: FontWeight.w600,
        color: foreground,
        fontFamilyFallback: DesktopType.fallback,
      ),
      titleLarge: TextStyle(
        fontSize: DesktopType.subtitle,
        fontWeight: FontWeight.w600,
        color: foreground,
        fontFamilyFallback: DesktopType.fallback,
      ),
      body: TextStyle(
        fontSize: DesktopType.body,
        height: 1.5,
        color: foreground,
        fontFamilyFallback: DesktopType.fallback,
      ),
      caption: TextStyle(
        fontSize: DesktopType.caption,
        height: 1.5,
        color: foreground,
        fontFamilyFallback: DesktopType.fallback,
      ),
    ),
    focusTheme: FocusThemeData(
      borderRadius: BorderRadius.circular(DesktopMetrics.radius),
      primaryBorder: BorderSide(color: accent, width: DesktopMetrics.focus),
      secondaryBorder: BorderSide(
        color: DesktopColors.canvas,
        width: DesktopMetrics.border,
      ),
    ),
    fasterAnimationDuration: duration,
    fastAnimationDuration: duration,
    mediumAnimationDuration: reduceMotion ? Duration.zero : DesktopMotion.page,
    slowAnimationDuration: reduceMotion ? Duration.zero : DesktopMotion.page,
  );
}
