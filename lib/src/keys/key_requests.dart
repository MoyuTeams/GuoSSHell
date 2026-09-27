import 'dart:async';

import '../bindings/bindings.dart';

/// 私钥请求：带请求号发出，等同号的 [KeyResult]。
int _nextRequestId = 1;

const _timeout = Duration(seconds: 30);

Future<KeyResult> _request(void Function(int requestId) send, {Duration timeout = _timeout}) {
  final requestId = _nextRequestId++;
  final result = KeyResult.rustSignalStream
      .map((pack) => pack.message)
      .firstWhere((result) => result.requestId == requestId)
      .timeout(timeout);
  send(requestId);
  return result;
}

Future<KeyResult> importKey({
  required String name,
  required String privateKey,
  required String passphrase,
}) =>
    _request((requestId) => ImportKey(
          requestId: requestId,
          name: name,
          privateKey: privateKey,
          passphrase: passphrase,
        ).sendSignalToRust());

Future<KeyResult> renameKey(String id, String name) => _request(
    (requestId) => RenameKey(requestId: requestId, id: id, name: name).sendSignalToRust());

Future<KeyResult> deleteKey(String id) =>
    _request((requestId) => DeleteKey(requestId: requestId, id: id).sendSignalToRust());

Future<KeyResult> forgetPassphrase(String id) => _request(
    (requestId) => ForgetPassphrase(requestId: requestId, id: id).sendSignalToRust());

Future<KeyResult> addCardKey(String ident, String name) => _request((requestId) =>
    AddCardKey(requestId: requestId, ident: ident, name: name).sendSignalToRust());

/// 在安全密钥上新建凭据并登记。系统界面等用户插上、靠近或触摸安全密钥（最多约 3 分钟），
/// 超时放宽。
Future<KeyResult> registerSecurityKey(String name) => _request(
    (requestId) => RegisterSecurityKey(requestId: requestId, name: name).sendSignalToRust(),
    timeout: const Duration(seconds: 200));

/// 读卡。NFC 读卡要等用户把卡靠近，超时放宽。
Future<CardScanResult> scanCards({bool nfc = false}) {
  final requestId = _nextRequestId++;
  final result = CardScanResult.rustSignalStream
      .map((pack) => pack.message)
      .firstWhere((result) => result.requestId == requestId)
      .timeout(nfc ? const Duration(seconds: 90) : _timeout);
  ScanCards(requestId: requestId, nfc: nfc).sendSignalToRust();
  return result;
}

Future<KeyResult> setKeySync(bool enabled) => _request(
    (requestId) => SetKeySync(requestId: requestId, enabled: enabled).sendSignalToRust());

String keyErrorText(KeyError error) => switch (error) {
      KeyError.none => '',
      KeyError.invalid => '不是能识别的私钥',
      KeyError.passphraseRequired => '这把私钥有口令保护，请填写口令',
      KeyError.passphraseWrong => '口令不正确',
      KeyError.alreadyExists => '这把私钥已经导入过',
      KeyError.notFound => '私钥不存在',
      KeyError.inUse => '还有连接在用这把私钥',
      KeyError.keychain => '钥匙串读写失败',
      KeyError.syncUnavailable => '无法写入 iCloud 钥匙串',
      KeyError.cardNotFound => '没有找到 OpenPGP 卡',
      KeyError.cardUnsupported => '这张卡的认证槽没有密钥，或算法 SSH 用不了（支持 Ed25519、RSA 与 NIST P-256/384/521）',
      KeyError.securityKeyUnavailable => '这台设备或这个版本的 App 用不了安全密钥',
      KeyError.securityKeyCancelled => '已取消',
      KeyError.securityKeyFailed => '安全密钥没有完成登记（需要支持 ES256 的 FIDO2 安全密钥）',
    };
