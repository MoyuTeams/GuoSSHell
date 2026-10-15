import 'package:file_selector/file_selector.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/app.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/settings/interface_font.dart';
import 'package:guosh_shell/src/settings/interface_font_setting.dart';

class FontFixture {
  int id = 0;
  final actions = <String>[];
  final stored = <String, String>{};
  Future<FontReply> send(
    String slot,
    String action,
    String name,
    Uint8List bytes,
  ) async {
    actions.add(action);
    final fallback = slot == 'terminal' ? 'MesloLGS NF' : 'MiSans';
    final family = action == 'preview' || action == 'apply'
        ? 'GuoshFont_fixture'
        : action == 'reset'
        ? fallback
        : stored[slot] ?? fallback;
    if (action == 'apply' || action == 'reset') stored[slot] = family;
    return FontReply(
      FontFileState(
        requestId: ++id,
        slot: slot,
        family: family,
        label: family == 'GuoshFont_fixture' ? '预览测试字体' : fallback,
        error: name == 'bad.ttf' ? '损坏的字体文件' : '',
        applied: action != 'preview',
      ),
      bytes,
    );
  }

  InterfaceTypography controller(String slot) => InterfaceTypography(
    slot: slot,
    transport: send,
    register: (_, _) async {},
  );
}

void main() {
  test('预览字体注册共享额度，相同字体复用且超限不调用引擎', () async {
    final loaded = <String>[];
    final registry = InterfaceFontRegistry(
      register: (family, _) async => loaded.add(family),
      maxFonts: 2,
      maxBytes: 6,
    );
    await registry.load('one', Uint8List(4));
    await registry.load('one', Uint8List(4));
    await expectLater(registry.load('large', Uint8List(3)), throwsStateError);
    await registry.load('two', Uint8List(2));
    await expectLater(registry.load('third', Uint8List(1)), throwsStateError);
    expect(loaded, ['one', 'two']);
  });
  test('注册失败退还额度并移出缓存，同一字体可再次尝试', () async {
    var attempts = 0;
    final registry = InterfaceFontRegistry(
      register: (family, _) async {
        if (++attempts == 1) throw StateError('引擎拒绝');
      },
      maxFonts: 1,
      maxBytes: 4,
    );
    await expectLater(registry.load('one', Uint8List(4)), throwsStateError);
    expect(registry.accountedFontCount, 0);
    expect(registry.accountedByteCount, 0);
    await registry.load('one', Uint8List(4));
    expect(attempts, 2);
    expect(registry.accountedFontCount, 1);
    expect(registry.accountedByteCount, 4);
  });
  test('同步抛出的注册失败同样退额', () async {
    final registry = InterfaceFontRegistry(
      register: (family, _) => throw StateError('同步失败'),
      maxFonts: 1,
      maxBytes: 4,
    );
    await expectLater(registry.load('one', Uint8List(4)), throwsStateError);
    expect(registry.accountedFontCount, 0);
    expect(registry.accountedByteCount, 0);
  });
  test('重复预览同一字体只注册与计额一次', () async {
    final fixture = FontFixture();
    final loaded = <String>[];
    final registry = InterfaceFontRegistry(
      register: (family, _) async => loaded.add(family),
    );
    final interface = InterfaceTypography(
      transport: fixture.send,
      register: registry.load,
    );
    final terminal = InterfaceTypography(
      slot: 'terminal',
      transport: fixture.send,
      register: registry.load,
    );
    XFile chosen() =>
        XFile.fromData(Uint8List(4), name: 'chosen.ttf', path: 'chosen.ttf');
    await interface.prepare(chosen());
    await interface.prepare(chosen());
    await terminal.prepare(chosen());
    expect(loaded, ['GuoshFont_fixture']);
    expect(registry.accountedFontCount, 1);
    expect(registry.accountedByteCount, 4);
    interface.dispose();
    terminal.dispose();
  });
  for (final platform in [
    TargetPlatform.iOS,
    TargetPlatform.android,
    TargetPlatform.macOS,
    TargetPlatform.linux,
    TargetPlatform.windows,
  ]) {
    for (final slot in ['interface', 'terminal']) {
      testWidgets('$platform / $slot 使用文件选择、预览、应用与恢复内置', (tester) async {
        final fixture = FontFixture();
        final font = fixture.controller(slot);
        final fallback = font.defaultFamily;
        debugDefaultTargetPlatformOverride = platform;
        await tester.pumpWidget(
          MaterialApp(
            home: Scaffold(
              body: InterfaceFontSetting(
                terminal: slot == 'terminal',
                previewFontSize: slot == 'terminal' ? 22 : null,
                controller: font,
                pickFile: () async => XFile.fromData(
                  Uint8List.fromList([0, 1, 2]),
                  name: 'chosen.ttf',
                  path: 'chosen.ttf',
                ),
              ),
            ),
          ),
        );
        expect(find.byType(TextField), findsNothing);
        if (slot == 'terminal') {
          expect(
            tester
                .widget<Text>(
                  find.byKey(const ValueKey('terminal-font-preview')),
                )
                .style!
                .fontSize,
            22,
          );
        }
        await tester.tap(find.byKey(ValueKey('pick-$slot-font')));
        await tester.pumpAndSettle();
        expect(font.family, fallback);
        expect(fixture.stored, isEmpty, reason: '预览不能写入偏好');
        expect(
          tester
              .widget<Text>(find.byKey(ValueKey('$slot-font-preview')))
              .style!
              .fontFamily,
          'GuoshFont_fixture',
        );
        await tester.tap(find.text('取消预览'));
        await tester.pumpAndSettle();
        expect(font.family, fallback);
        await tester.tap(find.byKey(ValueKey('pick-$slot-font')));
        await tester.pumpAndSettle();
        await tester.tap(find.byKey(ValueKey('apply-$slot-font')));
        await tester.pumpAndSettle();
        expect(font.family, 'GuoshFont_fixture');
        expect(fixture.stored[slot], 'GuoshFont_fixture');
        await tester.tap(
          find.text(slot == 'terminal' ? '恢复内置等宽字体' : '恢复内置 MiSans'),
        );
        await tester.pumpAndSettle();
        expect(font.family, fallback);
        await tester.pumpWidget(const SizedBox());
        font.dispose();
        debugDefaultTargetPlatformOverride = null;
      });
    }
  }
  testWidgets('取消文件选择与校验失败均保留当前字体', (tester) async {
    final fixture = FontFixture();
    final font = fixture.controller('interface');
    Future<void> open(Future<XFile?> Function() pick) => tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: InterfaceFontSetting(controller: font, pickFile: pick),
        ),
      ),
    );
    await open(() async => null);
    await tester.tap(find.byKey(const ValueKey('pick-interface-font')));
    await tester.pumpAndSettle();
    expect(fixture.actions, isEmpty);
    await open(
      () async =>
          XFile.fromData(Uint8List(4), name: 'bad.ttf', path: 'bad.ttf'),
    );
    await tester.tap(find.byKey(const ValueKey('pick-interface-font')));
    await tester.pumpAndSettle();
    expect(find.text('损坏的字体文件'), findsOneWidget);
    expect(font.family, 'MiSans');
    expect(fixture.stored, isEmpty);
    await tester.pumpWidget(const SizedBox());
    font.dispose();
  });
  testWidgets('应用字体时保留导航器实例，重启查询恢复所选字体', (tester) async {
    final fixture = FontFixture();
    final font = fixture.controller('interface');
    await tester.pumpWidget(
      InterfaceFontScope(controller: font, child: const GuoSSHellApp()),
    );
    await tester.pump();
    final navigator = tester.state<NavigatorState>(
      find.byType(Navigator).first,
    );
    expect(
      tester
          .widget<MaterialApp>(find.byType(MaterialApp))
          .theme!
          .textTheme
          .bodyMedium!
          .fontFamily,
      'MiSans',
    );
    final draft = await font.prepare(
      XFile.fromData(Uint8List(4), name: 'chosen.ttf', path: 'chosen.ttf'),
    );
    await font.apply(draft);
    await tester.pump();
    expect(
      tester
          .widget<MaterialApp>(find.byType(MaterialApp))
          .theme!
          .textTheme
          .bodyMedium!
          .fontFamily,
      'GuoshFont_fixture',
    );
    expect(
      tester.state<NavigatorState>(find.byType(Navigator).first),
      same(navigator),
    );
    final reopened = fixture.controller('interface');
    await reopened.refresh();
    expect(reopened.family, 'GuoshFont_fixture');
    await tester.pumpWidget(const SizedBox());
    font.dispose();
    reopened.dispose();
  });
}
