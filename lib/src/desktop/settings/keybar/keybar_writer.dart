import 'dart:async';

import 'package:flutter/foundation.dart';

import '../../../bindings/bindings.dart';
import 'keybar_draft.dart';
import 'keybar_transport.dart';

enum KeyBarWriteState { ready, pending, quarantined, needsVerify, verifying }

enum KeyBarSaveStatus { saved, rejected, uncertain, blocked }

class KeyBarSaveOutcome {
  const KeyBarSaveOutcome(this.status, {this.detail = ''});
  final KeyBarSaveStatus status;
  final String detail;
}

/// 本地身份不是 wire request ID，也不能用来关联 SettingsState。
class KeyBarWriteIdentity {
  KeyBarWriteIdentity._(this.owner, this.rows);
  final Object owner;
  final List<List<String>> rows;
  final _completion = Completer<KeyBarSaveOutcome>();
  Future<KeyBarSaveOutcome> get result => _completion.future;
  Timer? _timer;
}

class KeyBarQuarantineToken {
  KeyBarQuarantineToken._(this.write);
  final KeyBarWriteIdentity write;
}

/// 仅在旧结果已被本 writer 消耗之后发行。
class KeyBarSettlementToken {
  KeyBarSettlementToken._(this.quarantine, this.result);
  final KeyBarQuarantineToken quarantine;
  final KeyBarLayoutResult result;
}

/// 调用方必须保证真实来源：不能用“下一条 SettingsState”伪造 fresh query。
/// evidence 是调用方的来源证据，token 只校验本地隔离归属。
class KeyBarVerificationProof {
  KeyBarVerificationProof.afterSettlement({
    required KeyBarSettlementToken token,
    required List<List<String>> rows,
    required this.evidence,
  }) : settlement = token,
       restart = null,
       rows = immutableKeyBarRows(rows);

  /// 已确认后端重开清空旧请求／回包队列，且其后完成真实 query。
  KeyBarVerificationProof.afterBackendRestart({
    required KeyBarQuarantineToken token,
    required List<List<String>> rows,
    required this.evidence,
  }) : settlement = null,
       restart = token,
       rows = immutableKeyBarRows(rows);

  final KeyBarSettlementToken? settlement;
  final KeyBarQuarantineToken? restart;
  final List<List<String>> rows;
  final Object evidence;
}

abstract interface class KeyBarVerifier {
  Future<KeyBarVerificationProof?> verify(KeyBarSettlementToken token);
}

/// 全应用所有 SaveKeyBarLayout 写入口必须使用同一实例。
/// 不订阅旧 helper 的 next-return；同一 transport 只允许一个 writer。
class KeyBarWriter extends ChangeNotifier {
  KeyBarWriter({
    required KeyBarTransport transport,
    this.timeout = const Duration(seconds: 10),
    this.verifier,
    this.applyRows,
    this.beforeSend,
  }) : _transport = transport {
    if (_owners[transport] != null) {
      throw StateError('transport 已有键位条 writer');
    }
    _owners[transport] = this;
    _subscription = transport.results.listen(
      _receive,
      onError: (Object error) => _uncertain(),
      onDone: _uncertain,
    );
  }

  static final _owners = Expando<KeyBarWriter>();
  static final shared = KeyBarWriter(transport: RinfKeyBarTransport.instance);
  final KeyBarTransport _transport;
  final Duration timeout;
  KeyBarVerifier? verifier;
  ValueChanged<List<List<String>>>? applyRows;
  VoidCallback? beforeSend;
  late final StreamSubscription<KeyBarLayoutResult> _subscription;
  KeyBarWriteState _state = KeyBarWriteState.ready;
  KeyBarWriteState get state => _state;
  KeyBarWriteIdentity? _pending;
  KeyBarQuarantineToken? _quarantine;
  KeyBarQuarantineToken? get quarantine => _quarantine;
  KeyBarSettlementToken? _settlement;
  KeyBarSettlementToken? get settlement => _settlement;
  bool _disposed = false;
  bool _sending = false;
  List<List<String>>? _confirmedRows;
  List<List<String>>? get confirmedRows => _confirmedRows;
  KeyBarVerificationProof? _verifiedBaseline;

  bool get canSave => !_disposed && state == KeyBarWriteState.ready;
  KeyBarWriteIdentity? get pending => _pending;

  KeyBarWriteIdentity? save(Object owner, List<List<String>> rows) {
    if (!canSave) return null;
    if (!validKeyBarRows(rows)) throw ArgumentError.value(rows);
    final write = KeyBarWriteIdentity._(owner, immutableKeyBarRows(rows));
    _pending = write;
    _state = KeyBarWriteState.pending;
    notifyListeners();
    write._timer = Timer(timeout, _uncertain);
    try {
      // 构造时已订阅；共享集成可在发送前检查自己的单写者边界。
      beforeSend?.call();
      _sending = true;
      _transport.send(write.rows);
    } catch (_) {
      _uncertain();
    }
    return write;
  }

  void _receive(KeyBarLayoutResult result) {
    final write = _pending;
    if (write == null || !_sending) return;
    write._timer?.cancel();
    _pending = null;
    _sending = false;
    if (quarantine != null) {
      _settlement = KeyBarSettlementToken._(quarantine!, result);
      _state = KeyBarWriteState.needsVerify;
      notifyListeners();
      // 迟到 ok 只归旧操作，不能直接应用或确认任何当前草稿。
      return;
    }
    _state = KeyBarWriteState.ready;
    if (result.ok) {
      _confirmedRows = write.rows;
      _verifiedBaseline = null;
      write._completion.complete(
        const KeyBarSaveOutcome(KeyBarSaveStatus.saved),
      );
      notifyListeners();
      applyRows?.call(write.rows);
    } else {
      write._completion.complete(
        KeyBarSaveOutcome(KeyBarSaveStatus.rejected, detail: result.detail),
      );
      notifyListeners();
    }
  }

  void _uncertain() {
    final write = _pending;
    if (write == null || _disposed) return;
    write._timer?.cancel();
    _quarantine ??= KeyBarQuarantineToken._(write);
    _state = KeyBarWriteState.quarantined;
    if (!write._completion.isCompleted) {
      write._completion.complete(
        const KeyBarSaveOutcome(KeyBarSaveStatus.uncertain),
      );
    }
    notifyListeners();
  }

  /// 只分离 UI，不向后端伪造取消，也不丢弃旧结果监听。
  void detach(Object owner) {
    if (identical(_pending?.owner, owner)) _uncertain();
  }

  Future<bool> verify() async {
    final token = settlement;
    final source = verifier;
    if (_disposed ||
        token == null ||
        source == null ||
        state != KeyBarWriteState.needsVerify) {
      return false;
    }
    _state = KeyBarWriteState.verifying;
    notifyListeners();
    try {
      final proof = await source.verify(token);
      if (_disposed || !identical(settlement, token)) return false;
      if (proof != null && acceptProof(proof)) return true;
    } catch (_) {
      // 核实失败不改变隔离归属或用户草稿。
    }
    if (!_disposed && identical(settlement, token)) {
      _state = KeyBarWriteState.needsVerify;
      notifyListeners();
    }
    return false;
  }

  bool acceptProof(KeyBarVerificationProof proof) {
    if (_disposed || quarantine == null || !validKeyBarRows(proof.rows)) {
      return false;
    }
    final valid = proof.settlement != null
        ? identical(proof.settlement, settlement)
        : identical(proof.restart, quarantine);
    if (!valid) return false;
    _pending?._timer?.cancel();
    _pending = null;
    _sending = false;
    _quarantine = null;
    _settlement = null;
    _confirmedRows = proof.rows;
    _verifiedBaseline = proof;
    _state = KeyBarWriteState.ready;
    notifyListeners();
    applyRows?.call(proof.rows);
    return true;
  }

  /// 仅应用最终退出时释放。编辑 scope 不得 dispose 共享 writer。
  @override
  void dispose() {
    _uncertain();
    _disposed = true;
    _pending?._timer?.cancel();
    unawaited(_subscription.cancel());
    super.dispose();
  }
}

class KeyBarEditorController extends ChangeNotifier {
  KeyBarEditorController({required this.draft, required this.writer}) {
    _syncVerifiedBaseline();
    draft.addListener(_changed);
    writer.addListener(_writerChanged);
  }
  final KeyBarDraft draft;
  final KeyBarWriter writer;
  final Object _owner = Object();
  bool _closed = false;
  KeyBarVerificationProof? _lastVerifiedBaseline;
  KeyBarSaveOutcome? outcome;
  bool get canSave => !_closed && draft.dirty && writer.canSave;

  void _changed() {
    if (!_closed) notifyListeners();
  }

  void _syncVerifiedBaseline() {
    final proof = writer._verifiedBaseline;
    if (proof == null || identical(proof, _lastVerifiedBaseline)) return;
    _lastVerifiedBaseline = proof;
    draft.confirm(proof.rows);
  }

  void _writerChanged() {
    if (_closed) return;
    _syncVerifiedBaseline();
    notifyListeners();
  }

  Future<KeyBarSaveOutcome> save() async {
    if (!canSave) return const KeyBarSaveOutcome(KeyBarSaveStatus.blocked);
    outcome = null;
    final write = writer.save(_owner, draft.rows);
    if (write == null) return const KeyBarSaveOutcome(KeyBarSaveStatus.blocked);
    final result = await write.result;
    if (!_closed) {
      outcome = result;
      if (result.status == KeyBarSaveStatus.saved) draft.confirm(write.rows);
      notifyListeners();
    }
    return result;
  }

  @override
  void dispose() {
    if (_closed) return;
    _closed = true;
    draft.removeListener(_changed);
    writer.removeListener(_writerChanged);
    writer.detach(_owner);
    super.dispose();
  }
}
