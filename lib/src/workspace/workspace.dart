import 'package:flutter/widgets.dart';
import 'package:multi_split_view/multi_split_view.dart';

import '../terminal/session_target.dart';
import '../terminal/terminal_pane.dart';

/// 工作区里的一个窗格：一条会话。[key] 让窗格在分屏、关窗格引起的重新布局中保住
/// 自己的状态（会话不断）。
class WorkspacePane {
  WorkspacePane(SessionTarget target)
    : controller = TerminalPaneController(target);

  final TerminalPaneController controller;
  final GlobalKey key = GlobalKey();
}

/// 分屏树：叶子是窗格，分支是同一方向上并排的若干子树（同方向的分屏合并成一层，
/// 分隔条都能拖）。
sealed class PaneNode {}

final class PaneLeaf extends PaneNode {
  PaneLeaf(this.pane);

  final WorkspacePane pane;
}

final class PaneSplit extends PaneNode {
  PaneSplit(this.axis, List<PaneNode> children) : children = List.of(children) {
    _syncAreas();
  }

  /// [Axis.horizontal] = 左右并排，[Axis.vertical] = 上下叠放。
  final Axis axis;
  final List<PaneNode> children;

  /// 分隔条拖出的比例存在这里；子树增减时保留其余子树的比例。
  final MultiSplitViewController layout = MultiSplitViewController();

  void _syncAreas() {
    final previous = {for (final area in layout.areas) area.data: area.flex};
    layout.areas = [
      for (final child in children)
        Area(flex: previous[child] ?? 1, data: child),
    ];
  }
}

/// 一个标签：一棵分屏树与其中的活动窗格。
class WorkspaceTab {
  WorkspaceTab(WorkspacePane pane) : root = PaneLeaf(pane), active = pane;

  PaneNode root;
  WorkspacePane active;

  /// 树里的全部窗格（从左到右、从上到下）。
  List<WorkspacePane> get panes {
    final panes = <WorkspacePane>[];
    void visit(PaneNode node) {
      switch (node) {
        case PaneLeaf(:final pane):
          panes.add(pane);
        case PaneSplit(:final children):
          children.forEach(visit);
      }
    }

    visit(root);
    return panes;
  }
}

/// 工作区的布局：标签、每个标签的分屏树与活动窗格。这是界面状态——每个窗格是一条
/// 独立的会话，Rust 不需要知道它们怎么摆（PLAN §2：标签与分屏在 Flutter 层）。
class Workspace extends ChangeNotifier {
  final List<WorkspaceTab> tabs = [];
  int _activeIndex = 0;

  int get activeIndex => _activeIndex;
  WorkspaceTab? get activeTab => tabs.isEmpty ? null : tabs[_activeIndex];
  WorkspacePane? get activePane => activeTab?.active;

  Iterable<WorkspacePane> get panes => tabs.expand((tab) => tab.panes);

  /// 新标签（放在当前标签之后）并切过去。
  WorkspacePane openTab(SessionTarget target) {
    final pane = _adopt(WorkspacePane(target));
    final index = tabs.isEmpty ? 0 : _activeIndex + 1;
    tabs.insert(index, WorkspaceTab(pane));
    _activeIndex = index;
    notifyListeners();
    return pane;
  }

  /// 在 [pane] 旁边分出一个新窗格（[Axis.horizontal] 放右边，[Axis.vertical] 放下边），
  /// 新窗格成为活动窗格。
  WorkspacePane split(WorkspacePane pane, Axis axis, SessionTarget target) {
    final tab = _tabOf(pane);
    final added = _adopt(WorkspacePane(target));
    final leaf = PaneLeaf(added);
    final (parent, index) = _parentOf(tab.root, pane);
    if (parent != null && parent.axis == axis) {
      parent.children.insert(index + 1, leaf);
      parent._syncAreas();
    } else {
      final old = parent == null ? tab.root : parent.children[index];
      final replacement = PaneSplit(axis, [old, leaf]);
      if (parent == null) {
        tab.root = replacement;
      } else {
        parent.children[index] = replacement;
        parent._syncAreas();
      }
    }
    tab.active = added;
    _activeIndex = tabs.indexOf(tab);
    notifyListeners();
    return added;
  }

  /// 关掉窗格；标签里最后一个窗格关了就关掉标签。窗格的控制器随之释放（窗格的
  /// widget 从界面上移除时断开会话）。
  void close(WorkspacePane pane) {
    final tab = _tabOf(pane);
    final (parent, index) = _parentOf(tab.root, pane);
    if (parent == null) {
      final tabIndex = tabs.indexOf(tab);
      tabs.removeAt(tabIndex);
      if (_activeIndex > tabIndex || _activeIndex >= tabs.length) {
        _activeIndex = (_activeIndex - 1).clamp(0, tabs.length);
      }
    } else {
      // 先在同级子树里选相邻窗格，不能把局部 index 当作整棵树的索引。
      final nextActive = index > 0
          ? _edgePane(parent.children[index - 1], last: true)
          : _edgePane(parent.children[index + 1], last: false);
      parent.children.removeAt(index);
      if (parent.children.length == 1) {
        _replace(tab, parent, parent.children.single);
      } else {
        parent._syncAreas();
      }
      if (tab.active == pane) {
        tab.active = nextActive;
      }
    }
    _release(pane);
    notifyListeners();
  }

  /// 关掉整个标签。
  void closeTab(WorkspaceTab tab) {
    final index = tabs.indexOf(tab);
    if (index < 0) return;
    tabs.removeAt(index);
    if (_activeIndex > index || _activeIndex >= tabs.length) {
      _activeIndex = (_activeIndex - 1).clamp(0, tabs.length);
    }
    tab.panes.forEach(_release);
    notifyListeners();
  }

  void activate(WorkspacePane pane) {
    final tab = _tabOf(pane);
    final index = tabs.indexOf(tab);
    if (tab.active == pane && _activeIndex == index) return;
    tab.active = pane;
    _activeIndex = index;
    notifyListeners();
  }

  void selectTab(int index) {
    if (index < 0 || index >= tabs.length || index == _activeIndex) return;
    _activeIndex = index;
    notifyListeners();
  }

  /// 同一标签里切到前 / 后一个窗格（循环）。
  void cyclePane(int step) {
    final tab = activeTab;
    if (tab == null) return;
    final panes = tab.panes;
    final next = (panes.indexOf(tab.active) + step) % panes.length;
    activate(panes[next < 0 ? next + panes.length : next]);
  }

  @override
  void dispose() {
    panes.toList().forEach(_release);
    super.dispose();
  }

  WorkspacePane _adopt(WorkspacePane pane) {
    pane.controller.addListener(notifyListeners);
    return pane;
  }

  void _release(WorkspacePane pane) {
    pane.controller.removeListener(notifyListeners);
    // 窗格的 widget 要等这一帧重建时才从界面上移除（届时断开会话），之后再释放控制器。
    WidgetsBinding.instance.addPostFrameCallback(
      (_) => pane.controller.dispose(),
    );
  }

  WorkspaceTab _tabOf(WorkspacePane pane) =>
      tabs.firstWhere((tab) => tab.panes.contains(pane));

  WorkspacePane _edgePane(PaneNode node, {required bool last}) =>
      switch (node) {
        PaneLeaf(:final pane) => pane,
        PaneSplit(:final children) => _edgePane(
          last ? children.last : children.first,
          last: last,
        ),
      };

  /// 窗格叶子的父分支与它在父分支里的位置；叶子就是根时父分支为 null。
  (PaneSplit?, int) _parentOf(PaneNode node, WorkspacePane pane) {
    if (node case PaneSplit(:final children)) {
      for (var i = 0; i < children.length; i++) {
        final child = children[i];
        if (child is PaneLeaf && child.pane == pane) return (node, i);
        final found = _parentOf(child, pane);
        if (found.$1 != null) return found;
      }
    }
    return (null, 0);
  }

  /// 把 [old] 分支换成 [replacement]；换上来的分支与上一层同方向时并进上一层。
  void _replace(WorkspaceTab tab, PaneSplit old, PaneNode replacement) {
    final grandparent = _splitContaining(tab.root, old);
    if (grandparent == null) {
      tab.root = replacement;
      return;
    }
    final index = grandparent.children.indexOf(old);
    if (replacement is PaneSplit && replacement.axis == grandparent.axis) {
      grandparent.children
        ..removeAt(index)
        ..insertAll(index, replacement.children);
    } else {
      grandparent.children[index] = replacement;
    }
    grandparent._syncAreas();
  }

  PaneSplit? _splitContaining(PaneNode node, PaneNode target) {
    if (node case PaneSplit(:final children)) {
      if (children.contains(target)) return node;
      for (final child in children) {
        final found = _splitContaining(child, target);
        if (found != null) return found;
      }
    }
    return null;
  }
}
