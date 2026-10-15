import 'package:file_selector/file_selector.dart';
import 'package:flutter/services.dart';

import '../../bindings/bindings.dart';
import '../../keys/key_requests.dart' as requests;

/// 默认复用带请求号和超时的现有请求层；可注入替身以测试异步界面。
class DesktopKeyActions {
  final void Function() query;
  final Future<KeyResult> Function(
    String name,
    String privateKey,
    String passphrase,
  )
  import;
  final Future<KeyResult> Function(String id, String name) rename;
  final Future<KeyResult> Function(String id) delete;
  final Future<KeyResult> Function(String id) forgetPassphrase;
  final Future<KeyResult> Function(bool enabled) setSync;
  final Future<CardScanResult> Function(bool nfc) scan;
  final Future<KeyResult> Function(String ident, String name) addCard;
  final Future<KeyResult> Function(String name) registerSecurityKey;
  final Future<XFile?> Function() pickFile;
  final Future<String?> Function() readClipboard;

  DesktopKeyActions({
    void Function()? query,
    Future<KeyResult> Function(String, String, String)? import,
    Future<KeyResult> Function(String, String)? rename,
    Future<KeyResult> Function(String)? delete,
    Future<KeyResult> Function(String)? forgetPassphrase,
    Future<KeyResult> Function(bool)? setSync,
    Future<CardScanResult> Function(bool)? scan,
    Future<KeyResult> Function(String, String)? addCard,
    Future<KeyResult> Function(String)? registerSecurityKey,
    Future<XFile?> Function()? pickFile,
    Future<String?> Function()? readClipboard,
  }) : query = query ?? _query,
       import = import ?? _import,
       rename = rename ?? requests.renameKey,
       delete = delete ?? requests.deleteKey,
       forgetPassphrase = forgetPassphrase ?? requests.forgetPassphrase,
       setSync = setSync ?? requests.setKeySync,
       scan = scan ?? _scan,
       addCard = addCard ?? requests.addCardKey,
       registerSecurityKey =
           registerSecurityKey ?? requests.registerSecurityKey,
       pickFile = pickFile ?? openFile,
       readClipboard = readClipboard ?? _readClipboard;

  static void _query() => KeyQuery().sendSignalToRust();
  static Future<KeyResult> _import(
    String name,
    String key,
    String passphrase,
  ) => requests.importKey(name: name, privateKey: key, passphrase: passphrase);
  static Future<CardScanResult> _scan(bool nfc) => requests.scanCards(nfc: nfc);
  static Future<String?> _readClipboard() async =>
      (await Clipboard.getData(Clipboard.kTextPlain))?.text;
}
