import 'dart:async';

import 'package:fluent_ui/fluent_ui.dart' as fluent;
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart' as material;
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/desktop/desktop_theme.dart';
import 'package:guosh_shell/src/desktop/interaction/desktop_prompt_dialogs.dart';
import 'package:guosh_shell/src/terminal/prompt_dialogs.dart' show PromptAnswer;
import 'package:guosh_shell/src/terminal/session_target.dart';
import 'package:guosh_shell/src/terminal/terminal_pane.dart';
import 'package:rinf/rinf.dart';

import 'desktop_session_controller_test.dart'
    show DesktopSessionTestGateway, prepared;

InteractionPrompt _prompt(
  PromptKind kind, {
  int sessionId = 1,
  int promptId = 1,
  bool changed = false,
  bool retry = false,
  bool canRemember = false,
  String name = '',
  String instruction = '',
  int triesLeft = -1,
  List<PromptField> fields = const [],
}) => InteractionPrompt(
  sessionId: sessionId,
  promptId: promptId,
  kind: kind,
  username: 'probe',
  host: 'nas.lan',
  port: 2222,
  address: '10.0.0.8',
  algorithm: 'ssh-ed25519',
  fingerprint: 'SHA256:abc',
  changed: changed,
  name: name,
  instruction: instruction,
  fields: fields,
  canRemember: canRemember,
  retry: retry,
  triesLeft: triesLeft,
);

Future<void> _open(
  WidgetTester tester,
  InteractionPrompt prompt,
  ValueNotifier<bool> dismissed,
  void Function(PromptAnswer) onAnswer, {
  bool legacy = false,
  double width = 800,
  bool highContrast = false,
}) async {
  tester.view.physicalSize = fluent.Size(width, 900);
  tester.view.devicePixelRatio = 1;
  addTearDown(() {
    tester.view.resetPhysicalSize();
    tester.view.resetDevicePixelRatio();
  });
  addTearDown(dismissed.dispose);
  final open = fluent.Builder(
    builder: (context) => fluent.Button(
      onPressed: () async =>
          onAnswer(await showDesktopPromptDialog(context, prompt, dismissed)),
      child: const fluent.Text('打开'),
    ),
  );
  await tester.pumpWidget(
    fluent.FluentApp(
      theme: desktopTheme(highContrast: highContrast, reduceMotion: true),
      home: legacy
          ? material.MaterialApp(home: material.Scaffold(body: open))
          : fluent.NavigationView(content: open),
    ),
  );
  await tester.tap(find.text('打开'));
  await tester.pumpAndSettle();
}

Future<void> _removePaneWithActivePrompt(
  WidgetTester tester, {
  required bool changedHostKey,
}) async {
  debugDefaultTargetPlatformOverride = TargetPlatform.windows;
  addTearDown(() => debugDefaultTargetPlatformOverride = null);
  assignRustSignal['SettingsState']!(
    const SettingsState(
      fontSize: 14,
      minFontSize: 8,
      maxFontSize: 32,
      scrollbackLines: 5000,
      maxScrollbackLines: 20000,
      showKeyBar: false,
      keyBarRows: [[], []],
    ).bincodeSerialize(),
    Uint8List(0),
  );
  addTearDown(() => SettingsState.latestRustSignal = null);
  final showing = ValueNotifier(true);
  final gateway = DesktopSessionTestGateway();
  addTearDown(gateway.dispose);
  void ended(DesktopSource source) => gateway.lifecycleBus.add(
    RustSignalPack(
      DesktopNativeLifecycle(
        version: 1,
        source: source,
        identity: 'frozen',
        attempt: Uint64(BigInt.one),
        focusEpoch: Uint64(BigInt.one),
        lifecycleRevision: Uint64(BigInt.one),
        state: NativeLifecycleState.ended,
      ),
      Uint8List(0),
    ),
  );
  gateway.onOrdered = (request) {
    if (request.event is DesktopEventDisconnect) {
      scheduleMicrotask(() => ended(request.source));
    }
    return DesktopSessionTestGateway.accept(request);
  };
  final controller = TerminalPaneController(
    const SessionTarget.quick(host: 'nas.lan', port: 22, username: 'probe'),
  );
  addTearDown(showing.dispose);
  addTearDown(controller.dispose);
  await tester.pumpWidget(
    fluent.FluentApp(
      theme: desktopTheme(highContrast: false, reduceMotion: true),
      home: material.MaterialApp(
        home: material.Scaffold(
          body: material.ValueListenableBuilder<bool>(
            valueListenable: showing,
            builder: (_, visible, _) => visible
                ? material.SizedBox(
                    height: 1,
                    width: 1,
                    child: TerminalPane(
                      controller: controller,
                      desktopGateway: gateway,
                      onActivate: () {},
                      onClose: () {},
                    ),
                  )
                : const material.SizedBox.shrink(),
          ),
        ),
      ),
    ),
  );
  await tester.pump();
  expect(controller.sessionId, greaterThan(0));
  expect(controller.ownsNativeInput, isFalse);
  // 桌面窗格走 V2：Prepared 给出 identity 后，认证提示经 DesktopPrompt 到达。
  final source = gateway.prepares.single.source;
  final snapshot = prepared(gateway.prepares.single.requestId);
  gateway.emit(snapshot.copyWith(source: source));
  await tester.pump();
  gateway.promptBus.add(
    RustSignalPack(
      DesktopPrompt(
        source: source,
        identity: 'frozen',
        prompt: _prompt(
          changedHostKey ? PromptKind.hostKey : PromptKind.password,
          changed: changedHostKey,
          sessionId: controller.sessionId,
        ),
      ),
      Uint8List(0),
    ),
  );
  await tester.pumpAndSettle();
  expect(find.byType(fluent.ContentDialog), findsOneWidget);
  if (changedHostKey) {
    await tester.tap(find.text('我已通过其他途径核对新指纹'));
    await tester.pump();
    await tester.tap(find.text('替换旧密钥并连接'));
    await tester.pumpAndSettle();
    expect(find.byType(fluent.ContentDialog), findsNWidgets(2));
  }
  // 引擎先报告取消但没有 closed/failed；窗格随即被工作区直接卸载。
  gateway.statusBus.add(
    RustSignalPack(
      DesktopSessionStatus(
        source: source,
        identity: 'frozen',
        status: SessionStatus(
          sessionId: controller.sessionId,
          state: SessionState.cancelled,
          failure: FailureKind.none,
          detail: '',
          localNetworkSettingsUrl: '',
          hint: ConnectHint.none,
        ),
      ),
      Uint8List(0),
    ),
  );
  await tester.pump();
  expect(
    find.byType(fluent.ContentDialog),
    findsNWidgets(changedHostKey ? 2 : 1),
  );
  showing.value = false;
  await tester.pumpAndSettle();
  expect(find.byType(fluent.ContentDialog), findsNothing);
  // 卸载即关闭：未 Start 时取消准备，已 Start 时等 Accepted 后有序断开。
  if (gateway.starts.isEmpty) {
    gateway.emit(
      snapshot.copyWith(
        source: source,
        requestId: gateway.cancels.single.requestId,
        state: DesktopPreparationState.cancelled,
        snapshot: () => null,
      ),
    );
  } else {
    gateway.emit(
      snapshot.copyWith(
        source: source,
        requestId: gateway.starts.single.requestId,
        state: DesktopPreparationState.accepted,
        snapshot: () => null,
      ),
    );
  }
  await tester.pumpAndSettle();
  // 测试无 Native 动态库；若旧异步回调仍尝试送出接受回包，这里会留下 FFI 异常。
  expect(tester.takeException(), isNull);
  debugDefaultTargetPlatformOverride = null;
}

void main() {
  testWidgets('终端窗格直接销毁时仍打开的密码提示必须立即移除', (tester) async {
    await _removePaneWithActivePrompt(tester, changedHostKey: false);
  });

  testWidgets('终端窗格直接销毁时两层变更密钥确认必须一并移除', (tester) async {
    await _removePaneWithActivePrompt(tester, changedHostKey: true);
  });

  testWidgets('兼容 Material Navigator：密码与私钥口令使用 Fluent 掩码，成功和取消', (
    tester,
  ) async {
    final dismissed = ValueNotifier(false);
    PromptAnswer? answer;
    await _open(
      tester,
      _prompt(PromptKind.password, canRemember: true),
      dismissed,
      (result) => answer = result,
      legacy: true,
    );
    expect(find.byType(fluent.ContentDialog), findsOneWidget);
    expect(find.byType(material.AlertDialog), findsNothing);
    expect(
      tester.widget<fluent.TextBox>(find.byType(fluent.TextBox)).obscureText,
      isTrue,
    );
    await tester.enterText(find.byType(fluent.TextBox), 'secret');
    await tester.tap(find.text('连接成功后保存到钥匙串'));
    await tester.pump();
    await tester.tap(find.text('连接'));
    await tester.pumpAndSettle();
    expect(answer?.answers, ['secret']);
    expect(answer?.remember, isTrue);

    await _open(
      tester,
      _prompt(PromptKind.passphrase, retry: true, name: 'laptop'),
      ValueNotifier(false),
      (result) => answer = result,
    );
    expect(find.textContaining('私钥「laptop」'), findsOneWidget);
    expect(find.text('口令不正确，请重新输入'), findsOneWidget);
    await tester.tap(find.text('取消'));
    await tester.pumpAndSettle();
    expect(answer?.accept, isFalse);
  });

  testWidgets('OpenPGP PIN 剩余尝试警告、掩码、仅内存记住', (tester) async {
    PromptAnswer? answer;
    await _open(
      tester,
      _prompt(PromptKind.cardPin, name: 'CanoKey', triesLeft: 1, retry: true),
      ValueNotifier(false),
      (result) => answer = result,
    );
    expect(find.text('PIN 不正确。只剩最后 1 次，再错卡会锁定'), findsOneWidget);
    expect(
      tester.widget<fluent.TextBox>(find.byType(fluent.TextBox)).obscureText,
      isTrue,
    );
    await tester.enterText(find.byType(fluent.TextBox), '123456');
    await tester.tap(find.text('记住 PIN，直到退出 App'));
    await tester.pump();
    await tester.tap(find.text('连接'));
    await tester.pumpAndSettle();
    expect(answer?.answers, ['123456']);
    expect(answer?.remember, isTrue);
  });

  testWidgets('keyboard-interactive 多字段按顺序回答，只掩码非回显字段', (tester) async {
    PromptAnswer? answer;
    await _open(
      tester,
      _prompt(
        PromptKind.keyboardInteractive,
        instruction: '请输入动态验证码',
        fields: const [
          PromptField(label: 'Password: ', echo: false),
          PromptField(label: 'Code: ', echo: true),
        ],
      ),
      ValueNotifier(false),
      (result) => answer = result,
    );
    expect(find.text('请输入动态验证码'), findsOneWidget);
    expect(
      tester
          .widget<fluent.TextBox>(find.byType(fluent.TextBox).at(0))
          .obscureText,
      isTrue,
    );
    expect(
      tester
          .widget<fluent.TextBox>(find.byType(fluent.TextBox).at(1))
          .obscureText,
      isFalse,
    );
    await tester.enterText(find.byType(fluent.TextBox).at(0), 'secret');
    await tester.enterText(find.byType(fluent.TextBox).at(1), '123456');
    await tester.tap(find.text('确定'));
    await tester.pumpAndSettle();
    expect(answer?.answers, ['secret', '123456']);
  });

  testWidgets('首次主机展示实际地址、算法与指纹；明确允许才接受', (tester) async {
    PromptAnswer? answer;
    await _open(
      tester,
      _prompt(PromptKind.hostKey),
      ValueNotifier(false),
      (result) => answer = result,
    );
    expect(find.textContaining('nas.lan:2222（10.0.0.8）'), findsOneWidget);
    expect(find.textContaining('ssh-ed25519\nSHA256:abc'), findsOneWidget);
    await tester.tap(find.text('信任并连接'));
    await tester.pumpAndSettle();
    expect(answer?.accept, isTrue);
    expect(answer?.answers, isEmpty);
  });

  testWidgets('变更密钥默认断开；勾选与二次核对缺一不可', (tester) async {
    PromptAnswer? answer;
    await _open(
      tester,
      _prompt(PromptKind.hostKey, changed: true),
      ValueNotifier(false),
      (result) => answer = result,
    );
    expect(
      tester
          .widget<fluent.Button>(find.widgetWithText(fluent.Button, '替换旧密钥并连接'))
          .onPressed,
      isNull,
    );
    await tester.tap(find.text('我已通过其他途径核对新指纹'));
    await tester.pump();
    await tester.tap(find.text('替换旧密钥并连接'));
    await tester.pumpAndSettle();
    expect(find.text('再次确认更换主机密钥'), findsOneWidget);
    expect(answer, isNull);
    await tester.tap(find.text('断开').last);
    await tester.pumpAndSettle();
    expect(answer?.accept, isFalse);

    await _open(
      tester,
      _prompt(PromptKind.hostKey, changed: true),
      ValueNotifier(false),
      (result) => answer = result,
    );
    await tester.tap(find.text('我已通过其他途径核对新指纹'));
    await tester.pump();
    await tester.tap(find.text('替换旧密钥并连接'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('确认替换并连接'));
    await tester.pumpAndSettle();
    expect(answer?.accept, isTrue);
  });

  testWidgets('会话失败或超时撤销二次确认，陈旧提示不能再被接受', (tester) async {
    final dismissed = ValueNotifier(false);
    PromptAnswer? answer;
    await _open(
      tester,
      _prompt(PromptKind.hostKey, changed: true, promptId: 40),
      dismissed,
      (result) => answer = result,
    );
    await tester.tap(find.text('我已通过其他途径核对新指纹'));
    await tester.pump();
    await tester.tap(find.text('替换旧密钥并连接'));
    await tester.pumpAndSettle();
    dismissed.value = true;
    await tester.pumpAndSettle();
    expect(find.byType(fluent.ContentDialog), findsNothing);
    expect(answer?.accept, isFalse);
    expect(answer?.answers, isEmpty);
  });

  testWidgets('提示到达前会话已结束：不显示弹窗，只返回拒绝', (tester) async {
    final dismissed = ValueNotifier(true);
    PromptAnswer? answer;
    await _open(
      tester,
      _prompt(PromptKind.password, sessionId: 9, promptId: 41),
      dismissed,
      (result) => answer = result,
    );
    expect(find.byType(fluent.ContentDialog), findsNothing);
    expect(answer?.accept, isFalse);
  });

  testWidgets('兼容工作区窄窗二次核对：路由仍为 Fluent，取消即拒绝', (tester) async {
    final dismissed = ValueNotifier(false);
    PromptAnswer? answer;
    await _open(
      tester,
      _prompt(PromptKind.hostKey, changed: true),
      dismissed,
      (result) => answer = result,
      legacy: true,
      width: 375,
      highContrast: true,
    );
    await tester.tap(find.text('我已通过其他途径核对新指纹'));
    await tester.pump();
    await tester.tap(find.text('替换旧密钥并连接'));
    await tester.pumpAndSettle();
    expect(find.byType(material.AlertDialog), findsNothing);
    expect(find.byType(fluent.ContentDialog), findsNWidgets(2));
    expect(tester.takeException(), isNull);
    dismissed.value = true;
    await tester.pumpAndSettle();
    expect(find.byType(fluent.ContentDialog), findsNothing);
    expect(answer?.accept, isFalse);
  });

  for (final width in [375.0, 768.0, 1280.0]) {
    testWidgets('$width 宽：高对比度、减少动画下仍可拒绝', (tester) async {
      final dismissed = ValueNotifier(false);
      PromptAnswer? answer;
      await _open(
        tester,
        _prompt(PromptKind.password, retry: true),
        dismissed,
        (result) => answer = result,
        width: width,
        highContrast: true,
      );
      expect(
        tester.getSize(find.byType(fluent.ContentDialog)).width,
        lessThanOrEqualTo(width),
      );
      expect(tester.takeException(), isNull);
      dismissed.value = true;
      await tester.pumpAndSettle();
      expect(answer?.accept, isFalse);
    });
  }
}
