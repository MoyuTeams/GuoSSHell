#!/usr/bin/env python3
"""验证同一次构建的全部目标，上传草稿后再公开 GitHub Release。"""

import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

from metadata import CONFIG
from policy import require_release_source


def validate(directory, version, commit):
    expected = {target["id"] for target in CONFIG["targets"]}
    seen = set()
    files = {}
    build_numbers = set()
    for path in sorted(directory.glob("manifest-*.json")):
        manifest = json.loads(path.read_text())
        target = manifest["target"]
        if target not in expected or target in seen or path.name != f"manifest-{target}.json":
            raise ValueError("重复或未知目标清单")
        seen.add(target)
        if manifest["version"] != version or manifest["commit"] != commit:
            raise ValueError("产物版本或提交与当前 tag 不一致")
        build_numbers.add(manifest["build_number"])
        if target == "android" and manifest["signing"] != "release":
            raise ValueError("GitHub Release 不接受 Android 开发签名包")
        if not manifest["files"]:
            raise ValueError("目标没有产物")
        for item in manifest["files"]:
            name = item["name"]
            if Path(name).name != name or name in files or "\\" in name:
                raise ValueError("非法或重复的产物文件名")
            artifact = directory / name
            if not artifact.is_file() or artifact.is_symlink():
                raise ValueError(f"缺少产物：{name}")
            actual = hashlib.sha256(artifact.read_bytes()).hexdigest()
            if actual != item["sha256"] or artifact.stat().st_size != item["size"]:
                raise ValueError(f"产物校验失败：{name}")
            files[name] = actual
    if seen != expected or len(build_numbers) != 1:
        raise ValueError(f"构建矩阵不完整或混用了构建：缺少 {sorted(expected - seen)}")
    unexpected = {path.name for path in directory.iterdir()} - set(files) - {f"manifest-{target}.json" for target in seen}
    if unexpected:
        raise ValueError(f"产物目录包含清单之外的文件：{sorted(unexpected)}")
    return files


def gh(*args, **kwargs):
    return subprocess.run(["gh", *args], check=True, **kwargs)


def verify_tag(repository, tag, commit):
    actual = subprocess.check_output(
        ["gh", "api", f"repos/{repository}/commits/{tag}", "--jq", ".sha"], text=True
    ).strip()
    if actual != commit:
        raise RuntimeError("远端 tag 已移动，拒绝发布与标签不一致的构建")


def main():
    require_release_source()
    directory = Path(sys.argv[1]).resolve()
    version = os.environ["GUOSH_CI_VERSION"]
    commit = os.environ["GUOSH_CI_COMMIT"]
    repository = os.environ["GITHUB_REPOSITORY"]
    tag = f"v{version}"
    if os.environ.get("GITHUB_REF") != f"refs/tags/{tag}":
        raise RuntimeError("仅允许当前版本 tag 发布")
    verify_tag(repository, tag, commit)
    files = validate(directory, version, commit)
    checksums = directory / "SHA256SUMS.txt"
    checksums.write_text("".join(f"{digest}  {name}\n" for name, digest in sorted(files.items())))
    notes = directory.parent / "release-notes.md"
    notes.write_text(
        f"GuoSSHell {version}\n\n提交：`{commit}`\n\n"
        "- Android：发布密钥签名的分架构 APK，可直接安装。\n"
        "- Windows：x64 / arm64 便携包，解压后运行 guosh_shell.exe；未做 Authenticode 签名。\n"
        "- Linux：x64 / arm64 的 deb 与完整目录压缩包，桌面会话需要 Secret Service（如 GNOME Keyring）。\n"
        "- macOS：Intel 与 Apple Silicon 通用包，使用 ad-hoc 签名，未进行 Apple 公证。\n"
        "- iPhone / iPad：arm64 未签名 IPA，安装前需使用自己的 Apple 证书重新签名。\n"
        "- iOS 模拟器：arm64 / x64 的 debug App 包，供对应架构模拟器使用。\n\n"
        "下载后可用 SHA256SUMS.txt 校验文件；manifest 文件记录各平台的版本、提交、架构和签名状态。\n"
    )
    existing = subprocess.run(["gh", "release", "view", tag, "--repo", repository, "--json", "isDraft,targetCommitish"], capture_output=True, text=True)
    if existing.returncode == 0:
        if not json.loads(existing.stdout)["isDraft"]:
            raise RuntimeError("该版本已经公开发布，不覆盖已有发布资产")
    else:
        # 先验证标签确实存在；网络或权限失败不能被误判为可创建版本。
        gh("api", f"repos/{repository}/git/ref/tags/{tag}", stdout=subprocess.DEVNULL)
        gh("release", "create", tag, "--repo", repository, "--verify-tag", "--target", commit,
           "--draft", "--title", f"GuoSSHell {version}", "--notes-file", str(notes))
    assets = [str(path) for path in sorted(directory.iterdir())]
    gh("release", "upload", tag, *assets, "--repo", repository, "--clobber")
    prerelease = os.environ.get("GUOSH_CI_PRERELEASE") == "true"
    require_release_source()
    verify_tag(repository, tag, commit)
    gh("release", "edit", tag, "--repo", repository, "--draft=false",
       f"--prerelease={str(prerelease).lower()}", "--notes-file", str(notes))


if __name__ == "__main__":
    main()
