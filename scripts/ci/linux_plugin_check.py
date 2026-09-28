"""编译并运行真实 GTK/Flutter 插件注册回归；无显示环境时用 xvfb-run 调用。"""

import argparse
import os
from pathlib import Path
import shlex
import shutil
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--engine", type=Path, help="Flutter Linux 引擎目录")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    engine = args.engine
    if engine is None:
        flutter = shutil.which("flutter")
        if not flutter:
            raise SystemExit("找不到 Flutter SDK")
        sdk = Path(flutter).resolve().parents[1]
        engine = sdk / "bin/cache/artifacts/engine/linux-x64"
    engine = engine.resolve()
    if not (engine / "libflutter_linux_gtk.so").is_file():
        raise SystemExit("缺少 Linux 引擎，请先执行 flutter precache --linux")
    output = root / "build/linux-plugin-check"
    output.mkdir(parents=True, exist_ok=True)
    binary = output / "registration-test"
    flags = shlex.split(subprocess.check_output(
        ["pkg-config", "--cflags", "--libs", "gtk+-3.0"], text=True))
    sources = [root / "test/native/linux_plugin_registration_test.cc"]
    includes = [f"-I{engine}"]
    for package, filename in [
        ("window_manager", "window_manager_plugin.cc"),
        ("flutter_acrylic", "flutter_acrylic_plugin.cc"),
    ]:
        directory = root / "packages" / package / "linux"
        sources.append(directory / filename)
        includes.append(f"-I{directory / 'include'}")
    subprocess.run([
        os.environ.get("CXX", "g++"), "-std=c++14", "-Wall", "-Werror",
        *includes, *map(str, sources), *flags, f"-L{engine}",
        f"-Wl,-rpath,{engine}", "-lflutter_linux_gtk", "-o", str(binary),
    ], check=True)
    subprocess.run([str(binary)], check=True)


if __name__ == "__main__":
    main()
