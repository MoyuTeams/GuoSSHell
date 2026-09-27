import 'dart:async';

import '../bindings/bindings.dart';

/// 目录请求：带请求号发出，等同号的 [CatalogResult]。
int _nextRequestId = 1;

const _timeout = Duration(seconds: 15);

Future<CatalogResult> _request(void Function(int requestId) send) {
  final requestId = _nextRequestId++;
  // 先订阅再发：Rust 的回答可能比 Future 的回调注册更早到。
  final result = CatalogResult.rustSignalStream
      .map((pack) => pack.message)
      .firstWhere((result) => result.requestId == requestId)
      .timeout(_timeout);
  send(requestId);
  return result;
}

Future<CatalogResult> saveConnection({
  required String id,
  required String name,
  required String host,
  required int port,
  required String username,
  required AuthMethod auth,
  required PasswordAction passwordAction,
  required String password,
  required String keyId,
  required String command,
}) {
  return _request((requestId) => SaveConnection(
        requestId: requestId,
        id: id,
        name: name,
        host: host,
        port: port,
        username: username,
        auth: auth,
        passwordAction: passwordAction,
        password: password,
        keyId: keyId,
        command: command,
      ).sendSignalToRust());
}

Future<CatalogResult> deleteConnection(String id) => _request(
    (requestId) => DeleteConnection(requestId: requestId, id: id).sendSignalToRust());

Future<CatalogResult> duplicateConnection(String id) => _request(
    (requestId) => DuplicateConnection(requestId: requestId, id: id).sendSignalToRust());

String catalogErrorText(CatalogError error) => switch (error) {
      CatalogError.none => '',
      CatalogError.hostRequired => '请填写主机',
      CatalogError.hostInvalid => '主机地址无效',
      CatalogError.portInvalid => '端口无效',
      CatalogError.usernameRequired => '请填写用户名',
      CatalogError.passwordRequired => '请填写要保存的密码',
      CatalogError.keyRequired => '请选择私钥',
      CatalogError.notFound => '这条连接已不存在',
      CatalogError.keychain => '钥匙串读写失败',
      CatalogError.storage => '保存失败',
    };

/// 列表与标题里显示的名字：没起名就用 用户@主机。
String connectionTitle(ConnectionSummary connection) => connection.name.isNotEmpty
    ? connection.name
    : '${connection.username}@${connection.host}';
