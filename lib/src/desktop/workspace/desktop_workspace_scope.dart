import 'package:flutter/widgets.dart';

import '../../bindings/bindings.dart';
import '../../terminal/terminal_pane.dart';

/// 明确区分执行、陈旧来源与尚待共用窗格提供的真实操作，不能将缺口当成功。
enum DesktopActionOutcome { executed, staleOrigin, unavailable, needsPaneApi }

/// 4C 校验 source/generation/identity/seq 后，才将原 controller 交给常驻壳。
/// 本层不订阅 Rust action，也不发送任何会话或有序输入消息。
class DesktopWorkspaceScope extends InheritedWidget {
  const DesktopWorkspaceScope({
    super.key,
    required this.execute,
    required this.ownsTerminalInput,
    required this.isCurrentOrigin,
    required super.child,
  });

  final Future<DesktopActionOutcome> Function(
    DesktopAction action, {
    required TerminalPaneController origin,
  })
  execute;

  /// 4C 同步启用单一有序输入时登记，卸载/退出时撤销；旧路径不登记。
  /// 登记后壳不抢先执行静态快捷键或缩放，未匹配候选由 Rust 返回一次。
  final void Function(TerminalPaneController controller, bool enabled)
  ownsTerminalInput;
  final bool Function(TerminalPaneController controller) isCurrentOrigin;

  /// 共用窗格在移动入口找不到本 scope，继续保留移动旧输入路径。
  static DesktopWorkspaceScope? maybeOf(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<DesktopWorkspaceScope>();

  static DesktopWorkspaceScope of(BuildContext context) => maybeOf(context)!;

  @override
  bool updateShouldNotify(DesktopWorkspaceScope oldWidget) => false;
}
