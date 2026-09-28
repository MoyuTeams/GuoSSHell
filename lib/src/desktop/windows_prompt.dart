import 'package:fluent_ui/fluent_ui.dart' as f;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../bindings/bindings.dart';
import '../terminal/prompt_dialogs.dart';
import 'windows_chrome.dart';

/// Windows 认证界面只收集回答；主机密钥与认证决策仍交给原来的 Rust 流程。
class WindowsPrompt extends StatefulWidget {
  const WindowsPrompt({super.key, required this.prompt});
  final InteractionPrompt prompt;
  @override
  State<WindowsPrompt> createState() => _WindowsPromptState();
}

class _WindowsPromptState extends State<WindowsPrompt> {
  late final _answers = List.generate(
    widget.prompt.kind == PromptKind.keyboardInteractive
        ? widget.prompt.fields.length
        : 1,
    (_) => TextEditingController(),
  );
  bool _remember = false;
  bool _verified = false;
  @override
  void dispose() {
    for (final controller in _answers) {
      controller.dispose();
    }
    super.dispose();
  }

  void _accept() => Navigator.pop(
    context,
    PromptAnswer.accepted(
      widget.prompt.kind == PromptKind.hostKey
          ? []
          : _answers.map((item) => item.text).toList(),
      remember: _remember,
    ),
  );

  @override
  Widget build(BuildContext context) {
    final prompt = widget.prompt;
    final hostKey = prompt.kind == PromptKind.hostKey;
    final changed = hostKey && prompt.changed;
    final title = switch (prompt.kind) {
      PromptKind.hostKey => changed ? '主机密钥已变更' : '确认主机密钥',
      PromptKind.password => '输入密码',
      PromptKind.passphrase => '输入私钥口令',
      PromptKind.cardPin => '输入卡的 PIN',
      PromptKind.keyboardInteractive =>
        prompt.name.isEmpty ? '服务器要求验证' : prompt.name,
    };
    return f.ContentDialog(
      constraints: const BoxConstraints(maxWidth: 540, maxHeight: 700),
      title: Text(title),
      content: SingleChildScrollView(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text(
              '${prompt.username}@${prompt.host}:${prompt.port}',
              style: const TextStyle(color: desktopMuted),
            ),
            const SizedBox(height: 16),
            if (hostKey) ...[
              if (changed)
                const f.InfoBar(
                  title: Text('服务器身份与保存的记录不一致'),
                  content: Text('可能是服务器更换密钥，也可能是有人在冒充服务器。请通过其他途径核对指纹后再连接。'),
                  severity: f.InfoBarSeverity.warning,
                )
              else
                const Text('首次连接，请核对服务器的密钥指纹。'),
              const SizedBox(height: 16),
              SelectableText(
                '${prompt.host}（${prompt.address}）\n${prompt.algorithm}\n${prompt.fingerprint}',
                style: const TextStyle(fontFamily: 'Consolas', height: 1.7),
              ),
              if (changed)
                Padding(
                  padding: const EdgeInsets.only(top: 20),
                  child: f.Checkbox(
                    checked: _verified,
                    content: const Text('我已通过其他途径核对新指纹'),
                    onChanged: (value) =>
                        setState(() => _verified = value ?? false),
                  ),
                ),
            ] else ...[
              if (prompt.name.isNotEmpty &&
                  prompt.kind != PromptKind.keyboardInteractive)
                Padding(
                  padding: const EdgeInsets.only(bottom: 12),
                  child: Text(prompt.name),
                ),
              if (prompt.instruction.isNotEmpty)
                Padding(
                  padding: const EdgeInsets.only(bottom: 12),
                  child: Text(prompt.instruction),
                ),
              if (prompt.retry)
                Padding(
                  padding: const EdgeInsets.only(bottom: 12),
                  child: f.InfoBar(
                    title: const Text('验证未通过，请重新输入'),
                    severity: f.InfoBarSeverity.error,
                  ),
                ),
              if (prompt.kind == PromptKind.cardPin && prompt.triesLeft >= 0)
                Padding(
                  padding: const EdgeInsets.only(bottom: 12),
                  child: Text('剩余 ${prompt.triesLeft} 次尝试，用完后卡会锁定。'),
                ),
              for (var index = 0; index < _answers.length; index++)
                Padding(
                  padding: const EdgeInsets.only(bottom: 14),
                  child: f.InfoLabel(
                    label: prompt.kind == PromptKind.keyboardInteractive
                        ? prompt.fields[index].label.trim()
                        : switch (prompt.kind) {
                            PromptKind.cardPin => 'PIN',
                            PromptKind.passphrase => '口令',
                            _ => '密码',
                          },
                    child: f.TextBox(
                      controller: _answers[index],
                      autofocus: index == 0,
                      obscureText:
                          prompt.kind != PromptKind.keyboardInteractive ||
                          !prompt.fields[index].echo,
                      autocorrect: false,
                      enableSuggestions: false,
                      inputFormatters: [
                        FilteringTextInputFormatter.deny(RegExp('[\r\n]')),
                      ],
                      onSubmitted: index == _answers.length - 1
                          ? (_) => _accept()
                          : null,
                      textInputAction: index == _answers.length - 1
                          ? TextInputAction.done
                          : TextInputAction.next,
                    ),
                  ),
                ),
              if (prompt.canRemember || prompt.kind == PromptKind.cardPin)
                f.Checkbox(
                  checked: _remember,
                  content: Text(
                    prompt.kind == PromptKind.cardPin
                        ? '记住 PIN，直到退出 App'
                        : '验证成功后安全保存',
                  ),
                  onChanged: (value) =>
                      setState(() => _remember = value ?? false),
                ),
            ],
          ],
        ),
      ),
      actions: [
        f.Button(
          onPressed: () =>
              Navigator.pop(context, const PromptAnswer.declined()),
          child: Text(changed ? '断开' : '取消'),
        ),
        f.FilledButton(
          onPressed: changed && !_verified ? null : _accept,
          child: Text(
            changed
                ? '替换旧密钥并连接'
                : hostKey
                ? '信任并连接'
                : '连接',
          ),
        ),
      ],
    );
  }
}
