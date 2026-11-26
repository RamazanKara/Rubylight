//! scRGB uses linear Rec.709 primaries and 1.0 = 80 nits. HDR encoders take
//! nonlinear Rec.2020 PQ. Resize in linear light before subsampling.
use crate::capture::{Image, Pixel};
use rayon::prelude::*;
use std::sync::OnceLock;

fn pq(nits: f32) -> f32 {
    let y = (nits.max(0.) / 10000.).powf(2610. / 16384.);
    ((3424. / 4096. + 2413. / 128. * y) / (1. + 2392. / 128. * y)).powf(2523. / 32.)
}
/// color.hlsl's `hlg` on the CPU, for the tests: BT.2020 display light in
/// scRGB units (1.0 = 80 nits) to the BT.2100 HLG signal, graded for a fixed
/// 1000-nit display (inverse reference OOTF at system gamma 1.2, then the
/// ARIB STD-B67 OETF).
#[cfg(test)]
pub(crate) fn hlg(light: [f64; 3]) -> [f64; 3] {
    const A: f64 = 0.17883277;
    let display = light.map(|v| v.max(0.) * 80. / 1000.);
    let y = 0.2627 * display[0] + 0.6780 * display[1] + 0.0593 * display[2];
    display.map(|v| {
        let scene = if y > 0. {
            v * y.powf((1. - 1.2) / 1.2)
        } else {
            0.
        };
        if scene <= 1. / 12. {
            (3. * scene).sqrt()
        } else {
            A * (12. * scene - (1. - 4. * A)).ln() + 0.5 - A * (4. * A).ln()
        }
    })
}
/// The inverse of [`hlg`], color.hlsl's `hlgLight`: scRGB display light for
/// an HLG signal (inverse OETF, then the reference OOTF for 1000 nits).
#[cfg(test)]
pub(crate) fn hlg_light(signal: [f64; 3]) -> [f64; 3] {
    let scene = signal.map(|v| {
        let v = v.max(0.);
        if v <= 0.5 {
            v * v / 3.
        } else {
            (((v - 0.55991073) / 0.17883277).exp() + 0.28466892) / 12.
        }
    });
    let y = 0.2627 * scene[0] + 0.6780 * scene[1] + 0.0593 * scene[2];
    if y <= 0. {
        return [0.; 3];
    }
    scene.map(|v| v * y.powf(0.2) * 1000. / 80.)
}
/// color.hlsl's `method_b` on the CPU, for the tests: scRGB (Rec.709, 1.0 =
/// 80 nits) to the SDR signal in the primaries of `color_matrix` (0 SMPTE
/// 170M, 1 BT.709, 2 BT.2020). ITU-R BT.2446-1 Method B, ported from
/// Foundation Sunshine's MethodBScRgbToSdr and LimitSdrGamut
/// (AlkaidLab/foundation-sunshine, commit 9d761ba, PR #1387). Copyright (c)
/// Foundation Sunshine authors, GPL-3.0. For a 1000-nit source, as
/// section 5.2.1 has it; [`method_b_peak`] takes the source's peak.
#[cfg(test)]
pub(crate) fn method_b(light: [f64; 3], color_matrix: u8) -> [f64; 3] {
    method_b_peak(light, color_matrix, 1000.)
}
/// [`method_b`] for a source of `peak` nits: the HLG reference OOTF it
/// inverts is a `peak`-nit display's, at BT.2100's system gamma
/// 1.2 + 0.42 log10(peak / 1000), so `peak` maps to SDR white.
#[cfg(test)]
pub(crate) fn method_b_peak(light: [f64; 3], color_matrix: u8, peak: f64) -> [f64; 3] {
    type Matrix = [[f64; 3]; 3];
    const RGB709_TO_LMS: Matrix = [
        [0.295809355, 0.623101305, 0.081089341],
        [0.156248976, 0.727325725, 0.116425299],
        [0.035135663, 0.156574727, 0.808289610],
    ];
    const LMS_TO_709: Matrix = [
        [6.173362742, -5.320380568, 0.147017826],
        [-1.324311500, 2.560226421, -0.235914921],
        [-0.011816903, -0.264672035, 1.276488938],
    ];
    const LMS_TO_SMPTE_C: Matrix = [
        [6.650457157, -5.807419747, 0.156962590],
        [-1.493468713, 2.762204921, -0.268736208],
        [-0.007513590, -0.260458703, 1.267972293],
    ];
    const TO_SMPTE_C: Matrix = [
        [1.065379, -0.055401, -0.009978],
        [-0.019633, 1.036363, -0.016730],
        [0.001632, 0.004412, 0.993956],
    ];
    fn mul(m: Matrix, v: [f64; 3]) -> [f64; 3] {
        m.map(|row| row[0] * v[0] + row[1] * v[1] + row[2] * v[2])
    }
    fn encode(nits: f64) -> f64 {
        let p = (nits.clamp(0., 10000.) / 10000.).powf(2610. / 16384.);
        ((3424. / 4096. + 2413. / 128. * p) / (1. + 2392. / 128. * p)).powf(2523. / 32.)
    }
    fn decode(signal: f64) -> f64 {
        let p = signal.clamp(0., 1.).powf(32. / 2523.);
        ((p - 3424. / 4096.).max(0.) / (2413. / 128. - 2392. / 128. * p)).powf(16384. / 2610.)
            * 10000.
    }
    let inside = |c: [f64; 3]| c.iter().all(|v| (0. ..=1.).contains(v));
    // Chroma reduction at constant ICtCp intensity and hue (section 5.2.4).
    let limit = |rgb709: [f64; 3], target: [f64; 3], to_target: Matrix| {
        if inside(target) {
            return target;
        }
        if target.iter().all(|v| *v >= 1.) {
            return [1.; 3];
        }
        let lms = mul(RGB709_TO_LMS, rgb709).map(|v| encode(v * 100.));
        let intensity = (lms[0] + lms[1]) * 0.5;
        if intensity >= 0.50807842 {
            return [1.; 3];
        }
        let (mut lo, mut hi, mut outer) = (0., 1., target);
        let mut inner = [decode(intensity) / 100.; 3];
        for _ in 0..5 {
            let mid = (lo + hi) * 0.5;
            let linear = lms.map(|v| decode(intensity + (v - intensity) * mid) / 100.);
            let candidate = mul(to_target, linear);
            if inside(candidate) {
                (lo, inner) = (mid, candidate);
            } else {
                (hi, outer) = (mid, candidate);
            }
        }
        if hi == 1. && mul(RGB709_TO_LMS, rgb709).iter().any(|v| *v < 0.) {
            return target.map(|v| v.clamp(0., 1.));
        }
        // Where the chord from inside to outside meets the cube.
        let reach = (0..3).fold(1.0_f64, |reach, i| {
            let chord = outer[i] - inner[i];
            if outer[i] > 1. {
                reach.min((1. - inner[i]) / chord)
            } else if outer[i] < 0. {
                reach.min(-inner[i] / chord)
            } else {
                reach
            }
        });
        std::array::from_fn(|i| {
            (inner[i] + (outer[i] - inner[i]) * reach.clamp(0., 1.)).clamp(0., 1.)
        })
    };
    let y = 0.212639 * light[0] + 0.715169 * light[1] + 0.072192 * light[2];
    if y <= 0. {
        return [0.; 3];
    }
    let system_gamma = 1.2 + 0.42 * (peak / 1000.).log10();
    let bridge = 291. * (y * 80. / peak).powf(1. / system_gamma);
    let (knee, width) = (55.0842, 16.44899);
    let nits = if bridge <= knee {
        bridge
    } else {
        knee + width * (1. + (bridge - knee) / width).ln()
    };
    let mapped = light.map(|v| v * nits / 100. / y);
    let mut sdr = limit(mapped, mapped, LMS_TO_709);
    if color_matrix == 0 {
        sdr = limit(sdr, mul(TO_SMPTE_C, sdr), LMS_TO_SMPTE_C);
    } else if color_matrix == 2 {
        sdr = mul(BT709_TO_2020, sdr);
    }
    sdr.map(|v| v.max(0.).powf(1. / 2.4))
}
/// color.hlsl's `gamut2020` and `gamut709`.
#[cfg(test)]
const BT709_TO_2020: [[f64; 3]; 3] = [
    [0.627404, 0.329283, 0.043313],
    [0.069097, 0.919540, 0.011362],
    [0.016391, 0.088013, 0.895595],
];
#[cfg(test)]
const BT2020_TO_709: [[f64; 3]; 3] = [
    [1.660491, -0.587641, -0.072850],
    [-0.124550, 1.132900, -0.008349],
    [-0.018151, -0.100579, 1.118730],
];
/// color.hlsl's `load` for a tone-mapped PQ capture on the CPU, for the
/// tests: 10-bit PQ codes of BT.2020 light to scRGB (`gamut709(depq(rgb) *
/// 125)`), which [`method_b`] then maps as it does an FP16 capture.
#[cfg(test)]
pub(crate) fn pq_scrgb(codes: [u16; 3]) -> [f64; 3] {
    let light = codes.map(|code| {
        let p = (f64::from(code) / 1023.).powf(32. / 2523.);
        ((p - 3424. / 4096.).max(0.) / (2413. / 128. - 2392. / 128. * p)).powf(16384. / 2610.)
            * 125.
    });
    BT2020_TO_709.map(|row| row[0] * light[0] + row[1] * light[1] + row[2] * light[2])
}
/// The 10-bit PQ codes of scRGB `light` in BT.2020, as an HDR10 desktop
/// hands them over; light outside BT.2020 clips.
#[cfg(test)]
pub(crate) fn scrgb_pq10(light: [f32; 3]) -> [u16; 3] {
    let light = light.map(f64::from);
    BT709_TO_2020.map(|row| {
        let nits = (row[0] * light[0] + row[1] * light[1] + row[2] * light[2]) * 80.;
        let p = (nits.clamp(0., 10000.) / 10000.).powf(2610. / 16384.);
        let signal =
            ((3424. / 4096. + 2413. / 128. * p) / (1. + 2392. / 128. * p)).powf(2523. / 32.);
        (signal * 1023.).round() as u16
    })
}
pub(crate) fn pointer_linear(value: u8) -> f32 {
    srgb_table()[value as usize] * 1.25
}
pub(crate) fn pointer_srgb(linear: f32) -> f32 {
    let value = (linear / 1.25).clamp(0., 1.);
    if value <= 0.0031308 {
        value * 12.92
    } else {
        1.055 * value.powf(1. / 2.4) - 0.055
    }
}
pub(crate) fn pointer_read(bytes: &[u8], pixel: Pixel) -> [f32; 3] {
    if pixel == Pixel::RgbaF16 {
        std::array::from_fn(|i| {
            half::f16::from_le_bytes(bytes[i * 2..i * 2 + 2].try_into().unwrap()).to_f32()
        })
    } else {
        let packed = u32::from_le_bytes(bytes.try_into().unwrap());
        let rgb: [f32; 3] = std::array::from_fn(|i| {
            let p = (((packed >> (i * 10)) & 1023) as f32 / 1023.).powf(32. / 2523.);
            ((p - 3424. / 4096.).max(0.) / (2413. / 128. - 2392. / 128. * p).max(0.000001))
                .powf(16384. / 2610.)
                * 125.
        });
        [
            1.660491 * rgb[0] - 0.587641 * rgb[1] - 0.072850 * rgb[2],
            -0.124550 * rgb[0] + 1.1329 * rgb[1] - 0.008349 * rgb[2],
            -0.018151 * rgb[0] - 0.100579 * rgb[1] + 1.11873 * rgb[2],
        ]
    }
}
pub(crate) fn pointer_write(bytes: &mut [u8], pixel: Pixel, rgb: [f32; 3]) {
    if pixel == Pixel::RgbaF16 {
        for i in 0..3 {
            bytes[i * 2..i * 2 + 2].copy_from_slice(&half::f16::from_f32(rgb[i]).to_le_bytes());
        }
    } else {
        let rgb = [
            0.627404 * rgb[0] + 0.329283 * rgb[1] + 0.043313 * rgb[2],
            0.069097 * rgb[0] + 0.919540 * rgb[1] + 0.011362 * rgb[2],
            0.016391 * rgb[0] + 0.088013 * rgb[1] + 0.895595 * rgb[2],
        ];
        let mut packed = u32::from_le_bytes(bytes.try_into().unwrap()) & 0xc0000000;
        for (i, value) in rgb.into_iter().enumerate() {
            packed |= ((pq(value * 80.).clamp(0., 1.) * 1023.).round() as u32) << (i * 10);
        }
        bytes.copy_from_slice(&packed.to_le_bytes());
    }
}
fn pq_table() -> &'static [u16] {
    static TABLE: OnceLock<Vec<u16>> = OnceLock::new();
    TABLE.get_or_init(|| {
        (0..=65535)
            .map(|i| {
                let y = i as f32 / 65535.;
                (pq(y * y * 10000.) * 65535. + 0.5) as u16
            })
            .collect()
    })
}
fn code(linear: f32, table: &[u16]) -> u16 {
    let index = ((linear.clamp(0., 125.) / 125.).sqrt() * 65535. + 0.5) as usize;
    table[index.min(65535)]
}
fn half_table() -> &'static [f32] {
    static TABLE: OnceLock<Vec<f32>> = OnceLock::new();
    TABLE.get_or_init(|| {
        (0..=65535u16)
            .map(|bits| {
                let value = half::f16::from_bits(bits).to_f32();
                if value.is_finite() { value } else { 0. }
            })
            .collect()
    })
}
fn srgb_table() -> &'static [f32; 256] {
    static TABLE: OnceLock<[f32; 256]> = OnceLock::new();
    TABLE.get_or_init(|| {
        std::array::from_fn(|i| {
            let value = i as f32 / 255.;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        })
    })
}
fn pool() -> Option<&'static rayon::ThreadPool> {
    static POOL: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();
    POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(std::thread::available_parallelism().map_or(1, |n| n.get().min(8)))
            .thread_name(|index| format!("hdr-color-{index}"))
            .build()
            .ok()
    })
    .as_ref()
}
#[inline]
fn sample<const FORMAT: u8>(
    bytes: &[u8],
    offset: usize,
    half: &[f32],
    srgb: &[f32; 256],
) -> [f32; 3] {
    match FORMAT {
        0 => [
            srgb[bytes[offset + 2] as usize],
            srgb[bytes[offset + 1] as usize],
            srgb[bytes[offset] as usize],
        ],
        1 => std::array::from_fn(|channel| {
            let at = offset + channel * 2;
            half[u16::from_le_bytes([bytes[at], bytes[at + 1]]) as usize]
        }),
        _ => {
            let value = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
            [
                (value & 1023) as f32 / 1023.,
                ((value >> 10) & 1023) as f32 / 1023.,
                ((value >> 20) & 1023) as f32 / 1023.,
            ]
        }
    }
}
fn convert<const FORMAT: u8>(
    image: &Image,
    width: u32,
    height: u32,
    luminance: [f32; 2],
    out: &mut [u8],
) {
    let stride = width as usize * 8;
    let pixel_bytes = if FORMAT == 1 { 8 } else { 4 };
    let half = half_table();
    let srgb = srgb_table();
    let pq = pq_table();
    let columns: Vec<_> = (0..width as usize)
        .map(|x| {
            let source = ((x as f32 + 0.5) * image.width as f32 / width as f32 - 0.5).max(0.);
            let left = (source as usize).min(image.width as usize - 1);
            (
                left * pixel_bytes,
                (left + 1).min(image.width as usize - 1) * pixel_bytes,
                source - left as f32,
            )
        })
        .collect();
    let row = |(y, output): (usize, &mut [u8])| {
        let source = ((y as f32 + 0.5) * image.height as f32 / height as f32 - 0.5).max(0.);
        let top = (source as usize).min(image.height as usize - 1);
        let fy = source - top as f32;
        let a_row = top * image.stride;
        let b_row = (top + 1).min(image.height as usize - 1) * image.stride;
        for (&(x0, x1, fx), target) in columns.iter().zip(output.as_chunks_mut::<8>().0.iter_mut())
        {
            let a = sample::<FORMAT>(&image.bytes, a_row + x0, half, srgb);
            let rgb = if fx == 0. && fy == 0. {
                a
            } else {
                let b = sample::<FORMAT>(&image.bytes, a_row + x1, half, srgb);
                let c = sample::<FORMAT>(&image.bytes, b_row + x0, half, srgb);
                let d = sample::<FORMAT>(&image.bytes, b_row + x1, half, srgb);
                std::array::from_fn(|i| {
                    (a[i] + (b[i] - a[i]) * fx) * (1. - fy) + (c[i] + (d[i] - c[i]) * fx) * fy
                })
            };
            let rgb = if FORMAT == 2 {
                rgb.map(|value| (value * 65535. + 0.5) as u16)
            } else {
                let factor = if FORMAT == 0 {
                    luminance[0] / 80.
                } else {
                    luminance[1]
                };
                let [r, g, b] = rgb.map(|v| v * factor);
                [
                    code(0.627404 * r + 0.329283 * g + 0.043313 * b, pq),
                    code(0.069097 * r + 0.919540 * g + 0.011362 * b, pq),
                    code(0.016391 * r + 0.088013 * g + 0.895595 * b, pq),
                ]
            };
            target[..2].copy_from_slice(&rgb[0].to_le_bytes());
            target[2..4].copy_from_slice(&rgb[1].to_le_bytes());
            target[4..6].copy_from_slice(&rgb[2].to_le_bytes());
            target[6..8].copy_from_slice(&u16::MAX.to_le_bytes());
        }
    };
    if u64::from(width) * u64::from(height) >= 262144
        && let Some(pool) = pool()
    {
        pool.install(|| out.par_chunks_exact_mut(stride).enumerate().for_each(row));
    } else {
        out.chunks_exact_mut(stride).enumerate().for_each(row);
    }
}
pub fn hdr_rgba(image: &Image, out: &mut Vec<u8>) {
    hdr_rgba_scaled(image, image.width, image.height, out);
}
pub fn hdr_rgba_scaled(image: &Image, width: u32, height: u32, out: &mut Vec<u8>) {
    hdr_rgba_scaled_luminance(image, width, height, [100., 1.], out);
}
pub fn hdr_rgba_scaled_luminance(
    image: &Image,
    width: u32,
    height: u32,
    luminance: [f32; 2],
    out: &mut Vec<u8>,
) {
    if image.width == 0 || image.height == 0 || width == 0 || height == 0 {
        out.clear();
        return;
    }
    out.resize(width as usize * height as usize * 8, 0);
    match image.pixel {
        Pixel::Bgra8 => convert::<0>(image, width, height, luminance, out),
        Pixel::RgbaF16 => convert::<1>(image, width, height, luminance, out),
        Pixel::Rgba10Pq => convert::<2>(image, width, height, luminance, out),
    }
}
/// scRGB patches and their BT.709 codes, from Foundation Sunshine's tests:
/// nine greys (0 to 4000 nits), then seven colours.
#[cfg(test)]
pub(crate) const METHOD_B_PATCHES: [([f32; 3], [u8; 3]); 16] = [
    ([0., 0., 0.], [16, 128, 128]),
    ([0.25, 0.25, 0.25], [104, 128, 128]),
    ([0.5, 0.5, 0.5], [128, 128, 128]),
    ([1., 1., 1.], [158, 128, 128]),
    ([2.5, 2.5, 2.5], [203, 128, 128]),
    ([2.537, 2.537, 2.537], [204, 128, 128]),
    ([5., 5., 5.], [220, 128, 128]),
    ([12.5, 12.5, 12.5], [235, 128, 128]),
    ([50., 50., 50.], [235, 128, 128]),
    ([1., 0., 0.], [50, 109, 209]),
    ([0., 1., 0.], [120, 71, 60]),
    ([0., 0., 1.], [28, 215, 120]),
    ([2., 0.75, 0.25], [153, 97, 163]),
    ([2., -0.125, -0.03125], [56, 110, 222]),
    ([-0.375, 1.5, -0.125], [134, 82, 51]),
    ([10., 1., 5.], [214, 131, 142]),
];
#[cfg(test)]
const METHOD_B_SMPTE_C: [[u8; 3]; 7] = [
    [66, 112, 207],
    [106, 96, 62],
    [61, 178, 95],
    [157, 93, 167],
    [74, 109, 220],
    [117, 98, 55],
    [216, 130, 142],
];
#[cfg(test)]
const METHOD_B_BT2020: [[u8; 3]; 7] = [
    [87, 105, 169],
    [138, 90, 107],
    [56, 195, 132],
    [155, 101, 149],
    [99, 101, 176],
    [152, 89, 104],
    [214, 131, 136],
];
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    #[test]
    fn pq_matches_hdr10_reference_points() {
        assert!((pq(100.) - 0.5080784).abs() < 0.00002);
        assert!((pq(1000.) - 0.7518271).abs() < 0.00002);
        assert!((pq(10000.) - 1.).abs() < 0.00002);
        assert!((f32::from(code(1., pq_table())) / 65535. - pq(80.)).abs() < 0.0001);
    }
    /// 8-bit limited-range codes of an SDR signal in `matrix` (0 BT.601,
    /// 1 BT.709, 2 BT.2020).
    fn limited_codes(rgb: [f64; 3], matrix: usize) -> [f64; 3] {
        let (kr, kb) = [(0.299, 0.114), (0.2126, 0.0722), (0.2627, 0.0593)][matrix];
        let y = kr * rgb[0] + (1. - kr - kb) * rgb[1] + kb * rgb[2];
        [
            16. + 219. * y,
            128. + 224. * (rgb[2] - y) / (2. * (1. - kb)),
            128. + 224. * (rgb[0] - y) / (2. * (1. - kr)),
        ]
        .map(f64::round)
    }
    #[test]
    fn method_b_of_an_hdr10_capture_matches_the_fp16_codes() {
        // The same desktop as 10-bit PQ: Sunshine's codes within one for the
        // greys and the colours inside the cube. The BT.709 primaries (9 to
        // 11) and the colour outside BT.2020 (13) sit on the cube's edge,
        // where a PQ code's error of 1e-4 moves the gamut limiting by up to
        // 25 codes; the GPU tests hold those to `method_b` of the PQ light.
        for (patch, (light, codes)) in METHOD_B_PATCHES.iter().enumerate() {
            if (9..=11).contains(&patch) || patch == 13 {
                continue;
            }
            let pq = pq_scrgb(scrgb_pq10(*light));
            for matrix in [1, 0, 2] {
                let wanted = match matrix {
                    1 => *codes,
                    _ if patch < 9 => continue,
                    0 => METHOD_B_SMPTE_C[patch - 9],
                    _ => METHOD_B_BT2020[patch - 9],
                };
                let actual = limited_codes(method_b(pq, matrix as u8), matrix);
                for (actual, wanted) in actual.into_iter().zip(wanted) {
                    assert!(
                        (actual - f64::from(wanted)).abs() <= 1.,
                        "matrix {matrix} {light:?}: {actual}, wanted {wanted}"
                    );
                }
            }
        }
        // PQ round trip: 10-bit codes keep scRGB within a code's step.
        let grey = pq_scrgb(scrgb_pq10([1.; 3]));
        assert!(grey.iter().all(|v| (v - 1.).abs() < 0.01), "{grey:?}");
        assert_eq!(scrgb_pq10([0.; 3]), [0; 3]);
        assert_eq!(scrgb_pq10([125.; 3]), [1023; 3]);
    }
    #[test]
    fn method_b_maps_the_sources_peak_to_sdr_white() {
        let grey = |nits: f64, peak| method_b_peak([nits / 80.; 3], 1, peak)[0];
        for light in [[0.5; 3], [2.5; 3], [2., 0.75, 0.25], [10., 1., 5.]] {
            assert_eq!(method_b_peak(light, 1, 1000.), method_b(light, 1));
        }
        for peak in [400., 600., 1000., 1500., 2000.] {
            assert!((grey(peak, peak) - 1.).abs() < 1e-4, "{peak}");
            assert!(grey(peak * 0.9, peak) < 1., "{peak}");
        }
        // A brighter display's 203-nit white is a lower signal, a dimmer
        // one's a higher, and black stays black.
        assert!(grey(203., 400.) > grey(203., 1000.));
        assert!(grey(203., 1000.) > grey(203., 2000.));
        assert!(
            (grey(203., 2000.) - 0.761).abs() < 0.002,
            "{}",
            grey(203., 2000.)
        );
        assert_eq!(method_b_peak([0.; 3], 1, 400.), [0.; 3]);
    }
    #[test]
    fn method_b_matches_its_fixed_points() {
        let grey = |nits: f64| method_b([nits / 80.; 3], 1)[0].min(1.);
        assert_eq!(method_b([0.; 3], 1), [0.; 3]);
        assert!((grey(1000.) - 1.).abs() < 1e-4);
        assert!((grey(4000.) - 1.).abs() < 1e-9);
        assert!((grey(203.) - 0.857).abs() < 0.002);
        // 8-bit limited-range codes of Foundation Sunshine's patches
        // (test_hdr_encoding.cpp, commit 9d761ba), BT.709 for all sixteen and
        // SMPTE 170M and BT.2020 for the seven colours.
        for (matrix, patches) in [
            (1, &METHOD_B_PATCHES[..]),
            (0, &METHOD_B_PATCHES[9..]),
            (2, &METHOD_B_PATCHES[9..]),
        ] {
            for (patch, (light, codes)) in patches.iter().enumerate() {
                let rgb = method_b(light.map(f64::from), matrix as u8);
                // Out-of-gamut and over-range colours land inside the cube.
                assert!(rgb.iter().all(|v| (0. ..=1.0001).contains(v)), "{rgb:?}");
                let actual = limited_codes(rgb, matrix);
                let wanted = match matrix {
                    1 => *codes,
                    0 => METHOD_B_SMPTE_C[patch],
                    _ => METHOD_B_BT2020[patch],
                };
                for (actual, wanted) in actual.into_iter().zip(wanted) {
                    assert!(
                        (actual - f64::from(wanted)).abs() <= 1.,
                        "matrix {matrix} {light:?}: {actual}, wanted {wanted}"
                    );
                }
            }
        }
    }
    #[test]
    fn hlg_matches_bt2100_and_bt2408_reference_points() {
        let grey = |nits: f64| hlg([nits / 80.; 3])[0];
        assert_eq!(hlg([0.; 3]), [0.; 3]);
        assert_eq!(hlg([-1., 0., 0.]), [0.; 3]);
        // The 1000-nit grading's peak is signal 1, and BT.2408's HDR reference
        // white, 203 nits on a 1000-nit display, is 75 %.
        assert!((grey(1000.) - 1.).abs() < 1e-6);
        assert!((grey(203.) - 0.75).abs() < 0.0005);
        // ARIB STD-B67's two segments meet at scene light 1/12, signal 0.5.
        let knee = (1f64 / 12.).powf(1.2) * 1000.;
        assert!((grey(knee) - 0.5).abs() < 1e-6);
        assert!((grey(knee * 0.999) - 0.5).abs() < 0.001);
        assert!((grey(knee * 1.001) - 0.5).abs() < 0.001);
        // Light above the grading keeps its headroom.
        assert!(grey(2000.) > 1.1);
        // The inverse OOTF scales by luminance, so a saturated 1000-nit red
        // lifts above signal 1 and keeps its hue: green and blue stay black.
        let red = hlg([12.5, 0., 0.]);
        assert!((red[0] - 1.0407).abs() < 0.0001, "{red:?}");
        assert_eq!(&red[1..], &[0., 0.]);
        // `hlg_light` undoes it, saturated colours and the headroom above the grading too.
        for light in [
            [2.5, 2.5, 2.5],
            [12.5, 0., 0.],
            [0.3, 5., 1.],
            [25., 20., 30.],
        ] {
            let back = hlg_light(hlg(light));
            assert!(
                back.iter()
                    .zip(light)
                    .all(|(b, l)| (b - l).abs() <= 1e-6 * l.max(1.)),
                "{light:?}: {back:?}"
            );
        }
    }
    #[test]
    fn half_lookup_preserves_finite_values_and_sanitizes_nonfinite() {
        for bits in 0..=65535u16 {
            let value = half::f16::from_bits(bits).to_f32();
            let expected = if value.is_finite() { value } else { 0. };
            assert_eq!(half_table()[bits as usize].to_bits(), expected.to_bits());
        }
    }
    #[test]
    fn resizing_interpolates_light_before_pq() {
        let mut bytes = Vec::new();
        for value in [0., 25.] {
            for _ in 0..3 {
                bytes.extend_from_slice(&half::f16::from_f32(value).to_bits().to_le_bytes());
            }
            bytes.extend_from_slice(&half::f16::ONE.to_bits().to_le_bytes());
        }
        let image = Image {
            width: 2,
            height: 1,
            stride: 16,
            bytes,
            captured: Instant::now(),
            pixel: Pixel::RgbaF16,
        };
        let mut output = vec![];
        hdr_rgba_scaled(&image, 1, 1, &mut output);
        let r = u16::from_le_bytes([output[0], output[1]]) as f32 / 65535.;
        assert!((r - pq(1000.)).abs() < 0.0001);
        assert!((r - pq(2000.) / 2.).abs() > 0.1);
    }
    #[test]
    fn ten_bit_pq_input_keeps_transfer_and_gamut() {
        let pixel = 512u32 | (256 << 10) | (768 << 20) | (3 << 30);
        let image = Image {
            width: 1,
            height: 1,
            stride: 4,
            bytes: pixel.to_le_bytes().to_vec(),
            captured: Instant::now(),
            pixel: Pixel::Rgba10Pq,
        };
        let mut output = vec![];
        hdr_rgba(&image, &mut output);
        for (channel, value) in [512., 256., 768.].into_iter().enumerate() {
            let actual = u16::from_le_bytes([output[channel * 2], output[channel * 2 + 1]]);
            assert!((actual as f32 - value / 1023. * 65535.).abs() <= 0.51);
        }
    }
}
