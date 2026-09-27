import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/scheduler.dart';

import '../bindings/bindings.dart';

/// 一窗性能数据（M6 验收的度量，PLAN §5 M6 / 验收文档 §2.5）：Rust 的 PerfStats（出帧、
/// render / pack、显示延迟、ACK 超时、吞吐）+ 同一段时间里 Dart 的帧应用耗时与 Flutter
/// 帧耗时、进程内存。
@immutable
class PerfRecord {
  final int sessionId;
  final int windowMs;
  final int frames;
  final int renderUsAvg, renderUsMax, packUsAvg, packUsMax, bytesAvg;
  final int latencyUsP50, latencyUsP95, latencyUsMax;
  final int ackTimeouts, syncTimeouts, inputBytes;

  /// Dart 解码 + 填行池，微秒。
  final int applyUsAvg, applyUsMax;

  /// Flutter 帧（全 App，不分窗格）：build / raster 耗时，卡顿帧数（总耗时超过 16.7 ms）。
  final int flutterFrames, buildUsP90, buildUsMax, rasterUsP90, rasterUsMax, janky;
  final int rssBytes;

  const PerfRecord({
    required this.sessionId,
    required this.windowMs,
    required this.frames,
    required this.renderUsAvg,
    required this.renderUsMax,
    required this.packUsAvg,
    required this.packUsMax,
    required this.bytesAvg,
    required this.latencyUsP50,
    required this.latencyUsP95,
    required this.latencyUsMax,
    required this.ackTimeouts,
    required this.syncTimeouts,
    required this.inputBytes,
    required this.applyUsAvg,
    required this.applyUsMax,
    required this.flutterFrames,
    required this.buildUsP90,
    required this.buildUsMax,
    required this.rasterUsP90,
    required this.rasterUsMax,
    required this.janky,
    required this.rssBytes,
  });

  double get fps => windowMs == 0 ? 0 : frames * 1000 / windowMs;

  Map<String, Object> toJson() => {
        'session': sessionId,
        'window_ms': windowMs,
        'fps': double.parse(fps.toStringAsFixed(1)),
        'render_us': [renderUsAvg, renderUsMax],
        'pack_us': [packUsAvg, packUsMax],
        'bytes_avg': bytesAvg,
        'latency_us': [latencyUsP50, latencyUsP95, latencyUsMax],
        'ack_timeouts': ackTimeouts,
        'sync_timeouts': syncTimeouts,
        'input_bytes': inputBytes,
        'apply_us': [applyUsAvg, applyUsMax],
        'flutter_frames': flutterFrames,
        'build_us': [buildUsP90, buildUsMax],
        'raster_us': [rasterUsP90, rasterUsMax],
        'janky': janky,
        'rss_mb': (rssBytes / (1 << 20)).round(),
      };
}

/// 收集性能数据：Rust 每 5 秒一条 PerfStats，这里补上同一段时间里 Dart 与 Flutter 的数据，
/// 一窗一行 JSON 打进日志（`[m6-perf] {…}`，debug 与 profile 构建），集成测试直接读 [records]。
class PerfMonitor {
  PerfMonitor._() {
    if (!kReleaseMode) {
      SchedulerBinding.instance.addTimingsCallback(_onTimings);
    }
  }

  static final PerfMonitor instance = PerfMonitor._();

  /// 卡顿：一帧的总耗时超过 60 Hz 的一个刷新周期。
  static const int jankUs = 16667;

  final List<PerfRecord> records = [];
  final Map<int, _ApplyWindow> _apply = {};
  final List<FrameTiming> _timings = [];

  /// 一帧解码 + 填行池的耗时（窗格在处理完一帧时报）。
  void recordApply(int sessionId, int micros) {
    (_apply[sessionId] ??= _ApplyWindow()).add(micros);
  }

  void _onTimings(List<FrameTiming> timings) {
    _timings.addAll(timings);
    if (_timings.length > 4096) _timings.removeRange(0, _timings.length - 4096);
  }

  /// Rust 的一窗汇总到了：拼上 Dart 这边的数据。
  PerfRecord report(PerfStats stats) {
    final apply = _apply.remove(stats.sessionId) ?? _ApplyWindow();
    final build = [for (final t in _timings) t.buildDuration.inMicroseconds]..sort();
    final raster = [for (final t in _timings) t.rasterDuration.inMicroseconds]..sort();
    final janky = _timings.where((t) => t.totalSpan.inMicroseconds > jankUs).length;
    final record = PerfRecord(
      sessionId: stats.sessionId,
      windowMs: stats.windowMs,
      frames: stats.frames,
      renderUsAvg: stats.renderUsAvg,
      renderUsMax: stats.renderUsMax,
      packUsAvg: stats.packUsAvg,
      packUsMax: stats.packUsMax,
      bytesAvg: stats.bytesAvg,
      latencyUsP50: stats.latencyUsP50,
      latencyUsP95: stats.latencyUsP95,
      latencyUsMax: stats.latencyUsMax,
      ackTimeouts: stats.ackTimeouts,
      syncTimeouts: stats.syncTimeouts,
      inputBytes: stats.inputBytes.toBigInt().toInt(),
      applyUsAvg: apply.average,
      applyUsMax: apply.max,
      flutterFrames: _timings.length,
      buildUsP90: _percentile(build, 90),
      buildUsMax: build.isEmpty ? 0 : build.last,
      rasterUsP90: _percentile(raster, 90),
      rasterUsMax: raster.isEmpty ? 0 : raster.last,
      janky: janky,
      rssBytes: ProcessInfo.currentRss,
    );
    _timings.clear();
    records.add(record);
    if (records.length > 2000) records.removeRange(0, records.length - 2000);
    if (!kReleaseMode) debugPrint('[m6-perf] ${jsonEncode(record.toJson())}');
    return record;
  }

  static int _percentile(List<int> sorted, int p) =>
      sorted.isEmpty ? 0 : sorted[((sorted.length - 1) * p / 100).floor()];
}

class _ApplyWindow {
  int _count = 0;
  int _total = 0;
  int max = 0;

  void add(int micros) {
    _count += 1;
    _total += micros;
    if (micros > max) max = micros;
  }

  int get average => _count == 0 ? 0 : _total ~/ _count;
}
