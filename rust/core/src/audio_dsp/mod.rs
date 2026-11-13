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
mod tests;
