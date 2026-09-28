import 'dart:async';

import 'package:fluent_ui/fluent_ui.dart' as f;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../bindings/bindings.dart';
import '../keys/key_import_page.dart';
import '../keys/key_requests.dart';
import '../keys/name_dialog.dart';
import 'desktop_widgets.dart';
import 'windows_chrome.dart';

class WindowsKeys extends StatefulWidget {
  const WindowsKeys({super.key});
  @override
  State<WindowsKeys> createState() => _WindowsKeysState();
}

class _WindowsKeysState extends State<WindowsKeys> {
  KeyListState? _state = KeyListState.latestRustSignal?.message;
  StreamSubscription? _subscription;
  String? _error;
  bool _busy = false;

  @override
  void initState() {
    super.initState();
    _subscription = KeyListState.rustSignalStream.listen((pack) {
      if (mounted) setState(() => _state = pack.message);
    });
    KeyQuery().sendSignalToRust();
  }

  @override
  void dispose() {
    _subscription?.cancel();
    super.dispose();
  }

  Future<void> _request(Future<KeyResult> Function() action) async {
    if (_busy) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final result = await action();
      if (mounted && result.error != KeyError.none) {
        setState(() => _error = keyErrorText(result.error));
      }
    } on TimeoutException {
      if (mounted) setState(() => _error = '操作超时，请重试');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _rename(KeySummary key) async {
    final name = await f.showDialog<String>(
      context: context,
      builder: (_) => NameDialog(title: '重命名私钥', initial: key.name),
    );
    if (name != null && mounted) await _request(() => renameKey(key.id, name));
  }

  Future<void> _delete(KeySummary key) async {
    if (key.usedBy > 0) {
      setState(
        () => _error = '${key.usedBy} 个连接正在使用「${key.name}」，请先更换这些连接的认证方式。',
      );
      return;
    }
    if (await desktopConfirm(
      context,
      '删除私钥？',
      '从当前用户的安全存储中删除「${key.name}」和已保存的口令。',
      action: '删除',
    )) {
      await _request(() => deleteKey(key.id));
    }
  }

  Future<void> _details(KeySummary key) => f.showDialog<void>(
    context: context,
    builder: (context) => f.ContentDialog(
      constraints: const BoxConstraints(maxWidth: 620, maxHeight: 650),
      title: Text(key.name),
      content: SingleChildScrollView(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text(
              '${key.algorithm} · ${key.usedBy} 个连接使用',
              style: const TextStyle(color: desktopMuted),
            ),
            const SizedBox(height: 16),
            const Text('公钥'),
            const SizedBox(height: 8),
            SelectableText(
              key.publicKey,
              style: const TextStyle(fontFamily: 'Consolas', fontSize: 12),
            ),
            const SizedBox(height: 16),
            SelectableText(
              key.fingerprint,
              style: const TextStyle(fontSize: 12, color: desktopMuted),
            ),
            if (key.encrypted)
              Padding(
                padding: const EdgeInsets.only(top: 12),
                child: Text(
                  key.passphraseSaved ? '私钥已加密，口令已安全保存。' : '私钥已加密，连接时询问口令。',
                ),
              ),
            if (key.passphraseSaved)
              Padding(
                padding: const EdgeInsets.only(top: 12),
                child: f.Button(
                  onPressed: () {
                    Navigator.pop(context);
                    _request(() => forgetPassphrase(key.id));
                  },
                  child: const Text('清除已保存的口令'),
                ),
              ),
          ],
        ),
      ),
      actions: [
        f.Button(
          onPressed: () => Navigator.pop(context),
          child: const Text('关闭'),
        ),
        f.FilledButton(
          onPressed: () async {
            await Clipboard.setData(ClipboardData(text: key.publicKey));
            if (context.mounted) Navigator.pop(context);
          },
          child: const Text('复制公钥'),
        ),
      ],
    ),
  );

  @override
  Widget build(BuildContext context) => f.ContentDialog(
    constraints: const BoxConstraints(maxWidth: 740, maxHeight: 740),
    title: const Text('私钥管理'),
    actions: [
      f.Button(
        onPressed: () => Navigator.pop(context),
        child: const Text('关闭'),
      ),
      f.FilledButton(
        onPressed: _busy
            ? null
            : () => f.showDialog<String>(
                context: context,
                builder: (_) => const KeyImportPage(),
              ),
        child: const Text('导入私钥'),
      ),
    ],
    content: SizedBox(
      height: 400,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          const Text(
            '私钥与口令使用当前 Windows 用户的安全存储。',
            style: TextStyle(fontSize: 12, color: desktopMuted),
          ),
          const SizedBox(height: 18),
          if (_error != null)
            f.InfoBar(
              title: const Text('操作未完成'),
              content: Text(_error!),
              severity: f.InfoBarSeverity.error,
              onClose: () => setState(() => _error = null),
            ),
          if (_state?.listError case final error? when error != KeyError.none)
            f.InfoBar(
              title: const Text('无法读取私钥'),
              content: Text(keyErrorText(error)),
              severity: f.InfoBarSeverity.error,
              action: f.Button(
                onPressed: () => KeyQuery().sendSignalToRust(),
                child: const Text('重试'),
              ),
            ),
          if (_busy) const f.ProgressBar(),
          Expanded(
            child: _state == null
                ? const Center(child: f.ProgressRing())
                : _state!.keys.isEmpty
                ? const Center(
                    child: Text(
                      '还没有私钥。导入后可用于服务器认证。',
                      style: TextStyle(color: desktopMuted),
                    ),
                  )
                : ListView(
                    children: [
                      for (final key in _state!.keys)
                        f.ListTile(
                          leading: const Icon(
                            f.FluentIcons.permissions,
                            size: 20,
                            color: desktopAccent,
                          ),
                          title: Text(key.name),
                          subtitle: Text(key.algorithm),
                          onPressed: () => _details(key),
                          trailing: Row(
                            mainAxisSize: MainAxisSize.min,
                            children: [
                              DesktopIconButton(
                                icon: f.FluentIcons.edit,
                                label: '重命名私钥',
                                onPressed: _busy ? null : () => _rename(key),
                              ),
                              DesktopIconButton(
                                icon: f.FluentIcons.delete,
                                label: '删除私钥',
                                onPressed: _busy ? null : () => _delete(key),
                              ),
                            ],
                          ),
                        ),
                    ],
                  ),
          ),
        ],
      ),
    ),
  );
}
