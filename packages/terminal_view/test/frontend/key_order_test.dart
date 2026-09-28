import 'package:flutter_test/flutter_test.dart';
import 'package:terminal_view/src/ui/key_order.dart';

void main() {
  testWidgets('keys run immediately when no typed text is in flight',
      (tester) async {
    final order = HardwareKeyOrder();
    final log = <String>[];
    order.run(() => log.add('enter'));
    expect(log, ['enter']);
    order.dispose();
  });

  testWidgets('a key waits for the text typed before it', (tester) async {
    final order = HardwareKeyOrder();
    final log = <String>[];

    order.textKeyDispatched(); // "l" left to the text input
    order.textKeyDispatched(); // "s"
    order.run(
        () => log.add('enter')); // Return pressed before the text came back
    expect(log, isEmpty);

    log.add('l');
    order.textInputUpdated();
    expect(log, ['l']);

    log.add('s');
    order.textInputUpdated();
    expect(log, ['l', 's', 'enter']);
    expect(order.isBusy, isFalse);
    order.dispose();
  });

  testWidgets('a key does not wait for text typed after it', (tester) async {
    final order = HardwareKeyOrder();
    final log = <String>[];

    order.textKeyDispatched(); // "a"
    order.run(() => log.add('enter'));
    order.textKeyDispatched(); // "b", typed after Return
    expect(log, isEmpty);

    log.add('a');
    order.textInputUpdated();
    expect(log, ['a', 'enter']);

    log.add('b');
    order.textInputUpdated();
    expect(log, ['a', 'enter', 'b']);
    expect(order.isBusy, isFalse);
    order.dispose();
  });

  testWidgets('held keys keep their own order', (tester) async {
    final order = HardwareKeyOrder();
    final log = <String>[];

    order.textKeyDispatched();
    order.run(() => log.add('left'));
    order.run(() => log.add('right'));
    order.textInputUpdated();
    expect(log, ['left', 'right']);
    order.dispose();
  });

  testWidgets(
      'held keys are released after the timeout if the text never comes',
      (tester) async {
    final order = HardwareKeyOrder(timeout: const Duration(milliseconds: 150));
    final log = <String>[];

    order.textKeyDispatched(); // e.g. a dead key the platform never answers
    order.run(() => log.add('arrow_up'));
    await tester.pump(const Duration(milliseconds: 100));
    expect(log, isEmpty);
    await tester.pump(const Duration(milliseconds: 100));
    expect(log, ['arrow_up']);
    expect(order.isBusy, isFalse);
    order.dispose();
  });

  testWidgets(
      'a burst of typing that takes longer than the timeout keeps its order',
      (tester) async {
    final order = HardwareKeyOrder(timeout: const Duration(milliseconds: 150));
    final log = <String>[];

    for (var i = 0; i < 12; i++) {
      order
          .textKeyDispatched(); // twelve keys typed at once, e.g. by automation
    }
    order.run(() => log.add('enter'));
    // The platform answers one key every 40 ms: 480 ms in total.
    for (var i = 0; i < 12; i++) {
      await tester.pump(const Duration(milliseconds: 40));
      expect(log, isNot(contains('enter')),
          reason: 'key ${i + 1} not answered yet');
      log.add('t$i');
      order.textInputUpdated();
    }
    expect(log.last, 'enter');
    expect(log.length, 13);
    order.dispose();
  });
}
