// flutter drive 的驱动端（M6）：截图落盘到 build/m6/screenshots/<设备>/，集成测试写的
// reportData 落盘到 build/m6/reports/<M6_REPORT>.json（scripts/m6.sh 设 M6_REPORT）。
import 'dart:convert';
import 'dart:io';

import 'package:integration_test/integration_test_driver_extended.dart';

Future<void> main() async {
  final report = Platform.environment['M6_REPORT'] ?? 'report';
  await integrationDriver(
    writeResponseOnFailure: true,
    onScreenshot: (name, bytes, [args]) async {
      final file = File('build/m6/screenshots/$name.png');
      await file.parent.create(recursive: true);
      await file.writeAsBytes(bytes);
      return true;
    },
    responseDataCallback: (data) async {
      // 截图已经经 onScreenshot 落盘，报告里只留名字（字节以 JSON 数组存，动辄上百 MB）。
      final screenshots = data?['screenshots'];
      if (screenshots is List) {
        data!['screenshots'] = [
          for (final shot in screenshots)
            if (shot is Map) {'screenshotName': shot['screenshotName']},
        ];
      }
      final file = File('build/m6/reports/$report.json');
      await file.parent.create(recursive: true);
      await file.writeAsString(const JsonEncoder.withIndent('  ').convert(data));
    },
  );
}
