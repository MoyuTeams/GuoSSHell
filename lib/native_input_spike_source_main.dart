import 'dart:io';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'src/bindings/bindings.dart';
import 'src/desktop/native_input_spike_sdk/bootstrap.dart';
import 'src/desktop/native_input_spike_sdk/sdk.dart';
import 'src/desktop/native_input_spike_source/adapter.dart';
import 'src/desktop/native_input_spike_source/fixture_app.dart';

/// 独立私人入口；不导入普通 main／GuoConfig／Typography。
void main() {
  WidgetsFlutterBinding.ensureInitialized();
  final enabled =
      Platform.isWindows &&
      const bool.fromEnvironment('GUOSSH_NATIVE_INPUT_SPIKE_SDK') &&
      const bool.fromEnvironment('GUOSSH_NATIVE_INPUT_SPIKE_SOURCE');
  final bootstrap = SdkBootstrap(
    native: const MethodChannelFixtureAdapter(),
    sdk: RinfFixtureSdk(),
    normalHandlers: assignRustSignal,
    controlledPath: const String.fromEnvironment(
      'GUOSSH_NATIVE_INPUT_SPIKE_DLL',
    ),
    explicitOptIn: enabled,
  );
  runApp(
    SourceFixtureApp(
      bootstrap: bootstrap,
      source: const MethodChannelSourceAdapter(),
      explicitOptIn: enabled,
      onClosed: SystemNavigator.pop,
    ),
  );
}
