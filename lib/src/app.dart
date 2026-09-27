import 'dart:async';

import 'package:fluent_ui/fluent_ui.dart' as fluent;
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_localizations/flutter_localizations.dart';
import 'package:path_provider/path_provider.dart';

import 'bindings/bindings.dart';
import 'catalog/connection_list_page.dart';
import 'desktop/desktop_shell.dart';
import 'desktop/desktop_status.dart';
import 'desktop/desktop_theme.dart';
import 'desktop/legacy_material_surface.dart';
import 'desktop/windows_chrome.dart';
import 'desktop/windows_shell.dart';
import 'platform_target.dart';
import 'settings/interface_font.dart';
import 'terminal/session_target.dart';
import 'workspace/workspace_page.dart';

/// 调试用的自动连接（debug 构建，见 README）：启动后直接以快速连接打开终端。取值先看
/// --dart-define，没有就用 Rust 随 AppReady 转交的进程环境变量（XCUITest、simctl 传入）。
const _definedHost = String.fromEnvironment('GUOSH_HOST');
const _definedPort = int.fromEnvironment('GUOSH_PORT', defaultValue: 22);
const _definedUser = String.fromEnvironment('GUOSH_USER');
const _definedPass = String.fromEnvironment('GUOSH_PASS');
const _definedCmd = String.fromEnvironment('GUOSH_CMD');

class GuoSSHellApp extends StatelessWidget {
  final AppTarget? targetOverride;
  final StartupPort? startupOverride;
  final Widget? legacyHomeOverride;

  const GuoSSHellApp({
    super.key,
    this.targetOverride,
    this.startupOverride,
    @visibleForTesting this.legacyHomeOverride,
  });

  @override
  Widget build(BuildContext context) {
    final fontFamily = InterfaceFontScope.fontFamilyOf(context);
    final target = targetOverride ?? appTargetFor(defaultTargetPlatform);
    if (target == AppTarget.desktop && isWindowsDesktop) {
      return MaterialApp(
        title: 'GuoSSHell',
        debugShowCheckedModeBanner: false,
        localizationsDelegates: const [
          GlobalMaterialLocalizations.delegate,
          GlobalWidgetsLocalizations.delegate,
          GlobalCupertinoLocalizations.delegate,
          fluent.FluentLocalizations.delegate,
        ],
        supportedLocales: const [Locale('zh'), Locale('en')],
        builder: (context, child) => fluent.FluentTheme(
          data: desktopFluentTheme(fontFamily: fontFamily),
          child: WindowsFrame(child: child!),
        ),
        theme: desktopMaterialTheme(fontFamily: fontFamily),
        home: _StartupGate(
          desktop: true,
          windows: true,
          startup: startupOverride,
        ),
      );
    }
    if (target == AppTarget.desktop) {
      return fluent.FluentApp(
        title: 'GuoSSHell',
        builder: (context, child) {
          final media = MediaQuery.of(context);
          return fluent.FluentTheme(
            data: desktopTheme(
              highContrast: media.highContrast,
              reduceMotion:
                  media.disableAnimations || media.accessibleNavigation,
            ),
            child: child!,
          );
        },
        home: _StartupGate(
          desktop: true,
          startup: startupOverride,
          legacyHome: legacyHomeOverride,
        ),
      );
    }
    return MaterialApp(
      title: 'GuoSSHell',
      theme: ThemeData(
        fontFamily: fontFamily,
        brightness: Brightness.dark,
        colorScheme: ColorScheme.fromSeed(
          seedColor: Colors.teal,
          brightness: Brightness.dark,
        ),
      ),
      home: _StartupGate(startup: startupOverride),
    );
  }
}

/// 将启动事件的测试接缝与 Rust 业务契约隔离；运行时始终使用真实 rinf 信号。
abstract class StartupPort {
  StreamSubscription<dynamic> listenReady(
    void Function(bool, String, SessionTarget?) onReady,
  );
  StreamSubscription<dynamic> listenSettings(void Function() onSettings);
  Future<String> supportDirectory();
  void start(String directory);
  void querySettings();
  void foreground(bool foreground);
}

class _RustStartupPort implements StartupPort {
  const _RustStartupPort();

  @override
  StreamSubscription<dynamic> listenReady(
    void Function(bool, String, SessionTarget?) onReady,
  ) => AppReady.rustSignalStream.listen(
    (pack) => onReady(
      pack.message.ok,
      pack.message.detail,
      _StartupGateState._autoConnectTarget(pack.message),
    ),
  );

  @override
  StreamSubscription<dynamic> listenSettings(void Function() onSettings) =>
      SettingsState.rustSignalStream.listen((_) => onSettings());

  @override
  Future<String> supportDirectory() async =>
      (await getApplicationSupportDirectory()).path;

  @override
  void start(String directory) =>
      AppStart(supportDir: directory).sendSignalToRust();

  @override
  void querySettings() {
    if (isWindowsDesktop) WindowsAppearanceQuery().sendSignalToRust();
    SettingsQuery().sendSignalToRust();
  }

  @override
  void foreground(bool foreground) =>
      AppLifecycle(foreground: foreground).sendSignalToRust();
}

/// 启动：把数据目录交给 Rust 打开存储，等它就绪、设置到位后进入连接列表。
class _StartupGate extends StatefulWidget {
  final bool desktop;
  final bool windows;
  final StartupPort? startup;
  final Widget? legacyHome;

  const _StartupGate({
    this.desktop = false,
    this.windows = false,
    this.startup,
    this.legacyHome,
  });

  @override
  State<_StartupGate> createState() => _StartupGateState();
}

class _StartupGateState extends State<_StartupGate> {
  StreamSubscription? _readySub;
  StreamSubscription? _settingsSub;
  bool _ready = false;
  bool _loadingFonts = false;
  String? _error;

  /// 自动连接的目标（没有为 null）。
  SessionTarget? _autoTarget;

  /// 前后台切换告诉 Rust（进后台时它向系统申请一小段后台时间，连接不会立刻挂起）。
  late final AppLifecycleListener _lifecycle;
  late final StartupPort _startup;

  @override
  void initState() {
    super.initState();
    _startup = widget.startup ?? const _RustStartupPort();
    _lifecycle = AppLifecycleListener(
      onShow: () => _startup.foreground(true),
      onHide: () => _startup.foreground(false),
    );
    _readySub = _startup.listenReady((ok, detail, autoTarget) {
      if (!mounted) return;
      if (ok) {
        _autoTarget = autoTarget;
        _startup.querySettings();
      } else {
        setState(() => _error = detail);
      }
    });
    // 终端页的字体取自设置：第一份设置到了才算就绪。
    _settingsSub = _startup.listenSettings(() async {
      if (!mounted || _ready) return;
      if (_loadingFonts) return;
      _loadingFonts = true;
      try {
        // 注入启动接缝时不访问真实字体 transport；生产仍等待两个字体槽就绪。
        if (widget.startup == null) {
          await Future.wait([
            InterfaceTypography.instance.refresh(),
            InterfaceTypography.terminal.refresh(),
          ]);
        }
      } catch (error) {
        if (mounted) setState(() => _error = '无法读取字体设置：$error');
        return;
      }
      if (!mounted) return;
      setState(() => _ready = true);
      _autoConnect();
    });
    _start();
  }

  Future<void> _start() async {
    try {
      final directory = await _startup.supportDirectory();
      if (!mounted) return;
      _startup.start(directory);
    } catch (error) {
      if (mounted) setState(() => _error = '$error');
    }
  }

  static SessionTarget? _autoConnectTarget(AppReady ready) {
    if (!kDebugMode) return null;
    if (_definedHost.isNotEmpty) {
      return const SessionTarget.quick(
        host: _definedHost,
        port: _definedPort,
        username: _definedUser,
        password: _definedPass,
        command: _definedCmd,
      );
    }
    if (ready.autoHost.isEmpty) return null;
    return SessionTarget.quick(
      host: ready.autoHost,
      port: ready.autoPort,
      username: ready.autoUser,
      password: ready.autoPass,
      command: ready.autoCommand,
    );
  }

  void _autoConnect() {
    if (widget.windows) return;
    final target = _autoTarget;
    if (target == null) return;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      if (widget.desktop) {
        _openLegacy(target);
      } else {
        Navigator.of(context).push(
          MaterialPageRoute<void>(
            builder: (_) => WorkspacePage(initial: target),
          ),
        );
      }
    });
  }

  void _openLegacy([SessionTarget? target]) {
    final reduceMotion =
        MediaQuery.disableAnimationsOf(context) ||
        MediaQuery.accessibleNavigationOf(context);
    Navigator.of(context).push(
      _DesktopLegacyRoute(
        reduceMotion: reduceMotion,
        builder: (_) => LegacyMaterialSurface(
          initialTarget: target,
          homeOverride: widget.legacyHome,
        ),
      ),
    );
  }

  @override
  void dispose() {
    _lifecycle.dispose();
    _readySub?.cancel();
    _settingsSub?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final error = _error;
    if (widget.desktop) {
      if (error != null) {
        return DesktopStatus(
          kind: DesktopStatusKind.error,
          title: '无法打开数据存储',
          detail: '请检查应用数据存储权限后重新启动。',
        );
      }
      if (!_ready) {
        return const DesktopStatus(
          kind: DesktopStatusKind.loading,
          title: '正在准备工作台',
          detail: '等待 Rust 就绪与首份设置。',
        );
      }
      return widget.windows
          ? WindowsShell(initial: _autoTarget)
          : DesktopShell(openLegacy: _openLegacy);
    }
    if (error != null) {
      return Scaffold(
        body: Center(
          child: Padding(
            padding: const EdgeInsets.all(24),
            child: Text('无法打开数据存储：$error', textAlign: TextAlign.center),
          ),
        ),
      );
    }
    if (!_ready) {
      return const Scaffold(body: Center(child: CircularProgressIndicator()));
    }
    return const ConnectionListPage();
  }
}

class _DesktopLegacyRoute extends fluent.FluentPageRoute<void> {
  final bool reduceMotion;

  _DesktopLegacyRoute({required super.builder, required this.reduceMotion});

  @override
  Duration get transitionDuration =>
      reduceMotion ? Duration.zero : super.transitionDuration;

  @override
  Duration get reverseTransitionDuration =>
      reduceMotion ? Duration.zero : super.reverseTransitionDuration;
}
