#!/usr/bin/env python3
"""Compose Play Store screenshots from the demo-mode stills.

Input: docs/screenshots/demo/*.png (1080x2400 portrait, 2400x1080 landscape),
made by scripts/demo/capture-stills.ps1. Output: fastlane phoneScreenshots,
1080x1920 portrait and 1920x1080 landscape (Play rejects sides over 2:1).
Needs Pillow and the Inter font, static or variable (set INTER_DIR if it is not in
a system path). Flat Nightjar night background, ink text and an amber bar.
"""
import os
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont

ROOT = Path(__file__).resolve().parents[2]
SRC = ROOT / "docs/screenshots/demo"
OUT = ROOT / "fastlane/metadata/android/en-US/images/phoneScreenshots"

# Nightjar brand: night #14161F, moon amber #F2A93B, ink #F4EFE6; the subline is
# the app's night on-surface-variant.
BG = (20, 22, 31)
MOON = (242, 169, 59)
TEXT = (244, 239, 230)
SUB = (193, 196, 210)

# (file, source, crop box or None, headline, subline). Uncropped portrait
# screens run off the bottom edge; cropped ones sit under the caption as a card.
# The stream crop starts at x 150 to keep the compact overlay, which sits in the
# screen's top-left corner over the pillarbox, whole; presets end below Audio,
# and the PiP card stops above the navigation bar.
SHOTS = [
    ("1_stream.png", "landscape-compact.png", (150, 0, 2070, 1036),
     "Stream your PC with low latency", "Your desktop and apps, on your phone"),
    ("2_computers.png", "pc-list.png", None,
     "All your PCs", "Pair once, then connect with a tap"),
    ("3_library.png", "library.png", None,
     "Your whole library", "Pick an app and start streaming"),
    ("4_presets.png", "settings-presets.png", (0, 0, 1080, 1627),
     "Quick presets", "Native quality, or save battery"),
    ("5_pip.png", "pip.png", (330, 1660, 1080, 2266),
     "Keep it in view", "Picture-in-picture, on top"),
]


def font(weight, size):
    names = {"bold": ["InterDisplay-Bold.otf", "Inter-Bold.otf"],
             "regular": ["InterDisplay-Regular.otf", "Inter-Regular.otf"]}[weight]
    dirs = [os.environ.get("INTER_DIR", ""), "/usr/share/fonts/opentype/inter",
            "C:/Windows/Fonts", str(Path.home() / "AppData/Local/Microsoft/Windows/Fonts")]
    for d in dirs:
        for n in names:
            p = Path(d) / n
            if d and p.exists():
                return ImageFont.truetype(str(p), size)
    # Inter 4 variable font: display optical size at the static files' weight.
    for d in dirs:
        for n in ["InterVariable.ttf", "Inter-Variable.ttf"]:
            p = Path(d) / n
            if d and p.exists():
                f = ImageFont.truetype(str(p), size)
                axes = {a["name"]: a for a in f.get_variation_axes()}
                f.set_variation_by_axes([axes[b"Optical size"]["maximum"] if name == b"Optical size" else
                                         (700 if weight == "bold" else 400) for name in axes])
                return f
    sys.exit("Inter font not found; set INTER_DIR")


def background(w, h):
    return Image.new("RGB", (w, h), BG)


def rounded(im, radius):
    mask = Image.new("L", im.size, 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, im.width - 1, im.height - 1), radius, fill=255)
    out = Image.new("RGBA", im.size)
    out.paste(im, (0, 0), mask)
    return out


def place(canvas, shot, x, y, radius):
    shadow = Image.new("RGBA", (shot.width + 80, shot.height + 80), (0, 0, 0, 0))
    ImageDraw.Draw(shadow).rounded_rectangle((40, 50, shot.width + 40, shot.height + 50), radius, fill=(0, 0, 0, 150))
    canvas.alpha_composite(shadow.filter(ImageFilter.GaussianBlur(24)), (x - 40, y - 40))
    framed = rounded(shot, radius)
    border = Image.new("RGBA", shot.size, (0, 0, 0, 0))
    ImageDraw.Draw(border).rounded_rectangle((0, 0, shot.width - 1, shot.height - 1), radius,
                                            outline=(255, 255, 255, 40), width=3)
    canvas.alpha_composite(framed, (x, y))
    canvas.alpha_composite(border, (x, y))


def wrap(d, text, f, max_w):
    lines, line = [], ""
    for word in text.split():
        trial = (line + " " + word).strip()
        if line and d.textlength(trial, font=f) > max_w:
            lines.append(line)
            line = word
        else:
            line = trial
    return lines + [line]


def caption(canvas, head, sub, top, size_head, size_sub):
    """Draw headline, amber bar and subline centred; return the y below them."""
    d = ImageDraw.Draw(canvas)
    fh, fs = font("bold", size_head), font("regular", size_sub)
    w = canvas.width
    y = top
    for line in wrap(d, head, fh, w * 0.88):
        d.text(((w - d.textlength(line, font=fh)) / 2, y), line, font=fh, fill=TEXT)
        y += int(size_head * 1.12)
    bar_y = y + int(size_head * 0.38)
    d.rounded_rectangle((w / 2 - 40, bar_y, w / 2 + 40, bar_y + 10), 5, fill=MOON)
    y = bar_y + 10 + int(size_sub * 0.6)
    for line in wrap(d, sub, fs, w * 0.88):
        d.text(((w - d.textlength(line, font=fs)) / 2, y), line, font=fs, fill=SUB)
        y += int(size_sub * 1.25)
    return y


def fit(shot, max_w, max_h):
    scale = min(max_w / shot.width, max_h / shot.height)
    return shot.convert("RGB").resize((round(shot.width * scale), round(shot.height * scale)), Image.LANCZOS)


def portrait(src, head, sub, cropped):
    W, H = 1080, 1920
    canvas = background(W, H).convert("RGBA")
    top = caption(canvas, head, sub, 96, 100, 54) + 64
    if cropped:
        shot = fit(src, W - 140, H - top - 70)
        place(canvas, shot, (W - shot.width) // 2, top + 20, 44)
    else:
        shot = fit(src, W - 120, 10_000)
        place(canvas, shot, (W - shot.width) // 2, top, 44)
    return canvas


def landscape(src, head, sub):
    W, H = 1920, 1080
    canvas = background(W, H).convert("RGBA")
    below = caption(canvas, head, sub, 48, 72, 42)
    top = below + 36
    shot = fit(src, W - 200, H - top - 44)
    place(canvas, shot, (W - shot.width) // 2, top, 32)
    return canvas


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    for old in OUT.glob("*.png"):
        old.unlink()
    for name, src_name, box, head, sub in SHOTS:
        src = Image.open(SRC / src_name)
        if box:
            src = src.crop(box)
        out = landscape(src, head, sub) if src.width > src.height * 1.3 else portrait(src, head, sub, box is not None)
        out.convert("RGB").save(OUT / name, optimize=True)
        print(name, out.size)


if __name__ == "__main__":
    main()
