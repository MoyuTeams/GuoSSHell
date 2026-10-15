import 'dart:async';

import 'package:file_selector/file_selector.dart';
import 'package:fluent_ui/fluent_ui.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/desktop/desktop_theme.dart';
import 'package:guosh_shell/src/desktop/settings/desktop_settings_controller.dart';
import 'package:guosh_shell/src/desktop/settings/desktop_settings_page.dart';
import 'package:guosh_shell/src/desktop/settings/keybar/desktop_keybar.dart';
import 'package:guosh_shell/src/desktop/settings/resources/desktop_resources_contracts.dart';
import 'package:guosh_shell/src/desktop/windows_chrome.dart';
import 'package:guosh_shell/src/settings/interface_font.dart';
import 'package:guosh_shell/src/terminal/key_bar_layout.dart';

import 'desktop_settings_gateway_test.dart';

class _KeyBarTransport implements KeyBarTransport {
  final stream = StreamController<KeyBarLayoutResult>.broadcast(sync: true);
  final sent = <List<List<String>>>[];
  @override
  Stream<KeyBarLayoutResult> get results => stream.stream;
  @override
  void send(List<List<String>> rows) => sent.add(rows);
}

InterfaceTypography _typography(String slot, {Completer<void>? timeout}) =>
    InterfaceTypography(
      slot: slot,
      transport: (slot, action, name, bytes) async {
        if (timeout != null && action == 'apply') {
          throw TimeoutException('fixture');
        }
        // 超时后的核实 query 由测试决定何时结算。
        if (timeout != null && action == 'query') await timeout.future;
        return FontReply(
          FontFileState(
            requestId: 1,
            slot: slot,
            family: 'GuoshFont_fixture',
            label: 'fixture',
            error: '',
            applied: action != 'preview',
          ),
          bytes,
        );
      },
      register: (_, _) async {},
    );

const _settingsState = SettingsState(
  fontSize: 14,
  minFontSize: 8,
  maxFontSize: 32,
  scrollbackLines: 1000,
  maxScrollbackLines: 100000,
  showKeyBar: false,
  keyBarRows: defaultKeyBarRows,
);

class _Page {
  _Page({this.platform = TargetPlatform.windows, Completer<void>? fontTimeout})
    : interfaceFont = _typography('interface', timeout: fontTimeout),
      terminalFont = _typography('terminal');
  final TargetPlatform platform;
  final fixture = DesktopSettingsFixture();
  late final DesktopSettingsController controller = fixture.controller();
  final InterfaceTypography interfaceFont;
  final InterfaceTypography terminalFont;
  final appearanceStates = StreamController<WindowsAppearanceState>.broadcast(
    sync: true,
  );
  final appearanceSaves = <SaveWindowsAppearance>[];
  late final appearance = WindowsAppearance(
    states: appearanceStates.stream,
    send: appearanceSaves.add,
  );
  final keyBarTransport = _KeyBarTransport();
  late final writer = KeyBarWriter(transport: keyBarTransport);
  final showKeyBar = <bool>[];

  Future<void> open(
    WidgetTester tester, {
    DesktopSettingsSection section = DesktopSettingsSection.app,
  }) async {
    tester.view.devicePixelRatio = 1;
    tester.view.physicalSize = const Size(1280, 900);
    addTearDown(tester.view.reset);
    addTearDown(() => close(tester));
    await tester.pumpWidget(
      FluentApp(
        theme: desktopTheme(highContrast: false, reduceMotion: true),
        home: Builder(
          builder: (context) => Button(
            key: const ValueKey('open-settings'),
            onPressed: () => showDialog<void>(
              context: context,
              dismissWithEsc: false,
              builder: (_) => DesktopSettingsPage(
                controller: controller,
                initialSection: section,
                settings: _settingsState,
                platform: platform,
                interfaceFont: interfaceFont,
                terminalFont: terminalFont,
                appearance: appearance,
                keyBarWriter: writer,
                saveShowKeyBar: showKeyBar.add,
                pickTtf: () async => XFile.fromData(
                  Uint8List(4),
                  name: 'fixture.ttf',
                  path: 'fixture.ttf',
                ),
              ),
            ),
            child: const Text('打开设置'),
          ),
        ),
      ),
    );
    await tester.tap(find.byKey(const ValueKey('open-settings')));
    await tester.pumpAndSettle();
  }

  Future<void> close(WidgetTester tester) async {
    // 先卸载页面并执行外观回撤，再释放注入的服务。
    await tester.pumpWidget(const SizedBox());
    controller.dispose();
    writer.dispose();
    interfaceFont.dispose();
    terminalFont.dispose();
    appearance.dispose();
    await appearanceStates.close();
    await keyBarTransport.stream.close();
    await fixture.close();
  }
}

Finder _key(String key) => find.byKey(ValueKey(key));

Future<void> _tap(WidgetTester tester, String key) async {
  await tester.ensureVisible(_key(key));
  await tester.tap(_key(key));
  await tester.pumpAndSettle();
}

Future<void> _nav(WidgetTester tester, DesktopSettingsSection section) =>
    _tap(tester, 'settings-nav-${section.name}');

WindowsAppearanceState _appearanceState(
  double opacity, {
  required int requestId,
  String error = '',
}) => WindowsAppearanceState(
  requestId: requestId,
  acrylic: true,
  opacity: opacity,
  error: error,
);

void main() {
  testWidgets('左侧导航切换五页，各页内容只在所选页可见', (tester) async {
    final page = _Page();
    await page.open(tester);
    final pages = {
      DesktopSettingsSection.app: 'default_terminal_profile',
      DesktopSettingsSection.profile: 'selected-profile',
      DesktopSettingsSection.appearance: 'pick-interface-resource',
      DesktopSettingsSection.keybar: 'keybar-save',
      DesktopSettingsSection.about: 'open-resource-licenses',
    };
    for (final section in DesktopSettingsSection.values) {
      await _nav(tester, section);
      expect(find.text(section.label), findsWidgets);
      for (final entry in pages.entries) {
        expect(
          _key(entry.value),
          entry.key == section ? findsOneWidget : findsNothing,
          reason: '${section.name} 页',
        );
      }
    }
    await _nav(tester, DesktopSettingsSection.appearance);
    expect(_key('resource-acrylic'), findsOneWidget);
    expect(_key('terminal-resource-preview'), findsOneWidget);
    expect(_key('open-resource-licenses'), findsNothing);
    await _nav(tester, DesktopSettingsSection.profile);
    await _tap(tester, 'profile-default');
    expect(_key('default_terminal_profile'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('非 Windows 外观页只有字体，不显示亚克力与透明度', (tester) async {
    final page = _Page(platform: TargetPlatform.linux);
    await page.open(tester, section: DesktopSettingsSection.appearance);
    expect(_key('pick-terminal-resource'), findsOneWidget);
    expect(_key('resource-acrylic'), findsNothing);
    expect(_key('resource-transparency-slider'), findsNothing);
  });

  testWidgets('关闭按钮与 Esc 共用脏检查，合并资源区与键位条草稿', (tester) async {
    final page = _Page();
    await page.open(tester, section: DesktopSettingsSection.keybar);
    await _tap(tester, 'keybar-palette-tab');
    expect(find.text('排布尚未保存。'), findsOneWidget);
    await _tap(tester, 'close-settings');
    expect(find.text('关闭设置？'), findsOneWidget);
    await tester.tap(find.text('继续编辑'));
    await tester.pumpAndSettle();
    expect(find.byType(DesktopSettingsPage), findsOneWidget);
    await _tap(tester, 'keybar-defaults');
    expect(find.text('排布没有未保存修改。'), findsOneWidget);

    await _nav(tester, DesktopSettingsSection.appearance);
    final slider = tester.widget<Slider>(_key('resource-transparency-slider'));
    slider.onChanged!(40);
    await tester.pumpAndSettle();
    expect(page.appearance.opacity, closeTo(0.6, 0.001));
    await _tap(tester, 'resource-acrylic');
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(find.text('关闭设置？'), findsOneWidget);
    await tester.tap(find.text('明确放弃'));
    await tester.pumpAndSettle();
    expect(find.byType(DesktopSettingsPage), findsNothing);
    // 未保存的外观预览在关闭后回撤到持久值。
    expect(page.appearance.opacity, closeTo(0.78, 0.001));
    expect(page.appearance.acrylicEnabled, isTrue);
    expect(page.appearanceSaves, isEmpty);
  });

  testWidgets('无草稿时关闭不确认；有配置草稿时同样拦截', (tester) async {
    final page = _Page();
    await page.open(tester);
    final original = page.controller.app!.value;
    page.controller.editApp(value: original.copyWith(colorScheme: 'nord'));
    await tester.pumpAndSettle();
    await _nav(tester, DesktopSettingsSection.about);
    await _tap(tester, 'close-settings');
    expect(find.text('关闭设置？'), findsOneWidget);
    await tester.tap(find.text('继续编辑'));
    await tester.pumpAndSettle();
    page.controller.editApp(value: original);
    expect(page.controller.dirty, isFalse);
    await tester.pumpAndSettle();
    await _tap(tester, 'close-settings');
    expect(find.text('关闭设置？'), findsNothing);
    expect(find.byType(DesktopSettingsPage), findsNothing);
  });

  testWidgets('键位条页的显示开关发送 showKeyBar 请求', (tester) async {
    final page = _Page();
    await page.open(tester, section: DesktopSettingsSection.keybar);
    expect(
      tester.widget<ToggleSwitch>(_key('settings-show-keybar')).checked,
      isFalse,
    );
    await _tap(tester, 'settings-show-keybar');
    expect(page.showKeyBar, [true]);
    await _tap(tester, 'keybar-palette-tab');
    await _tap(tester, 'keybar-save');
    expect(page.keyBarTransport.sent.single.first.last, 'tab');
    page.keyBarTransport.stream.add(
      const KeyBarLayoutResult(ok: true, detail: ''),
    );
    await tester.pumpAndSettle();
    expect(find.text('键位条已保存。'), findsOneWidget);
  });

  testWidgets('外观 persist：error→失败，超时→待核实且隔离到迟到回报结算', (tester) async {
    final page = _Page();
    final appearance = page.appearance..connect();
    addTearDown(() => page.close(tester));
    const draft = DesktopAppearanceValue(acrylic: true, opacity: 0.65);

    int lastId() => page.appearanceSaves.last.requestId;

    final ok = appearance.persist(draft);
    expect(appearance.writeReady, isFalse);
    expect(page.appearanceSaves.single.opacity, 0.65);
    expect(lastId(), isNot(WindowsAppearance.queryRequestId));
    page.appearanceStates.add(_appearanceState(0.65, requestId: lastId()));
    final confirmed = await ok;
    expect(confirmed.status, DesktopResourceWriteStatus.confirmed);
    expect(confirmed.saved!.opacity, 0.65);
    expect(appearance.saved.opacity, 0.65);
    expect(appearance.writeReady, isTrue);

    final rejected = appearance.persist(
      const DesktopAppearanceValue(acrylic: true, opacity: 0.5),
    );
    page.appearanceStates.add(
      _appearanceState(0.65, requestId: lastId(), error: 'fixture'),
    );
    expect((await rejected).status, DesktopResourceWriteStatus.failed);
    expect(appearance.saved.opacity, 0.65);
    expect(appearance.writeReady, isTrue);

    final pending = appearance.persist(draft);
    await tester.pump(const Duration(seconds: 11));
    expect((await pending).status, DesktopResourceWriteStatus.uncertain);
    expect(appearance.writeReady, isFalse);
    expect(appearance.verification, 0);
    expect(() => appearance.persist(draft), throwsStateError);
    page.appearanceStates.add(_appearanceState(0.65, requestId: lastId()));
    expect(appearance.writeReady, isTrue);
    expect(appearance.verification, 1);
  });

  testWidgets('外观 persist：启动查询回包先到不当作保存结果，只按 request_id 结算', (tester) async {
    final page = _Page();
    final appearance = page.appearance..connect();
    addTearDown(() => page.close(tester));
    const draft = DesktopAppearanceValue(acrylic: true, opacity: 0.65);

    var settled = false;
    final save = appearance.persist(draft).whenComplete(() => settled = true);
    final id = page.appearanceSaves.single.requestId;
    // 启动查询的旧状态晚于保存请求到达：仍更新显示值，但不结算本次写入。
    page.appearanceStates.add(
      _appearanceState(0.78, requestId: WindowsAppearance.queryRequestId),
    );
    page.appearanceStates.add(_appearanceState(0.5, requestId: id + 1));
    await tester.pump();
    expect(settled, isFalse);
    expect(appearance.writeReady, isFalse);
    page.appearanceStates.add(_appearanceState(0.65, requestId: id));
    final result = await save;
    expect(result.status, DesktopResourceWriteStatus.confirmed);
    expect(result.saved!.opacity, 0.65);
    expect(appearance.writeReady, isTrue);

    // 匹配回包携带错误：结算为失败，不被之前的成功查询掩盖。
    final failed = appearance.persist(draft);
    final failedId = page.appearanceSaves.last.requestId;
    expect(failedId, greaterThan(id));
    page.appearanceStates.add(
      _appearanceState(0.78, requestId: WindowsAppearance.queryRequestId),
    );
    page.appearanceStates.add(
      _appearanceState(0.78, requestId: failedId, error: 'fixture'),
    );
    expect((await failed).status, DesktopResourceWriteStatus.failed);
    expect(appearance.writeReady, isTrue);
  });

  testWidgets('外观 persist 超时后只有匹配 request_id 的迟到回包解除隔离', (tester) async {
    final page = _Page();
    final appearance = page.appearance..connect();
    addTearDown(() => page.close(tester));

    final pending = appearance.persist(
      const DesktopAppearanceValue(acrylic: false, opacity: 0.5),
    );
    final id = page.appearanceSaves.single.requestId;
    await tester.pump(const Duration(seconds: 11));
    expect((await pending).status, DesktopResourceWriteStatus.uncertain);
    expect(appearance.writeReady, isFalse);

    page.appearanceStates.add(
      _appearanceState(0.78, requestId: WindowsAppearance.queryRequestId),
    );
    expect(appearance.saved.opacity, 0.78);
    expect(appearance.writeReady, isFalse);
    expect(appearance.verification, 0);

    page.appearanceStates.add(_appearanceState(0.5, requestId: id));
    expect(appearance.saved.opacity, 0.5);
    expect(appearance.writeReady, isTrue);
    expect(appearance.verification, 1);
  });

  testWidgets('字体写入超时映射为待核实，后续 query 成功才解除暂停', (tester) async {
    final verified = Completer<void>();
    final page = _Page(fontTimeout: verified);
    await page.open(tester, section: DesktopSettingsSection.appearance);
    await _tap(tester, 'pick-interface-resource');
    await _tap(tester, 'apply-interface-resource');
    expect(find.text('字体结果待核实，保留草稿；不能盲目重发。'), findsOneWidget);
    expect(
      tester.widget<Button>(_key('reset-interface-resource')).onPressed,
      isNull,
    );
    verified.complete();
    await tester.pumpAndSettle();
    expect(
      tester.widget<Button>(_key('reset-interface-resource')).onPressed,
      isNotNull,
    );
    expect(
      tester.widget<FilledButton>(_key('apply-interface-resource')).onPressed,
      isNotNull,
    );
  });
}
