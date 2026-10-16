part of 'desktop_font.dart';

/// 字体结果所属的完整 Prepared 身份；configuration revision 不能代替它。
@immutable
class DesktopPreparedFontIdentity {
  const DesktopPreparedFontIdentity({
    required this.source,
    required this.prepareRequestId,
    required this.identity,
  });

  factory DesktopPreparedFontIdentity.fromResult(
    DesktopPreparationResult result,
  ) => DesktopPreparedFontIdentity(
    source: result.source,
    prepareRequestId: result.prepareRequestId,
    identity: result.identity,
  );

  final DesktopSource source;
  final int prepareRequestId;
  final String identity;

  @override
  bool operator ==(Object other) =>
      other is DesktopPreparedFontIdentity &&
      source == other.source &&
      prepareRequestId == other.prepareRequestId &&
      identity == other.identity;

  @override
  int get hashCode => Object.hash(source, prepareRequestId, identity);
}

/// 可消费的字体投影。只有 identity 非空的结果才属于原生 Prepared；legacy
/// 材料没有可用于 Start 的 projection。字号和期望 grid 来自有效 profile。
@immutable
class DesktopFontMaterial {
  const DesktopFontMaterial._({
    required this.identity,
    required this.family,
    required this.projection,
    required this.fontSize,
    required this.expectedCols,
    required this.expectedRows,
    required this.usedLoadFallback,
    required this.loadError,
    required this.snapshotFallbackReason,
  });

  final DesktopPreparedFontIdentity? identity;
  final String family;
  final String? projection;
  final double fontSize;
  final int expectedCols;
  final int expectedRows;
  final bool usedLoadFallback;
  final Object? loadError;
  final ProfileFontFailure? snapshotFallbackReason;

  /// pane 缩放仍由现有 TerminalZoom 管理，textScaler 不再乘进 fontSize，
  /// 避免 TerminalView 与度量重复缩放。
  DesktopFontLayout layout({
    double? zoomFontSize,
    TextScaler textScaler = TextScaler.noScaling,
  }) {
    final size = zoomFontSize ?? fontSize;
    _checkSize(size);
    return DesktopFontLayout._(
      style: TerminalStyle(
        fontSize: size,
        fontFamily: family,
        fontFamilyFallback: const [
          bundledFontFamily,
          'MiSans',
          'monospace',
          'Courier New',
        ],
      ),
      textScaler: textScaler,
      expectedCols: expectedCols,
      expectedRows: expectedRows,
    );
  }
}

/// 把 style / textScaler 原样交给 TerminalView；measureCellSize 只度量字符。
/// 期望 grid 不是真实 viewport geometry，Start 必须等实际布局的 resize。
@immutable
class DesktopFontLayout {
  const DesktopFontLayout._({
    required this.style,
    required this.textScaler,
    required this.expectedCols,
    required this.expectedRows,
  });

  final TerminalStyle style;
  final TextScaler textScaler;
  final int expectedCols;
  final int expectedRows;

  Size measureCellSize() => calcCharSize(style, textScaler);
}
