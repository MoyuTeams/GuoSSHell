#!/usr/bin/env python3
"""只允许受信分支和已进入受信分支的版本标签构建、签名与发布。"""

import json
import os
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
TRUSTED_BRANCHES = ("main", "dev")
VERSION = r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
TAG = re.compile(r"v" + VERSION + r"(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?\Z")


def version_tag(tag):
    matched = TAG.fullmatch(tag)
    if not matched:
        raise ValueError("版本标签必须为 vX.Y.Z 或 vX.Y.Z-预发布标识")
    suffix = matched.group(4)
    if suffix and any(part.isdigit() and len(part) > 1 and part[0] == "0" for part in suffix.split(".")):
        raise ValueError("预发布数字标识不能包含前导零")
    return matched


def verify_checkout(commit, root=ROOT):
    if not re.fullmatch(r"[0-9a-f]{40}", commit):
        raise ValueError("提交标识必须是完整 SHA")
    actual = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()
    if actual != commit:
        raise RuntimeError("检出提交与工作流提交不一致")


def verify_trusted_commit(commit, root=ROOT):
    for branch in TRUSTED_BRANCHES:
        ref = f"refs/remotes/origin/{branch}"
        found = subprocess.run(["git", "show-ref", "--verify", "--quiet", ref], cwd=root)
        if found.returncode == 1:
            continue
        found.check_returncode()
        result = subprocess.run(["git", "merge-base", "--is-ancestor", commit, ref], cwd=root)
        if result.returncode == 0:
            return
        if result.returncode != 1:
            result.check_returncode()
    raise RuntimeError("版本标签的提交尚未进入当前仓库的 main 或 dev，拒绝构建、签名与发布")


def build_allowed(event, ref, commit, root=ROOT):
    # PR 的来源或目标分支同名也不能授权；只采用 GitHub 的事件和完整 ref。
    if event not in ("push", "workflow_dispatch"):
        return False
    if ref in {f"refs/heads/{branch}" for branch in TRUSTED_BRANCHES}:
        return True
    if event == "push" and ref.startswith("refs/tags/"):
        version_tag(ref.removeprefix("refs/tags/"))
        verify_trusted_commit(commit, root)
        return True
    return False


def require_build_source(environ=None, root=ROOT):
    environ = os.environ if environ is None else environ
    commit = environ.get("GUOSH_CI_COMMIT", "")
    verify_checkout(commit, root)
    if environ.get("GITHUB_SHA") != commit:
        raise RuntimeError("构建提交与 GitHub 事件的 SHA 不一致")
    if not build_allowed(environ.get("GITHUB_EVENT_NAME", ""), environ.get("GITHUB_REF", ""), commit, root):
        raise RuntimeError("当前事件或分支仅允许静态检查，拒绝构建与签名")


def require_release_source(environ=None, root=ROOT):
    environ = os.environ if environ is None else environ
    if environ.get("GITHUB_EVENT_NAME") != "push" or not environ.get("GITHUB_REF", "").startswith("refs/tags/"):
        raise RuntimeError("只有版本标签推送可以发布 GitHub Release")
    require_build_source(environ, root)


def check_results(jobs, event, ref):
    if any(jobs[name]["result"] != "success" for name in ("metadata", "checks")):
        raise RuntimeError("静态检查或来源门禁未通过")
    allowed = jobs["metadata"]["outputs"].get("build_allowed")
    if allowed not in ("true", "false"):
        raise RuntimeError("缺少有效的构建来源判定")
    eligible = event in ("push", "workflow_dispatch") and (
        ref in {f"refs/heads/{branch}" for branch in TRUSTED_BRANCHES}
        or (event == "push" and ref.startswith("refs/tags/v"))
    )
    if (allowed == "true") != eligible:
        raise RuntimeError("构建来源判定与原始 GitHub 事件不一致")
    expected = "success" if eligible else "skipped"
    if any(jobs[name]["result"] != expected for name in ("build", "android")):
        raise RuntimeError(f"构建与签名任务的结果必须为 {expected}")


if __name__ == "__main__":
    if sys.argv[1:] == ["results"]:
        check_results(json.loads(os.environ["RESULTS"]), os.environ["GITHUB_EVENT_NAME"], os.environ["GITHUB_REF"])
    elif sys.argv[1:] == ["release"]:
        require_release_source()
    else:
        require_build_source()
