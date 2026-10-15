import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/desktop/native_input/desktop_composition.dart';

void main() {
  for (final width in [375.0, 768.0, 1280.0]) {
    testWidgets('composition 在 $width 宽使用实际 TextPainter 绘制与 UTF-16 glyph 布局', (
      tester,
    ) async {
      tester.view.physicalSize = Size(width, 720);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      List<Map<String, Object?>>? measured;
      List<Map<String, Object?>>? caretLayout;
      Size? size;
      await tester.pumpWidget(
        Directionality(
          textDirection: TextDirection.ltr,
          child: Align(
            alignment: Alignment.topLeft,
            child: SizedBox(
              width: width,
              child: DesktopCompositionSurface(
                text: '中🙂Wi\ná',
                style: const TextStyle(fontSize: 18),
                textScaler: TextScaler.noScaling,
                onPainted: (value, glyphs, carets) {
                  size = value;
                  measured = glyphs;
                  caretLayout = carets;
                },
              ),
            ),
          ),
        ),
      );
      await tester.pump();
      expect(tester.takeException(), isNull);
      expect(size!.width, lessThanOrEqualTo(width));
      expect(size!.height, greaterThan(18));
      expect(measured, isNotEmpty);
      expect(caretLayout!.first['offset'], 0);
      expect(caretLayout!.last['offset'], '中🙂Wi\ná'.length);
      expect(
        caretLayout!.last['y'] as double,
        greaterThan(caretLayout!.first['y'] as double),
      );
      expect(
        measured!.any((rect) => rect['start'] == 1 && rect['end'] == 3),
        isTrue,
      );
      for (final rect in measured!) {
        for (final key in ['left', 'top', 'right', 'bottom']) {
          expect((rect[key] as double).isFinite, isTrue);
        }
      }
      await tester.pumpWidget(const SizedBox.shrink());
      expect(tester.takeException(), isNull);
    });
  }
}
