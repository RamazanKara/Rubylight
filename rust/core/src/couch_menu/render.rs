//! Draws a page into the 1× panel the encoder composites (section 6.5): crisp
//! 1-bit text on an opaque panel with a 2-pixel border, RGBA8 sRGB, alpha
//! 255 everywhere.
use super::Frame;
use super::layout::{COLUMNS, Layout};
use super::model::{Menu, Snapshot, Style};

pub const PANEL: [u8; 3] = [0x17, 0x17, 0x1c];
pub const TEXT: [u8; 3] = [0xf0, 0xf0, 0xf0];
pub const DISABLED: [u8; 3] = [0x8a, 0x8a, 0x93];
/// Ruby: the focus bar and the border.
pub const FOCUS: [u8; 3] = [0xe0, 0x11, 0x5f];
pub const ERROR: [u8; 3] = [0xff, 0x5a, 0x5a];
/// Border width at 1×.
pub const BORDER: u32 = 2;

struct Canvas {
    width: u32,
    rgba: Vec<u8>,
}

impl Canvas {
    fn fill(&mut self, [x, y, w, h]: [u32; 4], colour: [u8; 3]) {
        for row in y..y + h {
            let start = ((row * self.width + x) * 4) as usize;
            for pixel in self.rgba[start..start + (w * 4) as usize]
                .as_chunks_mut::<4>()
                .0
            {
                pixel.copy_from_slice(&[colour[0], colour[1], colour[2], 0xff]);
            }
        }
    }
}

/// Draws `menu`'s current page for `snap` at `layout`'s font, centred in the
/// target.
pub fn render(menu: &Menu, snap: &Snapshot, layout: &Layout) -> Frame {
    let lines = menu.lines(snap);
    let count = lines.len() as u32;
    let rect = layout.panel(count);
    let [width, height] = layout.size_1x(count);
    let font = layout.font.font();
    let [pad_x, pad_y] = layout.padding();
    let mut canvas = Canvas {
        width,
        rgba: vec![0; (width * height * 4) as usize],
    };
    canvas.fill([0, 0, width, height], FOCUS);
    canvas.fill(
        [BORDER, BORDER, width - 2 * BORDER, height - 2 * BORDER],
        PANEL,
    );
    for (index, line) in lines.iter().enumerate() {
        let y = pad_y + index as u32 * font.height;
        let ink = match line.style {
            Style::Title | Style::Message => TEXT,
            Style::Row { focus, locked } => {
                if focus && !locked {
                    canvas.fill([pad_x, y, COLUMNS * font.width, font.height], FOCUS);
                } else if focus {
                    // A locked row cannot take the bar: a ruby mark in the
                    // left padding shows the focus, the text stays grey.
                    canvas.fill([pad_x * 3 / 8, y, pad_x / 2, font.height], FOCUS);
                }
                if locked { DISABLED } else { TEXT }
            }
            Style::Info | Style::Note | Style::Hint => DISABLED,
            Style::Error => ERROR,
        };
        for (column, c) in line.text.chars().take(COLUMNS as usize).enumerate() {
            let x = pad_x + column as u32 * font.width;
            for gy in 0..font.height {
                for gx in 0..font.width {
                    if font.pixel(c, gx, gy) {
                        canvas.fill([x + gx, y + gy, 1, 1], ink);
                    }
                }
            }
        }
    }
    Frame {
        origin: rect.origin,
        size: rect.size,
        scale: layout.scale,
        rgba: canvas.rgba.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::couch_menu::Nav;
    use crate::couch_menu::model::tests::{snapshot, strings};

    fn fnv1a(frame: &Frame) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        let header = [frame.origin, frame.size, [frame.scale, 0]];
        for byte in header
            .iter()
            .flatten()
            .flat_map(|v| v.to_le_bytes())
            .chain(frame.rgba.iter().copied())
        {
            hash = (hash ^ byte as u64).wrapping_mul(0x0100_0000_01b3);
        }
        hash
    }

    /// The main page and the Radeon page, as the tests' snapshot shows them.
    fn pages(layout: &Layout) -> [Frame; 2] {
        let snap = snapshot();
        let mut menu = Menu::new(strings(false));
        let now = Instant::now();
        menu.nav(Nav::Opened, now, &snap, layout);
        let main = render(&menu, &snap, layout);
        menu.nav(Nav::Down, now, &snap, layout);
        menu.nav(Nav::Accept, now, &snap, layout);
        menu.nav(Nav::Down, now, &snap, layout);
        [main, render(&menu, &snap, layout)]
    }

    fn colour(frame: &Frame, x: u32, y: u32) -> [u8; 4] {
        let width = frame.size[0] / frame.scale;
        let at = ((y * width + x) * 4) as usize;
        frame.rgba[at..at + 4].try_into().unwrap()
    }

    #[test]
    fn couch_menu_render_golden_hash_per_page() {
        // Changing a page's look changes these: check the dump by eye, then
        // update them.
        let golden = [
            ([1280, 720], [0x4e89_4713_575d_9a2a, 0x639e_a070_ad48_f842]),
            ([3840, 2160], [0x4770_6db7_c2a9_5a47, 0x35a5_677f_abbf_1a2b]),
        ];
        let mut got = Vec::new();
        for (target, _) in golden {
            let layout = Layout::for_stream(target[0], target[1]).unwrap();
            got.push((target, pages(&layout).map(|page| fnv1a(&page))));
        }
        let want: Vec<_> = golden.to_vec();
        assert_eq!(got, want, "golden hashes");
    }

    #[test]
    fn couch_menu_render_is_opaque_bordered_and_sized_for_the_layout() {
        for [w, h] in [
            [640, 360],
            [1280, 720],
            [1920, 1080],
            [3840, 2160],
            [1080, 2400],
        ] {
            let layout = Layout::for_stream(w, h).unwrap();
            for frame in pages(&layout) {
                let [fw, fh] = frame.size.map(|v| v / frame.scale);
                assert_eq!(frame.size, [fw, fh].map(|v| v * layout.scale));
                assert_eq!(frame.rgba.len(), (fw * fh * 4) as usize);
                assert!(
                    frame.rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 0xff),
                    "alpha 255"
                );
                assert!(frame.origin.iter().chain(&frame.size).all(|v| v % 16 == 0));
                for (x, y) in [(0, 0), (1, 1), (fw - 1, fh - 1), (fw - 2, 7), (5, fh - 2)] {
                    assert_eq!(colour(&frame, x, y)[..3], FOCUS, "border at {x},{y}");
                }
                assert_eq!(colour(&frame, 2, 2)[..3], PANEL);
                let used: std::collections::HashSet<_> = frame
                    .rgba
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|p| [p[0], p[1], p[2]])
                    .collect();
                assert!(
                    used.iter()
                        .all(|c| [PANEL, TEXT, DISABLED, FOCUS, ERROR].contains(c))
                );
                assert!(used.contains(&TEXT) && used.contains(&DISABLED));
            }
        }
    }

    /// Writes the golden pages as PPM files into the temp directory, for a
    /// look by eye: `cargo test -p butterpollo-core couch_menu_render_dump -- --ignored`.
    #[test]
    #[ignore]
    fn couch_menu_render_dump() {
        let layout = Layout::for_stream(1280, 720).unwrap();
        for (name, frame) in ["main", "radeon"].into_iter().zip(pages(&layout)) {
            let [w, h] = frame.size.map(|v| v / frame.scale);
            let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
            ppm.extend(
                frame
                    .rgba
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .flat_map(|p| [p[0], p[1], p[2]]),
            );
            let path = std::env::temp_dir().join(format!("couch-menu-{name}.ppm"));
            std::fs::write(&path, ppm).unwrap();
            println!("{}", path.display());
        }
    }
}
