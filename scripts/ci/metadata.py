#!/usr/bin/env python3
"""校验版本标签，统一所有平台的版本、提交和构建矩阵。"""

import json
import os
import re
import subprocess
import sys
import time

from policy import ROOT, build_allowed, verify_checkout, version_tag

CONFIG = json.loads((ROOT / "scripts/ci/config.json").read_text())


def metadata(ref, sha, pubspec, now=None):
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise ValueError("提交标识必须是完整 SHA")
    release = ref.startswith("refs/tags/")
    prerelease = False
    if release:
        tag = ref.removeprefix("refs/tags/")
        matched = version_tag(tag)
        suffix = matched.group(4)
        build_name = ".".join(matched.groups()[:3])
        version = tag[1:]
        prerelease = bool(suffix)
    else:
        matched = re.search(r"^version:\s*([0-9]+\.[0-9]+\.[0-9]+)", pubspec, re.M)
        if not matched:
            raise ValueError("pubspec.yaml 缺少版本号")
        build_name = matched.group(1)
        version = f"{build_name}-dev.{sha[:12]}"
    # 两个仓库使用同一时间基准，避免各自的 run_number 使 Android 更新版本倒退。
    number = int(time.time() if now is None else now) - 1577836800
    if not 1 <= number < 2_000_000_000:
        raise ValueError("构建号超出移动平台允许范围")
    return {
        "version": version,
        "build_name": build_name,
        "build_number": str(number),
        "release": str(release).lower(),
        "prerelease": str(prerelease).lower(),
        "commit": sha,
        "matrix": json.dumps({"include": [row for row in CONFIG["targets"] if row["platform"] != "android"]}, separators=(",", ":")),
        "android_matrix": json.dumps({"include": [row for row in CONFIG["targets"] if row["platform"] == "android"]}, separators=(",", ":")),
    }


def emit(values):
    text = "".join(f"{key}={value}\n" for key, value in values.items())
    if "GITHUB_OUTPUT" in os.environ:
        with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
            output.write(text)
    else:
        print(text, end="")


if __name__ == "__main__":
    if sys.argv[1:] == ["tools"]:
        emit({key: value for key, value in CONFIG.items() if key != "targets"})
    else:
        commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
        if os.environ.get("GITHUB_SHA"):
            verify_checkout(os.environ["GITHUB_SHA"])
        event = os.getenv("GITHUB_EVENT_NAME", "local")
        ref = os.getenv("GITHUB_REF", "refs/heads/local")
        allowed = build_allowed(event, ref, commit)
        values = metadata(ref, commit, (ROOT / "pubspec.yaml").read_text())
        values["build_allowed"] = str(allowed).lower()
        values["release"] = str(allowed and event == "push" and ref.startswith("refs/tags/")).lower()
        emit(values)
