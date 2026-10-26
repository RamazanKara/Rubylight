//! Where the panel goes and how large it is (`docs/couch-menu-design.md`
//! section 6.4). The font and whole-number scale follow the stream's shorter
//! side; the panel is centred, 34 columns wide, and its origin and size are
//! multiples of 16 target pixels, the strictest block size of every format.
use std::fmt;

use super::font::{self, Font};

/// Text columns in a line.
pub const COLUMNS: u32 = 34;
/// The most lines a page shows: title, info line, 12 rows, message and hint.
pub const MAX_LINES: u32 = 16;
/// Target-pixel alignment of the panel's origin and size.
pub const ALIGN: u32 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontSize {
    /// Spleen 8×16.
    Small,
    /// Spleen 16×32.
    Large,
}

impl FontSize {
    pub fn font(self) -> &'static Font {
        match self {
            Self::Small => &font::SMALL,
            Self::Large => &font::LARGE,
        }
    }
}

/// The stream cannot fit the panel even at 8×16 ×1: opening is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooSmall {
    pub width: u32,
    pub height: u32,
}

impl fmt::Display for TooSmall {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "couch menu: stream too small ({}x{})",
            self.width, self.height
        )
    }
}

impl std::error::Error for TooSmall {}

/// A panel rectangle in target pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub origin: [u32; 2],
    pub size: [u32; 2],
}

/// Where a stream position falls on the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Outside,
    /// On the panel, on this text line.
    Line(u32),
    /// On the panel's padding.
    Panel,
}

/// The menu's geometry for one encoder target size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub target: [u32; 2],
    pub font: FontSize,
    pub scale: u32,
}

impl Layout {
    /// The font and scale for a `width`×`height` target: the table's choice,
    /// one scale step smaller while the largest page does not fit.
    pub fn for_stream(width: u32, height: u32) -> Result<Self, TooSmall> {
        let (font, scale) = match width.min(height) {
            ..720 => (FontSize::Small, 1),
            720..1440 => (FontSize::Large, 1),
            1440..2160 => (FontSize::Large, 2),
            _ => (FontSize::Large, 3),
        };
        let mut layout = Self {
            target: [width, height],
            font,
            scale,
        };
        loop {
            let [w, h] = layout.size(MAX_LINES);
            if w <= width && h <= height {
                return Ok(layout);
            }
            (layout.font, layout.scale) = match (layout.font, layout.scale) {
                (FontSize::Large, 1) => (FontSize::Small, 1),
                (FontSize::Large, scale) => (FontSize::Large, scale - 1),
                (FontSize::Small, _) => return Err(TooSmall { width, height }),
            };
        }
    }
    /// Text cell at 1×.
    pub fn cell(&self) -> [u32; 2] {
        let font = self.font.font();
        [font.width, font.height]
    }
    /// Panel padding at 1×: one column on each side, half a line above and
    /// below.
    pub fn padding(&self) -> [u32; 2] {
        let [w, h] = self.cell();
        [w, h / 2]
    }
    /// The panel's size at 1× for `lines` text lines. With 8- or 16-pixel
    /// cells both sides are multiples of 16 at every scale.
    pub fn size_1x(&self, lines: u32) -> [u32; 2] {
        let [w, h] = self.cell();
        [(COLUMNS + 2) * w, (lines + 1) * h]
    }
    /// The panel's size in target pixels.
    pub fn size(&self, lines: u32) -> [u32; 2] {
        self.size_1x(lines).map(|v| v * self.scale)
    }
    /// The centred panel for `lines` text lines, origin rounded down to 16.
    pub fn panel(&self, lines: u32) -> Rect {
        let size = self.size(lines);
        let origin = [0, 1].map(|i| (self.target[i].saturating_sub(size[i]) / 2) / ALIGN * ALIGN);
        Rect { origin, size }
    }
    /// What a stream position (`0..=1` on each axis) hits on a panel of
    /// `lines` lines.
    pub fn hit(&self, lines: u32, x: f32, y: f32) -> Hit {
        let rect = self.panel(lines);
        let px = x * self.target[0] as f32 - rect.origin[0] as f32;
        let py = y * self.target[1] as f32 - rect.origin[1] as f32;
        if !(0.0..rect.size[0] as f32).contains(&px) || !(0.0..rect.size[1] as f32).contains(&py) {
            return Hit::Outside;
        }
        let [pad_x, pad_y] = self.padding().map(|v| (v * self.scale) as f32);
        let [cell_w, cell_h] = self.cell().map(|v| (v * self.scale) as f32);
        let line = ((py - pad_y) / cell_h).floor();
        if px < pad_x || px >= pad_x + COLUMNS as f32 * cell_w || line < 0. || line >= lines as f32
        {
            return Hit::Panel;
        }
        Hit::Line(line as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn couch_menu_layout_table() {
        let rows = [
            // (target, font, scale, largest panel)
            ([640, 360], FontSize::Small, 1, [288, 272]),
            ([1280, 720], FontSize::Large, 1, [576, 544]),
            ([1920, 1080], FontSize::Large, 1, [576, 544]),
            ([2560, 1440], FontSize::Large, 2, [1152, 1088]),
            ([3840, 2160], FontSize::Large, 3, [1728, 1632]),
            ([1080, 2400], FontSize::Large, 1, [576, 544]),
            ([854, 480], FontSize::Small, 1, [288, 272]),
        ];
        for (target, font, scale, size) in rows {
            let layout = Layout::for_stream(target[0], target[1]).unwrap();
            assert_eq!((layout.font, layout.scale), (font, scale), "{target:?}");
            assert_eq!(layout.size(MAX_LINES), size, "{target:?}");
        }
        // Below 288×272 even 8×16 ×1 does not fit.
        for [w, h] in [[320, 240], [287, 400], [400, 271]] {
            let error = Layout::for_stream(w, h).unwrap_err();
            assert_eq!(
                error.to_string(),
                format!("couch menu: stream too small ({w}x{h})")
            );
        }
        assert!(Layout::for_stream(288, 272).is_ok());
    }

    #[test]
    fn couch_menu_layout_is_centred_and_aligned_to_16() {
        for [w, h] in [
            [640, 360],
            [1280, 720],
            [1366, 768],
            [1920, 1080],
            [2560, 1440],
            [3440, 1440],
            [3840, 2160],
            [1080, 2400],
            [1170, 2532],
        ] {
            let layout = Layout::for_stream(w, h).unwrap();
            for lines in 1..=MAX_LINES {
                let rect = layout.panel(lines);
                for i in 0..2 {
                    assert_eq!(rect.origin[i] % ALIGN, 0, "{w}x{h} {lines}");
                    assert_eq!(rect.size[i] % ALIGN, 0, "{w}x{h} {lines}");
                    assert!(rect.origin[i] + rect.size[i] <= layout.target[i], "{w}x{h}");
                    let centre = (layout.target[i] - rect.size[i]) / 2;
                    assert!(centre - rect.origin[i] < ALIGN, "{w}x{h} {lines}");
                }
                assert_eq!(rect.size, layout.size_1x(lines).map(|v| v * layout.scale));
            }
        }
    }

    #[test]
    fn couch_menu_layout_hit_maps_positions_to_lines() {
        let layout = Layout::for_stream(1920, 1080).unwrap();
        let rect = layout.panel(4);
        // 576×160 at (672, 448): padding 16 px across, 16 px down, 32-px lines.
        assert_eq!(
            rect,
            Rect {
                origin: [672, 448],
                size: [576, 160]
            }
        );
        let at = |x: u32, y: u32| layout.hit(4, x as f32 / 1920., y as f32 / 1080.);
        assert_eq!(at(100, 100), Hit::Outside);
        assert_eq!(at(680, 460), Hit::Panel);
        assert_eq!(at(700, 465), Hit::Line(0));
        assert_eq!(at(700, 494), Hit::Line(0));
        assert_eq!(at(1200, 497), Hit::Line(1));
        assert_eq!(at(700, 590), Hit::Line(3));
        assert_eq!(at(700, 600), Hit::Panel);
        assert_eq!(at(1240, 500), Hit::Panel, "right padding");
        assert_eq!(at(1249, 500), Hit::Outside);
        let big = Layout::for_stream(3840, 2160).unwrap();
        let rect = big.panel(4);
        let x = (rect.origin[0] + 3 * 16 + 5) as f32 / 3840.;
        let y = (rect.origin[1] + 3 * 16 + 3 * 32 + 2) as f32 / 2160.;
        assert_eq!(big.hit(4, x, y), Hit::Line(1));
    }
}
