import 'dart:typed_data';

const sdkProbeEndpoint = 'guossh_native_input_sdk_probe';
const sdkProbeReplySignal = 'GuoNativeInputSdkProbeReply';
const sdkRequestBytes = 32;
const sdkResponseBytes = 48;

typedef FixtureSignalHandler = void Function(Uint8List, Uint8List);

Uint8List encodeSdkProbe(Uint8List nonce) {
  if (nonce.length != 16 || nonce.every((byte) => byte == 0)) {
    throw ArgumentError('fixture nonce 必须为非零的固定 16 bytes');
  }
  final bytes = Uint8List(sdkRequestBytes);
  bytes.setRange(0, 4, const [71, 78, 83, 81]); // GNSQ
  final view = ByteData.sublistView(bytes);
  view.setUint16(4, 1, Endian.little);
  view.setUint16(6, 1, Endian.little);
  view.setUint32(8, 1, Endian.little);
  bytes.setRange(16, 32, nonce);
  return bytes;
}

bool matchingSdkReply(Uint8List bytes, Uint8List binary, Uint8List nonce) {
  if (bytes.length != sdkResponseBytes ||
      binary.isNotEmpty ||
      nonce.length != 16) {
    return false;
  }
  final view = ByteData.sublistView(bytes);
  if (bytes[0] != 71 ||
      bytes[1] != 78 ||
      bytes[2] != 83 ||
      bytes[3] != 82 ||
      view.getUint16(4, Endian.little) != 1 ||
      view.getUint16(6, Endian.little) != 0 ||
      view.getUint32(8, Endian.little) != 2 ||
      view.getUint32(12, Endian.little) != 64 ||
      view.getUint64(32, Endian.little) == 0 ||
      view.getUint64(32, Endian.little) % 8 != 0 ||
      view.getUint32(40, Endian.little) != 1 ||
      view.getUint32(44, Endian.little) < 1 ||
      view.getUint32(44, Endian.little) > 8) {
    return false;
  }
  for (var index = 0; index < 16; index++) {
    if (bytes[16 + index] != nonce[index]) return false;
  }
  return true;
}

class FixtureDiagnostic {
  const FixtureDiagnostic({
    required this.status,
    required this.coreAbi,
    required this.pointerBits,
    required this.owners,
    required this.rinfExports,
    required this.coreExports,
    required this.fixtureOnly,
    required this.sameModule,
  });

  final int status;
  final int coreAbi;
  final int pointerBits;
  final int owners;
  final int rinfExports;
  final int coreExports;
  final bool fixtureOnly;
  final bool sameModule;

  factory FixtureDiagnostic.fromMap(Object? raw) {
    if (raw is! Map) throw const FormatException('native 诊断格式无效');
    int number(String key) {
      final value = raw[key];
      if (value is! int || value < 0) {
        throw const FormatException('native 诊断数值无效');
      }
      return value;
    }

    bool flag(String key) {
      final value = raw[key];
      if (value is! bool) throw const FormatException('native 诊断标记无效');
      return value;
    }

    return FixtureDiagnostic(
      status: number('status'),
      coreAbi: number('coreAbi'),
      pointerBits: number('pointerBits'),
      owners: number('owners'),
      rinfExports: number('rinfExports'),
      coreExports: number('coreExports'),
      fixtureOnly: flag('fixtureOnly'),
      sameModule: flag('sameModule'),
    );
  }

  bool get safePreflight =>
      status == 0 &&
      coreAbi == 2 &&
      pointerBits == 64 &&
      owners == 0 &&
      rinfExports == 4 &&
      coreExports == 8 &&
      fixtureOnly;
}

class FixturePreflight {
  const FixturePreflight(this.diagnostic, this.compiledLibPath);
  final FixtureDiagnostic diagnostic;
  // 只交给 SDK，不展示、记录或当作 module identity。
  final String compiledLibPath;
}
