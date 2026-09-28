import 'package:flutter/material.dart';
import 'package:rinf/rinf.dart';

import 'src/app.dart';
import 'src/bindings/bindings.dart';
import 'src/settings/licenses.dart';
import 'src/desktop/windows_chrome.dart';
import 'src/settings/interface_font.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  await WindowsDesktopWindow.initialize();
  await initializeRust(assignRustSignal);
  registerLicenses();
  InterfaceTypography.instance.connect();
  InterfaceTypography.terminal.connect();
  runApp(InterfaceFontScope(child: const GuoSSHellApp()));
}
