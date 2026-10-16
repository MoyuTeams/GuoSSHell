import 'package:file_selector/file_selector.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';

import '../../../settings/interface_font.dart';

enum DesktopFontSlot { interface, terminal }

/// 父持有的设置会话身份，不代表原生 source 身份或 wire 关联证明。
class DesktopResourceIdentity {
  const DesktopResourceIdentity(this.owner, this.generation);
  final Object owner;
  final int generation;
  bool matches(DesktopResourceIdentity other) =>
      identical(owner, other.owner) && generation == other.generation;
}

/// acquire 完成且 ready 为真后才能输入；原生焦点确认由父负责。
/// generation 在边界撤销时改变，使未完成的选择无法回到另一个焦点域。
/// release 必须令本轮未完成 acquire 失效，不能迟到重新占用原生焦点。
abstract interface class DesktopResourcesInputBoundary implements Listenable {
  bool get ready;
  int get generation;
  Future<void> acquire();
  void release();
}

enum DesktopResourceWriteStatus { confirmed, failed, uncertain }

/// confirmed 只用于父已关联并确认的持久化回包，不用于 preview 或 query。
class DesktopResourceWriteResult<T> {
  const DesktopResourceWriteResult.confirmed(T value)
    : status = DesktopResourceWriteStatus.confirmed,
      saved = value;
  const DesktopResourceWriteResult.failed()
    : status = DesktopResourceWriteStatus.failed,
      saved = null;
  const DesktopResourceWriteResult.uncertain()
    : status = DesktopResourceWriteStatus.uncertain,
      saved = null;
  final DesktopResourceWriteStatus status;
  final T? saved;
}

class DesktopFontSlotState {
  const DesktopFontSlotState({
    required this.savedFamily,
    required this.savedLabel,
    required this.effectiveFamily,
    this.fallbackReason,
    this.writeReady = true,
    this.verificationVersion = 0,
  });
  final String savedFamily;
  final String savedLabel;
  final String effectiveFamily;
  final String? fallbackReason;
  final bool writeReady;

  /// 父完成未决归属核实后递增；普通 query、重建 Widget 不得递增。
  final int verificationVersion;
}

class DesktopTtfPickRequest {
  const DesktopTtfPickRequest(this.slot);
  final DesktopFontSlot slot;
  static const typeMetadata = <XTypeGroup>[
    XTypeGroup(
      label: 'TrueType 字体',
      extensions: ['ttf'],
      mimeTypes: [
        'font/ttf',
        'application/x-font-ttf',
        'application/octet-stream',
      ],
      uniformTypeIdentifiers: ['public.font'],
    ),
  ];
}

typedef DesktopResourceTtfPicker = Future<XFile?> Function(
  BuildContext context,
  DesktopTtfPickRequest request,
);

/// 只有非持久 preview；复用原有 32 MiB 校验、Rust 校验、FontLoader 缓存。
/// 也可由父 profile picker 调用，不能以此替代 profile CAS 保存。
Future<FontDraft> prepareDesktopTtfPreview(
  InterfaceTypography typography,
  XFile file,
) => typography.prepare(file);

class DesktopFontResourceCallbacks {
  const DesktopFontResourceCallbacks({
    required this.identity,
    required this.slot,
    required this.state,
    required this.pick,
    required this.prepare,
    required this.apply,
    required this.reset,
  });
  final DesktopResourceIdentity identity;
  final DesktopFontSlot slot;
  final ValueListenable<DesktopFontSlotState> state;
  final DesktopResourceTtfPicker pick;
  final Future<FontDraft> Function(XFile file) prepare;

  /// 父共享 app writer 关联 request_id、slot 和操作，检查 applied/error。
  /// Widget 销毁不取消已发送写入；父继续持有未决请求及结果。
  final Future<DesktopResourceWriteResult<DesktopFontSlotState>> Function(
    FontDraft draft,
  )
  apply;
  final Future<DesktopResourceWriteResult<DesktopFontSlotState>> Function()
  reset;
}

class DesktopAppearanceValue {
  const DesktopAppearanceValue({required this.acrylic, required this.opacity});
  final bool acrylic;
  final double opacity;
  double get transparency => (1 - opacity) * 100;
}

class DesktopAppearanceState {
  const DesktopAppearanceState({
    required this.saved,
    required this.materialAvailable,
    this.writeReady = true,
    this.verificationVersion = 0,
  });
  final DesktopAppearanceValue saved;
  final bool materialAvailable;
  final bool writeReady;
  final int verificationVersion;
}

class DesktopAppearanceCallbacks {
  const DesktopAppearanceCallbacks({
    required this.identity,
    required this.state,
    required this.preview,
    required this.save,
    required this.restoreSavedPreview,
  });
  final DesktopResourceIdentity identity;
  final ValueListenable<DesktopAppearanceState> state;

  /// 父复用 WindowsAppearance.preview；高对比度宿主保持不透明。
  final ValueChanged<DesktopAppearanceValue> preview;

  /// 无 RID 协议由父确保全应用单写者；超时后保持未决隔离。
  /// failed 必须意味着已关联失败，uncertain 不能用 query 或重开排空。
  final Future<DesktopResourceWriteResult<DesktopAppearanceValue>> Function(
    DesktopAppearanceValue draft,
  )
  save;

  /// 回撤到父当前确认值，不删除本地失败草稿；dispose 也需释放预览。
  final VoidCallback restoreSavedPreview;
}

typedef DesktopLicenseRegistry = Stream<LicenseEntry> Function();
