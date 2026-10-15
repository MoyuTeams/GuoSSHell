import 'dart:io';

import 'package:flutter/widgets.dart';
import 'package:flutter/services.dart';

import 'src/bindings/bindings.dart';
import 'src/desktop/native_input_spike_sdk/bootstrap.dart';
import 'src/desktop/native_input_spike_sdk/fixture_app.dart';
import 'src/desktop/native_input_spike_sdk/sdk.dart';

/// 独立显式 entry；不导入 main/app，也不 bind 任何普通输入或配置服务。
void main() {
  WidgetsFlutterBinding.ensureInitialized();
  final bootstrap = SdkBootstrap(
    native: const MethodChannelFixtureAdapter(),
    sdk: RinfFixtureSdk(),
    normalHandlers: assignRustSignal,
    controlledPath: const String.fromEnvironment(
      'GUOSSH_NATIVE_INPUT_SPIKE_DLL',
    ),
    explicitOptIn:
        Platform.isWindows &&
        const bool.fromEnvironment('GUOSSH_NATIVE_INPUT_SPIKE_SDK'),
  );
  runApp(
    SdkFixtureApp(
      bootstrap: bootstrap,
      onClose: () {
        bootstrap.close();
        SystemNavigator.pop();
      },
    ),
  );
}
