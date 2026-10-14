import 'dart:async';

import 'package:file_selector/file_selector.dart';
import 'package:fluent_ui/fluent_ui.dart';

import '../../bindings/bindings.dart';
import '../../catalog/catalog_requests.dart';
import '../../terminal/session_target.dart';
import '../desktop_status.dart';
import '../desktop_theme.dart';
import '../keys/desktop_key_actions.dart';
import '../keys/desktop_key_dialogs.dart';
import '../keys/desktop_key_sync_state.dart';
import '../workspace/desktop_target_options.dart';
import 'desktop_catalog_gateway.dart';

/// 桌面连接编辑器；快速连接只返回会话目标，不写入目录。
class DesktopConnectionEditor extends StatefulWidget {
  final DesktopCatalogGateway gateway;
  final ConnectionSummary? existing;
  final bool quick;
  final DesktopKeyActions? keyActions;
  final DesktopKeySyncState? keySyncState;

  const DesktopConnectionEditor({
    super.key,
    required this.gateway,
    this.existing,
    this.quick = false,
    this.keyActions,
    this.keySyncState,
    this.targetOptions,
  });

  /// 快速连接由调用方先行查询的传输能力；目录编辑自行查询，不使用此项。
  final DesktopTargetOptions? targetOptions;

  @override
  State<DesktopConnectionEditor> createState() =>
      _DesktopConnectionEditorState();
}

class _DesktopConnectionEditorState extends State<DesktopConnectionEditor> {
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

  /// System OpenSSH 的私钥文件路径；已有连接的值随目录投影读回。
  final _identity = TextEditingController();
  bool _identityEdited = false;

  late DesktopTransport _transport =
      widget.existing?.transport ?? DesktopTransport.nativeSsh;

  /// 系统 ssh 是否可用；null 为尚未查询到。
  late bool? _systemAvailable = widget.quick
      ? widget.targetOptions?.systemAvailable
      : null;
  late String _systemReason = widget.targetOptions?.systemReason ?? '';

  bool get _systemSsh => _transport == DesktopTransport.systemOpenSsh;

  late AuthMethod _auth = widget.existing?.auth ?? AuthMethod.password;
  late bool _savePassword = widget.existing?.passwordSaved ?? true;
  late String _keyId = widget.existing?.keyId ?? '';
  bool _newKeyPending = false;
  late List<KeySummary> _keys =
      widget.keySyncState?.snapshot?.keys ??
      (widget.gateway is RustDesktopCatalogGateway
          ? KeyListState.latestRustSignal?.message.keys
          : null) ??
      const [];
  StreamSubscription<KeyListState>? _keySubscription;
  bool _busy = false;
  String? _error;

  // 终端 profile 引用：空串为应用默认；null 表示已有连接的当前值尚未读到。
  late String? _profileId = widget.existing == null ? '' : null;
  List<DesktopProfileOption> _profiles = const [];
  bool _profilesFailed = false;
  bool _profileIdFailed = false;

  @override
  void initState() {
    super.initState();
    widget.keySyncState?.addListener(_syncChanged);
    if (!widget.quick) {
      _keySubscription = widget.gateway.keyStates.listen((state) {
        if (mounted) setState(() => _keys = state.keys);
      });
      widget.gateway.queryKeys();
      _loadProfiles();
    }
  }

  Future<void> _loadProfiles() async {
    final existing = widget.existing;
    if (existing != null) {
      widget.gateway
          .connection(existing.id)
          .then(
            (connection) {
              if (!mounted) return;
              setState(() {
                // 用户已显式选择或编辑则不覆盖。
                _profileId ??= connection.profileId;
                if (!_identityEdited) {
                  _identity.text = connection.systemIdentityPath;
                }
              });
            },
            onError: (_) {
              if (mounted) setState(() => _profileIdFailed = true);
            },
          );
    }
    try {
      final options = await widget.gateway.targetOptions();
      if (mounted) {
        setState(() {
          _profiles = options.profiles;
          _systemAvailable = options.systemAvailable;
          _systemReason = options.systemReason;
        });
      }
    } catch (_) {
      if (mounted) setState(() => _profilesFailed = true);
    }
  }

  Future<void> _pickIdentity() async {
    final file = await openFile(confirmButtonText: '选择');
    if (!mounted || file == null) return;
    setState(() {
      _identity.text = file.path;
      _identityEdited = true;
    });
  }

  void _syncChanged() {
    if (mounted) setState(() {});
  }

  @override
  void dispose() {
    _keySubscription?.cancel();
    widget.keySyncState?.removeListener(_syncChanged);
    _password.clear();
    for (final controller in [
      _name,
      _host,
      _port,
      _username,
      _password,
      _command,
      _identity,
    ]) {
      controller.dispose();
    }
    super.dispose();
  }

  PasswordAction get _passwordAction {
    if (_auth != AuthMethod.password || !_savePassword) {
      return PasswordAction.clear;
    }
    if (_password.text.isNotEmpty) return PasswordAction.set;
    return (widget.existing?.passwordSaved ?? false)
        ? PasswordAction.keep
        : PasswordAction.set;
  }

  int get _portValue => int.tryParse(_port.text.trim()) ?? 0;

  Future<void> _createKey({required bool card}) async {
    final syncState = widget.keySyncState;
    if (syncState == null || !syncState.canMutate) return;
    final actions = widget.keyActions ?? DesktopKeyActions();
    final id = await showDialog<String>(
      context: context,
      builder: (_) => card
          ? DesktopCardScanDialog(actions: actions, syncState: syncState)
          : DesktopKeyImportDialog(actions: actions, syncState: syncState),
    );
    if (!mounted || id == null || id.isEmpty) return;
    setState(() {
      _keyId = id;
      _newKeyPending = true;
    });
    widget.gateway.queryKeys();
  }

  Future<void> _submit() async {
    if (widget.quick) {
      Navigator.of(context).pop(
        SessionTarget.quick(
          host: _host.text.trim(),
          port: _portValue,
          username: _username.text.trim(),
          // System OpenSSH 由系统 ssh 在终端里询问密码，不携带任何秘密。
          password: _systemSsh ? '' : _password.text,
          command: _command.text.trim(),
          systemSsh: _systemSsh,
        ),
      );
      return;
    }
    setState(() {
      _busy = true;
      _error = null;
    });
    String? error;
    try {
      final result = await widget.gateway.save(
        DesktopConnectionDraft(
          id: widget.existing?.id ?? '',
          name: _name.text,
          host: _host.text,
          port: _portValue,
          username: _username.text,
          auth: _auth,
          passwordAction: _passwordAction,
          password: _passwordAction == PasswordAction.clear
              ? ''
              : _password.text,
          keyId: _auth == AuthMethod.publicKey ? _keyId : '',
          command: _command.text,
          profileId: _profileId ?? '',
          transport: _transport,
          systemIdentityPath: _systemSsh ? _identity.text : '',
        ),
      );
      if (result.error != CatalogError.none) {
        error = catalogErrorText(result.error);
      }
    } on DesktopCatalogRejected catch (rejected) {
      error = rejected.message;
    } on TimeoutException {
      error = '保存超时';
    } catch (_) {
      error = '保存失败，请重试';
    }
    if (!mounted) return;
    if (error == null) {
      Navigator.of(context).pop();
    } else {
      setState(() {
        _busy = false;
        _error = error;
      });
    }
  }

  Widget _field(
    TextEditingController controller,
    String label, {
    String? hint,
    bool secret = false,
  }) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(label, style: FluentTheme.of(context).typography.body),
        const SizedBox(height: DesktopSpacing.sm),
        TextBox(
          key: ValueKey('catalog-field-$label'),
          controller: controller,
          placeholder: hint,
          obscureText: secret,
          autocorrect: false,
          enableSuggestions: false,
          smartDashesType: SmartDashesType.disabled,
          smartQuotesType: SmartQuotesType.disabled,
        ),
      ],
    );
  }

  @override
  Widget build(BuildContext context) {
    final title = widget.quick
        ? '快速连接'
        : widget.existing == null
        ? '添加连接'
        : '编辑连接';
    // 错误与操作按钮常驻页脚，表单再高也不会滚出可视区。
    final footer = Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      mainAxisSize: MainAxisSize.min,
      children: [
        if (_error != null) ...[
          Semantics(
            liveRegion: true,
            child: Text(
              _error!,
              style: FluentTheme.of(context).typography.body,
            ),
          ),
          const SizedBox(height: DesktopSpacing.md),
        ],
        Wrap(
          spacing: DesktopSpacing.md,
          runSpacing: DesktopSpacing.md,
          children: [
            FilledButton(
              // 已有连接的 profile 引用未读到前不保存，避免误清空。
              onPressed: _busy || (!widget.quick && _profileId == null)
                  ? null
                  : _submit,
              child: Text(widget.quick ? '连接' : '保存'),
            ),
            Button(
              onPressed: () => Navigator.of(context).pop(),
              child: const Text('取消'),
            ),
            if (_busy) const ProgressRing(),
          ],
        ),
      ],
    );
    return DesktopPage(
      title: title,
      description: widget.quick
          ? '只用于本次连接，不保存到目录；主机密钥仍须在连接时核对。'
          : '沿用现有 SSH 目录字段；连接时仍会核验主机密钥。',
      footer: footer,
      children: [
        if (!widget.quick) ...[
          _field(_name, '名称', hint: '可选，默认显示 用户名@主机'),
          const SizedBox(height: DesktopSpacing.md),
        ],
        _field(_host, '主机', hint: '主机地址或域名'),
        const SizedBox(height: DesktopSpacing.md),
        _field(_port, '端口'),
        const SizedBox(height: DesktopSpacing.md),
        _field(_username, '用户名'),
        const SizedBox(height: DesktopSpacing.lg),
        Text('连接方式', style: FluentTheme.of(context).typography.body),
        const SizedBox(height: DesktopSpacing.sm),
        ComboBox<DesktopTransport>(
          key: const Key('catalog-transport'),
          value: _transport,
          items: [
            const ComboBoxItem(
              value: DesktopTransport.nativeSsh,
              child: Text('Native SSH'),
            ),
            ComboBoxItem(
              value: DesktopTransport.systemOpenSsh,
              // 已是 System 的连接即使暂时找不到 ssh 也保留原值。
              enabled: _systemAvailable == true || _systemSsh,
              child: const Text('System OpenSSH'),
            ),
          ],
          onChanged: _busy
              ? null
              : (value) {
                  if (value != null) setState(() => _transport = value);
                },
        ),
        if (_systemAvailable == false) ...[
          const SizedBox(height: DesktopSpacing.sm),
          Text(
            'System OpenSSH 不可用：$_systemReason',
            style: FluentTheme.of(context).typography.caption,
          ),
        ],
        const SizedBox(height: DesktopSpacing.lg),
        if (!widget.quick && !_systemSsh) ...[
          Text('认证方式', style: FluentTheme.of(context).typography.body),
          const SizedBox(height: DesktopSpacing.sm),
          ComboBox<AuthMethod>(
            key: const Key('catalog-auth'),
            value: _auth,
            items: const [
              ComboBoxItem(value: AuthMethod.password, child: Text('密码')),
              ComboBoxItem(value: AuthMethod.publicKey, child: Text('私钥')),
              ComboBoxItem(
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
          const SizedBox(height: DesktopSpacing.lg),
        ],
        if (_systemSsh) ...[
          Text(
            '由系统 ssh 处理认证：~/.ssh/config、agent 与 known_hosts 照常生效，'
            '主机密钥确认与密码在终端里输入。',
            style: FluentTheme.of(context).typography.body,
          ),
          const SizedBox(height: DesktopSpacing.md),
        ],
        // 快速连接的 System 目标只用 ssh 配置与 agent，不另设私钥路径。
        if (widget.quick) ...[
          if (!_systemSsh) ...[
            _field(_password, '密码', hint: '可留空，连接时再输入', secret: true),
            const SizedBox(height: DesktopSpacing.md),
          ],
        ] else if (_systemSsh) ...[
          Text('私钥文件（可选）', style: FluentTheme.of(context).typography.body),
          const SizedBox(height: DesktopSpacing.sm),
          Row(
            children: [
              Expanded(
                child: TextBox(
                  key: const ValueKey('catalog-field-私钥文件（可选）'),
                  controller: _identity,
                  placeholder: '留空则由 ssh 配置与 agent 决定',
                  autocorrect: false,
                  enableSuggestions: false,
                  onChanged: (_) => _identityEdited = true,
                ),
              ),
              const SizedBox(width: DesktopSpacing.sm),
              Button(
                onPressed: _busy ? null : _pickIdentity,
                child: const Text('浏览…'),
              ),
            ],
          ),
          const SizedBox(height: DesktopSpacing.md),
        ] else if (_auth == AuthMethod.password) ...[
          ToggleSwitch(
            key: const Key('catalog-save-password'),
            checked: _savePassword,
            onChanged: _busy
                ? null
                : (value) => setState(() => _savePassword = value),
            content: const Text('在钥匙串中保存密码'),
          ),
          const SizedBox(height: DesktopSpacing.sm),
          Text(
            _savePassword ? '连接时自动使用' : '每次连接时询问',
            style: FluentTheme.of(context).typography.caption,
          ),
          if (_savePassword) ...[
            const SizedBox(height: DesktopSpacing.md),
            _field(
              _password,
              '密码',
              hint: widget.existing?.passwordSaved == true
                  ? '已保存；留空则不修改'
                  : null,
              secret: true,
            ),
            const SizedBox(height: DesktopSpacing.md),
          ],
        ] else if (_auth == AuthMethod.publicKey) ...[
          Text('私钥', style: FluentTheme.of(context).typography.body),
          const SizedBox(height: DesktopSpacing.sm),
          ComboBox<String>(
            key: const Key('catalog-key'),
            placeholder: const Text('选择私钥'),
            value:
                _keys.any((key) => key.id == _keyId) ||
                    (_newKeyPending && _keyId.isNotEmpty)
                ? _keyId
                : null,
            items: [
              if (_newKeyPending &&
                  _keyId.isNotEmpty &&
                  !_keys.any((key) => key.id == _keyId))
                ComboBoxItem(value: _keyId, child: const Text('新登记的私钥')),
              for (final key in _keys)
                ComboBoxItem(
                  value: key.id,
                  child: Text(switch ((
                    key.cardIdent.isNotEmpty,
                    key.securityKey,
                  )) {
                    (true, _) => '${key.name}（OpenPGP 卡）',
                    (false, true) => '${key.name}（安全密钥）',
                    _ => '${key.name}（${key.algorithm}）',
                  }),
                ),
            ],
            onChanged: _busy
                ? null
                : (value) => setState(() {
                    _keyId = value ?? '';
                    _newKeyPending = false;
                  }),
          ),
          const SizedBox(height: DesktopSpacing.sm),
          Wrap(
            spacing: DesktopSpacing.sm,
            runSpacing: DesktopSpacing.sm,
            children: [
              Button(
                onPressed: _busy || widget.keySyncState?.canMutate != true
                    ? null
                    : () => _createKey(card: false),
                child: const Text('导入私钥…'),
              ),
              Button(
                onPressed: _busy || widget.keySyncState?.canMutate != true
                    ? null
                    : () => _createKey(card: true),
                child: const Text('添加 OpenPGP 卡…'),
              ),
            ],
          ),
          if (widget.keySyncState?.canMutate != true) ...[
            const SizedBox(height: DesktopSpacing.sm),
            Text(
              '私钥存储位置尚未确认，暂不能导入或登记卡片。',
              style: FluentTheme.of(context).typography.caption,
            ),
          ],
          const SizedBox(height: DesktopSpacing.md),
        ] else ...[
          Text(
            '由服务器逐项提问（例如密码、一次性验证码），连接时回答。',
            style: FluentTheme.of(context).typography.body,
          ),
          const SizedBox(height: DesktopSpacing.md),
        ],
        _field(_command, '命令（可选）', hint: '留空进入 shell'),
        if (!widget.quick) ...[
          const SizedBox(height: DesktopSpacing.md),
          Text('终端配置', style: FluentTheme.of(context).typography.body),
          const SizedBox(height: DesktopSpacing.sm),
          ComboBox<String>(
            key: const Key('catalog-profile'),
            isExpanded: true,
            placeholder: Text(_profileIdFailed ? '未读取到' : '读取中…'),
            value: _profileId,
            items: [
              const ComboBoxItem(value: '', child: Text('使用默认配置')),
              for (final profile in _profiles)
                ComboBoxItem(value: profile.id, child: Text(profile.name)),
              if (_profileId != null &&
                  _profileId!.isNotEmpty &&
                  !_profiles.any((profile) => profile.id == _profileId))
                ComboBoxItem(value: _profileId!, child: const Text('当前配置')),
            ],
            onChanged: _busy
                ? null
                : (value) {
                    if (value != null) setState(() => _profileId = value);
                  },
          ),
          if (_profileIdFailed && _profileId == null) ...[
            const SizedBox(height: DesktopSpacing.sm),
            Text(
              '无法读取该连接当前的终端配置；请重新选择后再保存。',
              style: FluentTheme.of(context).typography.caption,
            ),
          ] else if (_profilesFailed) ...[
            const SizedBox(height: DesktopSpacing.sm),
            Text(
              '无法读取终端配置列表，保存时保留当前选择。',
              style: FluentTheme.of(context).typography.caption,
            ),
          ],
        ],
      ],
    );
  }
}
