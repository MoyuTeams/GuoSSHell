import '../../bindings/bindings.dart';
import '../../catalog/catalog_requests.dart';

/// 仅复用现有目录协议；不在桌面层定义尚未落地的传输或配置字段。
abstract class DesktopCatalogGateway {
  Stream<CatalogState> get catalogStates;
  Stream<KeyListState> get keyStates;

  void query(String text);
  void queryKeys();
  Future<CatalogResult> save(DesktopConnectionDraft draft);
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

  @override
  Future<CatalogResult> save(DesktopConnectionDraft draft) => saveConnection(
    id: draft.id,
    name: draft.name,
    host: draft.host,
    port: draft.port,
    username: draft.username,
    auth: draft.auth,
    passwordAction: draft.passwordAction,
    password: draft.password,
    keyId: draft.keyId,
    command: draft.command,
  );

  @override
  Future<CatalogResult> duplicate(String id) => duplicateConnection(id);

  @override
  Future<CatalogResult> delete(String id) => deleteConnection(id);
}
