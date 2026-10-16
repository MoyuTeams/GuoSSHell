import 'package:fluent_ui/fluent_ui.dart';

import '../../../bindings/bindings.dart';
import '../../desktop_theme.dart';
import '../desktop_settings_draft.dart';
import 'settings_form_controls.dart';

const desktopActionLabels = <String, String>{
  'send:\u001b[3~': '发送 VT220 Delete',
  'send:\u007f': '发送 DEL',
  'send:\u0008': '发送退格 BS',
  'clear_scrollback': '清空滚回',
  'new_tab': '新建标签页',
  'split_vertical': '垂直分屏',
};

const desktopNamedKeys = <String, String>{
  'Enter': '回车',
  'Escape': '退出',
  'Tab': '制表',
  'Backspace': '退格',
  'Delete': '删除',
  'Insert': '插入',
  'Home': '行首',
  'End': '行尾',
  'PageUp': '上一页',
  'PageDown': '下一页',
  'ArrowUp': '上方向',
  'ArrowDown': '下方向',
  'ArrowLeft': '左方向',
  'ArrowRight': '右方向',
};

bool desktopSameChord(DesktopKeyBinding a, DesktopKeyBinding b) =>
    a.key == b.key &&
    a.shift == b.shift &&
    a.control == b.control &&
    a.alt == b.alt &&
    a.superKey == b.superKey;

class DesktopSettingsBindingsEditor extends StatefulWidget {
  const DesktopSettingsBindingsEditor({
    super.key,
    required this.rows,
    required this.actions,
    required this.onChanged,
    this.errors,
    this.appBindings = const [],
  });
  final List<DesktopBindingRow> rows;
  final List<String> actions;
  final ValueChanged<List<DesktopBindingRow>>? onChanged;
  final DesktopDraftErrors? errors;
  final List<DesktopKeyBinding> appBindings;

  @override
  State<DesktopSettingsBindingsEditor> createState() => _BindingsEditorState();
}

class _BindingsEditorState extends State<DesktopSettingsBindingsEditor> {
  final _fields = <String, GlobalKey<SettingsTextFieldState>>{};
  final _modifierFocus = <String, FocusNode>{};

  @override
  void dispose() {
    for (final node in _modifierFocus.values) {
      node.dispose();
    }
    super.dispose();
  }

  void _edit(DesktopBindingRow row, DesktopKeyBinding value) =>
      widget.onChanged?.call([
        for (final item in widget.rows)
          if (item.id == row.id) row.edit(value) else item,
      ]);

  void _move(int index, int delta) {
    final rows = [...widget.rows];
    final row = rows.removeAt(index);
    rows.insert(index + delta, row);
    widget.onChanged?.call(rows);
  }

  @override
  Widget build(BuildContext context) {
    final actions = {
      for (final action in widget.actions)
        if (desktopActionLabels.containsKey(action))
          action: desktopActionLabels[action]!,
    };
    return FocusTraversalGroup(
      child: SettingsFields(
        children: [
          if (widget.rows.isEmpty) const Text('尚无自定义绑定；未命中时使用默认操作。'),
          for (var index = 0; index < widget.rows.length; index++)
            _row(widget.rows[index], index, actions),
          Button(
            key: const ValueKey('binding-add'),
            onPressed: widget.onChanged == null || actions.isEmpty
                ? null
                : () {
                    widget.onChanged!([
                      ...widget.rows,
                      DesktopBindingRow(
                        DesktopKeyBinding(
                          key: '',
                          shift: false,
                          control: false,
                          alt: false,
                          superKey: false,
                          action: actions.keys.first,
                        ),
                      ),
                    ]);
                  },
            child: const Text('新增绑定'),
          ),
        ],
      ),
    );
  }

  Widget _row(DesktopBindingRow row, int index, Map<String, String> actions) {
    final value = row.value;
    final enabled = widget.onChanged != null;
    final errors = widget.errors?.byBindingRow[row.id] ?? const [];
    final keyErrors = errors
        .where((e) => e.field.endsWith('.key') || e.field.endsWith('.code'))
        .toList();
    final actionErrors = errors
        .where((e) => e.field.endsWith('.action'))
        .toList();
    final modifierErrors = errors
        .where((e) => e.field.endsWith('.modifiers'))
        .toList();
    final field = _fields.putIfAbsent(
      row.id,
      GlobalKey<SettingsTextFieldState>.new,
    );
    return Container(
      key: ValueKey(row.id),
      padding: const EdgeInsets.all(DesktopSpacing.md),
      decoration: BoxDecoration(
        border: Border.all(
          color: FluentTheme.of(context).resources.controlStrokeColorDefault,
          width: DesktopMetrics.border,
        ),
        borderRadius: BorderRadius.circular(DesktopMetrics.radius),
      ),
      child: SettingsFields(
        children: [
          Text('绑定 ${index + 1}'),
          if (widget.appBindings.any((a) => desktopSameChord(a, value)))
            const Text('此配置覆盖应用中的相同按键组合'),
          SettingsTextField(
            key: field,
            label: '按键（单个 Unicode 字符、命名键或 F1–F24）',
            value: value.key,
            errors: keyErrors,
            onChanged: enabled
                ? (v) => _edit(row, value.copyWith(key: v))
                : null,
          ),
          SettingsChoice(
            key: ValueKey('binding-key-${row.id}'),
            label: '选择命名键',
            value: value.key,
            options: {
              ...desktopNamedKeys,
              for (var i = 1; i <= 24; i++) 'F$i': 'F$i',
            },
            onChanged: enabled
                ? (v) => _edit(row, value.copyWith(key: v))
                : null,
          ),
          Wrap(
            spacing: DesktopSpacing.lg,
            runSpacing: DesktopSpacing.md,
            children: [
              Checkbox(
                key: ValueKey('shift-${row.id}'),
                focusNode: _modifierFocus.putIfAbsent(row.id, FocusNode.new),
                content: const Text('Shift'),
                checked: value.shift,
                onChanged: enabled
                    ? (v) => _edit(row, value.copyWith(shift: v ?? false))
                    : null,
              ),
              Checkbox(
                key: ValueKey('control-${row.id}'),
                content: const Text('Control'),
                checked: value.control,
                onChanged: enabled
                    ? (v) => _edit(row, value.copyWith(control: v ?? false))
                    : null,
              ),
              Checkbox(
                key: ValueKey('alt-${row.id}'),
                content: const Text('Alt'),
                checked: value.alt,
                onChanged: enabled
                    ? (v) => _edit(row, value.copyWith(alt: v ?? false))
                    : null,
              ),
              Checkbox(
                key: ValueKey('super-${row.id}'),
                content: const Text('Super'),
                checked: value.superKey,
                onChanged: enabled
                    ? (v) => _edit(row, value.copyWith(superKey: v ?? false))
                    : null,
              ),
            ],
          ),
          SettingsChoice(
            key: ValueKey('binding-action-${row.id}'),
            errors: actionErrors,
            label: '动作',
            value: value.action,
            options: actions,
            onChanged: enabled
                ? (v) => _edit(row, value.copyWith(action: v))
                : null,
          ),
          Wrap(
            spacing: DesktopSpacing.sm,
            runSpacing: DesktopSpacing.sm,
            children: [
              Button(
                key: ValueKey('binding-up-${row.id}'),
                onPressed: enabled && index > 0 ? () => _move(index, -1) : null,
                child: const Text('上移'),
              ),
              Button(
                key: ValueKey('binding-down-${row.id}'),
                onPressed: enabled && index < widget.rows.length - 1
                    ? () => _move(index, 1)
                    : null,
                child: const Text('下移'),
              ),
              Button(
                key: ValueKey('binding-delete-${row.id}'),
                onPressed: enabled
                    ? () => widget.onChanged!([
                        for (final item in widget.rows)
                          if (item.id != row.id) item,
                      ])
                    : null,
                child: const Text('删除绑定'),
              ),
              if (keyErrors.isNotEmpty)
                Button(
                  onPressed: () => field.currentState?.focus(),
                  child: const Text('定位错误'),
                ),
              if (modifierErrors.isNotEmpty)
                Button(
                  onPressed: () {
                    _modifierFocus[row.id]!.requestFocus();
                    Scrollable.ensureVisible(_modifierFocus[row.id]!.context!);
                  },
                  child: const Text('定位修饰符错误'),
                ),
            ],
          ),
          for (final error in modifierErrors)
            Semantics(liveRegion: true, child: Text(settingsErrorText(error))),
        ],
      ),
    );
  }
}
