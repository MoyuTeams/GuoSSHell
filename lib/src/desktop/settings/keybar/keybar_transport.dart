import '../../../bindings/bindings.dart';

abstract interface class KeyBarTransport {
  Stream<KeyBarLayoutResult> get results;
  void send(List<List<String>> rows);
}

/// 单一生成 DTO 通道；生命周期属于应用而不是编辑窗口。
class RinfKeyBarTransport implements KeyBarTransport {
  RinfKeyBarTransport._();
  static final instance = RinfKeyBarTransport._();

  @override
  Stream<KeyBarLayoutResult> get results =>
      KeyBarLayoutResult.rustSignalStream.map((pack) => pack.message);

  @override
  void send(List<List<String>> rows) =>
      SaveKeyBarLayout(rows: rows).sendSignalToRust();
}
