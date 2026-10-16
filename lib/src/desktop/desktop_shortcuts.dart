import 'package:flutter/services.dart';

enum DesktopCommand {
  newTab,
  closePane,
  splitRight,
  splitDown,
  nextTab,
  previousTab,
  nextPane,
  search,
  settings,
  toggleSidebar,
  localTerminal,
}

/// Windows 工作区只截获明确的桌面组合键；Ctrl+C/D/W 等保持终端语义。
/// 本地终端为 Ctrl+Shift+L，macOS 习惯的 Cmd+Shift+L（[meta]）同样接受。
DesktopCommand? desktopCommand(
  LogicalKeyboardKey key, {
  required bool control,
  required bool shift,
  bool alt = false,
  bool meta = false,
}) {
  if (key == LogicalKeyboardKey.keyL && shift && !alt && (control || meta)) {
    return DesktopCommand.localTerminal;
  }
  if (!control || alt) return null;
  if (key == LogicalKeyboardKey.tab) {
    return shift ? DesktopCommand.previousTab : DesktopCommand.nextTab;
  }
  if (key == LogicalKeyboardKey.comma && !shift) return DesktopCommand.settings;
  if (!shift) return null;
  return {
    LogicalKeyboardKey.keyT: DesktopCommand.newTab,
    LogicalKeyboardKey.keyW: DesktopCommand.closePane,
    LogicalKeyboardKey.keyD: DesktopCommand.splitRight,
    LogicalKeyboardKey.keyE: DesktopCommand.splitDown,
    LogicalKeyboardKey.keyP: DesktopCommand.nextPane,
    LogicalKeyboardKey.keyF: DesktopCommand.search,
    LogicalKeyboardKey.keyB: DesktopCommand.toggleSidebar,
  }[key];
}
