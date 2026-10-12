import 'package:fluent_ui/fluent_ui.dart';

import '../../settings/interface_font_setting.dart';
import '../desktop_theme.dart';

/// macOS/Linux 原兼容设置的双字体槽，使用已有 Fluent 控件；不调用 Windows 宿主。
/// 完整 profile 设置编辑器仍由阶段五负责。
class DesktopWorkspaceSettings extends StatelessWidget {
  const DesktopWorkspaceSettings({super.key});

  @override
  Widget build(BuildContext context) => ContentDialog(
    constraints: const BoxConstraints(maxWidth: DesktopMetrics.contentWidth),
    title: const Text('字体设置'),
    content: const SingleChildScrollView(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          InterfaceFontSetting(fluent: true),
          SizedBox(height: DesktopSpacing.xl),
          InterfaceFontSetting(fluent: true, terminal: true),
        ],
      ),
    ),
    actions: [
      Button(
        onPressed: () => Navigator.of(context).pop(),
        child: const Text('完成'),
      ),
    ],
  );
}
