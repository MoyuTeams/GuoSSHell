import 'dart:async';

import 'package:fluent_ui/fluent_ui.dart' as f;
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:window_manager/window_manager.dart';

import '../bindings/bindings.dart';
import 'desktop_app_color_scheme.dart';
import 'desktop_theme.dart';
import 'settings/resources/desktop_resources_contracts.dart';

bool get isWindowsDesktop =>
    !kIsWeb && defaultTargetPlatform == TargetPlatform.windows;

const desktopAccent = Color(0xff76c9ff);
const desktopBackground = Color(0xff191d25);
const desktopMuted = Color(0xffa7b1c2);

/// 默认配色下的弹层底色（fluent_ui 深色 menuColor）。
const desktopMenu = Color(0xff2c2c2c);

/// dev 工作区的透明表面与分隔色，不改变既有 Windows 材质方向。
abstract final class DesktopWorkspaceColors {
  static final surface = const Color(0xff10141b).withValues(alpha: 0.3);
  static const border = Colors.white10;
  static const divider = Color(0xff323b4b);
}

/// 保留 dev 工具栏高度，窄工作区横向滚动而不压缩动作命中区。
abstract final class DesktopWorkspaceMetrics {
  static const toolbarHeight = 40.0;
  static const toolbarMinWidth = 360.0;
  static const terminalBackgroundOpacity = 0.12;
}

const desktopFontFallback = [
  'Microsoft YaHei UI',
  'Microsoft YaHei',
  'Segoe UI Symbol',
];

/// Segoe UI 不包含中文字形，显式选择 Windows 的无衬线中文界面字体。
/// [colorScheme] 为应用配色：`default` 保持设计 token，其余由终端配色派生，
/// 高对比度时忽略。窗口画布见 [desktopCanvas]。
f.FluentThemeData desktopFluentTheme({
  String? fontFamily,
  bool highContrast = false,
  bool reduceMotion = false,
  String colorScheme = 'default',
}) {
  final palette = highContrast ? null : desktopAppPalette(colorScheme);
  final ramp = f.Typography.fromBrightness(
    brightness: palette?.brightness ?? Brightness.dark,
    color: palette?.text,
  );
  TextStyle? cjk(TextStyle? style) =>
      style?.copyWith(fontFamilyFallback: desktopFontFallback);
  final accessibility = desktopTheme(
    highContrast: highContrast,
    reduceMotion: reduceMotion,
    fontFamily: fontFamily ?? DesktopType.family,
  );
  // 弹层（菜单、下拉、选择工具栏）底色：不透明，跟随应用配色。fluent_ui 在其上
  // 叠加 menuColor × 0.65，二者取同色使弹层呈现单一实色。
  final menu = highContrast
      ? DesktopColors.highContrastCanvas
      : palette?.surface ?? desktopMenu;
  return f.FluentThemeData(
    menuColor: menu,
    acrylicBackgroundColor: menu,
    brightness: palette?.brightness ?? Brightness.dark,
    accentColor: highContrast
        ? accessibility.accentColor
        : palette == null
        ? f.Colors.blue
        : f.AccentColor.swatch({
            for (final shade in [
              'darkest',
              'darker',
              'dark',
              'normal',
              'light',
              'lighter',
              'lightest',
            ])
              shade: palette.accent,
          }),
    resources: highContrast ? accessibility.resources : null,
    focusTheme: highContrast
        ? accessibility.focusTheme
        : f.FocusThemeData(
            primaryBorder: BorderSide(
              color: palette?.accent ?? desktopAccent,
              width: DesktopMetrics.focus,
            ),
            secondaryBorder: BorderSide(
              color: palette?.canvas ?? desktopBackground,
            ),
          ),
    micaBackgroundColor: palette?.canvas,
    fasterAnimationDuration: accessibility.fasterAnimationDuration,
    fastAnimationDuration: accessibility.fastAnimationDuration,
    mediumAnimationDuration: accessibility.mediumAnimationDuration,
    slowAnimationDuration: accessibility.slowAnimationDuration,
    fontFamily: fontFamily ?? DesktopType.family,
    typography: f.Typography.raw(
      display: cjk(ramp.display),
      titleLarge: cjk(ramp.titleLarge),
      title: cjk(ramp.title),
      subtitle: cjk(ramp.subtitle),
      bodyLarge: cjk(ramp.bodyLarge),
      bodyStrong: cjk(ramp.bodyStrong),
      body: cjk(ramp.body),
      caption: cjk(ramp.caption)?.copyWith(fontWeight: FontWeight.normal),
    ),
    scaffoldBackgroundColor: highContrast
        ? DesktopColors.highContrastCanvas
        : palette?.surface,
  );
}

/// 窗口画布：应用配色为 `default` 或高对比度时保持 [desktopBackground]。
Color desktopCanvas(String colorScheme) =>
    desktopAppPalette(colorScheme)?.canvas ?? desktopBackground;

/// 原生窗口操作集中在 Windows 入口，其他平台不初始化桌面插件。
class WindowsDesktopWindow {
  static bool initialized = false;
  static Future<bool> Function()? confirmClose;

  static Future<void> initialize() async {
    if (!isWindowsDesktop) return;
    await windowManager.ensureInitialized();
    await windowManager.setTitleBarStyle(
      TitleBarStyle.hidden,
      windowButtonVisibility: false,
    );
    await windowManager.setMinimumSize(const Size(760, 520));
    await windowManager.setBackgroundColor(Colors.transparent);
    await windowManager.setPreventClose(true);
    // 首次应用失败时 applyMaterial 会关闭材质，保留完全不透明且可操作的窗口。
    WindowsAppearance.instance.materialAvailable = true;
    initialized = true;
    await WindowsAppearance.instance.applyMaterial();
    WindowsAppearance.instance.connect();
  }
}

/// 外观预览与全应用唯一的持久化写入口；合法值校验由 Rust 完成。
class WindowsAppearance extends ChangeNotifier {
  WindowsAppearance({
    Stream<WindowsAppearanceState>? states,
    void Function(SaveWindowsAppearance request)? send,
  }) : _states =
           states ??
           WindowsAppearanceState.rustSignalStream.map((pack) => pack.message),
       _send = send ?? ((request) => request.sendSignalToRust());
  static final instance = WindowsAppearance();
  // 由 Windows 宿主 windows/runner/window_material.cpp 实现。
  static const _material = MethodChannel('guosshell/window_material');
  final Stream<WindowsAppearanceState> _states;
  final void Function(SaveWindowsAppearance request) _send;
  bool acrylicEnabled = true;
  double opacity = 0.78;
  bool materialAvailable = false;

  /// 最近一次 Rust 回报的持久值；预览不改变它。
  DesktopAppearanceValue saved = const DesktopAppearanceValue(
    acrylic: true,
    opacity: 0.78,
  );

  /// 查询固定使用的 request_id；保存从 1 起全应用递增，查询回包不会被当成保存结果。
  static const queryRequestId = 0;
  static int _lastRequestId = queryRequestId;

  /// 超时后保持隔离，直到同一 request_id 的迟到回报结算该写入。
  int verification = 0;
  bool _sending = false;
  bool _timedOut = false;
  int? _pendingRequestId;
  bool get writeReady => !_sending && !_timedOut;
  StreamSubscription? _subscription;
  Future<void> _effectQueue = Future.value();

  static int _nextRequestId() =>
      _lastRequestId = _lastRequestId % 0xffffffff + 1;

  void connect() {
    _subscription ??= _states.listen((state) {
      acrylicEnabled = state.acrylic;
      opacity = state.opacity;
      saved = DesktopAppearanceValue(
        acrylic: state.acrylic,
        opacity: state.opacity,
      );
      if (_timedOut && state.requestId == _pendingRequestId) {
        _timedOut = false;
        _pendingRequestId = null;
        ++verification;
      }
      notifyListeners();
      unawaited(applyMaterial());
    });
  }

  void preview({bool? enabled, double? value}) {
    final changedEffect = enabled != null && enabled != acrylicEnabled;
    acrylicEnabled = enabled ?? acrylicEnabled;
    opacity = (value ?? opacity).clamp(0.35, 1.0);
    notifyListeners();
    // 连续拖动只改变 Flutter 背景遮罩，避免反复重建 DWM 材质。
    if (changedEffect) unawaited(applyMaterial());
  }

  /// 先订阅再发送，只接受同一 request_id 的回包；error 非空为已关联失败，
  /// 超时为结果待核实。
  Future<DesktopResourceWriteResult<DesktopAppearanceValue>> persist(
    DesktopAppearanceValue value,
  ) async {
    if (!writeReady) throw StateError('上一外观写入尚未结算');
    _sending = true;
    final requestId = _pendingRequestId = _nextRequestId();
    notifyListeners();
    final reply = _states
        .firstWhere((state) => state.requestId == requestId)
        .timeout(const Duration(seconds: 10));
    try {
      _send(
        SaveWindowsAppearance(
          requestId: requestId,
          acrylic: value.acrylic,
          opacity: value.opacity,
        ),
      );
    } catch (_) {
      // 发送异常不能证明 Rust 未收到，按待核实处理。
      reply.ignore();
      _timedOut = true;
      _sending = false;
      notifyListeners();
      return const DesktopResourceWriteResult.uncertain();
    }
    try {
      final state = await reply;
      _pendingRequestId = null;
      saved = DesktopAppearanceValue(
        acrylic: state.acrylic,
        opacity: state.opacity,
      );
      return state.error.isEmpty
          ? DesktopResourceWriteResult.confirmed(saved)
          : const DesktopResourceWriteResult.failed();
    } on TimeoutException {
      _timedOut = true;
      return const DesktopResourceWriteResult.uncertain();
    } finally {
      _sending = false;
      notifyListeners();
    }
  }

  Future<void> applyMaterial() {
    if (!materialAvailable) return Future.value();
    _effectQueue = _effectQueue.then((_) async {
      try {
        await _material.invokeMethod<void>('setMaterial', {
          'acrylic': acrylicEnabled,
          // 原生层只提供模糊，色调透明度统一由 Flutter 背景遮罩控制。
          'color':
              (acrylicEnabled ? const Color(0x01191d25) : desktopBackground)
                  .toARGB32(),
        });
      } catch (_) {
        materialAvailable = false;
        notifyListeners();
      }
    });
    return _effectQueue;
  }
}

/// 包在 Navigator 外面，设置页及认证对话框打开时也保留自绘窗口控件。
class WindowsFrame extends StatefulWidget {
  const WindowsFrame({
    super.key,
    required this.child,
    this.background = desktopBackground,
  });
  final Widget child;

  /// 窗口画布色，随应用配色变化；透明度仍由外观设置控制。
  final Color background;

  @override
  State<WindowsFrame> createState() => _WindowsFrameState();
}

class _WindowsFrameState extends State<WindowsFrame> with WindowListener {
  bool _maximized = false;
  bool _closing = false;

  @override
  void initState() {
    super.initState();
    if (WindowsDesktopWindow.initialized) windowManager.addListener(this);
  }

  @override
  void dispose() {
    if (WindowsDesktopWindow.initialized) windowManager.removeListener(this);
    super.dispose();
  }

  @override
  void onWindowMaximize() => setState(() => _maximized = true);
  @override
  void onWindowUnmaximize() => setState(() => _maximized = false);

  @override
  void onWindowClose() async {
    if (_closing) return;
    _closing = true;
    try {
      if (await (WindowsDesktopWindow.confirmClose?.call() ??
          Future.value(true))) {
        await windowManager.destroy();
      }
    } finally {
      _closing = false;
    }
  }

  @override
  Widget build(BuildContext context) {
    final frame = AnimatedBuilder(
      animation: WindowsAppearance.instance,
      builder: (context, _) {
        final appearance = WindowsAppearance.instance;
        final transparent =
            appearance.acrylicEnabled &&
            appearance.materialAvailable &&
            !MediaQuery.highContrastOf(context);
        // 浅色画布（如日晒浅色）上标题与窗口按钮改用深色。
        final brightness = ThemeData.estimateBrightnessForColor(
          widget.background,
        );
        return ColoredBox(
          color: widget.background.withValues(
            alpha: transparent ? appearance.opacity : 1,
          ),
          child: Column(
            children: [
              SizedBox(
                height: 40,
                child: Row(
                  children: [
                    Expanded(
                      child: DragToMoveArea(
                        child: Container(
                          color: Colors.transparent,
                          padding: const EdgeInsets.only(left: 18),
                          child: Row(
                            children: [
                              Image.asset(
                                'assets/icon/app_icon.png',
                                width: 20,
                                height: 20,
                                excludeFromSemantics: true,
                              ),
                              const SizedBox(width: 12),
                              Text(
                                'GuoSSHell',
                                style: TextStyle(
                                  fontSize: 12,
                                  fontWeight: FontWeight.w600,
                                  color: brightness == Brightness.light
                                      ? Colors.black87
                                      : null,
                                ),
                              ),
                              const SizedBox(width: 16),
                              const Flexible(
                                child: Text(
                                  'SSH 工作区',
                                  overflow: TextOverflow.ellipsis,
                                  style: TextStyle(
                                    fontSize: 12,
                                    color: desktopMuted,
                                  ),
                                ),
                              ),
                            ],
                          ),
                        ),
                      ),
                    ),
                    _caption(
                      '最小化',
                      WindowCaptionButton.minimize(
                        onPressed: windowManager.minimize,
                        brightness: brightness,
                      ),
                    ),
                    _caption(
                      _maximized ? '还原' : '最大化',
                      _maximized
                          ? WindowCaptionButton.unmaximize(
                              onPressed: windowManager.unmaximize,
                              brightness: brightness,
                            )
                          : WindowCaptionButton.maximize(
                              onPressed: windowManager.maximize,
                              brightness: brightness,
                            ),
                    ),
                    _caption(
                      '关闭窗口',
                      WindowCaptionButton.close(
                        onPressed: windowManager.close,
                        brightness: brightness,
                      ),
                    ),
                  ],
                ),
              ),
              Expanded(child: widget.child),
            ],
          ),
        );
      },
    );
    return _WindowOverlay(
      child: WindowsDesktopWindow.initialized
          ? VirtualWindowFrame(child: frame)
          : frame,
    );
  }

  Widget _caption(String label, Widget child) => Semantics(
    label: label,
    button: true,
    child: f.Tooltip(
      message: label,
      child: SizedBox(width: 46, height: 40, child: child),
    ),
  );
}

/// 标题栏位于应用 Navigator 外，单独提供提示气泡所需的 Overlay。
class _WindowOverlay extends StatefulWidget {
  const _WindowOverlay({required this.child});
  final Widget child;
  @override
  State<_WindowOverlay> createState() => _WindowOverlayState();
}

class _WindowOverlayState extends State<_WindowOverlay> {
  late final _entry = OverlayEntry(builder: (_) => widget.child);
  @override
  void didUpdateWidget(_WindowOverlay oldWidget) {
    super.didUpdateWidget(oldWidget);
    _entry.markNeedsBuild();
  }

  @override
  void dispose() {
    _entry.remove();
    _entry.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => Overlay(initialEntries: [_entry]);
}

/// 兼容现有页面中的 Material 终端画布与平台对话框。
ThemeData desktopMaterialTheme({String? fontFamily}) => ThemeData(
  brightness: Brightness.dark,
  fontFamily: fontFamily ?? 'Segoe UI',
  fontFamilyFallback: desktopFontFallback,
  scaffoldBackgroundColor: Colors.transparent,
  canvasColor: desktopBackground,
  colorScheme: ColorScheme.fromSeed(
    seedColor: desktopAccent,
    brightness: Brightness.dark,
  ),
  visualDensity: VisualDensity.compact,
  appBarTheme: const AppBarTheme(
    backgroundColor: Colors.transparent,
    elevation: 0,
  ),
  dialogTheme: const DialogThemeData(
    backgroundColor: Color(0xff252a34),
    shape: RoundedRectangleBorder(
      borderRadius: BorderRadius.all(Radius.circular(8)),
    ),
  ),
);
