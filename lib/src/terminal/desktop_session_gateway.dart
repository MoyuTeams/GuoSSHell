import 'package:rinf/rinf.dart';

import '../bindings/bindings.dart';

/// 每个 pane 先安装所有来源订阅，再发 Prepare；正常路径不订阅旧裸广播。
abstract interface class DesktopSessionGateway {
  Stream<RustSignalPack<DesktopPreparationResult>> get preparations;
  Stream<RustSignalPack<DesktopFrame>> get frames;
  Stream<RustSignalPack<DesktopSessionStatus>> get statuses;
  Stream<RustSignalPack<DesktopPrompt>> get prompts;
  Stream<RustSignalPack<DesktopSelection>> get selections;
  Stream<RustSignalPack<DesktopNativeProjection>> get projections;
  Stream<RustSignalPack<DesktopNativeInvalidation>> get invalidations;
  Stream<RustSignalPack<DesktopNativeActionReady>> get actions;
  Stream<RustSignalPack<DesktopNativeCommandResult>> get commandResults;
  Stream<RustSignalPack<DesktopNativeLifecycle>> get lifecycles;
  Stream<RustSignalPack<DesktopNativeLifecycleQueryResult>>
  get lifecycleQueries;
  Stream<RustSignalPack<DesktopNativeTextEndRequired>> get textEnds;
  Stream<RustSignalPack<DesktopNativeContextMenuReady>> get contextMenus;
  Stream<RustSignalPack<DesktopEventResult>> get eventResults;
  Stream<RustSignalPack<DesktopClipboard>> get clipboards;
  void prepare(DesktopPrepare request);
  void start(DesktopStart request);
  void cancel(DesktopCancel request);
  void command(DesktopNativeCommand request);
  void ordered(DesktopOrdered request);
  void reply(DesktopReply request);
  void queryLifecycle(DesktopNativeLifecycleQuery request);
}

class RustDesktopSessionGateway implements DesktopSessionGateway {
  const RustDesktopSessionGateway();
  @override
  get preparations => DesktopPreparationResult.rustSignalStream;
  @override
  get frames => DesktopFrame.rustSignalStream;
  @override
  get statuses => DesktopSessionStatus.rustSignalStream;
  @override
  get prompts => DesktopPrompt.rustSignalStream;
  @override
  get selections => DesktopSelection.rustSignalStream;
  @override
  get projections => DesktopNativeProjection.rustSignalStream;
  @override
  get invalidations => DesktopNativeInvalidation.rustSignalStream;
  @override
  get actions => DesktopNativeActionReady.rustSignalStream;
  @override
  get commandResults => DesktopNativeCommandResult.rustSignalStream;
  @override
  get lifecycles => DesktopNativeLifecycle.rustSignalStream;
  @override
  get lifecycleQueries => DesktopNativeLifecycleQueryResult.rustSignalStream;
  @override
  get textEnds => DesktopNativeTextEndRequired.rustSignalStream;
  @override
  get contextMenus => DesktopNativeContextMenuReady.rustSignalStream;
  @override
  get eventResults => DesktopEventResult.rustSignalStream;
  @override
  get clipboards => DesktopClipboard.rustSignalStream;
  @override
  void prepare(DesktopPrepare request) => request.sendSignalToRust();
  @override
  void start(DesktopStart request) => request.sendSignalToRust();
  @override
  void cancel(DesktopCancel request) => request.sendSignalToRust();
  @override
  void command(DesktopNativeCommand request) => request.sendSignalToRust();
  @override
  void ordered(DesktopOrdered request) => request.sendSignalToRust();
  @override
  void reply(DesktopReply request) => request.sendSignalToRust();
  @override
  void queryLifecycle(DesktopNativeLifecycleQuery request) =>
      request.sendSignalToRust();
}
