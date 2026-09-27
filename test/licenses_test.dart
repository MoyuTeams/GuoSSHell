import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/settings/licenses.dart';

void main() {
  test('Rust 依赖的许可按「@@@ 包名」分段，每段列出用到它的包', () {
    const source = '''
@@@ ring 0.17.14

   Apache License
   Version 2.0

@@@ russh 0.62.5, russh-util 0.52.0
Copyright (c) 2016 Pierre-Étienne Meunier
MIT License
''';
    final entries = parseRustLicenses(source);
    expect(entries, hasLength(2));
    expect(entries[0].packages, ['ring 0.17.14']);
    expect(entries[0].paragraphs.first.text, 'Apache License');
    expect(entries[1].packages, ['russh 0.62.5', 'russh-util 0.52.0']);
    expect(entries[1].paragraphs.map((paragraph) => paragraph.text).join('\\n'),
        contains('MIT License'));
  });

  test('开头没有「@@@」的内容不成段', () {
    expect(parseRustLicenses('stray text\\n'), isEmpty);
  });
}
