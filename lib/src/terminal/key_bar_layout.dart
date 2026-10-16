import 'dart:async';

import 'package:terminal_view/terminal_view.dart' show TerminalKey;

import '../bindings/bindings.dart';

/// 按钮标识保存在偏好文件中，标签和行为在这里定义。
class KeyBarButton {
  final String id;
  final String label;
  final TerminalKey? key;
  final String? text;
  final bool ctrl;

  const KeyBarButton(
    this.id,
    this.label, {
    this.key,
    this.text,
    this.ctrl = false,
  });

  String get compactLabel => switch (id) {
    'enter' => '↵',
    'home' => '行首',
    'end' => '行尾',
    'pageUp' => '上页',
    'pageDown' => '下页',
    'ctrlC' => '^C',
    'ctrlD' => '^D',
    'ctrlZ' => '^Z',
    'ctrlL' => '^L',
    'zoomReset' => '重置',
    _ => label,
  };
}

const keyBarButtons = [
  KeyBarButton('spacer', '空位'),
  KeyBarButton('escape', 'Esc', key: TerminalKey.escape),
  KeyBarButton('tab', 'Tab', key: TerminalKey.tab),
  KeyBarButton('up', '↑', key: TerminalKey.arrowUp),
  KeyBarButton('down', '↓', key: TerminalKey.arrowDown),
  KeyBarButton('left', '←', key: TerminalKey.arrowLeft),
  KeyBarButton('right', '→', key: TerminalKey.arrowRight),
  KeyBarButton('ctrl', 'Ctrl'),
  KeyBarButton('alt', 'Alt'),
  KeyBarButton('keyboard', '键盘'),
  KeyBarButton('backspace', '⌫', key: TerminalKey.backspace),
  KeyBarButton('disconnect', '断开'),
  KeyBarButton('copy', '复制'),
  KeyBarButton('paste', '粘贴'),
  KeyBarButton('pipe', '|', text: '|'),
  KeyBarButton('slash', '/', text: '/'),
  KeyBarButton('minus', '-', text: '-'),
  KeyBarButton('tilde', '~', text: '~'),
  KeyBarButton('period', '.', text: '.'),
  KeyBarButton('enter', 'Enter', key: TerminalKey.enter),
  KeyBarButton('delete', 'Del', key: TerminalKey.delete),
  KeyBarButton('home', 'Home', key: TerminalKey.home),
  KeyBarButton('end', 'End', key: TerminalKey.end),
  KeyBarButton('pageUp', 'PgUp', key: TerminalKey.pageUp),
  KeyBarButton('pageDown', 'PgDn', key: TerminalKey.pageDown),
  KeyBarButton('f1', 'F1', key: TerminalKey.f1),
  KeyBarButton('f2', 'F2', key: TerminalKey.f2),
  KeyBarButton('f3', 'F3', key: TerminalKey.f3),
  KeyBarButton('f4', 'F4', key: TerminalKey.f4),
  KeyBarButton('f5', 'F5', key: TerminalKey.f5),
  KeyBarButton('f6', 'F6', key: TerminalKey.f6),
  KeyBarButton('f7', 'F7', key: TerminalKey.f7),
  KeyBarButton('f8', 'F8', key: TerminalKey.f8),
  KeyBarButton('f9', 'F9', key: TerminalKey.f9),
  KeyBarButton('f10', 'F10', key: TerminalKey.f10),
  KeyBarButton('f11', 'F11', key: TerminalKey.f11),
  KeyBarButton('f12', 'F12', key: TerminalKey.f12),
  KeyBarButton('ctrlC', 'Ctrl+C', key: TerminalKey.keyC, ctrl: true),
  KeyBarButton('ctrlD', 'Ctrl+D', key: TerminalKey.keyD, ctrl: true),
  KeyBarButton('ctrlZ', 'Ctrl+Z', key: TerminalKey.keyZ, ctrl: true),
  KeyBarButton('ctrlL', 'Ctrl+L', key: TerminalKey.keyL, ctrl: true),
  KeyBarButton('zoomIn', '放大'),
  KeyBarButton('zoomOut', '缩小'),
  KeyBarButton('zoomReset', '原字号'),
];

const defaultKeyBarRows = [
  ['escape', 'slash', 'minus', 'home', 'up', 'end', 'keyboard', 'backspace'],
  ['tab', 'ctrl', 'alt', 'left', 'down', 'right', 'copy', 'paste'],
];

KeyBarButton? keyBarButton(String id) {
  if (id.startsWith('text:')) {
    final text = id.substring(5);
    return KeyBarButton(id, text, text: text);
  }
  for (final button in keyBarButtons) {
    if (button.id == id) return button;
  }
  return null;
}

/// 旧协议没有请求编号；超时只结束等待，不释放未结算的写入槽。
class KeyBarLayoutWriter {
  KeyBarLayoutWriter({
    required this.results,
    required this.send,
    this.timeout = const Duration(seconds: 10),
  });

  final Stream<KeyBarLayoutResult> results;
  final void Function(List<List<String>>) send;
  final Duration timeout;
  Future<void>? _pending;

  Future<void> save(List<List<String>> rows) {
    if (_pending != null) {
      return Future.error(StateError('上次保存尚未确认，请稍后再试'));
    }
    final completed = results.first.then<void>((response) {
      if (!response.ok) throw StateError(response.detail);
    });
    _pending = completed;
    // 只释放本次占用的写入槽：send 失败后槽可能已被下一次保存占用。
    void release() {
      if (identical(_pending, completed)) _pending = null;
    }

    completed.then(
      (_) => release(),
      onError: (Object error, StackTrace stack) => release(),
    );
    try {
      send(rows);
    } catch (error, stack) {
      // 请求未发出，不会有确认到达；立即释放写入槽并把错误交给调用者。
      release();
      return Future.error(error, stack);
    }
    return completed.timeout(timeout);
  }
}

final _keyBarWriter = KeyBarLayoutWriter(
  results: KeyBarLayoutResult.rustSignalStream.map((pack) => pack.message),
  send: (rows) => SaveKeyBarLayout(rows: rows).sendSignalToRust(),
);

Future<void> saveKeyBarLayout(List<List<String>> rows) =>
    _keyBarWriter.save(rows);
