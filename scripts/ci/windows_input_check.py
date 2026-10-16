"""启动隔离的环回 SSH 服务，验证 Windows 原生终端输入；日志保留在 build 下。"""

import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import time


def main():
    if os.name != "nt":
        raise SystemExit("此验收需要 Windows")
    root = Path(__file__).resolve().parents[2]
    output = root / "build/windows-input-check"
    output.mkdir(parents=True, exist_ok=True)
    directory = Path(tempfile.mkdtemp(prefix="run-", dir=output))
    subprocess.run(["cargo", "build", "--locked", "-p", "rshell-m0", "--example", "demo_server"],
                   cwd=root, check=True)
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"], cwd=root, text=True))
    server = Path(metadata["target_directory"]) / "debug/examples/demo_server.exe"
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
    environment = os.environ.copy()
    environment.update({
        "GUOSH_HOST": "127.0.0.1", "GUOSH_PORT": str(port),
        "GUOSH_USER": "probe", "GUOSH_PASS": "", "GUOSH_CMD": "",
        "GUOSH_DEMO_DATA": str(directory / "server"),
        "GUOSH_DEMO_INPUT_LOG": str(directory / "received.txt"),
        "GUOSH_DESKTOP_TEST_DATA": str(directory / "app"),
    })
    flutter = shutil.which("flutter")
    if not flutter:
        raise SystemExit("找不到 Flutter SDK")
    with (directory / "server.log").open("w", encoding="utf-8") as log:
        process = subprocess.Popen([str(server), str(port)], cwd=root, env=environment,
                                   stdout=log, stderr=subprocess.STDOUT,
                                   creationflags=subprocess.CREATE_NO_WINDOW)
        try:
            deadline = time.monotonic() + 30
            while True:
                if process.poll() is not None or time.monotonic() > deadline:
                    raise RuntimeError(f"测试服务未就绪，参见 {directory / 'server.log'}")
                try:
                    with socket.create_connection(("127.0.0.1", port), timeout=0.2):
                        break
                except OSError:
                    time.sleep(0.1)
            subprocess.run([
                flutter, "test", "integration_test/windows_terminal_input_test.dart",
                "-d", "windows", "--no-pub", "--reporter", "expanded",
            ], cwd=root, env=environment, check=True)
        finally:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()


if __name__ == "__main__":
    main()
