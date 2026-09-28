import 'dart:async';

import 'package:flutter/foundation.dart';

/// Keeps hardware key input in the order it was typed.
///
/// Printable keys are left to the platform's text input so IMEs can compose
/// with them; the platform answers each one asynchronously, in order, with one
/// editing update (an insertion, a preedit change, a commit). Keys the
/// terminal handles directly (Enter, arrows, Ctrl combinations…) would
/// otherwise overtake that text: typing `ls` and Return quickly could run `l`
/// and leave `s` on the next line. Such a key is held until the platform has
/// answered every text key typed before it — not the ones typed after it — or
/// until the platform stops answering for [timeout] (keys it never answers,
/// such as dead keys). The timeout counts from the last answer, not from the
/// last key: a fast burst of typing keeps making progress for longer than
/// [timeout] and must not release the held keys early.
class HardwareKeyOrder {
  HardwareKeyOrder({this.timeout = const Duration(milliseconds: 150)});

  final Duration timeout;

  /// Printable keys left to the text input so far.
  int _dispatched = 0;

  /// Editing updates the text input has answered with so far.
  int _answered = 0;

  final _held = <_HeldKey>[];

  Timer? _timer;

  /// Whether a key handled now could overtake earlier typed text.
  bool get isBusy => _answered < _dispatched || _held.isNotEmpty;

  /// A printable key was left to the platform's text input.
  void textKeyDispatched() {
    _dispatched++;
    _restartTimer();
  }

  /// The platform's text input delivered one editing update.
  void textInputUpdated() {
    _answered++;
    while (_held.isNotEmpty && _held.first.after <= _answered) {
      _held.removeAt(0).action();
    }
    if (_answered >= _dispatched) {
      _answered = _dispatched = 0;
      if (_held.isEmpty) _cancelTimer();
    } else {
      // Still answering typed text: give the rest another [timeout].
      _restartTimer();
    }
  }

  /// Runs [action] now, or once the text typed before it has arrived.
  void run(VoidCallback action) {
    if (!isBusy) {
      action();
      return;
    }
    _held.add(_HeldKey(_dispatched, action));
    _restartTimer();
  }

  void dispose() {
    _cancelTimer();
    _held.clear();
    _answered = _dispatched = 0;
  }

  void _releaseAll() {
    _cancelTimer();
    _answered = _dispatched = 0;
    final held = List.of(_held);
    _held.clear();
    for (final key in held) {
      key.action();
    }
  }

  void _restartTimer() {
    _timer?.cancel();
    _timer = Timer(timeout, _releaseAll);
  }

  void _cancelTimer() {
    _timer?.cancel();
    _timer = null;
  }
}

class _HeldKey {
  _HeldKey(this.after, this.action);

  /// Runs once this many text keys have been answered.
  final int after;

  final VoidCallback action;
}
