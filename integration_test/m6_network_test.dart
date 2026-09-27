// 真机测试台连通性诊断。设备地址只写入本机报告，不进入入库文档。
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

import 'm6_harness.dart';

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.fullyLive;
  final report = <String, Object>{'device': m6Device};
  tearDownAll(
    () => binding.reportData = {...?binding.reportData, 'network': report},
  );

  testWidgets('真机到验收服务器的 SSH 与 HTTP 连通性', (tester) async {
    report['interfaces'] = [
      for (final interface in await NetworkInterface.list())
        {
          'name': interface.name,
          'addresses': interface.addresses.map((a) => a.address).toList(),
        },
    ];
    Socket? socket;
    try {
      socket = await Socket.connect(
        m6Host,
        m6Port,
        timeout: const Duration(seconds: 5),
      );
      report['ssh_peer_address'] = socket.remoteAddress.address;
      report['ssh_banner'] = String.fromCharCodes(
        await socket.first.timeout(const Duration(seconds: 5)),
      ).trim();
    } on Object catch (error) {
      report['ssh_error'] = error.toString();
    } finally {
      socket?.destroy();
    }
    final client = HttpClient()..connectionTimeout = const Duration(seconds: 5);
    try {
      final request = await client.getUrl(
        m6Api('/m6/plans', query: {'limit': '1'}),
      );
      final response = await request.close().timeout(
        const Duration(seconds: 5),
      );
      report['http_status'] = response.statusCode;
      await response.drain<void>();
    } on Object catch (error) {
      report['http_error'] = error.toString();
    } finally {
      client.close(force: true);
    }
    // 即使连接失败，也先交出诊断；报告由 Mac 端驱动保存。
    binding.reportData = {...?binding.reportData, 'network': report};
    expect(report['ssh_error'], isNull);
    expect(report['http_status'], 200);
  });
}
