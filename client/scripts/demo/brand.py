#!/usr/bin/env python3
"""Render Nightjar's brand graphics in the trailer's look.

    python scripts/demo/brand.py [--out DIR]

writes the README banner (docs/assets/banner.png, 1280x400), the site's social card (site/assets/og.png,
1200x630) and the Play feature graphic and TV banner (fastlane .../images/featureGraphic.png, 1024x500, and
tvBanner.png, 1280x720). --out writes all four into one folder instead, for review.

The sky, the moon mark, the wordmark and the device frames come from trailer.py; the screens in them are the
published demo-mode captures under docs/assets/shots (real captures, nothing generated). The TV banner is the mark
and the wordmark only, so it still reads at launcher-tile size.
"""
import argparse
import os

from PIL import Image

from trailer import (CREAM, MOON, MUTED, REPO, SHOTS, Background, Device, blit, fit_cover, glow, mark, text_img)

TAGLINE = "Your PC games. Every screen."
SUBLINE = "Low-latency game streaming from your PC"

TARGETS = {
    "banner": os.path.join(REPO, "docs", "assets", "banner.png"),
    "og": os.path.join(REPO, "site", "assets", "og.png"),
    "feature": os.path.join(REPO, "fastlane", "metadata", "android", "en-US", "images", "featureGraphic.png"),
    "tv": os.path.join(REPO, "fastlane", "metadata", "android", "en-US", "images", "tvBanner.png"),
}


def shot(name):
    return Image.open(os.path.join(SHOTS, name)).convert("RGB")


def sky(W, H, rings):
    return Background(W, H, noise=False).render(rings)


def devices(cv, x, y, tv_w, hh_w, hh_dx, hh_dy):
    """The TV with Home, and the handheld in front of its lower right corner, like the trailer's every-screen card."""
    tv = Device("tv", tv_w, round(tv_w * 9 / 16))
    hh = Device("handheld", hh_w, round(hh_w * 9 / 16))
    blit(cv, glow(tv.w * 2, tv.h * 2, 0.16), x + tv.w / 2 - tv.w, y + tv.h / 2 - tv.h)
    tv.draw(cv, fit_cover(shot("tv-home.png"), tv.sw, tv.sh), x, y)
    hh.draw(cv, fit_cover(shot("handheld-home.png"), hh.sw, hh.sh), x + hh_dx, y + hh_dy)


def lockup(cv, x, y, ms, word_size, gap):
    """The mark with the wordmark beside it; returns the wordmark's left edge and the lockup's bottom."""
    word = text_img("Nightjar", word_size, CREAM)
    blit(cv, glow(ms * 4, ms * 4, 0.10), x + ms / 2 - ms * 2, y + ms / 2 - ms * 2)
    blit(cv, mark(ms), x, y)
    wx = x + ms + gap
    blit(cv, word, wx, y + ms / 2 - word.size[1] / 2 - word_size * 0.12)
    return wx, y + ms


def banner():
    W, H = 1280, 400
    cv = sky(W, H, (0.78, 0.55))
    devices(cv, 676, 60, 450, 232, 272, 160)
    wx, bottom = lockup(cv, 72, 104, 128, 104, 34)
    tag = text_img(TAGLINE, 34, MUTED, bold=False)
    blit(cv, tag, 74, bottom + 34)
    return cv


def og():
    W, H = 1200, 630
    cv = sky(W, H, (0.80, 0.55))
    devices(cv, 648, 150, 480, 252, 262, 186)
    lockup(cv, 72, 70, 76, 58, 22)
    l1 = text_img("Your PC games.", 70, CREAM)
    l2 = text_img("Every screen.", 70, MOON)
    blit(cv, l1, 68, 222)
    blit(cv, l2, 68, 222 + 84)
    sub = text_img(SUBLINE, 24, MUTED, bold=False)
    blit(cv, sub, 72, 222 + 84 + 112)
    small = text_img("Android · Android TV · Fire TV · Windows · Linux", 21, MUTED, bold=False)
    blit(cv, small, 72, 540)
    return cv


def feature():
    # Play crops nothing but may lay a play button over the centre when a video is set; the copy and the TV
    # keep to the sides.
    W, H = 1024, 500
    cv = sky(W, H, (0.80, 0.55))
    devices(cv, 524, 104, 410, 216, 236, 150)
    wx, bottom = lockup(cv, 56, 132, 92, 72, 24)
    tag = text_img("Your PC games.", 40, CREAM)
    tag2 = text_img("Every screen.", 40, MOON)
    blit(cv, tag, 58, bottom + 40)
    blit(cv, tag2, 58, bottom + 40 + 50)
    return cv


def tv_banner():
    W, H = 1280, 720
    cv = sky(W, H, (0.5, 0.5))
    ms, size, gap = 250, 184, 52
    word = text_img("Nightjar", size, CREAM)
    total = ms + gap + word.size[0]
    x = (W - total) / 2
    y = (H - ms) / 2
    lockup(cv, x, y, ms, size, gap)
    return cv


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--out", help="write all four here instead of their places in the repository")
    a = ap.parse_args()
    for key, fn in (("banner", banner), ("og", og), ("feature", feature), ("tv", tv_banner)):
        path = os.path.join(a.out, os.path.basename(TARGETS[key])) if a.out else TARGETS[key]
        img = fn().convert("RGB")
        img.save(path, optimize=True)
        print("%-18s %dx%d %4d KB" % (os.path.basename(path), img.size[0], img.size[1], os.path.getsize(path) // 1024))


if __name__ == "__main__":
    main()
