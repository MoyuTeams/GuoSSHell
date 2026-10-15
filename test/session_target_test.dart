import 'package:flutter_test/flutter_test.dart';

import 'package:guosh_shell/src/terminal/session_target.dart';

void main() {
  test('旧目录目标保留默认解析和原目标字段', () {
    const target = SessionTarget.saved(
      connectionId: 'saved-connection',
      title: '示例连接',
    );

    expect(target.profileId, isEmpty);
    expect(target.connectionId, 'saved-connection');
    expect(target.title, '示例连接');
    expect(target.host, isEmpty);
    expect(target.port, 0);
    expect(target.username, isEmpty);
    expect(target.password, isEmpty);
    expect(target.command, isEmpty);
  });

  test('旧快速目标保留默认解析和原认证字段', () {
    const target = SessionTarget.quick(
      host: 'example.invalid',
      port: 22,
      username: 'fixture',
      password: 'synthetic-password',
      command: 'echo fixture',
    );

    expect(target.profileId, isEmpty);
    expect(target.connectionId, isEmpty);
    expect(target.title, 'fixture@example.invalid');
    expect(target.host, 'example.invalid');
    expect(target.port, 22);
    expect(target.username, 'fixture');
    expect(target.password, 'synthetic-password');
    expect(target.command, 'echo fixture');
  });

  test('桌面显式 profile 选择保留目录与快速目标的独立字段', () {
    const saved = SessionTarget.saved(
      connectionId: 'saved-connection',
      title: '示例连接',
      profileId: 'selected-profile',
    );
    const quick = SessionTarget.quick(
      host: 'example.invalid',
      port: 22,
      username: 'fixture',
      profileId: 'selected-profile',
    );

    expect(saved.profileId, 'selected-profile');
    expect(saved.connectionId, 'saved-connection');
    expect(saved.title, '示例连接');
    expect(saved.password, isEmpty);
    expect(quick.profileId, 'selected-profile');
    expect(quick.connectionId, isEmpty);
    expect(quick.title, 'fixture@example.invalid');
    expect(quick.password, isEmpty);
  });
}
