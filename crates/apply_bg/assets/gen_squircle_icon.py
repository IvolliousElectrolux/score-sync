"""从底色图生成 iOS 风格连续圆角 (cubic Bezier squircle) 图标."""

from __future__ import annotations

import re
from pathlib import Path

from PIL import Image, ImageDraw

SRC = Path(r"D:\施纳贝尔\PEARL 贝多芬钢琴作品集 5厚盒11CD\曲谱同步\底色.png")
ASSETS = Path(__file__).resolve().parent
OUT_ICO = ASSETS / "app.ico"
OUT_PREVIEW = ASSETS / "app_preview.png"
OUT_SVG = ASSETS / "ios_squircle.svg"

# iOS / App Store 风格连续圆角: 四角由多段三次贝塞尔拼接 (viewBox 0..1024)
SQUIRCLE_D = (
    "M512 0C298.027 0 213.333.853 146.347 15.147 "
    "C79.68 29.227 32.64 57.813 11.947 119.04 "
    "C.853 185.173 0 269.867 0 512s.853 326.827 11.947 392.96"
    "c20.693 61.227 67.733 89.813 134.4 103.893"
    "C213.333 1023.147 298.027 1024 512 1024s298.667-.853 365.653-15.147"
    "c66.667-14.08 113.707-42.666 134.4-103.893"
    "C1023.147 838.827 1024 754.133 1024 512s-.853-326.827-11.947-392.96"
    "C991.36 57.813 944.32 29.227 877.653 15.147 "
    "C810.667.853 725.973 0 512 0z"
)


def lerp(a: float, b: float, t: float) -> float:
    return a + (b - a) * t


def cubic(
    p0: tuple[float, float],
    p1: tuple[float, float],
    p2: tuple[float, float],
    p3: tuple[float, float],
    t: float,
) -> tuple[float, float]:
    a = (lerp(p0[0], p1[0], t), lerp(p0[1], p1[1], t))
    b = (lerp(p1[0], p2[0], t), lerp(p1[1], p2[1], t))
    c = (lerp(p2[0], p3[0], t), lerp(p2[1], p3[1], t))
    d = (lerp(a[0], b[0], t), lerp(a[1], b[1], t))
    e = (lerp(b[0], c[0], t), lerp(b[1], c[1], t))
    return (lerp(d[0], e[0], t), lerp(d[1], e[1], t))


def tokenize(d: str) -> list[str]:
    return re.findall(r"[MmCcSsLlZz]|[-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?", d)


def path_to_points(d: str, samples_per_curve: int = 64) -> list[tuple[float, float]]:
    tokens = tokenize(d)
    i = 0
    pts: list[tuple[float, float]] = []
    cx = cy = 0.0
    start = (0.0, 0.0)
    last_c2: tuple[float, float] | None = None

    def num() -> float:
        nonlocal i
        v = float(tokens[i])
        i += 1
        return v

    while i < len(tokens):
        cmd = tokens[i]
        i += 1
        if cmd == "M":
            cx, cy = num(), num()
            start = (cx, cy)
            pts.append((cx, cy))
            last_c2 = None
            while i < len(tokens) and tokens[i] not in "MmCcSsLlZz":
                cx, cy = num(), num()
                pts.append((cx, cy))
                last_c2 = None
        elif cmd == "m":
            cx += num()
            cy += num()
            start = (cx, cy)
            pts.append((cx, cy))
            last_c2 = None
            while i < len(tokens) and tokens[i] not in "MmCcSsLlZz":
                cx += num()
                cy += num()
                pts.append((cx, cy))
                last_c2 = None
        elif cmd == "C":
            while i < len(tokens) and tokens[i] not in "MmCcSsLlZz":
                x1, y1, x2, y2, x, y = num(), num(), num(), num(), num(), num()
                p0, p1, p2, p3 = (cx, cy), (x1, y1), (x2, y2), (x, y)
                for k in range(1, samples_per_curve + 1):
                    pts.append(cubic(p0, p1, p2, p3, k / samples_per_curve))
                cx, cy = x, y
                last_c2 = (x2, y2)
        elif cmd == "c":
            while i < len(tokens) and tokens[i] not in "MmCcSsLlZz":
                x1, y1 = cx + num(), cy + num()
                x2, y2 = cx + num(), cy + num()
                x, y = cx + num(), cy + num()
                p0, p1, p2, p3 = (cx, cy), (x1, y1), (x2, y2), (x, y)
                for k in range(1, samples_per_curve + 1):
                    pts.append(cubic(p0, p1, p2, p3, k / samples_per_curve))
                cx, cy = x, y
                last_c2 = (x2, y2)
        elif cmd == "S":
            while i < len(tokens) and tokens[i] not in "MmCcSsLlZz":
                if last_c2 is None:
                    x1, y1 = cx, cy
                else:
                    x1, y1 = 2 * cx - last_c2[0], 2 * cy - last_c2[1]
                x2, y2, x, y = num(), num(), num(), num()
                p0, p1, p2, p3 = (cx, cy), (x1, y1), (x2, y2), (x, y)
                for k in range(1, samples_per_curve + 1):
                    pts.append(cubic(p0, p1, p2, p3, k / samples_per_curve))
                cx, cy = x, y
                last_c2 = (x2, y2)
        elif cmd == "s":
            while i < len(tokens) and tokens[i] not in "MmCcSsLlZz":
                if last_c2 is None:
                    x1, y1 = cx, cy
                else:
                    x1, y1 = 2 * cx - last_c2[0], 2 * cy - last_c2[1]
                x2, y2 = cx + num(), cy + num()
                x, y = cx + num(), cy + num()
                p0, p1, p2, p3 = (cx, cy), (x1, y1), (x2, y2), (x, y)
                for k in range(1, samples_per_curve + 1):
                    pts.append(cubic(p0, p1, p2, p3, k / samples_per_curve))
                cx, cy = x, y
                last_c2 = (x2, y2)
        elif cmd in ("L", "l"):
            rel = cmd == "l"
            while i < len(tokens) and tokens[i] not in "MmCcSsLlZz":
                if rel:
                    cx += num()
                    cy += num()
                else:
                    cx, cy = num(), num()
                pts.append((cx, cy))
                last_c2 = None
        elif cmd in ("Z", "z"):
            pts.append(start)
            cx, cy = start
            last_c2 = None
        else:
            raise ValueError(f"unsupported cmd {cmd}")
    return pts


def center_square(im: Image.Image) -> Image.Image:
    w, h = im.size
    side = min(w, h)
    left = (w - side) // 2
    top = (h - side) // 2
    return im.crop((left, top, left + side, top + side))


def make_icon(tex_square: Image.Image, size: int) -> Image.Image:
    ss = size * 4
    tex = tex_square.resize((ss, ss), Image.Resampling.LANCZOS)
    mask = Image.new("L", (ss, ss), 0)
    ImageDraw.Draw(mask).polygon(
        [(p[0] * ss / 1024.0, p[1] * ss / 1024.0) for p in path_to_points(SQUIRCLE_D, 64)],
        fill=255,
    )
    out = Image.new("RGBA", (ss, ss), (0, 0, 0, 0))
    out.paste(tex, (0, 0))
    out.putalpha(mask)
    return out.resize((size, size), Image.Resampling.LANCZOS)


def main() -> None:
    OUT_SVG.write_text(
        '<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" '
        f'viewBox="0 0 1024 1024">\n  <path fill="#fff" d="{SQUIRCLE_D}"/>\n</svg>\n',
        encoding="utf-8",
    )

    base = center_square(Image.open(SRC).convert("RGBA"))
    sizes = [256, 128, 64, 48, 32, 16]
    icons = [make_icon(base, s) for s in sizes]

    prev_bg = Image.new("RGBA", (icons[0].width + 40, icons[0].height + 40), (248, 250, 252, 255))
    prev_bg.paste(icons[0], (20, 20), icons[0])
    prev_bg.save(OUT_PREVIEW)

    icons[0].save(
        OUT_ICO,
        format="ICO",
        sizes=[(im.width, im.height) for im in icons],
        append_images=icons[1:],
    )
    print(f"wrote {OUT_ICO}")
    print(f"wrote {OUT_PREVIEW}")
    print(f"wrote {OUT_SVG}")


if __name__ == "__main__":
    main()
