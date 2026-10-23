//! Nightjar's look on the desktop: the Android app's night palette (blue-black surfaces,
//! the amber of the brand moon), Figtree type, and the brand mark drawn from its vector path.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use egui::{Color32, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle};

/// A colour role. The palette behind it is the normal night one or the high-contrast one, chosen
/// by [`set_high_contrast`], so drawing code names a role and never a colour. It derefs to the
/// current `Color32` (`*MOON`) and converts into one where egui takes `impl Into<Color32>`.
#[derive(Clone, Copy, Debug)]
pub struct Role(u8);

impl std::ops::Deref for Role {
    type Target = Color32;
    fn deref(&self) -> &Color32 {
        &palette(high_contrast())[usize::from(self.0)]
    }
}

impl From<Role> for Color32 {
    fn from(role: Role) -> Self {
        *role
    }
}

macro_rules! roles {
    ($($name:ident: $normal:expr, $high:expr;)*) => {
        #[allow(non_camel_case_types, clippy::upper_case_acronyms)]
        enum Index { $($name),* }
        const COUNT: usize = [$(Index::$name),*].len();
        const NORMAL: [Color32; COUNT] = [$($normal),*];
        const HIGH: [Color32; COUNT] = [$($high),*];
        $(pub const $name: Role = Role(Index::$name as u8);)*
    };
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

// `palette.xml` night roles; the second colour is the high-contrast one: black surfaces, white
// text, and no grey text at all (the variant and outline roles are on-surface).
roles! {
    NIGHT: rgb(0x14161F), rgb(0x000000);
    SURFACE_LOWEST: rgb(0x0E1017), rgb(0x000000);
    SURFACE_LOW: rgb(0x1A1C26), rgb(0x0A0A0C);
    SURFACE: rgb(0x1E202B), rgb(0x101114);
    SURFACE_HIGH: rgb(0x282A36), rgb(0x1C1D22);
    SURFACE_HIGHEST: rgb(0x333541), rgb(0x2A2C33);
    ON_SURFACE: rgb(0xE3E4EE), rgb(0xFFFFFF);
    ON_SURFACE_VARIANT: rgb(0xC1C4D2), rgb(0xFFFFFF);
    // Text on hovered cards and buttons too (4.5:1 on SURFACE_HIGHEST), lighter than Android's.
    OUTLINE: rgb(0xA0A3B1), rgb(0xFFFFFF);
    OUTLINE_VARIANT: rgb(0x3F4250), rgb(0xA8ABBA);
    MOON: rgb(0xF2A93B), rgb(0xFFC555);
    ON_MOON: rgb(0x2B1700), rgb(0x000000);
    MOON_CONTAINER: rgb(0x5C3E00), rgb(0x5C3E00);
    ON_MOON_CONTAINER: rgb(0xFFDDB0), rgb(0xFFFFFF);
    TERTIARY: rgb(0xAFC8E9), rgb(0xCFE2FF);
    ERROR: rgb(0xFFB4AB), rgb(0xFFCFC9);
    // The orbit rings of the site banner.
    ORBIT: rgb(0x2A2D3A), rgb(0x2A2D3A);
    // Paper white of the wordmark.
    PAPER: rgb(0xF4EFE6), rgb(0xFFFFFF);
}

fn palette(high: bool) -> &'static [Color32; COUNT] {
    if high { &HIGH } else { &NORMAL }
}

static HIGH_CONTRAST: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
thread_local! {
    /// A test's own palette, so a test looking at high contrast does not recolour the others.
    static TEST_HIGH: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

pub fn high_contrast() -> bool {
    #[cfg(test)]
    if let Some(high) = TEST_HIGH.with(std::cell::Cell::get) {
        return high;
    }
    HIGH_CONTRAST.load(Ordering::Relaxed)
}

pub fn set_high_contrast(on: bool) {
    HIGH_CONTRAST.store(on, Ordering::Relaxed);
}

/// The stroke of the focus ring: thick enough to find from across a room, thicker still in high
/// contrast.
pub fn focus_ring_width() -> f32 {
    if high_contrast() { 3.5 } else { 2.0 }
}

/// Whether the operating system asks for less motion. Windows: "Show animations in Windows" is
/// off (`SPI_GETCLIENTAREAANIMATION`). Other systems expose no setting we read, so motion stays.
/// Asked again at most every two seconds; the answer is cheap but frames are many.
pub fn system_reduces_motion() -> bool {
    static CHECKED: OnceLock<Instant> = OnceLock::new();
    static LAST_MS: AtomicU64 = AtomicU64::new(u64::MAX);
    static ANSWER: AtomicBool = AtomicBool::new(false);
    let now = CHECKED.get_or_init(Instant::now).elapsed().as_millis() as u64;
    let last = LAST_MS.load(Ordering::Relaxed);
    if last == u64::MAX || now.saturating_sub(last) >= 2000 {
        ANSWER.store(os_reduces_motion(), Ordering::Relaxed);
        LAST_MS.store(now, Ordering::Relaxed);
    }
    ANSWER.load(Ordering::Relaxed)
}

#[cfg(windows)]
fn os_reduces_motion() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    };
    let mut animations = 1i32;
    // SAFETY: the pointer is to a live BOOL-sized integer, which this action writes.
    let read = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some((&mut animations as *mut i32).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    read.is_ok() && animations == 0
}

#[cfg(not(windows))]
fn os_reduces_motion() -> bool {
    false
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

pub fn regular(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn apply(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "figtree".into(),
        Arc::new(FontData::from_static(include_bytes!("../assets/Figtree-400.ttf"))),
    );
    fonts.font_data.insert(
        "figtree-semibold".into(),
        Arc::new(FontData::from_static(include_bytes!("../assets/Figtree-600.ttf"))),
    );
    let fallback = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let proportional = fonts.families.entry(FontFamily::Proportional).or_default();
    proportional.insert(0, "figtree".into());
    let mut bold = vec!["figtree-semibold".to_owned()];
    bold.extend(fallback);
    fonts.families.insert(FontFamily::Name("semibold".into()), bold);
    ctx.set_fonts(fonts);
    set_look(ctx, high_contrast(), false);
}

/// Sets the palette and the style that goes with it. `still` turns egui's animations off
/// (reduced motion): focus and hover fades, collapsing and scrolling jump instead of glide.
pub fn set_look(ctx: &egui::Context, high: bool, still: bool) {
    set_high_contrast(high);
    let mut style = (*ctx.global_style()).clone();
    style.text_styles = [
        (TextStyle::Heading, semibold(26.0)),
        (TextStyle::Body, regular(15.0)),
        (TextStyle::Button, semibold(15.0)),
        (TextStyle::Small, regular(12.5)),
        (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
    ]
    .into();
    style.spacing.item_spacing = egui::vec2(10.0, 10.0);
    style.spacing.button_padding = egui::vec2(16.0, 9.0);
    style.spacing.interact_size.y = 34.0;
    style.spacing.combo_width = 220.0;
    style.animation_time = if still { 0.0 } else { egui::Style::default().animation_time };
    style.scroll_animation = if still { egui::style::ScrollAnimation::none() } else { egui::style::ScrollAnimation::default() };
    let v = &mut style.visuals;
    *v = egui::Visuals::dark();
    v.panel_fill = *NIGHT;
    v.window_fill = *SURFACE_HIGH;
    v.window_stroke = Stroke::new(if high { 2.0 } else { 1.0 }, *OUTLINE_VARIANT);
    v.extreme_bg_color = *SURFACE_LOWEST;
    v.faint_bg_color = *SURFACE_LOW;
    v.override_text_color = Some(*ON_SURFACE);
    v.hyperlink_color = *MOON;
    v.selection.bg_fill = *MOON_CONTAINER;
    v.selection.stroke = Stroke::new(1.0, *ON_MOON_CONTAINER);
    v.window_corner_radius = 14.into();
    v.menu_corner_radius = 10.into();
    v.window_shadow = egui::Shadow::NONE;
    v.popup_shadow = egui::Shadow::NONE;
    let radius = egui::CornerRadius::same(8);
    for (w, fill) in [
        (&mut v.widgets.noninteractive, *SURFACE),
        (&mut v.widgets.inactive, *SURFACE_HIGH),
        (&mut v.widgets.hovered, *SURFACE_HIGHEST),
        (&mut v.widgets.active, *SURFACE_HIGHEST),
        (&mut v.widgets.open, *SURFACE_HIGHEST),
    ] {
        w.bg_fill = fill;
        w.weak_bg_fill = fill;
        w.corner_radius = radius;
        w.fg_stroke = Stroke::new(1.0, *ON_SURFACE);
        // High contrast outlines every control, so a button reads as one without its fill.
        w.bg_stroke = if high { Stroke::new(1.5, *OUTLINE_VARIANT) } else { Stroke::NONE };
        w.expansion = 0.0;
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, *OUTLINE_VARIANT);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, *ON_SURFACE_VARIANT);
    // Keyboard and controller focus must be obvious from across a room.
    v.widgets.hovered.bg_stroke = Stroke::new(if high { 2.0 } else { 1.0 }, *OUTLINE);
    v.widgets.active.bg_stroke = Stroke::new(focus_ring_width(), *MOON);
    ctx.set_global_style(style);
}

/// The brand mark: the nightjar crossing the moon, from `site/assets/og.svg` (a 108-unit
/// adaptive icon: moon radius 27 around (54, 54)).
const BIRD: &str = "M 51.75,41.24 C 53.31,40.96 54.50,41.92 54.89,43.31 C 55.16,44.00 55.33,44.55 55.55,44.95 C 58.63,42.95 61.40,40.41 64.45,38.63 C 68.66,39.72 73.12,42.22 78.13,45.72 C 72.88,44.97 68.19,45.36 64.07,46.82 C 60.97,47.94 58.40,49.93 56.11,51.87 L 56.49,56.49 L 59.86,66.49 C 58.86,67.47 57.71,67.97 56.50,68.18 C 55.30,68.39 54.04,68.32 52.77,67.74 L 52.51,57.19 L 51.29,52.72 C 48.47,51.68 45.38,50.70 42.08,50.69 C 37.70,50.73 33.17,51.97 28.50,54.48 C 32.00,49.47 35.34,45.59 38.92,43.13 C 42.40,43.76 45.87,45.20 49.45,46.03 C 49.52,45.58 49.49,45.00 49.51,44.26 C 49.40,42.82 50.19,41.51 51.75,41.24 Z";

/// The bird outline flattened to a polygon in icon units.
fn bird_polygon() -> Vec<(f32, f32)> {
    let mut points = Vec::new();
    let mut tokens = BIRD
        .split([' ', ','])
        .filter(|t| !t.is_empty())
        .peekable();
    let mut current = (0.0f32, 0.0f32);
    let mut command = 'M';
    let number = |tokens: &mut std::iter::Peekable<_>| -> f32 {
        let t: &str = tokens.next().unwrap_or("0");
        t.parse().unwrap_or(0.0)
    };
    while let Some(&token) = tokens.peek() {
        if let Some(c) = token.chars().next().filter(char::is_ascii_alphabetic) {
            command = c;
            tokens.next();
            if c == 'Z' {
                break;
            }
            continue;
        }
        match command {
            'M' | 'L' => {
                current = (number(&mut tokens), number(&mut tokens));
                points.push(current);
            }
            'C' => {
                let c1 = (number(&mut tokens), number(&mut tokens));
                let c2 = (number(&mut tokens), number(&mut tokens));
                let end = (number(&mut tokens), number(&mut tokens));
                for step in 1..=12 {
                    let t = step as f32 / 12.0;
                    let u = 1.0 - t;
                    let x = u * u * u * current.0 + 3.0 * u * u * t * c1.0 + 3.0 * u * t * t * c2.0 + t * t * t * end.0;
                    let y = u * u * u * current.1 + 3.0 * u * u * t * c1.1 + 3.0 * u * t * t * c2.1 + t * t * t * end.1;
                    points.push((x, y));
                }
                current = end;
            }
            _ => {
                tokens.next();
            }
        }
    }
    points
}

fn inside(polygon: &[(f32, f32)], x: f32, y: f32) -> bool {
    let mut inside = false;
    let mut j = polygon.len() - 1;
    for i in 0..polygon.len() {
        let (xi, yi) = polygon[i];
        let (xj, yj) = polygon[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// The mark as straight RGBA pixels, `size` square: the amber moon with the night bird cut
/// out, 4x4 supersampled.
pub fn logo_rgba(size: usize) -> Vec<u8> {
    let bird = bird_polygon();
    // The mark keeps the brand colours in every palette.
    let (brand_moon, brand_night) = (NORMAL[Index::MOON as usize], NORMAL[Index::NIGHT as usize]);
    let mut out = vec![0u8; size * size * 4];
    let scale = 54.0 / size as f32; // the moon fills the image: 27 units of radius
    for py in 0..size {
        for px in 0..size {
            let (mut moon, mut cut) = (0u32, 0u32);
            for sy in 0..4 {
                for sx in 0..4 {
                    let x = 27.0 + (px as f32 + (sx as f32 + 0.5) / 4.0) * scale;
                    let y = 27.0 + (py as f32 + (sy as f32 + 0.5) / 4.0) * scale;
                    let in_moon = (x - 54.0).powi(2) + (y - 54.0).powi(2) <= 27.0 * 27.0;
                    if in_moon {
                        moon += 1;
                        if inside(&bird, x, y) {
                            cut += 1;
                        }
                    }
                }
            }
            let i = (py * size + px) * 4;
            if moon == 0 {
                continue;
            }
            // Bird pixels are the night colour; moon pixels amber; edges blend.
            let amber = (moon - cut) as f32;
            let night = cut as f32;
            let total = amber + night;
            let mix = |a: u8, n: u8| ((f32::from(a) * amber + f32::from(n) * night) / total) as u8;
            out[i] = mix(brand_moon.r(), brand_night.r());
            out[i + 1] = mix(brand_moon.g(), brand_night.g());
            out[i + 2] = mix(brand_moon.b(), brand_night.b());
            out[i + 3] = (moon * 255 / 16) as u8;
        }
    }
    out
}

pub fn logo_image(size: usize) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied([size, size], &logo_rgba(size))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bird_parses_into_a_closed_shape_inside_the_moon() {
        let bird = bird_polygon();
        assert!(bird.len() > 100);
        assert!(bird.iter().all(|&(x, y)| (27.0..=81.0).contains(&x) && (27.0..=81.0).contains(&y)));
        // The body is solid, the sky above the wings is not.
        assert!(inside(&bird, 54.5, 60.0));
        assert!(!inside(&bird, 54.0, 32.0));
    }

    fn luminance(c: Color32) -> f64 {
        let lin = |v: u8| {
            let s = f64::from(v) / 255.0;
            if s <= 0.04045 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
    }

    /// WCAG contrast ratio of two colours, 1 to 21.
    fn contrast(a: Color32, b: Color32) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    fn color(high: bool, role: Role) -> Color32 {
        palette(high)[usize::from(role.0)]
    }

    const GROUNDS: [Role; 4] = [NIGHT, SURFACE, SURFACE_HIGH, SURFACE_HIGHEST];

    #[test]
    fn text_meets_wcag_aa_normally_and_seven_to_one_in_high_contrast() {
        for (high, floor) in [(false, 4.5), (true, 7.0)] {
            for text in [ON_SURFACE, ON_SURFACE_VARIANT, OUTLINE, MOON, TERTIARY, ERROR] {
                for ground in GROUNDS {
                    let ratio = contrast(color(high, text), color(high, ground));
                    assert!(ratio >= floor, "high={high}: text {text:?} on {ground:?} is {ratio:.2}:1, wants {floor}");
                }
            }
            let on_moon = contrast(color(high, ON_MOON), color(high, MOON));
            assert!(on_moon >= floor, "high={high}: button text on the moon is {on_moon:.2}:1");
            let on_container = contrast(color(high, ON_MOON_CONTAINER), color(high, MOON_CONTAINER));
            assert!(on_container >= floor, "high={high}: text on a selection is {on_container:.2}:1");
        }
    }

    #[test]
    fn the_focus_ring_stands_out_from_every_surface_in_both_palettes() {
        for high in [false, true] {
            for ground in GROUNDS {
                let ratio = contrast(color(high, MOON), color(high, ground));
                assert!(ratio >= 3.0, "high={high}: ring on {ground:?} is {ratio:.2}:1, wants 3");
            }
        }
        // The ring is also thicker in high contrast.
        TEST_HIGH.with(|h| h.set(Some(false)));
        let normal = focus_ring_width();
        TEST_HIGH.with(|h| h.set(Some(true)));
        let thick = focus_ring_width();
        TEST_HIGH.with(|h| h.set(None));
        assert!(thick > normal);
    }

    #[test]
    fn high_contrast_outlines_are_visible_against_the_surfaces() {
        for ground in [NIGHT, SURFACE, SURFACE_HIGH] {
            let ratio = contrast(color(true, OUTLINE_VARIANT), color(true, ground));
            assert!(ratio >= 3.0, "outline on {ground:?} is {ratio:.2}:1, wants 3");
        }
    }

    #[test]
    fn logo_is_a_moon_with_a_night_bird() {
        let size = 64;
        let px = logo_rgba(size);
        let at = |x: usize, y: usize| &px[(y * size + x) * 4..(y * size + x) * 4 + 4];
        assert_eq!(at(0, 0)[3], 0, "corners are transparent");
        let moon = NORMAL[Index::MOON as usize];
        assert_eq!(at(32, 10), &[moon.r(), moon.g(), moon.b(), 255]);
        // The bird's body, just below the centre.
        let body = at(33, 40);
        assert!(body[0] < 80, "bird pixels are dark: {body:?}");
    }
}
