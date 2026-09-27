import 'package:flutter/material.dart';
import 'package:rinf/rinf.dart';

import 'src/app.dart';
import 'src/bindings/bindings.dart';
import 'src/settings/licenses.dart';

Future<void> main() async {
  await initializeRust(assignRustSignal);
  registerLicenses();
  runApp(const GuoSSHellApp());
}
