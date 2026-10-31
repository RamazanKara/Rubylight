#!/usr/bin/env python3
"""Render the Nightjar feature trailer from real demo-mode captures.

Every picture of the app is a real capture: emulator recordings of the debug build's offline demo mode
(see docs/demo/README.md, "Trailer") and the published stills under docs/assets/shots and
docs/screenshots/demo. Everything else (background rings, device frames, type) is drawn here with
Pillow, in the Nightjar palette and the Figtree font. Frames are piped raw to FFmpeg.

    python scripts/demo/trailer.py --clips C:/src/nj-trailer/clips --out C:/src/nj-trailer/out

writes trailer-16x9.mp4, trailer-9x16.mp4, teaser.webp and poster.webp into --out. --frames t1,t2,...
renders only those seconds of the 16:9 cut as PNGs (for review); --cut picks one cut.
"""

import argparse
import math
import os
import re
import subprocess
import sys
from functools import lru_cache

import numpy as np
from PIL import Image, ImageDraw, ImageFilter, ImageFont

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
FPS = 60

NIGHT = (20, 22, 31)
NIGHT_DEEP = (13, 14, 20)
MOON = (242, 169, 59)
CREAM = (244, 239, 230)
MUTED = (185, 179, 194)
DIM = (128, 124, 140)
RING = (42, 45, 58)
SURFACE = (28, 31, 42)
BEZEL = (9, 10, 14)

FONT_REG = os.path.join(REPO, "protocol", "desktop", "assets", "Figtree-400.ttf")
FONT_BOLD = os.path.join(REPO, "protocol", "desktop", "assets", "Figtree-600.ttf")
SHOTS = os.path.join(REPO, "docs", "assets", "shots")
DEMO = os.path.join(REPO, "docs", "screenshots", "demo")

# The moon-and-nightjar mark (app/src/main/res/drawable/ic_nightjar_foreground.xml), 108-unit viewport.
BIRD = ("M 51.75,41.24 C 53.31,40.96 54.50,41.92 54.89,43.31 C 55.16,44.00 55.33,44.55 55.55,44.95 "
        "C 58.63,42.95 61.40,40.41 64.45,38.63 C 68.66,39.72 73.12,42.22 78.13,45.72 "
        "C 72.88,44.97 68.19,45.36 64.07,46.82 C 60.97,47.94 58.40,49.93 56.11,51.87 L 56.49,56.49 "
        "L 59.86,66.49 C 58.86,67.47 57.71,67.97 56.50,68.18 C 55.30,68.39 54.04,68.32 52.77,67.74 "
        "L 52.51,57.19 L 51.29,52.72 C 48.47,51.68 45.38,50.70 42.08,50.69 C 37.70,50.73 33.17,51.97 "
        "28.50,54.48 C 32.00,49.47 35.34,45.59 38.92,43.13 C 42.40,43.76 45.87,45.20 49.45,46.03 "
        "C 49.52,45.58 49.49,45.00 49.51,44.26 C 49.40,42.82 50.19,41.51 51.75,41.24 Z")


# ---------------------------------------------------------------------------------------------- easing

def clamp(v, lo=0.0, hi=1.0):
    return lo if v < lo else hi if v > hi else v


def ease_out(t):
    t = clamp(t)
    return 1 - (1 - t) ** 3


def ease_in(t):
    t = clamp(t)
    return t ** 3


def ease_in_out(t):
    t = clamp(t)
    return 4 * t ** 3 if t < 0.5 else 1 - (-2 * t + 2) ** 3 / 2


def lerp(a, b, t):
    return a + (b - a) * t


# ---------------------------------------------------------------------------------------------- drawing

@lru_cache(maxsize=None)
def font(size, bold=True):
    return ImageFont.truetype(FONT_BOLD if bold else FONT_REG, size)


def _bird_polygon(scale):
    tokens = re.findall(r"[MCLZ]|-?\d+\.?\d*", BIRD)
    pts, i, cur = [], 0, (0, 0)
    while i < len(tokens):
        tok = tokens[i]
        if tok in "ML":
            cur = (float(tokens[i + 1]), float(tokens[i + 2]))
            pts.append(cur)
            i += 3
        elif tok == "C":
            p1 = (float(tokens[i + 1]), float(tokens[i + 2]))
            p2 = (float(tokens[i + 3]), float(tokens[i + 4]))
            p3 = (float(tokens[i + 5]), float(tokens[i + 6]))
            for k in range(1, 17):
                s = k / 16
                x = (1 - s) ** 3 * cur[0] + 3 * (1 - s) ** 2 * s * p1[0] + 3 * (1 - s) * s * s * p2[0] + s ** 3 * p3[0]
                y = (1 - s) ** 3 * cur[1] + 3 * (1 - s) ** 2 * s * p1[1] + 3 * (1 - s) * s * s * p2[1] + s ** 3 * p3[1]
                pts.append((x, y))
            cur = p3
            i += 7
        else:
            i += 1
    return [((x - 27) * scale, (y - 27) * scale) for x, y in pts]


@lru_cache(maxsize=None)
def mark_master():
    """The moon disc with the bird, 1024 px, antialiased from 4x."""
    size, ss = 1024, 4
    big = Image.new("RGBA", (size * ss, size * ss), (0, 0, 0, 0))
    d = ImageDraw.Draw(big)
    d.ellipse((0, 0, size * ss - 1, size * ss - 1), fill=MOON + (255,))
    d.polygon(_bird_polygon(size * ss / 54), fill=NIGHT + (255,))
    return big.resize((size, size), Image.LANCZOS)


def mark(size):
    return mark_master().resize((max(1, int(size)), max(1, int(size))), Image.LANCZOS)


def text_img(text, size, color, bold=True, tracking=0):
    f = font(size, bold)
    asc, desc = f.getmetrics()
    if tracking:
        w = sum(f.getlength(c) for c in text) + tracking * (len(text) - 1)
    else:
        w = f.getlength(text)
    img = Image.new("RGBA", (int(math.ceil(w)) + 4, asc + desc + 4), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    if tracking:
        x = 2
        for c in text:
            d.text((x, 2), c, font=f, fill=color + (255,))
            x += f.getlength(c) + tracking
    else:
        d.text((2, 2), text, font=f, fill=color + (255,))
    return img


def wrap(text, size, maxw, bold=False):
    f = font(size, bold)
    lines = []
    for para in text.split("\n"):
        cur = ""
        for word in para.split(" "):
            trial = (cur + " " + word).strip()
            if f.getlength(trial) <= maxw or not cur:
                cur = trial
            else:
                lines.append(cur)
                cur = word
        lines.append(cur)
    return lines


def block_img(text, size, color, maxw, bold=False, leading=1.25):
    """Wrapped paragraph as one RGBA image."""
    lines = wrap(text, size, maxw, bold)
    f = font(size, bold)
    lh = int(size * leading)
    asc, desc = f.getmetrics()
    w = int(max(f.getlength(l) for l in lines)) + 4
    img = Image.new("RGBA", (w, lh * (len(lines) - 1) + asc + desc + 4), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    for i, l in enumerate(lines):
        d.text((2, 2 + i * lh), l, font=f, fill=color + (255,))
    return img


def rounded_mask(w, h, r, ss=3):
    big = Image.new("L", (w * ss, h * ss), 0)
    ImageDraw.Draw(big).rounded_rectangle((0, 0, w * ss - 1, h * ss - 1), r * ss, fill=255)
    return big.resize((w, h), Image.LANCZOS)


def with_alpha(img, alpha):
    if alpha >= 0.999:
        return img
    out = img.copy()
    out.putalpha(img.getchannel("A").point(lambda v: int(v * alpha + 0.5)))
    return out


def blit(canvas, layer, x, y, alpha=1.0):
    """Composite layer at a fractional position with an overall alpha; clips to the canvas."""
    if alpha <= 0.002:
        return
    layer = with_alpha(layer, alpha)
    ix, iy = math.floor(x), math.floor(y)
    fx, fy = x - ix, y - iy
    if fx > 0.02 or fy > 0.02:
        w, h = layer.size
        layer = layer.transform((w + 1, h + 1), Image.AFFINE, (1, 0, -fx, 0, 1, -fy), resample=Image.BILINEAR)
    W, H = canvas.size
    w, h = layer.size
    if ix >= W or iy >= H or ix + w <= 0 or iy + h <= 0:
        return
    cx0, cy0 = max(0, -ix), max(0, -iy)
    cx1, cy1 = min(w, W - ix), min(h, H - iy)
    if (cx0, cy0, cx1, cy1) != (0, 0, w, h):
        layer = layer.crop((cx0, cy0, cx1, cy1))
    canvas.alpha_composite(layer, (ix + cx0, iy + cy0))


def shadow(w, h, r, blur, opacity, pad=None):
    pad = pad or blur * 3
    img = Image.new("L", (w + pad * 2, h + pad * 2), 0)
    ImageDraw.Draw(img).rounded_rectangle((pad, pad, pad + w, pad + h), r, fill=int(255 * opacity))
    img = img.filter(ImageFilter.GaussianBlur(blur))
    out = Image.new("RGBA", img.size, (0, 0, 0, 255))
    out.putalpha(img)
    return out, pad


@lru_cache(maxsize=None)
def glow(w, h, strength=0.13):
    """A soft moonlight glow, an amber ellipse fading to nothing."""
    yy, xx = np.mgrid[0:h, 0:w].astype(np.float32)
    d = np.sqrt(((xx - w / 2) / (w / 2)) ** 2 + ((yy - h / 2) / (h / 2)) ** 2)
    a = np.clip(1 - d, 0, 1) ** 2 * 255 * strength
    arr = np.zeros((h, w, 4), np.uint8)
    arr[..., 0], arr[..., 1], arr[..., 2] = MOON
    arr[..., 3] = a.astype(np.uint8)
    return Image.fromarray(arr, "RGBA")


# ---------------------------------------------------------------------------------------------- devices

class Device:
    """A drawn device frame with a screen rectangle. compose(content) returns the RGBA device image."""

    def __init__(self, kind, screen_w, screen_h):
        self.kind, self.sw, self.sh = kind, screen_w, screen_h
        ss = 2
        if kind == "tv":
            b, r, sr = 14, 14, 6
            stand = int(screen_h * 0.10)
            W, H = screen_w + 2 * b, screen_h + 2 * b + stand
            big = Image.new("RGBA", (W * ss, H * ss), (0, 0, 0, 0))
            d = ImageDraw.Draw(big)
            cx = W * ss // 2
            neck_w, foot_w = int(screen_w * 0.06) * ss, int(screen_w * 0.30) * ss
            top = (screen_h + 2 * b) * ss
            d.rectangle((cx - neck_w // 2, top - 10 * ss, cx + neck_w // 2, H * ss - 8 * ss), fill=(24, 26, 34, 255))
            d.rounded_rectangle((cx - foot_w // 2, H * ss - 10 * ss, cx + foot_w // 2, H * ss - 1), 5 * ss,
                                fill=(30, 33, 43, 255))
            d.rounded_rectangle((0, 0, W * ss - 1, (screen_h + 2 * b) * ss - 1), r * ss, fill=BEZEL + (255,),
                                outline=(52, 55, 70, 255), width=2 * ss)
            self.ox, self.oy = b, b
            self.body_h = screen_h + 2 * b
        elif kind == "handheld":
            b, r, sr = int(screen_h * 0.045), int(screen_h * 0.12), int(screen_h * 0.055)
            W, H = screen_w + 2 * b, screen_h + 2 * b
            big = Image.new("RGBA", (W * ss, H * ss), (0, 0, 0, 0))
            d = ImageDraw.Draw(big)
            d.rounded_rectangle((0, 0, W * ss - 1, H * ss - 1), r * ss, fill=(17, 18, 25, 255),
                                outline=(56, 60, 76, 255), width=2 * ss)
            self.ox, self.oy = b, b
            self.body_h = H
        elif kind == "phone":
            b, r, sr = int(screen_w * 0.035), int(screen_w * 0.13), int(screen_w * 0.10)
            W, H = screen_w + 2 * b, screen_h + 2 * b
            big = Image.new("RGBA", (W * ss, H * ss), (0, 0, 0, 0))
            d = ImageDraw.Draw(big)
            d.rounded_rectangle((0, 0, W * ss - 1, H * ss - 1), r * ss, fill=(17, 18, 25, 255),
                                outline=(56, 60, 76, 255), width=2 * ss)
            self.ox, self.oy = b, b
            self.body_h = H
        elif kind == "window":
            bar, r, sr = 44, 14, 0
            W, H = screen_w + 2, screen_h + bar + 1
            big = Image.new("RGBA", (W * ss, H * ss), (0, 0, 0, 0))
            d = ImageDraw.Draw(big)
            d.rounded_rectangle((0, 0, W * ss - 1, H * ss - 1), r * ss, fill=(25, 27, 36, 255),
                                outline=(60, 64, 80, 255), width=2 * ss)
            # Window controls: minimise, maximise, close, as thin glyphs on the right.
            cy = bar * ss // 2
            for i, glyph in enumerate(("min", "max", "close")):
                gx = (W - 30 - (2 - i) * 46) * ss
                g = 6 * ss
                col = (150, 146, 160, 255)
                if glyph == "min":
                    d.line((gx - g, cy, gx + g, cy), fill=col, width=ss * 2)
                elif glyph == "max":
                    d.rectangle((gx - g, cy - g, gx + g, cy + g), outline=col, width=ss * 2)
                else:
                    d.line((gx - g, cy - g, gx + g, cy + g), fill=col, width=ss * 2)
                    d.line((gx - g, cy + g, gx + g, cy - g), fill=col, width=ss * 2)
            self.ox, self.oy = 1, bar
            self.body_h = H
        else:
            raise ValueError(kind)
        self.frame = big.resize((W, H), Image.LANCZOS)
        if kind == "window":
            m = mark(24)
            self.frame.alpha_composite(m, (16, (bar - 24) // 2))
            t = text_img("Nightjar", 20, MUTED, bold=False)
            self.frame.alpha_composite(t, (50, (bar - t.size[1]) // 2 + 1))
            mask = Image.new("L", (screen_w, screen_h), 255)
            # Only the bottom corners of the content are rounded.
            corner = rounded_mask(screen_w, screen_h + 40, r)
            mask = corner.crop((0, 40, screen_w, screen_h + 40))
            self.mask = mask
        else:
            self.mask = rounded_mask(screen_w, screen_h, sr)
        self.w, self.h = self.frame.size
        self.shadow, self.spad = shadow(self.w, self.body_h, 20, 28, 0.55)

    def compose(self, content):
        if content.size != (self.sw, self.sh):
            content = content.resize((self.sw, self.sh), Image.LANCZOS)
        out = self.frame.copy()
        out.paste(content.convert("RGB"), (self.ox, self.oy), self.mask)
        return out

    def draw(self, canvas, content, x, y, alpha=1.0, shadow_alpha=1.0):
        blit(canvas, self.shadow, x - self.spad, y - self.spad + 18, alpha * shadow_alpha)
        blit(canvas, self.compose(content), x, y, alpha)


# ---------------------------------------------------------------------------------------------- media

def fit_cover(img, w, h, align=(0.5, 0.5)):
    """Scale to cover w x h and crop, keeping align (0..1) of the overflow."""
    iw, ih = img.size
    s = max(w / iw, h / ih)
    nw, nh = int(math.ceil(iw * s)), int(math.ceil(ih * s))
    img = img.resize((nw, nh), Image.LANCZOS)
    x0, y0 = int((nw - w) * align[0]), int((nh - h) * align[1])
    return img.crop((x0, y0, x0 + w, y0 + h))


class Still:
    def __init__(self, path, crop=None):
        self.path = path
        img = Image.open(path).convert("RGB")
        if crop:
            img = img.crop(crop)
        self.img = img
        self._cache = {}

    def frame(self, t, w, h):
        key = (w, h)
        if key not in self._cache:
            self._cache[key] = fit_cover(self.img, w, h)
        return self._cache[key]


class Clip:
    """A recorded clip window, decoded on demand at 60 fps and the screen size it is shown at."""

    def __init__(self, path, start, dur, crop=None, hold_last=True):
        self.path, self.start, self.dur, self.crop = path, start, dur, crop
        self.proc, self.size, self.idx, self.last = None, None, -1, None

    def _open(self, w, h):
        vf = []
        if self.crop:
            vf.append("crop=%d:%d:%d:%d" % self.crop)
        vf += ["fps=%d" % FPS, "scale=%d:%d:flags=lanczos" % (w, h)]
        cmd = ["ffmpeg", "-v", "error", "-ss", "%.3f" % self.start, "-t", "%.3f" % (self.dur + 0.2), "-i", self.path,
               "-vf", ",".join(vf), "-f", "rawvideo", "-pix_fmt", "rgb24", "-"]
        self.proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        self.size, self.idx = (w, h), -1

    def frame(self, t, w, h):
        want = int(round(clamp(t, 0, self.dur) * FPS))
        if self.proc is None or self.size != (w, h) or want < self.idx:
            if self.proc:
                self.proc.kill()
            self._open(w, h)
        n = w * h * 3
        while self.idx < want:
            buf = self.proc.stdout.read(n)
            if len(buf) < n:
                break
            self.last = Image.frombuffer("RGB", (w, h), buf, "raw", "RGB", 0, 1)
            self.idx += 1
        if self.last is None:
            raise RuntimeError("no frames from %s at %.2f" % (self.path, self.start))
        return self.last

    def close(self):
        if self.proc:
            self.proc.kill()
            self.proc = None


class Sequence:
    """Media one after another: [(source, seconds), ...]."""

    def __init__(self, parts):
        self.parts = parts

    def frame(self, t, w, h):
        acc = 0
        for i, (src, d) in enumerate(self.parts):
            if t < acc + d or i == len(self.parts) - 1:
                return src.frame(t - acc, w, h)
            acc += d

    def close(self):
        for src, _ in self.parts:
            if hasattr(src, "close"):
                src.close()


def media(clips_dir, name, start, dur, still, crop=None, still_crop=None):
    """A clip from the recordings if it exists, else the published still."""
    path = os.path.join(clips_dir, name) if clips_dir else None
    if path and os.path.exists(path):
        return Clip(path, start, dur, crop)
    return Still(still, still_crop)


# ---------------------------------------------------------------------------------------------- scenes

class Ctx:
    def __init__(self, W, H, clips):
        self.W, self.H, self.clips = W, H, clips
        self.V = H > W
        self.margin = 96 if not self.V else 84


def envelope(lt, dur, t_in, d_in=0.55, d_out=0.38):
    """(alpha, dy) for an element that enters at t_in and leaves with its scene."""
    a_in = ease_out((lt - t_in) / d_in)
    a_out = 1 - ease_in((lt - (dur - d_out)) / d_out)
    alpha = clamp(min(a_in, a_out))
    dy = (1 - a_in) * 26 - (1 - a_out) * 14
    return alpha, dy


class Scene:
    dur = 4.0
    rings = (0.86, 0.5)

    def __init__(self, ctx, dur=None):
        self.c = ctx
        if dur:
            self.dur = dur

    def headline(self, canvas, lt, lines, x, y, t_in=0.25, size=None, color=CREAM, accent_last=False,
                 sub=None, small=None, maxw=None, swap=None):
        """Headline lines + supporting line + small print as one column at (x, y)."""
        c = self.c
        size = size or (92 if not c.V else 88)
        maxw = maxw or (560 if not c.V else c.W - 2 * c.margin - 60)
        yy = y
        for i, line in enumerate(lines):
            col = MOON if (accent_last and i == len(lines) - 1) else color
            img = text_img(line, size, col)
            a, dy = envelope(lt, self.dur, t_in + i * 0.08)
            if swap is not None:
                a = min(a, swap)
            blit(canvas, img, x - 2, yy + dy, a)
            yy += int(size * 1.08)
        if sub:
            yy += int(size * 0.28)
            img = block_img(sub, 38 if not c.V else 40, MUTED, maxw)
            a, dy = envelope(lt, self.dur, t_in + 0.22)
            if swap is not None:
                a = min(a, swap)
            blit(canvas, img, x - 2, yy + dy, a)
            yy += img.size[1]
        if small:
            yy += 26
            img = block_img(small, 26, DIM, maxw)
            a, dy = envelope(lt, self.dur, t_in + 0.34)
            if swap is not None:
                a = min(a, swap)
            blit(canvas, img, x - 2, yy + dy, a)
            yy += img.size[1]
        return yy

    def device_in(self, lt, t_in=0.0, dx=90, d_in=0.8):
        a_in = ease_out((lt - t_in) / d_in)
        a_out = 1 - ease_in((lt - (self.dur - 0.38)) / 0.38)
        return clamp(min(a_in, a_out)), (1 - a_in) * dx - (1 - a_out) * dx * 0.4

    def close(self):
        pass


def swap_alpha(lt, at, d=0.3):
    """Two-phase caption swap around time `at`: (alpha of the old caption, alpha of the new one)."""
    old = 1 - ease_in((lt - (at - d)) / d)
    new = ease_out((lt - at) / (d * 1.4))
    return clamp(old), clamp(new)


class Intro(Scene):
    dur = 3.4
    rings = (0.5, 0.5)

    def render(self, cv, lt):
        c = self.c
        W, H = c.W, c.H
        out = 1 - ease_in((lt - (self.dur - 0.4)) / 0.4)
        ms = 250 if not c.V else 300
        s = lerp(0.82, 1.0, ease_out(lt / 0.8))
        a = ease_out(lt / 0.6) * out
        word = text_img("Nightjar", 150 if not c.V else 160, CREAM)
        tag = text_img("Your PC games. Every screen.", 54 if not c.V else 56, MUTED, bold=False)
        if not c.V:
            gap = 54
            total = ms + gap + word.size[0]
            slide = ease_in_out((lt - 0.65) / 0.75)
            mx = lerp(W / 2 - ms / 2, W / 2 - total / 2, slide)
            my = H / 2 - ms / 2 - 40
            m = mark(ms * s)
            blit(cv, glow(900, 900, 0.10), mx + ms / 2 - 450, my + ms / 2 - 450, a)
            blit(cv, m, mx + (ms - m.size[0]) / 2, my + (ms - m.size[1]) / 2, a)
            wa = ease_out((lt - 0.95) / 0.6) * out
            wx = mx + ms + gap
            blit(cv, word, wx + (1 - wa) * 40, my + ms / 2 - word.size[1] / 2 - 18, wa)
            ta = ease_out((lt - 1.45) / 0.6) * out
            blit(cv, tag, W / 2 - tag.size[0] / 2, my + ms + 70 + (1 - ta) * 20, ta)
        else:
            mx, my = W / 2 - ms / 2, H * 0.36 - ms / 2
            m = mark(ms * s)
            blit(cv, glow(900, 900, 0.10), W / 2 - 450, my + ms / 2 - 450, a)
            blit(cv, m, mx + (ms - m.size[0]) / 2, my + (ms - m.size[1]) / 2, a)
            wa = ease_out((lt - 0.7) / 0.6) * out
            blit(cv, word, W / 2 - word.size[0] / 2, my + ms + 60 + (1 - wa) * 30, wa)
            ta = ease_out((lt - 1.2) / 0.6) * out
            tag2 = block_img("Your PC games.\nEvery screen.", 64, MUTED, W - 200)
            blit(cv, tag2, W / 2 - tag2.size[0] / 2, my + ms + 290 + (1 - ta) * 20, ta)


class TvHome(Scene):
    """TV Home: D-pad along the row (the hero follows), then a game's page."""
    dur = 8.42
    rings = (0.72, 0.52)
    swap_at = 5.0

    def __init__(self, ctx, dur=None, tv=None):
        super().__init__(ctx, dur)
        c = ctx
        if not c.V:
            self.dev = Device("tv", 1176, 662)
        else:
            self.dev = Device("tv", 960, 540)
        self.src = tv

    def render(self, cv, lt):
        c = self.c
        a, dx = self.device_in(lt, 0.0)
        if not c.V:
            x, y = c.W - self.dev.w - 70 + dx, (c.H - self.dev.h) / 2 + 10
            blit(cv, glow(1500, 1000), x + self.dev.w / 2 - 750, y + self.dev.h / 2 - 500, a)
            self.dev.draw(cv, self.src.frame(lt, self.dev.sw, self.dev.sh), x, y, a)
            tx, ty = c.margin, 300
        else:
            x, y = (c.W - self.dev.w) / 2 + dx, 1020
            blit(cv, glow(1300, 1000), x + self.dev.w / 2 - 650, y + self.dev.h / 2 - 500, a)
            self.dev.draw(cv, self.src.frame(lt, self.dev.sw, self.dev.sh), x, y, a)
            tx, ty = c.margin, 330
        maxw = None if c.V else x - dx - tx - 70  # keep the column clear of the TV
        old, new = swap_alpha(lt, self.swap_at)
        self.headline(cv, lt, ["One library.", "Every PC."], tx, ty, accent_last=True,
                      sub="Games from every store, with covers, on every paired PC.",
                      small="Store libraries with a Rubylight 3.0 PC.", swap=old, maxw=maxw)
        if new > 0:
            self.headline(cv, lt, ["A page for", "every game."], tx, ty, accent_last=True, t_in=self.swap_at,
                          sub="Play, resume or wake a sleeping PC, right from the couch.", swap=new, maxw=maxw)


class Handheld(Scene):
    """Handheld: a game from Home into the stream, compact overlay, long-press to Statistics."""
    dur = 9.0
    rings = (0.3, 0.5)
    swap_at = 6.0

    def __init__(self, ctx, dur=None, src=None):
        super().__init__(ctx, dur)
        if not ctx.V:
            self.dev = Device("handheld", 1200, 540)
        else:
            self.dev = Device("handheld", 960, 432)
        self.src = src

    def render(self, cv, lt):
        c = self.c
        a, dx = self.device_in(lt, 0.0, dx=-90)
        if not c.V:
            x, y = 70 + dx, (c.H - self.dev.h) / 2 + 20
            tx, ty = x - dx + self.dev.w + 70, 330
            maxw = c.W - tx - c.margin + 20
        else:
            x, y = (c.W - self.dev.w) / 2 + dx, 1060
            tx, ty, maxw = c.margin, 330, None
        blit(cv, glow(1500, 900), x + self.dev.w / 2 - 750, y + self.dev.h / 2 - 450, a)
        self.dev.draw(cv, self.src.frame(lt, self.dev.sw, self.dev.sh), x, y, a)
        old, new = swap_alpha(lt, self.swap_at)
        size = 84 if not c.V else 88
        self.headline(cv, lt, ["Pick a game.", "Play."], tx, ty, accent_last=True, size=size, maxw=maxw,
                      sub="Phones, tablets and handhelds, with touch, mouse and controllers.", swap=old)
        if new > 0:
            self.headline(cv, lt, ["See every", "millisecond."], tx, ty, accent_last=True, size=size, maxw=maxw,
                          t_in=self.swap_at, sub="A compact overlay, or full statistics with a long-press.",
                          small="Demo footage, illustrative readings.", swap=new)


class Control(Scene):
    """Mid-stream control: stream menu, the PC's Radeon settings, keyboard, then picture-in-picture on a phone."""
    dur = 10.1
    rings = (0.7, 0.45)

    def __init__(self, ctx, dur=None, beats=None):
        super().__init__(ctx, dur)
        self.beats = beats  # [(start, source, caption, small, "handheld" | "phone")]
        if not ctx.V:
            self.hh = Device("handheld", 1200, 540)
            self.ph = Device("phone", 300, 667)
        else:
            self.hh = Device("handheld", 960, 432)
            self.ph = Device("phone", 330, 733)
        self.pip_at = next((b[0] for b in beats if b[4] == "phone"), dur or self.dur)

    def render(self, cv, lt):
        c = self.c
        a, dx = self.device_in(lt, 0.0)
        idx = max(i for i, b in enumerate(self.beats) if lt >= b[0] or i == 0)
        start, src, cap, small, kind = self.beats[idx]
        nxt = self.beats[idx + 1][0] if idx + 1 < len(self.beats) else self.dur + 10
        if not c.V:
            hx, hy = 70 + dx, 360
            px, py = hx - dx + self.hh.w / 2 - self.ph.w / 2, 300
            cap_x, cap_y = 70 + self.hh.w + 64, 470
            hl_x, hl_y, cw = c.margin, 110, 470
            if kind == "phone":
                cap_x, cap_y = px + self.ph.w + 90, py + 250
        else:
            hx, hy = (c.W - self.hh.w) / 2 + dx, 800
            px, py = (c.W - self.ph.w) / 2, 690
            cap_x, cap_y = c.margin, (hy + self.hh.h + 80) if kind == "handheld" else (py + self.ph.h + 70)
            hl_x, hl_y, cw = c.margin, 330, c.W - 2 * c.margin - 40
        self.headline(cv, lt, ["In control,", "mid-game."], hl_x, hl_y, accent_last=True,
                      size=84 if not c.V else 88, maxw=1600)
        # The handheld carries the stream beats; it gives way to the phone for picture-in-picture.
        hh_a = a * (1 - ease_in((lt - (self.pip_at - 0.3)) / 0.3))
        if hh_a > 0.002:
            if kind == "handheld":
                content = src.frame(lt - start, self.hh.sw, self.hh.sh)
                # Between two stream beats the screen dips briefly, like a cut on the device itself.
                dip = min(ease_out((lt - start) / 0.22) if idx else 1.0, 1 - ease_in((lt - (nxt - 0.16)) / 0.16))
                if dip < 0.999:
                    content = Image.blend(Image.new("RGB", content.size, (0, 0, 0)), content, dip)
            else:
                content = Image.new("RGB", (self.hh.sw, self.hh.sh), (0, 0, 0))
            blit(cv, glow(1500, 900), hx + self.hh.w / 2 - 750, hy + self.hh.h / 2 - 450, hh_a)
            self.hh.draw(cv, content, hx, hy, hh_a)
        if lt >= self.pip_at:
            k = ease_out((lt - self.pip_at) / 0.55)
            pa = min(a, k)
            blit(cv, glow(1000, 1100), px + self.ph.w / 2 - 500, py + self.ph.h / 2 - 550, pa)
            self.ph.draw(cv, src.frame(lt - start, self.ph.sw, self.ph.sh), px, py + (1 - k) * 40, pa)
        ca = min(a, ease_out((lt - start - 0.12) / 0.45), 1 - ease_in((lt - (nxt - 0.25)) / 0.25))
        img = block_img(cap, 46 if not c.V else 50, CREAM, cw, bold=True, leading=1.16)
        blit(cv, img, cap_x, cap_y + (1 - ca) * 18, ca)
        yy = cap_y + img.size[1] + 24
        if small:
            s = block_img(small, 28, DIM, cw)
            blit(cv, s, cap_x, yy + (1 - ca) * 18, ca)
        if not c.V and kind == "handheld":
            note = text_img("Demo mode footage.", 24, DIM, bold=False)
            blit(cv, note, hx + 6, hy + self.hh.h + 26, hh_a * 0.9)


class FeatureWall(Scene):
    dur = 7.0
    rings = (0.5, 0.95)
    TILES = [
        ("VIDEO", "H.264 · HEVC · AV1 · PyroWave"),
        ("HDR", "HDR10 · HDR10+ · Dolby Vision · HLG"),
        ("AUDIO", "Up to 7.1.4 surround and passthrough"),
        ("DUALSENSE", "Haptics and adaptive triggers"),
        ("SHARING", "Clipboard, files and USB devices"),
        ("CO-OP", "Split-screen co-op"),
        ("UPSCALING", "FSR 1 · Snapdragon GSR"),
        ("CONTROLLERS", "Up to 16 at once"),
    ]

    def render(self, cv, lt):
        c = self.c
        if not c.V:
            cols, tw, th, gx, gy = 4, 404, 230, 28, 28
            x0 = (c.W - (cols * tw + (cols - 1) * gx)) / 2
            y0 = 360
            hx, hy = x0, 150
        else:
            cols, tw, th, gx, gy = 2, 438, 250, 28, 28
            x0 = (c.W - (cols * tw + (cols - 1) * gx)) / 2
            y0 = 640
            hx, hy = x0, 330
        self.headline(cv, lt, ["Everything a great", "stream needs."], hx, hy, accent_last=True, t_in=0.1,
                      size=84 if not c.V else 80, maxw=1600)
        tile = self._tile(tw, th)
        for i, (eyebrow, label) in enumerate(self.TILES):
            col, row = i % cols, i // cols
            x, y = x0 + col * (tw + gx), y0 + row * (th + gy)
            a, dy = envelope(lt, self.dur, 0.45 + i * 0.09, d_in=0.6)
            blit(cv, tile, x, y + dy * 1.6, a)
            e = text_img(eyebrow, 24, MOON, tracking=3)
            blit(cv, e, x + 32, y + 30 + dy * 1.6, a)
            t = block_img(label, 42 if not c.V else 44, CREAM, tw - 64, bold=True, leading=1.14)
            blit(cv, t, x + 32, y + 78 + dy * 1.6, a)
        small = ("HDR10+, Dolby Vision, HLG, 7.1.4, passthrough, sharing and co-op with a Rubylight 3.0 PC. "
                 "Features follow what each PC and device supports.")
        img = block_img(small, 26, DIM, (cols * tw + (cols - 1) * gx) if not c.V else c.W - 2 * c.margin)
        a, dy = envelope(lt, self.dur, 1.4)
        rows = (len(self.TILES) + cols - 1) // cols
        blit(cv, img, x0, y0 + rows * (th + gy) + 18 + dy, a)

    @lru_cache(maxsize=None)
    def _tile(self, w, h):
        img = Image.new("RGBA", (w, h), (0, 0, 0, 0))
        big = Image.new("RGBA", (w * 2, h * 2), (0, 0, 0, 0))
        ImageDraw.Draw(big).rounded_rectangle((0, 0, w * 2 - 1, h * 2 - 1), 44, fill=SURFACE + (255,),
                                              outline=(48, 52, 68, 255), width=3)
        img = big.resize((w, h), Image.LANCZOS)
        return img


class Measured(Scene):
    dur = 5.0
    rings = (0.3, 0.5)

    def render(self, cv, lt):
        c = self.c
        if not c.V:
            x, y = c.margin + 10, 190
        else:
            x, y = c.margin, 420
        e = text_img("OUR MEASUREMENTS", 28, MOON, tracking=4)
        a, dy = envelope(lt, self.dur, 0.1)
        blit(cv, e, x, y + dy, a)
        big = text_img("7 ms", 300 if not c.V else 280, CREAM)
        a, dy = envelope(lt, self.dur, 0.25, d_in=0.7)
        s = lerp(0.94, 1.0, ease_out((lt - 0.25) / 0.9))
        b2 = big.resize((int(big.size[0] * s), int(big.size[1] * s)), Image.LANCZOS)
        blit(cv, b2, x - 12, y + 30 + dy + (big.size[1] - b2.size[1]) / 2, a)
        lab = text_img("median picture age", 58, MOON)
        a, dy = envelope(lt, self.dur, 0.5)
        blit(cv, lab, x, y + 30 + big.size[1] - 10 + dy, a)
        maxw = 820 if not c.V else c.W - 2 * c.margin
        small = block_img("From the PC's capture to the frame rendered on the device. Retroid Pocket 6 over "
                          "Wi-Fi, HEVC 1080p60, Rubylight PC with a Radeon RX 7900 XT.", 28, DIM, maxw)
        a, dy = envelope(lt, self.dur, 0.7)
        blit(cv, small, x, y + 30 + big.size[1] + 80 + dy, a)
        # Second figure: 120 fps on a handheld.
        if not c.V:
            rx, ry = 1180, 340
            line_a, ldy = envelope(lt, self.dur, 1.2)
            d = Image.new("RGBA", (3, 380), RING + (255,))
            blit(cv, d, rx - 70, ry - 40, line_a)
        else:
            rx, ry = c.margin, 1260
        n = text_img("120 fps", 150, CREAM)
        a, dy = envelope(lt, self.dur, 1.3)
        blit(cv, n, rx - 6, ry + dy, a)
        l2 = text_img("on a handheld", 52, MOON)
        a, dy = envelope(lt, self.dur, 1.45)
        blit(cv, l2, rx, ry + n.size[1] - 4 + dy, a)
        s2 = block_img("Retroid Pocket 6, HEVC and PyroWave 1080p120.", 28, DIM, 560 if not c.V else maxw)
        a, dy = envelope(lt, self.dur, 1.6)
        blit(cv, s2, rx, ry + n.size[1] + 72 + dy, a)


class Platforms(Scene):
    """Every screen: the desktop app in a window, the TV and the handheld around it."""
    dur = 4.3
    rings = (0.5, 0.62)

    def __init__(self, ctx, dur=None):
        super().__init__(ctx, dur)
        V = ctx.V
        self.win = Device("window", 880 if not V else 760, 550 if not V else 475)
        self.tv = Device("tv", 640 if not V else 560, 360 if not V else 315)
        self.hh = Device("handheld", 520 if not V else 470, 234 if not V else 212)
        self.desk = Still(os.path.join(SHOTS, "desktop-home.png"))
        self.tvs = Still(os.path.join(SHOTS, "tv-home.png"))
        self.hhs = Still(os.path.join(SHOTS, "handheld-home.png"))

    def render(self, cv, lt):
        c = self.c
        W, H = c.W, c.H
        if not c.V:
            # The TV behind on the left, the desktop window in the middle, the handheld in front on the right.
            spots = [(self.tv, self.tvs, 110, 330, 0.15, -60),
                     (self.win, self.desk, 610, 380, 0.0, 0),
                     (self.hh, self.hhs, 1300, 660, 0.3, 60)]
        else:
            spots = [(self.tv, self.tvs, 60, 700, 0.15, -60),
                     (self.win, self.desk, W - 760 - 50, 1010, 0.0, 60),
                     (self.hh, self.hhs, 90, 1430, 0.3, -60)]
        for dev, src, x, y, t_in, ddx in spots:
            a, dy = envelope(lt, self.dur, t_in + 0.1, d_in=0.7)
            dev.draw(cv, src.frame(0, dev.sw, dev.sh), x + (1 - ease_out((lt - t_in - 0.1) / 0.7)) * ddx,
                     y + dy * 2, a)
        title = "On every screen you own."
        plat = "Android · Android TV · Fire TV · Windows · Linux"
        if not c.V:
            t = text_img(title, 84, CREAM)
            a, dy = envelope(lt, self.dur, 0.25)
            blit(cv, t, W / 2 - t.size[0] / 2, 80 + dy, a)
            p = text_img(plat, 42, MOON)
            a, dy = envelope(lt, self.dur, 0.45)
            blit(cv, p, W / 2 - p.size[0] / 2, 200 + dy, a)
        else:
            t = block_img("On every screen\nyou own.", 88, CREAM, W - 2 * c.margin, bold=True, leading=1.08)
            a, dy = envelope(lt, self.dur, 0.25)
            blit(cv, t, c.margin, 330 + dy, a)
            p = block_img("Android · Android TV · Fire TV\nWindows · Linux", 46, MOON, W - 2 * c.margin, bold=True)
            a, dy = envelope(lt, self.dur, 0.45)
            blit(cv, p, c.margin, 330 + t.size[1] + 30 + dy, a)


class Hosts(Scene):
    dur = 3.7
    rings = (0.5, 0.5)

    def render(self, cv, lt):
        c = self.c
        W, H = c.W, c.H
        if not c.V:
            l1 = text_img("Works with your host.", 92, CREAM)
            l2 = text_img("Sunshine · Apollo · Vibepollo · Rubylight", 56, MOON)
            l3 = text_img("and other Moonlight hosts", 40, MUTED, bold=False)
            l4 = text_img("No accounts. No ads. No tracking.", 64, CREAM)
            ys = [250, 390, 470, 700]
            items = [(l1, 0.15), (l2, 0.35), (l3, 0.5), (l4, 1.2)]
            for (img, t_in), y in zip(items, ys):
                a, dy = envelope(lt, self.dur, t_in)
                blit(cv, img, W / 2 - img.size[0] / 2, y + dy, a)
            a, dy = envelope(lt, self.dur, 1.0)
            blit(cv, Image.new("RGBA", (120, 3), MOON + (255,)), W / 2 - 60, 610 + dy, a)
        else:
            mw = W - 2 * c.margin
            l1 = block_img("Works with\nyour host.", 92, CREAM, mw, bold=True, leading=1.08)
            l2 = block_img("Sunshine · Apollo\nVibepollo · Rubylight", 56, MOON, mw, bold=True)
            l3 = text_img("and other Moonlight hosts", 42, MUTED, bold=False)
            l4 = block_img("No accounts.\nNo ads.\nNo tracking.", 80, CREAM, mw, bold=True, leading=1.12)
            ys = [380, 620, 780, 1040]
            for (img, t_in), y in zip([(l1, 0.15), (l2, 0.35), (l3, 0.5), (l4, 1.2)], ys):
                a, dy = envelope(lt, self.dur, t_in)
                blit(cv, img, c.margin, y + dy, a)
            a, dy = envelope(lt, self.dur, 1.0)
            blit(cv, Image.new("RGBA", (120, 4), MOON + (255,)), c.margin + 2, 960 + dy, a)


class Outro(Scene):
    dur = 4.4
    rings = (0.5, 0.5)
    fade = False  # the trailer ends on the held last frame; the looping teaser fades out instead

    def render(self, cv, lt):
        c = self.c
        W, H = c.W, c.H
        out = 1 - ease_in((lt - (self.dur - 0.45)) / 0.45) if self.fade else 1.0
        ms = 190 if not c.V else 240
        a, dy = envelope(lt, self.dur + 10, 0.05, d_in=0.7)
        a *= out
        s = lerp(0.9, 1.0, ease_out(lt / 0.8))
        m = mark(ms * s)
        my = (H / 2 - 300) if not c.V else 560
        blit(cv, glow(900, 900, 0.09), W / 2 - 450, my + ms / 2 - 450, a)
        blit(cv, m, W / 2 - m.size[0] / 2, my + (ms - m.size[1]) / 2, a)
        y = my + ms + 50
        for img, t_in, gap in [(text_img("Free and open source", 72, CREAM), 0.35, 40),
                               (None, 0.6, 0),
                               (text_img("github.com/RamazanKara/nightjar", 44, MUTED, bold=False), 0.8, 0)]:
            a, dy = envelope(lt, self.dur + 10, t_in)
            a *= out
            if img is None:
                pill = self._pill()
                blit(cv, pill, W / 2 - pill.size[0] / 2, y + dy, a)
                y += pill.size[1] + 40
                continue
            blit(cv, img, W / 2 - img.size[0] / 2, y + dy, a)
            y += img.size[1] + gap

    @lru_cache(maxsize=None)
    def _pill(self):
        t = text_img("Get Nightjar", 50, NIGHT)
        w, h = t.size[0] + 100, 100
        big = Image.new("RGBA", (w * 2, h * 2), (0, 0, 0, 0))
        ImageDraw.Draw(big).rounded_rectangle((0, 0, w * 2 - 1, h * 2 - 1), h, fill=MOON + (255,))
        img = big.resize((w, h), Image.LANCZOS)
        img.alpha_composite(t, ((w - t.size[0]) // 2, (h - t.size[1]) // 2 - 4))
        return img


# ---------------------------------------------------------------------------------------------- background

class Background:
    def __init__(self, W, H):
        self.W, self.H = W, H
        yy = np.linspace(0, 1, H, dtype=np.float32)[:, None]
        xx = np.linspace(0, 1, W, dtype=np.float32)[None, :]
        v = np.clip(1 - (((xx - 0.5) * 1.2) ** 2 + ((yy - 0.45) * 1.3) ** 2), 0, 1)
        base = np.zeros((H, W, 3), np.float32)
        for k in range(3):
            base[..., k] = NIGHT_DEEP[k] + (NIGHT[k] - NIGHT_DEEP[k]) * (0.35 + 0.65 * v)
        # A little noise keeps the gradient from banding after H.264.
        rng = np.random.default_rng(7)
        base += rng.normal(0, 0.6, base.shape).astype(np.float32)
        self.base = Image.fromarray(np.clip(base, 0, 255).astype(np.uint8), "RGB").convert("RGBA")
        R = int(max(W, H) * 0.95)
        ss = 2
        big = Image.new("RGBA", (R * 2 * ss, R * 2 * ss), (0, 0, 0, 0))
        d = ImageDraw.Draw(big)
        for i, f in enumerate((1.0, 0.8, 0.6, 0.4)):
            r = R * f * ss
            d.ellipse((R * ss - r, R * ss - r, R * ss + r, R * ss + r), outline=RING + (int(200 - i * 25),),
                      width=2 * ss)
        self.rings = big.resize((R * 2, R * 2), Image.LANCZOS)
        self.R = R

    def render(self, center, alpha=1.0):
        cv = self.base.copy()
        cx, cy = center
        # Whole pixels, and only the visible part: the ring layer is far larger than the frame.
        x, y = round(cx * self.W - self.R), round(cy * self.H - self.R)
        x0, y0 = max(0, -x), max(0, -y)
        part = self.rings.crop((x0, y0, min(2 * self.R, self.W - x), min(2 * self.R, self.H - y)))
        blit(cv, part, x + x0, y + y0, alpha)
        return cv


# ---------------------------------------------------------------------------------------------- cuts

def build(cut, ctx, clips):
    def C(name, start, dur, still):
        return media(clips, name, start, dur, still)

    def tv_src(skip=0.0):
        # Home: Down to Recently played, Right three times, OK on Tidebreaker; its page (the placeholder while it
        # loads is cut) up to Play. The emulator streams at about 20 fps, so the stream itself is left to the handheld.
        return Sequence([(C("tv-home.mp4", 0.2 + skip, 5.22 - skip, os.path.join(SHOTS, "tv-home.png")), 5.22 - skip),
                         (C("tv-home.mp4", 5.88, 2.0, os.path.join(SHOTS, "tv-game-page.png")), 2.0)])

    hh = C("hh-flow.mp4", 1.5, 9.0, os.path.join(DEMO, "landscape-compact.png"))
    beats = [
        (0.0, C("hh-menu.mp4", 2.75, 2.5, os.path.join(DEMO, "landscape-stream-menu.png")),
         "Bitrate, quality and the overlay, mid-stream.", None, "handheld"),
        (2.5, C("hh-radeon.mp4", 2.2, 2.9, os.path.join(SHOTS, "stream-menu-radeon.png")),
         "Your PC's Radeon settings, from the couch.", "With a Rubylight 3.0 PC.", "handheld"),
        (5.4, C("hh-keyboard.mp4", 1.3, 2.3, os.path.join(SHOTS, "stream-keyboard.png")),
         "A full PC keyboard, even with a controller.", None, "handheld"),
        (7.7, C("pip.mp4", 2.5, 2.4, os.path.join(DEMO, "pip.png")),
         "Keep watching in picture-in-picture.", None, "phone"),
    ]
    if cut == "teaser":
        # A 14.8 s loop: no intro, and the outro fades so the loop point is the empty night.
        t = TvHome(ctx, 5.0, tv=tv_src(skip=2.0))
        t.swap_at = 3.0
        h = Handheld(ctx, 4.6, src=C("hh-flow.mp4", 4.9, 4.6, os.path.join(DEMO, "landscape-statistics.png")))
        h.swap_at = 2.3
        o = Outro(ctx, 2.2)
        o.fade = True
        return [t, h, Measured(ctx, 3.0), o]
    return [Intro(ctx), TvHome(ctx, 7.22, tv=tv_src()), Handheld(ctx, src=hh), Control(ctx, beats=beats),
            FeatureWall(ctx), Measured(ctx), Platforms(ctx), Hosts(ctx), Outro(ctx)]


def ring_center(scenes, t):
    """Ring centre follows the scenes, gliding between them over the cut."""
    acc = 0
    for i, s in enumerate(scenes):
        if t < acc + s.dur or i == len(scenes) - 1:
            cur = s.rings
            lt = t - acc
            if i + 1 < len(scenes) and lt > s.dur - 0.5:
                k = ease_in_out((lt - (s.dur - 0.5)) / 1.0)
                n = scenes[i + 1].rings
                return (lerp(cur[0], n[0], k), lerp(cur[1], n[1], k))
            if i > 0 and lt < 0.5:
                k = ease_in_out((lt + 0.5) / 1.0)
                p = scenes[i - 1].rings
                return (lerp(p[0], cur[0], k), lerp(p[1], cur[1], k))
            return cur
        acc += s.dur


def frame_at(scenes, bg, t):
    acc = 0
    for i, s in enumerate(scenes):
        if t < acc + s.dur or i == len(scenes) - 1:
            cv = bg.render(ring_center(scenes, t))
            s.render(cv, t - acc)
            return cv.convert("RGB")
        acc += s.dur


def segments(scenes):
    out, acc = [], 0
    for s in scenes:
        out.append((type(s).__name__, round(acc, 2), round(s.dur, 2)))
        acc += s.dur
    return out, acc


def encode(scenes, bg, W, H, path, fps=FPS, extra=None):
    total = sum(s.dur for s in scenes)
    n = int(round(total * fps))
    cmd = ["ffmpeg", "-y", "-v", "error", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", "%dx%d" % (W, H),
           "-r", str(fps), "-i", "-", "-an", "-c:v", "libx264", "-profile:v", "high", "-preset", "slow",
           "-crf", "20", "-tune", "animation", "-pix_fmt", "yuv420p", "-movflags", "+faststart",
           "-color_primaries", "bt709", "-color_trc", "bt709", "-colorspace", "bt709"] + (extra or []) + [path]
    p = subprocess.Popen(cmd, stdin=subprocess.PIPE)
    for k in range(n):
        p.stdin.write(frame_at(scenes, bg, k / fps).tobytes())
        if k % 300 == 0:
            print("  %s %d/%d" % (os.path.basename(path), k, n), flush=True)
    p.stdin.close()
    if p.wait() != 0:
        raise SystemExit("ffmpeg failed for " + path)


def normalize(raw, out):
    """screenrecord writes a frame only when the screen changes, stamped with its composition time. Put every
    recorded frame on the 60 fps grid at its time (one composed early, whose slot is taken, moves to the next
    slot), repeat the last frame over still stretches, and write a constant-rate clip."""
    def probe(entries):
        return subprocess.check_output(["ffprobe", "-v", "error", "-select_streams", "v:0", "-show_entries", entries,
                                        "-of", "csv=p=0", raw], text=True)
    w, h = [int(v) for v in probe("stream=width,height").strip().split(",")[:2]]
    times = [float(l.strip().rstrip(",")) for l in probe("frame=pts_time").splitlines() if l.strip()]
    t0 = times[0]
    slots, last, pushed = [], -1, 0
    for t in times:
        want = int(round((t - t0) * FPS))
        s = max(want, last + 1)
        pushed += s != want
        slots.append(s)
        last = s
    dec = subprocess.Popen(["ffmpeg", "-v", "error", "-i", raw, "-fps_mode", "passthrough", "-enc_time_base", "1:1000", "-f", "rawvideo",
                            "-pix_fmt", "rgb24", "-"], stdout=subprocess.PIPE)
    enc = subprocess.Popen(["ffmpeg", "-y", "-v", "error", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", "%dx%d" % (w, h),
                            "-r", str(FPS), "-i", "-", "-c:v", "libx264", "-preset", "medium", "-crf", "10",
                            "-pix_fmt", "yuv444p", out], stdin=subprocess.PIPE)
    n = w * h * 3
    prev, written, dropped = None, 0, 0
    for s in slots:
        buf = dec.stdout.read(n)
        if len(buf) < n:
            break
        while prev is not None and written < s:
            enc.stdin.write(prev)
            written += 1
        enc.stdin.write(buf)
        written += 1
        prev = buf
    enc.stdin.close()
    enc.wait()
    dec.kill()
    print("%s: %d recorded frames over %d slots (%.2f s), %d moved to a later slot" % (
        os.path.basename(out), len(slots), written, written / FPS, pushed))


def main():
    if len(sys.argv) == 4 and sys.argv[1] == "--normalize":
        return normalize(sys.argv[2], sys.argv[3])
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--clips", help="folder with the recorded clips (tv-home.mp4, hh-flow.mp4, ...)")
    ap.add_argument("--out", required=True)
    ap.add_argument("--cut", choices=["16x9", "9x16", "teaser", "all"], default="all")
    ap.add_argument("--frames", help="comma-separated seconds: write PNG review frames only")
    ap.add_argument("--maxrate", default="2600k", help="H.264 bitrate cap for the 16:9 and 9:16 cuts")
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)

    def make(cut, W, H):
        ctx = Ctx(W, H, a.clips)
        return build(cut, ctx, a.clips), Background(W, H)

    if a.frames:
        cut = "16x9" if a.cut == "all" else a.cut
        W, H = (1080, 1920) if cut == "9x16" else (1920, 1080)
        scenes, bg = make(cut, W, H)
        for t in [float(x) for x in a.frames.split(",")]:
            frame_at(scenes, bg, t).save(os.path.join(a.out, "%s-%05.2f.png" % (cut, t)))
        return

    rate = ["-maxrate", a.maxrate, "-bufsize", "8M"]
    if a.cut in ("16x9", "all"):
        scenes, bg = make("16x9", 1920, 1080)
        segs, total = segments(scenes)
        print("16x9: %.2f s" % total, segs)
        encode(scenes, bg, 1920, 1080, os.path.join(a.out, "trailer-16x9.mp4"), extra=rate)
        # Poster: the TV scene with the library in view.
        t_poster = segs[1][1] + 4.2
        frame_at(build("16x9", Ctx(1920, 1080, a.clips), a.clips), bg, t_poster).save(
            os.path.join(a.out, "poster.webp"), quality=90, method=6)
    if a.cut in ("9x16", "all"):
        scenes, bg = make("9x16", 1080, 1920)
        print("9x16: %.2f s" % segments(scenes)[1])
        encode(scenes, bg, 1080, 1920, os.path.join(a.out, "trailer-9x16.mp4"), extra=rate)
    if a.cut in ("teaser", "all"):
        scenes, bg = make("teaser", 1920, 1080)
        print("teaser: %.2f s" % segments(scenes)[1])
        tmp = os.path.join(a.out, "teaser-master.mp4")
        encode(scenes, bg, 1920, 1080, tmp, extra=["-crf", "14"])
        subprocess.check_call(["ffmpeg", "-y", "-v", "error", "-i", tmp, "-vf",
                               "fps=25,scale=960:-2:flags=lanczos", "-c:v", "libwebp_anim", "-loop", "0",
                               "-quality", "85", "-compression_level", "6", "-preset", "picture",
                               os.path.join(a.out, "teaser.webp")])


if __name__ == "__main__":
    sys.exit(main())
