import 'package:fluent_ui/fluent_ui.dart';

import 'desktop_theme.dart';

enum DesktopStatusKind { loading, empty, error }

/// 统一的启动与页面状态；图标永远和可读说明同时出现。
class DesktopStatus extends StatelessWidget {
  final DesktopStatusKind kind;
  final String title;
  final String detail;

  const DesktopStatus({
    super.key,
    required this.kind,
    required this.title,
    required this.detail,
  });

  @override
  Widget build(BuildContext context) {
    final reduceMotion =
        MediaQuery.disableAnimationsOf(context) ||
        MediaQuery.accessibleNavigationOf(context);
    final icon = switch (kind) {
      DesktopStatusKind.loading => FluentIcons.clock,
      DesktopStatusKind.empty => FluentIcons.folder_open,
      DesktopStatusKind.error => FluentIcons.error_badge,
    };
    return Semantics(
      liveRegion: kind != DesktopStatusKind.empty,
      child: Padding(
        padding: const EdgeInsets.all(DesktopSpacing.xl),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          mainAxisSize: MainAxisSize.min,
          children: [
            if (kind == DesktopStatusKind.loading && !reduceMotion)
              const ProgressRing(key: Key('desktop-loading-indicator'))
            else
              Icon(
                icon,
                size: DesktopMetrics.icon,
                color: FluentTheme.of(context).accentColor,
              ),
            const SizedBox(height: DesktopSpacing.lg),
            Text(title, style: FluentTheme.of(context).typography.titleLarge),
            const SizedBox(height: DesktopSpacing.sm),
            Text(detail, style: FluentTheme.of(context).typography.body),
          ],
        ),
      ),
    );
  }
}

/// 左对齐、可滚动的桌面内容栏。
class DesktopPage extends StatelessWidget {
  final String title;
  final String description;
  final List<Widget> children;

  const DesktopPage({
    super.key,
    required this.title,
    required this.description,
    required this.children,
  });

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, constraints) {
        final inset = constraints.maxWidth < DesktopMetrics.narrowWidth
            ? DesktopSpacing.lg
            : DesktopSpacing.xl;
        return ColoredBox(
          color: FluentTheme.of(context).scaffoldBackgroundColor,
          child: SingleChildScrollView(
            child: Align(
              alignment: Alignment.topLeft,
              child: Padding(
                padding: EdgeInsets.all(inset),
                child: ConstrainedBox(
                  key: const Key('desktop-page-content'),
                  constraints: const BoxConstraints(
                    maxWidth: DesktopMetrics.contentWidth,
                  ),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        title,
                        style: FluentTheme.of(context).typography.display,
                      ),
                      const SizedBox(height: DesktopSpacing.sm),
                      Text(
                        description,
                        style: FluentTheme.of(context).typography.body,
                      ),
                      const SizedBox(height: DesktopSpacing.xxl),
                      ...children,
                    ],
                  ),
                ),
              ),
            ),
          ),
        );
      },
    );
  }
}
