# Measures rendered contrast in the white-cover stress shots: the brightest backdrop pixel in a text-free patch of each
# surface against the label and secondary text colours. Coordinates are CSS px times scale.
from PIL import Image
LABEL = (0xF5, 0xF6, 0xFB)
SEC = (0xCD, 0xD0, 0xDC)
DAY_LABEL = (0x1A, 0x1C, 0x22)
DAY_SEC = (0x3D, 0x40, 0x4D)


def lin(c):
    c /= 255
    return c / 12.92 if c <= 0.03928 else ((c + 0.055) / 1.055) ** 2.4


def lum(p):
    return 0.2126 * lin(p[0]) + 0.7152 * lin(p[1]) + 0.0722 * lin(p[2])


def ratio(a, b):
    x, y = sorted((lum(a), lum(b)), reverse=True)
    return (x + 0.05) / (y + 0.05)


CASES = [
    ('stress-day-phone-home', 'day tab bar over black cover', (214, 842, 220, 860), 2),
    ('stress-day-phone-sheet', 'day sheet', (372, 372, 395, 392), 2),
    ('stress-day-tv-home', 'day capsule over black', (760 * 1.5, 34 * 1.5, 780 * 1.5, 40 * 1.5), 2),
    ('stress-phone-home-white', 'tab bar over white cover', (214, 842, 220, 860), 2),
    ('stress-phone-home-white', 'selected tab pill', (26, 862, 34, 872), 2),
    ('stress-phone-home-white', 'round button', (352, 58, 356, 66), 2),
    ('stress-phone-home-white', 'ambient behind header', (120, 578, 300, 590), 2),
    ('stress-phone-sheet-white', 'sheet', (372, 372, 395, 392), 2),
    ('stress-tv-stream-white', 'stream menu', (1090 * 1.5, 40 * 1.5, 1130 * 1.5, 70 * 1.5), 2),
    ('stress-tv-stream-window-white', 'stream menu, cross-window', (1090 * 1.5, 40 * 1.5, 1130 * 1.5, 70 * 1.5), 2),
    ('stress-tv-stream-white', 'stats capsule, label only', (150, 38, 250, 42), 2),
    ('stress-tv-game-white', 'ambient behind title', (900 * 1.5, 130 * 1.5, 1100 * 1.5, 160 * 1.5), 2),
    ('stress-tv-game-white', 'group row on ambient', (800 * 1.5, 480 * 1.5, 1000 * 1.5, 486 * 1.5), 2),
]
bad = 0
for shot, what, box, scale in CASES:
    im = Image.open(f'shots/{shot}.png').convert('RGB').crop(tuple(int(v * scale) for v in box))
    day = what.startswith('day')
    px = (min if day else max)(list(im.getdata()), key=lum)
    a, b = ratio(DAY_LABEL if day else LABEL, px), ratio(DAY_SEC if day else (0xFF, 0xD7, 0x9A) if 'selected' in what else SEC, px)
    flag = '' if a >= 4.5 and (b >= 4.5 or 'label only' in what) else 'FAIL'
    bad += bool(flag)
    print(f'{what:30s} worst {px}  label {a:5.2f}  secondary {b:5.2f} {flag}')
print('fails:', bad)
