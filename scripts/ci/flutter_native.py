#!/usr/bin/env python3
"""官方未提供完整 SDK 压缩包的平台使用固定 Flutter 源码及其原生 Dart SDK。"""

import os
from pathlib import Path
import subprocess

from metadata import CONFIG


def main():
    root = Path(os.environ["RUNNER_TEMP"]) / "flutter-native"
    if not root.exists():
        subprocess.run(["git", "clone", "--depth", "1", "--branch", CONFIG["flutter"],
                        "https://github.com/flutter/flutter.git", str(root)], check=True)
    revision = subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip()
    if revision != CONFIG["flutter_revision"]:
        raise RuntimeError("Flutter 源码提交与固定版本不一致")
    windows = os.name == "nt"
    env = dict(os.environ)
    if windows:
        # Git Bash 可能是仿真进程，SDK 下载仍必须使用 runner 的原生架构。
        env["PROCESSOR_ARCHITECTURE"] = "ARM64"
        command = ["cmd.exe", "/d", "/c", str(root / "bin/flutter.bat")]
    else:
        command = [str(root / "bin/flutter")]
    subprocess.run([*command, "--version"], check=True, env=env)
    dart = root / "bin/cache/dart-sdk/bin" / ("dart.exe" if windows else "dart")
    version = subprocess.run([str(dart), "--version"], capture_output=True, text=True, check=True)
    expected = "windows_arm64" if windows else "linux_arm64"
    if expected not in version.stdout + version.stderr:
        raise RuntimeError(f"Dart SDK 必须原生运行于 {expected}，拒绝静默使用 x64 SDK")
    with open(os.environ["GITHUB_PATH"], "a", encoding="utf-8") as output:
        output.write(str(root / "bin") + "\n")
    with open(os.environ["GITHUB_ENV"], "a", encoding="utf-8") as output:
        output.write(f"FLUTTER_ROOT={root}\n")
        if windows:
            output.write("PROCESSOR_ARCHITECTURE=ARM64\n")


if __name__ == "__main__":
    main()
