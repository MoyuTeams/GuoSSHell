import 'dart:async';
import 'dart:typed_data';

import 'package:flutter/services.dart' show FontLoader;
import 'package:flutter/widgets.dart';
import 'package:terminal_view/terminal_view.dart' show TerminalStyle;
// 与现有 TerminalPane 使用同一个 fork 度量接缝，不另造字符测量器。
// ignore: implementation_imports
import 'package:terminal_view/src/ui/char_metrics.dart' show calcCharSize;

import '../bindings/bindings.dart';
import '../settings/interface_font.dart'
    show FontRegistrar, InterfaceTypography;
import '../settings/terminal_font.dart' show bundledFontFamily;

part 'desktop_font_material.dart';
part 'desktop_font_cache.dart';

/// 一个 pane 持有一个控制器。调用者先过滤 Prepare 关联键和过期回包，再选择
/// Prepared；本模块不订阅 FFI、不发 Start，也不负责协议超时或会话状态。
class DesktopFontController extends ChangeNotifier {
  DesktopFontController({
    required this._cache,
    InterfaceTypography? legacyTypography,
  }) : _legacyTypography = legacyTypography ?? InterfaceTypography.terminal;

  final DesktopFontCache _cache;
  final InterfaceTypography _legacyTypography;
  _PreparedCapture? _capture;
  _FontEntry? _entry;
  DesktopTerminalProfile? _legacyProfile;
  DesktopFontMaterial? _current;
  Future<DesktopFontMaterial?>? _pending;
  int _revision = 0;
  bool _disposed = false;

  DesktopFontMaterial? get current => _current;
  DesktopPreparedFontIdentity? get selectedIdentity => _capture?.identity;
  bool get isLoading => _pending != null;

  /// 捕获并加载本回包的 binary。替换、失效或 dispose 后，旧 Future 返回 null，
  /// 不通知新 pane；失败返回可用 fallback，loadError 保留可观察的错误。
  Future<DesktopFontMaterial?> selectPrepared(
    DesktopPreparationResult result,
    Uint8List binary,
  ) {
    _checkAlive();
    final capture = _PreparedCapture(result, binary);
    // 先 acquire 再释放旧选择，同一资源在同 pane 重选时也共享正在进行的加载。
    final resource = capture.capturedResource;
    final entry = resource == null ? null : _cache._acquire(resource, binary);
    _clearSelection();
    _capture = capture;
    _entry = entry;
    return _resolve();
  }

  /// 仅用于无 metadata 的 dev 旧 profile（含 LegacyUndetermined）。不看历史
  /// raw family，继续跟随现有终端槽的即时通知；显式绑定必须走 Prepared。
  DesktopFontMaterial selectLegacy(
    DesktopTerminalProfile profile, {
    ProfileFontBinding? binding,
  }) {
    _checkAlive();
    if (binding != null && binding.mode != ProfileFontMode.legacyUndetermined) {
      throw ArgumentError('有明确字体绑定的 profile 不能走 legacy 路径');
    }
    _checkProfile(profile);
    _clearSelection();
    _legacyProfile = profile;
    _legacyTypography.addListener(_onLegacyChanged);
    _onLegacyChanged();
    return _current!;
  }

  /// 仅重试当前捕获；失败时 entry 持有原始字节，绝不重新查询旧槽。
  Future<DesktopFontMaterial?> retry() {
    _checkAlive();
    if (_pending != null) return _pending!;
    if (_capture == null) return Future.value(_current);
    return _resolve();
  }

  Future<DesktopFontMaterial?> _resolve() {
    final capture = _capture!;
    final entry = _entry;
    final revision = _revision;
    if (entry == null || entry.loaded) {
      final material = capture.material();
      _current = material;
      notifyListeners();
      return Future.microtask(
        () => _disposed || revision != _revision ? null : material,
      );
    }
    final pending = _cache._load(entry).then((error) {
      if (_disposed || revision != _revision) return null;
      _pending = null;
      final material = capture.material(error: error);
      _current = material;
      notifyListeners();
      return _disposed || revision != _revision ? null : material;
    });
    _pending = pending;
    notifyListeners();
    return pending;
  }

  void _onLegacyChanged() {
    final profile = _legacyProfile;
    if (_disposed || profile == null) return;
    _current = DesktopFontMaterial._(
      identity: null,
      family: _legacyTypography.fontFamily,
      projection: null,
      fontSize: profile.fontSize,
      expectedCols: profile.initialCols,
      expectedRows: profile.initialRows,
      usedLoadFallback: false,
      loadError: _legacyTypography.error,
      snapshotFallbackReason: null,
    );
    notifyListeners();
  }

  /// 关联身份可用于忽略旧 Cancel / 过期通知。无参数则清除当前选择。
  bool invalidate({DesktopPreparedFontIdentity? identity}) {
    _checkAlive();
    if (identity != null && identity != selectedIdentity) return false;
    _clearSelection();
    notifyListeners();
    return true;
  }

  void _clearSelection() {
    ++_revision;
    if (_legacyProfile != null) {
      _legacyTypography.removeListener(_onLegacyChanged);
    }
    _legacyProfile = null;
    final entry = _entry;
    if (entry != null) _cache._release(entry);
    _entry = null;
    _capture = null;
    _current = null;
    _pending = null;
  }

  void _checkAlive() {
    if (_disposed) throw StateError('字体控制器已卸载');
  }

  @override
  void dispose() {
    _clearSelection();
    _disposed = true;
    super.dispose();
  }
}

class _PreparedCapture {
  _PreparedCapture(DesktopPreparationResult result, Uint8List binary)
    : identity = DesktopPreparedFontIdentity.fromResult(result),
      snapshot = result.snapshot ?? (throw ArgumentError('Prepared 缺少快照')) {
    if (result.state != DesktopPreparationState.prepared ||
        result.failure != DesktopFailure.none ||
        result.identity.isEmpty) {
      throw ArgumentError('字体加载只接受成功的 Prepared 身份');
    }
    _checkProfile(snapshot.profile);
    final font = snapshot.font;
    final resource = font.resource;
    // 只检查 DTO 消费边界，不重做 Rust 的 hash、字体格式或 binary 身份校验。
    if (font.version != 1 ||
        font.profileId != snapshot.profile.id ||
        font.actualFamily.trim().isEmpty ||
        snapshot.fontProjection.isEmpty ||
        snapshot.fallbackProjection.isEmpty ||
        snapshot.fontProjection == snapshot.fallbackProjection ||
        snapshot.fallbackFamily.trim().isEmpty ||
        snapshot.resourceBytes != binary.length ||
        (resource == null && binary.isNotEmpty) ||
        (resource != null &&
            (resource.version != 1 ||
                resource.hash.isEmpty ||
                (binary.isNotEmpty && resource.family != font.actualFamily) ||
                (binary.isEmpty &&
                    (font.fallbackReason == null ||
                        font.actualFamily != snapshot.fallbackFamily))))) {
      throw ArgumentError('Prepared 字体投影与资源材料不一致');
    }
  }

  final DesktopPreparedFontIdentity identity;
  final DesktopPreparedSnapshot snapshot;

  // Rust 的失败投影可以保留请求资源引用；只有真实捕获字节才需要注册。
  ProfileFontResourceRef? get capturedResource =>
      snapshot.resourceBytes == 0 ? null : snapshot.font.resource;

  DesktopFontMaterial material({Object? error}) => DesktopFontMaterial._(
    identity: identity,
    family: error == null
        ? snapshot.font.actualFamily
        : snapshot.fallbackFamily,
    projection: error == null
        ? snapshot.fontProjection
        : snapshot.fallbackProjection,
    fontSize: snapshot.profile.fontSize,
    expectedCols: snapshot.profile.initialCols,
    expectedRows: snapshot.profile.initialRows,
    usedLoadFallback: error != null,
    loadError: error,
    snapshotFallbackReason: snapshot.font.fallbackReason,
  );
}

void _checkSize(double size) {
  if (!size.isFinite || size <= 0) throw ArgumentError('字体字号必须为有限正数');
}

void _checkProfile(DesktopTerminalProfile profile) {
  _checkSize(profile.fontSize);
  if (profile.initialCols <= 0 || profile.initialRows <= 0) {
    throw ArgumentError('期望 grid 必须为正数');
  }
}
