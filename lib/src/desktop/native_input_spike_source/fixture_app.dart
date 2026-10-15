import 'dart:async';

import 'package:fluent_ui/fluent_ui.dart';

import '../desktop_status.dart';
import '../desktop_theme.dart';
import '../native_input_spike_sdk/bootstrap.dart';
import 'adapter.dart';

/// 不加载普通壳、配置、认证或终端；按钮只投递固定合成 probe。
class SourceFixtureApp extends StatefulWidget {
  const SourceFixtureApp({
    super.key,
    required this.bootstrap,
    required this.source,
    required this.explicitOptIn,
    required this.onClosed,
  });
  final SdkBootstrap bootstrap;
  final SourceFixtureAdapter source;
  final bool explicitOptIn;
  final VoidCallback onClosed;

  @override
  State<SourceFixtureApp> createState() => _SourceFixtureAppState();
}

class _SourceFixtureAppState extends State<SourceFixtureApp> {
  bool _busy = false;
  bool _attempted = false;
  bool _opened = false;
  bool _sourceAttempted = false;
  String _status = '默认不创建 source；仅受控双开关允许运行一次。';
  SourceNumbers? _numbers;

  Future<void> _run() async {
    if (_attempted || !widget.explicitOptIn) return;
    setState(() {
      _busy = true;
      _attempted = true;
      _status = '正在核验公共 SDK 实际同 module 回应。';
    });
    try {
      await widget.bootstrap.run();
      if (widget.bootstrap.phase != BootstrapPhase.proven) {
        if (mounted) setState(() => _status = widget.bootstrap.failure);
        return;
      }
      _sourceAttempted = true;
      final numbers = await widget.source.open(explicitOptIn: true);
      if (!mounted) return;
      setState(() {
        _numbers = numbers;
        _opened = numbers.number('openHr') == 0;
        _status = _opened ? '自有 source 已创建；仅合成输入，物理来源未知。' : 'source 创建未通过；不重试。';
      });
    } catch (_) {
      if (mounted) setState(() => _status = '受控接线不可用；不显示本机路径或异常正文。');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _probe() async {
    setState(() => _busy = true);
    try {
      final status = await widget.source.probe(SourceProbe.f4Down);
      await Future<void>.delayed(const Duration(milliseconds: 100));
      final numbers = await widget.source.inspect();
      if (mounted) {
        setState(() {
          _numbers = numbers;
          _status = '固定 F4 投递状态：$status；不证明物理设备或 GUI 动作。';
        });
      }
    } catch (_) {
      if (mounted) setState(() => _status = '合成探针不可用；请关闭夹具。');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _close() async {
    setState(() {
      _busy = true;
      _status = '停止准入并等待非 UI 管理线程发布真实 join 结果。';
    });
    try {
      if (_sourceAttempted) {
        final numbers = await closeSourceFixture(widget.source);
        if (mounted) setState(() => _numbers = numbers);
        if (!numbers.joinedAndEmpty) {
          if (mounted) {
            setState(() => _status = '关闭尚未确认；原生 owner 保留，动态库保持进程寿命 PIN。');
          }
          return;
        }
        _opened = false;
        _sourceAttempted = false;
      }
      widget.bootstrap.close();
      if (mounted) {
        setState(() => _status = 'source 真实 join 和零 charge 已确认；SDK 已关闭。');
      }
      widget.onClosed();
    } catch (_) {
      if (mounted) setState(() => _status = '关闭结果不可用；不释放未确认的原生 owner。');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) => FluentApp(
    debugShowCheckedModeBanner: false,
    title: '原生 source 私人夹具',
    theme: desktopTheme(highContrast: false, reduceMotion: false),
    builder: (context, child) => FluentTheme(
      data: desktopTheme(
        highContrast: MediaQuery.highContrastOf(context),
        reduceMotion:
            MediaQuery.disableAnimationsOf(context) ||
            MediaQuery.accessibleNavigationOf(context),
      ),
      child: child!,
    ),
    home: ScaffoldPage(
      content: DesktopPage(
        title: '同 SDK source 接线',
        description: '仅私人受控 DLL；没有登录、真实配置、正常终端或任意键入口。',
        children: [
          Semantics(liveRegion: true, child: Text(_status)),
          const SizedBox(height: DesktopSpacing.lg),
          if (_numbers case final numbers?) ...[
            Text(
              'owner：${numbers.number('owners')}；Gate：${numbers.number('gate')}；实际循环 handled：${numbers.number('loopHandled')}',
            ),
            const SizedBox(height: DesktopSpacing.sm),
            Text(
              'charge slots：${numbers.number('chargedSlots')}；bytes：${numbers.number('chargedBytes')}；inflight：${numbers.number('inflight')}',
            ),
            const SizedBox(height: DesktopSpacing.lg),
          ],
          Wrap(
            spacing: DesktopSpacing.md,
            runSpacing: DesktopSpacing.md,
            children: [
              FilledButton(
                key: const Key('source-fixture-run'),
                onPressed: !_busy && !_attempted && widget.explicitOptIn
                    ? () => unawaited(_run())
                    : null,
                child: const Text('核验并创建一次'),
              ),
              Button(
                key: const Key('source-fixture-probe'),
                onPressed: !_busy && _opened ? () => unawaited(_probe()) : null,
                child: const Text('投递固定 F4'),
              ),
              Button(
                key: const Key('source-fixture-close'),
                onPressed: !_busy ? () => unawaited(_close()) : null,
                child: const Text('关闭'),
              ),
            ],
          ),
          const SizedBox(height: DesktopSpacing.lg),
          const Text(
            '物理输入、IME／文本／粘贴／Pointer 不受支持。Action 仅记录意图，不执行 shell、分栏或菜单；本页不证明完整 A／4C／P1。',
          ),
        ],
      ),
    ),
  );
}
