import 'package:fluent_ui/fluent_ui.dart' as f;
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/desktop/desktop_app_color_scheme.dart';
import 'package:guosh_shell/src/desktop/desktop_theme.dart';
import 'package:guosh_shell/src/desktop/settings/desktop_settings_controller.dart';
import 'package:guosh_shell/src/desktop/windows_chrome.dart';

import 'desktop_settings_gateway_test.dart';

DesktopConfiguration _withScheme(String scheme, {int revision = 7}) =>
    settingsSnapshot(
      revision: revision,
      app: DesktopAppSettings(
        defaultTerminalProfile: settingsProfileA,
        colorScheme: scheme,
        keyBindings: const [settingsBinding],
      ),
    );

double _contrast(Color a, Color b) {
  final la = a.computeLuminance();
  final lb = b.computeLuminance();
  return la > lb ? (la + 0.05) / (lb + 0.05) : (lb + 0.05) / (la + 0.05);
}

void main() {
  test('启动查询以固定 owner 发出；任何 owner 的成功快照都刷新应用配色', () async {
    final fixture = DesktopSettingsFixture();
    final scheme = DesktopAppColorScheme();
    expect(scheme.value, 'default');
    scheme.connect(fixture.gateway);
    final query = fixture.settings.single;
    expect(query.operation, 'query');
    expect(query.requestId, startsWith('desktop-app-color-scheme-'));
    fixture.emit(settingsReply(query.requestId, snapshot: _withScheme('nord')));
    expect(scheme.value, 'nord');

    // 设置页 controller 的结果同样更新 gateway 快照，壳无需重启。
    final controller = DesktopSettingsController(gateway: fixture.gateway);
    final id = controller.refresh();
    fixture.emit(
      settingsReply(id, snapshot: _withScheme('dracula', revision: 8)),
    );
    expect(scheme.value, 'dracula');

    // 未知值不让主题构建失败。
    final again = controller.refresh();
    fixture.emit(
      settingsReply(again, snapshot: _withScheme('unknown', revision: 9)),
    );
    expect(scheme.value, 'default');
    controller.dispose();
    scheme.dispose();
    await fixture.close();
  });

  test('配色派生：default 不派生；强调色取首个对比度 ≥3:1 的 ANSI 色', () {
    expect(desktopAppPalette('default'), isNull);
    expect(desktopAppPalette('unknown'), isNull);
    final dracula = desktopAppPalette('dracula')!;
    expect(dracula.canvas, const Color(0xFF282A36));
    expect(dracula.text, const Color(0xFFF8F8F2));
    expect(dracula.accent, const Color(0xFF8BE9FD));
    expect(dracula.brightness, Brightness.dark);
    expect(dracula.surface, isNot(dracula.canvas));
    // 日晒浅色的 cyan 不足 3:1，改取 blue；浅色画布用浅色主题。
    final light = desktopAppPalette('solarized_light')!;
    expect(light.accent, const Color(0xFF268BD2));
    expect(light.brightness, Brightness.light);
    for (final name in settingsCapabilities.colorSchemes.skip(1)) {
      final palette = desktopAppPalette(name)!;
      expect(
        palette.accent == DesktopColors.accent ||
            _contrast(palette.accent, palette.canvas) >= 3,
        isTrue,
        reason: name,
      );
    }
  });

  test('desktopFluentTheme：default 保持原 token，其他配色派生，高对比度不受影响', () {
    final base = desktopFluentTheme();
    final same = desktopFluentTheme(colorScheme: 'default');
    expect(same.scaffoldBackgroundColor, base.scaffoldBackgroundColor);
    expect(same.accentColor.normal, f.Colors.blue.normal);
    expect(same.micaBackgroundColor, base.micaBackgroundColor);
    expect(desktopCanvas('default'), desktopBackground);

    final dracula = desktopAppPalette('dracula')!;
    final themed = desktopFluentTheme(colorScheme: 'dracula');
    expect(themed.scaffoldBackgroundColor, dracula.surface);
    expect(themed.micaBackgroundColor, dracula.canvas);
    expect(themed.accentColor.normal, dracula.accent);
    expect(themed.typography.body?.color, dracula.text);
    expect(desktopCanvas('dracula'), dracula.canvas);
    expect(
      desktopFluentTheme(colorScheme: 'solarized_light').brightness,
      Brightness.light,
    );

    final contrast = desktopFluentTheme(
      highContrast: true,
      colorScheme: 'dracula',
    );
    expect(contrast.scaffoldBackgroundColor, DesktopColors.highContrastCanvas);
    expect(
      contrast.accentColor.normal,
      desktopFluentTheme(highContrast: true).accentColor.normal,
    );
  });
}
