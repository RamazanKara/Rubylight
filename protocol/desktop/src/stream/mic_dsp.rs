//! Noise suppression and automatic gain for the microphone (Settings → Audio → Noise suppression
//! and Automatic gain), Android's `checkbox_mic_noise_suppression` and `checkbox_mic_auto_gain`.
//! Android turns the system's NoiseSuppressor and AutomaticGainControl effects on; desktop runs
//! WebRTC's noise suppressor (Moderate, about 12 dB) and AGC2's adaptive digital gain through
//! sonora, the pure-Rust port of WebRTC's audio processing that Nightjar Android's echo canceller
//! already uses (BSD-3-Clause; Copyright the WebRTC project authors, Arun Raghavan and
//! contributors, and dignifiedquire; https://github.com/dignifiedquire/sonora;
//! `assets/NOTICE-SONORA.txt`).
//!
//! The microphone's capture thread hands each 10 ms frame over before "Microphone gain" and Opus,
//! after the system's echo cancellation (PipeWire's or PulseAudio's echo-cancel source): echo
//! cancellation, then noise suppression, then automatic gain, WebRTC's order, and the manual gain
//! last, as on Android. A frame comes out in the call it goes in with, so nothing is buffered
//! beyond the algorithms' own delay inside it:
//!
//! - Noise suppression at 48 kHz splits the frame into three 16 kHz bands (WebRTC's three-band
//!   filter bank; its analysis and synthesis together delay the signal by 46 samples, 0.96 ms)
//!   and suppresses in 256-point FFT blocks with a 160-sample hop, whose overlap-add holds back
//!   `FFT_SIZE - NS_FRAME_SIZE` = 96 samples at 16 kHz (6 ms); the upper bands wait as long to
//!   match. Together 334 samples at 48 kHz, 6.96 ms ([`delay_samples`], measured by a test).
//! - Automatic gain alone needs no band split (AGC2 works on the full band) and has no look-ahead:
//!   0 ms. With noise suppression it adds nothing to the split's delay.
//!
//! With both settings off nothing is created and the frames go to Opus as recorded.
//!
//! Windows' Voice Capture DSP, the echo canceller on Windows, has its own noise suppressor and AGC
//! after its echo cancellation; there the two settings turn those on and off (`voice_capture`) and
//! this processor is not made, so a frame is never suppressed twice.

use sonora::config::{AdaptiveDigital, GainController2, MaxProcessingRate, NoiseSuppression, NoiseSuppressionLevel, Pipeline};
use sonora::{AudioProcessing, Config, StreamConfig};

/// What the two settings ask of a microphone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Choice {
    /// Noise suppression: steady background noise out.
    pub noise_suppression: bool,
    /// Automatic gain: quiet and loud speech evened out.
    pub auto_gain: bool,
}

impl Choice {
    pub fn of(settings: &crate::config::Settings) -> Self {
        Self { noise_suppression: settings.mic_noise_suppression, auto_gain: settings.mic_auto_gain }
    }

    /// Whether either is on: only then is there anything to run.
    pub fn any(self) -> bool {
        self.noise_suppression || self.auto_gain
    }
}

/// The delay the processor adds to 48 kHz frames, in samples: with noise suppression the
/// three-band filter bank's 46 (analysis and synthesis together) and the suppressor's 96-sample
/// overlap at the 16 kHz band rate, 3 × 96 at 48 kHz; automatic gain alone adds none.
pub fn delay_samples(choice: Choice) -> usize {
    const FILTER_BANK: usize = 46;
    const OVERLAP: usize = 3 * 96;
    if choice.noise_suppression { FILTER_BANK + OVERLAP } else { 0 }
}

/// Noise suppression, automatic gain or both, on one microphone's 10 ms frames.
pub struct MicDsp {
    apm: AudioProcessing,
    /// Samples a frame: 10 ms at the capture's rate.
    frame: usize,
    /// The frame as it came in: sonora reads one buffer and writes another.
    input: Vec<f32>,
}

impl std::fmt::Debug for MicDsp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MicDsp").field("frame", &self.frame).finish_non_exhaustive()
    }
}

impl MicDsp {
    /// The processor for mono frames at `rate`, or `None` with both settings off.
    pub fn new(choice: Choice, rate: u32) -> Option<Self> {
        if !choice.any() {
            return None;
        }
        let config = Config {
            // The voice keeps its full band: at the default 32 kHz a 48 kHz microphone would be
            // resampled and cut at 16 kHz.
            pipeline: Pipeline { maximum_internal_processing_rate: MaxProcessingRate::Rate48kHz, ..Default::default() },
            noise_suppression: choice.noise_suppression.then(|| NoiseSuppression { level: NoiseSuppressionLevel::Moderate, ..Default::default() }),
            gain_controller2: choice.auto_gain.then(|| GainController2 { adaptive_digital: Some(AdaptiveDigital::default()), ..Default::default() }),
            ..Default::default()
        };
        let stream = StreamConfig::new(rate, 1);
        let apm = AudioProcessing::builder().config(config).capture_config(stream).build();
        let frame = stream.num_frames();
        Some(Self { apm, frame, input: vec![0.0; frame] })
    }

    /// Samples a frame.
    #[cfg(test)]
    pub fn frame(&self) -> usize {
        self.frame
    }

    /// Cleans one 10 ms frame in place. A sample that isn't a number goes in as silence, so one
    /// bad sample cannot stay in the filters' memory.
    pub fn process(&mut self, samples: &mut [f32]) -> Result<(), String> {
        if samples.len() != self.frame {
            return Err(format!("Microphone noise suppression takes {} samples, not {}.", self.frame, samples.len()));
        }
        for (kept, &sample) in self.input.iter_mut().zip(samples.iter()) {
            *kept = if sample.is_finite() { sample } else { 0.0 };
        }
        let input: &[f32] = &self.input;
        self.apm
            .process_capture_f32(&[input], &mut [samples])
            .map_err(|error| format!("Microphone noise suppression failed: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    const RATE: u32 = 48_000;
    const FRAME: usize = 480;

    /// A repeatable white noise in [-1, 1).
    struct Noise(u32);

    impl Noise {
        fn sample(&mut self) -> f32 {
            self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (self.0 >> 8) as f32 / (1u32 << 23) as f32 - 1.0
        }
    }

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32).sqrt()
    }

    fn noise_suppression() -> Choice {
        Choice { noise_suppression: true, auto_gain: false }
    }

    fn auto_gain() -> Choice {
        Choice { noise_suppression: false, auto_gain: true }
    }

    fn both() -> Choice {
        Choice { noise_suppression: true, auto_gain: true }
    }

    /// Runs `signal` through `dsp` frame by frame.
    fn run(dsp: &mut MicDsp, signal: &[f32]) -> Vec<f32> {
        let mut out = signal.to_vec();
        for frame in out.chunks_mut(dsp.frame()) {
            dsp.process(frame).unwrap();
        }
        out
    }

    #[test]
    fn with_both_off_nothing_is_made() {
        assert!(MicDsp::new(Choice::default(), RATE).is_none());
        assert!(MicDsp::new(Choice::default(), 16_000).is_none());
        assert!(!Choice::default().any());
        for choice in [noise_suppression(), auto_gain(), both()] {
            assert!(choice.any());
            assert!(MicDsp::new(choice, RATE).is_some(), "{choice:?}");
        }
    }

    #[test]
    fn the_settings_choose_it() {
        let mut settings = crate::config::Settings { mic_noise_suppression: true, mic_auto_gain: false, ..Default::default() };
        assert_eq!(Choice::of(&settings), noise_suppression());
        settings.mic_noise_suppression = false;
        settings.mic_auto_gain = true;
        assert_eq!(Choice::of(&settings), auto_gain());
    }

    #[test]
    fn a_frame_is_ten_milliseconds_at_either_rate() {
        assert_eq!(MicDsp::new(both(), 48_000).unwrap().frame(), 480);
        assert_eq!(MicDsp::new(both(), 16_000).unwrap().frame(), 160);
        let mut dsp = MicDsp::new(noise_suppression(), RATE).unwrap();
        assert!(dsp.process(&mut [0.0; 160]).is_err(), "a frame of another length is refused, not cut");
        assert!(dsp.process(&mut [0.0; FRAME]).is_ok());
    }

    /// Steady noise with a tone that comes and goes, like a voice over a fan: after the
    /// suppressor has learnt the noise, the pauses are quieter and the tone is still there.
    #[test]
    fn noise_suppression_lowers_steady_noise_and_keeps_the_tone() {
        let mut dsp = MicDsp::new(noise_suppression(), RATE).unwrap();
        let mut noise = Noise(7);
        let (seconds, tone_hz, tone) = (6, 440.0, 0.3);
        // 300 ms of tone in each second, the rest noise alone.
        let tone_on = |i: usize| (300 * 48..600 * 48).contains(&(i % RATE as usize));
        let signal: Vec<f32> = (0..seconds * RATE as usize)
            .map(|i| noise.sample() * 0.02 + if tone_on(i) { (TAU * tone_hz * i as f32 / RATE as f32).sin() * tone } else { 0.0 })
            .collect();
        let out = run(&mut dsp, &signal);
        // The last three seconds: the noise floor in the pauses, skipping 50 ms around each edge.
        let quiet = |s: &[f32]| -> f32 {
            let pause: Vec<f32> = (3 * RATE as usize..signal.len())
                .filter(|&i| (650 * 48..950 * 48).contains(&(i % RATE as usize)))
                .map(|i| s[i])
                .collect();
            rms(&pause)
        };
        let (before, after) = (quiet(&signal[..]), quiet(&out[..]));
        assert!(after < before * 0.5, "noise floor {before} became {after}: at least 6 dB lower (Moderate aims at 12)");
        // The tone's amplitude in its last burst, by projection (the delay only turns its phase).
        let burst = (5 * RATE as usize + 350 * 48)..(5 * RATE as usize + 550 * 48);
        let (mut sin, mut cos) = (0.0f64, 0.0f64);
        for i in burst.clone() {
            let phase = TAU as f64 * tone_hz as f64 * i as f64 / RATE as f64;
            sin += out[i] as f64 * phase.sin();
            cos += out[i] as f64 * phase.cos();
        }
        let amplitude = 2.0 * (sin * sin + cos * cos).sqrt() / burst.len() as f64;
        assert!(amplitude > tone as f64 * 0.5, "the tone came out at {amplitude}, put in at {tone}");
    }

    /// A quiet voice-like buzz (syllables of a 150 Hz buzz with overtones, about -45 dBFS) comes
    /// out louder within the first second: AGC2 starts at 15 dB and only lowers it slowly.
    #[test]
    fn automatic_gain_raises_quiet_speech_within_a_second() {
        let mut dsp = MicDsp::new(auto_gain(), RATE).unwrap();
        let buzz = |i: usize| -> f32 {
            let t = i as f32 / RATE as f32;
            let syllable = (TAU * 4.0 * t).sin().max(0.0);
            (1..=12).map(|k| (TAU * 150.0 * k as f32 * t).sin() / k as f32).sum::<f32>() * 0.004 * syllable
        };
        let signal: Vec<f32> = (0..RATE as usize).map(buzz).collect();
        let out = run(&mut dsp, &signal);
        // Frames 10 to 100: the first 100 ms aside.
        let window = 10 * FRAME..100 * FRAME;
        let (before, after) = (rms(&signal[window.clone()]), rms(&out[window]));
        assert!(after > before * 2.0, "quiet speech at {before} came out at {after}: at least 6 dB louder");
    }

    /// AGC2's limiter keeps a loud voice under full scale instead of boosting it into clipping.
    #[test]
    fn automatic_gain_never_pushes_a_loud_voice_past_full_scale() {
        let mut dsp = MicDsp::new(both(), RATE).unwrap();
        let signal: Vec<f32> = (0..RATE as usize).map(|i| (TAU * 200.0 * i as f32 / RATE as f32).sin() * 0.9).collect();
        let out = run(&mut dsp, &signal);
        assert!(out.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
    }

    #[test]
    fn silence_clipping_and_bad_samples_never_break_it() {
        for choice in [noise_suppression(), auto_gain(), both()] {
            for rate in [48_000u32, 16_000] {
                let mut dsp = MicDsp::new(choice, rate).unwrap();
                let frame = dsp.frame();
                let mut frames: Vec<Vec<f32>> = vec![vec![0.0; frame]; 50];
                frames.extend((0..50).map(|n| (0..frame).map(|i| if (i / 8 + n) % 2 == 0 { 1.0 } else { -1.0 }).collect()));
                frames.extend((0..10).map(|_| (0..frame).map(|i| [1.5, -2.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY][i % 5]).collect()));
                frames.extend((0..50).map(|_| vec![0.0; frame]));
                for (n, mut samples) in frames.into_iter().enumerate() {
                    dsp.process(&mut samples).unwrap();
                    assert!(samples.iter().all(|s| s.is_finite() && s.abs() <= 1.0), "{choice:?} at {rate} Hz, frame {n}: {samples:?}");
                }
            }
        }
    }

    /// The delay the module documentation states, measured: broadband noise through the
    /// suppressor lines up with its input 334 samples (6.96 ms) later, and automatic gain alone
    /// adds none.
    #[test]
    fn the_stated_delay_is_the_measured_delay() {
        assert_eq!(delay_samples(noise_suppression()), 334);
        assert_eq!(delay_samples(both()), 334);
        assert_eq!(delay_samples(auto_gain()), 0);
        for choice in [noise_suppression(), auto_gain()] {
            let mut dsp = MicDsp::new(choice, RATE).unwrap();
            let mut noise = Noise(11);
            // 50 ms noise bursts, so the suppressor never takes them for the steady background.
            let signal: Vec<f32> = (0..3 * RATE as usize).map(|i| if (i / 2400) % 2 == 0 { noise.sample() * 0.3 } else { 0.0 }).collect();
            let out = run(&mut dsp, &signal);
            let scores: Vec<f64> = (0..600)
                .map(|lag| (RATE as usize..signal.len()).map(|i| out[i] as f64 * signal[i - lag] as f64).sum())
                .collect();
            let lag = (0..scores.len()).max_by(|&a, &b| scores[a].total_cmp(&scores[b])).unwrap();
            let expected = delay_samples(choice);
            assert!(lag.abs_diff(expected) <= 1, "{choice:?}: the output lines up {lag} samples late, {expected} stated");
        }
    }
}
