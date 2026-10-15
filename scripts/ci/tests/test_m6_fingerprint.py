"""执行真实 M6 编排脚本，用隔离工具替身验证指纹传递，不启动应用或设备。"""

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[2] / "m6.sh"
FINGERPRINT = "SHA256:" + "A" * 43
ENTRIES = (
    (["run", "protocol", "iphone-17"], 2),
    (["matrix", "protocol"], 8),
    (["macos", "protocol"], 2),
    (["device", "protocol", "fixture-device"], 1),
)


class M6FingerprintTests(unittest.TestCase):
    def setUp(self):
        # Windows 显式选择已安装的 Git Bash，不启动 WSL 或用户会话。
        self.bash = os.environ.get("M6_TEST_BASH")
        if not self.bash and os.name != "nt":
            self.bash = shutil.which("bash")
        if not self.bash:
            self.skipTest("需要已有 Bash；Windows 用 M6_TEST_BASH 指定 Git Bash")
        self.owned = tempfile.TemporaryDirectory(prefix="guosh-m6-script-")
        self.addCleanup(self.owned.cleanup)
        self.root = Path(self.owned.name)
        scripts = self.root / "scripts"
        scripts.mkdir()
        shutil.copyfile(SCRIPT, scripts / "m6.sh")
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.calls = self.root / "calls"
        self.trace = self.root / "trace"
        tools = {
            "flutter": 'printf "%s\\n" CALL "$@" END >> "$M6_CALLS"\necho "All tests passed"',
            "xcrun": 'echo \'{"devices":{}}\'',
            "python3": 'echo "fixture-udid"',
            "sysctl": 'echo "{ 0 0 0 }"',
            "plutil": "exit 0",
        }
        for name, body in tools.items():
            tool = self.bin / name
            tool.write_text(
                '#!/usr/bin/env bash\nprintf "%s\\n" "${0##*/}" >> "$M6_TRACE"\n'
                + body + "\n",
                encoding="utf-8",
                newline="\n",
            )
            tool.chmod(0o700)

    def run_entry(self, arguments, fingerprint=None):
        for output in (self.calls, self.trace):
            output.unlink(missing_ok=True)
        environment = {
            key: value for key, value in os.environ.items()
            if not key.startswith("M6_") and key not in ("BASH_ENV", "ENV")
        }
        environment.update({
            "HOME": self.root.as_posix(),
            "PATH": str(self.bin) + os.pathsep + os.environ["PATH"],
            "M6_CALLS": self.calls.as_posix(),
            "M6_TRACE": self.trace.as_posix(),
            "M6_HOST": "fixture.invalid",
            "M6_LOAD_MAX": "1",
        })
        if fingerprint is not None:
            environment["M6_HOST_KEY_SHA256"] = fingerprint
        tools = subprocess.run(
            [self.bash, "-c", "command -v flutter xcrun python3 sysctl plutil"],
            env=environment,
            cwd=self.root,
            capture_output=True,
            text=True,
            timeout=10,
        )
        self.assertEqual(tools.returncode, 0, tools.stderr)
        resolved = tools.stdout.splitlines()
        self.assertEqual(len(resolved), 5)
        for tool in resolved:
            self.assertIn(self.root.name + "/bin/", tool.replace("\\", "/"),
                          "工具替身未隔离，拒绝执行脚本")
        return subprocess.run(
            [self.bash, (self.root / "scripts" / "m6.sh").as_posix(), *arguments],
            env=environment,
            cwd=self.root,
            capture_output=True,
            text=True,
            encoding="utf-8",
            timeout=30,
        )

    def assert_forwarded(self, result, count):
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        calls = self.calls.read_text(encoding="utf-8").split("CALL\n")[1:]
        self.assertEqual(len(calls), count)
        for call in calls:
            # build/drive 必须都实际收到明确指纹，不能只写进日志或父进程环境。
            self.assertEqual(call.splitlines()[-2], "--dart-define=M6_HOST_KEY_SHA256=" + FINGERPRINT)

    def test_environment_fingerprint_reaches_every_build_and_drive(self):
        for entry, count in ENTRIES:
            with self.subTest(entry=entry):
                self.assert_forwarded(self.run_entry(entry, FINGERPRINT), count)

    def test_explicit_define_is_supported_by_single_target_entries(self):
        for entry, count in ENTRIES:
            if entry[0] == "matrix":
                continue
            for define in (
                ["--dart-define=M6_HOST_KEY_SHA256=" + FINGERPRINT],
                ["--dart-define", "M6_HOST_KEY_SHA256=" + FINGERPRINT],
            ):
                with self.subTest(entry=entry, define=define):
                    self.assert_forwarded(self.run_entry(entry + define), count)

    def test_missing_or_invalid_fingerprint_stops_before_tools_run(self):
        for entry, _ in ENTRIES:
            for fingerprint in (None, "", "SHA256:short", "SHA256:" + "!" * 43):
                with self.subTest(entry=entry, fingerprint=fingerprint):
                    result = self.run_entry(entry, fingerprint)
                    self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
                    self.assertFalse(self.trace.exists(), "拒绝后不得启动设备或构建工具")

    def test_conflicting_or_empty_override_is_rejected_before_tools_run(self):
        for value in ("SHA256:" + "B" * 43, ""):
            result = self.run_entry(
                ["run", "protocol", "iphone-17", "--dart-define=M6_HOST_KEY_SHA256=" + value],
                FINGERPRINT,
            )
            self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
            self.assertFalse(self.trace.exists())

    def test_simulator_listing_does_not_require_fingerprint(self):
        result = self.run_entry(["sims"])
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse(self.calls.exists(), "列举模拟器不应调用 Flutter")


if __name__ == "__main__":
    unittest.main()
