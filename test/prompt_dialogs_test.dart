import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/terminal/prompt_dialogs.dart';

InteractionPrompt _prompt(
  PromptKind kind, {
  bool changed = false,
  bool canRemember = false,
  bool retry = false,
  String name = '',
  List<PromptField> fields = const [],
  int triesLeft = -1,
}) =>
    InteractionPrompt(
      sessionId: 1,
      promptId: 1,
      kind: kind,
      username: 'probe',
      host: 'nas.lan',
      port: 22,
      address: '10.0.0.8',
      algorithm: 'ssh-ed25519',
      fingerprint: 'SHA256:abc',
      changed: changed,
      name: name,
      instruction: '',
      fields: fields,
      canRemember: canRemember,
      retry: retry,
      triesLeft: triesLeft,
    );

void main() {
  late ValueNotifier<bool> dismissed;
  PromptAnswer? answer;

  setUp(() {
    dismissed = ValueNotifier(false);
    answer = null;
  });

  Future<void> open(WidgetTester tester, InteractionPrompt prompt) async {
    await tester.pumpWidget(MaterialApp(
      home: Builder(
        builder: (context) => TextButton(
          onPressed: () async => answer = await showPromptDialog(context, prompt, dismissed),
          child: const Text('open'),
        ),
      ),
    ));
    await tester.tap(find.text('open'));
    await tester.pumpAndSettle();
  }

  testWidgets('新主机：显示主机名与实际地址，信任即接受', (tester) async {
    await open(tester, _prompt(PromptKind.hostKey));
    expect(find.textContaining('nas.lan（10.0.0.8）'), findsOneWidget);

    await tester.tap(find.text('信任并连接'));
    await tester.pumpAndSettle();
    expect(answer?.accept, isTrue);
  });

  testWidgets('密钥变更：勾选核对之前不能替换', (tester) async {
    await open(tester, _prompt(PromptKind.hostKey, changed: true));
    expect(find.text('主机密钥已变更'), findsOneWidget);

    final replace = find.widgetWithText(TextButton, '替换旧密钥并连接');
    expect(tester.widget<TextButton>(replace).onPressed, isNull);

    await tester.tap(find.text('我已通过其他途径核对新指纹'));
    await tester.pump();
    await tester.tap(replace);
    await tester.pumpAndSettle();
    expect(answer?.accept, isTrue);
  });

  testWidgets('密钥变更：默认按钮是断开', (tester) async {
    await open(tester, _prompt(PromptKind.hostKey, changed: true));
    await tester.tap(find.widgetWithText(FilledButton, '断开'));
    await tester.pumpAndSettle();
    expect(answer?.accept, isFalse);
  });

  testWidgets('密码：只有目录里的连接可以勾选保存', (tester) async {
    await open(tester, _prompt(PromptKind.password));
    expect(find.text('连接成功后保存到钥匙串'), findsNothing);
    await tester.tap(find.text('取消'));
    await tester.pumpAndSettle();
    expect(answer?.accept, isFalse);

    await open(tester, _prompt(PromptKind.password, canRemember: true));
    await tester.enterText(find.byType(TextField), 'hunter2');
    await tester.tap(find.text('连接成功后保存到钥匙串'));
    await tester.pump();
    await tester.tap(find.widgetWithText(FilledButton, '连接'));
    await tester.pumpAndSettle();
    expect(answer?.answers, ['hunter2']);
    expect(answer?.remember, isTrue);
  });

  testWidgets('私钥口令：显示私钥名称，口令错了提示重输', (tester) async {
    await open(
      tester,
      _prompt(PromptKind.passphrase, name: 'laptop', canRemember: true, retry: true),
    );
    expect(find.text('输入私钥口令'), findsOneWidget);
    expect(find.textContaining('私钥「laptop」'), findsOneWidget);
    expect(find.text('口令不正确，请重新输入'), findsOneWidget);
    await tester.enterText(find.byType(TextField), 'secret');
    await tester.tap(find.text('保存口令到钥匙串'));
    await tester.pump();
    await tester.tap(find.widgetWithText(FilledButton, '连接'));
    await tester.pumpAndSettle();
    expect(answer?.answers, ['secret']);
    expect(answer?.remember, isTrue);
  });

  testWidgets('卡 PIN：错了提示剩余次数重输，可以记住到退出 App', (tester) async {
    await open(
      tester,
      _prompt(PromptKind.cardPin, name: 'CanoKey', triesLeft: 2, retry: true),
    );
    expect(find.text('输入卡的 PIN'), findsOneWidget);
    expect(find.textContaining('OpenPGP 卡「CanoKey」'), findsOneWidget);
    expect(find.text('PIN 不正确。还可以试 2 次，用完卡会锁定'), findsOneWidget);
    await tester.enterText(find.byType(TextField), '123456');
    await tester.tap(find.text('记住 PIN，直到退出 App'));
    await tester.pump();
    await tester.tap(find.widgetWithText(FilledButton, '连接'));
    await tester.pumpAndSettle();
    expect(answer?.answers, ['123456']);
    expect(answer?.remember, isTrue);
  });

  testWidgets('卡 PIN：只剩一次时明确警告；次数未知时不显示', (tester) async {
    await open(tester, _prompt(PromptKind.cardPin, name: 'CanoKey', triesLeft: 1));
    expect(find.text('只剩最后 1 次，再错卡会锁定'), findsOneWidget);
    await tester.tap(find.text('取消'));
    await tester.pumpAndSettle();
    expect(answer?.accept, isFalse);

    await open(tester, _prompt(PromptKind.cardPin, name: 'CanoKey'));
    expect(find.textContaining('还可以试'), findsNothing);
  });

  testWidgets('keyboard-interactive：按输入项逐个回答', (tester) async {
    await open(
      tester,
      _prompt(PromptKind.keyboardInteractive, fields: [
        PromptField(label: 'Password: ', echo: false),
        PromptField(label: 'Code: ', echo: true),
      ]),
    );
    await tester.enterText(find.byType(TextField).at(0), 'secret');
    await tester.enterText(find.byType(TextField).at(1), '123456');
    await tester.tap(find.text('确定'));
    await tester.pumpAndSettle();
    expect(answer?.answers, ['secret', '123456']);
  });

  testWidgets('会话结束时对话框自行关闭，视为取消', (tester) async {
    await open(tester, _prompt(PromptKind.password, canRemember: true));
    dismissed.value = true;
    await tester.pumpAndSettle();
    expect(find.byType(AlertDialog), findsNothing);
    expect(answer?.accept, isFalse);
  });
}
