import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/terminal/key_bar_layout.dart';

void main() {
  testWidgets('超时写入隔离到迟到回执结算，下一次保存只能由自身结果完成', (tester) async {
    final replies = StreamController<KeyBarLayoutResult>.broadcast(sync: true);
    final sent = <List<List<String>>>[];
    final writer = KeyBarLayoutWriter(results: replies.stream, send: sent.add);
    final first = writer.save([
      ['escape'],
    ]);
    final timedOut = expectLater(first, throwsA(isA<TimeoutException>()));
    await tester.pump(const Duration(seconds: 10));
    await timedOut;
    await expectLater(
      writer.save([
        ['tab'],
      ]),
      throwsStateError,
    );
    expect(sent, hasLength(1));
    replies.add(const KeyBarLayoutResult(ok: true, detail: ''));
    await tester.pump();
    var done = false;
    final next = writer
        .save([
          ['tab'],
        ])
        .then((_) => done = true);
    final rejected = expectLater(next, throwsStateError);
    await tester.pump();
    expect(done, isFalse);
    replies.add(const KeyBarLayoutResult(ok: false, detail: 'failed'));
    await rejected;
    expect(done, isFalse);
    await replies.close();
  });
}
