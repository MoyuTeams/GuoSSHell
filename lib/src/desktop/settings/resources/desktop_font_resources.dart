import 'package:fluent_ui/fluent_ui.dart';

import '../../../settings/interface_font.dart';
import '../../desktop_theme.dart';
import 'desktop_resources_contracts.dart';

class DesktopFontResources extends StatefulWidget {
  const DesktopFontResources({
    super.key,
    required this.callbacks,
    required this.inputBoundary,
    required this.enabled,
    required this.onDirtyChanged,
  });
  final DesktopFontResourceCallbacks callbacks;
  final DesktopResourcesInputBoundary inputBoundary;
  final bool enabled;
  final ValueChanged<bool> onDirtyChanged;

  @override
  State<DesktopFontResources> createState() => _FontResourcesState();
}

class _FontResourcesState extends State<DesktopFontResources> {
  FontDraft? _draft;
  int _version = 0;
  bool _picking = false;
  bool _writing = false;
  int? _uncertainVersion;
  String? _status;
  DesktopFontResourceCallbacks get c => widget.callbacks;
  String get slot => c.slot.name;

  void _notifyDirty() => WidgetsBinding.instance.addPostFrameCallback((_) {
    if (mounted) widget.onDirtyChanged(_draft != null);
  });

  @override
  void didUpdateWidget(DesktopFontResources oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!oldWidget.callbacks.identity.matches(c.identity) ||
        oldWidget.callbacks.slot != c.slot) {
      ++_version;
      _draft = null;
      _picking = _writing = false;
      _status = null;
      _uncertainVersion = null;
      _notifyDirty();
    } else if (oldWidget.inputBoundary != widget.inputBoundary) {
      ++_version;
      _picking = false;
      _status = null;
    }
  }

  Future<void> _pick() async {
    if (!widget.enabled || !widget.inputBoundary.ready || _writing) return;
    final callbacks = c;
    final boundary = widget.inputBoundary;
    final generation = boundary.generation;
    final version = ++_version;
    bool current() =>
        mounted &&
        version == _version &&
        callbacks.identity.matches(c.identity) &&
        callbacks.slot == c.slot &&
        boundary == widget.inputBoundary &&
        boundary.ready &&
        widget.enabled &&
        generation == boundary.generation;
    setState(() {
      _picking = true;
      _status = null;
    });
    try {
      final file = await callbacks.pick(
        context,
        DesktopTtfPickRequest(callbacks.slot),
      );
      if (!current() || file == null) return;
      final draft = await callbacks.prepare(file);
      if (current()) {
        setState(() {
          _draft = draft;
          _status = '字体校验通过，仅本地预览，尚未保存。';
        });
        _notifyDirty();
      }
    } catch (_) {
      if (current()) setState(() => _status = '字体选择或校验失败，保留原选择；可重新选择。');
    } finally {
      if (mounted && version == _version) setState(() => _picking = false);
    }
  }

  Future<void> _write({required bool reset}) async {
    final callbacks = c;
    final draft = _draft;
    final version = _version;
    final boundary = widget.inputBoundary;
    final generation = boundary.generation;
    final verification = callbacks.state.value.verificationVersion;
    if (_writing ||
        !widget.enabled ||
        !boundary.ready ||
        !callbacks.state.value.writeReady ||
        (_uncertainVersion != null && verification <= _uncertainVersion!) ||
        (!reset && draft == null)) {
      return;
    }
    setState(() {
      _writing = true;
      _status = '正在等待字体持久化确认…';
    });
    DesktopResourceWriteResult<DesktopFontSlotState> result;
    try {
      result = reset ? await callbacks.reset() : await callbacks.apply(draft!);
    } catch (_) {
      result = const DesktopResourceWriteResult.uncertain();
    }
    if (!mounted ||
        !callbacks.identity.matches(c.identity) ||
        callbacks.slot != c.slot) {
      return;
    }
    setState(() {
      _writing = false;
      if (result.status == DesktopResourceWriteStatus.uncertain) {
        _uncertainVersion = verification;
      }
      if (version != _version ||
          boundary != widget.inputBoundary ||
          generation != boundary.generation ||
          !boundary.ready ||
          !widget.enabled) {
        _status = null;
        return;
      }
      switch (result.status) {
        case DesktopResourceWriteStatus.confirmed:
          _draft = null;
          _status = reset ? '已确认恢复内置字体；持久字体缓存保留。' : '字体已确认保存；实际生效字体见上方。';
        case DesktopResourceWriteStatus.failed:
          _status = '字体保存失败，保留预览草稿；确认后可重试。';
        case DesktopResourceWriteStatus.uncertain:
          _status = '字体结果待核实，保留草稿；不能盲目重发。';
      }
    });
    _notifyDirty();
  }

  void _cancel() {
    if (!widget.enabled || !widget.inputBoundary.ready || _writing) return;
    setState(() {
      ++_version;
      _draft = null;
      _picking = false;
      _status = null;
      _notifyDirty();
    });
  }

  @override
  Widget build(
    BuildContext context,
  ) => ValueListenableBuilder<DesktopFontSlotState>(
    valueListenable: c.state,
    builder: (context, saved, _) {
      final terminal = c.slot == DesktopFontSlot.terminal;
      final ready = widget.enabled && widget.inputBoundary.ready && !_writing;
      final writable =
          ready &&
          saved.writeReady &&
          (_uncertainVersion == null ||
              saved.verificationVersion > _uncertainVersion!);
      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            terminal ? '全局终端字体' : '全局界面字体',
            style: FluentTheme.of(context).typography.titleLarge,
          ),
          const SizedBox(height: DesktopSpacing.sm),
          Text('已保存：${saved.savedLabel}'),
          Text('实际使用：${saved.effectiveFamily}'),
          if (saved.fallbackReason != null) Text(saved.fallbackReason!),
          Text(
            terminal
                ? '应用后全局槽与旧版终端即时生效；显式 profile 不改变，继承槽的 profile 在新会话采用。'
                : '应用后界面即时生效；选择和预览不会修改已保存字体。',
          ),
          const SizedBox(height: DesktopSpacing.md),
          Container(
            padding: const EdgeInsets.all(DesktopSpacing.lg),
            decoration: BoxDecoration(
              border: Border.all(
                color: MediaQuery.highContrastOf(context)
                    ? DesktopColors.highContrastText
                    : DesktopColors.border,
              ),
              borderRadius: BorderRadius.circular(DesktopMetrics.radius),
            ),
            child: Text(
              terminal
                  ? '0123456789  ABCDEFGHIJ\nabcdefghij  ──────────\n中文终端 →  main  ✓'
                  : '清晰的界面，熟悉的工作方式。\nAa Bb 0123456789 · 中文预览',
              key: ValueKey('$slot-resource-preview'),
              style: FluentTheme.of(context).typography.body!.copyWith(
                fontFamily: _draft?.family ?? saved.effectiveFamily,
                fontFamilyFallback: terminal
                    ? const ['MesloLGS NF', ...DesktopType.fallback]
                    : DesktopType.fallback,
              ),
            ),
          ),
          if (_draft != null) Text('预览：${_draft!.label} · ${_draft!.fileName}'),
          if (_picking)
            Semantics(liveRegion: true, child: const Text('正在选择并校验字体；可取消或改选。')),
          const SizedBox(height: DesktopSpacing.md),
          Wrap(
            spacing: DesktopSpacing.sm,
            runSpacing: DesktopSpacing.sm,
            children: [
              Button(
                key: ValueKey('pick-$slot-resource'),
                onPressed: ready ? _pick : null,
                child: const Text('选择 TTF 文件'),
              ),
              Button(
                key: ValueKey('reset-$slot-resource'),
                onPressed: writable && !_picking
                    ? () => _write(reset: true)
                    : null,
                child: const Text('恢复内置字体'),
              ),
              if (_draft != null || _picking)
                Button(
                  key: ValueKey('cancel-$slot-resource'),
                  onPressed: ready ? _cancel : null,
                  child: const Text('取消预览'),
                ),
              FilledButton(
                key: ValueKey('apply-$slot-resource'),
                onPressed: writable && !_picking && _draft != null
                    ? () => _write(reset: false)
                    : null,
                child: const Text('应用并保存字体'),
              ),
            ],
          ),
          if (_status != null)
            Semantics(liveRegion: true, child: Text(_status!)),
          if (!saved.writeReady ||
              (_uncertainVersion != null &&
                  saved.verificationVersion <= _uncertainVersion!))
            Semantics(
              liveRegion: true,
              child: const Text('父写入服务尚有未决结果；可预览，保存与重置等待核实。'),
            ),
        ],
      );
    },
  );
}
