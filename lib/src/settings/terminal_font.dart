import 'package:terminal_view/terminal_view.dart' show TerminalStyle;

import '../bindings/bindings.dart';
import 'interface_font.dart';

/// App 内置的终端字体（pubspec 的 fonts 声明）。
const bundledFontFamily = 'MesloLGS NF';

/// 字体选择与默认字号组成终端样式；缺字依次回退到 Nerd Font、MiSans 和系统等宽字体。
TerminalStyle terminalStyle(SettingsState settings) => TerminalStyle(
  fontSize: settings.fontSize,
  fontFamily: InterfaceTypography.terminal.fontFamily,
  fontFamilyFallback: const [
    bundledFontFamily,
    'MiSans',
    'monospace',
    'Courier New',
  ],
);
