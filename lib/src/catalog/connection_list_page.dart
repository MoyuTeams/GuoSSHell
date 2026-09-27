import 'dart:async';

import 'package:flutter/material.dart';

import '../bindings/bindings.dart';
import '../settings/settings_page.dart';
import '../terminal/session_target.dart';
import '../workspace/workspace_page.dart';
import 'catalog_requests.dart';
import 'connection_editor_page.dart';

enum _RowAction { edit, duplicate, delete }

/// 首页：连接目录（搜索、增删改、点按连接）。
class ConnectionListPage extends StatefulWidget {
  final ValueChanged<String>? queryCatalog;

  const ConnectionListPage({super.key, this.queryCatalog});

  @override
  State<ConnectionListPage> createState() => _ConnectionListPageState();
}

class _ConnectionListPageState extends State<ConnectionListPage> {
  final _search = TextEditingController();
  StreamSubscription? _catalogSub;

  /// null = 还没收到目录。
  List<ConnectionSummary>? _connections;
  bool _wasCurrent = true;

  @override
  void initState() {
    super.initState();
    _catalogSub = CatalogState.rustSignalStream.listen((pack) {
      // 输入过程中迟到的旧查询结果不覆盖新的。
      if (!mounted || pack.message.query != _search.text) return;
      setState(() => _connections = pack.message.connections);
    });
    _search.addListener(_query);
    _query();
  }

  void _query() {
    if (widget.queryCatalog case final query?) {
      query(_search.text);
    } else {
      CatalogQuery(query: _search.text).sendSignalToRust();
    }
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    // 选择器可在其他路由查询整个目录；返回首页时重新订阅搜索框的查询。
    final current = ModalRoute.isCurrentOf(context) ?? true;
    if (current && !_wasCurrent) _query();
    _wasCurrent = current;
  }

  @override
  void dispose() {
    _catalogSub?.cancel();
    _search.dispose();
    super.dispose();
  }

  void _open(ConnectionSummary connection) {
    Navigator.of(context).push(
      MaterialPageRoute<void>(
        builder: (_) => WorkspacePage(
          initial: SessionTarget.saved(
            connectionId: connection.id,
            title: connectionTitle(connection),
          ),
        ),
      ),
    );
  }

  void _edit(ConnectionSummary? connection) {
    Navigator.of(context).push(
      MaterialPageRoute<void>(
        builder: (_) => ConnectionEditorPage(existing: connection),
      ),
    );
  }

  void _quickConnect() {
    Navigator.of(context).push(
      MaterialPageRoute<void>(
        builder: (_) => const ConnectionEditorPage(quick: true),
      ),
    );
  }

  void _settings() {
    Navigator.of(context)
        .push(MaterialPageRoute<void>(builder: (_) => const SettingsPage()));
  }

  Future<void> _onAction(
    ConnectionSummary connection,
    _RowAction action,
  ) async {
    switch (action) {
      case _RowAction.edit:
        _edit(connection);
      case _RowAction.duplicate:
        await _run(() => duplicateConnection(connection.id));
      case _RowAction.delete:
        final confirmed = await showDialog<bool>(
          context: context,
          builder: (context) => AlertDialog(
            title: Text('删除「${connectionTitle(connection)}」？'),
            content: connection.passwordSaved
                ? const Text('保存在钥匙串里的密码也会一起删除。')
                : null,
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
        if (confirmed == true) {
          await _run(() => deleteConnection(connection.id));
        }
    }
  }

  Future<void> _run(Future<CatalogResult> Function() request) async {
    String? message;
    try {
      final result = await request();
      if (result.error != CatalogError.none) {
        message = catalogErrorText(result.error);
      }
    } on TimeoutException {
      message = '操作超时';
    }
    if (message != null && mounted) {
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text(message)));
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        title: const Text('GuoSSHell'),
        actions: [
          IconButton(
            tooltip: '快速连接',
            icon: const Icon(Icons.bolt),
            onPressed: _quickConnect,
          ),
          IconButton(
            tooltip: '设置',
            icon: const Icon(Icons.settings_outlined),
            onPressed: _settings,
          ),
        ],
      ),
      floatingActionButton: FloatingActionButton(
        tooltip: '添加连接',
        onPressed: () => _edit(null),
        child: const Icon(Icons.add),
      ),
      body: SafeArea(
        child: Column(
          children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(16, 8, 16, 8),
              child: SearchBar(
                controller: _search,
                hintText: '搜索名称、主机或用户名',
                leading: const Icon(Icons.search),
                elevation: const WidgetStatePropertyAll(0),
                trailing: [
                  if (_search.text.isNotEmpty)
                    IconButton(
                      icon: const Icon(Icons.clear),
                      onPressed: _search.clear,
                    ),
                ],
              ),
            ),
            Expanded(child: _buildList(context)),
          ],
        ),
      ),
    );
  }

  Widget _buildList(BuildContext context) {
    final connections = _connections;
    if (connections == null) {
      return const Center(child: CircularProgressIndicator());
    }
    if (connections.isEmpty) {
      final searching = _search.text.isNotEmpty;
      return Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(searching ? '没有匹配的连接' : '还没有连接'),
            if (!searching) ...[
              const SizedBox(height: 12),
              FilledButton.icon(
                onPressed: () => _edit(null),
                icon: const Icon(Icons.add),
                label: const Text('添加连接'),
              ),
            ],
          ],
        ),
      );
    }
    return ListView.builder(
      padding: const EdgeInsets.only(bottom: 88),
      itemCount: connections.length,
      itemBuilder: (context, index) {
        final connection = connections[index];
        final target =
            '${connection.username}@${connection.host}:${connection.port}';
        return ListTile(
          leading: const Icon(Icons.dns_outlined),
          title: Text(connectionTitle(connection)),
          subtitle: Text(
            connection.command.isEmpty
                ? target
                : '$target · ${connection.command}',
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
          ),
          onTap: () => _open(connection),
          trailing: PopupMenuButton<_RowAction>(
            onSelected: (action) => _onAction(connection, action),
            itemBuilder: (context) => const [
              PopupMenuItem(value: _RowAction.edit, child: Text('编辑')),
              PopupMenuItem(value: _RowAction.duplicate, child: Text('复制')),
              PopupMenuItem(value: _RowAction.delete, child: Text('删除')),
            ],
          ),
        );
      },
    );
  }
}
