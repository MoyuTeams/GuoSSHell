import 'package:flutter/widgets.dart';

/// 同一 TextPainter 同时绘制 composition 与测量 UTF-16 ACP，避免格宽估算。
class DesktopCompositionSurface extends StatefulWidget {
  const DesktopCompositionSurface({
    super.key,
    required this.text,
    required this.style,
    required this.textScaler,
    required this.onPainted,
  });
  final String text;
  final TextStyle style;
  final TextScaler textScaler;
  final void Function(
    Size size,
    List<Map<String, Object?>> glyphs,
    List<Map<String, Object?>> carets,
  )
  onPainted;

  @override
  State<DesktopCompositionSurface> createState() => _DesktopCompositionState();
}

class _DesktopCompositionState extends State<DesktopCompositionSurface> {
  TextPainter? _text;
  @override
  void dispose() {
    _text?.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => LayoutBuilder(
    builder: (context, constraints) {
      _text?.dispose();
      final painter = _text = TextPainter(
        text: TextSpan(text: widget.text, style: widget.style),
        textDirection: TextDirection.ltr,
        textScaler: widget.textScaler,
      )..layout(maxWidth: constraints.maxWidth);
      final glyphs = <Map<String, Object?>>[];
      final carets = <Map<String, Object?>>[];
      void caret(int offset) {
        final position = TextPosition(offset: offset);
        final prototype = Rect.fromLTWH(0, 0, 0, painter.preferredLineHeight);
        final point = painter.getOffsetForCaret(position, prototype);
        carets.add({
          'offset': offset,
          'x': point.dx,
          'y': point.dy,
          'height': painter.getFullHeightForCaret(position, prototype),
        });
      }

      int offset = 0;
      caret(offset);
      for (final rune in widget.text.runes) {
        final length = rune > 0xffff ? 2 : 1;
        final boxes = painter.getBoxesForSelection(
          TextSelection(baseOffset: offset, extentOffset: offset + length),
        );
        for (final box in boxes) {
          glyphs.add({
            'start': offset,
            'end': offset + length,
            'left': box.left,
            'top': box.top,
            'right': box.right,
            'bottom': box.bottom,
          });
        }
        offset += length;
        caret(offset);
      }
      return IgnorePointer(
        child: CustomPaint(
          size: painter.size,
          painter: _CompositionPainter(
            painter,
            glyphs,
            carets,
            widget.onPainted,
          ),
        ),
      );
    },
  );
}

class _CompositionPainter extends CustomPainter {
  _CompositionPainter(this.text, this.glyphs, this.carets, this.painted);
  final TextPainter text;
  final List<Map<String, Object?>> glyphs;
  final List<Map<String, Object?>> carets;
  final void Function(
    Size size,
    List<Map<String, Object?>> glyphs,
    List<Map<String, Object?>> carets,
  )
  painted;
  bool _reported = false;
  @override
  void paint(Canvas canvas, Size size) {
    text.paint(canvas, Offset.zero);
    if (!_reported) {
      _reported = true;
      WidgetsBinding.instance.addPostFrameCallback(
        (_) => painted(size, glyphs, carets),
      );
    }
  }

  @override
  bool shouldRepaint(_CompositionPainter oldDelegate) =>
      oldDelegate.text != text;
}
