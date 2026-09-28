import 'dart:async';

import 'package:flutter/material.dart';

import '../bindings/bindings.dart';
import '../keys/keys_page.dart';
import 'key_bar_editor.dart';
import '../terminal/key_bar_layout.dart';
import 'interface_font_setting.dart';

/// 字体与键位条即时生效；默认字号和滚回行数用于新会话。
class SettingsPage extends StatefulWidget {
  final ValueChanged<SaveSettings>? saveSettings;
  final VoidCallback? querySettings;

  const SettingsPage({super.key, this.saveSettings, this.querySettings});

  @override
  State<SettingsPage> createState() => _SettingsPageState();
}

class _SettingsPageState extends State<SettingsPage> {
  StreamSubscription? _settingsSub;
  SettingsState? _settings = SettingsState.latestRustSignal?.message;

  /// 拖动中的字号（松手才保存）。
  double? _draggingSize;

  @override
  void initState() {
    super.initState();
    _settingsSub = SettingsState.rustSignalStream.listen((pack) {
      if (mounted) setState(() => _settings = pack.message);
    });
    if (widget.querySettings case final query?) {
      query();
    } else {
      SettingsQuery().sendSignalToRust();
    }
  }

  @override
  void dispose() {
    _settingsSub?.cancel();
    super.dispose();
  }

  void _save({double? fontSize, int? scrollbackLines, bool? showKeyBar}) {
    if (_settings == null) return;
    // 回包只用于显示，不用旧快照补齐未修改字段。
    final request = SaveSettings(
      fontSize: fontSize,
      scrollbackLines: scrollbackLines,
      showKeyBar: showKeyBar,
    );
    if (widget.saveSettings case final save?) {
      save(request);
    } else {
      request.sendSignalToRust();
    }
  }

  /// 滚回行数的可选档位，不超过本机上界（上界本身也是一档）。
  static List<int> _scrollbackChoices(int max) => [
    for (final lines in const [1000, 2000, 5000, 10000, 20000, 50000, 100000])
      if (lines < max) lines,
    max,
  ];

  @override
  Widget build(BuildContext context) {
    final settings = _settings;
    return Scaffold(
      appBar: AppBar(title: const Text('设置')),
      body: settings == null
          ? const Center(child: CircularProgressIndicator())
          : _buildBody(context, settings),
    );
  }

  Widget _buildBody(BuildContext context, SettingsState settings) {
    final scheme = Theme.of(context).colorScheme;
    final size = _draggingSize ?? settings.fontSize;
    return SafeArea(
      child: ListView(
        padding: const EdgeInsets.symmetric(vertical: 8),
        children: [
          const InterfaceFontSetting(),
          ListTile(
            leading: const Icon(Icons.key),
            title: const Text('私钥'),
            subtitle: const Text('导入私钥、复制公钥、iCloud 钥匙串同步'),
            trailing: const Icon(Icons.chevron_right),
            onTap: () => Navigator.of(
              context,
            ).push(MaterialPageRoute<void>(builder: (_) => const KeysPage())),
          ),
          InterfaceFontSetting(terminal: true, previewFontSize: size),
          const _SectionTitle('字号'),
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 16),
            child: Row(
              children: [
                Expanded(
                  child: Slider(
                    min: settings.minFontSize,
                    max: settings.maxFontSize,
                    divisions: (settings.maxFontSize - settings.minFontSize)
                        .round(),
                    value: size.clamp(
                      settings.minFontSize,
                      settings.maxFontSize,
                    ),
                    label: size.round().toString(),
                    onChanged: (value) => setState(() => _draggingSize = value),
                    onChangeEnd: (value) {
                      setState(() => _draggingSize = null);
                      _save(fontSize: value.roundToDouble());
                    },
                  ),
                ),
                SizedBox(width: 32, child: Text('${size.round()}')),
              ],
            ),
          ),
          const _SectionTitle('键盘'),
          SwitchListTile(
            title: const Text('显示键位条'),
            subtitle: const Text('终端下方的 Esc、Tab、方向键、Ctrl、Alt 与复制粘贴'),
            value: settings.showKeyBar,
            onChanged: (value) => _save(showKeyBar: value),
          ),
          ListTile(
            leading: const Icon(Icons.tune),
            title: const Text('编辑功能按钮'),
            subtitle: const Text('排序、增删、移动到另一排或添加自定义文本'),
            trailing: const Icon(Icons.chevron_right),
            onTap: () => Navigator.of(context).push(
              MaterialPageRoute<void>(
                builder: (_) => KeyBarEditor(
                  initialRows: settings.keyBarRows,
                  onSave: saveKeyBarLayout,
                ),
              ),
            ),
          ),
          const _SectionTitle('滚回'),
          ListTile(
            title: const Text('每个会话保留的行数'),
            subtitle: Text('本机最多 ${settings.maxScrollbackLines} 行（按设备内存）'),
            trailing: DropdownButton<int>(
              value: settings.scrollbackLines,
              items: [
                for (final lines in _scrollbackChoices(
                  settings.maxScrollbackLines,
                ))
                  DropdownMenuItem(value: lines, child: Text('$lines')),
                if (!_scrollbackChoices(settings.maxScrollbackLines)
                    .contains(settings.scrollbackLines))
                  DropdownMenuItem(
                    value: settings.scrollbackLines,
                    child: Text('${settings.scrollbackLines}'),
                  ),
              ],
              onChanged: (lines) {
                if (lines != null) _save(scrollbackLines: lines);
              },
            ),
          ),
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 16),
            child: Text(
              '字体与按钮排布立即生效；默认字号和滚回设置用于新会话。',
              style: TextStyle(color: scheme.onSurfaceVariant, fontSize: 13),
            ),
          ),
          const Divider(height: 32),
          ListTile(
            leading: const Icon(Icons.info_outline),
            title: const Text('关于与开源许可'),
            trailing: const Icon(Icons.chevron_right),
            onTap: () => showLicensePage(
              context: context,
              applicationName: 'GuoSSHell',
              applicationLegalese: 'SSH 终端客户端。终端与 SSH 内核基于 rsHell（MIT 许可）。',
            ),
          ),
        ],
      ),
    );
  }
}

class _SectionTitle extends StatelessWidget {
  final String text;
  const _SectionTitle(this.text);

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 16, 16, 4),
      child: Text(
        text,
        style: Theme.of(context).textTheme.titleSmall
            ?.copyWith(color: Theme.of(context).colorScheme.primary),
      ),
    );
  }
}
