import 'dart:async';

import '../../bindings/bindings.dart';
import '../../catalog/catalog_requests.dart';
import '../../terminal/session_target.dart';
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

  /// 已有连接的桌面目录投影：profile 引用（空串跟随应用默认）、连接方式与
  /// System 私钥文件路径。不含任何秘密。
  Future<DesktopConnectionSummary> connection(String id);

  /// 可选的终端 profile 列表与各传输的可用性。
  Future<DesktopTargetOptions> targetOptions();
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

  /// 连接方式：Native SSH 或 System OpenSSH。System 时 [auth]、密码与 [keyId]
  /// 不使用，认证由系统 ssh 处理。
  final DesktopTransport transport;

  /// System OpenSSH 可选的私钥文件路径；空串表示由 ssh 自己的配置与 agent 决定。
  final String systemIdentityPath;

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
    this.transport = DesktopTransport.nativeSsh,
    this.systemIdentityPath = '',
  });

  bool get systemSsh => transport == DesktopTransport.systemOpenSsh;
}

/// 目录连接的会话目标：System 连接带上传输，Prepare 按它选择目标种类。
SessionTarget desktopSavedTarget(
  ConnectionSummary item, {
  String profileId = '',
}) => SessionTarget.saved(
  connectionId: item.id,
  title: connectionTitle(item),
  profileId: profileId,
  systemSsh: item.transport == DesktopTransport.systemOpenSsh,
);

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
    desktopSaveRequest(draft, requestId).sendSignalToRust();
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
  Future<DesktopConnectionSummary> connection(String id) async {
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
      if (connection.id == id) return connection;
    }
    throw const DesktopCatalogRejected('这条连接已不存在');
  }

  @override
  Future<DesktopTargetOptions> targetOptions() =>
      const RustDesktopTargetOptionsGateway().query();

  @override
  Future<CatalogResult> duplicate(String id) => duplicateConnection(id);

  @override
  Future<CatalogResult> delete(String id) => deleteConnection(id);
}

/// 草稿 → 桌面保存请求。System 连接不带密码与应用私钥；认证按是否给了私钥文件取
/// PublicKey（ssh -i）或 Agent（由 ssh 的配置与 agent 决定）。
DesktopSaveConnection desktopSaveRequest(
  DesktopConnectionDraft draft,
  int requestId,
) {
  final system = draft.systemSsh;
  final identity = system ? draft.systemIdentityPath.trim() : '';
  return DesktopSaveConnection(
    version: _version,
    requestId: requestId,
    id: draft.id,
    name: draft.name,
    targetKind: DesktopCatalogTargetKind.ssh,
    transport: draft.transport,
    host: draft.host,
    port: draft.port,
    username: draft.username,
    auth: system
        ? (identity.isEmpty
              ? DesktopAuthMethod.agent
              : DesktopAuthMethod.publicKey)
        : switch (draft.auth) {
            AuthMethod.password => DesktopAuthMethod.password,
            AuthMethod.publicKey => DesktopAuthMethod.publicKey,
            AuthMethod.keyboardInteractive =>
              DesktopAuthMethod.keyboardInteractive,
          },
    // 切到 System 时清掉可能存过的 Native 密码。
    passwordAction: system ? PasswordAction.clear : draft.passwordAction,
    password: system ? '' : draft.password,
    keyId: system ? '' : draft.keyId,
    systemIdentityPath: identity,
    command: draft.command,
    profileId: draft.profileId,
  );
}
