import 'package:terminal_view/terminal_view.dart' show TerminalStyle;

import '../bindings/bindings.dart';

/// App 内置的终端字体（pubspec 的 fonts 声明）。
const bundledFontFamily = 'MesloLGS NF';

/// 设置 → 终端样式。所选字体缺字时依次回退：内置 Nerd Font（powerline / 图标）、
/// 系统等宽字体；中日韩等再由系统字体回退补齐。
TerminalStyle terminalStyle(SettingsState settings) => TerminalStyle(
      fontSize: settings.fontSize,
      fontFamily: settings.fontFamily,
      fontFamilyFallback: const [bundledFontFamily, 'Menlo', 'monospace', 'Courier New'],
    );
