# The ambient bake, as a reference implementation for BackdropBlur.make and the desktop's CPU blur.
# Squash the whole cover into a thumbnail, blur, lift saturation, then clamp each pixel's luminance. Night: pull yellows
# toward amber so they never dim to olive, then clamp from above, so a white cover comes out as a safe grey and a dark
# cover keeps its colour. Day: mix toward white up to a floor, so a dark cover comes out as a pale wash and a light one
# keeps its colour. Runs once per cover, no per-frame work. Writes ambient/<cover>-<l|p>[-day].png (gitignored).
import glob
import json
import os
import numpy as np
from PIL import Image, ImageFilter

HERE = os.path.dirname(os.path.abspath(__file__))
TOKENS = json.load(open(os.path.join(HERE, '..', 'tokens.json'), encoding='utf-8'))
OUT = os.path.join(HERE, 'ambient')
LUMA = np.array([0.2126, 0.7152, 0.0722])


def to_linear(c):
    return np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)


def to_srgb(c):
    return np.where(c <= 0.0031308, c * 12.92, 1.055 * np.clip(c, 0, None) ** (1 / 2.4) - 0.055)


def bake(img, size, day):
    small = img.convert('RGB').resize(size, Image.BILINEAR).filter(ImageFilter.GaussianBlur(radius=size[0] / 8))
    hsv = np.asarray(small.convert('HSV'), dtype=np.float32)
    h, s, v = hsv[..., 0] * 360 / 255, hsv[..., 1] / 255, hsv[..., 2] / 255
    s = np.clip(s * (TOKENS['day.ambient.saturate'] if day else TOKENS['ambient.saturate']), 0, 1)
    if not day:
        # Yellow to green-yellow (35..80 deg) dims to olive: move it toward amber-orange.
        band = np.clip((h - 25) / 15, 0, 1) * np.clip((95 - h) / 15, 0, 1)
        h = h + (22 - h) * band
    rgb = np.asarray(Image.fromarray(np.stack([h * 255 / 360, s * 255, v * 255], -1).astype(np.uint8), 'HSV').convert('RGB'), dtype=np.float32) / 255
    lin = to_linear(rgb)
    lum = (lin * LUMA).sum(-1, keepdims=True)
    if day:
        floor = TOKENS['day.ambient.textZoneMinLuma']
        lin = lin + (1 - lin) * np.maximum(0, (floor - lum) / (1 - lum + 1e-5))
    else:
        lin = lin * np.minimum(1, TOKENS['ambient.textZoneMaxLuma'] / np.maximum(lum, 1e-5))
    return Image.fromarray((np.clip(to_srgb(lin), 0, 1) * 255 + 0.5).astype(np.uint8), 'RGB')


os.makedirs(OUT, exist_ok=True)
sources = {os.path.splitext(os.path.basename(p))[0]: Image.open(p) for p in glob.glob(os.path.join(HERE, 'covers', '*.png'))}
sources['white'] = Image.new('RGB', (600, 900), (255, 255, 255))
sources['black'] = Image.new('RGB', (600, 900), (0, 0, 0))
for name, img in sources.items():
    for suffix, size in (('l', (96, 54)), ('p', (54, 96))):
        bake(img, size, False).save(f'{OUT}/{name}-{suffix}.png')
        bake(img, size, True).save(f'{OUT}/{name}-{suffix}-day.png')
for name, day in (('white', False), ('black', True)):
    px = np.asarray(bake(sources[name], (96, 54), day), dtype=np.float32)[0, 0] / 255
    print(name, 'cover bakes to', (px * 255).astype(int), 'luma', round(float((to_linear(px) * LUMA).sum()), 3), 'day' if day else 'night')
