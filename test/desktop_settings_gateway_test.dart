import 'dart:async';
import 'dart:typed_data';

import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/desktop/settings/desktop_settings_controller.dart';
import 'package:guosh_shell/src/desktop/settings/desktop_settings_gateway.dart';
import 'package:rinf/rinf.dart';

const settingsProfileA = '00000000-0000-4000-8000-000000000001';
const settingsProfileB = '00000000-0000-4000-8000-000000000002';
const settingsProfileC = '00000000-0000-4000-8000-000000000003';
const settingsFontHash =
    'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa';
const settingsFontFamily = 'GuoshFont_$settingsFontHash';
const settingsCachedFontBinding = ProfileFontBinding(
  version: 1,
  mode: ProfileFontMode.explicitCachedTtf,
  hash: settingsFontHash,
);
Uint64 settingsRevision(int value) => Uint64(BigInt.from(value));

const settingsBinding = DesktopKeyBinding(
  key: 'Delete',
  shift: true,
  control: false,
  alt: true,
  superKey: false,
  action: 'send:\u001b[3~',
);
const settingsFontBinding = ProfileFontBinding(
  version: 1,
  mode: ProfileFontMode.legacyUndetermined,
);
const settingsCapabilities = DesktopSettingsCapabilities(
  available: true,
  unavailableReason: '',
  colorSchemes: [
    'default',
    'one_dark',
    'solarized_dark',
    'solarized_light',
    'dracula',
    'monokai',
    'nord',
    'gruvbox_dark',
    'tokyo_night',
    'campbell_powershell',
  ],
  keyActions: [
    'send:\u001b[3~',
    'send:\u007f',
    'send:\u0008',
    'clear_scrollback',
    'new_tab',
    'split_vertical',
  ],
  minFontSize: 6,
  maxFontSize: 72,
  minScrollbackLines: 100,
  maxScrollbackLines: 1000000,
  minGridSize: 1,
  maxGridSize: 999,
  profileChangesRequireNewSession: true,
);

DesktopTerminalProfile settingsProfile(String id, {String name = '同名'}) =>
    DesktopTerminalProfile(
      id: id,
      name: name,
      version: 1,
      terminalType: 'xterm-256color',
      initialCols: 80,
      initialRows: 24,
      scrollbackLines: 1000,
      fontFamily: 'MesloLGS NF',
      fontSize: 14,
      colorScheme: 'default',
      keyBindings: [settingsBinding],
      leftAltAsMeta: true,
      rightAltAsMeta: false,
      enableCsiU: false,
      enableKittyKeyboard: false,
      mouseReporting: true,
      scrollOnOutput: true,
      scrollOnKeypress: false,
      answerback: 'GuoSSHell',
    );

DesktopConfiguration settingsSnapshot({
  int revision = 7,
  DesktopAppSettings? app,
  List<DesktopTerminalProfile>? profiles,
  Map<String, ProfileFontBinding> bindings = const {},
}) {
  final values =
      profiles ??
      [settingsProfile(settingsProfileA), settingsProfile(settingsProfileB)];
  return DesktopConfiguration(
    revision: settingsRevision(revision),
    app:
        app ??
        const DesktopAppSettings(
          defaultTerminalProfile: settingsProfileA,
          colorScheme: 'default',
          keyBindings: [settingsBinding],
        ),
    profiles: values,
    effectiveProfiles: [
      for (final profile in values)
        DesktopProfileEffective(
          id: profile.id,
          keyBindings: profile.keyBindings,
          scrollbackLines: profile.scrollbackLines,
          scrollbackLimitReason: '',
          fontBinding: bindings[profile.id] ?? settingsFontBinding,
        ),
    ],
  );
}

DesktopSettingsResult settingsReply(
  String id, {
  String status = 'ok',
  String targetId = '',
  DesktopConfiguration? snapshot,
  Uint64? revision,
  List<DesktopSettingsFieldError> errors = const [],
}) => DesktopSettingsResult(
  protocolVersion: 1,
  requestId: id,
  status: status,
  targetId: targetId,
  detail: '',
  fieldErrors: errors,
  revision: snapshot?.revision ?? revision,
  snapshot: snapshot,
  capabilities: settingsCapabilities,
);

/// 三个独立请求队列，不自动模拟 receiver queue 的执行顺序。
/// 每次发送、回包都经过实际生成 DTO 的二进制 codec 和 typed emission。
class DesktopSettingsFixture {
  final stream =
      StreamController<RustSignalPack<DesktopSettingsResult>>.broadcast(
        sync: true,
      );
  final settings = <DesktopSettingsRequest>[];
  final profiles = <DesktopProfileSettingsRequest>[];
  final ttfs = <DesktopProfileTtfRequest>[];
  final binaries = <Uint8List>[];
  late final DesktopSettingsGateway gateway = DesktopSettingsGateway(
    results: stream.stream,
    sendSettings: (request) => settings.add(
      DesktopSettingsRequest.bincodeDeserialize(request.bincodeSerialize()),
    ),
    sendProfile: (request) => profiles.add(
      DesktopProfileSettingsRequest.bincodeDeserialize(
        request.bincodeSerialize(),
      ),
    ),
    sendTtf: (request, bytes) {
      ttfs.add(
        DesktopProfileTtfRequest.bincodeDeserialize(request.bincodeSerialize()),
      );
      binaries.add(Uint8List.fromList(bytes));
    },
    timeout: const Duration(milliseconds: 40),
  );
  void emit(DesktopSettingsResult result) => stream.add(
    RustSignalPack(
      DesktopSettingsResult.bincodeDeserialize(result.bincodeSerialize()),
      Uint8List(0),
    ),
  );
  DesktopSettingsController controller({DesktopConfiguration? snapshot}) {
    final controller = DesktopSettingsController(gateway: gateway);
    final id = controller.refresh();
    emit(settingsReply(id, snapshot: snapshot ?? settingsSnapshot()));
    return controller;
  }

  Future<void> close() async {
    await gateway.close();
    await stream.close();
  }
}

DesktopSettingsRequestContext settingsContext(
  String unit, {
  String owner = 'fixture',
}) => DesktopSettingsRequestContext(
  ownerId: owner,
  generation: 3,
  editVersion: 5,
  unit: unit,
);

void main() {
  test('查询的加载、不可用和无效快照明确区分；失败不会制造默认可编辑配置', () async {
    final fixture = DesktopSettingsFixture();
    final controller = DesktopSettingsController(gateway: fixture.gateway);
    expect(controller.queryState, DesktopSettingsQueryState.idle);
    final unavailable = controller.refresh();
    expect(controller.queryState, DesktopSettingsQueryState.loading);
    fixture.emit(
      settingsReply(
        unavailable,
        status: 'unsupported_platform',
      ).copyWith(capabilities: settingsCapabilities.copyWith(available: false)),
    );
    expect(controller.queryState, DesktopSettingsQueryState.unavailable);
    expect(controller.editable, isFalse);
    expect(controller.app, isNull);
    final invalid = controller.refresh();
    fixture.emit(
      settingsReply(invalid, snapshot: settingsSnapshot(profiles: [])),
    );
    expect(controller.queryState, DesktopSettingsQueryState.failed);
    expect(controller.queryIssue, isFormatException);
    expect(controller.editable, isFalse);
    final retry = controller.refresh();
    fixture.emit(settingsReply(retry, snapshot: settingsSnapshot()));
    expect(controller.queryState, DesktopSettingsQueryState.ready);
    expect(controller.editable, isTrue);
    controller.dispose();
    await fixture.close();
  });

  test('gateway 在发送前监听，并消费实际生成 Rinf receive hook', () async {
    late DesktopSettingsGateway gateway;
    final received = Completer<DesktopSettingsGatewayEvent>();
    var listenedBeforeSend = false;
    gateway = DesktopSettingsGateway(
      sendSettings: (request) {
        listenedBeforeSend =
            gateway.pending.single.command.requestId == request.requestId;
        assignRustSignal['DesktopSettingsResult']!(
          settingsReply(
            request.requestId,
            snapshot: settingsSnapshot(),
          ).bincodeSerialize(),
          Uint8List(0),
        );
      },
    );
    final listener = gateway.events.listen(received.complete);
    gateway.send(
      DesktopConfigurationCommand(
        DesktopSettingsRequest(
          protocolVersion: 1,
          requestId: 'real-receive-hook',
          baseRevision: settingsRevision(0),
          operation: 'query',
          targetId: '',
          name: '',
        ),
      ),
      settingsContext('query'),
    );
    final event = await received.future.timeout(const Duration(seconds: 2));
    expect(listenedBeforeSend, isTrue);
    expect(event.pending.result!.status, 'ok');
    expect(gateway.snapshot, settingsSnapshot());
    expect(gateway.pending, isEmpty);
    await listener.cancel();
    await gateway.close();
  });

  for (final status in [
    'validation_error',
    'conflict',
    'storage_error',
    'unsupported_version',
    'unsupported_platform',
  ]) {
    test('$status 的空 target 和当前／空 revision 不吞掉真实错误', () async {
      final fixture = DesktopSettingsFixture();
      final controller = fixture.controller();
      controller.editProfile(
        settingsProfileA,
        value: controller.profiles[settingsProfileA]!.value.copyWith(
          answerback: '草稿',
        ),
      );
      final id = controller.saveProfile(settingsProfileA)!;
      fixture.emit(
        settingsReply(
          id,
          status: status,
          revision: status == 'storage_error' ? null : settingsRevision(12),
          errors: status == 'validation_error'
              ? [
                  const DesktopSettingsFieldError(
                    field: 'answerback',
                    code: 'blank',
                  ),
                ]
              : [],
        ),
      );
      expect(
        controller.stateFor(settingsProfileA).state,
        DesktopSettingsWriteState.failed,
      );
      expect(controller.stateFor(settingsProfileA).result!.status, status);
      expect(controller.profiles[settingsProfileA]!.value.answerback, '草稿');
      expect(
        controller.profiles[settingsProfileA]!.baseRevision,
        settingsRevision(7),
      );
      expect(fixture.gateway.blockingWrite, isNull);
      controller.dispose();
      await fixture.close();
    });
  }

  test('query ok、未知 RID 和重复结果不能确认写入', () async {
    final fixture = DesktopSettingsFixture();
    final controller = fixture.controller();
    controller.editApp(
      value: controller.app!.value.copyWith(colorScheme: 'nord'),
    );
    final id = controller.saveApp();
    fixture.emit(
      settingsReply(
        'not-this-request',
        status: 'applied',
        snapshot: settingsSnapshot(revision: 8),
      ),
    );
    final query = controller.refresh();
    fixture.emit(settingsReply(query, snapshot: settingsSnapshot()));
    expect(controller.app!.dirty, isTrue);
    expect(controller.stateFor('app').state, DesktopSettingsWriteState.saving);
    final saved = settingsSnapshot(
      revision: 8,
      app: fixture.settings
          .lastWhere((request) => request.operation == 'save_app')
          .app,
    );
    fixture.emit(settingsReply(id, status: 'applied', snapshot: saved));
    expect(controller.app!.dirty, isFalse);
    controller.editApp(
      value: controller.app!.value.copyWith(colorScheme: 'dracula'),
    );
    fixture.emit(settingsReply(id, status: 'applied', snapshot: saved));
    expect(controller.app!.value.colorScheme, 'dracula');
    expect(controller.app!.dirty, isTrue);
    controller.dispose();
    await fixture.close();
  });

  test('无效 Applied 不保存，旧 RID 的有效结果仍能被消费后显式核实', () async {
    final fixture = DesktopSettingsFixture();
    final controller = fixture.controller();
    controller.editApp(
      value: controller.app!.value.copyWith(colorScheme: 'nord'),
    );
    final id = controller.saveApp();
    fixture.emit(
      settingsReply(
        id,
        status: 'applied',
        snapshot: settingsSnapshot(revision: 7),
      ),
    );
    expect(controller.app!.dirty, isTrue);
    expect(
      fixture.gateway.blockingWrite!.phase,
      DesktopSettingsPendingPhase.uncertain,
    );
    final saved = settingsSnapshot(revision: 8, app: controller.app!.value);
    fixture.emit(settingsReply(id, status: 'applied', snapshot: saved));
    expect(
      fixture.gateway.blockingWrite!.phase,
      DesktopSettingsPendingPhase.needsConfirmation,
    );
    expect(
      () => controller.confirmUncertainWrite(id, saved.revision),
      throwsStateError,
    );
    final query = controller.refresh();
    fixture.emit(settingsReply(query, snapshot: saved));
    controller.confirmUncertainWrite(id, saved.revision);
    expect(fixture.gateway.blockingWrite, isNull);
    // 核实不是新的配置提交，也不能清除此窗口的未提交草稿。
    expect(controller.app!.baseRevision, settingsRevision(7));
    controller.dispose();
    await fixture.close();
  });
}
