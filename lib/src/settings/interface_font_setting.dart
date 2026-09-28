import 'package:file_selector/file_selector.dart';
import 'package:fluent_ui/fluent_ui.dart' as f;
import 'package:flutter/material.dart';

import 'interface_font.dart';

/// 两处选择器都只接收 TTF 文件，确认应用前不写入偏好。
class InterfaceFontSetting extends StatefulWidget {
  const InterfaceFontSetting({
    super.key,
    this.fluent = false,
    this.terminal = false,
    this.controller,
    this.pickFile,
    this.previewFontSize,
  });
  final bool fluent;
  final bool terminal;
  final InterfaceTypography? controller;
  final Future<XFile?> Function()? pickFile;
  final double? previewFontSize;
  @override
  State<InterfaceFontSetting> createState() => _InterfaceFontSettingState();
}

class _InterfaceFontSettingState extends State<InterfaceFontSetting> {
  InterfaceTypography get _font =>
      widget.controller ??
      (widget.terminal
          ? InterfaceTypography.terminal
          : InterfaceTypography.instance);
  FontDraft? _draft;
  bool _busy = false;
  String? _error;
  String get _slot => widget.terminal ? 'terminal' : 'interface';
  Future<void> _pick() async {
    await _run(() async {
      final file =
          await (widget.pickFile ??
              () => openFile(
                acceptedTypeGroups: const [
                  XTypeGroup(
                    label: 'TrueType 字体',
                    extensions: ['ttf'],
                    mimeTypes: [
                      'font/ttf',
                      'application/x-font-ttf',
                      'application/octet-stream',
                    ],
                    uniformTypeIdentifiers: ['public.font'],
                  ),
                ],
              ))();
      if (file == null) return;
      final draft = await _font.prepare(file);
      if (mounted) setState(() => _draft = draft);
    });
  }

  Future<void> _run(Future<void> Function() action) async {
    if (_busy) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await action();
    } catch (error) {
      if (mounted) {
        setState(
          () => _error = error is StateError ? error.message : '字体操作失败：$error',
        );
      }
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _apply() => _run(() async {
    final draft = _draft;
    if (draft == null) return;
    await _font.apply(draft);
    if (mounted) setState(() => _draft = null);
  });
  Future<void> _reset() => _run(() async {
    await _font.reset();
    if (mounted) setState(() => _draft = null);
  });
  @override
  Widget build(BuildContext context) => AnimatedBuilder(
    animation: _font,
    builder: (context, _) {
      final theme = Theme.of(context);
      final family = _draft?.family ?? _font.family;
      final error = _error ?? _font.error;
      return Padding(
        padding: widget.fluent ? EdgeInsets.zero : const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Row(
              children: [
                Expanded(
                  child: Text(
                    widget.terminal ? '终端字体' : '界面字体',
                    style: const TextStyle(fontWeight: FontWeight.w600),
                  ),
                ),
                _button('选择 TTF 文件', _pick, key: ValueKey('pick-$_slot-font')),
              ],
            ),
            const SizedBox(height: 8),
            Text(
              _draft == null
                  ? '当前：${_font.label}${_font.custom ? '' : '（内置）'}'
                  : '预览：${_draft!.label} · ${_draft!.fileName}',
            ),
            const SizedBox(height: 8),
            Text(
              widget.terminal
                  ? '建议选择等宽字体，检查下方中英文及符号是否对齐。'
                  : '字体文件将保存在应用内，无需在系统中安装。',
              style: const TextStyle(fontSize: 12),
            ),
            Container(
              padding: const EdgeInsets.all(14),
              margin: const EdgeInsets.symmetric(vertical: 12),
              decoration: BoxDecoration(
                color: widget.terminal ? const Color(0xff11151c) : null,
                border: Border.all(color: theme.dividerColor),
                borderRadius: BorderRadius.circular(6),
              ),
              child: Text(
                widget.terminal
                    ? '0123456789  ABCDEFGHIJ\nabcdefghij  ──────────\n中文终端 →   main  ✓'
                    : '清晰的界面，熟悉的工作方式。\nAa Bb 0123456789 · 中文预览',
                key: ValueKey('$_slot-font-preview'),
                style: theme.textTheme.bodyMedium!.copyWith(
                  fontFamily: family,
                  fontSize: widget.previewFontSize,
                  height: 1.6,
                  fontFamilyFallback: widget.terminal
                      ? const ['MesloLGS NF', 'MiSans']
                      : const ['MiSans'],
                ),
              ),
            ),
            if (_busy)
              widget.fluent
                  ? const f.ProgressBar()
                  : const LinearProgressIndicator(),
            Wrap(
              spacing: 8,
              runSpacing: 8,
              alignment: WrapAlignment.end,
              children: [
                _button(widget.terminal ? '恢复内置等宽字体' : '恢复内置 MiSans', _reset),
                if (_draft != null) ...[
                  _button('取消预览', () => setState(() => _draft = null)),
                  _button(
                    '应用字体',
                    _apply,
                    filled: true,
                    key: ValueKey('apply-$_slot-font'),
                  ),
                ],
              ],
            ),
            if (error != null)
              Padding(
                padding: const EdgeInsets.only(top: 12),
                child: Text(
                  error,
                  style: TextStyle(color: theme.colorScheme.error),
                ),
              ),
          ],
        ),
      );
    },
  );
  Widget _button(
    String label,
    VoidCallback action, {
    bool filled = false,
    Key? key,
  }) {
    final callback = _busy ? null : action;
    if (widget.fluent) {
      return filled
          ? f.FilledButton(key: key, onPressed: callback, child: Text(label))
          : f.Button(key: key, onPressed: callback, child: Text(label));
    }
    return filled
        ? FilledButton(key: key, onPressed: callback, child: Text(label))
        : TextButton(key: key, onPressed: callback, child: Text(label));
  }
}
