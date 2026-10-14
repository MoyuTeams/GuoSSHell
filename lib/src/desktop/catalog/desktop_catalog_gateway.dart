import 'dart:async';

import '../../bindings/bindings.dart';
import '../../catalog/catalog_requests.dart';
import '../workspace/desktop_target_options.dart';

const _version = 1;
const _timeout = Duration(seconds: 15);
int _nextRequestId = 1;

/// Rust 以错误码拒绝桌面目录请求；[message] 可直接展示。
class DesktopCatalogRejected implements Exception {
  const DesktopCatalogRejected(this.message);
  final String message;
}

String _desktopErrorText(DesktopCatalogError error) => switch (error) {
  DesktopCatalogError.none => '',
  DesktopCatalogError.hostRequired => catalogErrorText(
    CatalogError.hostRequired,
  ),
  DesktopCatalogError.hostInvalid => catalogErrorText(CatalogError.hostInvalid),
  DesktopCatalogError.portInvalid => catalogErrorText(CatalogError.portInvalid),
  DesktopCatalogError.usernameRequired => catalogErrorText(
    CatalogError.usernameRequired,
  ),
  DesktopCatalogError.passwordRequired => catalogErrorText(
    CatalogError.passwordRequired,
  ),
  DesktopCatalogError.keyRequired => catalogErrorText(CatalogError.keyRequired),
  DesktopCatalogError.notFound => catalogErrorText(CatalogError.notFound),
  DesktopCatalogError.keychain => catalogErrorText(CatalogError.keychain),
  DesktopCatalogError.storage => catalogErrorText(CatalogError.storage),
  DesktopCatalogError.invalidProfileId ||
  DesktopCatalogError.profileNotFound => '所选终端配置已不存在，请重新选择',
  DesktopCatalogError.usernameInvalid => '用户名无效',
  DesktopCatalogError.invalidConnectionId => '连接标识无效',
  DesktopCatalogError.unsupportedVersion ||
  DesktopCatalogError.desktopOnly ||
  DesktopCatalogError.unsupportedTransport ||
  DesktopCatalogError.targetMismatch ||
  DesktopCatalogError.invalidIdentity ||
  DesktopCatalogError.invalidAuthentication ||
  DesktopCatalogError.credentialMismatch => '保存失败：请求与当前连接类型不匹配',
};

/// 桌面目录网关：保存带终端 profile 引用，复制／删除沿用现有目录协议。
abstract class DesktopCatalogGateway {
  Stream<CatalogState> get catalogStates;
  Stream<KeyListState> get keyStates;

  void query(String text);
  void queryKeys();

  /// 保存失败时可抛 [DesktopCatalogRejected]；超时抛 [TimeoutException]。
  Future<CatalogResult> save(DesktopConnectionDraft draft);

  /// 已有连接当前引用的 profile id；空串表示跟随应用默认。
  Future<String> connectionProfileId(String id);

  /// 可选的终端 profile 列表。
  Future<List<DesktopProfileOption>> terminalProfiles();
  Future<CatalogResult> duplicate(String id);
  Future<CatalogResult> delete(String id);
}

/// 密码只在表单提交时短暂传入请求，不从目录摘要反向读取。
class DesktopConnectionDraft {
  final String id;
  final String name;
  final String host;
  final int port;
  final String username;
  final AuthMethod auth;
  final PasswordAction passwordAction;
  final String password;
  final String keyId;
  final String command;

  /// 终端 profile 引用；空串表示跟随应用默认。
  final String profileId;

  const DesktopConnectionDraft({
    required this.id,
    required this.name,
    required this.host,
    required this.port,
    required this.username,
    required this.auth,
    required this.passwordAction,
    required this.password,
    required this.keyId,
    required this.command,
    required this.profileId,
  });
}

class RustDesktopCatalogGateway implements DesktopCatalogGateway {
  const RustDesktopCatalogGateway();

  @override
  Stream<CatalogState> get catalogStates =>
      CatalogState.rustSignalStream.map((pack) => pack.message);

  @override
  Stream<KeyListState> get keyStates =>
      KeyListState.rustSignalStream.map((pack) => pack.message);

  @override
  void query(String text) => CatalogQuery(query: text).sendSignalToRust();

  @override
  void queryKeys() => KeyQuery().sendSignalToRust();

  /// 桌面保存走 [DesktopSaveConnection]（带 profile_id）；回包为
  /// [DesktopCatalogResult]，失败以 [DesktopCatalogRejected] 抛出可读文案。
  @override
  Future<CatalogResult> save(DesktopConnectionDraft draft) async {
    final requestId = _nextRequestId++;
    // 先订阅再发：Rust 的回答可能比 Future 的回调注册更早到。
    final reply = DesktopCatalogResult.rustSignalStream
        .map((pack) => pack.message)
        .firstWhere((result) => result.requestId == requestId)
        .timeout(_timeout);
    DesktopSaveConnection(
      version: _version,
      requestId: requestId,
      id: draft.id,
      name: draft.name,
      targetKind: DesktopCatalogTargetKind.ssh,
      transport: DesktopTransport.nativeSsh,
      host: draft.host,
      port: draft.port,
      username: draft.username,
      auth: switch (draft.auth) {
        AuthMethod.password => DesktopAuthMethod.password,
        AuthMethod.publicKey => DesktopAuthMethod.publicKey,
        AuthMethod.keyboardInteractive => DesktopAuthMethod.keyboardInteractive,
      },
      passwordAction: draft.passwordAction,
      password: draft.password,
      keyId: draft.keyId,
      systemIdentityPath: '',
      command: draft.command,
      profileId: draft.profileId,
    ).sendSignalToRust();
    final result = await reply;
    if (result.error != DesktopCatalogError.none) {
      throw DesktopCatalogRejected(_desktopErrorText(result.error));
    }
    return CatalogResult(
      requestId: result.requestId,
      error: CatalogError.none,
      connectionId: result.connectionId,
    );
  }

  @override
  Future<String> connectionProfileId(String id) async {
    final requestId = _nextRequestId++;
    final reply = DesktopCatalogState.rustSignalStream
        .map((pack) => pack.message)
        .firstWhere((state) => state.requestId == requestId)
        .timeout(_timeout);
    DesktopCatalogQuery(
      version: _version,
      requestId: requestId,
      query: '',
    ).sendSignalToRust();
    final state = await reply;
    if (state.error != DesktopCatalogError.none) {
      throw DesktopCatalogRejected(_desktopErrorText(state.error));
    }
    for (final connection in state.connections) {
      if (connection.id == id) return connection.profileId;
    }
    throw const DesktopCatalogRejected('这条连接已不存在');
  }

  @override
  Future<List<DesktopProfileOption>> terminalProfiles() async =>
      (await const RustDesktopTargetOptionsGateway().query()).profiles;

  @override
  Future<CatalogResult> duplicate(String id) => duplicateConnection(id);

  @override
  Future<CatalogResult> delete(String id) => deleteConnection(id);
}
