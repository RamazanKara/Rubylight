//! Full-resolution YUV carried as sample data in one ordinary 4:2:0 picture.
//! The codec is lossy. Version 1 packs reversibly; version 2 bounds prediction
//! quantization to one code and stores a 4:2:0 base with chroma enhancement.

pub const VERSION: u8 = 1;
pub const LAYERED_VERSION: u8 = 2;
pub const VERSION_ATTRIBUTE: &str = "x-rl-video.packed444Version";
pub const MODES_ATTRIBUTE: &str = "x-rl-video.packed444Modes";
pub const LAYERED_MODES_ATTRIBUTE: &str = "x-rl-video.packed444LayeredModes";
pub const REQUEST_ATTRIBUTE: &str = "x-rl-video[0].chromaPacking";
pub const ALL_MODES: u8 = 15;
// Residual quantization is small relative to 10-bit precision. The measured
// 8-bit colour error does not justify automatically replacing version 1.
pub const LAYERED_MODES: u8 = 2 | 8;

pub const fn supported(version: u8) -> bool {
    matches!(version, VERSION | LAYERED_VERSION)
}

/// Codec numbers are the existing ANNOUNCE bitStreamFormat values.
pub const fn mode_bit(codec: u8, ten_bit: bool) -> u8 {
    match (codec, ten_bit) {
        (1, false) => 1,
        (1, true) => 2,
        (2, false) => 4,
        (2, true) => 8,
        _ => 0,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    pub width: u32,
    pub height: u32,
    pub padded_width: u32,
    pub padded_height: u32,
}

impl Geometry {
    pub fn new(width: u32, height: u32) -> Option<Self> {
        if !(1..=16384).contains(&width) || !(1..=8192).contains(&height) {
            return None;
        }
        Some(Self {
            width,
            height,
            padded_width: (width + 63) & !63,
            padded_height: (height + 7) & !7,
        })
    }

    pub const fn coded_size(self) -> (u32, u32) {
        (self.padded_width, self.padded_height * 2)
    }

    pub const fn samples(self) -> usize {
        self.padded_width as usize * self.padded_height as usize * 3
    }

    /// Position in the dense planar Y/U/V decoded picture of a source sample.
    pub fn sample(self, plane: usize, x: u32, y: u32) -> usize {
        let w = self.padded_width as usize;
        let h = self.padded_height as usize;
        let (x, y) = (x as usize, y as usize);
        match plane {
            0 => y * w + x,
            1 | 2 if x % 2 == 0 => 2 * w * h + (plane - 1) * w * h / 2 + y * w / 2 + x / 2,
            1 | 2 => (h + y) * w + (plane - 1) * w / 2 + x / 2,
            _ => unreachable!(),
        }
    }

    /// Reference implementation for independent decoders and GPU conformance tests.
    pub fn pack<T: Copy>(self, source: [&[T]; 3], packed: &mut [T]) -> bool {
        let count = self.width as usize * self.height as usize;
        if source.iter().any(|p| p.len() != count) || packed.len() != self.samples() {
            return false;
        }
        for (plane, samples) in source.iter().enumerate() {
            for y in 0..self.padded_height {
                for x in 0..self.padded_width {
                    let input = y.min(self.height - 1) as usize * self.width as usize
                        + x.min(self.width - 1) as usize;
                    packed[self.sample(plane, x, y)] = samples[input];
                }
            }
        }
        true
    }

    pub fn unpack<T: Copy>(self, packed: &[T], output: &mut [T]) -> bool {
        let count = self.width as usize * self.height as usize;
        if packed.len() != self.samples() || output.len() != count * 3 {
            return false;
        }
        for plane in 0..3 {
            for y in 0..self.height {
                for x in 0..self.width {
                    output[plane * count + y as usize * self.width as usize + x as usize] =
                        packed[self.sample(plane, x, y)];
                }
            }
        }
        true
    }

    /// Version 2 keeps a co-sited 4:2:0 base in the top half. The remaining
    /// three chroma samples in each 2x2 block are differences from its anchor.
    /// Halving differences avoids wraparound under lossy coding and costs at
    /// most one source code before compression; version 1 remains reversible.
    pub fn pack_layered(self, source: [&[u16]; 3], packed: &mut [u16], maximum: u16) -> bool {
        let count = self.width as usize * self.height as usize;
        if !matches!(maximum, 255 | 1023)
            || source
                .iter()
                .any(|p| p.len() != count || p.iter().any(|&v| v > maximum))
            || packed.len() != self.samples()
        {
            return false;
        }
        for (plane, samples) in source.iter().enumerate() {
            for y in 0..self.padded_height {
                for x in 0..self.padded_width {
                    let index = |x: u32, y: u32| {
                        y.min(self.height - 1) as usize * self.width as usize
                            + x.min(self.width - 1) as usize
                    };
                    let value = samples[index(x, y)];
                    let value = if plane == 0 || (x & 1 == 0 && y & 1 == 0) {
                        value
                    } else {
                        let base = i32::from(samples[index(x & !1, y & !1)]);
                        ((i32::from(value) - base + i32::from(maximum) + 2) / 2)
                            .min(i32::from(maximum)) as u16
                    };
                    packed[self.layered_sample(plane, x, y)] = value;
                }
            }
        }
        true
    }

    pub fn unpack_layered(self, packed: &[u16], output: &mut [u16], maximum: u16) -> bool {
        let count = self.width as usize * self.height as usize;
        if !matches!(maximum, 255 | 1023)
            || packed.len() != self.samples()
            || packed.iter().any(|&v| v > maximum)
            || output.len() != count * 3
        {
            return false;
        }
        for plane in 0..3 {
            for y in 0..self.height {
                for x in 0..self.width {
                    let value = i32::from(packed[self.layered_sample(plane, x, y)]);
                    let value = if plane == 0 || (x & 1 == 0 && y & 1 == 0) {
                        value
                    } else {
                        let base = i32::from(packed[self.layered_sample(plane, x & !1, y & !1)]);
                        (base + 2 * value - i32::from(maximum) - 1).clamp(0, i32::from(maximum))
                    };
                    output[plane * count + y as usize * self.width as usize + x as usize] =
                        value as u16;
                }
            }
        }
        true
    }

    pub fn layered_sample(self, plane: usize, x: u32, y: u32) -> usize {
        let w = self.padded_width as usize;
        let h = self.padded_height as usize;
        let (x, y) = (x as usize, y as usize);
        match plane {
            0 => y * w + x,
            1 | 2 if x % 2 == 0 => {
                2 * w * h + (plane - 1) * w * h / 2 + ((y % 2) * h / 2 + y / 2) * w / 2 + x / 2
            }
            1 | 2 => (h + y) * w + (plane - 1) * w / 2 + x / 2,
            _ => unreachable!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    #[test]
    fn geometry_bounds_and_alignment() {
        assert_eq!(
            Geometry::new(2560, 1440).unwrap().coded_size(),
            (2560, 2880)
        );
        assert_eq!(
            Geometry::new(1920, 1080).unwrap().coded_size(),
            (1920, 2160)
        );
        assert_eq!(Geometry::new(66, 66).unwrap().coded_size(), (128, 144));
        for (w, h) in [
            (0, 64),
            (64, 0),
            (64, 8194),
            (16386, 64),
            (u32::MAX, u32::MAX),
        ] {
            assert!(Geometry::new(w, h).is_none());
        }
        assert!(Geometry::new(16384, 8192).is_some());
    }

    #[test]
    fn every_sample_round_trips_without_chroma_filtering() {
        for (w, h) in [(1, 1), (65, 67), (64, 64), (66, 70), (128, 72), (192, 108)] {
            let layout = Geometry::new(w, h).unwrap();
            for maximum in [255u16, 1023] {
                let count = w as usize * h as usize;
                let source: Vec<u16> = (0..count * 3)
                    .map(|i| ((i * 313 + i / w as usize * 71) & usize::from(maximum)) as u16)
                    .collect();
                let mut packed = vec![u16::MAX; layout.samples()];
                assert!(layout.pack(
                    [
                        &source[..count],
                        &source[count..2 * count],
                        &source[2 * count..]
                    ],
                    &mut packed
                ));
                assert!(packed.iter().all(|&v| v <= maximum));
                let mut decoded = vec![0; source.len()];
                assert!(layout.unpack(&packed, &mut decoded));
                assert_eq!(decoded, source);
                for plane in 0..3 {
                    for y in 0..layout.padded_height {
                        for x in 0..layout.padded_width {
                            assert_eq!(
                                packed[layout.sample(plane, x, y)],
                                source[plane * count
                                    + y.min(h - 1) as usize * w as usize
                                    + x.min(w - 1) as usize]
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn only_standard_hevc_and_av1_modes_have_bits() {
        assert_eq!(
            [
                mode_bit(1, false),
                mode_bit(1, true),
                mode_bit(2, false),
                mode_bit(2, true)
            ],
            [1, 2, 4, 8]
        );
        assert_eq!(mode_bit(0, false) | mode_bit(3, true), 0);
    }

    #[test]
    fn layered_base_and_prediction_are_bounded_for_every_code_pair() {
        for maximum in [255u16, 1023] {
            let layout = Geometry::new(2 * u32::from(maximum + 1), 2).unwrap();
            let count = (layout.width * layout.height) as usize;
            let mut source = vec![0; count];
            let mut packed = vec![0; layout.samples()];
            let mut decoded = vec![0; count * 3];
            for base in 0..=maximum {
                for row in source.chunks_mut(layout.width as usize) {
                    for (value, pair) in row.chunks_mut(2).enumerate() {
                        pair.copy_from_slice(&[base, value as u16]);
                    }
                }
                assert!(layout.pack_layered([&source; 3], &mut packed, maximum));
                assert!(layout.unpack_layered(&packed, &mut decoded, maximum));
                assert_eq!(&decoded[..count], &source);
                for chroma in decoded[count..].chunks(count) {
                    assert!(chroma
                        .iter()
                        .zip(&source)
                        .all(|(&a, &b)| a.abs_diff(b) <= 1));
                    assert!(chroma.iter().step_by(2).all(|&value| value == base));
                }
                assert_eq!(
                    packed[layout.layered_sample(1, u32::from(base) * 2 + 1, 0)],
                    (maximum + 1) / 2
                );
            }
        }
    }

    #[test]
    fn layered_layout_covers_every_sample_and_preserves_full_chroma() {
        for (w, h) in [(1, 1), (65, 67), (64, 64), (66, 70), (128, 72)] {
            let layout = Geometry::new(w, h).unwrap();
            let count = (w * h) as usize;
            for maximum in [255u16, 1023] {
                let source: Vec<u16> = (0..count * 3)
                    .map(|i| ((i * 313 + i / w as usize * 71) & usize::from(maximum)) as u16)
                    .collect();
                let mut packed = vec![u16::MAX; layout.samples()];
                assert!(layout.pack_layered(
                    [
                        &source[..count],
                        &source[count..2 * count],
                        &source[2 * count..]
                    ],
                    &mut packed,
                    maximum
                ));
                assert!(packed.iter().all(|&v| v <= maximum));
                let mut decoded = vec![0; source.len()];
                assert!(layout.unpack_layered(&packed, &mut decoded, maximum));
                assert_eq!(&decoded[..count], &source[..count]);
                assert!(decoded
                    .iter()
                    .zip(&source)
                    .all(|(&a, &b)| a.abs_diff(b) <= 1));
                let mut visited = vec![false; packed.len()];
                for plane in 0..3 {
                    for y in 0..layout.padded_height {
                        for x in 0..layout.padded_width {
                            let at = layout.layered_sample(plane, x, y);
                            assert!(!visited[at]);
                            visited[at] = true;
                            if plane != 0 && x & 1 == 0 && y & 1 == 0 {
                                assert_eq!(
                                    packed[at],
                                    source[plane * count
                                        + y.min(h - 1) as usize * w as usize
                                        + x.min(w - 1) as usize]
                                );
                            }
                        }
                    }
                }
                assert!(visited.iter().all(|&v| v));
                assert!(!layout.unpack_layered(&packed[..packed.len() - 1], &mut decoded, maximum));
                assert!(!layout.pack_layered(
                    [
                        &source[..count],
                        &source[count..2 * count],
                        &source[2 * count..]
                    ],
                    &mut packed,
                    256
                ));
            }
        }
    }

    #[test]
    fn single_pixel_chroma_and_buffer_boundaries() {
        let layout = Geometry::new(65, 9).unwrap();
        let count = layout.width as usize * layout.height as usize;
        let mut source = vec![0u8; count * 3];
        for plane in [1, 2] {
            for (x, y) in [(0, 0), (1, 0), (63, 7), (64, 8)] {
                source.fill(0);
                source[plane * count + y * 65 + x] = 255;
                let mut packed = vec![0; layout.samples()];
                assert!(layout.pack(
                    [
                        &source[..count],
                        &source[count..2 * count],
                        &source[2 * count..]
                    ],
                    &mut packed
                ));
                let mut output = vec![0; count * 3];
                assert!(layout.unpack(&packed, &mut output));
                assert_eq!(source, output);
                assert!(!layout.unpack(&packed[..packed.len() - 1], &mut output));
                assert!(!layout.pack(
                    [
                        &source[..count - 1],
                        &source[count..2 * count],
                        &source[2 * count..]
                    ],
                    &mut packed
                ));
            }
        }
    }
}
