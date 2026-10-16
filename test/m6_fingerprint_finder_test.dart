import 'package:fluent_ui/fluent_ui.dart' as fluent;
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/desktop/desktop_theme.dart';
import 'package:guosh_shell/src/desktop/interaction/desktop_prompt_dialogs.dart';
import 'package:guosh_shell/src/terminal/prompt_dialogs.dart';

import '../integration_test/m6_harness.dart';

// 用真实的主机密钥对话框验证 M6 harness 的指纹 finder：移动端 Material 与桌面 Fluent
// 都以 SelectableText 显示「算法\n指纹」，finder 必须恰好命中其中一个。
const _fingerprint = 'SHA256:abc';

const _prompt = InteractionPrompt(
  sessionId: 1,
  promptId: 1,
  kind: PromptKind.hostKey,
  username: 'probe',
  host: 'nas.lan',
  port: 2222,
  address: '10.0.0.8',
  algorithm: 'ssh-ed25519',
  fingerprint: _fingerprint,
  changed: false,
  name: '',
  instruction: '',
  fields: [],
  canRemember: false,
  retry: false,
  triesLeft: -1,
);

void main() {
  testWidgets('移动端 Material 主机密钥对话框：指纹匹配命中 1 个，不匹配命中 0', (tester) async {
    final dismissed = ValueNotifier(false);
    addTearDown(dismissed.dispose);
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () => showPromptDialog(context, _prompt, dismissed),
            child: const Text('open'),
          ),
        ),
      ),
    );
    await tester.tap(find.text('open'));
    await tester.pumpAndSettle();

    expect(find.text('信任并连接'), findsOneWidget);
    expect(find.byType(SelectableText), findsOneWidget);
    expect(m6FingerprintFinder(_fingerprint), findsOneWidget);
    expect(m6FingerprintFinder('SHA256:abd'), findsNothing);
    // 只比最后一行，前缀或子串都不算匹配。
    expect(m6FingerprintFinder('SHA256:ab'), findsNothing);
    expect(m6FingerprintFinder('ssh-ed25519'), findsNothing);
  });

  testWidgets('桌面 Fluent 主机密钥对话框：指纹匹配命中 1 个，不匹配命中 0', (tester) async {
    final dismissed = ValueNotifier(false);
    addTearDown(dismissed.dispose);
    await tester.pumpWidget(
      fluent.FluentApp(
        theme: desktopTheme(highContrast: false, reduceMotion: true),
        home: fluent.NavigationView(
          content: fluent.Builder(
            builder: (context) => fluent.Button(
              onPressed: () =>
                  showDesktopPromptDialog(context, _prompt, dismissed),
              child: const fluent.Text('打开'),
            ),
          ),
        ),
      ),
    );
    await tester.tap(find.text('打开'));
    await tester.pumpAndSettle();

    expect(find.text('信任并连接'), findsOneWidget);
    expect(find.byType(SelectableText), findsOneWidget);
    expect(m6FingerprintFinder(_fingerprint), findsOneWidget);
    expect(m6FingerprintFinder('SHA256:abd'), findsNothing);
    expect(m6FingerprintFinder('SHA256:ab'), findsNothing);
    expect(m6FingerprintFinder('ssh-ed25519'), findsNothing);
  });

  test('纯 Text 同样按最后一行精确匹配', () {
    expect(m6FingerprintMatches(const Text('a\nSHA256:x'), 'SHA256:x'), isTrue);
    expect(
      m6FingerprintMatches(const Text('a\nSHA256:x'), 'SHA256:y'),
      isFalse,
    );
    expect(m6FingerprintMatches(const SizedBox(), 'SHA256:x'), isFalse);
  });
}
