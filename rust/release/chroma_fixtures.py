#!/usr/bin/env python3
"""Deterministic text and UI frame sequences for the 4:2:0 against packed 4:4:4 measurements (B3).

Three scenes (code editor, browser article, spreadsheet) are drawn with ClearType-style
sub-pixel text and saved as numbered PNG frames. Each sequence holds still for 30 frames,
scrolls 6 px a frame for 30 frames, then holds still again, so it covers the first picture,
the scroll and the settled picture. The same inputs always give the same pixels (fonts are
the Windows ones: Consolas, Segoe UI and Calibri).

A fourth scene, `mixed`, is for the encoder focus A/B (focus_ab.py): the editor stays still while a
640x360 clip window plays and a game view pans under a still HUD bar, every frame.

Each folder gets a `regions.json` beside its frames: the still and moving rectangles (x, y, width,
height, on even coordinates) the quality scores are cropped to, and for `mixed` the HUD bar.

  python chroma_fixtures.py OUTDIR [--size 1920x1080] [--kind editor|browser|sheet|mixed|all] [--frames 90]
"""
import argparse
import io
import json
import pathlib
import random
import re
import sys

import numpy as np
from PIL import Image, ImageDraw, ImageFont

FONT_DIRECTORY = pathlib.Path('C:/Windows/Fonts')
HOLD, SCROLL, STEP = 30, 30, 6
KINDS = ('editor', 'browser', 'sheet')
# Where each scene's scrolling layer starts, and the rows below it that stay still.
LAYER_ORIGIN = dict(editor=(300, 36), browser=(0, 84), sheet=(0, 166))
LAYER_BOTTOM = dict(editor=24, browser=0, sheet=54)


def scroll_offset(frame):
    """Pixels scrolled at `frame`: still, then STEP px a frame, then still."""
    return STEP * min(max(frame - HOLD + 1, 0), SCROLL)


def hex_color(text):
    return tuple(int(text[i:i + 2], 16) for i in (1, 3, 5))


class Font:
    """A TrueType font drawn at three times its size, so the text can be filtered per sub-pixel."""

    cache = {}

    def __init__(self, name, size):
        self.big = ImageFont.truetype(str(FONT_DIRECTORY / name), size * 3)
        self.size = size
        self.height = int(round(self.big.getmetrics()[0] / 3 + self.big.getmetrics()[1] / 3)) + 2

    @classmethod
    def get(cls, name, size):
        return cls.cache.setdefault((name, size), cls(name, size))

    def width(self, text):
        return self.big.getlength(text) / 3


FILTER = np.array([1, 2, 3, 2, 1], np.float32) / 9
COVERAGE = {}


def coverage(font, text):
    """Per-channel coverage (rows, columns, RGB) of `text`, as ClearType draws it with RGB stripes."""
    key = (id(font), text)
    if key not in COVERAGE:
        w = int(font.width(text)) + 6
        mask = Image.new('L', (w * 3, font.height * 3), 0)
        ImageDraw.Draw(mask).text((6, 3), text, font=font.big, fill=255)
        rows = np.asarray(mask, np.float32).reshape(font.height, 3, w * 3).mean(axis=1) / 255
        padded = np.pad(rows, ((0, 0), (2, 2)))
        sub = sum(FILTER[i] * padded[:, i:i + w * 3] for i in range(5))
        COVERAGE[key] = np.clip(sub, 0, 1).reshape(font.height, w, 3)
    return COVERAGE[key]


class Canvas:
    def __init__(self, width, height, fill):
        self.pixels = np.empty((height, width, 3), np.float32)
        self.pixels[:] = hex_color(fill)

    @property
    def width(self):
        return self.pixels.shape[1]

    @property
    def height(self):
        return self.pixels.shape[0]

    def rect(self, x0, y0, x1, y1, fill):
        x0, y0 = max(int(x0), 0), max(int(y0), 0)
        self.pixels[y0:max(int(y1), y0), x0:max(int(x1), x0)] = hex_color(fill)

    def box(self, x0, y0, x1, y1, color, thickness=1):
        self.rect(x0, y0, x1, y0 + thickness, color)
        self.rect(x0, y1 - thickness, x1, y1, color)
        self.rect(x0, y0, x0 + thickness, y1, color)
        self.rect(x1 - thickness, y0, x1, y1, color)

    def text(self, x, y, text, font, color, anchor='l'):
        """Draw `text` with its line box at (x, y); returns the advance in pixels."""
        advance = font.width(text)
        if text.strip():
            if anchor == 'r':
                x -= advance
            elif anchor == 'm':
                x -= advance / 2
            cov = coverage(font, text)
            left, top = int(round(x)) - 2, int(y)
            x0, y0 = max(left, 0), max(top, 0)
            x1, y1 = min(left + cov.shape[1], self.width), min(top + cov.shape[0], self.height)
            if x1 > x0 and y1 > y0:
                c = cov[y0 - top:y1 - top, x0 - left:x1 - left]
                region = self.pixels[y0:y1, x0:x1]
                region += (np.array(hex_color(color), np.float32) - region) * c
        return advance

    def line(self, x0, y0, x1, y1, color):
        self.rect(x0, y0, x1 + (x0 == x1), y1 + (y0 == y1), color)

    def image(self, y0, y1):
        return Image.fromarray(np.clip(self.pixels[y0:y1] + 0.5, 0, 255).astype(np.uint8), 'RGB')


KEYWORDS = {'fn', 'let', 'mut', 'pub', 'if', 'else', 'for', 'while', 'match', 'return', 'impl', 'struct', 'use',
            'const', 'loop', 'in', 'as', 'self', 'true', 'false', 'where', 'enum', 'mod'}
TOKEN = re.compile(r'//.*|"[^"]*"|\b\d+(?:\.\d+)?\b|\b[A-Za-z_]\w*\b|\s+|.')
WORDS = ('stream encoder frame packet latency chroma luma colour bitrate session display capture decode buffer '
         'window cursor client host driver queue timing record preset quality network link profile device '
         'setting option pairing audio video input output sample value block layer plane texture shader').split()
NAMES = ('compute_offset', 'read_frame', 'send_packet', 'update_rate', 'pack_chroma', 'open_session', 'flush_queue',
         'parse_header', 'next_block', 'encode_plane', 'check_limit', 'build_table')
TYPES = ('Result', 'Option', 'Vec', 'FrameInfo', 'Session', 'Config', 'Encoder', 'Packet', 'Geometry', 'Duration')


def code_lines(rng, count):
    out = []
    indent = 0
    while len(out) < count:
        n, m, t = rng.choice(NAMES), rng.choice(WORDS), rng.choice(TYPES)
        a, b = rng.randint(1, 4096), rng.choice(WORDS)
        template = rng.choice([
            f'pub fn {n}(&mut self, {m}: &[u8], limit: usize) -> {t}<u32> {{',
            f'let {m} = self.{n}({b}, {a})?;',
            f'// {m} {b} {rng.choice(WORDS)}: keep the {rng.choice(WORDS)} within {a} bytes',
            f'if {m}.len() > {a} {{ return Err("{b} {m} too large"); }}',
            f'for ({b}, {m}) in {rng.choice(WORDS)}.iter().enumerate() {{',
            f'let mut {m}: {t} = {t}::new({a}, {rng.random():.3f});',
            f'match {m} {{ Some(v) => v, None => bail!("missing {b}") }}',
            f'self.{m} = {n}({b}).max({a}) as u64;',
            f'struct {t.title()}{a} {{ {m}: u32, {b}: Vec<u8> }}',
            f'let total = {m} * {a} + {b}.len() as u32;',
        ])
        if template.startswith(('pub fn', 'struct', 'for', 'match')) or template.endswith('{'):
            out.append('    ' * indent + template)
            indent = min(indent + 1, 4)
        else:
            out.append('    ' * indent + template)
            if rng.random() < 0.3 and indent:
                indent -= 1
                out.append('    ' * indent + '}')
    return out[:count]


def draw_code(canvas, x, y, line, font, palette):
    for token in TOKEN.findall(line):
        if token.isspace() or not token:
            x += font.width(token)
            continue
        if token.startswith('//'):
            color = palette['comment']
        elif token.startswith('"'):
            color = palette['string']
        elif token[0].isdigit():
            color = palette['number']
        elif token in KEYWORDS:
            color = palette['keyword']
        elif token in TYPES or token[0].isupper():
            color = palette['type']
        elif line[line.index(token) + len(token):].startswith('('):
            color = palette['function']
        else:
            color = palette['text']
        x += canvas.text(x, y, token, font, color)
    return x


def scene_editor(width, height):
    rng = random.Random('editor')
    base = Canvas(width, height, '#1e1e1e')
    top, bottom, side = 36, height - 24, 300
    base.rect(0, 0, width, top, '#252526')
    ui = Font.get('segoeui.ttf', 13)
    x = side
    for index, name in enumerate(['encoder.rs', 'session.rs', 'config.toml', 'README.md']):
        w = int(ui.width(name)) + 54
        base.rect(x, 0, x + w, top, '#1e1e1e' if index == 0 else '#2d2d2d')
        if index == 0:
            base.rect(x, 0, x + w, 2, '#007acc')
        base.rect(x + 12, 13, x + 22, 23, ['#dea584', '#dea584', '#cbcb41', '#519aba'][index])
        base.text(x + 30, 9, name, ui, '#ffffff' if index == 0 else '#969696')
        x += w + 1
    base.rect(0, top, 48, height, '#333333')
    for index in range(6):
        base.rect(14, top + 16 + index * 48, 34, top + 36 + index * 48, '#ffffff' if index == 0 else '#858585')
    base.rect(48, top, side, bottom, '#252526')
    base.text(64, top + 8, 'EXPLORER', Font.get('segoeui.ttf', 11), '#bbbbbb')
    rows = [(0, 'rubylight', '#c5c5c5', None), (1, 'rust', '#c5c5c5', None), (2, 'core', '#c5c5c5', None),
            (3, 'config.rs', '#e2c08d', '#dea584'), (3, 'rtsp.rs', '#c5c5c5', '#dea584'),
            (3, 'packed444.rs', '#73c991', '#dea584'), (3, 'lib.rs', '#c5c5c5', '#dea584'), (2, 'windows', '#c5c5c5', None),
            (3, 'amf.rs', '#c5c5c5', '#dea584'), (3, 'encoder.rs', '#ffffff', '#dea584'), (3, 'gpu_color.rs', '#e2c08d', '#dea584'),
            (2, 'host', '#c5c5c5', None), (3, 'state.rs', '#c5c5c5', '#dea584'), (1, 'docs', '#c5c5c5', None),
            (2, 'configuration.md', '#c5c5c5', '#519aba'), (2, 'architecture.md', '#c5c5c5', '#519aba'),
            (1, 'Cargo.toml', '#f14c4c', '#cbcb41'), (1, 'NOTICE', '#c5c5c5', '#a0a0a0')]
    for index, (level, name, color, icon) in enumerate(rows):
        y = top + 34 + index * 22
        if name == 'encoder.rs':
            base.rect(48, y - 2, side, y + 20, '#37373d')
        if icon:
            base.rect(66 + level * 12, y + 5, 76 + level * 12, y + 15, icon)
        else:
            base.text(62 + level * 12, y, '>', ui, '#c5c5c5')
        base.text(84 + level * 12, y, name, ui, color)
    base.rect(0, bottom, width, height, '#007acc')
    small = Font.get('segoeui.ttf', 12)
    base.text(12, bottom + 3, 'main*   0 errors  2 warnings', small, '#ffffff')
    base.text(width - 12, bottom + 3, 'Ln 41, Col 17   Spaces: 4   UTF-8   Rust', small, '#ffffff', 'r')
    # Scrolling layer: gutter and code.
    font = Font.get('consola.ttf', 16)
    line_height = 21
    count = (bottom - top + SCROLL * STEP) // line_height + 4
    layer = Canvas(width - side, count * line_height, '#1e1e1e')
    palette = dict(text='#d4d4d4', keyword='#569cd6', type='#4ec9b0', function='#dcdcaa', string='#ce9178',
                   number='#b5cea8', comment='#6a9955')
    lines = code_lines(rng, count)
    for index, line in enumerate(lines):
        y = index * line_height
        if index == 5:
            layer.rect(60, y, layer.width, y + line_height, '#282828')
            layer.box(60, y, layer.width, y + line_height, '#3a3a3a')
        if index in (9, 10):
            layer.rect(60 + 8 * 4 * 8, y, 60 + 8 * 4 * 8 + 190, y + line_height, '#264f78')
        layer.text(52, y + 1, str(index + 1), font, '#c6c6c6' if index == 5 else '#858585', 'r')
        depth = (len(line) - len(line.lstrip())) // 4
        for level in range(depth):
            layer.line(70 + level * 4 * font.width(' '), y, 70 + level * 4 * font.width(' '), y + line_height, '#404040')
        draw_code(layer, 70, y + 1, line, font, palette)
    return base, layer, (side, top)


def scene_browser(width, height):
    rng = random.Random('browser')
    base = Canvas(width, height, '#ffffff')
    ui = Font.get('segoeui.ttf', 14)
    base.rect(0, 0, width, 40, '#dee1e6')
    base.rect(10, 8, 250, 40, '#ffffff')
    base.rect(24, 17, 38, 31, '#1a73e8')
    base.text(48, 13, 'Streaming guide - Docs', ui, '#202124')
    base.rect(262, 12, 480, 36, '#c9ccd2')
    base.text(274, 13, 'Issue 607 - Tracker', ui, '#3c4043')
    base.rect(0, 40, width, 84, '#ffffff')
    base.rect(0, 83, width, 84, '#dadce0')
    for index in range(3):
        base.rect(16 + index * 40, 52, 36 + index * 40, 72, '#5f6368')
    base.rect(140, 50, width - 220, 76, '#f1f3f4')
    base.text(160, 53, 'https://docs.example.org/guide/streaming#chroma', ui, '#202124')
    base.rect(width - 190, 52, width - 170, 72, '#188038')
    base.rect(width - 150, 52, width - 130, 72, '#d93025')
    base.rect(width - 110, 52, width - 90, 72, '#f9ab00')
    top = 84
    page = Font.get('segoeui.ttf', 16)
    bold = Font.get('segoeuib.ttf', 16)
    h1 = Font.get('segoeuib.ttf', 32)
    h2 = Font.get('segoeuib.ttf', 22)
    code = Font.get('consola.ttf', 14)
    page_height = height - top + SCROLL * STEP + 40
    layer = Canvas(width, page_height, '#ffffff')
    layer.rect(0, 0, width, 64, '#0b57d0')
    layer.text(48, 14, 'Rubylight Docs', Font.get('segoeuib.ttf', 24), '#ffffff')
    for index, name in enumerate(['Guide', 'Reference', 'Releases', 'Community']):
        layer.text(width - 560 + index * 130, 20, name, page, '#ffffff')
    column = min(880, width - 460)
    left = 90
    y = 100
    layer.text(left, y, 'Choosing the chroma format', h1, '#202124')
    y += 58

    def sentence(words):
        return ' '.join(rng.choice(WORDS) for _ in range(words))

    def paragraph(y, link_chance=0.12):
        for _ in range(4):
            x = left
            words = sentence(rng.randint(12, 18)).split()
            for word in words:
                piece = word + ' '
                w = page.width(piece)
                if x + w > left + column:
                    break
                kind = rng.random()
                if kind < link_chance:
                    layer.text(x, y, word, page, '#1a0dab')
                    layer.line(int(x), y + 20, int(x + page.width(word)), y + 20, '#1a0dab')
                elif kind < link_chance + 0.06:
                    layer.rect(x - 2, y + 1, x + code.width(word) + 3, y + 23, '#f1f3f4')
                    layer.text(x, y + 2, word, code, '#c7254e')
                    w = code.width(word) + 6
                else:
                    layer.text(x, y, word, page, '#3c4043')
                x += w
            y += 26
        return y + 12

    while y < page_height - 300:
        layer.text(left, y, sentence(3).title(), h2, '#202124')
        y += 44
        y = paragraph(y)
        pick = rng.random()
        if pick < 0.25:
            layer.rect(left, y, left + column, y + 80, '#e8f0fe')
            layer.rect(left, y, left + 4, y + 80, '#1a73e8')
            layer.text(left + 20, y + 12, 'Note', bold, '#174ea6')
            layer.text(left + 20, y + 40, sentence(9), page, '#1f1f1f')
            y += 100
        elif pick < 0.5:
            layer.rect(left, y, left + column, y + 80, '#fce8e6')
            layer.rect(left, y, left + 4, y + 80, '#d93025')
            layer.text(left + 20, y + 12, 'Warning', bold, '#a50e0e')
            layer.text(left + 20, y + 40, sentence(9), page, '#1f1f1f')
            y += 100
        elif pick < 0.75:
            layer.rect(left, y, left + column, y + 6 * 24 + 16, '#202124')
            palette = dict(text='#e8eaed', keyword='#c58af9', type='#78d9ec', function='#fdd663', string='#81c995',
                           number='#f28b82', comment='#9aa0a6')
            for row, line in enumerate(code_lines(rng, 6)):
                draw_code(layer, left + 16, y + 8 + row * 24, line, code, palette)
            y += 6 * 24 + 36
        else:
            cw = column // 4
            for row in range(4):
                for col in range(4):
                    x0, y0 = left + col * cw, y + row * 30
                    if row == 0:
                        layer.rect(x0, y0, x0 + cw, y0 + 30, '#f1f3f4')
                    layer.box(x0, y0, x0 + cw + 1, y0 + 31, '#dadce0')
                    value = ['Mode', 'Bitrate', 'Chroma', 'Result'][col] if row == 0 else (
                        sentence(1) if col == 0 else f'{rng.randint(5, 900)} Mbps' if col == 1 else
                        rng.choice(['4:2:0', '4:4:4']) if col == 2 else rng.choice(['pass', 'fail', 'skip']))
                    color = {'pass': '#188038', 'fail': '#d93025', 'skip': '#b06000'}.get(value, '#202124')
                    layer.text(x0 + 10, y0 + 4, value, bold if row == 0 else page, color)
            y += 4 * 30 + 36
        layer.rect(left, y - 8, left + 150, y + 26, '#188038')
        layer.text(left + 75, y - 3, 'Download', bold, '#ffffff', 'm')
        y += 56
    # Right-hand table of contents.
    toc = left + column + 60
    layer.text(toc, 110, 'On this page', bold, '#202124')
    for index in range(8):
        active = index == 2
        if active:
            layer.rect(toc - 12, 146 + index * 32, toc - 9, 170 + index * 32, '#1a73e8')
        layer.text(toc, 148 + index * 32, sentence(2).title(), page, '#1a73e8' if active else '#5f6368')
    return base, layer, (0, top)


def scene_sheet(width, height):
    rng = random.Random('sheet')
    base = Canvas(width, height, '#ffffff')
    ui = Font.get('calibri.ttf', 15)
    small = Font.get('calibri.ttf', 13)
    base.rect(0, 0, width, 32, '#217346')
    base.text(width // 2, 5, 'Quarterly report.xlsx - Excel', Font.get('calibri.ttf', 15), '#ffffff', 'm')
    base.rect(0, 32, width, 112, '#f3f3f3')
    for index, name in enumerate(['File', 'Home', 'Insert', 'Page Layout', 'Formulas', 'Data', 'Review', 'View']):
        base.text(24 + index * 96, 38, name, ui, '#217346' if name == 'Home' else '#444444')
    base.rect(24 + 96, 62, 24 + 96 + 40, 64, '#217346')
    colors = ['#217346', '#2b579a', '#d83b01', '#ffb900', '#7719aa', '#008272', '#c239b3', '#ca5010']
    for index in range(12):
        x = 24 + index * 82
        base.rect(x, 70, x + 28, 98, colors[index % len(colors)])
        base.text(x + 14, 98, ['Paste', 'Cut', 'Bold', 'Fill', 'Borders', 'Merge', 'Sort', 'Filter', 'Chart', 'Pivot',
                              'Format', 'Find'][index], small, '#444444', 'm')
    base.rect(0, 112, width, 142, '#ffffff')
    base.box(8, 116, 100, 138, '#c6c6c6')
    base.text(14, 117, 'C7', ui, '#222222')
    base.text(130, 117, 'fx', Font.get('calibri.ttf', 15), '#217346')
    base.text(170, 117, '=SUMME(C2:C6)*1,19', ui, '#222222')
    columns = ['A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L']
    row_header, col_width, row_height, grid_top = 52, 128, 24, 142
    bottom = height - 54
    base.rect(0, grid_top, width, grid_top + row_height, '#e6e6e6')
    for index, name in enumerate(columns):
        x = row_header + index * col_width
        base.text(x + col_width / 2, grid_top + 2, name, ui, '#444444', 'm')
        base.line(x, grid_top, x, grid_top + row_height, '#ababab')
    base.line(0, grid_top + row_height, width, grid_top + row_height, '#ababab')
    base.rect(0, bottom, width, height, '#f3f3f3')
    for index, name in enumerate(['Sheet1', 'Costs', 'Chart data', 'Notes']):
        base.rect(12 + index * 110, bottom + 4, 12 + index * 110 + 100, bottom + 28, '#ffffff' if index == 0 else '#e1e1e1')
        base.text(62 + index * 110, bottom + 6, name, ui, '#217346' if index == 0 else '#444444', 'm')
    base.rect(0, height - 22, width, height, '#217346')
    base.text(14, height - 21, 'Ready', small, '#ffffff')
    base.text(width - 14, height - 21, 'Average: 4.071   Count: 18   Sum: 73.280', small, '#ffffff', 'r')
    top = grid_top + row_height
    visible = bottom - top + SCROLL * STEP
    rows = visible // row_height + 3
    layer = Canvas(width, rows * row_height, '#ffffff')
    heads = ['Product', 'Region', 'Units', 'Revenue', 'Margin', 'Share', 'Status', 'Owner', 'Forecast', 'Delta']
    names = ['Alpha', 'Bravo', 'Cirrus', 'Delta', 'Ember', 'Falcon', 'Garnet', 'Harbor', 'Indigo', 'Juniper']
    regions = ['North', 'South', 'East', 'West', 'Central']
    statuses = {'OK': ('#c6efce', '#006100'), 'Late': ('#ffeb9c', '#9c5700'), 'Failed': ('#ffc7ce', '#9c0006')}
    for r in range(rows):
        y = r * row_height
        layer.rect(0, y, row_header, y + row_height, '#e6e6e6')
        layer.text(row_header / 2, y + 2, str(r + 1), ui, '#444444', 'm')
        for c in range(10):
            x = row_header + c * col_width
            if r == 0:
                layer.rect(x, y, x + col_width, y + row_height, '#1f4e79')
                layer.text(x + 8, y + 2, heads[c], Font.get('calibrib.ttf', 15), '#ffffff')
                continue
            if r % 2 == 0:
                layer.rect(x, y, x + col_width, y + row_height, '#f2f2f2')
            if c == 0:
                layer.text(x + 8, y + 2, f'{rng.choice(names)} {rng.randint(10, 99)}', ui, '#222222')
            elif c == 1:
                layer.text(x + 8, y + 2, rng.choice(regions), ui, '#222222')
            elif c == 6:
                label = rng.choice(list(statuses))
                fill, ink = statuses[label]
                layer.rect(x + 1, y + 1, x + col_width, y + row_height, fill)
                layer.text(x + 8, y + 2, label, ui, ink)
            elif c == 5:
                share = rng.randint(3, 99)
                layer.rect(x + 3, y + 5, x + 3 + (col_width - 6) * share // 100, y + row_height - 5, '#5b9bd5')
                layer.text(x + col_width - 8, y + 2, f'{share} %', ui, '#222222', 'r')
            elif c == 7:
                layer.text(x + 8, y + 2, rng.choice(['m.kara', 'j.beck', 'a.lehr', 't.sommer']), ui, '#0563c1')
            elif c == 9:
                delta = rng.uniform(-9, 9)
                layer.text(x + col_width - 8, y + 2, f'{delta:+.1f}', ui, '#c00000' if delta < 0 else '#00803c', 'r')
            else:
                layer.text(x + col_width - 8, y + 2, f'{rng.uniform(1, 9999):,.2f}'.replace(',', ' '), ui, '#222222', 'r')
    for c in range(11):
        layer.line(row_header + c * col_width, 0, row_header + c * col_width, layer.height, '#d4d4d4')
    for r in range(rows + 1):
        layer.line(0, r * row_height, width, r * row_height, '#d4d4d4')
    layer.box(row_header + 2 * col_width, 6 * row_height, row_header + 5 * col_width, 12 * row_height + 1, '#217346', 2)
    layer.rect(row_header + 2 * col_width + 2, 6 * row_height + 2, row_header + 5 * col_width - 2, 6 * row_height + 3, '#217346')
    return base, layer, (0, top)


SCENES = dict(editor=scene_editor, browser=scene_browser, sheet=scene_sheet)
TITLE, HUD = 30, 48
# Pixels a frame the clip window's and the game view's pictures pan by.
CLIP_PAN, GAME_PAN = (3, 1), (5, 2)


def even(x, y, w, h):
    """A crop rectangle on even coordinates and sizes, as a 4:2:0 picture needs."""
    return [int(x) & ~1, int(y) & ~1, int(w) & ~1, int(h) & ~1]


def mixed_layout(width, height):
    """The mixed scene's pictures (x, y, w, h): a clip window at the top right of the editor's code and
    a game view at the bottom left, its HUD bar along the bottom edge. Title bars sit above both."""
    side, top = LAYER_ORIGIN['editor']
    bottom = height - LAYER_BOTTOM['editor']
    cw = min(640, width // 3) & ~1
    clip = even(width - 60 - cw, top + 40 + TITLE, cw, cw * 9 // 16)
    gw = width * 3 // 10 & ~1
    gh = gw * 9 // 16 & ~1
    game = even(side + 40, bottom - 40 - gh, gw, gh)
    if game[1] - TITLE - 20 <= clip[1] + clip[3] or clip[0] - 20 <= game[0]:
        raise ValueError(f'{width}x{height} is too small for the mixed scene')
    return dict(clip=clip, game=game, hud=even(game[0], game[1] + gh - HUD, gw, HUD))


def regions(kind, width, height):
    """The still and moving rectangles of a scene, and the HUD bar of `mixed`; the scores crop to them."""
    if kind == 'mixed':
        layout = mixed_layout(width, height)
        top, bottom = LAYER_ORIGIN['editor'][1], height - LAYER_BOTTOM['editor']
        (cx, _, _, _), (gx, gy, gw, gh) = layout['clip'], layout['game']
        return dict(still=[even(0, top, gx - 20, bottom - top), even(gx, top, cx - 20 - gx, gy - TITLE - 20 - top)],
                    moving=[layout['clip'], even(gx, gy, gw, gh - HUD)], hud=[layout['hud']])
    x, y = LAYER_ORIGIN[kind]
    bottom = height - LAYER_BOTTOM[kind]
    still = [even(0, y, x, bottom - y)] if x else [even(0, 0, width, y)]
    if bottom < height:
        still.append(even(0, bottom, width, height - bottom))
    return dict(still=still, moving=[even(x, y, width - x, bottom - y)])


def texture(seed, width, height):
    """Soft colour areas, finer light and shade and grain: a stand-in for game footage the camera pans across."""
    rng = np.random.default_rng(seed)

    def noise(step, channels):
        small = rng.integers(0, 256, (height // step + 2, width // step + 2, channels), dtype=np.uint8)
        image = Image.fromarray(small[..., 0] if channels == 1 else small, 'L' if channels == 1 else 'RGB')
        return np.asarray(image.resize((width, height), Image.BICUBIC), np.float32).reshape(height, width, channels)

    colour = noise(160, 3)
    colour = colour.mean(axis=2, keepdims=True) * .6 + colour * .4
    picture = .5 * colour + .5 * noise(12, 1) + rng.normal(0, 6, (height, width, 1)).astype(np.float32)
    return np.clip(picture, 0, 255)


def bounce(position, span):
    """`position` folded into 0..span and back, as a ball bouncing between two walls."""
    position %= 2 * span
    return position if position < span else 2 * span - position


def mixed_frames(width, height, count):
    """Yield (index, PNG bytes) of the mixed scene; the clip and the game view move in every frame."""
    layout = mixed_layout(width, height)
    base, layer, (x, y) = scene_editor(width, height)
    desk = Canvas(width, height, '#000000')
    desk.pixels[:] = base.pixels
    visible = height - LAYER_BOTTOM['editor'] - y
    desk.pixels[y:y + visible, x:] = layer.pixels[:visible, :width - x]
    ui = Font.get('segoeui.ttf', 13)
    for name, title in (('clip', 'trailer.mp4 - Media Player'), ('game', 'Game')):
        cx, cy, cw, ch = layout[name]
        desk.rect(cx - 1, cy - TITLE, cx + cw + 1, cy + ch + 1, '#3c3c3c')
        desk.rect(cx, cy - TITLE + 1, cx + cw, cy, '#2b2b2b')
        desk.text(cx + 10, cy - TITLE + 6, title, ui, '#ffffff')
    hx, hy, hw, hh = layout['hud']
    hud = Canvas(hw, hh, '#101820')
    label = Font.get('segoeuib.ttf', 16)
    hud.text(12, 12, 'HP', label, '#ff5a5a')
    hud.rect(44, 18, 204, 30, '#3a1010')
    hud.rect(44, 18, 168, 30, '#e03c3c')
    hud.text(hw // 2, 12, 'SCORE 004250', label, '#ffffff', 'm')
    hud.text(hw - 12, 12, 'AMMO 24 / 120', label, '#ffd24a', 'r')
    pictures = []
    for seed, (name, (px, py)) in enumerate((('clip', CLIP_PAN), ('game', GAME_PAN)), 1):
        cx, cy, cw, ch = layout[name]
        ch -= HUD if name == 'game' else 0
        pictures.append((cx, cy, cw, ch, px, py, texture(seed, cw + px * count, ch + py * count)))
    gx, gy, gw, gh = layout['game']
    radius = max(gh // 12, 8)
    rows, columns = np.ogrid[:gh - HUD, :gw]
    for index in range(count):
        frame = Canvas(width, height, '#000000')
        frame.pixels[:] = desk.pixels
        for cx, cy, cw, ch, px, py, picture in pictures:
            frame.pixels[cy:cy + ch, cx:cx + cw] = picture[py * index:py * index + ch, px * index:px * index + cw]
        # A bright ball bouncing through the game view, over its panning background.
        bx = radius + bounce(index * 9, gw - 2 * radius)
        by = radius + bounce(index * 5, gh - HUD - 2 * radius)
        view = frame.pixels[gy:gy + gh - HUD, gx:gx + gw]
        view[(columns - bx) ** 2 + (rows - by) ** 2 <= radius * radius] = hex_color('#ffc828')
        frame.pixels[hy:hy + hh, hx:hx + hw] = hud.pixels
        buffer = io.BytesIO()
        frame.image(0, height).save(buffer, 'PNG', compress_level=3)
        yield index, buffer.getvalue()


def frames(kind, width, height, count=HOLD + SCROLL + HOLD):
    """Yield (index, PNG bytes of the frame); identical neighbouring frames share their bytes."""
    if kind == 'mixed':
        yield from mixed_frames(width, height, count)
        return
    base, layer, (x, y) = SCENES[kind](width, height)
    if (x, y) != LAYER_ORIGIN[kind]:
        raise ValueError(f'{kind}: the scrolling layer starts at {(x, y)}; update LAYER_ORIGIN and the regions')
    bottom = height - LAYER_BOTTOM[kind]
    visible = bottom - y
    cache = {}
    for index in range(count):
        offset = scroll_offset(index)
        if offset not in cache:
            frame = Canvas(width, height, '#000000')
            frame.pixels[:] = base.pixels
            part = layer.pixels[offset:offset + visible]
            frame.pixels[y:y + visible, x:x + part.shape[1]] = part[:, :width - x]
            buffer = io.BytesIO()
            frame.image(0, height).save(buffer, 'PNG', compress_level=3)
            cache[offset] = buffer.getvalue()
        yield index, cache[offset]


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    parser.add_argument('out', type=pathlib.Path)
    parser.add_argument('--size', default='1920x1080')
    parser.add_argument('--kind', default='all', choices=KINDS + ('mixed', 'all'))
    parser.add_argument('--frames', type=int, default=HOLD + SCROLL + HOLD)
    args = parser.parse_args(argv)
    width, height = (int(v) for v in args.size.split('x'))
    for kind in KINDS if args.kind == 'all' else (args.kind,):
        folder = args.out / f'{kind}-{width}x{height}'
        folder.mkdir(parents=True, exist_ok=True)
        for index, data in frames(kind, width, height, args.frames):
            (folder / f'{index:03d}.png').write_bytes(data)
        (folder / 'regions.json').write_text(json.dumps(dict(
            kind=kind, width=width, height=height, frames=args.frames, regions=regions(kind, width, height)), indent=2))
        print(folder)
    return 0


if __name__ == '__main__':
    sys.exit(main())
