import 'dart:async';

import 'package:flutter/material.dart';
import 'package:fluent_ui/fluent_ui.dart' as f;

import '../bindings/bindings.dart';
import '../keys/card_scan_page.dart';
import '../keys/key_import_page.dart';
import '../terminal/session_target.dart';
import '../workspace/workspace_page.dart';
import 'catalog_requests.dart';
import '../desktop/windows_chrome.dart';

/// 新建 / 编辑一条连接；`quick` 时是快速连接（不存目录，按钮是「连接」）。
class ConnectionEditorPage extends StatefulWidget {
  /// 要编辑的连接；null 为新建。
  final ConnectionSummary? existing;
  final bool quick;

  /// 快速连接时把目标交回调用方（工作区开新标签、分屏），而不是自己开工作区。
  final bool returnTarget;

  const ConnectionEditorPage({
    super.key,
    this.existing,
    this.quick = false,
    this.returnTarget = false,
  });

  @override
  State<ConnectionEditorPage> createState() => _ConnectionEditorPageState();
}

class _ConnectionEditorPageState extends State<ConnectionEditorPage> {
  late final _name = TextEditingController(text: widget.existing?.name ?? '');
  late final _host = TextEditingController(text: widget.existing?.host ?? '');
  late final _port = TextEditingController(
    text: '${widget.existing?.port ?? 22}',
  );
  late final _username = TextEditingController(
    text: widget.existing?.username ?? '',
  );
  final _password = TextEditingController();
  late final _command = TextEditingController(
    text: widget.existing?.command ?? '',
  );

  late AuthMethod _auth = widget.existing?.auth ?? AuthMethod.password;
  late bool _savePassword = widget.existing?.passwordSaved ?? true;
  late String _keyId = widget.existing?.keyId ?? '';
  List<KeySummary> _keys =
      KeyListState.latestRustSignal?.message.keys ?? const [];
  StreamSubscription? _keysSub;
  bool _busy = false;
  String? _error;

  @override
  void initState() {
    super.initState();
    _keysSub = KeyListState.rustSignalStream.listen((pack) {
      if (mounted) setState(() => _keys = pack.message.keys);
    });
    KeyQuery().sendSignalToRust();
  }

  bool get _editing => widget.existing != null;
  bool get _passwordAlreadySaved => widget.existing?.passwordSaved ?? false;

  @override
  void dispose() {
    _keysSub?.cancel();
    for (final controller in [
      _name,
      _host,
      _port,
      _username,
      _password,
      _command,
    ]) {
      controller.dispose();
    }
    super.dispose();
  }

  Future<void> _importKey() async {
    if (isWindowsDesktop) {
      final id = await f.showDialog<String>(
        context: context,
        builder: (_) => const KeyImportPage(),
      );
      if (id != null && mounted) setState(() => _keyId = id);
      return;
    }
    final id = await Navigator.of(context)
        .push<String>(MaterialPageRoute(builder: (_) => const KeyImportPage()));
    if (id != null && mounted) setState(() => _keyId = id);
  }

  Future<void> _addCard() async {
    final id = await Navigator.of(context)
        .push<String>(MaterialPageRoute(builder: (_) => const CardScanPage()));
    if (id != null && mounted) setState(() => _keyId = id);
  }

  /// 端口按文本解析；解析不了的交给 Rust 判为无效。
  int get _portValue => int.tryParse(_port.text.trim()) ?? 0;

  PasswordAction get _passwordAction {
    if (_auth != AuthMethod.password || !_savePassword) {
      return PasswordAction.clear;
    }
    if (_password.text.isNotEmpty) return PasswordAction.set;
    // 已存过且没改：保留；新建或之前没存：Rust 会要求填写。
    return _passwordAlreadySaved ? PasswordAction.keep : PasswordAction.set;
  }

  Future<void> _save() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    String? error;
    try {
      final result = await saveConnection(
        id: widget.existing?.id ?? '',
        name: _name.text,
        host: _host.text,
        port: _portValue,
        username: _username.text,
        auth: _auth,
        passwordAction: _passwordAction,
        password: _password.text,
        keyId: _auth == AuthMethod.publicKey ? _keyId : '',
        command: _command.text,
      );
      if (result.error != CatalogError.none) {
        error = catalogErrorText(result.error);
      }
    } on TimeoutException {
      error = '保存超时';
    }
    if (!mounted) return;
    if (error == null) {
      Navigator.of(context).pop();
      return;
    }
    setState(() {
      _busy = false;
      _error = error;
    });
  }

  void _connectQuick() {
    final target = SessionTarget.quick(
      host: _host.text.trim(),
      port: _portValue,
      username: _username.text.trim(),
      password: _password.text,
      command: _command.text.trim(),
    );
    if (widget.returnTarget) {
      Navigator.of(context).pop(target);
      return;
    }
    Navigator.of(context).push(
      MaterialPageRoute<void>(builder: (_) => WorkspacePage(initial: target)),
    );
  }

  @override
  Widget build(BuildContext context) {
    final title = widget.quick
        ? '快速连接'
        : _editing
        ? '编辑连接'
        : '添加连接';
    final scheme = Theme.of(context).colorScheme;
    if (isWindowsDesktop) return _desktopEditor(title);
    return Scaffold(
      appBar: AppBar(
        title: Text(title),
        actions: [
          TextButton(
            onPressed: _busy ? null : (widget.quick ? _connectQuick : _save),
            child: Text(widget.quick ? '连接' : '保存'),
          ),
        ],
      ),
      body: SafeArea(
        child: ListView(
          padding: const EdgeInsets.all(20),
          children: [
            if (!widget.quick) ...[
              _field(_name, '名称', hint: '可选，默认显示 用户名@主机'),
              const SizedBox(height: 12),
            ],
            _field(
              _host,
              '主机',
              hint: '192.168.x.x 或域名',
              keyboardType: TextInputType.url,
              autofillHints: const [AutofillHints.url],
            ),
            const SizedBox(height: 12),
            _field(_port, '端口', keyboardType: TextInputType.number),
            const SizedBox(height: 12),
            _field(
              _username,
              '用户名',
              autofillHints: const [AutofillHints.username],
            ),
            const SizedBox(height: 16),
            if (!widget.quick) ...[
              SegmentedButton<AuthMethod>(
                segments: const [
                  ButtonSegment(value: AuthMethod.password, label: Text('密码')),
                  ButtonSegment(value: AuthMethod.publicKey, label: Text('私钥')),
                  ButtonSegment(
                    value: AuthMethod.keyboardInteractive,
                    label: Text('键盘交互'),
                  ),
                ],
                selected: {_auth},
                onSelectionChanged: (selection) =>
                    setState(() => _auth = selection.first),
              ),
              const SizedBox(height: 8),
            ],
            if (widget.quick) ...[
              _field(
                _password,
                '密码',
                hint: '可留空，连接时再输入',
                obscure: true,
                autofillHints: const [AutofillHints.password],
              ),
              const SizedBox(height: 12),
            ] else if (_auth == AuthMethod.password) ...[
              SwitchListTile(
                contentPadding: EdgeInsets.zero,
                title: const Text('在钥匙串中保存密码'),
                subtitle: Text(_savePassword ? '连接时自动使用' : '每次连接时询问'),
                value: _savePassword,
                onChanged: (value) => setState(() => _savePassword = value),
              ),
              if (_savePassword) ...[
                _field(
                  _password,
                  '密码',
                  helper: _passwordAlreadySaved ? '已保存；留空则不修改' : null,
                  obscure: true,
                  autofillHints: const [AutofillHints.password],
                ),
                const SizedBox(height: 12),
              ],
            ] else if (_auth == AuthMethod.publicKey) ...[
              const SizedBox(height: 8),
              DropdownButtonFormField<String>(
                // 导入新私钥后选中它：选中项与列表一变就重建表单项。
                key: ValueKey('$_keyId/${_keys.length}'),
                initialValue: _keys.any((key) => key.id == _keyId)
                    ? _keyId
                    : null,
                isExpanded: true,
                decoration: const InputDecoration(
                  labelText: '私钥',
                  border: OutlineInputBorder(),
                ),
                hint: Text(_keys.isEmpty ? '还没有私钥' : '选择私钥'),
                items: [
                  for (final key in _keys)
                    DropdownMenuItem(
                      value: key.id,
                      child: Text(switch ((
                        key.cardIdent.isNotEmpty,
                        key.securityKey,
                      )) {
                        (true, _) => '${key.name}（OpenPGP 卡）',
                        (false, true) => '${key.name}（安全密钥）',
                        (false, false) => '${key.name}（${key.algorithm}）',
                      }, overflow: TextOverflow.ellipsis),
                    ),
                ],
                onChanged: (id) => setState(() => _keyId = id ?? ''),
              ),
              Align(
                alignment: Alignment.centerRight,
                child: Wrap(
                  alignment: WrapAlignment.end,
                  children: [
                    TextButton.icon(
                      onPressed: _addCard,
                      icon: const Icon(Icons.credit_card),
                      label: const Text('添加 OpenPGP 卡…'),
                    ),
                    TextButton.icon(
                      onPressed: _importKey,
                      icon: const Icon(Icons.add),
                      label: const Text('导入私钥…'),
                    ),
                  ],
                ),
              ),
            ] else ...[
              Text(
                '由服务器逐项提问（例如密码、一次性验证码），连接时回答。',
                style: TextStyle(color: scheme.onSurfaceVariant, fontSize: 13),
              ),
              const SizedBox(height: 12),
            ],
            _field(_command, '命令（可选）', hint: '填了就连上直接执行，如 top；留空进 shell'),
            if (_error != null) ...[
              const SizedBox(height: 16),
              Text(_error!, style: TextStyle(color: scheme.error)),
            ],
          ],
        ),
      ),
    );
  }

  Widget _field(
    TextEditingController controller,
    String label, {
    String? hint,
    String? helper,
    bool obscure = false,
    TextInputType? keyboardType,
    Iterable<String>? autofillHints,
  }) {
    if (isWindowsDesktop) {
      return f.InfoLabel(
        label: label,
        child: f.TextBox(
          controller: controller,
          obscureText: obscure,
          keyboardType: keyboardType,
          placeholder: hint,
          enabled: !_busy,
          autocorrect: false,
          enableSuggestions: false,
        ),
      );
    }
    // 主机、命令等都是要原样发出的文本：iOS 的智能标点会把 `--` 换成破折号。
    return TextField(
      controller: controller,
      obscureText: obscure,
      keyboardType: keyboardType,
      autofillHints: autofillHints,
      autocorrect: false,
      enableSuggestions: false,
      smartDashesType: SmartDashesType.disabled,
      smartQuotesType: SmartQuotesType.disabled,
      decoration: InputDecoration(
        labelText: label,
        hintText: hint,
        helperText: helper,
        border: const OutlineInputBorder(),
      ),
    );
  }

  Widget _desktopEditor(String title) => f.ContentDialog(
    constraints: const BoxConstraints(maxWidth: 620, maxHeight: 730),
    title: Text(title),
    actions: [
      f.Button(
        onPressed: _busy ? null : () => Navigator.pop(context),
        child: const Text('取消'),
      ),
      f.FilledButton(
        onPressed: _busy ? null : (widget.quick ? _connectQuick : _save),
        child: Text(
          _busy
              ? '正在保存…'
              : widget.quick
              ? '连接'
              : '保存',
        ),
      ),
    ],
    content: SingleChildScrollView(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (!widget.quick) ...[
            _field(_name, '连接名称', hint: '可选'),
            const SizedBox(height: 16),
          ],
          Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Expanded(child: _field(_host, '主机', hint: '主机名或 IP 地址')),
              const SizedBox(width: 16),
              SizedBox(
                width: 90,
                child: _field(_port, '端口', keyboardType: TextInputType.number),
              ),
            ],
          ),
          const SizedBox(height: 16),
          _field(_username, '用户名'),
          const SizedBox(height: 20),
          if (!widget.quick) ...[
            f.InfoLabel(
              label: '身份验证',
              child: f.ComboBox<AuthMethod>(
                value: _auth,
                isExpanded: true,
                items: const [
                  f.ComboBoxItem(value: AuthMethod.password, child: Text('密码')),
                  f.ComboBoxItem(
                    value: AuthMethod.publicKey,
                    child: Text('私钥'),
                  ),
                  f.ComboBoxItem(
                    value: AuthMethod.keyboardInteractive,
                    child: Text('键盘交互'),
                  ),
                ],
                onChanged: _busy
                    ? null
                    : (value) {
                        if (value != null) setState(() => _auth = value);
                      },
              ),
            ),
            const SizedBox(height: 16),
          ],
          if (widget.quick)
            _field(_password, '密码', hint: '可留空，连接时询问', obscure: true)
          else if (_auth == AuthMethod.password) ...[
            f.Checkbox(
              checked: _savePassword,
              content: const Text('使用 Windows 凭证管理器保存密码'),
              onChanged: _busy
                  ? null
                  : (value) => setState(() => _savePassword = value ?? false),
            ),
            if (_savePassword) ...[
              const SizedBox(height: 14),
              _field(
                _password,
                '密码',
                hint: _passwordAlreadySaved ? '已保存；留空则保留' : null,
                obscure: true,
              ),
            ],
          ] else if (_auth == AuthMethod.publicKey) ...[
            f.InfoLabel(
              label: '私钥',
              child: f.ComboBox<String>(
                value: _keys.any((key) => key.id == _keyId) ? _keyId : null,
                isExpanded: true,
                placeholder: const Text('选择已导入的私钥'),
                items: [
                  for (final key in _keys)
                    f.ComboBoxItem(
                      value: key.id,
                      child: Text(
                        '${key.name} · ${key.algorithm}',
                        overflow: TextOverflow.ellipsis,
                      ),
                    ),
                ],
                onChanged: _busy
                    ? null
                    : (value) => setState(() => _keyId = value ?? ''),
              ),
            ),
            Align(
              alignment: Alignment.centerRight,
              child: f.HyperlinkButton(
                onPressed: _busy ? null : _importKey,
                child: const Text('导入私钥…'),
              ),
            ),
          ] else
            const Text(
              '连接时按服务器提示输入密码或一次性验证码。',
              style: TextStyle(color: desktopMuted),
            ),
          const SizedBox(height: 20),
          _field(_command, '启动命令（可选）', hint: '留空进入交互式 shell'),
          if (_error != null)
            Padding(
              padding: const EdgeInsets.only(top: 16),
              child: f.InfoBar(
                title: const Text('无法保存连接'),
                content: Text(_error!),
                severity: f.InfoBarSeverity.error,
              ),
            ),
        ],
      ),
    ),
  );
}
