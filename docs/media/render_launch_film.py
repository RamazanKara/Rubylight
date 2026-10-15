#!/usr/bin/env python3
"""Rubylight's 60-second introduction film, rendered deterministically with Pillow.

No service, game, display or GPU access. Requires Pillow, NumPy and FFmpeg.
The console scenes use docs/media/console/*.png: screenshots of the real web
console with mocked host replies, made by capture_console.cjs. Measured numbers
come from README.md and docs/performance.md; each measured scene names its
source and date in one line.

python docs/media/demo_audio.py --output score.wav --duration 60
python docs/media/render_launch_film.py --storyboard film.jpg
python docs/media/render_launch_film.py --output film.mp4 --audio score.wav
python docs/media/render_launch_film.py --frame 21 --output frame.png
"""
import argparse
from decimal import Decimal, ROUND_HALF_UP
from functools import lru_cache
import json
import math
from pathlib import Path
import shutil
import subprocess

import numpy as np
from PIL import Image, ImageChops, ImageDraw, ImageFont

W, H, DURATION = 1920, 1080, 60
BG = (9, 10, 14)
INK = (244, 243, 238)
RUBY = (255, 64, 108)
RUBY_DEEP = (196, 18, 66)
ROSE = (255, 138, 168)
MINT = (98, 226, 184)
MUTED = (150, 157, 172)
GREY = (126, 133, 150)
DIM = (68, 74, 90)
RULE = (36, 40, 52)
CARD = (19, 22, 30)
STARTS = (0, 6, 24, 38, 46, 53)
ENDS = STARTS[1:] + (DURATION,)
WIPE = .6
MEDIA = Path(__file__).resolve().parent

# docs/performance.md, "On a real client over Wi-Fi": render to received,
# average, 1968x2184 AV1 HDR at 120 fps to a laptop on 5 GHz Wi-Fi, October 8, 2026.
WIFI_MS = 13.54
# docs/performance.md, "Next to Vibepollo 2.0": picture age beside the synthetic
# game load, (Vibepollo 2.0, Rubylight), October 8, 2026.
HOSTS = {('HEVC', 'average'): ('26.96', '19.61'), ('HEVC', 'p95'): ('37.62', '21.56'),
         ('AV1', 'average'): ('24.53', '17.15'), ('AV1', 'p95'): ('36.77', '18.96')}
# docs/performance.md, "Isolating Radeon compute": same build, compute off/on,
# average render-to-decode delay beside a controlled GPU load, October 4, 2026.
COMPUTE = ('41.0', '33.5')


def rounded(value, places='0.1'):
    return Decimal(value).quantize(Decimal(places), ROUND_HALF_UP)


SOONER = {codec: rounded(Decimal(HOSTS[codec, 'average'][0]) - Decimal(HOSTS[codec, 'average'][1]))
          for codec in ('HEVC', 'AV1')}
P95_GAIN = {codec: rounded((Decimal(HOSTS[codec, 'p95'][0]) - Decimal(HOSTS[codec, 'p95'][1]))
                           / Decimal(HOSTS[codec, 'p95'][0]) * 100, '1') for codec in ('HEVC', 'AV1')}
assert len(set(SOONER.values())) == 1, 'the film says the same gain for both codecs'
COMPUTE_GAIN = Decimal(COMPUTE[0]) - Decimal(COMPUTE[1])


def clamp(x):
    return min(1., max(0., x))


def smooth(x):
    x = clamp(x)
    return x*x*(3-2*x)


def ease(x):
    x = clamp(x)
    return 1-(1-x)**3


def mix(a, b, p):
    return tuple(round(x+(y-x)*clamp(p)) for x, y in zip(a, b))


FONT_ROOTS = (Path('C:/Windows/Fonts'), Path('/mnt/c/Windows/Fonts'))


@lru_cache(maxsize=None)
def font(size, weight='regular'):
    if weight == 'mono':
        for root in FONT_ROOTS:
            if (root/'consola.ttf').exists():
                return ImageFont.truetype(str(root/'consola.ttf'), size)
        return ImageFont.truetype('/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf', size)
    for root in FONT_ROOTS:
        if (root/'SegUIVar.ttf').exists():
            f = ImageFont.truetype(str(root/'SegUIVar.ttf'), size)
            f.set_variation_by_name({'light': 'Light Display', 'regular': 'Regular Display',
                                     'semibold': 'Semibold Display', 'bold': 'Bold Display'}[weight])
            return f
    name = 'DejaVuSans-Bold.ttf' if weight in ('bold', 'semibold') else 'DejaVuSans.ttf'
    return ImageFont.truetype('/usr/share/fonts/truetype/dejavu/'+name, size)


@lru_cache(maxsize=4000)
def lettering(value, size, color, weight):
    f = font(size, weight)
    box = f.getbbox(value)
    im = Image.new('RGBA', (max(1, box[2]-box[0]+4), max(1, box[3]-box[1]+4)))
    ImageDraw.Draw(im).text((2-box[0], 2-box[1]), value, font=f, fill=color)
    return im


def width_of(value, size, weight='regular'):
    return lettering(value, size, INK, weight).width


def txt(im, x, y, value, size=32, color=INK, weight='regular', align='left', opacity=1.):
    if opacity <= 0 or not value:
        return
    stamp = lettering(value, size, color, weight)
    if align == 'center':
        x -= stamp.width/2
    elif align == 'right':
        x -= stamp.width
    if opacity < 1:
        stamp = stamp.copy()
        stamp.putalpha(stamp.getchannel('A').point(lambda v: round(v*opacity)))
    im.paste(stamp, (round(x), round(y)), stamp)


def reveal(im, x, y, value, size, local, at=0., color=INK, weight='bold', align='left', rise=28):
    p = ease((local-at)/.6)
    txt(im, x, y+rise*(1-p), value, size, color, weight, align, p)


def draw(im):
    return ImageDraw.Draw(im)


def rect(im, box, fill=CARD, outline=None, width=1, radius=14):
    draw(im).rounded_rectangle(tuple(round(v) for v in box), radius=radius, fill=fill,
                               outline=outline, width=width)


def line(im, xy, fill=RULE, width=2):
    draw(im).line(tuple(round(v) for v in xy), fill=fill, width=width)


def overlay(im, box, paint, opacity):
    """Paint into a transparent layer over box, then blend it in at opacity."""
    if opacity <= 0:
        return
    x0, y0, x1, y1 = (round(v) for v in box)
    x0, y0, x1, y1 = max(0, x0), max(0, y0), min(W, x1), min(H, y1)
    if x1 <= x0 or y1 <= y0:
        return
    layer = Image.new('RGBA', (x1-x0, y1-y0))
    paint(ImageDraw.Draw(layer), -x0, -y0)
    if opacity < 1:
        layer.putalpha(layer.getchannel('A').point(lambda v: round(v*opacity)))
    im.paste(layer, (x0, y0), layer)


@lru_cache(maxsize=1)
def backdrop():
    im = Image.new('RGB', (W, H), BG)
    d = ImageDraw.Draw(im)
    for y in range(H):
        d.line((0, y, W, y), fill=mix(BG, (14, 16, 23), y/H))
    # A quiet dot grid: a measuring surface, not decoration that moves.
    for y in range(36, H, 48):
        for x in range(36, W, 48):
            d.point((x, y), fill=(29, 32, 42))
    return im


@lru_cache(maxsize=1)
def glow_sprite():
    size = 120
    im = Image.new('RGB', (size, size))
    px = im.load()
    for y in range(size):
        for x in range(size):
            d = math.hypot(x-size/2+.5, y-size/2+.5)/(size/2)
            a = clamp(1-d)**2.4*.30
            c = mix(RUBY_DEEP, RUBY, clamp(1.15-d))
            px[x, y] = tuple(round(v*a) for v in c)
    return im.resize((1500, 1500), Image.Resampling.BICUBIC)


# Where the warm light sits in each scene. It changes under the wipe and
# holds still otherwise, so a GIF of the film changes only what moves.
GLOW = ((1500, 900), (960, 1240), (1700, 260), (260, 980), (1600, 820), (960, 560))


def glow_at(t):
    return GLOW[scene_at(t)[0]]


@lru_cache(maxsize=4)
def base_at(x, y):
    layer = Image.new('RGB', (W, H))
    layer.paste(glow_sprite(), (x-750, y-750))
    return ImageChops.add(backdrop(), layer)


def base(t):
    return base_at(*glow_at(t)).copy()


def badge(im, x, y, size, opacity=1.):
    if opacity <= 0:
        return
    rect(im, (x, y, x+size, y+size), mix(BG, RUBY_DEEP, opacity), radius=round(size*.26))
    glyph = lettering('R', round(size*.66), INK, 'bold')
    txt(im, x+size/2, y+(size-glyph.height)/2, 'R', round(size*.66), INK, 'bold', 'center', opacity)


def brand(im):
    badge(im, 104, 54, 42)
    txt(im, 160, 56, 'Rubylight', 30, INK, 'bold')


def heading(im, value, local, subtitle=None):
    reveal(im, 100, 116, value, 68, local, at=.1)
    if subtitle:
        reveal(im, 104, 210, subtitle, 34, local, at=.25, color=MUTED, weight='regular', rise=16)


def source(im, value, local=99., at=0.):
    """The one line that says where a measurement comes from."""
    txt(im, 104, 1008, value, 28, MUTED, 'regular', opacity=ease((local-at)/.5))


# 1 · Cold open -----------------------------------------------------------------

@lru_cache(maxsize=None)
def picture(w, h):
    """The frame the film follows: a small dusk landscape from 'the game'."""
    k = 4
    im = Image.new('RGB', (w*k, h*k))
    d = ImageDraw.Draw(im)
    horizon = .64*h*k
    for y in range(h*k):
        if y < horizon:
            c = mix((22, 16, 44), (236, 70, 104), (y/horizon)**1.6)
        else:
            c = mix((40, 14, 34), (14, 10, 20), (y-horizon)/(h*k-horizon))
        d.line((0, y, w*k, y), fill=c)
    r = .2*h*k
    d.ellipse((.66*w*k-r, horizon-1.25*r, .66*w*k+r, horizon+.75*r), fill=(255, 196, 150))
    ridge = [(0, horizon)] + [(x*w*k/8, horizon-(.08+.07*((x*37) % 5)/4)*h*k) for x in range(9)] + [(w*k, horizon)]
    d.polygon(ridge + [(w*k, horizon+2), (0, horizon+2)], fill=(58, 20, 52))
    d.rectangle((0, horizon, w*k, h*k), fill=(26, 12, 26))
    vx = .5*w*k
    for i in range(-6, 7):
        d.line((vx, horizon, vx+i*.32*w*k, h*k), fill=(150, 40, 80), width=max(1, k))
    for j in range(1, 6):
        y = horizon+(h*k-horizon)*(j/5)**1.8
        d.line((0, y, w*k, y), fill=(120, 34, 70), width=max(1, k))
    return im.resize((w, h), Image.Resampling.LANCZOS)


PC_SCREEN = (176, 372, 476, 541)
PORT = (604, 470)
# Screen boxes of the TV, the laptop (the measured device) and the phone.
DEVICES = ((1452, 290, 1752, 459), (1478, 536, 1718, 671), (1522, 760, 1680, 828))
LAPTOP = 1


def ends():
    return [((box[0]-14), (box[1]+box[3])/2) for box in DEVICES]


def bezier(p0, p3, s):
    p1, p2 = (p0[0]+320, p0[1]), (p3[0]-320, p3[1])
    u = 1-s
    return (u**3*p0[0]+3*u*u*s*p1[0]+3*u*s*s*p2[0]+s**3*p3[0],
            u**3*p0[1]+3*u*u*s*p1[1]+3*u*s*s*p2[1]+s**3*p3[1])


def screen(im, box, lit):
    x0, y0, x1, y1 = box
    rect(im, box, CARD, radius=4)
    if lit > 0:
        shot = picture(x1-x0, y1-y0)
        if lit < 1:
            shot = Image.blend(Image.new('RGB', shot.size, CARD), shot, lit)
        im.paste(shot, (x0, y0))


def stage(im, lit):
    x0, y0, x1, y1 = PC_SCREEN
    rect(im, (x0-12, y0-12, x1+12, y1+12), (24, 27, 36), (52, 58, 72), 2, radius=12)
    screen(im, PC_SCREEN, 1)
    rect(im, (308, y1+12, 344, y1+44), (38, 42, 54), radius=2)
    rect(im, (252, y1+44, 400, y1+56), (38, 42, 54), radius=6)
    rect(im, (508, 340, 598, 600), (24, 27, 36), (52, 58, 72), 2, radius=12)
    rect(im, (522, 360, 530, 580), RUBY, radius=3)
    for i in range(3):
        line(im, (544, 380+i*16, 584, 380+i*16), (52, 58, 72), 2)
    txt(im, 386, 640, 'Your Radeon PC', 32, MUTED, 'regular', 'center')
    for end in ends():
        for k in range(1, 40):
            x, y = bezier(PORT, end, k/40)
            draw(im).ellipse((x-3, y-3, x+3, y+3), fill=(58, 64, 82))
    tv, laptop, phone = DEVICES
    measured = lit[LAPTOP] >= 1
    rect(im, (tv[0]-8, tv[1]-8, tv[2]+8, tv[3]+8), (24, 27, 36), (52, 58, 72), 2, radius=8)
    line(im, (tv[0]+60, tv[3]+8, tv[0]+44, tv[3]+30), (52, 58, 72), 4)
    line(im, (tv[2]-60, tv[3]+8, tv[2]-44, tv[3]+30), (52, 58, 72), 4)
    rect(im, (laptop[0]-12, laptop[1]-12, laptop[2]+12, laptop[3]+10), (24, 27, 36),
         RUBY if measured else (52, 58, 72), 3 if measured else 2, radius=10)
    draw(im).polygon([(laptop[0]-40, laptop[3]+12), (laptop[2]+40, laptop[3]+12),
                      (laptop[2]+62, laptop[3]+30), (laptop[0]-62, laptop[3]+30)], fill=(38, 42, 54))
    rect(im, (phone[0]-14, phone[1]-10, phone[2]+14, phone[3]+10), (24, 27, 36), (52, 58, 72), 2, radius=16)
    for box, level in zip(DEVICES, lit):
        screen(im, box, level)


def token(im, x, y, w=112):
    h = round(w*9/16)
    rect(im, (x-w/2-3, y-h/2-3, x+w/2+3, y+h/2+3), RUBY, radius=5)
    im.paste(picture(w, h), (round(x-w/2), round(y-h/2)))


FLIGHT = (.6, 3.1)


def cold_open(t):
    im = base(t)
    badge(im, 104, 64, 72)
    txt(im, 196, 62, 'Rubylight', 76, INK, 'bold')
    txt(im, 106, 170, 'Written in Rust. Built for Radeon. Made for Moonlight.', 44, INK, 'semibold')
    depart, arrive = FLIGHT
    p = (t-depart)/(arrive-depart)
    lit = tuple(smooth((t-arrive)/.3) for _ in DEVICES)
    stage(im, lit)
    targets = ends()
    if 0 < p < 1:
        s = smooth(p)
        for target in targets:
            for k in range(8, 0, -1):
                x, y = bezier(PORT, target, max(0, s-k*.02))
                r = 11-k
                draw(im).ellipse((x-r, y-r, x+r, y+r), fill=mix(BG, RUBY, .95-k*.1))
            token(im, *bezier(PORT, target, s))
    elif p >= 1:
        # Streaming on: packets keep arriving at every device.
        for target in targets:
            for n in range(6):
                s = ((t-arrive)/.9 + n/6) % 1
                x, y = bezier(PORT, target, s)
                a = smooth(s*5)*smooth((1-s)*5)*smooth((t-arrive)/.4)
                draw(im).ellipse((x-5, y-5, x+5, y+5), fill=mix(BG, RUBY, a))
    value = WIFI_MS*clamp(p)
    landed = p >= 1
    slot = width_of('00.0', 150, 'bold')
    color = RUBY if landed else INK
    number = f'{value:.1f}'
    stamp = lettering(number, 150, color, 'bold')
    txt(im, 104+slot, 820, number, 150, color, 'bold', 'right')
    unit = lettering('ms', 72, color, 'bold')
    txt(im, 104+slot+20, 820+stamp.height-unit.height, 'ms', 72, color, 'bold')
    label = 104+slot+20+unit.width+56
    reveal(im, label, 838, 'from a rendered picture', 38, t, at=arrive, color=INK, weight='semibold', rise=14)
    reveal(im, label, 890, 'to a laptop over Wi-Fi', 38, t, at=arrive+.1, color=INK, weight='semibold', rise=14)
    source(im, 'Average · 1968×2184 AV1 HDR at 120 fps · 5 GHz Wi-Fi · October 8, 2026 · docs/performance.md',
           t, arrive+.3)
    return im


# 2 · The console ---------------------------------------------------------------

@lru_cache(maxsize=None)
def console_shot(name):
    return Image.open(MEDIA/'console'/f'{name}.png').convert('RGB')


@lru_cache(maxsize=1)
def console_boxes():
    return json.loads((MEDIA/'console'/'boxes.json').read_text(encoding='utf-8'))


def union(*boxes):
    return (min(b[0] for b in boxes), min(b[1] for b in boxes), max(b[2] for b in boxes), max(b[3] for b in boxes))


def fit(box, width, height, centre=(960, 450), pad=36):
    """Camera that shows box, padded, within width x height around centre."""
    x0, y0, x1, y1 = box[0]-pad, box[1]-pad, box[2]+pad, box[3]+pad
    return ((x0+x1)/2, (y0+y1)/2, min(width/(x1-x0), height/(y1-y0)), centre)


def blend_camera(a, b, p):
    p = smooth(p)
    s = math.exp(math.log(a[2])+(math.log(b[2])-math.log(a[2]))*p)
    return (a[0]+(b[0]-a[0])*p, a[1]+(b[1]-a[1])*p, s,
            (a[3][0]+(b[3][0]-a[3][0])*p, a[3][1]+(b[3][1]-a[3][1])*p))


@lru_cache(maxsize=6)
def window(name, cx, cy, s, ox, oy):
    """Screenshot name with its point (cx, cy) at (ox, oy) on screen, scaled by s."""
    src = console_shot(name)
    iw, ih = src.size
    x0, y0, x1, y1 = ox-cx*s, oy-cy*s, ox+(iw-cx)*s, oy+(ih-cy)*s
    dx0, dy0 = max(0, math.floor(x0)), max(0, math.floor(y0))
    dx1, dy1 = min(W, math.ceil(x1)), min(H, math.ceil(y1))
    box = (max(0., (dx0-ox)/s+cx), max(0., (dy0-oy)/s+cy), min(iw, (dx1-ox)/s+cx), min(ih, (dy1-oy)/s+cy))
    view = src.resize((dx1-dx0, dy1-dy0), Image.Resampling.BICUBIC, box=box)
    mask = Image.new('L', view.size, 0)
    radius = max(4, round(22*s))
    ImageDraw.Draw(mask).rounded_rectangle((x0-dx0, y0-dy0, x1-dx0-1, y1-dy0-1), radius=radius, fill=255)
    return view, mask, (dx0, dy0), (x0, y0, x1, y1), radius


def place(im, name, camera):
    cx, cy, s, (ox, oy) = camera
    view, mask, at, outer, radius = window(name, round(cx, 2), round(cy, 2), round(s, 5), round(ox, 2), round(oy, 2))
    im.paste(view, at, mask)
    draw(im).rounded_rectangle(tuple(round(v) for v in outer), radius=radius, outline=(58, 64, 80), width=2)


def to_screen(box, camera):
    cx, cy, s, (ox, oy) = camera
    return (ox+(box[0]-cx)*s, oy+(box[1]-cy)*s, ox+(box[2]-cx)*s, oy+(box[3]-cy)*s)


def ring(im, box, opacity, pad=12):
    x0, y0, x1, y1 = box[0]-pad, box[1]-pad, box[2]+pad, box[3]+pad

    def paint(d, dx, dy):
        d.rounded_rectangle((x0+dx, y0+dy, x1+dx, y1+dy), radius=16, outline=RUBY+(255,), width=5)
    overlay(im, (x0-4, y0-4, x1+5, y1+5), paint, opacity)


def caption(im, title, detail, local, at):
    p = ease((local-at)/.55)
    if p <= 0:
        return
    w = max(width_of(title, 48, 'bold'), width_of(detail, 32))+96
    y = 852+(1-p)*260
    rect(im, (64, y, 64+w, y+168), (15, 17, 23), (58, 64, 80), 2, radius=18)
    rect(im, (64+28, y+36, 64+36, y+132), RUBY, radius=4)
    txt(im, 64+60, y+28, title, 48, INK, 'bold')
    txt(im, 64+60, y+100, detail, 32, MUTED, 'regular')


@lru_cache(maxsize=1)
def shots():
    """Each console shot: screenshot, camera keys (local time, camera), caption, ring."""
    b = console_boxes()
    full = (0, 0) + console_shot('overview').size
    wide = fit(full, 1800, 960, (960, 540), 0)
    ov, dv, st = b['overview'], b['devices'], b['settings']
    card = fit(ov['card'], 1720, 700)
    pair = fit(union(dv['tabs'], dv['request']), 1720, 680)
    codecs = st['codecs']
    rows = fit((codecs[0], codecs[1], codecs[2], codecs[1]+680), 1760, 700)
    # Each shot opens on the whole page, then moves in on one part of it.
    return (
        dict(name='overview', span=(0, 6.3), keys=((0, wide), (1.5, wide), (2.5, card)),
             caption=('Watch every stream live.', 'Resolution, codec, frame rate and encoding time, in your browser.', .5),
             ring=(ov['metrics'], 3.0)),
        dict(name='devices', span=(5.7, 12.3), keys=((0, wide), (1.4, wide), (2.4, pair)),
             caption=('Pair a device with its PIN.', 'Moonlight shows a PIN. Type it here, then press Pair.', .8),
             ring=(dv['request'], 2.9)),
        dict(name='settings', span=(11.7, 18), keys=((0, wide), (1.4, wide), (2.4, rows)),
             caption=('Settings in plain words.', 'Each option says what it does and when it applies.', .8),
             ring=None),
    )


def camera_at(keys, local):
    if local <= keys[0][0]:
        return keys[0][1]
    for (t0, a), (t1, b) in zip(keys, keys[1:]):
        if local < t1:
            return blend_camera(a, b, (local-t0)/(t1-t0))
    return keys[-1][1]


def shot_frame(t, shot):
    start = shot['span'][0]
    local = t-start
    im = base(t+STARTS[1])
    camera = camera_at(shot['keys'], local)
    place(im, shot['name'], camera)
    if shot['ring']:
        box, at = shot['ring']
        ring(im, to_screen(box, camera), smooth((local-at)/.5))
    title, detail, at = shot['caption']
    caption(im, title, detail, local, at)
    return im


def sample_note(im):
    """The console is real; the stream it shows is sample data."""
    note = 'The real console, showing sample stream data'
    w = width_of(note, 28)
    rect(im, (1856-w-36, 8, 1856, 52), (15, 17, 23), (58, 64, 80), 2, radius=12)
    txt(im, 1856-18, 15, note, 28, MUTED, 'regular', 'right')


def console(t):
    active = [s for s in shots() if s['span'][0] <= t < s['span'][1]] or [shots()[-1]]
    im = shot_frame(t, active[-1])
    if len(active) == 2:
        # The next page pushes the last one out to the left.
        old, new = active
        shift = round(W*smooth((t-new['span'][0])/(old['span'][1]-new['span'][0])))
        if shift < W:
            im.paste(im.crop((0, 0, shift, H)), (W-shift, 0))
            im.paste(shot_frame(t, old).crop((shift, 0, W, H)), (0, 0))
    sample_note(im)
    return im


# 3 · Speed -----------------------------------------------------------------------

BASE_Y = 852
MS = 13.2
GROUPS = (('HEVC', 'average'), ('HEVC', 'p95'), ('AV1', 'average'), ('AV1', 'p95'))
BAR, GAP, PITCH, LEFT = 104, 14, 268, 120


def focus(local, metric):
    """How lit a group is while the film talks about averages, then p95."""
    if metric == 'average':
        return 1-.65*smooth((local-7.2)/.35)*(1-smooth((local-10.)/.35))
    return 1-.65*smooth((local-4.4)/.35)*(1-smooth((local-7.2)/.35))


def speed(t):
    im = base(t+STARTS[2])
    brand(im)
    heading(im, 'Picture age beside a game', t,
            'Lower is better · same RX 7900 XT and settings · HDR at 120 fps')
    p = ease((t-.5)/.6)
    rect(im, (106, 288, 132, 314), mix(BG, GREY, p), radius=4)
    txt(im, 146, 282, 'Vibepollo 2.0', 32, INK, 'semibold', opacity=p)
    rect(im, (406, 288, 432, 314), mix(BG, RUBY, p), radius=4)
    txt(im, 446, 282, 'Rubylight', 32, INK, 'semibold', opacity=p)
    line(im, (104, BASE_Y, LEFT+3*PITCH+2*BAR+GAP+16, BASE_Y), DIM, 2)
    for g, (codec, metric) in enumerate(GROUPS):
        gx = LEFT+g*PITCH
        lit = focus(t, metric)
        values = [float(v) for v in HOSTS[codec, metric]]
        heights = []
        for b, (value, color) in enumerate(zip(values, (GREY, RUBY))):
            x = gx+b*(BAR+GAP)
            q = ease((t-1.0-b*.9-g*.15)/1.4)
            h = value*MS*q
            heights.append(h)
            if h > 2:
                rect(im, (x, BASE_Y-h, x+BAR, BASE_Y), mix(BG, color, lit), radius=8)
                rect(im, (x, BASE_Y-min(h, 10), x+BAR, BASE_Y), mix(BG, color, lit), radius=0)
            if h > 54:
                txt(im, x+BAR/2, BASE_Y-h+14, f'{value*q:.2f}', 30, BG, 'bold', 'center')
        center = gx+BAR+GAP/2
        txt(im, center, BASE_Y+22, codec, 34, mix(BG, INK, lit), 'bold', 'center')
        txt(im, center, BASE_Y+66, 'average' if metric == 'average' else 'p95', 30, mix(BG, MUTED, lit),
            'regular', 'center')
        shown = smooth((t-(4.6 if metric == 'average' else 7.4))/.5)
        if shown > 0:
            top, ours = BASE_Y-heights[0], BASE_Y-heights[1]
            for x in range(gx, gx+2*BAR+GAP, 14):
                line(im, (x, top, x+7, top), mix(BG, INK, shown*lit), 2)
            gain = f'−{SOONER[codec]} ms' if metric == 'average' else f'−{P95_GAIN[codec]}%'
            txt(im, gx+BAR+GAP+BAR/2, (top+ours)/2-17, gain, 30, mix(BG, RUBY, shown*lit), 'bold', 'center')
    x = 1290
    sooner = next(iter(SOONER.values()))
    gains = sorted(P95_GAIN.values())
    reveal(im, x, 360, f'{sooner} ms', 124, t, at=4.6, color=RUBY)
    reveal(im, x+4, 512, 'sooner on average,', 38, t, at=4.8, color=INK, weight='semibold', rise=12)
    reveal(im, x+4, 560, 'in HEVC and AV1', 38, t, at=4.9, color=INK, weight='semibold', rise=12)
    reveal(im, x, 650, f'{gains[0]}–{gains[1]}%', 124, t, at=7.4, color=RUBY)
    reveal(im, x+4, 802, 'lower p95 picture age:', 38, t, at=7.6, color=INK, weight='semibold', rise=12)
    reveal(im, x+4, 850, 'fewer slow frames', 38, t, at=7.7, color=INK, weight='semibold', rise=12)
    source(im, 'Matched runs with a synthetic game load · rendered to decoded · October 8, 2026 · docs/performance.md',
           t, .6)
    return im


# 4 · Radeon compute -------------------------------------------------------------

LANES = (('Graphics queue', 344), ('Compute queue', 484), ('AMF encoder', 624))
LANE_H = 76
X0 = 430


def schedule():
    """(lane, x0, x1, label) blocks: the game keeps drawing on graphics while
    the previous picture is copied and converted on compute, then encoded."""
    blocks = []
    width, gap = 300, 12
    for i in range(5):
        start = X0+i*(width+gap)
        blocks.append((0, start, min(1816, start+width), 'Game'))
        if i < 3:
            end = start+width
            blocks += [(1, end+12, end+116, 'Copy'), (1, end+126, end+276, 'Colour'),
                       (2, end+286, end+456, 'Encode')]
    return blocks


def playhead(local):
    return X0+(1816-X0)*clamp((local-.7)/4.3)


def compute(t):
    im = base(t+STARTS[3])
    brand(im)
    heading(im, 'Frame prep runs beside your game.', t,
            'Rubylight copies and converts each picture on a Radeon compute queue, then AMF encodes it.')
    cursor = playhead(t)
    for name, y in LANES:
        txt(im, 104, y+18, name, 32, INK if name != 'Compute queue' else RUBY, 'semibold')
        line(im, (X0, y+LANE_H+10, 1816, y+LANE_H+10), RULE, 1)
    colours = (DIM, RUBY, (67, 133, 159))
    for lane, x0, x1, label in schedule():
        y = LANES[lane][1]
        rect(im, (x0, y, x1, y+LANE_H), CARD, RULE, 2, radius=8)
        p = clamp((cursor-x0)/(x1-x0))
        if p > 0:
            rect(im, (x0, y, x0+max(10, (x1-x0)*p), y+LANE_H), mix(CARD, colours[lane], .8), radius=8)
        if x1-x0 > 70:
            txt(im, (x0+x1)/2, y+20, label, 30, INK if p > .5 else MUTED, 'bold', 'center')
        if lane == 1 and label == 'Copy':
            # The finished game picture hands over to compute.
            on = cursor >= x0
            line(im, (x0-10, LANES[0][1]+LANE_H, x0-10, y+LANE_H/2), MINT if on else DIM, 3)
            line(im, (x0-10, y+LANE_H/2, x0, y+LANE_H/2), MINT if on else DIM, 3)
        if lane == 2:
            on = cursor >= x0
            line(im, (x0-10, LANES[1][1]+LANE_H, x0-10, y+LANE_H/2), MINT if on else DIM, 3)
            line(im, (x0-10, y+LANE_H/2, x0, y+LANE_H/2), MINT if on else DIM, 3)
    if 0 < cursor < 1816:
        line(im, (cursor, 326, cursor, 718), INK, 3)
    p = ease((t-5.1)/.6)
    if p:
        y = 790+20*(1-p)
        txt(im, 104, y, f'{COMPUTE_GAIN} ms', 92, RUBY, 'bold', opacity=p)
        gain_w = width_of(f'{COMPUTE_GAIN} ms', 92, 'bold')
        txt(im, 104+gain_w+40, y+6, 'lower average picture age with compute on,', 36, INK, 'semibold', opacity=p)
        txt(im, 104+gain_w+40, y+54, 'same build, beside a GPU load', 36, INK, 'semibold', opacity=p)
    source(im, 'Same build, compute off vs on · 1080p60 HEVC HDR · October 4, 2026 · docs/performance.md', t, 5.3)
    return im


# 5 · Colour ---------------------------------------------------------------------

HUD_BG = (22, 24, 32)


@lru_cache(maxsize=1)
def hud():
    """The same rendered text as 4:4:4 and as 4:2:0 (colour shared by 2x2 pixels)."""
    im = Image.new('RGB', (240, 108), HUD_BG)
    d = ImageDraw.Draw(im)
    f = font(17, 'semibold')
    d.text((10, 8), 'Objective captured', font=f, fill=(236, 236, 236))
    d.text((10, 42), 'Low ammo', font=f, fill=(240, 36, 52))
    d.text((10, 76), 'Ally: on my way', font=f, fill=(60, 118, 255))
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


PANELS = ((104, 296, 940, 806), (980, 296, 1816, 806))


def magnified(source, box, cx, cy, zoom):
    w, h = box[2]-box[0], box[3]-box[1]
    panel = Image.new('RGB', (w, h), HUD_BG)
    sx0, sy0 = cx-w/2/zoom, cy-h/2/zoom
    ix0, iy0 = max(0, math.ceil(sx0)), max(0, math.ceil(sy0))
    ix1, iy1 = min(source.width, math.floor(cx+w/2/zoom)), min(source.height, math.floor(cy+h/2/zoom))
    dx0, dy0 = round((ix0-sx0)*zoom), round((iy0-sy0)*zoom)
    dx1, dy1 = round((ix1-sx0)*zoom), round((iy1-sy0)*zoom)
    part = source.resize((dx1-dx0, dy1-dy0), Image.Resampling.NEAREST, box=(ix0, iy0, ix1, iy1))
    panel.paste(part, (dx0, dy0))
    return panel


def colour(t):
    im = base(t+STARTS[4])
    brand(im)
    heading(im, 'Full colour with PyroWave.', t,
            '4:2:0 shares one colour sample between four pixels. 4:4:4 keeps one for every pixel.')
    sampled, full = hud()
    p = smooth((t-1.2)/1.8)
    zoom = math.exp(math.log(3.4)+(math.log(8.)-math.log(3.4))*p)
    cx, cy = 120+(58-120)*p, 54+(66-54)*p
    for box, source_image, label, color in zip(PANELS, (sampled, full), ('4:2:0', '4:4:4 · PyroWave'),
                                               (MUTED, RUBY)):
        im.paste(magnified(source_image, box, cx, cy, round(zoom, 3)), box[:2])
        rect(im, box, None, (58, 64, 80), 2, radius=6)
        w = width_of(label, 36, 'bold')
        rect(im, (box[0]+18, box[1]+18, box[0]+18+w+36, box[1]+82), (12, 13, 18), color, 2, radius=10)
        txt(im, box[0]+36, box[1]+30, label, 36, color, 'bold')
    reveal(im, 104, 838, 'Red and blue text keeps its edges in 4:4:4.', 40, t, at=3.0, color=INK,
           weight='semibold', rise=12)
    reveal(im, 104, 900, 'Play it on Nightjar for Android or Nonary’s Moonlight, over a fast local network.', 32, t,
           at=3.3, color=MUTED, weight='regular', rise=12)
    source(im, 'Same rendered pixels in both panels · 4:2:0 averages colour over each 2×2 block · BT.709', t, 3.5)
    return im


# 6 · Outro ----------------------------------------------------------------------

def outro(t):
    im = base(t+STARTS[5])
    p = ease((t-.1)/.7)
    mark = width_of('Rubylight', 150, 'bold')
    left = (W-(140+44+mark))/2
    badge(im, left, 196+20*(1-p), 140, p)
    reveal(im, left+184, 180, 'Rubylight', 150, t, at=.15)
    words, gap = ('Install.', 'Pair.', 'Play.'), 64
    widths = [width_of(w, 84, 'bold') for w in words]
    x = (W-sum(widths)-gap*2)/2
    for i, value in enumerate(words):
        reveal(im, x, 410, value, 84, t, at=.6+i*.25, color=RUBY)
        x += widths[i]+gap
    reveal(im, 960, 590, 'github.com/RamazanKara/Rubylight', 60, t, at=1.5, weight='bold', align='center')
    reveal(im, 960, 690, 'Free and open source · Windows · Radeon · Moonlight', 34, t, at=1.8,
           color=MUTED, weight='regular', align='center', rise=12)
    reveal(im, 960, 846, 'As long as AMD users are happy, Rubylight is happy.', 44, t, at=2.4,
           color=INK, weight='semibold', align='center', rise=12)
    return im


SCENES = (cold_open, console, speed, compute, colour, outro)


def scene_at(t):
    index = next((i for i, end in enumerate(ENDS) if t < end), len(ENDS)-1)
    return index, t-STARTS[index]


def frame(t):
    index, local = scene_at(t)
    im = SCENES[index](local)
    if index and local < WIPE:
        old = SCENES[index-1](ENDS[index-1]-STARTS[index-1]-.001)
        p = smooth(local/WIPE)
        lean = 260
        edge = -lean-60+(W+2*lean+120)*p
        mask = Image.new('L', (W, H), 0)
        draw(mask).polygon([(-10, 0), (edge+lean, 0), (edge, H), (-10, H)], fill=255)
        im = Image.composite(im, old, mask)
        d = draw(im)
        d.polygon([(edge+lean, 0), (edge+lean+46, 0), (edge+46, H), (edge, H)], fill=RUBY)
        d.polygon([(edge+lean+46, 0), (edge+lean+58, 0), (edge+58, H), (edge+46, H)], fill=ROSE)
    return im


STORY = (1.0, 3.8, 5.8, 7.0, 9.5, 13.0, 16.5, 20.5, 23.6, 26.0, 30.0, 33.0, 37.5,
         40.5, 45.5, 47.5, 52.5, 54.0, 59.0, 59.9)


def storyboard(destination):
    columns, tile = 4, (480, 270)
    rows = math.ceil(len(STORY)/columns)
    sheet = Image.new('RGB', (columns*tile[0], rows*(tile[1]+32)), BG)
    for i, t in enumerate(STORY):
        image = frame(t).resize(tile, Image.Resampling.LANCZOS)
        x, y = i % columns*tile[0], i//columns*(tile[1]+32)
        sheet.paste(image, (x, y))
        txt(sheet, x+10, y+tile[1]+4, f'{t:04.1f}s', 20, MUTED, 'mono')
    destination.parent.mkdir(parents=True, exist_ok=True)
    sheet.save(destination)


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('--output', type=Path)
    p.add_argument('--audio', type=Path)
    p.add_argument('--frame', type=float)
    p.add_argument('--storyboard', type=Path)
    p.add_argument('--fps', type=int, default=60)
    p.add_argument('--width', type=int, default=1920)
    args = p.parse_args()
    if args.storyboard:
        storyboard(args.storyboard)
    if args.output and args.frame is not None:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        frame(args.frame).save(args.output)
    elif args.output:
        ffmpeg = shutil.which('ffmpeg')
        if not ffmpeg:
            p.error('FFmpeg is required.')
        args.output.parent.mkdir(parents=True, exist_ok=True)
        vf = f'scale={args.width}:-2:flags=lanczos:out_color_matrix=bt709:out_range=tv'
        cmd = [ffmpeg, '-hide_banner', '-loglevel', 'error', '-y', '-f', 'rawvideo', '-pixel_format', 'rgb24',
               '-video_size', f'{W}x{H}', '-framerate', str(args.fps), '-i', 'pipe:0']
        if args.audio:
            cmd += ['-i', str(args.audio), '-map', '0:v', '-map', '1:a', '-c:a', 'aac', '-b:a', '192k']
        else:
            cmd += ['-an']
        cmd += ['-vf', vf, '-c:v', 'libx264', '-threads', '4', '-preset', 'medium', '-crf', '19',
                '-pix_fmt', 'yuv420p', '-movflags', '+faststart', '-color_primaries', 'bt709',
                '-color_trc', 'bt709', '-colorspace', 'bt709', '-color_range', 'tv',
                '-t', str(DURATION), str(args.output)]
        with subprocess.Popen(cmd, stdin=subprocess.PIPE) as proc:
            try:
                for n in range(DURATION*args.fps):
                    proc.stdin.write(frame(n/args.fps).tobytes())
                    if n % (args.fps*5) == 0:
                        print(f'Rendered {n//args.fps}/{DURATION}s', flush=True)
            finally:
                proc.stdin.close()
            if proc.wait():
                raise RuntimeError('FFmpeg encoding failed')
        print(f'Wrote {args.output}', flush=True)
    elif not args.storyboard:
        p.error('Use --output or --storyboard.')


if __name__ == '__main__':
    main()
