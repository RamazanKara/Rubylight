//! Night mode: a linked feed-forward compressor, a slow leveller and a soft
//! clipper. No lookahead: each output frame is the input frame times a gain
//! computed from it and earlier frames, so it adds no samples of delay.

const RATE: f32 = 48_000.;
/// Compressor threshold, ratio and knee width.
const THRESHOLD_DB: f32 = -30.;
const RATIO: f32 = 3.;
const KNEE_DB: f32 = 6.;
/// The peak detector falls this fast between peaks, so a waveform's troughs
/// don't read as quiet; the gain then moves with the attack and release.
const DETECT_SECONDS: f32 = 0.05;
const ATTACK_SECONDS: f32 = 0.003;
const RELEASE_SECONDS: f32 = 0.25;
/// The leveller's target, its 3 s RMS, range, slew and hold.
const TARGET_DB: f32 = -23.;
const RMS_SECONDS: f32 = 3.;
const LEVEL_MIN_DB: f32 = -6.;
const LEVEL_MAX_DB: f32 = 12.;
const SLEW_DB_PER_SECOND: f32 = 3.;
const HOLD_BELOW_DB: f32 = -60.;
/// Turning night mode off ramps the gain back to unity over 10 ms.
const OFF_RAMP_FRAMES: u32 = 480;
/// The soft clipper is unity up to -3 dBFS and never reaches 0.98.
const CLIP_KNEE: f32 = 0.707_945_8;
pub(crate) const CLIP_CEILING: f32 = 0.98;

/// Values this small become 0, so neither samples nor decaying state reach
/// the subnormals, which are slow on x86.
pub(crate) const FLUSH_BELOW: f32 = 1e-30;

pub(crate) fn flush(x: f32) -> f32 {
    if x.abs() < FLUSH_BELOW { 0. } else { x }
}

/// The LFE of a 5.1 or 7.1 frame, which the leveller doesn't measure.
const LFE: usize = 3;

/// A frame's power for the leveller, BS.1770-style: without the LFE and
/// normalized to a stereo pair, so the same programme reads the same level in
/// 2, 6 or 8 channels.
fn power(frame: &[f32]) -> f32 {
    let square = |samples: &[f32]| samples.iter().map(|x| x * x).sum::<f32>();
    let sum = if frame.len() > LFE + 1 {
        square(&frame[..LFE]) + square(&frame[LFE + 1..])
    } else {
        square(frame)
    };
    sum / 2.
}

fn coefficient(seconds: f32) -> f32 {
    (-1. / (seconds * RATE)).exp()
}

/// The compressor's static gain, in dB (0 or less), for a peak level in dBFS.
pub(crate) fn curve(level_db: f32) -> f32 {
    let over = level_db - THRESHOLD_DB;
    let slope = 1. / RATIO - 1.;
    if 2. * over < -KNEE_DB {
        0.
    } else if 2. * over <= KNEE_DB {
        slope * (over + KNEE_DB / 2.).powi(2) / (2. * KNEE_DB)
    } else {
        slope * over
    }
}

/// Unity below -3 dBFS, then a smooth approach to 0.98 with a continuous slope.
pub(crate) fn soft_clip(x: f32) -> f32 {
    let magnitude = x.abs();
    if magnitude <= CLIP_KNEE {
        return x;
    }
    let room = CLIP_CEILING - CLIP_KNEE;
    (CLIP_KNEE + room * ((magnitude - CLIP_KNEE) / room).tanh()).copysign(x)
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    Off,
    On,
    /// Going back to unity from `from`, with `left` frames to go.
    Ending {
        from: f32,
        left: u32,
    },
}

pub struct Night {
    state: State,
    detector: f32,
    compressor_db: f32,
    mean_square: f32,
    leveller_db: f32,
    bad_samples: bool,
    detect: f32,
    attack: f32,
    release: f32,
    rms: f32,
}

impl Default for Night {
    fn default() -> Self {
        Self {
            state: State::Off,
            detector: 0.,
            compressor_db: 0.,
            mean_square: 0.,
            leveller_db: 0.,
            bad_samples: false,
            detect: coefficient(DETECT_SECONDS),
            attack: coefficient(ATTACK_SECONDS),
            release: coefficient(RELEASE_SECONDS),
            rms: coefficient(RMS_SECONDS),
        }
    }
}

impl Night {
    /// Processes interleaved `channels` in place. Off and fully ramped down,
    /// it leaves the samples untouched.
    pub fn process(&mut self, samples: &mut [f32], channels: usize, enabled: bool) {
        match (self.state, enabled) {
            (State::On, true) | (State::Off, false) => {}
            (State::Off | State::Ending { .. }, true) => self.start(),
            (State::On, false) => {
                self.state = State::Ending {
                    from: self.gain(),
                    left: OFF_RAMP_FRAMES,
                }
            }
            (State::Ending { .. }, false) => {}
        }
        if self.state == State::Off {
            return;
        }
        for frame in samples.chunks_exact_mut(channels) {
            for sample in frame.iter_mut() {
                if !sample.is_finite() {
                    *sample = 0.;
                    self.bad_samples = true;
                } else {
                    *sample = flush(*sample);
                }
            }
            let gain = match self.state {
                State::Off => break,
                State::On => self.track(frame),
                State::Ending { from, left } => {
                    let gain = 1. + (from - 1.) * (left as f32 / OFF_RAMP_FRAMES as f32);
                    self.state = match left - 1 {
                        0 => State::Off,
                        left => State::Ending { from, left },
                    };
                    gain
                }
            };
            for sample in frame.iter_mut() {
                *sample = soft_clip(*sample * gain);
            }
        }
    }

    /// Whether NaN or infinite samples were replaced since the last call.
    pub fn take_bad_samples(&mut self) -> bool {
        std::mem::take(&mut self.bad_samples)
    }

    fn start(&mut self) {
        self.state = State::On;
        self.detector = 0.;
        self.compressor_db = 0.;
        self.leveller_db = 0.;
        // As if the programme were already at the target: the leveller starts
        // from unity and only moves once it has measured something else.
        self.mean_square = 10f32.powf(TARGET_DB / 10.);
    }

    fn gain(&self) -> f32 {
        10f32.powf((self.compressor_db + self.leveller_db) / 20.)
    }

    /// Updates the detectors with one frame and returns its gain.
    fn track(&mut self, frame: &[f32]) -> f32 {
        let peak = frame.iter().fold(0f32, |peak, x| peak.max(x.abs()));
        self.detector = flush(peak.max(self.detector * self.detect));
        let level_db = 20. * self.detector.max(1e-6).log10();
        let target = curve(level_db);
        let coefficient = if target < self.compressor_db {
            self.attack
        } else {
            self.release
        };
        self.compressor_db = flush(target + coefficient * (self.compressor_db - target));
        // Below -60 dBFS (pauses, hiss) the leveller holds its gain and its
        // measurement, so quiet passages don't pump the noise floor up.
        if level_db >= HOLD_BELOW_DB {
            // It measures what the compressor lets through.
            let compressed = 10f32.powf(self.compressor_db / 10.);
            let power = power(frame) * compressed;
            self.mean_square = power + self.rms * (self.mean_square - power);
            let rms_db = 10. * self.mean_square.max(1e-12).log10();
            let want = (TARGET_DB - rms_db).clamp(LEVEL_MIN_DB, LEVEL_MAX_DB);
            let step = SLEW_DB_PER_SECOND / RATE;
            self.leveller_db += (want - self.leveller_db).clamp(-step, step);
        }
        self.gain()
    }

    #[cfg(test)]
    pub(crate) fn compressor_db(&self) -> f32 {
        self.compressor_db
    }

    #[cfg(test)]
    pub(crate) fn leveller_db(&self) -> f32 {
        self.leveller_db
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(dbfs: f32, hz: f32, frames: usize, channels: usize) -> Vec<f32> {
        let amplitude = 10f32.powf(dbfs / 20.);
        (0..frames)
            .flat_map(|n| {
                let v = amplitude * (std::f32::consts::TAU * hz * n as f32 / RATE).sin();
                std::iter::repeat_n(v, channels)
            })
            .collect()
    }

    fn rms_db(samples: &[f32]) -> f32 {
        10. * (samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32).log10()
    }

    #[test]
    fn the_curve_is_3_to_1_above_minus_30_with_a_6_db_knee() {
        assert_eq!(curve(-60.), 0.);
        assert_eq!(curve(-33.), 0.);
        assert!((curve(-30.) - -0.5).abs() < 1e-5);
        assert!((curve(-27.) - -2.).abs() < 1e-5);
        assert!((curve(-6.) - -16.).abs() < 1e-4);
        assert!((curve(0.) - -20.).abs() < 1e-4);
    }

    #[test]
    fn the_clipper_is_unity_below_minus_3_dbfs_and_bounded_by_0_98() {
        assert_eq!(soft_clip(0.5), 0.5);
        assert_eq!(soft_clip(-0.7), -0.7);
        for x in [0.75, 0.9, 1., 2., 100., f32::MAX] {
            let y = soft_clip(x);
            assert!(y > CLIP_KNEE && y <= CLIP_CEILING, "{x} -> {y}");
            assert_eq!(soft_clip(-x), -y);
        }
        // Monotonic, and continuous at the knee.
        let mut last = 0.;
        for i in 0..2000 {
            let y = soft_clip(i as f32 / 1000.);
            assert!(y >= last);
            assert!(y - last <= 0.0011);
            last = y;
        }
    }

    #[test]
    fn silence_stays_exactly_zero() {
        let mut night = Night::default();
        let mut samples = vec![0f32; 48_000 * 2];
        night.process(&mut samples, 2, true);
        assert!(samples.iter().all(|x| x.to_bits() == 0));
    }

    #[test]
    fn a_minus_6_dbfs_sine_gets_the_reduction_of_the_curve() {
        let mut night = Night::default();
        let mut samples = sine(-6., 1000., 48_000, 2);
        night.process(&mut samples, 2, true);
        assert!(
            (night.compressor_db() - curve(-6.)).abs() < 0.5,
            "{}",
            night.compressor_db()
        );
    }

    #[test]
    fn a_step_from_minus_40_to_minus_6_dbfs_stays_under_0_98_and_settles_in_10_ms() {
        let mut night = Night::default();
        let mut quiet = sine(-40., 1000., 48_000 * 10, 2);
        night.process(&mut quiet, 2, true);
        assert!(night.compressor_db() > -0.1);
        let mut loud = sine(-6., 1000., 480, 2);
        night.process(&mut loud, 2, true);
        assert!(loud.iter().all(|x| x.abs() < CLIP_CEILING));
        assert!(
            (night.compressor_db() - curve(-6.)).abs() < 1.,
            "{}",
            night.compressor_db()
        );
    }

    #[test]
    fn the_leveller_brings_a_quiet_sine_up_by_its_limit_and_a_moderate_one_to_minus_23() {
        let mut night = Night::default();
        let mut quiet = sine(-40., 440., 48_000 * 12, 2);
        night.process(&mut quiet, 2, true);
        assert!((night.leveller_db() - LEVEL_MAX_DB).abs() < 1.);
        let tail = &quiet[quiet.len() - 96_000..];
        assert!(
            (rms_db(tail) - (-43. + LEVEL_MAX_DB)).abs() < 1.,
            "{}",
            rms_db(tail)
        );

        let mut night = Night::default();
        let mut moderate = sine(-20., 440., 48_000 * 20, 2);
        night.process(&mut moderate, 2, true);
        let tail = &moderate[moderate.len() - 96_000..];
        assert!((rms_db(tail) - TARGET_DB).abs() < 1., "{}", rms_db(tail));
    }

    #[test]
    fn the_leveller_holds_below_minus_60_dbfs_and_slews_at_3_db_per_second() {
        let mut night = Night::default();
        let mut hiss = sine(-67., 3000., 48_000 * 20, 2);
        let input = hiss.clone();
        night.process(&mut hiss, 2, true);
        assert!(night.leveller_db().abs() < 0.5, "{}", night.leveller_db());
        assert!(rms_db(&hiss[hiss.len() - 48_000..]) - rms_db(&input) < 0.5);

        // A pause after quiet speech keeps the gain it had.
        let mut night = Night::default();
        let mut speech = sine(-40., 440., 48_000 * 6, 2);
        night.process(&mut speech, 2, true);
        let before = night.leveller_db();
        assert!(before > 5.);
        let mut pause = sine(-70., 440., 48_000 * 10, 2);
        night.process(&mut pause, 2, true);
        assert!(
            (night.leveller_db() - before).abs() < 0.2,
            "{before} {}",
            night.leveller_db()
        );

        let mut night = Night::default();
        let mut quiet = sine(-40., 440., 48_000, 2);
        night.process(&mut quiet, 2, true);
        assert!(night.leveller_db() <= 3.0001, "{}", night.leveller_db());
    }

    #[test]
    fn gain_is_linked_across_channels() {
        let mut night = Night::default();
        let mut samples: Vec<f32> = sine(-6., 500., 9600, 1)
            .into_iter()
            .flat_map(|v| [v, 0., 0., 0., 0., 0.])
            .collect();
        night.process(&mut samples, 6, true);
        for frame in samples.as_chunks::<6>().0 {
            assert!(frame[1..].iter().all(|x| x.to_bits() == 0));
        }
    }

    #[test]
    fn the_leveller_reads_stereo_the_same_in_2_and_8_channels_without_the_lfe() {
        let left = sine(-35., 440., 48_000 * 4, 1);
        let right = sine(-41., 660., 48_000 * 4, 1);
        let stereo: Vec<f32> = left
            .iter()
            .zip(&right)
            .flat_map(|(l, r)| [*l, *r])
            .collect();
        let padded: Vec<f32> = left
            .iter()
            .zip(&right)
            .flat_map(|(l, r)| [*l, *r, 0., 0., 0., 0., 0., 0.])
            .collect();
        let mut two = Night::default();
        two.process(&mut stereo.clone(), 2, true);
        let mut eight = Night::default();
        eight.process(&mut padded.clone(), 8, true);
        assert!(two.leveller_db() > 3., "{}", two.leveller_db());
        assert_eq!(eight.leveller_db(), two.leveller_db());
        assert_eq!(eight.compressor_db(), two.compressor_db());
        // The LFE's power doesn't count (its peaks, linked, would).
        let rumble = sine(-10., 40., 48_000 * 4, 1);
        let mut lfe = padded;
        for (frame, x) in lfe.chunks_exact_mut(8).zip(&rumble) {
            frame[LFE] = *x * 1e-3;
        }
        let mut with_lfe = Night::default();
        with_lfe.process(&mut lfe, 8, true);
        assert_eq!(with_lfe.leveller_db(), two.leveller_db());
    }

    #[test]
    fn an_impulse_peaks_at_sample_0() {
        let mut night = Night::default();
        let mut samples = vec![0f32; 960];
        samples[0] = 1.;
        samples[1] = 1.;
        night.process(&mut samples, 2, true);
        assert!(samples[2..].iter().all(|x| x.abs() < samples[0]));
        assert_eq!(samples[0], samples[1]);
        assert!(samples[0] > 0.9 && samples[0] < CLIP_CEILING);
    }

    #[test]
    fn output_does_not_depend_on_the_packet_size() {
        let mut input = sine(-30., 300., 48_000 * 2, 2);
        for (i, x) in sine(-8., 2100., 48_000 * 2, 2).into_iter().enumerate() {
            if (i / 20_000) % 2 == 1 {
                input[i] += x;
            }
        }
        let mut whole = input.clone();
        Night::default().process(&mut whole, 2, true);
        for ms in [5, 10, 20] {
            let mut night = Night::default();
            let mut packets = input.clone();
            for packet in packets.chunks_mut(48 * ms * 2) {
                night.process(packet, 2, true);
            }
            assert_eq!(packets, whole, "{ms} ms");
        }
    }

    #[test]
    fn turning_it_off_ramps_to_unity_without_a_step_then_bypasses() {
        let mut night = Night::default();
        let mut on = sine(-40., 200., 48_000 * 6, 2);
        night.process(&mut on, 2, true);
        assert!(night.gain() > 2.);
        let input = sine(-40., 200., 48_000, 2);
        let mut off = input.clone();
        for packet in off.chunks_mut(480) {
            night.process(packet, 2, false);
        }
        let largest = |s: &[f32]| {
            s.as_chunks::<2>()
                .0
                .windows(2)
                .map(|w| (w[1][0] - w[0][0]).abs())
                .fold(0f32, f32::max)
        };
        let before = largest(&on[on.len() - 9600..]);
        let joined: Vec<f32> = on[on.len() - 9600..]
            .iter()
            .chain(&off[..9600])
            .copied()
            .collect();
        assert!(largest(&joined) <= before * 1.01);
        // After 10 ms the samples pass through untouched.
        assert_eq!(&off[960..], &input[960..]);
    }

    #[test]
    fn subnormal_input_and_decaying_state_never_stay_subnormal() {
        let mut night = Night::default();
        let mut tiny: Vec<f32> = (0..9600)
            .map(|i| [1e-40, -1e-39, 1e-31, f32::MIN_POSITIVE][i % 4])
            .collect();
        night.process(&mut tiny, 2, true);
        assert!(tiny.iter().all(|x| x.to_bits() == 0));
        // A loud burst, then 30 s of silence: the detector and the
        // compressor decay toward 0 without passing through the subnormals.
        let mut burst = sine(0., 1000., 4800, 2);
        night.process(&mut burst, 2, true);
        assert!(night.compressor_db() < -10.);
        let mut silence = vec![0f32; 240 * 2];
        for _ in 0..30 * 200 {
            night.process(&mut silence, 2, true);
            assert!(!night.detector.is_subnormal(), "{:e}", night.detector);
            assert!(
                !night.compressor_db.is_subnormal(),
                "{:e}",
                night.compressor_db
            );
        }
        assert_eq!((night.detector, night.compressor_db), (0., 0.));
        assert!(silence.iter().all(|x| x.to_bits() == 0));
    }

    #[test]
    fn nan_and_infinite_samples_become_zero_and_are_reported() {
        let mut night = Night::default();
        let mut samples = vec![0.1, f32::NAN, f32::INFINITY, -0.1];
        night.process(&mut samples, 2, true);
        assert!(samples.iter().all(|x| x.is_finite()));
        assert_eq!(samples[1], 0.);
        assert!(night.take_bad_samples());
        assert!(!night.take_bad_samples());
    }
}
