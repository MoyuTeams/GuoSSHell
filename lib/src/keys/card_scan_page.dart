import 'dart:async';

import 'package:flutter/material.dart';

import '../bindings/bindings.dart';
import 'key_requests.dart';
import 'name_dialog.dart';

/// 添加 OpenPGP 卡：读出插着（或经 NFC 靠近）的卡，选一张登记它认证槽里的密钥。
/// 私钥始终留在卡里。成功后返回新登记的私钥 id。
class CardScanPage extends StatefulWidget {
  const CardScanPage({super.key});

  @override
  State<CardScanPage> createState() => _CardScanPageState();
}

class _CardScanPageState extends State<CardScanPage> {
  bool _busy = false;
  bool _nfcAvailable = false;
  List<CardSummary>? _cards;
  String? _error;

  @override
  void initState() {
    super.initState();
    _scan();
  }

  Future<void> _scan({bool nfc = false}) async {
    setState(() {
      _busy = true;
      _error = null;
    });
    CardScanResult? result;
    String? error;
    try {
      result = await scanCards(nfc: nfc);
      if (result.error != KeyError.none) error = keyErrorText(result.error);
    } on TimeoutException {
      error = '读卡超时';
    }
    if (!mounted) return;
    setState(() {
      _busy = false;
      _error = error;
      if (result != null) {
        _nfcAvailable = result.nfcAvailable;
        if (result.error == KeyError.none) _cards = result.cards;
      }
    });
  }

  Future<void> _add(CardSummary card) async {
    final name = await showDialog<String>(
      context: context,
      builder: (_) => NameDialog(
        title: '添加 OpenPGP 卡',
        initial: card.cardholder.isNotEmpty ? card.cardholder : 'OpenPGP 卡 ${card.ident}',
        confirm: '添加',
      ),
    );
    if (name == null || !mounted) return;
    setState(() => _busy = true);
    String? error;
    KeyResult? result;
    try {
      result = await addCardKey(card.ident, name);
      if (result.error != KeyError.none) error = keyErrorText(result.error);
    } on TimeoutException {
      error = '操作超时';
    }
    if (!mounted) return;
    if (error == null) {
      Navigator.of(context).pop(result!.keyId);
      return;
    }
    setState(() {
      _busy = false;
      _error = error;
    });
  }

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final cards = _cards;
    return Scaffold(
      appBar: AppBar(
        title: const Text('添加 OpenPGP 卡'),
        actions: [
          IconButton(
            tooltip: '重新读取',
            icon: const Icon(Icons.refresh),
            onPressed: _busy ? null : _scan,
          ),
        ],
      ),
      body: SafeArea(
        child: ListView(
          padding: const EdgeInsets.all(20),
          children: [
            Text(
              '把 OpenPGP 卡（或带 OpenPGP 功能的安全密钥）插到设备上，登记它认证槽里的密钥。'
              '私钥始终留在卡里，登录时要插着卡并输入卡的 PIN。',
              style: TextStyle(color: scheme.onSurfaceVariant),
            ),
            const SizedBox(height: 16),
            if (_busy) const LinearProgressIndicator(),
            if (_error != null) ...[
              const SizedBox(height: 8),
              Text(_error!, style: TextStyle(color: scheme.error)),
            ],
            if (cards != null && cards.isEmpty && !_busy)
              const Padding(
                padding: EdgeInsets.symmetric(vertical: 16),
                child: Text('没有找到 OpenPGP 卡。插上卡后点右上角重新读取。'),
              ),
            for (final card in cards ?? const <CardSummary>[])
              Card(
                margin: const EdgeInsets.only(top: 12),
                child: ListTile(
                  leading: const Icon(Icons.credit_card),
                  title: Text(card.cardholder.isNotEmpty ? card.cardholder : 'OpenPGP 卡'),
                  subtitle: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text('卡号 ${card.ident} · ${card.algorithm}'),
                      if (card.fingerprint.isNotEmpty)
                        Text(
                          card.fingerprint,
                          style: const TextStyle(fontFamily: 'Menlo', fontSize: 11),
                        ),
                      Text([
                        'PIN 还可以试 ${card.pinTriesLeft} 次',
                        if (card.touch) '签名要按卡上的按键',
                      ].join(' · ')),
                    ],
                  ),
                  isThreeLine: true,
                  trailing: card.added
                      ? const Text('已添加')
                      : card.publicKey.isEmpty
                          ? const Text('不可用')
                          : FilledButton(
                              onPressed: _busy ? null : () => _add(card),
                              child: const Text('添加'),
                            ),
                ),
              ),
            if (_nfcAvailable) ...[
              const SizedBox(height: 20),
              OutlinedButton.icon(
                onPressed: _busy ? null : () => _scan(nfc: true),
                icon: const Icon(Icons.contactless_outlined),
                label: const Text('通过 NFC 读取'),
              ),
            ],
          ],
        ),
      ),
    );
  }
}
