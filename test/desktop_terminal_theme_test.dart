import 'package:flutter/painting.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/terminal/desktop_terminal_theme.dart';
import 'package:guosh_shell/src/terminal/frame.dart';
import 'package:guosh_shell/src/terminal/frame_terminal.dart';
// 验证帧适配器到实际 renderer 的颜色解析边界，不复制解析算法。
// ignore: implementation_imports
import 'package:terminal_view/src/ui/painter.dart';
import 'package:terminal_view/terminal_view.dart';

const _schemes = [
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
];

List<Color> _ansi(TerminalTheme theme) => [
  theme.black,
  theme.red,
  theme.green,
  theme.yellow,
  theme.blue,
  theme.magenta,
  theme.cyan,
  theme.white,
  theme.brightBlack,
  theme.brightRed,
  theme.brightGreen,
  theme.brightYellow,
  theme.brightBlue,
  theme.brightMagenta,
  theme.brightCyan,
  theme.brightWhite,
];

double _contrast(Color a, Color b) {
  final luminances = [a.computeLuminance(), b.computeLuminance()]..sort();
  return (luminances.last + 0.05) / (luminances.first + 0.05);
}

TerminalPainter _painter(TerminalTheme theme) => TerminalPainter(
  theme: theme,
  textStyle: const TerminalStyle(),
  textScaler: TextScaler.noScaling,
);

TerminalFrame _colorFrame() {
  const colors = <TermColor>[
    DefaultColor(),
    AnsiColor(0),
    AnsiColor(1),
    AnsiColor(2),
    AnsiColor(3),
    AnsiColor(4),
    AnsiColor(5),
    AnsiColor(6),
    AnsiColor(7),
    AnsiColor(8),
    AnsiColor(9),
    AnsiColor(10),
    AnsiColor(11),
    AnsiColor(12),
    AnsiColor(13),
    AnsiColor(14),
    AnsiColor(15),
    RgbColor(19, 87, 143),
    AnsiColor(196),
  ];
  return TerminalFrame(
    cols: colors.length,
    rows: 1,
    cursorCol: -1,
    cursorRow: -1,
    firstStableRow: 0,
    screenTopStableRow: 0,
    lines: [
      FrameRow(
        stableRow: 0,
        wrapped: false,
        runs: [
          for (var i = 0; i < colors.length; i++)
            FrameRun(
              start: i,
              len: 1,
              attrs: 0,
              fg: colors[i],
              bg: colors[i],
              text: 'x',
            ),
        ],
      ),
    ],
  );
}

void main() {
  test('default 保留原 preset 身份', () {
    expect(themeForProfile('default'), same(TerminalThemes.defaultTheme));
  });

  test('十个实际 profile ID 可选择，命名资源的默认前背景组合各自独立', () {
    final themes = _schemes.map(themeForProfile).toList();
    expect(themes.toSet(), hasLength(_schemes.length));
    expect(
      themes.map((theme) => (theme.foreground, theme.background)).toSet(),
      hasLength(_schemes.length),
    );
    for (var i = 0; i < _schemes.length; i++) {
      expect(themeForProfile(_schemes[i]), same(themes[i]));
      for (final color in [
        themes[i].foreground,
        themes[i].background,
        ..._ansi(themes[i]),
      ]) {
        expect(color.a, 1, reason: _schemes[i]);
      }
    }
  });

  test('非法名称明确失败并保留原值，不伪装成 default', () {
    for (final scheme in ['', 'unknown', 'OneDark', 'one-dark', ' default ']) {
      expect(
        () => themeForProfile(scheme),
        throwsA(
          isA<ArgumentError>().having((e) => e.invalidValue, '原值', scheme),
        ),
      );
    }
  });

  for (final scheme in _schemes.skip(1)) {
    test('$scheme 默认正文、选区与搜索文字达到 AA 对比度', () {
      final theme = themeForProfile(scheme);
      for (final (fg, bg) in [
        (theme.foreground, theme.background),
        (theme.selectionForeground!, theme.selection),
        (theme.searchHitForeground, theme.searchHitBackground),
        (theme.searchHitForeground, theme.searchHitBackgroundCurrent),
      ]) {
        expect(_contrast(fg, bg), greaterThanOrEqualTo(4.5));
      }
      expect(
        _contrast(theme.cursor, theme.background),
        greaterThanOrEqualTo(3),
      );
      expect(theme.cursorAccent, theme.background);
    });
  }

  test('brightBlack 与正文可区分（灰显的自动建议依赖它）', () {
    for (final scheme in _schemes) {
      final theme = themeForProfile(scheme);
      expect(
        _contrast(theme.foreground, theme.brightBlack),
        greaterThanOrEqualTo(2),
        reason: scheme,
      );
    }
  });

  test('同一真实帧的 Default 与 ANSI16 跟随 profile，RGB 与扩展 ANSI 不变', () {
    final terminal = FrameTerminal()..applyFrame(_colorFrame());
    addTearDown(terminal.dispose);
    final line = terminal.lineAt(0);
    final painter = _painter(themeForProfile('default'));
    addTearDown(painter.dispose);

    for (final scheme in _schemes) {
      final theme = themeForProfile(scheme);
      painter.theme = theme;
      expect(
        painter.resolveForegroundColor(line.getForeground(0)),
        theme.foreground,
      );
      expect(
        painter.resolveBackgroundColor(line.getBackground(0)),
        theme.background,
      );
      final ansi = _ansi(theme);
      for (var i = 0; i < ansi.length; i++) {
        expect(
          painter.resolveForegroundColor(line.getForeground(i + 1)),
          ansi[i],
        );
        expect(
          painter.resolveBackgroundColor(line.getBackground(i + 1)),
          ansi[i],
        );
      }
      for (var i = 0; i < 8; i++) {
        expect(
          painter.resolveForegroundColor(line.getForeground(i + 1), bold: true),
          ansi[i + 8],
        );
      }
      expect(
        painter.resolveForegroundColor(line.getForeground(17)),
        const Color(0xFF13578F),
      );
      expect(
        painter.resolveBackgroundColor(line.getBackground(17)),
        const Color(0xFF13578F),
      );
      expect(
        painter.resolveForegroundColor(line.getForeground(18)),
        const Color(0xFFFF0000),
      );
      expect(
        painter.resolveBackgroundColor(line.getBackground(18)),
        const Color(0xFFFF0000),
      );
      expect(terminal.lineAt(0), same(line));
    }
  });

  test('不同 pane 解析同一帧时各持 theme，不被另一 profile 的选择覆盖', () {
    final first = FrameTerminal()..applyFrame(_colorFrame());
    final second = FrameTerminal()..applyFrame(_colorFrame());
    final dark = themeForProfile('one_dark');
    final light = themeForProfile('solarized_light');
    final firstPainter = _painter(dark);
    final secondPainter = _painter(light);
    addTearDown(() {
      firstPainter.dispose();
      secondPainter.dispose();
      first.dispose();
      second.dispose();
    });
    expect(
      firstPainter.resolveForegroundColor(first.lineAt(0).getForeground(0)),
      dark.foreground,
    );
    expect(
      secondPainter.resolveForegroundColor(second.lineAt(0).getForeground(0)),
      light.foreground,
    );
    secondPainter.theme = themeForProfile('nord');
    expect(
      firstPainter.resolveBackgroundColor(first.lineAt(0).getBackground(0)),
      dark.background,
    );
    expect(
      secondPainter.resolveBackgroundColor(second.lineAt(0).getBackground(0)),
      themeForProfile('nord').background,
    );
  });
}
