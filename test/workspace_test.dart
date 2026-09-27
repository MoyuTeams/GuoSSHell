import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/terminal/session_target.dart';
import 'package:guosh_shell/src/workspace/workspace.dart';

SessionTarget target(String name) =>
    SessionTarget.saved(connectionId: name, title: name);

/// 树的形状：叶子写连接名，分支写 `h[...]` / `v[...]`。
String shape(PaneNode node) => switch (node) {
  PaneLeaf(:final pane) => pane.controller.target.title,
  PaneSplit(:final axis, :final children) =>
    '${axis == Axis.horizontal ? 'h' : 'v'}[${children.map(shape).join(',')}]',
};

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  test('新标签放在当前标签之后并成为当前标签', () {
    final workspace = Workspace();
    workspace.openTab(target('a'));
    workspace.openTab(target('b'));
    workspace.selectTab(0);
    workspace.openTab(target('c'));
    expect(workspace.tabs.map((tab) => shape(tab.root)), ['a', 'c', 'b']);
    expect(workspace.activeIndex, 1);
  });

  test('同方向的分屏并进同一层，换方向才嵌套', () {
    final workspace = Workspace();
    final a = workspace.openTab(target('a'));
    final b = workspace.split(a, Axis.horizontal, target('b'));
    workspace.split(a, Axis.horizontal, target('c'));
    expect(shape(workspace.activeTab!.root), 'h[a,c,b]');

    final d = workspace.split(b, Axis.vertical, target('d'));
    expect(shape(workspace.activeTab!.root), 'h[a,c,v[b,d]]');
    expect(workspace.activePane, d);
  });

  test('关窗格：只剩一个的分支收起，同方向的并回上一层', () {
    final workspace = Workspace();
    final a = workspace.openTab(target('a'));
    final b = workspace.split(a, Axis.horizontal, target('b'));
    final c = workspace.split(b, Axis.vertical, target('c'));
    final d = workspace.split(c, Axis.horizontal, target('d'));
    expect(shape(workspace.activeTab!.root), 'h[a,v[b,h[c,d]]]');

    // b 关掉后 v[...] 只剩 h[c,d]，它与根同方向，并进根。
    workspace.close(b);
    expect(shape(workspace.activeTab!.root), 'h[a,c,d]');

    workspace.close(d);
    expect(workspace.activePane, c, reason: '活动窗格关掉后落到它前面的窗格');
    workspace.close(c);
    expect(shape(workspace.activeTab!.root), 'a');
  });

  test('关掉标签里最后一个窗格就关掉标签', () {
    final workspace = Workspace();
    final a = workspace.openTab(target('a'));
    final b = workspace.openTab(target('b'));
    workspace.openTab(target('c'));
    workspace.selectTab(1);
    workspace.close(b);
    expect(workspace.tabs.map((tab) => shape(tab.root)), ['a', 'c']);
    expect(workspace.activeTab!.active.controller.target.title, 'c');
    workspace.close(workspace.activePane!);
    workspace.close(a);
    expect(workspace.tabs, isEmpty);
    expect(workspace.activePane, isNull);
  });

  test('关闭嵌套分屏的末尾窗格，焦点留在同级前一个窗格', () {
    final workspace = Workspace();
    final a = workspace.openTab(target('a'));
    final b = workspace.split(a, Axis.horizontal, target('b'));
    final c = workspace.split(b, Axis.vertical, target('c'));
    workspace.close(c);
    expect(shape(workspace.activeTab!.root), 'h[a,b]');
    expect(workspace.activePane, b);
  });

  test('关闭嵌套分屏的首个窗格，焦点留在同级后一个窗格', () {
    final workspace = Workspace();
    final a = workspace.openTab(target('a'));
    final b = workspace.split(a, Axis.horizontal, target('b'));
    final c = workspace.split(b, Axis.vertical, target('c'));
    workspace.activate(b);
    workspace.close(b);
    expect(shape(workspace.activeTab!.root), 'h[a,c]');
    expect(workspace.activePane, c);
  });

  test('相邻项是子树时，焦点落在最靠近关闭位置的叶子', () {
    final workspace = Workspace();
    final a = workspace.openTab(target('a'));
    final b = workspace.split(a, Axis.horizontal, target('b'));
    final d = workspace.split(b, Axis.horizontal, target('d'));
    final c = workspace.split(b, Axis.vertical, target('c'));
    workspace.activate(d);
    workspace.close(d);
    expect(workspace.activePane, c);
  });

  test('窗格按从左到右轮换', () {
    final workspace = Workspace();
    final a = workspace.openTab(target('a'));
    final b = workspace.split(a, Axis.horizontal, target('b'));
    workspace.activate(a);
    workspace.cyclePane(1);
    expect(workspace.activePane, b);
    workspace.cyclePane(1);
    expect(workspace.activePane, a);
    workspace.cyclePane(-1);
    expect(workspace.activePane, b);
  });
}
