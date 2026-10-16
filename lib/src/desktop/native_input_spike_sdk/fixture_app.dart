import 'dart:async';

import 'package:fluent_ui/fluent_ui.dart';

import '../desktop_status.dart';
import '../desktop_theme.dart';
import 'bootstrap.dart';

/// 纯 bootstrap 面板，不挂接普通壳、窗口输入、终端或真实配置。
class SdkFixtureApp extends StatelessWidget {
  const SdkFixtureApp({
    super.key,
    required this.bootstrap,
    required this.onClose,
  });
  final SdkBootstrap bootstrap;
  final VoidCallback onClose;

  @override
  Widget build(BuildContext context) => FluentApp(
    debugShowCheckedModeBanner: false,
    title: 'SDK bootstrap fixture',
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
      content: ListenableBuilder(
        listenable: bootstrap,
        builder: (context, _) => DesktopPage(
          title: 'SDK 隔离握手',
          description: '仅核验受控 fixture 的启动模式和同 module。没有输入 owner、真实配置、认证或网络。',
          children: [
            Semantics(
              liveRegion: true,
              child: Text(switch (bootstrap.phase) {
                BootstrapPhase.idle =>
                  bootstrap.explicitOptIn ? '尚未运行；SDK 未启动。' : '默认关闭；SDK 未启动。',
                BootstrapPhase.preflight => '正在核验 fixture-only 构建；SDK 尚未启动。',
                BootstrapPhase.initializing => '隔离预检通过，正在启动公共 SDK。',
                BootstrapPhase.waiting => '等待 SDK 实际 nonce 回应；尚未证明同 module。',
                BootstrapPhase.verifying => '已收到 SDK 回应，正在核验 module 映射。',
                BootstrapPhase.proven => '同 module 握手通过；输入 owner 仍为 0。',
                BootstrapPhase.failed => bootstrap.failure,
                BootstrapPhase.closed => '已关闭本次 bootstrap。',
              }, style: FluentTheme.of(context).typography.body),
            ),
            const SizedBox(height: DesktopSpacing.lg),
            if (bootstrap.diagnostic case final diagnostic?) ...[
              Text(
                'native 状态：${diagnostic.status}；core ABI：${diagnostic.coreAbi}；指针位数：${diagnostic.pointerBits}',
              ),
              const SizedBox(height: DesktopSpacing.sm),
              Text(
                'Rinf 导出：${diagnostic.rinfExports}；core 导出：${diagnostic.coreExports}；owner：${diagnostic.owners}；同 module：${diagnostic.sameModule ? 1 : 0}',
              ),
              const SizedBox(height: DesktopSpacing.lg),
            ],
            Wrap(
              spacing: DesktopSpacing.md,
              runSpacing: DesktopSpacing.md,
              children: [
                FilledButton(
                  key: const Key('sdk-fixture-run'),
                  onPressed: bootstrap.canRun
                      ? () => unawaited(bootstrap.run())
                      : null,
                  child: const Text('运行一次'),
                ),
                Button(
                  key: const Key('sdk-fixture-close'),
                  onPressed: onClose,
                  child: const Text('关闭'),
                ),
              ],
            ),
            const SizedBox(height: DesktopSpacing.lg),
            const Text(
              '此结果不证明 engine、Windows pre-IME／TSF、完整输入集成或普通 GUI 回归。失败与超时均不绑定、不重试。',
            ),
          ],
        ),
      ),
    ),
  );
}
