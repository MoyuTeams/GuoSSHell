import 'package:fluent_ui/fluent_ui.dart';

import 'desktop_status.dart';
import 'desktop_theme.dart';

/// 阶段一只有 UI 基座；旧功能单独开兼容窗口，不能算作已迁移页面。
class DesktopShell extends StatefulWidget {
  final VoidCallback openLegacy;

  const DesktopShell({super.key, required this.openLegacy});

  @override
  State<DesktopShell> createState() => _DesktopShellState();
}

class _DesktopShellState extends State<DesktopShell> {
  int _selected = 0;

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, constraints) {
        final width = constraints.maxWidth;
        final mode = width < DesktopMetrics.narrowWidth
            ? PaneDisplayMode.minimal
            : width < DesktopMetrics.wideWidth
            ? PaneDisplayMode.compact
            : PaneDisplayMode.expanded;
        return NavigationView(
          pane: NavigationPane(
            displayMode: mode,
            size: const NavigationPaneSize(openWidth: DesktopMetrics.paneWidth),
            selected: _selected,
            onChanged: (index) => setState(() => _selected = index),
            header: const Padding(
              padding: EdgeInsets.all(DesktopSpacing.lg),
              child: Text('GuoSSHell'),
            ),
            items: [
              PaneItem(
                icon: const Icon(FluentIcons.home),
                title: const Text('概览'),
                body: _Overview(openLegacy: widget.openLegacy),
              ),
              PaneItem(
                icon: const Icon(FluentIcons.test_plan),
                title: const Text('组件状态'),
                body: const DesktopShowcase(),
              ),
            ],
          ),
        );
      },
    );
  }
}

class _Overview extends StatelessWidget {
  final VoidCallback openLegacy;

  const _Overview({required this.openLegacy});

  @override
  Widget build(BuildContext context) {
    return DesktopPage(
      title: '连接工作台',
      description: '桌面 Fluent 导航已就绪；连接目录、工作区和设置仍由原界面提供。',
      children: [
        const DesktopStatus(
          kind: DesktopStatusKind.empty,
          title: '桌面页面尚未迁移',
          detail: '打开兼容界面，继续使用现有 SSH 连接与终端功能。',
        ),
        const SizedBox(height: DesktopSpacing.lg),
        FilledButton(onPressed: openLegacy, child: const Text('打开原界面（尚未迁移）')),
      ],
    );
  }
}

/// 纯展示状态：不发 Rust 请求、不伪造连接成功。
class DesktopShowcase extends StatefulWidget {
  const DesktopShowcase({super.key});

  @override
  State<DesktopShowcase> createState() => _DesktopShowcaseState();
}

class _DesktopShowcaseState extends State<DesktopShowcase> {
  String _feedback = '输入框只用于演示，不保存配置。';

  @override
  Widget build(BuildContext context) {
    return DesktopPage(
      title: '组件状态',
      description: '鼠标、Tab 和 Enter 均可检查操作控件；示例不连接或写入数据。',
      children: [
        const DesktopStatus(
          kind: DesktopStatusKind.loading,
          title: '加载中',
          detail: '应用启动时等待 Rust 就绪和首份设置。',
        ),
        const SizedBox(height: DesktopSpacing.lg),
        const DesktopStatus(
          kind: DesktopStatusKind.empty,
          title: '空状态',
          detail: '没有内容时展示说明和下一步操作。',
        ),
        const SizedBox(height: DesktopSpacing.lg),
        const DesktopStatus(
          kind: DesktopStatusKind.error,
          title: '失败状态',
          detail: '操作失败时明确说明原因，不自动降级为其他传输方式。',
        ),
        const SizedBox(height: DesktopSpacing.xxl),
        Semantics(
          label: '示例名称',
          child: TextBox(
            placeholder: '示例名称',
            onChanged: (value) {
              setState(
                () => _feedback = value.isEmpty
                    ? '输入框只用于演示，不保存配置。'
                    : '已输入示例内容；未保存。',
              );
            },
          ),
        ),
        const SizedBox(height: DesktopSpacing.md),
        const TextBox(placeholder: '不可编辑示例', enabled: false),
        const SizedBox(height: DesktopSpacing.lg),
        Wrap(
          spacing: DesktopSpacing.md,
          runSpacing: DesktopSpacing.md,
          children: [
            FilledButton(
              onPressed: () => setState(() => _feedback = '已触发示例操作；未发送请求。'),
              child: const Text('示例操作'),
            ),
            const Button(onPressed: null, child: Text('不可用操作')),
          ],
        ),
        const SizedBox(height: DesktopSpacing.lg),
        Semantics(liveRegion: true, child: Text(_feedback)),
      ],
    );
  }
}
