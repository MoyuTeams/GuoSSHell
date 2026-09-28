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
}

/// Windows 工作区只截获明确的桌面组合键；Ctrl+C/D/W 等保持终端语义。
DesktopCommand? desktopCommand(
  LogicalKeyboardKey key, {
  required bool control,
  required bool shift,
  bool alt = false,
}) {
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
