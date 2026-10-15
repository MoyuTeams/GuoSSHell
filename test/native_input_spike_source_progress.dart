import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/desktop/native_input_spike_source/adapter.dart';

/// 每个已投递 MSG 必须对应新事件、观察、Gate／handled 和实际 phase。
void expectSourceProbeProgress({
  required SourceNumbers baseline,
  required SourceNumbers current,
  required int ordinal,
  required int phase,
  required int postStatus,
}) {
  expect(postStatus, 0);
  for (final field in ['lastEvent', 'observations', 'gate', 'loopHandled']) {
    expect(
      current.number(field),
      baseline.number(field) + ordinal,
      reason: field,
    );
  }
  expect(current.number('lastPhase'), phase);
}
