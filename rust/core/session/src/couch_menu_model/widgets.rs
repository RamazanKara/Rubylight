//! Drawing primitives over the menu's 1x canvas: anti-aliased rounded
//! rectangles, capsules and polygons, and the widgets built from them (pill
//! switch, inline slider, chevrons, header badge). Every edge is blended by
//! its coverage against the opaque colour already underneath, so the panel
//! stays opaque (alpha 255) for the compositor, which reads only RGB.
use super::theme_70::{
    BRAND, FOCUS, FOCUS_INK, FOCUS_STRONG, INK, INK_2, MUTED, OK, Rgb, STAGE, TRACK,
};

/// An opaque RGBA8 image, drawn back to front.
pub(super) struct Canvas {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// How a row's controls are coloured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tone {
    Normal,
    /// On the ruby focus card.
    Focused,
    Locked,
}

impl Canvas {
    pub fn new(width: u32, height: u32, ground: Rgb) -> Self {
        let mut canvas = Self {
            width,
            height,
            rgba: vec![0xff; (width * height * 4) as usize],
        };
        canvas.fill([0, 0, width, height], ground);
        canvas
    }
    pub fn fill(&mut self, [x, y, w, h]: [u32; 4], colour: Rgb) {
        for row in y..y + h {
            let start = ((row * self.width + x) * 4) as usize;
            for pixel in self.rgba[start..start + (w * 4) as usize]
                .as_chunks_mut::<4>()
                .0
            {
                pixel[..3].copy_from_slice(&colour);
            }
        }
    }
    /// A 1-pixel frame around the whole canvas.
    pub fn border(&mut self, colour: Rgb) {
        let [w, h] = [self.width, self.height];
        for rect in [
            [0, 0, w, 1],
            [0, h - 1, w, 1],
            [0, 0, 1, h],
            [w - 1, 0, 1, h],
        ] {
            self.fill(rect, colour);
        }
    }
    /// Mixes `colour` over pixel (`x`, `y`) by `coverage` (0 to 1).
    fn blend(&mut self, x: i64, y: i64, colour: Rgb, coverage: f32) {
        if coverage <= 0. || x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 {
            return;
        }
        let at = ((y as u32 * self.width + x as u32) * 4) as usize;
        let coverage = coverage.min(1.);
        for (under, over) in self.rgba[at..at + 3].iter_mut().zip(colour) {
            let mixed = f32::from(*under) + (f32::from(over) - f32::from(*under)) * coverage;
            *under = mixed.round() as u8;
        }
    }
    /// Paints the pixels of `bounds` by a signed distance (negative inside)
    /// taken at each pixel's centre: a one-pixel ramp across the edge.
    fn paint(&mut self, bounds: [f32; 4], colour: Rgb, distance: impl Fn(f32, f32) -> f32) {
        let [x0, y0, x1, y1] = bounds;
        for y in (y0.floor() as i64 - 1)..=(y1.ceil() as i64) {
            for x in (x0.floor() as i64 - 1)..=(x1.ceil() as i64) {
                let d = distance(x as f32 + 0.5, y as f32 + 0.5);
                self.blend(x, y, colour, (0.5 - d).clamp(0., 1.));
            }
        }
    }
    /// A rectangle with corners of `radius`; whole-pixel straight edges stay
    /// crisp.
    pub fn rounded(&mut self, [x, y, w, h]: [f32; 4], radius: f32, colour: Rgb) {
        let radius = radius.min(w / 2.).min(h / 2.);
        let (cx, cy) = (x + w / 2., y + h / 2.);
        let (hx, hy) = (w / 2. - radius, h / 2. - radius);
        self.paint([x, y, x + w, y + h], colour, |px, py| {
            let qx = (px - cx).abs() - hx;
            let qy = (py - cy).abs() - hy;
            qx.max(0.).hypot(qy.max(0.)) + qx.max(qy).min(0.) - radius
        });
    }
    pub fn circle(&mut self, [cx, cy]: [f32; 2], radius: f32, colour: Rgb) {
        self.rounded(
            [cx - radius, cy - radius, 2. * radius, 2. * radius],
            radius,
            colour,
        );
    }
    /// A line from `a` to `b`, `half` wide on each side, with round caps.
    pub fn capsule(&mut self, a: [f32; 2], b: [f32; 2], half: f32, colour: Rgb) {
        let bounds = [
            a[0].min(b[0]) - half,
            a[1].min(b[1]) - half,
            a[0].max(b[0]) + half,
            a[1].max(b[1]) + half,
        ];
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let length = (dx * dx + dy * dy).max(f32::EPSILON);
        self.paint(bounds, colour, |px, py| {
            let t = (((px - a[0]) * dx + (py - a[1]) * dy) / length).clamp(0., 1.);
            (px - a[0] - t * dx).hypot(py - a[1] - t * dy) - half
        });
    }
    /// A convex polygon, its coverage from 4x4 samples per pixel.
    pub fn polygon(&mut self, points: &[[f32; 2]], colour: Rgb) {
        let inside = |x: f32, y: f32| {
            let mut sign = 0.;
            for (i, a) in points.iter().enumerate() {
                let b = points[(i + 1) % points.len()];
                let cross = (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0]);
                if cross * sign < 0. {
                    return false;
                }
                if cross != 0. {
                    sign = cross;
                }
            }
            true
        };
        let x0 = points.iter().map(|p| p[0]).fold(f32::MAX, f32::min).floor() as i64;
        let x1 = points.iter().map(|p| p[0]).fold(f32::MIN, f32::max).ceil() as i64;
        let y0 = points.iter().map(|p| p[1]).fold(f32::MAX, f32::min).floor() as i64;
        let y1 = points.iter().map(|p| p[1]).fold(f32::MIN, f32::max).ceil() as i64;
        for y in y0..y1 {
            for x in x0..x1 {
                let hits = (0..16)
                    .filter(|s| {
                        let sx = x as f32 + (f32::from(*s as u8 % 4) + 0.5) / 4.;
                        let sy = y as f32 + (f32::from(*s as u8 / 4) + 0.5) / 4.;
                        inside(sx, sy)
                    })
                    .count();
                self.blend(x, y, colour, hits as f32 / 16.);
            }
        }
    }
}

/// A pill switch ending at `right`, centred on `cy`; `u` is the cell height
/// over 32. Returns its left edge.
pub(super) fn switch(
    canvas: &mut Canvas,
    right: f32,
    cy: f32,
    u: f32,
    on: bool,
    tone: Tone,
) -> f32 {
    let [w, h, knob] = [40., 24., 8.].map(|v| v * u);
    let (track, dot) = match (tone, on) {
        (Tone::Normal, false) | (Tone::Locked, false) => (TRACK, MUTED),
        (Tone::Normal, true) => (OK, STAGE),
        (Tone::Focused, false) => (FOCUS_STRONG, FOCUS_INK),
        (Tone::Focused, true) => (FOCUS_INK, FOCUS),
        (Tone::Locked, true) => (MUTED, TRACK),
    };
    let left = right - w;
    canvas.rounded([left, cy - h / 2., w, h], h / 2., track);
    let inset = h / 2.;
    let x = if on { right - inset } else { left + inset };
    canvas.circle([x, cy], knob, dot);
    left
}

/// An inline slider over `[x, x + w)`, centred on `cy`, its knob at
/// `fraction` (0 to 1) of the travel.
pub(super) fn slider(
    canvas: &mut Canvas,
    [x, w]: [f32; 2],
    cy: f32,
    u: f32,
    fraction: f32,
    tone: Tone,
) {
    let [rail, knob] = [4., 8.].map(|v| v * u);
    let (track, fill, dot) = match tone {
        Tone::Normal => (TRACK, INK_2, INK),
        Tone::Focused => (FOCUS_STRONG, FOCUS_INK, FOCUS_INK),
        Tone::Locked => (TRACK, MUTED, MUTED),
    };
    canvas.rounded([x, cy - rail / 2., w, rail], rail / 2., track);
    let at = x + knob + (w - 2. * knob) * fraction.clamp(0., 1.);
    canvas.rounded([x, cy - rail / 2., at - x, rail], rail / 2., fill);
    canvas.circle([at, cy], knob, dot);
}

/// A small chevron centred on (`cx`, `cy`), pointing right or left.
pub(super) fn chevron(canvas: &mut Canvas, [cx, cy]: [f32; 2], u: f32, right: bool, colour: Rgb) {
    let [half_w, half_h] = [3.5, 6.].map(|v| v * u);
    let stroke = (1.25 * u).max(0.75);
    let (tip, tail) = if right {
        (cx + half_w, cx - half_w)
    } else {
        (cx - half_w, cx + half_w)
    };
    canvas.capsule([tail, cy - half_h], [tip, cy], stroke, colour);
    canvas.capsule([tail, cy + half_h], [tip, cy], stroke, colour);
}

/// The Rubylight badge: a ruby rounded square of `size` at (`x`, `y`) with
/// a white play mark.
pub(super) fn badge(canvas: &mut Canvas, [x, y]: [f32; 2], size: f32) {
    canvas.rounded([x, y, size, size], size / 4., BRAND);
    let at = |fx: f32, fy: f32| [x + fx * size, y + fy * size];
    canvas.polygon(&[at(0.36, 0.26), at(0.78, 0.5), at(0.36, 0.74)], FOCUS_INK);
}

/// The colour of a label and of a value on a row of `tone`.
pub(super) fn inks(tone: Tone) -> (Rgb, Rgb) {
    match tone {
        Tone::Normal => (INK, INK_2),
        Tone::Focused => (FOCUS_INK, FOCUS_INK),
        Tone::Locked => (MUTED, MUTED),
    }
}
