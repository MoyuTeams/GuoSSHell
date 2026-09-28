import 'dart:async';

import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';
import 'package:fluent_ui/fluent_ui.dart' as f;

import '../desktop/windows_chrome.dart';

import '../bindings/bindings.dart';
import 'key_requests.dart';

/// 私钥文件不会超过这个大小；超过的不当作私钥读。
const _maxKeyFileBytes = 64 * 1024;

/// 导入私钥：粘贴原文或从文件选择。成功后返回私钥 id。
class KeyImportPage extends StatefulWidget {
  final Future<XFile?> Function()? pickFile;

  const KeyImportPage({super.key, this.pickFile});

  @override
  State<KeyImportPage> createState() => _KeyImportPageState();
}

class _KeyImportPageState extends State<KeyImportPage> {
  final _name = TextEditingController();
  final _key = TextEditingController();
  final _passphrase = TextEditingController();
  final _passphraseFocus = FocusNode();
  bool _busy = false;
  String? _error;

  @override
  void dispose() {
    _name.dispose();
    _key.dispose();
    _passphrase.dispose();
    _passphraseFocus.dispose();
    super.dispose();
  }

  Future<void> _pickFile() async {
    if (_busy) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final file = await (widget.pickFile ?? () => openFile())();
      if (file == null) return;
      if (await file.length() > _maxKeyFileBytes) {
        if (mounted) setState(() => _error = '文件太大，不像是私钥');
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

  Future<void> _import() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    KeyResult? result;
    String? error;
    try {
      result = await importKey(
        name: _name.text,
        privateKey: _key.text,
        passphrase: _passphrase.text,
      );
      if (result.error != KeyError.none) error = keyErrorText(result.error);
    } on TimeoutException {
      error = '导入超时';
    }
    if (!mounted) return;
    if (error == null) {
      Navigator.of(context).pop(result!.keyId);
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
  Widget build(BuildContext context) {
    if (isWindowsDesktop) return _desktop();
    final scheme = Theme.of(context).colorScheme;
    return Scaffold(
      appBar: AppBar(
        title: const Text('导入私钥'),
        actions: [
          TextButton(
            onPressed: _busy ? null : _import,
            child: const Text('导入'),
          ),
        ],
      ),
      body: SafeArea(
        child: ListView(
          padding: const EdgeInsets.all(20),
          children: [
            TextField(
              controller: _name,
              decoration: const InputDecoration(
                labelText: '名称',
                hintText: '可选，默认用私钥里的注释',
                border: OutlineInputBorder(),
              ),
            ),
            const SizedBox(height: 16),
            Row(
              children: [
                const Text('私钥'),
                const Spacer(),
                TextButton.icon(
                  onPressed: _busy ? null : _pickFile,
                  icon: const Icon(Icons.folder_open),
                  label: const Text('从文件选择'),
                ),
              ],
            ),
            TextField(
              controller: _key,
              minLines: 6,
              maxLines: 12,
              keyboardType: TextInputType.multiline,
              autocorrect: false,
              enableSuggestions: false,
              // 私钥要逐字保留：iOS 的智能标点会把 `-----` 换成破折号。
              smartDashesType: SmartDashesType.disabled,
              smartQuotesType: SmartQuotesType.disabled,
              style: const TextStyle(fontFamily: 'Menlo', fontSize: 11),
              decoration: const InputDecoration(
                hintText: '-----BEGIN OPENSSH PRIVATE KEY-----\n…',
                border: OutlineInputBorder(),
              ),
            ),
            const SizedBox(height: 16),
            TextField(
              controller: _passphrase,
              focusNode: _passphraseFocus,
              obscureText: true,
              autocorrect: false,
              enableSuggestions: false,
              decoration: const InputDecoration(
                labelText: '口令（可选）',
                helperText: '私钥有口令保护时填写；OpenSSH 格式可以留空，连接时再输入',
                helperMaxLines: 2,
                border: OutlineInputBorder(),
              ),
            ),
            if (_error != null) ...[
              const SizedBox(height: 16),
              Text(_error!, style: TextStyle(color: scheme.error)),
            ],
            const SizedBox(height: 16),
            Text(
              '私钥只保存在钥匙串里。',
              style: TextStyle(color: scheme.onSurfaceVariant, fontSize: 13),
            ),
          ],
        ),
      ),
    );
  }

  Widget _desktop() => f.ContentDialog(
    constraints: const BoxConstraints(maxWidth: 600, maxHeight: 700),
    title: const Text('导入私钥'),
    actions: [
      f.Button(
        onPressed: _busy ? null : () => Navigator.pop(context),
        child: const Text('取消'),
      ),
      f.FilledButton(
        onPressed: _busy ? null : _import,
        child: Text(_busy ? '正在导入…' : '导入'),
      ),
    ],
    content: SingleChildScrollView(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          f.InfoLabel(
            label: '名称',
            child: f.TextBox(
              controller: _name,
              enabled: !_busy,
              placeholder: '可选，默认使用私钥注释',
            ),
          ),
          const SizedBox(height: 16),
          Row(
            children: [
              const Expanded(child: Text('私钥')),
              f.HyperlinkButton(
                onPressed: _busy ? null : _pickFile,
                child: const Text('从文件选择'),
              ),
            ],
          ),
          f.TextBox(
            controller: _key,
            enabled: !_busy,
            minLines: 5,
            maxLines: 8,
            autocorrect: false,
            enableSuggestions: false,
            style: const TextStyle(fontFamily: 'Consolas', fontSize: 12),
            placeholder: 'PEM 或 OpenSSH 私钥',
          ),
          const SizedBox(height: 16),
          f.InfoLabel(
            label: '口令（可选）',
            child: f.TextBox(
              controller: _passphrase,
              focusNode: _passphraseFocus,
              enabled: !_busy,
              obscureText: true,
              autocorrect: false,
              enableSuggestions: false,
            ),
          ),
          const SizedBox(height: 16),
          const Text(
            '私钥由 Windows DPAPI 加密后存储在当前用户的数据目录中。',
            style: TextStyle(fontSize: 12, color: desktopMuted),
          ),
          if (_error != null)
            Padding(
              padding: const EdgeInsets.only(top: 16),
              child: f.InfoBar(
                title: const Text('导入失败'),
                content: Text(_error!),
                severity: f.InfoBarSeverity.error,
              ),
            ),
        ],
      ),
    ),
  );
}
