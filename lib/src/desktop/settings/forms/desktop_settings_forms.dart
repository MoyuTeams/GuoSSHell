import 'package:fluent_ui/fluent_ui.dart';

import '../../../bindings/bindings.dart';
import '../../desktop_theme.dart';
import '../desktop_settings_controller.dart';
import '../desktop_settings_draft.dart';
import '../desktop_settings_gateway.dart';
import 'desktop_profile_form.dart';
import 'desktop_settings_bindings_editor.dart';
import 'settings_form_controls.dart';

export 'desktop_profile_form.dart' show DesktopProfileTtfPicker;

enum DesktopSettingsFormSection { app, profile }

/// 可嵌入正常设置入口的完整配置区域；controller、字体适配器与分区导航由父持有。
class DesktopSettingsForms extends StatefulWidget {
  const DesktopSettingsForms({
    super.key,
    required this.controller,
    required this.onPickTtf,
    required this.onClose,
    this.section = DesktopSettingsFormSection.app,
    this.onShowApp,
    this.focusBoundary,
  });
  final DesktopSettingsController controller;
  final DesktopProfileTtfPicker onPickTtf;

  /// 确认关闭后由父移除设置区域并释放 controller；应用级 gateway 不随窗口销毁。
  final VoidCallback onClose;
  final DesktopSettingsFormSection section;

  /// 「设为默认」写入应用草稿后请父切到应用分区。
  final VoidCallback? onShowApp;
  final DesktopSettingsFocusBoundary? focusBoundary;

  @override
  State<DesktopSettingsForms> createState() => DesktopSettingsFormsState();
}

class DesktopSettingsFormsState extends State<DesktopSettingsForms> {
  String? _freshId;
  String? _issue;
  final _writeStates = <String, String>{};
  final _reviewUnits = <String>{};
  final _shortcuts = ShortcutManager(
    shortcuts: WidgetsApp.defaultShortcuts,
    modal: true,
  );
  late BuildContext _surfaceContext;
  DesktopSettingsController get c => widget.controller;

  @override
  void initState() {
    super.initState();
    c.addListener(_changed);
    _changed();
    if (c.queryState == DesktopSettingsQueryState.idle) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) c.refresh();
      });
    }
  }

  @override
  void didUpdateWidget(DesktopSettingsForms oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.controller != c) {
      oldWidget.controller.removeListener(_changed);
      c.addListener(_changed);
      _freshId = null;
      _reviewUnits.clear();
      _writeStates.clear();
      _changed();
    }
    if (oldWidget.focusBoundary != widget.focusBoundary) {
      oldWidget.focusBoundary?.setSettingsInputFocused(false);
    }
  }

  void _changed() {
    final units = ['app', 'lifecycle', ...c.profiles.keys];
    for (final unit in units) {
      final s = c.stateFor(unit);
      final signature =
          '${s.pending?.command.requestId}:${s.state}:${s.result?.status}';
      final previous = _writeStates[unit];
      if (previous == signature) continue;
      _writeStates[unit] = signature;
      if (previous != null) _freshId = null;
      final version = unit == 'app'
          ? c.app?.editVersion
          : c.profiles[unit]?.editVersion;
      if (s.state == DesktopSettingsWriteState.applied &&
          (s.issue != null ||
              (s.pending != null &&
                  version != null &&
                  s.pending!.context.editVersion != version))) {
        if (unit == 'app' ? c.app!.dirty : c.profiles[unit]?.dirty ?? false) {
          _reviewUnits.add(unit);
        }
      }
    }
    if (mounted) setState(() {});
  }

  @override
  void dispose() {
    c.removeListener(_changed);
    widget.focusBoundary?.setSettingsInputFocused(false);
    _shortcuts.dispose();
    super.dispose();
  }

  void _run(VoidCallback action) {
    setState(() => _issue = null);
    try {
      action();
    } catch (_) {
      if (mounted) setState(() => _issue = '操作未发送：请先完成查询、写入核实或草稿核对。');
    }
  }

  bool get _fresh =>
      _freshId != null &&
      c.queryState == DesktopSettingsQueryState.ready &&
      c.queryResult?.requestId == _freshId &&
      c.queryResult?.status == 'ok';

  void _query() => _run(() {
    final id = c.refresh();
    setState(() => _freshId = id);
  });

  Future<bool> _confirm(
    String title,
    String content, {
    BuildContext? context,
  }) async =>
      await showDialog<bool>(
        context: context ?? _surfaceContext,
        useRootNavigator: false,
        builder: (context) => ContentDialog(
          title: Text(title),
          content: Text(content),
          actions: [
            Button(
              onPressed: () => Navigator.pop(context, false),
              child: const Text('继续编辑'),
            ),
            FilledButton(
              onPressed: () => Navigator.pop(context, true),
              child: const Text('明确放弃'),
            ),
          ],
        ),
      ) ??
      false;

  /// 标题栏、父路由返回与页内关闭共用此确认入口；确认后才调用 onClose。
  /// extraDirty 并入父持有的其他设置区草稿；本区域不可见时由父提供对话框 context。
  Future<void> requestClose({
    bool extraDirty = false,
    BuildContext? dialogContext,
  }) async {
    final pending = c.blockingWrite;
    if ((c.dirty || pending != null || extraDirty) &&
        !await _confirm(
          '关闭设置？',
          pending == null ? '未保存的修改将放弃；已提交配置保持不变。' : '已发出的写入不能取消；关闭后仍须查询核实结果。',
          context: dialogContext,
        )) {
      return;
    }
    if (mounted) widget.onClose();
  }

  Map<String, String> _appSummary(DesktopAppSettings a) => {
    '默认配置': a.defaultTerminalProfile,
    '应用配色': desktopColorLabels[a.colorScheme] ?? a.colorScheme,
    '应用绑定': _bindings(a.keyBindings),
  };

  String _bindings(List<DesktopKeyBinding> bindings) => bindings
      .map(
        (b) =>
            '${b.shift ? "Shift+" : ""}${b.control ? "Control+" : ""}${b.alt ? "Alt+" : ""}${b.superKey ? "Super+" : ""}${b.key} → ${desktopActionLabels[b.action] ?? "不支持的动作"}',
      )
      .join('\n');

  Map<String, String> _profileSummary(
    DesktopTerminalProfile p,
    ProfileFontBinding binding, {
    DesktopProfileNumbers? numbers,
  }) => {
    '配置名称': p.name,
    '稳定 ID': p.id,
    '终端类型': p.terminalType,
    '初始列数': numbers?.initialCols ?? '${p.initialCols}',
    '初始行数': numbers?.initialRows ?? '${p.initialRows}',
    '应答文本': p.answerback,
    '字体名称': p.fontFamily,
    '字号': numbers?.fontSize ?? '${p.fontSize}',
    '字体绑定': '${desktopFontModeLabel(binding.mode)} ${binding.hash ?? ""}',
    '终端配色': desktopColorLabels[p.colorScheme] ?? p.colorScheme,
    '滚回行数': numbers?.scrollbackLines ?? '${p.scrollbackLines}',
    '输出时滚动': '${p.scrollOnOutput}',
    '输入时滚动': '${p.scrollOnKeypress}',
    '左 Alt Meta': '${p.leftAltAsMeta}',
    '右 Alt Meta': '${p.rightAltAsMeta}',
    'CSI-u': '${p.enableCsiU}',
    'Kitty 键盘': '${p.enableKittyKeyboard}',
    '鼠标报告': '${p.mouseReporting}',
    '配置绑定': _bindings(p.keyBindings),
  };

  Future<void> _review(String unit) async {
    if (!_fresh) return;
    final snapshot = c.snapshot!;
    final version = unit == 'app'
        ? c.app!.editVersion
        : c.profiles[unit]?.editVersion ?? 0;
    final pending = c.blockingWrite;
    if (pending != null && pending.context.unit != unit) return;
    final local = unit == 'app'
        ? _appSummary(c.app!.value)
        : unit == 'lifecycle'
        ? <String, String>{'配置列表': c.profiles.keys.join('\n')}
        : _profileSummary(
            c.profiles[unit]!.value,
            c.profiles[unit]!.fontBinding,
            numbers: c.profiles[unit]!.numbers,
          );
    final savedProfile = snapshot.profiles
        .where((p) => p.id == unit)
        .firstOrNull;
    final latest = unit == 'app'
        ? _appSummary(snapshot.app)
        : unit == 'lifecycle'
        ? <String, String>{
            '配置列表': snapshot.profiles
                .map((p) => '${p.name} / ${p.id}')
                .join('\n'),
          }
        : savedProfile == null
        ? <String, String>{'状态': '此配置已删除'}
        : _profileSummary(
            savedProfile,
            snapshot.effectiveProfiles
                .singleWhere((p) => p.id == unit)
                .fontBinding,
          );
    final keep = await showDialog<bool>(
      context: _surfaceContext,
      useRootNavigator: false,
      builder: (context) => ContentDialog(
        title: const Text('核对最新配置与本地草稿'),
        content: SingleChildScrollView(
          child: SettingsFields(
            children: [
              Text('最新 revision：${snapshot.revision}。这里只确认基线，不保存、不重试。'),
              if (pending != null)
                Text(
                  '核实请求：${pending.command.operation}\n目标：${pending.command.targetId.isEmpty ? "应用／新建配置" : pending.command.targetId}',
                ),
              for (final entry in local.entries)
                Text(
                  '${entry.key}\n本地：${entry.value}\n最新：${latest[entry.key] ?? "已删除"}',
                ),
              if (unit != 'app' &&
                  unit != 'lifecycle' &&
                  c.profiles[unit]!.ttf != null)
                Text('本地还保留待提交 TTF：${c.profiles[unit]!.ttf!.family}'),
            ],
          ),
        ),
        actions: [
          Button(
            onPressed: () => Navigator.pop(context),
            child: const Text('取消'),
          ),
          Button(
            key: const ValueKey('review-discard'),
            onPressed: () => Navigator.pop(context, false),
            child: const Text('采用最新／放弃草稿'),
          ),
          FilledButton(
            key: const ValueKey('review-keep'),
            onPressed:
                unit != 'app' && unit != 'lifecycle' && savedProfile == null
                ? null
                : () => Navigator.pop(context, true),
            child: const Text('保留草稿并确认基线'),
          ),
        ],
      ),
    );
    if (!mounted || keep == null) return;
    _run(() {
      if (!_fresh ||
          c.snapshot!.revision != snapshot.revision ||
          (unit == 'app'
                  ? c.app!.editVersion
                  : c.profiles[unit]?.editVersion ?? 0) !=
              version) {
        throw StateError('核对内容已变化');
      }
      if (pending != null) {
        c.confirmUncertainWrite(pending.command.requestId, snapshot.revision);
      }
      if (unit == 'app') {
        c.reviewApp(
          revision: snapshot.revision,
          editVersion: version,
          keepDraft: keep,
        );
      } else if (unit == 'lifecycle') {
        c.reviewLifecycle(snapshot.revision);
      } else {
        c.reviewProfile(
          unit,
          revision: snapshot.revision,
          editVersion: version,
          keepDraft: keep,
        );
      }
      _reviewUnits.remove(unit);
    });
  }

  bool _lifecycleTargetExists(String? id) =>
      id != null &&
      c.profiles.containsKey(id) &&
      (c.snapshot?.profiles.any((profile) => profile.id == id) ?? false);

  bool _checkLifecycleTarget(String id) {
    if (_lifecycleTargetExists(id)) return true;
    if (mounted) setState(() => _issue = '目标配置已不在保存快照中；请核对草稿或选择其他配置。');
    return false;
  }

  Future<bool> _settleSource(String id) async {
    if (!_checkLifecycleTarget(id)) return false;
    if (c.needsReview(id) || _reviewUnits.contains(id)) {
      setState(() => _issue = '请先显式查询并核对目标配置。');
      return false;
    }
    if (!c.profiles[id]!.dirty) return true;
    final decision = await showDialog<String>(
      context: _surfaceContext,
      useRootNavigator: false,
      builder: (context) => ContentDialog(
        title: const Text('目标配置有未保存修改'),
        content: const Text('复制使用已保存配置；生命周期操作前请明确保存或放弃草稿。'),
        actions: [
          Button(
            onPressed: () => Navigator.pop(context),
            child: const Text('继续编辑'),
          ),
          Button(
            onPressed: () => Navigator.pop(context, 'discard'),
            child: const Text('放弃目标草稿'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, 'save'),
            child: const Text('先保存目标'),
          ),
        ],
      ),
    );
    if (!mounted || decision == null) return false;
    if (!_checkLifecycleTarget(id)) return false;
    if (decision == 'save') {
      _run(() {
        c.saveProfile(id);
      });
      return false;
    }
    var success = false;
    _run(() {
      c.reviewProfile(
        id,
        revision: c.snapshot!.revision,
        editVersion: c.profiles[id]!.editVersion,
        keepDraft: false,
      );
      success = true;
    });
    return success;
  }

  Future<void> _lifecycle(String operation) async {
    final id = c.selectedProfileId;
    if (operation != 'create' && (id == null || !await _settleSource(id))) {
      return;
    }
    if (!mounted) return;
    if (operation != 'create' && !_checkLifecycleTarget(id!)) return;
    final revision = c.snapshot!.revision;
    if (operation == 'delete') {
      if (!await _confirm(
        '删除配置？',
        '${c.profiles[id]!.value.name}\n稳定 ID：$id\n默认配置或被引用配置由后端拒绝删除。',
      )) {
        return;
      }
      if (!mounted || !_checkLifecycleTarget(id!)) return;
      _run(() {
        if (c.snapshot!.revision != revision) throw StateError('配置已变化');
        c.deleteProfile(id);
      });
      return;
    }
    var name = operation == 'rename' ? c.profiles[id]!.value.name : '';
    final result = await showDialog<String>(
      context: _surfaceContext,
      useRootNavigator: false,
      builder: (context) => StatefulBuilder(
        builder: (context, update) => ContentDialog(
          title: Text(switch (operation) {
            'create' => '新建配置',
            'copy' => '复制已保存配置',
            _ => '重命名配置',
          }),
          content: SettingsTextField(
            key: const ValueKey('lifecycle-name'),
            label: '新名称',
            value: name,
            autofocus: true,
            onChanged: (value) => update(() => name = value),
          ),
          actions: [
            Button(
              onPressed: () => Navigator.pop(context),
              child: const Text('取消'),
            ),
            FilledButton(
              key: const ValueKey('lifecycle-confirm'),
              onPressed: name.trim().isEmpty
                  ? null
                  : () => Navigator.pop(context, name),
              child: const Text('确认'),
            ),
          ],
        ),
      ),
    );
    if (!mounted || result == null) return;
    if (operation != 'create' && !_checkLifecycleTarget(id!)) return;
    _run(() {
      if (c.snapshot!.revision != revision) throw StateError('配置已变化');
      switch (operation) {
        case 'create':
          c.createProfile(result);
        case 'copy':
          c.copyProfile(id!, result);
        default:
          c.renameProfile(id!, result);
      }
    });
  }

  bool _canSave(String unit) =>
      c.editable &&
      c.blockingWrite == null &&
      !c.needsReview(unit) &&
      !_reviewUnits.contains(unit);

  Widget _status(String unit) {
    final state = c.stateFor(unit);
    final dirty = unit == 'app'
        ? c.app?.dirty ?? false
        : c.profiles[unit]?.dirty ?? false;
    final review =
        c.needsReview(unit) ||
        _reviewUnits.contains(unit) ||
        state.result?.status == 'conflict';
    final pending = c.blockingWrite;
    final verified =
        pending == null ||
        (pending.context.unit == unit &&
            pending.phase == DesktopSettingsPendingPhase.needsConfirmation &&
            c.gateway.verificationFor(pending.command.requestId)?.revision ==
                c.snapshot?.revision);
    final writeState = pending?.context.unit != unit
        ? state.state
        : switch (pending!.phase) {
            DesktopSettingsPendingPhase.waiting =>
              DesktopSettingsWriteState.saving,
            DesktopSettingsPendingPhase.uncertain =>
              DesktopSettingsWriteState.uncertain,
            DesktopSettingsPendingPhase.needsConfirmation =>
              DesktopSettingsWriteState.needsConfirmation,
            DesktopSettingsPendingPhase.completed => state.state,
          };
    final message = switch (writeState) {
      DesktopSettingsWriteState.saving => '正在保存；可继续编辑，后续输入不会被回包覆盖。',
      DesktopSettingsWriteState.uncertain => '结果待核实；不能重试。须先收到原请求结果，再显式查询核对。',
      DesktopSettingsWriteState.needsConfirmation =>
        '原请求结果已收到；请显式查询并确认，不会自动重放。',
      DesktopSettingsWriteState.applied =>
        state.issue != null
            ? '写入结果已核实；草稿仍需核对。'
            : dirty
            ? '提交已完成；仍有未保存编辑。'
            : '已保存',
      DesktopSettingsWriteState.failed =>
        '未保存：${state.result?.status ?? "输入无效"}。草稿保持。',
      DesktopSettingsWriteState.idle => dirty ? '有未保存修改' : '已加载配置',
    };
    return SettingsFields(
      children: [
        Semantics(
          liveRegion: true,
          child: Text(message, key: ValueKey('status-$unit')),
        ),
        if (review) const Text('需要核对最新配置，保存暂不可用。'),
        for (final error in [
          ...state.errors.pageErrors,
          ...state.errors.byField.values.expand((e) => e),
        ])
          Text(settingsErrorText(error)),
        Wrap(
          spacing: DesktopSpacing.sm,
          runSpacing: DesktopSpacing.sm,
          children: [
            Button(
              key: ValueKey('fresh-query-$unit'),
              onPressed: c.queryState == DesktopSettingsQueryState.loading
                  ? null
                  : _query,
              child: const Text('显式查询最新配置'),
            ),
            Button(
              key: ValueKey('review-$unit'),
              onPressed: _fresh && verified && c.editable
                  ? () => _review(unit)
                  : null,
              child: const Text('核对并保留／放弃草稿'),
            ),
          ],
        ),
      ],
    );
  }

  Widget _appForm() {
    final d = c.app!;
    final errors = c.stateFor('app').errors;
    return SettingsFields(
      children: [
        const Text('应用配色提交后立即生效；终端配色单独设置。默认配置与绑定用于新会话。'),
        SettingsChoice(
          key: const ValueKey('default_terminal_profile'),
          label: '默认终端配置',
          value: d.value.defaultTerminalProfile,
          options: {
            for (final p in c.snapshot!.profiles) p.id: '${p.name} / ${p.id}',
          },
          errors: errors.byField['default_terminal_profile'] ?? const [],
          onChanged: c.editable
              ? (id) => c.editApp(
                  value: d.value.copyWith(defaultTerminalProfile: id),
                )
              : null,
        ),
        SettingsChoice(
          key: const ValueKey('app-color_scheme'),
          label: '应用配色',
          value: d.value.colorScheme,
          options: {
            for (final s in c.capabilities!.colorSchemes)
              s: desktopColorLabels[s] ?? s,
          },
          errors: errors.byField['color_scheme'] ?? const [],
          onChanged: c.editable
              ? (s) => c.editApp(value: d.value.copyWith(colorScheme: s))
              : null,
        ),
        Text('应用绑定', style: FluentTheme.of(context).typography.titleLarge),
        DesktopSettingsBindingsEditor(
          key: const ValueKey('app-bindings'),
          rows: d.bindings,
          actions: c.capabilities!.keyActions,
          errors: errors,
          onChanged: c.editable ? (rows) => c.editApp(bindings: rows) : null,
        ),
        FilledButton(
          key: const ValueKey('save-app'),
          onPressed: _canSave('app') && d.dirty
              ? () => _run(() {
                  c.saveApp();
                })
              : null,
          child: const Text('保存应用配置'),
        ),
        _status('app'),
      ],
    );
  }

  Widget _profileForm() {
    final id = c.selectedProfileId;
    return SettingsFields(
      children: [
        SettingsChoice(
          key: const ValueKey('selected-profile'),
          label: '编辑配置（切换保留独立草稿）',
          value: id ?? '',
          options: {
            for (final p in c.profiles.values)
              p.value.id:
                  '${p.value.name} / ${p.value.id}${p.dirty ? " · 未保存" : ""}',
          },
          onChanged: c.selectProfile,
        ),
        Wrap(
          spacing: DesktopSpacing.sm,
          runSpacing: DesktopSpacing.sm,
          children: [
            for (final operation in ['create', 'copy', 'rename', 'delete'])
              Button(
                key: ValueKey('profile-$operation'),
                onPressed:
                    c.editable &&
                        c.blockingWrite == null &&
                        (operation == 'create' || _lifecycleTargetExists(id))
                    ? () => _lifecycle(operation)
                    : null,
                child: Text(switch (operation) {
                  'create' => '新建',
                  'copy' => '复制',
                  'rename' => '重命名',
                  _ => '删除',
                }),
              ),
            Button(
              key: const ValueKey('profile-default'),
              onPressed: !c.editable || !_lifecycleTargetExists(id)
                  ? null
                  : () => _run(() {
                      if (!_checkLifecycleTarget(id!)) return;
                      c.editApp(
                        value: c.app!.value.copyWith(
                          defaultTerminalProfile: id,
                        ),
                      );
                      widget.onShowApp?.call();
                    }),
              child: const Text('设为默认（应用草稿）'),
            ),
          ],
        ),
        _status('lifecycle'),
        if (id != null && c.profiles.containsKey(id)) ...[
          DesktopProfileForm(
            key: ValueKey('profile-form-$id'),
            controller: c,
            profileId: id,
            onPickTtf: widget.onPickTtf,
          ),
          FilledButton(
            key: const ValueKey('save-profile'),
            onPressed: _canSave(id) && c.profiles[id]!.dirty
                ? () => _run(() {
                    c.saveProfile(id);
                  })
                : null,
            child: const Text('保存此配置'),
          ),
          _status(id),
        ] else
          const Text('请选择配置；新建与复制返回的稳定 ID 会列在选择器中。'),
      ],
    );
  }

  @override
  Widget build(BuildContext context) => Shortcuts.manager(
    manager: _shortcuts,
    child: FocusScope(
      onFocusChange: (focused) =>
          widget.focusBoundary?.setSettingsInputFocused(focused),
      child: Navigator(
        pages: [
          _SettingsFormPage(
            child: Builder(
              builder: (context) {
                _surfaceContext = context;
                return _surface(context);
              },
            ),
          ),
        ],
        onDidRemovePage: (_) {},
      ),
    ),
  );

  Widget _surface(BuildContext context) => FocusTraversalGroup(
    child: LayoutBuilder(
      builder: (context, constraints) => SingleChildScrollView(
        child: Align(
          alignment: Alignment.topLeft,
          child: ConstrainedBox(
            constraints: const BoxConstraints(
              maxWidth: DesktopMetrics.contentWidth,
            ),
            child: Padding(
              padding: EdgeInsets.all(
                constraints.maxWidth < DesktopMetrics.narrowWidth
                    ? DesktopSpacing.lg
                    : DesktopSpacing.xl,
              ),
              child: SettingsFields(
                children: [
                  Text(
                    widget.section == DesktopSettingsFormSection.app
                        ? '应用'
                        : '配置文件',
                    style: FluentTheme.of(context).typography.display,
                  ),
                  if (_issue != null)
                    Semantics(liveRegion: true, child: Text(_issue!)),
                  if (c.queryState == DesktopSettingsQueryState.loading)
                    Semantics(liveRegion: true, child: const Text('正在查询配置…')),
                  if (c.queryState == DesktopSettingsQueryState.failed ||
                      c.queryState == DesktopSettingsQueryState.unavailable)
                    SettingsFields(
                      children: [
                        Text(
                          c.queryState == DesktopSettingsQueryState.unavailable
                              ? '配置不可用：${c.capabilities?.unavailableReason ?? c.queryResult?.capabilities.unavailableReason ?? "未知原因"}'
                              : '配置查询失败；原草稿保持。',
                        ),
                        Button(
                          key: const ValueKey('query-retry'),
                          onPressed: _query,
                          child: const Text('重新查询'),
                        ),
                      ],
                    ),
                  if (c.app != null && c.capabilities != null)
                    widget.section == DesktopSettingsFormSection.profile
                        ? _profileForm()
                        : _appForm(),
                  if (c.app == null &&
                      c.queryState != DesktopSettingsQueryState.loading)
                    const Text('尚未获得有效配置快照，不能编辑。'),
                ],
              ),
            ),
          ),
        ),
      ),
    ),
  );
}

/// 将下拉弹层和对话框保留在设置焦点域内，不触达外层终端 shortcuts。
class _SettingsFormPage extends Page<void> {
  const _SettingsFormPage({required this.child})
    : super(key: const ValueKey('settings-surface'));
  final Widget child;
  @override
  Route<void> createRoute(BuildContext context) => _SettingsFormRoute(this);
}

class _SettingsFormRoute extends PageRoute<void> {
  _SettingsFormRoute(_SettingsFormPage page) : super(settings: page);
  @override
  Color? get barrierColor => null;
  @override
  String? get barrierLabel => null;
  @override
  bool get maintainState => true;
  @override
  Duration get transitionDuration => Duration.zero;
  @override
  Widget buildPage(
    BuildContext context,
    Animation<double> animation,
    Animation<double> secondaryAnimation,
  ) => (settings as _SettingsFormPage).child;
}
