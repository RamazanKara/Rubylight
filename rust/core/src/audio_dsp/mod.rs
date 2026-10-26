//! Couch audio processing between capture and Opus: night mode and headphone surround (`docs/wake-and-audio-design.md`).
//!
//! [`Chain`] processes each packet in place on the audio thread, without
//! buffering: with both settings off it leaves the samples untouched.
//! [`Controls`] on the session carry the live choice and the inactive
//! reasons; [`state`] and [`apply`] are what the native API and the couch menu
//! call.
mod binaural;
mod hrir;
mod night;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use anyhow::Result;
use rubylight_native_api::audio::{self as api, Audio, AudioWrite};
use rubylight_native_api::error::{ApiError, ErrorCode};
use rubylight_native_api::radeon::{ControlState, Kind, Setting, Value};

use binaural::{Binaural, Layout};
use night::Night;

/// The capture channels headphone surround renders from.
pub const SURROUND_CHANNELS: usize = 8;
/// A switch between the HRIRs and the plain downmix crossfades over one 5 ms
/// packet.
const CROSSFADE_FRAMES: u32 = 240;
/// The session warning code once capture delivered NaN or infinite samples.
pub const BAD_SAMPLES_WARNING: &str = "audio_bad_samples";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    pub night_mode: bool,
    pub headphone_surround: bool,
}

/// The two settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    NightMode,
    HeadphoneSurround,
}

impl Key {
    pub const ALL: [Key; 2] = [Key::NightMode, Key::HeadphoneSurround];

    /// The native API id.
    pub fn id(self) -> &'static str {
        match self {
            Self::NightMode => api::NIGHT_MODE,
            Self::HeadphoneSurround => api::HEADPHONE_SURROUND,
        }
    }

    /// The host, device and app config key.
    pub fn config(self) -> &'static str {
        match self {
            Self::NightMode => "audio_night_mode",
            Self::HeadphoneSurround => "audio_headphone_surround",
        }
    }

    /// The `/launch` and `/resume` parameter.
    pub fn parameter(self) -> &'static str {
        match self {
            Self::NightMode => "audioNightMode",
            Self::HeadphoneSurround => "audioHeadphoneSurround",
        }
    }

    /// The session warning code while it is on but doesn't apply.
    pub fn warning(self) -> &'static str {
        self.config()
    }

    fn of(self, settings: Settings) -> bool {
        match self {
            Self::NightMode => settings.night_mode,
            Self::HeadphoneSurround => settings.headphone_surround,
        }
    }
}

/// Why a setting doesn't apply to this stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Inactive {
    ClientSurround = 1,
    GameOnlyAudio,
    StereoEndpoint,
    Muted,
    NextStream,
}

impl Inactive {
    const ALL: [Inactive; 5] = [
        Self::ClientSurround,
        Self::GameOnlyAudio,
        Self::StereoEndpoint,
        Self::Muted,
        Self::NextStream,
    ];

    pub fn code(self) -> &'static str {
        match self {
            Self::ClientSurround => api::reason::CLIENT_SURROUND,
            Self::GameOnlyAudio => api::reason::GAME_ONLY_AUDIO,
            Self::StereoEndpoint => api::reason::STEREO_ENDPOINT,
            Self::Muted => api::reason::MUTED,
            Self::NextStream => api::reason::NEXT_STREAM,
        }
    }

    pub fn message(self, key: Key) -> &'static str {
        match (self, key) {
            (Self::Muted, Key::NightMode) => {
                "Night mode doesn't apply: audio isn't being streamed."
            }
            (Self::Muted, Key::HeadphoneSurround) => {
                "Headphone surround doesn't apply: audio isn't being streamed."
            }
            (Self::ClientSurround, _) => {
                "Headphone surround doesn't apply: this client asked for 5.1 or 7.1 and renders its own speakers."
            }
            (Self::GameOnlyAudio, _) => {
                "Headphone surround doesn't apply: Game only audio is stereo. Choose System audio in Audio settings to use it."
            }
            (Self::StereoEndpoint, _) => {
                "Headphone surround doesn't apply: the PC's playback device is stereo."
            }
            (Self::NextStream, _) => "Headphone surround applies from the next stream.",
        }
    }

    fn from_u8(value: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|reason| *reason as u8 == value)
    }
}

/// What decides whether the settings can apply to a stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Conditions {
    /// The channels the client asked for.
    pub client_channels: usize,
    pub game_audio: bool,
    pub muted: bool,
    /// The channels the stream captures.
    pub capture_channels: usize,
    /// Whether the capture endpoint has surround speakers, once it is open.
    pub source_surround: Option<bool>,
}

impl Conditions {
    /// The capture channels a stream starts with: 8 for headphone surround
    /// on a stereo stream of System audio, else what the client asked for.
    pub fn capture_channels(
        start: Settings,
        client_channels: usize,
        game_audio: bool,
        muted: bool,
    ) -> usize {
        if start.headphone_surround && client_channels == 2 && !game_audio && !muted {
            SURROUND_CHANNELS
        } else {
            client_channels
        }
    }

    /// Why each setting doesn't apply, highest priority first.
    pub fn reason(&self, key: Key) -> Option<Inactive> {
        if self.muted {
            return Some(Inactive::Muted);
        }
        match key {
            Key::NightMode => None,
            Key::HeadphoneSurround if self.client_channels != 2 => Some(Inactive::ClientSurround),
            Key::HeadphoneSurround if self.game_audio => Some(Inactive::GameOnlyAudio),
            Key::HeadphoneSurround if self.capture_channels != SURROUND_CHANNELS => {
                Some(Inactive::NextStream)
            }
            Key::HeadphoneSurround if self.source_surround == Some(false) => {
                Some(Inactive::StereoEndpoint)
            }
            Key::HeadphoneSurround => None,
        }
    }
}

/// The session's choice before live changes: the `/launch` or `/resume`
/// parameter, then what the client last chose live for this app
/// (`remembered`, from [`Memory`]), then the effective app, device and host
/// `config`. A parameter other than `on` or `off` is logged and ignored.
pub fn resolve(
    config: &crate::config::Config,
    options: &BTreeMap<String, String>,
    remembered: Live,
) -> Settings {
    let choice = |key: Key| {
        let parameter = options.get(key.parameter()).map(|value| value.trim());
        match parameter {
            Some("on") => true,
            Some("off") => false,
            other => {
                if let Some(value) = other {
                    tracing::info!(
                        parameter = key.parameter(),
                        value,
                        "unknown audio launch value; the audio settings decide"
                    );
                }
                remembered
                    .of(key)
                    .unwrap_or_else(|| config.boolean(key.config(), false))
            }
        }
    };
    Settings {
        night_mode: choice(Key::NightMode),
        headphone_surround: choice(Key::HeadphoneSurround),
    }
}

/// The settings a session changed live; `None` for one left as it started.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Live {
    pub night_mode: Option<bool>,
    pub headphone_surround: Option<bool>,
}

impl Live {
    fn of(self, key: Key) -> Option<bool> {
        match key {
            Key::NightMode => self.night_mode,
            Key::HeadphoneSurround => self.headphone_surround,
        }
    }
}

/// The last live choice of each client (its paired uuid) and app, which their
/// next session starts with: so a choice that "applies from the next stream"
/// does. In memory only: a host restart forgets it and no config is written.
#[derive(Default)]
pub struct Memory(std::sync::Mutex<BTreeMap<(String, u32), Live>>);

impl Memory {
    /// Keeps `live` for the client's next session of `app`; a setting it
    /// didn't change keeps what was remembered before.
    pub fn remember(&self, client: &str, app: u32, live: Live) {
        if live == Live::default() {
            return;
        }
        let mut choices = self.0.lock().unwrap();
        let remembered = choices.entry((client.into(), app)).or_default();
        remembered.night_mode = live.night_mode.or(remembered.night_mode);
        remembered.headphone_surround = live.headphone_surround.or(remembered.headphone_surround);
    }

    pub fn recall(&self, client: &str, app: u32) -> Live {
        self.0
            .lock()
            .unwrap()
            .get(&(client.into(), app))
            .copied()
            .unwrap_or_default()
    }
}

/// Values of [`Control::choice`].
const UNRESOLVED: u8 = 0;
const START_OFF: u8 = 1;
const START_ON: u8 = 2;
const LIVE_OFF: u8 = 3;
const LIVE_ON: u8 = 4;

#[derive(Default)]
struct Control {
    choice: AtomicU8,
    /// An [`Inactive`] as u8, 0 for none.
    reason: AtomicU8,
}

impl Control {
    fn value(&self) -> Option<bool> {
        match self.choice.load(Ordering::Relaxed) {
            UNRESOLVED => None,
            value => Some(matches!(value, START_ON | LIVE_ON)),
        }
    }

    fn live(&self) -> Option<bool> {
        match self.choice.load(Ordering::Relaxed) {
            LIVE_OFF => Some(false),
            LIVE_ON => Some(true),
            _ => None,
        }
    }
}

/// The live audio settings of one session. The audio thread reads them once
/// per packet; the native API and the couch menu write them.
#[derive(Default)]
pub struct Controls {
    night_mode: Control,
    headphone_surround: Control,
    revision: AtomicU64,
}

impl Controls {
    fn control(&self, key: Key) -> &Control {
        match key {
            Key::NightMode => &self.night_mode,
            Key::HeadphoneSurround => &self.headphone_surround,
        }
    }

    /// Sets the choice the stream starts with. A live change made before
    /// this keeps priority.
    pub fn start(&self, start: Settings) {
        for key in Key::ALL {
            let value = if key.of(start) { START_ON } else { START_OFF };
            if self
                .control(key)
                .choice
                .compare_exchange(UNRESOLVED, value, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                self.revision.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// A live change for this session; not saved.
    pub fn set(&self, key: Key, on: bool) {
        let value = if on { LIVE_ON } else { LIVE_OFF };
        if self.control(key).choice.swap(value, Ordering::Relaxed) != value {
            self.revision.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// The current choice, with `base` for a setting not yet resolved.
    pub fn settings(&self, base: Settings) -> Settings {
        Settings {
            night_mode: self.night_mode.value().unwrap_or(base.night_mode),
            headphone_surround: self
                .headphone_surround
                .value()
                .unwrap_or(base.headphone_surround),
        }
    }

    /// The live changes made in this session, for [`Memory`].
    pub fn live(&self) -> Live {
        Live {
            night_mode: self.night_mode.live(),
            headphone_surround: self.headphone_surround.live(),
        }
    }

    /// Records why `key` doesn't apply; returns whether that changed.
    pub fn set_reason(&self, key: Key, reason: Option<Inactive>) -> bool {
        let value = reason.map_or(0, |reason| reason as u8);
        let changed = self.control(key).reason.swap(value, Ordering::Relaxed) != value;
        if changed {
            self.revision.fetch_add(1, Ordering::Relaxed);
        }
        changed
    }

    pub fn reason(&self, key: Key) -> Option<Inactive> {
        Inactive::from_u8(self.control(key).reason.load(Ordering::Relaxed))
    }

    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Relaxed)
    }
}

fn setting(key: Key) -> Setting {
    Setting {
        id: key.id().into(),
        group: api::GROUP.into(),
        kind: Kind::Toggle,
        state: ControlState::Configurable,
        value: None,
        range: None,
        options: Vec::new(),
        unit: None,
        restore: None,
        reason: None,
        message: None,
    }
}

/// `GET /audio`: the caller's session `controls`, or `None` without a
/// session. `base` is the choice a stream starts with ([`resolve`]). A
/// setting that doesn't apply stays writable: its reason is a note.
pub fn state(controls: Option<&Controls>, base: Settings) -> Audio {
    let Some(controls) = controls else {
        return Audio {
            revision: 0,
            settings: Key::ALL
                .into_iter()
                .map(|key| Setting {
                    state: ControlState::TemporarilyUnavailable,
                    reason: Some(api::reason::NO_SESSION.into()),
                    message: Some("Start a stream to change this.".into()),
                    ..setting(key)
                })
                .collect(),
        };
    };
    let settings = controls.settings(base);
    Audio {
        revision: controls.revision(),
        settings: Key::ALL
            .into_iter()
            .map(|key| {
                let reason = controls.reason(key);
                Setting {
                    value: Some(Value::Bool(key.of(settings))),
                    reason: reason.map(|reason| reason.code().into()),
                    message: reason.map(|reason| reason.message(key).into()),
                    ..setting(key)
                }
            })
            .collect(),
    }
}

/// `POST /audio/settings` for the owner of `controls`: checks every change,
/// then applies them all to this session from the next packet. The session
/// and owner refusals are the caller's.
pub fn apply(controls: &Controls, base: Settings, write: &AudioWrite) -> Result<Audio, ApiError> {
    let refuse = |code, id: &str, message: String| ApiError {
        code,
        message,
        setting: Some(id.into()),
        retry_after_ms: None,
    };
    let mut changes = Vec::with_capacity(write.changes.len());
    for change in &write.changes {
        let key = Key::ALL
            .into_iter()
            .find(|key| key.id() == change.id)
            .ok_or_else(|| {
                refuse(
                    ErrorCode::NotFound,
                    &change.id,
                    format!("This host has no audio setting {}.", change.id),
                )
            })?;
        let Value::Bool(on) = change.value else {
            return Err(refuse(
                ErrorCode::InvalidValue,
                &change.id,
                format!("{} takes true or false.", change.id),
            ));
        };
        changes.push((key, on));
    }
    for (key, on) in changes {
        controls.set(key, on);
    }
    Ok(state(Some(controls), base))
}

/// Keeps a stream's inactive reasons, session warnings and log current.
#[derive(Default)]
pub struct Report {
    warned: [Option<Inactive>; 2],
    rendering: bool,
    bad_samples: bool,
}

impl Report {
    /// Call before each batch of packets with what the audio thread knows.
    pub fn update(
        &mut self,
        controls: &Controls,
        warnings: &crate::session::Warnings,
        conditions: &Conditions,
        settings: Settings,
        chain: &mut Chain,
    ) {
        for (key, warned) in Key::ALL.into_iter().zip(&mut self.warned) {
            let reason = conditions.reason(key);
            controls.set_reason(key, reason);
            // A setting that is off doesn't warn about not applying.
            let warning = reason.filter(|_| key.of(settings));
            if warning != *warned {
                match warning {
                    Some(reason) => warnings.set(key.warning(), reason.message(key)),
                    None => warnings.clear(key.warning()),
                }
                *warned = warning;
            }
        }
        let rendering = chain.rendering(settings);
        if rendering != self.rendering {
            if rendering {
                tracing::info!("headphone surround active (8 to 2)");
            } else {
                tracing::info!("headphone surround off; the 8 channels are downmixed to stereo");
            }
            self.rendering = rendering;
        }
        // Once per stream: the warning stays for the rest of it.
        if chain.take_bad_samples() && !self.bad_samples {
            warnings.set(
                BAD_SAMPLES_WARNING,
                "Audio capture delivered NaN or infinite samples; they were replaced with silence. Check the playback device or its audio enhancements.",
            );
            self.bad_samples = true;
        }
    }
}

/// The DSP of one stream, from capture to Opus.
pub struct Chain {
    capture_channels: usize,
    output_channels: usize,
    night: Night,
    surround: Option<Surround>,
    source_known: bool,
    bad_samples: bool,
}

/// Headphone surround: 8-channel capture to stereo.
struct Surround {
    layout: Layout,
    binaural: Binaural,
    /// Whether the HRIRs or the downmix render.
    rendering: bool,
    /// Frames of crossfade left toward `rendering`.
    fade: u32,
    output: Vec<f32>,
    other: Vec<f32>,
}

impl Surround {
    fn new(layout: Layout) -> Self {
        Self {
            binaural: Binaural::new(&layout, layout.level()),
            layout,
            rendering: false,
            fade: 0,
            output: Vec::new(),
            other: Vec::new(),
        }
    }

    /// Returns whether the output holds any HRIR rendering.
    fn process(&mut self, input: &[f32], on: bool) -> bool {
        let frames = input.len() / SURROUND_CHANNELS;
        self.output.resize(frames * 2, 0.);
        let on = on && self.layout.surround();
        if on != self.rendering {
            self.rendering = on;
            self.fade = CROSSFADE_FRAMES;
        }
        let (rendered, downmixed) = if self.rendering {
            (&mut self.output, &mut self.other)
        } else {
            (&mut self.other, &mut self.output)
        };
        let rendering = self.rendering || self.fade > 0;
        if rendering {
            rendered.resize(frames * 2, 0.);
            self.binaural.render(input, rendered);
        } else {
            self.binaural.listen(input);
        }
        if !self.rendering || self.fade > 0 {
            downmixed.resize(frames * 2, 0.);
            binaural::downmix(&self.layout.downmix, input, downmixed);
        }
        if self.fade > 0 {
            for (out, old) in self
                .output
                .as_chunks_mut::<2>()
                .0
                .iter_mut()
                .zip(self.other.as_chunks::<2>().0)
            {
                if self.fade == 0 {
                    break;
                }
                let t = self.fade as f32 / CROSSFADE_FRAMES as f32;
                out[0] += (old[0] - out[0]) * t;
                out[1] += (old[1] - out[1]) * t;
                self.fade -= 1;
            }
        }
        rendering
    }
}

impl Chain {
    /// A chain from `capture_channels` to the Opus layout's `output_channels`.
    pub fn new(capture_channels: usize, output_channels: usize) -> Result<Self> {
        let surround = match (capture_channels, output_channels) {
            (SURROUND_CHANNELS, 2) => Some(Surround::new(Layout::new(2, 3)?)),
            (capture, output) if capture == output => None,
            (capture, output) => {
                anyhow::bail!("no audio processing from {capture} to {output} channels")
            }
        };
        Ok(Self {
            capture_channels,
            output_channels,
            night: Night::default(),
            surround,
            source_known: false,
            bad_samples: false,
        })
    }

    /// The capture endpoint's own channels and speaker mask, once it opens.
    pub fn set_source(&mut self, channels: usize, mask: u32) -> Result<()> {
        if let Some(surround) = &mut self.surround {
            let layout = Layout::new(channels, mask)?;
            if layout != surround.layout {
                *surround = Surround::new(layout);
            }
        }
        self.source_known = true;
        Ok(())
    }

    /// Whether the endpoint has surround speakers, for an 8-channel capture
    /// whose endpoint has opened.
    pub fn source_surround(&self) -> Option<bool> {
        self.surround
            .as_ref()
            .filter(|_| self.source_known)
            .map(|surround| surround.layout.surround())
    }

    /// Whether the HRIRs render the next packet with `settings`.
    pub fn rendering(&self, settings: Settings) -> bool {
        settings.headphone_surround && self.source_surround() == Some(true)
    }

    /// Turns a packet of `capture_channels` into one of `output_channels`.
    pub fn process(&mut self, packet: &mut Vec<f32>, settings: Settings) {
        let mut rendered = false;
        if let Some(surround) = &mut self.surround {
            for sample in packet.iter_mut() {
                if !sample.is_finite() {
                    *sample = 0.;
                    self.bad_samples = true;
                } else {
                    *sample = night::flush(*sample);
                }
            }
            rendered = surround.process(packet, settings.headphone_surround);
            std::mem::swap(packet, &mut surround.output);
        }
        let clipped = self
            .night
            .process(packet, self.output_channels, settings.night_mode);
        // Night's clipper covers the frames it processed; the HRIRs' safety
        // clipper only the rest, so no sample is clipped twice.
        if rendered {
            for sample in &mut packet[clipped * self.output_channels..] {
                *sample = night::soft_clip(*sample);
            }
        }
    }

    /// Whether NaN or infinite samples were replaced since the last call.
    pub fn take_bad_samples(&mut self) -> bool {
        self.night.take_bad_samples() | std::mem::take(&mut self.bad_samples)
    }

    pub fn capture_channels(&self) -> usize {
        self.capture_channels
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rubylight_native_api::radeon::Change;

    const SEVEN_ONE: u32 = 0x63f;

    fn noise(samples: usize, seed: u64) -> Vec<f32> {
        let mut state = seed;
        (0..samples)
            .map(|_| {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                ((state >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 0.5
            })
            .collect()
    }

    const OFF: Settings = Settings {
        night_mode: false,
        headphone_surround: false,
    };
    const BOTH: Settings = Settings {
        night_mode: true,
        headphone_surround: true,
    };

    #[test]
    fn with_both_settings_off_the_output_is_bit_identical() {
        for channels in [2, 6, 8] {
            let mut chain = Chain::new(channels, channels).unwrap();
            let mut input = noise(240 * channels * 20, 5);
            input[3] = -0.;
            input[7] = 1e-40;
            for packet in input.chunks(240 * channels) {
                let mut samples = packet.to_vec();
                chain.process(&mut samples, OFF);
                assert_eq!(
                    samples.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                    packet.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
                );
            }
        }
        // 8-channel capture of a stereo endpoint passes FL and FR through.
        let mut chain = Chain::new(8, 2).unwrap();
        chain.set_source(2, 3).unwrap();
        let input = noise(240 * 8, 9);
        let mut samples = input.clone();
        chain.process(
            &mut samples,
            Settings {
                headphone_surround: true,
                ..OFF
            },
        );
        for (out, frame) in samples
            .as_chunks::<2>()
            .0
            .iter()
            .zip(input.as_chunks::<8>().0)
        {
            assert_eq!(out, &frame[..2]);
        }
    }

    #[test]
    fn surround_capture_is_8_channels_only_for_a_stereo_system_audio_stream() {
        let on = Settings {
            headphone_surround: true,
            ..OFF
        };
        assert_eq!(Conditions::capture_channels(on, 2, false, false), 8);
        assert_eq!(Conditions::capture_channels(OFF, 2, false, false), 2);
        assert_eq!(Conditions::capture_channels(on, 6, false, false), 6);
        assert_eq!(Conditions::capture_channels(on, 8, false, false), 8);
        assert_eq!(Conditions::capture_channels(on, 2, true, false), 2);
        assert_eq!(Conditions::capture_channels(on, 2, false, true), 2);
    }

    #[test]
    fn each_inactive_reason_is_set_for_its_case() {
        let active = Conditions {
            client_channels: 2,
            game_audio: false,
            muted: false,
            capture_channels: 8,
            source_surround: Some(true),
        };
        let table = [
            (active, None, None),
            (
                Conditions {
                    source_surround: None,
                    ..active
                },
                None,
                None,
            ),
            (
                Conditions {
                    muted: true,
                    client_channels: 6,
                    game_audio: true,
                    ..active
                },
                Some(Inactive::Muted),
                Some(Inactive::Muted),
            ),
            (
                Conditions {
                    client_channels: 6,
                    capture_channels: 6,
                    game_audio: true,
                    ..active
                },
                None,
                Some(Inactive::ClientSurround),
            ),
            (
                Conditions {
                    game_audio: true,
                    capture_channels: 2,
                    ..active
                },
                None,
                Some(Inactive::GameOnlyAudio),
            ),
            (
                Conditions {
                    capture_channels: 2,
                    source_surround: Some(false),
                    ..active
                },
                None,
                Some(Inactive::NextStream),
            ),
            (
                Conditions {
                    source_surround: Some(false),
                    ..active
                },
                None,
                Some(Inactive::StereoEndpoint),
            ),
        ];
        for (conditions, night, surround) in table {
            assert_eq!(conditions.reason(Key::NightMode), night, "{conditions:?}");
            assert_eq!(
                conditions.reason(Key::HeadphoneSurround),
                surround,
                "{conditions:?}"
            );
        }
        for reason in Inactive::ALL {
            assert_eq!(Inactive::from_u8(reason as u8), Some(reason));
        }
        assert_eq!(
            Inactive::NextStream.message(Key::HeadphoneSurround),
            "Headphone surround applies from the next stream."
        );
    }

    #[test]
    fn the_report_warns_only_for_a_setting_that_is_on_and_clears_it() {
        let controls = Controls::default();
        let warnings = crate::session::Warnings::default();
        let mut report = Report::default();
        let mut chain = Chain::new(2, 2).unwrap();
        let on = Settings {
            headphone_surround: true,
            ..OFF
        };
        let game = Conditions {
            client_channels: 2,
            game_audio: true,
            muted: false,
            capture_channels: 2,
            source_surround: None,
        };
        let codes = |warnings: &crate::session::Warnings| {
            warnings
                .snapshot()
                .into_iter()
                .map(|w| (w.code, w.message))
                .collect::<Vec<_>>()
        };
        report.update(&controls, &warnings, &game, OFF, &mut chain);
        assert_eq!(codes(&warnings), []);
        assert_eq!(
            controls.reason(Key::HeadphoneSurround),
            Some(Inactive::GameOnlyAudio)
        );
        report.update(&controls, &warnings, &game, on, &mut chain);
        assert_eq!(
            codes(&warnings),
            [(
                Key::HeadphoneSurround.warning().to_string(),
                Inactive::GameOnlyAudio
                    .message(Key::HeadphoneSurround)
                    .to_string()
            )]
        );
        let muted = Conditions {
            muted: true,
            ..game
        };
        let both = Settings {
            night_mode: true,
            ..on
        };
        report.update(&controls, &warnings, &muted, both, &mut chain);
        assert_eq!(codes(&warnings).len(), 2);
        assert_eq!(controls.reason(Key::NightMode), Some(Inactive::Muted));
        report.update(&controls, &warnings, &muted, OFF, &mut chain);
        assert_eq!(codes(&warnings), []);
    }

    #[test]
    fn precedence_is_live_then_launch_then_the_effective_config() {
        let mut config = crate::config::Config::parse("audio_night_mode = on\n").unwrap();
        let mut options = BTreeMap::new();
        assert_eq!(
            resolve(&config, &options, Live::default()),
            Settings {
                night_mode: true,
                headphone_surround: false
            }
        );
        // The app's and device's layers are already in the effective config.
        config
            .values
            .insert("audio_headphone_surround".into(), "true".into());
        assert!(resolve(&config, &options, Live::default()).headphone_surround);
        options.insert("audioNightMode".into(), "off".into());
        options.insert("audioHeadphoneSurround".into(), "loud".into());
        let start = resolve(&config, &options, Live::default());
        assert_eq!(
            start,
            Settings {
                night_mode: false,
                headphone_surround: true
            }
        );

        let controls = Controls::default();
        // Before the stream resolves, the base shows.
        assert_eq!(controls.settings(start), start);
        // A live change made before the audio thread starts keeps priority.
        controls.set(Key::NightMode, true);
        controls.start(start);
        assert_eq!(
            controls.settings(OFF),
            Settings {
                night_mode: true,
                headphone_surround: true
            }
        );
        controls.set(Key::HeadphoneSurround, false);
        assert!(!controls.settings(BOTH).headphone_surround);
    }

    #[test]
    fn a_remembered_live_choice_sits_between_the_launch_and_the_config() {
        let config = crate::config::Config::parse("audio_night_mode = on\n").unwrap();
        let mut options = BTreeMap::new();
        let remembered = Live {
            night_mode: Some(false),
            headphone_surround: Some(true),
        };
        assert_eq!(
            resolve(&config, &options, remembered),
            Settings {
                night_mode: false,
                headphone_surround: true
            }
        );
        options.insert("audioHeadphoneSurround".into(), "off".into());
        assert!(!resolve(&config, &options, remembered).headphone_surround);

        // Only live changes are remembered, merged per client and app.
        let controls = Controls::default();
        controls.start(BOTH);
        assert_eq!(controls.live(), Live::default());
        controls.set(Key::NightMode, false);
        let memory = Memory::default();
        memory.remember("a", 1, controls.live());
        memory.remember(
            "a",
            1,
            Live {
                night_mode: None,
                headphone_surround: Some(true),
            },
        );
        assert_eq!(
            memory.recall("a", 1),
            Live {
                night_mode: Some(false),
                headphone_surround: Some(true)
            }
        );
        assert_eq!(memory.recall("a", 2), Live::default());
        assert_eq!(memory.recall("b", 1), Live::default());
    }

    #[test]
    fn state_and_apply_follow_the_contract() {
        let none = state(None, BOTH);
        assert_eq!(none.settings.len(), 2);
        for setting in &none.settings {
            assert_eq!(setting.state, ControlState::TemporarilyUnavailable);
            assert_eq!(setting.value, None);
            assert_eq!(setting.reason.as_deref(), Some("no_session"));
        }

        let controls = Controls::default();
        controls.start(Settings {
            night_mode: false,
            headphone_surround: true,
        });
        controls.set_reason(Key::HeadphoneSurround, Some(Inactive::NextStream));
        let audio = state(Some(&controls), OFF);
        let [night, surround] = &audio.settings[..] else {
            panic!()
        };
        assert_eq!(
            (night.id.as_str(), night.group.as_str(), night.kind),
            ("audio.night_mode", "sound", Kind::Toggle)
        );
        assert_eq!(night.state, ControlState::Configurable);
        assert_eq!(night.value, Some(Value::Bool(false)));
        assert_eq!(night.reason, None);
        // Inactive but writable: still configurable, the reason is a note.
        assert_eq!(surround.state, ControlState::Configurable);
        assert_eq!(surround.value, Some(Value::Bool(true)));
        assert_eq!(surround.reason.as_deref(), Some("next_stream"));
        assert_eq!(
            surround.message.as_deref(),
            Some("Headphone surround applies from the next stream.")
        );

        let write = |changes: Vec<(&str, Value)>| AudioWrite {
            changes: changes
                .into_iter()
                .map(|(id, value)| Change {
                    id: id.into(),
                    value,
                })
                .collect(),
        };
        let revision = controls.revision();
        let audio = apply(
            &controls,
            OFF,
            &write(vec![
                ("audio.night_mode", Value::Bool(true)),
                ("audio.headphone_surround", Value::Bool(false)),
            ]),
        )
        .unwrap();
        assert!(audio.revision > revision);
        assert_eq!(
            controls.settings(OFF),
            Settings {
                night_mode: true,
                headphone_surround: false
            }
        );
        // A bad change refuses the whole write.
        for (changes, code) in [
            (
                vec![
                    ("audio.night_mode", Value::Bool(false)),
                    ("audio.volume", Value::Bool(true)),
                ],
                ErrorCode::NotFound,
            ),
            (
                vec![
                    ("audio.night_mode", Value::Bool(false)),
                    ("audio.headphone_surround", Value::Integer(1)),
                ],
                ErrorCode::InvalidValue,
            ),
            (
                vec![("audio.night_mode", Value::Choice("on".into()))],
                ErrorCode::InvalidValue,
            ),
        ] {
            let error = apply(&controls, OFF, &write(changes)).unwrap_err();
            assert_eq!(error.code, code);
            assert!(error.setting.is_some());
            assert!(controls.settings(OFF).night_mode);
        }
        let revision = controls.revision();
        assert!(!controls.set_reason(Key::HeadphoneSurround, Some(Inactive::NextStream)));
        assert_eq!(controls.revision(), revision);
    }

    #[test]
    fn surround_renders_8_to_2_and_switches_live_with_a_crossfade() {
        let mut chain = Chain::new(8, 2).unwrap();
        chain.set_source(8, SEVEN_ONE).unwrap();
        assert_eq!(chain.source_surround(), Some(true));
        let surround = Settings {
            headphone_surround: true,
            ..OFF
        };
        assert!(chain.rendering(surround));
        let input = noise(240 * 8 * 40, 2);
        let mut output = Vec::new();
        for (i, packet) in input.chunks(240 * 8).enumerate() {
            let mut samples = packet.to_vec();
            let settings = if (10..20).contains(&i) { OFF } else { surround };
            chain.process(&mut samples, settings);
            assert_eq!(samples.len(), 480);
            assert!(samples.iter().all(|v| v.abs() <= night::CLIP_CEILING));
            output.extend(samples);
        }
        // Each switch is smooth: no frame jumps more than the signal does.
        let step = |s: &[f32]| {
            s.as_chunks::<2>()
                .0
                .windows(2)
                .map(|w| (w[1][0] - w[0][0]).abs())
                .fold(0f32, f32::max)
        };
        let typical = step(&output[..480 * 10]);
        for at in [10, 20] {
            let around = &output[(at - 1) * 480..(at + 2) * 480];
            assert!(step(around) <= typical * 1.5, "{at}");
        }
        // Without 8-channel capture, nothing renders.
        let chain = Chain::new(2, 2).unwrap();
        assert!(!chain.rendering(surround));
    }

    #[test]
    fn silence_and_nan_through_the_whole_chain() {
        let mut chain = Chain::new(8, 2).unwrap();
        chain.set_source(8, SEVEN_ONE).unwrap();
        let mut silence = vec![0.; 240 * 8];
        chain.process(&mut silence, BOTH);
        assert_eq!(silence, vec![0.; 480]);
        let mut bad = vec![f32::NAN; 240 * 8];
        chain.process(&mut bad, BOTH);
        assert!(bad.iter().all(|v| *v == 0.));
        assert!(chain.take_bad_samples());
        assert!(!chain.take_bad_samples());
        let mut later = noise(240 * 8, 4);
        chain.process(&mut later, BOTH);
        assert!(later.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn with_night_mode_on_or_ramping_only_its_clipper_runs() {
        let mut chain = Chain::new(8, 2).unwrap();
        chain.set_source(8, SEVEN_ONE).unwrap();
        // The reference: the HRIR render without a clipper, then night mode.
        let mut surround = Surround::new(Layout::new(8, SEVEN_ONE).unwrap());
        let mut night = Night::default();
        let surround_only = Settings {
            headphone_surround: true,
            ..OFF
        };
        // 20 ms packets, so the 10 ms off ramp ends inside one.
        let input: Vec<f32> = noise(960 * 8 * 8, 6).iter().map(|v| v * 4.).collect();
        let mut loud = false;
        for (i, packet) in input.chunks(960 * 8).enumerate() {
            let settings = if i < 6 { BOTH } else { surround_only };
            let mut samples = packet.to_vec();
            chain.process(&mut samples, settings);
            assert!(surround.process(packet, true));
            let raw = surround.output.clone();
            loud |= raw.iter().any(|v| v.abs() > night::CLIP_KNEE * 1.1);
            let mut want = raw.clone();
            let clipped = night.process(&mut want, 2, settings.night_mode);
            assert_eq!(clipped, [960, 960, 960, 960, 960, 960, 480, 0][i]);
            for sample in &mut want[clipped * 2..] {
                *sample = night::soft_clip(*sample);
            }
            assert_eq!(bits(&samples), bits(&want), "packet {i}");
            assert!(samples.iter().all(|v| v.abs() <= night::CLIP_CEILING));
        }
        assert!(loud);
    }

    fn bits(samples: &[f32]) -> Vec<u32> {
        samples.iter().map(|v| v.to_bits()).collect()
    }

    #[test]
    fn subnormal_input_leaves_no_subnormal_output() {
        for (settings, channels) in [
            (BOTH, 8),
            (
                Settings {
                    headphone_surround: true,
                    ..OFF
                },
                8,
            ),
            (
                Settings {
                    night_mode: true,
                    ..OFF
                },
                2,
            ),
        ] {
            let mut chain = Chain::new(channels, 2).unwrap();
            chain
                .set_source(channels, if channels == 8 { SEVEN_ONE } else { 3 })
                .unwrap();
            for i in 0..8 {
                let mut packet: Vec<f32> = (0..240 * channels)
                    .map(|n| [1e-40, -1e-39, 5e-31, -f32::MIN_POSITIVE][(n + i) % 4])
                    .collect();
                chain.process(&mut packet, settings);
                assert!(
                    packet.iter().all(|v| v.to_bits() == 0),
                    "{settings:?} packet {i}"
                );
            }
        }
    }

    /// `cargo test --release ... audio_dsp -- --ignored --nocapture bench`
    #[test]
    #[ignore]
    fn bench() {
        let packets = 4000;
        let mut report = Vec::new();
        for (name, capture, output, settings) in [
            (
                "night mode, stereo",
                2,
                2,
                Settings {
                    night_mode: true,
                    ..OFF
                },
            ),
            (
                "headphone surround, 7.1 to 2",
                8,
                2,
                Settings {
                    headphone_surround: true,
                    ..OFF
                },
            ),
            ("both, 7.1 to 2", 8, 2, BOTH),
        ] {
            let mut chain = Chain::new(capture, output).unwrap();
            if capture == 8 {
                chain.set_source(8, SEVEN_ONE).unwrap();
            }
            let input = noise(240 * capture * 64, 8);
            let source: Vec<&[f32]> = input.chunks(240 * capture).collect();
            let mut packet = Vec::with_capacity(240 * capture);
            let mut times = Vec::with_capacity(packets);
            for i in 0..packets {
                packet.clear();
                packet.extend_from_slice(source[i % source.len()]);
                let start = std::time::Instant::now();
                chain.process(&mut packet, settings);
                times.push(start.elapsed().as_secs_f64() * 1e3);
                std::hint::black_box(&packet);
            }
            let mean = times.iter().sum::<f64>() / packets as f64;
            // A busy PC preempts some packets; the median is the work itself.
            times.sort_by(f64::total_cmp);
            let (median, p99) = (times[packets / 2], times[packets * 99 / 100]);
            report.push(format!(
                "{name}: {median:.4} ms median, {mean:.4} ms mean, {p99:.4} ms p99 per 5 ms packet"
            ));
            assert!(median < 0.25, "{name}: {median} ms");
        }
        println!("{}", report.join("\n"));
    }
}
