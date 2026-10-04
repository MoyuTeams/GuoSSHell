import 'dart:async';

import 'package:fluent_ui/fluent_ui.dart';
import 'package:flutter/services.dart';

import '../../bindings/bindings.dart';
import '../../keys/key_requests.dart' show keyErrorText;
import '../desktop_status.dart';
import '../desktop_theme.dart';
import 'desktop_key_actions.dart';
import 'desktop_key_dialogs.dart';
import 'desktop_key_sync_state.dart';

/// 桌面私钥工作区；由桌面导航的 content 直接引用，无需 Material 路由。
class DesktopKeysPage extends StatefulWidget {
  const DesktopKeysPage({
    super.key,
    DesktopKeySyncState? syncState,
    this.actions,
  }) : _providedSyncState = syncState;

  /// 父级应跨导航持有并在壳销毁时 dispose；旧壳未接入前共用应用级保底对象。
  final DesktopKeySyncState? _providedSyncState;
  static final DesktopKeySyncState _appLifetimeSyncState =
      DesktopKeySyncState();
  DesktopKeySyncState get syncState =>
      _providedSyncState ?? _appLifetimeSyncState;
  final DesktopKeyActions? actions;

  @override
  State<DesktopKeysPage> createState() => _DesktopKeysPageState();
}

class _DesktopKeysPageState extends State<DesktopKeysPage> {
  late final DesktopKeyActions _actions = widget.actions ?? DesktopKeyActions();
  String? _selectedId;
  String? _notice;
  bool _busy = false;

  bool get _mutationsBlocked => _busy || !widget.syncState.canMutate;

  @override
  void initState() {
    super.initState();
    widget.syncState.addListener(_syncChanged);
    _actions.query();
  }

  void _syncChanged() {
    if (mounted) setState(() {});
  }

  @override
  void didUpdateWidget(DesktopKeysPage oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.syncState != widget.syncState) {
      oldWidget.syncState.removeListener(_syncChanged);
      widget.syncState.addListener(_syncChanged);
      _actions.query();
    }
  }

  @override
  void dispose() {
    widget.syncState.removeListener(_syncChanged);
    super.dispose();
  }

  void _query() {
    setState(() => _notice = null);
    _actions.query();
  }

  Future<void> _run(
    Future<KeyResult> Function() request,
    String success,
  ) async {
    if (_mutationsBlocked) return;
    setState(() {
      _busy = true;
      _notice = null;
    });
    String message;
    try {
      final result = await request();
      message = result.error == KeyError.none
          ? success
          : keyErrorText(result.error);
    } on TimeoutException {
      message = '操作超时，请刷新列表确认结果';
    } catch (_) {
      message = '操作失败，请重试';
    }
    if (!mounted) return;
    setState(() {
      _busy = false;
      _notice = message;
    });
    _actions.query();
  }

  Future<void> _import() async {
    if (_mutationsBlocked) return;
    final id = await showDialog<String>(
      context: context,
      barrierDismissible: false,
      builder: (_) => DesktopKeyImportDialog(
        actions: _actions,
        syncState: widget.syncState,
      ),
    );
    if (!mounted || id == null) return;
    setState(() {
      _selectedId = id;
      _notice = '私钥已导入';
    });
    _actions.query();
  }

  Future<void> _card() async {
    if (_mutationsBlocked) return;
    final id = await showDialog<String>(
      context: context,
      barrierDismissible: false,
      builder: (_) =>
          DesktopCardScanDialog(actions: _actions, syncState: widget.syncState),
    );
    if (!mounted || id == null) return;
    setState(() {
      _selectedId = id;
      _notice = '卡片公钥已登记';
    });
    _actions.query();
  }

  Future<void> _securityKey() async {
    if (_mutationsBlocked ||
        !(widget.syncState.snapshot?.securityKeysAvailable ?? false)) {
      return;
    }
    final name = await desktopKeyNameDialog(
      context,
      title: '添加安全密钥（FIDO2）',
      initial: '',
      confirm: '继续',
    );
    if (!mounted || name == null || _mutationsBlocked) return;
    setState(() {
      _busy = true;
      _notice = null;
    });
    String? message;
    String? id;
    try {
      final result = await _actions.registerSecurityKey(name);
      if (result.error == KeyError.none) {
        message = '安全密钥已登记；请将公钥添加到服务器';
        id = result.keyId;
      } else if (result.error != KeyError.securityKeyCancelled) {
        message = keyErrorText(result.error);
      }
    } on TimeoutException {
      message = '登记超时，请刷新列表确认结果';
    } catch (_) {
      message = '登记失败，请重试';
    }
    if (!mounted) return;
    setState(() {
      _busy = false;
      _notice = message;
      if (id != null) _selectedId = id;
    });
    if (message != null) _actions.query();
  }

  Future<void> _rename(KeySummary key) async {
    if (_mutationsBlocked) return;
    final name = await desktopKeyNameDialog(
      context,
      title: '重命名私钥',
      initial: key.name,
    );
    if (!mounted || name == null) return;
    await _run(() => _actions.rename(key.id, name), '名称已更新');
  }

  Future<void> _forget(KeySummary key) async {
    if (_mutationsBlocked || !key.passphraseSaved) return;
    final confirmed = await desktopKeyConfirm(
      context,
      title: '忘记保存的口令？',
      detail: '下次使用这把私钥时需要重新输入口令。私钥本身不会删除。',
      action: '忘记口令',
    );
    if (!mounted || !confirmed || _mutationsBlocked) return;
    await _run(() => _actions.forgetPassphrase(key.id), '已忘记保存的口令');
  }

  Future<void> _delete(KeySummary key) async {
    if (_mutationsBlocked) return;
    // 对话框打开前及确认后均重验引用；后端仍是删除保护的最终裁决者。
    final current = _currentKey(key.id);
    if (current == null) return;
    if (current.usedBy > 0) {
      await desktopKeyConfirm(
        context,
        title: '私钥「${current.name}」还在使用',
        detail: '${current.usedBy} 个连接引用它。先更换这些连接的认证方式，再删除。',
        action: '知道了',
      );
      return;
    }
    final detail = switch ((
      current.cardIdent.isNotEmpty,
      current.securityKey,
      current.synchronized,
    )) {
      (true, _, _) => '仅删除登记；卡上的私钥不会受影响。',
      (false, true, _) => '此凭据将不能再登录；重新添加会产生新公钥，需要更新服务器。',
      (false, false, true) => '私钥将从 iCloud 钥匙串删除，其他设备也将不再有它。',
      _ => '私钥和存下的口令将从钥匙串删除。',
    };
    final confirmed = await desktopKeyConfirm(
      context,
      title: '删除私钥「${current.name}」？',
      detail: detail,
      action: '删除',
    );
    if (!mounted || !confirmed) return;
    final latest = _currentKey(key.id);
    if (_mutationsBlocked ||
        latest == null ||
        latest.usedBy > 0 ||
        latest.synchronized != current.synchronized ||
        latest.cardIdent != current.cardIdent ||
        latest.securityKey != current.securityKey) {
      setState(() => _notice = '私钥或存储位置已变化，请刷新列表后重新确认删除');
      _actions.query();
      return;
    }
    await _run(() => _actions.delete(key.id), '已删除私钥登记');
  }

  KeySummary? _currentKey(String id) {
    for (final key in widget.syncState.snapshot?.keys ?? const <KeySummary>[]) {
      if (key.id == id) return key;
    }
    return null;
  }

  Future<void> _toggleSync(bool target) async {
    if (_mutationsBlocked) return;
    final confirmed = await desktopKeyConfirm(
      context,
      title: target ? '通过 iCloud 同步私钥？' : '停止同步私钥？',
      detail: target
          ? '私钥与保存的口令将进入 iCloud 钥匙串，并同步至使用同一 Apple 账户的设备。它们不再只留在本机。'
          : '私钥与保存的口令将迁回本机钥匙串，其他设备上的副本会随迁移完成而删除。',
      action: target ? '上传并同步' : '停止同步',
    );
    if (mounted && confirmed && !_mutationsBlocked) await _applySync(target);
  }

  Future<void> _applySync(bool target) async {
    if (_busy || widget.syncState.busy) return;
    setState(() => _notice = null);
    await widget.syncState.migrate(
      target,
      setSync: _actions.setSync,
      query: _actions.query,
    );
  }

  @override
  Widget build(BuildContext context) {
    final state = widget.syncState.snapshot;
    final selected = _selectedId == null ? null : _currentKey(_selectedId!);
    final pending = widget.syncState.pendingTarget != null;
    final target = widget.syncState.pendingTarget ?? false;
    final hasCloudKey = state?.keys.any((key) => key.synchronized) ?? false;
    final hasLocalKey = state?.keys.any((key) => !key.synchronized) ?? false;
    return DesktopPage(
      title: '私钥',
      description: '管理登录凭据；只展示公钥与指纹，不显示私钥。',
      children: [
        Wrap(
          spacing: DesktopSpacing.sm,
          runSpacing: DesktopSpacing.sm,
          children: [
            FilledButton(
              onPressed: _mutationsBlocked ? null : _import,
              child: const Text('导入私钥'),
            ),
            Button(
              onPressed: _mutationsBlocked ? null : _card,
              child: const Text('添加 OpenPGP 卡'),
            ),
            if (state?.securityKeysAvailable ?? false)
              Button(
                onPressed: _mutationsBlocked ? null : _securityKey,
                child: const Text('添加安全密钥（FIDO2）'),
              ),
            Button(onPressed: _query, child: const Text('刷新列表')),
          ],
        ),
        if (_busy) ...[
          const SizedBox(height: DesktopSpacing.lg),
          const DesktopStatus(
            kind: DesktopStatusKind.loading,
            title: '正在处理',
            detail: '请按系统提示完成操作。',
          ),
        ],
        if (_notice != null) ...[
          const SizedBox(height: DesktopSpacing.lg),
          Semantics(liveRegion: true, child: Text(_notice!)),
        ],
        if (widget.syncState.statusMessage != null) ...[
          const SizedBox(height: DesktopSpacing.lg),
          Semantics(
            liveRegion: true,
            child: Text(widget.syncState.statusMessage!),
          ),
        ],
        const SizedBox(height: DesktopSpacing.xxl),
        Text(
          'iCloud 钥匙串',
          style: FluentTheme.of(context).typography.titleLarge,
        ),
        const SizedBox(height: DesktopSpacing.sm),
        if (state == null)
          const Text('等待钥匙串状态…')
        else ...[
          ToggleSwitch(
            checked: state.syncEnabled,
            onChanged:
                _busy ||
                    widget.syncState.locationUnknown ||
                    (!state.syncEnabled && !state.syncAvailable)
                ? null
                : _toggleSync,
            content: const Text('同步私钥与保存的口令'),
          ),
          const SizedBox(height: DesktopSpacing.sm),
          Text(
            widget.syncState.busy
                ? '正在迁移私钥与口令，两个存储可能暂时都有副本'
                : widget.syncState.locationUnknown
                ? target
                      ? '存储位置尚未确认，本机和 iCloud 钥匙串可能都有副本'
                      : '存储位置尚未确认，iCloud 钥匙串仍可能保留私钥与口令'
                : state.syncEnabled && hasLocalKey
                ? '列表中有私钥位于本机钥匙串；请逐把查看存储位置'
                : !state.syncEnabled && hasCloudKey
                ? state.syncAvailable
                      ? '列表中有私钥位于 iCloud 钥匙串；请逐把查看存储位置'
                      : '列表中有私钥位于 iCloud 钥匙串（此构建不能开启同步）；请逐把查看存储位置'
                : state.syncEnabled
                ? '私钥保存在 iCloud 钥匙串，可同步到其他设备'
                : state.syncAvailable
                ? '私钥只保存在这台设备上'
                : '私钥只保存在这台设备上（此构建不能使用 iCloud 钥匙串）',
            style: FluentTheme.of(context).typography.body,
          ),
          if (pending) ...[
            const SizedBox(height: DesktopSpacing.sm),
            const Text('私钥同步位置待确认'),
            Button(
              onPressed: _busy || widget.syncState.busy
                  ? null
                  : () => _applySync(target),
              child: const Text('按原目标重试'),
            ),
          ],
        ],
        const SizedBox(height: DesktopSpacing.xxl),
        Text('已登记的凭据', style: FluentTheme.of(context).typography.titleLarge),
        const SizedBox(height: DesktopSpacing.lg),
        if (state == null)
          const DesktopStatus(
            kind: DesktopStatusKind.loading,
            title: '正在读取私钥',
            detail: '等待钥匙串返回列表。',
          )
        else if (state.listError != KeyError.none) ...[
          DesktopStatus(
            kind: DesktopStatusKind.error,
            title: '无法读取私钥列表',
            detail: keyErrorText(state.listError),
          ),
          Button(onPressed: _query, child: const Text('重试读取')),
        ] else if (state.keys.isEmpty)
          const DesktopStatus(
            kind: DesktopStatusKind.empty,
            title: '还没有私钥',
            detail: '导入私钥或登记 OpenPGP 卡以开始使用。',
          )
        else ...[
          for (final key in state.keys) ...[
            Button(
              onPressed: () => setState(() => _selectedId = key.id),
              child: SizedBox(
                width: double.infinity,
                child: Row(
                  children: [
                    Icon(
                      key.cardIdent.isNotEmpty
                          ? FluentIcons.contact_card
                          : FluentIcons.permissions,
                      size: DesktopMetrics.icon,
                    ),
                    const SizedBox(width: DesktopSpacing.md),
                    Expanded(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Text(
                            key.name,
                            style: FluentTheme.of(context).typography.body,
                          ),
                          Text(
                            '${key.algorithm} · ${key.fingerprint}',
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: FluentTheme.of(context).typography.caption,
                          ),
                        ],
                      ),
                    ),
                    if (!widget.syncState.locationUnknown && key.synchronized)
                      const Text('iCloud'),
                  ],
                ),
              ),
            ),
            const SizedBox(height: DesktopSpacing.sm),
          ],
          if (selected != null) _detail(context, selected),
        ],
      ],
    );
  }

  Widget _detail(BuildContext context, KeySummary key) => Padding(
    padding: const EdgeInsets.only(top: DesktopSpacing.xl),
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(key.name, style: FluentTheme.of(context).typography.titleLarge),
        const SizedBox(height: DesktopSpacing.sm),
        Text(
          key.cardIdent.isNotEmpty
              ? '${key.algorithm} · 私钥在 OpenPGP 卡（卡号 ${key.cardIdent}）上，登录需要卡和 PIN'
              : key.securityKey
              ? '${key.algorithm} · 登录时需连接安全密钥并按系统提示触摸'
              : key.encrypted
              ? '${key.algorithm} · 口令保护${key.passphraseSaved ? '，口令已存入钥匙串' : ''}'
              : key.algorithm,
        ),
        Text(
          '${key.usedBy} 个连接引用 · ${widget.syncState.locationUnknown
              ? '存储位置待确认'
              : key.synchronized
              ? 'iCloud 钥匙串'
              : '本机凭据'}',
        ),
        const SizedBox(height: DesktopSpacing.lg),
        const Text('公钥（添加到服务器的 authorized_keys）'),
        const SizedBox(height: DesktopSpacing.sm),
        SelectableText(key.publicKey),
        const SizedBox(height: DesktopSpacing.md),
        const Text('指纹'),
        SelectableText(key.fingerprint),
        const SizedBox(height: DesktopSpacing.lg),
        Wrap(
          spacing: DesktopSpacing.sm,
          runSpacing: DesktopSpacing.sm,
          children: [
            Button(
              onPressed: () async {
                await Clipboard.setData(ClipboardData(text: key.publicKey));
                if (mounted) setState(() => _notice = '公钥已复制');
              },
              child: const Text('复制公钥'),
            ),
            Button(
              onPressed: _mutationsBlocked ? null : () => _rename(key),
              child: const Text('重命名'),
            ),
            if (key.passphraseSaved)
              Button(
                onPressed: _mutationsBlocked ? null : () => _forget(key),
                child: const Text('忘记口令'),
              ),
            Button(
              onPressed: _mutationsBlocked ? null : () => _delete(key),
              child: const Text('删除'),
            ),
          ],
        ),
      ],
    ),
  );
}
