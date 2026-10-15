import 'dart:async';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/desktop/native_input/desktop_native_host.dart';
import 'package:guosh_shell/src/desktop/workspace/desktop_workspace_scope.dart';
import 'package:guosh_shell/src/terminal/desktop_font.dart';
import 'package:guosh_shell/src/terminal/desktop_session_controller.dart';
import 'package:guosh_shell/src/terminal/desktop_session_gateway.dart';
import 'package:guosh_shell/src/terminal/desktop_terminal_theme.dart';
import 'package:guosh_shell/src/terminal/session_target.dart';
import 'package:rinf/rinf.dart';

Uint64 u64(int value) => Uint64(BigInt.from(value));
final source = DesktopSource(sessionId: 61, generation: u64(7));

DesktopPreparationResult prepared(int request) => DesktopPreparationResult(
  requestId: request,
  prepareRequestId: request,
  source: source,
  identity: 'frozen',
  state: DesktopPreparationState.prepared,
  failure: DesktopFailure.none,
  remainingMs: 5000,
  snapshot: DesktopPreparedSnapshot(
    profile: DesktopTerminalProfile(
      id: 'selected',
      name: '',
      version: 1,
      terminalType: 'xterm-256color',
      initialCols: 90,
      initialRows: 30,
      scrollbackLines: 1000,
      fontFamily: 'ignored',
      fontSize: 18,
      colorScheme: 'default',
      keyBindings: const [],
      leftAltAsMeta: true,
      rightAltAsMeta: false,
      enableCsiU: false,
      enableKittyKeyboard: false,
      mouseReporting: true,
      scrollOnOutput: true,
      scrollOnKeypress: true,
      answerback: '',
    ),
    font: ProfileFontSnapshot(
      version: 1,
      profileId: 'selected',
      configurationRevision: u64(2),
      saved: const ProfileFontBinding(
        version: 1,
        mode: ProfileFontMode.explicitFamily,
      ),
      rawFamily: 'ignored',
      runtimeSource: ProfileFontRuntimeSource.profileBinding,
      actualFamily: 'MesloLGS NF',
    ),
    fontProjection: 'primary',
    fallbackProjection: 'fallback',
    fallbackFamily: 'MesloLGS NF',
    resourceBytes: 0,
    adjustments: const [],
  ),
);

class DesktopSessionTestGateway implements DesktopSessionGateway {
  final preparationBus =
      StreamController<RustSignalPack<DesktopPreparationResult>>.broadcast(
        sync: true,
      );
  final lifecycleBus =
      StreamController<RustSignalPack<DesktopNativeLifecycle>>.broadcast(
        sync: true,
      );
  final actionBus =
      StreamController<RustSignalPack<DesktopNativeActionReady>>.broadcast(
        sync: true,
      );
  final commandBus =
      StreamController<RustSignalPack<DesktopNativeCommandResult>>.broadcast(
        sync: true,
      );
  final textEndBus =
      StreamController<RustSignalPack<DesktopNativeTextEndRequired>>.broadcast(
        sync: true,
      );
  final menuBus =
      StreamController<RustSignalPack<DesktopNativeContextMenuReady>>.broadcast(
        sync: true,
      );
  final frameBus = StreamController<RustSignalPack<DesktopFrame>>.broadcast(
    sync: true,
  );
  final projectionBus =
      StreamController<RustSignalPack<DesktopNativeProjection>>.broadcast(
        sync: true,
      );
  void Function(DesktopNativeCommand)? onCommand;
  final statusBus =
      StreamController<RustSignalPack<DesktopSessionStatus>>.broadcast(
        sync: true,
      );
  final promptBus = StreamController<RustSignalPack<DesktopPrompt>>.broadcast(
    sync: true,
  );
  final eventResultBus =
      StreamController<RustSignalPack<DesktopEventResult>>.broadcast(
        sync: true,
      );
  final clipboardBus =
      StreamController<RustSignalPack<DesktopClipboard>>.broadcast(sync: true);
  final prepares = <DesktopPrepare>[];
  final starts = <DesktopStart>[];
  final cancels = <DesktopCancel>[];
  final commands = <DesktopNativeCommand>[];
  final queries = <DesktopNativeLifecycleQuery>[];
  final orderedEvents = <DesktopOrdered>[];

  /// 有序事件的回执；默认受理。返回 null 表示暂不回执。
  DesktopEventResult? Function(DesktopOrdered request)? onOrdered;

  static DesktopEventResult accept(
    DesktopOrdered request, {
    DesktopFailure failure = DesktopFailure.none,
    DesktopAction? action,
  }) => DesktopEventResult(
    source: request.source,
    identity: request.identity,
    seq: request.seq,
    failure: failure,
    action: action,
  );

  @override
  get preparations => preparationBus.stream;
  @override
  get lifecycles => lifecycleBus.stream;
  @override
  get actions => actionBus.stream;
  @override
  Stream<RustSignalPack<DesktopNativeCommandResult>> get commandResults =>
      commandBus.stream;
  @override
  Stream<RustSignalPack<DesktopFrame>> get frames => frameBus.stream;
  @override
  Stream<RustSignalPack<DesktopSessionStatus>> get statuses => statusBus.stream;
  @override
  Stream<RustSignalPack<DesktopPrompt>> get prompts => promptBus.stream;
  @override
  Stream<RustSignalPack<DesktopEventResult>> get eventResults =>
      eventResultBus.stream;
  @override
  Stream<RustSignalPack<DesktopClipboard>> get clipboards =>
      clipboardBus.stream;
  @override
  void ordered(DesktopOrdered request) {
    orderedEvents.add(request);
    final result = onOrdered == null ? accept(request) : onOrdered!(request);
    // 与 Rust 一样异步回执，避免在同步广播回调中重入。
    if (result != null) {
      scheduleMicrotask(
        () => eventResultBus.add(RustSignalPack(result, Uint8List(0))),
      );
    }
  }

  @override
  Stream<RustSignalPack<DesktopSelection>> get selections =>
      const Stream.empty();
  @override
  Stream<RustSignalPack<DesktopNativeProjection>> get projections =>
      projectionBus.stream;
  @override
  Stream<RustSignalPack<DesktopNativeInvalidation>> get invalidations =>
      const Stream.empty();
  @override
  Stream<RustSignalPack<DesktopNativeLifecycleQueryResult>>
  get lifecycleQueries => const Stream.empty();
  @override
  Stream<RustSignalPack<DesktopNativeTextEndRequired>> get textEnds =>
      textEndBus.stream;
  @override
  Stream<RustSignalPack<DesktopNativeContextMenuReady>> get contextMenus =>
      menuBus.stream;
  @override
  void prepare(DesktopPrepare request) {
    expect(preparationBus.hasListener, isTrue);
    expect(lifecycleBus.hasListener, isTrue);
    prepares.add(request);
  }

  @override
  void start(DesktopStart request) => starts.add(request);
  @override
  void cancel(DesktopCancel request) => cancels.add(request);
  @override
  void command(DesktopNativeCommand request) {
    commands.add(request);
    onCommand?.call(request);
  }

  @override
  void reply(DesktopReply request) => throw StateError('本用例没有认证对话框');
  @override
  void queryLifecycle(DesktopNativeLifecycleQuery request) =>
      queries.add(request);
  void emit(DesktopPreparationResult message) =>
      preparationBus.add(RustSignalPack(message, Uint8List(0)));
  void lifecycle(
    NativeLifecycleState state, {
    int revision = 1,
    int attempt = 12,
    int epoch = 4,
  }) => lifecycleBus.add(
    RustSignalPack(
      DesktopNativeLifecycle(
        version: 1,
        source: source,
        identity: 'frozen',
        attempt: u64(attempt),
        focusEpoch: u64(epoch),
        lifecycleRevision: u64(revision),
        state: state,
      ),
      Uint8List(0),
    ),
  );
  void action(DesktopAction action) => actionBus.add(
    RustSignalPack(
      DesktopNativeActionReady(
        version: 1,
        source: source,
        identity: 'frozen',
        attempt: u64(12),
        focusEpoch: u64(4),
        effect: u64(11),
      ),
      Uint8List(0),
    ),
  );
  Future<void> dispose() async {
    await preparationBus.close();
    await lifecycleBus.close();
    await actionBus.close();
    await commandBus.close();
    await textEndBus.close();
    await menuBus.close();
    await frameBus.close();
    await projectionBus.close();
    await statusBus.close();
    await promptBus.close();
    await eventResultBus.close();
    await clipboardBus.close();
  }
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  const channel = MethodChannel('test/session-native-host');
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
  late DesktopSessionTestGateway gateway;
  late DesktopSessionController controller;
  final hostCalls = <MethodCall>[];
  late String claimAction;
  late Future<Object?> Function(MethodCall) clipboard;
  Future<Object?> Function(MethodCall)? layoutReply;
  Future<Object?> Function(MethodCall)? focusReply;
  Future<Object?> Function(MethodCall)? presentationReply;
  setUp(() {
    gateway = DesktopSessionTestGateway();
    hostCalls.clear();
    claimAction = 'search';
    layoutReply = null;
    focusReply = null;
    presentationReply = null;
    messenger.setMockMethodCallHandler(channel, (call) async {
      hostCalls.add(call);
      if (call.method == 'textLayout' && layoutReply != null) {
        return await layoutReply!(call);
      }
      if (call.method == 'requestFocus' && focusReply != null) {
        return await focusReply!(call);
      }
      if (call.method == 'presentedFrame' && presentationReply != null) {
        return await presentationReply!(call);
      }
      return switch (call.method) {
        'attachPane' => {
          'version': 1,
          'state': 'attached',
          'paneToken': 'origin-pane',
        },
        'requestFocus' => {
          'version': 1,
          'state': 'active',
          'attempt': '12',
          'focusEpoch': '4',
        },
        'retirePane' => {
          'version': 1,
          'state': (call.arguments as Map)['phase'] == 'quiet'
              ? 'nativequiet'
              : 'released',
        },
        'claimAction' => {
          'version': 1,
          'state': 'claimed',
          'claimToken': 'origin-claim',
          'action': claimAction,
          'payload': '完整🙂\n内容',
        },
        'completeAction' => {
          'version': 1,
          'state': (call.arguments as Map)['kind'] == 'abort'
              ? 'aborted'
              : 'completed',
        },
        'textEndRequired' => {'version': 1, 'state': 'accepted'},
        'textLayout' => {'version': 1, 'state': 'laidOut'},
        'presentedFrame' => {'version': 1, 'state': 'presented'},
        _ => throw MissingPluginException(),
      };
    });
    clipboard = (_) async => null;
    messenger.setMockMethodCallHandler(
      SystemChannels.platform,
      (call) => clipboard(call),
    );
    controller = DesktopSessionController(
      source: source,
      target: const SessionTarget.quick(
        host: 'example.test',
        port: 22,
        username: 'test',
        profileId: 'selected',
      ),
      gateway: gateway,
      host: DesktopNativeHost(channel: channel),
      font: DesktopFontController(
        cache: DesktopFontCache(register: (_, _) async {}),
      ),
    )..canFocus = () => true;
    controller.begin();
  });
  tearDown(() async {
    controller.dispose();
    await gateway.dispose();
    messenger.setMockMethodCallHandler(channel, null);
    messenger.setMockMethodCallHandler(SystemChannels.platform, null);
  });

  Future<void> measured() async {
    gateway.emit(prepared(gateway.prepares.single.requestId));
    await Future<void>.delayed(Duration.zero);
    controller.measure(
      const DesktopGeometry(
        cols: 103,
        rows: 37,
        pixelWidth: 824,
        pixelHeight: 740,
        dpi: 96,
      ),
      {'width': 824.0},
    );
  }

  Future<void> ready() async {
    await measured();
    gateway.lifecycle(NativeLifecycleState.authenticated);
    await Future<void>.delayed(Duration.zero);
    expect(controller.active, isTrue);
  }

  Future<void> compositionNotice(Map<String, Object?> fields) async {
    final done = Completer<void>();
    messenger.handlePlatformMessage(
      channel.name,
      const StandardMethodCodec().encodeMethodCall(
        MethodCall('compositionLayout', {
          'version': 1,
          'paneToken': 'origin-pane',
          'contextId': '8',
          'transactionId': '19',
          'editRevision': '1',
          'acpStart': 0,
          'text': '中',
          'frameRevision': 1,
          ...fields,
        }),
      ),
      (_) => done.complete(),
    );
    await done.future;
  }

  void compositionFrame(int seq) {
    controller.frame = DesktopPresentedFrame(
      DesktopFrame(
        source: source,
        identity: 'frozen',
        frame: FrameUpdate(
          sessionId: source.sessionId,
          cols: 90,
          rows: 30,
          seq: seq,
          topStableRow: 0,
          firstStableRow: 0,
          screenTopStableRow: 0,
          cursorCol: 0,
          cursorRow: 0,
          title: '',
          mouseReporting: false,
          alternateScreen: false,
        ),
      ),
      Uint8List(0),
      DesktopNativeProjection(
        source: source,
        identity: 'frozen',
        attempt: u64(12),
        focusEpoch: u64(4),
        frameRevision: seq,
        appliedEffect: u64(0),
        rowRevision: u64(1),
        followBottom: true,
        firstStableRow: 0,
        screenTopStableRow: 0,
        viewportTop: 0,
        mouseReporting: false,
        bracketedPaste: false,
      ),
    );
  }

  for (final initial in [true, false]) {
    test('检查点 focus 同 root Authenticated epoch 先于 RPC，首次：$initial', () async {
      if (initial) {
        await measured();
      } else {
        await ready();
      }
      final reply = Completer<Object?>();
      focusReply = (_) => reply.future;
      Future<bool>? focus;
      if (initial) {
        gateway.lifecycle(NativeLifecycleState.authenticated);
      } else {
        focus = controller.requestFocus();
      }
      await Future<void>.delayed(Duration.zero);
      gateway.lifecycle(
        NativeLifecycleState.authenticated,
        revision: 2,
        epoch: 5,
      );
      expect(controller.active, isFalse);
      reply.complete({
        'version': 1,
        'state': 'active',
        'attempt': '12',
        'focusEpoch': '5',
      });
      if (focus != null) expect(await focus, isTrue);
      await Future<void>.delayed(Duration.zero);
      expect(controller.active, isTrue);
      expect(hostCalls.where((call) => call.method == 'retirePane'), isEmpty);
    });
  }

  test('检查点 静态帧 epoch 升级无需新输出，宿主使用当前 binding epoch', () async {
    await ready();
    compositionFrame(1);
    final original = controller.frame!;
    focusReply = (_) async => {
      'version': 1,
      'state': 'active',
      'attempt': '12',
      'focusEpoch': '5',
    };
    expect(await controller.requestFocus(), isTrue);
    gateway.lifecycle(
      NativeLifecycleState.authenticated,
      revision: 2,
      epoch: 5,
    );
    presentationReply = (call) async {
      expect((call.arguments as Map)['focusEpoch'], '5');
      expect((call.arguments as Map)['frameRevision'], 1);
      return {'version': 1, 'state': 'presented'};
    };
    await controller.presented(original, {'width': 824.0});
    expect(controller.frame, same(original));
    expect(controller.error, isEmpty);
    expect(controller.active, isTrue);
  });

  for (final invalidation in [
    'revoked',
    'attempt',
    'inactive',
    'permission',
    'rpcEpoch',
  ]) {
    test('检查点 pending focus 遇到 $invalidation 必须取消，不以生命周期冒充 host', () async {
      await ready();
      final reply = Completer<Object?>();
      focusReply = (_) => reply.future;
      final focused = controller.requestFocus();
      await Future<void>.delayed(Duration.zero);
      gateway.lifecycle(
        NativeLifecycleState.authenticated,
        revision: 2,
        epoch: 5,
      );
      switch (invalidation) {
        case 'revoked':
          gateway.lifecycle(
            NativeLifecycleState.inputRevoked,
            revision: 3,
            epoch: 6,
          );
        case 'attempt':
          gateway.lifecycle(
            NativeLifecycleState.connecting,
            revision: 3,
            attempt: 13,
            epoch: 6,
          );
        case 'inactive':
          final done = Completer<void>();
          messenger.handlePlatformMessage(
            channel.name,
            const StandardMethodCodec().encodeMethodCall(
              const MethodCall('paneState', {
                'version': 1,
                'paneToken': 'origin-pane',
                'state': 'inactive',
              }),
            ),
            (_) => done.complete(),
          );
          await done.future;
        case 'permission':
          controller.canFocus = () => false;
        case 'rpcEpoch':
          break;
      }
      reply.complete({
        'version': 1,
        'state': 'active',
        'attempt': '12',
        'focusEpoch': invalidation == 'rpcEpoch' ? '4' : '5',
      });
      expect(await focused, isFalse);
      await Future<void>.delayed(Duration.zero);
      expect(controller.active, isFalse);
      expect(
        hostCalls.where((call) => call.method == 'retirePane'),
        isNotEmpty,
      );
    });
  }

  for (final lifecycleFirst in [false, true]) {
    test('检查点 InputRevoked 并发 focus 共用原 RPC，生命周期先返回：$lifecycleFirst', () async {
      await ready();
      gateway.lifecycle(
        NativeLifecycleState.inputRevoked,
        revision: 2,
        epoch: 5,
      );
      await Future<void>.delayed(Duration.zero);
      final reply = Completer<Object?>();
      focusReply = (_) => reply.future;
      final first = controller.requestFocus();
      await Future<void>.delayed(Duration.zero);
      final second = controller.requestFocus();
      final requestCount = hostCalls
          .where((call) => call.method == 'requestFocus')
          .length;
      expect(requestCount, 2);
      expect(controller.active, isFalse);
      if (lifecycleFirst) {
        gateway.lifecycle(
          NativeLifecycleState.authenticated,
          revision: 3,
          epoch: 6,
        );
        expect(controller.active, isFalse);
      }
      reply.complete({
        'version': 1,
        'state': 'active',
        'attempt': '12',
        'focusEpoch': '6',
      });
      expect(await first, isTrue);
      expect(await second, isTrue);
      if (!lifecycleFirst) {
        gateway.lifecycle(
          NativeLifecycleState.authenticated,
          revision: 3,
          epoch: 6,
        );
      }
      expect(controller.active, isTrue);
      expect(
        hostCalls.where((call) => call.method == 'requestFocus').length,
        requestCount,
      );
    });
  }

  for (final epoch in [4, 5]) {
    test('检查点 旧 pending presentation 不重绑新 focus owner，epoch=$epoch', () async {
      await ready();
      compositionFrame(1);
      final original = controller.frame!;
      final reply = Completer<Object?>();
      presentationReply = (_) => reply.future;
      final old = controller.presented(original, {'width': 824.0});
      await Future<void>.delayed(Duration.zero);
      await controller.presented(original, {'width': 820.0});
      focusReply = (_) async => {
        'version': 1,
        'state': 'active',
        'attempt': '12',
        'focusEpoch': '$epoch',
      };
      expect(await controller.requestFocus(), isTrue);
      reply.complete({'version': 1, 'state': 'rejected'});
      await old;
      await Future<void>.delayed(Duration.zero);
      expect(
        hostCalls.where((call) => call.method == 'presentedFrame').length,
        1,
      );
      expect(controller.error, isEmpty);
      presentationReply = (call) async {
        expect((call.arguments as Map)['focusEpoch'], '$epoch');
        return {'version': 1, 'state': 'presented'};
      };
      await controller.presented(original, {'width': 810.0});
      expect(controller.frame, same(original));
      expect(
        hostCalls.where((call) => call.method == 'presentedFrame').length,
        2,
      );
    });
  }

  test('检查点 旧 presentation 回执不会阻止新 root 静态帧当前 binding 呈现', () async {
    await ready();
    compositionFrame(1);
    final original = controller.frame!;
    final reply = Completer<Object?>();
    int sends = 0;
    presentationReply = (call) async {
      if (++sends == 1) return await reply.future;
      expect((call.arguments as Map)['focusEpoch'], '5');
      expect((call.arguments as Map)['width'], 900.0);
      return {'version': 1, 'state': 'presented'};
    };
    final old = controller.presented(original, {'width': 824.0});
    await Future<void>.delayed(Duration.zero);
    await controller.quiet();
    focusReply = (_) async => {
      'version': 1,
      'state': 'active',
      'attempt': '12',
      'focusEpoch': '5',
    };
    expect(await controller.requestFocus(), isTrue);
    await controller.presented(original, {'width': 900.0});
    reply.complete({'version': 1, 'state': 'rejected'});
    await old;
    await Future<void>.delayed(Duration.zero);
    expect(sends, 2);
    expect(controller.error, isEmpty);
    expect(controller.active, isTrue);
  });

  test('检查点 composition 超64退休 context持续推进，旧 open/ended 不复活或清新输入', () async {
    await ready();
    compositionFrame(1);
    for (int context = 1; context <= 80; context++) {
      final fields = {'contextId': '$context', 'transactionId': '19'};
      await compositionNotice(fields);
      expect(controller.composition?['contextId'], '$context');
      await controller.compositionPainted(controller.composition!, {});
      if (context > 1) {
        final current = controller.composition;
        await compositionNotice({
          'contextId': '${context - 1}',
          'transactionId': '19',
        });
        await compositionNotice({
          'contextId': '${context - 1}',
          'transactionId': '19',
          'state': 'ended',
        });
        expect(controller.composition, same(current));
      }
      await compositionNotice({...fields, 'state': 'ended'});
      expect(controller.composition, isNull);
    }
    expect(hostCalls.where((call) => call.method == 'textLayout').length, 80);
    await compositionNotice({'contextId': '1', 'transactionId': '19'});
    expect(controller.composition, isNull);
    expect(controller.error, isEmpty);
  });

  test('composition ended 按事务清理，不依赖帧且不能复活或清新事务', () async {
    await ready();
    compositionFrame(1);
    await compositionNotice({});
    expect(controller.composition, isNotNull);
    compositionFrame(2);
    await compositionNotice({'state': 'ended', 'text': ''});
    expect(controller.composition, isNull);
    expect(controller.editableSafe, isFalse);
    expect(hostCalls.where((call) => call.method == 'retirePane'), isEmpty);
    await compositionNotice({'frameRevision': 2});
    expect(controller.composition, isNull);
    await compositionNotice({'transactionId': '20', 'frameRevision': 2});
    final current = controller.composition;
    expect(current, isNotNull);
    await compositionNotice({'state': 'ended', 'text': ''});
    expect(controller.composition, same(current));
    await compositionNotice({
      'transactionId': '20',
      'editRevision': '0',
      'frameRevision': 2,
    });
    expect(controller.composition, same(current));
  });

  test('composition 异步旧布局拒绝不污染新输入，当前有效拒绝仍报告', () async {
    await ready();
    compositionFrame(1);
    await compositionNotice({});
    final reply = Completer<Object?>();
    messenger.setMockMethodCallHandler(channel, (_) => reply.future);
    final painted = controller.compositionPainted(controller.composition!, {});
    await compositionNotice({'editRevision': '2'});
    reply.complete({'version': 1, 'state': 'rejected'});
    await painted;
    expect(controller.error, isEmpty);
    await controller.compositionPainted(controller.composition!, {});
    expect(controller.error, isNotEmpty);
  });

  test('composition 来源与生命周期隔离，不把 UI 清理当静默 ACK', () async {
    await ready();
    compositionFrame(1);
    await compositionNotice({'paneToken': 'foreign'});
    expect(controller.composition, isNull);
    await compositionNotice({});
    gateway.lifecycle(
      NativeLifecycleState.authenticated,
      attempt: 13,
      revision: 2,
    );
    await compositionNotice({});
    expect(controller.composition, isNull);
  });

  for (final boundary in [
    'context',
    'transaction',
    'edit',
    'frame',
    'owner',
    'root',
    'closed',
  ]) {
    test('composition 异步异常遇到 $boundary 变更不污染当前输入', () async {
      await ready();
      compositionFrame(1);
      await compositionNotice({});
      final reply = Completer<Object?>();
      layoutReply = (_) => reply.future;
      final original = controller.composition!;
      final painted = controller.compositionPainted(original, {});
      await Future<void>.delayed(Duration.zero);
      switch (boundary) {
        case 'context':
          await compositionNotice({'contextId': '9'});
        case 'transaction':
          await compositionNotice({'transactionId': '20'});
        case 'edit':
          await compositionNotice({'editRevision': '2'});
        case 'frame':
          compositionFrame(2);
        case 'owner':
          await controller.requestFocus();
        case 'root':
          await controller.quiet();
        case 'closed':
          final closed = controller.close();
          gateway.lifecycle(NativeLifecycleState.ended, revision: 2);
          await closed;
      }
      reply.completeError(PlatformException(code: 'layout-rejected'));
      await painted;
      expect(controller.error, isEmpty);
    });
  }

  test('composition 当前有效异步异常必须报告失败', () async {
    await ready();
    compositionFrame(1);
    await compositionNotice({});
    layoutReply = (_) async => throw PlatformException(code: 'layout-rejected');
    await controller.compositionPainted(controller.composition!, {});
    expect(controller.phase, DesktopPanePhase.failed);
    expect(controller.error, isNotEmpty);
  });

  test('先订阅再准备，冻结显式 profile，实际度量后只发一次 Start', () async {
    final prepare = gateway.prepares.single;
    expect(prepare.profileId, 'selected');
    expect(prepare.source, source);
    gateway.emit(prepared(prepare.requestId));
    await Future<void>.delayed(Duration.zero);
    expect(gateway.starts, isEmpty);
    const geometry = DesktopGeometry(
      cols: 103,
      rows: 37,
      pixelWidth: 824,
      pixelHeight: 740,
      dpi: 96,
    );
    controller.measure(geometry, {'width': 824.0});
    controller.measure(geometry, {'width': 824.0});
    expect(gateway.starts.single.geometry, geometry);
    expect(gateway.starts.single.fontProjection, 'primary');
  });

  DesktopNativeCommandResult result(
    DesktopNativeCommand command, {
    NativeCommandAdmission admission = NativeCommandAdmission.accepted,
    int consumed = 0,
    NativeCommandFailure failure = NativeCommandFailure.none,
  }) => DesktopNativeCommandResult(
    version: command.version,
    source: command.source,
    identity: command.identity,
    attempt: command.attempt,
    focusEpoch: command.focusEpoch,
    requestId: command.requestId,
    admission: admission,
    effect: u64(31),
    consumedLatches: consumed,
    failure: failure,
    guiModifierRevision: command.command is NativeCommandGuiIntent
        ? (command.command as NativeCommandGuiIntent).value.modifierRevision
        : null,
  );
  void admit(DesktopNativeCommandResult value) =>
      gateway.commandBus.add(RustSignalPack(value, Uint8List(0)));

  test('GUI 唯一 writer 共用 request 序列，拒绝不消费，实际位只结算一次', () async {
    await ready();
    controller.tapModifier('ctrl');
    controller.lockModifier('alt');
    final first = controller.guiInput(
      const GuiInputGuiKey(value: GuiKeyArrowUp()),
    );
    final command = gateway.commands.last;
    final intent = (command.command as NativeCommandGuiIntent).value;
    expect(
      intent.modifiers,
      const GuiModifiers(button: 0, latched: 1, locked: 2),
    );
    expect(command.attempt, u64(12));
    expect(command.focusEpoch, u64(4));
    expect(
      await controller.guiInput(const GuiInputGuiText(value: 'x')),
      NativeCommandAdmission.rejected,
    );
    expect(gateway.commands.length, 1);
    for (final wrong in [
      result(command).copyWith(version: 2),
      result(command).copyWith(source: source.copyWith(generation: u64(99))),
      result(command).copyWith(identity: 'foreign'),
      result(command).copyWith(attempt: u64(13)),
      result(command).copyWith(focusEpoch: u64(5)),
      result(command).copyWith(requestId: u64(99)),
      result(command).copyWith(guiModifierRevision: () => u64(99)),
    ]) {
      admit(wrong);
    }
    expect(controller.guiBusy, isTrue);
    admit(result(command, admission: NativeCommandAdmission.rejected));
    expect(await first, NativeCommandAdmission.rejected);
    expect(controller.isModifierLatched('ctrl'), isTrue);
    final second = controller.guiInput(
      const GuiInputGuiKey(value: GuiKeyArrowUp()),
    );
    final next = gateway.commands.last;
    expect(
      next.requestId.toBigInt(),
      greaterThan(command.requestId.toBigInt()),
    );
    admit(result(next, consumed: 1));
    expect(await second, NativeCommandAdmission.accepted);
    expect(controller.isModifierLatched('ctrl'), isFalse);
    expect(controller.isModifierLocked('alt'), isTrue);
    controller.tapModifier('ctrl');
    admit(result(next, consumed: 1));
    expect(controller.isModifierLatched('ctrl'), isTrue);
    expect(
      () => controller.event(const DesktopEventText(text: 'fake')),
      throwsArgumentError,
    );
  });

  test('GUI 旧 modifier revision 与 owner 回执不消费新状态', () async {
    await ready();
    controller.tapModifier('ctrl');
    final old = controller.guiInput(const GuiInputGuiText(value: 'c'));
    final command = gateway.commands.last;
    controller.tapModifier('ctrl');
    controller.tapModifier('ctrl');
    controller.tapModifier('ctrl');
    admit(result(command, consumed: 1));
    expect(await old, NativeCommandAdmission.accepted);
    expect(controller.isModifierLatched('ctrl'), isTrue);
    final after = controller.guiInput(const GuiInputGuiText(value: 'x'));
    final pending = gateway.commands.last;
    await controller.quiet();
    await controller.requestFocus();
    controller.tapModifier('ctrl');
    admit(result(pending, consumed: 1));
    expect(await after, NativeCommandAdmission.accepted);
    expect(controller.isModifierLatched('ctrl'), isTrue);
  });

  testWidgets('GUI 5秒或 Duplicate 隔离保留 pending，控制 ACK 与断开仍可发送', (tester) async {
    await tester.runAsync(ready);
    controller.tapModifier('ctrl');
    unawaited(controller.guiInput(const GuiInputGuiText(value: 'x')));
    final original = gateway.commands.last;
    await tester.pump(const Duration(seconds: 5));
    expect(controller.guiUnknown, isTrue);
    expect(controller.guiBusy, isTrue);
    unawaited(controller.guiInput(const GuiInputGuiText(value: 'y')));
    expect(gateway.commands.length, 1);
    controller.event(
      const DesktopEventFrameAck(frameSeq: 1),
      requiresFocus: false,
    );
    controller.event(const DesktopEventDisconnect(), requiresFocus: false);
    expect(gateway.commands.length, 3);
    admit(
      result(
        original,
        admission: NativeCommandAdmission.unknown,
        failure: NativeCommandFailure.duplicate,
      ),
    );
    expect(controller.guiUnknown, isTrue);
    admit(result(original, consumed: 1));
    expect(controller.guiBusy, isFalse);
    expect(controller.guiUnknown, isFalse);
    expect(controller.isModifierLatched('ctrl'), isFalse);
    await tester.pump();
    controller.dispose();
  });

  test('GUI 文本 scalar 保持原样，32上限，多scalar不消费，按钮位覆盖不消费同位', () async {
    await ready();
    controller.tapModifier('ctrl');
    final text = '🙂中\r\ná';
    final sent = controller.guiInput(GuiInputGuiText(value: text));
    final command = gateway.commands.last;
    expect(
      (command.command as NativeCommandGuiIntent).value.input,
      GuiInputGuiText(value: text),
    );
    admit(result(command));
    expect(await sent, NativeCommandAdmission.accepted);
    expect(controller.isModifierLatched('ctrl'), isTrue);
    final limit = controller.guiInput(
      GuiInputGuiText(value: List.filled(32, '🙂').join()),
    );
    admit(result(gateway.commands.last));
    expect(await limit, NativeCommandAdmission.accepted);
    expect(controller.isModifierLatched('ctrl'), isTrue);
    expect(
      await controller.guiInput(
        GuiInputGuiText(value: List.filled(33, '🙂').join()),
      ),
      NativeCommandAdmission.rejected,
    );
    expect(
      await controller.guiInput(const GuiInputGuiText(value: '\ud800')),
      NativeCommandAdmission.rejected,
    );
    final button = controller.guiInput(
      const GuiInputGuiKey(value: GuiKeyCtrlC()),
      button: 1,
    );
    admit(result(gateway.commands.last));
    expect(await button, NativeCommandAdmission.accepted);
    expect(controller.isModifierLatched('ctrl'), isTrue);
  });

  for (final failure in [
    NativeCommandFailure.none,
    NativeCommandFailure.duplicate,
  ]) {
    test('GUI $failure 未确定受理立即隔离不重放，晚拒绝不消费', () async {
      await ready();
      controller.tapModifier('ctrl');
      final gui = controller.guiInput(
        const GuiInputGuiKey(value: GuiKeyEnter()),
      );
      final command = gateway.commands.last;
      admit(
        result(
          command,
          admission: NativeCommandAdmission.unknown,
          failure: failure,
        ),
      );
      expect(await gui, NativeCommandAdmission.unknown);
      expect(controller.guiUnknown, isTrue);
      expect(controller.guiBusy, isTrue);
      expect(controller.isModifierLatched('ctrl'), isTrue);
      expect(
        await controller.guiInput(const GuiInputGuiKey(value: GuiKeyEnter())),
        NativeCommandAdmission.rejected,
      );
      expect(gateway.commands.length, 1);
      admit(result(command, admission: NativeCommandAdmission.rejected));
      expect(controller.guiBusy, isFalse);
      expect(controller.isModifierLatched('ctrl'), isTrue);
    });
  }

  test('控制回执必须关联完整身份，关闭后的 GUI 迟到回执不复活', () async {
    await ready();
    final reconnect = controller.reconnect();
    await Future<void>.delayed(Duration.zero);
    final command = gateway.commands.last;
    admit(result(command).copyWith(attempt: u64(0), focusEpoch: u64(0)));
    admit(result(command));
    expect(await reconnect, isTrue);
    await controller.requestFocus();
    controller.tapModifier('ctrl');
    final gui = controller.guiInput(const GuiInputGuiKey(value: GuiKeyEnter()));
    final input = gateway.commands.last;
    final close = controller.close();
    admit(result(input, consumed: 1));
    expect(await gui, NativeCommandAdmission.accepted);
    expect(controller.active, isFalse);
    expect(controller.closing, isTrue);
    expect(controller.isModifierLatched('ctrl'), isFalse);
    gateway.lifecycle(NativeLifecycleState.ended, revision: 2);
    expect(await close, isTrue);
  });

  test('GUI 发送异常保留原请求为 Unknown，不自动重放', () async {
    await ready();
    gateway.onCommand = (_) => throw StateError('发送未确认');
    expect(
      await controller.guiInput(const GuiInputGuiText(value: 'x')),
      NativeCommandAdmission.unknown,
    );
    expect(controller.guiBusy, isTrue);
    expect(controller.guiUnknown, isTrue);
    expect(
      await controller.guiInput(const GuiInputGuiText(value: 'x')),
      NativeCommandAdmission.rejected,
    );
    expect(gateway.commands.length, 1);
  });

  test('Accepted 与首帧前关闭等待真实 attempt，不猜编号；晚 Prepared 不复活', () async {
    gateway.emit(prepared(gateway.prepares.single.requestId));
    await Future<void>.delayed(Duration.zero);
    controller.measure(
      const DesktopGeometry(
        cols: 103,
        rows: 37,
        pixelWidth: 824,
        pixelHeight: 740,
        dpi: 96,
      ),
      {},
    );
    final close = controller.close();
    expect(gateway.commands, isEmpty);
    gateway.lifecycle(NativeLifecycleState.connecting, attempt: 81);
    final command = gateway.commands.single;
    expect(command.attempt, u64(81));
    expect(
      (command.command as NativeCommandEvent).value,
      isA<DesktopEventDisconnect>(),
    );
    gateway.lifecycle(NativeLifecycleState.ended, attempt: 81, revision: 2);
    expect(await close, isTrue);
    gateway.emit(prepared(gateway.prepares.single.requestId));
    expect(gateway.starts.length, 1);
  });

  test('静态配色未知名称明确失败，不发 Start 冒用默认主题', () async {
    final result = prepared(gateway.prepares.single.requestId);
    gateway.emit(
      result.copyWith(
        snapshot: () => result.snapshot!.copyWith(
          profile: result.snapshot!.profile.copyWith(
            colorScheme: 'selected-dark',
          ),
        ),
      ),
    );
    await Future<void>.delayed(Duration.zero);
    expect(controller.phase, DesktopPanePhase.failed);
    expect(controller.error, isNotEmpty);
    expect(gateway.starts, isEmpty);
  });

  test(
    '静态配色冻结到原 source，多 controller 与迟到 Prepared 不互相覆盖，重连仍沿原 profile',
    () async {
      final result = prepared(gateway.prepares.single.requestId);
      DesktopPreparationResult colored(String scheme, DesktopSource origin) =>
          result.copyWith(
            source: origin,
            snapshot: () => result.snapshot!.copyWith(
              profile: result.snapshot!.profile.copyWith(colorScheme: scheme),
            ),
          );
      gateway.emit(colored('one_dark', source));
      await Future<void>.delayed(Duration.zero);
      final otherSource = source.copyWith(sessionId: 62);
      final otherGateway = DesktopSessionTestGateway();
      final other = DesktopSessionController(
        source: otherSource,
        target: controller.target,
        gateway: otherGateway,
        host: controller.host,
        font: DesktopFontController(
          cache: DesktopFontCache(register: (_, _) async {}),
        ),
      );
      other.begin();
      otherGateway.emit(colored('solarized_light', otherSource));
      await Future<void>.delayed(Duration.zero);
      expect(controller.theme, same(themeForProfile('one_dark')));
      expect(other.theme, same(themeForProfile('solarized_light')));
      gateway.emit(colored('nord', source.copyWith(generation: u64(1))));
      gateway.emit(colored('dracula', source));
      expect(controller.theme, same(themeForProfile('one_dark')));
      controller.measure(
        const DesktopGeometry(
          cols: 90,
          rows: 30,
          pixelWidth: 720,
          pixelHeight: 600,
          dpi: 96,
        ),
        {},
      );
      expect(gateway.starts.length, 1);
      gateway.lifecycle(NativeLifecycleState.authenticated);
      final reconnect = controller.reconnect();
      await Future<void>.delayed(Duration.zero);
      final command = gateway.commands.last;
      expect(
        (command.command as NativeCommandEvent).value,
        isA<DesktopEventReconnect>(),
      );
      gateway.commandBus.add(
        RustSignalPack(
          DesktopNativeCommandResult(
            version: 1,
            source: source,
            identity: 'frozen',
            attempt: command.attempt,
            focusEpoch: command.focusEpoch,
            admission: NativeCommandAdmission.accepted,
            consumedLatches: 0,
            requestId: command.requestId,
            effect: u64(3),
            failure: NativeCommandFailure.none,
          ),
          Uint8List(0),
        ),
      );
      expect(await reconnect, isTrue);
      gateway.lifecycle(
        NativeLifecycleState.connecting,
        attempt: 13,
        revision: 2,
      );
      expect(controller.theme, same(themeForProfile('one_dark')));
      expect(other.theme, same(themeForProfile('solarized_light')));
      other.dispose();
      await otherGateway.dispose();
    },
  );

  test('Search 在原 claim 未结算时先 quiet，实际 GUI 完成后 ACK，再释放 root', () async {
    await ready();
    final gui = Completer<void>();
    controller.executeAction = (_) async {
      expect(await controller.quiet(), isTrue);
      await gui.future;
      return DesktopActionOutcome.executed;
    };
    gateway.action(DesktopAction.search);
    await Future<void>.delayed(Duration.zero);
    expect(hostCalls.where((call) => call.method == 'retirePane').length, 1);
    expect(hostCalls.where((call) => call.method == 'completeAction'), isEmpty);
    gui.complete();
    await Future<void>.delayed(Duration.zero);
    final completion = hostCalls.indexWhere(
      (call) => call.method == 'completeAction',
    );
    final release = hostCalls.lastIndexWhere(
      (call) => call.method == 'retirePane',
    );
    expect(completion, greaterThan(0));
    expect(release, greaterThan(completion));
  });

  test('Copy 真正写入完整剪贴板后才 ACK', () async {
    await ready();
    claimAction = 'copy';
    final written = Completer<Object?>();
    clipboard = (call) {
      expect(call.method, 'Clipboard.setData');
      expect((call.arguments as Map)['text'], '完整🙂\n内容');
      return written.future;
    };
    gateway.action(DesktopAction.copy);
    await Future<void>.delayed(Duration.zero);
    expect(hostCalls.where((call) => call.method == 'completeAction'), isEmpty);
    written.complete(null);
    await Future<void>.delayed(Duration.zero);
    expect(
      (hostCalls.firstWhere((call) => call.method == 'completeAction').arguments
          as Map)['kind'],
      'completed',
    );
  });

  test('原 Paste 在剪贴板读取期间撤权，迟到正文只 abort，不投向新焦点', () async {
    await ready();
    claimAction = 'paste';
    final read = Completer<Object?>();
    clipboard = (_) => read.future;
    gateway.action(DesktopAction.paste);
    await Future<void>.delayed(Duration.zero);
    expect(await controller.quiet(), isTrue);
    read.complete({'text': '迟到原文🙂'});
    await Future<void>.delayed(Duration.zero);
    final completion =
        hostCalls
                .firstWhere((call) => call.method == 'completeAction')
                .arguments
            as Map;
    expect(completion['kind'], 'abort');
    expect(completion.containsKey('text'), isFalse);
  });

  test(
    'TextEndRequired 即时路由原 pane/context/fence，不等 ActionReady 或签静默',
    () async {
      await ready();
      final end = DesktopNativeTextEndRequired(
        version: 1,
        source: source,
        identity: 'frozen',
        attempt: u64(12),
        focusEpoch: u64(4),
        contextId: u64(8),
        transactionId: u64(19),
        endFence: u64(23),
      );
      gateway.textEndBus.add(RustSignalPack(end, Uint8List(0)));
      await Future<void>.delayed(Duration.zero);
      final call = hostCalls.singleWhere(
        (call) => call.method == 'textEndRequired',
      );
      expect(call.arguments, containsPair('paneToken', 'origin-pane'));
      expect(call.arguments, containsPair('contextId', '8'));
      expect(call.arguments, containsPair('transactionId', '19'));
      expect(call.arguments, containsPair('endFence', '23'));
      expect(hostCalls.where((call) => call.method == 'claimAction'), isEmpty);
      expect(controller.editableSafe, isFalse);
      gateway.textEndBus.add(
        RustSignalPack(end.copyWith(focusEpoch: u64(1)), Uint8List(0)),
      );
      await Future<void>.delayed(Duration.zero);
      expect(
        hostCalls.where((call) => call.method == 'textEndRequired').length,
        1,
      );
      expect(controller.error, isEmpty);
    },
  );

  test('ContextMenuReady 只向原活跃 source/attempt/binding 打开一次菜单', () async {
    await ready();
    final shown = <DesktopNativeContextMenuReady>[];
    controller.showContextMenu = (request) async {
      shown.add(request);
    };
    final menu = DesktopNativeContextMenuReady(
      version: 1,
      source: source,
      identity: 'frozen',
      attempt: u64(12),
      focusEpoch: u64(4),
      effect: u64(31),
      stableRow: 0,
      column: 3,
    );
    gateway.menuBus.add(
      RustSignalPack(menu.copyWith(attempt: u64(3)), Uint8List(0)),
    );
    gateway.menuBus.add(
      RustSignalPack(
        menu.copyWith(source: source.copyWith(generation: u64(8))),
        Uint8List(0),
      ),
    );
    gateway.menuBus.add(RustSignalPack(menu, Uint8List(0)));
    await Future<void>.delayed(Duration.zero);
    gateway.menuBus.add(RustSignalPack(menu, Uint8List(0)));
    await Future<void>.delayed(Duration.zero);
    expect(shown, [menu]);
    expect(hostCalls.where((call) => call.method == 'claimAction'), isEmpty);
    await controller.quiet();
    gateway.menuBus.add(
      RustSignalPack(menu.copyWith(effect: u64(32)), Uint8List(0)),
    );
    expect(shown, [menu]);
  });

  test('数据待决额度不计 FrameAck/Disconnect，两控制关联独立且有界', () async {
    await ready();
    for (int i = 0; i < 64; i++) {
      controller.event(
        DesktopEventViewport(followBottom: false, topStableRow: i, rows: 37),
        requiresFocus: false,
      );
    }
    expect(gateway.commands.length, 64);
    controller.event(
      const DesktopEventViewport(followBottom: true, topStableRow: 0, rows: 0),
      requiresFocus: false,
    );
    expect(gateway.commands.length, 64);
    controller.event(
      const DesktopEventFrameAck(frameSeq: 1),
      requiresFocus: false,
    );
    controller.event(
      const DesktopEventFrameAck(frameSeq: 2),
      requiresFocus: false,
    );
    controller.event(const DesktopEventDisconnect(), requiresFocus: false);
    expect(gateway.commands.length, 67);
    final first = gateway.commands.first;
    gateway.commandBus.add(
      RustSignalPack(
        DesktopNativeCommandResult(
          version: 1,
          source: source,
          identity: 'frozen',
          attempt: first.attempt,
          focusEpoch: first.focusEpoch,
          admission: NativeCommandAdmission.accepted,
          consumedLatches: 0,
          requestId: first.requestId,
          effect: u64(1),
          failure: NativeCommandFailure.none,
        ),
        Uint8List(0),
      ),
    );
    controller.event(
      const DesktopEventViewport(followBottom: true, topStableRow: 0, rows: 0),
      requiresFocus: false,
    );
    expect(gateway.commands.length, 68);
    expect(
      gateway.commands.map((request) => request.requestId).toSet().length,
      68,
    );
  });
}
