import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

/// Rust 核心各依赖的许可（`scripts/licenses.sh` 由 cargo-about 生成）：每段以
/// `@@@ 包名 版本, …` 一行开头，后面是许可原文。
const _rustLicenses = 'assets/licenses/rust-crates.txt';
const _rsHellLicense = 'rust/LICENSES/rsHell-MIT.txt';
const _fontLicense = 'assets/fonts/LICENSE-MesloLGS-NF.txt';
const _interfaceFontLicense = 'assets/fonts/LICENSE-MiSans.txt';

/// 许可页的条目：Dart 包的许可 Flutter 自己收集；这里补上上游 rsHell、内置字体与 Rust 依赖。
void registerLicenses() {
  LicenseRegistry.addLicense(() async* {
    yield LicenseEntryWithLineBreaks(const [
      'MiSans（小米界面字体）',
    ], await rootBundle.loadString(_interfaceFontLicense));
    yield LicenseEntryWithLineBreaks(const [
      'rsHell',
    ], await rootBundle.loadString(_rsHellLicense));
    yield LicenseEntryWithLineBreaks(const [
      'MesloLGS NF',
    ], await rootBundle.loadString(_fontLicense));
    for (final entry in parseRustLicenses(
      await rootBundle.loadString(_rustLicenses),
    )) {
      yield entry;
    }
  });
}

@visibleForTesting
List<LicenseEntry> parseRustLicenses(String source) {
  final entries = <LicenseEntry>[];
  List<String>? packages;
  final text = StringBuffer();
  void flush() {
    final current = packages;
    if (current != null && current.isNotEmpty) {
      entries.add(LicenseEntryWithLineBreaks(current, text.toString().trim()));
    }
    text.clear();
  }

  for (final line in const LineSplitter().convert(source)) {
    if (line.startsWith('@@@ ')) {
      flush();
      packages = [
        for (final package in line.substring(4).split(','))
          if (package.trim().isNotEmpty) package.trim(),
      ];
    } else if (packages != null) {
      text.writeln(line);
    }
  }
  flush();
  return entries;
}
