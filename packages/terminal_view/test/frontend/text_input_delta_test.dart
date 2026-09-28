import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:terminal_view/src/ui/text_input_delta.dart';

/// The deleteDetection initial state used by CustomTextEdit.
const _init = '  ';

TextEditingDeltaInsertion insert(
  String oldText,
  String inserted, {
  int? at,
  TextRange composing = TextRange.empty,
}) {
  final offset = at ?? oldText.length;
  return TextEditingDeltaInsertion(
    oldText: oldText,
    textInserted: inserted,
    insertionOffset: offset,
    selection: TextSelection.collapsed(offset: offset + inserted.length),
    composing: composing,
  );
}

void main() {
  late TextInputDeltaInterpreter interpreter;

  setUp(() => interpreter = TextInputDeltaInterpreter());

  TerminalTextEdit edit(TextEditingDelta delta) => interpreter.applyDelta(delta).edit;

  test('typing from the initial state sends exactly the typed text', () {
    expect(edit(insert(_init, 'a')), const TerminalTextEdit(text: 'a'));
  });

  test('a reset that lands late does not resend earlier text', () {
    // "a" was sent and a reset to "  " requested, but the platform still had
    // "  a" when the next key arrived.
    expect(edit(insert(_init, 'a')), const TerminalTextEdit(text: 'a'));
    interpreter.reset();
    expect(edit(insert('  a', 'b')), const TerminalTextEdit(text: 'b'));
    expect(edit(insert('  ab', 'c')), const TerminalTextEdit(text: 'c'));
  });

  test('backspace on the initial state erases one character', () {
    final delta = TextEditingDeltaDeletion(
      oldText: _init,
      deletedRange: const TextRange(start: 1, end: 2),
      selection: const TextSelection.collapsed(offset: 1),
      composing: TextRange.empty,
    );
    expect(edit(delta), const TerminalTextEdit(backspaces: 1));
  });

  group('IME composition', () {
    test('preedit is never sent; the commit sends the committed text', () {
      final n = interpreter.applyDelta(
        insert(_init, 'n', composing: const TextRange(start: 2, end: 3)),
      );
      expect(n.edit.isEmpty, isTrue);
      expect(n.composingText, 'n');

      final ni = interpreter.applyDelta(
        insert('  n', 'i', composing: const TextRange(start: 2, end: 4)),
      );
      expect(ni.edit.isEmpty, isTrue);
      expect(ni.composingText, 'ni');

      final commit = interpreter.applyDelta(
        const TextEditingDeltaReplacement(
          oldText: '  ni',
          replacementText: '你',
          replacedRange: TextRange(start: 2, end: 4),
          selection: TextSelection.collapsed(offset: 3),
          composing: TextRange.empty,
        ),
      );
      expect(commit.edit, const TerminalTextEdit(text: '你'));
      expect(commit.composingText, isNull);
    });

    test('committing the raw letters sends them once', () {
      interpreter.applyDelta(
        insert(_init, 'ni', composing: const TextRange(start: 2, end: 4)),
      );
      final commit = interpreter.applyDelta(
        const TextEditingDeltaNonTextUpdate(
          oldText: '  ni',
          selection: TextSelection.collapsed(offset: 4),
          composing: TextRange.empty,
        ),
      );
      expect(commit.edit, const TerminalTextEdit(text: 'ni'));
    });

    test('a partial commit sends the committed part and keeps the rest composing', () {
      interpreter.applyDelta(
        insert(_init, 'nihao', composing: const TextRange(start: 2, end: 7)),
      );
      final partial = interpreter.applyDelta(
        const TextEditingDeltaReplacement(
          oldText: '  nihao',
          replacementText: '你',
          replacedRange: TextRange(start: 2, end: 4),
          selection: TextSelection.collapsed(offset: 6),
          composing: TextRange(start: 3, end: 6),
        ),
      );
      expect(partial.edit, const TerminalTextEdit(text: '你'));
      expect(partial.composingText, 'hao');

      final rest = interpreter.applyDelta(
        const TextEditingDeltaReplacement(
          oldText: '  你hao',
          replacementText: '好',
          replacedRange: TextRange(start: 3, end: 6),
          selection: TextSelection.collapsed(offset: 4),
          composing: TextRange.empty,
        ),
      );
      expect(rest.edit, const TerminalTextEdit(text: '好'));
    });

    test('deleting inside the preedit sends nothing', () {
      interpreter.applyDelta(
        insert(_init, 'ni', composing: const TextRange(start: 2, end: 4)),
      );
      final delta = TextEditingDeltaDeletion(
        oldText: '  ni',
        deletedRange: const TextRange(start: 3, end: 4),
        selection: const TextSelection.collapsed(offset: 3),
        composing: const TextRange(start: 2, end: 3),
      );
      expect(edit(delta).isEmpty, isTrue);
    });
  });

  group("iOS '.' shortcut", () {
    // Captured from the iOS 27 simulator: "a" then two spaces, typed faster
    // than the field reset lands.
    test('the separate replacement form types exactly the spaces that were pressed', () {
      final edits = [
        insert(_init, 'a'),
        insert('  a', ' '),
        const TextEditingDeltaReplacement(
          oldText: '  a ',
          replacementText: '.',
          replacedRange: TextRange(start: 3, end: 4),
          selection: TextSelection.collapsed(offset: 4),
          composing: TextRange.empty,
        ),
        insert('  a.', ' '),
      ].map(interpreter.applyDelta).toList();

      expect(edits.map((u) => u.edit.text).join(), 'a  ');
      expect(edits.every((u) => u.edit.backspaces == 0), isTrue);
      // The replacement is not an answer to a key press.
      expect(edits.map((u) => u.answersKey), [true, true, false, true]);
    });

    test('the one-step form types a single space', () {
      final update = interpreter.applyDelta(
        const TextEditingDeltaReplacement(
          oldText: '  a ',
          replacementText: '. ',
          replacedRange: TextRange(start: 3, end: 4),
          selection: TextSelection.collapsed(offset: 5),
          composing: TextRange.empty,
        ),
      );
      expect(update.edit, const TerminalTextEdit(text: ' '));
      expect(update.answersKey, isTrue);
    });
  });

  test('replacing already-sent text erases it and types the replacement', () {
    final delta = TextEditingDeltaReplacement(
      oldText: '  ab',
      replacementText: 'AB',
      replacedRange: const TextRange(start: 2, end: 4),
      selection: const TextSelection.collapsed(offset: 4),
      composing: TextRange.empty,
    );
    expect(edit(delta), const TerminalTextEdit(backspaces: 2, text: 'AB'));
  });

  test('surrogate pairs count as one character', () {
    expect(edit(insert(_init, '😀')), const TerminalTextEdit(text: '😀'));
    final delta = TextEditingDeltaDeletion(
      oldText: '  😀',
      deletedRange: const TextRange(start: 2, end: 4),
      selection: const TextSelection.collapsed(offset: 2),
      composing: TextRange.empty,
    );
    expect(edit(delta), const TerminalTextEdit(backspaces: 1));
  });

  test('whole-value updates are diffed against the previous value', () {
    final update = interpreter.applyValue(
      const TextEditingValue(text: _init, selection: TextSelection.collapsed(offset: 2)),
      const TextEditingValue(text: '  xy', selection: TextSelection.collapsed(offset: 4)),
    );
    expect(update.edit, const TerminalTextEdit(text: 'xy'));
  });
}
