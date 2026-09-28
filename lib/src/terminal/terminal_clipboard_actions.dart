/// 鼠标右键只决定复制或粘贴；选区文字仍由 Rust 返回。
class TerminalClipboardActions {
  TerminalClipboardActions({
    required this.selection,
    required this.requestCopy,
    required this.writeClipboard,
    required this.clearSelection,
    required this.paste,
    required this.onError,
  });

  /// 按值比较的选区快照；复制期间改选时不清除新的选区。
  final Object? Function() selection;
  final void Function() requestCopy;
  final Future<void> Function(String text) writeClipboard;
  final void Function() clearSelection;
  final Future<void> Function() paste;
  final void Function(Object error) onError;
  bool _copying = false;
  bool _pasting = false;
  bool _disposed = false;
  Object? _copiedSelection;
  int _generation = 0;

  void copy() {
    if (_disposed || _copying || _pasting) return;
    final current = selection();
    if (current == null) return;
    _copiedSelection = current;
    ++_generation;
    _copying = true;
    try {
      requestCopy();
    } catch (error) {
      cancelCopy();
      onError(error);
    }
  }

  Future<void> rightClick() async {
    if (_disposed || _copying || _pasting) return;
    if (selection() != null) {
      copy();
      return;
    }
    _pasting = true;
    try {
      await paste();
    } catch (error) {
      if (!_disposed) onError(error);
    } finally {
      _pasting = false;
    }
  }

  Future<void> receiveCopy(String text) async {
    if (_disposed || !_copying) return;
    final generation = _generation;
    final copiedSelection = _copiedSelection;
    try {
      await writeClipboard(text);
      if (!_disposed &&
          generation == _generation &&
          selection() == copiedSelection) {
        clearSelection();
      }
    } catch (error) {
      if (!_disposed && generation == _generation) onError(error);
    } finally {
      if (generation == _generation) {
        _copying = false;
        _copiedSelection = null;
      }
    }
  }

  void cancelCopy() {
    ++_generation;
    _copying = false;
    _copiedSelection = null;
  }

  void dispose() {
    _disposed = true;
    cancelCopy();
  }
}
