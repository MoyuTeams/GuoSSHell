import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/catalog/connection_list_page.dart';

void main() {
  testWidgets('选择器改变查询后返回首页，会恢复搜索并接收后续删除结果', (tester) async {
    final navigation = GlobalKey<NavigatorState>();
    final queries = <String>[];
    var currentQuery = '';
    var exists = true;
    void publish() {
      final state = CatalogState(
        query: currentQuery,
        connections: [
          if (exists)
            const ConnectionSummary(
              id: 'fixture',
              name: '目标连接',
              host: 'fixture.invalid',
              port: 22,
              username: 'fixture',
              auth: AuthMethod.password,
              passwordSaved: false,
              keyId: '',
              command: '',
            ),
        ],
      );
      assignRustSignal['CatalogState']!(state.bincodeSerialize(), Uint8List(0));
    }

    void query(String value) {
      queries.add(value);
      currentQuery = value;
      publish();
    }

    await tester.pumpWidget(
      MaterialApp(
        navigatorKey: navigation,
        home: ConnectionListPage(queryCatalog: query),
      ),
    );
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(SearchBar), '目标');
    await tester.pumpAndSettle();
    navigation.currentState!.push(
      MaterialPageRoute<void>(
        builder: (_) => const Scaffold(body: Text('工作区')),
      ),
    );
    await tester.pumpAndSettle();
    query('');
    await tester.pumpAndSettle();
    navigation.currentState!.pop();
    await tester.pumpAndSettle();
    expect(queries.last, '目标');
    expect(
      tester.widget<SearchBar>(find.byType(SearchBar)).controller!.text,
      '目标',
    );
    exists = false;
    publish();
    await tester.pumpAndSettle();
    expect(find.text('目标连接'), findsNothing);
    expect(find.text('没有匹配的连接'), findsOneWidget);
    await tester.pumpWidget(const SizedBox());
  });
}
