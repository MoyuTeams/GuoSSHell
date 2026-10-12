part of 'desktop_font.dart';

Future<void> _registerFont(String family, Uint8List bytes) {
  final loader = FontLoader(family)
    ..addFont(Future.value(ByteData.sublistView(bytes)));
  return loader.load();
}

/// 由工作区显式共享，按不可变资源引用复用加载，不按 profile 或终端槽缓存。
/// 最后一个使用者离开即清除本模块的 byte / Future 引用；注册成功后只记资源
/// 身份，Flutter engine 的字体无法卸载，不能将这一步称为释放 engine 字体。
class DesktopFontCache {
  DesktopFontCache({FontRegistrar? register})
    : _register = register ?? _registerFont;

  final FontRegistrar _register;
  final _entries = <ProfileFontResourceRef, _FontEntry>{};
  final _registered = <ProfileFontResourceRef>{};

  int get activeResourceCount => _entries.length;
  int get registeredResourceCount => _registered.length;
  int get retainedByteCount => _entries.values.fold(
    0,
    (total, entry) => total + (entry.bytes?.length ?? 0),
  );
  int get retainedFutureCount =>
      _entries.values.where((entry) => entry.pending != null).length;

  _FontEntry _acquire(ProfileFontResourceRef resource, Uint8List binary) {
    final entry = _entries.putIfAbsent(
      resource,
      () => _FontEntry(
        resource,
        _registered.contains(resource)
            ? null
            : Uint8List.fromList(binary).asUnmodifiableView(),
        loaded: _registered.contains(resource),
      ),
    );
    ++entry.references;
    return entry;
  }

  Future<Object?> _load(_FontEntry entry) {
    if (entry.loaded) return Future.value();
    if (entry.pending != null) return entry.pending!;
    // registrar 只接收这一捕获的只读字节；不查询当前槽，也不重读资源路径。
    final bytes = entry.bytes!;
    final pending =
        Future<void>.sync(() => _register(entry.resource.family, bytes))
            .then<Object?>(
              (_) {
                _registered.add(entry.resource);
                entry.loaded = true;
                entry.bytes = null;
                entry.pending = null;
                return null;
              },
              onError: (Object error, StackTrace stack) {
                entry.pending = null;
                return error;
              },
            );
    entry.pending = pending;
    return pending;
  }

  void _release(_FontEntry entry) {
    if (--entry.references != 0) return;
    _entries.remove(entry.resource);
    entry.bytes = null;
    entry.pending = null;
    // 已交给 registrar / engine 的进行中调用不能取消；迟到结果不重新插入 entry。
  }
}

class _FontEntry {
  _FontEntry(this.resource, this.bytes, {required this.loaded});
  final ProfileFontResourceRef resource;
  Uint8List? bytes;
  Future<Object?>? pending;
  bool loaded;
  int references = 0;
}
