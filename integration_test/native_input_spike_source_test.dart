import 'dart:io';

import 'package:flutter/widgets.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/desktop/native_input_spike_sdk/bootstrap.dart';
import 'package:guosh_shell/src/desktop/native_input_spike_sdk/sdk.dart';
import 'package:guosh_shell/src/desktop/native_input_spike_source/adapter.dart';
import 'package:guosh_shell/src/desktop/native_input_spike_source/fixture_app.dart';

import '../test/native_input_spike_source_progress.dart';

/// 只接受真实 Windows loop 与同 SDK DLL，不以直接调用替代 PostMessage。
void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  testWidgets('实际 SDK 同 module source、固定消息循环探针及非 UI join', (tester) async {
    expect(Platform.isWindows, isTrue);
    expect(const bool.fromEnvironment('GUOSSH_NATIVE_INPUT_SPIKE_SDK'), isTrue);
    expect(
      const bool.fromEnvironment('GUOSSH_NATIVE_INPUT_SPIKE_SOURCE'),
      isTrue,
    );
    final bootstrap = SdkBootstrap(
      native: const MethodChannelFixtureAdapter(),
      sdk: RinfFixtureSdk(),
      normalHandlers: assignRustSignal,
      controlledPath: const String.fromEnvironment(
        'GUOSSH_NATIVE_INPUT_SPIKE_DLL',
      ),
      explicitOptIn: true,
    );
    const source = MethodChannelSourceAdapter();
    final report = <String, Object>{
      'syntheticOnly': true,
      'physical': false,
      'originUnknown': true,
      'unknownScalarCovered': false,
      'noUnknownDataVerified': false,
      'guiActionExecuted': false,
      'readerTwoSlotBackpressureVerified': false,
      'integrationNativePluginCoverage': false,
      'textImePastePointerSupported': false,
    };
    final rows = <Map<String, Object>>[];
    report['observations'] = rows;
    SourceNumbers? opened;
    SourceNumbers? closed;
    var sdkProven = false;
    try {
      await tester.pumpWidget(
        SourceFixtureApp(
          bootstrap: bootstrap,
          source: source,
          explicitOptIn: true,
          onClosed: () {},
        ),
      );
      await tester.pumpAndSettle();
      await tester.runAsync(() async {
        await bootstrap.run();
        sdkProven = bootstrap.phase == BootstrapPhase.proven;
        report['sdkActualReply'] = sdkProven;
        if (!sdkProven) return;
        final diagnostic = bootstrap.diagnostic!;
        report.addAll({
          'sameModule': diagnostic.sameModule,
          'fixtureOnly': diagnostic.fixtureOnly,
          'sdkOnlyOwners': diagnostic.owners,
          'coreAbi': diagnostic.coreAbi,
          'coreExports': diagnostic.coreExports,
        });
        opened = await source.open(explicitOptIn: true);
        report['open'] = opened!.values;
        report['sourceActualSameSdk'] = opened!.flag('sourceActualSameSdk');
        if (opened!.number('openHr') != 0) return;

        // 普通 source 夹具未给 exit-test flag，不允许消费线程 WM_QUIT。
        await expectLater(
          source.testDirectExit(),
          throwsA(isA<PlatformException>()),
        );

        // 不抢前台；焦点失败时仍测真实 peer 协议与合成消息循环拒绝路线。
        final focused = opened!.number('focusHr') == 0;
        report['ownedFocus'] = focused;
        if (focused) {
          for (final (probe, phase) in [
            (SourceProbe.f4Down, 0),
            (SourceProbe.f4Up, 2),
            (SourceProbe.f5Down, 0),
            (SourceProbe.f5Up, 2),
            (SourceProbe.f2Down, 0),
            (SourceProbe.f2Repeat, 1),
            (SourceProbe.f2Up, 2),
            (SourceProbe.unknownDown, 0),
            (SourceProbe.unknownRepeat, 1),
            (SourceProbe.unknownUp, 2),
          ]) {
            final posted = await source.probe(probe);
            // 固定单次调度间隔及单次 inspect/read，不重投、不轮询至 green。
            await Future<void>.delayed(const Duration(milliseconds: 100));
            final numbers = await source.inspect();
            final bytes = await source.read();
            rows.add({
              'probe': probe.name,
              'postStatus': posted,
              'numbers': numbers.values,
              'peerStatus': bytes.status,
              'peerBytes': bytes.bytes.toList(),
            });
            expectSourceProbeProgress(
              baseline: opened!,
              current: numbers,
              ordinal: rows.length,
              phase: phase,
              postStatus: posted,
            );
          }
          report['unknownScalarCovered'] = true;
        }
        final modeRows = <Map<String, Object>>[];
        report['modes'] = modeRows;
        for (final mode in [
          SourceMode.mode1,
          SourceMode.mode2,
          SourceMode.mode3,
        ]) {
          final commandStatus = await source.mode(mode);
          await Future<void>.delayed(const Duration(milliseconds: 100));
          final queryStatus = await source.mode(SourceMode.query);
          await Future<void>.delayed(const Duration(milliseconds: 100));
          final numbers = await source.inspect();
          final bytes = await source.read();
          modeRows.add({
            'mode': mode.index,
            'commandStatus': commandStatus,
            'queryStatus': queryStatus,
            'numbers': numbers.values,
            'peerStatus': bytes.status,
            'peerBytes': bytes.bytes.toList(),
          });
        }
        if (!focused) {
          final posted = await source.probe(SourceProbe.f4Down);
          await Future<void>.delayed(const Duration(milliseconds: 100));
          rows.add({
            'probe': SourceProbe.f4Down.name,
            'postStatus': posted,
            'numbers': (await source.inspect()).values,
          });
        }
      });
    } finally {
      await tester.runAsync(() async {
        if (sdkProven) {
          closed = await closeSourceFixture(source);
          report['close'] = closed!.values;
          report['joinedAndEmpty'] = closed!.joinedAndEmpty;
        }
        // 正常收尾始终先停止／确认 source，再关闭 SDK；未确认则原生保留。
        if (closed?.joinedAndEmpty ?? !sdkProven) bootstrap.close();
      });
      report['sdkClosed'] = bootstrap.phase == BootstrapPhase.closed;
      binding.reportData = report;
      await tester.pumpWidget(const SizedBox.shrink());
      if (bootstrap.phase == BootstrapPhase.closed) bootstrap.dispose();
    }
    expect(sdkProven, isTrue);
    expect(report['sameModule'], isTrue);
    expect(report['fixtureOnly'], isTrue);
    expect(report['sdkOnlyOwners'], 0);
    expect(opened!.number('openHr'), 0);
    expect(opened!.flag('sourceActualSameSdk'), isTrue);
    expect(closed!.joinedAndEmpty, isTrue);
    expect(report['sdkClosed'], isTrue);
    final modeRows = report['modes'] as List<Map<String, Object>>;
    for (final row in modeRows) {
      expect(row['commandStatus'], 0);
      expect(row['queryStatus'], 0);
      expect(row['peerStatus'], 0);
      expect(row['peerBytes'], [27, 91, 63, 48 + (row['mode'] as int), 117]);
    }
    if (report['ownedFocus'] == true) {
      expect(report['unknownScalarCovered'], isTrue);
      expect(rows, hasLength(10));
      expect(rows[0]['peerBytes'], [27, 91, 51, 126]);
      expect(rows[2]['peerBytes'], [127]);
      expect(rows[0]['peerStatus'], 0);
      expect(rows[2]['peerStatus'], 0);
      final f2Down = SourceNumbers.fromMap(rows[4]['numbers']);
      expect(f2Down.number('lastAction'), 1);
      for (final row in rows.skip(5)) {
        final numbers = SourceNumbers.fromMap(row['numbers']);
        expect(numbers.number('lastAction'), 0);
        expect(numbers.number('lastByteLength'), 0);
        expect(row['peerBytes'], isEmpty);
      }
      for (final row in rows.skip(7)) {
        final numbers = SourceNumbers.fromMap(row['numbers']);
        expect(numbers.number('lastRawSource'), 0);
        expect(numbers.number('tsfDown'), 0);
        expect(numbers.number('tsfTestDown'), 0);
        // Busy 只表示本次未读到数据，不表示成功收到空 packet。
        expect(row['peerStatus'], 2);
      }
      final unknownUp = SourceNumbers.fromMap(rows[9]['numbers']);
      expect(unknownUp.number('lastSeq'), opened!.number('nextSeq') + 9);
      expect(unknownUp.number('lastWrittenSeq'), unknownUp.number('lastSeq'));
      for (final row in modeRows) {
        final numbers = SourceNumbers.fromMap(row['numbers']);
        expect(
          numbers.number('lastWrittenSeq'),
          unknownUp.number('lastSeq') + 2 * (row['mode'] as int),
        );
      }
      expect(closed!.number('lastWrittenSeq'), unknownUp.number('lastSeq') + 6);
      // 完整 named bytes、Unknown Up 写完成、随后全序 mode 帧及最终零 charge 共同构成证据。
      report['noUnknownDataVerified'] = true;
    }
    final finalNumbers = SourceNumbers.fromMap(rows.last['numbers']);
    expect(finalNumbers.number('loopHandled'), greaterThan(0));
    expect(finalNumbers.number('missingPretranslate'), 0);
    expect(finalNumbers.number('tsfDown'), 0);
    expect(finalNumbers.number('tsfTestDown'), 0);
  });
}
