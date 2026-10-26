#!/usr/bin/env python3
"""Rubylight's product film, rendered deterministically with Pillow.

No service, game, display or GPU access. Requires Pillow, NumPy and FFmpeg.
The console scenes use docs/media/console/*.jpg: the real web console with
mocked host replies and the made-up library of the README pictures, made by
capture_console.cjs (film mode). The game picture is the console's own Home
artwork, rust/web/src/assets/home-landscape.webp.

Every measured number and its caption lives in NUMBERS below, so a remeasured
value is a one-line change. Each number scene names its source in one line.

python docs/media/demo_audio.py --output score.wav --duration 70
python docs/media/render_launch_film.py --storyboard film.jpg
python docs/media/render_launch_film.py --frame 7.2 --output frame.png
python docs/media/render_launch_film.py --draft draft.mp4          # 800 wide, 10 fps
python docs/media/render_launch_film.py --gif-master gif.mkv       # the README GIF's frames
python docs/media/render_launch_film.py --output film.mp4 --audio score.wav --size-mb 2.9
"""
import argparse
from decimal import Decimal
from functools import lru_cache
import json
import math
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

import numpy as np
from PIL import Image, ImageChops, ImageDraw, ImageFilter, ImageFont

# Measured numbers ------------------------------------------------------------------
# Published values from README.md "Measured" and docs/performance.md. Swap a value
# here and every scene, count-up and bar follows. 'value' strings keep their
# decimals: the count-up shows as many places as the string has.
NUMBERS = {
    'latency': {
        'value': '7.41', 'unit': 'ms',
        'caption': 'from capture to a presented frame',
        'detail': 'Median · H.264 1080p60 · client on the same PC',
        'source': 'Frame timing record · RX 7900 XT · October 23, 2026 · docs/performance.md',
    },
    'versus': {
        # 'heading' is checked against the bars below: "half" needs ours under half of theirs.
        'heading': 'Less than half the picture age.',
        'subtitle': 'Mean picture age beside a game-like GPU load · lower is better',
        'unit': 'ms',
        'theirs': ('Vibepollo 2.0', '96'),
        'ours': ('Rubylight', '42'),
        'detail': 'HEVC 10-bit HDR 1080p60 · 20 Mbps · three runs each, same PC and settings',
        'source': 'Rubylight 2.0.0-rc.2 and Vibepollo 2.0 · RX 7900 XT · October 4, 2026 · docs/performance.md',
    },
    'more': {
        'heading': 'Fresh frames on every link.',
        'subtitle': 'With variable refresh, over Wi-Fi and in the browser.',
        'tiles': (
            {'value': '0.27', 'unit': 'ms', 'caption': 'picture age at the encoder',
             'detail': 'with VRR, from 4.68 ms · 120 Hz game'},
            {'value': '4', 'unit': '×', 'caption': 'cleaner 2.4 GHz Wi-Fi',
             'detail': 'lost frames per run, 61 to 15.7'},
            {'value': '17', 'unit': 'ms', 'caption': 'glass to glass in a browser',
             'detail': 'WebRTC median · AV1 1080p60'},
        ),
        'source': 'VRR, Wi-Fi pacing and WebRTC runs · RX 7900 XT · October 24–25, 2026 · docs/performance.md',
    },
}


def check_numbers():
    """The words around the numbers must stay true when the numbers change."""
    versus = NUMBERS['versus']
    ours, theirs = Decimal(versus['ours'][1]), Decimal(versus['theirs'][1])
    assert ours < theirs, 'the comparison scene shows Rubylight ahead'
    if 'half' in versus['heading']:
        assert ours*2 < theirs, f"'{versus['heading']}' needs {ours} under half of {theirs}"


check_numbers()

# Timeline ------------------------------------------------------------------------
# (scene, start, end, transition into it). 120 BPM: every cut lands on a bar line.
TIMELINE = (
    ('hero', 0, 8, None),
    ('home', 8, 14, 'wipe'),
    ('library', 14, 18, 'push'),
    ('add_games', 18, 24, 'push'),
    ('radeon', 24, 30, 'push'),
    ('pair', 30, 36, 'push'),
    ('colour', 36, 42, 'fade'),
    ('displays', 42, 46, 'fade'),
    ('latency', 46, 50, 'wipe'),
    ('versus', 50, 56, 'fade'),
    ('more', 56, 62, 'fade'),
    ('end', 62, 70, 'wipe'),
)
DURATION = TIMELINE[-1][2]
STARTS = tuple(s for _, s, _, _ in TIMELINE)
TRANSITION = .6
# A still of the opening once every device shows the game: the website poster.
POSTER_AT = 7.4

W, H = 1920, 1080
BG = (13, 13, 17)
BG_WARM = (33, 14, 24)
INK = (240, 240, 243)
MUTED = (150, 155, 168)
RUBY = (255, 64, 108)
RUBY_DEEP = (196, 18, 66)
ROSE = (255, 138, 168)
GREY = (122, 128, 142)
DIM = (66, 70, 84)
LINE = (46, 50, 60)
PANEL = (22, 24, 30)
OUTLINE = (238, 53, 105)
MEDIA = Path(__file__).resolve().parent
LANDSCAPE = MEDIA.parent.parent/'rust'/'web'/'src'/'assets'/'home-landscape.webp'


# The README GIF is rendered with STILL set: cuts instead of transitions and
# camera moves, and no drifting or sliding, so that a GIF frame only stores the
# pixels that really change. The film keeps every move.
STILL = False


def slide(px):
    return 0 if STILL else px


def clamp(x):
    return min(1., max(0., x))


def smooth(x):
    x = clamp(x)
    return x*x*(3-2*x)


def ease(x):
    x = clamp(x)
    return 1-(1-x)**3


def ease_io(x):
    x = clamp(x)
    return 4*x**3 if x < .5 else 1-(-2*x+2)**3/2


def mix(a, b, p):
    return tuple(round(x+(y-x)*clamp(p)) for x, y in zip(a, b))


def lerp(a, b, p):
    return a+(b-a)*p


# Type ------------------------------------------------------------------------------

FONT_ROOTS = (Path('C:/Windows/Fonts'), Path('/mnt/c/Windows/Fonts'))


@lru_cache(maxsize=None)
def font(size, weight='regular'):
    for root in FONT_ROOTS:
        if (root/'SegUIVar.ttf').exists():
            f = ImageFont.truetype(str(root/'SegUIVar.ttf'), size)
            f.set_variation_by_name({'light': 'Light Display', 'regular': 'Regular Display',
                                     'semibold': 'Semibold Display', 'bold': 'Bold Display'}[weight])
            return f
    name = 'DejaVuSans-Bold.ttf' if weight in ('bold', 'semibold') else 'DejaVuSans.ttf'
    return ImageFont.truetype('/usr/share/fonts/truetype/dejavu/'+name, size)


@lru_cache(maxsize=4000)
def lettering(value, size, color, weight, spacing=0):
    f = font(size, weight)
    if spacing:
        widths = [f.getlength(c) for c in value]
        width = round(sum(widths)+spacing*(len(value)-1))+8
        ascent, descent = f.getmetrics()
        im = Image.new('RGBA', (width, ascent+descent+4))
        x = 2.
        for c, w in zip(value, widths):
            ImageDraw.Draw(im).text((x, 2), c, font=f, fill=color)
            x += w+spacing
        return im.crop(im.getbbox() or (0, 0, 1, 1))
    box = f.getbbox(value)
    im = Image.new('RGBA', (max(1, box[2]-box[0]+4), max(1, box[3]-box[1]+4)))
    ImageDraw.Draw(im).text((2-box[0], 2-box[1]), value, font=f, fill=color)
    return im


def width_of(value, size, weight='regular'):
    return lettering(value, size, INK, weight).width


def cap_height(size, weight='bold'):
    return lettering('H', size, INK, weight).height


def txt(im, x, y, value, size=32, color=INK, weight='regular', align='left', opacity=1., spacing=0):
    """Text whose capital letters' top sits at y."""
    if opacity <= 0 or not value:
        return
    stamp = lettering(value, size, color, weight, spacing)
    # Line up on the cap height rather than on each string's own ink box.
    y += lettering('H', size, color, weight).getbbox()[1]-stamp.getbbox()[1] if not spacing else 0
    top = font(size, weight).getbbox(value)[1]-font(size, weight).getbbox('H')[1]
    y += top
    if align == 'center':
        x -= stamp.width/2
    elif align == 'right':
        x -= stamp.width
    if opacity < 1:
        stamp = stamp.copy()
        stamp.putalpha(stamp.getchannel('A').point(lambda v: round(v*opacity)))
    im.paste(stamp, (round(x), round(y)), stamp)


def reveal(im, x, y, value, size, local, at=0., color=INK, weight='bold', align='left', rise=26, spacing=0):
    p = ease((local-at)/.6)
    txt(im, x, y+rise*(1-p), value, size, color, weight, align, p, spacing)


# Shapes ----------------------------------------------------------------------------

@lru_cache(maxsize=512)
def rr_mask(w, h, radius, width=0):
    """An antialiased rounded rectangle (or its outline) as an L mask."""
    k = 4
    big = Image.new('L', (w*k, h*k), 0)
    d = ImageDraw.Draw(big)
    d.rounded_rectangle((0, 0, w*k-1, h*k-1), radius=radius*k, fill=255)
    if width:
        d.rounded_rectangle((width*k, width*k, w*k-1-width*k, h*k-1-width*k),
                            radius=max(0, (radius-width)*k), fill=0)
    return big.resize((w, h), Image.Resampling.BOX)


def faded(mask, opacity):
    return mask if opacity >= 1 else mask.point(lambda v: round(v*opacity))


def rrect(im, box, fill=None, outline=None, width=2, radius=16, opacity=1.):
    if opacity <= 0:
        return
    x0, y0, x1, y1 = (round(v) for v in box)
    w, h = x1-x0, y1-y0
    if w < 2 or h < 2:
        return
    radius = min(radius, w//2, h//2)
    if fill is not None:
        im.paste(fill, (x0, y0, x1, y1), faded(rr_mask(w, h, radius), opacity))
    if outline is not None:
        im.paste(outline, (x0, y0, x1, y1), faded(rr_mask(w, h, radius, width), opacity))


def line(im, xy, fill=LINE, width=2):
    ImageDraw.Draw(im).line(tuple(round(v) for v in xy), fill=fill, width=width)


@lru_cache(maxsize=8)
def glow_sprite(size, color):
    yy, xx = np.mgrid[0:size, 0:size]
    d = np.hypot(xx-size/2+.5, yy-size/2+.5)/(size/2)
    a = np.clip(1-d, 0, 1)**2.2
    out = np.stack([a*c for c in color], -1)
    return Image.fromarray(np.round(out).astype(np.uint8))


def glow(im, x, y, size, color=RUBY_DEEP, strength=1.):
    """Add light: the sprite is added, never pasted, so glows can overlap."""
    sprite = glow_sprite(size, tuple(round(c*strength) for c in color))
    x0, y0 = round(x-size/2), round(y-size/2)
    box = (max(0, x0), max(0, y0), min(W, x0+size), min(H, y0+size))
    if box[2] <= box[0] or box[3] <= box[1]:
        return
    part = sprite.crop((box[0]-x0, box[1]-y0, box[2]-x0, box[3]-y0))
    im.paste(ImageChops.add(im.crop(box), part), box[:2])


@lru_cache(maxsize=1)
def backdrop():
    """The banner's diagonal: near black to a warm ruby dark, with a faint dot grid."""
    yy, xx = np.mgrid[0:H, 0:W].astype(np.float32)
    p = np.clip((xx/W*.55+yy/H*.45), 0, 1)**1.6
    a, b = np.array(BG, np.float32), np.array(BG_WARM, np.float32)
    rgb = a+(b-a)*p[..., None]
    im = Image.fromarray(np.round(rgb).astype(np.uint8))
    d = ImageDraw.Draw(im)
    for y in range(40, H, 56):
        for x in range(40, W, 56):
            d.point((x, y), fill=(32, 30, 40))
    return im


# Where the warm light sits in each scene. It holds still inside a scene so the
# README GIF only changes what moves.
GLOWS = {'hero': (1500, 300), 'home': (1700, 200), 'library': (300, 160), 'add_games': (1650, 160),
         'radeon': (240, 200), 'pair': (1600, 200), 'colour': (960, 980), 'displays': (1500, 860),
         'latency': (1500, 560), 'versus': (1650, 300), 'more': (300, 900), 'end': (960, 560)}


@lru_cache(maxsize=16)
def base_for(name):
    im = backdrop().copy()
    x, y = GLOWS[name]
    glow(im, x, y, 1500, RUBY_DEEP, .42)
    return im


def badge(im, x, y, size, opacity=1.):
    if opacity <= 0:
        return
    rrect(im, (x, y, x+size, y+size), RUBY_DEEP, radius=round(size*.24), opacity=opacity)
    stamp = lettering('R', round(size*.64), INK, 'bold')
    im.paste(*faded_stamp(stamp, opacity, x+(size-stamp.width)/2, y+(size-stamp.height)/2))


def faded_stamp(stamp, opacity, x, y):
    if opacity < 1:
        stamp = stamp.copy()
        stamp.putalpha(stamp.getchannel('A').point(lambda v: round(v*opacity)))
    return stamp, (round(x), round(y)), stamp


def brand(im, opacity=1.):
    """The small mark in the top-right corner of every middle scene."""
    w = width_of('Rubylight', 30, 'bold')
    badge(im, 1824-w-52, 52, 40, opacity)
    txt(im, 1824-w, 58, 'Rubylight', 30, INK, 'bold', opacity=opacity)


def heading(im, value, local, subtitle=None, at=.1):
    reveal(im, 100, 92, value, 64, local, at=at)
    if subtitle:
        reveal(im, 102, 180, subtitle, 32, local, at=at+.15, color=MUTED, weight='regular', rise=14)


def source(im, value, local=99., at=0.):
    """The one line that says where a measurement comes from."""
    p = ease((local-at)/.5)
    if p <= 0:
        return
    rrect(im, (100, 1020, 110, 1030), RUBY, radius=5, opacity=p)
    txt(im, 124, 1014, value, 26, MUTED, 'regular', opacity=p)


def chip(im, x, y, label, size=28, color=INK, fill=PANEL, outline=LINE, opacity=1., weight='semibold'):
    """A rounded label; returns its right edge."""
    w = width_of(label, size, weight)+size*1.3
    h = size*1.8
    rrect(im, (x, y, x+w, y+h), fill, outline, 2, round(h/2), opacity)
    txt(im, x+size*.65, y+(h-cap_height(size, weight))/2, label, size, color, weight, opacity=opacity)
    return x+w


# The game picture ------------------------------------------------------------------

@lru_cache(maxsize=1)
def landscape():
    im = Image.open(LANDSCAPE).convert('RGB')
    # The right part: castle, sunset and the figure on the cliff, at 16:9.
    h = im.height
    w = round(h*16/9)
    return im.crop((im.width-w, 0, im.width, h))


@lru_cache(maxsize=32)
def picture(w, h, focus=.62):
    """The game picture cropped to w x h around the castle."""
    src = landscape()
    aspect = w/h
    sw, sh = src.size
    if sw/sh > aspect:
        cw = sh*aspect
        x0 = clamp((sw*focus-cw/2)/(sw-cw))*(sw-cw)
        box = (x0, 0, x0+cw, sh)
    else:
        ch = sw/aspect
        box = (0, (sh-ch)/2, sw, (sh+ch)/2)
    return src.resize((w, h), Image.Resampling.LANCZOS, box=box)


# Console shots ---------------------------------------------------------------------

@lru_cache(maxsize=None)
def shot(name):
    return Image.open(MEDIA/'console'/f'{name}.jpg').convert('RGB')


@lru_cache(maxsize=1)
def boxes():
    return json.loads((MEDIA/'console'/'boxes.json').read_text(encoding='utf-8'))


@lru_cache(maxsize=1)
def console_bg():
    return shot('radeon').getpixel((2860, 1790))


# The console window: below the heading, running off the bottom of the frame.
VP = (100, 262, 1820, 1080)


def fit(name, box, pad=40, vp=VP, anchor=None):
    """Camera (cx, cy, scale) that shows box, padded, in the viewport."""
    vw, vh = vp[2]-vp[0], vp[3]-vp[1]
    x0, y0, x1, y1 = box[0]-pad, box[1]-pad, box[2]+pad, box[3]+pad
    s = min(vw/(x1-x0), vh/(y1-y0))
    cx, cy = (x0+x1)/2, (y0+y1)/2
    if anchor == 'top':
        cy = y0+vh/2/s
    return keep_inside(name, (cx, cy, s), vp)


def wide(name, top=0, vp=VP):
    """The whole width of the page, from top down."""
    vw, vh = vp[2]-vp[0], vp[3]-vp[1]
    iw, _ = shot(name).size
    s = vw/iw
    return keep_inside(name, (iw/2, top+vh/2/s, s), vp)


def keep_inside(name, camera, vp=VP):
    cx, cy, s = camera
    iw, ih = shot(name).size
    hw, hh = (vp[2]-vp[0])/2/s, (vp[3]-vp[1])/2/s
    cx = iw/2 if hw*2 >= iw else min(max(cx, hw), iw-hw)
    cy = ih/2 if hh*2 >= ih else min(max(cy, hh), ih-hh)
    return (cx, cy, s)


def blend_camera(a, b, p):
    p = ease_io(p)
    s = math.exp(lerp(math.log(a[2]), math.log(b[2]), p))
    return (lerp(a[0], b[0], p), lerp(a[1], b[1], p), s)


def camera_at(keys, local):
    if STILL:
        # Cut at the middle of each move.
        return [k for i, k in enumerate(keys) if i == 0 or local >= (keys[i-1][0]+k[0])/2][-1][1]
    if local <= keys[0][0]:
        return keys[0][1]
    for (t0, a), (t1, b) in zip(keys, keys[1:]):
        if local < t1:
            return blend_camera(a, b, (local-t0)/(t1-t0))
    return keys[-1][1]


@lru_cache(maxsize=8)
def view(name, cx, cy, s, vp=VP):
    src = shot(name)
    vw, vh = vp[2]-vp[0], vp[3]-vp[1]
    sx0, sy0 = cx-vw/2/s, cy-vh/2/s
    out = Image.new('RGB', (vw, vh), console_bg())
    ix0, iy0 = max(0., sx0), max(0., sy0)
    ix1, iy1 = min(src.width, cx+vw/2/s), min(src.height, cy+vh/2/s)
    dx0, dy0 = round((ix0-sx0)*s), round((iy0-sy0)*s)
    dx1, dy1 = round((ix1-sx0)*s), round((iy1-sy0)*s)
    if dx1 > dx0 and dy1 > dy0:
        out.paste(src.resize((dx1-dx0, dy1-dy0), Image.Resampling.BICUBIC, box=(ix0, iy0, ix1, iy1),
                             reducing_gap=2.), (dx0, dy0))
    return out


@lru_cache(maxsize=4)
def window_mask(vp=VP):
    w, h = vp[2]-vp[0], vp[3]-vp[1]
    mask = rr_mask(w, h+40, 22).crop((0, 0, w, h))
    return mask


def to_screen(box, camera, vp=VP):
    cx, cy, s = camera
    ox, oy = (vp[0]+vp[2])/2, (vp[1]+vp[3])/2
    return (ox+(box[0]-cx)*s, oy+(box[1]-cy)*s, ox+(box[2]-cx)*s, oy+(box[3]-cy)*s)


def console_window(im, name, camera, vp=VP, rise=0.):
    cx, cy, s = camera
    pane = view(name, round(cx, 1), round(cy, 1), round(s, 4), vp)
    y = vp[1]+round(rise)
    glow(im, (vp[0]+vp[2])/2, y+260, 1400, RUBY_DEEP, .18)
    im.paste(pane, (vp[0], y), window_mask(vp))
    w, h = vp[2]-vp[0], vp[3]-vp[1]
    outline = rr_mask(w+4, h+44, 24, 2).crop((0, 0, w+4, h+2))
    im.paste((70, 62, 78), (vp[0]-2, y-2), outline)


def ring(im, box, opacity, pad=10, color=RUBY):
    x0, y0, x1, y1 = box[0]-pad, box[1]-pad, box[2]+pad, box[3]+pad
    rrect(im, (x0, y0, x1, y1), None, color, 4, 16, opacity)


def sample_note(im):
    """The console is real; its games and stream are a sample."""
    txt(im, 1820, 120, 'The real console, with a sample library', 24, MUTED, 'regular', 'right')


def console_scene(im, name, local, keys, title, subtitle):
    brand(im)
    heading(im, title, local, subtitle)
    sample_note(im)
    p = ease(local/.7)
    camera = camera_at(keys, local)
    console_window(im, name, camera, rise=slide(40*(1-p)))
    return camera


# 1 · Hero ------------------------------------------------------------------------

# Device line art: (kind, outer box, screen box or None, label).
DEVICES = (
    ('laptop', (130, 490, 400, 660), (130, 490, 400, 660), 'Laptop'),
    ('phone', (474, 560, 664, 666), (487, 569, 651, 657), 'Phone'),
    ('tv', (690, 330, 1230, 680), (700, 340, 1220, 632), 'TV'),
    ('handheld', (1280, 544, 1570, 666), (1352, 556, 1498, 654), 'Steam Deck'),
    ('console', (1616, 486, 1836, 666), None, 'Xbox'),
)
TV_SCREEN = DEVICES[2][2]


@lru_cache(maxsize=1)
def device_art():
    """The devices drawn once at 3x and scaled down, like the banner's outlines."""
    k = 3
    art = Image.new('RGBA', (W*k, H*k))
    d = ImageDraw.Draw(art)
    stroke, fill = OUTLINE+(255,), (30, 13, 22, 255)
    sw = 3*k

    def R(box, r, outline=stroke, f=fill, width=sw):
        d.rounded_rectangle(tuple(v*k for v in box), radius=r*k, fill=f, outline=outline, width=width)

    for kind, outer, screen, _ in DEVICES:
        x0, y0, x1, y1 = outer
        if kind == 'laptop':
            R((x0-10, y0-10, x1+10, y1+8), 12)
            d.polygon([((x0-34)*k, (y1+10)*k), ((x1+34)*k, (y1+10)*k), ((x1+52)*k, (y1+24)*k),
                       ((x0-52)*k, (y1+24)*k)], fill=fill, outline=stroke, width=sw)
        elif kind == 'phone':
            R((x0, y0, x1, y1), 22)
        elif kind == 'tv':
            R((x0, y0, x1, y1-38), 14)
            d.line(((960-50)*k, (y1-38)*k, (960-80)*k, (y1-4)*k), fill=stroke, width=sw)
            d.line(((960+50)*k, (y1-38)*k, (960+80)*k, (y1-4)*k), fill=stroke, width=sw)
        elif kind == 'handheld':
            R((x0, y0, x1, y1), 52)
            for cx in (x0+38, x1-38):
                d.ellipse(((cx-17)*k, (y0+30)*k, (cx+17)*k, (y0+64)*k), outline=stroke, width=sw)
                d.line(((cx-12)*k, (y0+96)*k, (cx+12)*k, (y0+96)*k), fill=stroke, width=sw)
                d.line((cx*k, (y0+84)*k, cx*k, (y0+108)*k), fill=stroke, width=sw)
        elif kind == 'console':
            R((x0+8, y0, x0+100, y1), 14)
            d.ellipse(((x0+38)*k, (y0+22)*k, (x0+70)*k, (y0+54)*k), outline=stroke, width=sw)
            # A controller beside it.
            cx, cy = x0+164, y1-38
            d.rounded_rectangle(((cx-56)*k, (cy-30)*k, (cx+56)*k, (cy+20)*k), radius=24*k, fill=fill,
                                outline=stroke, width=sw)
            d.ellipse(((cx-58)*k, (cy-6)*k, (cx-18)*k, (cy+38)*k), fill=fill, outline=stroke, width=sw)
            d.ellipse(((cx+18)*k, (cy-6)*k, (cx+58)*k, (cy+38)*k), fill=fill, outline=stroke, width=sw)
            d.rectangle(((cx-40)*k, (cy-24+sw/k)*k, (cx+40)*k, (cy+12)*k), fill=fill)
            d.ellipse(((cx-34)*k, (cy-16)*k, (cx-18)*k, cy*k), outline=stroke, width=sw)
            d.ellipse(((cx+18)*k, (cy-16)*k, (cx+34)*k, cy*k), outline=stroke, width=sw)
        if screen:
            d.rectangle(tuple(v*k for v in screen), fill=(17, 12, 18, 255))
    return art.resize((W, H), Image.Resampling.LANCZOS)


def lit_screen(im, box, level):
    if level <= 0:
        return
    x0, y0, x1, y1 = box
    pic = picture(x1-x0, y1-y0)
    if level < 1:
        flash = mix((17, 12, 18), RUBY, 1-abs(level*2-1)) if level < .5 else None
        under = Image.new('RGB', pic.size, flash or (17, 12, 18))
        pic = Image.blend(under, pic, smooth((level-.3)/.7))
    im.paste(pic, (x0, y0))


def devices(im, local, appear, lit_at):
    art = device_art()
    p = ease((local-appear)/.6)
    if p <= 0:
        return
    for i, (kind, outer, screen, label) in enumerate(DEVICES):
        cx = (outer[0]+outer[2])/2
        level = clamp((local-lit_at-i*.16)/.5)
        if level > 0:
            glow(im, cx, (outer[1]+outer[3])/2, 520, RUBY_DEEP, .45*level)
    im.paste(art, (0, round(30*(1-p))), faded(art.getchannel('A'), p))
    for i, (kind, outer, screen, label) in enumerate(DEVICES):
        level = clamp((local-lit_at-i*.16)/.5)
        if screen and kind != 'tv':
            lit_screen(im, (screen[0], screen[1]+round(30*(1-p)), screen[2], screen[3]+round(30*(1-p))), level)
        if kind == 'console' and level > 0:
            x0, y0 = outer[0], outer[1]
            ImageDraw.Draw(im).ellipse((x0+46, y0+30, x0+62, y0+46), fill=mix((30, 13, 22), ROSE, level))
        txt(im, (outer[0]+outer[2])/2, 722, label, 30, mix(BG, INK, p*(.55+.45*level)), 'semibold', 'center')


@lru_cache(maxsize=1)
def hero_shade():
    """Darkens the left of the full-bleed picture so the promise reads."""
    x = np.linspace(0, 1, W, dtype=np.float32)
    a = np.clip(1-(x/.72), 0, 1)**1.1*245
    y = np.linspace(0, 1, H, dtype=np.float32)
    a = np.maximum(a[None, :], (np.clip((y-.72)/.28, 0, 1)**2*150)[:, None])
    return Image.fromarray(np.round(a).astype(np.uint8), 'L')


def hero(t):
    im = base_for('hero').copy()
    # 0-3.6 s: the game, full screen, under the promise. 3.6-4.6 s: it settles into the TV.
    shrink = ease_io((t-3.5)/1.1)
    zoom = 1+slide(.07*smooth(t/4.6))
    src = landscape()
    sw, sh = src.size
    cw, ch = sw/zoom, sh/zoom
    cx, cy = sw*.6, sh*.46
    box = (cx-cw*.6, cy-ch*.46, cx+cw*.4, cy+ch*.54)
    full = (0, 0, W, H)
    target = TV_SCREEN
    rect = tuple(lerp(a, b, shrink) for a, b in zip(full, target))
    devices(im, t, appear=3.9, lit_at=4.7)
    if shrink < 1:
        w, h = round(rect[2]-rect[0]), round(rect[3]-rect[1])
        pic = src.resize((w, h), Image.Resampling.BICUBIC, box=box, reducing_gap=2.)
        shade = hero_shade().resize((w, h)) if shrink < .99 else None
        if shade is not None:
            dark = Image.new('RGB', (w, h), BG)
            pic = Image.composite(dark, pic, faded(shade, 1-shrink))
        if shrink > 0:
            mask = rr_mask(w, h, round(lerp(0, 4, shrink)))
            im.paste(pic, (round(rect[0]), round(rect[1])), mask)
        else:
            im.paste(pic, (0, 0))
    else:
        im.paste(picture(target[2]-target[0], target[3]-target[1]), target[:2])
    # The promise, over the picture.
    out = 1-smooth((t-3.1)/.5)
    if out > 0:
        p = ease((t-.3)/.7)
        badge(im, 100, 300-0*p, 64, p*out)
        txt(im, 184, 312, 'Rubylight', 44, INK, 'bold', opacity=p*out)
        for i, (value, color) in enumerate((('Your Radeon PC.', INK), ('On every screen.', RUBY))):
            q = ease((t-.55-i*.3)/.7)
            txt(im, 96, 420+i*136+30*(1-q), value, 120, color, 'bold', opacity=q*out)
        q = ease((t-1.5)/.7)
        txt(im, 102, 720+14*(1-q), 'Every game, on any Moonlight client.', 40,
            (222, 220, 228), 'regular', opacity=q*out)
    # Then: what it is, over every device.
    reveal(im, 960, 104, 'The game-streaming host built for Radeon.', 68, t, at=4.2, align='center')
    reveal(im, 960, 200, 'HDR, full-colour 4:4:4 and up to 240 fps, on every screen you own.', 34, t, at=4.4,
           color=MUTED, weight='regular', align='center', rise=14)
    q = ease((t-5.6)/.6)
    if q > 0:
        labels = ('Free and open source', 'Windows 10 and 11', 'Any AMD Radeon with AMF')
        widths = [width_of(v, 28, 'semibold')+28*1.3 for v in labels]
        x = 960-(sum(widths)+24*2)/2
        for v, w in zip(labels, widths):
            chip(im, x, 860+16*(1-q), v, 28, (222, 220, 228), (24, 18, 26), (74, 44, 60), q)
            x += w+24
    return im


# 2 · Console -----------------------------------------------------------------------

def home(t):
    im = base_for('home').copy()
    b = boxes()['home']
    keys = ((0, wide('home')), (1.4, wide('home')), (2.8, fit('home', b['card'], 30)),
            (3.9, fit('home', b['card'], 30)), (5.3, fit('home', b['shelf'], 30)))
    camera = console_scene(im, 'home', t, keys, 'Your PC, ready to play.',
                           'Home shows every live stream, the running game and your shelf.')
    ring(im, to_screen(b['spec'], camera), smooth((t-3.0)/.4)*(1-smooth((t-4.1)/.3)), pad=8)
    return im


def library(t):
    im = base_for('library').copy()
    b = boxes()['library']
    g = b['grid']
    first_row = (g[0], g[1], g[2], g[1]+(g[3]-g[1])*.5)
    keys = ((0, wide('library', 0)), (.8, wide('library', 0)), (3.4, fit('library', first_row, 24)))
    console_scene(im, 'library', t, keys, 'Your whole library, with covers.',
                  'Every game on the PC, one click from the couch.')
    return im


LAUNCHERS = ('Steam', 'Epic Games', 'Battle.net', 'EA app', 'Ubisoft Connect', 'GOG', 'Xbox')


def add_games(t):
    im = base_for('add_games').copy()
    b = boxes()['add-games']
    panel = b['panel']
    keys = ((0, wide('add-games', 0)), (.8, wide('add-games', 0)),
            (2.6, fit('add-games', (panel[0], panel[1], panel[2], panel[1]+1150), 20, anchor='top')))
    brand(im)
    heading(im, 'Every launcher in one scan.', t)
    sample_note(im)
    p = ease(t/.7)
    console_window(im, 'add-games', camera_at(keys, t), rise=slide(40*(1-p)))
    # The launchers, one by one, where the subtitle would be.
    x = 100
    for i, name in enumerate(LAUNCHERS):
        q = ease((t-.4-i*.14)/.45)
        x = chip(im, x, 172+12*(1-q), name, 26, INK, (40, 18, 30), (120, 40, 70), q) + 12
    return im


def radeon(t):
    im = base_for('radeon').copy()
    b = boxes()['radeon']['radeon']
    keys = ((0, wide('radeon', 300)), (.9, wide('radeon', 300)), (2.6, fit('radeon', b, 30)))
    camera = console_scene(im, 'radeon', t, keys, 'Radeon settings for every game.',
                           'Anti-Lag, frame caps and filtering at launch, put back when you stop.')
    profile = (b[0]+50, b[1]+250, b[2]-50, b[3]-50)
    ring(im, to_screen(profile, camera), smooth((t-3.0)/.4), pad=12)
    return im


def phone(im, x, y, local, paired_at, opacity):
    """A phone in a Moonlight client, showing the PIN and then the game."""
    w, h = 300, 560
    y += 30*(1-opacity)
    rrect(im, (x, y, x+w, y+h), (24, 16, 22), OUTLINE, 3, 44, opacity)
    sx0, sy0, sx1, sy1 = x+16, y+50, x+w-16, y+h-50
    rrect(im, (sx0, sy0, sx1, sy1), (14, 14, 18), None, radius=18, opacity=opacity)
    rrect(im, (x+w/2-30, y+22, x+w/2+30, y+30), OUTLINE, radius=4, opacity=opacity)
    done = smooth((local-paired_at)/.5)
    if done < 1:
        o = opacity*(1-done)
        txt(im, x+w/2, sy0+40, 'Living room PC', 28, INK, 'semibold', 'center', o)
        txt(im, x+w/2, sy0+84, 'Enter this PIN on the PC', 22, MUTED, 'regular', 'center', o)
        txt(im, x+w/2, sy0+170, '4821', 76, INK, 'bold', 'center', o, spacing=10)
        rrect(im, (sx0+40, sy0+290, sx1-40, sy0+296), (60, 44, 54), radius=3, opacity=o)
        frac = 1-clamp((local-.6)/(paired_at-.6))
        rrect(im, (sx0+40, sy0+290, sx0+40+(sx1-sx0-80)*frac, sy0+296), RUBY, radius=3, opacity=o)
    if done > 0:
        pic = picture(round(sx1-sx0), round(sy1-sy0), .6)
        mask = faded(rr_mask(pic.width, pic.height, 18), done*opacity)
        im.paste(pic, (round(sx0), round(sy0)), mask)
        p = ease((local-paired_at-.2)/.5)
        cx, cy = x+w/2, y+h+10
        rrect(im, (cx-110, cy-30, cx+110, cy+30), RUBY, None, radius=30, opacity=p)
        txt(im, cx, cy-cap_height(30)/2-1, 'Paired', 30, (24, 10, 16), 'bold', 'center', p)


def pair(t):
    im = base_for('pair').copy()
    b = boxes()['pair']
    vp = (480, 262, 1820, 1080)
    steps = b['steps']
    keys = ((0, fit('pair', b['panel'], 30, vp)), (1.0, fit('pair', b['panel'], 30, vp)),
            (2.2, fit('pair', (b['panel'][0], steps[1]-120, b['panel'][2], b['request'][3]+40), 20, vp)))
    brand(im)
    heading(im, 'Pair in seconds.', t, 'Moonlight shows a PIN. Type it in, and you are in.')
    sample_note(im)
    p = ease(t/.7)
    camera = camera_at(keys, t)
    console_window(im, 'pair', camera, vp, rise=slide(40*(1-p)))
    # The PIN, typed digit by digit: the filled field from the second shot.
    typed = min(4, max(0, math.floor((t-2.4)/.22)+1))
    pin = b['pin']
    if typed:
        x0, y0, x1, y1 = (round(v) for v in to_screen((pin[0]-6, pin[1]-6, pin[2]+6, pin[3]+6), camera, vp))
        field = view('pair-pin', round(camera[0], 1), round(camera[1], 1), round(camera[2], 4), vp)
        ox, oy = vp[0], vp[1]+round(slide(40*(1-p)))
        crop = field.crop((x0-ox, y0-oy, x1-ox, y1-oy))
        # Digits sit evenly across the field; reveal one quarter per keystroke.
        if typed < 4:
            empty = view('pair', round(camera[0], 1), round(camera[1], 1), round(camera[2], 4), vp)
            blank = empty.crop((x0-ox, y0-oy, x1-ox, y1-oy))
            cut = round(crop.width*(.18+.2*typed))
            blank.paste(crop.crop((0, 0, cut, crop.height)), (0, 0))
            crop = blank
        im.paste(crop, (x0, y0))
    paired_at = 3.7
    request = to_screen(b['request'], camera, vp)
    ring(im, request, smooth((t-paired_at+.15)/.3)*(1-smooth((t-paired_at-1.2)/.4)), pad=6)
    phone(im, 110, 330, t, paired_at, ease((t-.3)/.7))
    return im


# 3 · Picture and displays ----------------------------------------------------------

HUD_BG = (22, 24, 32)


@lru_cache(maxsize=1)
def hud():
    """The same rendered text as 4:4:4 and as 4:2:0 (colour shared by 2x2 pixels)."""
    im = Image.new('RGB', (240, 108), HUD_BG)
    d = ImageDraw.Draw(im)
    f = font(17, 'semibold')
    d.text((10, 8), 'Objective captured', font=f, fill=(236, 236, 236))
    d.text((10, 42), 'Low ammo', font=f, fill=(240, 36, 52))
    d.text((10, 76), 'Ally ready', font=f, fill=(60, 118, 255))
    rgb = np.asarray(im, np.float64)/255
    r, g, b = rgb[..., 0], rgb[..., 1], rgb[..., 2]
    # BT.709 luma and colour differences on the gamma-encoded values, as an encoder sees them.
    y = .2126*r+.7152*g+.0722*b
    cb, cr = (b-y)/1.8556, (r-y)/1.5748

    def shared(plane):
        h, w = plane.shape
        small = plane.reshape(h//2, 2, w//2, 2).mean(axis=(1, 3)).astype(np.float32)
        return np.asarray(Image.fromarray(small, 'F').resize((w, h), Image.Resampling.BILINEAR), np.float64)
    cb, cr = shared(cb), shared(cr)
    r2, b2 = y+1.5748*cr, y+1.8556*cb
    g2 = (y-.2126*r2-.0722*b2)/.7152
    out = np.clip(np.stack([r2, g2, b2], -1), 0, 1)
    return Image.fromarray(np.round(out*255).astype(np.uint8)), im


PANELS = ((100, 300, 940, 860), (980, 300, 1820, 860))


def magnified(source_image, box, cx, cy, zoom):
    w, h = box[2]-box[0], box[3]-box[1]
    panel = Image.new('RGB', (w, h), HUD_BG)
    sx0, sy0 = cx-w/2/zoom, cy-h/2/zoom
    ix0, iy0 = max(0, math.ceil(sx0)), max(0, math.ceil(sy0))
    ix1, iy1 = min(source_image.width, math.floor(cx+w/2/zoom)), min(source_image.height, math.floor(cy+h/2/zoom))
    dx0, dy0 = round((ix0-sx0)*zoom), round((iy0-sy0)*zoom)
    dx1, dy1 = round((ix1-sx0)*zoom), round((iy1-sy0)*zoom)
    part = source_image.resize((dx1-dx0, dy1-dy0), Image.Resampling.NEAREST, box=(ix0, iy0, ix1, iy1))
    panel.paste(part, (dx0, dy0))
    return panel


def colour(t):
    im = base_for('colour').copy()
    brand(im)
    heading(im, 'Full colour. Bright HDR.', t,
            '10-bit HDR in HEVC and AV1, and full-colour 4:4:4 with PyroWave for crisp text.')
    sampled, full = hud()
    p = smooth((t-1.0)/1.8) if not STILL else float(t > 1.9)
    zoom = math.exp(lerp(math.log(3.4), math.log(8.), p))
    cx, cy = lerp(120, 60, p), lerp(54, 68, p)
    for i, (box, image, label, color) in enumerate(zip(PANELS, (sampled, full), ('4:2:0', '4:4:4 · PyroWave'),
                                                       (MUTED, RUBY))):
        q = ease((t-.3-i*.15)/.6)
        if q <= 0:
            continue
        y = round(slide(24*(1-q)))
        panel = magnified(image, box, cx, cy, round(zoom, 3))
        im.paste(panel, (box[0], box[1]+y), faded(rr_mask(panel.width, panel.height, 16), q))
        rrect(im, (box[0], box[1]+y, box[2], box[3]+y), None, (58, 64, 80) if i == 0 else RUBY, 2, 16, q)
        chip(im, box[0]+20, box[1]+20+y, label, 32, color, (12, 13, 18), color, q, 'bold')
    reveal(im, 100, 900, 'Red and blue text keeps its edges in 4:4:4.', 38, t, at=2.6, color=INK,
           weight='semibold', rise=12)
    source(im, 'Same rendered pixels in both panels · 4:2:0 averages colour over each 2×2 block · BT.709', t, 2.9)
    return im


MONITORS = ((330, 380, 900, 700), (980, 380, 1550, 700))


def desktop_fill(w, h):
    im = Image.new('RGB', (w, h))
    a, b = np.array((28, 34, 58), np.float32), np.array((70, 34, 70), np.float32)
    x = np.linspace(0, 1, w, dtype=np.float32)[None, :, None]
    y = np.linspace(0, 1, h, dtype=np.float32)[:, None, None]
    rgb = a+(b-a)*(x*.6+y*.4)
    im = Image.fromarray(np.round(np.broadcast_to(rgb, (h, w, 3))).astype(np.uint8))
    d = ImageDraw.Draw(im)
    d.rectangle((0, h-22, w, h), fill=(18, 20, 28))
    for i in range(3):
        d.rounded_rectangle((24, 24+i*58, 64, 64+i*58), radius=8, fill=(90, 96, 130))
    return im


@lru_cache(maxsize=4)
def desktop(w, h):
    return desktop_fill(w, h)


def displays(t):
    im = base_for('displays').copy()
    brand(im)
    heading(im, 'Your monitors, back after every stream.', t,
            'A virtual display sized for your device while you play. Then your layout, exactly as it was.')
    # 0-1.1 s your desk; 1.1-2.3 s streaming on a virtual display; from 2.5 s back as it was.
    away = smooth((t-1.1)/.5)*(1-smooth((t-2.5)/.5))
    vd = ease((t-1.2)/.5)*(1-smooth((t-2.4)/.4))
    back = ease((t-2.8)/.5)
    p = ease((t-.2)/.6)
    for i, (x0, y0, x1, y1) in enumerate(MONITORS):
        y0, y1 = y0+round(slide(26*(1-p))), y1+round(slide(26*(1-p)))
        rrect(im, (x0-12, y0-12, x1+12, y1+12), (24, 20, 28), (90, 84, 100), 3, 16, p)
        line(im, ((x0+x1)/2, y1+12, (x0+x1)/2, y1+52), (90, 84, 100), 4)
        line(im, ((x0+x1)/2-70, y1+54, (x0+x1)/2+70, y1+54), (90, 84, 100), 5)
        screen = desktop(x1-x0, y1-y0)
        if away > 0:
            screen = Image.blend(screen, Image.new('RGB', screen.size, (14, 14, 18)), .82*away)
        im.paste(screen, (x0, y0), faded(Image.new('L', screen.size, 255), p))
    if vd > 0:
        x0, y0, x1, y1 = 700, 300+round(20*(1-vd)), 1220, 593+round(20*(1-vd))
        rrect(im, (x0-6, y0-6, x1+6, y1+6), (20, 14, 20), RUBY, 4, 14, vd)
        pic = picture(x1-x0, y1-y0)
        im.paste(pic, (x0, y0), faded(rr_mask(pic.width, pic.height, 8), vd))
        chip(im, 960-width_of('Virtual display · 2560×1440 · 120 Hz', 26, 'semibold')/2-17, y1+22,
             'Virtual display · 2560×1440 · 120 Hz', 26, INK, (40, 18, 30), RUBY, vd)
    if back > 0:
        label = 'Layout restored'
        w = width_of(label, 30, 'bold')+90
        x0, y0 = 960-w/2, 800+14*(1-back)
        rrect(im, (x0, y0, x0+w, y0+60), RUBY, None, radius=30, opacity=back)
        cx, cy = x0+36, y0+30
        d = ImageDraw.Draw(im)
        if back > .5:
            d.line((cx-11, cy, cx-3, cy+8, cx+12, cy-9), fill=(24, 10, 16), width=5, joint='curve')
        txt(im, x0+60, y0+30-cap_height(30)/2-1, label, 30, (24, 10, 16), 'bold', opacity=back)
    return im


# 4 · Proof -------------------------------------------------------------------------

def decimals(value):
    return len(value.split('.')[1]) if '.' in value else 0


def counted(value, local, at, length=1.1):
    p = ease((local-at)/length)
    return f'{float(value)*p:.{decimals(value)}f}', p


def latency(t):
    im = base_for('latency').copy()
    n = NUMBERS['latency']
    brand(im)
    reveal(im, 100, 96, 'MEASURED, IN THE OPEN', 28, t, at=.1, color=RUBY, weight='bold', spacing=4, rise=10)
    shown, p = counted(n['value'], t, .5)
    size = 300
    slot = width_of('8'*len(n['value'].replace('.', ''))+'.', size, 'bold')
    o = ease((t-.4)/.4)
    txt(im, 100+slot, 250, shown, size, RUBY, 'bold', 'right', o)
    unit_x = 100+slot+30
    txt(im, unit_x, 250+cap_height(size)-cap_height(110), n['unit'], 110, RUBY, 'bold', opacity=o)
    reveal(im, 104, 620, n['caption'], 56, t, at=1.3, color=INK, weight='semibold', rise=14)
    reveal(im, 106, 708, n['detail'], 34, t, at=1.5, color=MUTED, weight='regular', rise=12)
    # A frame's trip, start to end, drawn as the number counts.
    y = 850
    x0, x1 = 106, 1814
    line(im, (x0, y, x1, y), LINE, 4)
    q = ease((t-.5)/1.1)
    if q > 0:
        line(im, (x0, y, x0+(x1-x0)*q, y), RUBY, 6)
        rrect(im, (x0-10, y-10, x0+10, y+10), RUBY, radius=10)
        hx = x0+(x1-x0)*q
        glow(im, hx, y, 160, RUBY, .8)
        rrect(im, (hx-10, y-10, hx+10, y+10), ROSE if q < 1 else RUBY, radius=10)
    txt(im, x0, y+34, 'Captured on the PC', 28, MUTED, 'regular', opacity=o)
    txt(im, x1, y+34, 'On the client’s screen', 28, INK if q >= 1 else MUTED, 'regular', 'right', o)
    source(im, n['source'], t, 1.6)
    return im


def versus(t):
    im = base_for('versus').copy()
    n = NUMBERS['versus']
    brand(im)
    heading(im, n['heading'], t, n['subtitle'])
    rows = (n['theirs'], n['ours'])
    top = max(float(v) for _, v in rows)
    x0, maxw = 420, 1200
    for i, (label, value) in enumerate(rows):
        y = 380+i*200
        ours = i == 1
        color = RUBY if ours else GREY
        o = ease((t-.4-i*.2)/.5)
        txt(im, 100, y+30, label, 40, INK if ours else (200, 202, 210), 'bold' if ours else 'semibold', opacity=o)
        shown, p = counted(value, t, .7+i*.6, 1.2)
        w = maxw*float(value)/top*p
        if w > 4:
            rrect(im, (x0, y, x0+w, y+104), color, radius=14, opacity=o)
        if o > 0:
            txt(im, x0+w+28, y+18, shown, 80, color if ours else (210, 212, 220), 'bold', opacity=o)
            vw = width_of(shown, 80, 'bold')
            txt(im, x0+w+28+vw+14, y+18+cap_height(80)-cap_height(40), n['unit'], 40, MUTED, 'semibold', opacity=o)
    reveal(im, 100, 830, n['detail'], 32, t, at=2.4, color=MUTED, weight='regular', rise=10)
    source(im, n['source'], t, 2.6)
    return im


def more(t):
    im = base_for('more').copy()
    n = NUMBERS['more']
    brand(im)
    heading(im, n['heading'], t, n['subtitle'])
    tiles = n['tiles']
    gap = 40
    tw = (1720-gap*(len(tiles)-1))/len(tiles)
    for i, tile in enumerate(tiles):
        x0 = 100+i*(tw+gap)
        y0 = 320
        o = ease((t-.3-i*.18)/.6)
        if o <= 0:
            continue
        y = y0+slide(30*(1-o))
        rrect(im, (x0, y, x0+tw, y+520), (24, 20, 28), (64, 46, 58), 2, 22, o)
        rrect(im, (x0+40, y+44, x0+96, y+50), RUBY, radius=3, opacity=o)
        shown, _ = counted(tile['value'], t, .6+i*.25, 1.0)
        txt(im, x0+36, y+110, shown, 150, RUBY, 'bold', opacity=o)
        uw = width_of(shown, 150, 'bold')
        txt(im, x0+36+uw+(14 if tile['unit'] != '×' else 6), y+110+cap_height(150)-cap_height(64), tile['unit'], 64,
            RUBY, 'bold', opacity=o)
        txt(im, x0+40, y+340, tile['caption'], 36, INK, 'semibold', opacity=o)
        txt(im, x0+40, y+400, tile['detail'], 28, MUTED, 'regular', opacity=o)
    source(im, n['source'], t, 1.4)
    return im


# 5 · End ---------------------------------------------------------------------------

@lru_cache(maxsize=1)
def end_backdrop():
    pic = landscape().resize((W, H), Image.Resampling.LANCZOS).filter(ImageFilter.GaussianBlur(18))
    return Image.blend(base_for('end'), pic, .2)


def end(t):
    im = end_backdrop().copy()
    p = ease((t-.1)/.7)
    mark = width_of('Rubylight', 140, 'bold')
    left = (W-(128+40+mark))/2
    badge(im, left, 196+20*(1-p), 128, p)
    reveal(im, left+168, 206, 'Rubylight', 140, t, at=.15)
    words, gap = ('Install.', 'Pair.', 'Play.'), 60
    widths = [width_of(w, 96, 'bold') for w in words]
    x = (W-sum(widths)-gap*2)/2
    for i, value in enumerate(words):
        reveal(im, x, 430, value, 96, t, at=.6+i*.3, color=RUBY)
        x += widths[i]+gap
    q = ease((t-1.7)/.6)
    if q > 0:
        url = 'github.com/RamazanKara/Rubylight'
        w = width_of(url, 54, 'bold')+100
        rrect(im, (960-w/2, 620+16*(1-q), 960+w/2, 720+16*(1-q)), (30, 16, 24), RUBY, 3, 50, q)
        txt(im, 960, 620+16*(1-q)+50-cap_height(54)/2, url, 54, INK, 'bold', 'center', q)
    reveal(im, 960, 790, 'Free and open source · Windows 10 and 11 · Any AMD Radeon with AMF', 34, t, at=2.1,
           color=(200, 200, 210), weight='regular', align='center', rise=12)
    reveal(im, 960, 850, 'Works with Moonlight on your TV, phone, laptop, Steam Deck and Xbox.', 34, t, at=2.3,
           color=(200, 200, 210), weight='regular', align='center', rise=12)
    # Fade to black over the last second, so the loop restarts from dark.
    fade = 0 if STILL else smooth((t-(DURATION-STARTS[-1]-1.0))/1.0)
    if fade > 0:
        im = Image.blend(im, Image.new('RGB', (W, H), (0, 0, 0)), fade)
    return im


SCENES = {'hero': hero, 'home': home, 'library': library, 'add_games': add_games, 'radeon': radeon,
          'pair': pair, 'colour': colour, 'displays': displays, 'latency': latency, 'versus': versus,
          'more': more, 'end': end}


def scene_at(t):
    for i, (_, start, stop, _) in enumerate(TIMELINE):
        if t < stop:
            return i, t-start
    return len(TIMELINE)-1, t-TIMELINE[-1][1]


def render(index, local):
    return SCENES[TIMELINE[index][0]](local)


def frame(t):
    index, local = scene_at(t)
    im = render(index, local)
    kind = TIMELINE[index][3]
    if index and local < TRANSITION and kind and not STILL:
        name, start, stop, _ = TIMELINE[index-1]
        old = render(index-1, stop-start+local)
        p = ease_io(local/TRANSITION)
        if kind == 'fade':
            # Through the bare background, so two headings never overlap.
            bare = base_for(TIMELINE[index][0])
            im = Image.blend(old, bare, smooth(p*2)) if p < .5 else Image.blend(bare, im, smooth(p*2-1))
        elif kind == 'push':
            shift = round(W*p)
            out = Image.new('RGB', (W, H))
            out.paste(old.crop((shift, 0, W, H)), (0, 0))
            out.paste(im.crop((0, 0, shift, H)), (W-shift, 0))
            im = out
        else:
            lean = 260
            edge = -lean-80+(W+2*lean+160)*p
            mask = Image.new('L', (W, H), 0)
            ImageDraw.Draw(mask).polygon([(-10, 0), (edge+lean, 0), (edge, H), (-10, H)], fill=255)
            im = Image.composite(im, old, mask)
            d = ImageDraw.Draw(im)
            d.polygon([(edge+lean, 0), (edge+lean+46, 0), (edge+46, H), (edge, H)], fill=RUBY)
            d.polygon([(edge+lean+46, 0), (edge+lean+58, 0), (edge+58, H), (edge+46, H)], fill=ROSE)
    if t < .5 and not STILL:
        im = Image.blend(Image.new('RGB', (W, H)), im, smooth(t/.5))
    return im


STORY = (1.6, 3.9, 4.6, 7.4, 10.5, 13.0, 16.8, 21.5, 27.5, 33.0, 34.6, 39.5,
         43.0, 44.6, 48.6, 54.0, 60.0, 66.0)


def storyboard(destination, times=STORY):
    columns, tile = 3, (640, 360)
    rows = math.ceil(len(times)/columns)
    sheet = Image.new('RGB', (columns*tile[0], rows*(tile[1]+30)), BG)
    for i, t in enumerate(times):
        image = frame(t).resize(tile, Image.Resampling.LANCZOS)
        x, y = i % columns*tile[0], i//columns*(tile[1]+30)
        sheet.paste(image, (x, y))
        ImageDraw.Draw(sheet).text((x+10, y+tile[1]+6), f'{t:04.1f}s', font=font(18), fill=MUTED)
    destination.parent.mkdir(parents=True, exist_ok=True)
    sheet.save(destination, quality=90)


def ffmpeg():
    path = shutil.which('ffmpeg')
    if not path:
        raise SystemExit('FFmpeg is required.')
    return path


def pipe_frames(cmd, fps):
    with subprocess.Popen(cmd, stdin=subprocess.PIPE) as proc:
        try:
            for n in range(round(DURATION*fps)):
                proc.stdin.write(frame(n/fps).tobytes())
                if n % (fps*10) == 0:
                    print(f'Rendered {n//fps}/{DURATION}s', flush=True)
        finally:
            proc.stdin.close()
        if proc.wait():
            raise RuntimeError('FFmpeg failed')


RAW = ['-f', 'rawvideo', '-pixel_format', 'rgb24', '-video_size', f'{W}x{H}']
BT709 = ['-color_primaries', 'bt709', '-color_trc', 'bt709', '-colorspace', 'bt709', '-color_range', 'tv']


def master(destination, fps, width):
    """Every frame, lossless (FFV1 RGB), for the two-pass encode and the GIF."""
    destination.parent.mkdir(parents=True, exist_ok=True)
    pipe_frames([ffmpeg(), '-hide_banner', '-loglevel', 'error', '-y', *RAW, '-framerate', str(fps), '-i', 'pipe:0',
                 '-vf', f'scale={width}:-2:flags=lanczos', '-c:v', 'ffv1', '-pix_fmt', 'bgr0', '-threads', '4',
                 str(destination)], fps)


def encode(source, destination, audio=None, size_mb=2.9, audio_kbps=64):
    """Two-pass H.264 sized to size_mb, so the README film stays small."""
    destination.parent.mkdir(parents=True, exist_ok=True)
    video_kbps = int((size_mb*8e6*.97/DURATION-(audio_kbps*1000 if audio else 0))/1000)
    common = ['-vf', 'scale=out_color_matrix=bt709:out_range=tv', '-c:v', 'libx264', '-preset', 'veryslow',
              '-tune', 'animation', '-b:v', f'{video_kbps}k', '-maxrate', f'{video_kbps*4}k',
              '-bufsize', f'{video_kbps*8}k', '-g', '120', '-pix_fmt', 'yuv420p', '-threads', '4', *BT709]
    with tempfile.TemporaryDirectory(prefix='rubylight-x264-') as directory:
        log = str(Path(directory)/'pass')
        base = [ffmpeg(), '-hide_banner', '-loglevel', 'error', '-y', '-i', str(source)]
        subprocess.run(base+[*common, '-pass', '1', '-passlogfile', log, '-an', '-f', 'mp4', os.devnull], check=True)
        sound = ['-i', str(audio), '-map', '0:v', '-map', '1:a', '-c:a', 'aac',
                 '-b:a', f'{audio_kbps}k'] if audio else []
        subprocess.run(base+[*sound, *common, '-pass', '2', '-passlogfile', log, '-movflags', '+faststart',
                             '-t', str(DURATION), str(destination)], check=True)
    print(f'Wrote {destination} ({destination.stat().st_size/1e6:.2f} MB, video {video_kbps} kbps)', flush=True)


def draft(destination):
    """A quick 800-wide, 10 fps look at the whole film."""
    destination.parent.mkdir(parents=True, exist_ok=True)
    pipe_frames([ffmpeg(), '-hide_banner', '-loglevel', 'error', '-y', *RAW, '-framerate', '10', '-i', 'pipe:0',
                 '-vf', 'scale=800:-2:flags=lanczos', '-c:v', 'libx264', '-preset', 'veryfast', '-crf', '20',
                 '-pix_fmt', 'yuv420p', '-threads', '2', str(destination)], 10)


def main():
    global STILL
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('--output', type=Path, help='the film (mp4), or a still with --frame')
    p.add_argument('--audio', type=Path)
    p.add_argument('--frame', type=float)
    p.add_argument('--storyboard', type=Path)
    p.add_argument('--draft', type=Path, help='a quick 800-wide, 10 fps preview')
    p.add_argument('--gif-master', type=Path, help='the lossless GIF cut (.mkv): 800 wide, 10 fps, no moves')
    p.add_argument('--fps', type=int, default=30)
    p.add_argument('--size-mb', type=float, default=2.9)
    args = p.parse_args()
    if args.storyboard:
        storyboard(args.storyboard)
    if args.draft:
        draft(args.draft)
    if args.output and args.frame is not None:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        frame(args.frame).save(args.output)
    elif args.output:
        with tempfile.TemporaryDirectory(prefix='rubylight-master-') as directory:
            lossless = Path(directory)/'master.mkv'
            master(lossless, args.fps, W)
            encode(lossless, args.output, args.audio, args.size_mb)
    if args.gif_master:
        STILL = True
        master(args.gif_master, 10, 800)
        STILL = False
    if not (args.output or args.storyboard or args.draft or args.gif_master):
        p.error('Use --output, --draft, --gif-master or --storyboard.')


if __name__ == '__main__':
    main()
