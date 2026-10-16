import 'package:fluent_ui/fluent_ui.dart';

import '../terminal/session_target.dart';
import 'catalog/desktop_catalog_gateway.dart';
import 'catalog/desktop_catalog_page.dart';
import 'desktop_status.dart';
import 'desktop_theme.dart';
import 'keys/desktop_keys_page.dart';
import 'keys/desktop_key_actions.dart';
import 'keys/desktop_key_sync_state.dart';

/// 目录和密钥进入 Fluent；尚未迁移的工作区与设置继续使用兼容入口。
class DesktopShell extends StatefulWidget {
  final ValueChanged<SessionTarget>? openTarget;
  final VoidCallback? openSettings;
  final DesktopCatalogGateway catalogGateway;
  final DesktopKeyActions? keyActions;

  const DesktopShell({
    super.key,
    this.openTarget,
    this.openSettings,
    this.catalogGateway = const RustDesktopCatalogGateway(),
    this.keyActions,
  });

  @override
  State<DesktopShell> createState() => _DesktopShellState();
}

class _DesktopShellState extends State<DesktopShell> {
  int _selected = 0;
  final DesktopKeySyncState _keySyncState = DesktopKeySyncState();

  @override
  void dispose() {
    _keySyncState.dispose();
    super.dispose();
  }

  void _unavailableTarget(SessionTarget _) {
    showDialog<void>(
      context: context,
      builder: (dialogContext) => ContentDialog(
        title: const Text('工作区入口不可用'),
        content: const Text('未配置工作区入口，连接尚未启动。'),
        actions: [
          FilledButton(
            onPressed: () => Navigator.of(dialogContext).pop(),
            child: const Text('关闭'),
          ),
        ],
      ),
    );
  }

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
                body: _Overview(
                  onOpenCatalog: () => setState(() => _selected = 2),
                  onOpenSettings: widget.openSettings,
                ),
              ),
              PaneItem(
                icon: const Icon(FluentIcons.test_plan),
                title: const Text('组件状态'),
                body: const DesktopShowcase(),
              ),
              PaneItem(
                icon: const Icon(FluentIcons.folder_open),
                title: const Text('连接目录'),
                body: DesktopCatalogPage(
                  gateway: widget.catalogGateway,
                  openTarget: widget.openTarget ?? _unavailableTarget,
                  keySyncState: _keySyncState,
                ),
              ),
              PaneItem(
                icon: const Icon(FluentIcons.password_field),
                title: const Text('密钥管理'),
                body: DesktopKeysPage(
                  syncState: _keySyncState,
                  actions: widget.keyActions,
                ),
              ),
            ],
          ),
        );
      },
    );
  }
}

class _Overview extends StatelessWidget {
  final VoidCallback onOpenCatalog;
  final VoidCallback? onOpenSettings;

  const _Overview({required this.onOpenCatalog, this.onOpenSettings});

  @override
  Widget build(BuildContext context) {
    return DesktopPage(
      title: '连接工作台',
      description: '连接目录与密钥管理已迁移至 Fluent；工作区与字体设置暂由明确标记的兼容界面提供。',
      children: [
        const DesktopStatus(
          kind: DesktopStatusKind.empty,
          title: '从连接目录开始',
          detail: '选择已保存的连接或快速连接；工作区内的新标签与分屏选择器仍待迁移。',
        ),
        const SizedBox(height: DesktopSpacing.lg),
        Wrap(
          spacing: DesktopSpacing.md,
          runSpacing: DesktopSpacing.md,
          children: [
            FilledButton(onPressed: onOpenCatalog, child: const Text('打开连接目录')),
            Button(onPressed: onOpenSettings, child: const Text('打开字体设置（兼容）')),
          ],
        ),
        const SizedBox(height: DesktopSpacing.sm),
        const Text('兼容设置仅提供现有界面／终端字体的选择、预览与恢复；完整 Fluent 设置尚未迁移。'),
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
