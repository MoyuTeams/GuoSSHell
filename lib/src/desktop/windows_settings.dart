import 'dart:async';

import 'package:fluent_ui/fluent_ui.dart' as f;
import 'package:flutter/material.dart';

import '../bindings/bindings.dart';
import 'windows_chrome.dart';
import 'desktop_widgets.dart';
import '../settings/interface_font_setting.dart';

class WindowsSettings extends StatefulWidget {
  const WindowsSettings({super.key});
  @override
  State<WindowsSettings> createState() => _WindowsSettingsState();
}

class _WindowsSettingsState extends State<WindowsSettings> {
  SettingsState? _settings = SettingsState.latestRustSignal?.message;
  StreamSubscription? _subscription;
  double? _fontSize;

  @override
  void initState() {
    super.initState();
    _subscription = SettingsState.rustSignalStream.listen((pack) {
      if (mounted) setState(() => _settings = pack.message);
    });
    SettingsQuery().sendSignalToRust();
  }

  @override
  void dispose() {
    _subscription?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => f.ContentDialog(
    constraints: const BoxConstraints(maxWidth: 740, maxHeight: 760),
    title: const Text('设置'),
    actions: [
      f.FilledButton(
        onPressed: () => Navigator.pop(context),
        child: const Text('完成'),
      ),
    ],
    content: SingleChildScrollView(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          const InterfaceFontSetting(fluent: true),
          const SizedBox(height: 30),
          _heading('窗口外观', '亚克力只作用于背景，文字与控件保持清晰。'),
          AnimatedBuilder(
            animation: WindowsAppearance.instance,
            builder: (context, _) {
              final appearance = WindowsAppearance.instance;
              final transparency = ((1 - appearance.opacity) * 100).round();
              return Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  _row(
                    '亚克力半透明',
                    f.ToggleSwitch(
                      checked: appearance.acrylicEnabled,
                      onChanged: (value) {
                        appearance.preview(enabled: value);
                        appearance.save();
                      },
                    ),
                  ),
                  const SizedBox(height: 18),
                  Row(
                    children: [
                      const Expanded(child: Text('背景透明度')),
                      Text(
                        '$transparency%',
                        style: const TextStyle(color: desktopAccent),
                      ),
                    ],
                  ),
                  f.Slider(
                    value: transparency.toDouble(),
                    min: 0,
                    max: 65,
                    divisions: 65,
                    label: '$transparency%',
                    onChanged: appearance.acrylicEnabled
                        ? (value) => appearance.preview(value: 1 - value / 100)
                        : null,
                    onChangeEnd: appearance.acrylicEnabled
                        ? (_) => appearance.save()
                        : null,
                  ),
                  const Text(
                    '0% 为不透明；调节即时预览，松开后自动保存。高对比度模式使用不透明背景。',
                    style: TextStyle(fontSize: 12, color: desktopMuted),
                  ),
                  if (!appearance.materialAvailable)
                    const Padding(
                      padding: EdgeInsets.only(top: 10),
                      child: Text(
                        '当前环境未启用原生亚克力，使用纯色背景。',
                        style: TextStyle(fontSize: 12, color: desktopMuted),
                      ),
                    ),
                  if (appearance.error case final error?)
                    Padding(
                      padding: const EdgeInsets.only(top: 10),
                      child: f.InfoBar(
                        title: const Text('外观保存失败'),
                        content: Text(error),
                        severity: f.InfoBarSeverity.error,
                      ),
                    ),
                ],
              );
            },
          ),
          const SizedBox(height: 30),
          _heading('终端', '字体切换即时生效，字号和滚回设置用于新建会话。'),
          if (_settings case final settings?) ...[
            InterfaceFontSetting(
              fluent: true,
              terminal: true,
              previewFontSize: _fontSize ?? settings.fontSize,
            ),
            const SizedBox(height: 18),
            Row(
              children: [
                const Expanded(child: Text('字号')),
                Text('${(_fontSize ?? settings.fontSize).round()} px'),
              ],
            ),
            f.Slider(
              value: (_fontSize ?? settings.fontSize).clamp(
                settings.minFontSize,
                settings.maxFontSize,
              ),
              min: settings.minFontSize,
              max: settings.maxFontSize,
              divisions: (settings.maxFontSize - settings.minFontSize).round(),
              onChanged: (value) => setState(() => _fontSize = value),
              onChangeEnd: (value) {
                SaveSettings(fontSize: value.roundToDouble())
                    .sendSignalToRust();
                setState(() => _fontSize = null);
              },
            ),
            _row(
              '滚回行数',
              f.ComboBox<int>(
                value: settings.scrollbackLines,
                items: [
                  for (final value
                      in ({
                            1000,
                            2000,
                            5000,
                            10000,
                            20000,
                            50000,
                            100000,
                            settings.maxScrollbackLines,
                            settings.scrollbackLines,
                          }
                          .where(
                            (value) => value <= settings.maxScrollbackLines,
                          )
                          .toList()
                        ..sort()))
                    f.ComboBoxItem(value: value, child: Text('$value 行')),
                ],
                onChanged: (value) =>
                    SaveSettings(scrollbackLines: value).sendSignalToRust(),
              ),
            ),
            const SizedBox(height: 20),
            _row(
              '显示终端功能键栏',
              f.ToggleSwitch(
                checked: settings.showKeyBar,
                onChanged: (value) =>
                    SaveSettings(showKeyBar: value).sendSignalToRust(),
              ),
            ),
          ] else
            const f.ProgressBar(),
          const SizedBox(height: 30),
          _heading('键盘快捷键', '终端中的 Ctrl+C、Ctrl+D 和 Ctrl+W 保留远端含义。'),
          const Wrap(
            spacing: 24,
            children: [
              ShortcutHint('新建会话', 'Ctrl + Shift + T'),
              ShortcutHint('关闭窗格', 'Ctrl + Shift + W'),
              ShortcutHint('左右分屏', 'Ctrl + Shift + D'),
              ShortcutHint('上下分屏', 'Ctrl + Shift + E'),
              ShortcutHint('切换标签', 'Ctrl + Tab'),
              ShortcutHint('切换窗格', 'Ctrl + Shift + P'),
              ShortcutHint('搜索连接', 'Ctrl + Shift + F'),
              ShortcutHint('终端缩放', 'Ctrl + / − / 0'),
            ],
          ),
          const SizedBox(height: 20),
          f.HyperlinkButton(
            onPressed: () => showLicensePage(
              context: context,
              applicationName: 'GuoSSHell',
              applicationLegalese: 'SSH 内核基于 rsHell · MIT',
            ),
            child: const Text('关于与开源许可'),
          ),
        ],
      ),
    ),
  );

  Widget _heading(String title, String subtitle) => Padding(
    padding: const EdgeInsets.only(bottom: 18),
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          title,
          style: const TextStyle(fontSize: 17, fontWeight: FontWeight.w600),
        ),
        const SizedBox(height: 6),
        Text(
          subtitle,
          style: const TextStyle(color: desktopMuted, fontSize: 12),
        ),
      ],
    ),
  );
  Widget _row(String label, Widget control) => Row(
    children: [
      Expanded(child: Text(label)),
      control,
    ],
  );
}
