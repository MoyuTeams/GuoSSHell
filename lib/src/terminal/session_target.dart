import 'package:flutter/foundation.dart';

/// 会话要连的目标：目录里的一条连接，或快速连接（不存目录）。
@immutable
class SessionTarget {
  /// 目录里的连接 id；快速连接为空。
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

  const SessionTarget.saved({
    required this.connectionId,
    required String title,
    this.profileId = '',
  }) : _name = title,
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
  }) : connectionId = '',
       _name = '';

  /// 页面与提示里显示的名字。
  String get title => connectionId.isEmpty ? '$username@$host' : _name;
}
