import 'dart:typed_data';

import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/desktop/settings/desktop_settings_draft.dart';

import 'desktop_settings_gateway_test.dart';

void main() {
  test('完整快照与能力拥有不可变列表副本，不与发起者的嵌套列表别名共享', () {
    final bindings = [settingsBinding];
    final profiles = [
      settingsProfile(settingsProfileA).copyWith(keyBindings: bindings),
    ];
    final original = settingsSnapshot(
      profiles: profiles,
      app: settingsSnapshot().app.copyWith(keyBindings: bindings),
    );
    final saved = copyDesktopConfiguration(original);
    bindings.clear();
    profiles.clear();
    expect(saved.app.keyBindings, [settingsBinding]);
    expect(saved.profiles.single.keyBindings, [settingsBinding]);
    expect(saved.effectiveProfiles.single.keyBindings, [settingsBinding]);
    expect(() => saved.profiles.clear(), throwsUnsupportedError);
    expect(() => saved.app.keyBindings.clear(), throwsUnsupportedError);
    expect(
      () => saved.effectiveProfiles.single.keyBindings.clear(),
      throwsUnsupportedError,
    );
    final colors = ['default'];
    final capabilities = copyDesktopCapabilities(
      settingsCapabilities.copyWith(colorSchemes: colors),
    );
    colors.clear();
    expect(capabilities.colorSchemes, ['default']);
    expect(() => capabilities.colorSchemes.clear(), throwsUnsupportedError);
  });

  test('数字中间态保留原文，辅助校验不回填默认；领域范围仍交 Rust', () {
    final draft = DesktopProfileDraft.fromSaved(
      settingsProfile(settingsProfileA),
      settingsFontBinding,
      settingsRevision(7),
    );
    final edited = draft.edit(
      numbers: draft.numbers.copyWith(
        initialCols: '',
        initialRows: 'abc',
        fontSize: 'NaN',
      ),
    );
    expect(edited.dirty, isTrue);
    expect(edited.numbers.initialCols, '');
    expect(edited.numbers.initialRows, 'abc');
    expect(edited.numbers.errors.map((error) => error.field), [
      'initial_cols',
      'initial_rows',
      'font_size',
    ]);
    expect(edited.value.initialCols, 80);
    final encodable = edited.edit(
      numbers: edited.numbers.copyWith(
        initialCols: '1000',
        initialRows: '0',
        fontSize: '23.25',
      ),
    );
    expect(encodable.numbers.errors, isEmpty);
    expect(encodable.numbers.apply(encodable.value).initialCols, 1000);
    expect(encodable.numbers.apply(encodable.value).initialRows, 0);
    expect(encodable.numbers.apply(encodable.value).fontSize, 23.25);
  });

  test('普通字段／绑定重排保留重复行的独立稳定身份与控制字符 action', () {
    final draft = DesktopAppDraft.fromSaved(
      settingsSnapshot().app.copyWith(
        keyBindings: [settingsBinding, settingsBinding],
      ),
      settingsRevision(7),
    );
    expect(draft.bindings[0].id, isNot(draft.bindings[1].id));
    final edited = draft.edit(value: draft.value.copyWith(colorScheme: 'nord'));
    expect(
      edited.bindings.map((row) => row.id),
      draft.bindings.map((row) => row.id),
    );
    final reordered = edited.edit(
      bindings: [
        edited.bindings[1],
        edited.bindings[0].edit(
          settingsBinding.copyWith(action: 'send:\u007f'),
        ),
      ],
    );
    expect(reordered.bindings.first.id, edited.bindings[1].id);
    expect(reordered.value.keyBindings.last.action.codeUnits, [
      115,
      101,
      110,
      100,
      58,
      127,
    ]);
    expect(
      settingsCapabilities.keyActions
          .take(3)
          .map((action) => action.substring(5).codeUnits),
      [
        [27, 91, 51, 126],
        [127],
        [8],
      ],
    );
  });

  test('Legacy 保全不是新选择；Uneditable 只禁止 family／绑定变化，不禁止其他字段', () {
    final legacy = DesktopProfileDraft.fromSaved(
      settingsProfile(settingsProfileA),
      settingsFontBinding,
      settingsRevision(7),
    );
    expect(
      legacy.edit(value: legacy.value.copyWith(answerback: '新回答')).fontBinding,
      settingsFontBinding,
    );
    expect(
      DesktopProfileDraft.newFontModes,
      isNot(contains(ProfileFontMode.legacyUndetermined)),
    );
    final explicit = legacy.edit(
      fontBinding: const ProfileFontBinding(
        version: 1,
        mode: ProfileFontMode.explicitFamily,
      ),
    );
    expect(
      () => explicit.edit(fontBinding: settingsFontBinding),
      throwsStateError,
    );
    const uneditable = ProfileFontBinding(
      version: 99,
      mode: ProfileFontMode.uneditable,
      uneditableReason: ProfileFontMetadataError.unsupportedVersion,
    );
    final preserved = DesktopProfileDraft.fromSaved(
      settingsProfile(settingsProfileA),
      uneditable,
      settingsRevision(7),
    );
    final edited = preserved.edit(
      value: preserved.value.copyWith(
        fontSize: 24.5,
        colorScheme: 'dracula',
        leftAltAsMeta: false,
      ),
    );
    expect(edited.fontBinding, uneditable);
    expect(edited.numbers.fontSize, '24.5');
    expect(edited.value.leftAltAsMeta, isFalse);
    expect(
      () => preserved.edit(
        value: preserved.value.copyWith(fontFamily: 'different'),
      ),
      throwsStateError,
    );
    expect(
      () => preserved.edit(fontBinding: settingsFontBinding),
      throwsStateError,
    );
  });

  test('未知字段保留页级，索引绑定错误按请求行身份而非当前选择映射', () {
    final rows = desktopBindingRows([settingsBinding, settingsBinding]);
    final errors = DesktopDraftErrors([
      const DesktopSettingsFieldError(field: 'font_size', code: 'out_of_range'),
      const DesktopSettingsFieldError(
        field: 'key_bindings[1].action',
        code: 'invalid_action',
      ),
      const DesktopSettingsFieldError(
        field: 'key_bindings.code',
        code: 'invalid_chord',
      ),
      const DesktopSettingsFieldError(
        field: 'future_field',
        code: 'future_error',
      ),
      const DesktopSettingsFieldError(
        field: 'key_bindings[8].action',
        code: 'invalid_action',
      ),
    ], bindings: rows);
    expect(
      errors.byField.keys,
      containsAll(['font_size', 'key_bindings.code']),
    );
    expect(errors.byBindingRow.keys, [rows[1].id]);
    expect(errors.pageErrors.map((error) => error.field), [
      'future_field',
      'key_bindings[8].action',
    ]);
    expect(() => errors.pageErrors.clear(), throwsUnsupportedError);
  });

  test('TTF 选择拥有二进制副本；preview 本身不修改 saved 基线', () {
    final bytes = Uint8List.fromList([0, 1, 2, 3]);
    final selection = DesktopTtfSelection(
      family: 'GuoshFont_fixture',
      bytes: bytes,
    );
    bytes[0] = 99;
    expect(selection.bytes, [0, 1, 2, 3]);
    expect(() => selection.bytes[0] = 5, throwsUnsupportedError);
    final draft = DesktopProfileDraft.fromSaved(
      settingsProfile(settingsProfileA),
      settingsFontBinding,
      settingsRevision(7),
    );
    final edited = draft.edit(ttf: selection);
    expect(edited.dirty, isTrue);
    expect(edited.saved, draft.saved);
    expect(edited.savedFontBinding, settingsFontBinding);
    expect(edited.edit(clearTtf: true).dirty, isFalse);
  });
}
