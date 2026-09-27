import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../bindings/bindings.dart';

/// 对话框的回答。`accept = false` 为取消 / 拒绝。
@immutable
class PromptAnswer {
  final bool accept;
  final List<String> answers;
  final bool remember;

  const PromptAnswer.accepted(this.answers, {this.remember = false}) : accept = true;
  const PromptAnswer.declined()
      : accept = false,
        answers = const [],
        remember = false;
}

/// 按问题种类弹对话框。[dismissed] 变 true 时（会话已失败或被关闭）对话框自行关闭，
/// 视为取消。
Future<PromptAnswer> showPromptDialog(
  BuildContext context,
  InteractionPrompt prompt,
  ValueListenable<bool> dismissed,
) async {
  final answer = await showDialog<PromptAnswer>(
    context: context,
    barrierDismissible: false,
    builder: (context) => _DismissOn(
      dismissed: dismissed,
      child: switch (prompt.kind) {
        PromptKind.password || PromptKind.passphrase => _PasswordDialog(prompt: prompt),
        PromptKind.hostKey => _HostKeyDialog(prompt: prompt),
        PromptKind.keyboardInteractive => _KeyboardInteractiveDialog(prompt: prompt),
        PromptKind.cardPin => _CardPinDialog(prompt: prompt),
      },
    ),
  );
  return answer ?? const PromptAnswer.declined();
}

/// [dismissed] 变 true 时关掉所在的对话框（只移除它自己的 route：它可能正在
/// 退场动画里，这时顶上已经是下面的页面了）。
class _DismissOn extends StatefulWidget {
  final ValueListenable<bool> dismissed;
  final Widget child;

  const _DismissOn({required this.dismissed, required this.child});

  @override
  State<_DismissOn> createState() => _DismissOnState();
}

class _DismissOnState extends State<_DismissOn> {
  @override
  void initState() {
    super.initState();
    widget.dismissed.addListener(_check);
    WidgetsBinding.instance.addPostFrameCallback((_) => _check());
  }

  void _check() {
    if (!widget.dismissed.value || !mounted) return;
    final route = ModalRoute.of(context);
    if (route != null && route.isActive) Navigator.of(context).removeRoute(route);
  }

  @override
  void dispose() {
    widget.dismissed.removeListener(_check);
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => widget.child;
}

String _target(InteractionPrompt prompt) =>
    '${prompt.username}@${prompt.host}${prompt.port == 22 ? '' : ':${prompt.port}'}';

class _PasswordDialog extends StatefulWidget {
  final InteractionPrompt prompt;
  const _PasswordDialog({required this.prompt});

  @override
  State<_PasswordDialog> createState() => _PasswordDialogState();
}

class _PasswordDialogState extends State<_PasswordDialog> {
  final _password = TextEditingController();
  bool _remember = false;

  @override
  void dispose() {
    _password.dispose();
    super.dispose();
  }

  void _submit() {
    Navigator.of(context).pop(PromptAnswer.accepted([_password.text], remember: _remember));
  }

  @override
  Widget build(BuildContext context) {
    final prompt = widget.prompt;
    final passphrase = prompt.kind == PromptKind.passphrase;
    return AlertDialog(
      title: Text(passphrase ? '输入私钥口令' : '输入密码'),
      content: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(passphrase ? '私钥「${prompt.name}」 · ${_target(prompt)}' : _target(prompt)),
          const SizedBox(height: 12),
          TextField(
            controller: _password,
            autofocus: true,
            obscureText: true,
            autocorrect: false,
            enableSuggestions: false,
            autofillHints: passphrase ? null : const [AutofillHints.password],
            decoration: InputDecoration(
              labelText: passphrase ? '口令' : '密码',
              errorText: prompt.retry ? '口令不正确，请重新输入' : null,
            ),
            onSubmitted: (_) => _submit(),
          ),
          if (prompt.canRemember)
            CheckboxListTile(
              contentPadding: EdgeInsets.zero,
              controlAffinity: ListTileControlAffinity.leading,
              title: Text(passphrase ? '保存口令到钥匙串' : '连接成功后保存到钥匙串'),
              value: _remember,
              onChanged: (value) => setState(() => _remember = value ?? false),
            ),
        ],
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(const PromptAnswer.declined()),
          child: const Text('取消'),
        ),
        FilledButton(onPressed: _submit, child: const Text('连接')),
      ],
    );
  }
}

/// OpenPGP 卡的 PIN。可以记住到退出 App（只在内存里，不进钥匙串）。
class _CardPinDialog extends StatefulWidget {
  final InteractionPrompt prompt;
  const _CardPinDialog({required this.prompt});

  @override
  State<_CardPinDialog> createState() => _CardPinDialogState();
}

class _CardPinDialogState extends State<_CardPinDialog> {
  final _pin = TextEditingController();
  bool _remember = false;

  @override
  void dispose() {
    _pin.dispose();
    super.dispose();
  }

  void _submit() {
    Navigator.of(context).pop(PromptAnswer.accepted([_pin.text], remember: _remember));
  }

  @override
  Widget build(BuildContext context) {
    final prompt = widget.prompt;
    final tries = prompt.triesLeft;
    final left = switch (tries) {
      < 0 => null,
      1 => '只剩最后 1 次，再错卡会锁定',
      _ => '还可以试 $tries 次，用完卡会锁定',
    };
    return AlertDialog(
      title: const Text('输入卡的 PIN'),
      content: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text('OpenPGP 卡「${prompt.name}」 · ${_target(prompt)}'),
          const SizedBox(height: 12),
          TextField(
            controller: _pin,
            autofocus: true,
            obscureText: true,
            autocorrect: false,
            enableSuggestions: false,
            decoration: InputDecoration(
              labelText: 'PIN',
              errorText: prompt.retry ? 'PIN 不正确。${left ?? ''}' : null,
              helperText: prompt.retry ? null : left,
            ),
            onSubmitted: (_) => _submit(),
          ),
          CheckboxListTile(
            contentPadding: EdgeInsets.zero,
            controlAffinity: ListTileControlAffinity.leading,
            title: const Text('记住 PIN，直到退出 App'),
            value: _remember,
            onChanged: (value) => setState(() => _remember = value ?? false),
          ),
        ],
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(const PromptAnswer.declined()),
          child: const Text('取消'),
        ),
        FilledButton(onPressed: _submit, child: const Text('连接')),
      ],
    );
  }
}

/// 主机密钥确认。首次连接核对指纹即可；密钥变更时给出醒目警告，
/// 必须勾选「已核对」才能替换（二次确认）。
class _HostKeyDialog extends StatefulWidget {
  final InteractionPrompt prompt;
  const _HostKeyDialog({required this.prompt});

  @override
  State<_HostKeyDialog> createState() => _HostKeyDialogState();
}

class _HostKeyDialogState extends State<_HostKeyDialog> {
  bool _verified = false;

  @override
  Widget build(BuildContext context) {
    final prompt = widget.prompt;
    final scheme = Theme.of(context).colorScheme;
    final port = prompt.port == 22 ? '' : ':${prompt.port}';
    final endpoint = prompt.address.isEmpty || prompt.address == prompt.host
        ? '${prompt.host}$port'
        : '${prompt.host}$port（${prompt.address}）';
    final fingerprint = Container(
      width: double.infinity,
      padding: const EdgeInsets.all(8),
      decoration: BoxDecoration(
        color: scheme.surfaceContainerHighest,
        borderRadius: BorderRadius.circular(6),
      ),
      child: SelectableText(
        '${prompt.algorithm}\n${prompt.fingerprint}',
        style: const TextStyle(fontFamily: 'Menlo', fontSize: 12),
      ),
    );

    if (!prompt.changed) {
      return AlertDialog(
        title: const Text('确认主机密钥'),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('首次连接 $endpoint。请核对服务器的密钥指纹：'),
            const SizedBox(height: 12),
            fingerprint,
          ],
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(const PromptAnswer.declined()),
            child: const Text('取消'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(context).pop(const PromptAnswer.accepted([])),
            child: const Text('信任并连接'),
          ),
        ],
      );
    }

    return AlertDialog(
      icon: Icon(Icons.gpp_bad_outlined, color: scheme.error, size: 36),
      title: Text('主机密钥已变更', style: TextStyle(color: scheme.error)),
      content: SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              '$endpoint 出示的密钥与之前记录的不一致。可能是服务器重装或更换了密钥，'
              '也可能是有人在冒充服务器（中间人攻击）。确认原因之前不要继续。',
            ),
            const SizedBox(height: 12),
            const Text('新的密钥指纹：'),
            const SizedBox(height: 4),
            fingerprint,
            CheckboxListTile(
              contentPadding: EdgeInsets.zero,
              controlAffinity: ListTileControlAffinity.leading,
              title: const Text('我已通过其他途径核对新指纹'),
              value: _verified,
              onChanged: (value) => setState(() => _verified = value ?? false),
            ),
          ],
        ),
      ),
      actions: [
        FilledButton(
          onPressed: () => Navigator.of(context).pop(const PromptAnswer.declined()),
          child: const Text('断开'),
        ),
        TextButton(
          onPressed: _verified
              ? () => Navigator.of(context).pop(const PromptAnswer.accepted([]))
              : null,
          style: TextButton.styleFrom(foregroundColor: scheme.error),
          child: const Text('替换旧密钥并连接'),
        ),
      ],
    );
  }
}

class _KeyboardInteractiveDialog extends StatefulWidget {
  final InteractionPrompt prompt;
  const _KeyboardInteractiveDialog({required this.prompt});

  @override
  State<_KeyboardInteractiveDialog> createState() => _KeyboardInteractiveDialogState();
}

class _KeyboardInteractiveDialogState extends State<_KeyboardInteractiveDialog> {
  late final List<TextEditingController> _answers = [
    for (final _ in widget.prompt.fields) TextEditingController(),
  ];

  @override
  void dispose() {
    for (final controller in _answers) {
      controller.dispose();
    }
    super.dispose();
  }

  void _submit() {
    Navigator.of(context).pop(
      PromptAnswer.accepted([for (final controller in _answers) controller.text]),
    );
  }

  @override
  Widget build(BuildContext context) {
    final prompt = widget.prompt;
    return AlertDialog(
      title: Text(prompt.name.isNotEmpty ? prompt.name : '服务器要求验证'),
      content: SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(_target(prompt)),
            if (prompt.instruction.isNotEmpty) ...[
              const SizedBox(height: 8),
              Text(prompt.instruction),
            ],
            for (final (index, field) in prompt.fields.indexed)
              TextField(
                controller: _answers[index],
                autofocus: index == 0,
                obscureText: !field.echo,
                autocorrect: false,
                enableSuggestions: false,
                smartDashesType: SmartDashesType.disabled,
                smartQuotesType: SmartQuotesType.disabled,
                decoration: InputDecoration(labelText: field.label.trim()),
                textInputAction: index == prompt.fields.length - 1
                    ? TextInputAction.done
                    : TextInputAction.next,
                onSubmitted: index == prompt.fields.length - 1 ? (_) => _submit() : null,
                inputFormatters: [FilteringTextInputFormatter.deny(RegExp('[\r\n]'))],
              ),
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(const PromptAnswer.declined()),
          child: const Text('取消'),
        ),
        FilledButton(onPressed: _submit, child: const Text('确定')),
      ],
    );
  }
}
