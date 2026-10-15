import 'dart:async';

import 'package:file_selector/file_selector.dart';
import 'package:fluent_ui/fluent_ui.dart';
import 'package:flutter/foundation.dart';

import '../../bindings/bindings.dart';
import '../../settings/interface_font.dart';
import '../../terminal/key_bar_layout.dart';
import '../desktop_theme.dart';
import '../windows_chrome.dart';
import 'desktop_settings_controller.dart';
import 'desktop_settings_draft.dart';
import 'forms/desktop_settings_forms.dart';
import 'keybar/desktop_keybar.dart';
import 'resources/desktop_about_resources.dart';
import 'resources/desktop_settings_resources.dart';

enum DesktopSettingsSection {
  app('应用'),
  profile('配置文件'),
  appearance('外观与字体'),
  keybar('键位条'),
  about('关于');

  const DesktopSettingsSection(this.label);
  final String label;
}

const _applicationName = 'GuoSSHell';
const _applicationVersion = String.fromEnvironment(
  'FLUTTER_BUILD_NAME',
  defaultValue: '未知',
);

Stream<LicenseEntry> _licenses() => LicenseRegistry.licenses;

/// 三桌面平台共用设置入口。controller 在打开前创建，关闭后释放并分离未决写入。
Future<void> showDesktopSettings(
  BuildContext context, {
  DesktopSettingsSection section = DesktopSettingsSection.app,
  SettingsState? settings,
}) async {
  final controller = DesktopSettingsController();
  try {
    await showDialog<void>(
      context: context,
      // Esc 与关闭按钮统一走脏检查，不能由对话框直接弹出。
      dismissWithEsc: false,
      builder: (_) => DesktopSettingsPage(
        controller: controller,
        initialSection: section,
        settings: settings,
      ),
    );
  } finally {
    controller.dispose();
  }
}

class DesktopSettingsPage extends StatefulWidget {
  const DesktopSettingsPage({
    super.key,
    required this.controller,
    this.initialSection = DesktopSettingsSection.app,
    this.settings,
    this.platform,
    this.interfaceFont,
    this.terminalFont,
    this.appearance,
    this.keyBarWriter,
    this.saveShowKeyBar,
    this.pickTtf,
  });
  final DesktopSettingsController controller;
  final DesktopSettingsSection initialSection;

  /// 打开时最近的旧偏好状态；键位条基线与显示开关来自它及后续 SettingsState。
  final SettingsState? settings;
  final TargetPlatform? platform;
  final InterfaceTypography? interfaceFont;
  final InterfaceTypography? terminalFont;

  /// 仅 Windows 使用；默认全应用单例。
  final WindowsAppearance? appearance;

  /// 默认全应用共享 writer；页面关闭只分离，不释放。
  final KeyBarWriter? keyBarWriter;
  final ValueChanged<bool>? saveShowKeyBar;
  final Future<XFile?> Function()? pickTtf;

  @override
  State<DesktopSettingsPage> createState() => DesktopSettingsPageState();
}

class DesktopSettingsPageState extends State<DesktopSettingsPage> {
  final _forms = GlobalKey<DesktopSettingsFormsState>();
  final _resources = GlobalKey<DesktopSettingsResourcesState>();
  late DesktopSettingsSection _section = widget.initialSection;
  late final TargetPlatform _platform =
      widget.platform ?? defaultTargetPlatform;
  late final InterfaceTypography _terminalTypography =
      widget.terminalFont ?? InterfaceTypography.terminal;
  late final _FontAdapter _interface;
  late final _FontAdapter _terminal;
  _AppearanceAdapter? _appearance;
  late final KeyBarDraft _keyBarDraft;
  late final KeyBarEditorController _keyBar;
  SettingsState? _settings;
  StreamSubscription<dynamic>? _settingsSubscription;
  bool _closing = false;

  DesktopSettingsSection get section => _section;

  @override
  void initState() {
    super.initState();
    _settings = widget.settings;
    _interface = _FontAdapter(
      widget.interfaceFont ?? InterfaceTypography.instance,
      DesktopFontSlot.interface,
      _pickResourceTtf,
    );
    _terminal = _FontAdapter(
      _terminalTypography,
      DesktopFontSlot.terminal,
      _pickResourceTtf,
    );
    if (_platform == TargetPlatform.windows) {
      _appearance = _AppearanceAdapter(
        widget.appearance ?? WindowsAppearance.instance,
      );
    }
    final rows = _settings?.keyBarRows;
    _keyBarDraft = KeyBarDraft(
      rows != null && validKeyBarRows(rows) ? rows : defaultKeyBarRows,
    );
    final writer =
        widget.keyBarWriter ??
        (KeyBarWriter.shared..verifier ??= const SettingsQueryKeyBarVerifier());
    _keyBar = KeyBarEditorController(draft: _keyBarDraft, writer: writer);
    _settingsSubscription = SettingsState.rustSignalStream.listen((pack) {
      if (mounted) setState(() => _settings = pack.message);
    });
  }

  @override
  void dispose() {
    _settingsSubscription?.cancel();
    _keyBar.dispose();
    _keyBarDraft.dispose();
    _interface.dispose();
    _terminal.dispose();
    _appearance?.dispose();
    super.dispose();
  }

  Future<XFile?> _openTtf() =>
      widget.pickTtf?.call() ??
      openFile(acceptedTypeGroups: DesktopTtfPickRequest.typeMetadata);

  Future<XFile?> _pickResourceTtf(
    BuildContext context,
    DesktopTtfPickRequest request,
  ) => _openTtf();

  /// 复用全局终端槽的 32 MiB、Rust 校验与字体注册；只产出草稿，不保存。
  Future<DesktopTtfSelection?> _pickProfileTtf(
    BuildContext context,
    String profileId,
  ) async {
    final file = await _openTtf();
    if (file == null) return null;
    final draft = await prepareDesktopTtfPreview(_terminalTypography, file);
    return DesktopTtfSelection(family: draft.family, bytes: draft.bytes);
  }

  void _saveShowKeyBar(bool value) {
    final save = widget.saveShowKeyBar;
    if (save != null) {
      save(value);
    } else {
      SaveSettings(showKeyBar: value).sendSignalToRust();
    }
  }

  /// 关闭按钮、Esc 与路由返回共用 forms 的确认入口，并入资源区与键位条草稿。
  Future<void> requestClose() async {
    if (_closing) return;
    _closing = true;
    try {
      await _forms.currentState?.requestClose(
        extraDirty:
            (_resources.currentState?.hasUnsavedChanges ?? false) ||
            _keyBarDraft.dirty,
        dialogContext: context,
      );
    } finally {
      _closing = false;
    }
  }

  void _select(DesktopSettingsSection value) =>
      setState(() => _section = value);

  Widget _navigation(bool narrow) {
    final items = [
      for (final value in DesktopSettingsSection.values)
        ToggleButton(
          key: ValueKey('settings-nav-${value.name}'),
          checked: value == _section,
          autofocus: value == widget.initialSection,
          onChanged: (_) => _select(value),
          child: Align(
            alignment: narrow ? Alignment.center : Alignment.centerLeft,
            widthFactor: narrow ? null : 1,
            child: Text(value.label),
          ),
        ),
    ];
    if (narrow) {
      return Padding(
        padding: const EdgeInsets.symmetric(horizontal: DesktopSpacing.lg),
        child: Wrap(
          spacing: DesktopSpacing.sm,
          runSpacing: DesktopSpacing.sm,
          children: items,
        ),
      );
    }
    return SizedBox(
      width: DesktopMetrics.paneWidth,
      child: Padding(
        padding: const EdgeInsets.all(DesktopSpacing.lg),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            for (final item in items)
              Padding(
                padding: const EdgeInsets.only(bottom: DesktopSpacing.sm),
                child: item,
              ),
          ],
        ),
      ),
    );
  }

  Widget _scroll(Widget child) => SingleChildScrollView(
    padding: const EdgeInsets.all(DesktopSpacing.xl),
    child: Align(
      alignment: Alignment.topLeft,
      child: ConstrainedBox(
        constraints: const BoxConstraints(
          maxWidth: DesktopMetrics.contentWidth,
        ),
        child: child,
      ),
    ),
  );

  Widget _keyBarPage() {
    final settings = _settings;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(
            DesktopSpacing.xl,
            DesktopSpacing.xl,
            DesktopSpacing.xl,
            0,
          ),
          child: Align(
            alignment: Alignment.centerLeft,
            child: ToggleSwitch(
              key: const ValueKey('settings-show-keybar'),
              checked: settings?.showKeyBar ?? false,
              content: const Text('显示键位条'),
              onChanged: settings == null ? null : _saveShowKeyBar,
            ),
          ),
        ),
        Expanded(child: DesktopKeyBarEditor(controller: _keyBar)),
      ],
    );
  }

  @override
  Widget build(BuildContext context) {
    final narrow =
        MediaQuery.sizeOf(context).width < DesktopMetrics.narrowWidth;
    final content = IndexedStack(
      sizing: StackFit.expand,
      index: switch (_section) {
        DesktopSettingsSection.app || DesktopSettingsSection.profile => 0,
        DesktopSettingsSection.appearance => 1,
        DesktopSettingsSection.keybar => 2,
        DesktopSettingsSection.about => 3,
      },
      children: [
        DesktopSettingsForms(
          key: _forms,
          controller: widget.controller,
          onPickTtf: _pickProfileTtf,
          onClose: () => Navigator.of(context).pop(),
          section: _section == DesktopSettingsSection.profile
              ? DesktopSettingsFormSection.profile
              : DesktopSettingsFormSection.app,
          onShowApp: () => _select(DesktopSettingsSection.app),
        ),
        _scroll(
          DesktopSettingsResources(
            key: _resources,
            interfaceFont: _interface.callbacks,
            terminalFont: _terminal.callbacks,
            inputBoundary: const _ReadyBoundary(),
            platform: _platform,
            appearance: _appearance?.callbacks,
            applicationName: _applicationName,
            applicationVersion: _applicationVersion,
            registry: _licenses,
            showAbout: false,
          ),
        ),
        _keyBarPage(),
        _scroll(
          const DesktopAboutResources(
            applicationName: _applicationName,
            applicationVersion: _applicationVersion,
            registry: _licenses,
            enabled: true,
          ),
        ),
      ],
    );
    return PopScope(
      canPop: false,
      onPopInvokedWithResult: (didPop, _) {
        if (!didPop) requestClose();
      },
      child: Actions(
        actions: {
          DismissIntent: CallbackAction<DismissIntent>(
            onInvoke: (_) {
              requestClose();
              return null;
            },
          ),
        },
        // 切页排除原焦点时焦点退回本域，Esc 仍能找到上方的关闭动作。
        child: FocusScope(
          autofocus: true,
          child: Padding(
            padding: EdgeInsets.all(
              narrow ? DesktopSpacing.sm : DesktopSpacing.xl,
            ),
            child: Center(
              child: ConstrainedBox(
                constraints: const BoxConstraints(
                  maxWidth:
                      DesktopMetrics.paneWidth + DesktopMetrics.contentWidth,
                ),
                child: ClipRRect(
                  borderRadius: BorderRadius.circular(DesktopMetrics.radius),
                  child: ColoredBox(
                    // 与 ContentDialog 同为不透明弹层底色；scaffoldBackgroundColor
                    // 默认半透明，会让透明窗口画布与后方页面透进来。
                    color: FluentTheme.of(context).menuColor,
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        Padding(
                          padding: const EdgeInsets.all(DesktopSpacing.lg),
                          child: Row(
                            children: [
                              Expanded(
                                child: Text(
                                  '设置',
                                  style: FluentTheme.of(context)
                                      .typography
                                      .titleLarge,
                                ),
                              ),
                              Button(
                                key: const ValueKey('close-settings'),
                                onPressed: requestClose,
                                child: const Text('关闭设置'),
                              ),
                            ],
                          ),
                        ),
                        Expanded(
                          child: narrow
                              ? Column(
                                  crossAxisAlignment:
                                      CrossAxisAlignment.stretch,
                                  children: [
                                    _navigation(true),
                                    Expanded(child: content),
                                  ],
                                )
                              : Row(
                                  crossAxisAlignment:
                                      CrossAxisAlignment.stretch,
                                  children: [
                                    _navigation(false),
                                    Expanded(child: content),
                                  ],
                                ),
                        ),
                      ],
                    ),
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// 设置在独立对话框路由中，焦点与终端输入已由壳在打开前静默；边界恒为就绪。
class _ReadyBoundary implements DesktopResourcesInputBoundary {
  const _ReadyBoundary();
  @override
  bool get ready => true;
  @override
  int get generation => 0;
  @override
  Future<void> acquire() async {}
  @override
  void release() {}
  @override
  void addListener(VoidCallback listener) {}
  @override
  void removeListener(VoidCallback listener) {}
}

/// 把全局字体槽的 apply/reset 结果映射为资源区写入状态。
class _FontAdapter {
  _FontAdapter(
    this.typography,
    DesktopFontSlot slot,
    DesktopResourceTtfPicker pick,
  ) {
    typography.addListener(_sync);
    callbacks = DesktopFontResourceCallbacks(
      identity: DesktopResourceIdentity(this, 0),
      slot: slot,
      state: state,
      pick: pick,
      prepare: (file) => prepareDesktopTtfPreview(typography, file),
      apply: (draft) => _write(() => typography.apply(draft)),
      reset: () => _write(typography.reset),
    );
  }
  final InterfaceTypography typography;
  late final state = ValueNotifier(_derive());
  late final DesktopFontResourceCallbacks callbacks;
  int _verification = 0;
  bool _disposed = false;

  DesktopFontSlotState _derive() => DesktopFontSlotState(
    savedFamily: typography.family,
    savedLabel: typography.custom
        ? typography.label
        : '${typography.label}（内置）',
    effectiveFamily: typography.family,
    fallbackReason: typography.error,
    verificationVersion: _verification,
  );

  void _sync() {
    if (!_disposed) state.value = _derive();
  }

  Future<DesktopResourceWriteResult<DesktopFontSlotState>> _write(
    Future<void> Function() action,
  ) async {
    try {
      await action();
      _sync();
      return DesktopResourceWriteResult.confirmed(_derive());
    } on TimeoutException {
      unawaited(_verify());
      return const DesktopResourceWriteResult.uncertain();
    } catch (_) {
      return const DesktopResourceWriteResult.failed();
    }
  }

  /// Rust 按序处理字体请求：其后的 query 成功即说明超时的写入已结算。
  Future<void> _verify() async {
    try {
      await typography.refresh();
    } catch (_) {
      return;
    }
    if (_disposed) return;
    ++_verification;
    _sync();
  }

  void dispose() {
    _disposed = true;
    typography.removeListener(_sync);
    state.dispose();
  }
}

/// 预览复用 WindowsAppearance.preview；持久值与隔离状态由全应用单例持有。
class _AppearanceAdapter {
  _AppearanceAdapter(this.appearance) {
    appearance.addListener(_sync);
    callbacks = DesktopAppearanceCallbacks(
      identity: DesktopResourceIdentity(this, 0),
      state: state,
      preview: (value) =>
          appearance.preview(enabled: value.acrylic, value: value.opacity),
      save: appearance.persist,
      restoreSavedPreview: _restore,
    );
  }
  final WindowsAppearance appearance;
  late final state = ValueNotifier(_derive());
  late final DesktopAppearanceCallbacks callbacks;
  bool _disposed = false;

  DesktopAppearanceState _derive() => DesktopAppearanceState(
    saved: appearance.saved,
    materialAvailable: appearance.materialAvailable,
    writeReady: appearance.writeReady,
    verificationVersion: appearance.verification,
  );

  void _sync() {
    if (!_disposed) state.value = _derive();
  }

  // 卸载期间回撤会通知窗口框架重建，延后到微任务避免锁定的树内标脏。
  void _restore() => scheduleMicrotask(() {
    final saved = appearance.saved;
    appearance.preview(enabled: saved.acrylic, value: saved.opacity);
  });

  void dispose() {
    _disposed = true;
    appearance.removeListener(_sync);
    state.dispose();
  }
}

/// 仅在旧 KeyBarLayoutResult 已被 writer 消耗后调用。Rust 设置循环按序处理，
/// 此后收到的 SettingsState 都反映该写入结算后的偏好，可作为真实来源证据。
class SettingsQueryKeyBarVerifier implements KeyBarVerifier {
  const SettingsQueryKeyBarVerifier();

  @override
  Future<KeyBarVerificationProof?> verify(KeyBarSettlementToken token) async {
    final next = SettingsState.rustSignalStream.first.timeout(
      const Duration(seconds: 10),
    );
    try {
      const SettingsQuery().sendSignalToRust();
    } catch (_) {
      next.ignore();
      rethrow;
    }
    final state = (await next).message;
    return KeyBarVerificationProof.afterSettlement(
      token: token,
      rows: state.keyBarRows,
      evidence: state,
    );
  }
}
