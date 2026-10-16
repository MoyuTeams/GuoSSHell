#!/usr/bin/env python3
"""构建完整 Flutter 应用，核对架构并生成可供发版验证的产物清单。"""

import hashlib
import json
import os
from pathlib import Path
import plistlib
import shutil
import struct
import sys
import subprocess
import tarfile
import zipfile

from metadata import CONFIG, ROOT
from policy import require_build_source


def run(*args, **kwargs):
    args = [str(arg) for arg in args]
    executable = shutil.which(args[0])
    if os.name == "nt" and executable and executable.lower().endswith((".bat", ".cmd")):
        args = ["cmd.exe", "/d", "/c", executable, *args[1:]]
    subprocess.run(args, check=True, **kwargs)


def archive_zip(source, destination):
    # ditto 保留 Apple bundle 中的符号链接、权限与框架目录结构。
    if sys.platform == "darwin":
        run("ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", source, destination)
    else:
        with zipfile.ZipFile(destination, "w", zipfile.ZIP_DEFLATED) as archive:
            for path in sorted(source.rglob("*")):
                if path.is_file():
                    archive.write(path, path.relative_to(source.parent))


def check_binary(path, platform, arch):
    with open(path, "rb") as binary:
        header = binary.read(64)
        if platform == "linux":
            expected = {"x64": 62, "arm64": 183}[arch]
            if header[:4] != b"\x7fELF" or struct.unpack("<H", header[18:20])[0] != expected:
                raise RuntimeError(f"ELF 架构不符：{path}")
        elif platform == "windows":
            expected = {"x64": 0x8664, "arm64": 0xAA64}[arch]
            if header[:2] != b"MZ":
                raise RuntimeError(f"缺少 PE 文件头：{path}")
            binary.seek(struct.unpack("<I", header[60:64])[0])
            pe = binary.read(6)
            if pe[:4] != b"PE\0\0" or struct.unpack("<H", pe[4:6])[0] != expected:
                raise RuntimeError(f"PE 架构不符：{path}")
        else:
            actual = subprocess.check_output(["lipo", "-archs", str(path)], text=True).split()
            expected = {"arm64", "x86_64"} if arch == "universal" else {"x86_64" if arch == "x64" else arch}
            if set(actual) != expected:
                raise RuntimeError(f"Mach-O 架构不符：{path}，实际 {actual}")


def package_deb(bundle, output, version, arch):
    root = ROOT / "build/ci/deb"
    shutil.rmtree(root, ignore_errors=True)
    app = root / "usr/lib/guosshell"
    shutil.copytree(bundle, app, symlinks=True)
    (root / "usr/bin").mkdir(parents=True)
    (root / "usr/bin/guosshell").symlink_to("../lib/guosshell/guosh_shell")
    desktop = root / "usr/share/applications/guosshell.desktop"
    desktop.parent.mkdir(parents=True)
    # StartupWMClass 即 linux/CMakeLists.txt 的 APPLICATION_ID，桌面环境据此把运行中的窗口对应到本条目与图标。
    desktop.write_text(
        "[Desktop Entry]\nType=Application\nName=GuoSSHell\nExec=guosshell\nIcon=guosshell\n"
        "StartupWMClass=com.guosshell.guosh_shell\nTerminal=false\nCategories=Network;RemoteAccess;\n"
    )
    shutil.copytree(ROOT / "linux/icons", root / "usr/share/icons/hicolor")
    control = root / "DEBIAN/control"
    control.parent.mkdir()
    control.write_text(
        f"Package: guosshell\nVersion: {version.replace('-', '~', 1)}\n"
        f"Architecture: {'amd64' if arch == 'x64' else 'arm64'}\n"
        "Maintainer: GuoSSHell contributors\nSection: net\nPriority: optional\n"
        "Depends: libgtk-3-0t64 | libgtk-3-0, libstdc++6, libsecret-1-0, dbus-user-session, gnome-keyring\n"
        "Description: GuoSSHell SSH terminal client\n"
    )
    run("dpkg-deb", "--root-owner-group", "--build", root, output)


def main():
    os.chdir(ROOT)
    require_build_source()
    target = next(row for row in CONFIG["targets"] if row["id"] == os.environ["GUOSH_CI_TARGET"])
    platform, arch = target["platform"], target["arch"]
    version = os.environ["GUOSH_CI_VERSION"]
    dist = ROOT / "dist"
    dist.mkdir(exist_ok=True)
    if any(dist.iterdir()):
        raise RuntimeError("dist 必须为空，避免混入其他构建的产物")
    options = ["--no-pub", "--build-name", os.environ["GUOSH_CI_BUILD_NAME"], "--build-number", os.environ["GUOSH_CI_BUILD_NUMBER"]]
    prefix = f"GuoSSHell-{version}-{target['id']}"
    signing = "unsigned"
    configuration = "release"
    if platform in ["ios", "ios-simulator", "macos"]:
        config = ROOT / "build/ci/unsigned.xcconfig"
        config.parent.mkdir(parents=True, exist_ok=True)
        architectures = "arm64 x86_64" if arch == "universal" else ("x86_64" if arch == "x64" else arch)
        config.write_text(
            "CODE_SIGNING_ALLOWED = NO\nCODE_SIGNING_REQUIRED = NO\nDEVELOPMENT_TEAM =\n"
            f"ARCHS = {architectures}\nONLY_ACTIVE_ARCH = NO\n"
        )
        os.environ["XCODE_XCCONFIG_FILE"] = str(config)
    if platform == "macos":
        run("flutter", "build", "macos", "--release", *options)
        applications = list((ROOT / "build/macos/Build/Products/Release").glob("*.app"))
        if len(applications) != 1:
            raise RuntimeError("macOS 构建目录必须包含唯一的应用 bundle")
        app = applications[0]
        with open(app / "Contents/Info.plist", "rb") as info:
            executable = plistlib.load(info)["CFBundleExecutable"]
        check_binary(app / "Contents/MacOS" / executable, platform, arch)
        run("codesign", "--force", "--deep", "--sign", "-", "--entitlements", "macos/Runner/Release.entitlements", app)
        run("codesign", "--verify", "--deep", "--strict", app)
        archive_zip(app, dist / f"{prefix}-adhoc.zip")
        signing = "ad-hoc"
    elif platform == "ios":
        run("flutter", "build", "ios", "--release", "--no-codesign", *options)
        app = ROOT / "build/ios/iphoneos/Runner.app"
        check_binary(app / "Runner", platform, arch)
        payload = ROOT / "build/ci/Payload"
        payload.mkdir(parents=True, exist_ok=True)
        shutil.copytree(app, payload / "Runner.app", dirs_exist_ok=True, symlinks=True)
        archive_zip(payload, dist / f"{prefix}-unsigned.ipa")
    elif platform == "ios-simulator":
        configuration = "debug"
        run("flutter", "build", "ios", "--simulator", "--debug", "--no-codesign", *options)
        app = ROOT / "build/ios/iphonesimulator/Runner.app"
        check_binary(app / "Runner", platform, arch)
        archive_zip(app, dist / f"{prefix}-debug.zip")
    elif platform == "windows":
        run("flutter", "build", "windows", "--release", *options)
        bundle = ROOT / f"build/windows/{arch}/runner/Release"
        check_binary(bundle / "guosh_shell.exe", platform, arch)
        check_binary(bundle / "hub.dll", platform, arch)
        archive_zip(bundle, dist / f"{prefix}-portable.zip")
    elif platform == "linux":
        run("flutter", "build", "linux", "--release", f"--target-platform=linux-{arch}", *options)
        bundle = ROOT / f"build/linux/{arch}/release/bundle"
        check_binary(bundle / "guosh_shell", platform, arch)
        check_binary(bundle / "lib/libhub.so", platform, arch)
        with tarfile.open(dist / f"{prefix}.tar.gz", "w:gz") as archive:
            archive.add(bundle, arcname="GuoSSHell")
        package_deb(bundle, dist / f"{prefix}.deb", version, arch)
    elif platform == "android":
        if not os.environ.get("ANDROID_KEYSTORE_PATH"):
            raise RuntimeError("CI 的 Android 构建必须使用发布签名")
        ndk = CONFIG["android_ndk"]
        sdk_root = Path(os.environ["ANDROID_HOME"])
        sdkmanager = sdk_root / "cmdline-tools/latest/bin/sdkmanager"
        if not sdkmanager.is_file():
            raise RuntimeError("ANDROID_HOME 下缺少 Android command-line tools")
        run(sdkmanager, f"ndk;{ndk}", "platforms;android-36", "build-tools;36.0.0", input="y\n" * 30, text=True)
        # 发布模式需要 Flutter 重新生成排除开发插件的 Android 注册表。
        # --no-pub 的上游限制：flutter/flutter#169336。
        android_options = [option for option in options if option != "--no-pub"]
        run("flutter", "build", "apk", "--release", "--split-per-abi", "--target-platform=android-arm,android-arm64,android-x64", *android_options)
        run("git", "diff", "--exit-code", "--", "Cargo.lock", "pubspec.lock")
        signing = "release" if os.environ.get("ANDROID_KEYSTORE_PATH") else "development"
        for abi in ["armeabi-v7a", "arm64-v8a", "x86_64"]:
            apk = ROOT / f"build/app/outputs/flutter-apk/app-{abi}-release.apk"
            with zipfile.ZipFile(apk) as archive:
                if f"lib/{abi}/libhub.so" not in archive.namelist():
                    raise RuntimeError(f"APK 缺少 Rust 库：{abi}")
            signer = sdk_root / "build-tools/36.0.0/apksigner"
            run(signer, "verify", "--verbose", apk)
            shutil.copyfile(apk, dist / f"GuoSSHell-{version}-android-{abi}-{signing}.apk")
    else:
        raise RuntimeError(f"未知平台：{platform}")
    files = [
        {"name": path.name, "size": path.stat().st_size, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
        for path in sorted(dist.iterdir()) if path.is_file()
    ]
    if not files:
        raise RuntimeError("构建没有生成应用产物")
    manifest = {"target": target["id"], "version": version, "commit": os.environ["GUOSH_CI_COMMIT"],
                "build_number": os.environ["GUOSH_CI_BUILD_NUMBER"], "configuration": configuration, "signing": signing, "files": files}
    (dist / f"manifest-{target['id']}.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n")
    print(f"已生成 {target['id']}：{len(files)} 个应用产物")


if __name__ == "__main__":
    main()
