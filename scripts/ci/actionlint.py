#!/usr/bin/env python3
"""下载固定版本并核对校验值，再检查 GitHub Actions 工作流。"""

import hashlib
from pathlib import Path
import subprocess
import tarfile
import tempfile
import urllib.request

VERSION = "1.7.12"
SHA256 = "8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8"
URL = f"https://github.com/rhysd/actionlint/releases/download/v{VERSION}/actionlint_{VERSION}_linux_amd64.tar.gz"


if __name__ == "__main__":
    with tempfile.TemporaryDirectory() as directory:
        archive = Path(directory) / "actionlint.tar.gz"
        urllib.request.urlretrieve(URL, archive)
        if hashlib.sha256(archive.read_bytes()).hexdigest() != SHA256:
            raise RuntimeError("actionlint 校验值不一致")
        with tarfile.open(archive) as bundle:
            member = bundle.getmember("actionlint")
            bundle.extract(member, directory, filter="data")
        subprocess.run([str(Path(directory) / "actionlint"), "-color"], check=True)
