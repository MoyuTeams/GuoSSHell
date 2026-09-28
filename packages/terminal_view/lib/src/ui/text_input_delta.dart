import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

/// The effect of one editing step on the terminal: erase [backspaces]
/// characters before the cursor, then type [text].
@immutable
class TerminalTextEdit {
  const TerminalTextEdit({this.backspaces = 0, this.text = ''});

  final int backspaces;

  final String text;

  bool get isEmpty => backspaces == 0 && text.isEmpty;

  @override
  bool operator ==(Object other) =>
      other is TerminalTextEdit &&
      other.backspaces == backspaces &&
      other.text == text;

  @override
  int get hashCode => Object.hash(backspaces, text);

  @override
  String toString() => 'TerminalTextEdit(backspaces: $backspaces, text: $text)';
}

/// Result of interpreting one platform editing update.
@immutable
class TextInputUpdate {
  const TextInputUpdate({
    required this.edit,
    required this.value,
    required this.composingText,
    this.answersKey = true,
  });

  /// What to send to the terminal.
  final TerminalTextEdit edit;

  /// The platform's editing value after the update.
  final TextEditingValue value;

  /// Text still being composed by the IME (preedit), or null. Never sent.
  final String? composingText;

  /// Whether this update is the platform's answer to a key press. False for
  /// the extra update of iOS's "." shortcut (see [TextInputDeltaInterpreter]).
  final bool answersKey;
}

/// Turns platform text editing updates into terminal edits.
///
/// The hidden text field behind the terminal's IME connection is reset after
/// every committed edit, but the platform may deliver the next update before
/// that reset lands. Deltas carry the platform's own `oldText`, so edits are
/// computed against what the platform actually had rather than against what
/// the framework last asked for — a late reset can no longer make text that
/// was already sent be sent again.
///
/// Only committed text reaches the terminal: everything before the composing
/// region, or the whole text when nothing is composing. When the committed
/// part changes, the edit erases the changed tail and types its replacement,
/// which covers typing, deletion, IME commits (including partial ones) and
/// autocorrect-style replacements alike.
class TextInputDeltaInterpreter {
  TextRange _composing = TextRange.empty;

  /// The platform's editing value was replaced by one without composing.
  void reset() => _composing = TextRange.empty;

  /// Interprets one delta from [DeltaTextInputClient.updateEditingValueWithDeltas].
  TextInputUpdate applyDelta(TextEditingDelta delta) {
    final previous = TextEditingValue(
      text: delta.oldText,
      composing: _activeComposing(_composing, delta.oldText),
    );
    final next = delta.apply(previous);

    // iOS's "." shortcut: a space typed right after "word + space" turns that
    // space into a period — either as its own replacement (" " → ".", with the
    // new space inserted by the next delta) or in one step (" " → ". "). The
    // first space already reached the terminal and the user typed a space,
    // not a period, so the edit is a plain space (or nothing).
    final shortcut = _periodShortcut(delta);
    if (shortcut != null) {
      _composing = TextRange.empty;
      return TextInputUpdate(
        edit: shortcut,
        value: next,
        composingText: null,
        answersKey: !shortcut.isEmpty,
      );
    }

    return _update(previous, next);
  }

  /// Interprets a whole-value update ([TextInputClient.updateEditingValue])
  /// relative to [previous], the last value the platform reported.
  TextInputUpdate applyValue(TextEditingValue previous, TextEditingValue next) {
    return _update(
      TextEditingValue(
        text: previous.text,
        composing: _activeComposing(previous.composing, previous.text),
      ),
      next,
    );
  }

  TextInputUpdate _update(TextEditingValue previous, TextEditingValue next) {
    final composing = _activeComposing(next.composing, next.text);
    _composing = composing;

    final before = _committedText(previous.text, previous.composing);
    final after = _committedText(next.text, composing);
    final common = _commonPrefixLength(before, after);

    return TextInputUpdate(
      edit: TerminalTextEdit(
        backspaces: before.sublist(common).length,
        text: String.fromCharCodes(after.sublist(common)),
      ),
      value: next,
      composingText: composing.isCollapsed ? null : composing.textInside(next.text),
    );
  }

  static TerminalTextEdit? _periodShortcut(TextEditingDelta delta) {
    if (delta is! TextEditingDeltaReplacement ||
        delta.textReplaced != ' ' ||
        _activeComposing(delta.composing, delta.oldText) != TextRange.empty ||
        !delta.selection.isCollapsed) {
      return null;
    }
    final cursor = delta.selection.baseOffset;
    return switch (delta.replacementText) {
      '.' when cursor == delta.replacedRange.start + 1 => const TerminalTextEdit(),
      '. ' when cursor == delta.replacedRange.start + 2 => const TerminalTextEdit(text: ' '),
      _ => null,
    };
  }

  /// The committed part of [text] as code points (so edits never split a
  /// surrogate pair).
  static List<int> _committedText(String text, TextRange composing) {
    final end = composing.isCollapsed ? text.length : composing.start;
    return text.substring(0, end).runes.toList(growable: false);
  }

  static TextRange _activeComposing(TextRange composing, String text) {
    if (!composing.isValid || composing.isCollapsed || composing.end > text.length) {
      return TextRange.empty;
    }
    return composing;
  }

  static int _commonPrefixLength(List<int> a, List<int> b) {
    final limit = a.length < b.length ? a.length : b.length;
    var i = 0;
    while (i < limit && a[i] == b[i]) {
      i++;
    }
    return i;
  }
}
