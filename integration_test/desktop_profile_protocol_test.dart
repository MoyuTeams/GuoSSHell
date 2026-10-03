import 'dart:async';
import 'dart:io';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:integration_test/integration_test.dart';
import 'package:rinf/rinf.dart';

// flutter test integration_test/desktop_profile_protocol_test.dart -d windows --no-pub
// 只验证真实 native hub 与生成 bindings；不挂载设置 UI，不发送任何连接或私钥请求。
const _timeout = Duration(seconds: 15);
const _fontAsset = 'assets/fonts/MesloLGS-NF-Regular.ttf';
// 仓库 TTF 的 SHA-256；替换该资产时同步更新，不引入额外摘要依赖。
const _fontHash =
    'd97946186e97f8d7c0139e8983abf40a1d2d086924f2c5dbf1c29bd8f2c6e57d';

Future<RustSignalPack<T>> _exchange<T>(
  WidgetTester tester,
  Stream<RustSignalPack<T>> stream,
  bool Function(T) matches,
  void Function() send,
  String description,
) async {
  RustSignalPack<T>? response;
  // 必须先订阅再发送；只接收本轮响应，不读取 latestRustSignal 或替代 Rust 回包。
  final listener = stream.listen((pack) {
    if (matches(pack.message)) response ??= pack;
  });
  try {
    send();
    final elapsed = Stopwatch()..start();
    while (response == null) {
      if (elapsed.elapsed >= _timeout) {
        throw TestFailure('等待真实 Rust 响应超时：$description');
      }
      // integration_test 使用真实时钟；泵帧使 native 信号继续送达，不依赖固定等待。
      await tester.pump(const Duration(milliseconds: 50));
      expect(tester.takeException(), isNull, reason: description);
    }
    return response!;
  } finally {
    // 包括发送失败和超时路径，监听必须在 finalizeRust 前取消；取消错误不吞掉。
    await listener.cancel();
  }
}

Future<DesktopSettingsResult> _settingsReply(
  WidgetTester tester,
  String requestId,
  void Function() send,
) async => (await _exchange(
  tester,
  DesktopSettingsResult.rustSignalStream,
  (message) => message.requestId == requestId,
  send,
  requestId,
)).message;

DesktopConfiguration _snapshot(DesktopSettingsResult result, String status) {
  expect(result.protocolVersion, 1);
  expect(result.capabilities.available, isTrue);
  expect(
    result.status,
    status,
    reason: '桌面配置协议返回：${result.detail} ${result.fieldErrors}',
  );
  expect(result.fieldErrors, isEmpty);
  expect(result.snapshot, isNotNull);
  final snapshot = result.snapshot!;
  expect(result.revision, snapshot.revision);
  return snapshot;
}

Future<DesktopConfiguration> _query(
  WidgetTester tester,
  String requestId,
) async => _snapshot(
  await _settingsReply(
    tester,
    requestId,
    () => DesktopSettingsRequest(
      protocolVersion: 1,
      requestId: requestId,
      baseRevision: Uint64(BigInt.zero),
      operation: 'query',
      targetId: '',
      name: '',
    ).sendSignalToRust(),
  ),
  'ok',
);

DesktopConfiguration _applied(
  DesktopSettingsResult result,
  DesktopConfiguration before,
  String targetId,
) {
  final snapshot = _snapshot(result, 'applied');
  expect(result.targetId, targetId);
  expect(snapshot.revision.toBigInt(), before.revision.toBigInt() + BigInt.one);
  return snapshot;
}

DesktopTerminalProfile _profile(DesktopConfiguration snapshot, String id) =>
    snapshot.profiles.singleWhere((profile) => profile.id == id);

ProfileFontSnapshot _font(DesktopConfiguration snapshot, String id) {
  final effective = snapshot.effectiveProfiles.singleWhere(
    (profile) => profile.id == id,
  );
  expect(effective.font, isNotNull);
  final font = effective.font!;
  expect(font.version, 1);
  expect(font.profileId, id);
  expect(font.configurationRevision, snapshot.revision);
  expect(font.saved, effective.fontBinding);
  expect(font.rawFamily, _profile(snapshot, id).fontFamily);
  expect(font.runtimeSource, ProfileFontRuntimeSource.profileBinding);
  expect(font.capturedTerminalSlot, isNull);
  expect(font.fallbackReason, isNull);
  return font;
}

Future<List<int>?> _preferences(Directory support) async {
  final file = File('${support.path}${Platform.pathSeparator}preferences.json');
  return await file.exists() ? await file.readAsBytes() : null;
}

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('桌面 profile 字体协议真实 Dart↔Rust 保存、冲突及 TTF 二进制往返', (tester) async {
    if (!Platform.isWindows) {
      throw TestFailure('此协议验证只允许在 Windows 桌面设备上运行，不跳过 native 验证');
    }
    final owned = await Directory.systemTemp.createTemp(
      'guossh-profile-protocol-',
    );
    var rustStarted = false;
    try {
      await initializeRust(assignRustSignal);
      rustStarted = true;
      await tester.pumpWidget(
        const Directionality(
          textDirection: TextDirection.ltr,
          child: Text('桌面字体协议验证'),
        ),
      );
      final ready = await _exchange(
        tester,
        AppReady.rustSignalStream,
        (_) => true,
        () => AppStart(supportDir: owned.path).sendSignalToRust(),
        '隔离存储启动',
      );
      expect(ready.message.ok, isTrue, reason: ready.message.detail);
      // 不消费 AppReady 的调试自动连接字段，目录仅来自本轮 createTemp。
      final preferencesBefore = await _preferences(owned);
      final initial = await _query(tester, 'profile-query-initial');
      final id = initial.app.defaultTerminalProfile;
      expect(id, isNotEmpty);
      final original = _profile(initial, id);

      const inherit = ProfileFontBinding(
        version: 1,
        mode: ProfileFontMode.inheritTerminalSlot,
      );
      final inheritedDraft = original.copyWith(
        name: '协议：明确继承',
        fontSize: 20.0,
        initialCols: 93,
        initialRows: 31,
        answerback: '协议继承',
      );
      final inherited = _applied(
        await _settingsReply(
          tester,
          'profile-save-inherit',
          () => DesktopProfileSettingsRequest(
            protocolVersion: 1,
            requestId: 'profile-save-inherit',
            baseRevision: initial.revision,
            targetId: id,
            profile: inheritedDraft,
            fontBinding: inherit,
          ).sendSignalToRust(),
        ),
        initial,
        id,
      );
      expect(_profile(inherited, id), inheritedDraft);
      final inheritedFont = _font(inherited, id);
      expect(inheritedFont.saved, inherit);
      expect(inheritedFont.resource, isNull);
      expect(inheritedFont.actualFamily, 'MesloLGS NF');
      expect(await _query(tester, 'profile-query-inherit'), inherited);

      const explicit = ProfileFontBinding(
        version: 1,
        mode: ProfileFontMode.explicitFamily,
      );
      final explicitDraft = inheritedDraft.copyWith(
        name: '协议：显式字体',
        fontFamily: 'monospace',
        fontSize: 24.0,
        rightAltAsMeta: !inheritedDraft.rightAltAsMeta,
      );
      final explicitSaved = _applied(
        await _settingsReply(
          tester,
          'profile-save-explicit',
          () => DesktopProfileSettingsRequest(
            protocolVersion: 1,
            requestId: 'profile-save-explicit',
            baseRevision: inherited.revision,
            targetId: id,
            profile: explicitDraft,
            fontBinding: explicit,
          ).sendSignalToRust(),
        ),
        inherited,
        id,
      );
      expect(_profile(explicitSaved, id), explicitDraft);
      final explicitFont = _font(explicitSaved, id);
      expect(explicitFont.saved, explicit);
      expect(explicitFont.resource, isNull);
      expect(explicitFont.actualFamily, explicitDraft.fontFamily);
      final persisted = await _query(tester, 'profile-query-explicit');
      expect(persisted, explicitSaved);

      final conflict = await _settingsReply(
        tester,
        'profile-save-stale',
        () => DesktopProfileSettingsRequest(
          protocolVersion: 1,
          requestId: 'profile-save-stale',
          baseRevision: initial.revision,
          targetId: id,
          profile: inheritedDraft.copyWith(name: '不应保存', fontSize: 30.0),
          fontBinding: inherit,
        ).sendSignalToRust(),
      );
      expect(conflict.protocolVersion, 1);
      expect(conflict.status, 'conflict');
      expect(conflict.revision, persisted.revision);
      expect(conflict.snapshot, isNull);
      expect(conflict.fieldErrors, isEmpty);
      // 重新走真实查询，比较完整配置而非仅比较错误回包，确认旧 revision 未改库。
      final afterConflict = await _query(
        tester,
        'profile-query-after-conflict',
      );
      expect(afterConflict, persisted);

      final asset = await rootBundle.load(_fontAsset).timeout(_timeout);
      final bytes = asset.buffer.asUint8List(
        asset.offsetInBytes,
        asset.lengthInBytes,
      );
      expect(bytes, isNotEmpty);
      final ttfDraft = explicitDraft.copyWith(
        name: '协议：缓存 TTF',
        fontSize: 26.0,
      );
      final ttfSaved = _applied(
        await _settingsReply(
          tester,
          'profile-save-ttf',
          () => DesktopProfileTtfRequest(
            protocolVersion: 1,
            requestId: 'profile-save-ttf',
            baseRevision: afterConflict.revision,
            targetId: id,
            profile: ttfDraft,
          ).sendSignalToRust(bytes),
        ),
        afterConflict,
        id,
      );
      final ttfFont = _font(ttfSaved, id);
      expect(ttfFont.resource, isNotNull);
      final captured = ttfFont.resource!;
      expect(captured.version, 1);
      expect(captured.hash, _fontHash);
      expect(captured.family, 'GuoshFont_${captured.hash}');
      expect(
        ttfFont.saved,
        ProfileFontBinding(
          version: 1,
          mode: ProfileFontMode.explicitCachedTtf,
          hash: captured.hash,
        ),
      );
      expect(ttfFont.actualFamily, captured.family);
      expect(
        _profile(ttfSaved, id),
        ttfDraft.copyWith(fontFamily: captured.family),
      );

      // 直接发送保存结果捕获的资源引用；不重新查询当前槽，也不构造路径或字体加载器。
      final resource = await _exchange(
        tester,
        ProfileFontResourceState.rustSignalStream,
        (message) => message.requestId == 'profile-load-captured-resource',
        () => ProfileFontResourceRequest(
          requestId: 'profile-load-captured-resource',
          resource: captured,
        ).sendSignalToRust(),
        'profile-load-captured-resource',
      );
      expect(resource.message.failure, isNull);
      expect(resource.message.resource, captured);
      expect(resource.message.family, captured.family);
      expect(
        resource.binary,
        orderedEquals(bytes),
        reason: 'TTF binary 必须逐字节往返相同',
      );
      expect(await _query(tester, 'profile-query-ttf'), ttfSaved);
      // profile TTF 不走 FontFileRequest.apply；双槽偏好文件包括不存在状态也须保持原样。
      expect(await _preferences(owned), preferencesBefore);
      expect(
        await File('${owned.path}${Platform.pathSeparator}known_hosts')
            .exists(),
        isFalse,
      );
      expect(tester.takeException(), isNull);
    } finally {
      // 各次 exchange 已取消监听。即使卸载失败也必须尝试关闭 native 并清理独占目录。
      try {
        await tester.pumpWidget(const SizedBox.shrink());
      } finally {
        try {
          // 同步等待 Rust runtime 退出和 SQLite worker 关闭；不启动 build 或其他 runner。
          if (rustStarted) finalizeRust();
        } finally {
          // 精确删除本轮 createTemp 返回的目录；删除或存在检查失败直接使测试失败。
          await owned.delete(recursive: true);
          expect(await owned.exists(), isFalse);
        }
      }
    }
  });
}
