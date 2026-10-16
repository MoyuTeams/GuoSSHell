import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/settings/settings_page.dart';

void main() {
  testWidgets('未收到旧回包前连续修改不同设置，仅发送各自改动', (tester) async {
    const initial = SettingsState(
      fontSize: 14,
      minFontSize: 8,
      maxFontSize: 32,
      scrollbackLines: 5000,
      maxScrollbackLines: 20000,
      showKeyBar: true,
      keyBarRows: [[], []],
    );
    assignRustSignal['SettingsState']!(
      initial.bincodeSerialize(),
      Uint8List(0),
    );
    final requests = <SaveSettings>[];
    await tester.pumpWidget(
      MaterialApp(
        home: SettingsPage(saveSettings: requests.add, querySettings: () {}),
      ),
    );
    final pageScroll = find
        .descendant(
          of: find.byType(ListView),
          matching: find.byType(Scrollable),
        )
        .first;
    await tester.scrollUntilVisible(
      find.byType(Slider),
      160,
      scrollable: pageScroll,
    );
    tester.widget<Slider>(find.byType(Slider)).onChangeEnd!(22);
    await tester.pump();
    await tester.scrollUntilVisible(
      find.byType(SwitchListTile),
      160,
      scrollable: pageScroll,
    );
    await tester.tap(find.byType(SwitchListTile));
    await tester.pump();
    expect(requests.length, 2);
    expect(requests[0].fontSize, 22);
    expect(requests[1].showKeyBar, isFalse);
    expect(requests[1].fontSize, isNull);
    expect(
      requests.every((request) => request.scrollbackLines == null),
      isTrue,
    );
    await tester.pumpWidget(const SizedBox());
    SettingsState.latestRustSignal = null;
  });
}
