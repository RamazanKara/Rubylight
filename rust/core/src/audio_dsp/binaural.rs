//! Headphone surround: renders Moonlight's 8 channels (FL, FR, FC, LFE, BL,
//! BR, SL, SR) to binaural stereo with one HRIR pair per speaker. Direct-form
//! FIR, so the first output sample of an impulse is its first tap: no samples
//! of delay beyond the filters' own time of arrival.
use super::hrir::{self, TAPS};
use anyhow::Result;

/// Moonlight's channel slots and their Windows speaker bits.
pub const SLOTS: [u32; 8] = [1, 2, 4, 8, 16, 32, 512, 1024];
const LFE: usize = 3;
/// LFE goes to both ears at -6 dB, unfiltered.
const LFE_GAIN: f32 = 0.5;
const SURROUND: u32 = 16 | 32 | 512 | 1024;
/// Frames rendered per register block.
const BLOCK: usize = 16;

/// How a source endpoint lands in the 8 capture channels.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    /// Which slots the source fills.
    pub present: [bool; 8],
    /// Today's stereo downmix of the source, as weights of the 8 slots:
    /// applied to the 8-channel capture it gives what 2-channel capture gives.
    pub downmix: [[f32; 8]; 2],
}

impl Layout {
    pub fn new(source_channels: usize, mask: u32) -> Result<Self> {
        let stereo = crate::audio::mix_matrix(source_channels, mask, 2)?;
        let eight = crate::audio::mix_matrix(source_channels, mask, 8)?;
        let mut layout = Self {
            present: [false; 8],
            downmix: [[0.; 8]; 2],
        };
        for (slot, row) in eight.iter().enumerate() {
            layout.present[slot] = row.iter().any(|v| *v != 0.);
            for (downmix, ear) in layout.downmix.iter_mut().zip(&stereo) {
                downmix[slot] = row.iter().zip(ear).map(|(a, b)| a * b).sum();
            }
        }
        Ok(layout)
    }

    /// Whether there is anything to place around the listener: an endpoint
    /// without surround speakers keeps today's downmix.
    pub fn surround(&self) -> bool {
        SLOTS
            .iter()
            .zip(self.present)
            .any(|(bit, present)| present && bit & SURROUND != 0)
    }

    /// The direction of each slot's filters, `None` for LFE and absent ones.
    /// 7.1 puts the sides at 90 and the backs at 135 degrees; a 5.1 source's
    /// surround pair, whichever slots it uses, sits at 110.
    pub fn directions(&self) -> [Option<i16>; 8] {
        let both = self.present[4..8].iter().all(|p| *p);
        let azimuths = [
            30,
            -30,
            0,
            0,
            if both { 135 } else { 110 },
            if both { -135 } else { -110 },
            if both { 90 } else { 110 },
            if both { -90 } else { -110 },
        ];
        std::array::from_fn(|slot| (self.present[slot] && slot != LFE).then_some(azimuths[slot]))
    }

    /// The gain that makes uncorrelated noise in every present slot as loud
    /// through the HRIRs as through the downmix.
    pub fn level(&self) -> f32 {
        let downmix: f32 = (0..8)
            .filter(|slot| self.present[*slot])
            .map(|slot| self.downmix[0][slot].powi(2) + self.downmix[1][slot].powi(2))
            .sum();
        let mut binaural: f32 = self
            .directions()
            .iter()
            .flatten()
            .flat_map(|azimuth| hrir::pair(*azimuth))
            .map(|h| h.iter().map(|v| v * v).sum::<f32>())
            .sum();
        if self.present[LFE] {
            binaural += 2. * LFE_GAIN * LFE_GAIN;
        }
        if binaural == 0. {
            return 1.;
        }
        (downmix / binaural).sqrt()
    }
}

struct Speaker {
    slot: usize,
    /// Both ears' filters, time-reversed and scaled by the level.
    taps: Box<[[f32; TAPS]; 2]>,
    /// The last `TAPS - 1` samples, then the packet's.
    line: Vec<f32>,
}

pub struct Binaural {
    speakers: Vec<Speaker>,
    lfe: Option<f32>,
    /// Each ear's output of the packet being rendered.
    left: Vec<f32>,
    right: Vec<f32>,
}

impl Binaural {
    pub fn new(layout: &Layout, level: f32) -> Self {
        let speakers = layout
            .directions()
            .iter()
            .enumerate()
            .filter_map(|(slot, azimuth)| {
                let pair = hrir::pair((*azimuth)?);
                let mut taps = Box::new([[0.; TAPS]; 2]);
                for (ear, h) in taps.iter_mut().enumerate() {
                    for (tap, v) in h.iter_mut().zip(pair[ear].iter().rev()) {
                        *tap = v * level;
                    }
                }
                Some(Speaker {
                    slot,
                    taps,
                    line: vec![0.; TAPS - 1],
                })
            })
            .collect();
        Self {
            speakers,
            lfe: layout.present[LFE].then_some(LFE_GAIN * level),
            left: Vec::new(),
            right: Vec::new(),
        }
    }

    /// Renders interleaved 8-channel `input` into interleaved stereo `output`.
    pub fn render(&mut self, input: &[f32], output: &mut [f32]) {
        let frames = input.len() / 8;
        debug_assert_eq!(output.len(), frames * 2);
        let (left, right) = (&mut self.left, &mut self.right);
        left.clear();
        right.clear();
        match self.lfe {
            Some(gain) => {
                left.extend(input.iter().skip(LFE).step_by(8).map(|x| x * gain));
                right.extend_from_slice(left);
            }
            None => {
                left.resize(frames, 0.);
                right.resize(frames, 0.);
            }
        }
        for speaker in &mut self.speakers {
            speaker.feed(input);
            // BLOCK frames at a time, tap by tap: both ears' sums stay in
            // registers and each step is a contiguous multiply-add across
            // frames, so it vectorizes.
            let blocks = frames / BLOCK * BLOCK;
            for start in (0..blocks).step_by(BLOCK) {
                let (mut l, mut r) = ([0f32; BLOCK], [0f32; BLOCK]);
                for (tap, (hl, hr)) in speaker.taps[0].iter().zip(&speaker.taps[1]).enumerate() {
                    let x: &[f32; BLOCK] = speaker.line[start + tap..start + tap + BLOCK]
                        .try_into()
                        .unwrap();
                    for ((a, b), x) in l.iter_mut().zip(r.iter_mut()).zip(x) {
                        *a += hl * x;
                        *b += hr * x;
                    }
                }
                for (y, a) in left[start..start + BLOCK].iter_mut().zip(l) {
                    *y += a;
                }
                for (z, b) in right[start..start + BLOCK].iter_mut().zip(r) {
                    *z += b;
                }
            }
            for n in blocks..frames {
                let window = &speaker.line[n..n + TAPS];
                left[n] += speaker.taps[0]
                    .iter()
                    .zip(window)
                    .map(|(h, x)| h * x)
                    .sum::<f32>();
                right[n] += speaker.taps[1]
                    .iter()
                    .zip(window)
                    .map(|(h, x)| h * x)
                    .sum::<f32>();
            }
            speaker.line.drain(..frames);
        }
        for (out, (l, r)) in output
            .as_chunks_mut::<2>()
            .0
            .iter_mut()
            .zip(left.iter().zip(right.iter()))
        {
            out[0] = *l;
            out[1] = *r;
        }
    }

    /// Keeps the filters' history current without rendering, so switching to
    /// [`Self::render`] later starts without a click.
    pub fn listen(&mut self, input: &[f32]) {
        let frames = input.len() / 8;
        for speaker in &mut self.speakers {
            speaker.feed(input);
            speaker.line.drain(..frames);
        }
    }
}

impl Speaker {
    fn feed(&mut self, input: &[f32]) {
        self.line
            .extend(input.iter().skip(self.slot).step_by(8).copied());
    }
}

/// Applies the 2x8 `downmix` to interleaved 8-channel `input`, skipping zero
/// weights so a stereo source comes out bit for bit.
pub fn downmix(weights: &[[f32; 8]; 2], input: &[f32], output: &mut [f32]) {
    for (out, frame) in output
        .as_chunks_mut::<2>()
        .0
        .iter_mut()
        .zip(input.as_chunks::<8>().0)
    {
        for (ear, row) in weights.iter().enumerate() {
            let mut sum = None;
            for (w, x) in row.iter().zip(frame) {
                if *w != 0. {
                    let v = if *w == 1. { *x } else { w * x };
                    sum = Some(sum.map_or(v, |s: f32| s + v));
                }
            }
            out[ear] = sum.unwrap_or(0.);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEVEN_ONE: u32 = 0x63f;
    const FIVE_ONE_BACK: u32 = 0x3f;
    const FIVE_ONE_SIDE: u32 = 0x60f;

    fn impulse(slot: usize, frames: usize) -> Vec<f32> {
        let mut input = vec![0.; frames * 8];
        input[slot] = 1.;
        input
    }

    fn render(binaural: &mut Binaural, input: &[f32]) -> Vec<f32> {
        let mut output = vec![0.; input.len() / 4];
        binaural.render(input, &mut output);
        output
    }

    fn ear(output: &[f32], ear: usize) -> Vec<f32> {
        output.iter().skip(ear).step_by(2).copied().collect()
    }

    fn noise(frames: usize, seed: u64) -> Vec<f32> {
        let mut state = seed;
        (0..frames * 8)
            .map(|_| {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                ((state >> 40) as f32 / (1u64 << 24) as f32) - 0.5
            })
            .collect()
    }

    #[test]
    fn the_7_1_and_5_1_direction_sets() {
        let seven = Layout::new(8, SEVEN_ONE).unwrap();
        assert!(seven.surround());
        assert_eq!(
            seven.directions(),
            [
                Some(30),
                Some(-30),
                Some(0),
                None,
                Some(135),
                Some(-135),
                Some(90),
                Some(-90)
            ]
        );
        let back = Layout::new(6, FIVE_ONE_BACK).unwrap();
        assert_eq!(
            back.directions(),
            [
                Some(30),
                Some(-30),
                Some(0),
                None,
                Some(110),
                Some(-110),
                None,
                None
            ]
        );
        let side = Layout::new(6, FIVE_ONE_SIDE).unwrap();
        assert_eq!(
            side.directions(),
            [
                Some(30),
                Some(-30),
                Some(0),
                None,
                None,
                None,
                Some(110),
                Some(-110)
            ]
        );
        // No surround speakers: today's downmix stays.
        let stereo = Layout::new(2, 3).unwrap();
        assert!(!stereo.surround());
        assert_eq!(stereo.downmix[0], [1., 0., 0., 0., 0., 0., 0., 0.]);
        assert_eq!(stereo.downmix[1], [0., 1., 0., 0., 0., 0., 0., 0.]);
        assert!(!Layout::new(1, 4).unwrap().surround());
    }

    #[test]
    fn the_downmix_of_the_8_slots_matches_2_channel_capture() {
        for (channels, mask) in [
            (8, SEVEN_ONE),
            (6, FIVE_ONE_BACK),
            (6, FIVE_ONE_SIDE),
            (2, 3),
            (1, 4),
        ] {
            let layout = Layout::new(channels, mask).unwrap();
            let stereo = crate::audio::mix_matrix(channels, mask, 2).unwrap();
            let eight = crate::audio::mix_matrix(channels, mask, 8).unwrap();
            let source: Vec<f32> = (0..channels).map(|i| 0.1 + i as f32 * 0.07).collect();
            let capture: Vec<f32> = eight
                .iter()
                .map(|row| row.iter().zip(&source).map(|(a, b)| a * b).sum())
                .collect();
            let mut output = [0.; 2];
            downmix(&layout.downmix, &capture, &mut output);
            for ear in 0..2 {
                let want: f32 = stereo[ear].iter().zip(&source).map(|(a, b)| a * b).sum();
                assert!((output[ear] - want).abs() < 1e-6, "{channels} {mask:x}");
            }
        }
    }

    #[test]
    fn a_stereo_downmix_is_bit_exact() {
        let layout = Layout::new(2, 3).unwrap();
        let mut input = noise(480, 7);
        input[0] = -0.;
        input[9] = f32::MIN_POSITIVE / 4.;
        let mut output = vec![0.; 960];
        downmix(&layout.downmix, &input, &mut output);
        for (out, frame) in output
            .as_chunks::<2>()
            .0
            .iter()
            .zip(input.as_chunks::<8>().0)
        {
            assert_eq!(out[0].to_bits(), frame[0].to_bits());
            assert_eq!(out[1].to_bits(), frame[1].to_bits());
        }
    }

    #[test]
    fn an_impulse_in_each_slot_comes_out_as_its_hrir() {
        let layout = Layout::new(8, SEVEN_ONE).unwrap();
        let mut binaural = Binaural::new(&layout, 1.);
        for (slot, azimuth) in layout.directions().iter().enumerate() {
            let output = render(&mut binaural, &impulse(slot, TAPS * 2));
            match azimuth {
                Some(azimuth) => {
                    let pair = hrir::pair(*azimuth);
                    for (e, filter) in pair.iter().enumerate() {
                        let got = ear(&output, e);
                        assert_eq!(&got[..TAPS], &filter[..], "{slot} {e}");
                        assert!(got[TAPS..].iter().all(|v| *v == 0.));
                    }
                }
                None => {
                    // LFE: both ears at -6 dB, unfiltered.
                    assert_eq!(&output[..2], &[0.5, 0.5]);
                    assert!(output[2..].iter().all(|v| *v == 0.));
                }
            }
        }
    }

    #[test]
    fn front_left_reaches_the_left_ear_earlier_and_louder_and_mirrors_front_right() {
        let layout = Layout::new(8, SEVEN_ONE).unwrap();
        let mut binaural = Binaural::new(&layout, 1.);
        let fl = render(&mut binaural, &impulse(0, TAPS));
        let fr = render(&mut binaural, &impulse(1, TAPS));
        let (left, right) = (ear(&fl, 0), ear(&fl, 1));
        let onset = |h: &[f32]| h.iter().position(|v| v.abs() > 0.05).unwrap();
        let energy = |h: &[f32]| h.iter().map(|v| v * v).sum::<f32>();
        assert!(onset(&left) < onset(&right));
        assert!(energy(&left) > 2. * energy(&right));
        assert_eq!(ear(&fr, 1), left);
        assert_eq!(ear(&fr, 0), right);
        let fc = render(&mut binaural, &impulse(2, TAPS));
        assert_eq!(ear(&fc, 0), ear(&fc, 1));
    }

    #[test]
    fn rendering_matches_an_f64_reference_convolution_in_any_packet_size() {
        let layout = Layout::new(8, SEVEN_ONE).unwrap();
        let level = layout.level();
        let input = noise(4800, 1);
        let frames = input.len() / 8;
        let mut want = vec![0f64; frames * 2];
        for (slot, azimuth) in layout.directions().iter().enumerate() {
            for n in 0..frames {
                let x = |k: usize| input[(n - k) * 8 + slot] as f64;
                for e in 0..2 {
                    want[n * 2 + e] += match azimuth {
                        Some(azimuth) => {
                            let h = &hrir::pair(*azimuth)[e];
                            (0..TAPS.min(n + 1))
                                .map(|k| h[k] as f64 * x(k))
                                .sum::<f64>()
                        }
                        None => 0.5 * x(0),
                    } * level as f64;
                }
            }
        }
        for packet in [100, 240, 480, 960, 4800] {
            let mut binaural = Binaural::new(&layout, level);
            let mut got = Vec::new();
            for chunk in input.chunks(packet * 8) {
                got.extend(render(&mut binaural, chunk));
            }
            let error = got
                .iter()
                .zip(&want)
                .map(|(a, b)| (*a as f64 - b).abs())
                .fold(0., f64::max);
            assert!(error < 1e-5, "{packet}: {error}");
        }
    }

    #[test]
    fn listening_keeps_the_history_so_a_switch_is_seamless() {
        let layout = Layout::new(6, FIVE_ONE_BACK).unwrap();
        let input = noise(960, 3);
        let mut always = Binaural::new(&layout, 1.);
        let whole = render(&mut always, &input);
        let mut switched = Binaural::new(&layout, 1.);
        switched.listen(&input[..480 * 8]);
        let tail = render(&mut switched, &input[480 * 8..]);
        assert_eq!(tail, whole[960..]);
    }

    #[test]
    fn noise_through_the_hrirs_is_as_loud_as_the_downmix() {
        for (channels, mask) in [(8, SEVEN_ONE), (6, FIVE_ONE_BACK), (6, FIVE_ONE_SIDE)] {
            let layout = Layout::new(channels, mask).unwrap();
            let mut input = noise(48_000 * 2, 11);
            for frame in input.as_chunks_mut::<8>().0 {
                for (slot, v) in frame.iter_mut().enumerate() {
                    if !layout.present[slot] {
                        *v = 0.;
                    }
                }
            }
            let mut binaural = Binaural::new(&layout, layout.level());
            let surround = render(&mut binaural, &input);
            let mut stereo = vec![0.; surround.len()];
            downmix(&layout.downmix, &input, &mut stereo);
            let power = |s: &[f32]| s.iter().map(|v| (*v as f64).powi(2)).sum::<f64>();
            let db = 10. * (power(&surround) / power(&stereo)).log10();
            assert!(db.abs() < 0.5, "{channels} {mask:x}: {db} dB");
        }
    }
}
