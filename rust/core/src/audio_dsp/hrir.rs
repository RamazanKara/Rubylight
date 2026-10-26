//! The headphone surround HRIRs: SADIE II, Neumann KU100, 48 kHz (Apache-2.0,
//! Copyright 2018 University of York; see `LICENSE-SADIE-II.txt`). Made by
//! `rust/tools/hrir/generate.py`, which records each step in the `.json`.
use std::sync::OnceLock;

/// Taps per filter.
pub const TAPS: usize = 256;
/// The measured directions in the order of the data, in degrees anticlockwise
/// (positive is left) on the horizontal plane.
pub const AZIMUTHS: [i16; 9] = [0, 30, -30, 90, -90, 110, -110, 135, -135];

const DATA: &[u8] = include_bytes!("hrir_sadie2_ku100_48k.bin");
#[cfg(test)]
const SHA256: &str = "03ef44fae306779996f4fe2bdcaeab480897a01e3e190107d22d041435b7281a";

/// The left and right ear filters of one direction.
pub type Pair = [[f32; TAPS]; 2];

fn all() -> &'static [Pair; AZIMUTHS.len()] {
    static FILTERS: OnceLock<Box<[Pair; AZIMUTHS.len()]>> = OnceLock::new();
    FILTERS.get_or_init(|| {
        assert_eq!(DATA.len(), AZIMUTHS.len() * 2 * TAPS * 4);
        let mut filters = Box::new([[[0.; TAPS]; 2]; AZIMUTHS.len()]);
        let mut samples = DATA
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]));
        for tap in filters.iter_mut().flatten().flatten() {
            *tap = samples.next().unwrap();
        }
        filters
    })
}

/// The filters for `azimuth`, one of [`AZIMUTHS`].
pub fn pair(azimuth: i16) -> &'static Pair {
    let index = AZIMUTHS
        .iter()
        .position(|a| *a == azimuth)
        .unwrap_or_else(|| panic!("no HRIR measured at {azimuth} degrees"));
    &all()[index]
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Digest;

    #[test]
    fn the_data_is_the_generated_file() {
        assert_eq!(hex::encode(sha2::Sha256::digest(DATA)), SHA256);
        // The manifest names the same output.
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("hrir_sadie2_ku100_48k.json")).unwrap();
        assert_eq!(manifest["output_sha256"], SHA256);
        assert_eq!(manifest["format"]["taps"], TAPS);
    }

    #[test]
    fn mirror_directions_swap_ears_and_the_front_is_symmetric() {
        for azimuth in AZIMUTHS {
            let (here, there) = (pair(azimuth), pair(-azimuth));
            assert_eq!(here[0], there[1], "{azimuth}");
        }
        let front = pair(0);
        assert_eq!(front[0], front[1]);
    }

    #[test]
    fn the_near_ear_of_the_front_pair_peaks_within_8_samples_and_is_louder() {
        let peak = |h: &[f32; TAPS]| {
            (0..TAPS)
                .max_by(|a, b| h[*a].abs().total_cmp(&h[*b].abs()))
                .unwrap()
        };
        let energy = |h: &[f32; TAPS]| h.iter().map(|v| v * v).sum::<f32>();
        let left = pair(30);
        assert!(peak(&left[0]) <= 8, "{}", peak(&left[0]));
        assert!(peak(&left[0]) < peak(&left[1]));
        assert!(energy(&left[0]) > 2. * energy(&left[1]));
    }
}
