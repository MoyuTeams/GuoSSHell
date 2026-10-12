import 'dart:async';
import 'dart:typed_data';

import 'package:flutter/services.dart' show rootBundle;
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/settings/interface_font.dart';
import 'package:guosh_shell/src/settings/terminal_font.dart';
import 'package:guosh_shell/src/terminal/desktop_font.dart';
import 'package:guosh_shell/src/terminal/frame_terminal.dart';
import 'package:terminal_view/terminal_view.dart' show TerminalView;
// 与现有 pane 相同的实际度量接缝。
// ignore: implementation_imports
import 'package:terminal_view/src/ui/char_metrics.dart' show calcCharSize;

DesktopTerminalProfile _profile({String family = '旧 raw family'}) =>
    DesktopTerminalProfile(
      id: 'profile',
      name: '',
      version: 1,
      terminalType: 'xterm-256color',
      initialCols: 91,
      initialRows: 31,
      scrollbackLines: 1000,
      fontFamily: family,
      fontSize: 19.25,
      colorScheme: 'default',
      keyBindings: const [],
      leftAltAsMeta: true,
      rightAltAsMeta: false,
      enableCsiU: false,
      enableKittyKeyboard: false,
      mouseReporting: true,
      scrollOnOutput: true,
      scrollOnKeypress: true,
      answerback: '',
    );

ProfileFontBinding _binding(ProfileFontMode mode) =>
    ProfileFontBinding(version: 1, mode: mode);

ProfileFontResourceRef _resource([String hash = '已由发送端验证的内容身份']) =>
    ProfileFontResourceRef(version: 1, hash: hash, family: 'GuoshFont_$hash');

DesktopPreparationResult _prepared({
  ProfileFontResourceRef? resource,
  String family = 'Menlo',
  String identity = 'prepared-a',
  int session = 7,
  String generation = '9007199254740993',
  int request = 11,
  int byteCount = 0,
  ProfileFontFailure? fallbackReason,
  ProfileFontMode mode = ProfileFontMode.explicitFamily,
}) => DesktopPreparationResult(
  requestId: request,
  prepareRequestId: request,
  source: DesktopSource(
    sessionId: session,
    generation: Uint64.parse(generation),
  ),
  identity: identity,
  state: DesktopPreparationState.prepared,
  failure: DesktopFailure.none,
  remainingMs: 5000,
  snapshot: DesktopPreparedSnapshot(
    profile: _profile(),
    font: ProfileFontSnapshot(
      version: 1,
      profileId: 'profile',
      configurationRevision: Uint64.parse('42'),
      saved: _binding(mode),
      rawFamily: '旧 raw family',
      runtimeSource: mode == ProfileFontMode.legacyUndetermined
          ? ProfileFontRuntimeSource.legacyTerminalSlot
          : ProfileFontRuntimeSource.profileBinding,
      resource: resource,
      actualFamily: fallbackReason == null
          ? resource?.family ?? family
          : family,
      fallbackReason: fallbackReason,
    ),
    fontProjection: '$identity:主投影',
    fallbackProjection: '$identity:允许回退',
    fallbackFamily: bundledFontFamily,
    resourceBytes: byteCount,
    adjustments: const [],
  ),
);

class _Registration {
  _Registration(this.family, this.bytes);
  final String family;
  final Uint8List bytes;
  final completion = Completer<void>();
}

class _Registrar {
  final calls = <_Registration>[];

  Future<void> register(String family, Uint8List bytes) {
    final call = _Registration(family, bytes);
    calls.add(call);
    return call.completion.future;
  }
}

class _LegacySlot {
  String family = '终端槽初始字体';
  int queries = 0;
  late final typography = InterfaceTypography(
    slot: 'terminal',
    register: (_, _) async {},
    transport: (slot, action, name, bytes) async => FontReply(
      FontFileState(
        requestId: ++queries,
        slot: slot,
        family: family,
        label: family,
        error: '',
        applied: true,
      ),
      Uint8List(0),
    ),
  );

  Future<void> update(String value) {
    family = value;
    return typography.refresh();
  }
}

void main() {
  test('捕获只读字节，等待注册后才输出同身份实际投影与有效 profile 材料', () async {
    final registrar = _Registrar();
    final cache = DesktopFontCache(register: registrar.register);
    final font = DesktopFontController(cache: cache);
    addTearDown(font.dispose);
    final binary = Uint8List.fromList([1, 2, 3, 4]);
    final result = _prepared(resource: _resource(), byteCount: binary.length);
    final pending = font.selectPrepared(result, binary);
    expect(font.current, isNull);
    expect(font.isLoading, isTrue);
    expect(cache.retainedFutureCount, 1);
    expect(cache.retainedByteCount, 4);
    binary[0] = 99;
    expect(registrar.calls.single.bytes, [1, 2, 3, 4]);
    expect(() => registrar.calls.single.bytes[0] = 9, throwsUnsupportedError);
    registrar.calls.single.completion.complete();
    final material = (await pending)!;
    expect(material, same(font.current));
    expect(material.identity, DesktopPreparedFontIdentity.fromResult(result));
    expect(material.identity!.source.generation.toString(), '9007199254740993');
    expect(material.family, result.snapshot!.font.actualFamily);
    expect(material.family, isNot(result.snapshot!.profile.fontFamily));
    expect(material.projection, result.snapshot!.fontProjection);
    expect(material.fontSize, 19.25);
    expect((material.expectedCols, material.expectedRows), (91, 31));
    expect(material.usedLoadFallback, isFalse);
    expect(material.loadError, isNull);
    expect(font.isLoading, isFalse);
    expect(cache.retainedByteCount, 0);
    expect(cache.retainedFutureCount, 0);
    expect(cache.registeredResourceCount, 1);
  });

  test('不同 Prepared 身份共享同资源加载；最后解引用只保留 engine 注册身份', () async {
    final registrar = _Registrar();
    final cache = DesktopFontCache(register: registrar.register);
    final first = DesktopFontController(cache: cache);
    final second = DesktopFontController(cache: cache);
    addTearDown(first.dispose);
    addTearDown(second.dispose);
    final result = _prepared(resource: _resource(), byteCount: 3);
    final one = first.selectPrepared(result, Uint8List.fromList([4, 5, 6]));
    final twoResult = result.copyWith(
      source: result.source.copyWith(
        generation: Uint64.parse('9007199254740994'),
      ),
      identity: 'prepared-b',
      snapshot: () => result.snapshot!.copyWith(
        fontProjection: 'prepared-b:主投影',
        fallbackProjection: 'prepared-b:允许回退',
      ),
    );
    final two = second.selectPrepared(twoResult, Uint8List.fromList([4, 5, 6]));
    expect(registrar.calls, hasLength(1));
    expect(cache.activeResourceCount, 1);
    expect(cache.retainedByteCount, 3);
    first.invalidate();
    expect(cache.retainedByteCount, 3, reason: '另一个 pane 仍持有捕获');
    registrar.calls.single.completion.complete();
    expect(await one, isNull);
    expect(
      (await two)!.identity,
      DesktopPreparedFontIdentity.fromResult(twoResult),
    );
    expect(second.current!.projection, 'prepared-b:主投影');
    second.invalidate();
    expect(cache.activeResourceCount, 0);
    expect(cache.retainedByteCount, 0);
    expect(cache.retainedFutureCount, 0);
    await first.selectPrepared(result, Uint8List.fromList([4, 5, 6]));
    expect(registrar.calls, hasLength(1), reason: '已注册身份不重复加载');
  });

  test('同 pane 重选同资源复用进行中的加载，不接受旧选择完成', () async {
    final registrar = _Registrar();
    final cache = DesktopFontCache(register: registrar.register);
    final font = DesktopFontController(cache: cache);
    addTearDown(font.dispose);
    final result = _prepared(resource: _resource(), byteCount: 2);
    final first = font.selectPrepared(result, Uint8List.fromList([1, 2]));
    final second = font.selectPrepared(result, Uint8List.fromList([1, 2]));
    expect(registrar.calls, hasLength(1));
    registrar.calls.single.completion.complete();
    expect(await first, isNull);
    expect(await second, same(font.current));
    expect(cache.activeResourceCount, 1);
    font.invalidate();
    expect(cache.activeResourceCount, 0);
  });

  test('失败可观察且使用允许 fallback；重试同捕获字节，不查询槽', () async {
    final registrar = _Registrar();
    final cache = DesktopFontCache(register: registrar.register);
    final slot = _LegacySlot();
    final font = DesktopFontController(
      cache: cache,
      legacyTypography: slot.typography,
    );
    addTearDown(font.dispose);
    addTearDown(slot.typography.dispose);
    final result = _prepared(resource: _resource(), byteCount: 3);
    final binary = Uint8List.fromList([7, 8, 9]);
    final pending = font.selectPrepared(result, binary);
    final failure = StateError('注册失败');
    registrar.calls.single.completion.completeError(failure);
    final fallback = (await pending)!;
    expect(fallback.loadError, same(failure));
    expect(fallback.usedLoadFallback, isTrue);
    expect(fallback.family, result.snapshot!.fallbackFamily);
    expect(fallback.projection, result.snapshot!.fallbackProjection);
    expect(fallback.identity, DesktopPreparedFontIdentity.fromResult(result));
    expect(cache.retainedByteCount, 3);
    expect(cache.retainedFutureCount, 0, reason: '不永久缓存失败 Future');
    binary.fillRange(0, binary.length, 99);
    await slot.update('当前槽已换成别的字体');
    expect(font.current, same(fallback));
    final retry = font.retry();
    expect(font.retry(), same(retry), reason: '并发重试也复用同一 Future');
    expect(registrar.calls, hasLength(2));
    expect(registrar.calls.last.bytes, same(registrar.calls.first.bytes));
    expect(registrar.calls.last.bytes, [7, 8, 9]);
    expect(slot.queries, 1, reason: '只有测试主动 refresh，没有隐式槽查询');
    registrar.calls.last.completion.complete();
    final loaded = (await retry)!;
    expect(loaded.projection, result.snapshot!.fontProjection);
    expect(loaded.loadError, isNull);
    expect(loaded.usedLoadFallback, isFalse);
    expect(cache.retainedByteCount, 0);
    expect(cache.retainedFutureCount, 0);
  });

  test('registrar 同步异常同样转为可用 fallback，释放失败材料后可重选重试', () async {
    var attempts = 0;
    final failure = StateError('同步注册失败');
    final cache = DesktopFontCache(
      register: (_, _) {
        if (++attempts == 1) throw failure;
        return Future.value();
      },
    );
    final font = DesktopFontController(cache: cache);
    addTearDown(font.dispose);
    final result = _prepared(resource: _resource(), byteCount: 1);
    expect(
      (await font.selectPrepared(result, Uint8List(1)))!.loadError,
      same(failure),
    );
    font.invalidate();
    expect(cache.activeResourceCount, 0);
    expect(cache.retainedByteCount, 0);
    expect(cache.retainedFutureCount, 0);
    expect(cache.registeredResourceCount, 0);
    final loaded = await font.selectPrepared(result, Uint8List(1));
    expect(attempts, 2);
    expect(loaded!.usedLoadFallback, isFalse);
  });

  test('替换资源后旧 Future 不覆盖新选择；完整关联键才能失效当前材料', () async {
    final registrar = _Registrar();
    final cache = DesktopFontCache(register: registrar.register);
    final font = DesktopFontController(cache: cache);
    addTearDown(font.dispose);
    var notifications = 0;
    font.addListener(() => ++notifications);
    final old = _prepared(resource: _resource('旧资源'), byteCount: 2);
    final next = _prepared(
      resource: _resource('新资源'),
      byteCount: 3,
      identity: 'prepared-b',
    );
    final oldPending = font.selectPrepared(old, Uint8List(2));
    final nextPending = font.selectPrepared(next, Uint8List(3));
    expect(cache.retainedByteCount, 3);
    expect(cache.retainedFutureCount, 1);
    final key = DesktopPreparedFontIdentity.fromResult(next);
    for (final wrong in [
      DesktopPreparedFontIdentity.fromResult(old),
      DesktopPreparedFontIdentity(
        source: key.source.copyWith(sessionId: 8),
        prepareRequestId: key.prepareRequestId,
        identity: key.identity,
      ),
      DesktopPreparedFontIdentity(
        source: key.source.copyWith(
          generation: Uint64.parse('9007199254740994'),
        ),
        prepareRequestId: key.prepareRequestId,
        identity: key.identity,
      ),
      DesktopPreparedFontIdentity(
        source: key.source,
        prepareRequestId: key.prepareRequestId + 1,
        identity: key.identity,
      ),
    ]) {
      expect(font.invalidate(identity: wrong), isFalse);
    }
    final count = notifications;
    registrar.calls.first.completion.complete();
    expect(await oldPending, isNull);
    expect(notifications, count);
    expect(font.current, isNull);
    expect(font.selectedIdentity, key);
    registrar.calls.last.completion.complete();
    expect((await nextPending)!.identity, key);
    expect(font.current!.family, next.snapshot!.font.actualFamily);
    expect(font.invalidate(identity: key), isTrue);
    expect(font.current, isNull);
    expect(font.selectedIdentity, isNull);
    expect(cache.activeResourceCount, 0);
  });

  test('卸载释放 byte / Future 引用，迟到成功不能恢复已卸载 pane', () async {
    final registrar = _Registrar();
    final cache = DesktopFontCache(register: registrar.register);
    final font = DesktopFontController(cache: cache);
    final pending = font.selectPrepared(
      _prepared(resource: _resource(), byteCount: 5),
      Uint8List(5),
    );
    font.dispose();
    expect(cache.activeResourceCount, 0);
    expect(cache.retainedByteCount, 0);
    expect(cache.retainedFutureCount, 0);
    expect(font.isLoading, isFalse);
    registrar.calls.single.completion.complete();
    expect(await pending, isNull);
    expect(font.current, isNull);
    expect(cache.activeResourceCount, 0);
    expect(cache.registeredResourceCount, 1, reason: 'engine 注册不能假称被卸载');
    expect(font.retry, throwsStateError);
  });

  test('失效后同资源的新 pane 不被旧失败 Future 污染，也不恢复旧材料', () async {
    final registrar = _Registrar();
    final cache = DesktopFontCache(register: registrar.register);
    final oldFont = DesktopFontController(cache: cache);
    final newFont = DesktopFontController(cache: cache);
    addTearDown(oldFont.dispose);
    addTearDown(newFont.dispose);
    final result = _prepared(resource: _resource(), byteCount: 2);
    final oldPending = oldFont.selectPrepared(result, Uint8List(2));
    oldFont.invalidate();
    final newPending = newFont.selectPrepared(result, Uint8List(2));
    expect(registrar.calls, hasLength(2));
    registrar.calls.first.completion.completeError(StateError('旧调用失败'));
    expect(await oldPending, isNull);
    expect(newFont.current, isNull);
    expect(newFont.isLoading, isTrue);
    expect(cache.retainedByteCount, 2);
    expect(cache.retainedFutureCount, 1);
    registrar.calls.last.completion.complete();
    expect((await newPending)!.usedLoadFallback, isFalse);
    expect(oldFont.current, isNull);
  });

  test('无资源合法 family 与 Rust 已选内置 fallback 不调用 registrar', () async {
    final cache = DesktopFontCache(
      register: (_, _) => throw StateError('不应加载'),
    );
    final font = DesktopFontController(cache: cache);
    addTearDown(font.dispose);
    final family = (await font.selectPrepared(_prepared(), Uint8List(0)))!;
    expect(family.family, 'Menlo');
    expect(family.projection, 'prepared-a:主投影');
    final fallback = (await font.selectPrepared(
      _prepared(
        family: bundledFontFamily,
        fallbackReason: ProfileFontFailure.missing,
      ),
      Uint8List(0),
    ))!;
    expect(fallback.family, bundledFontFamily);
    expect(fallback.snapshotFallbackReason, ProfileFontFailure.missing);
    expect(
      fallback.projection,
      'prepared-a:主投影',
      reason: 'Rust 的实际投影已是 fallback',
    );
    expect(fallback.usedLoadFallback, isFalse);
    expect(fallback.loadError, isNull);
    expect(cache.activeResourceCount, 0);
    final missingResource = (await font.selectPrepared(
      _prepared(
        resource: _resource(),
        family: bundledFontFamily,
        fallbackReason: ProfileFontFailure.corrupt,
      ),
      Uint8List(0),
    ))!;
    expect(missingResource.family, bundledFontFamily);
    expect(missingResource.snapshotFallbackReason, ProfileFontFailure.corrupt);
    expect(missingResource.projection, 'prepared-a:主投影');
    expect(missingResource.usedLoadFallback, isFalse);
    expect(cache.activeResourceCount, 0, reason: '失败 DTO 的请求引用不是已捕获资源');
    final namedFamily = (await font.selectPrepared(
      _prepared(family: 'GuoshFont_已由发送端接受的family'),
      Uint8List(0),
    ))!;
    expect(namedFamily.family, 'GuoshFont_已由发送端接受的family');
  });

  test('所有无 metadata 历史 family 均走 dev 即时槽；Prepared 不随槽变化', () async {
    final slot = _LegacySlot();
    final font = DesktopFontController(
      cache: DesktopFontCache(),
      legacyTypography: slot.typography,
    );
    addTearDown(font.dispose);
    addTearDown(slot.typography.dispose);
    await slot.update('槽 A');
    for (final family in [
      'Menlo',
      bundledFontFamily,
      'GuoshFont_历史值',
      '其他历史值',
    ]) {
      for (final binding in [
        null,
        _binding(ProfileFontMode.legacyUndetermined),
      ]) {
        final material = font.selectLegacy(
          _profile(family: family),
          binding: binding,
        );
        expect(material.family, '槽 A');
        expect(material.identity, isNull);
        expect(material.projection, isNull);
        expect(material.fontSize, 19.25);
        await slot.update('槽 B');
        expect(font.current!.family, '槽 B');
        await slot.update('槽 A');
      }
    }
    final explicit = await font.selectPrepared(
      _prepared(family: 'Consolas'),
      Uint8List(0),
    );
    await slot.update('槽 C');
    expect(font.current, same(explicit));
    expect(font.current!.family, 'Consolas');
    for (final mode in [
      ProfileFontMode.explicitFamily,
      ProfileFontMode.explicitCachedTtf,
    ]) {
      expect(
        () => font.selectLegacy(_profile(), binding: _binding(mode)),
        throwsArgumentError,
      );
      expect(font.current, same(explicit));
    }
    // 原生 Prepared 即使来源是 legacy，也必须保全这次捕获，不回查当前槽。
    final capturedLegacy = await font.selectPrepared(
      _prepared(family: '已捕获槽字体', mode: ProfileFontMode.legacyUndetermined),
      Uint8List(0),
    );
    await slot.update('槽 D');
    expect(font.current, same(capturedLegacy));
    expect(font.current!.family, '已捕获槽字体');
  });

  test('切换 legacy 后 Prepared 迟到失败不覆盖即时槽，失效后槽通知不恢复材料', () async {
    final registrar = _Registrar();
    final cache = DesktopFontCache(register: registrar.register);
    final slot = _LegacySlot();
    final font = DesktopFontController(
      cache: cache,
      legacyTypography: slot.typography,
    );
    addTearDown(font.dispose);
    addTearDown(slot.typography.dispose);
    final pending = font.selectPrepared(
      _prepared(resource: _resource(), byteCount: 2),
      Uint8List(2),
    );
    final legacy = font.selectLegacy(_profile());
    registrar.calls.single.completion.completeError(StateError('迟到失败'));
    expect(await pending, isNull);
    expect(font.current, same(legacy));
    expect(cache.retainedByteCount, 0);
    font.invalidate();
    await slot.update('失效后的槽');
    expect(font.current, isNull);
  });

  test('无资源及已注册资源的异步消费也忽略中途失效，不返回另一选择材料', () async {
    final cache = DesktopFontCache(register: (_, _) async {});
    final font = DesktopFontController(cache: cache);
    addTearDown(font.dispose);
    final first = font.selectPrepared(_prepared(), Uint8List(0));
    final second = font.selectPrepared(
      _prepared(family: 'Consolas', identity: 'prepared-b'),
      Uint8List(0),
    );
    expect(await first, isNull);
    expect((await second)!.family, 'Consolas');
    final resource = _prepared(resource: _resource(), byteCount: 1);
    await font.selectPrepared(resource, Uint8List(1));
    final alreadyLoaded = font.selectPrepared(resource, Uint8List(1));
    font.invalidate();
    expect(await alreadyLoaded, isNull);
    expect(font.current, isNull);
    final immediateFont = DesktopFontController(cache: cache);
    final beforeDispose = immediateFont.selectPrepared(
      _prepared(),
      Uint8List(0),
    );
    immediateFont.dispose();
    expect(await beforeDispose, isNull);
  });

  test('边界材料不一致时拒绝，保留当前选择，不复制发送端字体验证器', () async {
    final font = DesktopFontController(cache: DesktopFontCache());
    addTearDown(font.dispose);
    final result = _prepared();
    final current = await font.selectPrepared(result, Uint8List(0));
    final snapshot = result.snapshot!;
    final invalid = <DesktopPreparationResult>[
      result.copyWith(snapshot: () => null),
      result.copyWith(identity: ''),
      result.copyWith(state: DesktopPreparationState.preparing),
      result.copyWith(failure: DesktopFailure.identityMismatch),
      result.copyWith(snapshot: () => snapshot.copyWith(resourceBytes: 1)),
      result.copyWith(snapshot: () => snapshot.copyWith(fontProjection: '')),
      result.copyWith(
        snapshot: () =>
            snapshot.copyWith(fallbackProjection: snapshot.fontProjection),
      ),
      result.copyWith(snapshot: () => snapshot.copyWith(fallbackFamily: '')),
      result.copyWith(
        snapshot: () =>
            snapshot.copyWith(font: snapshot.font.copyWith(version: 2)),
      ),
      result.copyWith(
        snapshot: () =>
            snapshot.copyWith(font: snapshot.font.copyWith(profileId: '错误配置')),
      ),
      result.copyWith(
        snapshot: () =>
            snapshot.copyWith(font: snapshot.font.copyWith(actualFamily: '')),
      ),
      result.copyWith(
        snapshot: () => snapshot.copyWith(
          profile: snapshot.profile.copyWith(fontSize: double.nan),
        ),
      ),
      result.copyWith(
        snapshot: () => snapshot.copyWith(
          profile: snapshot.profile.copyWith(initialCols: 0),
        ),
      ),
      _prepared(resource: _resource(), byteCount: 0),
    ];
    for (final value in invalid) {
      expect(
        () => font.selectPrepared(value, Uint8List(0)),
        throwsArgumentError,
      );
      expect(font.current, same(current));
    }
    final resource = _prepared(resource: _resource(), byteCount: 1);
    expect(
      () => font.selectPrepared(
        resource.copyWith(
          snapshot: () => resource.snapshot!.copyWith(
            font: resource.snapshot!.font.copyWith(actualFamily: '错误family'),
          ),
        ),
        Uint8List(1),
      ),
      throwsArgumentError,
    );
    expect(
      () => font.selectPrepared(
        result.copyWith(snapshot: () => snapshot.copyWith(resourceBytes: 1)),
        Uint8List(1),
      ),
      throwsArgumentError,
    );
  });

  testWidgets('真实 TTF 经 FontLoader 注册，字号 / 缩放材料与 TerminalView 实际布局一致', (
    tester,
  ) async {
    final cache = DesktopFontCache();
    final font = DesktopFontController(cache: cache);
    addTearDown(font.dispose);
    DesktopFontMaterial? material;
    await tester.runAsync(() async {
      final data = await rootBundle.load(
        'assets/fonts/MesloLGS-NF-Regular.ttf',
      );
      final binary = data.buffer.asUint8List(
        data.offsetInBytes,
        data.lengthInBytes,
      );
      material = await font.selectPrepared(
        _prepared(resource: _resource('真实TTF测试'), byteCount: binary.length),
        binary,
      );
    });
    expect(material!.usedLoadFallback, isFalse);
    expect(cache.registeredResourceCount, 1);
    expect(cache.retainedByteCount, 0);
    final terminal = FrameTerminal();
    addTearDown(terminal.dispose);
    final geometry = <TerminalGeometry>[];
    terminal.onResize = geometry.add;
    Future<Size> mount(double size, TextScaler scaler) async {
      final layout = material!.layout(zoomFontSize: size, textScaler: scaler);
      expect(layout.style.fontFamily, material!.family);
      expect(layout.style.fontSize, size);
      expect(layout.style.toTextStyle().fontSize, size);
      expect(layout.style.toStrutStyle().fontSize, size);
      expect(layout.style.toStrutStyle().fontFamily, material!.family);
      expect(layout.textScaler, same(scaler));
      expect((layout.expectedCols, layout.expectedRows), (91, 31));
      final cell = layout.measureCellSize();
      expect(cell, calcCharSize(layout.style, scaler));
      expect(cell.width, greaterThan(0));
      expect(cell.height, greaterThan(0));
      await tester.pumpWidget(
        Directionality(
          textDirection: TextDirection.ltr,
          child: Center(
            child: SizedBox(
              width: 600,
              height: 300,
              child: TerminalView(
                terminal,
                textStyle: layout.style,
                textScaler: layout.textScaler,
                padding: EdgeInsets.zero,
                readOnly: true,
              ),
            ),
          ),
        ),
      );
      await tester.pump();
      expect(geometry, isNotEmpty);
      final measured = geometry.last;
      expect(measured.cols, (600 / cell.width).floor());
      expect(measured.rows, (300 / cell.height).floor());
      expect((
        measured.cols,
        measured.rows,
      ), isNot((layout.expectedCols, layout.expectedRows)));
      return cell;
    }

    final normal = await mount(material!.fontSize, TextScaler.noScaling);
    final scaled = await mount(23.5, TextScaler.linear(1.5));
    expect(scaled.width, greaterThan(normal.width));
    expect(scaled.height, greaterThan(normal.height));
    expect(material!.fontSize, 19.25, reason: 'pane 缩放不改快照字号');
    for (final size in [0.0, -1.0, double.infinity, double.nan]) {
      expect(() => material!.layout(zoomFontSize: size), throwsArgumentError);
    }
    await tester.pumpWidget(const SizedBox());
  });
}
