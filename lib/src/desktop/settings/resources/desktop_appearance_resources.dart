import 'package:fluent_ui/fluent_ui.dart';

import '../../desktop_theme.dart';
import 'desktop_resources_contracts.dart';

class DesktopAppearanceResources extends StatefulWidget {
  const DesktopAppearanceResources({
    super.key,
    required this.callbacks,
    required this.inputBoundary,
    required this.enabled,
    required this.onDirtyChanged,
  });
  final DesktopAppearanceCallbacks callbacks;
  final DesktopResourcesInputBoundary inputBoundary;
  final bool enabled;
  final ValueChanged<bool> onDirtyChanged;

  @override
  State<DesktopAppearanceResources> createState() =>
      _AppearanceResourcesState();
}

class _AppearanceResourcesState extends State<DesktopAppearanceResources> {
  late final _raw = TextEditingController(text: _savedRaw);
  DesktopAppearanceValue? _draft;
  bool _writing = false;
  int _version = 0;
  int? _uncertainVersion;
  String? _status;
  void _notifyDirty() => WidgetsBinding.instance.addPostFrameCallback((_) {
    if (mounted) widget.onDirtyChanged(_draft != null);
  });
  DesktopAppearanceCallbacks get c => widget.callbacks;
  String _percentage(DesktopAppearanceValue value) =>
      double.parse(value.transparency.toStringAsFixed(6)).toString();
  String get _savedRaw => _percentage(c.state.value.saved);
  double? get _number {
    final value = double.tryParse(_raw.text);
    return value != null && value.isFinite && value >= 0 && value <= 65
        ? value
        : null;
  }

  @override
  void initState() {
    super.initState();
    widget.inputBoundary.addListener(_boundaryChanged);
    c.state.addListener(_savedChanged);
  }

  void _savedChanged() {
    if (_draft == null && !_writing) _raw.text = _savedRaw;
  }

  void _boundaryChanged() {
    if (!widget.inputBoundary.ready) {
      ++_version;
      c.restoreSavedPreview();
    }
  }

  @override
  void didUpdateWidget(DesktopAppearanceResources oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!oldWidget.callbacks.identity.matches(c.identity)) {
      oldWidget.callbacks.restoreSavedPreview();
      ++_version;
      _draft = null;
      _writing = false;
      _status = null;
      _uncertainVersion = null;
      _raw.text = _savedRaw;
      _notifyDirty();
    }
    if (oldWidget.callbacks.state != c.state) {
      oldWidget.callbacks.state.removeListener(_savedChanged);
      c.state.addListener(_savedChanged);
      _savedChanged();
    }
    if (oldWidget.inputBoundary != widget.inputBoundary) {
      oldWidget.inputBoundary.removeListener(_boundaryChanged);
      widget.inputBoundary.addListener(_boundaryChanged);
      ++_version;
      oldWidget.callbacks.restoreSavedPreview();
    }
  }

  @override
  void dispose() {
    widget.inputBoundary.removeListener(_boundaryChanged);
    c.state.removeListener(_savedChanged);
    c.restoreSavedPreview();
    _raw.dispose();
    super.dispose();
  }

  void _edit({bool? acrylic, double? transparency, bool rawChanged = false}) {
    if (!widget.enabled || !widget.inputBoundary.ready || _writing) return;
    setState(() {
      ++_version;
      _status = null;
      if (transparency != null) _raw.text = transparency.round().toString();
      final previous = _draft ?? c.state.value.saved;
      _draft = DesktopAppearanceValue(
        acrylic: acrylic ?? previous.acrylic,
        opacity: _number == null || (transparency == null && !rawChanged)
            ? previous.opacity
            : 1 - _number! / 100,
      );
    });
    if (_number != null) c.preview(_draft!);
    _notifyDirty();
  }

  Future<void> _save() async {
    if (_writing ||
        !widget.enabled ||
        _draft == null ||
        _number == null ||
        !widget.inputBoundary.ready ||
        (_uncertainVersion != null &&
            c.state.value.verificationVersion <= _uncertainVersion!) ||
        !c.state.value.writeReady) {
      return;
    }
    final callbacks = c;
    final boundary = widget.inputBoundary;
    final generation = boundary.generation;
    final version = _version;
    final verification = c.state.value.verificationVersion;
    final draft = _draft!;
    setState(() {
      _writing = true;
      _status = '正在等待外观持久化确认…';
    });
    DesktopResourceWriteResult<DesktopAppearanceValue> result;
    try {
      result = await callbacks.save(draft);
    } catch (_) {
      result = const DesktopResourceWriteResult.uncertain();
    }
    if (!mounted || !callbacks.identity.matches(c.identity)) return;
    bool current() =>
        version == _version &&
        boundary == widget.inputBoundary &&
        generation == boundary.generation &&
        boundary.ready &&
        widget.enabled;
    if (current() && result.status != DesktopResourceWriteStatus.confirmed) {
      callbacks.restoreSavedPreview();
    }
    setState(() {
      _writing = false;
      if (result.status == DesktopResourceWriteStatus.uncertain) {
        _uncertainVersion = verification;
      }
      if (!current()) {
        _status = null;
        return;
      }
      switch (result.status) {
        case DesktopResourceWriteStatus.confirmed:
          _draft = null;
          _raw.text = _percentage(result.saved!);
          _status = '外观已确认保存，当前窗口即时生效。';
        case DesktopResourceWriteStatus.failed:
          _status = '外观保存失败，窗口预览已回撤，草稿保留；可重试。';
        case DesktopResourceWriteStatus.uncertain:
          _status = '外观结果待核实，草稿保留；保存暂停，不能盲目重发。';
      }
    });
    _notifyDirty();
  }

  void _cancel() {
    if (!widget.enabled || !widget.inputBoundary.ready || _writing) return;
    c.restoreSavedPreview();
    setState(() {
      ++_version;
      _draft = null;
      _raw.text = _savedRaw;
      _status = null;
    });
    _notifyDirty();
  }

  @override
  Widget build(BuildContext context) =>
      ValueListenableBuilder<DesktopAppearanceState>(
        valueListenable: c.state,
        builder: (context, state, _) {
          final draft = _draft ?? state.saved;
          final editable =
              widget.enabled && widget.inputBoundary.ready && !_writing;
          final writable =
              editable &&
              state.writeReady &&
              (_uncertainVersion == null ||
                  state.verificationVersion > _uncertainVersion!);
          return Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Text(
                'Windows 窗口外观',
                style: FluentTheme.of(context).typography.titleLarge,
              ),
              const SizedBox(height: DesktopSpacing.sm),
              const Text('调节即时预览，点击保存后持久化；仅作用于背景，文字与控件保持清晰。'),
              if (MediaQuery.highContrastOf(context))
                const Text('高对比度：当前背景保持不透明；退出高对比度后采用保存值。'),
              if (!state.materialAvailable)
                const Text('当前环境无原生亚克力，使用纯色背景；偏好仍可保存。'),
              const SizedBox(height: DesktopSpacing.md),
              ToggleSwitch(
                key: const ValueKey('resource-acrylic'),
                checked: draft.acrylic,
                content: const Text('亚克力半透明'),
                onChanged: editable ? (value) => _edit(acrylic: value) : null,
              ),
              const SizedBox(height: DesktopSpacing.md),
              InfoLabel(
                label: '背景透明度（0–65%）',
                child: TextBox(
                  key: const ValueKey('resource-transparency'),
                  controller: _raw,
                  enabled: editable && draft.acrylic,
                  keyboardType: const TextInputType.numberWithOptions(
                    decimal: true,
                  ),
                  onChanged: (_) => _edit(rawChanged: true),
                ),
              ),
              Slider(
                key: const ValueKey('resource-transparency-slider'),
                value: (_number ?? draft.transparency).clamp(0, 65).toDouble(),
                min: 0,
                max: 65,
                divisions: 65,
                label: '${(_number ?? draft.transparency).round()}%',
                onChanged: editable && draft.acrylic
                    ? (value) => _edit(transparency: value)
                    : null,
              ),
              if (_number == null)
                Semantics(
                  liveRegion: true,
                  child: const Text('请输入 0–65 的有限数字，原始输入保留；无效输入不会预览或保存。'),
                ),
              const Text('0% 为不透明；透明度上限沿用 Windows 平台约束。'),
              const SizedBox(height: DesktopSpacing.md),
              Wrap(
                spacing: DesktopSpacing.sm,
                runSpacing: DesktopSpacing.sm,
                children: [
                  FilledButton(
                    key: const ValueKey('save-resource-appearance'),
                    onPressed: writable && _draft != null && _number != null
                        ? _save
                        : null,
                    child: const Text('保存外观'),
                  ),
                  Button(
                    key: const ValueKey('cancel-resource-appearance'),
                    onPressed: editable && _draft != null ? _cancel : null,
                    child: const Text('放弃外观预览'),
                  ),
                ],
              ),
              if (_status != null)
                Semantics(liveRegion: true, child: Text(_status!)),
              if (!state.writeReady ||
                  (_uncertainVersion != null &&
                      state.verificationVersion <= _uncertainVersion!))
                Semantics(
                  liveRegion: true,
                  child: const Text('外观写入服务尚有未决结果；普通查询或关闭重开不能解除隔离。'),
                ),
            ],
          );
        },
      );
}
