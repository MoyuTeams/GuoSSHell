import 'dart:async';

import 'package:fluent_ui/fluent_ui.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/desktop/desktop_theme.dart';
import 'package:guosh_shell/src/desktop/settings/desktop_settings_controller.dart';
import 'package:guosh_shell/src/desktop/settings/desktop_settings_draft.dart';
import 'package:guosh_shell/src/desktop/settings/forms/desktop_profile_form.dart';
import 'package:guosh_shell/src/desktop/settings/forms/desktop_settings_bindings_editor.dart';
import 'package:guosh_shell/src/desktop/settings/forms/desktop_settings_forms.dart';
import 'package:guosh_shell/src/desktop/settings/forms/settings_form_controls.dart';

import 'desktop_settings_bindings_editor_test.dart' show bindingChoice;
import 'desktop_settings_gateway_test.dart';

Finder textField(String label) => find.descendant(
  of: find.byWidgetPredicate((w) => w is SettingsTextField && w.label == label),
  matching: find.byType(TextBox),
);

Future<void> formTap(
  WidgetTester tester,
  String key, {
  bool settle = true,
}) async {
  final finder = find.byKey(ValueKey(key));
  await tester.ensureVisible(finder);
  await tester.tap(finder);
  if (settle) {
    await tester.pumpAndSettle();
  } else {
    await tester.pump();
  }
}

Future<void> formEdit(WidgetTester tester, String label, String value) async {
  final finder = textField(label);
  await tester.ensureVisible(finder);
  await tester.enterText(finder, value);
  await tester.pump();
}

Future<void> mountForms(
  WidgetTester tester,
  DesktopSettingsController controller, {
  double width = 1280,
  double scale = 1,
  bool highContrast = false,
  bool reduceMotion = true,
  VoidCallback? onClose,
  DesktopProfileTtfPicker? picker,
  DesktopSettingsFocusBoundary? focus,
  VoidCallback? shellChord,
  GlobalKey<DesktopSettingsFormsState>? formsKey,
}) async {
  tester.view.devicePixelRatio = 1;
  tester.view.physicalSize = Size(width, 900);
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  final key = formsKey ?? GlobalKey<DesktopSettingsFormsState>();
  var section = DesktopSettingsFormSection.app;
  // 分区导航与关闭由父设置页持有；测试壳以同名 key 复现这些父级入口。
  await tester.pumpWidget(
    CallbackShortcuts(
      bindings: {
        const SingleActivator(LogicalKeyboardKey.keyK, control: true):
            shellChord ?? () {},
        const SingleActivator(LogicalKeyboardKey.equal, control: true):
            shellChord ?? () {},
      },
      child: FluentApp(
        theme: desktopTheme(
          highContrast: highContrast,
          reduceMotion: reduceMotion,
        ),
        home: MediaQuery(
          data: MediaQueryData(
            size: Size(width, 900),
            textScaler: TextScaler.linear(scale),
            highContrast: highContrast,
            disableAnimations: reduceMotion,
            accessibleNavigation: reduceMotion,
          ),
          child: CallbackShortcuts(
            bindings: {
              const SingleActivator(LogicalKeyboardKey.keyK, control: true):
                  shellChord ?? () {},
              const SingleActivator(LogicalKeyboardKey.equal, control: true):
                  shellChord ?? () {},
            },
            child: ScaffoldPage(
              content: StatefulBuilder(
                builder: (context, update) => Column(
                  children: [
                    Wrap(
                      children: [
                        Button(
                          key: const ValueKey('section-app'),
                          onPressed: () => update(
                            () => section = DesktopSettingsFormSection.app,
                          ),
                          child: const Text('应用'),
                        ),
                        Button(
                          key: const ValueKey('section-profile'),
                          onPressed: () => update(
                            () => section = DesktopSettingsFormSection.profile,
                          ),
                          child: const Text('配置文件'),
                        ),
                        Button(
                          key: const ValueKey('close-settings'),
                          onPressed: () => key.currentState!.requestClose(),
                          child: const Text('关闭设置'),
                        ),
                      ],
                    ),
                    Expanded(
                      child: DesktopSettingsForms(
                        key: key,
                        controller: controller,
                        onPickTtf: picker ?? (_, _) async => null,
                        onClose: onClose ?? () {},
                        section: section,
                        onShowApp: () => update(
                          () => section = DesktopSettingsFormSection.app,
                        ),
                        focusBoundary: focus,
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ),
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
}

class FormFocus implements DesktopSettingsFocusBoundary {
  bool focused = false;
  @override
  void setSettingsInputFocused(bool value) => focused = value;
}

void formCleanup(
  DesktopSettingsFixture fixture,
  DesktopSettingsController controller,
) {
  addTearDown(() async {
    controller.dispose();
    await fixture.close();
  });
}

void applyProfile(DesktopSettingsFixture fixture, DesktopSettingsController c) {
  final request = fixture.profiles.last;
  final previous = c.snapshot!;
  fixture.emit(
    settingsReply(
      request.requestId,
      status: 'applied',
      targetId: request.targetId,
      snapshot: settingsSnapshot(
        revision: previous.revision.toBigInt().toInt() + 1,
        app: previous.app,
        profiles: [
          for (final p in previous.profiles)
            if (p.id == request.targetId) request.profile else p,
        ],
        bindings: {
          for (final p in previous.effectiveProfiles)
            p.id: p.id == request.targetId
                ? request.fontBinding
                : p.fontBinding,
        },
      ),
    ),
  );
}

void applyApp(DesktopSettingsFixture fixture, DesktopSettingsController c) {
  final request = fixture.settings.last;
  final previous = c.snapshot!;
  fixture.emit(
    settingsReply(
      request.requestId,
      status: 'applied',
      snapshot: settingsSnapshot(
        revision: previous.revision.toBigInt().toInt() + 1,
        app: request.app,
        profiles: previous.profiles,
        bindings: {
          for (final p in previous.effectiveProfiles) p.id: p.fontBinding,
        },
      ),
    ),
  );
}

void main() {
  testWidgets('Delete Applied 后消失目标不可执行生命周期或设默认，创建和切有效目标仍可用', (tester) async {
    final fixture = DesktopSettingsFixture();
    final c = fixture.controller();
    formCleanup(fixture, c);
    await mountForms(tester, c);
    await formTap(tester, 'section-profile');
    await bindingChoice(tester, 'selected-profile', '同名 / $settingsProfileB');
    await formTap(tester, 'profile-delete');
    await tester.tap(find.text('明确放弃').last);
    await tester.pump();
    fixture.emit(
      settingsReply(
        fixture.settings.last.requestId,
        status: 'applied',
        targetId: settingsProfileB,
        snapshot: settingsSnapshot(
          revision: 8,
          profiles: [settingsProfile(settingsProfileA)],
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(c.selectedProfileId, settingsProfileB);
    expect(c.profiles.containsKey(settingsProfileB), isFalse);
    final requests = fixture.settings.length;
    for (final operation in ['copy', 'rename', 'delete', 'default']) {
      expect(
        tester
            .widget<Button>(find.byKey(ValueKey('profile-$operation')))
            .onPressed,
        isNull,
      );
      await formTap(tester, 'profile-$operation');
    }
    expect(fixture.settings, hasLength(requests));
    expect(c.app!.value.defaultTerminalProfile, settingsProfileA);
    expect(tester.takeException(), isNull);
    await formTap(tester, 'profile-create');
    await tester.enterText(
      find.byKey(const ValueKey('lifecycle-name')),
      '删除后新建',
    );
    await tester.pump();
    await formTap(tester, 'lifecycle-confirm', settle: false);
    expect(fixture.settings.last.operation, 'create_profile');
    expect(fixture.settings.last.targetId, isEmpty);
    fixture.emit(
      settingsReply(
        fixture.settings.last.requestId,
        status: 'applied',
        targetId: settingsProfileC,
        snapshot: settingsSnapshot(
          revision: 9,
          profiles: [
            settingsProfile(settingsProfileA),
            settingsProfile(settingsProfileC, name: '删除后新建'),
          ],
        ),
      ),
    );
    await tester.pumpAndSettle();
    await bindingChoice(tester, 'selected-profile', '同名 / $settingsProfileA');
    for (final operation in ['copy', 'rename', 'delete', 'default']) {
      expect(
        tester
            .widget<Button>(find.byKey(ValueKey('profile-$operation')))
            .onPressed,
        isNotNull,
      );
    }
  });

  testWidgets('生命周期旧帧回调遇到目标消失安全拒绝，不将失效 ID 写入应用草稿', (tester) async {
    final fixture = DesktopSettingsFixture();
    final c = fixture.controller();
    formCleanup(fixture, c);
    await mountForms(tester, c);
    await formTap(tester, 'section-profile');
    await bindingChoice(tester, 'selected-profile', '同名 / $settingsProfileB');
    final callbacks = [
      for (final operation in ['copy', 'rename', 'delete', 'default'])
        tester
            .widget<Button>(find.byKey(ValueKey('profile-$operation')))
            .onPressed!,
    ];
    final id = c.refresh();
    fixture.emit(
      settingsReply(
        id,
        snapshot: settingsSnapshot(
          revision: 8,
          profiles: [settingsProfile(settingsProfileA)],
        ),
      ),
    );
    final requests = fixture.settings.length;
    for (final callback in callbacks) {
      callback();
      await tester.pump();
      expect(tester.takeException(), isNull);
    }
    expect(c.app!.value.defaultTerminalProfile, settingsProfileA);
    expect(fixture.settings, hasLength(requests));
    expect(find.byKey(const ValueKey('lifecycle-name')), findsNothing);
  });

  testWidgets('生命周期草稿确认期间外部删除目标，不放弃保留草稿或继续打开失效目标操作', (tester) async {
    for (final operation in ['copy', 'rename', 'delete']) {
      final fixture = DesktopSettingsFixture();
      final c = fixture.controller();
      formCleanup(fixture, c);
      await mountForms(tester, c);
      await formTap(tester, 'section-profile');
      await bindingChoice(tester, 'selected-profile', '同名 / $settingsProfileB');
      await formEdit(tester, '应答文本 Answerback', '待核对草稿');
      await formEdit(tester, '初始列数', '080');
      await formTap(tester, 'profile-$operation');
      final id = c.refresh();
      fixture.emit(
        settingsReply(
          id,
          snapshot: settingsSnapshot(
            revision: 8,
            profiles: [settingsProfile(settingsProfileA)],
          ),
        ),
      );
      await tester.pump();
      final requests = fixture.settings.length;
      await tester.tap(find.text('放弃目标草稿').last);
      await tester.pumpAndSettle();
      expect(tester.takeException(), isNull);
      expect(find.byKey(const ValueKey('lifecycle-name')), findsNothing);
      expect(c.profiles[settingsProfileB]?.value.answerback, '待核对草稿');
      expect(c.profiles[settingsProfileB]?.numbers.initialCols, '080');
      expect(fixture.settings, hasLength(requests));
      expect(c.needsReview(settingsProfileB), isTrue);
    }
  });

  testWidgets('外部删除后保留草稿可普通编辑，但不可选择已无保存目标的 TTF 资源', (tester) async {
    final fixture = DesktopSettingsFixture();
    final c = fixture.controller();
    formCleanup(fixture, c);
    var picks = 0;
    await mountForms(
      tester,
      c,
      picker: (_, _) async {
        picks++;
        return null;
      },
    );
    await formTap(tester, 'section-profile');
    await bindingChoice(tester, 'selected-profile', '同名 / $settingsProfileB');
    await formEdit(tester, '应答文本 Answerback', '保留草稿');
    await formEdit(tester, '初始列数', '080');
    final query = c.refresh();
    fixture.emit(
      settingsReply(
        query,
        snapshot: settingsSnapshot(
          revision: 8,
          profiles: [settingsProfile(settingsProfileA)],
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(
      tester.widget<Button>(find.byKey(const ValueKey('pick-ttf'))).onPressed,
      isNull,
    );
    final modes = tester.widget<ComboBox<String>>(
      find.descendant(
        of: find.byKey(const ValueKey('font_binding')),
        matching: find.byType(ComboBox<String>),
      ),
    );
    expect(
      modes.items!
          .singleWhere(
            (item) => item.value == ProfileFontMode.explicitCachedTtf.name,
          )
          .enabled,
      isFalse,
    );
    expect(
      c.profiles[settingsProfileB]!.fontBinding.mode,
      ProfileFontMode.legacyUndetermined,
    );
    expect(tester.widget<TextBox>(textField('字体名称 Family')).enabled, isTrue);
    await formEdit(tester, '字体名称 Family', '草稿字体');
    await formEdit(tester, '字号', '14.');
    await formEdit(tester, '应答文本 Answerback', '继续核对');
    expect(c.profiles[settingsProfileB]!.value.fontFamily, '草稿字体');
    expect(
      c.profiles[settingsProfileB]!.fontBinding.mode,
      ProfileFontMode.explicitFamily,
    );
    expect(c.profiles[settingsProfileB]!.numbers.initialCols, '080');
    expect(c.profiles[settingsProfileB]!.numbers.fontSize, '14.');
    expect(c.profiles[settingsProfileB]!.value.answerback, '继续核对');
    expect(picks, 0);
    expect(fixture.ttfs, isEmpty);
  });

  for (final fromMode in [false, true]) {
    testWidgets('资源初始化前目标消失（模式回调 $fromMode）有可见反馈且不调用 picker', (tester) async {
      final fixture = DesktopSettingsFixture();
      final c = fixture.controller();
      formCleanup(fixture, c);
      var picks = 0;
      await mountForms(
        tester,
        c,
        picker: (_, _) async {
          picks++;
          return null;
        },
      );
      await formTap(tester, 'section-profile');
      await bindingChoice(tester, 'selected-profile', '同名 / $settingsProfileB');
      await formEdit(tester, '应答文本 Answerback', '保留草稿');
      final start = tester
          .widget<Button>(find.byKey(const ValueKey('pick-ttf')))
          .onPressed!;
      final modeChanged = tester
          .widget<SettingsChoice>(find.byKey(const ValueKey('font_binding')))
          .onChanged!;
      final query = c.refresh();
      fixture.emit(
        settingsReply(
          query,
          snapshot: settingsSnapshot(
            revision: 8,
            profiles: [settingsProfile(settingsProfileA)],
          ),
        ),
      );
      if (fromMode) {
        modeChanged(ProfileFontMode.explicitCachedTtf.name);
      } else {
        start();
      }
      await tester.pumpAndSettle();
      expect(tester.takeException(), isNull);
      expect(picks, 0);
      expect(fixture.ttfs, isEmpty);
      expect(find.byKey(const ValueKey('profile-font-issue')), findsOneWidget);
      expect(c.profiles[settingsProfileB]!.value.answerback, '保留草稿');
      expect(
        c.profiles[settingsProfileB]!.fontBinding.mode,
        ProfileFontMode.legacyUndetermined,
      );
      expect(c.profiles[settingsProfileB]!.ttf, isNull);
    });
  }

  testWidgets('字体选择等待期间目标删除，迟到结果不串新目标或关闭后的草稿', (tester) async {
    final fixture = DesktopSettingsFixture();
    final c = fixture.controller();
    formCleanup(fixture, c);
    final selections = <String, Completer<DesktopTtfSelection?>>{};
    await mountForms(
      tester,
      c,
      picker: (_, id) {
        final result = Completer<DesktopTtfSelection?>();
        selections[id] = result;
        return result.future;
      },
    );
    await formTap(tester, 'section-profile');
    await bindingChoice(tester, 'selected-profile', '同名 / $settingsProfileB');
    await formEdit(tester, '应答文本 Answerback', '删除后保留');
    await formTap(tester, 'pick-ttf');
    final query = c.refresh();
    fixture.emit(
      settingsReply(
        query,
        snapshot: settingsSnapshot(
          revision: 8,
          profiles: [settingsProfile(settingsProfileA)],
        ),
      ),
    );
    await tester.pumpAndSettle();
    await bindingChoice(tester, 'selected-profile', '同名 / $settingsProfileA');
    await formTap(tester, 'pick-ttf');
    selections[settingsProfileB]!.complete(
      DesktopTtfSelection(family: '旧目标选择', bytes: Uint8List.fromList([1, 2])),
    );
    await tester.pumpAndSettle();
    expect(c.profiles[settingsProfileB]!.ttf, isNull);
    expect(c.profiles[settingsProfileA]!.ttf, isNull);
    expect(
      tester.widget<Button>(find.byKey(const ValueKey('pick-ttf'))).onPressed,
      isNull,
    );
    await tester.pumpWidget(const SizedBox.shrink());
    selections[settingsProfileA]!.complete(
      DesktopTtfSelection(family: '关闭后选择', bytes: Uint8List.fromList([3, 4])),
    );
    await tester.pumpAndSettle();
    expect(c.profiles[settingsProfileB]!.value.answerback, '删除后保留');
    expect(c.profiles[settingsProfileA]!.ttf, isNull);
    expect(fixture.ttfs, isEmpty);
    expect(tester.takeException(), isNull);
  });

  testWidgets('完整 profile 的 16 字段真实编辑、DTO 提交、Applied 回读；数字原文不强制回填', (
    tester,
  ) async {
    final fixture = DesktopSettingsFixture();
    final c = fixture.controller();
    formCleanup(fixture, c);
    await mountForms(tester, c);
    await formTap(tester, 'section-profile');
    for (final entry in {
      '配置名称': '已编辑配置',
      '终端类型 TERM': 'screen-256color',
      '初始列数': '080',
      '初始行数': '35',
      '应答文本 Answerback': '终端应答',
      '字体名称 Family': 'Noto Sans Mono',
      '字号': '14.',
      '滚回行数': '20000',
    }.entries) {
      await formEdit(tester, entry.key, entry.value);
    }
    await bindingChoice(
      tester,
      'profile-color_scheme',
      desktopColorLabels['nord']!,
    );
    for (final field in [
      'scroll_on_output',
      'scroll_on_keypress',
      'left_alt_as_meta',
      'right_alt_as_meta',
      'enable_csi_u',
      'enable_kitty_keyboard',
      'mouse_reporting',
    ]) {
      await formTap(tester, field);
    }
    final row = c.selectedProfile!.bindings.single;
    await tester.ensureVisible(
      find
          .descendant(
            of: find.byType(DesktopSettingsBindingsEditor),
            matching: find.byType(TextBox),
          )
          .first,
    );
    await tester.enterText(
      find
          .descendant(
            of: find.byType(DesktopSettingsBindingsEditor),
            matching: find.byType(TextBox),
          )
          .first,
      '界',
    );
    await tester.pump();
    await bindingChoice(
      tester,
      'binding-action-${row.id}',
      desktopActionLabels['send:\u0008']!,
    );
    await formTap(tester, 'save-profile', settle: false);
    final request = fixture.profiles.single;
    expect(request.baseRevision, settingsRevision(7));
    expect(request.targetId, settingsProfileA);
    final p = request.profile;
    expect(p.id, settingsProfileA);
    expect(p.name, '已编辑配置');
    expect(p.terminalType, 'screen-256color');
    expect(p.initialCols, 80);
    expect(p.initialRows, 35);
    expect(p.answerback, '终端应答');
    expect(p.fontFamily, 'Noto Sans Mono');
    expect(p.fontSize, 14);
    expect(p.colorScheme, 'nord');
    expect(p.scrollbackLines, 20000);
    expect(p.scrollOnOutput, isFalse);
    expect(p.scrollOnKeypress, isTrue);
    expect(p.leftAltAsMeta, isFalse);
    expect(p.rightAltAsMeta, isTrue);
    expect(p.enableCsiU, isTrue);
    expect(p.enableKittyKeyboard, isTrue);
    expect(p.mouseReporting, isFalse);
    expect(p.keyBindings.single.key, '界');
    expect(p.keyBindings.single.action.codeUnits, [115, 101, 110, 100, 58, 8]);
    expect(request.fontBinding.mode, ProfileFontMode.explicitFamily);
    applyProfile(fixture, c);
    await tester.pumpAndSettle();
    expect(c.selectedProfile!.dirty, isFalse);
    expect(c.selectedProfile!.numbers.initialCols, '080');
    expect(c.selectedProfile!.numbers.fontSize, '14.');
    await formTap(tester, 'section-app');
    await formTap(tester, 'section-profile');
    expect(tester.widget<TextBox>(textField('初始列数')).controller!.text, '080');
    expect(tester.widget<TextBox>(textField('字号')).controller!.text, '14.');
    final query = c.refresh();
    fixture.emit(settingsReply(query, snapshot: c.snapshot));
    await tester.pump();
    expect(c.selectedProfile!.value, p);
    expect(tester.takeException(), isNull);
  });

  testWidgets('应用三字段与双方全部配色逐项提交回读、App 与 profile 独立', (tester) async {
    final fixture = DesktopSettingsFixture();
    final c = fixture.controller();
    formCleanup(fixture, c);
    await mountForms(tester, c);
    await bindingChoice(
      tester,
      'default_terminal_profile',
      '同名 / $settingsProfileB',
    );
    final row = c.app!.bindings.single;
    await bindingChoice(
      tester,
      'binding-action-${row.id}',
      desktopActionLabels['new_tab']!,
    );
    for (final color in settingsCapabilities.colorSchemes.reversed) {
      await bindingChoice(
        tester,
        'app-color_scheme',
        desktopColorLabels[color]!,
      );
      await formTap(tester, 'save-app', settle: false);
      expect(fixture.settings.last.app!.colorScheme, color);
      expect(
        fixture.settings.last.app!.defaultTerminalProfile,
        settingsProfileB,
      );
      expect(fixture.settings.last.app!.keyBindings.single.action, 'new_tab');
      applyApp(fixture, c);
      await tester.pumpAndSettle();
      expect(c.app!.dirty, isFalse);
      expect(c.snapshot!.app.colorScheme, color);
      expect(c.selectedProfile!.value.colorScheme, 'default');
    }
    await formTap(tester, 'section-profile');
    for (final color in settingsCapabilities.colorSchemes.reversed) {
      await bindingChoice(
        tester,
        'profile-color_scheme',
        desktopColorLabels[color]!,
      );
      await formTap(tester, 'save-profile', settle: false);
      expect(fixture.profiles.last.profile.colorScheme, color);
      applyProfile(fixture, c);
      await tester.pumpAndSettle();
      expect(c.selectedProfile!.dirty, isFalse);
      expect(c.snapshot!.app.colorScheme, 'default');
    }
  });

  testWidgets('空和非法数字保草稿、错误能定位；空 target 的 Rust 错误按 RID 定位当前单元', (tester) async {
    final fixture = DesktopSettingsFixture();
    final c = fixture.controller();
    formCleanup(fixture, c);
    await mountForms(tester, c);
    await formTap(tester, 'section-profile');
    await formEdit(tester, '初始列数', '');
    await formEdit(tester, '字号', '非数字');
    await formTap(tester, 'save-profile');
    expect(fixture.profiles, isEmpty);
    expect(c.selectedProfile!.numbers.initialCols, '');
    await formTap(tester, 'locate-initial_cols');
    expect(
      tester.widget<TextBox>(textField('初始列数')).focusNode!.hasFocus,
      isTrue,
    );
    await formEdit(tester, '初始列数', '99');
    await formEdit(tester, '字号', '16.5');
    await formTap(tester, 'save-profile', settle: false);
    fixture.emit(
      settingsReply(
        fixture.profiles.last.requestId,
        status: 'validation_error',
        errors: [
          const DesktopSettingsFieldError(field: 'answerback', code: 'empty'),
          const DesktopSettingsFieldError(
            field: 'extension.unknown',
            code: 'unsupported',
          ),
        ],
      ),
    );
    await tester.pumpAndSettle();
    expect(
      c.stateFor(settingsProfileA).errors.byField.containsKey('answerback'),
      isTrue,
    );
    expect(c.stateFor(settingsProfileA).errors.pageErrors, hasLength(1));
    await formTap(tester, 'locate-answerback');
    expect(
      tester.widget<TextBox>(textField('应答文本 Answerback')).focusNode!.hasFocus,
      isTrue,
    );
    expect(c.selectedProfile!.dirty, isTrue);
  });

  testWidgets('切换同名目标保独立 ID 草稿，脏关闭须确认，表单隔离终端 chord／zoom', (tester) async {
    final fixture = DesktopSettingsFixture();
    final c = fixture.controller();
    formCleanup(fixture, c);
    var closed = 0;
    var chords = 0;
    final focus = FormFocus();
    final formsKey = GlobalKey<DesktopSettingsFormsState>();
    await mountForms(
      tester,
      c,
      onClose: () => closed++,
      formsKey: formsKey,
      shellChord: () => chords++,
      focus: focus,
    );
    await formTap(tester, 'section-profile');
    await formEdit(tester, '初始列数', '080');
    await formEdit(tester, '应答文本 Answerback', 'A草稿');
    await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyK);
    await tester.sendKeyEvent(LogicalKeyboardKey.equal);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.tab);
    await tester.pump();
    expect(chords, 0);
    expect(focus.focused, isTrue);
    await bindingChoice(tester, 'selected-profile', '同名 / $settingsProfileB');
    await formEdit(tester, '应答文本 Answerback', 'B草稿');
    await bindingChoice(
      tester,
      'selected-profile',
      '同名 / $settingsProfileA · 未保存',
    );
    expect(
      tester.widget<TextBox>(textField('应答文本 Answerback')).controller!.text,
      'A草稿',
    );
    expect(tester.widget<TextBox>(textField('初始列数')).controller!.text, '080');
    expect(c.profiles[settingsProfileB]!.value.answerback, 'B草稿');
    unawaited(formsKey.currentState!.requestClose());
    await tester.pumpAndSettle();
    expect(closed, 0);
    await tester.tap(find.text('继续编辑').last);
    await tester.pumpAndSettle();
    expect(closed, 0);
    await formTap(tester, 'close-settings');
    await tester.tap(find.text('明确放弃').last);
    await tester.pumpAndSettle();
    expect(closed, 1);
  });

  testWidgets(
    'conflict 必须显式 fresh query 后 Keep／Discard；query ok 不是 saved，不自动 retry',
    (tester) async {
      final fixture = DesktopSettingsFixture();
      final c = fixture.controller();
      formCleanup(fixture, c);
      await mountForms(tester, c);
      await formTap(tester, 'section-profile');
      await formEdit(tester, '应答文本 Answerback', '本地草稿');
      await formTap(tester, 'save-profile', settle: false);
      fixture.emit(
        settingsReply(
          fixture.profiles.last.requestId,
          status: 'conflict',
          revision: settingsRevision(8),
        ),
      );
      await tester.pumpAndSettle();
      expect(
        tester
            .widget<FilledButton>(find.byKey(const ValueKey('save-profile')))
            .onPressed,
        isNull,
      );
      await formTap(tester, 'fresh-query-$settingsProfileA', settle: false);
      final external = settingsSnapshot(
        revision: 8,
        profiles: [
          settingsProfile(settingsProfileA).copyWith(answerback: '外部值'),
          settingsProfile(settingsProfileB),
        ],
      );
      fixture.emit(
        settingsReply(fixture.settings.last.requestId, snapshot: external),
      );
      await tester.pumpAndSettle();
      expect(fixture.profiles, hasLength(1));
      expect(c.selectedProfile!.value.answerback, '本地草稿');
      await formTap(tester, 'review-$settingsProfileA');
      await formTap(tester, 'review-keep');
      expect(c.selectedProfile!.value.answerback, '本地草稿');
      expect(c.selectedProfile!.baseRevision, settingsRevision(8));
      expect(fixture.profiles, hasLength(1));
      await formTap(tester, 'fresh-query-$settingsProfileA', settle: false);
      fixture.emit(
        settingsReply(fixture.settings.last.requestId, snapshot: external),
      );
      await tester.pumpAndSettle();
      await formTap(tester, 'review-$settingsProfileA');
      await formTap(tester, 'review-discard');
      expect(c.selectedProfile!.value.answerback, '外部值');
      expect(c.selectedProfile!.dirty, isFalse);
      expect(
        c.stateFor(settingsProfileA).state,
        DesktopSettingsWriteState.idle,
      );
    },
  );

  testWidgets('字体模式与 family 意图保持；TTF callback 确认走完整二进制 gateway，取消和切目标不串写', (
    tester,
  ) async {
    final fixture = DesktopSettingsFixture();
    final c = fixture.controller();
    formCleanup(fixture, c);
    Completer<DesktopTtfSelection?>? pick;
    var calls = 0;
    await mountForms(
      tester,
      c,
      picker: (_, id) {
        expect(id, settingsProfileA);
        calls++;
        pick = Completer();
        return pick!.future;
      },
    );
    await formTap(tester, 'section-profile');
    expect(
      c.selectedProfile!.fontBinding.mode,
      ProfileFontMode.legacyUndetermined,
    );
    await bindingChoice(
      tester,
      'font_binding',
      desktopFontModeLabel(ProfileFontMode.inheritTerminalSlot),
    );
    await formTap(tester, 'save-profile', settle: false);
    expect(
      fixture.profiles.last.fontBinding.mode,
      ProfileFontMode.inheritTerminalSlot,
    );
    expect(fixture.profiles.last.profile.fontFamily, 'MesloLGS NF');
    applyProfile(fixture, c);
    await tester.pumpAndSettle();
    await bindingChoice(
      tester,
      'font_binding',
      desktopFontModeLabel(ProfileFontMode.explicitFamily),
    );
    expect(c.selectedProfile!.value.fontFamily, 'MesloLGS NF');
    expect(c.selectedProfile!.fontBinding.mode, ProfileFontMode.explicitFamily);
    await formTap(tester, 'pick-ttf');
    pick!.complete(null);
    await tester.pumpAndSettle();
    expect(c.selectedProfile!.fontBinding.mode, ProfileFontMode.explicitFamily);
    await formTap(tester, 'pick-ttf');
    await bindingChoice(tester, 'selected-profile', '同名 / $settingsProfileB');
    pick!.complete(
      DesktopTtfSelection(
        family: settingsFontFamily,
        bytes: Uint8List.fromList([0, 1, 2]),
      ),
    );
    await tester.pumpAndSettle();
    expect(c.profiles[settingsProfileA]!.ttf, isNull);
    expect(c.selectedProfile!.ttf, isNull);
    await bindingChoice(
      tester,
      'selected-profile',
      '同名 / $settingsProfileA · 未保存',
    );
    await formTap(tester, 'pick-ttf');
    pick!.complete(
      DesktopTtfSelection(
        family: settingsFontFamily,
        bytes: Uint8List.fromList([0, 1, 2, 3]),
      ),
    );
    await tester.pumpAndSettle();
    expect(c.selectedProfile!.ttf!.family, settingsFontFamily);
    expect(
      c.selectedProfile!.fontBinding.mode,
      ProfileFontMode.explicitCachedTtf,
    );
    await formEdit(tester, '字号', '17.25');
    await formTap(tester, 'save-profile', settle: false);
    expect(fixture.ttfs.single.profile.fontFamily, settingsFontFamily);
    expect(fixture.ttfs.single.profile.fontSize, 17.25);
    expect(fixture.ttfs.single.targetId, settingsProfileA);
    expect(fixture.ttfs.single.baseRevision, settingsRevision(8));
    expect(fixture.binaries.single, [0, 1, 2, 3]);
    fixture.emit(
      settingsReply(
        fixture.ttfs.single.requestId,
        status: 'applied',
        targetId: settingsProfileA,
        snapshot: settingsSnapshot(
          revision: 9,
          profiles: [
            fixture.ttfs.single.profile,
            settingsProfile(settingsProfileB),
          ],
          bindings: {settingsProfileA: settingsCachedFontBinding},
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(c.selectedProfile!.dirty, isFalse);
    expect(c.selectedProfile!.ttf, isNull);
    expect(c.selectedProfile!.fontBinding, settingsCachedFontBinding);
    expect(calls, 3);
  });

  testWidgets('不可编辑字体元数据只限制字体身份，非字体字段与字号仍能原样保全保存', (tester) async {
    final fixture = DesktopSettingsFixture();
    final c = fixture.controller(
      snapshot: settingsSnapshot(
        bindings: {
          settingsProfileA: const ProfileFontBinding(
            version: 99,
            mode: ProfileFontMode.uneditable,
            uneditableReason: ProfileFontMetadataError.unsupportedVersion,
          ),
        },
      ),
    );
    formCleanup(fixture, c);
    await mountForms(tester, c);
    await formTap(tester, 'section-profile');
    expect(tester.widget<TextBox>(textField('字体名称 Family')).enabled, isFalse);
    await formEdit(tester, '终端类型 TERM', 'vt100');
    await formEdit(tester, '字号', '19.5');
    await formTap(tester, 'save-profile', settle: false);
    expect(fixture.profiles.single.fontBinding.version, 99);
    expect(
      fixture.profiles.single.fontBinding.mode,
      ProfileFontMode.uneditable,
    );
    expect(fixture.profiles.single.profile.terminalType, 'vt100');
    expect(fixture.profiles.single.profile.fontSize, 19.5);
    applyProfile(fixture, c);
    await tester.pumpAndSettle();
    expect(c.selectedProfile!.dirty, isFalse);
  });

  testWidgets('保存期间继续编辑须显式核对；超时、迟到结果、fresh query 不盲重试或误清草稿', (tester) async {
    final fixture = DesktopSettingsFixture();
    final c = fixture.controller();
    formCleanup(fixture, c);
    await mountForms(tester, c);
    await formTap(tester, 'section-profile');
    await formEdit(tester, '应答文本 Answerback', '发出值');
    await formTap(tester, 'save-profile', settle: false);
    await formEdit(tester, '应答文本 Answerback', '后续输入');
    applyProfile(fixture, c);
    await tester.pumpAndSettle();
    expect(c.selectedProfile!.value.answerback, '后续输入');
    expect(
      tester
          .widget<FilledButton>(find.byKey(const ValueKey('save-profile')))
          .onPressed,
      isNull,
    );
    await formTap(tester, 'fresh-query-$settingsProfileA', settle: false);
    fixture.emit(
      settingsReply(fixture.settings.last.requestId, snapshot: c.snapshot),
    );
    await tester.pumpAndSettle();
    await formTap(tester, 'review-$settingsProfileA');
    await formTap(tester, 'review-keep');
    await formTap(tester, 'save-profile', settle: false);
    await tester.pump(const Duration(milliseconds: 45));
    expect(c.blockingWrite, isNotNull);
    expect(
      c.stateFor(settingsProfileA).state,
      DesktopSettingsWriteState.uncertain,
    );
    await formTap(tester, 'fresh-query-$settingsProfileA', settle: false);
    fixture.emit(
      settingsReply(fixture.settings.last.requestId, snapshot: c.snapshot),
    );
    await tester.pumpAndSettle();
    expect(
      tester
          .widget<Button>(find.byKey(ValueKey('review-$settingsProfileA')))
          .onPressed,
      isNull,
    );
    applyProfile(fixture, c);
    await tester.pumpAndSettle();
    expect(
      c.stateFor(settingsProfileA).state,
      DesktopSettingsWriteState.needsConfirmation,
    );
    await formTap(tester, 'fresh-query-$settingsProfileA', settle: false);
    fixture.emit(
      settingsReply(fixture.settings.last.requestId, snapshot: c.snapshot),
    );
    await tester.pumpAndSettle();
    await formTap(tester, 'review-$settingsProfileA');
    await formTap(tester, 'review-discard');
    expect(c.blockingWrite, isNull);
    expect(c.selectedProfile!.dirty, isFalse);
    expect(fixture.profiles, hasLength(2));
    expect(c.stateFor(settingsProfileA).state, DesktopSettingsWriteState.idle);
  });

  testWidgets('profile 生命周期创建／复制获得新 ID、重命名保 ID、设默认独立提交、删除保护不迁移引用', (
    tester,
  ) async {
    final fixture = DesktopSettingsFixture();
    final c = fixture.controller();
    formCleanup(fixture, c);
    await mountForms(tester, c);
    await formTap(tester, 'section-profile');
    Future<void> namedOperation(
      String operation,
      String name,
      DesktopConfiguration result,
      String target,
    ) async {
      await formTap(tester, 'profile-$operation');
      await tester.enterText(
        find.byKey(const ValueKey('lifecycle-name')),
        name,
      );
      await tester.pump();
      await formTap(tester, 'lifecycle-confirm', settle: false);
      final req = fixture.settings.last;
      expect(
        req.operation,
        '${operation == "create"
            ? "create"
            : operation == "copy"
            ? "copy"
            : "rename"}_profile',
      );
      expect(req.name, name);
      fixture.emit(
        settingsReply(
          req.requestId,
          status: 'applied',
          targetId: target,
          snapshot: result,
        ),
      );
      await tester.pumpAndSettle();
    }

    await namedOperation(
      'create',
      '新配置',
      settingsSnapshot(
        revision: 8,
        profiles: [
          settingsProfile(settingsProfileA),
          settingsProfile(settingsProfileB),
          settingsProfile(settingsProfileC, name: '新配置'),
        ],
      ),
      settingsProfileC,
    );
    expect(c.selectedProfileId, settingsProfileA);
    expect(c.profiles.containsKey(settingsProfileC), isTrue);
    const copiedId = '00000000-0000-4000-8000-000000000004';
    await namedOperation(
      'copy',
      '复制配置',
      settingsSnapshot(
        revision: 9,
        profiles: [
          ...c.snapshot!.profiles,
          settingsProfile(copiedId, name: '复制配置'),
        ],
      ),
      copiedId,
    );
    expect(fixture.settings.last.targetId, settingsProfileA);
    expect(c.selectedProfileId, settingsProfileA);
    await bindingChoice(tester, 'selected-profile', '复制配置 / $copiedId');
    await namedOperation(
      'rename',
      '重命名',
      settingsSnapshot(
        revision: 10,
        profiles: [
          for (final p in c.snapshot!.profiles)
            if (p.id == copiedId) p.copyWith(name: '重命名') else p,
        ],
      ),
      copiedId,
    );
    expect(c.selectedProfileId, copiedId);
    expect(c.selectedProfile!.value.name, '重命名');
    await formTap(tester, 'profile-default');
    expect(c.app!.value.defaultTerminalProfile, copiedId);
    await formTap(tester, 'save-app', settle: false);
    applyApp(fixture, c);
    await tester.pumpAndSettle();
    await formTap(tester, 'section-profile');
    await formTap(tester, 'profile-delete');
    await tester.tap(find.text('明确放弃').last);
    await tester.pump();
    expect(fixture.settings.last.operation, 'delete_profile');
    expect(fixture.settings.last.targetId, copiedId);
    fixture.emit(
      settingsReply(
        fixture.settings.last.requestId,
        status: 'validation_error',
        errors: [
          const DesktopSettingsFieldError(
            field: 'target_id',
            code: 'referenced_profile',
          ),
        ],
      ),
    );
    await tester.pumpAndSettle();
    expect(c.profiles.containsKey(copiedId), isTrue);
    expect(c.app!.value.defaultTerminalProfile, copiedId);
    await bindingChoice(tester, 'selected-profile', '新配置 / $settingsProfileC');
    await formTap(tester, 'profile-delete');
    await tester.tap(find.text('明确放弃').last);
    await tester.pump();
    fixture.emit(
      settingsReply(
        fixture.settings.last.requestId,
        status: 'applied',
        targetId: settingsProfileC,
        snapshot: settingsSnapshot(
          revision: 12,
          app: c.snapshot!.app,
          profiles: c.snapshot!.profiles
              .where((p) => p.id != settingsProfileC)
              .toList(),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(c.profiles.containsKey(settingsProfileC), isFalse);
    expect(tester.takeException(), isNull);
  });

  testWidgets('应用与 profile 六动作均通过真实编辑器提交生成 DTO；同 chord 覆盖提示而不执行动作', (
    tester,
  ) async {
    final fixture = DesktopSettingsFixture();
    final c = fixture.controller();
    formCleanup(fixture, c);
    await mountForms(tester, c);
    for (final action in settingsCapabilities.keyActions.reversed) {
      await bindingChoice(
        tester,
        'binding-action-${c.app!.bindings.single.id}',
        desktopActionLabels[action]!,
      );
      await formTap(tester, 'save-app', settle: false);
      expect(
        fixture.settings.last.app!.keyBindings.single.action.codeUnits,
        action.codeUnits,
      );
      applyApp(fixture, c);
      await tester.pumpAndSettle();
    }
    await formTap(tester, 'section-profile');
    expect(find.text('此配置覆盖应用中的相同按键组合'), findsOneWidget);
    for (final action in settingsCapabilities.keyActions.reversed) {
      await bindingChoice(
        tester,
        'binding-action-${c.selectedProfile!.bindings.single.id}',
        desktopActionLabels[action]!,
      );
      await formTap(tester, 'save-profile', settle: false);
      expect(
        fixture.profiles.last.profile.keyBindings.single.action.codeUnits,
        action.codeUnits,
      );
      applyProfile(fixture, c);
      await tester.pumpAndSettle();
    }
    expect(
      fixture.settings.where((r) => r.operation == 'save_app'),
      hasLength(6),
    );
    expect(fixture.profiles, hasLength(6));
  });

  testWidgets('源草稿复制前明确放弃或先保存；生命周期表单 Tab／Enter 可操作且正常动效无失效引用', (tester) async {
    final fixture = DesktopSettingsFixture();
    final c = fixture.controller();
    formCleanup(fixture, c);
    await mountForms(tester, c, reduceMotion: false);
    await formTap(tester, 'section-profile');
    await formEdit(tester, '应答文本 Answerback', '未保存源');
    await formTap(tester, 'profile-copy');
    expect(
      fixture.settings.where((r) => r.operation == 'copy_profile'),
      isEmpty,
    );
    await tester.tap(find.text('继续编辑').last);
    await tester.pumpAndSettle();
    expect(c.selectedProfile!.dirty, isTrue);
    await formTap(tester, 'profile-copy');
    await tester.tap(find.text('先保存目标').last);
    await tester.pump();
    expect(fixture.profiles, hasLength(1));
    expect(
      fixture.settings.where((r) => r.operation == 'copy_profile'),
      isEmpty,
    );
    applyProfile(fixture, c);
    await tester.pumpAndSettle();
    await formEdit(tester, '应答文本 Answerback', '将放弃源');
    await formTap(tester, 'profile-copy');
    await tester.tap(find.text('放弃目标草稿').last);
    await tester.pumpAndSettle();
    expect(c.selectedProfile!.value.answerback, '未保存源');
    await tester.enterText(
      find.byKey(const ValueKey('lifecycle-name')),
      '键盘复制',
    );
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.tab);
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.tab);
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pump();
    expect(fixture.settings.last.operation, 'copy_profile');
    expect(fixture.settings.last.name, '键盘复制');
    fixture.emit(
      settingsReply(fixture.settings.last.requestId, status: 'storage_error'),
    );
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
    expect(c.selectedProfile!.value.answerback, '未保存源');
  });

  testWidgets('首次加载／失败／不可用／重试由真实 query 驱动，不制造可编辑占位字段', (tester) async {
    final fixture = DesktopSettingsFixture();
    final c = DesktopSettingsController(gateway: fixture.gateway);
    formCleanup(fixture, c);
    await mountForms(tester, c);
    expect(c.app, isNull);
    expect(find.byType(TextBox), findsNothing);
    await formTap(tester, 'query-retry', settle: false);
    expect(c.queryState, DesktopSettingsQueryState.loading);
    fixture.emit(
      settingsReply(
        fixture.settings.last.requestId,
        status: 'unsupported_platform',
      ).copyWith(
        capabilities: settingsCapabilities.copyWith(
          available: false,
          unavailableReason: '配置不可用',
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(c.editable, isFalse);
    expect(find.byType(TextBox), findsNothing);
    await formTap(tester, 'query-retry', settle: false);
    fixture.emit(
      settingsReply(
        fixture.settings.last.requestId,
        snapshot: settingsSnapshot(),
      ),
    );
    await tester.pumpAndSettle();
    expect(c.editable, isTrue);
    expect(find.byType(TextBox), findsWidgets);
    expect(c.stateFor('app').state, DesktopSettingsWriteState.idle);
  });

  testWidgets('下拉弹层与生命周期对话框的输入不触发 Navigator 外部的终端 chord／zoom', (tester) async {
    final fixture = DesktopSettingsFixture();
    final c = fixture.controller();
    formCleanup(fixture, c);
    var chords = 0;
    final focus = FormFocus();
    await mountForms(tester, c, shellChord: () => chords++, focus: focus);
    await tester.tap(
      find.descendant(
        of: find.byKey(const ValueKey('app-color_scheme')),
        matching: find.byType(ComboBox<String>),
      ),
    );
    await tester.pumpAndSettle();
    await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyK);
    await tester.sendKeyEvent(LogicalKeyboardKey.equal);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
    await tester.pump();
    expect(chords, 0);
    await tester.tap(find.text(desktopColorLabels['nord']!).last);
    await tester.pumpAndSettle();
    await formTap(tester, 'section-profile');
    await formTap(tester, 'profile-create');
    await tester.enterText(
      find.byKey(const ValueKey('lifecycle-name')),
      '键盘焦点',
    );
    await tester.pump();
    await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyK);
    await tester.sendKeyEvent(LogicalKeyboardKey.equal);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
    await tester.pump();
    expect(chords, 0);
    expect(focus.focused, isTrue);
    await tester.tap(find.text('取消').last);
    await tester.pumpAndSettle();
  });

  test('实际正文和焦点 token 的暗色及高对比度对比度', () {
    double contrast(Color a, Color b) {
      final values = [a.computeLuminance(), b.computeLuminance()]..sort();
      return (values.last + 0.05) / (values.first + 0.05);
    }

    expect(
      contrast(DesktopColors.text, DesktopColors.surface),
      greaterThanOrEqualTo(7),
    );
    expect(
      contrast(DesktopColors.muted, DesktopColors.surface),
      greaterThanOrEqualTo(7),
    );
    expect(
      contrast(DesktopColors.accent, DesktopColors.surface),
      greaterThanOrEqualTo(3),
    );
    expect(
      contrast(DesktopColors.onAccent, DesktopColors.accent),
      greaterThanOrEqualTo(4.5),
    );
    expect(
      contrast(
        DesktopColors.highContrastText,
        DesktopColors.highContrastCanvas,
      ),
      21,
    );
  });

  for (final width in [375.0, 768.0, 1280.0]) {
    for (final accessible in [false, true]) {
      testWidgets('宽度 $width（辅助模式 $accessible）字段可达且不溢出', (tester) async {
        final fixture = DesktopSettingsFixture();
        final c = fixture.controller();
        formCleanup(fixture, c);
        await mountForms(
          tester,
          c,
          width: width,
          scale: accessible ? 1.6 : 1,
          highContrast: accessible,
          reduceMotion: accessible,
        );
        await formTap(tester, 'section-profile');
        await formEdit(tester, '应答文本 Answerback', '响应式编辑');
        await bindingChoice(
          tester,
          'selected-profile',
          '同名 / $settingsProfileB',
        );
        await bindingChoice(
          tester,
          'profile-color_scheme',
          desktopColorLabels['nord']!,
        );
        await bindingChoice(
          tester,
          'font_binding',
          desktopFontModeLabel(ProfileFontMode.inheritTerminalSlot),
        );
        await bindingChoice(
          tester,
          'binding-action-${c.selectedProfile!.bindings.single.id}',
          desktopActionLabels['new_tab']!,
        );
        await bindingChoice(
          tester,
          'selected-profile',
          '同名 / $settingsProfileA · 未保存',
        );
        await formTap(tester, 'mouse_reporting');
        await formTap(tester, 'save-profile', settle: false);
        expect(fixture.profiles.last.profile.answerback, '响应式编辑');
        applyProfile(fixture, c);
        await tester.pumpAndSettle();
        expect(tester.takeException(), isNull);
      });
    }
  }
}
