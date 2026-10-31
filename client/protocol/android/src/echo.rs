//! Nightjar's own microphone echo canceller (`com.limelight.binding.audio.EchoCanceller`):
//! WebRTC's AEC3 through sonora, a pure-Rust port of WebRTC's audio processing
//! (BSD-3-Clause, Copyright the WebRTC project authors and the sonora contributors,
//! https://github.com/dignifiedquire/sonora), fed with the stream's sound as the renderer hands
//! it to the output.
//!
//! Android's AcousticEchoCanceler only hears what the device routes into its echo reference,
//! and on some phones that leaves the stream's game-usage output out (issue 56). This one
//! hears exactly what Nightjar plays, on whichever output path.
//!
//! The renderer's thread only queues a mono copy ([`Canceller::reference`]). The microphone's
//! thread hands the queued sound to AEC3 before each of its frames ([`Canceller::process`]), so
//! render and capture reach AEC3 interleaved and from one thread.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::Mutex;

use jni::objects::{JClass, JShortArray};
use jni::sys::{jboolean, jint, jintArray, jlong, JNI_FALSE, JNI_TRUE};
use jni::JNIEnv;
use sonora::config::{EchoCanceller, HighPassFilter, MaxProcessingRate, Pipeline};
use sonora::{AudioProcessing, Config, StreamConfig};

/// The rate of the stream's sound and of the microphone.
pub const RATE: usize = 48_000;
/// AEC3's frame, 10 ms.
pub const FRAME: usize = RATE / 100;
/// Sound queued beyond this means the microphone stopped reading (muted, reopening): the
/// oldest is dropped, it can never be in a frame read later.
const MAX_QUEUED: usize = RATE / 5;
/// From a sound's hand-off to the output to its echo in a read frame, beyond the output's own
/// queue: the device's buffers on both sides. AEC3 finds the delay itself; this only starts it near.
const DEVICE_DELAY_MS: i32 = 20;

struct Reference {
    queued: VecDeque<i16>,
    /// The output's queue ahead of the newest sound, in ms.
    output_ms: i32,
    /// Whether the renderer handed over any sound (passthrough plays none it can hear).
    heard: bool,
}

struct Processing {
    apm: AudioProcessing,
    render: Vec<i16>,
    scratch: Vec<i16>,
}

pub struct Canceller {
    processing: Mutex<Processing>,
    reference: Mutex<Reference>,
}

impl Default for Canceller {
    fn default() -> Self {
        Self::new()
    }
}

impl Canceller {
    pub fn new() -> Self {
        let config = Config {
            // The voice keeps its full band; 32 kHz would cut it at 16 kHz.
            pipeline: Pipeline {
                maximum_internal_processing_rate: MaxProcessingRate::Rate48kHz,
                ..Default::default()
            },
            high_pass_filter: Some(HighPassFilter::default()),
            echo_canceller: Some(EchoCanceller::default()),
            ..Default::default()
        };
        let stream = StreamConfig::new(RATE as u32, 1);
        let apm = AudioProcessing::builder()
            .config(config)
            .capture_config(stream)
            .render_config(stream)
            .build();
        Self {
            processing: Mutex::new(Processing {
                apm,
                render: Vec::with_capacity(MAX_QUEUED),
                scratch: vec![0; FRAME],
            }),
            reference: Mutex::new(Reference {
                queued: VecDeque::with_capacity(MAX_QUEUED + FRAME * 8),
                output_ms: 0,
                heard: false,
            }),
        }
    }

    /// What the renderer just handed to the output: `pcm` interleaved with `channels` channels,
    /// `output_ms` of the output's queue ahead of it. Kept as mono until the microphone reads.
    pub fn reference(&self, pcm: &[i16], channels: usize, output_ms: i32) {
        if channels == 0 {
            return;
        }
        let mut reference = self.reference.lock().unwrap();
        for frame in pcm.chunks_exact(channels) {
            let sum: i32 = frame.iter().map(|&s| i32::from(s)).sum();
            reference.queued.push_back((sum / channels as i32) as i16);
        }
        let excess = reference.queued.len().saturating_sub(MAX_QUEUED);
        reference.queued.drain(..excess);
        reference.output_ms = output_ms.max(0);
        reference.heard |= pcm.len() >= channels;
    }

    /// Drops the queued sound: the microphone starts reading again after a pause, and what
    /// played meanwhile is in none of its frames.
    pub fn restart(&self) {
        self.reference.lock().unwrap().queued.clear();
    }

    /// Takes the stream's sound out of one microphone frame in place. `pcm` is whole 10 ms
    /// frames; anything else is left untouched and returns false.
    pub fn process(&self, pcm: &mut [i16]) -> bool {
        if pcm.is_empty() || pcm.len() % FRAME != 0 {
            return false;
        }
        let mut processing = self.processing.lock().unwrap();
        let Processing {
            apm,
            render,
            scratch,
        } = &mut *processing;
        let output_ms = {
            let mut reference = self.reference.lock().unwrap();
            let whole = reference.queued.len() / FRAME * FRAME;
            render.clear();
            render.extend(reference.queued.drain(..whole));
            reference.output_ms
        };
        for frame in render.chunks_exact(FRAME) {
            // Only the analysis matters; the render output is not played.
            let _ = apm.process_render_i16(frame, scratch);
        }
        // Clamped to AEC3's 0-500 ms; a clamp is no error here.
        let _ = apm.set_stream_delay_ms(output_ms + DEVICE_DELAY_MS);
        for frame in pcm.chunks_exact_mut(FRAME) {
            if apm.process_capture_i16(frame, scratch).is_err() {
                return false;
            }
            frame.copy_from_slice(scratch);
        }
        true
    }

    /// AEC3's echo return loss enhancement in dB and its delay estimate in ms, once it has them,
    /// and whether the renderer handed over any sound at all.
    pub fn stats(&self) -> (Option<f64>, Option<i32>, bool) {
        let heard = self.reference.lock().unwrap().heard;
        let processing = self.processing.lock().unwrap();
        let stats = processing.apm.statistics();
        (stats.echo_return_loss_enhancement, stats.delay_ms, heard)
    }
}

thread_local! {
    static SCRATCH: RefCell<Vec<i16>> = const { RefCell::new(Vec::new()) };
}

/// # Safety
/// `handle` is 0 or a live handle from `nativeCreate`; EchoCanceller.java keeps it so.
unsafe fn canceller<'a>(handle: jlong) -> Option<&'a Canceller> {
    (handle as *const Canceller).as_ref()
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_binding_audio_EchoCanceller_nativeCreate(
    _: JNIEnv,
    _: JClass,
) -> jlong {
    Box::into_raw(Box::new(Canceller::new())) as jlong
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_binding_audio_EchoCanceller_nativeReference(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
    pcm: JShortArray,
    length: jint,
    channels: jint,
    output_ms: jint,
) {
    // SAFETY: EchoCanceller.java passes a live handle under its reference lock.
    let Some(canceller) = (unsafe { canceller(handle) }) else {
        return;
    };
    if pcm.is_null()
        || length <= 0
        || channels <= 0
        || env.get_array_length(&pcm).unwrap_or(0) < length
    {
        return;
    }
    SCRATCH.with_borrow_mut(|samples| {
        samples.resize(length as usize, 0);
        if env.get_short_array_region(&pcm, 0, samples).is_ok() {
            canceller.reference(samples, channels as usize, output_ms);
        }
    });
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_binding_audio_EchoCanceller_nativeProcess(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
    pcm: JShortArray,
    samples: jint,
) -> jboolean {
    // SAFETY: EchoCanceller.java passes a live handle under its capture lock.
    let Some(canceller) = (unsafe { canceller(handle) }) else {
        return JNI_FALSE;
    };
    if pcm.is_null() || samples <= 0 || env.get_array_length(&pcm).unwrap_or(0) < samples {
        return JNI_FALSE;
    }
    SCRATCH.with_borrow_mut(|frame| {
        frame.resize(samples as usize, 0);
        let done = env.get_short_array_region(&pcm, 0, frame).is_ok()
            && canceller.process(frame)
            && env.set_short_array_region(&pcm, 0, frame).is_ok();
        if done {
            JNI_TRUE
        } else {
            JNI_FALSE
        }
    })
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_binding_audio_EchoCanceller_nativeRestart(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    // SAFETY: as in nativeProcess.
    if let Some(canceller) = unsafe { canceller(handle) } {
        canceller.restart();
    }
}

/// `{ERLE in tenths of a dB, delay in ms}`, -1 for what AEC3 has not estimated yet.
#[no_mangle]
pub extern "system" fn Java_com_limelight_binding_audio_EchoCanceller_nativeStats(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jintArray {
    // SAFETY: as in nativeProcess.
    let (erle, delay, heard) =
        unsafe { canceller(handle) }.map_or((None, None, false), Canceller::stats);
    // {ERLE measured, ERLE in tenths of a dB (negative when it added echo), delay or -1, heard}.
    let erle = erle.filter(|value| value.is_finite());
    let values = [
        jint::from(erle.is_some()),
        erle.map_or(0, |value| (value * 10.0).round() as jint),
        delay.unwrap_or(-1),
        jint::from(heard),
    ];
    match env.new_int_array(values.len() as jint) {
        Ok(array) => {
            let _ = env.set_int_array_region(&array, 0, &values);
            array.into_raw()
        }
        Err(_) => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_binding_audio_EchoCanceller_nativeDestroy(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    if handle != 0 {
        // SAFETY: EchoCanceller.java destroys a handle once, holding both its locks, after
        // which no call can pass it again.
        drop(unsafe { Box::from_raw(handle as *mut Canceller) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic noise generator: broadband like music, repeatable.
    struct Noise(u64);
    impl Noise {
        fn next(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((self.0 >> 33) as f64 / (1u64 << 31) as f64) * 2.0 - 1.0
        }
    }

    fn energy(pcm: &[i16]) -> f64 {
        pcm.iter()
            .map(|&s| f64::from(s) * f64::from(s))
            .sum::<f64>()
            / pcm.len().max(1) as f64
    }

    fn db(ratio: f64) -> f64 {
        10.0 * ratio.log10()
    }

    /// Runs `seconds` of stereo game sound through the reference and its echo (delayed by
    /// `delay_ms` behind the hand-off, scaled by `gain`) plus `near` through the microphone,
    /// in 20 ms microphone frames with the renderer's 10 ms writes between them. Returns the
    /// microphone's input and output over the last second.
    fn run(
        seconds: usize,
        delay_ms: usize,
        gain: f64,
        near: &dyn Fn(usize) -> f64,
    ) -> (Vec<i16>, Vec<i16>) {
        run_stalled(seconds, delay_ms, gain, near, (0, 0))
    }

    /// [`run`] with the renderer stalling: for `stall.1` of every `stall.0` microphone frames
    /// its writes wait and then arrive at once, while the output's queue keeps the echo going.
    fn run_stalled(
        seconds: usize,
        delay_ms: usize,
        gain: f64,
        near: &dyn Fn(usize) -> f64,
        stall: (usize, usize),
    ) -> (Vec<i16>, Vec<i16>) {
        let canceller = Canceller::new();
        let mut noise = Noise(56);
        let total = seconds * RATE;
        let game: Vec<i16> = (0..total).map(|_| (noise.next() * 6000.0) as i16).collect();
        let delay = delay_ms * RATE / 1000;
        let mic_frame = 2 * FRAME;
        let (mut input, mut output) = (Vec::new(), Vec::new());
        let mut waiting: Vec<Vec<i16>> = Vec::new();
        for (index, start) in (0..total).step_by(mic_frame).enumerate() {
            for half in [start, start + FRAME] {
                waiting.push(
                    game[half..half + FRAME]
                        .iter()
                        .flat_map(|&s| [s, s])
                        .collect(),
                );
            }
            if stall.0 == 0 || index % stall.0 >= stall.1 {
                for stereo in waiting.drain(..) {
                    canceller.reference(&stereo, 2, 40);
                }
            }
            let mut frame: Vec<i16> = (start..start + mic_frame)
                .map(|i| {
                    let echo = if i >= delay {
                        f64::from(game[i - delay]) * gain
                    } else {
                        0.0
                    };
                    (echo + near(i)).clamp(-32767.0, 32767.0) as i16
                })
                .collect();
            let before = frame.clone();
            assert!(canceller.process(&mut frame));
            if start + mic_frame > total - RATE {
                input.extend(before);
                output.extend(frame);
            }
        }
        (input, output)
    }

    #[test]
    fn the_streams_sound_comes_out_of_the_microphone() {
        // A loud echo 70 ms behind the hand-off, as a phone speaker next to its microphone.
        let (input, output) = run(6, 70, 0.8, &|_| 0.0);
        let removed = db(energy(&input) / energy(&output).max(1.0));
        assert!(removed > 20.0, "only {removed:.1} dB of echo removed");
    }

    #[test]
    fn a_renderer_that_stalls_still_has_its_sound_removed() {
        // The network holds the stream's sound for 60 ms every second, then delivers it at once.
        let (input, output) = run_stalled(6, 70, 0.8, &|_| 0.0, (50, 3));
        let removed = db(energy(&input) / energy(&output).max(1.0));
        assert!(removed > 20.0, "only {removed:.1} dB of echo removed");
    }

    #[test]
    fn a_voice_with_nothing_playing_passes() {
        let canceller = Canceller::new();
        let tone =
            |i: usize| (2.0 * std::f64::consts::PI * 300.0 * i as f64 / RATE as f64).sin() * 8000.0;
        let mut kept = Vec::new();
        let mut sent = Vec::new();
        for start in (0..3 * RATE).step_by(2 * FRAME) {
            let mut frame: Vec<i16> = (start..start + 2 * FRAME).map(|i| tone(i) as i16).collect();
            let before = frame.clone();
            assert!(canceller.process(&mut frame));
            if start >= 2 * RATE {
                sent.extend(before);
                kept.extend(frame);
            }
        }
        let change = db(energy(&kept) / energy(&sent));
        assert!(change.abs() < 1.0, "the voice changed by {change:.1} dB");
    }

    #[test]
    fn a_voice_over_the_echo_survives() {
        let voice =
            |i: usize| (2.0 * std::f64::consts::PI * 440.0 * i as f64 / RATE as f64).sin() * 6000.0;
        let (_, output) = run(6, 70, 0.8, &|i| voice(i));
        let voice_only: Vec<i16> = (5 * RATE..6 * RATE).map(|i| voice(i) as i16).collect();
        let kept = db(energy(&output) / energy(&voice_only));
        // Double talk: AEC3 suppresses some voice while the echo is loud, but never most of it.
        assert!(kept > -10.0, "the voice dropped by {:.1} dB", -kept);
    }

    #[test]
    fn a_microphone_that_stopped_reading_keeps_only_recent_sound() {
        let canceller = Canceller::new();
        assert!(!canceller.stats().2, "nothing handed over yet");
        // A mono ramp: the newest MAX_QUEUED samples stay, the oldest go.
        let second: Vec<i16> = (0..RATE).map(|i| (i % 30000) as i16).collect();
        canceller.reference(&second, 1, 0);
        {
            let reference = canceller.reference.lock().unwrap();
            assert_eq!(reference.queued.len(), MAX_QUEUED);
            assert_eq!(reference.queued.front(), Some(&second[RATE - MAX_QUEUED]));
            assert_eq!(reference.queued.back(), second.last());
        }
        assert!(canceller.stats().2);
        canceller.restart();
        assert!(canceller.reference.lock().unwrap().queued.is_empty());
    }

    #[test]
    fn frames_that_are_not_whole_10_ms_are_left_alone() {
        let canceller = Canceller::new();
        let mut frame = vec![123i16; FRAME + 1];
        assert!(!canceller.process(&mut frame));
        assert!(frame.iter().all(|&s| s == 123));
        assert!(!canceller.process(&mut []));
    }

    #[test]
    fn the_reference_is_the_average_of_the_channels() {
        let canceller = Canceller::new();
        canceller.reference(&[100, 300, -50, -150, 7], 2, 12);
        let reference = canceller.reference.lock().unwrap();
        assert_eq!(
            reference.queued.iter().copied().collect::<Vec<_>>(),
            vec![200, -100]
        );
        assert_eq!(reference.output_ms, 12);
    }
}
