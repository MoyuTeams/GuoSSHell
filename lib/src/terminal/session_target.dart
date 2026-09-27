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

  const SessionTarget.saved({required this.connectionId, required String title})
      : _name = title,
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
  })  : connectionId = '',
        _name = '';

  /// 页面与提示里显示的名字。
  String get title => connectionId.isEmpty ? '$username@$host' : _name;
}
