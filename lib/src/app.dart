import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:path_provider/path_provider.dart';
import 'package:fluent_ui/fluent_ui.dart' as f;
import 'package:flutter_localizations/flutter_localizations.dart';

import 'bindings/bindings.dart';
import 'catalog/connection_list_page.dart';
import 'terminal/session_target.dart';
import 'workspace/workspace_page.dart';
import 'desktop/windows_chrome.dart';
import 'desktop/windows_shell.dart';
import 'settings/interface_font.dart';

/// 调试用的自动连接（debug 构建，见 README）：启动后直接以快速连接打开终端。取值先看
/// --dart-define，没有就用 Rust 随 AppReady 转交的进程环境变量（XCUITest、simctl 传入）。
const _definedHost = String.fromEnvironment('GUOSH_HOST');
const _definedPort = int.fromEnvironment('GUOSH_PORT', defaultValue: 22);
const _definedUser = String.fromEnvironment('GUOSH_USER');
const _definedPass = String.fromEnvironment('GUOSH_PASS');
const _definedCmd = String.fromEnvironment('GUOSH_CMD');

class GuoSSHellApp extends StatelessWidget {
  const GuoSSHellApp({super.key});

  @override
  Widget build(BuildContext context) {
    final fontFamily = InterfaceFontScope.fontFamilyOf(context);
    return MaterialApp(
      title: 'GuoSSHell',
      debugShowCheckedModeBanner: !isWindowsDesktop,
      localizationsDelegates: isWindowsDesktop
          ? const [
              GlobalMaterialLocalizations.delegate,
              GlobalWidgetsLocalizations.delegate,
              GlobalCupertinoLocalizations.delegate,
              f.FluentLocalizations.delegate,
            ]
          : null,
      supportedLocales: isWindowsDesktop
          ? const [Locale('zh'), Locale('en')]
          : const [Locale('en', 'US')],
      builder: isWindowsDesktop
          ? (context, child) => f.FluentTheme(
              data: desktopFluentTheme(fontFamily: fontFamily),
              child: WindowsFrame(child: child!),
            )
          : null,
      theme: isWindowsDesktop
          ? desktopMaterialTheme(fontFamily: fontFamily)
          : ThemeData(
              fontFamily: fontFamily,
              brightness: Brightness.dark,
              colorScheme: ColorScheme.fromSeed(
                seedColor: Colors.teal,
                brightness: Brightness.dark,
              ),
            ),
      home: const _StartupGate(),
    );
  }
}

/// 启动：把数据目录交给 Rust 打开存储，等它就绪、设置到位后进入连接列表。
class _StartupGate extends StatefulWidget {
  const _StartupGate();

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

  @override
  void initState() {
    super.initState();
    _lifecycle = AppLifecycleListener(
      onShow: () => AppLifecycle(foreground: true).sendSignalToRust(),
      onHide: () => AppLifecycle(foreground: false).sendSignalToRust(),
    );
    _readySub = AppReady.rustSignalStream.listen((pack) {
      if (!mounted) return;
      if (pack.message.ok) {
        _autoTarget = _autoConnectTarget(pack.message);
        if (isWindowsDesktop) WindowsAppearanceQuery().sendSignalToRust();
        SettingsQuery().sendSignalToRust();
      } else {
        setState(() => _error = pack.message.detail);
      }
    });
    // 终端页的字体取自设置：第一份设置到了才算就绪。
    _settingsSub = SettingsState.rustSignalStream.listen((_) async {
      if (!mounted || _ready) return;
      if (_loadingFonts) return;
      _loadingFonts = true;
      try {
        await Future.wait([
          InterfaceTypography.instance.refresh(),
          InterfaceTypography.terminal.refresh(),
        ]);
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
      final directory = await getApplicationSupportDirectory();
      AppStart(supportDir: directory.path).sendSignalToRust();
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
    if (isWindowsDesktop) return;
    final target = _autoTarget;
    if (target == null) return;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      Navigator.of(context).push(
        MaterialPageRoute<void>(builder: (_) => WorkspacePage(initial: target)),
      );
    });
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
    return isWindowsDesktop
        ? WindowsShell(initial: _autoTarget)
        : const ConnectionListPage();
  }
}
