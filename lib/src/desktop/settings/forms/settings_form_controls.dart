import 'package:fluent_ui/fluent_ui.dart';

import '../../../bindings/bindings.dart';
import '../../desktop_theme.dart';

const desktopColorLabels = <String, String>{
  'default': '默认',
  'one_dark': '单一深色',
  'solarized_dark': '日晒深色',
  'solarized_light': '日晒浅色',
  'dracula': '德古拉',
  'monokai': '莫诺凯',
  'nord': '北欧',
  'gruvbox_dark': '复古深色',
  'tokyo_night': '东京之夜',
  'campbell_powershell': '坎贝尔 PowerShell',
};

String settingsErrorText(DesktopSettingsFieldError error) =>
    switch (error.code) {
      'invalid_number' => '请输入可编码的有限数字',
      'invalid_key' => '按键无效，请使用单个字符或支持的命名键',
      'invalid_action' => '请选择支持的动作',
      'duplicate_chord' => '按键组合重复',
      'out_of_range' => '数值超出允许范围',
      'referenced_profile' => '此配置仍被引用，不能删除',
      _ => '${error.field}：${error.code}',
    };

class SettingsTextField extends StatefulWidget {
  const SettingsTextField({
    super.key,
    required this.label,
    required this.value,
    required this.onChanged,
    this.errors = const [],
    this.description,
    this.autofocus = false,
    this.fieldKey,
  });
  final String label;
  final String value;
  final ValueChanged<String>? onChanged;
  final List<DesktopSettingsFieldError> errors;
  final String? description;
  final bool autofocus;

  /// 输入框自身的 key；外层 key 由表单用于定位错误。
  final Key? fieldKey;

  @override
  State<SettingsTextField> createState() => SettingsTextFieldState();
}

class SettingsTextFieldState extends State<SettingsTextField> {
  late final _text = TextEditingController(text: widget.value);
  final _focus = FocusNode();

  void focus() {
    _focus.requestFocus();
    Scrollable.ensureVisible(context);
  }

  @override
  void didUpdateWidget(SettingsTextField oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.value != oldWidget.value && widget.value != _text.text) {
      _text.value = TextEditingValue(
        text: widget.value,
        selection: TextSelection.collapsed(offset: widget.value.length),
      );
    }
  }

  @override
  void dispose() {
    _text.dispose();
    _focus.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => InfoLabel(
    label: widget.label,
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        TextBox(
          key: widget.fieldKey,
          controller: _text,
          focusNode: _focus,
          autofocus: widget.autofocus,
          enabled: widget.onChanged != null,
          onChanged: widget.onChanged,
          placeholder: widget.description,
        ),
        if (widget.description != null) Text(widget.description!),
        for (final error in widget.errors)
          Semantics(liveRegion: true, child: Text(settingsErrorText(error))),
      ],
    ),
  );
}

class SettingsChoice extends StatefulWidget {
  const SettingsChoice({
    super.key,
    required this.label,
    required this.value,
    required this.options,
    required this.onChanged,
    this.errors = const [],
    this.disabledOptions = const {},
  });
  final String label;
  final String value;
  final Map<String, String> options;
  final Set<String> disabledOptions;
  final ValueChanged<String>? onChanged;
  final List<DesktopSettingsFieldError> errors;

  @override
  State<SettingsChoice> createState() => _SettingsChoiceState();
}

class _SettingsChoiceState extends State<SettingsChoice> {
  final _focus = FocusNode();

  @override
  void dispose() {
    _focus.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => InfoLabel(
    label: widget.label,
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        ComboBox<String>(
          focusNode: _focus,
          isExpanded: true,
          value: widget.options.containsKey(widget.value) ? widget.value : null,
          placeholder: Text('保留当前值：${widget.value}'),
          items: [
            for (final entry in widget.options.entries)
              ComboBoxItem(
                value: entry.key,
                enabled: !widget.disabledOptions.contains(entry.key),
                child: Text(entry.value),
              ),
          ],
          onChanged: widget.onChanged == null
              ? null
              : (v) {
                  if (v != null) widget.onChanged!(v);
                },
        ),
        for (final error in widget.errors)
          Semantics(liveRegion: true, child: Text(settingsErrorText(error))),
        if (widget.errors.isNotEmpty)
          Button(
            onPressed: () {
              _focus.requestFocus();
              Scrollable.ensureVisible(context);
            },
            child: const Text('定位错误'),
          ),
      ],
    ),
  );
}

class SettingsFields extends StatelessWidget {
  const SettingsFields({super.key, required this.children});
  final List<Widget> children;

  @override
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.stretch,
    children: [
      for (final child in children)
        Padding(
          padding: const EdgeInsets.only(bottom: DesktopSpacing.lg),
          child: child,
        ),
    ],
  );
}
