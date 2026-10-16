import 'dart:async';
import 'dart:math' as math;

import 'package:file_selector/file_selector.dart';
import 'package:fluent_ui/fluent_ui.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/desktop/desktop_theme.dart';
import 'package:guosh_shell/src/desktop/settings/resources/desktop_about_resources.dart';
import 'package:guosh_shell/src/desktop/settings/resources/desktop_settings_resources.dart';
import 'package:guosh_shell/src/settings/interface_font.dart';
import 'package:guosh_shell/src/settings/licenses.dart';

class Boundary extends ChangeNotifier implements DesktopResourcesInputBoundary {
  @override
  bool ready = false;
  @override
  int generation = 0;
  Completer<void>? pending;
  int acquisitions = 0;
  int releases = 0;
  bool failAcquire = false;
  @override
  Future<void> acquire() async {
    ++acquisitions;
    if (failAcquire) throw StateError('fixture_boundary_failure');
    await pending?.future;
    ready = true;
    notifyListeners();
  }

  void revoke() {
    ready = false;
    ++generation;
    notifyListeners();
  }

  @override
  void release() {
    ++releases;
    ready = false;
    ++generation;
  }
}

class FontService {
  FontService(this.slot) {
    typography = InterfaceTypography(
      slot: slot.name,
      transport: send,
      register: (family, bytes) async {
        registrations.add(family);
      },
    );
    state = ValueNotifier(
      DesktopFontSlotState(
        savedFamily: typography.family,
        savedLabel: typography.label,
        effectiveFamily: typography.family,
      ),
    );
  }
  final DesktopFontSlot slot;
  final owner = Object();
  int generation = 0;
  int id = 0;
  int picks = 0;
  int writes = 0;
  bool failWrite = false;
  bool failPreview = false;
  bool cancelPick = false;
  String selectedName = 'chosen.ttf';
  final actions = <String>[];
  final registrations = <String>[];
  final stored = <String, String>{};
  late final InterfaceTypography typography;
  late final ValueNotifier<DesktopFontSlotState> state;
  Future<XFile?> Function()? pickOverride;
  Future<FontDraft> Function(XFile)? prepareOverride;
  Future<DesktopResourceWriteResult<DesktopFontSlotState>> Function(FontDraft)?
  writeOverride;
  Future<FontReply> send(
    String slot,
    String action,
    String name,
    Uint8List bytes,
  ) async {
    actions.add('$slot:$action');
    final failed = action == 'preview' ? failPreview : failWrite;
    final family = action == 'reset'
        ? typography.defaultFamily
        : 'GuoshFont_${slot}_$name';
    if (!failed && (action == 'apply' || action == 'reset')) {
      stored[slot] = family;
    }
    return FontReply(
      FontFileState(
        requestId: ++id,
        slot: slot,
        family: family,
        label: family,
        error: failed ? 'fixture_failure' : '',
        applied: !failed && action != 'preview',
      ),
      bytes,
    );
  }

  DesktopFontSlotState snapshot() => DesktopFontSlotState(
    savedFamily: typography.family,
    savedLabel: typography.label,
    effectiveFamily: typography.family,
  );
  DesktopFontResourceCallbacks callbacks() => DesktopFontResourceCallbacks(
    identity: DesktopResourceIdentity(owner, generation),
    slot: slot,
    state: state,
    pick: (context, request) async {
      expect(request.slot, slot);
      expect(DesktopTtfPickRequest.typeMetadata.single.extensions, ['ttf']);
      ++picks;
      if (pickOverride != null) return pickOverride!();
      return cancelPick
          ? null
          : XFile.fromData(
              Uint8List(4),
              name: selectedName,
              path: selectedName,
            );
    },
    prepare:
        prepareOverride ?? (file) => prepareDesktopTtfPreview(typography, file),
    apply: (draft) async {
      ++writes;
      if (writeOverride != null) return writeOverride!(draft);
      try {
        await typography.apply(draft);
        state.value = snapshot();
        return DesktopResourceWriteResult.confirmed(state.value);
      } catch (_) {
        return const DesktopResourceWriteResult.failed();
      }
    },
    reset: () async {
      ++writes;
      try {
        await typography.reset();
        state.value = snapshot();
        return DesktopResourceWriteResult.confirmed(state.value);
      } catch (_) {
        return const DesktopResourceWriteResult.failed();
      }
    },
  );
  void dispose() {
    typography.dispose();
    state.dispose();
  }
}

class AppearanceService {
  final owner = Object();
  int generation = 0;
  final state = ValueNotifier(
    const DesktopAppearanceState(
      saved: DesktopAppearanceValue(acrylic: true, opacity: .78),
      materialAvailable: true,
    ),
  );
  final previews = <DesktopAppearanceValue>[];
  final saves = <DesktopAppearanceValue>[];
  int restores = 0;
  DesktopResourceWriteStatus result = DesktopResourceWriteStatus.confirmed;
  Completer<DesktopResourceWriteResult<DesktopAppearanceValue>>? pending;
  DesktopAppearanceCallbacks callbacks() => DesktopAppearanceCallbacks(
    identity: DesktopResourceIdentity(owner, generation),
    state: state,
    preview: previews.add,
    restoreSavedPreview: () => ++restores,
    save: (draft) async {
      saves.add(draft);
      if (pending != null) return pending!.future;
      if (result == DesktopResourceWriteStatus.failed) {
        return const DesktopResourceWriteResult.failed();
      }
      if (result == DesktopResourceWriteStatus.uncertain) {
        return const DesktopResourceWriteResult.uncertain();
      }
      state.value = DesktopAppearanceState(
        saved: draft,
        materialAvailable: state.value.materialAvailable,
      );
      return DesktopResourceWriteResult.confirmed(draft);
    },
  );
}

class Surface {
  final resourceKey = GlobalKey<DesktopSettingsResourcesState>();
  final dirtyChanges = <bool>[];
  Boundary boundary = Boundary();
  final retiredBoundaries = <Boundary>[];
  void replaceBoundary() {
    retiredBoundaries.add(boundary);
    boundary = Boundary();
  }

  final interface = FontService(DesktopFontSlot.interface);
  final terminal = FontService(DesktopFontSlot.terminal);
  final appearance = AppearanceService();
  TargetPlatform platform = TargetPlatform.windows;
  DesktopLicenseRegistry registry = () => const Stream<LicenseEntry>.empty();

  Widget widget({
    double scale = 1,
    bool highContrast = false,
    bool reduceMotion = true,
  }) => FluentApp(
    theme: desktopTheme(highContrast: highContrast, reduceMotion: reduceMotion),
    builder: (context, child) => MediaQuery(
      data: MediaQuery.of(context).copyWith(
        textScaler: TextScaler.linear(scale),
        highContrast: highContrast,
        disableAnimations: reduceMotion,
      ),
      child: child!,
    ),
    home: ScaffoldPage(
      content: SingleChildScrollView(
        padding: const EdgeInsets.all(DesktopSpacing.lg),
        child: DesktopSettingsResources(
          key: resourceKey,
          interfaceFont: interface.callbacks(),
          terminalFont: terminal.callbacks(),
          inputBoundary: boundary,
          platform: platform,
          appearance: appearance.callbacks(),
          applicationName: 'GuoSSHell',
          applicationVersion: 'fixture',
          registry: registry,
          onDirtyChanged: dirtyChanges.add,
        ),
      ),
    ),
  );

  Future<void> close(WidgetTester tester) async {
    await tester.pumpWidget(const SizedBox());
    interface.dispose();
    terminal.dispose();
    appearance.state.dispose();
    boundary.dispose();
    for (final boundary in retiredBoundaries) {
      boundary.dispose();
    }
  }
}

Future<void> activate(WidgetTester tester, String key) async {
  final finder = find.byKey(ValueKey(key));
  await tester.ensureVisible(finder);
  await tester.pump();
  await tester.tap(finder);
  await tester.pumpAndSettle();
}

Future<void> editTransparency(WidgetTester tester, String raw) async {
  final finder = find.byKey(const ValueKey('resource-transparency'));
  await tester.ensureVisible(finder);
  await tester.pump();
  await tester.enterText(finder, raw);
  await tester.pump();
}

String previewFamily(WidgetTester tester, String slot) => tester
    .widget<Text>(find.byKey(ValueKey('$slot-resource-preview')))
    .style!
    .fontFamily!;

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('双槽选择、校验和本地预览与持久化分离；reset 保留已有资源', (tester) async {
    final s = Surface();
    await tester.pumpWidget(s.widget());
    await tester.pumpAndSettle();
    for (final font in [s.interface, s.terminal]) {
      final slot = font.slot.name;
      await activate(tester, 'pick-$slot-resource');
      expect(font.actions, ['$slot:preview']);
      expect(font.stored, isEmpty);
      expect(font.typography.family, font.typography.defaultFamily);
      expect(previewFamily(tester, slot), 'GuoshFont_${slot}_chosen.ttf');
      await activate(tester, 'apply-$slot-resource');
      expect(font.actions, ['$slot:preview', '$slot:apply']);
      expect(font.stored[slot], 'GuoshFont_${slot}_chosen.ttf');
      expect(
        tester
            .widget<FilledButton>(find.byKey(ValueKey('apply-$slot-resource')))
            .onPressed,
        isNull,
      );
      final loaded = List<String>.of(font.registrations);
      await activate(tester, 'reset-$slot-resource');
      expect(font.actions.last, '$slot:reset');
      expect(font.typography.family, font.typography.defaultFamily);
      expect(font.registrations, containsAll(loaded));
    }
    expect(s.interface.actions, everyElement(startsWith('interface:')));
    expect(s.terminal.actions, everyElement(startsWith('terminal:')));
    await s.close(tester);
  });

  testWidgets('取消选择、失败校验、失败保存不替换旧预览或保存字体', (tester) async {
    final s = Surface();
    await tester.pumpWidget(s.widget());
    await tester.pumpAndSettle();
    await activate(tester, 'pick-interface-resource');
    final selected = previewFamily(tester, 'interface');
    s.interface.cancelPick = true;
    await activate(tester, 'pick-interface-resource');
    expect(previewFamily(tester, 'interface'), selected);
    expect(s.interface.actions, ['interface:preview']);
    s.interface.cancelPick = false;
    s.interface.failPreview = true;
    await activate(tester, 'pick-interface-resource');
    expect(previewFamily(tester, 'interface'), selected);
    s.interface.failWrite = true;
    await activate(tester, 'apply-interface-resource');
    expect(s.interface.stored, isEmpty);
    expect(previewFamily(tester, 'interface'), selected);
    expect(
      tester
          .widget<FilledButton>(
            find.byKey(const ValueKey('apply-interface-resource')),
          )
          .onPressed,
      isNotNull,
    );
    await activate(tester, 'reset-interface-resource');
    expect(previewFamily(tester, 'interface'), selected);
    await activate(tester, 'cancel-interface-resource');
    expect(
      previewFamily(tester, 'interface'),
      s.interface.typography.defaultFamily,
    );
    await s.close(tester);
  });

  testWidgets('改选 midpending、取消和迟到 preview 不串槽', (tester) async {
    final s = Surface();
    final first = Completer<FontDraft>();
    s.interface.prepareOverride = (_) => first.future;
    await tester.pumpWidget(s.widget());
    await tester.pumpAndSettle();
    await activate(tester, 'pick-interface-resource');
    s.interface.prepareOverride = (_) async =>
        FontDraft('second.ttf', 'second-family', 'second', Uint8List(4));
    await tester.pumpWidget(s.widget());
    await tester.pump();
    await activate(tester, 'pick-interface-resource');
    expect(previewFamily(tester, 'interface'), 'second-family');
    first.complete(
      FontDraft('first.ttf', 'first-family', 'first', Uint8List(4)),
    );
    await tester.pumpAndSettle();
    expect(previewFamily(tester, 'interface'), 'second-family');
    expect(
      previewFamily(tester, 'terminal'),
      s.terminal.typography.defaultFamily,
    );
    final cancelled = Completer<FontDraft>();
    s.interface.prepareOverride = (_) => cancelled.future;
    await tester.pumpWidget(s.widget());
    await tester.pump();
    await activate(tester, 'pick-interface-resource');
    await activate(tester, 'cancel-interface-resource');
    cancelled.complete(
      FontDraft('late.ttf', 'late-family', 'late', Uint8List(4)),
    );
    await tester.pumpAndSettle();
    expect(
      previewFamily(tester, 'interface'),
      s.interface.typography.defaultFamily,
    );
    await s.close(tester);
  });

  testWidgets('picker 取消与关闭后的结果不注册、不追加写入', (tester) async {
    final s = Surface();
    final pick = Completer<XFile?>();
    s.interface.pickOverride = () => pick.future;
    await tester.pumpWidget(s.widget());
    await tester.pumpAndSettle();
    await activate(tester, 'pick-interface-resource');
    await tester.pumpWidget(const SizedBox());
    pick.complete(XFile.fromData(Uint8List(4), name: 'late.ttf'));
    await tester.pumpAndSettle();
    expect(s.interface.actions, isEmpty);
    expect(tester.takeException(), isNull);
    await s.close(tester);
  });

  testWidgets('身份和输入边界 generation 撤销使 preview 失效', (tester) async {
    final s = Surface();
    final first = Completer<FontDraft>();
    s.interface.prepareOverride = (_) => first.future;
    await tester.pumpWidget(s.widget());
    await tester.pumpAndSettle();
    await activate(tester, 'pick-interface-resource');
    ++s.interface.generation;
    await tester.pumpWidget(s.widget());
    await tester.pump();
    first.complete(FontDraft('old.ttf', 'old-owner', 'old', Uint8List(4)));
    await tester.pumpAndSettle();
    expect(
      previewFamily(tester, 'interface'),
      s.interface.typography.defaultFamily,
    );
    final second = Completer<FontDraft>();
    s.interface.prepareOverride = (_) => second.future;
    await tester.pumpWidget(s.widget());
    await tester.pump();
    await activate(tester, 'pick-interface-resource');
    s.boundary.revoke();
    await tester.pump();
    second.complete(FontDraft('old.ttf', 'old-focus', 'old', Uint8List(4)));
    await tester.pumpAndSettle();
    expect(
      previewFamily(tester, 'interface'),
      s.interface.typography.defaultFamily,
    );
    await s.close(tester);
  });

  testWidgets('未知写入结果隔离，父明确核实后可恢复保存；普通 rebuild 无效', (tester) async {
    final s = Surface();
    s.interface.writeOverride = (_) async =>
        const DesktopResourceWriteResult.uncertain();
    await tester.pumpWidget(s.widget());
    await tester.pumpAndSettle();
    await activate(tester, 'pick-interface-resource');
    final oldApply = tester
        .widget<FilledButton>(
          find.byKey(const ValueKey('apply-interface-resource')),
        )
        .onPressed!;
    await activate(tester, 'apply-interface-resource');
    expect(
      tester
          .widget<FilledButton>(
            find.byKey(const ValueKey('apply-interface-resource')),
          )
          .onPressed,
      isNull,
    );
    await tester.pumpWidget(s.widget());
    await tester.pumpAndSettle();
    expect(
      tester
          .widget<Button>(
            find.byKey(const ValueKey('reset-interface-resource')),
          )
          .onPressed,
      isNull,
    );
    oldApply();
    await tester.pumpAndSettle();
    expect(s.interface.writes, 1);
    final saved = s.interface.state.value;
    s.interface.state.value = DesktopFontSlotState(
      savedFamily: saved.savedFamily,
      savedLabel: saved.savedLabel,
      effectiveFamily: saved.effectiveFamily,
      verificationVersion: 1,
    );
    await tester.pump();
    expect(
      tester
          .widget<FilledButton>(
            find.byKey(const ValueKey('apply-interface-resource')),
          )
          .onPressed,
      isNotNull,
    );
    await s.close(tester);
  });

  testWidgets('写入中槽不可重发，迟到确认不能清除新身份的选择', (tester) async {
    final s = Surface();
    final pending =
        Completer<DesktopResourceWriteResult<DesktopFontSlotState>>();
    s.interface.writeOverride = (_) => pending.future;
    await tester.pumpWidget(s.widget());
    await tester.pumpAndSettle();
    await activate(tester, 'pick-interface-resource');
    await activate(tester, 'apply-interface-resource');
    expect(
      tester
          .widget<Button>(find.byKey(const ValueKey('pick-interface-resource')))
          .onPressed,
      isNull,
    );
    ++s.interface.generation;
    s.interface.writeOverride = null;
    s.interface.selectedName = 'new-owner.ttf';
    await tester.pumpWidget(s.widget());
    await tester.pump();
    await activate(tester, 'pick-interface-resource');
    final family = previewFamily(tester, 'interface');
    pending.complete(
      DesktopResourceWriteResult.confirmed(s.interface.state.value),
    );
    await tester.pumpAndSettle();
    expect(previewFamily(tester, 'interface'), family);
    expect(
      tester
          .widget<FilledButton>(
            find.byKey(const ValueKey('apply-interface-resource')),
          )
          .onPressed,
      isNotNull,
    );
    await s.close(tester);
  });

  testWidgets('输入 boundaryPending 时文本、选择与写入均不可编辑', (tester) async {
    final s = Surface();
    s.boundary.pending = Completer<void>();
    await tester.pumpWidget(s.widget());
    await tester.pump();
    expect(
      tester
          .widget<TextBox>(find.byKey(const ValueKey('resource-transparency')))
          .enabled,
      isFalse,
    );
    expect(
      tester
          .widget<Button>(find.byKey(const ValueKey('pick-interface-resource')))
          .onPressed,
      isNull,
    );
    expect(s.interface.picks, 0);
    s.boundary.pending!.complete();
    await tester.pumpAndSettle();
    expect(
      tester
          .widget<TextBox>(find.byKey(const ValueKey('resource-transparency')))
          .enabled,
      isTrue,
    );
    await s.close(tester);
    expect(s.boundary.releases, 1);
  });

  testWidgets('边界获取失败即使 ready 残留也不能编辑，重试确认才恢复', (tester) async {
    final s = Surface();
    s.boundary.ready = true;
    s.boundary.failAcquire = true;
    await tester.pumpWidget(s.widget());
    await tester.pumpAndSettle();
    expect(
      tester
          .widget<TextBox>(find.byKey(const ValueKey('resource-transparency')))
          .enabled,
      isFalse,
    );
    expect(
      tester
          .widget<Button>(find.byKey(const ValueKey('pick-interface-resource')))
          .onPressed,
      isNull,
    );
    s.boundary.failAcquire = false;
    await activate(tester, 'retry-resource-boundary');
    expect(
      tester
          .widget<TextBox>(find.byKey(const ValueKey('resource-transparency')))
          .enabled,
      isTrue,
    );
    await s.close(tester);
  });

  testWidgets('脏状态覆盖双槽与无效数字，保存失败保留，明确放弃后清除', (tester) async {
    final s = Surface();
    await tester.pumpWidget(s.widget());
    await tester.pumpAndSettle();
    expect(s.resourceKey.currentState!.hasUnsavedChanges, isFalse);
    await activate(tester, 'pick-interface-resource');
    expect(s.dirtyChanges, [true]);
    await activate(tester, 'pick-terminal-resource');
    await activate(tester, 'apply-interface-resource');
    expect(s.resourceKey.currentState!.hasUnsavedChanges, isTrue);
    await activate(tester, 'cancel-terminal-resource');
    expect(s.dirtyChanges, [true, false]);
    await editTransparency(tester, '');
    await tester.pump();
    expect(s.resourceKey.currentState!.hasUnsavedChanges, isTrue);
    await activate(tester, 'cancel-resource-appearance');
    expect(s.resourceKey.currentState!.hasUnsavedChanges, isFalse);
    await s.close(tester);
  });

  testWidgets('外观外部已保存值刷新与小数草稿重开不截断，toggle 不改透明度', (tester) async {
    final s = Surface();
    await tester.pumpWidget(s.widget());
    await tester.pumpAndSettle();
    s.appearance.state.value = const DesktopAppearanceState(
      saved: DesktopAppearanceValue(acrylic: true, opacity: .875),
      materialAvailable: true,
    );
    await tester.pump();
    expect(
      tester
          .widget<TextBox>(find.byKey(const ValueKey('resource-transparency')))
          .controller!
          .text,
      '12.5',
    );
    final toggle = tester.widget<ToggleSwitch>(
      find.byKey(const ValueKey('resource-acrylic')),
    );
    toggle.onChanged!(false);
    await tester.pump();
    expect(s.appearance.previews.last.opacity, .875);
    await activate(tester, 'save-resource-appearance');
    expect(s.appearance.saves.single.opacity, .875);
    await s.close(tester);
  });

  testWidgets('Windows 范围、数字 raw、即时预览与明确保存的调用分离', (tester) async {
    final s = Surface();
    await tester.pumpWidget(s.widget());
    await tester.pumpAndSettle();
    for (final raw in ['', 'NaN', '-1', '66']) {
      await editTransparency(tester, raw);
      expect(
        tester
            .widget<TextBox>(
              find.byKey(const ValueKey('resource-transparency')),
            )
            .controller!
            .text,
        raw,
      );
      expect(s.appearance.previews, isEmpty);
      expect(
        tester
            .widget<FilledButton>(
              find.byKey(const ValueKey('save-resource-appearance')),
            )
            .onPressed,
        isNull,
      );
    }
    for (final value in ['0', '65', '12.5']) {
      await editTransparency(tester, value);
      expect(
        s.appearance.previews.last.opacity,
        closeTo(1 - double.parse(value) / 100, 1e-9),
      );
    }
    expect(s.appearance.saves, isEmpty);
    await activate(tester, 'save-resource-appearance');
    expect(s.appearance.saves.single.opacity, .875);
    expect(s.appearance.state.value.saved.opacity, .875);
    final slider = tester.widget<Slider>(
      find.byKey(const ValueKey('resource-transparency-slider')),
    );
    expect(slider.min, 0);
    expect(slider.max, 65);
    await s.close(tester);
  });

  testWidgets('外观失败回撤预览但保草稿；取消不持久化；未知结果不能重发', (tester) async {
    final s = Surface();
    s.appearance.result = DesktopResourceWriteStatus.failed;
    await tester.pumpWidget(s.widget());
    await tester.pumpAndSettle();
    await editTransparency(tester, '40');
    await activate(tester, 'save-resource-appearance');
    expect(s.appearance.restores, 1);
    expect(s.appearance.state.value.saved.opacity, .78);
    expect(
      tester
          .widget<TextBox>(find.byKey(const ValueKey('resource-transparency')))
          .controller!
          .text,
      '40',
    );
    s.appearance.result = DesktopResourceWriteStatus.uncertain;
    await activate(tester, 'save-resource-appearance');
    expect(s.appearance.restores, 2);
    expect(
      tester
          .widget<FilledButton>(
            find.byKey(const ValueKey('save-resource-appearance')),
          )
          .onPressed,
      isNull,
    );
    await tester.pumpWidget(s.widget());
    await tester.pump();
    expect(
      tester
          .widget<FilledButton>(
            find.byKey(const ValueKey('save-resource-appearance')),
          )
          .onPressed,
      isNull,
    );
    await activate(tester, 'cancel-resource-appearance');
    expect(s.appearance.saves, hasLength(2));
    expect(
      tester
          .widget<TextBox>(find.byKey(const ValueKey('resource-transparency')))
          .controller!
          .text,
      '22.0',
    );
    await s.close(tester);
  });

  testWidgets('外观迟到结果不串父身份；焦点撤销回撤预览', (tester) async {
    final s = Surface();
    s.appearance.pending =
        Completer<DesktopResourceWriteResult<DesktopAppearanceValue>>();
    await tester.pumpWidget(s.widget());
    await tester.pumpAndSettle();
    await editTransparency(tester, '40');
    await activate(tester, 'save-resource-appearance');
    ++s.appearance.generation;
    await tester.pumpWidget(s.widget());
    await tester.pump();
    await editTransparency(tester, '30');
    s.appearance.pending!.complete(
      const DesktopResourceWriteResult.confirmed(
        DesktopAppearanceValue(acrylic: true, opacity: .6),
      ),
    );
    await tester.pumpAndSettle();
    expect(
      tester
          .widget<TextBox>(find.byKey(const ValueKey('resource-transparency')))
          .controller!
          .text,
      '30',
    );
    final restores = s.appearance.restores;
    s.boundary.revoke();
    await tester.pump();
    expect(s.appearance.restores, restores + 1);
    expect(
      tester
          .widget<TextBox>(find.byKey(const ValueKey('resource-transparency')))
          .enabled,
      isFalse,
    );
    await s.close(tester);
  });

  for (final platform in [TargetPlatform.macOS, TargetPlatform.linux]) {
    testWidgets('$platform 不暴露 Windows 编辑器，不调用宿主服务', (tester) async {
      final s = Surface()..platform = platform;
      await tester.pumpWidget(s.widget());
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('resource-transparency')), findsNothing);
      expect(find.byKey(const ValueKey('resource-acrylic')), findsNothing);
      expect(s.appearance.previews, isEmpty);
      expect(s.appearance.saves, isEmpty);
      expect(s.appearance.restores, 0);
      await activate(tester, 'pick-terminal-resource');
      expect(s.terminal.actions, ['terminal:preview']);
      await s.close(tester);
      expect(s.appearance.restores, 0);
    });
  }

  testWidgets('许可 Fluent route 使用注入的真实 Flutter registry，返回保留草稿', (
    tester,
  ) async {
    LicenseRegistry.reset();
    LicenseRegistry.addLicense(() async* {
      yield LicenseEntryWithLineBreaks([
        'fixture-package',
      ], 'fixture-license-boundary');
      yield LicenseEntryWithLineBreaks([
        'second-package',
      ], 'second-license-boundary');
    });
    final s = Surface()..registry = (() => LicenseRegistry.licenses);
    await tester.pumpWidget(s.widget());
    await tester.pumpAndSettle();
    await activate(tester, 'pick-interface-resource');
    final family = previewFamily(tester, 'interface');
    await activate(tester, 'open-resource-licenses');
    expect(find.byType(DesktopResourceLicenses), findsOneWidget);
    expect(find.text('fixture-package'), findsOneWidget);
    expect(find.text('fixture-license-boundary'), findsOneWidget);
    expect(find.text('second-package'), findsOneWidget);
    await activate(tester, 'return-resource-licenses');
    expect(previewFamily(tester, 'interface'), family);
    expect(s.interface.actions, ['interface:preview']);
    await s.close(tester);
    LicenseRegistry.reset();
  });

  test('已有字体、rsHell、Rust 许可通过实际注册资源可加载', () async {
    LicenseRegistry.reset();
    registerLicenses();
    final entries = await LicenseRegistry.licenses.toList();
    expect(entries, isNotEmpty);
    expect(
      entries.expand((entry) => entry.packages),
      containsAll(['rsHell', 'MesloLGS NF', 'MiSans（小米界面字体）']),
    );
    expect(entries.length, greaterThan(3));
    expect(
      entries.every(
        (entry) =>
            entry.paragraphs.any((paragraph) => paragraph.text.isNotEmpty),
      ),
      isTrue,
    );
    LicenseRegistry.reset();
  });

  testWidgets('许可 stream 失败保留已加载条目，重试与离开释放订阅', (tester) async {
    final s = Surface();
    final stream = StreamController<LicenseEntry>();
    var loads = 0;
    s.registry = () {
      ++loads;
      return stream.stream;
    };
    await tester.pumpWidget(s.widget());
    await tester.pumpAndSettle();
    await activate(tester, 'open-resource-licenses');
    expect(stream.hasListener, isTrue);
    stream.add(LicenseEntryWithLineBreaks(['live-package'], 'live-boundary'));
    stream.addError(StateError('fixture_license_failure'));
    await tester.pumpAndSettle();
    expect(find.text('live-package'), findsOneWidget);
    expect(loads, 1);
    await activate(tester, 'return-resource-licenses');
    expect(stream.hasListener, isFalse);
    unawaited(stream.close());
    await tester.pump();
    await s.close(tester);
    expect(stream.isClosed, isTrue);
  });

  testWidgets('实际预览正文与边界满足 AA 对比度，高对比度采用主题覆盖', (tester) async {
    final s = Surface();
    double luminance(Color color) {
      double linear(double value) => value <= .04045
          ? value / 12.92
          : math.pow((value + .055) / 1.055, 2.4).toDouble();
      return .2126 * linear(color.r) +
          .7152 * linear(color.g) +
          .0722 * linear(color.b);
    }

    double contrast(Color a, Color b) {
      final x = luminance(a), y = luminance(b);
      return (math.max(x, y) + .05) / (math.min(x, y) + .05);
    }

    await tester.pumpWidget(s.widget(reduceMotion: false));
    await tester.pumpAndSettle();
    final preview = tester.widget<Text>(
      find.byKey(const ValueKey('interface-resource-preview')),
    );
    expect(preview.style!.fontSize, greaterThanOrEqualTo(DesktopType.body));
    expect(
      contrast(preview.style!.color!, DesktopColors.surface),
      greaterThanOrEqualTo(7),
    );
    expect(
      contrast(DesktopColors.border, DesktopColors.surface),
      greaterThanOrEqualTo(3),
    );
    await tester.pumpWidget(s.widget(highContrast: true));
    await tester.pumpAndSettle();
    final high = tester.widget<Text>(
      find.byKey(const ValueKey('interface-resource-preview')),
    );
    expect(
      contrast(high.style!.color!, DesktopColors.highContrastCanvas),
      greaterThanOrEqualTo(7),
    );
    await s.close(tester);
  });

  testWidgets('Tab 和 Enter 激活 Fluent 字体按钮；状态可访问', (tester) async {
    final s = Surface();
    final semantics = tester.ensureSemantics();
    await tester.pumpWidget(s.widget());
    await tester.pumpAndSettle();
    await tester.sendKeyEvent(LogicalKeyboardKey.tab);
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pumpAndSettle();
    expect(s.interface.picks, 1);
    expect(s.interface.actions, ['interface:preview']);
    expect(
      find.byWidgetPredicate(
        (widget) => widget is Semantics && widget.properties.liveRegion == true,
      ),
      findsWidgets,
    );
    semantics.dispose();
    await s.close(tester);
  });

  testWidgets('许可加载失败可重试真实 registry callback，成功后呈现新条目', (tester) async {
    final s = Surface();
    var loads = 0;
    s.registry = () => ++loads == 1
        ? Stream<LicenseEntry>.error(StateError('fixture_failure'))
        : Stream<LicenseEntry>.value(
            LicenseEntryWithLineBreaks(['retry-package'], 'retry-boundary'),
          );
    await tester.pumpWidget(s.widget());
    await tester.pumpAndSettle();
    await activate(tester, 'open-resource-licenses');
    await activate(tester, 'retry-resource-licenses');
    expect(loads, 2);
    expect(find.text('retry-package'), findsOneWidget);
    expect(find.text('retry-boundary'), findsOneWidget);
    await activate(tester, 'return-resource-licenses');
    await s.close(tester);
  });

  test('非持久 preview adapter 沿用 32 MiB 检查，不设额外资源上限', () async {
    final font = FontService(DesktopFontSlot.terminal);
    await expectLater(
      prepareDesktopTtfPreview(
        font.typography,
        XFile.fromData(
          Uint8List(4),
          path: 'large.ttf',
          length: 32 * 1024 * 1024 + 1,
        ),
      ),
      throwsStateError,
    );
    expect(font.actions, isEmpty);
    await prepareDesktopTtfPreview(
      font.typography,
      XFile.fromData(
        Uint8List(4),
        path: 'boundary.ttf',
        length: 32 * 1024 * 1024,
      ),
    );
    expect(font.actions, ['terminal:preview']);
    expect(font.stored, isEmpty);
    expect(font.typography.family, font.typography.defaultFamily);
    font.dispose();
  });

  testWidgets('同字体槽更换 lease 保留预览草稿与 dirty，旧 picker 结果不进入新 lease', (
    tester,
  ) async {
    final s = Surface();
    await tester.pumpWidget(s.widget());
    await tester.pumpAndSettle();
    await activate(tester, 'pick-interface-resource');
    final family = previewFamily(tester, 'interface');
    final latePick = Completer<XFile?>();
    s.interface.pickOverride = () => latePick.future;
    await activate(tester, 'pick-interface-resource');
    s.replaceBoundary();
    await tester.pumpWidget(s.widget());
    await tester.pumpAndSettle();
    latePick.complete(XFile.fromData(Uint8List(4), path: 'old-lease.ttf'));
    await tester.pumpAndSettle();
    expect(previewFamily(tester, 'interface'), family);
    expect(s.resourceKey.currentState!.hasUnsavedChanges, isTrue);
    expect(s.dirtyChanges, [true]);
    expect(s.interface.actions, ['interface:preview']);
    expect(
      tester
          .widget<FilledButton>(
            find.byKey(const ValueKey('apply-interface-resource')),
          )
          .onPressed,
      isNotNull,
    );
    expect(
      previewFamily(tester, 'terminal'),
      s.terminal.typography.defaultFamily,
    );
    await s.close(tester);
  });

  for (final status in [
    DesktopResourceWriteStatus.confirmed,
    DesktopResourceWriteStatus.failed,
  ]) {
    testWidgets('更换字体 lease 后 pending $status 保留草稿，实际 settle 只结束本请求 busy', (
      tester,
    ) async {
      final s = Surface();
      final pending =
          Completer<DesktopResourceWriteResult<DesktopFontSlotState>>();
      s.interface.writeOverride = (_) => pending.future;
      await tester.pumpWidget(s.widget());
      await tester.pumpAndSettle();
      await activate(tester, 'pick-interface-resource');
      final family = previewFamily(tester, 'interface');
      await activate(tester, 'apply-interface-resource');
      s.replaceBoundary();
      await tester.pumpWidget(s.widget());
      await tester.pumpAndSettle();
      expect(
        tester
            .widget<Button>(
              find.byKey(const ValueKey('pick-interface-resource')),
            )
            .onPressed,
        isNull,
      );
      expect(
        tester
            .widget<FilledButton>(
              find.byKey(const ValueKey('apply-interface-resource')),
            )
            .onPressed,
        isNull,
      );
      expect(previewFamily(tester, 'interface'), family);
      pending.complete(
        status == DesktopResourceWriteStatus.confirmed
            ? DesktopResourceWriteResult.confirmed(s.interface.state.value)
            : const DesktopResourceWriteResult.failed(),
      );
      await tester.pumpAndSettle();
      expect(previewFamily(tester, 'interface'), family);
      expect(s.resourceKey.currentState!.hasUnsavedChanges, isTrue);
      expect(
        tester
            .widget<Button>(
              find.byKey(const ValueKey('pick-interface-resource')),
            )
            .onPressed,
        isNotNull,
      );
      expect(
        tester
            .widget<FilledButton>(
              find.byKey(const ValueKey('apply-interface-resource')),
            )
            .onPressed,
        isNotNull,
      );
      s.interface.writeOverride = null;
      await activate(tester, 'apply-interface-resource');
      expect(s.interface.writes, 2);
      expect(s.interface.stored['interface'], family);
      expect(s.resourceKey.currentState!.hasUnsavedChanges, isFalse);
      await s.close(tester);
    });
  }

  testWidgets('字体 unknown 隔离跨 lease 保留，原 writer 核实后恢复编辑保存', (tester) async {
    final s = Surface();
    final pending =
        Completer<DesktopResourceWriteResult<DesktopFontSlotState>>();
    s.interface.writeOverride = (_) => pending.future;
    await tester.pumpWidget(s.widget());
    await tester.pumpAndSettle();
    await activate(tester, 'pick-interface-resource');
    final family = previewFamily(tester, 'interface');
    await activate(tester, 'apply-interface-resource');
    s.replaceBoundary();
    await tester.pumpWidget(s.widget());
    await tester.pumpAndSettle();
    expect(
      tester
          .widget<Button>(find.byKey(const ValueKey('pick-interface-resource')))
          .onPressed,
      isNull,
    );
    pending.complete(const DesktopResourceWriteResult.uncertain());
    await tester.pumpAndSettle();
    expect(
      tester
          .widget<Button>(find.byKey(const ValueKey('pick-interface-resource')))
          .onPressed,
      isNotNull,
    );
    expect(
      tester
          .widget<Button>(
            find.byKey(const ValueKey('reset-interface-resource')),
          )
          .onPressed,
      isNull,
    );
    s.replaceBoundary();
    await tester.pumpWidget(s.widget());
    await tester.pumpAndSettle();
    expect(previewFamily(tester, 'interface'), family);
    expect(s.resourceKey.currentState!.hasUnsavedChanges, isTrue);
    expect(
      tester
          .widget<Button>(
            find.byKey(const ValueKey('reset-interface-resource')),
          )
          .onPressed,
      isNull,
    );
    final saved = s.interface.state.value;
    s.interface.state.value = DesktopFontSlotState(
      savedFamily: saved.savedFamily,
      savedLabel: saved.savedLabel,
      effectiveFamily: saved.effectiveFamily,
      fallbackReason: saved.fallbackReason,
      verificationVersion: 1,
    );
    await tester.pump();
    expect(
      tester
          .widget<FilledButton>(
            find.byKey(const ValueKey('apply-interface-resource')),
          )
          .onPressed,
      isNotNull,
    );
    s.interface.writeOverride = null;
    await activate(tester, 'apply-interface-resource');
    expect(s.interface.writes, 2);
    expect(s.interface.stored['interface'], family);
    await s.close(tester);
  });

  for (final replace in [false, true]) {
    for (final status in [
      DesktopResourceWriteStatus.failed,
      DesktopResourceWriteStatus.uncertain,
    ]) {
      testWidgets('外观 lease ${replace ? "替换" : "撤销"} 后迟到 $status 不回撤当前宿主预览', (
        tester,
      ) async {
        final s = Surface();
        final pending =
            Completer<DesktopResourceWriteResult<DesktopAppearanceValue>>();
        s.appearance.pending = pending;
        await tester.pumpWidget(s.widget());
        await tester.pumpAndSettle();
        await editTransparency(tester, '40.5');
        await activate(tester, 'save-resource-appearance');
        if (replace) {
          s.replaceBoundary();
          await tester.pumpWidget(s.widget());
          await tester.pumpAndSettle();
        } else {
          s.boundary.revoke();
          await tester.pump();
          await activate(tester, 'retry-resource-boundary');
        }
        final restores = s.appearance.restores;
        const currentPreview = DesktopAppearanceValue(
          acrylic: true,
          opacity: .7,
        );
        s.appearance.callbacks().preview(currentPreview);
        pending.complete(
          status == DesktopResourceWriteStatus.failed
              ? const DesktopResourceWriteResult.failed()
              : const DesktopResourceWriteResult.uncertain(),
        );
        await tester.pumpAndSettle();
        expect(s.appearance.restores, restores);
        expect(s.appearance.previews.last, same(currentPreview));
        expect(
          tester
              .widget<TextBox>(
                find.byKey(const ValueKey('resource-transparency')),
              )
              .controller!
              .text,
          '40.5',
        );
        expect(s.resourceKey.currentState!.hasUnsavedChanges, isTrue);
        expect(
          tester
              .widget<TextBox>(
                find.byKey(const ValueKey('resource-transparency')),
              )
              .enabled,
          isTrue,
        );
        if (status == DesktopResourceWriteStatus.uncertain) {
          expect(
            tester
                .widget<FilledButton>(
                  find.byKey(const ValueKey('save-resource-appearance')),
                )
                .onPressed,
            isNull,
          );
          s.appearance.state.value = DesktopAppearanceState(
            saved: s.appearance.state.value.saved,
            materialAvailable: true,
            verificationVersion: 1,
          );
          await tester.pump();
        }
        expect(
          tester
              .widget<FilledButton>(
                find.byKey(const ValueKey('save-resource-appearance')),
              )
              .onPressed,
          isNotNull,
        );
        s.appearance.pending = null;
        await activate(tester, 'save-resource-appearance');
        expect(s.appearance.saves, hasLength(2));
        expect(s.appearance.state.value.saved.opacity, closeTo(.595, 1e-9));
        await s.close(tester);
      });
    }
  }

  for (final width in [375.0, 768.0, 1280.0]) {
    testWidgets('$width / textScale2 / highContrast / reducedMotion 无溢出且可操作', (
      tester,
    ) async {
      tester.view.resetPhysicalSize();
      await tester.binding.setSurfaceSize(Size(width, 900));
      final s = Surface();
      await tester.pumpWidget(s.widget(reduceMotion: false));
      await tester.pumpAndSettle();
      expect(tester.takeException(), isNull);
      expect(
        tester
            .getSize(find.byKey(const ValueKey('interface-resource-preview')))
            .width,
        lessThanOrEqualTo(DesktopMetrics.contentWidth),
      );
      await tester.pumpWidget(s.widget(scale: 2, highContrast: true));
      await tester.pumpAndSettle();
      expect(tester.takeException(), isNull);
      await activate(tester, 'pick-interface-resource');
      await activate(tester, 'apply-interface-resource');
      await activate(tester, 'pick-terminal-resource');
      await editTransparency(tester, '65');
      await activate(tester, 'save-resource-appearance');
      await activate(tester, 'open-resource-licenses');
      expect(tester.takeException(), isNull);
      await activate(tester, 'return-resource-licenses');
      expect(tester.takeException(), isNull);
      await s.close(tester);
      await tester.binding.setSurfaceSize(null);
    });
  }
}
