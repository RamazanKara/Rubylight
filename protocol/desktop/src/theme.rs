//! Nightjar's look on the desktop: the Android app's night palette (blue-black surfaces,
//! the amber of the brand moon), Figtree type, and the brand mark drawn from its vector path.

use std::sync::Arc;

use egui::{Color32, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle};

/// `palette.xml` night roles.
pub const NIGHT: Color32 = Color32::from_rgb(0x14, 0x16, 0x1F);
pub const SURFACE_LOWEST: Color32 = Color32::from_rgb(0x0E, 0x10, 0x17);
pub const SURFACE_LOW: Color32 = Color32::from_rgb(0x1A, 0x1C, 0x26);
pub const SURFACE: Color32 = Color32::from_rgb(0x1E, 0x20, 0x2B);
pub const SURFACE_HIGH: Color32 = Color32::from_rgb(0x28, 0x2A, 0x36);
pub const SURFACE_HIGHEST: Color32 = Color32::from_rgb(0x33, 0x35, 0x41);
pub const ON_SURFACE: Color32 = Color32::from_rgb(0xE3, 0xE4, 0xEE);
pub const ON_SURFACE_VARIANT: Color32 = Color32::from_rgb(0xC1, 0xC4, 0xD2);
pub const OUTLINE: Color32 = Color32::from_rgb(0x8B, 0x8E, 0x9C);
pub const OUTLINE_VARIANT: Color32 = Color32::from_rgb(0x3F, 0x42, 0x50);
pub const MOON: Color32 = Color32::from_rgb(0xF2, 0xA9, 0x3B);
pub const ON_MOON: Color32 = Color32::from_rgb(0x2B, 0x17, 0x00);
pub const MOON_CONTAINER: Color32 = Color32::from_rgb(0x5C, 0x3E, 0x00);
pub const ON_MOON_CONTAINER: Color32 = Color32::from_rgb(0xFF, 0xDD, 0xB0);
pub const TERTIARY: Color32 = Color32::from_rgb(0xAF, 0xC8, 0xE9);
pub const ERROR: Color32 = Color32::from_rgb(0xFF, 0xB4, 0xAB);
/// The orbit rings of the site banner.
pub const ORBIT: Color32 = Color32::from_rgb(0x2A, 0x2D, 0x3A);
/// Paper white of the wordmark.
pub const PAPER: Color32 = Color32::from_rgb(0xF4, 0xEF, 0xE6);

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
    let v = &mut style.visuals;
    *v = egui::Visuals::dark();
    v.panel_fill = NIGHT;
    v.window_fill = SURFACE_HIGH;
    v.window_stroke = Stroke::new(1.0, OUTLINE_VARIANT);
    v.extreme_bg_color = SURFACE_LOWEST;
    v.faint_bg_color = SURFACE_LOW;
    v.override_text_color = Some(ON_SURFACE);
    v.hyperlink_color = MOON;
    v.selection.bg_fill = MOON_CONTAINER;
    v.selection.stroke = Stroke::new(1.0, ON_MOON_CONTAINER);
    v.window_corner_radius = 14.into();
    v.menu_corner_radius = 10.into();
    v.window_shadow = egui::Shadow::NONE;
    v.popup_shadow = egui::Shadow::NONE;
    let radius = egui::CornerRadius::same(8);
    for (w, fill) in [
        (&mut v.widgets.noninteractive, SURFACE),
        (&mut v.widgets.inactive, SURFACE_HIGH),
        (&mut v.widgets.hovered, SURFACE_HIGHEST),
        (&mut v.widgets.active, SURFACE_HIGHEST),
        (&mut v.widgets.open, SURFACE_HIGHEST),
    ] {
        w.bg_fill = fill;
        w.weak_bg_fill = fill;
        w.corner_radius = radius;
        w.fg_stroke = Stroke::new(1.0, ON_SURFACE);
        w.bg_stroke = Stroke::NONE;
        w.expansion = 0.0;
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, OUTLINE_VARIANT);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, ON_SURFACE_VARIANT);
    // Keyboard and controller focus must be obvious from across a room.
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, OUTLINE);
    v.widgets.active.bg_stroke = Stroke::new(2.0, MOON);
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
            out[i] = mix(MOON.r(), NIGHT.r());
            out[i + 1] = mix(MOON.g(), NIGHT.g());
            out[i + 2] = mix(MOON.b(), NIGHT.b());
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

    #[test]
    fn logo_is_a_moon_with_a_night_bird() {
        let size = 64;
        let px = logo_rgba(size);
        let at = |x: usize, y: usize| &px[(y * size + x) * 4..(y * size + x) * 4 + 4];
        assert_eq!(at(0, 0)[3], 0, "corners are transparent");
        assert_eq!(at(32, 10), &[MOON.r(), MOON.g(), MOON.b(), 255]);
        // The bird's body, just below the centre.
        let body = at(33, 40);
        assert!(body[0] < 80, "bird pixels are dark: {body:?}");
    }
}
