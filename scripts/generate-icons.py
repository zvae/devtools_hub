#!/usr/bin/env python3
"""从 crates/devtools-hub/assets 下的原图生成各平台图标，生成的文件一并提交。

- logo.png     → AppIcon.icns：macOS 应用图标，按 Apple 图标网格裁成 824/1024 的圆角矩形；
               → icon.ico / icon.png：Windows 程序图标（build.rs 嵌入 exe）与窗口图标（256 像素）；
- menu_bar.png → tray.png：菜单栏 / 托盘图标，裁掉透明边后居中，运行时再缩放到所需尺寸。

需要 Pillow；生成 .icns 用的是 macOS 自带的 iconutil。
用法：python3 scripts/generate-icons.py
"""

import math
import subprocess
import tempfile
from pathlib import Path

from PIL import Image, ImageDraw

ASSETS = Path(__file__).resolve().parent.parent / "crates" / "devtools-hub" / "assets"


def load(name: str) -> Image.Image:
    image = Image.open(ASSETS / name).convert("RGBA")
    # 原图边缘有一圈几乎不可见的半透明噪点，清掉以免影响裁切和系统对图标形状的判断。
    alpha = image.getchannel("A").point(lambda a: 0 if a < 8 else a)
    image.putalpha(alpha)
    return image


def fit(image: Image.Image, canvas: int, body: float, stretch: bool = False) -> Image.Image:
    """把不透明主体缩放到宽高最大边为 `body` 像素，居中放到 `canvas` 见方的透明画布上。

    `stretch` 时宽高分别缩放，主体正好是 `body` 见方。
    """
    left, top, right, bottom = image.getchannel("A").point(lambda a: 255 if a > 128 else 0).getbbox()
    scale_x = scale_y = body / max(right - left, bottom - top)
    if stretch:
        scale_x, scale_y = body / (right - left), body / (bottom - top)
    scaled = image.resize(
        (round(image.width * scale_x), round(image.height * scale_y)), Image.Resampling.LANCZOS
    )
    center_x, center_y = (left + right) / 2 * scale_x, (top + bottom) / 2 * scale_y
    result = Image.new("RGBA", (canvas, canvas))
    result.paste(scaled, (round(canvas / 2 - center_x), round(canvas / 2 - center_y)))
    return result


def squircle(canvas: int, body: int, exponent: float = 5.0, supersample: int = 4) -> Image.Image:
    """Apple 图标网格的圆角矩形（指数为 5 的超椭圆）蒙版，按像素中心超采样后缩小抗锯齿。"""
    size, center, half = canvas * supersample, canvas * supersample / 2, body * supersample / 2
    mask = Image.new("L", (size, size))
    draw = ImageDraw.Draw(mask)
    for y in range(size):
        v = abs(y + 0.5 - center) / half
        if v > 1:
            continue
        reach = half * (1 - v**exponent) ** (1 / exponent)
        start, end = math.ceil(center - reach - 0.5), math.floor(center + reach - 0.5)
        if start <= end:
            draw.line([(start, y), (end, y)], fill=255)
    return mask.resize((canvas, canvas), Image.Resampling.LANCZOS)


def macos_icns(logo: Image.Image) -> None:
    # macOS 26 只原样显示正好填满系统圆角矩形、且其中完全不透明的图标，否则缩小后放进灰色底板。
    # 原图主体略扁且只有 99% 不透明，所以拉伸到正好 824 见方，下面垫一层放大的同一张图
    # 填满四角，再按系统的形状裁切。蒙版的大小或形状差几个像素都会被判为不合格。
    master = fit(logo, 1024, 824 * 1.1, stretch=True)
    master.alpha_composite(fit(logo, 1024, 824, stretch=True))
    master.putalpha(squircle(1024, 824))
    with tempfile.TemporaryDirectory() as tmp:
        iconset = Path(tmp) / "AppIcon.iconset"
        iconset.mkdir()
        for size in (16, 32, 128, 256, 512):
            for scale in (1, 2):
                suffix = "" if scale == 1 else "@2x"
                pixels = size * scale
                master.resize((pixels, pixels), Image.Resampling.LANCZOS).save(
                    iconset / f"icon_{size}x{size}{suffix}.png"
                )
        subprocess.run(
            ["iconutil", "-c", "icns", "-o", ASSETS / "AppIcon.icns", iconset], check=True
        )


def windows_icons(logo: Image.Image) -> None:
    # Windows 的图标几乎占满画布，只留一点边距。
    master = fit(logo, 256, 240)
    master.save(ASSETS / "icon.png", optimize=True)
    sizes = [(s, s) for s in (16, 20, 24, 32, 40, 48, 64, 256)]
    master.save(ASSETS / "icon.ico", sizes=sizes)


def tray(menu_bar: Image.Image) -> None:
    # 宽大于高的 V 形：左右留 4% 边距，上下自然留白；128 像素足够缩放到各 DPI 下的尺寸。
    fit(menu_bar, 128, 118).save(ASSETS / "tray.png", optimize=True)


if __name__ == "__main__":
    logo = load("logo.png")
    macos_icns(logo)
    windows_icons(logo)
    tray(load("menu_bar.png"))
    print(f"已生成图标：{ASSETS}")
