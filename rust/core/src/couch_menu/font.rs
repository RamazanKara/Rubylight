//! The menu's bitmap fonts: Spleen 2.2.0 (BSD-2-Clause, `font/LICENSE-spleen.txt`)
//! at 8×16 and 16×32, printable Latin-1 only. `font/convert_bdf.py` made the
//! atlases from the upstream BDF files and documents their format.

/// A fixed-cell 1-bpp font.
#[derive(Debug)]
pub struct Font {
    pub width: u32,
    pub height: u32,
    stride: usize,
    glyphs: &'static [u8],
}

/// Glyphs in an atlas: U+0020 to U+007E, then U+00A0 to U+00FF.
const GLYPHS: usize = 95 + 96;

const fn load(bytes: &'static [u8]) -> Font {
    let (header, glyphs) = bytes.split_at(8);
    assert!(header[0] == b'R' && header[1] == b'L' && header[2] == b'F' && header[3] == b'1');
    let (width, height) = (header[4] as u32, header[5] as u32);
    assert!(u16::from_le_bytes([header[6], header[7]]) as usize == GLYPHS);
    let stride = (width as usize).div_ceil(8);
    assert!(glyphs.len() == GLYPHS * height as usize * stride);
    Font {
        width,
        height,
        stride,
        glyphs,
    }
}

pub static SMALL: Font = load(include_bytes!("font/spleen-8x16.bin"));
pub static LARGE: Font = load(include_bytes!("font/spleen-16x32.bin"));

impl Font {
    /// Whether the atlas has a glyph for `c` (printable Latin-1).
    pub fn has(c: char) -> bool {
        index(c).is_some()
    }
    /// Whether pixel (`x`, `y`) of `c`'s cell is set. Characters outside the
    /// atlas draw as `?`: the catalogue is checked to be Latin-1, so only
    /// text from outside it (setting labels, readings) can reach that.
    pub fn pixel(&self, c: char, x: u32, y: u32) -> bool {
        let glyph = index(c).unwrap_or(b'?' as usize - 0x20);
        let row = (glyph * self.height as usize + y as usize) * self.stride;
        let byte = self.glyphs[row + x as usize / 8];
        byte & (0x80 >> (x % 8)) != 0
    }
}

fn index(c: char) -> Option<usize> {
    match c as u32 {
        code @ 0x20..=0x7e => Some(code as usize - 0x20),
        code @ 0xa0..=0xff => Some(code as usize - 0xa0 + 95),
        _ => None,
    }
}

/// The first character of `text` the atlases cannot draw.
pub fn uncovered(text: &str) -> Option<char> {
    text.chars().find(|c| !Font::has(*c))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn art(font: &Font, c: char) -> Vec<String> {
        (0..font.height)
            .map(|y| {
                (0..font.width)
                    .map(|x| if font.pixel(c, x, y) { '#' } else { '.' })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn couch_menu_font_atlases_load_with_their_cells() {
        assert_eq!((SMALL.width, SMALL.height), (8, 16));
        assert_eq!((LARGE.width, LARGE.height), (16, 32));
        // Spleen's 8×16 "A", as the BDF draws it.
        let a = art(&SMALL, 'A');
        assert_eq!(
            &a[2..12],
            [
                ".#####..", "##...##.", "##...##.", "##...##.", "#######.", "##...##.", "##...##.",
                "##...##.", "##...##.", "##...##."
            ]
        );
        assert!(a[..2].iter().chain(&a[12..]).all(|row| row == "........"));
        for font in [&SMALL, &LARGE] {
            assert!(art(font, ' ').iter().all(|row| !row.contains('#')));
            for c in ['ä', 'ö', 'ü', 'ß', 'Ä', 'Ö', 'Ü', '°', 'é'] {
                assert!(art(font, c).iter().any(|row| row.contains('#')), "{c}");
            }
        }
    }

    #[test]
    fn couch_menu_font_covers_printable_latin1_and_nothing_else() {
        for code in (0x20..0x7f).chain(0xa0..0x100) {
            assert!(Font::has(char::from_u32(code).unwrap()), "U+{code:04X}");
        }
        for c in ['\n', '\u{7f}', '\u{85}', '\u{9f}', '…', '▸', '€', '™'] {
            assert!(!Font::has(c), "{c:?}");
        }
        assert_eq!(uncovered("Host-Menü 72 °C 95%"), None);
        assert_eq!(uncovered("Loading…"), Some('…'));
        assert_eq!(
            art(&SMALL, '…'),
            art(&SMALL, '?'),
            "outside the atlas draws ?"
        );
    }
}
