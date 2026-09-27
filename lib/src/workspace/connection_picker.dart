import 'dart:async';

import 'package:flutter/material.dart';

import '../bindings/bindings.dart';
import '../catalog/catalog_requests.dart';
import '../catalog/connection_editor_page.dart';
import '../terminal/session_target.dart';

/// 选一个连接开新标签或分屏：[current] 是当前窗格的连接（排在最前，便于再开一个），
/// 其下是目录，最后是快速连接。取消返回 null。
Future<SessionTarget?> pickConnection(BuildContext context, {SessionTarget? current}) {
  return showModalBottomSheet<SessionTarget>(
    context: context,
    isScrollControlled: true,
    showDragHandle: true,
    builder: (context) => _ConnectionPicker(current: current),
  );
}

class _ConnectionPicker extends StatefulWidget {
  final SessionTarget? current;

  const _ConnectionPicker({this.current});

  @override
  State<_ConnectionPicker> createState() => _ConnectionPickerState();
}

class _ConnectionPickerState extends State<_ConnectionPicker> {
  StreamSubscription? _catalogSub;
  List<ConnectionSummary>? _connections = switch (CatalogState.latestRustSignal?.message) {
    final state? when state.query.isEmpty => state.connections,
    _ => null,
  };

  @override
  void initState() {
    super.initState();
    _catalogSub = CatalogState.rustSignalStream.listen((pack) {
      if (mounted && pack.message.query.isEmpty) {
        setState(() => _connections = pack.message.connections);
      }
    });
    CatalogQuery(query: '').sendSignalToRust();
  }

  @override
  void dispose() {
    _catalogSub?.cancel();
    super.dispose();
  }

  Future<void> _quickConnect() async {
    final navigator = Navigator.of(context);
    final target = await navigator.push<SessionTarget>(MaterialPageRoute(
      builder: (_) => const ConnectionEditorPage(quick: true, returnTarget: true),
    ));
    if (target != null && mounted) navigator.pop(target);
  }

  @override
  Widget build(BuildContext context) {
    final current = widget.current;
    final connections = _connections;
    return SafeArea(
      child: ConstrainedBox(
        constraints: BoxConstraints(maxHeight: MediaQuery.sizeOf(context).height * 0.7),
        child: ListView(
          shrinkWrap: true,
          children: [
            if (current != null)
              ListTile(
                leading: const Icon(Icons.copy_all_outlined),
                title: Text('再开一个「${current.title}」'),
                onTap: () => Navigator.pop(context, current),
              ),
            if (current != null) const Divider(),
            if (connections == null)
              const Padding(
                padding: EdgeInsets.all(24),
                child: Center(child: CircularProgressIndicator()),
              ),
            for (final connection in connections ?? const <ConnectionSummary>[])
              ListTile(
                leading: const Icon(Icons.dns_outlined),
                title: Text(connectionTitle(connection)),
                subtitle: Text(
                  '${connection.username}@${connection.host}:${connection.port}',
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                ),
                onTap: () => Navigator.pop(
                  context,
                  SessionTarget.saved(
                    connectionId: connection.id,
                    title: connectionTitle(connection),
                  ),
                ),
              ),
            ListTile(
              leading: const Icon(Icons.bolt),
              title: const Text('快速连接…'),
              onTap: _quickConnect,
            ),
          ],
        ),
      ),
    );
  }
}
