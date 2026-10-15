import 'package:fluent_ui/fluent_ui.dart';

import '../../desktop_theme.dart';
import 'desktop_about_resources.dart';
import 'desktop_appearance_resources.dart';
import 'desktop_font_resources.dart';
import 'desktop_resources_contracts.dart';

export 'desktop_resources_contracts.dart';

/// 父持有真实服务与输入边界；此区域不连接 Rinf、宿主或全局单例。
class DesktopSettingsResources extends StatefulWidget {
  DesktopSettingsResources({
    super.key,
    required this.interfaceFont,
    required this.terminalFont,
    required this.inputBoundary,
    required this.platform,
    required this.applicationName,
    required this.applicationVersion,
    required this.registry,
    this.appearance,
    this.onDirtyChanged,
    this.showAbout = true,
  }) : assert(interfaceFont.slot == DesktopFontSlot.interface),
       assert(terminalFont.slot == DesktopFontSlot.terminal);
  final DesktopFontResourceCallbacks interfaceFont;
  final DesktopFontResourceCallbacks terminalFont;
  final DesktopResourcesInputBoundary inputBoundary;
  final TargetPlatform platform;
  final DesktopAppearanceCallbacks? appearance;
  final String applicationName;
  final String applicationVersion;
  final DesktopLicenseRegistry registry;
  final ValueChanged<bool>? onDirtyChanged;

  /// 设置页把「关于」单独成页时关闭；许可注册表仍由父提供。
  final bool showAbout;

  @override
  State<DesktopSettingsResources> createState() =>
      DesktopSettingsResourcesState();
}

class DesktopSettingsResourcesState extends State<DesktopSettingsResources> {
  final _dirty = <String, bool>{};
  bool get hasUnsavedChanges => _dirty.values.any((value) => value);
  void _dirtyChanged(String section, bool value) {
    final previous = hasUnsavedChanges;
    _dirty[section] = value;
    if (previous != hasUnsavedChanges) {
      widget.onDirtyChanged?.call(hasUnsavedChanges);
    }
  }

  final _shortcuts = ShortcutManager(
    shortcuts: WidgetsApp.defaultShortcuts,
    modal: true,
  );
  bool _acquiring = true;
  bool _acquired = false;
  int? _boundaryGeneration;
  int _version = 0;

  @override
  void initState() {
    super.initState();
    widget.inputBoundary.addListener(_changed);
    _scheduleAcquire();
  }

  void _changed() {
    if (mounted) setState(() {});
  }

  void _scheduleAcquire() => WidgetsBinding.instance.addPostFrameCallback((_) {
    if (mounted) _acquire();
  });

  Future<void> _acquire() async {
    final version = ++_version;
    final boundary = widget.inputBoundary;
    final generation = boundary.generation;
    setState(() {
      _acquiring = true;
      _acquired = false;
    });
    var confirmed = false;
    try {
      await boundary.acquire();
      confirmed = boundary.ready && generation == boundary.generation;
    } catch (_) {
      /* 未确认的边界保持不可编辑。 */
    }
    if (mounted && version == _version && boundary == widget.inputBoundary) {
      setState(() {
        _acquiring = false;
        _acquired = confirmed;
        _boundaryGeneration = generation;
      });
    }
  }

  @override
  void didUpdateWidget(DesktopSettingsResources oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.inputBoundary != widget.inputBoundary) {
      ++_version;
      oldWidget.inputBoundary.removeListener(_changed);
      oldWidget.inputBoundary.release();
      widget.inputBoundary.addListener(_changed);
      _acquiring = true;
      _acquired = false;
      _scheduleAcquire();
    }
  }

  @override
  void dispose() {
    ++_version;
    widget.inputBoundary.removeListener(_changed);
    widget.inputBoundary.release();
    _shortcuts.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final ready =
        !_acquiring &&
        _acquired &&
        widget.inputBoundary.ready &&
        _boundaryGeneration == widget.inputBoundary.generation;
    final surface = Shortcuts.manager(
      manager: _shortcuts,
      child: FocusTraversalGroup(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (!ready)
              Semantics(
                liveRegion: true,
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    const Text('输入边界尚未确认，设置暂不可编辑。'),
                    if (!_acquiring)
                      Button(
                        key: const ValueKey('retry-resource-boundary'),
                        onPressed: _acquire,
                        child: const Text('重新确认输入边界'),
                      ),
                  ],
                ),
              ),
            AbsorbPointer(
              absorbing: !ready,
              child: ExcludeFocus(
                excluding: !ready,
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    DesktopFontResources(
                      key: const ValueKey('interface-resources'),
                      callbacks: widget.interfaceFont,
                      inputBoundary: widget.inputBoundary,
                      enabled: ready,
                      onDirtyChanged: (value) =>
                          _dirtyChanged('interface', value),
                    ),
                    const SizedBox(height: DesktopSpacing.xxl),
                    DesktopFontResources(
                      key: const ValueKey('terminal-resources'),
                      callbacks: widget.terminalFont,
                      inputBoundary: widget.inputBoundary,
                      enabled: ready,
                      onDirtyChanged: (value) =>
                          _dirtyChanged('terminal', value),
                    ),
                    const SizedBox(height: DesktopSpacing.xxl),
                    if (widget.platform == TargetPlatform.windows &&
                        widget.appearance != null)
                      DesktopAppearanceResources(
                        callbacks: widget.appearance!,
                        inputBoundary: widget.inputBoundary,
                        enabled: ready,
                        onDirtyChanged: (value) =>
                            _dirtyChanged('appearance', value),
                      )
                    else
                      Text(
                        widget.platform == TargetPlatform.windows
                            ? 'Windows 外观服务尚未连接；字体与关于区域仍可使用。'
                            : '窗口透明度仅适用于 Windows；当前平台不调用宿主外观。',
                      ),
                    if (widget.showAbout) ...[
                      const SizedBox(height: DesktopSpacing.xxl),
                      DesktopAboutResources(
                        applicationName: widget.applicationName,
                        applicationVersion: widget.applicationVersion,
                        registry: widget.registry,
                        enabled: ready,
                      ),
                    ],
                  ],
                ),
              ),
            ),
          ],
        ),
      ),
    );
    return Align(
      alignment: Alignment.topLeft,
      child: ConstrainedBox(
        constraints: const BoxConstraints(
          maxWidth: DesktopMetrics.contentWidth,
        ),
        child: surface,
      ),
    );
  }
}
