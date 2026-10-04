import 'dart:math' as math;

import 'package:fluent_ui/fluent_ui.dart' as fluent;
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart' as widgets;

import '../../bindings/bindings.dart';
import '../../terminal/prompt_dialogs.dart' show PromptAnswer;
import '../desktop_theme.dart';

/// 桌面认证入口；兼容旧 Material Navigator 内的终端窗格，不依赖其 Material 主题。
Future<PromptAnswer> showDesktopPromptDialog(
  fluent.BuildContext context,
  InteractionPrompt prompt,
  ValueListenable<bool> dismissed,
) async {
  if (dismissed.value || !context.mounted) return const PromptAnswer.declined();
  final answer = await _showPromptRoute<PromptAnswer>(
    context,
    dismissed,
    (_) => _DesktopPrompt(prompt: prompt, dismissed: dismissed),
  );
  return dismissed.value
      ? const PromptAnswer.declined()
      : answer ?? const PromptAnswer.declined();
}

/// 兼容工作区有独立 MaterialApp，其本地化覆盖外层 FluentApp；路由须压在就近
/// Navigator 上（随工作区卸载），并为弹窗单独补 Fluent 本地化。
Future<T?> _showPromptRoute<T>(
  fluent.BuildContext context,
  ValueListenable<bool> dismissed,
  fluent.Widget Function(fluent.BuildContext) builder,
) => widgets.showGeneralDialog<T>(
  context: context,
  barrierDismissible: false,
  barrierLabel: '拒绝认证',
  transitionDuration:
      widgets.MediaQuery.disableAnimationsOf(context) ||
          widgets.MediaQuery.accessibleNavigationOf(context)
      ? Duration.zero
      : DesktopMotion.page,
  pageBuilder: (dialogContext, _, _) => widgets.Localizations.override(
    context: dialogContext,
    delegates: const [fluent.FluentLocalizations.delegate],
    child: _DismissOn(
      dismissed: dismissed,
      child: widgets.SafeArea(
        child: widgets.Center(child: builder(dialogContext)),
      ),
    ),
  ),
);

/// 只移除自己所在的 route；二次核对在它上方时不会误关终端工作区。
class _DismissOn extends fluent.StatefulWidget {
  const _DismissOn({required this.dismissed, required this.child});

  final ValueListenable<bool> dismissed;
  final fluent.Widget child;

  @override
  fluent.State<_DismissOn> createState() => _DismissOnState();
}

class _DismissOnState extends fluent.State<_DismissOn> {
  @override
  void initState() {
    super.initState();
    widget.dismissed.addListener(_check);
    fluent.WidgetsBinding.instance.addPostFrameCallback((_) => _check());
  }

  void _check() {
    if (!mounted || !widget.dismissed.value) return;
    final route = fluent.ModalRoute.of(context);
    if (route != null && route.isActive) {
      fluent.Navigator.of(context).removeRoute(route);
    }
  }

  @override
  void dispose() {
    widget.dismissed.removeListener(_check);
    super.dispose();
  }

  @override
  fluent.Widget build(fluent.BuildContext context) => widget.child;
}

String _target(InteractionPrompt prompt) =>
    '${prompt.username}@${prompt.host}${prompt.port == 22 ? '' : ':${prompt.port}'}';

String _endpoint(InteractionPrompt prompt) {
  final port = prompt.port == 22 ? '' : ':${prompt.port}';
  return prompt.address.isEmpty || prompt.address == prompt.host
      ? '${prompt.host}$port'
      : '${prompt.host}$port（${prompt.address}）';
}

class _DesktopPrompt extends fluent.StatefulWidget {
  const _DesktopPrompt({required this.prompt, required this.dismissed});

  final InteractionPrompt prompt;
  final ValueListenable<bool> dismissed;

  @override
  fluent.State<_DesktopPrompt> createState() => _DesktopPromptState();
}

class _DesktopPromptState extends fluent.State<_DesktopPrompt> {
  late final List<fluent.TextEditingController> _answers = List.generate(
    switch (widget.prompt.kind) {
      PromptKind.hostKey => 0,
      PromptKind.keyboardInteractive => widget.prompt.fields.length,
      _ => 1,
    },
    (_) => fluent.TextEditingController(),
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

  void _decline() =>
      fluent.Navigator.of(context).pop(const PromptAnswer.declined());

  void _accept() {
    if (widget.dismissed.value) return;
    fluent.Navigator.of(context).pop(
      PromptAnswer.accepted([
        for (final controller in _answers) controller.text,
      ], remember: _remember),
    );
  }

  Future<void> _confirmChangedKey() async {
    if (!_verified || widget.dismissed.value) return;
    final prompt = widget.prompt;
    final confirmed = await _showPromptRoute<bool>(
      context,
      widget.dismissed,
      (_) => fluent.ContentDialog(
        title: const fluent.Text('再次确认更换主机密钥'),
        content: fluent.SingleChildScrollView(
          child: fluent.Column(
            mainAxisSize: fluent.MainAxisSize.min,
            crossAxisAlignment: fluent.CrossAxisAlignment.start,
            children: [
              fluent.Text('请再次核对 ${_endpoint(prompt)} 的新指纹。若原因不明，请断开连接。'),
              const fluent.SizedBox(height: DesktopSpacing.md),
              _fingerprint(prompt),
            ],
          ),
        ),
        actions: [
          fluent.FilledButton(
            onPressed: () => fluent.Navigator.of(context).pop(false),
            child: const fluent.Text('断开'),
          ),
          fluent.Button(
            onPressed: () => fluent.Navigator.of(context).pop(true),
            child: const fluent.Text('确认替换并连接'),
          ),
        ],
      ),
    );
    if (!mounted || widget.dismissed.value) return;
    if (confirmed == true) {
      _accept();
    } else {
      _decline();
    }
  }

  fluent.Widget _fingerprint(InteractionPrompt prompt) => fluent.Container(
    width: double.infinity,
    padding: const fluent.EdgeInsets.all(DesktopSpacing.md),
    decoration: fluent.BoxDecoration(
      color: DesktopColors.canvas,
      border: fluent.Border.all(
        color: DesktopColors.border,
        width: DesktopMetrics.border,
      ),
      borderRadius: fluent.BorderRadius.circular(DesktopMetrics.radius),
    ),
    child: fluent.SelectableText(
      '${prompt.algorithm}\n${prompt.fingerprint}',
      style: fluent.FluentTheme.of(context).typography.body
          ?.copyWith(fontFamily: 'MesloLGS NF'),
    ),
  );

  fluent.Widget _input(
    int index,
    String label, {
    required bool secret,
    bool autofocus = false,
    bool last = true,
  }) => fluent.Column(
    crossAxisAlignment: fluent.CrossAxisAlignment.start,
    mainAxisSize: fluent.MainAxisSize.min,
    children: [
      fluent.Text(label, style: fluent.FluentTheme.of(context).typography.body),
      const fluent.SizedBox(height: DesktopSpacing.sm),
      fluent.TextBox(
        controller: _answers[index],
        autofocus: autofocus,
        obscureText: secret,
        autocorrect: false,
        enableSuggestions: false,
        smartDashesType: SmartDashesType.disabled,
        smartQuotesType: SmartQuotesType.disabled,
        inputFormatters: [FilteringTextInputFormatter.deny(RegExp('[\r\n]'))],
        textInputAction: last ? TextInputAction.done : TextInputAction.next,
        onSubmitted: last ? (_) => _accept() : null,
      ),
    ],
  );

  @override
  fluent.Widget build(fluent.BuildContext context) {
    final prompt = widget.prompt;
    final passphrase = prompt.kind == PromptKind.passphrase;
    final pin = prompt.kind == PromptKind.cardPin;
    final host = prompt.kind == PromptKind.hostKey;
    final changed = host && prompt.changed;
    final interactive = prompt.kind == PromptKind.keyboardInteractive;
    final tries = prompt.triesLeft;
    final left = tries < 0
        ? null
        : tries == 1
        ? '只剩最后 1 次，再错卡会锁定'
        : '还可以试 $tries 次，用完卡会锁定';
    final title = host
        ? changed
              ? '主机密钥已变更'
              : '确认主机密钥'
        : interactive
        ? prompt.name.isNotEmpty
              ? prompt.name
              : '服务器要求验证'
        : pin
        ? '输入卡的 PIN'
        : passphrase
        ? '输入私钥口令'
        : '输入密码';
    return fluent.ContentDialog(
      title: fluent.Text(title),
      content: fluent.SizedBox(
        width: math.max(
          0,
          math.min(
            DesktopMetrics.paneWidth * 2,
            fluent.MediaQuery.sizeOf(context).width - DesktopSpacing.xxl * 2,
          ),
        ),
        child: fluent.SingleChildScrollView(
          child: fluent.Column(
            crossAxisAlignment: fluent.CrossAxisAlignment.start,
            mainAxisSize: fluent.MainAxisSize.min,
            children: [
              if (host) ...[
                if (changed)
                  fluent.Semantics(
                    liveRegion: true,
                    child: fluent.Text(
                      '${_endpoint(prompt)} 出示的密钥与之前记录的不一致。可能是服务器更换了密钥，'
                      '也可能是中间人攻击。确认原因之前不要继续。',
                    ),
                  )
                else
                  fluent.Text('首次连接 ${_endpoint(prompt)}。请核对服务器的密钥指纹：'),
                const fluent.SizedBox(height: DesktopSpacing.md),
                _fingerprint(prompt),
                if (changed) ...[
                  const fluent.SizedBox(height: DesktopSpacing.lg),
                  fluent.Checkbox(
                    checked: _verified,
                    onChanged: (value) =>
                        setState(() => _verified = value ?? false),
                    content: const fluent.Text('我已通过其他途径核对新指纹'),
                  ),
                ],
              ] else ...[
                fluent.Text(
                  pin
                      ? 'OpenPGP 卡「${prompt.name}」 · ${_target(prompt)}'
                      : passphrase
                      ? '私钥「${prompt.name}」 · ${_target(prompt)}'
                      : _target(prompt),
                ),
                if (interactive && prompt.instruction.isNotEmpty) ...[
                  const fluent.SizedBox(height: DesktopSpacing.sm),
                  fluent.Text(prompt.instruction),
                ],
                if ((pin && (prompt.retry || left != null)) ||
                    (!pin && !interactive && prompt.retry)) ...[
                  const fluent.SizedBox(height: DesktopSpacing.sm),
                  fluent.Semantics(
                    liveRegion: true,
                    child: fluent.Text(
                      pin
                          ? '${prompt.retry ? 'PIN 不正确。' : ''}${left ?? ''}'
                          : '口令不正确，请重新输入',
                    ),
                  ),
                ],
                for (final (index, field) in prompt.fields.indexed)
                  if (interactive) ...[
                    const fluent.SizedBox(height: DesktopSpacing.md),
                    _input(
                      index,
                      field.label.trim(),
                      secret: !field.echo,
                      autofocus: index == 0,
                      last: index == prompt.fields.length - 1,
                    ),
                  ],
                if (!interactive) ...[
                  const fluent.SizedBox(height: DesktopSpacing.md),
                  _input(
                    0,
                    pin
                        ? 'PIN'
                        : passphrase
                        ? '口令'
                        : '密码',
                    secret: true,
                    autofocus: true,
                  ),
                ],
                if (pin || (!interactive && prompt.canRemember)) ...[
                  const fluent.SizedBox(height: DesktopSpacing.lg),
                  fluent.Checkbox(
                    checked: _remember,
                    onChanged: (value) =>
                        setState(() => _remember = value ?? false),
                    content: fluent.Text(
                      pin
                          ? '记住 PIN，直到退出 App'
                          : passphrase
                          ? '保存口令到钥匙串'
                          : '连接成功后保存到钥匙串',
                    ),
                  ),
                ],
              ],
            ],
          ),
        ),
      ),
      actions: [
        if (changed)
          fluent.FilledButton(
            onPressed: _decline,
            child: const fluent.Text('断开'),
          )
        else
          fluent.Button(onPressed: _decline, child: const fluent.Text('取消')),
        if (changed)
          fluent.Button(
            onPressed: _verified ? _confirmChangedKey : null,
            child: const fluent.Text('替换旧密钥并连接'),
          )
        else
          fluent.FilledButton(
            onPressed: _accept,
            child: fluent.Text(
              host
                  ? '信任并连接'
                  : interactive
                  ? '确定'
                  : '连接',
            ),
          ),
      ],
    );
  }
}
