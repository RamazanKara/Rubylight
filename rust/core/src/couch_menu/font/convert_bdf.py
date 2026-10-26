#!/usr/bin/env python3
"""Converts a Spleen BDF font into the couch menu's 1-bpp atlas.

A dev tool, not part of the build: the outputs are committed next to it and
loaded with include_bytes!. Rerun it only to change the font:

    python convert_bdf.py spleen-8x16.bdf spleen-8x16.bin
    python convert_bdf.py spleen-16x32.bdf spleen-16x32.bin

Output: the header b"RLF1", cell width (u8), cell height (u8), glyph count
(u16 LE), then the glyphs of printable Latin-1 in code point order, U+0020 to
U+007E and U+00A0 to U+00FF (C0, DEL and C1 are control codes, no glyphs).
Each glyph is `height` rows of ceil(width / 8) bytes, most significant bit
leftmost. The same input always gives the same bytes.

Fails, writing nothing, when the font is not a fixed cell or any glyph is
missing: then use Terminus (OFL) instead, as docs/couch-menu-design.md says.
"""
import hashlib
import struct
import sys

CODE_POINTS = list(range(0x20, 0x7F)) + list(range(0xA0, 0x100))


def fail(message):
    sys.exit(f"convert_bdf: {message}")


def parse(text):
    """Returns (width, height, ascent, x offset, {code point: rows of ints})."""
    lines = iter(text.splitlines())
    bbox = None
    glyphs = {}
    for line in lines:
        words = line.split()
        if not words:
            continue
        if words[0] == "FONTBOUNDINGBOX":
            w, h, x, y = map(int, words[1:5])
            bbox = (w, h, x, y)
        elif words[0] == "STARTCHAR":
            code = None
            gbox = None
            for line in lines:
                words = line.split() or [""]
                if words[0] == "ENCODING":
                    code = int(words[1])
                elif words[0] == "BBX":
                    gbox = tuple(map(int, words[1:5]))
                elif words[0] == "BITMAP":
                    if bbox is None or gbox is None:
                        fail(f"glyph {code} has no bounding box")
                    rows = [int(next(lines), 16) for _ in range(gbox[1])]
                    if next(lines).strip() != "ENDCHAR":
                        fail(f"glyph {code}: bitmap longer than its BBX")
                    glyphs[code] = (gbox, rows)
                    break
    if bbox is None:
        fail("no FONTBOUNDINGBOX")
    return bbox, glyphs


def convert(text):
    (width, height, x0, y0), glyphs = parse(text)
    stride = (width + 7) // 8
    missing = [f"U+{c:04X}" for c in CODE_POINTS if c not in glyphs]
    if missing:
        fail(f"missing {len(missing)} Latin-1 glyphs: {' '.join(missing)}")
    out = bytearray(b"RLF1" + struct.pack("<BBH", width, height, len(CODE_POINTS)))
    for code in CODE_POINTS:
        (w, h, x, y), rows = glyphs[code]
        # BDF rows are padded to whole bytes, glyph pixels leftmost.
        pad = (w + 7) // 8 * 8
        top = (height + y0) - (y + h)
        left = x - x0
        if top < 0 or left < 0 or top + h > height or left + w > width:
            fail(f"U+{code:04X} does not fit the {width}x{height} cell")
        cell = [0] * height
        for i, row in enumerate(rows):
            cell[top + i] = (row >> (pad - w)) << (stride * 8 - w - left)
        for row in cell:
            out += row.to_bytes(stride, "big")
    return width, height, bytes(out)


def main():
    if len(sys.argv) != 3:
        fail("usage: convert_bdf.py <font.bdf> <out.bin>")
    source = open(sys.argv[1], "rb").read()
    width, height, data = convert(source.decode("latin-1"))
    open(sys.argv[2], "wb").write(data)
    print(f"{sys.argv[1]}: {width}x{height}, {len(CODE_POINTS)} glyphs, {len(data)} bytes")
    print(f"source sha256 {hashlib.sha256(source).hexdigest()}")
    print(f"output sha256 {hashlib.sha256(data).hexdigest()}")


if __name__ == "__main__":
    main()
