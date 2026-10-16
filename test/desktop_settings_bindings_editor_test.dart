import 'package:fluent_ui/fluent_ui.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/desktop/desktop_theme.dart';
import 'package:guosh_shell/src/desktop/settings/desktop_settings_draft.dart';
import 'package:guosh_shell/src/desktop/settings/forms/desktop_settings_bindings_editor.dart';
import 'package:guosh_shell/src/desktop/settings/forms/settings_form_controls.dart';

import 'desktop_settings_gateway_test.dart';

Future<void> bindingTap(WidgetTester tester, Finder finder) async {
  await tester.ensureVisible(finder);
  await tester.tap(finder);
  await tester.pumpAndSettle();
}

Future<void> bindingChoice(
  WidgetTester tester,
  String key,
  String label,
) async {
  await bindingTap(
    tester,
    find.descendant(
      of: find.byKey(ValueKey(key)),
      matching: find.byType(ComboBox<String>),
    ),
  );
  await bindingTap(tester, find.text(label).last);
}

void main() {
  testWidgets('结构化绑定的六动作保留真实控制字符；所有修饰符和 Unicode／命名键可编辑', (tester) async {
    var rows = desktopBindingRows([settingsBinding]);
    await tester.pumpWidget(
      FluentApp(
        theme: desktopTheme(highContrast: false, reduceMotion: true),
        home: StatefulBuilder(
          builder: (context, update) => ScaffoldPage(
            content: SingleChildScrollView(
              child: DesktopSettingsBindingsEditor(
                rows: rows,
                actions: settingsCapabilities.keyActions,
                onChanged: (value) => update(() => rows = value),
              ),
            ),
          ),
        ),
      ),
    );
    final id = rows.single.id;
    final keyBox = find.descendant(
      of: find.byType(SettingsTextField),
      matching: find.byType(TextBox),
    );
    await tester.enterText(keyBox, '😀');
    await tester.pump();
    expect(rows.single.value.key.runes, hasLength(1));
    expect(rows.single.value.key, '😀');
    for (final modifier in ['shift', 'control', 'alt', 'super']) {
      await bindingTap(tester, find.byKey(ValueKey('$modifier-$id')));
    }
    expect(rows.single.value.shift, isFalse);
    expect(rows.single.value.control, isTrue);
    expect(rows.single.value.alt, isFalse);
    expect(rows.single.value.superKey, isTrue);
    for (final key in desktopNamedKeys.keys) {
      await bindingChoice(tester, 'binding-key-$id', desktopNamedKeys[key]!);
      expect(rows.single.value.key, key);
    }
    for (var i = 1; i <= 24; i++) {
      await bindingChoice(tester, 'binding-key-$id', 'F$i');
      expect(rows.single.value.key, 'F$i');
    }
    for (final action in settingsCapabilities.keyActions) {
      await bindingChoice(
        tester,
        'binding-action-$id',
        desktopActionLabels[action]!,
      );
      expect(rows.single.value.action, action);
      final roundtrip = DesktopKeyBinding.bincodeDeserialize(
        rows.single.value.bincodeSerialize(),
      );
      expect(roundtrip.action, action);
    }
    expect(
      settingsCapabilities.keyActions
          .take(3)
          .map((a) => a.substring(5).codeUnits)
          .toList(),
      [
        [27, 91, 51, 126],
        [127],
        [8],
      ],
    );
    expect(rows.single.id, id);
  });

  testWidgets('重复行独立身份、增删排序、错误跟随发起请求的行并可聚焦', (tester) async {
    var rows = desktopBindingRows([settingsBinding, settingsBinding]);
    final firstId = rows.first.id;
    final secondId = rows.last.id;
    final errors = DesktopDraftErrors([
      const DesktopSettingsFieldError(
        field: 'key_bindings[0].action',
        code: 'invalid_action',
      ),
    ], bindings: rows);
    await tester.pumpWidget(
      FluentApp(
        theme: desktopTheme(highContrast: true, reduceMotion: true),
        home: StatefulBuilder(
          builder: (context, update) => ScaffoldPage(
            content: SingleChildScrollView(
              child: DesktopSettingsBindingsEditor(
                rows: rows,
                actions: settingsCapabilities.keyActions,
                errors: errors,
                appBindings: [settingsBinding],
                onChanged: (v) => update(() => rows = v),
              ),
            ),
          ),
        ),
      ),
    );
    expect(find.text('此配置覆盖应用中的相同按键组合'), findsNWidgets(2));
    await bindingTap(tester, find.byKey(ValueKey('binding-down-$firstId')));
    expect(rows.map((r) => r.id), [secondId, firstId]);
    final erroredRow = find.byKey(ValueKey(firstId));
    expect(
      find.descendant(of: erroredRow, matching: find.text('定位错误')),
      findsOneWidget,
    );
    await bindingTap(
      tester,
      find.descendant(of: erroredRow, matching: find.text('定位错误')),
    );
    expect(FocusManager.instance.primaryFocus?.context?.widget, isNotNull);
    expect(
      tester
          .widget<ComboBox<String>>(
            find.descendant(
              of: find.byKey(ValueKey('binding-action-$firstId')),
              matching: find.byType(ComboBox<String>),
            ),
          )
          .focusNode!
          .hasFocus,
      isTrue,
    );
    await tester.sendKeyEvent(LogicalKeyboardKey.tab);
    await tester.pump();
    await bindingTap(tester, find.byKey(ValueKey('binding-add')));
    expect(rows, hasLength(3));
    expect(rows.map((r) => r.id).toSet(), hasLength(3));
    await bindingTap(tester, find.byKey(ValueKey('binding-delete-$secondId')));
    expect(rows, hasLength(2));
    expect(rows.first.id, firstId);
    expect(tester.takeException(), isNull);
  });
}
