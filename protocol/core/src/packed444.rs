//! Full-resolution YUV carried as sample data in one ordinary 4:2:0 picture.
//! The codec is still lossy; only the packing itself is reversible.

pub const VERSION: u8 = 1;
pub const VERSION_ATTRIBUTE: &str = "x-rl-video.packed444Version";
pub const MODES_ATTRIBUTE: &str = "x-rl-video.packed444Modes";
pub const REQUEST_ATTRIBUTE: &str = "x-rl-video[0].chromaPacking";
pub const ALL_MODES: u8 = 15;

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
        if !(64..=16384).contains(&width)
            || !(64..=8192).contains(&height)
            || width % 2 != 0
            || height % 2 != 0
        {
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
            (63, 64),
            (65, 64),
            (64, 65),
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
        for (w, h) in [(64, 64), (66, 70), (128, 72), (192, 108)] {
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
}
