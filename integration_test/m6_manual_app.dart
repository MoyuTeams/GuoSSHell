// 实体键盘、触控板与中文输入法验收入口；不向输入链路合成按键。
import 'package:flutter/material.dart';
import 'package:guosh_shell/src/terminal/session_target.dart';
import 'package:guosh_shell/src/catalog/connection_list_page.dart';
import 'package:guosh_shell/src/workspace/workspace_page.dart';

import 'm6_harness.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  final ready = startRust();
  var opened = false;
  runApp(
    MaterialApp(
      theme: ThemeData(brightness: Brightness.dark),
      home: FutureBuilder<void>(
        future: ready,
        builder: (context, snapshot) {
          if (snapshot.hasError) {
            return Scaffold(
              body: Center(child: Text('验收环境启动失败：${snapshot.error}')),
            );
          }
          if (snapshot.connectionState != ConnectionState.done) {
            return const Scaffold(
              body: Center(child: CircularProgressIndicator()),
            );
          }
          if (!opened) {
            opened = true;
            WidgetsBinding.instance.addPostFrameCallback((_) {
              if (!context.mounted) return;
              Navigator.of(context).push(
                MaterialPageRoute<void>(
                  builder: (_) => const WorkspacePage(
                    initial: SessionTarget.quick(
                      host: m6Host,
                      port: m6Port,
                      username: 'probe',
                      password: 'probe',
                      command: 'm6-keyecho motion --seconds 1800',
                    ),
                  ),
                ),
              );
            });
          }
          return const ConnectionListPage();
        },
      ),
    ),
  );
}
