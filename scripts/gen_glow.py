#!/usr/bin/env python3
"""Generates the TV focus glow 9-patch (app/src/main/res/drawable-xhdpi/tv_focus_glow.9.png).

The glow is amber (#F2A93B) at 35% alpha, falling off over 16 dp around a 12 dp rounded poster. It is drawn
once into a 9-patch so the focus animation only fades a bitmap: no blur and no shadow at run time. The file
is drawn for xhdpi (2 px per dp) and scaled for other densities. Needs Pillow; the output is committed.

  python scripts/gen_glow.py [output.9.png]
"""
import math
import sys
from pathlib import Path

from PIL import Image

PX_PER_DP = 2
GLOW_DP = 16
RADIUS_DP = 12
ALPHA = 0.35
AMBER = (0xF2, 0xA9, 0x3B)


def distance_outside(x, y, half_w, half_h, radius):
    """Signed distance from a rounded rectangle centred on the origin; negative inside."""
    qx = abs(x) - (half_w - radius)
    qy = abs(y) - (half_h - radius)
    outside = math.hypot(max(qx, 0.0), max(qy, 0.0))
    inside = min(max(qx, qy), 0.0)
    return outside + inside - radius


def main():
    glow = GLOW_DP * PX_PER_DP
    radius = RADIUS_DP * PX_PER_DP
    edge = glow + radius  # pixels on each side that are not stretched
    size = edge * 2 + 1  # one stretched pixel in the middle
    half = size / 2.0
    image = Image.new("RGBA", (size + 2, size + 2), (0, 0, 0, 0))
    pixels = image.load()
    for y in range(size):
        for x in range(size):
            # The card is the image minus the glow margin on every side.
            d = distance_outside(x + 0.5 - half, y + 0.5 - half, half - glow, half - glow, radius)
            t = min(max(d / glow, 0.0), 1.0)
            # Full strength at the card's edge, easing out to nothing at the glow's reach.
            alpha = ALPHA * (1.0 - t) ** 2.2
            pixels[x + 1, y + 1] = AMBER + (round(alpha * 255),)
    # 9-patch markers: the single middle pixel is the stretchable region on both axes.
    pixels[edge + 1, 0] = (0, 0, 0, 255)
    pixels[0, edge + 1] = (0, 0, 0, 255)
    out = Path(sys.argv[1]) if len(sys.argv) > 1 else (
        Path(__file__).resolve().parent.parent / "app/src/main/res/drawable-xhdpi/tv_focus_glow.9.png")
    image.save(out, optimize=True)
    print(f"{out} {image.size[0]}x{image.size[1]}")


if __name__ == "__main__":
    main()
