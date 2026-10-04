import 'dart:async';

import 'package:fluent_ui/fluent_ui.dart';

import '../../bindings/bindings.dart';
import '../../catalog/catalog_requests.dart';
import '../../terminal/session_target.dart';
import '../desktop_status.dart';
import '../desktop_theme.dart';
import '../keys/desktop_key_sync_state.dart';
import 'desktop_catalog_gateway.dart';
import 'desktop_connection_editor.dart';

/// Fluent 目录：只接受当前查询的整份快照，路由返回后主动重查。
class DesktopCatalogPage extends StatefulWidget {
  final DesktopCatalogGateway gateway;
  final ValueChanged<SessionTarget> openTarget;
  final DesktopKeySyncState? keySyncState;

  const DesktopCatalogPage({
    super.key,
    this.gateway = const RustDesktopCatalogGateway(),
    required this.openTarget,
    this.keySyncState,
  });

  @override
  State<DesktopCatalogPage> createState() => _DesktopCatalogPageState();
}

class _DesktopCatalogPageState extends State<DesktopCatalogPage> {
  final _search = TextEditingController();
  StreamSubscription<CatalogState>? _subscription;
  List<ConnectionSummary>? _connections;
  String? _error;
  bool _busy = false;
  bool _wasCurrent = true;

  @override
  void initState() {
    super.initState();
    _subscription = widget.gateway.catalogStates.listen((state) {
      if (!mounted || state.query != _search.text) return;
      setState(() => _connections = state.connections);
    });
    _search.addListener(_onSearch);
    widget.gateway.query(_search.text);
  }

  void _onSearch() {
    setState(() {
      _connections = null;
      _error = null;
    });
    widget.gateway.query(_search.text);
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final current = ModalRoute.isCurrentOf(context) ?? true;
    if (current && !_wasCurrent) widget.gateway.query(_search.text);
    _wasCurrent = current;
  }

  @override
  void dispose() {
    _subscription?.cancel();
    _search.dispose();
    super.dispose();
  }

  Future<void> _edit([ConnectionSummary? existing, bool quick = false]) async {
    final result = await Navigator.of(context).push<SessionTarget>(
      FluentPageRoute(
        builder: (_) => DesktopConnectionEditor(
          gateway: widget.gateway,
          keySyncState: widget.keySyncState,
          existing: existing,
          quick: quick,
        ),
      ),
    );
    if (!mounted) return;
    widget.gateway.query(_search.text);
    if (result != null) widget.openTarget(result);
  }

  Future<void> _run(Future<CatalogResult> Function() action) async {
    if (_busy) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    String? error;
    try {
      final result = await action();
      if (result.error != CatalogError.none) {
        error = catalogErrorText(result.error);
      }
    } on TimeoutException {
      error = '操作超时';
    } catch (_) {
      error = '操作失败，请重试';
    }
    if (!mounted) return;
    setState(() {
      _busy = false;
      _error = error;
    });
    // 结果只确认请求是否成功，列表仍以 Rust 发来的整份目录为准。
    if (error == null) widget.gateway.query(_search.text);
  }

  Future<void> _delete(ConnectionSummary connection) async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => ContentDialog(
        title: Text('删除「${connectionTitle(connection)}」？'),
        content: Text(
          connection.passwordSaved ? '保存在钥匙串里的密码也会一起删除。此操作不可撤销。' : '此操作不可撤销。',
        ),
        actions: [
          Button(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('取消'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, true),
            child: const Text('删除'),
          ),
        ],
      ),
    );
    if (confirmed == true && mounted) {
      await _run(() => widget.gateway.delete(connection.id));
    }
  }

  Widget _row(ConnectionSummary connection) {
    final target =
        '${connection.username}@${connection.host}:${connection.port}';
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: DesktopSpacing.md),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            connectionTitle(connection),
            style: FluentTheme.of(context).typography.titleLarge,
          ),
          const SizedBox(height: DesktopSpacing.xs),
          Text(
            connection.command.isEmpty
                ? target
                : '$target · ${connection.command}',
            overflow: TextOverflow.ellipsis,
            style: FluentTheme.of(context).typography.caption,
          ),
          const SizedBox(height: DesktopSpacing.md),
          Wrap(
            spacing: DesktopSpacing.sm,
            runSpacing: DesktopSpacing.sm,
            children: [
              FilledButton(
                onPressed: () => widget.openTarget(
                  SessionTarget.saved(
                    connectionId: connection.id,
                    title: connectionTitle(connection),
                  ),
                ),
                child: const Text('连接'),
              ),
              Button(
                onPressed: () => _edit(connection),
                child: const Text('编辑'),
              ),
              Button(
                onPressed: _busy
                    ? null
                    : () => _run(() => widget.gateway.duplicate(connection.id)),
                child: const Text('复制'),
              ),
              Button(
                onPressed: _busy ? null : () => _delete(connection),
                child: const Text('删除'),
              ),
            ],
          ),
          const SizedBox(height: DesktopSpacing.md),
          Divider(style: DividerThemeData(horizontalMargin: EdgeInsets.zero)),
        ],
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final connections = _connections;
    return DesktopPage(
      title: '连接目录',
      description: '查找已保存的 SSH 连接，或发起不入库的快速连接。',
      children: [
        Wrap(
          spacing: DesktopSpacing.md,
          runSpacing: DesktopSpacing.md,
          children: [
            FilledButton(onPressed: () => _edit(), child: const Text('添加连接')),
            Button(
              onPressed: () => _edit(null, true),
              child: const Text('快速连接'),
            ),
          ],
        ),
        const SizedBox(height: DesktopSpacing.xl),
        Text('搜索连接', style: FluentTheme.of(context).typography.body),
        const SizedBox(height: DesktopSpacing.sm),
        TextBox(
          key: const Key('catalog-search'),
          controller: _search,
          placeholder: '搜索名称、主机或用户名',
          suffix: _search.text.isEmpty
              ? null
              : IconButton(
                  icon: const Icon(FluentIcons.clear),
                  onPressed: _search.clear,
                ),
        ),
        if (_error != null) ...[
          const SizedBox(height: DesktopSpacing.lg),
          Semantics(
            liveRegion: true,
            child: Text(
              _error!,
              style: FluentTheme.of(context).typography.body,
            ),
          ),
        ],
        const SizedBox(height: DesktopSpacing.xl),
        if (connections == null)
          const DesktopStatus(
            kind: DesktopStatusKind.loading,
            title: '正在读取目录',
            detail: '等待当前搜索的目录结果。',
          )
        else if (connections.isEmpty)
          DesktopStatus(
            kind: DesktopStatusKind.empty,
            title: _search.text.isEmpty ? '还没有连接' : '没有匹配的连接',
            detail: _search.text.isEmpty ? '添加连接后即可从这里连接。' : '试试其他名称、主机或用户名。',
          )
        else
          for (final connection in connections) _row(connection),
      ],
    );
  }
}
