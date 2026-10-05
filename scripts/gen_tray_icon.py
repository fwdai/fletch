#!/usr/bin/env python3
"""Generate the macOS menu-bar tray template icon.

The color app icon (src-tauri/icons/icon.png) is a dark rounded square with a
light bolt mark and a drop shadow; thresholding it to monochrome renders muddy.
So we redraw the same bolt cleanly as a *template image*: pure black pixels
with a varying alpha channel and nothing else, which is what macOS needs to
tint the icon for light/dark menu bars and click-highlight.

The bolt geometry is the two filled paths from the brand SVG (the same ones
`src/components/FletchMark.tsx` renders), flattened to polygons here so no SVG
rasterizer is needed.

Anti-aliasing: draw at 8x on a transparent canvas, then downscale with LANCZOS.
Both the ink (black) and the transparent background have RGB (0,0,0), so the
blended edge pixels keep RGB=0 and only alpha varies — the template invariant.

Outputs, both 44x44 (a @2x asset for the ~22pt menu bar; macOS scales the single
template image to fit):
  - src-tauri/icons/tray-macos-template.png   human-reviewable preview
  - src-tauri/icons/tray-macos-template.rgba  raw RGBA the app consumes via
    `Image::new` (no PNG-decode crate/feature needed at runtime)

Run: python3 scripts/gen_tray_icon.py
"""

import re
from pathlib import Path

from PIL import Image, ImageDraw

SIZE = 44          # @2x for a ~22pt menu bar
SCALE = 8          # supersample factor for anti-aliasing
MARK_HEIGHT = 30   # bolt height at 1x; leaves breathing room like system items

# Brand bolt, in the SVG's own coordinate space (see FletchMark.tsx).
BOLT_PATHS = (
    "M182.98 34.5L12.98 269C12.98 269 118.744 269.023 124.48 269"
    "C130.216 268.977 148.09 265.305 160.332 257C169.746 250.613 180.409 236.5 "
    "180.409 236.5L341.98 8.5H231.98C231.98 8.5 219.48 9 205.98 16"
    "C192.48 23 182.98 34.5 182.98 34.5Z",
    "M351.98 269L161.98 525.5V330.5C161.98 320.5 164.98 306 182.98 289.5"
    "C205.344 269 228.48 269 228.48 269H351.98Z",
)


def flatten_path(d: str, segments: int = 24) -> list[tuple[float, float]]:
    """Turn an absolute M/L/H/V/C/Z path into a polygon point list."""
    tokens = re.findall(r"[MLHVCZ]|-?\d*\.?\d+(?:e-?\d+)?", d)
    pts: list[tuple[float, float]] = []
    i = 0
    cmd = None
    x = y = 0.0
    while i < len(tokens):
        if tokens[i].isalpha():
            cmd = tokens[i]
            i += 1
            if cmd == "Z":
                continue
        if cmd in ("M", "L"):
            x, y = float(tokens[i]), float(tokens[i + 1])
            i += 2
            pts.append((x, y))
        elif cmd == "H":
            x = float(tokens[i])
            i += 1
            pts.append((x, y))
        elif cmd == "V":
            y = float(tokens[i])
            i += 1
            pts.append((x, y))
        elif cmd == "C":
            c1x, c1y, c2x, c2y, ex, ey = (float(t) for t in tokens[i : i + 6])
            i += 6
            for s in range(1, segments + 1):
                t = s / segments
                u = 1 - t
                pts.append((
                    u**3 * x + 3 * u**2 * t * c1x + 3 * u * t**2 * c2x + t**3 * ex,
                    u**3 * y + 3 * u**2 * t * c1y + 3 * u * t**2 * c2y + t**3 * ey,
                ))
            x, y = ex, ey
        else:
            raise ValueError(f"unsupported path command {cmd!r}")
    return pts


def main() -> None:
    polys = [flatten_path(d) for d in BOLT_PATHS]
    xs = [p[0] for poly in polys for p in poly]
    ys = [p[1] for poly in polys for p in poly]
    min_x, max_x, min_y, max_y = min(xs), max(xs), min(ys), max(ys)

    big = SIZE * SCALE
    k = MARK_HEIGHT * SCALE / (max_y - min_y)
    off_x = (big - (max_x - min_x) * k) / 2
    off_y = (big - (max_y - min_y) * k) / 2

    img = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)
    for poly in polys:
        draw.polygon(
            [((px - min_x) * k + off_x, (py - min_y) * k + off_y) for px, py in poly],
            fill=(0, 0, 0, 255),
        )

    # Downscale to the target size; LANCZOS anti-aliases the edges into alpha.
    small = img.resize((SIZE, SIZE), Image.LANCZOS)

    icons = Path(__file__).resolve().parent.parent / "src-tauri" / "icons"
    png_out = icons / "tray-macos-template.png"
    rgba_out = icons / "tray-macos-template.rgba"
    small.save(png_out)
    rgba_out.write_bytes(small.tobytes())  # width*height*4 bytes, row-major RGBA
    print(f"wrote {png_out} and {rgba_out} ({SIZE}x{SIZE})")


if __name__ == "__main__":
    main()
