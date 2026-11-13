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
pub(crate) const CLIP_KNEE: f32 = 0.707_945_8;
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

/// The off ramp's gain from `from` with `left` frames to go.
fn ramp(from: f32, left: u32) -> f32 {
    1. + (from - 1.) * (left as f32 / OFF_RAMP_FRAMES as f32)
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
    /// Processes interleaved `channels` in place and returns the frames it
    /// gained and clipped: all of them, or fewer when the off ramp ends within
    /// the packet. Off and fully ramped down, it leaves the samples untouched.
    pub fn process(&mut self, samples: &mut [f32], channels: usize, enabled: bool) -> usize {
        match (self.state, enabled) {
            (State::On, true) | (State::Off, false) => {}
            (State::Off, true) => self.start(),
            (State::Ending { from, left }, true) => self.resume(from, left),
            (State::On, false) => {
                self.state = State::Ending {
                    from: self.gain(),
                    left: OFF_RAMP_FRAMES,
                }
            }
            (State::Ending { .. }, false) => {}
        }
        let mut clipped = 0;
        for frame in samples.chunks_exact_mut(channels) {
            if self.state == State::Off {
                break;
            }
            for sample in frame.iter_mut() {
                if !sample.is_finite() {
                    *sample = 0.;
                    self.bad_samples = true;
                } else {
                    *sample = flush(*sample);
                }
            }
            let gain = if let State::Ending { from, left } = self.state {
                let gain = ramp(from, left);
                self.state = match left - 1 {
                    0 => State::Off,
                    left => State::Ending { from, left },
                };
                gain
            } else {
                self.track(frame)
            };
            for sample in frame.iter_mut() {
                *sample = soft_clip(*sample * gain);
            }
            clipped += 1;
        }
        clipped
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

    /// Back on during the off ramp: from the ramp's gain where it is, with the
    /// measurements kept. The compressor and leveller keep their share of it.
    fn resume(&mut self, from: f32, left: u32) {
        self.state = State::On;
        let total_db = self.compressor_db + self.leveller_db;
        let share = if total_db == 0. {
            0.
        } else {
            20. * ramp(from, left).log10() / total_db
        };
        self.compressor_db *= share;
        self.leveller_db *= share;
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
mod tests;
