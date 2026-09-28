import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import 'text_input_delta.dart';

class CustomTextEdit extends StatefulWidget {
  CustomTextEdit({
    super.key,
    required this.child,
    required this.onInsert,
    required this.onDelete,
    required this.onComposing,
    required this.onAction,
    required this.onKeyEvent,
    required this.focusNode,
    this.onTextInputUpdate,
    this.autofocus = false,
    this.readOnly = false,
    // this.initEditingState = TextEditingValue.empty,
    this.inputType = TextInputType.text,
    this.inputAction = TextInputAction.newline,
    this.keyboardAppearance = Brightness.light,
    this.deleteDetection = false,
  });

  final Widget child;

  final void Function(String) onInsert;

  final void Function() onDelete;

  final void Function(String?) onComposing;

  final void Function(TextInputAction) onAction;

  final KeyEventResult Function(FocusNode, KeyEvent) onKeyEvent;

  /// Called after each editing update from the platform has been delivered.
  final VoidCallback? onTextInputUpdate;

  final FocusNode focusNode;

  final bool autofocus;

  final bool readOnly;

  final TextInputType inputType;

  final TextInputAction inputAction;

  final Brightness keyboardAppearance;

  final bool deleteDetection;

  @override
  CustomTextEditState createState() => CustomTextEditState();
}

class CustomTextEditState extends State<CustomTextEdit>
    with TextInputClient
    implements DeltaTextInputClient {
  TextInputConnection? _connection;

  final _deltas = TextInputDeltaInterpreter();

  @override
  void initState() {
    widget.focusNode.addListener(_onFocusChange);
    super.initState();
  }

  @override
  void didUpdateWidget(CustomTextEdit oldWidget) {
    super.didUpdateWidget(oldWidget);

    if (widget.focusNode != oldWidget.focusNode) {
      oldWidget.focusNode.removeListener(_onFocusChange);
      widget.focusNode.addListener(_onFocusChange);
    }

    if (!_shouldCreateInputConnection) {
      _closeInputConnectionIfNeeded();
    } else {
      if (oldWidget.readOnly && widget.focusNode.hasFocus) {
        _openInputConnection();
      }
    }
  }

  @override
  void dispose() {
    widget.focusNode.removeListener(_onFocusChange);
    _closeInputConnectionIfNeeded();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Focus(
      focusNode: widget.focusNode,
      autofocus: widget.autofocus,
      onKeyEvent: _onKeyEvent,
      child: widget.child,
    );
  }

  bool get hasInputConnection => _connection != null && _connection!.attached;

  void requestKeyboard() {
    if (widget.focusNode.hasFocus) {
      _openInputConnection();
    } else {
      widget.focusNode.requestFocus();
    }
  }

  void closeKeyboard() {
    if (hasInputConnection) {
      _connection?.close();
    }
  }

  void setEditingState(TextEditingValue value) {
    _currentEditingState = value;
    if (value.composing.isCollapsed) _deltas.reset();
    _connection?.setEditingState(value);
  }

  void setEditableRect(Rect rect, Rect caretRect) {
    if (!hasInputConnection) {
      return;
    }

    _connection?.setEditableSizeAndTransform(
      rect.size,
      Matrix4.translationValues(0, 0, 0),
    );

    _connection?.setCaretRect(caretRect);
  }

  void _onFocusChange() {
    _openOrCloseInputConnectionIfNeeded();
  }

  KeyEventResult _onKeyEvent(FocusNode focusNode, KeyEvent event) {
    if (_currentEditingState.composing.isCollapsed) {
      return widget.onKeyEvent(focusNode, event);
    }

    return KeyEventResult.skipRemainingHandlers;
  }

  void _openOrCloseInputConnectionIfNeeded() {
    if (widget.focusNode.hasFocus && widget.focusNode.consumeKeyboardToken()) {
      _openInputConnection();
    } else if (!widget.focusNode.hasFocus) {
      _closeInputConnectionIfNeeded();
    }
  }

  bool get _shouldCreateInputConnection => kIsWeb || !widget.readOnly;

  void _openInputConnection() {
    if (!_shouldCreateInputConnection) {
      return;
    }

    if (hasInputConnection) {
      _connection!.show();
    } else {
      final config = TextInputConfiguration(
        inputType: widget.inputType,
        inputAction: widget.inputAction,
        keyboardAppearance: widget.keyboardAppearance,
        autocorrect: false,
        enableSuggestions: false,
        enableIMEPersonalizedLearning: false,
        // A terminal needs the characters that were typed: smart quotes and
        // dashes would turn " into a curly quote and -- into an en dash.
        smartDashesType: SmartDashesType.disabled,
        smartQuotesType: SmartQuotesType.disabled,
        // Deltas carry the platform's own base text, so edits stay correct
        // even when the next keystroke arrives before our reset lands.
        enableDeltaModel: true,
      );

      _connection = TextInput.attach(this, config);

      _connection!.show();

      _resetEditingState();
    }
  }

  void _closeInputConnectionIfNeeded() {
    if (_connection != null && _connection!.attached) {
      _connection!.close();
      _connection = null;
    }
  }

  TextEditingValue get _initEditingState => widget.deleteDetection
      ? const TextEditingValue(
          text: '  ',
          selection: TextSelection.collapsed(offset: 2),
        )
      : const TextEditingValue(
          text: '',
          selection: TextSelection.collapsed(offset: 0),
        );

  late var _currentEditingState = _initEditingState.copyWith();

  @override
  TextEditingValue? get currentTextEditingValue {
    return _currentEditingState;
  }

  @override
  AutofillScope? get currentAutofillScope {
    return null;
  }

  @override
  void updateEditingValueWithDeltas(List<TextEditingDelta> textEditingDeltas) {
    TextInputUpdate? last;
    for (final delta in textEditingDeltas) {
      last = _deltas.applyDelta(delta);
      _deliver(last);
      if (last.answersKey) widget.onTextInputUpdate?.call();
    }
    if (last != null) _afterUpdate(last);
  }

  /// Fallback for platforms that deliver whole values instead of deltas.
  @override
  void updateEditingValue(TextEditingValue value) {
    final update = _deltas.applyValue(_currentEditingState, value);
    _deliver(update);
    widget.onTextInputUpdate?.call();
    _afterUpdate(update);
  }

  void _deliver(TextInputUpdate update) {
    _currentEditingState = update.value;
    final edit = update.edit;
    for (var i = 0; i < edit.backspaces; i++) {
      widget.onDelete();
    }
    if (edit.text.isNotEmpty) {
      widget.onInsert(edit.text);
    }
  }

  void _afterUpdate(TextInputUpdate update) {
    widget.onComposing(update.composingText);

    // Once nothing is composing, go back to the initial state so the next
    // keystroke (including a backspace, see deleteDetection) starts clean.
    if (update.composingText == null &&
        _currentEditingState.text != _initEditingState.text) {
      _resetEditingState();
    }
  }

  void _resetEditingState() {
    _currentEditingState = _initEditingState;
    _deltas.reset();
    _connection?.setEditingState(_initEditingState);
  }

  @override
  void performAction(TextInputAction action) {
    widget.onAction(action);
  }


  @override
  void updateFloatingCursor(RawFloatingCursorPoint point) {}

  @override
  void showAutocorrectionPromptRect(int start, int end) {}

  @override
  void connectionClosed() {}

  @override
  void performPrivateCommand(String action, Map<String, dynamic> data) {}

  @override
  void insertTextPlaceholder(Size size) {}

  @override
  void removeTextPlaceholder() {}

  @override
  void showToolbar() {}
}
