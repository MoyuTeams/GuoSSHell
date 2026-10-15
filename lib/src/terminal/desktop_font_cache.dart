part of 'desktop_font.dart';

Future<void> _registerFont(String family, Uint8List bytes) {
  final loader = FontLoader(family)
    ..addFont(Future.value(ByteData.sublistView(bytes)));
  return loader.load();
}

enum DesktopFontBudgetLimit { resourceCount, inputBytes }

/// 字体准入受限；材料使用 Prepared 允许的回退，不修改已配置的字体。
class DesktopFontBudgetFailure implements Exception {
  const DesktopFontBudgetFailure(this.limit);

  final DesktopFontBudgetLimit limit;

  @override
  String toString() => switch (limit) {
    DesktopFontBudgetLimit.resourceCount => '独特字体数量已达应用额度',
    DesktopFontBudgetLimit.inputBytes => '字体输入资源字节已达应用额度',
  };
}

/// 同一注册域按不可变 hash 计量。默认最多 32 种、累计 128 MiB 输入资源；
/// 复制前准入，进行中及成功注册均占额。成功注册不可卸载，额度持续至应用结束；
/// 失败材料在最后使用者离开且注册完成后退额。这不是 engine heap 精确计量，
/// 也不包含 Rust / binary 通道、内置字体或旧终端槽的独立加载。
/// 测试或独立 registrar 可注入自己的状态，不占用生产注册域。
class DesktopFontBudget {
  DesktopFontBudget({
    this.maxResourceCount = 32,
    this.maxInputBytes = 128 * 1024 * 1024,
  }) {
    if (maxResourceCount < 0 || maxInputBytes < 0) {
      throw ArgumentError('字体额度不能为负数');
    }
  }

  final int maxResourceCount;
  final int maxInputBytes;
  final _entries = <String, _FontEntry>{};

  int get accountedResourceCount => _entries.length;
  int get accountedInputByteCount =>
      _entries.values.fold(0, (total, entry) => total + entry.inputBytes);
}

/// 生产缓存共用主 isolate 的应用期计量，重建工作区不重新开放字体额度。
/// 最后一个 pane 离开不取消注册：字节和 Future 由注册任务继续持有及计量，
/// 同 hash 的后续 pane 共享该任务；成功后只保留已注册身份和输入资源消耗。
class DesktopFontCache {
  DesktopFontCache({FontRegistrar? register, DesktopFontBudget? budget})
    : _register = register ?? _registerFont,
      budget =
          budget ??
          (register == null ? _productionBudget : DesktopFontBudget());

  static final _productionBudget = DesktopFontBudget();
  final FontRegistrar _register;
  final DesktopFontBudget budget;
  Map<String, _FontEntry> get _entries => budget._entries;

  int get activeResourceCount =>
      _entries.values.where((entry) => entry.references > 0).length;
  int get registeredResourceCount =>
      _entries.values.where((entry) => entry.loaded).length;
  int get retainedByteCount => _entries.values.fold(
    0,
    (total, entry) => total + (entry.bytes?.length ?? 0),
  );
  int get retainedFutureCount =>
      _entries.values.where((entry) => entry.pending != null).length;

  (_FontEntry?, DesktopFontBudgetFailure?) _acquire(
    ProfileFontResourceRef resource,
    Uint8List binary,
  ) {
    var entry = _entries[resource.hash];
    if (entry != null &&
        (entry.resource.family != resource.family ||
            entry.inputBytes != binary.length)) {
      throw ArgumentError('同一不可变 hash 的字体注册身份不一致');
    }
    if (entry == null) {
      if (budget.accountedResourceCount >= budget.maxResourceCount) {
        return (
          null,
          const DesktopFontBudgetFailure(DesktopFontBudgetLimit.resourceCount),
        );
      }
      if (binary.length >
          budget.maxInputBytes - budget.accountedInputByteCount) {
        return (
          null,
          const DesktopFontBudgetFailure(DesktopFontBudgetLimit.inputBytes),
        );
      }
      entry = _FontEntry(
        resource,
        Uint8List.fromList(binary).asUnmodifiableView(),
      );
      _entries[resource.hash] = entry;
    }
    ++entry.references;
    return (entry, null);
  }

  Future<Object?> _load(_FontEntry entry) {
    if (entry.loaded) return Future.value();
    if (entry.pending != null) return entry.pending!;
    // registrar 只接收这一捕获的只读字节；不查询当前槽，也不重读资源路径。
    final bytes = entry.bytes!;
    final completion = Completer<Object?>();
    final pending = completion.future;
    entry.pending = pending;
    // 先登记进行中所有权，再调用 registrar；同步回调释放 pane 也不能早退额。
    Future<void>.sync(() => _register(entry.resource.family, bytes)).then<void>(
      (_) {
        entry.loaded = true;
        entry.bytes = null;
        entry.pending = null;
        completion.complete();
      },
      onError: (Object error, StackTrace stack) {
        entry.pending = null;
        _dropUnused(entry);
        completion.complete(error);
      },
    );
    return pending;
  }

  void _release(_FontEntry entry) {
    --entry.references;
    _dropUnused(entry);
  }

  void _dropUnused(_FontEntry entry) {
    if (entry.references != 0 || entry.pending != null || entry.loaded) return;
    _entries.remove(entry.resource.hash);
    entry.bytes = null;
  }
}

class _FontEntry {
  _FontEntry(this.resource, Uint8List captured)
    : bytes = captured,
      inputBytes = captured.length;
  final ProfileFontResourceRef resource;
  final int inputBytes;
  Uint8List? bytes;
  Future<Object?>? pending;
  bool loaded = false;
  int references = 0;
}
