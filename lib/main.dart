import 'package:flutter/material.dart';
import 'package:rinf/rinf.dart';

import 'src/app.dart';
import 'src/bindings/bindings.dart';
import 'src/settings/licenses.dart';
import 'src/desktop/windows_chrome.dart';
import 'src/settings/interface_font.dart';
import 'src/terminal/terminal_input_mode.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  TerminalInputMode.shared.bind();
  await WindowsDesktopWindow.initialize();
  await initializeRust(assignRustSignal);
  registerLicenses();
  InterfaceTypography.instance.connect();
  InterfaceTypography.terminal.connect();
  runApp(InterfaceFontScope(child: const GuoSSHellApp()));
}
