import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/lifecycle/app_visibility.dart';
import 'package:guosh_shell/src/lifecycle/session_keeper.dart';

void main() {
  group('后台断开后的自动重连', () {
    late DateTime now;
    late AppVisibility visibility;
    late ResumeReconnect policy;

    setUp(() {
      now = DateTime(2026, 9, 28, 12);
      visibility = AppVisibility(clock: () => now);
      policy = ResumeReconnect(visibility);
    });

    test('一直在前台时断开不自动重连', () {
      expect(policy.onLost(), isFalse);
      visibility.update(foreground: false);
      visibility.update(foreground: true);
      expect(policy.onShown(), isFalse);
    });

    test('在后台断开，回到前台重连一次', () {
      visibility.update(foreground: false);
      expect(policy.onLost(), isFalse);
      visibility.update(foreground: true);
      expect(policy.onShown(), isTrue);
      expect(policy.onShown(), isFalse);
    });

    test('刚回前台时报出的断开立刻重连，过了窗口期不再算后台造成的', () {
      visibility.update(foreground: false);
      visibility.update(foreground: true);
      now = now.add(const Duration(seconds: 5));
      expect(policy.onLost(), isTrue);

      final later = ResumeReconnect(visibility);
      now = now.add(AppVisibility.resumeWindow);
      expect(later.onLost(), isFalse);
    });

    test('同一次回前台重连后又断开，不再自动重连', () {
      visibility.update(foreground: false);
      visibility.update(foreground: true);
      expect(policy.onLost(), isTrue);
      expect(policy.onLost(), isFalse);

      visibility.update(foreground: false);
      expect(policy.onLost(), isFalse);
      visibility.update(foreground: true);
      expect(policy.onShown(), isTrue);
    });

    test('用户在回前台前已处理（手动重连、关闭）就不再重连', () {
      visibility.update(foreground: false);
      policy.onLost();
      policy.cancel();
      visibility.update(foreground: true);
      expect(policy.onShown(), isFalse);
    });

    test('重复的前后台通知不计为新的一次回前台', () {
      visibility.update(foreground: true);
      expect(visibility.resumes, 0);
      visibility.update(foreground: false);
      visibility.update(foreground: false);
      visibility.update(foreground: true);
      visibility.update(foreground: true);
      expect(visibility.resumes, 1);
    });
  });

  group('Android 会话保活', () {
    TestWidgetsFlutterBinding.ensureInitialized();
    const channel = MethodChannel('guosshell/session_keeper.test');
    final calls = <Object?>[];

    setUp(() {
      calls.clear();
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(channel, (call) async {
            calls.add(call.arguments);
            return null;
          });
    });

    test('只在保活会话数变化时通知宿主', () {
      final keeper = SessionKeeper(enabled: true, channel: channel);
      final a = Object();
      final b = Object();
      keeper
        ..report(a, live: true)
        ..report(a, live: true)
        ..report(b, live: true)
        ..report(a, live: false)
        ..report(b, live: false)
        ..report(b, live: false);
      expect(calls, [1, 2, 1, 0]);
    });

    test('非 Android 平台不通知宿主', () {
      final keeper = SessionKeeper(enabled: false, channel: channel);
      keeper.report(Object(), live: true);
      expect(keeper.count, 1);
      expect(calls, isEmpty);
    });
  });
}
