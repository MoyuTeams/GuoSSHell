import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/terminal/terminal_clipboard_actions.dart';

void main() {
  Object? selected;
  late TerminalClipboardActions actions;
  late List<String> calls;
  late List<Object> errors;
  Future<void> Function(String)? writer;
  setUp(() {
    selected = 'selected';
    calls = [];
    errors = [];
    writer = null;
    actions = TerminalClipboardActions(
      selection: () => selected,
      requestCopy: () => calls.add('copy'),
      writeClipboard: (text) async {
        calls.add('write:$text');
        await writer?.call(text);
      },
      clearSelection: () {
        calls.add('clear');
        selected = null;
      },
      paste: () async {
        calls.add('paste');
      },
      onError: errors.add,
    );
  });
  tearDown(() => actions.dispose());

  test('选中后右键复制，完成后再次右键直接粘贴', () async {
    await actions.rightClick();
    expect(calls, ['copy']);
    await actions.receiveCopy('engine text');
    expect(selected, isNull);
    await actions.rightClick();
    expect(calls, ['copy', 'write:engine text', 'clear', 'paste']);
  });
  test('剪贴板未写完时保留选区，不重复复制或粘贴旧文本', () async {
    final saved = Completer<void>();
    writer = (_) => saved.future;
    await actions.rightClick();
    final pending = actions.receiveCopy('new text');
    await actions.rightClick();
    expect(selected, 'selected');
    expect(calls, ['copy', 'write:new text']);
    saved.complete();
    await pending;
    expect(selected, isNull);
  });
  test('复制期间改选不清除新选区，失败时也保留选区', () async {
    final saved = Completer<void>();
    writer = (_) => saved.future;
    actions.copy();
    final pending = actions.receiveCopy('old');
    selected = 'new selection';
    saved.complete();
    await pending;
    expect(selected, 'new selection');
    writer = (_) async => throw StateError('clipboard unavailable');
    actions.copy();
    await actions.receiveCopy('new');
    expect(errors, hasLength(1));
    expect(selected, 'new selection');
  });
  test('取消旧复制后，迟到的写入不能清除新选区或新请求', () async {
    final old = Completer<void>();
    writer = (_) => old.future;
    actions.copy();
    final pending = actions.receiveCopy('old');
    actions.cancelCopy();
    selected = 'new';
    actions.copy();
    old.complete();
    await pending;
    expect(selected, 'new');
    writer = (_) async {};
    await actions.receiveCopy('new');
    expect(selected, isNull);
  });
  test('没有选区直接粘贴，销毁后不再操作', () async {
    selected = null;
    await actions.rightClick();
    expect(calls, ['paste']);
    actions.dispose();
    await actions.rightClick();
    expect(calls, ['paste']);
  });
}
