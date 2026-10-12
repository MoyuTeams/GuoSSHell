import 'dart:async';

import 'package:fluent_ui/fluent_ui.dart';
import 'package:flutter/foundation.dart';

import '../../desktop_theme.dart';
import 'desktop_resources_contracts.dart';

class DesktopAboutResources extends StatelessWidget {
  const DesktopAboutResources({
    super.key,
    required this.applicationName,
    required this.applicationVersion,
    required this.registry,
    required this.enabled,
  });
  final String applicationName;
  final String applicationVersion;
  final DesktopLicenseRegistry registry;
  final bool enabled;

  @override
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.stretch,
    children: [
      Text('关于', style: FluentTheme.of(context).typography.titleLarge),
      const SizedBox(height: DesktopSpacing.sm),
      Text(applicationName),
      Text('版本：$applicationVersion'),
      const Text('开源许可来自应用实际注册的 Flutter、Dart、Rust 和字体许可资源。'),
      const SizedBox(height: DesktopSpacing.md),
      Align(
        alignment: Alignment.centerLeft,
        child: Button(
          key: const ValueKey('open-resource-licenses'),
          onPressed: enabled
              ? () => Navigator.of(context).push<void>(
                  PageRouteBuilder<void>(
                    pageBuilder: (context, animation, secondaryAnimation) =>
                        DesktopResourceLicenses(registry: registry),
                    transitionDuration: Duration.zero,
                    reverseTransitionDuration: Duration.zero,
                  ),
                )
              : null,
          child: const Text('查看开源许可证'),
        ),
      ),
    ],
  );
}

/// 与旧许可入口共享 LicenseRegistry；不注册副本、不过滤现有依赖。
class DesktopResourceLicenses extends StatefulWidget {
  const DesktopResourceLicenses({super.key, required this.registry});
  final DesktopLicenseRegistry registry;

  @override
  State<DesktopResourceLicenses> createState() => _LicensesState();
}

class _LicensesState extends State<DesktopResourceLicenses> {
  final _entries = <LicenseEntry>[];
  StreamSubscription<LicenseEntry>? _subscription;
  int _version = 0;
  bool _loading = true;
  bool _failed = false;

  @override
  void initState() {
    super.initState();
    _load();
  }

  @override
  void didUpdateWidget(DesktopResourceLicenses oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.registry != widget.registry) _load();
  }

  void _load() {
    _subscription?.cancel();
    final version = ++_version;
    _entries.clear();
    _loading = true;
    _failed = false;
    try {
      _subscription = widget.registry().listen(
        (entry) {
          if (mounted && version == _version) {
            setState(() => _entries.add(entry));
          }
        },
        onError: (Object _) {
          if (mounted && version == _version) {
            setState(() {
              _failed = true;
              _loading = false;
            });
          }
        },
        onDone: () {
          if (mounted && version == _version) setState(() => _loading = false);
        },
      );
    } catch (_) {
      _failed = true;
      _loading = false;
    }
  }

  @override
  void dispose() {
    ++_version;
    _subscription?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => ScaffoldPage(
    header: PageHeader(
      title: const Text('开源许可证'),
      commandBar: Button(
        key: const ValueKey('return-resource-licenses'),
        onPressed: () => Navigator.of(context).pop(),
        child: const Text('返回设置'),
      ),
    ),
    content: ListView.builder(
      padding: const EdgeInsets.all(DesktopSpacing.lg),
      itemCount: _entries.length + 1,
      itemBuilder: (context, index) {
        if (index == _entries.length) {
          return Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              if (_loading)
                Semantics(liveRegion: true, child: const Text('正在加载已注册许可…')),
              if (!_loading && !_failed && _entries.isEmpty)
                const Text('实际许可注册表暂无条目。'),
              if (_failed) ...[
                Semantics(
                  liveRegion: true,
                  child: const Text('许可资源加载失败；已加载条目保留。'),
                ),
                Button(
                  key: const ValueKey('retry-resource-licenses'),
                  onPressed: () => setState(_load),
                  child: const Text('重试加载许可'),
                ),
              ],
            ],
          );
        }
        final entry = _entries[index];
        return Padding(
          padding: const EdgeInsets.only(bottom: DesktopSpacing.xl),
          child: Column(
            key: ValueKey('resource-license-$index'),
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Text(
                entry.packages.join(' · '),
                style: FluentTheme.of(context).typography.titleLarge,
              ),
              for (final paragraph in entry.paragraphs)
                Padding(
                  padding: const EdgeInsets.only(top: DesktopSpacing.sm),
                  child: SelectableText(
                    paragraph.text,
                    textAlign:
                        paragraph.indent == LicenseParagraph.centeredIndent
                        ? TextAlign.center
                        : TextAlign.start,
                    style: FluentTheme.of(context).typography.body,
                  ),
                ),
            ],
          ),
        );
      },
    ),
  );
}
