import 'dart:async';
import 'dart:math' as math;

import 'package:fluent_ui/fluent_ui.dart';

import '../../bindings/bindings.dart';
import '../../keys/key_requests.dart' show keyErrorText;
import '../desktop_theme.dart';
import 'desktop_key_actions.dart';
import 'desktop_key_sync_state.dart';

Future<String?> desktopKeyNameDialog(
  BuildContext context, {
  required String title,
  required String initial,
  String confirm = '保存',
}) => showDialog<String>(
  context: context,
  builder: (_) => _NameDialog(title: title, initial: initial, confirm: confirm),
);

Future<bool> desktopKeyConfirm(
  BuildContext context, {
  required String title,
  required String detail,
  required String action,
}) async =>
    await showDialog<bool>(
      context: context,
      builder: (_) => ContentDialog(
        title: Text(title),
        content: Text(detail),
        actions: [
          Button(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('取消'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, true),
            child: Text(action),
          ),
        ],
      ),
    ) ??
    false;

class _NameDialog extends StatefulWidget {
  const _NameDialog({
    required this.title,
    required this.initial,
    required this.confirm,
  });
  final String title;
  final String initial;
  final String confirm;

  @override
  State<_NameDialog> createState() => _NameDialogState();
}

class _NameDialogState extends State<_NameDialog> {
  late final _name = TextEditingController(text: widget.initial);

  @override
  void dispose() {
    _name.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => ContentDialog(
    title: Text(widget.title),
    content: TextBox(
      controller: _name,
      autofocus: true,
      placeholder: '名称',
      onSubmitted: (_) => Navigator.pop(context, _name.text),
    ),
    actions: [
      Button(onPressed: () => Navigator.pop(context), child: const Text('取消')),
      FilledButton(
        onPressed: () => Navigator.pop(context, _name.text),
        child: Text(widget.confirm),
      ),
    ],
  );
}

/// 原文仅保存在编辑器中，提交给请求层；不在错误文案或状态提示中回显。
class DesktopKeyImportDialog extends StatefulWidget {
  const DesktopKeyImportDialog({
    super.key,
    required this.actions,
    required this.syncState,
  });
  final DesktopKeyActions actions;
  final DesktopKeySyncState syncState;

  @override
  State<DesktopKeyImportDialog> createState() => _DesktopKeyImportDialogState();
}

class _DesktopKeyImportDialogState extends State<DesktopKeyImportDialog> {
  final _name = TextEditingController();
  final _key = TextEditingController();
  final _passphrase = TextEditingController();
  final _passphraseFocus = FocusNode();
  bool _busy = false;
  String? _error;

  @override
  void initState() {
    super.initState();
    widget.syncState.addListener(_syncChanged);
  }

  void _syncChanged() {
    if (mounted) setState(() {});
  }

  @override
  void dispose() {
    widget.syncState.removeListener(_syncChanged);
    _name.dispose();
    _key.clear();
    _passphrase.clear();
    _key.dispose();
    _passphrase.dispose();
    _passphraseFocus.dispose();
    super.dispose();
  }

  Future<void> _pick() async {
    if (_busy) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final file = await widget.actions.pickFile();
      if (file == null) return;
      if (await file.length() > 64 * 1024) {
        if (mounted) {
          setState(() => _error = '文件太大，不像是私钥');
        }
        return;
      }
      final text = await file.readAsString();
      if (!mounted) return;
      setState(() {
        _key.text = text;
        if (_name.text.isEmpty) _name.text = file.name;
      });
    } on FormatException {
      if (mounted) {
        setState(() => _error = '文件不是有效的 UTF-8 文本，请选择 PEM 或 OpenSSH 私钥文件');
      }
    } catch (_) {
      if (mounted) setState(() => _error = '无法读取文件，请确认文件可用后重试');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  /// 掩码框只显示单行；直接写入控制器，避免文本输入格式器移除 PEM 换行。
  Future<void> _paste() async {
    if (_busy) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final text = await widget.actions.readClipboard();
      if (!mounted || text == null) return;
      final value = _key.value;
      final selection = value.selection.isValid
          ? value.selection
          : TextSelection(baseOffset: 0, extentOffset: value.text.length);
      _key.value = value.replaced(selection, text);
    } catch (_) {
      if (mounted) setState(() => _error = '无法读取剪贴板，请重试或从文件选择');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _submit() async {
    if (_busy || !widget.syncState.canMutate) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    KeyResult? result;
    String? error;
    try {
      result = await widget.actions.import(
        _name.text,
        _key.text,
        _passphrase.text,
      );
      if (result.error != KeyError.none) error = keyErrorText(result.error);
    } on TimeoutException {
      error = '导入超时';
    } catch (_) {
      error = '导入失败，请重试';
    }
    if (!mounted) return;
    if (error == null) {
      Navigator.pop(context, result!.keyId);
      return;
    }
    setState(() {
      _busy = false;
      _error = error;
    });
    if (result?.error == KeyError.passphraseRequired ||
        result?.error == KeyError.passphraseWrong) {
      _passphraseFocus.requestFocus();
    }
  }

  @override
  Widget build(BuildContext context) => ContentDialog(
    title: const Text('导入私钥'),
    content: SizedBox(
      width: math.min(
        DesktopMetrics.contentWidth,
        MediaQuery.sizeOf(context).width - DesktopSpacing.xxl * 2,
      ),
      child: SingleChildScrollView(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          mainAxisSize: MainAxisSize.min,
          children: [
            const Text('名称（可选，默认用私钥里的注释）'),
            const SizedBox(height: DesktopSpacing.sm),
            TextBox(controller: _name, enabled: !_busy, placeholder: '名称'),
            const SizedBox(height: DesktopSpacing.lg),
            Wrap(
              spacing: DesktopSpacing.sm,
              runSpacing: DesktopSpacing.sm,
              crossAxisAlignment: WrapCrossAlignment.center,
              children: [
                const Text('私钥原文'),
                Button(
                  onPressed: _busy ? null : _paste,
                  child: const Text('粘贴私钥'),
                ),
                Button(
                  onPressed: _busy ? null : _pick,
                  child: const Text('从文件选择'),
                ),
              ],
            ),
            const SizedBox(height: DesktopSpacing.sm),
            Actions(
              actions: {
                PasteTextIntent: CallbackAction<PasteTextIntent>(
                  onInvoke: (_) {
                    unawaited(_paste());
                    return null;
                  },
                ),
              },
              child: TextBox(
                controller: _key,
                enabled: !_busy,
                // 多行密钥只经粘贴或文件更新，不能让单行输入格式器改写既有换行。
                readOnly: _key.text.contains('\n'),
                enableInteractiveSelection: true,
                obscureText: true,
                placeholder: '粘贴 OpenSSH、PEM 或 PKCS#8 私钥',
                autocorrect: false,
                enableSuggestions: false,
                smartDashesType: SmartDashesType.disabled,
                smartQuotesType: SmartQuotesType.disabled,
                contextMenuBuilder: (_, editable) =>
                    WindowsTextSelectionToolbar(
                      anchors: editable.contextMenuAnchors,
                      buttonItems: [
                        ContextMenuButtonItem(
                          type: ContextMenuButtonType.paste,
                          onPressed: () {
                            editable.hideToolbar();
                            unawaited(_paste());
                          },
                        ),
                        ...editable.contextMenuButtonItems.where(
                          (item) => item.type != ContextMenuButtonType.paste,
                        ),
                        if (editable.widget.undoController case final undo?)
                          UndoContextMenuButtonItem(onPressed: undo.undo),
                      ],
                    ),
              ),
            ),
            const SizedBox(height: DesktopSpacing.lg),
            const Text('口令（可选）'),
            const SizedBox(height: DesktopSpacing.sm),
            TextBox(
              controller: _passphrase,
              focusNode: _passphraseFocus,
              enabled: !_busy,
              obscureText: true,
              autocorrect: false,
              enableSuggestions: false,
            ),
            const SizedBox(height: DesktopSpacing.sm),
            Text(
              '有口令保护时填写；OpenSSH 格式可留空，连接时再输入。',
              style: FluentTheme.of(context).typography.caption,
            ),
            if (_error != null) ...[
              const SizedBox(height: DesktopSpacing.lg),
              Semantics(liveRegion: true, child: Text(_error!)),
            ],
            if (!widget.syncState.canMutate) ...[
              const SizedBox(height: DesktopSpacing.lg),
              const Text('存储位置尚未确认，暂不能导入私钥。'),
            ],
            const SizedBox(height: DesktopSpacing.lg),
            const Text('私钥与保存的口令只交给钥匙串，不在此页显示导入后的原文。'),
          ],
        ),
      ),
    ),
    actions: [
      Button(
        onPressed: _busy ? null : () => Navigator.pop(context),
        child: const Text('取消'),
      ),
      FilledButton(
        onPressed: _busy || !widget.syncState.canMutate ? null : _submit,
        child: Text(_busy ? '处理中…' : '导入'),
      ),
    ],
  );
}

/// 读卡结果中的 NFC 能力和认证槽是否可用均以请求返回为准。
class DesktopCardScanDialog extends StatefulWidget {
  const DesktopCardScanDialog({
    super.key,
    required this.actions,
    required this.syncState,
  });
  final DesktopKeyActions actions;
  final DesktopKeySyncState syncState;

  @override
  State<DesktopCardScanDialog> createState() => _DesktopCardScanDialogState();
}

class _DesktopCardScanDialogState extends State<DesktopCardScanDialog> {
  bool _busy = false;
  bool _nfcAvailable = false;
  List<CardSummary>? _cards;
  String? _error;

  @override
  void initState() {
    super.initState();
    widget.syncState.addListener(_syncChanged);
    _scan(false);
  }

  void _syncChanged() {
    if (mounted) setState(() {});
  }

  @override
  void dispose() {
    widget.syncState.removeListener(_syncChanged);
    super.dispose();
  }

  Future<void> _scan(bool nfc) async {
    if (_busy) return;
    setState(() {
      _busy = true;
      _error = null;
      _cards = null;
    });
    try {
      final result = await widget.actions.scan(nfc);
      if (!mounted) return;
      setState(() {
        _nfcAvailable = result.nfcAvailable;
        if (result.error == KeyError.none) {
          _cards = result.cards;
        } else {
          _error = keyErrorText(result.error);
        }
      });
    } on TimeoutException {
      if (mounted) setState(() => _error = '读卡超时');
    } catch (_) {
      if (mounted) setState(() => _error = '读卡失败，请重试');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _add(CardSummary card) async {
    if (_busy || !widget.syncState.canMutate) return;
    final name = await desktopKeyNameDialog(
      context,
      title: '添加 OpenPGP 卡',
      initial: card.cardholder.isNotEmpty
          ? card.cardholder
          : 'OpenPGP 卡 ${card.ident}',
      confirm: '添加',
    );
    if (!mounted || name == null || !widget.syncState.canMutate) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final result = await widget.actions.addCard(card.ident, name);
      if (!mounted) return;
      if (result.error == KeyError.none) {
        Navigator.pop(context, result.keyId);
      } else {
        setState(() => _error = keyErrorText(result.error));
      }
    } on TimeoutException {
      if (mounted) setState(() => _error = '操作超时');
    } catch (_) {
      if (mounted) setState(() => _error = '登记失败，请重试');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) => ContentDialog(
    title: const Text('添加 OpenPGP 卡'),
    content: SizedBox(
      width: math.min(
        DesktopMetrics.contentWidth,
        MediaQuery.sizeOf(context).width - DesktopSpacing.xxl * 2,
      ),
      child: SingleChildScrollView(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          mainAxisSize: MainAxisSize.min,
          children: [
            const Text('插入 OpenPGP 卡，登记认证槽公钥；私钥始终留在卡上。登录时需插卡并输入 PIN。'),
            const SizedBox(height: DesktopSpacing.lg),
            if (_busy) const ProgressRing(),
            if (_error != null)
              Semantics(liveRegion: true, child: Text(_error!)),
            if (!widget.syncState.canMutate) const Text('存储位置尚未确认，暂不能登记卡片。'),
            if (_cards != null && _cards!.isEmpty && !_busy)
              const Text('没有找到 OpenPGP 卡。插卡后重新读取。'),
            for (final card in _cards ?? const <CardSummary>[]) ...[
              const SizedBox(height: DesktopSpacing.md),
              Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  const Icon(
                    FluentIcons.contact_card,
                    size: DesktopMetrics.icon,
                  ),
                  const SizedBox(width: DesktopSpacing.md),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          card.cardholder.isNotEmpty
                              ? card.cardholder
                              : 'OpenPGP 卡',
                          style: FluentTheme.of(context).typography.titleLarge,
                        ),
                        Text('卡号 ${card.ident} · ${card.algorithm}'),
                        if (card.fingerprint.isNotEmpty)
                          SelectableText(card.fingerprint),
                        Text(
                          'PIN 剩余 ${card.pinTriesLeft} 次${card.touch ? ' · 需要触摸确认' : ''}',
                        ),
                      ],
                    ),
                  ),
                  if (card.added)
                    const Text('已添加')
                  else if (card.publicKey.isEmpty)
                    const Text('认证槽不可用')
                  else
                    Button(
                      onPressed: _busy || !widget.syncState.canMutate
                          ? null
                          : () => _add(card),
                      child: const Text('添加'),
                    ),
                ],
              ),
            ],
          ],
        ),
      ),
    ),
    actions: [
      Button(
        onPressed: _busy ? null : () => Navigator.pop(context),
        child: const Text('关闭'),
      ),
      Button(
        onPressed: _busy ? null : () => _scan(false),
        child: const Text('重新读取'),
      ),
      if (_nfcAvailable)
        Button(
          onPressed: _busy ? null : () => _scan(true),
          child: const Text('通过 NFC 读取'),
        ),
    ],
  );
}
