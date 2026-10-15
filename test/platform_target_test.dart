import 'package:flutter/foundation.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/platform_target.dart';

void main() {
  test('目标平台分流与屏幕宽度无关', () {
    for (final platform in [
      TargetPlatform.windows,
      TargetPlatform.macOS,
      TargetPlatform.linux,
    ]) {
      expect(appTargetFor(platform), AppTarget.desktop);
    }
    for (final platform in [TargetPlatform.android, TargetPlatform.iOS]) {
      expect(appTargetFor(platform), AppTarget.mobile);
    }
  });
}
