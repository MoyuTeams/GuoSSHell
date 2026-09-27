import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../bindings/bindings.dart';
import 'card_scan_page.dart';
import 'key_import_page.dart';
import 'key_requests.dart';
import 'name_dialog.dart';

/// 私钥：列表、导入私钥、登记 OpenPGP 卡与安全密钥、查看公钥、改名、删除，以及 iCloud 钥匙串
/// 同步开关。
class KeysPage extends StatefulWidget {
  final VoidCallback? queryKeys;
  final Future<KeyResult> Function(bool enabled)? setSync;

  const KeysPage({super.key, this.queryKeys, this.setSync});

  @override
  State<KeysPage> createState() => _KeysPageState();
}

class _KeysPageState extends State<KeysPage> {
  StreamSubscription? _sub;
  KeyListState? _state = KeyListState.latestRustSignal?.message;
  bool _syncBusy = false;
  bool? _unconfirmedSyncTarget;
  bool _registering = false;

  @override
  void initState() {
    super.initState();
    _sub = KeyListState.rustSignalStream.listen((pack) {
      if (!mounted) return;
      setState(() {
        _state = pack.message;
        if (!pack.message.syncPending &&
            pack.message.syncEnabled == _unconfirmedSyncTarget) {
          _unconfirmedSyncTarget = null;
        }
      });
    });
    _queryKeys();
  }

  void _queryKeys() {
    if (widget.queryKeys case final query?) {
      query();
    } else {
      KeyQuery().sendSignalToRust();
    }
  }

  @override
  void dispose() {
    _sub?.cancel();
    super.dispose();
  }

  void _snack(String message) {
    ScaffoldMessenger.of(context)
        .showSnackBar(SnackBar(content: Text(message)));
  }

  Future<void> _run(Future<KeyResult> Function() request) async {
    String? message;
    try {
      final result = await request();
      if (result.error != KeyError.none) message = keyErrorText(result.error);
    } on TimeoutException {
      message = '操作超时';
    }
    if (message != null && mounted) _snack(message);
  }

  Future<void> _import() async {
    await Navigator.of(context)
        .push<String>(MaterialPageRoute(builder: (_) => const KeyImportPage()));
  }

  Future<void> _addCard() async {
    await Navigator.of(context)
        .push<String>(MaterialPageRoute(builder: (_) => const CardScanPage()));
  }

  /// 在安全密钥上新建一把凭据：先起名，再由系统界面引导用户插上（或靠近）并触摸安全密钥。
  Future<void> _addSecurityKey() async {
    final name = await showDialog<String>(
      context: context,
      builder: (_) =>
          const NameDialog(title: '添加安全密钥', initial: '', confirm: '继续'),
    );
    if (name == null || !mounted) return;
    setState(() => _registering = true);
    String? message;
    try {
      final result = await registerSecurityKey(name);
      message = switch (result.error) {
        KeyError.none => '已添加安全密钥：点它查看公钥，加到服务器的 authorized_keys',
        KeyError.securityKeyCancelled => null,
        final error => keyErrorText(error),
      };
    } on TimeoutException {
      message = '操作超时';
    }
    if (!mounted) return;
    setState(() => _registering = false);
    if (message != null) _snack(message);
  }

  /// 开关同步前讲清楚后果，用户同意才动。
  Future<void> _toggleSync(bool enabled) async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text(enabled ? '通过 iCloud 同步私钥？' : '停止同步私钥？'),
        content: Text(
          enabled
              ? '私钥与存下的口令将存入 iCloud 钥匙串，同步到登录同一 Apple 账户、开启了 iCloud 钥匙串的其他设备。'
                    'iCloud 钥匙串是端到端加密的，但私钥将不再只留在这台设备上。'
              : '私钥与存下的口令会从 iCloud 钥匙串移回这台设备，其他设备上的这些私钥会随之删除。',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('取消'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, true),
            child: Text(enabled ? '上传并同步' : '停止同步'),
          ),
        ],
      ),
    );
    if (confirmed != true || !mounted) return;
    await _applySync(enabled);
  }

  Future<void> _applySync(bool target) async {
    setState(() => _syncBusy = true);
    try {
      final result = await (widget.setSync ?? setKeySync)(target);
      if (!mounted) return;
      if (result.error == KeyError.none) {
        // 成功回包确认迁移与来源清理均完成，不再沿用操作前的同步说明。
        setState(() {
          _unconfirmedSyncTarget = null;
          _state = _state?.copyWith(
            syncEnabled: target,
            syncPending: false,
            syncTargetEnabled: target,
          );
        });
      } else {
        // 失败回包结束未知状态；随后的列表快照会确认是否仍需清理来源。
        setState(() {
          _unconfirmedSyncTarget = null;
          _state = _state?.copyWith(
            syncPending: true,
            syncTargetEnabled: target,
          );
        });
        _snack(keyErrorText(result.error));
      }
    } on TimeoutException {
      if (!mounted) return;
      // 前端超时不会取消后端迁移；未确认完成前不能宣称私钥仅在本机。
      setState(() => _unconfirmedSyncTarget = target);
      _snack('同步请求尚未确认，可等待结果或重试');
    }
    if (mounted) setState(() => _syncBusy = false);
  }

  Future<void> _rename(KeySummary key) async {
    final name = await showDialog<String>(
      context: context,
      builder: (_) => NameDialog(title: '重命名私钥', initial: key.name),
    );
    if (name != null) await _run(() => renameKey(key.id, name));
  }

  Future<void> _delete(KeySummary key) async {
    if (key.usedBy > 0) {
      await showDialog<void>(
        context: context,
        builder: (context) => AlertDialog(
          title: Text('私钥「${key.name}」还在用'),
          content: Text('${key.usedBy} 个连接用它登录。先把这些连接换成别的私钥或认证方式，再删除它。'),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(context),
              child: const Text('知道了'),
            ),
          ],
        ),
      );
      return;
    }
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text('删除私钥「${key.name}」？'),
        content: Text(switch ((
          key.cardIdent.isNotEmpty,
          key.securityKey,
          key.synchronized,
        )) {
          (true, _, _) => '只删除这张卡的登记，卡上的密钥不受影响，以后可以再添加。',
          (false, true, _) => '删除后不能再用这个凭据登录。以后要用这把安全密钥，需要重新添加，并把新的公钥加到服务器。',
          (false, false, true) => '私钥会从 iCloud 钥匙串删除，其他设备上也将不再有它。',
          (false, false, false) => '私钥与存下的口令会从钥匙串删除。',
        }),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('取消'),
          ),
          TextButton(
            onPressed: () => Navigator.pop(context, true),
            style: TextButton.styleFrom(
              foregroundColor: Theme.of(context).colorScheme.error,
            ),
            child: const Text('删除'),
          ),
        ],
      ),
    );
    if (confirmed == true) await _run(() => deleteKey(key.id));
  }

  void _show(KeySummary key) {
    showModalBottomSheet<void>(
      context: context,
      showDragHandle: true,
      builder: (sheetContext) => SafeArea(
        child: Padding(
          padding: const EdgeInsets.fromLTRB(20, 0, 20, 16),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(key.name, style: Theme.of(context).textTheme.titleLarge),
              const SizedBox(height: 8),
              Text(switch ((
                key.cardIdent.isNotEmpty,
                key.securityKey,
                key.encrypted,
                key.passphraseSaved,
              )) {
                (true, _, _, _) =>
                  '${key.algorithm} · 私钥在 OpenPGP 卡（卡号 ${key.cardIdent}）上，'
                      '登录时要插着卡并输入卡的 PIN',
                (false, true, _, _) =>
                  '${key.algorithm} · 私钥在安全密钥上，'
                      '登录时按系统提示插上（或靠近）安全密钥并触摸它',
                (false, false, false, _) => key.algorithm,
                (false, false, true, false) => '${key.algorithm} · 有口令保护',
                (false, false, true, true) =>
                  '${key.algorithm} · 有口令保护，口令已存入钥匙串',
              }),
              if (key.usedBy > 0) Text('${key.usedBy} 个连接在用'),
              const SizedBox(height: 12),
              const Text('公钥（加到服务器的 ~/.ssh/authorized_keys）'),
              const SizedBox(height: 4),
              SelectableText(
                key.publicKey,
                maxLines: 4,
                style: const TextStyle(fontFamily: 'Menlo', fontSize: 11),
              ),
              const SizedBox(height: 4),
              SelectableText(
                key.fingerprint,
                style: const TextStyle(fontFamily: 'Menlo', fontSize: 11),
              ),
              const SizedBox(height: 12),
              Wrap(
                spacing: 8,
                children: [
                  FilledButton.icon(
                    onPressed: () {
                      Clipboard.setData(ClipboardData(text: key.publicKey));
                      Navigator.pop(sheetContext);
                      _snack('公钥已复制');
                    },
                    icon: const Icon(Icons.copy),
                    label: const Text('复制公钥'),
                  ),
                  OutlinedButton(
                    onPressed: () {
                      Navigator.pop(sheetContext);
                      _rename(key);
                    },
                    child: const Text('重命名'),
                  ),
                  if (key.passphraseSaved)
                    OutlinedButton(
                      onPressed: () {
                        Navigator.pop(sheetContext);
                        _run(() => forgetPassphrase(key.id));
                      },
                      child: const Text('忘记口令'),
                    ),
                  OutlinedButton(
                    onPressed: () {
                      Navigator.pop(sheetContext);
                      _delete(key);
                    },
                    style: OutlinedButton.styleFrom(
                      foregroundColor: Theme.of(context).colorScheme.error,
                    ),
                    child: const Text('删除'),
                  ),
                ],
              ),
            ],
          ),
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final state = _state;
    final syncPending =
        (state?.syncPending ?? false) || _unconfirmedSyncTarget != null;
    final syncTarget =
        _unconfirmedSyncTarget ?? state?.syncTargetEnabled ?? false;
    return Scaffold(
      appBar: AppBar(
        title: const Text('私钥'),
        actions: [
          PopupMenuButton<VoidCallback>(
            tooltip: '添加',
            icon: const Icon(Icons.add),
            onSelected: (action) => action(),
            enabled: !_registering,
            itemBuilder: (_) => [
              PopupMenuItem(value: _import, child: const Text('导入私钥')),
              PopupMenuItem(value: _addCard, child: const Text('添加 OpenPGP 卡')),
              if (state?.securityKeysAvailable ?? false)
                PopupMenuItem(
                  value: _addSecurityKey,
                  child: const Text('添加安全密钥（FIDO2）'),
                ),
            ],
          ),
        ],
      ),
      body: state == null
          ? const Center(child: CircularProgressIndicator())
          : SafeArea(
              child: ListView(
                children: [
                  SwitchListTile(
                    title: const Text('通过 iCloud 钥匙串同步私钥'),
                    subtitle: Text(
                      _syncBusy
                          ? '正在迁移私钥与口令，两个存储可能暂时都保留副本'
                          : syncPending
                          ? (syncTarget
                                ? '同步迁移尚未完成，本机与 iCloud 钥匙串可能都保留副本'
                                : '停止同步尚未完成，iCloud 钥匙串仍可能保留私钥与口令')
                          : switch ((state.syncEnabled, state.syncAvailable)) {
                              (true, _) => '私钥保存在 iCloud 钥匙串，随 Apple 账户同步到其他设备',
                              (false, true) => '私钥只保存在这台设备上',
                              (false, false) =>
                                '私钥只保存在这台设备上（这个版本的 App 不能使用 iCloud 钥匙串）',
                            },
                    ),
                    value: state.syncEnabled,
                    // 已开着的总能关掉；不能用 iCloud 钥匙串时不能打开。
                    onChanged:
                        _syncBusy ||
                            syncPending ||
                            (!state.syncEnabled && !state.syncAvailable)
                        ? null
                        : _toggleSync,
                  ),
                  if (syncPending)
                    ListTile(
                      title: Text(
                        _unconfirmedSyncTarget == null
                            ? '私钥同步迁移待完成'
                            : '私钥同步请求尚未确认',
                      ),
                      subtitle: const Text('原存储清理完成前，不会将此次切换视为完成'),
                      trailing: TextButton(
                        onPressed: _syncBusy
                            ? null
                            : () => _applySync(syncTarget),
                        child: const Text('重试'),
                      ),
                    ),
                  const Divider(),
                  if (state.listError != KeyError.none)
                    ListTile(
                      title: const Text('无法读取私钥列表'),
                      subtitle: Text(keyErrorText(state.listError)),
                      trailing: TextButton(
                        onPressed: _queryKeys,
                        child: const Text('重试'),
                      ),
                    ),
                  if (_registering)
                    const ListTile(
                      leading: SizedBox.square(
                        dimension: 24,
                        child: CircularProgressIndicator(strokeWidth: 2),
                      ),
                      title: Text('正在添加安全密钥'),
                      subtitle: Text('按系统提示插上（或靠近）安全密钥，并触摸它'),
                    ),
                  if (state.keys.isEmpty &&
                      !_registering &&
                      state.listError == KeyError.none)
                    Padding(
                      padding: const EdgeInsets.all(24),
                      child: Column(
                        children: [
                          const Text('还没有私钥'),
                          const SizedBox(height: 12),
                          Wrap(
                            spacing: 12,
                            runSpacing: 8,
                            alignment: WrapAlignment.center,
                            children: [
                              FilledButton.icon(
                                onPressed: _import,
                                icon: const Icon(Icons.add),
                                label: const Text('导入私钥'),
                              ),
                              OutlinedButton.icon(
                                onPressed: _addCard,
                                icon: const Icon(Icons.credit_card),
                                label: const Text('添加 OpenPGP 卡'),
                              ),
                              if (state.securityKeysAvailable)
                                OutlinedButton.icon(
                                  onPressed: _addSecurityKey,
                                  icon: const Icon(Icons.usb),
                                  label: const Text('添加安全密钥'),
                                ),
                            ],
                          ),
                        ],
                      ),
                    ),
                  for (final key in state.keys)
                    ListTile(
                      leading: Icon(switch ((
                        key.cardIdent.isNotEmpty,
                        key.securityKey,
                      )) {
                        (true, _) => Icons.credit_card,
                        (false, true) => Icons.usb,
                        (false, false) => Icons.key,
                      }),
                      title: Text(key.name),
                      subtitle: Text(
                        switch ((key.cardIdent.isNotEmpty, key.securityKey)) {
                          (true, _) =>
                            'OpenPGP 卡 ${key.cardIdent} · ${key.fingerprint}',
                          (false, true) => '安全密钥 · ${key.fingerprint}',
                          (false, false) =>
                            '${key.algorithm} · ${key.fingerprint}',
                        },
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                      ),
                      trailing: key.synchronized
                          ? const Icon(Icons.cloud_done_outlined)
                          : null,
                      onTap: () => _show(key),
                    ),
                ],
              ),
            ),
    );
  }
}
