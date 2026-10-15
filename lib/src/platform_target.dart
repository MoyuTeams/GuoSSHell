import 'package:flutter/foundation.dart';

enum AppTarget { mobile, desktop }

/// 只依据 Flutter 的目标平台决定 UI 根；transport 能力仍由 Rust 判定。
AppTarget appTargetFor(TargetPlatform platform) => switch (platform) {
  TargetPlatform.windows ||
  TargetPlatform.macOS ||
  TargetPlatform.linux => AppTarget.desktop,
  _ => AppTarget.mobile,
};
