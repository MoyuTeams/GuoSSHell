import 'dart:async';

import 'package:fluent_ui/fluent_ui.dart';

import '../../bindings/bindings.dart';
import '../../terminal/session_target.dart';
import '../desktop_theme.dart';

class DesktopProfileOption {
  const DesktopProfileOption(this.id, this.name);
  final String id;
  final String name;
}

/// 只查询无秘密的版本化设置/能力；不解析 profile，也不参与 prepare。
class DesktopTargetOptions {
  const DesktopTargetOptions({
    required this.profiles,
    required this.nativeAvailable,
    this.localAvailable = false,
    this.systemAvailable = false,
    this.nativeReason = '',
    this.systemReason = 'System OpenSSH 尚未开放，不能启动连接。',
    this.localReason = '本地 shell 尚未开放，不能启动会话。',
  });
  final List<DesktopProfileOption> profiles;
  final bool nativeAvailable;
  final bool localAvailable;
  final bool systemAvailable;
  final String nativeReason;
  final String systemReason;
  final String localReason;
}

abstract interface class DesktopTargetOptionsGateway {
  Future<DesktopTargetOptions> query();
}

class RustDesktopTargetOptionsGateway implements DesktopTargetOptionsGateway {
  const RustDesktopTargetOptionsGateway();
  static int _sequence = 0;

  @override
  Future<DesktopTargetOptions> query() async {
    final sequence = ++_sequence;
    final requestId = 'workspace-profile-$sequence';
    final settings = Completer<DesktopSettingsResult>();
    final capabilities = Completer<DesktopCapabilitiesResult>();
    final settingsSub = DesktopSettingsResult.rustSignalStream.listen((pack) {
      if (pack.message.requestId == requestId && !settings.isCompleted) {
        settings.complete(pack.message);
      }
    });
    final capabilitiesSub = DesktopCapabilitiesResult.rustSignalStream.listen((
      pack,
    ) {
      if (pack.message.requestId == sequence && !capabilities.isCompleted) {
        capabilities.complete(pack.message);
      }
    });
    try {
      DesktopSettingsRequest(
        protocolVersion: 1,
        requestId: requestId,
        baseRevision: Uint64(BigInt.zero),
        operation: 'query',
        targetId: '',
        name: '',
      ).sendSignalToRust();
      DesktopCapabilitiesQuery(
        protocolVersion: 1,
        requestId: sequence,
      ).sendSignalToRust();
      // 两个等待都立即安装错误处理，超时后释放订阅，不留下在途 UI 查询。
      final replies = await Future.wait<Object>([
        settings.future,
        capabilities.future,
      ]).timeout(const Duration(seconds: 10));
      final config = replies[0] as DesktopSettingsResult;
      final capability = replies[1] as DesktopCapabilitiesResult;
      if (config.protocolVersion != 1 ||
          config.status != 'ok' ||
          !config.capabilities.available ||
          config.snapshot == null ||
          capability.protocolVersion != 1 ||
          capability.status != DesktopCapabilitiesStatus.ready) {
        throw const FormatException('桌面目标配置不可用');
      }
      String reason(DesktopTargetKind kind, String fallback) {
        for (final item in capability.transports) {
          if (item.target == kind) {
            return item.unavailableReason.isEmpty
                ? fallback
                : item.unavailableReason;
          }
        }
        return fallback;
      }

      bool available(DesktopTargetKind kind) => capability.transports.any(
        (item) => item.target == kind && item.backendAvailable,
      );

      return DesktopTargetOptions(
        profiles: [
          for (final profile in config.snapshot!.profiles)
            DesktopProfileOption(profile.id, profile.name),
        ],
        nativeAvailable: available(DesktopTargetKind.nativeSsh),
        localAvailable: available(DesktopTargetKind.localShell),
        systemAvailable: available(DesktopTargetKind.systemOpenSsh),
        nativeReason: reason(DesktopTargetKind.nativeSsh, 'Native SSH 当前不可用。'),
        systemReason: reason(
          DesktopTargetKind.systemOpenSsh,
          'System OpenSSH 尚未开放。',
        ),
        localReason: reason(DesktopTargetKind.localShell, '本地 shell 尚未开放。'),
      );
    } finally {
      await settingsSub.cancel();
      await capabilitiesSub.cancel();
    }
  }
}

/// 只改变此次显式 profile，快速目标秘密不进入查询、选项或任何准备字段。
SessionTarget desktopTargetWithProfile(
  SessionTarget target,
  String profileId,
) => target.isLocal
    ? SessionTarget.local(profileId: profileId)
    : target.connectionId.isNotEmpty
    ? SessionTarget.saved(
        connectionId: target.connectionId,
        title: target.title,
        profileId: profileId,
        systemSsh: target.isSystemSsh,
      )
    : SessionTarget.quick(
        host: target.host,
        port: target.port,
        username: target.username,
        password: target.password,
        command: target.command,
        profileId: profileId,
      );

class DesktopProfilePicker extends StatelessWidget {
  const DesktopProfilePicker({
    super.key,
    required this.options,
    required this.value,
    required this.onChanged,
    required this.saved,
    this.systemSsh = false,
  });
  final DesktopTargetOptions options;
  final String value;
  final ValueChanged<String> onChanged;
  final bool saved;

  /// 所连目录连接是 System OpenSSH；传输由连接决定，这里只展示。
  final bool systemSsh;

  String get _local =>
      options.localAvailable ? '可用，从侧栏「本地终端」打开' : options.localReason;

  String get _system =>
      options.systemAvailable ? '可用，在连接编辑器的「连接方式」中选择' : options.systemReason;

  @override
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.stretch,
    children: [
      DropDownButton(
        key: const ValueKey('workspace-transport'),
        title: Text(systemSsh ? '传输：System OpenSSH' : '传输：Native SSH'),
        items: [
          MenuFlyoutItem(
            text: Text(systemSsh ? 'Native SSH' : 'Native SSH（当前传输）'),
            onPressed: null,
          ),
          MenuFlyoutItem(
            text: Text(
              systemSsh ? 'System OpenSSH（当前传输）' : 'System OpenSSH：$_system',
            ),
            onPressed: null,
          ),
          MenuFlyoutItem(text: Text('本地 shell：$_local'), onPressed: null),
        ],
      ),
      const SizedBox(height: DesktopSpacing.md),
      const Text('终端 profile（本次会话）'),
      const SizedBox(height: DesktopSpacing.sm),
      ComboBox<String>(
        key: const ValueKey('workspace-profile'),
        value: value,
        isExpanded: true,
        items: [
          ComboBoxItem(value: '', child: Text(saved ? '按连接引用／应用默认' : '按应用默认')),
          for (final profile in options.profiles)
            ComboBoxItem(value: profile.id, child: Text(profile.name)),
        ],
        onChanged: (id) {
          if (id != null) onChanged(id);
        },
      ),
      const SizedBox(height: DesktopSpacing.sm),
      const Text('显式选择优先；未选择时由 Rust 解析连接引用／应用默认。'),
      const SizedBox(height: DesktopSpacing.md),
      Text('System OpenSSH：$_system'),
      const SizedBox(height: DesktopSpacing.sm),
      Text('本地 shell：$_local'),
    ],
  );
}
