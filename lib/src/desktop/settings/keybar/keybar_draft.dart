import 'package:flutter/foundation.dart';

import '../../../terminal/key_bar_layout.dart';

List<List<String>> immutableKeyBarRows(List<List<String>> rows) =>
    List.unmodifiable(rows.map((row) => List<String>.unmodifiable(row)));

bool validKeyBarText(String text) =>
    text.runes.length <= 32 &&
    text.runes.any((rune) => !_rustWhitespace(rune)) &&
    !text.runes.any((rune) => rune <= 0x1f || rune >= 0x7f && rune <= 0x9f);

// 与 Rust str::trim 的 Unicode White_Space 属性一致，不截断用户数据。
bool _rustWhitespace(int rune) =>
    rune >= 0x09 && rune <= 0x0d ||
    const {
      0x20,
      0x85,
      0xa0,
      0x1680,
      0x2028,
      0x2029,
      0x202f,
      0x205f,
      0x3000,
    }.contains(rune) ||
    rune >= 0x2000 && rune <= 0x200a;

bool validKeyBarRows(List<List<String>> rows) =>
    rows.length == 2 &&
    rows.every((row) => row.length <= 24) &&
    rows
        .expand((row) => row)
        .every(
          (id) => id.startsWith('text:')
              ? validKeyBarText(id.substring(5))
              : keyBarButtons.any((button) => button.id == id),
        );

bool equalKeyBarRows(List<List<String>> a, List<List<String>> b) =>
    a.length == b.length &&
    Iterable.generate(a.length).every((i) => listEquals(a[i], b[i]));

/// identity 只属于本地草稿，编码只使用 code。
class KeyBarEntry {
  KeyBarEntry(this.code);
  final String code;
}

class KeyBarDrag {
  const KeyBarDrag.copy(this.code) : entry = null;
  KeyBarDrag.move(KeyBarEntry value) : code = value.code, entry = value;
  final String code;
  final KeyBarEntry? entry;
}

/// 由父级持有可跨编辑 scope 保留；关闭控件不会清除草稿或基线。
class KeyBarDraft extends ChangeNotifier {
  KeyBarDraft(List<List<String>> initialRows) {
    if (!validKeyBarRows(initialRows)) throw ArgumentError.value(initialRows);
    _baseline = immutableKeyBarRows(initialRows);
    _rows = _entries(initialRows);
  }

  late List<List<KeyBarEntry>> _rows;
  late List<List<String>> _baseline;
  KeyBarEntry? selected;
  int activeRow = 0;

  static List<List<KeyBarEntry>> _entries(List<List<String>> rows) => [
    for (final row in rows) [for (final code in row) KeyBarEntry(code)],
  ];

  List<List<KeyBarEntry>> get entries => List.unmodifiable(
    _rows.map((row) => List<KeyBarEntry>.unmodifiable(row)),
  );
  List<List<String>> get rows => immutableKeyBarRows([
    for (final row in _rows) [for (final entry in row) entry.code],
  ]);
  bool get dirty => !equalKeyBarRows(rows, _baseline);

  void select(KeyBarEntry? entry, int row) {
    selected = entry;
    activeRow = row;
    notifyListeners();
  }

  List<List<KeyBarEntry>>? _proposed(KeyBarDrag drag, int row, int index) {
    if (row < 0 || row > 1 || index < 0 || index > 24) return null;
    if (keyBarButton(drag.code) == null ||
        drag.code.startsWith('text:') &&
            !validKeyBarText(drag.code.substring(5))) {
      return null;
    }
    final next = [
      for (final source in _rows) [...source],
    ];
    if (drag.entry != null) {
      if (!next.any((source) => source.contains(drag.entry))) return null;
      final oldIndex = next[row].indexOf(drag.entry!);
      // 未物化 gap 的 index 表示最终列，不因源按钮移除而改号。
      if (oldIndex >= 0 && oldIndex < index && index <= next[row].length) {
        index--;
      }
      for (final source in next) {
        source.remove(drag.entry);
      }
    }
    while (next[row].length < index) {
      next[row].add(KeyBarEntry('spacer'));
    }
    next[row].insert(index, drag.entry ?? KeyBarEntry(drag.code));
    return next.every((source) => source.length <= 24) ? next : null;
  }

  bool canDrop(KeyBarDrag drag, int row, int index) =>
      _proposed(drag, row, index) != null;

  bool drop(KeyBarDrag drag, int row, int index) {
    final next = _proposed(drag, row, index);
    if (next == null) return false;
    _rows = next;
    activeRow = row;
    selected = drag.entry ?? _rows[row][index];
    notifyListeners();
    return true;
  }

  void deleteSelected() {
    if (selected == null) return;
    for (final row in _rows) {
      row.remove(selected);
    }
    selected = null;
    notifyListeners();
  }

  bool moveSelected(int dx, int dy) {
    final entry = selected;
    if (entry == null) return false;
    final row = _rows.indexWhere((source) => source.contains(entry));
    if (row < 0) return false;
    final index = _rows[row].indexOf(entry);
    final targetRow = row + dy;
    if (targetRow < 0 || targetRow > 1) return false;
    if (dy == 0) {
      final target = index + dx;
      if (target < 0 || target >= _rows[row].length) return false;
      return drop(KeyBarDrag.move(entry), row, dx > 0 ? target + 1 : target);
    }
    return drop(KeyBarDrag.move(entry), targetRow, index);
  }

  void restoreDefaults() {
    _rows = _entries(defaultKeyBarRows);
    selected = null;
    notifyListeners();
  }

  void replaceText(KeyBarEntry entry, String text) {
    if (!validKeyBarText(text)) return;
    for (final row in _rows) {
      final index = row.indexOf(entry);
      if (index < 0) continue;
      selected = row[index] = KeyBarEntry('text:$text');
      notifyListeners();
      return;
    }
  }

  /// 只改变已确认基线，不覆盖保存期间继续输入的草稿。
  void confirm(List<List<String>> saved) {
    _baseline = immutableKeyBarRows(saved);
    notifyListeners();
  }
}
