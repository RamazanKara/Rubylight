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
/// Scaled taps below this are 0: the HRIRs' fade-out leaves ~1e-37 in the
/// last tap, and that times a normal sample is subnormal (slow on CPUs
/// without flush-to-zero). 1e-20 is ~-400 dB, far below any audible tap.
const TAP_FLOOR: f32 = 1e-20;

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
                        let v = v * level;
                        *tap = if v.abs() < TAP_FLOOR { 0. } else { v };
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
mod tests;
