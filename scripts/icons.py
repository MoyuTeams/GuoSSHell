#!/usr/bin/env python3
"""从 assets/icon/ 的矢量源文件生成各平台的应用图标。

light.svg 与 dark.svg 是同一幅图的亮色与暗色版本。iOS / iPadOS 18 起按系统外观
切换两者；macOS、Android、Windows 与 Linux 的图标格式没有外观变体，使用暗色版。
Android 13 起另有按壁纸主题色着色的单色层。各平台引用图标的配置文件（Contents.json、
自适应图标 XML 与背景色）不由本脚本生成。

依赖 rsvg-convert（librsvg）与 ImageMagick：python3 scripts/icons.py
"""

import math
import shutil
import struct
import subprocess
import tempfile
import xml.etree.ElementTree as ET
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "assets/icon"
SVG = "http://www.w3.org/2000/svg"
# 源图的背景矩形。Android 自适应图标的前景层去掉它，背景色由颜色资源提供。
SURFACE = "ri-mint-surface"
MAGICK = shutil.which("magick") or shutil.which("convert")

ET.register_namespace("", SVG)


def artwork(variant, x, y, size, background=True):
    """把源图缩放进 (x, y) 处边长为 size 的方框。"""
    root = ET.parse(SOURCE / f"{variant}.svg").getroot()
    if not background:
        parents = [parent for parent in root.iter() for child in parent if child.get("id") == SURFACE]
        if len(parents) != 1:
            raise RuntimeError(f"{variant}.svg 必须有唯一的背景矩形 #{SURFACE}")
        parents[0].remove(next(child for child in parents[0] if child.get("id") == SURFACE))
    for name in ("role", "aria-labelledby"):
        root.attrib.pop(name, None)
    root.attrib.update(x=str(x), y=str(y), width=str(size), height=str(size))
    return ET.tostring(root, encoding="unicode")


def document(size, content):
    return f'<svg xmlns="{SVG}" width="{size}" height="{size}" viewBox="0 0 {size} {size}">{content}</svg>'


def squircle(x, y, size, radius, smoothing=0.6):
    """连续圆角正方形（Figma 的圆角平滑算法；平滑度 60% 接近 Apple 图标的圆角）。"""
    p = (1 + smoothing) * radius
    arc = math.radians(90 * (1 - smoothing))
    chord = math.sin(arc / 2) * radius * math.sqrt(2)
    beta = math.radians(45 * smoothing)
    c = radius * math.tan((math.pi / 2 - arc) / 4) * math.cos(beta)
    d = c * math.tan(beta)
    b = (p - chord - c - d) / 3
    a = 2 * b
    r, e = radius, size
    return (
        f"M{x + e - p},{y}"
        f"c{a},0 {a + b},0 {a + b + c},{d}a{r},{r} 0 0 1 {chord},{chord}c{d},{c} {d},{b + c} {d},{a + b + c}"
        f"L{x + e},{y + e - p}"
        f"c0,{a} 0,{a + b} {-d},{a + b + c}a{r},{r} 0 0 1 {-chord},{chord}c{-c},{d} {-(b + c)},{d} {-(a + b + c)},{d}"
        f"L{x + p},{y + e}"
        f"c{-a},0 {-(a + b)},0 {-(a + b + c)},{-d}a{r},{r} 0 0 1 {-chord},{-chord}c{-d},{-c} {-d},{-(b + c)} {-d},{-(a + b + c)}"
        f"L{x},{y + p}"
        f"c0,{-a} 0,{-(a + b)} {d},{-(a + b + c)}a{r},{r} 0 0 1 {chord},{-chord}c{c},{-d} {b + c},{-d} {a + b + c},{-d}Z"
    )


def tile(variant, body, shadow=False):
    """源图裁成边长 body 的连续圆角方块，居中放在 1024 画布上。"""
    inset = (1024 - body) / 2
    outline = squircle(inset, inset, body, body * 0.225)
    content = ""
    if shadow:
        # Apple macOS 图标模板的投影：向下 10、模糊 5、黑色 30%。
        content = (
            '<filter id="shadow" x="-10%" y="-10%" width="120%" height="130%">'
            '<feGaussianBlur in="SourceAlpha" stdDeviation="5"/><feOffset dy="10"/>'
            '<feComponentTransfer><feFuncA type="linear" slope="0.3"/></feComponentTransfer></filter>'
            f'<path d="{outline}" filter="url(#shadow)"/>'
        )
    content += f'<clipPath id="tile"><path d="{outline}"/></clipPath>'
    content += f'<g clip-path="url(#tile)">{artwork(variant, inset, inset, body)}</g>'
    return document(1024, content)


def render(svg, pixels, path, opaque=False):
    """渲染成 pixels 见方的 PNG；opaque 时去掉透明通道。"""
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as temp:
        raw = Path(temp) / "raw.png"
        subprocess.run(["rsvg-convert", "-w", str(pixels), "-h", str(pixels), "-o", raw], input=svg.encode(), check=True)
        subprocess.run(
            [MAGICK, raw, *(["-alpha", "off"] if opaque else []), "-strip", "-define", "png:compression-level=9",
             f"{'PNG24' if opaque else 'PNG32'}:{path}"],
            check=True,
        )


def windows_icon(svg, sizes, path):
    """多尺寸 ICO：按惯例 256 像素的帧存 PNG，其余存 32 位 BMP。"""
    with tempfile.TemporaryDirectory() as temp:
        frames = [Path(temp) / f"{size}.png" for size in sizes]
        for size, frame in zip(sizes, frames):
            render(svg, size, frame)
        bitmaps = Path(temp) / "bitmaps.ico"
        subprocess.run([MAGICK, *frames, bitmaps], check=True)
        # ImageMagick 把每一帧都写成 BMP；目录项顺序与输入一致，256 像素的帧换回 PNG。
        data = bitmaps.read_bytes()
        entries, images = [], []
        for index, (size, frame) in enumerate(zip(sizes, frames)):
            entry = data[6 + 16 * index:6 + 16 * (index + 1)]
            length, offset = struct.unpack_from("<II", entry, 8)
            if entry[0] != size % 256:
                raise RuntimeError(f"ICO 第 {index} 帧不是 {size} 像素")
            entries.append(entry[:8])
            images.append(frame.read_bytes() if size >= 256 else data[offset:offset + length])
    directory, offset = b"", 6 + 16 * len(images)
    for entry, image in zip(entries, images):
        directory += entry + struct.pack("<II", len(image), offset)
        offset += len(image)
    path.write_bytes(data[:6] + directory + b"".join(images))


def monochrome(path):
    """Android 13 起主题图标的单色层：系统只取透明度，按壁纸主题色着色。

    构图与彩色前景一致；头壳实心、脸部面板挖空、眼睛实心，天线、身体与腮红用较浅的透明度区分。
    """
    paths = {
        element.get("id").removeprefix("ri-mint-"): element.get("d")
        for element in ET.parse(SOURCE / "dark.svg").getroot().iter(f"{{{SVG}}}path")
    }
    layers = (
        (0.55, "nonZero", ("antenna-left", "antenna-right", "body-left", "body-center", "body-right")),
        (1, "evenOdd", ("head-shell", "face-panel")),
        (1, "nonZero", ("terminal-chevron", "terminal-cursor")),
        (0.45, "nonZero", ("cheek-left", "cheek-right")),
    )
    body = "".join(
        f'\n        <path android:fillColor="#FFFFFFFF" android:fillAlpha="{alpha}" android:fillType="{rule}"\n'
        f'            android:pathData="{" ".join(paths[name] for name in names)}" />'
        for alpha, rule, names in layers
    )
    # 1254 的源图画布放在 108dp 图层中央 72dp 的可见区，与彩色前景重合。
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        '<?xml version="1.0" encoding="utf-8"?>\n'
        "<!-- 由 scripts/icons.py 从 assets/icon/dark.svg 生成，不要手改。 -->\n"
        '<vector xmlns:android="http://schemas.android.com/apk/res/android"\n'
        '    android:width="108dp" android:height="108dp"\n'
        '    android:viewportWidth="1881" android:viewportHeight="1881">\n'
        '    <group android:translateX="313.5" android:translateY="313.5">\n'
        '        <clip-path android:pathData="M0,0H1254V1254H0Z" />'
        f"{body}\n    </group>\n</vector>\n"
    )


def main():
    if not shutil.which("rsvg-convert") or not MAGICK:
        raise SystemExit("需要 rsvg-convert（librsvg）与 ImageMagick")

    # iOS / iPadOS：单一 1024 尺寸，亮色为默认外观，暗色对应深色模式；App Store 要求不含透明通道。
    ios = ROOT / "ios/Runner/Assets.xcassets/AppIcon.appiconset"
    render(document(1024, artwork("light", 0, 0, 1024)), 1024, ios / "Icon-App-1024x1024@1x.png", opaque=True)
    render(document(1024, artwork("dark", 0, 0, 1024)), 1024, ios / "Icon-App-Dark-1024x1024@1x.png", opaque=True)

    # macOS：Apple 图标模板的网格，1024 画布中央 824 的圆角方块，带投影。
    macos = tile("dark", 824, shadow=True)
    for size in (16, 32, 64, 128, 256, 512, 1024):
        render(macos, size, ROOT / f"macos/Runner/Assets.xcassets/AppIcon.appiconset/app_icon_{size}.png")

    # Windows、Linux 与 Android 7.1 及更早版本：占画布 7/8 的圆角方块。
    desktop = tile("dark", 896)
    # 自绘标题栏与 Windows 程序图标使用相同图案，64 像素兼顾高分屏。
    render(desktop, 64, SOURCE / "app_icon.png")
    windows_icon(desktop, (16, 20, 24, 32, 40, 48, 64, 96, 256), ROOT / "windows/runner/resources/app_icon.ico")
    for size in (16, 24, 32, 48, 64, 128, 256, 512):
        render(desktop, size, ROOT / f"linux/icons/{size}x{size}/apps/guosshell.png")

    # Android 8.0 起的自适应图标：前景是去掉背景的源图，占 108dp 图层中央 72dp 的可见区。
    foreground = document(108, artwork("dark", 18, 18, 72, background=False))
    for density, scale in {"mdpi": 1, "hdpi": 1.5, "xhdpi": 2, "xxhdpi": 3, "xxxhdpi": 4}.items():
        res = ROOT / f"android/app/src/main/res/mipmap-{density}"
        render(desktop, round(48 * scale), res / "ic_launcher.png")
        render(foreground, round(108 * scale), res / "ic_launcher_foreground.png")
    monochrome(ROOT / "android/app/src/main/res/drawable/ic_launcher_monochrome.xml")


if __name__ == "__main__":
    main()
