import 'package:fluent_ui/fluent_ui.dart';

import '../../../bindings/bindings.dart';
import '../desktop_settings_controller.dart';
import '../desktop_settings_draft.dart';
import 'desktop_settings_bindings_editor.dart';
import 'settings_form_controls.dart';

/// 父适配器负责现有文件选择、Rust preview 校验及字体注册；取消返回 null。
typedef DesktopProfileTtfPicker = Future<DesktopTtfSelection?> Function(
  BuildContext context,
  String profileId,
);

String desktopFontModeLabel(ProfileFontMode mode) => switch (mode) {
  ProfileFontMode.inheritTerminalSlot => '继承全局终端字体槽',
  ProfileFontMode.explicitFamily => '显式字体名称',
  ProfileFontMode.explicitCachedTtf => '显式缓存 TTF',
  ProfileFontMode.legacyUndetermined => '旧版绑定意图（保留原状）',
  ProfileFontMode.uneditable => '字体元数据不可编辑（保留原状）',
};

String _fontFailureLabel(ProfileFontFailure failure) => switch (failure) {
  ProfileFontFailure.missing => '资源缺失',
  ProfileFontFailure.corrupt => '资源损坏',
  ProfileFontFailure.unavailable => '字体注册不可用',
  ProfileFontFailure.invalidResourceReference => '资源引用无效',
  ProfileFontFailure.uneditableMetadata => '字体元数据不可编辑',
};

class DesktopProfileForm extends StatefulWidget {
  const DesktopProfileForm({
    super.key,
    required this.controller,
    required this.profileId,
    required this.onPickTtf,
  });
  final DesktopSettingsController controller;
  final String profileId;
  final DesktopProfileTtfPicker onPickTtf;

  @override
  State<DesktopProfileForm> createState() => _ProfileFormState();
}

class _ProfileFormState extends State<DesktopProfileForm> {
  final _fields = <String, GlobalKey<SettingsTextFieldState>>{};
  bool _picking = false;
  int _pickNonce = 0;
  Object? _fontIssue;
  DesktopSettingsController get c => widget.controller;
  DesktopProfileDraft get d => c.profiles[widget.profileId]!;
  DesktopDraftErrors get errors => c.stateFor(widget.profileId).errors;

  @override
  void didUpdateWidget(DesktopProfileForm oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.controller != c || oldWidget.profileId != widget.profileId) {
      ++_pickNonce;
      _picking = false;
      _fontIssue = null;
    }
  }

  Future<void> _pick() async {
    if (!mounted) return;
    final owner = c;
    final id = widget.profileId;
    final nonce = ++_pickNonce;
    DesktopSettingsResourceToken? token;
    int? version;
    bool current() =>
        mounted &&
        nonce == _pickNonce &&
        widget.controller == owner &&
        widget.profileId == id;
    setState(() {
      _picking = true;
      _fontIssue = null;
    });
    try {
      final draft = owner.profiles[id];
      if (!owner.editable || draft == null || !draft.fontBindingEditable) {
        throw StateError('目标配置的字体不可编辑');
      }
      version = draft.fontSelectionVersion;
      token = owner.beginProfileResource(id);
      final selection = await widget.onPickTtf(context, id);
      final latest = owner.profiles[id];
      if (!current() ||
          !owner.acceptsProfileResource(token) ||
          latest == null ||
          latest.fontSelectionVersion != version) {
        return;
      }
      if (selection != null) {
        owner.editProfile(
          id,
          value: latest.value.copyWith(fontFamily: selection.family),
          ttf: selection,
          fontBinding: const ProfileFontBinding(
            version: 1,
            mode: ProfileFontMode.explicitCachedTtf,
          ),
        );
      }
    } catch (error) {
      if (current() &&
          owner.selectedProfileId == id &&
          (token == null || owner.acceptsProfileResource(token)) &&
          (version == null ||
              owner.profiles[id]?.fontSelectionVersion == version)) {
        setState(() => _fontIssue = error);
      }
    } finally {
      if (current()) setState(() => _picking = false);
    }
  }

  Widget _text(
    String field,
    String label,
    String value,
    ValueChanged<String> edit, {
    String? description,
    bool enabled = true,
  }) => SettingsTextField(
    key: _fields.putIfAbsent(field, GlobalKey<SettingsTextFieldState>.new),
    fieldKey: ValueKey('field-$field'),
    label: label,
    value: value,
    description: description,
    errors: errors.byField[field] ?? const [],
    onChanged: c.editable && enabled ? edit : null,
  );

  Widget _toggle(
    String field,
    String label,
    bool value,
    DesktopTerminalProfile Function(bool) edit,
  ) => Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      ToggleSwitch(
        key: ValueKey(field),
        content: Text(label),
        checked: value,
        onChanged: c.editable
            ? (v) => c.editProfile(widget.profileId, value: edit(v))
            : null,
      ),
      for (final error
          in errors.byField[field] ?? <DesktopSettingsFieldError>[])
        Semantics(liveRegion: true, child: Text(settingsErrorText(error))),
    ],
  );

  @override
  Widget build(BuildContext context) {
    final v = d.value;
    final n = d.numbers;
    final cap = c.capabilities!;
    final savedTargetExists = c.snapshot!.profiles.any(
      (profile) => profile.id == v.id,
    );
    final effective = c.snapshot!.effectiveProfiles
        .where((p) => p.id == v.id)
        .firstOrNull;
    return SettingsFields(
      children: [
        SelectableText('稳定 ID：${v.id}'),
        if (cap.profileChangesRequireNewSession)
          const Text('终端配置、字体与绑定在新会话生效；不会更改现有窗格。'),
        _text(
          'name',
          '配置名称',
          v.name,
          (s) => c.editProfile(v.id, value: v.copyWith(name: s)),
        ),
        _text(
          'terminal_type',
          '终端类型 TERM',
          v.terminalType,
          (s) => c.editProfile(v.id, value: v.copyWith(terminalType: s)),
        ),
        _text(
          'initial_cols',
          '初始列数',
          n.initialCols,
          (s) => c.editProfile(v.id, numbers: n.copyWith(initialCols: s)),
          description:
              '${cap.minGridSize}–${cap.maxGridSize}；实际网格取决于新会话布局与 PTY 约束',
        ),
        _text(
          'initial_rows',
          '初始行数',
          n.initialRows,
          (s) => c.editProfile(v.id, numbers: n.copyWith(initialRows: s)),
          description: '${cap.minGridSize}–${cap.maxGridSize}',
        ),
        _text(
          'answerback',
          '应答文本 Answerback',
          v.answerback,
          (s) => c.editProfile(v.id, value: v.copyWith(answerback: s)),
        ),
        Text('字体', style: FluentTheme.of(context).typography.titleLarge),
        Text(
          '已保存：${desktopFontModeLabel(d.savedFontBinding.mode)} / ${d.saved.fontFamily}',
        ),
        if (effective?.font != null) ...[
          Text('实际字体：${effective!.font!.actualFamily}'),
          if (effective.font!.fallbackReason != null)
            Text(
              '字体回退原因：${_fontFailureLabel(effective.font!.fallbackReason!)}',
            ),
        ],
        if (!d.fontBindingEditable)
          Text(
            '保留不可编辑的字体元数据：${d.savedFontBinding.uneditableReason == ProfileFontMetadataError.unsupportedVersion ? "不支持的元数据版本" : "元数据无效"}；其他字段仍可保存。',
          ),
        SettingsChoice(
          key: const ValueKey('font_binding'),
          label: '字体绑定模式',
          value: d.fontBinding.mode.name,
          options: {
            if (d.fontBinding.mode == ProfileFontMode.legacyUndetermined ||
                !d.fontBindingEditable)
              d.fontBinding.mode.name: desktopFontModeLabel(d.fontBinding.mode),
            for (final mode in DesktopProfileDraft.newFontModes)
              mode.name: desktopFontModeLabel(mode),
          },
          errors: errors.byField['font_binding'] ?? const [],
          disabledOptions: {
            if (!savedTargetExists) ProfileFontMode.explicitCachedTtf.name,
          },
          onChanged: !c.editable || !d.fontBindingEditable
              ? null
              : (s) {
                  final mode = ProfileFontMode.values.singleWhere(
                    (m) => m.name == s,
                  );
                  if (mode == d.fontBinding.mode) return;
                  if (mode == ProfileFontMode.explicitCachedTtf) {
                    _pick();
                    return;
                  }
                  c.cancelProfileResource();
                  c.editProfile(
                    v.id,
                    clearTtf: true,
                    fontBinding: ProfileFontBinding(version: 1, mode: mode),
                  );
                },
        ),
        _text(
          'font_family',
          '字体名称 Family',
          v.fontFamily,
          (s) {
            c.cancelProfileResource();
            c.editProfile(
              v.id,
              value: v.copyWith(fontFamily: s),
              clearTtf: true,
              fontBinding: const ProfileFontBinding(
                version: 1,
                mode: ProfileFontMode.explicitFamily,
              ),
            );
          },
          enabled: d.fontBindingEditable,
          description: '修改名称表示显式字体；不把相同名称误认为继承或旧版绑定。',
        ),
        _text(
          'font_size',
          '字号',
          n.fontSize,
          (s) => c.editProfile(v.id, numbers: n.copyWith(fontSize: s)),
          description: '${cap.minFontSize}–${cap.maxFontSize}，支持小数',
        ),
        Button(
          key: const ValueKey('pick-ttf'),
          onPressed:
              c.editable &&
                  d.fontBindingEditable &&
                  savedTargetExists &&
                  !_picking
              ? _pick
              : null,
          child: Text(_picking ? '正在选择／预览字体…' : '选择并预览 TTF'),
        ),
        if (d.ttf != null) Text('待保存 TTF：${d.ttf!.family}（与完整配置同一次提交）'),
        if (_fontIssue != null)
          Semantics(
            key: const ValueKey('profile-font-issue'),
            liveRegion: true,
            child: const Text('字体选择／预览失败；原选择保持，请核对目标后重新选择。'),
          ),
        for (final error
            in errors.byField['font_resource'] ?? <DesktopSettingsFieldError>[])
          Text(settingsErrorText(error)),
        SettingsChoice(
          key: const ValueKey('profile-color_scheme'),
          label: '终端配色',
          value: v.colorScheme,
          options: {
            for (final s in cap.colorSchemes) s: desktopColorLabels[s] ?? s,
          },
          errors: errors.byField['color_scheme'] ?? const [],
          onChanged: c.editable
              ? (s) => c.editProfile(v.id, value: v.copyWith(colorScheme: s))
              : null,
        ),
        _text(
          'scrollback_lines',
          '滚回行数',
          n.scrollbackLines,
          (s) => c.editProfile(v.id, numbers: n.copyWith(scrollbackLines: s)),
          description: '${cap.minScrollbackLines}–${cap.maxScrollbackLines}',
        ),
        if (effective != null)
          Text(
            '已保存配置：${d.saved.scrollbackLines}；有效滚回：${effective.scrollbackLines}${effective.scrollbackLimitReason.isEmpty ? "" : "；原因：${effective.scrollbackLimitReason}"}',
          ),
        _toggle(
          'scroll_on_output',
          '输出时跟随滚动',
          v.scrollOnOutput,
          (b) => v.copyWith(scrollOnOutput: b),
        ),
        _toggle(
          'scroll_on_keypress',
          '输入时跟随滚动',
          v.scrollOnKeypress,
          (b) => v.copyWith(scrollOnKeypress: b),
        ),
        Text('键盘与鼠标', style: FluentTheme.of(context).typography.titleLarge),
        _toggle(
          'left_alt_as_meta',
          '左 Alt 作为 Meta',
          v.leftAltAsMeta,
          (b) => v.copyWith(leftAltAsMeta: b),
        ),
        _toggle(
          'right_alt_as_meta',
          '右 Alt 作为 Meta',
          v.rightAltAsMeta,
          (b) => v.copyWith(rightAltAsMeta: b),
        ),
        _toggle(
          'enable_csi_u',
          '启用 CSI-u',
          v.enableCsiU,
          (b) => v.copyWith(enableCsiU: b),
        ),
        _toggle(
          'enable_kitty_keyboard',
          '启用 Kitty 键盘协议',
          v.enableKittyKeyboard,
          (b) => v.copyWith(enableKittyKeyboard: b),
        ),
        _toggle(
          'mouse_reporting',
          '允许鼠标报告',
          v.mouseReporting,
          (b) => v.copyWith(mouseReporting: b),
        ),
        Text('此配置的绑定', style: FluentTheme.of(context).typography.titleLarge),
        DesktopSettingsBindingsEditor(
          key: ValueKey('profile-bindings-${v.id}'),
          rows: d.bindings,
          actions: cap.keyActions,
          errors: errors,
          appBindings: c.app!.value.keyBindings,
          onChanged: c.editable
              ? (rows) => c.editProfile(v.id, bindings: rows)
              : null,
        ),
        for (final entry in errors.byField.entries)
          if (_fields.containsKey(entry.key))
            Button(
              key: ValueKey('locate-${entry.key}'),
              onPressed: () => _fields[entry.key]?.currentState?.focus(),
              child: Text('定位：${entry.key}'),
            ),
      ],
    );
  }
}
