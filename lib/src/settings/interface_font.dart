import 'dart:async';

import 'package:file_selector/file_selector.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../bindings/bindings.dart';

class FontReply {
  const FontReply(this.state, this.bytes);
  final FontFileState state;
  final Uint8List bytes;
}

class FontDraft {
  const FontDraft(this.fileName, this.family, this.label, this.bytes);
  final String fileName;
  final String family;
  final String label;
  final Uint8List bytes;
}

typedef FontTransport = Future<FontReply> Function(
  String slot,
  String action,
  String name,
  Uint8List bytes,
);
typedef FontRegistrar = Future<void> Function(String family, Uint8List bytes);
int _nextFontRequest = 1;
final _loadedFonts = <String, Future<void>>{};

Future<FontReply> _transport(
  String slot,
  String action,
  String name,
  Uint8List bytes,
) {
  final id = _nextFontRequest++;
  final result = FontFileState.rustSignalStream
      .firstWhere((pack) => pack.message.requestId == id)
      .timeout(const Duration(seconds: 30))
      .then((pack) => FontReply(pack.message, pack.binary));
  FontFileRequest(
    requestId: id,
    slot: slot,
    action: action,
    fileName: name,
  ).sendSignalToRust(bytes);
  return result;
}

Future<void> _register(String family, Uint8List bytes) {
  if (bytes.isEmpty) return Future.value();
  return _loadedFonts.putIfAbsent(family, () {
    final loader = FontLoader(family)
      ..addFont(Future.value(ByteData.sublistView(bytes)));
    return loader.load();
  });
}

/// 持久化与文件校验在 Rust，Flutter 只负责动态加载和显示。
class InterfaceTypography extends ChangeNotifier {
  InterfaceTypography({
    this.slot = 'interface',
    FontTransport? transport,
    FontRegistrar? register,
  }) : _send = transport ?? _transport,
       _load = register ?? _register;
  static final instance = InterfaceTypography();
  static final terminal = InterfaceTypography(slot: 'terminal');
  final String slot;
  final FontTransport _send;
  final FontRegistrar _load;
  String get defaultFamily => slot == 'terminal' ? 'MesloLGS NF' : 'MiSans';
  late String family = defaultFamily;
  late String label = defaultFamily;
  String? error;
  String get fontFamily => family;
  bool get custom => family.startsWith('GuoshFont_');
  StreamSubscription? _subscription;
  int _revision = 0;
  int _lastRequest = -1;
  Future<void> _pending = Future.value();
  void connect() {
    _subscription ??= FontFileState.rustSignalStream.listen((pack) {
      if (pack.message.slot == slot && pack.message.applied) {
        unawaited(_update(FontReply(pack.message, pack.binary)));
      }
    });
  }

  Future<void> _update(FontReply reply) {
    if (reply.state.requestId == _lastRequest) return _pending;
    if (reply.state.requestId < _lastRequest) return Future.value();
    _lastRequest = reply.state.requestId;
    final revision = ++_revision;
    _pending = () async {
      var nextFamily = reply.state.family;
      var nextLabel = reply.state.label;
      String? detail = reply.state.error.isEmpty ? null : reply.state.error;
      try {
        await _load(nextFamily, reply.bytes);
      } catch (_) {
        nextFamily = defaultFamily;
        nextLabel = defaultFamily;
        detail = '字体加载失败，已使用内置字体';
      }
      if (revision != _revision) return;
      family = nextFamily;
      label = nextLabel;
      error = detail;
      notifyListeners();
    }();
    return _pending;
  }

  Future<void> refresh() async {
    final reply = await _send(slot, 'query', '', Uint8List(0));
    if (!reply.state.applied) throw StateError(reply.state.error);
    await _update(reply);
  }

  Future<FontDraft> prepare(XFile file) async {
    if (await file.length() > 32 * 1024 * 1024) {
      throw StateError('字体文件不能超过 32 MiB');
    }
    final reply = await _send(
      slot,
      'preview',
      file.name,
      await file.readAsBytes(),
    );
    if (reply.state.error.isNotEmpty) throw StateError(reply.state.error);
    await _load(reply.state.family, reply.bytes);
    return FontDraft(
      file.name,
      reply.state.family,
      reply.state.label,
      reply.bytes,
    );
  }

  Future<void> apply(FontDraft draft) async {
    final reply = await _send(slot, 'apply', draft.fileName, draft.bytes);
    if (reply.state.error.isNotEmpty) throw StateError(reply.state.error);
    await _update(reply);
  }

  Future<void> reset() async {
    final reply = await _send(slot, 'reset', '', Uint8List(0));
    if (reply.state.error.isNotEmpty) throw StateError(reply.state.error);
    await _update(reply);
  }

  @override
  void dispose() {
    ++_revision;
    _subscription?.cancel();
    super.dispose();
  }
}

/// 更新主题时保留 Navigator 和会话工作区。
class InterfaceFontScope extends InheritedNotifier<InterfaceTypography> {
  InterfaceFontScope({
    super.key,
    InterfaceTypography? controller,
    required super.child,
  }) : super(notifier: controller ?? InterfaceTypography.instance);
  static String fontFamilyOf(BuildContext context) =>
      (context
                  .dependOnInheritedWidgetOfExactType<InterfaceFontScope>()
                  ?.notifier ??
              InterfaceTypography.instance)
          .fontFamily;
}
