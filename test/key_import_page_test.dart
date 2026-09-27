import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';

import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/keys/key_import_page.dart';

class _UnreadableFile extends XFile {
  _UnreadableFile() : super('unreadable.pem');

  @override
  Future<int> length() async => 10;

  @override
  Future<String> readAsString({Encoding encoding = utf8}) async {
    throw StateError('文件暂不可用');
  }
}

void main() {
  testWidgets('非 UTF-8 文件显示错误，后续仍可选择有效文件', (tester) async {
    var pick = 0;
    await tester.pumpWidget(
      MaterialApp(
        home: KeyImportPage(
          pickFile: () async {
            if (pick++ == 0) {
              return XFile.fromData(Uint8List.fromList([0xff, 0xfe]));
            }
            return XFile.fromData(
              Uint8List.fromList(utf8.encode('fixture-key')),
              name: 'fixture.pem',
            );
          },
        ),
      ),
    );
    await tester.tap(find.text('从文件选择'));
    await tester.pumpAndSettle();
    expect(find.textContaining('UTF-8'), findsOneWidget);
    expect(tester.takeException(), isNull);
    await tester.tap(find.text('从文件选择'));
    await tester.pumpAndSettle();
    expect(find.textContaining('UTF-8'), findsNothing);
    expect(find.text('fixture-key'), findsOneWidget);
  });

  testWidgets('云端文件读取失败显示提示，并保留原输入', (tester) async {
    await tester.pumpWidget(
      MaterialApp(home: KeyImportPage(pickFile: () async => _UnreadableFile())),
    );
    await tester.enterText(find.byType(TextField).at(1), '原有文本');
    await tester.tap(find.text('从文件选择'));
    await tester.pumpAndSettle();
    expect(find.textContaining('无法读取文件'), findsOneWidget);
    expect(find.text('原有文本'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('等待文件期间退出页面不会在销毁后更新状态', (tester) async {
    final selection = Completer<XFile?>();
    await tester.pumpWidget(
      MaterialApp(home: KeyImportPage(pickFile: () => selection.future)),
    );
    await tester.tap(find.text('从文件选择'));
    await tester.pumpWidget(const SizedBox());
    selection.complete(XFile.fromData(Uint8List(65537)));
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
  });
}
