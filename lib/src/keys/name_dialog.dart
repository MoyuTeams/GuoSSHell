import 'package:flutter/material.dart';
import 'package:fluent_ui/fluent_ui.dart' as f;

import '../desktop/windows_chrome.dart';

/// 起名 / 改名对话框，返回输入的名称，取消为 null。
/// 输入框的控制器随对话框一起销毁（对话框关闭动画期间还在用它）。
class NameDialog extends StatefulWidget {
  final String title;
  final String initial;
  final String confirm;

  const NameDialog({
    super.key,
    required this.title,
    required this.initial,
    this.confirm = '保存',
  });

  @override
  State<NameDialog> createState() => _NameDialogState();
}

class _NameDialogState extends State<NameDialog> {
  late final _controller = TextEditingController(text: widget.initial);

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    if (isWindowsDesktop) {
      return f.ContentDialog(
        title: Text(widget.title),
        content: f.TextBox(
          controller: _controller,
          autofocus: true,
          placeholder: '名称',
          onSubmitted: (value) => Navigator.pop(context, value),
        ),
        actions: [
          f.Button(
            onPressed: () => Navigator.pop(context),
            child: const Text('取消'),
          ),
          f.FilledButton(
            onPressed: () => Navigator.pop(context, _controller.text),
            child: Text(widget.confirm),
          ),
        ],
      );
    }
    return AlertDialog(
      title: Text(widget.title),
      content: TextField(
        controller: _controller,
        autofocus: true,
        decoration: const InputDecoration(labelText: '名称'),
        onSubmitted: (value) => Navigator.pop(context, value),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('取消'),
        ),
        FilledButton(
          onPressed: () => Navigator.pop(context, _controller.text),
          child: Text(widget.confirm),
        ),
      ],
    );
  }
}
