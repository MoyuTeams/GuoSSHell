// 真机性能验收共用一次 profile 构建，按顺序运行全屏 TUI 和 coding agent 套件。
import 'm6_agents_test.dart' as agents;
import 'm6_tui_test.dart' as tui;

void main() {
  tui.main();
  agents.main();
}
