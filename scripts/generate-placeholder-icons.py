#!/usr/bin/env python3
"""Generate placeholder PNG icons + .icns for Tauri bundle.

Writes solid-color PNGs at the sizes Tauri requires plus a macOS .icns
using `iconutil`. Replace with real icons before shipping.
"""
import os, struct, subprocess, sys, tempfile, zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT_DIR = ROOT / "src-tauri" / "icons"
OUT_DIR.mkdir(parents=True, exist_ok=True)

BG = (91, 141, 239)   # accent blue, matches app.css
FG = (255, 255, 255)


def write_png(path: Path, size: int):
    # Solid-colored square with a centered "a" glyph drawn as a pixel grid
    # (very crude, fine for a placeholder).
    pixels = [[BG for _ in range(size)] for _ in range(size)]

    # Draw a chunky 'a' in the middle: a 6x6 grid scaled to ~60% of icon
    glyph = [
        " ##### ",
        "#     #",
        "#     #",
        "  #####",
        " #    #",
        "#     #",
        " ##### ",
    ]
    gh = len(glyph)
    gw = len(glyph[0])
    scale = max(1, (size * 6) // (10 * gh))
    ox = (size - gw * scale) // 2
    oy = (size - gh * scale) // 2
    for y, row in enumerate(glyph):
        for x, ch in enumerate(row):
            if ch == "#":
                for dy in range(scale):
                    for dx in range(scale):
                        px, py = ox + x * scale + dx, oy + y * scale + dy
                        if 0 <= px < size and 0 <= py < size:
                            pixels[py][px] = FG

    # Encode as PNG (RGBA, 8-bit) — Tauri requires RGBA at build time.
    raw = b"".join(
        b"\x00" + b"".join(struct.pack("BBBB", *px, 255) for px in row)
        for row in pixels
    )

    def chunk(tag: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + tag + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    sig = b"\x89PNG\r\n\x1a\n"
    # color_type=6 (RGBA), bit_depth=8
    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    idat = zlib.compress(raw, 9)
    path.write_bytes(sig + chunk(b"IHDR", ihdr) + chunk(b"IDAT", idat) + chunk(b"IEND", b""))


def make_icns():
    """Bundle PNGs into icon.icns via macOS iconutil."""
    with tempfile.TemporaryDirectory() as td:
        iconset = Path(td) / "icon.iconset"
        iconset.mkdir()
        for sz in (16, 32, 64, 128, 256, 512):
            png = iconset / f"icon_{sz}x{sz}.png"
            write_png(png, sz)
        for sz in (16, 32, 128, 256):
            png = iconset / f"icon_{sz}x{sz}@2x.png"
            write_png(png, sz * 2)
        out = OUT_DIR / "icon.icns"
        subprocess.run(["iconutil", "-c", "icns", str(iconset), "-o", str(out)], check=True)
        print(f"wrote {out}")


def main():
    write_png(OUT_DIR / "32x32.png", 32)
    write_png(OUT_DIR / "128x128.png", 128)
    write_png(OUT_DIR / "128x128@2x.png", 256)
    print(f"wrote 32x32.png, 128x128.png, 128x128@2x.png in {OUT_DIR}")
    if sys.platform == "darwin":
        make_icns()
    else:
        print("skipping .icns generation (not on macOS)")


if __name__ == "__main__":
    main()
