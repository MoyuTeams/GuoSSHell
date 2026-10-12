import 'dart:async';
import 'dart:collection';

import 'package:flutter/foundation.dart';

import '../../bindings/bindings.dart';
import 'desktop_settings_draft.dart';
import 'desktop_settings_gateway.dart';

enum DesktopSettingsQueryState { idle, loading, ready, failed, unavailable }

enum DesktopSettingsWriteState {
  idle,
  saving,
  applied,
  failed,
  uncertain,
  needsConfirmation,
}

class DesktopSettingsUnitState {
  DesktopSettingsUnitState({
    required this.state,
    this.pending,
    this.result,
    this.issue,
    DesktopDraftErrors? errors,
  }) : errors = errors ?? DesktopDraftErrors(const []);
  final DesktopSettingsWriteState state;
  final DesktopSettingsPending? pending;
  final DesktopSettingsResult? result;
  final Object? issue;
  final DesktopDraftErrors errors;
}

/// 文件选择、预览及捕获字体资源加载共用本地 generation；不代表 backend cancel。
class DesktopSettingsResourceToken {
  const DesktopSettingsResourceToken._(
    this.ownerId,
    this.profileId,
    this.generation,
  );
  final String ownerId;
  final String profileId;
  final int generation;
}

class DesktopSettingsController extends ChangeNotifier {
  DesktopSettingsController({DesktopSettingsGateway? gateway})
    : gateway = gateway ?? DesktopSettingsGateway.instance,
      ownerId = _identity('owner') {
    _subscription = this.gateway.events.listen(_onGatewayEvent);
    final saved = this.gateway.snapshot;
    if (saved != null) {
      _ingest(saved, this.gateway.capabilities!);
      queryState = DesktopSettingsQueryState.ready;
    }
  }
  static int _sequence = 0;
  static final _epoch = DateTime.now().microsecondsSinceEpoch;
  static String _identity(String purpose) =>
      'desktop-settings-$_epoch-${++_sequence}-$purpose';
  final DesktopSettingsGateway gateway;
  final String ownerId;
  late final StreamSubscription<DesktopSettingsGatewayEvent> _subscription;
  bool _disposed = false;
  int _queryGeneration = 0;
  int _successfulQueryGeneration = -1;
  int _resourceGeneration = 0;
  String? _queryId;
  DesktopSettingsQueryState queryState = DesktopSettingsQueryState.idle;
  DesktopSettingsResult? queryResult;
  Object? queryIssue;
  DesktopConfiguration? _snapshot;
  DesktopSettingsCapabilities? _capabilities;
  DesktopAppDraft? _app;
  final _profiles = <String, DesktopProfileDraft>{};
  final _units = <String, DesktopSettingsUnitState>{};
  // 门槛按结果 RID 记录本窗口 generation；同 RID 确认不推进，不同 RID 重设。
  final _reviewNeedsQuery =
      <String, ({String requestId, int queryGeneration})>{};
  final _preservedDrafts = <String>{};
  String? _selectedProfileId;
  DesktopConfiguration? get snapshot => _snapshot;
  DesktopSettingsCapabilities? get capabilities => _capabilities;
  DesktopAppDraft? get app => _app;
  Map<String, DesktopProfileDraft> get profiles =>
      UnmodifiableMapView(_profiles);
  String? get selectedProfileId => _selectedProfileId;
  DesktopProfileDraft? get selectedProfile => _profiles[_selectedProfileId];
  DesktopSettingsPending? get blockingWrite => gateway.blockingWrite;
  bool get dirty =>
      (_app?.dirty ?? false) ||
      _profiles.values.any((draft) => draft.dirty) ||
      _preservedDrafts.isNotEmpty ||
      _reviewNeedsQuery.keys.any(
        (unit) => unit == 'app' || _profiles.containsKey(unit),
      );
  bool get editable =>
      !_disposed && _snapshot != null && (_capabilities?.available ?? false);
  DesktopSettingsUnitState stateFor(String unit) =>
      _units[unit] ??
      DesktopSettingsUnitState(state: DesktopSettingsWriteState.idle);
  bool needsReview(String unit) {
    final revision = unit == 'app'
        ? _app?.baseRevision
        : _profiles[unit]?.baseRevision;
    return _hasPreservedDraft(unit) ||
        (revision != null && revision != _snapshot?.revision);
  }

  void _ensureOpen() {
    if (_disposed) throw StateError('设置 controller 已关闭');
  }

  String refresh() {
    _ensureOpen();
    _invalidateQuery();
    ++_resourceGeneration;
    final id = _identity('query');
    _queryId = id;
    queryState = DesktopSettingsQueryState.loading;
    queryIssue = null;
    queryResult = null;
    notifyListeners();
    gateway.send(
      DesktopConfigurationCommand(
        DesktopSettingsRequest(
          protocolVersion: 1,
          requestId: id,
          baseRevision: _snapshot?.revision ?? Uint64(BigInt.zero),
          operation: 'query',
          targetId: '',
          name: '',
        ),
      ),
      DesktopSettingsRequestContext(
        ownerId: ownerId,
        generation: _queryGeneration,
        editVersion: 0,
        unit: 'query',
      ),
    );
    return id;
  }

  void _invalidateQuery() {
    ++_queryGeneration;
    if (_queryId != null) gateway.invalidateQuery(_queryId!);
    _queryId = null;
    if (queryState == DesktopSettingsQueryState.loading) {
      queryState = _snapshot == null
          ? DesktopSettingsQueryState.idle
          : DesktopSettingsQueryState.ready;
    }
  }

  void selectProfile(String id) {
    _ensureOpen();
    if (!_profiles.containsKey(id)) throw StateError('profile 尚未加载');
    if (_selectedProfileId == id) return;
    _invalidateQuery();
    ++_resourceGeneration;
    _selectedProfileId = id;
    notifyListeners();
  }

  DesktopSettingsResourceToken beginProfileResource(String id) {
    _ensureOpen();
    if (id != _selectedProfileId) throw StateError('预览目标不是选中 profile');
    if (!(_snapshot?.profiles.any((profile) => profile.id == id) ?? false)) {
      throw StateError('预览目标已不在保存快照中');
    }
    return DesktopSettingsResourceToken._(ownerId, id, ++_resourceGeneration);
  }

  bool acceptsProfileResource(DesktopSettingsResourceToken token) =>
      !_disposed &&
      token.ownerId == ownerId &&
      token.profileId == _selectedProfileId &&
      (_snapshot?.profiles.any((profile) => profile.id == token.profileId) ??
          false) &&
      token.generation == _resourceGeneration;

  void cancelProfileResource() {
    _ensureOpen();
    ++_resourceGeneration;
  }

  void editApp({DesktopAppSettings? value, List<DesktopBindingRow>? bindings}) {
    _ensureEditable();
    _app = _app!.edit(value: value, bindings: bindings);
    _preserveForeignPendingInput('app');
    notifyListeners();
  }

  void editProfile(
    String id, {
    DesktopTerminalProfile? value,
    DesktopProfileNumbers? numbers,
    List<DesktopBindingRow>? bindings,
    ProfileFontBinding? fontBinding,
    DesktopTtfSelection? ttf,
    bool clearTtf = false,
  }) {
    _ensureEditable();
    final draft = _profiles[id];
    if (draft == null) throw StateError('profile 尚未加载');
    _profiles[id] = draft.edit(
      value: value,
      numbers: numbers,
      bindings: bindings,
      fontBinding: fontBinding,
      ttf: ttf,
      clearTtf: clearTtf,
    );
    _preserveForeignPendingInput(id);
    notifyListeners();
  }

  void _ensureEditable() {
    _ensureOpen();
    if (!editable) throw StateError('配置尚不可编辑');
  }

  void _ensureWrite(Uint64 baseRevision, String unit) {
    _ensureEditable();
    if (gateway.blockingWrite != null) throw StateError('前一写入尚未完成或核实');
    if (baseRevision != _snapshot!.revision ||
        baseRevision != gateway.snapshot?.revision ||
        needsReview(unit)) {
      throw StateError('草稿需要显式核对最新配置');
    }
  }

  DesktopSettingsRequestContext _context(
    String unit,
    int version,
    List<DesktopBindingRow> rows, {
    String? affectedDraftUnit,
    DesktopTtfSelection? ttfSelection,
    int? fontSelectionVersion,
  }) => DesktopSettingsRequestContext(
    ownerId: ownerId,
    generation: _resourceGeneration,
    editVersion: version,
    unit: unit,
    affectedDraftUnit: affectedDraftUnit,
    bindings: rows,
    ttfSelection: ttfSelection,
    fontSelectionVersion: fontSelectionVersion,
  );

  String _send(
    DesktopSettingsCommand command,
    DesktopSettingsRequestContext context,
  ) {
    _units[context.unit] = DesktopSettingsUnitState(
      state: DesktopSettingsWriteState.saving,
    );
    final entry = gateway.send(command, context);
    // 同步 transport 可以在 send 内返回确定结果；不能覆盖其最终状态。
    if (entry.phase == DesktopSettingsPendingPhase.waiting) {
      _units[context.unit] = DesktopSettingsUnitState(
        state: DesktopSettingsWriteState.saving,
        pending: entry,
      );
    }
    notifyListeners();
    return command.requestId;
  }

  String saveApp() {
    _ensureEditable();
    final draft = _app!;
    _ensureWrite(draft.baseRevision, 'app');
    final id = _identity('save-app');
    return _send(
      DesktopConfigurationCommand(
        DesktopSettingsRequest(
          protocolVersion: 1,
          requestId: id,
          baseRevision: draft.baseRevision,
          operation: 'save_app',
          targetId: '',
          name: '',
          app: draft.value,
        ),
      ),
      _context(
        'app',
        draft.editVersion,
        draft.bindings,
        affectedDraftUnit: 'app',
      ),
    );
  }

  /// 完整 profile 与字体绑定同 CAS；已选 TTF 则使用实际二进制请求入口。
  String? saveProfile(String profileId) {
    _ensureEditable();
    final draft = _profiles[profileId];
    if (draft == null) throw StateError('profile 尚未加载');
    _ensureWrite(draft.baseRevision, profileId);
    final errors = draft.numbers.errors;
    if (errors.isNotEmpty) {
      _units[profileId] = DesktopSettingsUnitState(
        state: DesktopSettingsWriteState.failed,
        errors: DesktopDraftErrors(errors, bindings: draft.bindings),
      );
      notifyListeners();
      return null;
    }
    final value = draft.numbers.apply(draft.value);
    final id = _identity('save-profile');
    final ttf = draft.ttf;
    final DesktopSettingsCommand command = ttf == null
        ? DesktopProfileCommand(
            DesktopProfileSettingsRequest(
              protocolVersion: 1,
              requestId: id,
              baseRevision: draft.baseRevision,
              targetId: profileId,
              profile: value,
              fontBinding: draft.fontBinding,
            ),
          )
        : DesktopProfileTtfCommand(
            DesktopProfileTtfRequest(
              protocolVersion: 1,
              requestId: id,
              baseRevision: draft.baseRevision,
              targetId: profileId,
              profile: value,
            ),
            ttf.bytes,
          );
    return _send(
      command,
      _context(
        profileId,
        draft.editVersion,
        draft.bindings,
        affectedDraftUnit: profileId,
        ttfSelection: ttf,
        fontSelectionVersion: draft.fontSelectionVersion,
      ),
    );
  }

  String createProfile(String name) => _lifecycle('create_profile', '', name);
  String copyProfile(String id, String name) =>
      _lifecycle('copy_profile', id, name);
  String renameProfile(String id, String name) =>
      _lifecycle('rename_profile', id, name);

  /// UI 调用前须明确确认删除目标；默认及连接引用保护仍由 Rust 权威校验。
  String deleteProfile(String id) => _lifecycle('delete_profile', id, '');

  String _lifecycle(String operation, String targetId, String name) {
    _ensureEditable();
    _ensureWrite(_snapshot!.revision, 'lifecycle');
    final draft = targetId.isEmpty ? null : _profiles[targetId];
    if (targetId.isNotEmpty && draft == null) throw StateError('profile 尚未加载');
    if (draft?.dirty ?? false) throw StateError('生命周期操作前须明确保存或放弃目标草稿');
    if (draft != null && needsReview(targetId)) {
      throw StateError('先核对目标 profile');
    }
    final id = _identity(operation);
    final affectedDraftUnit =
        operation == 'rename_profile' || operation == 'delete_profile'
        ? targetId
        : null;
    return _send(
      DesktopConfigurationCommand(
        DesktopSettingsRequest(
          protocolVersion: 1,
          requestId: id,
          baseRevision: _snapshot!.revision,
          operation: operation,
          targetId: targetId,
          name: name,
        ),
      ),
      _context(
        'lifecycle',
        affectedDraftUnit == null ? 0 : draft!.editVersion,
        const [],
        affectedDraftUnit: affectedDraftUnit,
      ),
    );
  }

  /// 对话框必须捕获最新 revision 和编辑版本；不会自动 rebase 或合并脏字段。
  void reviewApp({
    required Uint64 revision,
    required int editVersion,
    required bool keepDraft,
  }) {
    _ensureEditable();
    _ensureReview(revision, editVersion, _app!.editVersion, 'app');
    _app = keepDraft
        ? _app!.reviewed(_snapshot!.app, revision)
        : _app!.acceptSaved(_snapshot!.app, revision, discard: true);
    _units.remove('app');
    _reviewNeedsQuery.remove('app');
    _preservedDrafts.remove('app');
    notifyListeners();
  }

  void reviewProfile(
    String id, {
    required Uint64 revision,
    required int editVersion,
    required bool keepDraft,
  }) {
    _ensureEditable();
    final draft = _profiles[id];
    if (draft == null) throw StateError('profile 尚未加载');
    _ensureReview(revision, editVersion, draft.editVersion, id);
    final saved = _snapshot!.profiles
        .where((profile) => profile.id == id)
        .firstOrNull;
    if (saved == null) {
      if (keepDraft) throw StateError('已删除的 profile 草稿不能直接保存');
      _profiles.remove(id);
      if (_selectedProfileId == id) _selectedProfileId = null;
    } else {
      final binding = _binding(_snapshot!, id);
      _profiles[id] = keepDraft
          ? _acknowledgeTtf(
              draft,
              _units[id]?.pending,
            ).reviewed(saved, binding, revision)
          : draft.acceptSaved(saved, binding, revision, discard: true);
    }
    ++_resourceGeneration;
    _units.remove(id);
    _reviewNeedsQuery.remove(id);
    _preservedDrafts.remove(id);
    notifyListeners();
  }

  void _ensureReview(
    Uint64 revision,
    int expectedVersion,
    int currentVersion,
    String unit,
  ) {
    if (gateway.blockingWrite != null) throw StateError('写入期间不能替换草稿基线');
    if (revision != _snapshot!.revision || expectedVersion != currentVersion) {
      throw StateError('核对内容已变化，请重新核对');
    }
    final reviewGate = _reviewNeedsQuery[unit];
    if (reviewGate != null &&
        _successfulQueryGeneration <= reviewGate.queryGeneration) {
      throw StateError('操作结果后必须显式重新查询');
    }
  }

  void reviewLifecycle(Uint64 revision) {
    _ensureEditable();
    _ensureReview(revision, 0, 0, 'lifecycle');
    _reviewNeedsQuery.remove('lifecycle');
    _units.remove('lifecycle');
    notifyListeners();
  }

  /// 不确认任何草稿为 saved；仅解除已消费旧结果、重新查询后的应用级写入隔离。
  void confirmUncertainWrite(String requestId, Uint64 reviewedRevision) {
    _ensureEditable();
    gateway.confirmUncertainWrite(requestId, reviewedRevision);
  }

  static ProfileFontBinding _binding(
    DesktopConfiguration snapshot,
    String id,
  ) => snapshot.effectiveProfiles
      .singleWhere((profile) => profile.id == id)
      .fontBinding;

  void _ingest(
    DesktopConfiguration snapshot,
    DesktopSettingsCapabilities capabilities,
  ) {
    if (_snapshot != null &&
        snapshot.revision.toBigInt() < _snapshot!.revision.toBigInt()) {
      return;
    }
    _snapshot = snapshot;
    _capabilities = capabilities;
    if (_app == null) {
      _app = DesktopAppDraft.fromSaved(snapshot.app, snapshot.revision);
    } else if (!_app!.dirty &&
        !_hasOwnPendingUnit('app') &&
        !_hasPreservedDraft('app')) {
      _app = _app!.acceptSaved(snapshot.app, snapshot.revision);
    }
    final ids = snapshot.profiles.map((profile) => profile.id).toSet();
    _profiles.removeWhere(
      (id, draft) =>
          !ids.contains(id) &&
          !draft.dirty &&
          !_hasOwnPendingUnit(id) &&
          !_hasPreservedDraft(id),
    );
    for (final saved in snapshot.profiles) {
      final draft = _profiles[saved.id];
      if (draft == null) {
        _profiles[saved.id] = DesktopProfileDraft.fromSaved(
          saved,
          _binding(snapshot, saved.id),
          snapshot.revision,
        );
      } else if (!draft.dirty &&
          !_hasOwnPendingUnit(saved.id) &&
          !_hasPreservedDraft(saved.id)) {
        _profiles[saved.id] = draft.acceptSaved(
          saved,
          _binding(snapshot, saved.id),
          snapshot.revision,
        );
      }
    }
    // 不因 create/copy/迟到结果改当前选择；仅首次加载提供默认选择。
    _selectedProfileId ??= snapshot.app.defaultTerminalProfile;
  }

  bool _hasOwnPendingUnit(String unit) {
    final pending = gateway.blockingWrite;
    return pending?.context.ownerId == ownerId &&
        pending?.context.affectedDraftUnit == unit;
  }

  void _preserveForeignPendingInput(String unit) {
    final pending = gateway.blockingWrite;
    if (pending != null &&
        pending.context.ownerId != ownerId &&
        pending.context.affectedDraftUnit == unit) {
      _preservedDrafts.add(unit);
    }
  }

  /// 相对旧基线已还原的输入仍可能不同于已发出的写入；隔离解除不能代替草稿核对。
  bool _hasPreservedDraft(String unit) =>
      _preservedDrafts.contains(unit) || _reviewNeedsQuery.containsKey(unit);

  void _requireQueryAfterResult(String unit, String requestId) {
    if (_reviewNeedsQuery[unit]?.requestId == requestId) return;
    _reviewNeedsQuery[unit] = (
      requestId: requestId,
      queryGeneration: _queryGeneration,
    );
  }

  void _onGatewayEvent(DesktopSettingsGatewayEvent event) {
    if (_disposed) return;
    final entry = event.pending;
    final context = entry.context;
    final result = entry.result;
    final own = context.ownerId == ownerId;
    if (!entry.command.isWrite) {
      if (!own) {
        final latest = gateway.snapshot;
        if (result?.status == 'ok' && latest != null) {
          _ingest(latest, gateway.capabilities!);
        }
        notifyListeners();
        return;
      }
      if (!own ||
          entry.command.requestId != _queryId ||
          context.generation != _queryGeneration) {
        return;
      }
      _queryId = null;
      queryResult = result;
      queryIssue = entry.issue;
      if (result?.status == 'ok') {
        _successfulQueryGeneration = context.generation;
        _ingest(result!.snapshot!, result.capabilities);
        queryState = DesktopSettingsQueryState.ready;
      } else {
        if (result != null && !result.capabilities.available) {
          _capabilities = result.capabilities;
          queryState = DesktopSettingsQueryState.unavailable;
        } else {
          queryState = DesktopSettingsQueryState.failed;
        }
      }
      notifyListeners();
      return;
    }
    if (own) {
      final affectedDraftUnit = context.affectedDraftUnit;
      final editVersion = affectedDraftUnit == 'app'
          ? _app?.editVersion
          : _profiles[affectedDraftUnit]?.editVersion;
      if (affectedDraftUnit != null &&
          (entry.issue != null ||
              (context.unit == 'lifecycle' && result?.status == 'applied')) &&
          editVersion != null &&
          editVersion != context.editVersion) {
        _preservedDrafts.add(affectedDraftUnit);
      }
      if (result?.status == 'conflict') {
        _requireQueryAfterResult(
          affectedDraftUnit ?? context.unit,
          entry.command.requestId,
        );
      }
      final phase = entry.phase;
      final state = switch (phase) {
        DesktopSettingsPendingPhase.waiting => DesktopSettingsWriteState.saving,
        DesktopSettingsPendingPhase.uncertain =>
          DesktopSettingsWriteState.uncertain,
        DesktopSettingsPendingPhase.needsConfirmation =>
          DesktopSettingsWriteState.needsConfirmation,
        DesktopSettingsPendingPhase.completed =>
          result?.status == 'applied'
              ? DesktopSettingsWriteState.applied
              : DesktopSettingsWriteState.failed,
      };
      _units[context.unit] = DesktopSettingsUnitState(
        state: state,
        pending: entry,
        result: result,
        issue: entry.issue,
        errors: DesktopDraftErrors(
          result?.fieldErrors ?? const [],
          bindings: context.bindings,
        ),
      );
      if (phase == DesktopSettingsPendingPhase.completed &&
          result?.status == 'applied') {
        _completeOwnDraft(entry, result!.snapshot!);
      }
    }
    final affectedDraftUnit = context.affectedDraftUnit;
    if (((context.unit == 'lifecycle' && result?.status == 'applied') ||
            result?.status == 'conflict') &&
        affectedDraftUnit != null &&
        _hasPreservedDraft(affectedDraftUnit)) {
      _requireQueryAfterResult(affectedDraftUnit, entry.command.requestId);
    }
    final latest = gateway.snapshot;
    if (latest != null) _ingest(latest, gateway.capabilities!);
    notifyListeners();
  }

  void _completeOwnDraft(
    DesktopSettingsPending entry,
    DesktopConfiguration saved,
  ) {
    final context = entry.context;
    // 待核实写入的显式确认不能把窗口里继续编辑的草稿标记为已提交。
    if (entry.issue != null || context.unit == 'lifecycle') return;
    final affectedDraftUnit = context.affectedDraftUnit;
    if (affectedDraftUnit == 'app') {
      final draft = _app!;
      _app = draft.editVersion == context.editVersion
          ? draft.acceptSaved(saved.app, saved.revision)
          : draft.reviewed(saved.app, saved.revision);
    } else if (affectedDraftUnit != null) {
      final draft = _profiles[affectedDraftUnit];
      if (draft == null) return;
      final profile = saved.profiles.singleWhere(
        (profile) => profile.id == affectedDraftUnit,
      );
      final binding = _binding(saved, affectedDraftUnit);
      _profiles[affectedDraftUnit] = draft.editVersion == context.editVersion
          ? draft.acceptSaved(profile, binding, saved.revision)
          : _acknowledgeTtf(
              draft,
              entry,
            ).reviewed(profile, binding, saved.revision);
    }
  }

  DesktopProfileDraft _acknowledgeTtf(
    DesktopProfileDraft draft,
    DesktopSettingsPending? entry,
  ) {
    final selection = entry?.context.ttfSelection;
    if (selection == null || entry?.result?.status != 'applied') return draft;
    final published = entry!.result!.snapshot!;
    return draft.acknowledgeAppliedTtf(
      published.profiles.singleWhere((profile) => profile.id == draft.value.id),
      _binding(published, draft.value.id),
      selection: selection,
      selectionVersion: entry.context.fontSelectionVersion,
    );
  }

  @override
  void dispose() {
    if (_disposed) return;
    _disposed = true;
    ++_queryGeneration;
    ++_resourceGeneration;
    gateway.detachOwner(ownerId);
    unawaited(_subscription.cancel());
    super.dispose();
  }
}
