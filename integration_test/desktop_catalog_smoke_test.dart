import 'dart:async';
import 'dart:io';

import 'package:fluent_ui/fluent_ui.dart' as fluent;
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:guosh_shell/src/app.dart';
import 'package:guosh_shell/src/bindings/bindings.dart';
import 'package:guosh_shell/src/desktop/catalog/desktop_catalog_gateway.dart';
import 'package:guosh_shell/src/desktop/catalog/desktop_connection_editor.dart';
import 'package:guosh_shell/src/desktop/desktop_widgets.dart';
import 'package:guosh_shell/src/desktop/keys/desktop_key_dialogs.dart';
import 'package:guosh_shell/src/desktop/keys/desktop_keys_page.dart';
import 'package:guosh_shell/src/desktop/windows_chrome.dart';
import 'package:guosh_shell/src/desktop/windows_shell.dart';
import 'package:guosh_shell/src/platform_target.dart';
import 'package:guosh_shell/src/terminal/session_target.dart';
import 'package:integration_test/integration_test.dart';
import 'package:rinf/rinf.dart';

// flutter test integration_test/desktop_catalog_smoke_test.dart -d windows --no-pub
// 不启动 SSH 会话；不借用 M6 的用户目录、Material 工作区或主机密钥自动信任逻辑。
class _OwnedStartup implements StartupPort {
  _OwnedStartup(this.directory);

  final Directory directory;

  @override
  StreamSubscription<dynamic> listenReady(
    void Function(bool, String, SessionTarget?) onReady,
  ) => AppReady.rustSignalStream.listen((pack) {
    // 忽略调试环境中的自动连接参数，绝不连接外部目标。
    onReady(pack.message.ok, pack.message.detail, null);
  });

  @override
  StreamSubscription<dynamic> listenSettings(void Function() onSettings) =>
      SettingsState.rustSignalStream.listen((_) => onSettings());

  @override
  Future<String> supportDirectory() async => directory.path;

  @override
  void start(String path) => AppStart(supportDir: path).sendSignalToRust();

  @override
  void querySettings() {
    WindowsAppearanceQuery().sendSignalToRust();
    SettingsQuery().sendSignalToRust();
  }

  @override
  void foreground(bool value) =>
      AppLifecycle(foreground: value).sendSignalToRust();
}

Future<void> _appears(WidgetTester tester, Finder finder) async {
  final deadline = DateTime.now().add(const Duration(seconds: 15));
  while (finder.evaluate().isEmpty) {
    if (DateTime.now().isAfter(deadline)) {
      throw TestFailure('等待桌面界面更新超时：$finder');
    }
    await tester.pump(const Duration(milliseconds: 50));
  }
}

Future<void> _disappears(WidgetTester tester, Finder finder) async {
  final deadline = DateTime.now().add(const Duration(seconds: 15));
  while (finder.evaluate().isNotEmpty) {
    if (DateTime.now().isAfter(deadline)) {
      throw TestFailure('等待桌面界面关闭超时：$finder');
    }
    await tester.pump(const Duration(milliseconds: 50));
  }
}

Future<void> _field(WidgetTester tester, String label, String value) async {
  final input = find.byKey(fluent.ValueKey('catalog-field-$label'));
  await tester.ensureVisible(input);
  await tester.enterText(input, value);
}

Finder _iconButton(String label) => find.byWidgetPredicate(
  (widget) => widget is DesktopIconButton && widget.label == label,
);

Future<void> _connectionAction(WidgetTester tester, String action) async {
  await tester.tap(_iconButton('连接操作'));
  // MenuFlyoutItem 是配置项，菜单直接构建其内容，不挂载该类型的 widget。
  final item = find.descendant(
    of: find.byType(fluent.MenuFlyout),
    matching: find.text(action),
  );
  await _appears(tester, item);
  await tester.pumpAndSettle();
  await tester.tap(item);
}

// 每轮用独有查询词，避免把上一轮自动广播的目录快照误当成当前读取。
Future<CatalogState> _readRustCatalog(String query) async {
  final answer = CatalogState.rustSignalStream
      .map((pack) => pack.message)
      .firstWhere((state) => state.query == query)
      .timeout(const Duration(seconds: 15));
  CatalogQuery(query: query).sendSignalToRust();
  return answer;
}

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('Windows 真实工作区目录 CRUD、私钥文件错误与 Rust 关闭清理（隔离存储）', (tester) async {
    if (!Platform.isWindows) {
      throw TestFailure('此 smoke 只允许在 Windows 桌面设备上运行');
    }
    final owned = await Directory.systemTemp.createTemp(
      'guossh-desktop-smoke-',
    );
    var rustStarted = false;
    const pickerChannel =
        'dev.flutter.pigeon.file_selector_windows.FileSelectorApi.showOpenDialog';
    final messenger = tester.binding.defaultBinaryMessenger;
    StreamSubscription? keyResults;
    try {
      // 与 M6 一致：先初始化桥；由启动门先订阅 AppReady / SettingsState，
      // 再将本轮独占目录交给 AppStart，收到 AppReady 后才查询设置。
      await WindowsDesktopWindow.initialize();
      await initializeRust(assignRustSignal);
      rustStarted = true;
      await tester.pumpWidget(
        GuoSSHellApp(
          targetOverride: AppTarget.desktop,
          startupOverride: _OwnedStartup(owned),
        ),
      );
      await _appears(tester, find.byType(WindowsShell));
      expect(find.byType(WindowsFrame), findsOneWidget);
      expect(WindowsDesktopWindow.initialized, isTrue);
      final shell = tester.state(find.byType(WindowsShell));
      expect(
        File('${owned.path}${Platform.pathSeparator}catalog.sqlite3')
            .existsSync(),
        isTrue,
      );

      // 从真实 Windows 工作区侧栏操作；只保存元数据，不打开会话或点击「连接」。
      await _appears(tester, find.textContaining('还没有保存的连接'));
      expect(find.text('0 个标签  ·  0 个窗格'), findsOneWidget);
      expect((await _readRustCatalog('smoke-initial')).connections, isEmpty);
      const gateway = RustDesktopCatalogGateway();
      gateway.query('');
      await tester.pump();

      await tester.tap(_iconButton('添加连接'));
      await _appears(tester, find.byType(DesktopConnectionEditor));
      await _field(tester, '名称', 'smoke-create');
      await _field(tester, '主机', 'localhost');
      await _field(tester, '用户名', 'smoke');
      // 不设置密码或私钥，避免对真实系统钥匙串写入。
      final savePassword = find.byKey(
        const fluent.Key('catalog-save-password'),
      );
      await tester.ensureVisible(savePassword);
      await tester.pumpAndSettle();
      expect(tester.widget<fluent.ToggleSwitch>(savePassword).checked, isTrue);
      // ToggleSwitch 的尾随说明不是点击区；点击真实开关轨道，禁止误保存密码。
      await tester.tapAt(
        tester.getTopLeft(savePassword) +
            fluent.Offset(20, tester.getSize(savePassword).height / 2),
      );
      await tester.pumpAndSettle();
      expect(tester.widget<fluent.ToggleSwitch>(savePassword).checked, isFalse);
      await tester.ensureVisible(find.text('保存'));
      await tester.tap(find.text('保存'));
      await _disappears(tester, find.byType(DesktopConnectionEditor));
      await _appears(tester, find.text('smoke-create'));
      final created = (await _readRustCatalog('smoke-create')).connections;
      expect(created, hasLength(1));
      final id = created.single.id;
      expect(id, isNotEmpty);
      expect(created.single.host, 'localhost');
      expect(created.single.username, 'smoke');
      expect(created.single.passwordSaved, isFalse);
      gateway.query('');
      await tester.pump();

      await _connectionAction(tester, '编辑');
      await _appears(tester, find.byType(DesktopConnectionEditor));
      await _field(tester, '名称', 'smoke-edited');
      await tester.ensureVisible(find.text('保存'));
      await tester.tap(find.text('保存'));
      await _disappears(tester, find.byType(DesktopConnectionEditor));
      await _appears(tester, find.text('smoke-edited'));
      final edited = (await _readRustCatalog('smoke-edited')).connections;
      expect(edited, hasLength(1));
      expect(edited.single.id, id);
      expect(edited.single.passwordSaved, isFalse);
      gateway.query('');
      await tester.pump();

      await _connectionAction(tester, '删除');
      await _appears(tester, find.text('删除连接？'));
      await tester.tap(find.text('取消'));
      await tester.pumpAndSettle();
      expect(find.text('smoke-edited'), findsOneWidget);
      await _connectionAction(tester, '删除');
      await _appears(tester, find.text('删除连接？'));
      await tester.tap(find.widgetWithText(fluent.FilledButton, '删除'));
      await _appears(tester, find.textContaining('还没有保存的连接'));
      expect((await _readRustCatalog('smoke-edited')).connections, isEmpty);

      // 只替代系统文件选择返回值；保留同一个真实工作区及产品私钥对话框。
      final invalidFile = File(
        '${owned.path}${Platform.pathSeparator}invalid-key.pem',
      );
      await invalidFile.writeAsBytes([0xff, 0xfe]);
      var fileSelections = 0;
      messenger.setMockMessageHandler(pickerChannel, (_) async {
        fileSelections++;
        // 已锁定 file_selector_windows 的 Pigeon 返回封套：[FileDialogResult]。
        // 131 是 FileDialogResult 标记；字段为 paths 与 typeGroupIndex。
        final reply = WriteBuffer()
          ..putUint8(12)
          ..putUint8(1)
          ..putUint8(131);
        const StandardMessageCodec().writeValue(reply, [
          [invalidFile.path],
          null,
        ]);
        return reply.done();
      });
      final results = <KeyResult>[];
      keyResults = KeyResult.rustSignalStream.listen(
        (pack) => results.add(pack.message),
      );
      await tester.tap(_iconButton('私钥管理'));
      await _appears(tester, find.byType(DesktopKeysPage));
      await tester.tap(find.widgetWithText(fluent.FilledButton, '导入私钥'));
      await _appears(tester, find.byType(DesktopKeyImportDialog));
      await tester.pumpAndSettle();
      await tester.tap(find.text('从文件选择'));
      await tester.pumpAndSettle();
      expect(fileSelections, 1);
      // 磁盘文件解码可抛 FileSystemException，内存文件可抛 FormatException。
      // 两条读取错误分支都不得把损坏输入送去导入；不据此声明私钥认证已验证。
      await _appears(
        tester,
        find.textContaining(RegExp('文件不是有效的 UTF-8 文本|无法读取文件，请确认文件可用后重试')),
      );
      final keys = KeyListState.rustSignalStream.first.timeout(
        const Duration(seconds: 15),
      );
      gateway.queryKeys();
      expect((await keys).message.keys, isEmpty);
      expect(results, isEmpty);
      expect(tester.state(find.byType(WindowsShell)), same(shell));
      expect(find.byType(WindowsFrame), findsOneWidget);
      expect(find.text('0 个标签  ·  0 个窗格'), findsOneWidget);
      expect(
        File('${owned.path}${Platform.pathSeparator}known_hosts').existsSync(),
        isFalse,
      );
      expect(tester.takeException(), isNull);
    } finally {
      await tester.pumpWidget(const fluent.SizedBox.shrink());
      await keyResults?.cancel();
      messenger.setMockMessageHandler(pickerChannel, null);
      // finalizeRust 同步等待 Rust runtime 退出，随后 Drop 关闭并 join SQLite worker。
      if (rustStarted) finalizeRust();
      // 只删除本轮 createTemp 返回的精确目录；不清理 HOME 或其他测试产物。
      await owned.delete(recursive: true);
      expect(await owned.exists(), isFalse);
    }
  });
}
