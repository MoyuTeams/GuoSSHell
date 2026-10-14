import 'dart:typed_data';

import '../../bindings/bindings.dart';

List<DesktopKeyBinding> copyDesktopBindings(List<DesktopKeyBinding> values) =>
    List.unmodifiable(values.map((value) => value.copyWith()));

DesktopAppSettings copyDesktopApp(DesktopAppSettings value) =>
    value.copyWith(keyBindings: copyDesktopBindings(value.keyBindings));

DesktopTerminalProfile copyDesktopProfile(DesktopTerminalProfile value) =>
    value.copyWith(keyBindings: copyDesktopBindings(value.keyBindings));

/// DTO 只是拥有字段投影；未知持久化扩展始终由 Rust 从已存对象保全。
DesktopConfiguration copyDesktopConfiguration(DesktopConfiguration value) =>
    value.copyWith(
      app: copyDesktopApp(value.app),
      profiles: List.unmodifiable(value.profiles.map(copyDesktopProfile)),
      effectiveProfiles: List.unmodifiable(
        value.effectiveProfiles.map(
          (profile) => profile.copyWith(
            keyBindings: copyDesktopBindings(profile.keyBindings),
            fontBinding: profile.fontBinding.copyWith(),
            font: () => profile.font?.copyWith(),
          ),
        ),
      ),
    );

DesktopSettingsCapabilities copyDesktopCapabilities(
  DesktopSettingsCapabilities value,
) => value.copyWith(
  colorSchemes: List.unmodifiable(value.colorSchemes),
  keyActions: List.unmodifiable(value.keyActions),
);

DesktopSettingsResult copyDesktopResult(DesktopSettingsResult value) =>
    value.copyWith(
      fieldErrors: List.unmodifiable(
        value.fieldErrors.map((error) => error.copyWith()),
      ),
      capabilities: copyDesktopCapabilities(value.capabilities),
      snapshot: () => value.snapshot == null
          ? null
          : copyDesktopConfiguration(value.snapshot!),
    );

/// 重复绑定也有独立行身份；编辑或移动行时保留 id，不从 chord 推导身份。
class DesktopBindingRow {
  DesktopBindingRow(DesktopKeyBinding value)
    : id = 'binding-${++_sequence}',
      value = value.copyWith();
  const DesktopBindingRow._(this.id, this.value);
  static int _sequence = 0;
  final String id;
  final DesktopKeyBinding value;
  DesktopBindingRow edit(DesktopKeyBinding value) =>
      DesktopBindingRow._(id, value.copyWith());
}

List<DesktopBindingRow> desktopBindingRows(List<DesktopKeyBinding> bindings) =>
    List.unmodifiable(bindings.map(DesktopBindingRow.new));

/// 只校验能否表示协议数字，不实现 Rust 的领域校验或默认值回填。
class DesktopProfileNumbers {
  const DesktopProfileNumbers({
    required this.initialCols,
    required this.initialRows,
    required this.scrollbackLines,
    required this.fontSize,
  });
  factory DesktopProfileNumbers.fromProfile(DesktopTerminalProfile profile) =>
      DesktopProfileNumbers(
        initialCols: '${profile.initialCols}',
        initialRows: '${profile.initialRows}',
        scrollbackLines: '${profile.scrollbackLines}',
        fontSize: '${profile.fontSize}',
      );
  final String initialCols;
  final String initialRows;
  final String scrollbackLines;
  final String fontSize;
  DesktopProfileNumbers copyWith({
    String? initialCols,
    String? initialRows,
    String? scrollbackLines,
    String? fontSize,
  }) => DesktopProfileNumbers(
    initialCols: initialCols ?? this.initialCols,
    initialRows: initialRows ?? this.initialRows,
    scrollbackLines: scrollbackLines ?? this.scrollbackLines,
    fontSize: fontSize ?? this.fontSize,
  );

  int? _uint32(String text) {
    final number = int.tryParse(text.trim());
    return number != null && number >= 0 && number <= 0xffffffff
        ? number
        : null;
  }

  List<DesktopSettingsFieldError> get errors => [
    if (_uint32(initialCols) == null)
      const DesktopSettingsFieldError(
        field: 'initial_cols',
        code: 'invalid_number',
      ),
    if (_uint32(initialRows) == null)
      const DesktopSettingsFieldError(
        field: 'initial_rows',
        code: 'invalid_number',
      ),
    if (_uint32(scrollbackLines) == null)
      const DesktopSettingsFieldError(
        field: 'scrollback_lines',
        code: 'invalid_number',
      ),
    if (!(double.tryParse(fontSize.trim())?.isFinite ?? false))
      const DesktopSettingsFieldError(
        field: 'font_size',
        code: 'invalid_number',
      ),
  ];

  DesktopTerminalProfile apply(DesktopTerminalProfile value) {
    if (errors.isNotEmpty) throw StateError('数字草稿不能编码');
    return copyDesktopProfile(value).copyWith(
      initialCols: _uint32(initialCols)!,
      initialRows: _uint32(initialRows)!,
      scrollbackLines: _uint32(scrollbackLines)!,
      fontSize: double.parse(fontSize.trim()),
    );
  }

  bool matches(DesktopTerminalProfile profile) =>
      errors.isEmpty && apply(profile) == profile;
}

/// 仅存已由现有字体系统校验／预览的选择；不创建注册器或第二套缓存。
class DesktopTtfSelection {
  DesktopTtfSelection({required this.family, required Uint8List bytes})
    : bytes = Uint8List.fromList(bytes).asUnmodifiableView();
  final String family;
  final Uint8List bytes;
}

class DesktopAppDraft {
  DesktopAppDraft.fromSaved(
    DesktopAppSettings saved,
    this.baseRevision, {
    List<DesktopBindingRow>? bindings,
    this.editVersion = 0,
  }) : saved = copyDesktopApp(saved),
       value = copyDesktopApp(saved),
       bindings = bindings == null
           ? desktopBindingRows(saved.keyBindings)
           : List.unmodifiable(bindings);
  const DesktopAppDraft._({
    required this.saved,
    required this.value,
    required this.bindings,
    required this.baseRevision,
    required this.editVersion,
  });
  final DesktopAppSettings saved;
  final DesktopAppSettings value;
  final List<DesktopBindingRow> bindings;
  final Uint64 baseRevision;
  final int editVersion;
  bool get dirty => value != saved;

  DesktopAppDraft acceptSaved(
    DesktopAppSettings saved,
    Uint64 revision, {
    bool discard = false,
  }) => DesktopAppDraft.fromSaved(
    saved,
    revision,
    bindings: listEquals(value.keyBindings, saved.keyBindings)
        ? bindings
        : null,
    editVersion: editVersion + (discard ? 1 : 0),
  );

  DesktopAppDraft edit({
    DesktopAppSettings? value,
    List<DesktopBindingRow>? bindings,
  }) {
    final rows = List<DesktopBindingRow>.unmodifiable(
      bindings ??
          (value != null &&
                  listEquals(value.keyBindings, this.value.keyBindings)
              ? this.bindings
              : value == null
              ? this.bindings
              : desktopBindingRows(value.keyBindings)),
    );
    return DesktopAppDraft._(
      saved: saved,
      value: copyDesktopApp(value ?? this.value).copyWith(
        keyBindings: copyDesktopBindings(rows.map((row) => row.value).toList()),
      ),
      bindings: rows,
      baseRevision: baseRevision,
      editVersion: editVersion + 1,
    );
  }

  DesktopAppDraft reviewed(DesktopAppSettings saved, Uint64 revision) =>
      DesktopAppDraft._(
        saved: copyDesktopApp(saved),
        value: value,
        bindings: bindings,
        baseRevision: revision,
        editVersion: editVersion,
      );
}

class DesktopProfileDraft {
  DesktopProfileDraft.fromSaved(
    DesktopTerminalProfile saved,
    ProfileFontBinding binding,
    this.baseRevision, {
    List<DesktopBindingRow>? bindings,
    DesktopProfileNumbers? numbers,
    this.editVersion = 0,
    this.fontSelectionVersion = 0,
  }) : saved = copyDesktopProfile(saved),
       value = copyDesktopProfile(saved),
       savedFontBinding = binding.copyWith(),
       fontBinding = binding.copyWith(),
       numbers = numbers ?? DesktopProfileNumbers.fromProfile(saved),
       bindings = bindings == null
           ? desktopBindingRows(saved.keyBindings)
           : List.unmodifiable(bindings),
       ttf = null;
  const DesktopProfileDraft._({
    required this.saved,
    required this.value,
    required this.savedFontBinding,
    required this.fontBinding,
    required this.numbers,
    required this.bindings,
    required this.ttf,
    required this.baseRevision,
    required this.editVersion,
    required this.fontSelectionVersion,
  });
  final DesktopTerminalProfile saved;
  final DesktopTerminalProfile value;
  final ProfileFontBinding savedFontBinding;
  final ProfileFontBinding fontBinding;
  final DesktopProfileNumbers numbers;
  final List<DesktopBindingRow> bindings;
  final DesktopTtfSelection? ttf;
  final Uint64 baseRevision;
  final int editVersion;

  /// 只随 family／绑定／TTF 选择变化推进，非字体输入不改变选择归属。
  final int fontSelectionVersion;
  bool get dirty =>
      ttf != null ||
      fontBinding != savedFontBinding ||
      value != saved ||
      !numbers.matches(saved);
  bool get fontBindingEditable =>
      savedFontBinding.mode != ProfileFontMode.uneditable;
  static const newFontModes = [
    ProfileFontMode.inheritTerminalSlot,
    ProfileFontMode.explicitFamily,
    ProfileFontMode.explicitCachedTtf,
  ];

  DesktopProfileDraft acceptSaved(
    DesktopTerminalProfile saved,
    ProfileFontBinding binding,
    Uint64 revision, {
    bool discard = false,
  }) => DesktopProfileDraft.fromSaved(
    saved,
    binding,
    revision,
    bindings: listEquals(value.keyBindings, saved.keyBindings)
        ? bindings
        : null,
    numbers: numbers.matches(saved) ? numbers : null,
    editVersion: editVersion + (discard ? 1 : 0),
    fontSelectionVersion: fontSelectionVersion + (discard ? 1 : 0),
  );

  DesktopProfileDraft edit({
    DesktopTerminalProfile? value,
    DesktopProfileNumbers? numbers,
    List<DesktopBindingRow>? bindings,
    ProfileFontBinding? fontBinding,
    DesktopTtfSelection? ttf,
    bool clearTtf = false,
  }) {
    var next = copyDesktopProfile(value ?? this.value);
    if (next.id != saved.id || next.version != saved.version) {
      throw StateError('profile 身份与版本只读');
    }
    final binding = fontBinding ?? this.fontBinding;
    if (!fontBindingEditable &&
        (next.fontFamily != saved.fontFamily ||
            binding != savedFontBinding ||
            ttf != null)) {
      throw StateError('字体元数据不可编辑');
    }
    if (binding != this.fontBinding && !newFontModes.contains(binding.mode)) {
      throw StateError('保全字体模式不能作为新选择');
    }
    final rows = List<DesktopBindingRow>.unmodifiable(
      bindings ??
          (listEquals(next.keyBindings, this.value.keyBindings)
              ? this.bindings
              : desktopBindingRows(next.keyBindings)),
    );
    final raw =
        numbers ??
        this.numbers.copyWith(
          initialCols: next.initialCols == this.value.initialCols
              ? null
              : '${next.initialCols}',
          initialRows: next.initialRows == this.value.initialRows
              ? null
              : '${next.initialRows}',
          scrollbackLines: next.scrollbackLines == this.value.scrollbackLines
              ? null
              : '${next.scrollbackLines}',
          fontSize: next.fontSize == this.value.fontSize
              ? null
              : '${next.fontSize}',
        );
    if (raw.errors.isEmpty) next = raw.apply(next);
    final nextTtf = clearTtf ? null : ttf ?? this.ttf;
    final fontSelectionChanged =
        next.fontFamily != this.value.fontFamily ||
        binding != this.fontBinding ||
        !identical(nextTtf, this.ttf);
    return DesktopProfileDraft._(
      saved: saved,
      value: next.copyWith(
        keyBindings: copyDesktopBindings(rows.map((row) => row.value).toList()),
      ),
      savedFontBinding: savedFontBinding,
      fontBinding: binding.copyWith(),
      numbers: raw,
      bindings: rows,
      ttf: nextTtf,
      baseRevision: baseRevision,
      editVersion: editVersion + 1,
      fontSelectionVersion:
          fontSelectionVersion + (fontSelectionChanged ? 1 : 0),
    );
  }

  /// 只确认匹配选择的已发布字体；草稿基线与所有非字体输入保持不变。
  DesktopProfileDraft acknowledgeAppliedTtf(
    DesktopTerminalProfile published,
    ProfileFontBinding binding, {
    required DesktopTtfSelection selection,
    required int? selectionVersion,
  }) {
    if (!identical(ttf, selection) ||
        fontSelectionVersion != selectionVersion) {
      return this;
    }
    return DesktopProfileDraft._(
      saved: saved,
      value: value.copyWith(fontFamily: published.fontFamily),
      savedFontBinding: savedFontBinding,
      fontBinding: binding.copyWith(),
      numbers: numbers,
      bindings: bindings,
      ttf: null,
      baseRevision: baseRevision,
      editVersion: editVersion,
      fontSelectionVersion: fontSelectionVersion,
    );
  }

  DesktopProfileDraft reviewed(
    DesktopTerminalProfile saved,
    ProfileFontBinding binding,
    Uint64 revision,
  ) => DesktopProfileDraft._(
    saved: copyDesktopProfile(saved),
    value: value,
    savedFontBinding: binding.copyWith(),
    fontBinding: fontBinding,
    numbers: numbers,
    bindings: bindings,
    ttf: ttf,
    baseRevision: revision,
    editVersion: editVersion,
    fontSelectionVersion: fontSelectionVersion,
  );
}

/// 未识别字段始终保留页级；带索引的绑定错误使用发起请求时的稳定行身份。
class DesktopDraftErrors {
  DesktopDraftErrors(
    Iterable<DesktopSettingsFieldError> errors, {
    List<DesktopBindingRow> bindings = const [],
  }) {
    final fields = <String, List<DesktopSettingsFieldError>>{};
    final rows = <String, List<DesktopSettingsFieldError>>{};
    final page = <DesktopSettingsFieldError>[];
    for (final error in errors) {
      final match = RegExp(
        r'^key_bindings\[(\d+)\]\.(key|code|action|modifiers)$',
      ).firstMatch(error.field);
      final index = match == null ? null : int.tryParse(match.group(1)!);
      if (index != null && index < bindings.length) {
        rows.putIfAbsent(bindings[index].id, () => []).add(error);
      } else if (_fields.contains(error.field)) {
        fields.putIfAbsent(error.field, () => []).add(error);
      } else {
        page.add(error);
      }
    }
    byField = Map.unmodifiable(
      fields.map(
        (key, value) =>
            MapEntry(key, List<DesktopSettingsFieldError>.unmodifiable(value)),
      ),
    );
    byBindingRow = Map.unmodifiable(
      rows.map(
        (key, value) =>
            MapEntry(key, List<DesktopSettingsFieldError>.unmodifiable(value)),
      ),
    );
    pageErrors = List.unmodifiable(page);
  }
  static const _fields = {
    'app',
    'profile',
    'target_id',
    'name',
    'profile.id',
    'profile.version',
    'default_terminal_profile',
    'color_scheme',
    'key_bindings',
    'key_bindings.code',
    'key_bindings.action',
    'key_bindings.modifiers',
    'terminal_type',
    'initial_cols',
    'initial_rows',
    'scrollback_lines',
    'font_family',
    'font_size',
    'font_binding',
    'font_resource',
    'left_alt_as_meta',
    'right_alt_as_meta',
    'enable_csi_u',
    'enable_kitty_keyboard',
    'mouse_reporting',
    'scroll_on_output',
    'scroll_on_keypress',
    'answerback',
  };
  late final Map<String, List<DesktopSettingsFieldError>> byField;
  late final Map<String, List<DesktopSettingsFieldError>> byBindingRow;
  late final List<DesktopSettingsFieldError> pageErrors;
}

/// 表单仅通知焦点所有者；不路由或调用任何终端 action。
abstract interface class DesktopSettingsFocusBoundary {
  void setSettingsInputFocused(bool focused);
}
