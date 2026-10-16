import 'dart:async';
import 'dart:typed_data';

import 'package:rinf/rinf.dart';

import '../../bindings/bindings.dart';
import 'desktop_settings_draft.dart';

enum DesktopSettingsPendingPhase {
  waiting,
  uncertain,
  needsConfirmation,
  completed,
}

/// Result 不携带 operation/baseRevision；归属仅由本地 RID 记录确定。
class DesktopSettingsRequestContext {
  const DesktopSettingsRequestContext({
    required this.ownerId,
    required this.generation,
    required this.editVersion,
    required this.unit,
    this.affectedDraftUnit,
    this.bindings = const [],
    this.ttfSelection,
    this.fontSelectionVersion,
  });
  final String ownerId;
  final int generation;
  final int editVersion;

  /// app、profile ID 或 lifecycle；不是 wire 字段。
  final String unit;

  /// 实际受写操作影响的草稿；editVersion 属于该单元，空值表示不修改已有草稿。
  final String? affectedDraftUnit;
  final List<DesktopBindingRow> bindings;
  final DesktopTtfSelection? ttfSelection;
  final int? fontSelectionVersion;
}

sealed class DesktopSettingsCommand {
  const DesktopSettingsCommand();
  String get requestId;
  Uint64 get baseRevision;
  String get operation;
  String get targetId;
  bool get isWrite => operation != 'query';
}

class DesktopConfigurationCommand extends DesktopSettingsCommand {
  DesktopConfigurationCommand(DesktopSettingsRequest request)
    : request = request.copyWith(
        app: () => request.app == null ? null : copyDesktopApp(request.app!),
        profile: () => request.profile == null
            ? null
            : copyDesktopProfile(request.profile!),
      );
  final DesktopSettingsRequest request;
  @override
  String get requestId => request.requestId;
  @override
  Uint64 get baseRevision => request.baseRevision;
  @override
  String get operation => request.operation;
  @override
  String get targetId => request.targetId;
}

class DesktopProfileCommand extends DesktopSettingsCommand {
  DesktopProfileCommand(DesktopProfileSettingsRequest request)
    : request = request.copyWith(
        profile: copyDesktopProfile(request.profile),
        fontBinding: request.fontBinding.copyWith(),
      );
  final DesktopProfileSettingsRequest request;
  @override
  String get requestId => request.requestId;
  @override
  Uint64 get baseRevision => request.baseRevision;
  @override
  String get operation => 'save_profile';
  @override
  String get targetId => request.targetId;
}

class DesktopProfileTtfCommand extends DesktopSettingsCommand {
  DesktopProfileTtfCommand(DesktopProfileTtfRequest request, Uint8List bytes)
    : request = request.copyWith(profile: copyDesktopProfile(request.profile)),
      bytes = Uint8List.fromList(bytes).asUnmodifiableView();
  final DesktopProfileTtfRequest request;
  final Uint8List bytes;
  @override
  String get requestId => request.requestId;
  @override
  Uint64 get baseRevision => request.baseRevision;
  @override
  String get operation => 'save_profile_ttf';
  @override
  String get targetId => request.targetId;
}

class DesktopSettingsPending {
  DesktopSettingsPending._(this.command, DesktopSettingsRequestContext context)
    : context = DesktopSettingsRequestContext(
        ownerId: context.ownerId,
        generation: context.generation,
        editVersion: context.editVersion,
        unit: context.unit,
        affectedDraftUnit: context.affectedDraftUnit,
        bindings: List.unmodifiable(context.bindings),
        ttfSelection: context.ttfSelection,
        fontSelectionVersion: context.fontSelectionVersion,
      );
  final DesktopSettingsCommand command;
  final DesktopSettingsRequestContext context;
  DesktopSettingsPendingPhase get phase => _phase;
  DesktopSettingsPendingPhase _phase = DesktopSettingsPendingPhase.waiting;
  DesktopSettingsResult? get result => _result;
  DesktopSettingsResult? _result;
  Object? get issue => _issue;
  Object? _issue;
  Timer? _timer;
  int? _settledSequence;
}

class DesktopSettingsGatewayEvent {
  const DesktopSettingsGatewayEvent(this.pending);
  final DesktopSettingsPending pending;
}

/// 应用级存活。独立 controller detach 不能销毁订阅或抹去已发出的写入。
/// 注入边界仍是实际 DTO/typed RustSignalPack；默认发送与接收均走生成 Rinf。
class DesktopSettingsGateway {
  DesktopSettingsGateway({
    Stream<RustSignalPack<DesktopSettingsResult>>? results,
    void Function(DesktopSettingsRequest)? sendSettings,
    void Function(DesktopProfileSettingsRequest)? sendProfile,
    void Function(DesktopProfileTtfRequest, Uint8List)? sendTtf,
    this.timeout = const Duration(seconds: 15),
  }) : _sendSettings =
           sendSettings ?? ((request) => request.sendSignalToRust()),
       _sendProfile = sendProfile ?? ((request) => request.sendSignalToRust()),
       _sendTtf =
           sendTtf ?? ((request, bytes) => request.sendSignalToRust(bytes)) {
    _subscription = (results ?? DesktopSettingsResult.rustSignalStream).listen(
      (pack) => _accept(pack.message),
      onError: _receiveFailure,
      onDone: () => _receiveFailure(StateError('配置结果通道已关闭')),
    );
  }
  static final instance = DesktopSettingsGateway();
  final Duration timeout;
  final void Function(DesktopSettingsRequest) _sendSettings;
  final void Function(DesktopProfileSettingsRequest) _sendProfile;
  final void Function(DesktopProfileTtfRequest, Uint8List) _sendTtf;
  late final StreamSubscription<RustSignalPack<DesktopSettingsResult>>
  _subscription;
  final _events = StreamController<DesktopSettingsGatewayEvent>.broadcast(
    sync: true,
  );
  final _pending = <String, DesktopSettingsPending>{};
  final _usedIds = <String>{};
  final _querySentSequence = <String, int>{};
  int _sequence = 0;
  bool _closed = false;
  DesktopConfiguration? _snapshot;
  DesktopSettingsCapabilities? _capabilities;
  Stream<DesktopSettingsGatewayEvent> get events => _events.stream;
  DesktopConfiguration? get snapshot => _snapshot;
  DesktopSettingsCapabilities? get capabilities => _capabilities;
  List<DesktopSettingsPending> get pending =>
      List.unmodifiable(_pending.values);
  DesktopSettingsPending? get blockingWrite {
    for (final entry in _pending.values) {
      if (entry.command.isWrite) return entry;
    }
    return null;
  }

  DesktopSettingsPending send(
    DesktopSettingsCommand command,
    DesktopSettingsRequestContext context,
  ) {
    if (_closed) throw StateError('配置 gateway 已关闭');
    if (_usedIds.contains(command.requestId)) {
      throw StateError('request ID 不可复用');
    }
    if (command.isWrite && blockingWrite != null) {
      throw StateError('前一配置写入尚未完成或核实');
    }
    final entry = DesktopSettingsPending._(command, context);
    _usedIds.add(command.requestId);
    _pending[command.requestId] = entry;
    if (!command.isWrite) _querySentSequence[command.requestId] = ++_sequence;
    entry._timer = Timer(
      timeout,
      () => _invalidate(entry, TimeoutException('配置结果待核实')),
    );
    // 监听和归属必须先于发送；同步结果同样能准确关联。
    try {
      switch (command) {
        case DesktopConfigurationCommand():
          _sendSettings(command.request);
        case DesktopProfileCommand():
          _sendProfile(command.request);
        case DesktopProfileTtfCommand():
          _sendTtf(command.request, command.bytes);
      }
    } catch (error) {
      // 发送异常不能证明 backend 尚未收到；写入继续隔离，不盲重试。
      if (_pending[command.requestId] == entry) _invalidate(entry, error);
    }
    return entry;
  }

  void detachOwner(String ownerId) {
    for (final entry in _pending.values.toList()) {
      if (entry.context.ownerId != ownerId) continue;
      if (entry.command.isWrite) {
        if (entry.phase == DesktopSettingsPendingPhase.waiting) {
          _invalidate(entry, StateError('设置窗口已关闭，写入结果待核实'));
        }
      } else {
        invalidateQuery(entry.command.requestId);
      }
    }
  }

  /// 只取消本地等待；不存在 backend cancel 或回滚的承诺。
  void invalidateQuery(String requestId) {
    final entry = _pending[requestId];
    if (entry == null || entry.command.isWrite) return;
    entry._timer?.cancel();
    _pending.remove(requestId);
    _querySentSequence.remove(requestId);
  }

  void _invalidate(DesktopSettingsPending entry, Object issue) {
    entry._timer?.cancel();
    entry._issue = issue;
    if (entry.command.isWrite) {
      if (entry.phase == DesktopSettingsPendingPhase.needsConfirmation) return;
      entry._phase = DesktopSettingsPendingPhase.uncertain;
    } else {
      entry._phase = DesktopSettingsPendingPhase.completed;
      _pending.remove(entry.command.requestId);
      _querySentSequence.remove(entry.command.requestId);
    }
    _events.add(DesktopSettingsGatewayEvent(entry));
  }

  void _receiveFailure(Object error) {
    for (final entry in _pending.values.toList()) {
      if (entry.phase == DesktopSettingsPendingPhase.waiting) {
        _invalidate(entry, error);
      }
    }
  }

  static bool validSnapshot(DesktopSettingsResult result) {
    final snapshot = result.snapshot;
    if (result.protocolVersion != 1 ||
        !result.capabilities.available ||
        result.fieldErrors.isNotEmpty ||
        snapshot == null ||
        result.revision != snapshot.revision) {
      return false;
    }
    final ids = snapshot.profiles.map((profile) => profile.id).toSet();
    final effectiveIds = snapshot.effectiveProfiles
        .map((profile) => profile.id)
        .toSet();
    return ids.length == snapshot.profiles.length &&
        !ids.contains('') &&
        ids.contains(snapshot.app.defaultTerminalProfile) &&
        effectiveIds.length == snapshot.effectiveProfiles.length &&
        effectiveIds.length == ids.length &&
        effectiveIds.containsAll(ids);
  }

  bool _validApplied(
    DesktopSettingsCommand command,
    DesktopSettingsResult result,
  ) {
    if (!validSnapshot(result) ||
        result.revision!.toBigInt() !=
            command.baseRevision.toBigInt() + BigInt.one) {
      return false;
    }
    final profiles = result.snapshot!.profiles;
    final returned = profiles.where((profile) => profile.id == result.targetId);
    switch (command.operation) {
      case 'save_app':
        return result.targetId.isEmpty;
      case 'create_profile':
      case 'copy_profile':
        return result.targetId.isNotEmpty &&
            result.targetId != command.targetId &&
            returned.length == 1;
      case 'delete_profile':
        return result.targetId == command.targetId && returned.isEmpty;
      case 'rename_profile':
      case 'save_profile':
      case 'save_profile_ttf':
        return result.targetId == command.targetId && returned.length == 1;
      default:
        return false;
    }
  }

  void _accept(DesktopSettingsResult incoming) {
    final entry = _pending[incoming.requestId];
    if (entry == null ||
        entry.phase == DesktopSettingsPendingPhase.needsConfirmation) {
      return;
    }
    final command = entry.command;
    final result = copyDesktopResult(incoming);
    final success = command.isWrite
        ? result.status == 'applied' && _validApplied(command, result)
        : result.status == 'ok' &&
              result.targetId.isEmpty &&
              validSnapshot(result);
    const failures = {
      'validation_error',
      'conflict',
      'storage_error',
      'unsupported_version',
      'unsupported_platform',
    };
    // 真实错误只按 RID、结果种类和协议版本关联；不要求空 target 或当前 revision 回显草稿。
    final failure =
        result.protocolVersion == 1 &&
        failures.contains(result.status) &&
        result.snapshot == null;
    if (!success && !failure) {
      _invalidate(entry, const FormatException('配置结果不符合请求契约'));
      return;
    }
    entry._timer?.cancel();
    entry._result = result;
    entry._settledSequence = ++_sequence;
    if (success) {
      final snapshot = result.snapshot!;
      if (_snapshot == null ||
          snapshot.revision.toBigInt() >= _snapshot!.revision.toBigInt()) {
        _snapshot = snapshot;
        _capabilities = result.capabilities;
      }
    }
    if (command.isWrite &&
        entry.phase == DesktopSettingsPendingPhase.uncertain) {
      entry._phase = DesktopSettingsPendingPhase.needsConfirmation;
    } else {
      entry._phase = DesktopSettingsPendingPhase.completed;
      _pending.remove(command.requestId);
    }
    if (!command.isWrite) {
      final sentSequence = _querySentSequence.remove(command.requestId)!;
      if (success) {
        for (final write in _pending.values) {
          if (write.phase == DesktopSettingsPendingPhase.needsConfirmation &&
              sentSequence > write._settledSequence! &&
              (write.result?.revision == null ||
                  snapshotRevisionAtLeast(result, write.result!.revision!))) {
            _verificationQueries[write.command.requestId] = result;
          }
        }
      }
    }
    _events.add(DesktopSettingsGatewayEvent(entry));
  }

  static bool snapshotRevisionAtLeast(
    DesktopSettingsResult result,
    Uint64 revision,
  ) => result.snapshot!.revision.toBigInt() >= revision.toBigInt();
  final _verificationQueries = <String, DesktopSettingsResult>{};

  DesktopSettingsResult? verificationFor(String requestId) =>
      _verificationQueries[requestId];

  /// 必须先消耗旧 RID 的确定结果，再发起 query 核对实际状态，最后由用户显式确认。
  /// query 即使显示已保存值，也不能证明另一 receiver queue 的写入已结束。
  void confirmUncertainWrite(String requestId, Uint64 reviewedRevision) {
    final entry = _pending[requestId];
    final query = _verificationQueries[requestId];
    if (entry?.phase != DesktopSettingsPendingPhase.needsConfirmation ||
        query == null ||
        query.revision != reviewedRevision ||
        _snapshot?.revision != reviewedRevision) {
      throw StateError('尚未完成旧写入结果消费和最新配置核对');
    }
    entry!._phase = DesktopSettingsPendingPhase.completed;
    _pending.remove(requestId);
    _verificationQueries.remove(requestId);
    _events.add(DesktopSettingsGatewayEvent(entry));
  }

  /// 仅应用宿主退出时关闭；有未决写入时拒绝销毁归属。
  Future<void> close() async {
    if (blockingWrite != null) throw StateError('未决写入的 gateway 不可销毁');
    _closed = true;
    for (final entry in _pending.values) {
      entry._timer?.cancel();
    }
    _pending.clear();
    _querySentSequence.clear();
    await _subscription.cancel();
    await _events.close();
  }
}
