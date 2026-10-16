import 'dart:typed_data';

import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/desktop/settings/desktop_settings_controller.dart';
import 'package:guosh_shell/src/desktop/settings/desktop_settings_draft.dart';
import 'package:guosh_shell/src/desktop/settings/desktop_settings_gateway.dart';

import 'desktop_settings_gateway_test.dart';

void main() {
  for (final secondStatus in ['applied', 'conflict']) {
    test('同 target 两 RID $secondStatus 要求最新结果之后重新 query，不沿用已达成的旧门槛', () async {
      final fixture = DesktopSettingsFixture();
      final initial = settingsSnapshot(
        profiles: [
          settingsProfile(settingsProfileA),
          settingsProfile(settingsProfileB, name: '旧名'),
        ],
      );
      final sender = fixture.controller(snapshot: initial);
      final holder = DesktopSettingsController(gateway: fixture.gateway);
      addTearDown(() async {
        holder.dispose();
        sender.dispose();
        await fixture.close();
      });
      final selection = DesktopTtfSelection(
        family: settingsFontFamily,
        bytes: Uint8List.fromList([0, 1]),
      );
      holder.editProfile(settingsProfileB, ttf: selection);
      holder.editProfile(settingsProfileB, clearTtf: true);
      final original = holder.profiles[settingsProfileB]!;
      final first = sender.renameProfile(settingsProfileB, '名字 A');
      holder.editProfile(
        settingsProfileB,
        value: original.value.copyWith(name: '临时输入'),
      );
      holder.editProfile(
        settingsProfileB,
        value: original.value,
        numbers: original.numbers.copyWith(initialCols: '080', fontSize: '14.'),
      );
      final savedA = settingsSnapshot(
        revision: 8,
        profiles: [
          settingsProfile(settingsProfileA),
          settingsProfile(settingsProfileB, name: '名字 A'),
        ],
      );
      fixture.emit(
        settingsReply(
          first,
          status: 'applied',
          targetId: settingsProfileB,
          snapshot: savedA,
        ),
      );
      final queryAfterA = holder.refresh();
      fixture.emit(settingsReply(queryAfterA, snapshot: savedA));
      final retained = holder.profiles[settingsProfileB]!;
      final second = sender.renameProfile(settingsProfileB, '名字 B');
      expect(second, isNot(first));
      final beforeResultB = holder.refresh();
      final savedB = settingsSnapshot(
        revision: 9,
        profiles: [
          settingsProfile(settingsProfileA),
          settingsProfile(settingsProfileB, name: '名字 B'),
        ],
      );
      fixture.emit(
        secondStatus == 'applied'
            ? settingsReply(
                second,
                status: 'applied',
                targetId: settingsProfileB,
                snapshot: savedB,
              )
            : settingsReply(
                second,
                status: 'conflict',
                revision: savedB.revision,
              ),
      );
      void review(bool keepDraft) => holder.reviewProfile(
        settingsProfileB,
        revision: holder.snapshot!.revision,
        editVersion: holder.profiles[settingsProfileB]!.editVersion,
        keepDraft: keepDraft,
      );
      expect(() => review(true), throwsStateError);
      expect(() => review(false), throwsStateError);
      fixture.emit(settingsReply(beforeResultB, snapshot: savedB));
      expect(() => review(true), throwsStateError);
      final foreignQuery = sender.refresh();
      fixture.emit(settingsReply(foreignQuery, snapshot: savedB));
      expect(() => review(false), throwsStateError);
      expect(holder.profiles[settingsProfileB], same(retained));
      final queryAfterB = holder.refresh();
      fixture.emit(settingsReply(queryAfterB, snapshot: savedB));
      review(true);
      final kept = holder.profiles[settingsProfileB]!;
      expect(holder.needsReview(settingsProfileB), isFalse);
      expect(kept.value.name, '旧名');
      expect(kept.saved.name, '名字 B');
      expect(kept.numbers.initialCols, '080');
      expect(kept.numbers.fontSize, '14.');
      expect(kept.fontSelectionVersion, original.fontSelectionVersion);
      expect(kept.bindings.single.id, original.bindings.single.id);
      final save = holder.saveProfile(settingsProfileB)!;
      expect(fixture.profiles.single.baseRevision, savedB.revision);
      expect(fixture.profiles.single.profile.name, '旧名');
      fixture.emit(
        settingsReply(
          save,
          status: 'applied',
          targetId: settingsProfileB,
          snapshot: settingsSnapshot(
            revision: 10,
            profiles: [
              settingsProfile(settingsProfileA),
              fixture.profiles.single.profile,
            ],
          ),
        ),
      );
      expect(holder.profiles[settingsProfileB]!.dirty, isFalse);
      expect(holder.needsReview(settingsProfileB), isFalse);
      expect(
        fixture.settings.where(
          (request) => request.operation == 'rename_profile',
        ),
        hasLength(2),
      );
    });
  }

  test('同 RID 未决确认不重置已由结果后成功 query 达成的目标门槛', () async {
    final fixture = DesktopSettingsFixture();
    final initial = settingsSnapshot(
      profiles: [
        settingsProfile(settingsProfileA),
        settingsProfile(settingsProfileB, name: '旧名'),
      ],
    );
    final controller = fixture.controller(snapshot: initial);
    addTearDown(() async {
      controller.dispose();
      await fixture.close();
    });
    final original = controller.profiles[settingsProfileB]!;
    final id = controller.renameProfile(settingsProfileB, '新名');
    controller.editProfile(
      settingsProfileB,
      value: original.value.copyWith(name: '临时输入'),
    );
    controller.editProfile(settingsProfileB, value: original.value);
    await Future<void>.delayed(const Duration(milliseconds: 41));
    final saved = settingsSnapshot(
      revision: 8,
      profiles: [
        settingsProfile(settingsProfileA),
        settingsProfile(settingsProfileB, name: '新名'),
      ],
    );
    fixture.emit(
      settingsReply(
        id,
        status: 'applied',
        targetId: settingsProfileB,
        snapshot: saved,
      ),
    );
    final query = controller.refresh();
    fixture.emit(settingsReply(query, snapshot: saved));
    controller.confirmUncertainWrite(id, saved.revision);
    controller.reviewProfile(
      settingsProfileB,
      revision: saved.revision,
      editVersion: controller.profiles[settingsProfileB]!.editVersion,
      keepDraft: true,
    );
    expect(controller.needsReview(settingsProfileB), isFalse);
    expect(controller.profiles[settingsProfileB]!.value.name, '旧名');
    controller.reviewProfile(
      settingsProfileB,
      revision: saved.revision,
      editVersion: controller.profiles[settingsProfileB]!.editVersion,
      keepDraft: false,
    );
    expect(controller.needsReview(settingsProfileB), isFalse);
    expect(controller.profiles[settingsProfileB]!.value.name, '新名');
    expect(fixture.gateway.blockingWrite, isNull);
  });

  for (final operation in ['rename_profile', 'delete_profile']) {
    test('$operation Applied 核对只接受结果后本窗口发出的成功 query', () async {
      final fixture = DesktopSettingsFixture();
      final initial = settingsSnapshot(
        profiles: [
          settingsProfile(settingsProfileA),
          settingsProfile(settingsProfileB, name: '旧名'),
        ],
      );
      final controller = fixture.controller(snapshot: initial);
      addTearDown(() async {
        controller.dispose();
        await fixture.close();
      });
      final original = controller.profiles[settingsProfileB]!;
      final id = operation == 'rename_profile'
          ? controller.renameProfile(settingsProfileB, '新名')
          : controller.deleteProfile(settingsProfileB);
      controller.editProfile(
        settingsProfileB,
        value: original.value.copyWith(name: '临时输入'),
      );
      controller.editProfile(
        settingsProfileB,
        value: original.value,
        numbers: original.numbers.copyWith(initialCols: '080', fontSize: '14.'),
      );
      final beforeResultQuery = controller.refresh();
      final saved = settingsSnapshot(
        revision: 8,
        profiles: [
          settingsProfile(settingsProfileA),
          if (operation == 'rename_profile')
            settingsProfile(settingsProfileB, name: '新名'),
        ],
      );
      fixture.emit(
        settingsReply(
          id,
          status: 'applied',
          targetId: settingsProfileB,
          snapshot: saved,
        ),
      );
      final retained = controller.profiles[settingsProfileB]!;
      expect(retained.dirty, isFalse);
      expect(controller.needsReview(settingsProfileB), isTrue);
      void review(bool keepDraft) => controller.reviewProfile(
        settingsProfileB,
        revision: saved.revision,
        editVersion: controller.profiles[settingsProfileB]!.editVersion,
        keepDraft: keepDraft,
      );
      expect(() => review(false), throwsStateError);
      expect(() => review(true), throwsStateError);
      controller.reviewLifecycle(saved.revision);
      expect(() => review(false), throwsStateError);
      fixture.emit(settingsReply(beforeResultQuery, snapshot: saved));
      expect(controller.queryState, DesktopSettingsQueryState.ready);
      expect(() => review(false), throwsStateError);
      final foreign = DesktopSettingsController(gateway: fixture.gateway);
      addTearDown(foreign.dispose);
      final foreignQuery = foreign.refresh();
      fixture.emit(settingsReply(foreignQuery, snapshot: saved));
      expect(() => review(false), throwsStateError);
      final failedFreshQuery = controller.refresh();
      fixture.emit(settingsReply(failedFreshQuery, status: 'storage_error'));
      expect(controller.queryState, DesktopSettingsQueryState.failed);
      expect(() => review(false), throwsStateError);
      expect(controller.profiles[settingsProfileB], same(retained));
      expect(
        fixture.settings.where((request) => request.operation == operation),
        hasLength(1),
      );
      expect(fixture.profiles, isEmpty);
      final freshQuery = controller.refresh();
      fixture.emit(settingsReply(freshQuery, snapshot: saved));
      if (operation == 'rename_profile') {
        review(true);
        final kept = controller.profiles[settingsProfileB]!;
        expect(kept.value.name, '旧名');
        expect(kept.saved.name, '新名');
        expect(kept.baseRevision, saved.revision);
        expect(kept.numbers.initialCols, '080');
        expect(kept.numbers.fontSize, '14.');
        expect(kept.fontSelectionVersion, retained.fontSelectionVersion);
        expect(kept.bindings.single.id, retained.bindings.single.id);
        expect(controller.needsReview(settingsProfileB), isFalse);
      } else {
        expect(() => review(true), throwsStateError);
        expect(controller.profiles[settingsProfileB], same(retained));
      }
      review(false);
      expect(controller.needsReview(settingsProfileB), isFalse);
      if (operation == 'rename_profile') {
        expect(controller.profiles[settingsProfileB]!.value.name, '新名');
        expect(controller.profiles[settingsProfileB]!.dirty, isFalse);
        review(true);
      } else {
        expect(controller.profiles.containsKey(settingsProfileB), isFalse);
      }
      expect(
        fixture.settings.where((request) => request.operation == operation),
        hasLength(1),
      );
    });
  }

  test('foreignview 受保护的生命周期目标也须自身发起结果后的 fresh query', () async {
    final fixture = DesktopSettingsFixture();
    final initial = settingsSnapshot(
      profiles: [
        settingsProfile(settingsProfileA),
        settingsProfile(settingsProfileB, name: '旧名'),
      ],
    );
    final sender = fixture.controller(snapshot: initial);
    final holder = DesktopSettingsController(gateway: fixture.gateway);
    addTearDown(() async {
      holder.dispose();
      sender.dispose();
      await fixture.close();
    });
    final id = sender.renameProfile(settingsProfileB, '新名');
    final original = holder.profiles[settingsProfileB]!;
    holder.editProfile(
      settingsProfileB,
      value: original.value.copyWith(name: '临时输入'),
    );
    holder.editProfile(settingsProfileB, value: original.value);
    final beforeResultQuery = holder.refresh();
    final saved = settingsSnapshot(
      revision: 8,
      profiles: [
        settingsProfile(settingsProfileA),
        settingsProfile(settingsProfileB, name: '新名'),
      ],
    );
    fixture.emit(
      settingsReply(
        id,
        status: 'applied',
        targetId: settingsProfileB,
        snapshot: saved,
      ),
    );
    final retained = holder.profiles[settingsProfileB]!;
    void review() => holder.reviewProfile(
      settingsProfileB,
      revision: saved.revision,
      editVersion: retained.editVersion,
      keepDraft: true,
    );
    expect(() => review(), throwsStateError);
    fixture.emit(settingsReply(beforeResultQuery, snapshot: saved));
    final senderQuery = sender.refresh();
    fixture.emit(settingsReply(senderQuery, snapshot: saved));
    expect(() => review(), throwsStateError);
    expect(holder.profiles[settingsProfileB], same(retained));
    expect(holder.stateFor('lifecycle').state, DesktopSettingsWriteState.idle);
    final freshQuery = holder.refresh();
    fixture.emit(settingsReply(freshQuery, snapshot: saved));
    review();
    expect(holder.needsReview(settingsProfileB), isFalse);
    expect(holder.profiles[settingsProfileB]!.value.name, '旧名');
    expect(holder.profiles[settingsProfileB]!.saved.name, '新名');
    expect(
      fixture.settings.where(
        (request) => request.operation == 'rename_profile',
      ),
      hasLength(1),
    );
  });

  for (final operation in ['rename_profile', 'delete_profile']) {
    for (final delivery in ['applied', 'timeout', 'reopened']) {
      test('$operation $delivery 只保护实际目标草稿，生命周期确认不丢改回旧基线的输入', () async {
        final fixture = DesktopSettingsFixture();
        final initial = settingsSnapshot(
          profiles: [
            settingsProfile(settingsProfileA),
            settingsProfile(settingsProfileB, name: '旧名'),
          ],
        );
        final old = fixture.controller(snapshot: initial);
        final preview = DesktopTtfSelection(
          family: settingsFontFamily,
          bytes: Uint8List.fromList([0, 1]),
        );
        old.editProfile(settingsProfileB, ttf: preview);
        old.editProfile(settingsProfileB, clearTtf: true);
        final capturedVersion = old.profiles[settingsProfileB]!.editVersion;
        final id = operation == 'rename_profile'
            ? old.renameProfile(settingsProfileB, '新名')
            : old.deleteProfile(settingsProfileB);
        expect(fixture.gateway.blockingWrite!.context.unit, 'lifecycle');
        expect(
          fixture.gateway.blockingWrite!.context.editVersion,
          capturedVersion,
        );
        expect(
          fixture.gateway.blockingWrite!.command.targetId,
          settingsProfileB,
        );
        final controller = delivery == 'reopened'
            ? (() {
                old.dispose();
                return fixture.controller(snapshot: initial);
              })()
            : old;
        addTearDown(() async {
          controller.dispose();
          await fixture.close();
        });
        final original = controller.profiles[settingsProfileB]!;
        controller.selectProfile(settingsProfileA);
        controller.editProfile(
          settingsProfileB,
          value: original.value.copyWith(name: '临时输入'),
        );
        controller.editProfile(
          settingsProfileB,
          value: original.value,
          numbers: original.numbers.copyWith(
            initialCols: '080',
            fontSize: '14.',
          ),
        );
        final edited = controller.profiles[settingsProfileB]!;
        expect(edited.dirty, isFalse);
        if (delivery == 'timeout') {
          await Future<void>.delayed(const Duration(milliseconds: 41));
          expect(
            fixture.gateway.blockingWrite!.phase,
            DesktopSettingsPendingPhase.uncertain,
          );
        }
        final saved = settingsSnapshot(
          revision: 8,
          profiles: [
            settingsProfile(settingsProfileA),
            if (operation == 'rename_profile')
              settingsProfile(settingsProfileB, name: '新名'),
          ],
        );
        final earlyQuery = controller.refresh();
        fixture.emit(settingsReply(earlyQuery, snapshot: saved));
        expect(fixture.gateway.blockingWrite, isNotNull);
        expect(controller.profiles[settingsProfileB], same(edited));
        fixture.emit(
          settingsReply(
            id,
            status: 'applied',
            targetId: settingsProfileB,
            snapshot: saved,
          ),
        );
        final query = controller.refresh();
        fixture.emit(settingsReply(query, snapshot: saved));
        if (delivery != 'applied') {
          controller.confirmUncertainWrite(id, saved.revision);
        }
        expect(fixture.gateway.blockingWrite, isNull);
        final retained = controller.profiles[settingsProfileB];
        expect(retained, isNotNull);
        expect(retained!.value, original.value);
        expect(retained.baseRevision, settingsRevision(7));
        expect(retained.editVersion, edited.editVersion);
        expect(retained.fontSelectionVersion, edited.fontSelectionVersion);
        expect(retained.numbers.initialCols, '080');
        expect(retained.numbers.fontSize, '14.');
        expect(retained.bindings.single.id, original.bindings.single.id);
        expect(controller.needsReview(settingsProfileB), isTrue);
        expect(controller.needsReview(settingsProfileA), isFalse);
        expect(
          controller.stateFor(settingsProfileB).state,
          DesktopSettingsWriteState.idle,
        );
        expect(controller.selectedProfileId, settingsProfileA);
        controller.reviewLifecycle(saved.revision);
        expect(controller.needsReview(settingsProfileB), isTrue);
        if (operation == 'rename_profile') {
          controller.reviewProfile(
            settingsProfileB,
            revision: saved.revision,
            editVersion: retained.editVersion,
            keepDraft: true,
          );
          expect(controller.profiles[settingsProfileB]!.value.name, '旧名');
          expect(controller.profiles[settingsProfileB]!.saved.name, '新名');
          expect(controller.profiles[settingsProfileB]!.dirty, isTrue);
          expect(controller.needsReview(settingsProfileB), isFalse);
        } else {
          expect(
            () => controller.reviewProfile(
              settingsProfileB,
              revision: saved.revision,
              editVersion: retained.editVersion,
              keepDraft: true,
            ),
            throwsStateError,
          );
          expect(controller.profiles[settingsProfileB], same(retained));
        }
        controller.reviewProfile(
          settingsProfileB,
          revision: saved.revision,
          editVersion: controller.profiles[settingsProfileB]!.editVersion,
          keepDraft: false,
        );
        if (operation == 'rename_profile') {
          expect(controller.profiles[settingsProfileB]!.value.name, '新名');
          expect(controller.profiles[settingsProfileB]!.dirty, isFalse);
        } else {
          expect(controller.profiles.containsKey(settingsProfileB), isFalse);
        }
        expect(controller.needsReview(settingsProfileB), isFalse);
      });
    }
  }

  for (final operation in ['create_profile', 'copy_profile']) {
    test('$operation 不把已有源 profile 标成被修改目标或误阻止后续复制', () async {
      final fixture = DesktopSettingsFixture();
      final controller = fixture.controller();
      addTearDown(() async {
        controller.dispose();
        await fixture.close();
      });
      final original = controller.profiles[settingsProfileB]!;
      final id = operation == 'create_profile'
          ? controller.createProfile('新增')
          : controller.copyProfile(settingsProfileB, '新增');
      controller.editProfile(
        settingsProfileB,
        value: original.value.copyWith(name: '临时输入'),
      );
      controller.editProfile(settingsProfileB, value: original.value);
      fixture.emit(
        settingsReply(
          id,
          status: 'applied',
          targetId: settingsProfileC,
          snapshot: settingsSnapshot(
            revision: 8,
            profiles: [
              settingsProfile(settingsProfileA),
              settingsProfile(settingsProfileB),
              settingsProfile(settingsProfileC, name: '新增'),
            ],
          ),
        ),
      );
      expect(controller.needsReview(settingsProfileB), isFalse);
      expect(controller.needsReview(settingsProfileA), isFalse);
      expect(
        controller.profiles[settingsProfileB]!.baseRevision,
        settingsRevision(8),
      );
      expect(controller.profiles[settingsProfileB]!.value, original.value);
      final next = controller.copyProfile(settingsProfileB, '再次复制');
      expect(fixture.settings.last.targetId, settingsProfileB);
      expect(fixture.settings.last.baseRevision, settingsRevision(8));
      fixture.emit(settingsReply(next, status: 'storage_error'));
    });
  }

  test(
    'rename conflict 空 target 按本地 RID 关联实际草稿，fresh query 后由 target 核对解除',
    () async {
      final fixture = DesktopSettingsFixture();
      final initial = settingsSnapshot(
        profiles: [
          settingsProfile(settingsProfileA),
          settingsProfile(settingsProfileB, name: '旧名'),
        ],
      );
      final controller = fixture.controller(snapshot: initial);
      addTearDown(() async {
        controller.dispose();
        await fixture.close();
      });
      final original = controller.profiles[settingsProfileB]!;
      final id = controller.renameProfile(settingsProfileB, '请求的新名');
      controller.editProfile(
        settingsProfileB,
        value: original.value.copyWith(name: '临时输入'),
      );
      controller.editProfile(settingsProfileB, value: original.value);
      fixture.emit(
        settingsReply(id, status: 'conflict', revision: settingsRevision(12)),
      );
      final saved = settingsSnapshot(
        revision: 12,
        profiles: [
          settingsProfile(settingsProfileA),
          settingsProfile(settingsProfileB, name: '外部的新名'),
        ],
      );
      final query = controller.refresh();
      fixture.emit(settingsReply(query, snapshot: saved));
      final retained = controller.profiles[settingsProfileB]!;
      expect(retained.value.name, '旧名');
      expect(retained.baseRevision, settingsRevision(7));
      expect(controller.needsReview(settingsProfileB), isTrue);
      expect(controller.stateFor('lifecycle').result!.targetId, '');
      expect(
        controller.stateFor('lifecycle').pending!.command.targetId,
        settingsProfileB,
      );
      controller.reviewLifecycle(saved.revision);
      expect(controller.needsReview(settingsProfileB), isTrue);
      controller.reviewProfile(
        settingsProfileB,
        revision: saved.revision,
        editVersion: retained.editVersion,
        keepDraft: true,
      );
      expect(controller.needsReview(settingsProfileB), isFalse);
      expect(controller.profiles[settingsProfileB]!.value.name, '旧名');
      expect(controller.profiles[settingsProfileB]!.saved.name, '外部的新名');
    },
  );

  for (final channel in ['app', 'profile', 'ttf']) {
    test('重开 scope $channel 的旧写入不能覆盖新窗口改回旧基线的输入', () async {
      final fixture = DesktopSettingsFixture();
      final old = fixture.controller();
      late String id;
      late DesktopConfiguration saved;
      if (channel == 'app') {
        old.editApp(value: old.app!.value.copyWith(colorScheme: 'nord'));
        id = old.saveApp();
        saved = settingsSnapshot(revision: 8, app: fixture.settings.last.app);
      } else {
        old.editProfile(
          settingsProfileA,
          value: old.profiles[settingsProfileA]!.value.copyWith(
            answerback: '旧写入',
          ),
          ttf: channel == 'ttf'
              ? DesktopTtfSelection(
                  family: settingsFontFamily,
                  bytes: Uint8List.fromList([0, 1, 2]),
                )
              : null,
        );
        id = old.saveProfile(settingsProfileA)!;
        final sent = channel == 'ttf'
            ? fixture.ttfs.single.profile
            : fixture.profiles.single.profile;
        saved = settingsSnapshot(
          revision: 8,
          profiles: [
            channel == 'ttf'
                ? sent.copyWith(fontFamily: settingsFontFamily)
                : sent,
            settingsProfile(settingsProfileB),
          ],
          bindings: channel == 'ttf'
              ? {settingsProfileA: settingsCachedFontBinding}
              : const {},
        );
      }
      final owner = old.ownerId;
      old.dispose();
      final reopened = fixture.controller();
      addTearDown(() async {
        reopened.dispose();
        await fixture.close();
      });
      expect(fixture.gateway.blockingWrite!.context.ownerId, owner);
      final unit = channel == 'app' ? 'app' : settingsProfileA;
      final originalApp = reopened.app!.value;
      final originalProfile = reopened.profiles[settingsProfileA]!.value;
      if (channel == 'app') {
        reopened.editApp(value: originalApp.copyWith(colorScheme: 'dracula'));
        reopened.editApp(value: originalApp);
        expect(reopened.app!.dirty, isFalse);
      } else {
        reopened.editProfile(
          unit,
          value: originalProfile.copyWith(answerback: '新窗口输入'),
        );
        reopened.editProfile(
          unit,
          value: originalProfile,
          numbers: DesktopProfileNumbers.fromProfile(originalProfile)
              .copyWith(initialCols: '080'),
        );
        expect(reopened.profiles[unit]!.dirty, isFalse);
      }
      fixture.emit(
        settingsReply(
          id,
          status: 'applied',
          targetId: channel == 'app' ? '' : unit,
          snapshot: saved,
        ),
      );
      final query = reopened.refresh();
      fixture.emit(settingsReply(query, snapshot: saved));
      reopened.confirmUncertainWrite(id, saved.revision);
      expect(fixture.gateway.blockingWrite, isNull);
      expect(reopened.stateFor(unit).state, DesktopSettingsWriteState.idle);
      expect(reopened.needsReview(unit), isTrue);
      if (channel == 'app') {
        expect(reopened.app!.value, originalApp);
        expect(reopened.app!.baseRevision, settingsRevision(7));
      } else {
        expect(reopened.profiles[unit]!.value, originalProfile);
        expect(reopened.profiles[unit]!.numbers.initialCols, '080');
        expect(reopened.profiles[unit]!.baseRevision, settingsRevision(7));
      }
      expect(fixture.ttfs, hasLength(channel == 'ttf' ? 1 : 0));
      expect(fixture.profiles, hasLength(channel == 'profile' ? 1 : 0));
    });
  }

  test('TTF 选择取消后再次使用同一对象，旧 Applied 不确认新的选择版本', () async {
    final fixture = DesktopSettingsFixture();
    final controller = fixture.controller();
    addTearDown(() async {
      controller.dispose();
      await fixture.close();
    });
    final selection = DesktopTtfSelection(
      family: settingsFontFamily,
      bytes: Uint8List.fromList([0, 1, 2]),
    );
    controller.editProfile(settingsProfileA, ttf: selection);
    final id = controller.saveProfile(settingsProfileA)!;
    final context = fixture.gateway.blockingWrite!.context;
    final sent = fixture.ttfs.single.profile;
    expect(identical(context.ttfSelection, selection), isTrue);
    controller.editProfile(settingsProfileA, clearTtf: true);
    controller.editProfile(settingsProfileA, ttf: selection);
    final version = controller.profiles[settingsProfileA]!.fontSelectionVersion;
    expect(version, greaterThan(context.fontSelectionVersion!));
    fixture.emit(
      settingsReply(
        id,
        status: 'applied',
        targetId: settingsProfileA,
        snapshot: settingsSnapshot(
          revision: 8,
          profiles: [
            sent.copyWith(fontFamily: settingsFontFamily),
            settingsProfile(settingsProfileB),
          ],
          bindings: {settingsProfileA: settingsCachedFontBinding},
        ),
      ),
    );
    final draft = controller.profiles[settingsProfileA]!;
    expect(identical(draft.ttf, selection), isTrue);
    expect(draft.fontSelectionVersion, version);
    expect(draft.value.fontFamily, sent.fontFamily);
    expect(draft.fontBinding, settingsFontBinding);
    expect(draft.savedFontBinding, settingsCachedFontBinding);
  });

  test('TTF 未决解除仍保输入，显式核对后确认已提交选择而非重发 binary', () async {
    final fixture = DesktopSettingsFixture();
    final controller = fixture.controller();
    addTearDown(() async {
      controller.dispose();
      await fixture.close();
    });
    final selection = DesktopTtfSelection(
      family: settingsFontFamily,
      bytes: Uint8List.fromList([0, 1, 2]),
    );
    controller.editProfile(settingsProfileA, ttf: selection);
    final id = controller.saveProfile(settingsProfileA)!;
    final sent = fixture.ttfs.single.profile;
    controller.editProfile(
      settingsProfileA,
      value: sent.copyWith(answerback: '未决时输入'),
      numbers: controller.profiles[settingsProfileA]!.numbers.copyWith(
        fontSize: '14.',
      ),
    );
    await Future<void>.delayed(const Duration(milliseconds: 41));
    final saved = settingsSnapshot(
      revision: 8,
      profiles: [
        sent.copyWith(fontFamily: settingsFontFamily),
        settingsProfile(settingsProfileB),
      ],
      bindings: {settingsProfileA: settingsCachedFontBinding},
    );
    fixture.emit(
      settingsReply(
        id,
        status: 'applied',
        targetId: settingsProfileA,
        snapshot: saved,
      ),
    );
    final query = controller.refresh();
    fixture.emit(settingsReply(query, snapshot: saved));
    controller.confirmUncertainWrite(id, saved.revision);
    var draft = controller.profiles[settingsProfileA]!;
    expect(draft.value.answerback, '未决时输入');
    expect(draft.baseRevision, settingsRevision(7));
    expect(identical(draft.ttf, selection), isTrue);
    expect(controller.needsReview(settingsProfileA), isTrue);
    controller.reviewProfile(
      settingsProfileA,
      revision: saved.revision,
      editVersion: draft.editVersion,
      keepDraft: true,
    );
    draft = controller.profiles[settingsProfileA]!;
    expect(draft.ttf, isNull);
    expect(draft.value.fontFamily, settingsFontFamily);
    expect(draft.fontBinding, settingsCachedFontBinding);
    expect(draft.value.answerback, '未决时输入');
    expect(draft.numbers.fontSize, '14.');
    final next = controller.saveProfile(settingsProfileA)!;
    expect(fixture.ttfs, hasLength(1));
    expect(fixture.profiles.single.fontBinding, settingsCachedFontBinding);
    fixture.emit(settingsReply(next, status: 'storage_error'));
  });

  for (final unit in ['app', settingsProfileA]) {
    for (final outcome in ['uncertain', 'conflict']) {
      for (final keepDraft in [true, false]) {
        test('$unit $outcome 改回旧基线的输入保留到显式核对 keepDraft=$keepDraft', () async {
          final fixture = DesktopSettingsFixture();
          final controller = fixture.controller();
          addTearDown(() async {
            controller.dispose();
            await fixture.close();
          });
          final originalApp = controller.app!.value;
          final originalProfile = controller.profiles[settingsProfileA]!.value;
          late String id;
          late DesktopConfiguration saved;
          if (unit == 'app') {
            controller.editApp(
              value: originalApp.copyWith(colorScheme: 'nord'),
            );
            id = controller.saveApp();
            saved = settingsSnapshot(
              revision: outcome == 'uncertain' ? 8 : 12,
              app: fixture.settings.last.app,
            );
            controller.editApp(value: originalApp);
            expect(controller.app!.dirty, isFalse);
          } else {
            controller.editProfile(
              unit,
              value: originalProfile.copyWith(
                answerback: '已发送',
                initialCols: 96,
              ),
            );
            id = controller.saveProfile(unit)!;
            saved = settingsSnapshot(
              revision: outcome == 'uncertain' ? 8 : 12,
              profiles: [
                fixture.profiles.single.profile,
                settingsProfile(settingsProfileB),
              ],
            );
            controller.editProfile(
              unit,
              value: originalProfile,
              numbers: DesktopProfileNumbers.fromProfile(originalProfile)
                  .copyWith(initialCols: '080'),
            );
            expect(controller.profiles[unit]!.dirty, isFalse);
          }
          final editedVersion = unit == 'app'
              ? controller.app!.editVersion
              : controller.profiles[unit]!.editVersion;
          expect(
            editedVersion,
            greaterThan(fixture.gateway.blockingWrite!.context.editVersion),
          );
          if (outcome == 'uncertain') {
            await Future<void>.delayed(const Duration(milliseconds: 41));
            expect(
              fixture.gateway.blockingWrite!.phase,
              DesktopSettingsPendingPhase.uncertain,
            );
            fixture.emit(
              settingsReply(
                id,
                status: 'applied',
                targetId: unit == 'app' ? '' : unit,
                snapshot: saved,
              ),
            );
          } else {
            fixture.emit(
              settingsReply(id, status: 'conflict', revision: saved.revision),
            );
          }
          final query = controller.refresh();
          fixture.emit(settingsReply(query, snapshot: saved));
          if (outcome == 'uncertain') {
            controller.confirmUncertainWrite(id, saved.revision);
          }
          expect(fixture.gateway.blockingWrite, isNull);
          if (unit == 'app') {
            expect(controller.app!.value, originalApp);
            expect(controller.app!.baseRevision, settingsRevision(7));
            expect(controller.app!.editVersion, editedVersion);
          } else {
            expect(controller.profiles[unit]!.value, originalProfile);
            expect(controller.profiles[unit]!.numbers.initialCols, '080');
            expect(
              controller.profiles[unit]!.baseRevision,
              settingsRevision(7),
            );
            expect(controller.profiles[unit]!.editVersion, editedVersion);
          }
          expect(controller.needsReview(unit), isTrue);
          expect(controller.dirty, isTrue);
          if (unit == 'app') {
            controller.reviewApp(
              revision: saved.revision,
              editVersion: editedVersion,
              keepDraft: keepDraft,
            );
            expect(controller.app!.value, keepDraft ? originalApp : saved.app);
            expect(controller.app!.baseRevision, saved.revision);
            expect(controller.app!.dirty, keepDraft);
          } else {
            controller.reviewProfile(
              unit,
              revision: saved.revision,
              editVersion: editedVersion,
              keepDraft: keepDraft,
            );
            expect(
              controller.profiles[unit]!.value,
              keepDraft ? originalProfile : saved.profiles.first,
            );
            expect(
              controller.profiles[unit]!.numbers.initialCols,
              keepDraft ? '080' : '96',
            );
            expect(controller.profiles[unit]!.baseRevision, saved.revision);
            expect(controller.profiles[unit]!.dirty, keepDraft);
          }
          expect(controller.needsReview(unit), isFalse);
          final afterReview = controller.refresh();
          fixture.emit(settingsReply(afterReview, snapshot: saved));
          if (unit == 'app') {
            expect(controller.app!.value, keepDraft ? originalApp : saved.app);
          } else {
            expect(
              controller.profiles[unit]!.value,
              keepDraft ? originalProfile : saved.profiles.first,
            );
          }
        });
      }
    }
  }

  test('没有未决输入意图的 clean 草稿仍自动接纳正常查询快照', () async {
    final fixture = DesktopSettingsFixture();
    final controller = fixture.controller();
    final saved = settingsSnapshot(
      revision: 8,
      app: controller.app!.value.copyWith(colorScheme: 'nord'),
      profiles: [
        controller.profiles[settingsProfileA]!.value.copyWith(
          initialCols: 96,
          answerback: '外部值',
        ),
        settingsProfile(settingsProfileB),
      ],
    );
    final query = controller.refresh();
    fixture.emit(settingsReply(query, snapshot: saved));
    expect(controller.app!.value, saved.app);
    expect(controller.profiles[settingsProfileA]!.value, saved.profiles.first);
    expect(controller.profiles[settingsProfileA]!.numbers.initialCols, '96');
    expect(controller.app!.baseRevision, saved.revision);
    expect(controller.profiles[settingsProfileA]!.baseRevision, saved.revision);
    expect(controller.dirty, isFalse);
    expect(controller.needsReview('app'), isFalse);
    expect(controller.needsReview(settingsProfileA), isFalse);
    controller.dispose();
    await fixture.close();
  });

  test('TTF Applied 合并同一已发送选择，保留新非字体输入且后续不重发旧 binary', () async {
    final fixture = DesktopSettingsFixture();
    final controller = fixture.controller();
    addTearDown(() async {
      controller.dispose();
      await fixture.close();
    });
    final selection = DesktopTtfSelection(
      family: settingsFontFamily,
      bytes: Uint8List.fromList([0, 1, 2]),
    );
    controller.editProfile(settingsProfileA, ttf: selection);
    final id = controller.saveProfile(settingsProfileA)!;
    final sent = fixture.ttfs.single.profile;
    controller.editProfile(
      settingsProfileA,
      value: controller.profiles[settingsProfileA]!.value.copyWith(
        answerback: '保存期间新输入',
      ),
      numbers: controller.profiles[settingsProfileA]!.numbers.copyWith(
        fontSize: '14.',
      ),
    );
    final version = controller.profiles[settingsProfileA]!.editVersion;
    fixture.emit(
      settingsReply(
        id,
        status: 'applied',
        targetId: settingsProfileA,
        snapshot: settingsSnapshot(
          revision: 8,
          profiles: [
            sent.copyWith(fontFamily: settingsFontFamily),
            settingsProfile(settingsProfileB),
          ],
          bindings: {settingsProfileA: settingsCachedFontBinding},
        ),
      ),
    );
    var draft = controller.profiles[settingsProfileA]!;
    expect(draft.value.answerback, '保存期间新输入');
    expect(draft.numbers.fontSize, '14.');
    expect(draft.editVersion, version);
    expect(draft.value.fontFamily, settingsFontFamily);
    expect(draft.fontBinding, settingsCachedFontBinding);
    expect(draft.ttf, isNull);
    controller.editProfile(
      settingsProfileA,
      value: draft.value.copyWith(answerback: sent.answerback),
    );
    expect(controller.profiles[settingsProfileA]!.dirty, isFalse);
    controller.editProfile(
      settingsProfileA,
      value: controller.profiles[settingsProfileA]!.value.copyWith(
        answerback: '下一次非字体保存',
      ),
    );
    final next = controller.saveProfile(settingsProfileA)!;
    expect(fixture.ttfs, hasLength(1));
    expect(fixture.binaries, hasLength(1));
    expect(fixture.profiles.single.baseRevision, settingsRevision(8));
    expect(fixture.profiles.single.fontBinding, settingsCachedFontBinding);
    expect(fixture.profiles.single.profile.fontFamily, settingsFontFamily);
    expect(fixture.profiles.single.profile.answerback, '下一次非字体保存');
    fixture.emit(
      settingsReply(
        next,
        status: 'applied',
        targetId: settingsProfileA,
        snapshot: settingsSnapshot(
          revision: 9,
          profiles: [
            fixture.profiles.single.profile,
            settingsProfile(settingsProfileB),
          ],
          bindings: {settingsProfileA: settingsCachedFontBinding},
        ),
      ),
    );
    draft = controller.profiles[settingsProfileA]!;
    expect(draft.dirty, isFalse);
  });

  test('TTF pending 换新选择不被旧 Applied 清除，新选择提交时仍按自身身份确认', () async {
    final fixture = DesktopSettingsFixture();
    final controller = fixture.controller();
    addTearDown(() async {
      controller.dispose();
      await fixture.close();
    });
    const hashB =
        'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb';
    const familyB = 'GuoshFont_$hashB';
    const bindingB = ProfileFontBinding(
      version: 1,
      mode: ProfileFontMode.explicitCachedTtf,
      hash: hashB,
    );
    final oldSelection = DesktopTtfSelection(
      family: settingsFontFamily,
      bytes: Uint8List.fromList([0, 1, 2]),
    );
    final newSelection = DesktopTtfSelection(
      family: familyB,
      bytes: Uint8List.fromList([3, 4, 5]),
    );
    controller.editProfile(settingsProfileA, ttf: oldSelection);
    final id = controller.saveProfile(settingsProfileA)!;
    final sent = fixture.ttfs.single.profile;
    controller.editProfile(
      settingsProfileA,
      ttf: newSelection,
      fontBinding: bindingB,
      value: controller.profiles[settingsProfileA]!.value.copyWith(
        fontFamily: familyB,
        answerback: '新选择与新输入',
      ),
    );
    fixture.emit(
      settingsReply(
        id,
        status: 'applied',
        targetId: settingsProfileA,
        snapshot: settingsSnapshot(
          revision: 8,
          profiles: [
            sent.copyWith(fontFamily: settingsFontFamily),
            settingsProfile(settingsProfileB),
          ],
          bindings: {settingsProfileA: settingsCachedFontBinding},
        ),
      ),
    );
    var draft = controller.profiles[settingsProfileA]!;
    expect(identical(draft.ttf, newSelection), isTrue);
    expect(draft.value.fontFamily, familyB);
    expect(draft.fontBinding, bindingB);
    expect(draft.saved.fontFamily, settingsFontFamily);
    expect(draft.savedFontBinding, settingsCachedFontBinding);
    final next = controller.saveProfile(settingsProfileA)!;
    expect(fixture.ttfs, hasLength(2));
    expect(fixture.profiles, isEmpty);
    expect(fixture.binaries, [
      [0, 1, 2],
      [3, 4, 5],
    ]);
    final nextSent = fixture.ttfs.last.profile;
    controller.editProfile(
      settingsProfileA,
      value: draft.value.copyWith(answerback: 'B 保存期间的新输入'),
    );
    fixture.emit(
      settingsReply(
        next,
        status: 'applied',
        targetId: settingsProfileA,
        snapshot: settingsSnapshot(
          revision: 9,
          profiles: [
            nextSent.copyWith(fontFamily: familyB),
            settingsProfile(settingsProfileB),
          ],
          bindings: {settingsProfileA: bindingB},
        ),
      ),
    );
    draft = controller.profiles[settingsProfileA]!;
    expect(draft.ttf, isNull);
    expect(draft.value.answerback, 'B 保存期间的新输入');
    expect(draft.fontBinding, bindingB);
    final finalSave = controller.saveProfile(settingsProfileA)!;
    expect(fixture.ttfs, hasLength(2));
    expect(fixture.profiles.single.profile.answerback, 'B 保存期间的新输入');
    fixture.emit(settingsReply(finalSave, status: 'storage_error'));
  });

  test('已删除目标的资源等待失效，不把预览或迟到结果重新贴到默认 profile', () async {
    final fixture = DesktopSettingsFixture();
    final controller = fixture.controller();
    controller.selectProfile(settingsProfileB);
    final token = controller.beginProfileResource(settingsProfileB);
    final id = controller.deleteProfile(settingsProfileB);
    fixture.emit(
      settingsReply(
        id,
        status: 'applied',
        targetId: settingsProfileB,
        snapshot: settingsSnapshot(
          revision: 8,
          profiles: [settingsProfile(settingsProfileA)],
        ),
      ),
    );
    expect(controller.acceptsProfileResource(token), isFalse);
    expect(controller.selectedProfileId, settingsProfileB);
    expect(controller.selectedProfile, isNull);
    expect(
      () => controller.beginProfileResource(settingsProfileB),
      throwsStateError,
    );
    controller.dispose();
    await fixture.close();
  });

  test('绑定行身份跨查询与 Applied 稳定，显式放弃仍推进编辑版本防止旧核对复用', () async {
    final fixture = DesktopSettingsFixture();
    final controller = fixture.controller();
    final appRow = controller.app!.bindings.single.id;
    final profileRow =
        controller.profiles[settingsProfileA]!.bindings.single.id;
    final query = controller.refresh();
    fixture.emit(settingsReply(query, snapshot: settingsSnapshot()));
    expect(controller.app!.bindings.single.id, appRow);
    expect(
      controller.profiles[settingsProfileA]!.bindings.single.id,
      profileRow,
    );
    controller.editApp(
      value: controller.app!.value.copyWith(colorScheme: 'nord'),
    );
    final id = controller.saveApp();
    fixture.emit(
      settingsReply(
        id,
        status: 'applied',
        snapshot: settingsSnapshot(revision: 8, app: fixture.settings.last.app),
      ),
    );
    expect(controller.app!.bindings.single.id, appRow);
    final version = controller.app!.editVersion;
    controller.reviewApp(
      revision: settingsRevision(8),
      editVersion: version,
      keepDraft: false,
    );
    expect(controller.app!.editVersion, greaterThan(version));
    expect(
      () => controller.reviewApp(
        revision: settingsRevision(8),
        editVersion: version,
        keepDraft: true,
      ),
      throwsStateError,
    );
    final profileVersion = controller.profiles[settingsProfileA]!.editVersion;
    controller.reviewProfile(
      settingsProfileA,
      revision: settingsRevision(8),
      editVersion: profileVersion,
      keepDraft: false,
    );
    expect(
      controller.profiles[settingsProfileA]!.editVersion,
      greaterThan(profileVersion),
    );
    controller.dispose();
    await fixture.close();
  });

  test('同 scope 的另一个 controller 查询推进全局 revision，但不确认本地脏单元', () async {
    final fixture = DesktopSettingsFixture();
    final first = fixture.controller();
    first.editApp(value: first.app!.value.copyWith(colorScheme: 'nord'));
    first.selectProfile(settingsProfileB);
    final second = DesktopSettingsController(gateway: fixture.gateway);
    final query = second.refresh();
    fixture.emit(
      settingsReply(query, snapshot: settingsSnapshot(revision: 12)),
    );
    expect(first.snapshot!.revision, settingsRevision(12));
    expect(first.selectedProfileId, settingsProfileB);
    expect(first.app!.baseRevision, settingsRevision(7));
    expect(first.app!.value.colorScheme, 'nord');
    expect(first.stateFor('app').state, DesktopSettingsWriteState.idle);
    expect(() => first.saveApp(), throwsStateError);
    first.dispose();
    second.dispose();
    await fixture.close();
  });

  test('晚到 Applied 不降低先接受的外部 revision，不覆盖保存期间的新输入', () async {
    final fixture = DesktopSettingsFixture();
    final controller = fixture.controller();
    controller.editApp(
      value: controller.app!.value.copyWith(colorScheme: 'nord'),
    );
    final id = controller.saveApp();
    final submitted = fixture.settings.last.app!;
    controller.editApp(
      value: controller.app!.value.copyWith(colorScheme: 'dracula'),
    );
    final query = controller.refresh();
    fixture.emit(
      settingsReply(query, snapshot: settingsSnapshot(revision: 12)),
    );
    fixture.emit(
      settingsReply(
        id,
        status: 'applied',
        snapshot: settingsSnapshot(revision: 8, app: submitted),
      ),
    );
    expect(controller.snapshot!.revision, settingsRevision(12));
    expect(fixture.gateway.snapshot!.revision, settingsRevision(12));
    expect(controller.app!.value.colorScheme, 'dracula');
    expect(controller.app!.saved.colorScheme, 'nord');
    expect(controller.app!.baseRevision, settingsRevision(8));
    expect(controller.needsReview('app'), isTrue);
    expect(() => controller.saveApp(), throwsStateError);
    controller.dispose();
    await fixture.close();
  });

  test('应用三个字段完整保存，全部 capabilities 配色／action 使用实际协议值', () async {
    final fixture = DesktopSettingsFixture();
    final controller = fixture.controller();
    var revision = 7;
    for (final color in settingsCapabilities.colorSchemes) {
      final bindings = [
        for (
          var index = 0;
          index < settingsCapabilities.keyActions.length;
          index++
        )
          settingsBinding.copyWith(
            key: 'F${index + 1}',
            action: settingsCapabilities.keyActions[index],
          ),
      ];
      controller.editApp(
        value: controller.app!.value.copyWith(
          defaultTerminalProfile: settingsProfileB,
          colorScheme: color,
          keyBindings: bindings,
        ),
      );
      final id = controller.saveApp();
      final sent = fixture.settings.last;
      expect(sent.operation, 'save_app');
      expect(sent.requestId, id);
      expect(sent.baseRevision, settingsRevision(revision));
      expect(sent.targetId, '');
      expect(sent.profile, isNull);
      expect(sent.app, controller.app!.value);
      expect(
        sent.app!.keyBindings.map((binding) => binding.action),
        settingsCapabilities.keyActions,
      );
      fixture.emit(
        settingsReply(
          id,
          status: 'applied',
          snapshot: settingsSnapshot(revision: ++revision, app: sent.app),
        ),
      );
      expect(controller.app!.dirty, isFalse);
      expect(controller.snapshot!.app, sent.app);
      expect(
        controller.stateFor('app').state,
        DesktopSettingsWriteState.applied,
      );
    }
    controller.dispose();
    await fixture.close();
  });

  test('profile 身份、16 字段与明确字体模式一次完整 CAS，数字支持小数', () async {
    final fixture = DesktopSettingsFixture();
    final controller = fixture.controller();
    final value = controller.profiles[settingsProfileA]!.value.copyWith(
      name: '完整配置',
      terminalType: 'vt220',
      initialCols: 117,
      initialRows: 41,
      scrollbackLines: 654321,
      fontFamily: 'monospace',
      fontSize: 23.25,
      colorScheme: 'tokyo_night',
      keyBindings: [
        settingsBinding.copyWith(
          key: '界',
          shift: false,
          control: true,
          alt: false,
          superKey: true,
          action: 'send:\u0008',
        ),
      ],
      leftAltAsMeta: false,
      rightAltAsMeta: true,
      enableCsiU: true,
      enableKittyKeyboard: true,
      mouseReporting: false,
      scrollOnOutput: false,
      scrollOnKeypress: true,
      answerback: '仅数据：精确回答',
    );
    const binding = ProfileFontBinding(
      version: 1,
      mode: ProfileFontMode.explicitFamily,
    );
    controller.editProfile(
      settingsProfileA,
      value: value,
      fontBinding: binding,
    );
    final id = controller.saveProfile(settingsProfileA)!;
    final sent = fixture.profiles.single;
    expect(sent.requestId, id);
    expect(sent.protocolVersion, 1);
    expect(sent.baseRevision, settingsRevision(7));
    expect(sent.targetId, settingsProfileA);
    expect(sent.profile, value);
    expect(sent.fontBinding, binding);
    fixture.emit(
      settingsReply(
        id,
        status: 'applied',
        targetId: settingsProfileA,
        snapshot: settingsSnapshot(
          revision: 8,
          profiles: [sent.profile, settingsProfile(settingsProfileB)],
          bindings: {settingsProfileA: binding},
        ),
      ),
    );
    expect(controller.profiles[settingsProfileA]!.dirty, isFalse);
    expect(controller.profiles[settingsProfileA]!.saved, value);
    controller.dispose();
    await fixture.close();
  });

  test('切 profile／原始数字中间态不隐式保存或放弃，Rust 仍校验 chord/action', () async {
    final fixture = DesktopSettingsFixture();
    final controller = fixture.controller();
    final a = controller.profiles[settingsProfileA]!;
    controller.editProfile(
      settingsProfileA,
      numbers: a.numbers.copyWith(initialCols: ''),
      value: a.value.copyWith(
        keyBindings: [
          settingsBinding.copyWith(key: 'NotAChord', action: 'unknown_action'),
        ],
      ),
    );
    controller.selectProfile(settingsProfileB);
    controller.editProfile(
      settingsProfileB,
      value: controller.selectedProfile!.value.copyWith(answerback: '独立 B'),
    );
    controller.selectProfile(settingsProfileA);
    expect(controller.selectedProfile!.numbers.initialCols, '');
    expect(controller.profiles[settingsProfileB]!.value.answerback, '独立 B');
    expect(controller.saveProfile(settingsProfileA), isNull);
    expect(fixture.profiles, isEmpty);
    expect(
      controller.stateFor(settingsProfileA).errors.byField,
      contains('initial_cols'),
    );
    controller.editProfile(
      settingsProfileA,
      numbers: controller.selectedProfile!.numbers.copyWith(initialCols: '96'),
    );
    final id = controller.saveProfile(settingsProfileA)!;
    expect(fixture.profiles.single.profile.keyBindings.single.key, 'NotAChord');
    expect(
      fixture.profiles.single.profile.keyBindings.single.action,
      'unknown_action',
    );
    fixture.emit(
      settingsReply(
        id,
        status: 'validation_error',
        revision: settingsRevision(7),
        errors: [
          const DesktopSettingsFieldError(
            field: 'key_bindings.code',
            code: 'invalid_chord',
          ),
          const DesktopSettingsFieldError(
            field: 'future_field',
            code: 'future_error',
          ),
        ],
      ),
    );
    expect(
      controller.stateFor(settingsProfileA).errors.byField,
      contains('key_bindings.code'),
    );
    expect(
      controller.stateFor(settingsProfileA).errors.pageErrors.single.field,
      'future_field',
    );
    expect(controller.profiles[settingsProfileA]!.dirty, isTrue);
    controller.dispose();
    await fixture.close();
  });

  test('写入串行；其他脏单元保原基线，必须显式核对后使用新全局 revision', () async {
    final fixture = DesktopSettingsFixture();
    final controller = fixture.controller();
    controller.editApp(
      value: controller.app!.value.copyWith(colorScheme: 'nord'),
    );
    controller.editProfile(
      settingsProfileA,
      value: controller.profiles[settingsProfileA]!.value.copyWith(
        answerback: 'A 草稿',
      ),
    );
    controller.editProfile(
      settingsProfileB,
      value: controller.profiles[settingsProfileB]!.value.copyWith(
        answerback: 'B 草稿',
      ),
    );
    final id = controller.saveApp();
    expect(() => controller.saveProfile(settingsProfileA), throwsStateError);
    final saved = settingsSnapshot(revision: 8, app: fixture.settings.last.app);
    fixture.emit(settingsReply(id, status: 'applied', snapshot: saved));
    expect(
      controller.profiles[settingsProfileA]!.baseRevision,
      settingsRevision(7),
    );
    expect(
      controller.profiles[settingsProfileB]!.baseRevision,
      settingsRevision(7),
    );
    expect(controller.needsReview(settingsProfileA), isTrue);
    expect(() => controller.saveProfile(settingsProfileA), throwsStateError);
    final version = controller.profiles[settingsProfileA]!.editVersion;
    expect(
      () => controller.reviewProfile(
        settingsProfileA,
        revision: settingsRevision(7),
        editVersion: version,
        keepDraft: true,
      ),
      throwsStateError,
    );
    controller.reviewProfile(
      settingsProfileA,
      revision: saved.revision,
      editVersion: version,
      keepDraft: true,
    );
    final profileId = controller.saveProfile(settingsProfileA)!;
    expect(fixture.profiles.single.baseRevision, settingsRevision(8));
    expect(fixture.profiles.single.profile.answerback, 'A 草稿');
    fixture.emit(settingsReply(profileId, status: 'storage_error'));
    expect(controller.profiles[settingsProfileA]!.dirty, isTrue);
    expect(controller.profiles[settingsProfileB]!.value.answerback, 'B 草稿');
    controller.dispose();
    await fixture.close();
  });

  for (final unit in ['app', settingsProfileA]) {
    test('$unit 保存期间继续编辑不被 Applied 覆盖，响应也不串当前目标', () async {
      final fixture = DesktopSettingsFixture();
      final controller = fixture.controller();
      late String id;
      late DesktopConfiguration saved;
      if (unit == 'app') {
        controller.editApp(
          value: controller.app!.value.copyWith(colorScheme: 'nord'),
        );
        id = controller.saveApp();
        saved = settingsSnapshot(revision: 8, app: fixture.settings.last.app);
        controller.editApp(
          value: controller.app!.value.copyWith(colorScheme: 'dracula'),
        );
      } else {
        controller.editProfile(
          unit,
          value: controller.profiles[unit]!.value.copyWith(answerback: '已发送'),
        );
        id = controller.saveProfile(unit)!;
        saved = settingsSnapshot(
          revision: 8,
          profiles: [
            fixture.profiles.single.profile,
            settingsProfile(settingsProfileB),
          ],
        );
        controller.editProfile(
          unit,
          value: controller.profiles[unit]!.value.copyWith(answerback: '新输入'),
          numbers: controller.profiles[unit]!.numbers.copyWith(fontSize: ''),
        );
      }
      controller.selectProfile(settingsProfileB);
      fixture.emit(
        settingsReply(
          id,
          status: 'applied',
          targetId: unit == 'app' ? '' : unit,
          snapshot: saved,
        ),
      );
      expect(controller.selectedProfileId, settingsProfileB);
      if (unit == 'app') {
        expect(controller.app!.value.colorScheme, 'dracula');
        expect(controller.app!.saved.colorScheme, 'nord');
        expect(controller.app!.dirty, isTrue);
        expect(controller.app!.baseRevision, settingsRevision(8));
      } else {
        expect(controller.profiles[unit]!.value.answerback, '新输入');
        expect(controller.profiles[unit]!.saved.answerback, '已发送');
        expect(controller.profiles[unit]!.numbers.fontSize, '');
        expect(controller.profiles[unit]!.dirty, isTrue);
        expect(controller.profiles[unit]!.baseRevision, settingsRevision(8));
      }
      controller.dispose();
      await fixture.close();
    });
  }

  test('conflict 保留请求上下文，必须新 query 和显式核对，不自动重放外部修改', () async {
    final fixture = DesktopSettingsFixture();
    final controller = fixture.controller();
    controller.editApp(
      value: controller.app!.value.copyWith(colorScheme: 'nord'),
    );
    final id = controller.saveApp();
    fixture.emit(
      settingsReply(id, status: 'conflict', revision: settingsRevision(12)),
    );
    expect(
      controller.stateFor('app').pending!.command.baseRevision,
      settingsRevision(7),
    );
    expect(controller.needsReview('app'), isTrue);
    expect(() => controller.saveApp(), throwsStateError);
    expect(
      () => controller.reviewApp(
        revision: settingsRevision(7),
        editVersion: controller.app!.editVersion,
        keepDraft: true,
      ),
      throwsStateError,
    );
    final external = settingsSnapshot(
      revision: 12,
      app: settingsSnapshot().app.copyWith(
        defaultTerminalProfile: settingsProfileB,
        colorScheme: 'dracula',
      ),
    );
    final query = controller.refresh();
    fixture.emit(settingsReply(query, snapshot: external));
    expect(controller.app!.value.colorScheme, 'nord');
    expect(controller.app!.saved.colorScheme, 'default');
    expect(controller.app!.baseRevision, settingsRevision(7));
    expect(
      fixture.settings.where((request) => request.operation == 'save_app'),
      hasLength(1),
    );
    controller.reviewApp(
      revision: external.revision,
      editVersion: controller.app!.editVersion,
      keepDraft: false,
    );
    expect(controller.app!.value, external.app);
    expect(controller.app!.dirty, isFalse);
    controller.editApp(
      value: controller.app!.value.copyWith(colorScheme: 'nord'),
    );
    final retry = controller.saveApp();
    expect(fixture.settings.last.baseRevision, settingsRevision(12));
    expect(fixture.settings.last.app!.defaultTerminalProfile, settingsProfileB);
    fixture.emit(settingsReply(retry, status: 'storage_error'));
    controller.dispose();
    await fixture.close();
  });

  for (final operation in [
    'create_profile',
    'copy_profile',
    'rename_profile',
    'delete_profile',
  ]) {
    test('$operation 使用准确请求及 Rust 返回的 ID，不改变用户当前选择', () async {
      final fixture = DesktopSettingsFixture();
      final controller = fixture.controller();
      final id = switch (operation) {
        'create_profile' => controller.createProfile('新名字'),
        'copy_profile' => controller.copyProfile(settingsProfileA, '新名字'),
        'rename_profile' => controller.renameProfile(settingsProfileB, '新名字'),
        _ => controller.deleteProfile(settingsProfileB),
      };
      final request = fixture.settings.last;
      expect(request.operation, operation);
      expect(request.baseRevision, settingsRevision(7));
      expect(request.app, isNull);
      expect(request.profile, isNull);
      final isNew =
          operation == 'create_profile' || operation == 'copy_profile';
      final targetId = isNew ? settingsProfileC : settingsProfileB;
      final profiles = [
        settingsProfile(settingsProfileA),
        if (operation != 'delete_profile')
          settingsProfile(
            settingsProfileB,
            name: operation == 'rename_profile' ? '新名字' : '同名',
          ),
        if (isNew) settingsProfile(settingsProfileC, name: '新名字'),
      ];
      fixture.emit(
        settingsReply(
          id,
          status: 'applied',
          targetId: targetId,
          snapshot: settingsSnapshot(revision: 8, profiles: profiles),
        ),
      );
      expect(controller.selectedProfileId, settingsProfileA);
      expect(controller.stateFor('lifecycle').result!.targetId, targetId);
      expect(
        controller.profiles.containsKey(targetId),
        operation != 'delete_profile',
      );
      controller.dispose();
      await fixture.close();
    });
  }

  test('有草稿的复制须先显式保存／放弃；删除引用拒绝保留请求及草稿', () async {
    final fixture = DesktopSettingsFixture();
    final controller = fixture.controller();
    controller.editProfile(
      settingsProfileB,
      value: controller.profiles[settingsProfileB]!.value.copyWith(
        name: '草稿名字',
      ),
    );
    expect(
      () => controller.copyProfile(settingsProfileB, '复制'),
      throwsStateError,
    );
    controller.reviewProfile(
      settingsProfileB,
      revision: settingsRevision(7),
      editVersion: controller.profiles[settingsProfileB]!.editVersion,
      keepDraft: false,
    );
    final id = controller.deleteProfile(settingsProfileB);
    fixture.emit(
      settingsReply(
        id,
        status: 'validation_error',
        revision: settingsRevision(7),
        errors: [
          const DesktopSettingsFieldError(
            field: 'target_id',
            code: 'profile_in_use',
          ),
        ],
      ),
    );
    expect(controller.profiles.containsKey(settingsProfileB), isTrue);
    expect(
      controller.stateFor('lifecycle').errors.byField['target_id']!.single.code,
      'profile_in_use',
    );
    expect(
      controller.stateFor('lifecycle').pending!.command.targetId,
      settingsProfileB,
    );
    controller.dispose();
    await fixture.close();
  });

  test('Uneditable 的非字体字段及 Legacy 模式通过实际 profile 请求保全', () async {
    final fixture = DesktopSettingsFixture();
    const uneditable = ProfileFontBinding(
      version: 99,
      mode: ProfileFontMode.uneditable,
      uneditableReason: ProfileFontMetadataError.unsupportedVersion,
    );
    final controller = fixture.controller(
      snapshot: settingsSnapshot(bindings: {settingsProfileA: uneditable}),
    );
    for (final profile in [settingsProfileA, settingsProfileB]) {
      controller.editProfile(
        profile,
        value: controller.profiles[profile]!.value.copyWith(
          fontSize: 29.5,
          enableCsiU: true,
        ),
      );
      final id = controller.saveProfile(profile)!;
      final sent = fixture.profiles.last;
      expect(
        sent.fontBinding,
        profile == settingsProfileA ? uneditable : settingsFontBinding,
      );
      expect(sent.profile.fontFamily, 'MesloLGS NF');
      expect(sent.profile.fontSize, 29.5);
      fixture.emit(
        settingsReply(
          id,
          status: 'applied',
          targetId: profile,
          snapshot: settingsSnapshot(
            revision: controller.snapshot!.revision.toInt() + 1,
            profiles: [
              for (final stored in controller.snapshot!.profiles)
                stored.id == profile ? sent.profile : stored,
            ],
            bindings: {settingsProfileA: uneditable},
          ),
        ),
      );
      expect(controller.profiles[profile]!.dirty, isFalse);
      expect(controller.profiles[profile]!.savedFontBinding, sent.fontBinding);
    }
    controller.dispose();
    await fixture.close();
  });

  test('TTF 完整字段与二进制同 CAS，profile binding 请求和全局槽不混用', () async {
    final fixture = DesktopSettingsFixture();
    final controller = fixture.controller();
    final bytes = Uint8List.fromList([0, 1, 2, 3]);
    controller.editProfile(
      settingsProfileA,
      value: controller.profiles[settingsProfileA]!.value.copyWith(
        answerback: 'TTF 与字段',
      ),
      ttf: DesktopTtfSelection(family: settingsFontFamily, bytes: bytes),
      numbers: controller.profiles[settingsProfileA]!.numbers.copyWith(
        fontSize: '26.25',
      ),
    );
    final id = controller.saveProfile(settingsProfileA)!;
    bytes[0] = 99;
    expect(fixture.profiles, isEmpty);
    expect(fixture.ttfs.single.baseRevision, settingsRevision(7));
    expect(fixture.ttfs.single.targetId, settingsProfileA);
    expect(fixture.ttfs.single.profile.answerback, 'TTF 与字段');
    expect(fixture.ttfs.single.profile.fontSize, 26.25);
    expect(fixture.binaries.single, [0, 1, 2, 3]);
    const binding = settingsCachedFontBinding;
    fixture.emit(
      settingsReply(
        id,
        status: 'applied',
        targetId: settingsProfileA,
        snapshot: settingsSnapshot(
          revision: 8,
          profiles: [
            fixture.ttfs.single.profile.copyWith(
              fontFamily: settingsFontFamily,
            ),
            settingsProfile(settingsProfileB),
          ],
          bindings: {settingsProfileA: binding},
        ),
      ),
    );
    expect(controller.profiles[settingsProfileA]!.dirty, isFalse);
    expect(controller.profiles[settingsProfileA]!.ttf, isNull);
    expect(controller.profiles[settingsProfileA]!.savedFontBinding, binding);
    controller.dispose();
    await fixture.close();
  });

  for (final channel in ['settings', 'profile', 'ttf']) {
    test('$channel 未决写跨关闭重开：先返回 query 不确认，不重复发送，不串新草稿', () async {
      final fixture = DesktopSettingsFixture();
      final old = fixture.controller();
      late String id;
      late DesktopConfiguration saved;
      if (channel == 'settings') {
        id = old.copyProfile(settingsProfileA, '保留来源名字');
        saved = settingsSnapshot(
          revision: 8,
          profiles: [
            settingsProfile(settingsProfileA),
            settingsProfile(settingsProfileB),
            settingsProfile(settingsProfileC, name: '保留来源名字'),
          ],
        );
      } else {
        old.editProfile(
          settingsProfileA,
          value: old.profiles[settingsProfileA]!.value.copyWith(
            answerback: '旧来源',
          ),
          ttf: channel == 'ttf'
              ? DesktopTtfSelection(
                  family: settingsFontFamily,
                  bytes: Uint8List.fromList([0, 1]),
                )
              : null,
        );
        id = old.saveProfile(settingsProfileA)!;
        final profile = channel == 'ttf'
            ? fixture.ttfs.single.profile
            : fixture.profiles.single.profile;
        saved = settingsSnapshot(
          revision: 8,
          profiles: [
            channel == 'ttf'
                ? profile.copyWith(fontFamily: settingsFontFamily)
                : profile,
            settingsProfile(settingsProfileB),
          ],
          bindings: channel == 'ttf'
              ? {settingsProfileA: settingsCachedFontBinding}
              : const {},
        );
      }
      final oldOwner = old.ownerId;
      old.dispose();
      final reopened = fixture.controller();
      expect(fixture.gateway.blockingWrite!.context.ownerId, oldOwner);
      expect(
        fixture.gateway.blockingWrite!.phase,
        DesktopSettingsPendingPhase.uncertain,
      );
      reopened.editProfile(
        settingsProfileB,
        value: reopened.profiles[settingsProfileB]!.value.copyWith(
          answerback: '新窗口草稿',
        ),
      );
      expect(
        () => reopened.copyProfile(settingsProfileA, '不可重复'),
        throwsStateError,
      );
      expect(() => reopened.saveProfile(settingsProfileB), throwsStateError);
      expect(
        () => reopened.confirmUncertainWrite(id, settingsRevision(7)),
        throwsStateError,
      );
      final earlyQuery = reopened.refresh();
      fixture.emit(
        settingsReply(
          id,
          status: 'applied',
          targetId: channel == 'settings' ? settingsProfileC : settingsProfileA,
          snapshot: saved,
        ),
      );
      fixture.emit(settingsReply(earlyQuery, snapshot: saved));
      expect(
        fixture.gateway.blockingWrite!.phase,
        DesktopSettingsPendingPhase.needsConfirmation,
      );
      expect(fixture.gateway.verificationFor(id), isNull);
      expect(
        reopened.stateFor('lifecycle').state,
        DesktopSettingsWriteState.idle,
      );
      expect(reopened.profiles[settingsProfileB]!.value.answerback, '新窗口草稿');
      expect(
        reopened.profiles[settingsProfileB]!.baseRevision,
        settingsRevision(7),
      );
      expect(reopened.selectedProfileId, settingsProfileA);
      final verification = reopened.refresh();
      fixture.emit(settingsReply(verification, snapshot: saved));
      expect(fixture.gateway.blockingWrite, isNotNull);
      reopened.confirmUncertainWrite(id, saved.revision);
      expect(fixture.gateway.blockingWrite, isNull);
      expect(
        fixture.settings.where(
          (request) => request.operation == 'copy_profile',
        ),
        hasLength(channel == 'settings' ? 1 : 0),
      );
      expect(fixture.profiles, hasLength(channel == 'profile' ? 1 : 0));
      expect(fixture.ttfs, hasLength(channel == 'ttf' ? 1 : 0));
      reopened.dispose();
      await fixture.close();
    });
  }

  test('超时的生命周期输入持久归属，query 不解除隔离，未决 gateway 不可销毁', () async {
    final fixture = DesktopSettingsFixture();
    final controller = fixture.controller();
    final id = controller.createProfile('timeout 输入');
    await Future<void>.delayed(const Duration(milliseconds: 41));
    expect(
      controller.stateFor('lifecycle').state,
      DesktopSettingsWriteState.uncertain,
    );
    expect(
      (fixture.gateway.blockingWrite!.command as DesktopConfigurationCommand)
          .request
          .name,
      'timeout 输入',
    );
    final query = controller.refresh();
    fixture.emit(settingsReply(query, snapshot: settingsSnapshot()));
    expect(
      fixture.gateway.blockingWrite!.phase,
      DesktopSettingsPendingPhase.uncertain,
    );
    await expectLater(fixture.gateway.close(), throwsStateError);
    expect(() => controller.createProfile('timeout 输入'), throwsStateError);
    fixture.emit(settingsReply(id, status: 'storage_error'));
    final verification = controller.refresh();
    fixture.emit(settingsReply(verification, snapshot: settingsSnapshot()));
    controller.confirmUncertainWrite(id, settingsRevision(7));
    controller.dispose();
    await fixture.close();
  });

  test(
    'query／资源 generation 失效不造 backend cancel；RID 跨 controller 永不复用',
    () async {
      final fixture = DesktopSettingsFixture();
      final controller = fixture.controller();
      final first = controller.refresh();
      final second = controller.refresh();
      fixture.emit(
        settingsReply(first, snapshot: settingsSnapshot(revision: 99)),
      );
      fixture.emit(
        settingsReply(second, snapshot: settingsSnapshot(revision: 8)),
      );
      expect(controller.snapshot!.revision, settingsRevision(8));
      final preview = controller.beginProfileResource(settingsProfileA);
      expect(controller.acceptsProfileResource(preview), isTrue);
      controller.selectProfile(settingsProfileB);
      expect(controller.acceptsProfileResource(preview), isFalse);
      final replacement = controller.beginProfileResource(settingsProfileB);
      controller.cancelProfileResource();
      expect(controller.acceptsProfileResource(replacement), isFalse);
      final closeToken = controller.beginProfileResource(settingsProfileB);
      var notifications = 0;
      controller.addListener(() => notifications++);
      final last = controller.refresh();
      controller.dispose();
      final before = notifications;
      fixture.emit(
        settingsReply(last, snapshot: settingsSnapshot(revision: 100)),
      );
      expect(notifications, before);
      expect(controller.acceptsProfileResource(closeToken), isFalse);
      final reopened = fixture.controller(
        snapshot: settingsSnapshot(revision: 8),
      );
      final ids = fixture.settings.map((request) => request.requestId).toList();
      expect(ids.toSet(), hasLength(ids.length));
      expect(reopened.ownerId, isNot(controller.ownerId));
      reopened.dispose();
      await fixture.close();
    },
  );
}
