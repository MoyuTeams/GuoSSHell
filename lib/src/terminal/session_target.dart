import 'package:flutter/foundation.dart';

/// 会话目标种类：SSH（目录连接或快速连接）、本机 shell（仅桌面），或在本地终端里
/// 运行系统 ssh 的目录连接（仅桌面）。
enum SessionTargetKind { ssh, local, systemSsh }

/// 会话要连的目标：目录里的一条连接、快速连接（不存目录），或本地终端。
@immutable
class SessionTarget {
  final SessionTargetKind kind;

  /// 目录里的连接 id；快速连接与本地终端为空。
  final String connectionId;

  /// 目录里的连接显示的名字。
  final String _name;

  // 以下仅快速连接使用；目录里的连接由 Rust 按 id 取配置。
  final String host;
  final int port;
  final String username;

  /// 为空时连接过程中弹框问。
  final String password;
  final String command;

  /// 桌面准备路径的显式选择；空值保留连接引用、应用默认的解析顺序。
  final String profileId;

  /// [systemSsh] 为目录里的 System OpenSSH 连接：由系统 ssh 在终端里自行认证。
  const SessionTarget.saved({
    required this.connectionId,
    required String title,
    this.profileId = '',
    bool systemSsh = false,
  }) : kind = systemSsh ? SessionTargetKind.systemSsh : SessionTargetKind.ssh,
       _name = title,
       host = '',
       port = 0,
       username = '',
       password = '',
       command = '';

  const SessionTarget.quick({
    required this.host,
    required this.port,
    required this.username,
    this.password = '',
    this.command = '',
    this.profileId = '',
  }) : kind = SessionTargetKind.ssh,
       connectionId = '',
       _name = '';

  /// 本机默认 shell；不认证，无主机、端口、用户名与密码。
  const SessionTarget.local({String? profileId})
    : kind = SessionTargetKind.local,
      profileId = profileId ?? '',
      connectionId = '',
      _name = '',
      host = '',
      port = 0,
      username = '',
      password = '',
      command = '';

  bool get isLocal => kind == SessionTargetKind.local;

  bool get isSystemSsh => kind == SessionTargetKind.systemSsh;

  /// 会话是本机子进程（本地 shell 或系统 ssh）：结束时带进程退出码。
  bool get runsLocalProcess => isLocal || isSystemSsh;

  /// 页面与提示里显示的名字。
  String get title => isLocal
      ? '本地终端'
      : connectionId.isEmpty
      ? '$username@$host'
      : _name;
}
