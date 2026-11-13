//! Haptics from game audio (Experimental, `docs/dualsense-haptics.md`): while a
//! game sends a client's DualSense no waveform or rumble, the low band of the
//! stream's own audio drives its two actuators, and transients add a short thump.
//!
//! The audio thread copies each packet it has already sent into the session's
//! [`Tap`], a lock-free ring, and only while a controller wants it: one atomic
//! load otherwise, never a lock, an allocation or a wait, so audio gains no
//! latency. The control loop's [`Arbiter`] analyses that copy into 0x5601
//! frames for the controllers without game haptics and stops the instant the
//! game's own waveform or rumble appears.
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};

use crate::config::{Config, parse_boolean, parse_integer};
use crate::haptics;
use crate::input::Input;

pub const ENABLED: &str = "audio_haptics";
pub const STRENGTH: &str = "audio_haptics_strength";
pub const CURVE: &str = "audio_haptics_curve";
pub const GATE: &str = "audio_haptics_gate";
/// The settings. A global or per-app save applies to the next packet; a
/// per-device override from the client's next stream (its record is the launch's).
pub const SETTINGS: [&str; 4] = [ENABLED, STRENGTH, CURVE, GATE];
/// Strength in percent of the default mapping.
pub const STRENGTHS: std::ops::RangeInclusive<i64> = 0..=200;
/// The noise gate in dBFS of the low band.
pub const GATES: std::ops::RangeInclusive<i64> = -80..=-10;

/// How long the game's own haptics or rumble keep audio haptics off for that
/// controller. Games that rumble only on hits stay with their own feel.
pub const GAME_HOLD: Duration = Duration::from_secs(60);
/// Controller feedback that counts as the game's own haptics.
const RUMBLE: u16 = 0x010b;
const TRIGGER_RUMBLE: u16 = 0x5500;
/// Real waveform samples this small (about -54 dBFS) are silence: an idle
/// actuator endpoint, not game haptics.
const SILENT: u16 = 64;

const RATE: f32 = 48_000.;
/// The band the actuators play: a 20 Hz high-pass and a 4th-order 150 Hz low-pass.
const HIGH_PASS_HZ: f32 = 20.;
const LOW_PASS_HZ: f32 = 150.;
/// The band's peak envelope, which the gate and the curve read.
const ENVELOPE_ATTACK: f32 = 0.001;
const ENVELOPE_RELEASE: f32 = 0.06;
/// The gate ramps open in 2 ms above the threshold and shut in 30 ms 3 dB below it.
const GATE_OPEN: f32 = 1. / (0.002 * RATE);
const GATE_CLOSE: f32 = 1. / (0.03 * RATE);
const GATE_HYSTERESIS: f32 = 0.707_945_8;
/// A band envelope this high (-12 dBFS) plays at full scale at 100 %.
const FULL_IN: f32 = 0.25;
/// The soft curve lifts quiet bass by at most 12 dB.
const MAX_BOOST: f32 = 4.;
/// Transients: a fast broadband envelope three times the slow one, above the
/// gate and 40 ms after the last, starts a decaying 100 Hz thump as strong as
/// the transient's peak.
const FAST_ATTACK: f32 = 0.0005;
const FAST_RELEASE: f32 = 0.01;
const SLOW_ATTACK: f32 = 0.03;
const SLOW_RELEASE: f32 = 0.3;
const ONSET_RATIO: f32 = 3.;
const REFRACTORY_FRAMES: u32 = 1_920;
const THUMP_HZ: f32 = 100.;
const THUMP_DECAY: f32 = 0.012;
const THUMP_FRAMES: usize = 1_920;
const THUMP_GAIN: f32 = 0.5;

/// The ring holds about 43 ms; the analysis takes at most the newest 20 ms
/// ([`haptics::MAX_AGE`]), so a late control pass never plays old audio.
const RING: usize = 2_048;
const KEEP: usize = 960;

const FLUSH_BELOW: f32 = 1e-30;

fn flush(x: f32) -> f32 {
    if x.abs() < FLUSH_BELOW { 0. } else { x }
}

fn coefficient(seconds: f32) -> f32 {
    (-1. / (seconds * RATE)).exp()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Curve {
    /// Actuator level follows the bass level.
    #[default]
    Linear,
    /// Quiet bass is lifted (square root), for subtle soundtracks.
    Soft,
    /// Quiet bass is pushed down (square), so only big hits come through.
    Punchy,
}

impl Curve {
    pub const NAMES: [&'static str; 3] = ["linear", "soft", "punchy"];

    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "linear" => Some(Self::Linear),
            "soft" => Some(Self::Soft),
            "punchy" => Some(Self::Punchy),
            _ => None,
        }
    }

    /// The gain on top of the linear mapping for an envelope `level` (1 = full scale).
    fn gain(self, level: f32) -> f32 {
        match self {
            Self::Linear => 1.,
            Self::Soft => (1. / level.max(f32::MIN_POSITIVE).sqrt()).min(MAX_BOOST),
            Self::Punchy => level.min(1.),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    pub enabled: bool,
    /// Percent, 0 to 200.
    pub strength: u16,
    pub curve: Curve,
    /// dBFS, -80 to -10.
    pub gate_db: i16,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            strength: 100,
            curve: Curve::Linear,
            gate_db: -50,
        }
    }
}

impl Settings {
    /// The settings in `config`. A value the console would refuse is an error,
    /// not a default: the caller says so and keeps audio haptics off.
    pub fn from_config(config: &Config) -> Result<Self> {
        let default = Self::default();
        for key in SETTINGS {
            if let Some(value) = config.values.get(key).filter(|v| !v.trim().is_empty()) {
                check(key, value)?;
            }
        }
        Ok(Self {
            enabled: config.boolean(ENABLED, default.enabled),
            strength: config.integer(STRENGTH, default.strength.into()) as u16,
            curve: Curve::parse(config.get(CURVE, "linear")).unwrap_or_default(),
            gate_db: config.integer(GATE, default.gate_db.into()) as i16,
        })
    }
}

/// Refuses a value of one of [`SETTINGS`] the host would not use.
pub fn check(key: &str, value: &str) -> Result<()> {
    match key {
        ENABLED if parse_boolean(value).is_none() => bail!("{key} must be true or false"),
        STRENGTH if !parse_integer(value).is_some_and(|v| STRENGTHS.contains(&v)) => {
            bail!("{key} must be between 0 and 200 percent")
        }
        CURVE if Curve::parse(value).is_none() => bail!("{key} must be linear, soft or punchy"),
        GATE if !parse_integer(value).is_some_and(|v| GATES.contains(&v)) => {
            bail!("{key} must be between -80 and -10 dBFS")
        }
        _ => Ok(()),
    }
}

/// The session's copy of its sent audio, as the two actuator lanes: one
/// producer (the audio thread) and one consumer (the control thread).
pub struct Tap {
    wanted: AtomicBool,
    /// The producer found the ring full; the consumer drops what is in it.
    overflow: AtomicBool,
    written: AtomicUsize,
    read: AtomicUsize,
    /// Left and right f32 bits, packed.
    ring: Box<[AtomicU64]>,
}

impl Default for Tap {
    fn default() -> Self {
        Self {
            wanted: AtomicBool::new(false),
            overflow: AtomicBool::new(false),
            written: AtomicUsize::new(0),
            read: AtomicUsize::new(0),
            ring: (0..RING).map(|_| AtomicU64::new(0)).collect(),
        }
    }
}

/// Moonlight's channel order: FL FR FC LFE BL BR SL SR.
fn lanes(frame: &[f32]) -> (f32, f32) {
    let at = |i: usize| frame.get(i).copied().unwrap_or(0.);
    match frame.len() {
        0 => (0., 0.),
        1 => (at(0), at(0)),
        2..=5 => (at(0), at(1)),
        _ => {
            let lfe = 0.7 * at(3);
            (
                at(0) + lfe + 0.5 * (at(4) + at(6)),
                at(1) + lfe + 0.5 * (at(5) + at(7)),
            )
        }
    }
}

fn pack((left, right): (f32, f32)) -> u64 {
    u64::from(left.to_bits()) | u64::from(right.to_bits()) << 32
}

fn unpack(bits: u64) -> (f32, f32) {
    (
        f32::from_bits(bits as u32),
        f32::from_bits((bits >> 32) as u32),
    )
}

impl Tap {
    /// Audio thread: copies a packet of interleaved `channels` it has sent.
    /// Does nothing while no controller wants it; drops the packet when the
    /// consumer is behind. Never blocks or allocates.
    pub fn push(&self, packet: &[f32], channels: usize) {
        if channels == 0 || !self.wanted.load(Ordering::Relaxed) {
            return;
        }
        let written = self.written.load(Ordering::Relaxed);
        let frames = packet.len() / channels;
        let used = written.wrapping_sub(self.read.load(Ordering::Acquire));
        if frames > RING - used {
            self.overflow.store(true, Ordering::Release);
            return;
        }
        for (i, frame) in packet.chunks_exact(channels).enumerate() {
            self.ring[written.wrapping_add(i) % RING].store(pack(lanes(frame)), Ordering::Relaxed);
        }
        self.written
            .store(written.wrapping_add(frames), Ordering::Release);
    }

    /// Control thread: appends the frames since the last take, at most the
    /// newest 20 ms. False when frames were dropped in between.
    fn take(&self, out: &mut Vec<(f32, f32)>) -> bool {
        let written = self.written.load(Ordering::Acquire);
        let mut read = self.read.load(Ordering::Relaxed);
        let mut continuous = true;
        if self.overflow.swap(false, Ordering::AcqRel) {
            read = written;
            continuous = false;
        } else if written.wrapping_sub(read) > KEEP {
            read = written.wrapping_sub(KEEP);
            continuous = false;
        }
        while read != written {
            out.push(unpack(self.ring[read % RING].load(Ordering::Relaxed)));
            read = read.wrapping_add(1);
        }
        self.read.store(written, Ordering::Release);
        continuous
    }

    /// Control thread: starts or stops the copy. A restart begins with fresh audio.
    fn set_wanted(&self, wanted: bool) {
        if wanted && !self.wanted.swap(true, Ordering::AcqRel) {
            self.read
                .store(self.written.load(Ordering::Acquire), Ordering::Release);
            self.overflow.store(false, Ordering::Release);
        } else if !wanted {
            self.wanted.store(false, Ordering::Release);
        }
    }

    /// Whether the audio thread copies packets.
    pub fn wanted(&self) -> bool {
        self.wanted.load(Ordering::Acquire)
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    /// A Butterworth (Q 0.707) section, RBJ cookbook.
    fn new(hz: f32, high: bool) -> Self {
        let (sin, cos) = (std::f32::consts::TAU * hz / RATE).sin_cos();
        let alpha = sin * std::f32::consts::FRAC_1_SQRT_2;
        let a0 = 1. + alpha;
        let (b0, b1) = if high {
            ((1. + cos) / 2., -(1. + cos))
        } else {
            ((1. - cos) / 2., 1. - cos)
        };
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b0 / a0,
            a1: -2. * cos / a0,
            a2: (1. - alpha) / a0,
            z1: 0.,
            z2: 0.,
        }
    }

    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = flush(self.b1 * x - self.a1 * y + self.z2);
        self.z2 = flush(self.b2 * x - self.a2 * y);
        y
    }
}

/// A peak follower with separate attack and release coefficients.
fn follow(envelope: f32, x: f32, attack: f32, release: f32) -> f32 {
    let k = if x > envelope { attack } else { release };
    flush(x + (envelope - x) * k)
}

/// The settings as the lanes use them.
#[derive(Clone, Copy, Debug)]
struct Shape {
    strength: f32,
    curve: Curve,
    gate: f32,
}

impl From<Settings> for Shape {
    fn from(settings: Settings) -> Self {
        Self {
            strength: f32::from(settings.strength) / 100.,
            curve: settings.curve,
            gate: 10f32.powf(f32::from(settings.gate_db) / 20.),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Coefficients {
    envelope: (f32, f32),
    fast: (f32, f32),
    slow: (f32, f32),
}

#[derive(Clone, Copy, Debug)]
struct Lane {
    high: Biquad,
    low: [Biquad; 2],
    envelope: f32,
    gate: f32,
    open: bool,
    fast: f32,
    slow: f32,
    since_onset: u32,
    thump: usize,
    /// The transient's peak so far.
    thump_peak: f32,
    onsets: u32,
}

impl Lane {
    fn new() -> Self {
        Self {
            high: Biquad::new(HIGH_PASS_HZ, true),
            low: [Biquad::new(LOW_PASS_HZ, false); 2],
            envelope: 0.,
            gate: 0.,
            open: false,
            fast: 0.,
            slow: 0.,
            since_onset: REFRACTORY_FRAMES,
            thump: THUMP_FRAMES,
            thump_peak: 0.,
            onsets: 0,
        }
    }

    fn process(&mut self, x: f32, shape: Shape, k: &Coefficients, thump: &[f32]) -> f32 {
        let x = if x.is_finite() { x } else { 0. };
        let band = self
            .low
            .iter_mut()
            .fold(self.high.process(x), |x, low| low.process(x));
        self.envelope = follow(self.envelope, band.abs(), k.envelope.0, k.envelope.1);
        if self.envelope > shape.gate {
            self.open = true;
        } else if self.envelope < shape.gate * GATE_HYSTERESIS {
            self.open = false;
        }
        self.gate = if self.open {
            (self.gate + GATE_OPEN).min(1.)
        } else {
            (self.gate - GATE_CLOSE).max(0.)
        };
        let level = self.envelope / FULL_IN;
        let mut y = band / FULL_IN * shape.strength * shape.curve.gain(level) * self.gate;

        self.fast = follow(self.fast, x.abs(), k.fast.0, k.fast.1);
        self.slow = follow(self.slow, x.abs(), k.slow.0, k.slow.1);
        self.since_onset = self.since_onset.saturating_add(1);
        if self.since_onset >= REFRACTORY_FRAMES
            && self.fast > shape.gate
            && self.fast > ONSET_RATIO * self.slow
        {
            self.since_onset = 0;
            self.thump = 0;
            self.thump_peak = 0.;
            self.onsets = self.onsets.wrapping_add(1);
        }
        if let Some(sample) = thump.get(self.thump) {
            self.thump_peak = self.thump_peak.max(self.fast);
            let level = (self.thump_peak / FULL_IN).min(1.);
            y += THUMP_GAIN * shape.strength * (level * shape.curve.gain(level)).min(1.) * sample;
            self.thump += 1;
        }
        if !y.is_finite() || !self.envelope.is_finite() {
            *self = Self::new();
            return 0.;
        }
        y.clamp(-1., 1.)
    }
}

/// The DSP: lane pairs in, actuator stereo S16LE out, one frame per frame.
#[derive(Clone, Debug)]
pub struct Analyzer {
    settings: Settings,
    shape: Shape,
    k: Coefficients,
    lanes: [Lane; 2],
    thump: Box<[f32]>,
}

impl Default for Analyzer {
    fn default() -> Self {
        let settings = Settings::default();
        let pair = |attack, release| (coefficient(attack), coefficient(release));
        Self {
            settings,
            shape: settings.into(),
            k: Coefficients {
                envelope: pair(ENVELOPE_ATTACK, ENVELOPE_RELEASE),
                fast: pair(FAST_ATTACK, FAST_RELEASE),
                slow: pair(SLOW_ATTACK, SLOW_RELEASE),
            },
            lanes: [Lane::new(); 2],
            thump: (0..THUMP_FRAMES)
                .map(|n| {
                    let t = n as f32 / RATE;
                    (std::f32::consts::TAU * THUMP_HZ * t).sin() * (-t / THUMP_DECAY).exp()
                })
                .collect(),
        }
    }
}

impl Analyzer {
    /// Applies from the next frame on; the filters keep their state.
    pub fn set(&mut self, settings: Settings) {
        self.settings = settings;
        self.shape = settings.into();
    }

    pub fn settings(&self) -> Settings {
        self.settings
    }

    fn reset(&mut self) {
        self.lanes = [Lane::new(); 2];
    }

    /// Appends one actuator frame (left, right S16LE) per lane pair.
    pub fn process(&mut self, input: &[(f32, f32)], out: &mut Vec<u8>) {
        out.reserve(input.len() * 4);
        for &(left, right) in input {
            for (lane, x) in self.lanes.iter_mut().zip([left, right]) {
                let y = lane.process(x, self.shape, &self.k, &self.thump);
                out.extend_from_slice(&((y * 32767.).round() as i16).to_le_bytes());
            }
        }
    }
}

/// Whether wrapping sequence `a` is newer than `b`, as the client checks.
fn newer(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) > 0
}

#[derive(Clone, Copy, Debug, Default)]
struct Slot {
    /// When the game last sent this controller haptics or rumble.
    game: Option<Instant>,
    /// The last 0x5601 sequence sent for it, real or generated.
    last: Option<u32>,
    /// Added to the pad's own sequences once generated frames went out, so
    /// the client's newer-sequence check keeps passing the game's frames.
    offset: u32,
    generated: bool,
}

/// One control peer's audio haptics: which controllers get generated frames,
/// and the switch to the game's own haptics.
#[derive(Default)]
pub struct Arbiter {
    slots: [Slot; 16],
    /// Controllers whose client reported waveform haptics, and those connected.
    waveform: u16,
    active: u16,
    generating: u16,
    analyzer: Analyzer,
    packetizer: haptics::Packetizer,
    lanes: Vec<(f32, f32)>,
    stereo: Vec<u8>,
    at: Option<Instant>,
}

impl Arbiter {
    pub fn set_settings(&mut self, settings: Settings) {
        self.analyzer.set(settings);
    }

    pub fn settings(&self) -> Settings {
        self.analyzer.settings()
    }

    /// Follows arrivals (after the host removed an unnegotiated waveform
    /// capability) and the client's connected-controller mask.
    pub fn input(&mut self, event: &Input) {
        match *event {
            Input::Arrival {
                id, capabilities, ..
            } if id < 16 => {
                let bit = 1 << id;
                if capabilities & haptics::CAPABILITY != 0 {
                    self.waveform |= bit;
                } else {
                    self.waveform &= !bit;
                }
                self.active |= bit;
            }
            Input::Controller { active, .. } => self.active = active,
            _ => {}
        }
    }

    /// The controllers that could get audio haptics now.
    pub fn controllers(&self) -> u16 {
        self.waveform & self.active
    }

    /// The game's own waveform frame. Audible, it stops audio haptics for its
    /// controller at once; silent while they play, it is dropped (false) so
    /// the two never interleave. With audio haptics never used for that
    /// controller, the frame is left as it is.
    pub fn game_frame(&mut self, frame: &mut haptics::Frame, now: Instant) -> bool {
        let Some(slot) = self.slots.get_mut(usize::from(frame.id)) else {
            return true;
        };
        let bit = 1 << frame.id;
        let audible = frame
            .samples
            .as_chunks::<2>()
            .0
            .iter()
            .any(|sample| i16::from_le_bytes(*sample).unsigned_abs() > SILENT);
        if audible {
            slot.game = Some(now);
            self.generating &= !bit;
        } else if self.generating & bit != 0 {
            return false;
        }
        if slot.generated {
            if let Some(last) = slot.last
                && !newer(frame.sequence.wrapping_add(slot.offset), last)
            {
                slot.offset = last.wrapping_add(1).wrapping_sub(frame.sequence);
            }
            frame.sequence = frame.sequence.wrapping_add(slot.offset);
        }
        slot.last = Some(frame.sequence);
        true
    }

    /// The game's controller feedback after `feedback_packets`: rumble or
    /// trigger rumble other than "stop" counts as its own haptics.
    pub fn game_feedback(&mut self, kind: u16, payload: &[u8], now: Instant) {
        let (id, motors) = match kind {
            RUMBLE => (payload.get(4..6), payload.get(6..10)),
            TRIGGER_RUMBLE => (payload.get(..2), payload.get(2..6)),
            _ => return,
        };
        if let (Some(id), Some(motors)) = (id, motors)
            && motors.iter().any(|&b| b != 0)
        {
            let id = u16::from_le_bytes([id[0], id[1]]);
            if let Some(slot) = self.slots.get_mut(usize::from(id)) {
                slot.game = Some(now);
                self.generating &= !(1 << id);
            }
        }
    }

    /// Frames for the controllers in `allowed` (the host forwards haptics to
    /// this peer) that have waveform haptics and no game haptics within
    /// [`GAME_HOLD`]. Starts or stops the tap's copy to match. Silent frames
    /// are not sent.
    pub fn generate(&mut self, allowed: bool, tap: &Tap, now: Instant) -> Vec<haptics::Frame> {
        let mut generating = 0;
        if allowed && self.analyzer.settings.enabled {
            for (id, slot) in self.slots.iter().enumerate() {
                let bit = 1 << id;
                if self.controllers() & bit != 0
                    && !slot
                        .game
                        .is_some_and(|at| now.saturating_duration_since(at) < GAME_HOLD)
                {
                    generating |= bit;
                }
            }
        }
        if generating != 0 && self.generating == 0 {
            self.analyzer.reset();
            self.packetizer.clear();
        }
        self.generating = generating;
        tap.set_wanted(generating != 0);
        if generating == 0 {
            self.at = None;
            return vec![];
        }
        self.at = Some(now);
        self.lanes.clear();
        if !tap.take(&mut self.lanes) {
            self.packetizer.clear();
        }
        self.stereo.clear();
        self.analyzer.process(&self.lanes, &mut self.stereo);
        let mut frames = vec![];
        for samples in self.packetizer.push(&self.stereo, now) {
            if samples.iter().all(|&b| b == 0) {
                continue;
            }
            for (id, slot) in self.slots.iter_mut().enumerate() {
                if generating & (1 << id) == 0 {
                    continue;
                }
                let sequence = slot.last.map_or(0, |last| last.wrapping_add(1));
                slot.last = Some(sequence);
                slot.generated = true;
                frames.push(haptics::Frame {
                    id: id as u16,
                    sequence,
                    created: now,
                    samples: samples.clone(),
                });
            }
        }
        frames
    }

    /// While generating, the next pass is due one 5 ms frame after the last.
    pub fn next_due(&self) -> Option<Instant> {
        self.at
            .map(|at| at + Duration::from_millis(haptics::FRAMES as u64 / 48))
    }

    /// The controllers generated for in the last pass.
    pub fn generating(&self) -> u16 {
        self.generating
    }

    /// The peer is gone: the audio thread stops copying if it copied for it.
    pub fn release(&mut self, tap: &Tap) {
        if self.generating != 0 {
            self.generating = 0;
            self.at = None;
            tap.set_wanted(false);
        }
    }
}

#[cfg(test)]
mod tests;
