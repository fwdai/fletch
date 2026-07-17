#!/usr/bin/env python3
"""Generate the macOS menu-bar tray template icon.

The color app icon (src-tauri/icons) is a dark rounded square with a light
double-chevron "»" mark; thresholding it to monochrome renders muddy. So we
redraw the same double-chevron mark cleanly as a *template image*: pure black
pixels with a varying alpha channel and nothing else, which is what macOS needs
to tint the icon for light/dark menu bars and click-highlight.

Anti-aliasing: draw at 4x on a transparent canvas, then downscale with LANCZOS.
Both the ink (black) and the transparent background have RGB (0,0,0), so the
blended edge pixels keep RGB=0 and only alpha varies — the template invariant.

Outputs, both 44x44 (a @2x asset for the ~22pt menu bar; macOS scales the single
template image to fit):
  - src-tauri/icons/tray-macos-template.png   human-reviewable preview
  - src-tauri/icons/tray-macos-template.rgba  raw RGBA the app consumes via
    `Image::new` (no PNG-decode crate/feature needed at runtime)

Run: python3 scripts/gen_tray_icon.py
"""

from pathlib import Path

from PIL import Image, ImageDraw

SIZE = 44          # @2x for a ~22pt menu bar
SCALE = 4          # supersample factor for anti-aliasing
STROKE = 5         # chevron stroke width at 1x


def draw_chevron(draw, apex_x, left_x, y_top, y_mid, y_bottom, width):
    """A single '>' chevron: top-left -> right apex -> bottom-left."""
    draw.line(
        [(left_x, y_top), (apex_x, y_mid), (left_x, y_bottom)],
        fill=(0, 0, 0, 255),
        width=width,
        joint="curve",
    )


def main():
    big = SIZE * SCALE
    img = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)

    s = SCALE
    y_top, y_mid, y_bottom = 11 * s, 22 * s, 33 * s
    width = STROKE * s

    # Two chevrons, roughly centered horizontally, forming "»".
    draw_chevron(draw, apex_x=21 * s, left_x=11 * s, y_top=y_top, y_mid=y_mid,
                 y_bottom=y_bottom, width=width)
    draw_chevron(draw, apex_x=33 * s, left_x=23 * s, y_top=y_top, y_mid=y_mid,
                 y_bottom=y_bottom, width=width)

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
