#!/usr/bin/env python3
"""M6：把集成测试的报告（build/m6/reports/*.json）与日志里的失败项汇总成一份 markdown。

  python3 scripts/m6-report.py build/m6/reports > build/m6/summary.md
"""

import json
import pathlib
import re
import sys


def ms(us):
    return "-" if us in (None, 0) else f"{us / 1000:.0f}"


def perf_cells(perf):
    if not perf or not perf.get("windows"):
        return ["-"] * 6
    return [
        f"{perf.get('fps', 0)}",
        ms(perf.get("latency_us_p95_max")),
        ms(perf.get("latency_us_max")),
        str(perf.get("ack_timeouts", 0)),
        f"{perf.get('janky', 0)}/{perf.get('flutter_frames', 0)}",
        str(perf.get("rss_mb_max", "-")),
    ]


def modes_ok(modes):
    if not modes or not modes.get("da1"):
        return "无应答"
    bad = [m for m, want in [("1049", 2), ("1000", 2), ("1002", 2), ("1003", 2), ("25", 1)] if modes.get(m) != want]
    return "干净" if not bad else "残留 " + ",".join(bad)


def failures(logs, name):
    log = logs / f"{name}.log"
    if not log.exists():
        return []
    return sorted(set(re.findall(r"Failure in method: (.+)", log.read_text(errors="replace"))))


def main() -> int:
    reports = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "build/m6/reports")
    logs = reports.parent / "logs"
    out = ["# M6 集成测试汇总", ""]
    for path in sorted(reports.glob("*.json")):
        data = json.loads(path.read_text())
        if not isinstance(data, dict):
            continue
        name = path.stem
        out += [f"## {name}", ""]
        failed = failures(logs, name)
        out += [f"失败：{'、'.join(failed) if failed else '无'}", ""]
        if "protocol" in data:
            p = data["protocol"]
            out += [
                f"- 宽字符画面一致性：{'通过' if not p.get('cjk_mismatches') else '不一致 ' + str(len(p['cjk_mismatches'])) + ' 处'}",
                f"- 同步输出无结束序列时显示用时：{p.get('sync_stall_visible_ms', '-')} ms",
                f"- 终端查询：{modes_ok(p.get('probe_modes'))}",
                f"- 按键编码：{len(p.get('keys') or {})} 个键，不一致 {len(p.get('keys_wrong') or [])} 个"
                + "".join(f"\n  - {w}" for w in p.get("keys_wrong") or []),
                f"- 鼠标悬停：1003 下上报 {len(p.get('hover_motion') or [])} 次，1002 下上报 {len(p.get('hover_drag') or [])} 次",
                "",
            ]
        if "tui" in data:
            out += [
                "| 程序 | fps | 延迟 p95 ms | 延迟 max ms | ACK 超时 | 卡顿帧 | RSS MB | 改尺寸后 | 不一致 | 退出后模式 | 滚回增长 |",
                "|---|---|---|---|---|---|---|---|---|---|---|",
            ]
            for tui, r in data["tui"].items():
                if not isinstance(r, dict):
                    continue
                out.append(
                    "| "
                    + " | ".join(
                        [tui, *perf_cells(r.get("running")), r.get("resized_to", "-"), str(r.get("resized_mismatches", "-")),
                         modes_ok(r.get("modes_after")), str(r.get("scrollback_growth", "-"))]
                    )
                    + " |"
                )
            out.append("")
        if "agents" in data:
            out += [
                "| agent × 场景 | 用时 s | 打字回显 ms | fps | 延迟 p95 ms | 延迟 max ms | ACK 超时 | 卡顿帧 | RSS MB | 不一致 | 乱码 | 退出后模式 |",
                "|---|---|---|---|---|---|---|---|---|---|---|---|",
            ]
            for case, r in data["agents"].items():
                if not isinstance(r, dict) or case == "panes":
                    continue
                out.append(
                    "| "
                    + " | ".join(
                        [case, f"{r.get('duration_ms', 0) / 1000:.1f}", str(r.get("typing_echo_ms", "-")),
                         *perf_cells(r.get("perf")), str(len(r.get("mismatches", []))), str(len(r.get("garbage", []))),
                         modes_ok(r.get("modes_after"))]
                    )
                    + " |"
                )
            out.append("")
            for case, r in data["agents"].items():
                perf = r.get("perf") if isinstance(r, dict) else None
                if case.endswith("/long") and isinstance(perf, dict) and perf.get("windows"):
                    out.append(
                        f"- S4 长会话（{case}）：RSS {perf.get('rss_mb_first')} → {perf.get('rss_mb_last')} MB"
                        f"（最高 {perf.get('rss_mb_max')}）"
                    )
            out.append("")
            panes = data["agents"].get("panes")
            if isinstance(panes, dict) and panes.get("echo_ms"):
                busy = panes.get("busy_perf", {})
                out += [
                    f"- 多窗格：shell 回显 p95 空闲时 {panes.get('echo_idle_ms_p95', '-')} ms，另一窗格 2000 行/秒输出时 "
                    f"{panes.get('echo_ms_p95')} ms（样本 {panes.get('echo_ms')}）；输出窗格 fps {busy.get('fps', '-')}，"
                    f"延迟 p95 {ms(busy.get('latency_us_p95_max'))} ms，ACK 超时 {busy.get('ack_timeouts', '-')}",
                    "",
                ]
        shots = [s.get("screenshotName") for s in data.get("screenshots", []) if isinstance(s, dict)]
        if shots:
            out += [f"截图（build/m6/screenshots/）：{len(shots)} 张", ""]
    print("\n".join(out))
    return 0


if __name__ == "__main__":
    sys.exit(main())
