//! A float reference for the stream's 4:2:0 colour conversion and the scores
//! the AMF video converter A/B (`amf::vpe_ab_tests`) compares converters with:
//! PSNR and SSIM per plane, ΔE ITP (ITU-R BT.2124) of the decoded colour,
//! and where a converter puts its chroma samples.
//!
//! The reference is what an ideal converter writes before rounding:
//!
//! - placement as the shaders place the picture (`content` in color.hlsl):
//!   the aspect ratio kept, centred between black bars, a size within a
//!   pixel of the stream's shape filling it;
//! - resampling by exact pixel coverage when reducing and bilinear when
//!   enlarging, in linear light for HDR and on the desktop's sRGB codes for
//!   SDR, as SDR desktops are scaled;
//! - the stream's transfer (sRGB codes as they are for Rec.601 and Rec.709,
//!   the BT.2020 curve, PQ, or HLG for a 1000-nit display), its matrix and
//!   range;
//! - chroma horizontally co-sited with the left luma sample and vertically
//!   centred (chroma_sample_loc_type 0, which decoders assume), filtered
//!   [1 2 1] across and [1 1] down.
use crate::amf_vpe::{Source, Transfer};

/// The stream a reference is made for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spec {
    pub width: usize,
    pub height: usize,
    pub source: Source,
    pub transfer: Transfer,
    /// 0 Rec.601, 1 Rec.709, 2 Rec.2020.
    pub matrix: u8,
    pub full_range: bool,
    pub ten_bit: bool,
}
impl Spec {
    pub fn bits(&self) -> u32 {
        if self.ten_bit { 10 } else { 8 }
    }
    pub fn peak(&self) -> f64 {
        f64::from((1u32 << self.bits()) - 1)
    }
}

/// A source picture in its own encoding: sRGB codes (0 to 1) for
/// [`Source::Bgra8`], linear scRGB (1.0 = 80 nits) for
/// [`Source::ScRgbF16`], PQ codes of BT.2020 for [`Source::Pq10`].
#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<[f64; 3]>,
}

/// Y, Cb and Cr code values. The chroma planes are half size each way.
#[derive(Clone, Debug, PartialEq)]
pub struct Planes {
    pub width: usize,
    pub height: usize,
    pub y: Vec<f64>,
    pub cb: Vec<f64>,
    pub cr: Vec<f64>,
}
impl Planes {
    /// Planes from an NV12 or P010 readback: `codes` holds the luma rows,
    /// then the interleaved chroma rows, each value already shifted down to
    /// its bit depth (P010's upper ten bits).
    pub fn from_semi_planar(codes: &[u16], width: usize, height: usize) -> Option<Self> {
        let luma = width * height;
        let chroma = (width / 2) * (height / 2);
        if codes.len() < luma + chroma * 2 {
            return None;
        }
        let y = codes[..luma].iter().map(|&c| f64::from(c)).collect();
        let pairs = &codes[luma..luma + chroma * 2];
        let cb = pairs.iter().step_by(2).map(|&c| f64::from(c)).collect();
        let cr = pairs
            .iter()
            .skip(1)
            .step_by(2)
            .map(|&c| f64::from(c))
            .collect();
        Some(Self {
            width,
            height,
            y,
            cb,
            cr,
        })
    }
}

const BT709_TO_2020: [[f64; 3]; 3] = [
    [0.627404, 0.329283, 0.043313],
    [0.069097, 0.919540, 0.011362],
    [0.016391, 0.088013, 0.895595],
];
const BT2020_TO_709: [[f64; 3]; 3] = [
    [1.660491, -0.587641, -0.072850],
    [-0.124550, 1.132900, -0.008349],
    [-0.018151, -0.100579, 1.118730],
];
fn apply(m: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    m.map(|row| row[0] * v[0] + row[1] * v[1] + row[2] * v[2])
}
fn each(v: [f64; 3], f: impl Fn(f64) -> f64) -> [f64; 3] {
    v.map(f)
}
pub fn srgb_decode(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}
pub fn srgb_encode(l: f64) -> f64 {
    let l = l.max(0.);
    if l <= 0.0031308 {
        l * 12.92
    } else {
        1.055 * l.powf(1. / 2.4) - 0.055
    }
}
/// BT.2020's SDR OETF (transfer_characteristics 14), as `bt2020_sdr`.
pub fn bt2020_oetf(l: f64) -> f64 {
    let l = l.clamp(0., 1.);
    if l < 0.018 {
        4.5 * l
    } else {
        1.099 * l.powf(0.45) - 0.099
    }
}
pub fn bt2020_eotf(v: f64) -> f64 {
    let v = v.clamp(0., 1.);
    if v < 0.081 {
        v / 4.5
    } else {
        ((v + 0.099) / 1.099).powf(1. / 0.45)
    }
}
const M1: f64 = 2610. / 16384.;
const M2: f64 = 2523. / 32.;
const C1: f64 = 3424. / 4096.;
const C2: f64 = 2413. / 128.;
const C3: f64 = 2392. / 128.;
/// The PQ code of `l`, light as a fraction of 10000 nits.
pub fn pq(l: f64) -> f64 {
    let p = l.clamp(0., 1.).powf(M1);
    ((C1 + C2 * p) / (1. + C3 * p)).powf(M2)
}
/// Light as a fraction of 10000 nits for a PQ code.
pub fn depq(v: f64) -> f64 {
    let p = v.clamp(0., 1.).powf(1. / M2);
    ((p - C1).max(0.) / (C2 - C3 * p).max(1e-12)).powf(1. / M1)
}
const HLG_A: f64 = 0.17883277;
const HLG_B: f64 = 0.28466892;
const HLG_C: f64 = 0.55991073;
const LUMA_2020: [f64; 3] = [0.2627, 0.6780, 0.0593];
fn hlg_oetf(e: f64) -> f64 {
    let e = e.max(0.);
    if e <= 1. / 12. {
        (3. * e).sqrt()
    } else {
        HLG_A * (12. * e - HLG_B).max(1e-12).ln() + HLG_C
    }
}
fn hlg_inverse_oetf(v: f64) -> f64 {
    let v = v.max(0.);
    if v <= 0.5 {
        v * v / 3.
    } else {
        (((v - HLG_C) / HLG_A).exp() + HLG_B) / 12.
    }
}
/// Scene light for BT.2020 display light (fractions of a 1000-nit peak):
/// the inverse reference OOTF at system gamma 1.2, as the shader's `hlg`.
pub fn hlg_scene(display: [f64; 3]) -> [f64; 3] {
    let display = each(display, |c| c.max(0.));
    let y = dot(display, LUMA_2020);
    if y <= 0. {
        return [0.; 3];
    }
    let gain = y.max(1e-6).powf((1. - 1.2) / 1.2);
    each(display, |c| c * gain)
}
fn hlg_display(scene: [f64; 3]) -> [f64; 3] {
    let y = dot(scene, LUMA_2020);
    if y <= 0. {
        return [0.; 3];
    }
    let gain = y.powf(0.2);
    each(scene, |c| c * gain)
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Where the source lands in the stream, in target pixels: x, y, width,
/// height (fractional), and whether it fills the stream.
fn placement(source: (usize, usize), target: (usize, usize)) -> [f64; 4] {
    let (sw, sh) = (source.0 as f64, source.1 as f64);
    let (tw, th) = (target.0 as f64, target.1 as f64);
    let scale = (tw / sw).min(th / sh);
    let (w, h) = (sw * scale, sh * scale);
    if (w - tw).abs() < 1. && (h - th).abs() < 1. {
        [0., 0., tw, th]
    } else {
        [(tw - w) / 2., (th - h) / 2., w, h]
    }
}

/// The source in the domain it is resampled in: linear BT.2020 light in
/// nits for HDR, the sRGB codes for SDR.
fn resample_domain(spec: &Spec, rgb: [f64; 3]) -> [f64; 3] {
    match (spec.transfer, spec.source) {
        (Transfer::Sdr, Source::Bgra8) => rgb,
        // An HDR desktop on an SDR stream: SDR white (80 nits) is 1.0.
        (Transfer::Sdr, Source::ScRgbF16) => each(rgb, |c| srgb_encode(c.clamp(0., 1.))),
        (Transfer::Sdr, Source::Pq10) => {
            let linear = apply(&BT2020_TO_709, each(rgb, |c| depq(c) * 125.));
            each(linear, |c| srgb_encode(c.clamp(0., 1.)))
        }
        (_, Source::Bgra8) => apply(&BT709_TO_2020, each(rgb, |c| srgb_decode(c) * 80.)),
        (_, Source::ScRgbF16) => apply(&BT709_TO_2020, each(rgb, |c| c * 80.)),
        (_, Source::Pq10) => each(rgb, |c| depq(c) * 10000.),
    }
}
/// The stream's nonlinear R'G'B' for a resampled value.
fn encode(spec: &Spec, value: [f64; 3]) -> [f64; 3] {
    match spec.transfer {
        Transfer::Sdr if spec.matrix == 2 => {
            let linear = apply(&BT709_TO_2020, each(value, srgb_decode));
            each(linear, bt2020_oetf)
        }
        Transfer::Sdr => each(value, |c| c.clamp(0., 1.)),
        Transfer::Pq => each(value, |c| pq(c / 10000.)),
        Transfer::Hlg => each(hlg_scene(each(value, |c| c / 1000.)), hlg_oetf),
    }
}
/// Display light in BT.2020 nits for the stream's R'G'B' (SDR white at 100).
fn decode(spec: &Spec, rgb: [f64; 3]) -> [f64; 3] {
    match spec.transfer {
        Transfer::Sdr if spec.matrix == 2 => each(rgb, |c| bt2020_eotf(c) * 100.),
        Transfer::Sdr => apply(
            &BT709_TO_2020,
            each(rgb, |c| srgb_decode(c.clamp(0., 1.)) * 100.),
        ),
        Transfer::Pq => each(rgb, |c| depq(c) * 10000.),
        Transfer::Hlg => each(hlg_display(each(rgb, hlg_inverse_oetf)), |c| c * 1000.),
    }
}
fn weights(spec: &Spec) -> [f64; 3] {
    match spec.matrix {
        2 => LUMA_2020,
        0 => [0.299, 0.587, 0.114],
        _ => [0.2126, 0.7152, 0.0722],
    }
}
fn luma_code(spec: &Spec, rgb: [f64; 3]) -> f64 {
    let y = dot(rgb, weights(spec));
    let max = spec.peak();
    let scale = 1u32 << (spec.bits() - 8);
    if spec.full_range {
        max * y
    } else {
        f64::from(16 * scale) + f64::from(219 * scale) * y
    }
}
fn chroma_codes(spec: &Spec, rgb: [f64; 3]) -> (f64, f64) {
    let k = weights(spec);
    let y = dot(rgb, k);
    let (u, v) = (
        (rgb[2] - y) / (2. * (1. - k[2])),
        (rgb[0] - y) / (2. * (1. - k[0])),
    );
    let scale = 1u32 << (spec.bits() - 8);
    let mid = f64::from(128 * scale);
    let span = if spec.full_range {
        spec.peak()
    } else {
        f64::from(224 * scale)
    };
    (mid + span * u, mid + span * v)
}
/// R'G'B' for codes, the inverse of `luma_code` and `chroma_codes`.
fn rgb_from_codes(spec: &Spec, y: f64, cb: f64, cr: f64) -> [f64; 3] {
    let k = weights(spec);
    let scale = f64::from(1u32 << (spec.bits() - 8));
    let (yn, span) = if spec.full_range {
        (y / spec.peak(), spec.peak())
    } else {
        ((y - 16. * scale) / (219. * scale), 224. * scale)
    };
    let mid = 128. * scale;
    let (u, v) = ((cb - mid) / span, (cr - mid) / span);
    let r = yn + 2. * (1. - k[0]) * v;
    let b = yn + 2. * (1. - k[2]) * u;
    let g = (yn - k[0] * r - k[2] * b) / k[1];
    [r, g, b]
}

/// Exact coverage of `[left, right) x [top, bottom)` in source pixels, the
/// edges extending outward as clamped texture reads do.
fn coverage(picture: &[[f64; 3]], width: usize, height: usize, rect: [f64; 4]) -> [f64; 3] {
    let [left, top, right, bottom] = rect;
    let first_x = (left.floor().max(0.) as usize).min(width - 1);
    let last_x = (right.ceil().max(1.) as usize).min(width);
    let first_y = (top.floor().max(0.) as usize).min(height - 1);
    let last_y = (bottom.ceil().max(1.) as usize).min(height);
    let mut sum = [0.; 3];
    let mut weight = 0.;
    for y in first_y..last_y {
        let start = if y == 0 { f64::NEG_INFINITY } else { y as f64 };
        let end = if y + 1 == height {
            f64::INFINITY
        } else {
            y as f64 + 1.
        };
        let wy = (bottom.min(end) - top.max(start)).max(0.);
        if wy == 0. {
            continue;
        }
        for x in first_x..last_x {
            let start = if x == 0 { f64::NEG_INFINITY } else { x as f64 };
            let end = if x + 1 == width {
                f64::INFINITY
            } else {
                x as f64 + 1.
            };
            let w = (right.min(end) - left.max(start)).max(0.) * wy;
            let p = picture[y * width + x];
            for c in 0..3 {
                sum[c] += p[c] * w;
            }
            weight += w;
        }
    }
    if weight == 0. {
        return picture[first_y * width + first_x];
    }
    sum.map(|s| s / weight)
}
fn bilinear(picture: &[[f64; 3]], width: usize, height: usize, x: f64, y: f64) -> [f64; 3] {
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let at = |x: f64, y: f64| {
        picture[y.clamp(0., (height - 1) as f64) as usize * width
            + x.clamp(0., (width - 1) as f64) as usize]
    };
    let mut out = [0.; 3];
    for (c, value) in out.iter_mut().enumerate() {
        *value = (at(x0, y0)[c] * (1. - fx) + at(x0 + 1., y0)[c] * fx) * (1. - fy)
            + (at(x0, y0 + 1.)[c] * (1. - fx) + at(x0 + 1., y0 + 1.)[c] * fx) * fy;
    }
    out
}

/// The stream's R'G'B' at every target pixel.
pub fn nonlinear(source: &Picture, spec: &Spec) -> Vec<[f64; 3]> {
    let domain: Vec<_> = source
        .rgb
        .iter()
        .map(|&p| resample_domain(spec, p))
        .collect();
    let [ox, oy, cw, ch] = placement((source.width, source.height), (spec.width, spec.height));
    let (sx, sy) = (cw / source.width as f64, ch / source.height as f64);
    let black = encode(spec, [0.; 3]);
    let mut out = Vec::with_capacity(spec.width * spec.height);
    for ty in 0..spec.height {
        for tx in 0..spec.width {
            let (cx, cy) = (tx as f64 + 0.5 - ox, ty as f64 + 0.5 - oy);
            if cx < 0. || cy < 0. || cx >= cw || cy >= ch {
                out.push(black);
                continue;
            }
            let value = if sx >= 1. && sy >= 1. {
                bilinear(
                    &domain,
                    source.width,
                    source.height,
                    cx / sx - 0.5,
                    cy / sy - 0.5,
                )
            } else {
                coverage(
                    &domain,
                    source.width,
                    source.height,
                    [
                        (cx - 0.5) / sx,
                        (cy - 0.5) / sy,
                        (cx + 0.5) / sx,
                        (cy + 0.5) / sy,
                    ],
                )
            };
            out.push(encode(spec, value));
        }
    }
    out
}

/// The ideal, unrounded codes for `source` streamed as `spec`.
pub fn reference(source: &Picture, spec: &Spec) -> Planes {
    let rgb = nonlinear(source, spec);
    planes_from_rgb(&rgb, spec)
}
/// 4:2:0 codes for target-sized R'G'B', with the left-sited chroma filter.
pub fn planes_from_rgb(rgb: &[[f64; 3]], spec: &Spec) -> Planes {
    let (w, h) = (spec.width, spec.height);
    let y = rgb.iter().map(|&p| luma_code(spec, p)).collect();
    let (cw, ch) = (w / 2, h / 2);
    let mut cb = Vec::with_capacity(cw * ch);
    let mut cr = Vec::with_capacity(cw * ch);
    for j in 0..ch {
        for i in 0..cw {
            let (x, y0) = (i * 2, j * 2);
            let mut sum = [0.; 3];
            for (dx, weight) in [(-1i64, 1.), (0, 2.), (1, 1.)] {
                let column = (x as i64 + dx).clamp(0, w as i64 - 1) as usize;
                for row in [y0, (y0 + 1).min(h - 1)] {
                    let p = rgb[row * w + column];
                    for c in 0..3 {
                        sum[c] += p[c] * weight;
                    }
                }
            }
            let (u, v) = chroma_codes(spec, sum.map(|s| s / 8.));
            cb.push(u);
            cr.push(v);
        }
    }
    Planes {
        width: w,
        height: h,
        y,
        cb,
        cr,
    }
}

/// Peak signal-to-noise ratio in dB; infinite for identical planes.
pub fn psnr(observed: &[f64], reference: &[f64], peak: f64) -> f64 {
    let n = observed.len().min(reference.len());
    if n == 0 {
        return f64::NAN;
    }
    let mse = observed
        .iter()
        .zip(reference)
        .map(|(a, b)| (a - b) * (a - b))
        .sum::<f64>()
        / n as f64;
    if mse == 0. {
        f64::INFINITY
    } else {
        10. * (peak * peak / mse).log10()
    }
}

/// Mean SSIM over 8x8 windows every 4 pixels (Wang et al. 2004, uniform
/// window), with the usual constants for `peak`.
pub fn ssim(observed: &[f64], reference: &[f64], width: usize, height: usize, peak: f64) -> f64 {
    if width < 8
        || height < 8
        || observed.len() < width * height
        || reference.len() < width * height
    {
        return f64::NAN;
    }
    let c1 = (0.01 * peak).powi(2);
    let c2 = (0.03 * peak).powi(2);
    let mut total = 0.;
    let mut windows = 0usize;
    let mut y = 0;
    while y + 8 <= height {
        let mut x = 0;
        while x + 8 <= width {
            let (mut sa, mut sb, mut saa, mut sbb, mut sab) = (0., 0., 0., 0., 0.);
            for row in y..y + 8 {
                for column in x..x + 8 {
                    let a = observed[row * width + column];
                    let b = reference[row * width + column];
                    sa += a;
                    sb += b;
                    saa += a * a;
                    sbb += b * b;
                    sab += a * b;
                }
            }
            let n = 64.;
            let (ma, mb) = (sa / n, sb / n);
            let va = (saa / n - ma * ma).max(0.);
            let vb = (sbb / n - mb * mb).max(0.);
            let cov = sab / n - ma * mb;
            total += ((2. * ma * mb + c1) * (2. * cov + c2))
                / ((ma * ma + mb * mb + c1) * (va + vb + c2));
            windows += 1;
            x += 4;
        }
        y += 4;
    }
    total / windows as f64
}

/// ICtCp (ITU-R BT.2100, PQ) of BT.2020 display light in nits.
pub fn ictcp(nits: [f64; 3]) -> [f64; 3] {
    let [r, g, b] = nits.map(|c| c.max(0.) / 10000.);
    let l = (1688. * r + 2146. * g + 262. * b) / 4096.;
    let m = (683. * r + 2951. * g + 462. * b) / 4096.;
    let s = (99. * r + 309. * g + 3688. * b) / 4096.;
    let (l, m, s) = (pq(l), pq(m), pq(s));
    [
        0.5 * l + 0.5 * m,
        (6610. * l - 13613. * m + 7003. * s) / 4096.,
        (17933. * l - 17390. * m - 543. * s) / 4096.,
    ]
}
/// ΔE ITP (ITU-R BT.2124) between two colours in ICtCp; 1 is about one
/// just-noticeable difference.
pub fn delta_e_itp(a: [f64; 3], b: [f64; 3]) -> f64 {
    let di = a[0] - b[0];
    let dt = 0.5 * (a[1] - b[1]);
    let dp = a[2] - b[2];
    720. * (di * di + dt * dt + dp * dp).sqrt()
}

/// Mean, 99th percentile and largest ΔE ITP between the colours two sets of
/// planes decode to, each chroma sample shared by its 2x2 luma block.
pub fn delta_e(observed: &Planes, reference: &Planes, spec: &Spec) -> [f64; 3] {
    let (w, h) = (spec.width, spec.height);
    let mut values = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let c = (y / 2) * (w / 2) + x / 2;
            let colour = |p: &Planes| {
                ictcp(decode(
                    spec,
                    rgb_from_codes(spec, p.y[y * w + x], p.cb[c], p.cr[c]),
                ))
            };
            values.push(delta_e_itp(colour(observed), colour(reference)));
        }
    }
    summarize(&mut values)
}
/// Mean, 99th percentile and largest of `values`.
pub fn summarize(values: &mut [f64]) -> [f64; 3] {
    if values.is_empty() {
        return [f64::NAN; 3];
    }
    values.sort_by(|a, b| a.total_cmp(b));
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let p99 = values[((values.len() - 1) as f64 * 0.99).round() as usize];
    [mean, p99, values[values.len() - 1]]
}

/// A source whose stream R'G'B' rises linearly across the picture's width
/// (`horizontal`) or height, so each chroma sample's value says where it was
/// taken. Native size only: no resampling between source and stream.
pub fn ramp(spec: &Spec, horizontal: bool) -> Picture {
    let (w, h) = (spec.width, spec.height);
    let mut rgb = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let t = if horizontal {
                (x as f64 + 0.5) / w as f64
            } else {
                (y as f64 + 0.5) / h as f64
            };
            rgb.push(source_for(spec, ramp_value(spec, t)));
        }
    }
    Picture {
        width: w,
        height: h,
        rgb,
    }
}
/// The ramp's stream R'G'B' at `t` (0 to 1 across the picture): red rising
/// and blue falling, so Cr rises and Cb falls in every matrix.
pub fn ramp_value(spec: &Spec, t: f64) -> [f64; 3] {
    let low = if spec.transfer == Transfer::Sdr {
        0.2
    } else {
        0.3
    };
    let span = 0.4;
    [low + span * t, 0.45, low + span * (1. - t)]
}
/// The source value whose stream R'G'B' is `target` at native size.
pub fn source_for(spec: &Spec, target: [f64; 3]) -> [f64; 3] {
    let domain = match spec.transfer {
        Transfer::Sdr if spec.matrix == 2 => {
            let linear = apply(&BT2020_TO_709, each(target, bt2020_eotf));
            each(linear, srgb_encode)
        }
        Transfer::Sdr => target,
        Transfer::Pq => each(target, |c| depq(c) * 10000.),
        Transfer::Hlg => {
            let scene = each(target, hlg_inverse_oetf);
            each(hlg_display(scene), |c| c * 1000.)
        }
    };
    match (spec.transfer, spec.source) {
        (Transfer::Sdr, Source::Bgra8) => domain,
        (Transfer::Sdr, Source::ScRgbF16) => each(domain, srgb_decode),
        (Transfer::Sdr, Source::Pq10) => {
            let nits = apply(&BT709_TO_2020, each(domain, |c| srgb_decode(c) * 80.));
            each(nits, |c| pq(c / 10000.))
        }
        (_, Source::Pq10) => each(domain, |c| pq(c / 10000.)),
        (_, Source::ScRgbF16) => apply(&BT2020_TO_709, each(domain, |c| c / 80.)),
        (_, Source::Bgra8) => apply(&BT2020_TO_709, each(domain, |c| c / 80.)).map(srgb_encode),
    }
}
/// Where a converter takes chroma, from the chroma row (or column) of a
/// ramp: the median distance in luma samples of each sample's position from
/// luma sample `2i`. 0 is co-sited (left or top); 0.5 is centred between
/// two luma samples. `code_at` is the ideal chroma code at a luma position.
pub fn siting(observed: &[f64], code_at: impl Fn(f64) -> f64) -> Option<f64> {
    let (a, b) = (code_at(0.), code_at(1.) - code_at(0.));
    if b.abs() < 1e-9 || observed.len() < 8 {
        return None;
    }
    // Skip the edges, where filters clamp.
    let mut offsets: Vec<f64> = observed
        .iter()
        .enumerate()
        .skip(2)
        .take(observed.len().saturating_sub(4))
        .map(|(i, &value)| (value - a) / b - 2. * i as f64)
        .collect();
    offsets.sort_by(|x, y| x.total_cmp(y));
    Some(offsets[offsets.len() / 2])
}
/// The ideal chroma codes of a ramp at a luma position (`x` along the ramp,
/// sample centres at whole numbers), for [`siting`]: Cb and Cr.
pub fn ramp_chroma(spec: &Spec, horizontal: bool, position: f64) -> (f64, f64) {
    let extent = if horizontal { spec.width } else { spec.height } as f64;
    chroma_codes(spec, ramp_value(spec, (position + 0.5) / extent))
}

#[cfg(test)]
mod tests;
