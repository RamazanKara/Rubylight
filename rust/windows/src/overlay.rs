//! The couch menu's panel as the converters draw it into the stream
//! (`docs/couch-menu-design.md` sections 6.2 and 6.3). While an overlay is
//! set, each conversion runs a second pass over the panel's blocks only, its
//! colours read from the panel image instead of the capture; closed, the
//! conversions record exactly what they did before.
use anyhow::{Result, ensure};
use butterpollo_core::couch_menu::{Frame, layout::ALIGN};
use std::sync::Arc;

/// One published panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Overlay {
    /// Rises whenever the panel changes; the converters upload it only then.
    pub revision: u64,
    /// Target pixels, multiples of [`ALIGN`].
    pub origin: [u32; 2],
    pub size: [u32; 2],
    /// Whole-number magnification of `rgba`.
    pub scale: u32,
    /// The panel at 1x, `size / scale` pixels, RGBA8 sRGB.
    pub rgba: Arc<[u8]>,
}

impl Overlay {
    pub fn new(revision: u64, frame: &Frame) -> Self {
        Self {
            revision,
            origin: frame.origin,
            size: frame.size,
            scale: frame.scale,
            rgba: frame.rgba.clone(),
        }
    }
    /// The panel image's size, `size / scale`.
    pub fn image_size(&self) -> [u32; 2] {
        self.size.map(|v| v / self.scale.max(1))
    }
    /// Refuse a panel the passes cannot draw on a `target`-sized stream:
    /// every format's blocks must lie wholly inside or outside it.
    pub fn validate(&self, target: [u32; 2]) -> Result<()> {
        ensure!(self.scale >= 1, "couch menu overlay: scale 0");
        ensure!(
            self.size
                .iter()
                .all(|&v| v > 0 && v.is_multiple_of(self.scale)),
            "couch menu overlay: size {:?} is not a nonzero multiple of scale {}",
            self.size,
            self.scale
        );
        ensure!(
            self.origin
                .iter()
                .chain(&self.size)
                .all(|v| v.is_multiple_of(ALIGN)),
            "couch menu overlay: origin {:?} or size {:?} not aligned to {ALIGN}",
            self.origin,
            self.size
        );
        ensure!(
            (0..2).all(
                |i| u64::from(self.origin[i]) + u64::from(self.size[i]) <= u64::from(target[i])
            ),
            "couch menu overlay: {:?} at {:?} does not fit the {target:?} stream",
            self.size,
            self.origin
        );
        let [width, height] = self.image_size();
        ensure!(
            self.rgba.len() as u64 == u64::from(width) * u64::from(height) * 4,
            "couch menu overlay: {} bytes for a {width}x{height} image",
            self.rgba.len()
        );
        Ok(())
    }
    /// `cbuffer Config`'s menu fields after its 20 colour values: origin,
    /// size, the first block and the block count for `block`-sized threads
    /// (2x2, or 2x1 for packing version 1), and the scale.
    pub fn constants(&self, block: [u32; 2]) -> [u32; 12] {
        [
            self.origin[0],
            self.origin[1],
            self.size[0],
            self.size[1],
            self.origin[0] / block[0],
            self.origin[1] / block[1],
            self.size[0] / block[0],
            self.size[1] / block[1],
            self.scale,
            0,
            0,
            0,
        ]
    }
}

/// What the passes should write for a panel, on the CPU, for the tests.
#[cfg(test)]
#[allow(
    dead_code,
    reason = "the overlay pass tests use it once the pass lands"
)]
pub(crate) mod reference {
    use super::Overlay;

    /// The colour fields of `cbuffer Config`.
    #[derive(Clone, Copy, Debug)]
    pub(crate) struct Colour {
        hdr: bool,
        matrix: u32,
        full_range: bool,
        ten_bit: bool,
        white: f64,
    }
    impl Colour {
        pub(crate) fn from_values(values: &[u32; 20]) -> Self {
            Self {
                hdr: values[5] != 0,
                matrix: values[6],
                full_range: values[7] != 0,
                ten_bit: values[8] != 0,
                white: f64::from(f32::from_bits(values[9])),
            }
        }
        fn maximum(self) -> f64 {
            if self.ten_bit { 1023. } else { 255. }
        }
        fn weights(self) -> [f64; 3] {
            match self.matrix {
                2 => [0.2627, 0.6780, 0.0593],
                0 => [0.299, 0.587, 0.114],
                _ => [0.2126, 0.7152, 0.0722],
            }
        }
        /// `menu_rgb`'s transform of a panel pixel.
        fn transform(self, rgb: [f64; 3]) -> [f64; 3] {
            let linear = || gamut2020(rgb.map(srgb_linear));
            if self.hdr {
                linear().map(|v| pq(v * self.white / 125.))
            } else if self.matrix == 2 {
                linear().map(bt2020_sdr)
            } else {
                rgb
            }
        }
        fn luma(self, rgb: [f64; 3]) -> u16 {
            let [kr, kg, kb] = self.weights();
            let y = kr * rgb[0] + kg * rgb[1] + kb * rgb[2];
            let code = match (self.ten_bit, self.full_range) {
                (true, true) => 1023. * y,
                (true, false) => 64. + 876. * y,
                (false, true) => 255. * y,
                (false, false) => 16. + 219. * y,
            };
            (code.clamp(0., self.maximum()) + 0.5).floor() as u16
        }
        fn chroma(self, rgb: [f64; 3]) -> [u16; 2] {
            let [kr, kg, kb] = self.weights();
            let y = kr * rgb[0] + kg * rgb[1] + kb * rgb[2];
            let uv = [
                (rgb[2] - y) / (2. * (1. - kb)),
                (rgb[0] - y) / (2. * (1. - kr)),
            ];
            let (centre, range) = match (self.ten_bit, self.full_range) {
                (true, true) => (512., 1023.),
                (true, false) => (512., 896.),
                (false, true) => (128., 255.),
                (false, false) => (128., 224.),
            };
            uv.map(|v| ((centre + range * v).clamp(0., self.maximum()) + 0.5).floor() as u16)
        }
        /// Packing version 2's signed half-difference.
        fn detail(self, value: [u16; 2], base: [u16; 2]) -> [u16; 2] {
            let centre = if self.ten_bit { 512. } else { 128. };
            [0, 1].map(|i| {
                ((f64::from(value[i]) - f64::from(base[i])) * 0.5 + centre + 0.5)
                    .floor()
                    .clamp(0., self.maximum()) as u16
            })
        }
    }
    fn srgb_linear(v: f64) -> f64 {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    }
    fn bt2020_sdr(v: f64) -> f64 {
        let v = v.clamp(0., 1.);
        if v <= 0.018 {
            4.5 * v
        } else {
            1.099 * v.powf(0.45) - 0.099
        }
    }
    fn gamut2020(rgb: [f64; 3]) -> [f64; 3] {
        let dot = |m: [f64; 3]| m[0] * rgb[0] + m[1] * rgb[1] + m[2] * rgb[2];
        [
            dot([0.627404, 0.329283, 0.043313]),
            dot([0.069097, 0.919540, 0.011362]),
            dot([0.016391, 0.088013, 0.895595]),
        ]
    }
    fn pq(v: f64) -> f64 {
        let power = v.clamp(0., 1.).powf(2610. / 16384.);
        ((3424. / 4096. + 2413. / 128. * power) / (1. + 2392. / 128. * power)).powf(2523. / 32.)
    }
    fn average(samples: &[[f64; 3]], weights: &[f64]) -> [f64; 3] {
        let total: f64 = weights.iter().sum();
        [0, 1, 2].map(|c| {
            samples
                .iter()
                .zip(weights)
                .map(|(s, w)| s[c] * w)
                .sum::<f64>()
                / total
        })
    }

    /// The planes a conversion writes, as the tests read them back.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum Layout {
        /// NV12 or P010 as one plane: luma rows, then interleaved chroma rows.
        Yuv420,
        /// Packed 4:4:4 (`chroma_pack_cs`) in the same one-plane form.
        Pack(u32),
        /// PyroWave's three planes, chroma halved unless 4:4:4.
        Pyro { yuv444: bool },
    }

    /// The panel's colour at a target pixel, clamped into the panel as
    /// `menu_rgb` does.
    struct Panel<'a> {
        overlay: &'a Overlay,
        colour: Colour,
    }
    impl Panel<'_> {
        fn inside(&self, [x, y]: [u32; 2]) -> bool {
            let o = self.overlay;
            (o.origin[0]..o.origin[0] + o.size[0]).contains(&x)
                && (o.origin[1]..o.origin[1] + o.size[1]).contains(&y)
        }
        fn at(&self, [x, y]: [u32; 2]) -> [f64; 3] {
            let o = self.overlay;
            let [width, height] = o.image_size();
            let local = |v: u32, origin: u32, limit: u32| {
                (v.saturating_sub(origin) / o.scale).min(limit - 1) as usize
            };
            let index =
                (local(y, o.origin[1], height) * width as usize + local(x, o.origin[0], width)) * 4;
            let rgb = [0, 1, 2].map(|c| f64::from(o.rgba[index + c]) / 255.);
            self.colour.transform(rgb)
        }
    }

    /// For each plane, every stored sample in order: the code the panel gives
    /// it, or `None` where the picture decides it.
    pub(crate) fn expected(
        overlay: &Overlay,
        colour: Colour,
        layout: Layout,
        [width, height]: [u32; 2],
    ) -> Vec<Vec<Option<u16>>> {
        let panel = Panel { overlay, colour };
        let luma = |p: [u32; 2]| panel.inside(p).then(|| colour.luma(panel.at(p)));
        match layout {
            Layout::Yuv420 => {
                let mut plane: Vec<_> = (0..height)
                    .flat_map(|y| (0..width).map(move |x| [x, y]))
                    .map(luma)
                    .collect();
                for y in 0..height / 2 {
                    for x in 0..width / 2 {
                        let at = [x * 2, y * 2];
                        let uv = panel.inside(at).then(|| {
                            let left = at[0].saturating_sub(1);
                            let samples = [
                                [left, at[1]],
                                [left, at[1] + 1],
                                at,
                                [at[0], at[1] + 1],
                                [at[0] + 1, at[1]],
                                [at[0] + 1, at[1] + 1],
                            ]
                            .map(|p| panel.at(p));
                            colour.chroma(average(&samples, &[1., 1., 2., 2., 1., 1.]))
                        });
                        plane.push(uv.map(|uv| uv[0]));
                        plane.push(uv.map(|uv| uv[1]));
                    }
                }
                vec![plane]
            }
            Layout::Pack(version) => {
                let size = [(width + 63) & !63, (height + 7) & !7];
                let half = size[0] / 2;
                let inside = |p: [u32; 2]| p[0] < width && p[1] < height && panel.inside(p);
                let chroma = |p: [u32; 2]| colour.chroma(panel.at(p));
                let mut plane = Vec::new();
                for y in 0..size[1] * 2 {
                    for x in 0..size[0] {
                        plane.push(if y < size[1] {
                            inside([x, y]).then(|| colour.luma(panel.at([x, y])))
                        } else {
                            let pixel = [(x % half) * 2 + 1, y - size[1]];
                            inside(pixel).then(|| {
                                let mut uv = chroma(pixel);
                                if version == 2 {
                                    uv = colour.detail(uv, chroma([pixel[0] & !1, pixel[1] & !1]));
                                }
                                uv[usize::from(x >= half)]
                            })
                        });
                    }
                }
                for y in 0..size[1] {
                    for x in 0..half {
                        let uv = if version == 2 {
                            let at = [x * 2, (y % (size[1] / 2)) * 2];
                            inside(at).then(|| {
                                let base = chroma(at);
                                if y < size[1] / 2 {
                                    base
                                } else {
                                    colour.detail(chroma([at[0], at[1] + 1]), base)
                                }
                            })
                        } else {
                            let at = [x * 2, y];
                            inside(at).then(|| chroma(at))
                        };
                        plane.push(uv.map(|uv| uv[0]));
                        plane.push(uv.map(|uv| uv[1]));
                    }
                }
                vec![plane]
            }
            Layout::Pyro { yuv444 } => {
                let pixels: Vec<[u32; 2]> = (0..height)
                    .flat_map(|y| (0..width).map(move |x| [x, y]))
                    .collect();
                let y = pixels.iter().map(|&p| luma(p)).collect();
                let chroma: Vec<Option<[u16; 2]>> = if yuv444 {
                    pixels
                        .iter()
                        .map(|&p| panel.inside(p).then(|| colour.chroma(panel.at(p))))
                        .collect()
                } else {
                    (0..height / 2)
                        .flat_map(|y| (0..width / 2).map(move |x| [x * 2, y * 2]))
                        .map(|at| {
                            panel.inside(at).then(|| {
                                let samples = [
                                    at,
                                    [at[0] + 1, at[1]],
                                    [at[0], at[1] + 1],
                                    [at[0] + 1, at[1] + 1],
                                ]
                                .map(|p| panel.at(p));
                                colour.chroma(average(&samples, &[1.; 4]))
                            })
                        })
                        .collect()
                };
                vec![
                    y,
                    chroma.iter().map(|uv| uv.map(|uv| uv[0])).collect(),
                    chroma.iter().map(|uv| uv.map(|uv| uv[1])).collect(),
                ]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlay(origin: [u32; 2], size: [u32; 2], scale: u32) -> Overlay {
        let pixels = (size[0] / scale.max(1)) * (size[1] / scale.max(1));
        Overlay {
            revision: 1,
            origin,
            size,
            scale,
            rgba: vec![255; pixels as usize * 4].into(),
        }
    }

    #[test]
    fn overlay_validation_rejects_unaligned_or_outside_panels() {
        let target = [1280, 720];
        overlay([16, 32], [576, 272], 1).validate(target).unwrap();
        overlay([0, 0], [1280, 720], 2).validate(target).unwrap();
        overlay([640, 448], [640, 272], 1).validate(target).unwrap();
        for (origin, size, scale) in [
            ([8, 32], [576, 272], 1),
            ([16, 40], [576, 272], 1),
            ([16, 32], [584, 272], 1),
            ([16, 32], [576, 280], 1),
            ([720, 32], [576, 272], 1),
            ([16, 464], [576, 272], 1),
            ([16, 32], [576, 272], 0),
            ([16, 32], [576, 272], 3),
            ([16, 32], [0, 272], 1),
        ] {
            assert!(
                overlay(origin, size, scale).validate(target).is_err(),
                "{origin:?} {size:?} x{scale}"
            );
        }
        let mut short = overlay([16, 32], [576, 272], 2);
        short.rgba = vec![255; 288 * 136 * 4 - 4].into();
        assert!(short.validate(target).is_err());
        let huge = Overlay {
            origin: [u32::MAX - 15, 0],
            ..overlay([0, 0], [16, 16], 1)
        };
        assert!(huge.validate(target).is_err());
    }

    #[test]
    fn overlay_constants_count_blocks_of_the_pass() {
        let panel = overlay([32, 48], [576, 272], 2);
        assert_eq!(
            panel.constants([2, 2]),
            [32, 48, 576, 272, 16, 24, 288, 136, 2, 0, 0, 0]
        );
        assert_eq!(
            panel.constants([2, 1]),
            [32, 48, 576, 272, 16, 48, 288, 272, 2, 0, 0, 0]
        );
    }
}
