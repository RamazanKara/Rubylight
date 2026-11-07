//! The ambient backdrop's bake: a cover squashed to a thumbnail, blurred, saturated, its yellows
//! pulled toward amber and its luminance capped, so white text reads over the brightest cover.
//! Pure CPU work on small images; hosts.rs runs it off the UI thread beside the cover's decode.

use egui::{Color32, ColorImage};

use crate::ui::design_tokens as t;

const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];
/// The cover glow's strength and how its colour is lit from the bake: `0 0 70px rgba(255,95,70,.28)`
/// in the target. A bake darker than the last dims the glow with it.
const GLOW_ALPHA: f32 = 0.28;
const GLOW_SATURATE: f32 = 1.5;
const GLOW_SATURATION_MOST: f32 = 0.74;
const GLOW_DARK_BELOW: f32 = 0.16;
const SIZE: [usize; 2] = [
    t::TIER_BAKED_THUMB_W as usize,
    t::TIER_BAKED_THUMB_H as usize,
];

fn to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.max(0.0).powf(1.0 / 2.4) - 0.055
    }
}

fn rgb_to_hsv([r, g, b]: [f32; 3]) -> [f32; 3] {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d <= 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    [h, if max <= 0.0 { 0.0 } else { d / max }, max]
}

fn hsv_to_rgb([h, s, v]: [f32; 3]) -> [f32; 3] {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = v - c;
    let [r, g, b] = match (h.rem_euclid(360.0) / 60.0) as u32 {
        0 => [c, x, 0.0],
        1 => [x, c, 0.0],
        2 => [0.0, c, x],
        3 => [0.0, x, c],
        4 => [x, 0.0, c],
        _ => [c, 0.0, x],
    };
    [r + m, g + m, b + m]
}

/// One thumbnail pixel's colour: saturation lifted, yellow moved toward amber (it dims to olive
/// otherwise), luminance capped in linear light so a white cover comes out a safe grey.
fn grade(rgb: [f32; 3]) -> [f32; 3] {
    let [mut h, s, v] = rgb_to_hsv(rgb);
    let band = ((h - 25.0) / 15.0).clamp(0.0, 1.0) * ((95.0 - h) / 15.0).clamp(0.0, 1.0);
    h += (22.0 - h) * band;
    let lifted = hsv_to_rgb([h, (s * t::AMBIENT_SATURATE).min(1.0), v]);
    let linear = lifted.map(to_linear);
    let luma: f32 = (0..3).map(|i| linear[i] * LUMA[i]).sum();
    let scale = (t::AMBIENT_TEXT_ZONE_MAX_LUMA / luma.max(1e-5)).min(1.0);
    linear.map(|c| to_srgb(c * scale).clamp(0.0, 1.0))
}

/// The area average of `cover` over each cell of a `SIZE` thumbnail: all of the cover, squashed.
fn squash(cover: &ColorImage) -> Vec<[f32; 3]> {
    let [w, h] = cover.size;
    let [tw, th] = SIZE;
    let mut out = vec![[0.0f32; 3]; tw * th];
    if w == 0 || h == 0 {
        return out;
    }
    let mut counts = vec![0u32; tw * th];
    for y in 0..h {
        let ty = y * th / h;
        for x in 0..w {
            let at = ty * tw + x * tw / w;
            let c = cover.pixels[y * w + x];
            out[at][0] += f32::from(c.r());
            out[at][1] += f32::from(c.g());
            out[at][2] += f32::from(c.b());
            counts[at] += 1;
        }
    }
    // A cover smaller than the thumbnail leaves cells empty: they take the nearest source pixel.
    for ty in 0..th {
        for tx in 0..tw {
            let at = ty * tw + tx;
            if counts[at] == 0 {
                let c = cover.pixels[(ty * h / th) * w + tx * w / tw];
                out[at] = [f32::from(c.r()), f32::from(c.g()), f32::from(c.b())];
                counts[at] = 1;
            }
            out[at] = out[at].map(|v| v / counts[at] as f32 / 255.0);
        }
    }
    out
}

/// A separable Gaussian of `sigma` thumbnail pixels; edges repeat.
fn gaussian(pixels: &[[f32; 3]], [w, h]: [usize; 2], sigma: f32) -> Vec<[f32; 3]> {
    let reach = (sigma * 3.0).ceil() as isize;
    let weights: Vec<f32> = (-reach..=reach)
        .map(|i| (-(i as f32).powi(2) / (2.0 * sigma * sigma)).exp())
        .collect();
    let total: f32 = weights.iter().sum();
    let line = |get: &dyn Fn(isize) -> [f32; 3]| {
        let mut sum = [0.0f32; 3];
        for (i, weight) in (-reach..=reach).zip(&weights) {
            let c = get(i);
            for k in 0..3 {
                sum[k] += c[k] * weight;
            }
        }
        sum.map(|v| v / total)
    };
    let mut across = vec![[0.0f32; 3]; pixels.len()];
    for y in 0..h {
        for x in 0..w {
            across[y * w + x] =
                line(&|i| pixels[y * w + (x as isize + i).clamp(0, w as isize - 1) as usize]);
        }
    }
    let mut down = vec![[0.0f32; 3]; pixels.len()];
    for y in 0..h {
        for x in 0..w {
            down[y * w + x] =
                line(&|i| across[(y as isize + i).clamp(0, h as isize - 1) as usize * w + x]);
        }
    }
    down
}

/// To bytes with one level of ordered noise, so wide dark gradients do not band.
fn finish(pixels: &[[f32; 3]], size: [usize; 2]) -> ColorImage {
    let mut image = ColorImage::filled(size, Color32::BLACK);
    for (at, c) in pixels.iter().enumerate() {
        let noise = ((at.wrapping_mul(2_654_435_761)) >> 24 & 0xFF) as f32 / 255.0 - 0.5;
        let [r, g, b] = c.map(|v| (v * 255.0 + noise + 0.5).clamp(0.0, 255.0) as u8);
        image.pixels[at] = Color32::from_rgb(r, g, b);
    }
    image
}

/// The ambient of a cover: one call per cover, off the UI thread.
pub fn bake(cover: &ColorImage) -> ColorImage {
    let small = squash(cover);
    let soft = gaussian(&small, SIZE, SIZE[0] as f32 / 8.0);
    let graded: Vec<[f32; 3]> = soft.into_iter().map(grade).collect();
    finish(&graded, SIZE)
}

/// What the cover glows with on its page: the bake's own colour lit up, its hue kept, weighted to
/// where the bake is most coloured, as strong as the spec's glow. A grey cover glows pale and a black
/// one not at all. Premultiplied.
pub fn glow_of(baked: &ColorImage) -> Color32 {
    let (mut sum, mut weights, mut brightest) = ([0.0f32; 3], 0.0f32, 0.0f32);
    for pixel in &baked.pixels {
        let c = [pixel.r(), pixel.g(), pixel.b()].map(|v| f32::from(v) / 255.0);
        let [_, s, v] = rgb_to_hsv(c);
        let weight = s * v + 1e-3;
        for k in 0..3 {
            sum[k] += c[k] * weight;
        }
        weights += weight;
        brightest = brightest.max(v);
    }
    if weights <= 0.0 {
        return Color32::TRANSPARENT;
    }
    let [h, s, _] = rgb_to_hsv(sum.map(|v| v / weights));
    let lit = hsv_to_rgb([h, (s * GLOW_SATURATE).min(GLOW_SATURATION_MOST), 1.0]);
    let strength = GLOW_ALPHA * (brightest / GLOW_DARK_BELOW).min(1.0);
    let [r, g, b] = lit.map(|v| (v * strength * 255.0).round() as u8);
    Color32::from_rgba_premultiplied(r, g, b, (strength * 255.0).round() as u8)
}

/// The ambient of a game without art: its tile's gradient, under the same luminance cap.
pub fn bake_tile(top: Color32, bottom: Color32) -> ColorImage {
    let [w, h] = SIZE;
    let channel = |c: Color32| [c.r(), c.g(), c.b()].map(|v| f32::from(v) / 255.0);
    let (top, bottom) = (channel(top), channel(bottom));
    // The tile's 165 degree gradient.
    let (dx, dy) = (165f32.to_radians().sin(), -165f32.to_radians().cos());
    let span = dx.abs() * w as f32 + dy.abs() * h as f32;
    let mut pixels = vec![[0.0f32; 3]; w * h];
    for y in 0..h {
        for x in 0..w {
            let along =
                ((x as f32 - w as f32 / 2.0) * dx + (y as f32 - h as f32 / 2.0) * dy) / span + 0.5;
            let mix = std::array::from_fn(|k| top[k] + (bottom[k] - top[k]) * along);
            pixels[y * w + x] = grade(mix);
        }
    }
    finish(&pixels, SIZE)
}

/// Where the sky's middle colour sits, top to bottom.
pub const SKY_MID_AT: f32 = 0.46;

/// The moon's glow at `d` of its radius: 36 % of the token's strength a quarter of the way out,
/// none at the rim, full at the middle.
pub fn sky_glow(d: f32) -> f32 {
    if d >= 1.0 {
        0.0
    } else if d >= 0.25 {
        0.36 * (1.0 - (d - 0.25) / 0.75)
    } else {
        1.0 + (0.36 - 1.0) * d / 0.25
    }
}

/// The night sky's colours, top to bottom, with the moon's warm glow around `moon` (fractions of
/// the page, `glow_radius` likewise): what glass samples on a page without art.
pub fn bake_sky(moon: [f32; 2], glow_radius: [f32; 2]) -> ColorImage {
    let [w, h] = SIZE;
    let channel = |c: Color32| [c.r(), c.g(), c.b(), c.a()].map(|v| f32::from(v) / 255.0);
    let (top, mid, bottom, glow) = (
        channel(t::SKY_TOP),
        channel(t::SKY_MID),
        channel(t::SKY_BOTTOM),
        channel(t::SKY_GLOW),
    );
    let mut pixels = vec![[0.0f32; 3]; w * h];
    for y in 0..h {
        for x in 0..w {
            let v = (y as f32 + 0.5) / h as f32;
            let (from, to, along) = if v < SKY_MID_AT {
                (top, mid, v / SKY_MID_AT)
            } else {
                (mid, bottom, (v - SKY_MID_AT) / (1.0 - SKY_MID_AT))
            };
            let d = (((x as f32 + 0.5) / w as f32 - moon[0]) / glow_radius[0])
                .hypot((v - moon[1]) / glow_radius[1]);
            let glow_at = sky_glow(d);
            pixels[y * w + x] = std::array::from_fn(|k| {
                let sky = from[k] + (to[k] - from[k]) * along;
                sky * (1.0 - glow[3] * glow_at) + glow[k] * glow_at
            });
        }
    }
    finish(&pixels, SIZE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luma(c: Color32) -> f32 {
        let linear = [c.r(), c.g(), c.b()].map(|v| to_linear(f32::from(v) / 255.0));
        (0..3).map(|i| linear[i] * LUMA[i]).sum()
    }

    #[test]
    fn a_white_cover_bakes_to_the_safe_grey_the_spec_names() {
        let white = ColorImage::filled([600, 900], Color32::WHITE);
        let out = bake(&white);
        assert_eq!(out.size, SIZE);
        for pixel in &out.pixels {
            assert!(
                luma(*pixel) <= t::AMBIENT_TEXT_ZONE_MAX_LUMA + 0.003,
                "{pixel:?}"
            );
            assert!((i32::from(pixel.r()) - 82).abs() <= 1, "{pixel:?}");
        }
    }

    #[test]
    fn a_dark_cover_keeps_its_colour_and_a_black_one_stays_black() {
        let dark = ColorImage::filled([300, 450], Color32::from_rgb(40, 10, 60));
        let c = bake(&dark).pixels[500];
        assert!(c.b() > c.r() && c.r() > c.g(), "{c:?}");
        let black = bake(&ColorImage::filled([300, 450], Color32::BLACK));
        assert!(black
            .pixels
            .iter()
            .all(|p| p.r() <= 1 && p.g() <= 1 && p.b() <= 1));
    }

    #[test]
    fn yellow_moves_toward_amber() {
        let [h, ..] = rgb_to_hsv(grade([0.9, 0.85, 0.1]));
        assert!(h < 45.0, "{h}");
    }

    #[test]
    fn the_bake_sees_the_whole_cover_squashed_not_a_crop() {
        // Red top, blue bottom: both ends of the cover reach the thumbnail.
        let mut image = ColorImage::filled([200, 300], Color32::from_rgb(90, 0, 0));
        for pixel in &mut image.pixels[200 * 150..] {
            *pixel = Color32::from_rgb(0, 0, 90);
        }
        let out = bake(&image);
        let (top, bottom) = (out.pixels[48], out.pixels[96 * 53 + 48]);
        assert!(top.r() > top.b(), "{top:?}");
        assert!(bottom.b() > bottom.r(), "{bottom:?}");
    }

    #[test]
    fn a_warm_cover_glows_warm_a_blue_one_blue_and_a_black_one_not_at_all() {
        let warm = glow_of(&bake(&ColorImage::filled(
            [200, 300],
            Color32::from_rgb(200, 70, 50),
        )));
        assert!(warm.r() > warm.g() * 2 && warm.r() > warm.b() * 2, "{warm:?}");
        let alpha = f32::from(warm.a()) / 255.0;
        assert!((alpha - GLOW_ALPHA).abs() < 0.02, "the spec's strength: {alpha}");
        let blue = glow_of(&bake(&ColorImage::filled(
            [200, 300],
            Color32::from_rgb(30, 60, 200),
        )));
        assert!(blue.b() > blue.r() * 2, "{blue:?}");
        let black = glow_of(&bake(&ColorImage::filled([200, 300], Color32::BLACK)));
        assert!(black.a() < 16, "{black:?}");
    }

    #[test]
    fn a_tile_gradient_stays_under_the_cap() {
        for (top, bottom) in [
            (t::TILE_PAIR6_TOP, t::TILE_PAIR6_BOTTOM),
            (t::TILE_PAIR0_TOP, t::TILE_PAIR0_BOTTOM),
        ] {
            let out = bake_tile(top, bottom);
            assert!(out
                .pixels
                .iter()
                .all(|p| luma(*p) <= t::AMBIENT_TEXT_ZONE_MAX_LUMA + 0.003));
        }
    }

    #[test]
    fn tiny_covers_bake_without_gaps() {
        let out = bake(&ColorImage::filled([2, 3], Color32::from_rgb(10, 60, 90)));
        assert!(
            out.pixels.iter().all(|p| p.b() > 20),
            "no empty cell stays black"
        );
    }
}
