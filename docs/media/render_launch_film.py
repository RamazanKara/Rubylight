#!/usr/bin/env python3
"""Rubylight's feature tour, rendered deterministically with Pillow.

No service, game, display or GPU access. Requires Pillow, NumPy and FFmpeg.
The console scenes use docs/media/console/*.jpg: the real web console with
mocked host replies and a made-up library, made by capture_console.cjs. The
Nightjar scene uses real, privacy-checked screenshots in docs/media/nightjar/.
The game picture is the console's own Home artwork,
rust/web/src/assets/home-landscape.webp. Everything else is drawn here.

One feature per scene. Every measured number and its conditions live in
NUMBERS below, so a remeasured value is a one-line change; each number scene
names its source in one line.

python docs/media/demo_audio.py --output score.wav
python docs/media/render_launch_film.py --storyboard film.jpg
python docs/media/render_launch_film.py --frame 7.2 --output frame.png
python docs/media/render_launch_film.py --draft draft.mp4          # 800 wide, 10 fps
python docs/media/render_launch_film.py --gif-master gif.mkv       # the README GIF's frames
python docs/media/render_launch_film.py --output film.mp4 --audio score.wav --size-mb 4.8
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
# Our own runs, as README.md "Measured performance" and rust/PERFORMANCE.md publish
# them. Swap a value here and every scene, count-up and bar follows. 'value'
# strings keep their decimals: the count-up shows as many places as the string has.
NUMBERS = {
    'network': {
        'subtitle': 'Bitrate follows the link, and a lost frame is repaired from one the client still holds.',
        'tiles': (
            {'topic': '2.4 GHz Wi-Fi', 'caption': 'Unrecoverable frames per run',
             'before': ('Without pacing', '61'), 'after': ('Paced from the first frame', '15.7'),
             'detail': 'HEVC 1080p60 at 47 Mbit/s · three runs per arm'},
            {'topic': 'Scene cuts at 0.3% loss', 'caption': 'Freezes after scene cuts',
             'before': ('Before', '15'), 'after': ('Oversized cuts sent again as keyframes', '0'),
             'detail': 'HEVC 1440p60 at 250 Mbps · 45 s · two runs per arm'},
        ),
        'more': 'Wi-Fi is paced from the first frame. A cut too big to protect goes out as a keyframe.',
        'source': 'Our runs · RX 7900 XT · October 25 and 28, 2026 · rust/PERFORMANCE.md',
    },
    'browser': {
        'value': '17', 'unit': 'ms', 'caption': 'median, glass to glass',
        'detail': 'WebRTC · AV1 1080p60 · Microsoft Edge on the same PC',
        'source': 'WebRTC on the real host · RX 7900 XT · October 25, 2026 · rust/PERFORMANCE.md',
    },
    'measured': {
        'heading': 'Fresh frames, timed one by one.',
        'latency': {'value': '5.89', 'unit': 'ms', 'caption': 'median, capture to client present',
                    'detail': 'AV1 1080p60 · client on the same PC'},
        'vrr': {'value': '0.25', 'unit': 'ms', 'caption': 'picture age at the encoder with VRR',
                'detail': 'median · from 4.68 ms · 120 Hz game, 60 fps stream'},
        'source': 'Frame timing record · Rubylight 3.0.0-rc.2 · RX 7900 XT · October 26, 2026 · rust/PERFORMANCE.md',
    },
    'versus': {
        # 'heading' is checked against the bars below: "half" needs ours under half of theirs.
        'heading': 'Less than half the picture age.',
        'subtitle': 'Mean picture age beside a game-like GPU load, against Vibepollo 2.0 · lower is better',
        'unit': 'ms',
        'theirs': ('Vibepollo 2.0', '96'),
        'ours': ('Rubylight', '42'),
        'detail': 'HEVC 10-bit HDR 1080p60 · 20 Mbps · Desktop Duplication · three runs each, same PC and settings',
        'source': 'Rubylight 2.0.0-rc.2 and Vibepollo 2.0 · RX 7900 XT · October 4, 2026 · rust/PERFORMANCE.md',
    },
}
# Words the film never shows (checked against NUMBERS and every scene's text at render).
BANNED = ('experimental', 'punktfunk', 'rx 7000', 'or newer', 'fastest')


def check_numbers():
    """The words around the numbers must stay true when the numbers change."""
    versus = NUMBERS['versus']
    ours, theirs = Decimal(versus['ours'][1]), Decimal(versus['theirs'][1])
    assert ours < theirs, 'the comparison scene shows Rubylight ahead'
    if 'half' in versus['heading']:
        assert ours*2 < theirs, f"'{versus['heading']}' needs {ours} under half of {theirs}"
    for tile in NUMBERS['network']['tiles']:
        assert Decimal(tile['after'][1]) < Decimal(tile['before'][1]), f"{tile['topic']} shows an improvement"


check_numbers()

# Timeline ------------------------------------------------------------------------
# (scene, start, end, transition into it). 120 BPM: every cut lands on a bar line.
TIMELINE = (
    ('hero', 0, 8, None),
    ('moving_in', 8, 14, 'wipe'),
    ('pair', 14, 20, 'push'),
    ('home', 20, 26, 'push'),
    ('library', 26, 30, 'push'),
    ('game', 30, 36, 'push'),
    ('displays', 36, 42, 'fade'),
    ('colour', 42, 48, 'fade'),
    ('network', 48, 54, 'fade'),
    ('coop', 54, 60, 'wipe'),
    ('connected', 60, 66, 'fade'),
    ('browser', 66, 70, 'fade'),
    ('nightjar', 70, 76, 'push'),
    ('measured', 76, 82, 'wipe'),
    ('versus', 82, 88, 'fade'),
    ('end', 88, 94, 'wipe'),
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
    assert not [word for word in BANNED if word in value.lower()], f'the film never says: {value}'
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
GLOWS = {'hero': (1500, 300), 'moving_in': (960, 520), 'pair': (1600, 200), 'home': (1700, 200),
         'library': (300, 160), 'game': (240, 200), 'displays': (1500, 860), 'colour': (960, 980),
         'network': (300, 900), 'coop': (1600, 300), 'connected': (1650, 900), 'browser': (1500, 560),
         'nightjar': (300, 300), 'measured': (1500, 560), 'versus': (1650, 300), 'end': (960, 560)}


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
    return shot('game').getpixel((2860, 1790))


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


# Icons, wrapped text, device frames -------------------------------------------------

ICON_FONT = next((root/'segmdl2.ttf' for root in FONT_ROOTS if (root/'segmdl2.ttf').exists()), None)
# Segoe MDL2 Assets code points; without the font (not on Windows) the tiles show a dot.
ICONS = {'clipboard': 0xE77F, 'upload': 0xE898, 'folder': 0xE838, 'usb': 0xE88E, 'mic': 0xE720,
         'vibrate': 0xE877, 'gamepad': 0xE7FC, 'pen': 0xEDC6, 'speaker': 0xE7F5, 'music': 0xE8D6,
         'check': 0xE73E, 'sync': 0xE895}


@lru_cache(maxsize=64)
def icon_stamp(name, size, color):
    if ICON_FONT is None:
        im = Image.new('RGBA', (size, size))
        ImageDraw.Draw(im).ellipse((size*.3, size*.3, size*.7, size*.7), fill=color)
        return im
    f = ImageFont.truetype(str(ICON_FONT), size)
    value = chr(ICONS[name])
    box = f.getbbox(value)
    im = Image.new('RGBA', (box[2]-box[0]+4, box[3]-box[1]+4))
    ImageDraw.Draw(im).text((2-box[0], 2-box[1]), value, font=f, fill=color)
    return im


def icon(im, x, y, name, size, color=RUBY, opacity=1.):
    if opacity > 0:
        im.paste(*faded_stamp(icon_stamp(name, size, color), opacity, x, y))


def wrap(value, size, width, weight='regular'):
    lines, current = [], ''
    for word in value.split():
        trial = f'{current} {word}'.strip()
        if current and width_of(trial, size, weight) > width:
            lines.append(current)
            current = word
        else:
            current = trial
    return lines+[current] if current else lines


def paragraph(im, x, y, value, size, width, color=INK, weight='regular', opacity=1., leading=1.32):
    """Wrapped text; returns the y below its last line."""
    for i, value in enumerate(wrap(value, size, width, weight)):
        txt(im, x, y+i*size*leading, value, size, color, weight, opacity=opacity)
        y_end = y+(i+1)*size*leading
    return y_end


def check_mark(im, cx, cy, size, color, width=5):
    ImageDraw.Draw(im).line((cx-size*.45, cy, cx-size*.12, cy+size*.34, cx+size*.5, cy-size*.36),
                            fill=color, width=width, joint='curve')


@lru_cache(maxsize=None)
def nightjar_shot(name):
    return Image.open(MEDIA/'nightjar'/f'{name}.png').convert('RGB')


@lru_cache(maxsize=None)
def handheld_shot(name):
    return nightjar_shot(name).resize((720, 360), Image.Resampling.LANCZOS)


def bezel(im, box, screen, radius, opacity=1., fill=(24, 16, 22)):
    """A device outline in the banner's ruby stroke, with its screen pasted in."""
    x0, y0, x1, y1 = (round(v) for v in box)
    rrect(im, (x0, y0, x1, y1), fill, OUTLINE, 3, radius, opacity)
    if screen is not None:
        sx0, sy0, sx1, sy1 = (round(v) for v in screen[0])
        pic = screen[1].resize((sx1-sx0, sy1-sy0), Image.Resampling.LANCZOS)
        im.paste(pic, (sx0, sy0), faded(rr_mask(pic.width, pic.height, 8), opacity))


def label_under(im, cx, y, value, opacity, size=28, color=INK):
    txt(im, cx, y, value, size, color, 'semibold', 'center', opacity)


# 1 · Hero ------------------------------------------------------------------------

# Device line art: (kind, outer box, screen box, label).
DEVICES = (
    ('laptop', (130, 490, 400, 660), (130, 490, 400, 660), 'Laptop'),
    ('phone', (474, 560, 664, 666), (487, 569, 651, 657), 'Phone'),
    ('tv', (690, 330, 1230, 680), (700, 340, 1220, 632), 'TV'),
    ('handheld', (1280, 544, 1570, 666), (1352, 556, 1498, 654), 'Steam Deck'),
    ('browser', (1610, 520, 1846, 666), (1620, 556, 1836, 656), 'Browser'),
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
        elif kind == 'browser':
            R((x0, y0, x1, y1), 12)
            d.line((x0*k, (y0+28)*k, x1*k, (y0+28)*k), fill=stroke, width=sw)
            for i in range(3):
                cx = x0+18+i*16
                d.ellipse(((cx-4)*k, (y0+10)*k, (cx+4)*k, (y0+18)*k), fill=stroke)
            d.rounded_rectangle(((x0+70)*k, (y0+8)*k, (x1-14)*k, (y0+20)*k), radius=6*k, outline=stroke, width=2*k)
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
        txt(im, (outer[0]+outer[2])/2, 722, label, 30, mix(BG, INK, p*(.55+.45*level)), 'semibold', 'center')


@lru_cache(maxsize=1)
def hero_shade():
    """Darkens the left of the full-bleed picture so the promise reads."""
    x = np.linspace(0, 1, W, dtype=np.float32)
    a = np.clip(1-(x/.72), 0, 1)**1.1*245
    y = np.linspace(0, 1, H, dtype=np.float32)
    a = np.maximum(a[None, :], (np.clip((y-.72)/.28, 0, 1)**2*150)[:, None])
    return Image.fromarray(np.round(a).astype(np.uint8), 'L')


FACTS = ('Free and open source', 'Windows 10 and 11', 'AMD Radeon with AMF')


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
        badge(im, 100, 300, 64, p*out)
        txt(im, 184, 312, 'Rubylight', 44, INK, 'bold', opacity=p*out)
        for i, (value, color) in enumerate((('Your Radeon PC.', INK), ('On every screen.', RUBY))):
            q = ease((t-.55-i*.3)/.7)
            txt(im, 96, 420+i*136+30*(1-q), value, 120, color, 'bold', opacity=q*out)
        q = ease((t-1.5)/.7)
        txt(im, 102, 720+14*(1-q), 'Your games, streamed to Nightjar, Moonlight or a browser.', 40,
            (222, 220, 228), 'regular', opacity=q*out)
    # Then: what it is, over every device.
    reveal(im, 960, 104, 'The game-streaming host built for Radeon.', 68, t, at=4.2, align='center')
    reveal(im, 960, 200, 'HDR, full-colour 4:4:4 and up to 240 fps, on every screen you own.', 34, t, at=4.4,
           color=MUTED, weight='regular', align='center', rise=14)
    q = ease((t-5.6)/.6)
    if q > 0:
        widths = [width_of(v, 28, 'semibold')+28*1.3 for v in FACTS]
        x = 960-(sum(widths)+24*2)/2
        for v, w in zip(FACTS, widths):
            chip(im, x, 860+16*(1-q), v, 28, (222, 220, 228), (24, 18, 26), (74, 44, 60), q)
            x += w+24
    return im


# 2 · Moving in -------------------------------------------------------------------

CARRIED = ('Settings', 'Paired devices', 'Games and covers')
INSTALLS = ('The Rubylight service', 'Virtual display driver', 'Controller support')


def host_card(im, box, title, caption, opacity, accent):
    x0, y0, x1, y1 = box
    rrect(im, box, (24, 20, 28), RUBY if accent else (70, 62, 78), 3 if accent else 2, 22, opacity)
    if accent:
        badge(im, x0+36, y0+34, 44, opacity)
    txt(im, x0+(100 if accent else 36), y0+42, title, 38, INK, 'bold', opacity=opacity)
    txt(im, x0+36, y0+108, caption, 26, MUTED, 'regular', opacity=opacity)


def moving_in(t):
    im = base_for('moving_in').copy()
    brand(im)
    heading(im, 'Moving in takes one install.', t,
            'An Apollo or Vibepollo install carries over, with everything you set up.')
    left, right = (100, 290, 760, 740), (1160, 290, 1820, 740)
    p = ease((t-.3)/.6)
    host_card(im, (left[0], left[1]+slide(24*(1-p)), left[2], left[3]+slide(24*(1-p))),
              'Apollo or Vibepollo', 'Your current host', p, False)
    q = ease((t-.5)/.6)
    host_card(im, (right[0], right[1]+slide(24*(1-q)), right[2], right[3]+slide(24*(1-q))),
              'Rubylight', 'After setup', q, True)
    # An arrow between the cards.
    a = ease((t-.8)/.5)
    if a > 0:
        x0, x1, y = 800, 800+(320*a), 515
        line(im, (x0, y, x1, y), RUBY, 6)
        if a > .9:
            ImageDraw.Draw(im).polygon([(1122, y), (1096, y-16), (1096, y+16)], fill=RUBY)
    # Each row is copied from the old host and lands in Rubylight with a tick; the
    # original stays behind, dimmed, under the copy.
    w = right[2]-right[0]-72
    for i, label in enumerate(CARRIED):
        o = ease((t-.7-i*.12)/.5)
        y = 460+i*86
        rrect(im, (left[0]+36, y, left[0]+36+w, y+66), (26, 24, 30), (60, 54, 66), 2, 14, o*.8)
        txt(im, left[0]+64, y+33-cap_height(30, 'semibold')/2, label, 30, DIM, 'semibold', opacity=o)
    for i, label in enumerate(CARRIED):
        o = ease((t-.7-i*.12)/.5)
        if o <= 0:
            continue
        y = 460+i*86
        start = 1.0+i*.35
        move = ease_io((t-start)/.8) if not STILL else float(t > start+.4)
        x = lerp(left[0]+36, right[0]+36, move)
        landed = move >= 1
        rrect(im, (x, y, x+w, y+66), (40, 18, 30) if landed else (32, 28, 38),
              RUBY if landed else (80, 72, 90), 2, 14, o)
        txt(im, x+28, y+33-cap_height(30, 'semibold')/2, label, 30, INK, 'semibold', opacity=o)
        if landed:
            check_mark(im, x+w-40, y+33, 26, RUBY, 5)
    # Setup installs the rest.
    s = ease((t-2.9)/.5)
    if s > 0:
        txt(im, 100, 800+slide(12*(1-s)), 'Setup installs', 28, RUBY, 'bold', opacity=s, spacing=1)
        x = 330
        for i, label in enumerate(INSTALLS):
            o = ease((t-3.0-i*.2)/.45)
            right_edge = chip(im, x, 784+slide(12*(1-o)), '      '+label, 28, INK, (30, 18, 26), (110, 44, 72), o)
            if o > 0:
                check_mark(im, x+34, 784+slide(12*(1-o))+25, 22, RUBY, 4)
            x = right_edge+18
    source(im, 'Updates are offered in the console; they are verified and installed in an idle minute when you allow it.',
           t, 3.8)
    return im


# 3 · Pairing -----------------------------------------------------------------------

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
        txt(im, x+w/2, sy1-60, 'Moonlight', 24, MUTED, 'semibold', 'center', o)
    if done > 0:
        pic = picture(round(sx1-sx0), round(sy1-sy0), .6)
        mask = faded(rr_mask(pic.width, pic.height, 18), done*opacity)
        im.paste(pic, (round(sx0), round(sy0)), mask)
        p = ease((local-paired_at-.2)/.5)
        cx, cy = x+w/2, y+h+10
        rrect(im, (cx-110, cy-30, cx+110, cy+30), RUBY, None, radius=30, opacity=p)
        txt(im, cx, cy-cap_height(30)/2-1, 'Paired', 30, (24, 10, 16), 'bold', 'center', p)


PAIR_VP = (480, 262, 1820, 1080)


@lru_cache(maxsize=1)
def pin_digits():
    """The typed PIN's four digits in pair-pin.jpg, as column runs of bright ink."""
    x0, y0, x1, y1 = boxes()['pair']['pin']
    ink = np.asarray(shot('pair-pin').crop((x0+16, y0+16, x1-16, y1-16)).convert('L')) > 150
    columns = ink.any(axis=0)
    runs, start = [], None
    for x, on in enumerate(columns):
        if on and start is None:
            start = x
        elif not on and start is not None:
            runs.append((start+16, x+16))
            start = None
    assert len(runs) == 4, f'expected four digits in the PIN field, found {len(runs)}'
    return runs


def digit_cuts(width):
    """Where to cut a screen crop of the PIN field after one, two and three digits."""
    x0, _, x1, _ = boxes()['pair']['pin']
    scale = width/(x1-x0+12)
    runs = pin_digits()
    return [round((6+(runs[i][1]+runs[i+1][0])/2)*scale) for i in range(3)]


def pair(t):
    im = base_for('pair').copy()
    b = boxes()['pair']
    vp = PAIR_VP
    ways, request = b['ways'], b['request']
    pin_side = (request[0]-150, ways[1], ways[2], ways[3])
    keys = ((0, fit('pair', ways, 30, vp)), (2.0, fit('pair', ways, 30, vp)),
            (2.8, fit('pair', pin_side, 24, vp)))
    brand(im)
    heading(im, 'Pair in seconds.', t, 'Scan the code with Nightjar, or type the PIN Moonlight shows.')
    sample_note(im)
    p = ease(t/.7)
    camera = camera_at(keys, t)
    rise = round(slide(40*(1-p)))
    console_window(im, 'pair', camera, vp, rise=rise)
    # The PIN, typed digit by digit: the filled field from the second shot.
    typed = min(4, max(0, math.floor((t-3.0)/.2)+1))
    pin = b['pin']
    if typed:
        # All four typed: the whole request from the second shot, its Pair device button now on.
        area = (pin[0]-6, pin[1]-6, pin[2]+6, pin[3]+6) if typed < 4 else request
        x0, y0, x1, y1 = (round(v) for v in to_screen(area, camera, vp))
        field = view('pair-pin', round(camera[0], 1), round(camera[1], 1), round(camera[2], 4), vp)
        ox, oy = vp[0], vp[1]+rise
        crop = field.crop((x0-ox, y0-oy, x1-ox, y1-oy))
        if typed < 4:
            # Cover the digits not typed yet with the field's fill, cutting between digits.
            crop = crop.copy()
            cut = digit_cuts(crop.width)[typed-1]
            fill = crop.getpixel((crop.width-16, crop.height//2))
            ImageDraw.Draw(crop).rectangle((cut, 12, crop.width-14, crop.height-12), fill=fill)
        im.paste(crop, (x0, y0))
    ring(im, to_screen(b['qr'], camera, vp), smooth((t-.7)/.3)*(1-smooth((t-2.0)/.3)), pad=4)
    paired_at = 4.1
    ring(im, to_screen(request, camera, vp), smooth((t-paired_at+.15)/.3)*(1-smooth((t-paired_at-1.3)/.4)), pad=6)
    phone(im, 110, 330, t, paired_at, ease((t-.3)/.7))
    return im


# 4 · Home ------------------------------------------------------------------------

def home(t):
    im = base_for('home').copy()
    b = boxes()['home']
    keys = ((0, wide('home', 0)), (2.0, wide('home', 0)), (3.2, fit('home', b['card'], 24)))
    brand(im)
    heading(im, 'Ready to play. Live while you play.', t,
            'Home shows the running game, each stream and its live numbers.')
    sample_note(im)
    p = ease(t/.7)
    camera = camera_at(keys, t)
    console_window(im, 'home', camera, rise=slide(40*(1-p)))
    # Before a device connects, Home says Ready to play: the first shot, faded out.
    ready = 1-smooth((t-1.1)/.5) if not STILL else float(t < 1.4)
    if ready > 0:
        at = (round(camera[0], 1), round(camera[1], 1), round(camera[2], 4))
        before, after = view('home-ready', *at), view('home', *at)
        im.paste(Image.blend(after, before, ready), (VP[0], VP[1]+round(slide(40*(1-p)))), window_mask())
    ring(im, to_screen(b['details'], camera), smooth((t-3.6)/.4), pad=8)
    return im


# 5 · Library -----------------------------------------------------------------------

LAUNCHERS = ('Steam', 'Epic Games', 'Battle.net', 'EA app', 'Ubisoft Connect', 'GOG', 'Microsoft Store',
             'Xbox', 'Playnite')


def library(t):
    im = base_for('library').copy()
    b = boxes()['library']
    g = b['grid']
    first_row = (g[0], g[1], g[2], g[1]+(g[3]-g[1])*.5)
    keys = ((0, wide('library', 0)), (.6, wide('library', 0)), (2.6, fit('library', first_row, 24)))
    brand(im)
    heading(im, 'Your whole library, with cover art.', t)
    sample_note(im)
    p = ease(t/.7)
    console_window(im, 'library', camera_at(keys, t), rise=slide(40*(1-p)))
    # Every launcher it reads, one by one, where the subtitle would be.
    x = 100
    for i, name in enumerate(LAUNCHERS):
        q = ease((t-.3-i*.1)/.4)
        x = chip(im, x, 172+12*(1-q), name, 24, INK, (40, 18, 30), (120, 40, 70), q) + 10
    return im


# 6 · Per-game settings -------------------------------------------------------------

def game(t):
    im = base_for('game').copy()
    b = boxes()['game']
    d, r = b['display'], b['radeon']
    keys = ((0, fit('game', (d[0], d[1], d[2], d[1]+1100), 24, anchor='top')), (1.6, fit('game', (d[0], d[1], d[2], d[1]+1100), 24, anchor='top')),
            (2.8, fit('game', (r[0], r[1], r[2], r[1]+1060), 24, anchor='top')))
    brand(im)
    heading(im, 'Settings for each game.', t,
            'Display, video, input, Radeon and frame limits: set at launch, put back afterwards.')
    sample_note(im)
    p = ease(t/.7)
    camera = camera_at(keys, t)
    console_window(im, 'game', camera, rise=slide(40*(1-p)))
    adjust = (r[0]+30, r[1]+600, r[0]+1300, r[1]+1040)
    ring(im, to_screen(adjust, camera), smooth((t-3.4)/.4), pad=6)
    return im


# 7 · Displays ------------------------------------------------------------------------

MONITORS = ((330, 380, 900, 700), (980, 380, 1550, 700))


def desktop_fill(w, h):
    """A drawn stand-in for a desktop: a gradient, a bar and three icons. Never a real one."""
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


VIRTUAL = 'Living room TV · 3840×2160 · 120 Hz · HDR'


def displays(t):
    im = base_for('displays').copy()
    brand(im)
    heading(im, 'A display for every device.', t,
            'A virtual display at the device’s own resolution, refresh rate and HDR.')
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
        chip(im, 960-width_of(VIRTUAL, 26, 'semibold')/2-17, y1+22, VIRTUAL, 26, INK, (40, 18, 30), RUBY, vd)
    if back > 0:
        label = 'Your monitor layout, restored'
        w = width_of(label, 30, 'bold')+90
        x0, y0 = 960-w/2, 800+14*(1-back)
        rrect(im, (x0, y0, x0+w, y0+60), RUBY, None, radius=30, opacity=back)
        if back > .5:
            check_mark(im, x0+36, y0+30, 26, (24, 10, 16), 5)
        txt(im, x0+60, y0+30-cap_height(30)/2-1, label, 30, (24, 10, 16), 'bold', opacity=back)
    return im


# 8 · Picture -------------------------------------------------------------------------

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


PANELS = ((100, 290, 940, 770), (980, 290, 1820, 770))
HDR = ('HDR10 on HEVC and AV1', 'HDR10+', 'Dolby Vision', 'HLG')


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
    heading(im, 'HDR and full colour.', t,
            'Full 4:4:4 colour with PyroWave, or packed in HEVC and AV1 for Nightjar.')
    sampled, full = hud()
    p = smooth((t-1.0)/1.8) if not STILL else float(t > 1.9)
    zoom = math.exp(lerp(math.log(3.4), math.log(8.), p))
    cx, cy = lerp(120, 60, p), lerp(54, 68, p)
    for i, (box, image, label, color) in enumerate(zip(PANELS, (sampled, full), ('4:2:0', '4:4:4'), (MUTED, RUBY))):
        q = ease((t-.3-i*.15)/.6)
        if q <= 0:
            continue
        y = round(slide(24*(1-q)))
        panel = magnified(image, box, cx, cy, round(zoom, 3))
        im.paste(panel, (box[0], box[1]+y), faded(rr_mask(panel.width, panel.height, 16), q))
        rrect(im, (box[0], box[1]+y, box[2], box[3]+y), None, (58, 64, 80) if i == 0 else RUBY, 2, 16, q)
        chip(im, box[0]+20, box[1]+20+y, label, 32, color, (12, 13, 18), color, q, 'bold')
    # The HDR formats, in a row under the panels.
    x = 100
    for i, name in enumerate(HDR):
        q = ease((t-2.4-i*.15)/.45)
        x = chip(im, x, 830+12*(1-q), name, 30, INK, (40, 18, 30), (120, 40, 70), q, 'semibold') + 16
    source(im, 'Same rendered pixels in both panels · 4:2:0 averages colour over each 2×2 block · BT.709', t, 2.9)
    return im


# 9 · Networks ------------------------------------------------------------------------

def before_after(im, box, n, local, at):
    """A tile with two bars: before (grey) and after (ruby), counted up."""
    x0, y0, x1, y1 = box
    o = ease((local-at)/.6)
    if o <= 0:
        return
    y0 += slide(30*(1-o))
    rrect(im, (x0, y0, x1, y0+(y1-box[1])), (24, 20, 28), (64, 46, 58), 2, 22, o)
    txt(im, x0+40, y0+40, n['topic'].upper(), 24, RUBY, 'bold', opacity=o, spacing=2)
    txt(im, x0+40, y0+90, n['caption'], 40, INK, 'semibold', opacity=o)
    top = max(float(n['before'][1]), float(n['after'][1]))
    maxw = x1-x0-260
    for i, (label, value) in enumerate((n['before'], n['after'])):
        y = y0+180+i*120
        ours = i == 1
        shown, p = counted(value, local, at+.4+i*.5, 1.0)
        txt(im, x0+40, y, label, 26, INK if ours else (200, 202, 210), 'semibold', opacity=o)
        w = max(10, maxw*float(value)/top*p) if float(value) > 0 else 10
        rrect(im, (x0+40, y+40, x0+40+w, y+96), RUBY if ours else GREY, radius=12, opacity=o)
        txt(im, x0+40+w+22, y+46, shown, 52, RUBY if ours else (210, 212, 220), 'bold', opacity=o)
    txt(im, x0+40, y0+440, n['detail'], 24, MUTED, 'regular', opacity=o)


def network(t):
    im = base_for('network').copy()
    n = NUMBERS['network']
    brand(im)
    heading(im, 'Built for real networks.', t, n['subtitle'])
    gap = 40
    tw = (1720-gap)/2
    for i, tile in enumerate(n['tiles']):
        x0 = 100+i*(tw+gap)
        before_after(im, (x0, 290, x0+tw, 790), tile, t, .3+i*.25)
    reveal(im, 100, 850, n['more'], 32, t, at=2.6, color=INK, weight='semibold', rise=12)
    source(im, n['source'], t, 2.8)
    return im


# 10 · Co-op --------------------------------------------------------------------------

COOP_VP = (100, 262, 1290, 1080)
PLAYERS = (('Player 1', 'Living room TV', .35), ('Player 2', 'Steam Deck', .62), ('Player 3', 'Laptop', .88))


def coop(t):
    im = base_for('coop').copy()
    b = boxes()['coop']
    brand(im)
    heading(im, 'Play together.', t,
            'Nucleus Co-op gives each device its own copy of the game, with its own picture, sound and controller.')
    sample_note(im)
    p = ease(t/.7)
    camera = fit('coop', b['card'], 30, COOP_VP)
    console_window(im, 'coop', camera, COOP_VP, rise=slide(40*(1-p)))
    for i, (player, device, focus) in enumerate(PLAYERS):
        o = ease((t-.8-i*.3)/.5)
        if o <= 0:
            continue
        x0, y0 = 1340, 290+i*262+slide(20*(1-o))
        w, h = 400, 225
        rrect(im, (x0-8, y0-8, x0+w+8, y0+h+8), (24, 16, 22), OUTLINE, 3, 14, o)
        pic = picture(w, h, focus)
        im.paste(pic, (x0, round(y0)), faded(rr_mask(w, h, 8), o))
        chip(im, x0+12, y0+12, f'{player} · {device}', 22, (24, 10, 16), RUBY, RUBY, o, 'bold')
    return im


# 11 · Connected ----------------------------------------------------------------------

CONNECTED = (
    ('clipboard', 'Clipboard', 'Copy on one, paste on the other'),
    ('upload', 'File transfer', 'Send files to the PC'),
    ('folder', 'Shared folders', 'The PC’s folders on the device'),
    ('usb', 'USB forwarding', 'Plug in on the device, use it on the PC'),
    ('mic', 'Microphone', 'The device’s microphone on the PC'),
    ('vibrate', 'DualSense', 'Haptics and adaptive triggers'),
    ('gamepad', 'Controllers', 'Switch Pro, Xbox and Steam Deck'),
    ('pen', 'Touch and pen', 'Touchscreens and styluses'),
    ('speaker', 'Surround sound', 'Audio up to 7.1.4'),
    ('music', 'Dolby Digital', 'Dolby Digital and Dolby Digital Plus'),
)


def connected(t):
    im = base_for('connected').copy()
    brand(im)
    heading(im, 'Your devices, connected.', t, 'What the device has, the PC can use.')
    columns, gap = 5, 24
    tw = (1720-gap*(columns-1))/columns
    th = 316
    for i, (name, title, detail) in enumerate(CONNECTED):
        col, row = i % columns, i//columns
        o = ease((t-.4-i*.09)/.5)
        if o <= 0:
            continue
        x0, y0 = 100+col*(tw+gap), 290+row*(th+gap)+slide(24*(1-o))
        rrect(im, (x0, y0, x0+tw, y0+th), (24, 20, 28), (64, 46, 58), 2, 20, o)
        icon(im, x0+32, y0+36, name, 60, RUBY, o)
        txt(im, x0+32, y0+140, title, 32, INK, 'semibold', opacity=o)
        paragraph(im, x0+32, y0+196, detail, 25, tw-64, MUTED, opacity=o)
    return im


# 12 · Browser ------------------------------------------------------------------------

def browser_window(im, box, opacity):
    x0, y0, x1, y1 = box
    rrect(im, box, (28, 24, 32), (90, 84, 100), 2, 18, opacity)
    for i, color in enumerate(((236, 96, 96), (236, 190, 80), (96, 196, 120))):
        rrect(im, (x0+24+i*30, y0+22, x0+40+i*30, y0+38), color, radius=8, opacity=opacity)
    rrect(im, (x0+140, y0+14, x1-24, y0+46), (18, 16, 22), None, radius=16, opacity=opacity)
    txt(im, x0+164, y0+22, 'Living room PC · Stream', 20, MUTED, 'regular', opacity=opacity)
    pic = picture(round(x1-x0-16), round(y1-y0-68))
    im.paste(pic, (round(x0+8), round(y0+60)), faded(rr_mask(pic.width, pic.height, 12), opacity))


def browser(t):
    im = base_for('browser').copy()
    n = NUMBERS['browser']
    brand(im)
    heading(im, 'Any browser. Samsung TVs too.', t, 'A built-in WebRTC server streams to a browser tab, when you turn it on.')
    o = ease((t-.2)/.6)
    browser_window(im, (100, 300+slide(24*(1-o)), 1040, 860+slide(24*(1-o))), o)
    shown, _ = counted(n['value'], t, .6, .9)
    q = ease((t-.5)/.4)
    txt(im, 1140, 360, shown, 240, RUBY, 'bold', opacity=q)
    uw = width_of(shown, 240, 'bold')
    txt(im, 1140+uw+24, 360+cap_height(240)-cap_height(90), n['unit'], 90, RUBY, 'bold', opacity=q)
    reveal(im, 1144, 660, n['caption'], 44, t, at=1.0, color=INK, weight='semibold', rise=12)
    reveal(im, 1146, 732, n['detail'], 28, t, at=1.2, color=MUTED, weight='regular', rise=10)
    source(im, n['source'], t, 1.3)
    return im


# 13 · Nightjar -----------------------------------------------------------------------

def nightjar(t):
    im = base_for('nightjar').copy()
    brand(im)
    heading(im, 'Nightjar, the companion client.', t, 'For Android, Android TV, Fire TV, Windows and Linux.')
    # A TV and a handheld: real screenshots from a TCL TV and a Retroid Pocket.
    o = ease((t-.3)/.6)
    tv = (100, 330+slide(24*(1-o)), 820, 330+417+slide(24*(1-o)))
    bezel(im, tv, ((tv[0]+12, tv[1]+12, tv[2]-12, tv[3]-12), nightjar_shot('away-tv')), 16, o)
    if o > 0:
        line(im, (460-50, tv[3], 460-80, tv[3]+40), OUTLINE, 3)
        line(im, (460+50, tv[3], 460+80, tv[3]+40), OUTLINE, 3)
        label_under(im, 460, tv[3]+66, 'Play away from home, on Android TV', o, 26, MUTED)
    q = ease((t-.6)/.6)
    if q > 0:
        x0, y0 = 880, 330+slide(24*(1-q))
        w, h = 940, 420
        # The Retroid's screen is 2:1, centred between the sticks; it moves on to Files at 3 s.
        sx0, sy0 = x0+(w-720)/2, y0+(h-360)/2
        swap = smooth((t-3.0)/.5) if not STILL else float(t > 3.2)
        shot = Image.blend(handheld_shot('settings-retroid'), handheld_shot('files-retroid'), swap)
        bezel(im, (x0, y0, x0+w, y0+h), ((sx0, sy0, sx0+720, sy0+360), shot), 70, q)
        for cx in (x0+55, x0+w-55):
            ImageDraw.Draw(im).ellipse((cx-28, y0+h/2-90, cx+28, y0+h/2-34), outline=OUTLINE, width=3)
        label_under(im, x0+w/2, y0+h+36, 'Video settings on a handheld', q*(1-swap), 26, MUTED)
        label_under(im, x0+w/2, y0+h+36, 'Files sent from a handheld to the PC', q*swap, 26, MUTED)
    reveal(im, 100, 1014, 'Real screenshots · Nightjar on a TCL TV and a Retroid Pocket 6', 26, t, at=1.4,
           color=MUTED, weight='regular', rise=8)
    return im


# 14 · Measured -----------------------------------------------------------------------

def decimals(value):
    return len(value.split('.')[1]) if '.' in value else 0


def counted(value, local, at, length=1.1):
    p = ease((local-at)/length)
    return f'{float(value)*p:.{decimals(value)}f}', p


def big_number(im, x, y, n, local, at, size=220):
    shown, p = counted(n['value'], local, at)
    o = ease((local-at+.1)/.4)
    slot = width_of('8'*len(n['value'].replace('.', ''))+'.', size, 'bold')
    txt(im, x+slot, y, shown, size, RUBY, 'bold', 'right', o)
    txt(im, x+slot+24, y+cap_height(size)-cap_height(84), n['unit'], 84, RUBY, 'bold', opacity=o)
    reveal(im, x+4, y+cap_height(size)+70, n['caption'], 40, local, at=at+.8, color=INK, weight='semibold', rise=12)
    reveal(im, x+6, y+cap_height(size)+136, n['detail'], 28, local, at=at+1.0, color=MUTED, weight='regular', rise=10)


def measured(t):
    im = base_for('measured').copy()
    n = NUMBERS['measured']
    brand(im)
    reveal(im, 100, 96, 'MEASURED, IN THE OPEN', 28, t, at=.1, color=RUBY, weight='bold', spacing=4, rise=10)
    reveal(im, 100, 160, n['heading'], 56, t, at=.2)
    big_number(im, 100, 360, n['latency'], t, .5)
    o = ease((t-1.0)/.5)
    if o > 0:
        line(im, (990, 360, 990, 360+(470*o)), LINE, 3)
    big_number(im, 1060, 360, n['vrr'], t, 1.2)
    source(im, n['source'], t, 2.0)
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


# 15 · End ---------------------------------------------------------------------------

@lru_cache(maxsize=1)
def end_backdrop():
    pic = landscape().resize((W, H), Image.Resampling.LANCZOS).filter(ImageFilter.GaussianBlur(18))
    return Image.blend(base_for('end'), pic, .2)


def end(t):
    im = end_backdrop().copy()
    p = ease((t-.1)/.7)
    mark = width_of('Rubylight', 140, 'bold')
    left = (W-(128+40+mark))/2
    badge(im, left, 216+20*(1-p), 128, p)
    reveal(im, left+168, 226, 'Rubylight', 140, t, at=.15)
    words, gap = ('Install.', 'Pair.', 'Play.'), 60
    widths = [width_of(w, 96, 'bold') for w in words]
    x = (W-sum(widths)-gap*2)/2
    for i, value in enumerate(words):
        reveal(im, x, 450, value, 96, t, at=.6+i*.3, color=RUBY)
        x += widths[i]+gap
    q = ease((t-1.7)/.6)
    if q > 0:
        url = 'github.com/RamazanKara/Rubylight'
        w = width_of(url, 54, 'bold')+100
        rrect(im, (960-w/2, 640+16*(1-q), 960+w/2, 740+16*(1-q)), (30, 16, 24), RUBY, 3, 50, q)
        txt(im, 960, 640+16*(1-q)+50-cap_height(54)/2, url, 54, INK, 'bold', 'center', q)
    reveal(im, 960, 820, 'Free and open source · Windows 10 and 11 · AMD Radeon with AMF · Works with Moonlight', 34, t,
           at=2.1, color=(200, 200, 210), weight='regular', align='center', rise=12)
    # Fade to black over the last second, so the loop restarts from dark.
    fade = 0 if STILL else smooth((t-(DURATION-STARTS[-1]-1.0))/1.0)
    if fade > 0:
        im = Image.blend(im, Image.new('RGB', (W, H), (0, 0, 0)), fade)
    return im


SCENES = {'hero': hero, 'moving_in': moving_in, 'pair': pair, 'home': home, 'library': library, 'game': game,
          'displays': displays, 'colour': colour, 'network': network, 'coop': coop, 'connected': connected,
          'browser': browser, 'nightjar': nightjar, 'measured': measured, 'versus': versus, 'end': end}


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


# One or two stills per scene, for the storyboard and the contact sheet.
STORY = (2.0, 7.4, 12.5, 17.0, 19.0, 21.0, 25.0, 29.0, 35.0, 37.0, 39.5, 47.0, 53.0, 59.0, 65.0,
         69.0, 75.0, 81.0, 87.0, 92.0)


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


def encode(source, destination, audio=None, size_mb=4.8, audio_kbps=96):
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
    p.add_argument('--size-mb', type=float, default=4.8)
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
