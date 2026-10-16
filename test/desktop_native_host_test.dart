import 'dart:async';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/desktop/native_input/desktop_native_host.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  const channel = MethodChannel('test/normal-native-input');
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
  final calls = <MethodCall>[];
  late Map<String, Object?> Function(MethodCall) response;
  setUp(() {
    calls.clear();
    messenger.setMockMethodCallHandler(channel, (call) async {
      calls.add(call);
      return response(call);
    });
  });
  tearDown(() => messenger.setMockMethodCallHandler(channel, null));

  Future<DesktopPaneToken> pane(DesktopNativeHost host) => host.attachPane(
    origin: DesktopSource(
      sessionId: 31,
      generation: Uint64.parse('9007199254740993'),
    ),
    identity: 'prepared',
    attempt: Uint64.parse('8'),
    focusEpoch: Uint64.parse('19'),
    lifecycleRevision: Uint64.parse('22'),
    geometry: {'width': 640.0},
  );

  test('模块关联不开放生产输入，attachRuntime 不传入路径或可信身份', () async {
    response = (_) => {
      'version': 1,
      'moduleAttached': true,
      'inputSourceAvailable': false,
      'productionEnabled': false,
    };
    final host = DesktopNativeHost(channel: channel);
    final capability = await host.attachRuntime();
    expect(capability.moduleAttached, isTrue);
    expect(capability.available, isFalse);
    expect(calls.single.arguments, isNull);
    await host.attachRuntime();
    expect(calls.length, 1);
  });

  test('正常窗格与焦点只消费不透明 token，64 位关联身份不截断', () async {
    response = (call) => call.method == 'attachPane'
        ? {'version': 1, 'state': 'attached', 'paneToken': 'opaque-pane'}
        : {'version': 1, 'state': 'active', 'attempt': '8', 'focusEpoch': '20'};
    final host = DesktopNativeHost(channel: channel);
    final token = await pane(host);
    final focus = await host.requestFocus(token);
    expect(focus.active, isTrue);
    expect(focus.focusEpoch, Uint64.parse('20'));
    final attach = calls.first.arguments as Map;
    expect((attach['source'] as Map)['generation'], '9007199254740993');
    expect((calls.last.arguments as Map)['paneToken'], 'opaque-pane');
    expect(attach.containsKey('hwnd'), isFalse);
    expect(attach.containsKey('inputObject'), isFalse);
  });

  test('pending 不能冒充 nativequiet，quiet 与 released 是两个完成点', () async {
    String state = 'pending';
    response = (call) => call.method == 'attachPane'
        ? {'version': 1, 'state': 'attached', 'paneToken': 'original'}
        : {'version': 1, 'state': state};
    final host = DesktopNativeHost(
      channel: channel,
      timeout: const Duration(milliseconds: 5),
    );
    final token = await pane(host);
    await expectLater(
      host.retirePane(token, release: false),
      throwsA(isA<TimeoutException>()),
    );
    state = 'nativequiet';
    expect(await host.retirePane(token, release: false), isTrue);
    expect(await host.retirePane(token, release: true), isFalse);
    state = 'released';
    expect(await host.retirePane(token, release: true), isTrue);
  });

  test('GUI claim 保留完整 payload 与原 token，Paste 回传完整原文', () async {
    const payload = '多行🙂\n第二行\r\n';
    response = (call) => switch (call.method) {
      'attachPane' => {
        'version': 1,
        'state': 'attached',
        'paneToken': 'original',
      },
      'claimAction' => {
        'version': 1,
        'state': 'claimed',
        'claimToken': 'opaque-claim',
        'action': 'paste',
        'payload': payload,
      },
      _ => {'version': 1, 'state': 'completed'},
    };
    final host = DesktopNativeHost(channel: channel);
    final token = await pane(host);
    final claim = (await host.claimAction(token, Uint64.parse('99')))!;
    expect(claim.payload, payload);
    expect(claim.action, DesktopAction.paste);
    expect(
      await host.completeAction(claim, kind: 'paste', text: payload),
      isTrue,
    );
    final ack = calls.last.arguments as Map;
    expect(ack['paneToken'], 'original');
    expect(ack['claimToken'], 'opaque-claim');
    expect(ack['text'], payload);
  });

  test('pending 仅由原 pane 与 request 的实际 quiet 通知完成，迟到其他请求不解锁', () async {
    response = (call) => call.method == 'attachPane'
        ? {'version': 1, 'state': 'attached', 'paneToken': 'original'}
        : {'version': 1, 'state': 'pending'};
    final host = DesktopNativeHost(channel: channel);
    final token = await pane(host);
    bool completed = false;
    final quiet = host.retirePane(token, release: false).then((value) {
      completed = true;
      return value;
    });
    await Future<void>.delayed(Duration.zero);
    final request = (calls.last.arguments as Map)['requestId'];
    void notify(Object? requestId) {
      TestWidgetsFlutterBinding.instance.channelBuffers.push(
        channel.name,
        const StandardMethodCodec().encodeMethodCall(
          MethodCall('paneState', {
            'version': 1,
            'paneToken': 'original',
            'requestId': requestId,
            'state': 'nativequiet',
          }),
        ),
        (_) {},
      );
    }

    notify('other-request');
    await Future<void>.delayed(Duration.zero);
    expect(completed, isFalse);
    notify(request);
    expect(await quiet, isTrue);
  });

  test('未知版本与缺少实际领取材料明确失败，不发成功 ACK', () async {
    response = (_) => {'version': 2, 'moduleAttached': true};
    expect(
      DesktopNativeHost(channel: channel).attachRuntime(),
      throwsFormatException,
    );
    response = (call) => call.method == 'attachPane'
        ? {'version': 1, 'state': 'attached', 'paneToken': 'original'}
        : {'version': 1, 'state': 'claimed', 'claimToken': 'opaque-claim'};
    final host = DesktopNativeHost(channel: channel);
    final token = await pane(host);
    await expectLater(
      host.claimAction(token, Uint64.parse('1')),
      throwsFormatException,
    );
    expect(calls.any((call) => call.method == 'completeAction'), isFalse);
  });
}
