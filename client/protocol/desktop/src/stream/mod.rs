//! A stream: launching or resuming the app over HTTPS, starting the session on a worker
//! thread, then on the UI thread showing pictures, playing nothing (audio has its own
//! thread), sending keyboard, mouse and controller input, and drawing the overlay.
//!
//! Input handling follows moonlight-qt's SdlInputHandler (GPL-3.0, Cameron Gutman and
//! contributors): positional keys, the Ctrl+Alt+Shift combos, raising every key when focus
//! is lost, relative capture with absolute fallback.

pub mod packed444;
pub mod color_probe;
pub mod pipeline;
mod playout;
pub mod present;
mod microphone;
#[cfg(windows)]
mod voice_capture;
mod cursor;
mod bitrate;
mod host_abr;
pub mod doctor;
pub mod overlay;
mod recovery;
mod quality;
mod rate_change;
pub mod menu_extras;
mod touch;
mod touchpad;
mod upscale;
pub mod enhance;
#[cfg(windows)]
mod amf_scaler;
#[cfg(windows)]
pub(crate) mod direct;
mod phase;
mod text_field;
mod clipboard_sync;
pub mod scanline;
pub mod race;
pub mod race_probe;
pub(crate) mod pyrowave;
pub mod pyrowave_gpu;
pub mod pyrowave_mode;
mod stage_times;
#[cfg(windows)]
mod d3d11;
#[cfg(windows)]
mod wasapi;
pub mod gpu_planes;
mod pyrowave_sdk;
#[cfg(unix)]
mod planes_gpu;

use std::collections::{HashSet, VecDeque};
use std::net::ToSocketAddrs;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nightjar_core::color;
use nightjar_core::control::{ClientMessage, ControlEvent};
use nightjar_core::http::foundation::AbrRequest;
use nightjar_core::http::{App, AppRole, Availability, ClientIdentity, Feature, HardwareMetrics, HostApiError, HostClient, Error as HttpError, LaunchRequest, NativeCapabilities, ServerInfo};
use nightjar_core::input::packets::{modifier, mouse_button};
use nightjar_core::sdp::*;
use nightjar_core::session::{self, HostInfo, Interrupt, Location, NetworkChange, NetworkHint, RoamingConfig, Session, SessionConfig, Stage};
use nightjar_policy::health::TrialOffer;
use nightjar_policy::overlay::{HardwareAnswer, HardwarePoll};
use nightjar_policy::stats::Stats;
use rubylight_protocol::control::{HostTimingRequest, LinkNotice, LinkNoticeRequest, LinkState, Reconfigure, CLOCK_SYNC_MESSAGE_TYPE, HOST_TIMING_MESSAGE_TYPE, LINK_NOTICE_MESSAGE_TYPE, RECONFIGURE_MESSAGE_TYPE};
use rubylight_protocol::pipeline_rates::{PipelineRatesReport, PipelineRatesRequest, PIPELINE_RATES_MESSAGE_TYPE, UNKNOWN};
use rubylight_protocol::present_clock::ClockOffset;
use rubylight_protocol::rate_change::{RateChangeRequest, RateChangeResult};
use sdl3_sys::everything::*;

use crate::config::{self, AudioChannels, CodecPreference, HostRecord, MouseMode, OverlayCorner, OverlayMode, QualityMode, Settings};
use crate::gamepad::{Action, Gamepads};
use crate::hosts::Waker;
use crate::strings::{en, tr, trf};
use crate::keymap;
use crate::media::Hardware;
use pipeline::{lock, Notice, Shared};
use present::{viewport, PacingMode, Pacer, VideoTexture};

/// What the display offers, for "native" resolution and "display" refresh.
#[derive(Clone, Copy, Debug)]
pub struct Display {
    pub width: u32,
    pub height: u32,
    pub refresh_hz: f32,
}

#[derive(Clone)]
pub struct LaunchPlan {
    pub host_id: u64,
    pub record: HostRecord,
    /// What this device calls the PC: the name chosen here, else the PC's own.
    pub host_name: String,
    /// Serverinfo with the native read merged in (`hosts::merge_native`).
    pub info: ServerInfo,
    /// The native read, merged into the serverinfo a resume reads again.
    pub native: Option<NativeCapabilities>,
    pub app: App,
    pub resume: bool,
    /// Input-only on a PC without its own mode: an ordinary small stream, shown and played nowhere.
    pub small_stream_input_only: bool,
    pub settings: Settings,
    pub identity: Arc<ClientIdentity>,
    pub display: Display,
    pub hardware: Hardware,
    /// VAAPI pictures go to the renderer as DMA-BUFs (`crate::dmabuf`), not through memory.
    pub dmabuf: bool,
    /// The renderer's Direct3D 11 device when PyroWave decodes on the GPU (`pyrowave_decoder`).
    pub gpu_decoder: Option<pipeline::GpuDevice>,
    pub packed_modes: u8,
    /// The 4:4:4 and 10-bit formats this device decodes and draws, and why the others are not
    /// (`color_probe::probe`); `(0, "")` when neither setting asks for them.
    pub color_decodable: (u32, String),
    pub pacing: PacingMode,
    pub vrr: bool,
    pub gamepad_mask: u32,
    pub prepared_controllers: String,
    pub microphone_muted: bool,
    pub bitrate_limit_kbps: u32,
    /// The internet connection. The route to the host decides the link speed and wireless hints
    /// and the pace calibration instead; this is the fallback when that lookup tells nothing.
    pub network: crate::network::Network,
    pub pace: Option<crate::config::PaceCalibration>,
    pub host_link_mbps: u32,
    pub recovery: Option<recovery::Backoff>,
    /// The user confirmed closing the running app for this launch (`rlConfirmReplace`).
    pub confirm_replace: bool,
}

/// The stream's negotiated shape.
#[derive(Clone, Debug)]
pub struct Facts {
    pub role: AppRole,
    pub width: i32,
    pub height: i32,
    pub fps: i32,
    pub bitrate_kbps: i32,
    pub format: u32,
    pub host_timing: bool,
    /// The host says when it lowers the video bitrate because of the network (0x5538).
    pub link_notice: bool,
    /// The host reports its pipeline frame rates (0x553D).
    pub pipeline_rates: bool,
    pub clock_sync: bool,
    pub display_caps: bool,
    pub cursor_shapes: bool,
    /// The host says when one of its text fields takes or loses focus (0x553B), once asked.
    pub text_input: bool,
    /// The host takes this device's keyboard layout (0x553E).
    pub keyboard_layout: bool,
    pub clipboard: bool,
    /// The PC offered clipboard sync (0x5508) to a launch that asked.
    pub clipboard_sync: bool,
    /// The PC takes and gives PNG images on its clipboard (Experimental).
    pub image_clipboard: bool,
    pub server_commands: Vec<String>,
    pub runtime_bitrate: bool,
    pub reconfigure: bool,
    /// The host takes mid-stream bitrate and rate changes (0x553C).
    pub rate_change: bool,
    /// Whether the PC forwards USB devices and lets this device attach them; asked only while
    /// the setting is on.
    pub usb: Availability,
    pub touch: bool,
    pub phase_lock: bool,
    /// PyroWave decodes blocks while a frame arrives (`Settings::early_decode`, or the
    /// `NIGHTJAR_EARLY_DECODE` override) on this stream.
    pub early_decode: bool,
    /// Early decode was asked for on a PyroWave stream but the host sent no records, so the
    /// frames arrive whole; the overlay and the log say so.
    pub early_without_records: bool,
    /// The picture is decoded in slices as its bands complete (`Settings::beam_race`).
    pub beam_race: bool,
    /// Why the beam race setting did nothing on this stream, if it was on and did not run.
    pub beam_race_off: Option<&'static str>,
    /// The slices a frame is decoded in when the beam race is on (0 when not).
    pub race_slices: u32,
    /// The pace the host sends the picture at (Mbps), the raced presenter's first guess at when
    /// slices are ready.
    pub pace_mbps: u32,
    /// What "Prefer YUV 4:4:4" and "10-bit color (SDR)" came to, told once the stream starts.
    pub color_notice: Option<String>,
    /// PyroWave was chosen and the stream runs another codec: what to tell the player, once per
    /// PC (`pyrowave_notice`).
    pub pyrowave_fallback: Option<String>,
    pub app_name: String,
    pub host_name: String,
}

pub fn codec_name(format: u32) -> &'static str {
    if format & VIDEO_FORMAT_MASK_PYROWAVE != 0 {
        if format & VIDEO_FORMAT_MASK_10BIT != 0 { "PyroWave 10-bit" } else { "PyroWave" }
    } else if format & VIDEO_FORMAT_MASK_AV1 != 0 {
        if format & VIDEO_FORMAT_MASK_10BIT != 0 {
            "AV1 10-bit"
        } else {
            "AV1"
        }
    } else if format & VIDEO_FORMAT_MASK_H265 != 0 {
        if format & VIDEO_FORMAT_MASK_10BIT != 0 {
            "HEVC Main10"
        } else {
            "HEVC"
        }
    } else {
        "H.264"
    }
}

/// The timing CSV when `NIGHTJAR_TIMING_CSV` names a file or folder.
fn open_timing_csv() -> Option<crate::timing_csv::TimingCsv> {
    let path = crate::timing_csv::path_from_env()?;
    match crate::timing_csv::TimingCsv::create(&path) {
        Ok(csv) => {
            eprintln!("nightjar: writing the timing CSV to {}", path.display());
            Some(csv)
        }
        Err(error) => {
            eprintln!("nightjar: cannot write the timing CSV {}: {error}", path.display());
            None
        }
    }
}

fn random_bytes<const N: usize>() -> [u8; N] {
    use std::hash::{BuildHasher, Hasher};
    let mut out = [0u8; N];
    for chunk in out.chunks_mut(8) {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(session::now_us());
        let bytes = h.finish().to_le_bytes();
        chunk.copy_from_slice(&bytes[..chunk.len()]);
    }
    out
}

/// The stream's resolution, rate and bitrate from the settings and display.
pub fn stream_mode(settings: &Settings, display: Display) -> (u32, u32, u32, u32) {
    let (mut w, mut h) = if settings.width == 0 || settings.height == 0 {
        (display.width, display.height)
    } else {
        (settings.width, settings.height)
    };
    w = (w.max(320) + 1) & !1;
    h = (h.max(240) + 1) & !1;
    let fps = if settings.fps == 0 {
        (display.refresh_hz.round() as u32).clamp(30, 240)
    } else {
        settings.fps
    };
    let bitrate = if settings.bitrate_kbps == 0 {
        config::default_bitrate_kbps(w, h, fps)
    } else {
        settings.bitrate_kbps
    };
    let bitrate = if matches!(settings.codec, CodecPreference::PyroWave | CodecPreference::PyroWave444) {
        bitrate.max(pyrowave::minimum_bitrate(w, h, fps, settings.codec == CodecPreference::PyroWave444, settings.hdr))
    } else { bitrate };
    (w, h, fps, bitrate)
}

/// With the link test switched on (`link_probe`, off by default because it measured worse), a new
/// stream at the automatic bitrate times the link first when the PC offers the test
/// (`Feature::BandwidthProbe`), and starts no higher than it holds. PyroWave has its own test and
/// a minimum bitrate; a resume keeps the bitrate of the running session's launch.
fn probes_first_bitrate(settings: &Settings, resume: bool, info: &ServerInfo) -> bool {
    settings.link_probe && !resume && settings.bitrate_kbps == 0
        && !matches!(settings.codec, CodecPreference::PyroWave | CodecPreference::PyroWave444)
        && info.paired && info.capabilities.has(Feature::BandwidthProbe)
}

fn capped_bitrate(bitrate: u32, limit: u32) -> u32 {
    if limit == 0 { bitrate } else { bitrate.min(limit) }
}

/// The formats offered for a codec preference, limited to what the host encodes.
pub fn video_formats(preference: CodecPreference, hdr: bool, host_scm: u32) -> u32 {
    let mut formats = match preference {
        CodecPreference::H264 => VIDEO_FORMAT_H264,
        CodecPreference::Hevc => VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265,
        CodecPreference::Av1 => VIDEO_FORMAT_H264 | VIDEO_FORMAT_AV1_MAIN8,
        CodecPreference::Auto => VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265,
        CodecPreference::PyroWave => VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_PYROWAVE,
        CodecPreference::PyroWave444 => VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_PYROWAVE | VIDEO_FORMAT_PYROWAVE_444,
    };
    if hdr {
        if formats & VIDEO_FORMAT_H265 != 0 {
            formats |= VIDEO_FORMAT_H265_MAIN10;
        }
        if formats & VIDEO_FORMAT_AV1_MAIN8 != 0 {
            formats |= VIDEO_FORMAT_AV1_MAIN10;
        }
        if formats & VIDEO_FORMAT_PYROWAVE != 0 { formats |= VIDEO_FORMAT_PYROWAVE_MAIN10; }
        if formats & VIDEO_FORMAT_PYROWAVE_444 != 0 { formats |= VIDEO_FORMAT_PYROWAVE_MAIN10_444; }
    }
    // Hosts that report codec support get only what they encode; H.264 always stays.
    if host_scm != 0 {
        if host_scm & SCM_HEVC == 0 {
            formats &= !VIDEO_FORMAT_H265;
        }
        if host_scm & SCM_HEVC_MAIN10 == 0 {
            formats &= !VIDEO_FORMAT_H265_MAIN10;
        }
        if host_scm & SCM_AV1_MAIN8 == 0 {
            formats &= !VIDEO_FORMAT_AV1_MAIN8;
        }
        if host_scm & SCM_AV1_MAIN10 == 0 {
            formats &= !VIDEO_FORMAT_AV1_MAIN10;
        }
        for (scm, format) in [(SCM_PYROWAVE, VIDEO_FORMAT_PYROWAVE), (SCM_PYROWAVE_444, VIDEO_FORMAT_PYROWAVE_444),
            (SCM_PYROWAVE_MAIN10, VIDEO_FORMAT_PYROWAVE_MAIN10), (SCM_PYROWAVE_MAIN10_444, VIDEO_FORMAT_PYROWAVE_MAIN10_444)] {
            if host_scm & scm == 0 { formats &= !format; }
        }
    }
    formats | VIDEO_FORMAT_H264
}

/// What "Prefer YUV 4:4:4" and "10-bit color (SDR)" ask of `color::offer` with these settings.
pub fn color_wants(settings: &Settings) -> color::Wants {
    color::Wants {
        yuv444: settings.prefer_yuv444,
        sdr10: settings.sdr_10bit,
        hdr: settings.hdr,
        packed444: settings.experimental_packed_444,
        pyrowave: settings.codec.is_pyrowave(),
    }
}

/// Whether a launch checks the decoder (`color_probe::probe`): when one of the settings asks and
/// nothing that takes the stream first (HDR, packed full chroma, PyroWave) refuses it anyway.
pub fn probes_color(settings: &Settings) -> bool {
    let wants = color_wants(settings);
    (wants.yuv444 || wants.sdr10) && !wants.hdr && !wants.packed444 && !wants.pyrowave
}

/// The formats a launch offers: the codec choice's, and the 4:4:4 and 10-bit ones the settings
/// add where this device (`decodable`, from the probe) and the PC (`scm`) both handle them.
/// Packed full chroma offers the plain formats whatever the PC reports.
fn offered_formats(settings: &Settings, scm: u32, decodable: &(u32, String)) -> color::Offer {
    let base = video_formats(settings.codec, settings.hdr, if settings.experimental_packed_444 { 0 } else { scm });
    color::offer(base, color_wants(settings), decodable.0, &decodable.1, scm)
}

/// Whether the launch says `hdrMode=0`: an SDR stream that offers a 10-bit format. Without it a
/// host streams that format as HDR, and an SDR display ends the stream. The format is picked
/// after the launch, so a stream that ends up 8-bit says it too: for an 8-bit format it means
/// what saying nothing does.
fn launches_ten_bit_sdr(settings: &Settings, offer: &color::Offer) -> bool {
    !settings.hdr && offer.formats & VIDEO_FORMAT_MASK_10BIT != 0
}

fn refusal_text(refusal: &color::Refusal) -> String {
    use color::Refusal;
    match refusal {
        Refusal::Off | Refusal::Hdr => String::new(),
        Refusal::Packed444 => tr("full chroma is on and has its own 4:4:4 and 10-bit handling.").into(),
        Refusal::PyroWave => tr("PyroWave is chosen, and it has its own 4:4:4 and 10-bit modes.").into(),
        Refusal::Decoder(why) => trf("this device can’t decode or draw it ({}).", &[why]),
        Refusal::Host => tr("the PC doesn’t encode it for this codec.").into(),
        Refusal::Yuv444 => tr("4:4:4 is on, and the PC takes 4:4:4 or 10-bit, not both.").into(),
    }
}

/// What to tell the player once the stream that `offer` was made for streams `negotiated`: what
/// was granted, or why not, one line each; None when neither setting was on or had a word.
fn color_notice(offer: &color::Offer, negotiated: u32) -> Option<String> {
    let mut lines = Vec::new();
    if offer.yuv444.is_ok() && negotiated & VIDEO_FORMAT_MASK_YUV444 != 0 {
        lines.push(tr("Streaming in YUV 4:4:4.").to_owned());
    }
    if offer.sdr10.is_ok() && negotiated & VIDEO_FORMAT_MASK_10BIT != 0 {
        lines.push(tr("Streaming 10-bit color (SDR).").to_owned());
    }
    for notice in color::notices(offer, negotiated) {
        lines.push(match notice {
            color::Notice::Yuv444Unavailable(refusal) => trf("YUV 4:4:4 unavailable; using 4:2:0: {}", &[&refusal_text(&refusal)]),
            color::Notice::Sdr10Unavailable(refusal) => trf("10-bit SDR isn’t used, so the stream is 8-bit: {}", &[&refusal_text(&refusal)]),
            color::Notice::Sdr10NotAccepted => tr("This PC didn’t accept 10-bit, so the stream is 8-bit.").to_owned(),
        });
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// When PyroWave was chosen and the stream runs another codec: the log line saying why, and
/// what to tell the player. A device that can't decode PyroWave never gets here: the launch
/// stops first with its reason (`pyrowave_mode::availability`).
/// `scm` is the codec support trusted for the offer, `reported` the serverinfo value before
/// `trusted_codec_mode_support`.
fn pyrowave_fallback(codec: CodecPreference, (scm, reported): (u32, u32), offered: u32, negotiated: u32) -> Option<(String, String)> {
    if !codec.is_pyrowave() || negotiated & VIDEO_FORMAT_MASK_PYROWAVE != 0 {
        return None;
    }
    let name = codec_name(negotiated);
    let why = if offered & VIDEO_FORMAT_MASK_PYROWAVE == 0 {
        "the host does not offer it"
    } else if scm == 0 {
        "the host reports no trusted codec support and chose another codec"
    } else {
        "the host chose another codec"
    };
    Some((
        format!("nightjar: PyroWave was chosen, but {why} (codec support {scm:#x}, reported {reported:#x}); streaming {name}"),
        trf("This PC doesn’t offer PyroWave, so Nightjar uses {}.", &[&name]),
    ))
}

/// The PyroWave fallback notice once per PC, like Android's `HostNotices`: the text the first time
/// a stream from it falls back, then nothing until PyroWave streams from it again. Reconnects
/// and later streams stay quiet. `fallback` and `format` are the stream's (`Facts`). Returns the
/// text to show and whether `record` changed.
pub fn pyrowave_notice(record: &mut HostRecord, chosen: bool, fallback: Option<&str>, format: u32) -> (Option<String>, bool) {
    use crate::config::NOTICE_PYROWAVE_FALLBACK;
    match fallback {
        Some(text) => {
            let first = record.claim_notice(NOTICE_PYROWAVE_FALLBACK);
            (first.then(|| text.to_owned()), first)
        }
        None if chosen && format & VIDEO_FORMAT_MASK_PYROWAVE != 0 => (None, record.reset_notice(NOTICE_PYROWAVE_FALLBACK)),
        None => (None, false),
    }
}

fn stage_text(stage: Stage) -> &'static str {
    match stage {
        Stage::PlatformInit | Stage::NameResolution => en("Finding the PC"),
        Stage::AudioStreamInit | Stage::RtspHandshake => en("Negotiating the stream"),
        Stage::ControlStreamInit | Stage::ControlStreamStart => en("Opening the control channel"),
        Stage::VideoStreamInit | Stage::VideoStreamStart => en("Starting video"),
        Stage::AudioStreamStart => en("Starting audio"),
        Stage::InputStreamInit | Stage::InputStreamStart => en("Connecting input"),
    }
}

pub fn termination_text(code: i32) -> Option<String> {
    use nightjar_core::session::error;
    Some(match code {
        error::GRACEFUL => return None,
        error::NO_VIDEO_TRAFFIC => {
            en("No video arrived from the PC. A firewall may be blocking UDP ports 47998-48000.").into()
        }
        error::NO_VIDEO_FRAME => en("Video arrived but no complete picture could be assembled.").into(),
        error::UNEXPECTED_EARLY => en("The PC ended the stream before it started.").into(),
        error::PROTECTED_CONTENT => en("The PC stopped streaming protected content.").into(),
        error::FRAME_CONVERSION => en("The PC could not convert the picture for streaming.").into(),
        error::DISCONNECTED => en("The connection to the PC was lost.").into(),
        other => trf("The stream ended unexpectedly (error {}).", &[&other]),
    })
}

/// Why a launch ended without a stream.
pub enum ConnectFailed {
    /// What to tell the user.
    Message(String),
    /// The PC asks the user to confirm closing the running app; the plan launches again with
    /// the answer.
    ConfirmReplace(Box<LaunchPlan>),
}

/// A launch and session start running on a worker thread.
pub struct Connecting {
    pub shared: Arc<Shared>,
    pub interrupt: Interrupt,
    pub stage: String,
    pub app_name: String,
    pub host_name: String,
    pub host_id: u64,
    pub pacing: PacingMode,
    pub settings: Settings,
    plan: LaunchPlan,
    result: Receiver<Result<(Session, Facts, LaunchPlan), ConnectError>>,
}

impl Connecting {
    /// The game this stream plays, for the recent list; None when the PC has no unique id yet
    /// or the stream is not a game (a remote monitor, input only).
    pub fn recent_game(&self) -> Option<config::Recent> {
        let record = &self.plan.record;
        if record.unique_id.is_empty() || self.plan.app.id < 0 || self.plan.app.control != nightjar_core::http::AppControl::None {
            return None;
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        Some(config::Recent {
            host: record.unique_id.clone(),
            install_id: self.plan.app.uuid.clone(),
            app_id: self.plan.app.id as u32,
            title: self.plan.app.name.clone(),
            at: now,
        })
    }

    pub fn start(mut plan: LaunchPlan, waker: Waker) -> Self {
        if let Some(recovery) = &mut plan.recovery { recovery.begin(recovery::now_ms()); }
        // Host-time pacing schedules every picture; the other modes keep only the newest.
        let limit = if plan.pacing == PacingMode::HostTimes { 8 } else { 2 };
        let shared = Shared::new(waker.clone(), limit);
        let interrupt = Interrupt::default();
        let (sender, result) = channel();
        let app_name = plan.app.name.clone();
        let host_name = plan.host_name.clone();
        let (host_id, pacing, settings) = (plan.host_id, plan.pacing, plan.settings.clone());
        let worker_shared = shared.clone();
        let worker_interrupt = interrupt.clone();
        let mut worker_plan = plan.clone();
        let _ = std::thread::Builder::new()
            .name("connect".into())
            .spawn(move || {
                let result = connect_with_recovery(&mut worker_plan, worker_shared, &worker_interrupt)
                    .map(|(session, facts)| (session, facts, worker_plan));
                let _ = sender.send(result);
                waker();
            });
        Self {
            shared,
            interrupt,
            stage: en("Starting the app on the PC").into(),
            app_name,
            host_name,
            host_id,
            pacing,
            settings,
            plan,
            result,
        }
    }

    /// The stream presents for a VRR display (the setting is on and the swap chain tears).
    pub fn vrr(&self) -> bool { self.plan.vrr }

    /// Ok(Some) once streaming, Err on failure.
    pub fn poll(&mut self) -> Result<Option<(Session, Facts)>, ConnectFailed> {
        let expired = self.plan.recovery.as_ref().is_some_and(|r| r.remaining(recovery::now_ms()) == 0);
        if expired { self.cancel(); }
        let notices = std::mem::take(&mut *lock(&self.shared.notices));
        let mut keep = Vec::new();
        for notice in notices {
            match notice {
                Notice::Stage(stage) => self.stage = stage_text(stage).into(),
                Notice::Reconnecting(attempt) => {
                    keep.clear();
                    self.stage = trf("Reconnecting · attempt {} of 5", &[&attempt]);
                }
                // The rest is for the stream.
                other => keep.push(other),
            }
        }
        if !keep.is_empty() {
            let mut queue = lock(&self.shared.notices);
            keep.append(&mut queue);
            *queue = keep;
        }
        match self.result.try_recv() {
            Ok(Ok((session, facts, plan))) => {
                if expired {
                    lock(&self.shared.roam_watch).take();
                    session.stop();
                    Err(ConnectFailed::Message("Automatic recovery stopped after one minute.".into()))
                } else {
                    self.plan = plan;
                    Ok(Some((session, facts)))
                }
            }
            Ok(Err(e)) => Err(if expired {
                ConnectFailed::Message("Automatic recovery stopped after one minute.".into())
            } else if e.asks_replace {
                ConnectFailed::ConfirmReplace(Box::new(self.plan.clone()))
            } else {
                ConnectFailed::Message(e.message)
            }),
            Err(std::sync::mpsc::TryRecvError::Empty) => Ok(None),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(ConnectFailed::Message("The connection attempt ended.".into())),
        }
    }

    pub fn cancel(&self) {
        self.interrupt.interrupt();
        if let Some(client) = &*lock(&self.shared.host_client) {
            client.cancel_pending_requests();
        }
    }
}

struct ConnectError {
    message: String,
    retryable: bool,
    /// The PC asks the user to confirm closing the running app.
    asks_replace: bool,
}

impl From<String> for ConnectError {
    fn from(message: String) -> Self { Self { message, retryable: false, asks_replace: false } }
}

impl From<&str> for ConnectError {
    fn from(message: &str) -> Self { message.to_owned().into() }
}

fn connect_with_recovery(plan: &mut LaunchPlan, shared: Arc<Shared>, interrupt: &Interrupt) -> Result<(Session, Facts), ConnectError> {
    loop {
        if let Some(recovery) = &mut plan.recovery {
            let delay = recovery.next_delay(recovery::now_ms()).ok_or("Automatic recovery exhausted its retry budget.")?;
            shared.notify(Notice::Reconnecting(recovery.attempts() as usize));
            let until = Instant::now() + Duration::from_millis(delay);
            while Instant::now() < until {
                if interrupt.is_interrupted() { return Err("Cancelled.".into()); }
                std::thread::sleep(Duration::from_millis(20).min(until.saturating_duration_since(Instant::now())));
            }
            let port = if plan.record.http_port == 0 { nightjar_core::http::DEFAULT_HTTP_PORT } else { plan.record.http_port };
            wait_for_route(&plan.record.address, port, recovery.remaining(recovery::now_ms()), interrupt)?;
        }
        match connect(plan, shared.clone(), interrupt) {
            Err(error) if error.retryable && plan.recovery.is_some() && !interrupt.is_interrupted() => {
                lock(&shared.frames).clear();
                shared.next_attempt();
            }
            result => return result,
        }
    }
}

/// Before a recovery attempt: returns once the PC is reachable, or when the recovery window ends
/// (the attempt then fails and the budget says so), Err when cancelled. A network is often back
/// before its address toward the PC (DHCP), so the attempt waits for a route instead of failing
/// at once and backing off.
fn wait_for_route(address: &str, port: u16, remaining_ms: u64, interrupt: &Interrupt) -> Result<(), ConnectError> {
    let started = Instant::now();
    let mut wait = recovery::RouteWait::new(recovery::now_ms() + remaining_ms);
    loop {
        if interrupt.is_interrupted() { return Err("Cancelled.".into()); }
        let checked = Instant::now();
        let problem = route_problem(address, port);
        match wait.next(recovery::now_ms(), problem.is_none()) {
            Some(0) => {
                if wait.unreachable_checks() > 0 {
                    eprintln!("nightjar: recovery: the PC is reachable again after {} ms", started.elapsed().as_millis());
                }
                return Ok(());
            }
            None => {
                eprintln!("nightjar: recovery: the PC stayed unreachable until the recovery window ended: {}",
                    problem.unwrap_or_default());
                return Ok(());
            }
            Some(delay) => {
                if wait.unreachable_checks() == 1 {
                    eprintln!("nightjar: recovery: no route to the PC yet ({}), checking every {} ms",
                        problem.unwrap_or_default(), recovery::ROUTE_RETRY_MS);
                }
                let until = checked + Duration::from_millis(delay);
                while Instant::now() < until {
                    if interrupt.is_interrupted() { return Err("Cancelled.".into()); }
                    std::thread::sleep(Duration::from_millis(20).min(until.saturating_duration_since(Instant::now())));
                }
            }
        }
    }
}

/// Why there is no route toward the PC's HTTP port yet ([`recovery::route_missing`], or the
/// name does not resolve), or None to go on with the attempt: an answer, refused included, and
/// also a check that timed out or failed otherwise.
fn route_problem(address: &str, port: u16) -> Option<String> {
    let remote = match (address, port).to_socket_addrs().map(|mut found| found.next()) {
        Ok(Some(remote)) => remote,
        Ok(None) => return Some(format!("{address} did not resolve")),
        Err(error) => return Some(error.to_string()),
    };
    match std::net::TcpStream::connect_timeout(&remote, Duration::from_millis(recovery::ROUTE_PROBE_TIMEOUT_MS)) {
        Err(error) if recovery::route_missing(&error) => Some(error.to_string()),
        _ => None,
    }
}

/// The overlay's early decode line: frames per second decoded ahead, and the fallbacks of the
/// last interval and in all (`now` and `last` are the early frame and fallback totals).
fn early_line(now: (u64, u64), last: (u64, u64), secs: f64) -> String {
    trf("{} early/s, {} whole in the last second, {} fallbacks in all",
        &[&format!("{:.0}", now.0.saturating_sub(last.0) as f64 / secs), &now.1.saturating_sub(last.1), &now.1])
}

/// The overlay's line while the host holds the bitrate down for the network: whole Mbps from 10,
/// one decimal below, as Nightjar Android says it.
fn link_notice_line(notice: &LinkNotice) -> String {
    let mbps = f64::from(notice.current_kbps) / 1000.0;
    let mbps = if mbps >= 10.0 { format!("{mbps:.0}") } else { format!("{mbps:.1}") };
    trf("Host lowered the bitrate to {} Mbps: network", &[&mbps])
}

/// The overlay's host pipeline line: the frames per second each host stage handled over the
/// host's last window (0x553D), "-" for a stage the host can't count.
fn pipeline_rates_line(report: &PipelineRatesReport) -> String {
    let fps = |mhz: u32| if mhz == UNKNOWN { "-".to_owned() } else { format!("{:.1}", f64::from(mhz) / 1000.0) };
    trf("{} source, {} captured, {} encoded, {} sent, {} repeated fps", &[
        &fps(report.source_mhz),
        &fps(report.capture_mhz),
        &fps(report.submit_mhz),
        &fps(report.output_mhz),
        &fps(report.repeat_mhz),
    ])
}

/// The line logged when any session ends: what the decoder was given, and the frames lost
/// before the picture (over the decoder queue, or replaced by a newer one before they were due).
fn video_summary(submitted: u64, queue_overflows: u64, replaced: u64) -> String {
    format!("nightjar: video: {submitted} frames to the decoder, {queue_overflows} dropped over the decoder queue, \
        {replaced} replaced before they were shown")
}

/// The line logged when an early decode session ends: the three counters of the decoder and
/// the chunks the queue dropped.
fn early_summary(early_frames: u64, fallbacks: u64, chunk_us: u64, chunks_dropped: u64) -> String {
    format!("nightjar: early decode: {early_frames} frames decoded ahead, {fallbacks} fell back to the whole frame, \
        {chunks_dropped} chunks dropped, {:.1} ms of decoding done while frames arrived", chunk_us as f64 / 1000.0)
}

/// The beam race in one line (overlay and session end): frames decoded in slices as they arrived,
/// the slice decodes made, and the frames that were not in row order and decoded whole. A host
/// that never sends row order says so instead of showing zeros.
fn race_line(frames: u64, slices: u64, unordered: u64) -> String {
    let mut line = format!("{frames} frames in {slices} slices, {unordered} not in row order");
    if frames == 0 && unordered > 0 {
        line.push_str(" (the host is not sending the row order)");
    }
    line
}

/// Whether a stream runs the display check for VRR: every VRR stream (`vrr`: the setting is on and
/// the swap chain tears) that presents with tearing and would pace otherwise without VRR
/// (`fallback`), unless `NIGHTJAR_VRR_CHECK` (`forced`) says off. The VRR setting is on by default,
/// and without the check a fixed refresh display tears.
fn checks_display_for_vrr(vrr: bool, pacing: PacingMode, fallback: PacingMode, forced: Option<bool>) -> bool {
    vrr && pacing != PacingMode::Vsync && fallback != pacing && forced != Some(false)
}

/// Whether the stream reads the scan line around its presents, and the topic of its log lines:
/// for the beam race, for the VRR display check, and for `NIGHTJAR_VRR_SYNC=scan`, which waits on
/// the beam. None for every other stream.
fn display_probe(beam_race: bool, vrr: &present::VrrPacing) -> Option<&'static str> {
    if beam_race {
        Some("beam race")
    } else if vrr.check || vrr.sync == present::VrrSync::Scan {
        Some("display check")
    } else {
        None
    }
}

/// The overlay's pacing, which says why a VRR stream waits for the refresh (or tears without
/// V-Sync) when the display check changed it.
fn pacing_label(mode: PacingMode, fell_back: bool) -> &'static str {
    match mode {
        PacingMode::Vsync if fell_back => "V-Sync: this display does not refresh on present",
        PacingMode::Immediate if fell_back => "Immediate: this display does not refresh on present",
        mode => mode.label(),
    }
}

/// What early decode does on a stream. `NoRecords` is the failure the earlier measurements
/// hit unseen: a host whose version did not parse never offered records, so no chunk ever came.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EarlyState {
    Off,
    On,
    NoRecords,
}

const EARLY_NO_RECORDS: &str = "nightjar: early decode is on but the host sent no PyroWave records \
    (its version did not offer them, or the client did not ask), so frames are decoded whole";

fn early_state(requested: bool, pyrowave: bool, records: bool) -> EarlyState {
    match (requested && pyrowave, records) {
        (false, _) => EarlyState::Off,
        (true, true) => EarlyState::On,
        (true, false) => EarlyState::NoRecords,
    }
}

/// What the beam race setting does on a stream: the client asks the host for the row order and
/// decodes the picture in slices as its bands complete. `NotWired` is the setting on a link that
/// is not known to be wired (the host's row order is for wired links), `NoGpuDecoder` the setting
/// when the stream decodes PyroWave on the CPU (only the GPU decoder slices a frame). Both are
/// reported, not silent, and neither asks the host for the row order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BeamRace {
    Off,
    On,
    NotWired,
    NoGpuDecoder,
}

impl BeamRace {
    /// Why the setting did nothing, for the overlay.
    fn reason(self) -> Option<&'static str> {
        match self {
            Self::NotWired => Some("off, the link is not a known wired one"),
            Self::NoGpuDecoder => Some("off, PyroWave is decoded on the CPU"),
            Self::Off | Self::On => None,
        }
    }
}

/// Everything the beam race setting asks of a stream, in one place: the row order from the host,
/// early decode (the slices are decoded from chunks) and the slice count. All of it is off
/// unless the decision is `On`, so a stream without the setting asks the host for nothing, starts
/// no scan line probe and decodes whole frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RaceRequest {
    row_order: bool,
    early_decode: bool,
    slices: u32,
}

fn race_request(beam: BeamRace, early_setting: bool, height: u32) -> RaceRequest {
    let on = beam == BeamRace::On;
    RaceRequest { row_order: on, early_decode: early_setting || on, slices: if on { beam_slices(height) } else { 0 } }
}

/// Whether the race runs on the stream (the presenter probes the display, follows the slices and
/// races them): slices were asked for, and early decode has the records they come from. This is
/// the one bool the presenter checks; false makes it all a no-op.
fn race_runs(slices: u32, early: EarlyState) -> bool {
    slices > 0 && early == EarlyState::On
}

const BEAM_RACE_NO_GPU: &str = "nightjar: beam race is on but PyroWave is decoded on the CPU, so the host is not asked \
    for the row order and frames are decoded whole";

const BEAM_RACE_NOT_WIRED: &str = "nightjar: beam race is on but the link is not a known wired one, so the host is not asked \
    for the row order and frames are decoded whole";

/// An on or off switch in an environment variable: `1`, `true`, `yes` or `on`, and `0`, `false`,
/// `no` or `off` (any case). Anything else is not a switch.
fn flag_value(text: &str) -> Option<bool> {
    match text.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// The switch `name` names, if it is set. A value that is not a switch is said so on stderr and
/// ignored, never taken as on or off.
fn env_flag(name: &str) -> Option<bool> {
    let text = std::env::var(name).ok()?;
    let value = flag_value(&text);
    if value.is_none() {
        eprintln!("nightjar: {name}={text:?} is not 1, true, yes, on, 0, false, no or off, so it is ignored");
    }
    value
}

/// The setting, unless `NIGHTJAR_BEAM_RACE` says on (whatever the link; for measurement runs) or
/// off. Needs the GPU decoder whatever asked.
fn beam_race(setting: bool, link_mbps: u32, gpu_decoder: bool) -> BeamRace {
    beam_race_for(env_flag("NIGHTJAR_BEAM_RACE"), setting, link_mbps, gpu_decoder)
}

fn beam_race_for(forced: Option<bool>, setting: bool, link_mbps: u32, gpu_decoder: bool) -> BeamRace {
    match forced {
        Some(false) => BeamRace::Off,
        Some(true) if gpu_decoder => BeamRace::On,
        Some(true) => BeamRace::NoGpuDecoder,
        _ if !setting => BeamRace::Off,
        _ if !gpu_decoder => BeamRace::NoGpuDecoder,
        _ if link_mbps == 0 => BeamRace::NotWired,
        _ => BeamRace::On,
    }
}

/// Most slices per frame: a frame has a present for each batch of slices and one for the picture,
/// and the timing CSV has room for `PRESENTS` of them.
const MAX_RACE_SLICES: u32 = crate::timing_csv::PRESENTS as u32 - 1;

/// Slices per frame: 4, or 3 at 4K where each slice decode costs more (`NIGHTJAR_BEAM_RACE_SLICES`
/// overrides, for measurement runs, up to `MAX_RACE_SLICES`).
fn beam_slices(height: u32) -> u32 {
    let default = if height >= 2000 { 3 } else { 4 };
    match std::env::var("NIGHTJAR_BEAM_RACE_SLICES") {
        Ok(text) => slices_override(&text).unwrap_or_else(|| {
            eprintln!("nightjar: NIGHTJAR_BEAM_RACE_SLICES={text:?} is not a number from 1 to {MAX_RACE_SLICES}, so {default} slices are used");
            default
        }),
        Err(_) => default,
    }
}

fn slices_override(text: &str) -> Option<u32> {
    text.trim().parse().ok().filter(|n| (1..=MAX_RACE_SLICES).contains(n))
}

/// Whether to send `x-rl-video[0].clientWireless`: the setting is on, the route is wireless and the
/// PC says it is a Rubylight host, the only host that reads it.
fn tells_host_wireless(setting: bool, family: nightjar_core::http::Family, wireless: bool) -> bool {
    setting && wireless && family == nightjar_core::http::Family::Rubylight
}

/// The value a network-hint variable names, if it names one (measurement runs on loopback, where
/// the real link is not the one under test). A value that is not a number is said so on stderr
/// and ignored.
fn hint_env(name: &str) -> Option<u32> {
    let text = std::env::var(name).ok()?;
    let parsed = text.trim().parse().ok();
    if parsed.is_none() {
        eprintln!("nightjar: {name}={text:?} is not a number, so it is ignored");
    }
    parsed
}

/// `value`, unless the variable names another one.
fn hint_override(name: &str, value: u32) -> u32 {
    hint_env(name).unwrap_or(value)
}

/// The link speed and pace the stream works with and what it tells the host.
#[derive(Debug, PartialEq)]
struct LinkHints {
    /// The speed the beam-race check goes by, whatever the host is told.
    link_mbps: u32,
    /// `x-ss-video[0].pyrowaveLinkMbps`; zero leaves the attribute out.
    sent_link_mbps: u32,
    /// `x-ss-video[0].pyrowavePaceMbps`, calibrated or overridden.
    pace_mbps: u32,
}

/// With "Tell the PC the link speed" off the host gets no speed, but the beam race and the
/// calibrated pace (measured and keyed on the real speed) still use it. A variable naming the
/// speed wins over the setting, so measurement runs keep their hint.
fn link_hints(tell: bool, routed_mbps: u32, link_env: Option<u32>, measured_pace: u32, pace_env: Option<u32>) -> LinkHints {
    let link_mbps = link_env.unwrap_or(routed_mbps);
    LinkHints {
        link_mbps,
        sent_link_mbps: if tell || link_env.is_some() { link_mbps } else { 0 },
        pace_mbps: pace_env.unwrap_or(measured_pace),
    }
}

/// `Auto` (moonlight-common-c's choice from the address), unless `NIGHTJAR_STREAM_LOCATION` is
/// `local` or `remote`: loopback counts as remote there and gets 1024 byte packets, which is not
/// the packet size of a wired LAN stream, so measurement runs on loopback say `local`.
fn location_override() -> Location {
    match std::env::var("NIGHTJAR_STREAM_LOCATION").as_deref() {
        Ok("local") => Location::Local,
        Ok("remote") => Location::Remote,
        _ => Location::Auto,
    }
}

/// The location and video packet size for a route to the PC: `Auto` over a VPN
/// ([`crate::network::route`]) streams as remote with 1024 byte packets, whatever the PC's
/// address, as Android does for a VPN network and Moonlight V+ PC for its `RI_VPN`: a tunnel's
/// smaller MTU (WireGuard's 1420) would fragment every 1392 byte packet. Otherwise 1392, and the
/// engine decides `Auto` from the address.
fn location_for_route(location: Location, vpn: bool) -> (Location, i32) {
    match location {
        Location::Auto if vpn => (Location::Remote, 1024),
        location => (location, 1392),
    }
}

/// The engine's session flags for this role and these settings. A Remote Monitor session is
/// view-only: the engine starts no input stream and does not wiggle the host's mouse.
/// `watching` says the network watcher runs and feeds `Session::network_changed`.
fn session_config(settings: &Settings, role: AppRole, stream: StreamConfig, location: Location, watching: bool) -> SessionConfig {
    SessionConfig {
        stream,
        location,
        network_grace: Duration::from_millis(u64::from(settings.network_grace_ms)),
        input_only: role == AppRole::InputOnly,
        remote_monitor: role == AppRole::RemoteMonitor,
        unbatched_input: settings.unbatched_input,
        network_priority: settings.network_priority,
        roaming: roaming_config(settings, watching),
    }
}

/// Client roaming as set. With the watcher running its hints say which networks are metered;
/// without it the session cannot tell, so roaming then needs "Continue on mobile data" too.
fn roaming_config(settings: &Settings, watching: bool) -> RoamingConfig {
    RoamingConfig {
        enabled: settings.client_roaming && (watching || settings.roaming_metered),
        metered: settings.roaming_metered,
        platform_hints: watching,
    }
}

/// Over the frozen picture while the stream's network is gone; the second when a reconnect follows.
const NETWORK_LOST: &str = en("Network lost.");
const NETWORK_LOST_RECONNECTING: &str = en("Network lost. Reconnecting as soon as it is back…");

/// The stream's network watcher toward the PC, feeding the stream through `Notice`s, or None
/// with why on stderr. It runs for every stream: a lost route that comes back where the
/// connection cannot carry on reconnects at once; with client roaming its changes are also the
/// session's hints.
fn roam_watch(settings: &Settings, address: &str, shared: &Arc<Shared>) -> Option<crate::network::RoamWatch> {
    let unwatched = if !settings.client_roaming || settings.roaming_metered { "" } else { " and roaming is off" };
    let Some(remote) = (address, 47989).to_socket_addrs().ok().and_then(|mut found| found.next()) else {
        eprintln!("nightjar: roaming: {address} did not resolve, network changes go unwatched{unwatched}");
        return None;
    };
    let shared = Arc::downgrade(shared);
    let on_change = move |event| {
        let notice = match event {
            crate::network::LinkEvent::Lost => Notice::NetworkLost,
            crate::network::LinkEvent::Changed { metered, after_loss } => Notice::NetworkChanged(NetworkHint { metered }, after_loss),
        };
        shared.upgrade().map(|s| s.notify(notice)).is_some()
    };
    crate::network::RoamWatch::start(remote, on_change)
        .map_err(|e| eprintln!("nightjar: roaming: network changes go unwatched{unwatched}: {e}"))
        .ok()
}

/// Keeps the watcher for the stream. The session roams on its changes only when the host roams
/// and client roaming is on, and tells on every stream whether a lost network's return lets the
/// connection carry on.
fn keep_roam_watch(shared: &Shared, watch: Option<crate::network::RoamWatch>) {
    *lock(&shared.roam_watch) = watch;
}

/// The setting, unless `NIGHTJAR_EARLY_DECODE` is `1` or `0` (for measurement runs).
fn early_decode(setting: bool) -> bool {
    match std::env::var("NIGHTJAR_EARLY_DECODE").as_deref() {
        Ok("1") => true,
        Ok("0") => false,
        _ => setting,
    }
}

/// The plan that rejoins a stream of `role` that `plan` started: a resume of the running app,
/// never the answer to a replace prompt. None when there is no app to resume.
fn rejoin(mut plan: LaunchPlan, role: AppRole) -> Option<LaunchPlan> {
    // Only the launch the user answered closes the running app; a reconnect never does.
    plan.confirm_replace = false;
    plan.resume = role == AppRole::Stream;
    if plan.resume && plan.app.control != nightjar_core::http::AppControl::None {
        if plan.info.current_game == 0 { return None; }
        plan.app.id = plan.info.current_game;
        plan.app.uuid = plan.info.current_game_uuid.clone().unwrap_or_default();
        plan.app.control = nightjar_core::http::AppControl::None;
    }
    Some(plan)
}

/// Whether a dropped stream of `role` reconnects on its own: the setting is on and the window is on
/// screen. A window without focus still shows the stream behind another one, so it reconnects; a
/// minimized or hidden one (`away`) waits for the user, as the Android app does in the background.
fn reconnects_automatically(auto_reconnect: bool, away: bool, role: AppRole) -> bool {
    auto_reconnect && !away && role == AppRole::Stream
}

/// The plan that reconnects a dropped stream of `role` that `plan` started, on its own, or None
/// when it waits for the user ([`reconnects_automatically`]) or there is nothing to rejoin. Its
/// budget renews after `connected_ms` of a stable stream; `retry_now`: the first attempt starts
/// without its delay (it still spends one).
fn automatic_rejoin(plan: &LaunchPlan, role: AppRole, auto_reconnect: bool, away: bool, connected_ms: u64, retry_now: bool) -> Option<LaunchPlan> {
    if !reconnects_automatically(auto_reconnect, away, role) { return None; }
    let mut plan = rejoin(plan.clone(), role)?;
    let recovery = plan.recovery.get_or_insert_with(Default::default);
    recovery.reset_after(connected_ms);
    if retry_now { recovery.retry_now(); }
    Some(plan)
}

/// The lost-network notice: it promises a reconnect only when one will follow (`on_its_own`: a
/// dropped stream reconnects by itself, so the connection's timeout or a return it cannot carry
/// on over recovers it), not where a network drop tolerance holds the connection instead.
fn lost_notice(settings: &Settings, on_its_own: bool) -> &'static str {
    if on_its_own && settings.network_grace_ms <= recovery::PEER_TIMEOUT_MS { NETWORK_LOST_RECONNECTING } else { NETWORK_LOST }
}

/// A network is back after the route was gone: whether the stream ends now to reconnect at once.
/// Only when the session says its connection cannot carry on (another address, not roamed): it
/// would only time out. Roamed, or the same address back, or no route yet, the stream goes on.
fn reconnects_on_return(change: NetworkChange, metered: bool, settings: &Settings, on_its_own: bool) -> bool {
    on_its_own && recovery::reconnects_at_once(change == NetworkChange::CannotContinue, metered,
        settings.roaming_metered, settings.auto_reconnect, settings.network_grace_ms)
}

/// Whether a refused launch of `plan` is the PC's prompt to close the running app, which the
/// user answers. A launch that already confirmed is not asked again, nor is a resume.
fn asks_replace(plan: &LaunchPlan, role: AppRole, error: &nightjar_core::http::Error) -> bool {
    !plan.resume && !plan.confirm_replace && role == AppRole::Stream && nightjar_core::http::asks_to_confirm_replace(error)
}

/// Whether this PC's default output plays 7.1.4 (12 channels or more): only Windows, through
/// WASAPI; SDL opens at most 8 channels, so Linux stays at 7.1.
pub fn plays_714() -> bool {
    #[cfg(windows)]
    return wasapi::plays_714();
    #[cfg(not(windows))]
    false
}

/// [`plays_714`] for the audio setting, which asks every frame it is shown.
pub fn offers_714() -> bool {
    #[cfg(windows)]
    return wasapi::offered();
    #[cfg(not(windows))]
    false
}

/// What the stream asks the PC for. 7.1.4 needs a PC that reports it in serverinfo
/// (`AudioLayouts`, which Rubylight sends while it streams 7.1.4) and an output that plays it;
/// otherwise 7.1, as a PC without 7.1.4 may answer 12 channels with no sound at all. Foundation
/// Sunshine isn't told apart from stock Sunshine (Moonlight V+ asks every PC), so it gets 7.1.
fn audio_configuration(choice: AudioChannels, host_714: bool, output_714: bool) -> AudioConfiguration {
    match choice {
        AudioChannels::Stereo => AudioConfiguration::STEREO,
        AudioChannels::Surround51 => AudioConfiguration::SURROUND_51,
        AudioChannels::Surround71 => AudioConfiguration::SURROUND_71,
        AudioChannels::Surround714 if host_714 && output_714 => AudioConfiguration::SURROUND_714,
        AudioChannels::Surround714 => {
            let why = if host_714 { "this output doesn't play 12 channels" } else { "the PC doesn't report 7.1.4" };
            eprintln!("nightjar: audio: {why}; asking for 7.1");
            AudioConfiguration::SURROUND_71
        }
    }
}

fn connect(plan: &mut LaunchPlan, shared: Arc<Shared>, interrupt: &Interrupt) -> Result<(Session, Facts), ConnectError> {
    let settings = &plan.settings;
    let role = plan.app.role(plan.small_stream_input_only);
    let (width, height, fps, bitrate) = stream_mode(settings, plan.display);
    let bitrate = capped_bitrate(bitrate, plan.bitrate_limit_kbps);
    let audio = audio_configuration(
        settings.audio,
        plan.info.capabilities.has(Feature::Surround714),
        settings.audio == AudioChannels::Surround714 && plays_714(),
    );
    let ri_key: [u8; 16] = random_bytes();
    let key_id_bytes: [u8; 4] = random_bytes();
    let ri_key_id = i32::from_be_bytes(key_id_bytes) & 0x7FFF_FFFF;
    let refresh_x100 = (plan.display.refresh_hz * 100.0).round() as u32;
    let mut request = LaunchRequest {
        app_id: plan.app.id,
        app_uuid: plan.app.uuid.clone(),
        role,
        width,
        height,
        fps,
        client_refresh_rate_x100: refresh_x100,
        vrr: plan.vrr,
        hdr: settings.hdr,
        audio,
        ri_key,
        ri_key_id,
        play_local_audio: settings.play_audio_on_host,
        gamepad_mask: plan.gamepad_mask,
        confirm_replace: plan.confirm_replace,
        prepared_controllers: if role != AppRole::RemoteMonitor && plan.info.capabilities.has(Feature::ControllerPreparation) {
            plan.prepared_controllers.clone()
        } else { String::new() },
        virtual_display: settings.virtual_display,
        virtual_display_scale: settings.virtual_display_scale,
        virtual_display_dpi: settings.virtual_display_dpi,
        host_display: settings.host_display.into(),
        fec: Some(settings.error_correction.into()),
        small_stream_input_only: plan.small_stream_input_only,
        // The host lists 0x553C in DESCRIBE only when asked.
        rate_change: rate_change::asked_at_launch(role),
        clipboard_sync: settings.clipboard_sync && role != AppRole::RemoteMonitor,
        ..Default::default()
    };
    let port = if plan.record.http_port == 0 {
        nightjar_core::http::DEFAULT_HTTP_PORT
    } else {
        plan.record.http_port
    };
    let pem = plan
        .record
        .server_certificate
        .as_deref()
        .ok_or_else(|| tr("This PC is not paired."))?;
    let client = HostClient::new(&plan.record.address, port, plan.identity.clone())
        .and_then(|c| c.with_server_certificate_pem(pem))
        .map_err(|e| e.to_string())?
        .with_https_port(plan.info.https_port);
    let client = Arc::new(client);
    *lock(&shared.host_client) = Some(client.clone());
    if interrupt.is_interrupted() {
        client.cancel_pending_requests();
        return Err("Cancelled.".into());
    }
    if plan.resume {
        plan.info = client.server_info_https().map_err(|e| ConnectError {
            retryable: recovery::transient_http(&e), message: format!("Checking the running app failed: {e}"), asks_replace: false,
        })?;
        crate::hosts::merge_native(&mut plan.info, plan.native.as_ref());
        if !plan.info.paired || !plan.app.matches_running_app(plan.info.current_game, plan.info.current_game_uuid.as_deref()) {
            return Err("The original app is no longer running on the PC. Choose an app to start a new stream.".into());
        }
    }
    let scm = plan
        .info
        .capabilities
        .trusted_codec_mode_support(plan.info.server_codec_mode_support);
    let offer = offered_formats(settings, scm, &plan.color_decodable);
    request.ten_bit_sdr = launches_ten_bit_sdr(settings, &offer);
    let bitrate = if probes_first_bitrate(settings, plan.resume, &plan.info) {
        match client.probe_link(&plan.info) {
            Ok(mbps) => {
                let first = nightjar_core::http::probe_start_kbps(bitrate, mbps);
                eprintln!("nightjar: bandwidth test {mbps:.1} Mbit/s, starting at {first} kbps");
                first
            }
            // The stream still starts, at the automatic bitrate, and says why it was not tested.
            Err(e) => {
                eprintln!("nightjar: bandwidth test failed, starting at {bitrate} kbps: {e}");
                bitrate
            }
        }
    } else { bitrate };
    if interrupt.is_interrupted() {
        client.cancel_pending_requests();
        return Err("Cancelled.".into());
    }
    let launched = if plan.resume {
        client.resume(&request, &plan.info)
    } else {
        client.launch(&request, &plan.info)
    };
    // A host that takes one stream at a time refuses the small stream while another one runs.
    let refused = |why: String| -> ConnectError {
        if plan.small_stream_input_only {
            format!("{why} {}", tr("Input-only on this PC uses a small ordinary stream, and the host refused it. Some hosts allow only one stream at a time: end the other stream or allow more in the host’s settings.")).into()
        } else { why.into() }
    };
    let launched = match launched {
        // The answer goes out with a new launch that says it confirms.
        Err(e) if asks_replace(plan, role, &e) => {
            return Err(ConnectError { message: e.to_string(), retryable: false, asks_replace: true })
        }
        Ok(Some(launched)) => launched,
        Ok(None) => return Err(refused("The PC declined to start the app.".into())),
        Err(nightjar_core::http::Error::Status { code, message }) => {
            return Err(refused(format!("The PC could not start the app ({code}: {message}).")))
        }
        Err(e) => return Err(ConnectError { retryable: recovery::transient_http(&e), message: format!("Starting the app failed: {e}"), asks_replace: false }),
    };
    if interrupt.is_interrupted() {
        return Err("Cancelled.".into());
    }
    // The interface the system sends this host's traffic over decides, not the internet
    // connection; the connection is the fallback when that lookup tells nothing.
    let route = crate::network::route(&plan.record.address);
    let toward = route.toward;
    let network = plan.network.toward(toward);
    let family = plan.info.capabilities.family();
    let client_wireless = tells_host_wireless(settings.tell_host_wireless, family, network.wireless());
    let routed_link_mbps = network.wired_link_mbps();
    if network.wireless() && !client_wireless {
        let why = if settings.tell_host_wireless { "the PC is not a Rubylight host" } else { "it is turned off in the settings" };
        eprintln!("nightjar: not telling the PC the route is wireless: {why}");
    }
    eprintln!(
        "nightjar: route to the PC: {} ({}), link {routed_link_mbps} Mbps",
        if network.wireless() { "wireless" } else { "wired or unknown" },
        match toward {
            Ok(_) => "from the route".to_string(),
            Err(why) => format!("from the internet connection, {why}"),
        },
    );
    let measured_pace = plan.pace.as_ref().map_or(0, |pace| pace.for_network(&network, plan.host_link_mbps));
    let LinkHints { link_mbps, sent_link_mbps, pace_mbps } = link_hints(
        settings.tell_host_link_speed,
        routed_link_mbps,
        hint_env("NIGHTJAR_PYROWAVE_LINK_MBPS"),
        measured_pace,
        hint_env("NIGHTJAR_PYROWAVE_PACE_MBPS"),
    );
    if link_mbps > 0 && sent_link_mbps == 0 {
        eprintln!("nightjar: not telling the PC the link speed ({link_mbps} Mbps): it is turned off in the settings");
    }
    let beam = beam_race(settings.beam_race, link_mbps, plan.gpu_decoder.is_some());
    match beam {
        BeamRace::NotWired => eprintln!("{BEAM_RACE_NOT_WIRED}"),
        BeamRace::NoGpuDecoder => eprintln!("{BEAM_RACE_NO_GPU}"),
        BeamRace::On => eprintln!("nightjar: beam race is on (experimental): asking the host for the row order"),
        BeamRace::Off => {}
    }
    let race_ask = race_request(beam, early_decode(settings.early_decode), height);
    let (location, packet_size) = location_for_route(location_override(), route.vpn);
    if route.vpn && location == Location::Remote {
        eprintln!("nightjar: the route to the PC is a VPN: streaming as remote with {packet_size} byte packets");
    }
    let mut iv = [0u8; 16];
    iv[..4].copy_from_slice(&ri_key_id.to_be_bytes());
    let stream = StreamConfig {
        width: width as i32,
        height: height as i32,
        fps: fps as i32,
        bitrate_kbps: bitrate as i32,
        packet_size,
        audio,
        supported_video_formats: offer.formats,
        experimental_packed_444: settings.experimental_packed_444,
        packed444_modes: plan.packed_modes,
        client_refresh_rate_x100: refresh_x100 as i32,
        color_space: if settings.hdr { COLORSPACE_REC_2020 } else { COLORSPACE_REC_709 },
        color_range: if settings.full_range { COLOR_RANGE_FULL } else { COLOR_RANGE_LIMITED },
        microphone: settings.microphone && role != AppRole::RemoteMonitor,
        audio_passthrough: pipeline::passthrough_codecs(settings.audio_passthrough),
        haptics_pcm: settings.controllers && settings.controller_audio && role != AppRole::RemoteMonitor,
        pyrowave_link_mbps: sent_link_mbps as i32,
        pyrowave_pace_mbps: pace_mbps as i32,
        client_wireless,
        pyrowave_records_supported: plan.info.capabilities.has(Feature::PyroWaveRecords),
        pyrowave_row_order: race_ask.row_order,
        encryption_flags: ENCFLG_ALL,
        remote_input_aes_key: ri_key,
        remote_input_aes_iv: iv,
        video_capabilities: 0,
        audio_capabilities: CAPABILITY_DIRECT_SUBMIT,
        video_fec: settings.error_correction.into(),
        ..Default::default()
    };
    let host = HostInfo {
        address: plan.record.address.clone(),
        app_version: plan.info.app_version.clone().unwrap_or_else(|| "7.1.431.-1".into()),
        rtsp_session_url: launched.rtsp_session_url,
        server_codec_mode_support: if scm != 0 { scm } else { SCM_H264 },
    };
    // The slices are decoded from chunks, so the beam race needs early decode.
    let (early, slices) = (race_ask.early_decode, race_ask.slices);
    // Started before the handshake so a change during it still reaches the session.
    let watch = roam_watch(settings, &plan.record.address, &shared);
    let roam_shared = shared.clone();
    // Asked beside the handshake: the stream controls offer USB devices only when the PC does.
    let usb_ask = (settings.usb_forwarding && plan.info.capabilities.has(Feature::NativeApi)).then(|| {
        let client = client.clone();
        std::thread::spawn(move || client.native_capabilities_with_files().map_err(|e| e.to_string()))
    });
    let passthrough = stream.audio_passthrough;
    let session = Session::start(
        &host,
        session_config(settings, role, stream, location, watch.is_some() && settings.client_roaming),
        Box::new(pipeline::Decoder::new(shared.clone(), plan.hardware).with_early_decode(early).with_beam_race(slices).with_gpu_decoder(plan.gpu_decoder).with_dmabuf(plan.dmabuf)),
        Box::new(pipeline::Audio::new(shared.clone(), passthrough)),
        Arc::new(pipeline::Events::new(shared)),
        interrupt,
    )
    .map_err(|e| ConnectError { retryable: recovery::transient_start(e.stage, e.code), message: format!("Could not start the stream: {e}"), asks_replace: false })?;
    let outcome = session.outcome();
    keep_roam_watch(&roam_shared, watch);
    let mut facts = Facts {
        role,
        width: outcome.config.width,
        height: outcome.config.height,
        fps: outcome.config.fps,
        bitrate_kbps: outcome.config.bitrate_kbps,
        format: outcome.negotiated_video_format,
        host_timing: outcome.host_control_messages.contains(&HOST_TIMING_MESSAGE_TYPE),
        link_notice: outcome.host_control_messages.contains(&LINK_NOTICE_MESSAGE_TYPE),
        pipeline_rates: outcome.host_control_messages.contains(&PIPELINE_RATES_MESSAGE_TYPE),
        clock_sync: outcome.host_control_messages.contains(&CLOCK_SYNC_MESSAGE_TYPE),
        display_caps: outcome.host_control_messages.contains(&rubylight_protocol::control::DISPLAY_CAPS_MESSAGE_TYPE),
        cursor_shapes: plan.info.capabilities.has(Feature::CursorShape) && outcome.host_control_messages.contains(&0x5540),
        text_input: plan.info.capabilities.has(Feature::TextInput)
            && outcome.host_control_messages.contains(&rubylight_protocol::text_input::TEXT_INPUT_MESSAGE_TYPE)
            && role != AppRole::RemoteMonitor,
        keyboard_layout: plan.info.capabilities.has(Feature::ClientKeyboardLayout)
            && outcome.host_control_messages.contains(&rubylight_protocol::keyboard_layout::KEYBOARD_LAYOUT_MESSAGE_TYPE)
            && role != AppRole::RemoteMonitor,
        clipboard: plan.info.capabilities.has(Feature::Clipboard) && role != AppRole::RemoteMonitor,
        clipboard_sync: settings.clipboard_sync
            && outcome.host_control_messages.contains(&rubylight_protocol::clipboard_sync::CLIPBOARD_SYNC_MESSAGE_TYPE)
            && role != AppRole::RemoteMonitor,
        image_clipboard: plan.info.capabilities.has(Feature::ImageClipboard) && role != AppRole::RemoteMonitor,
        server_commands: if plan.info.capabilities.has(Feature::ServerCommands) && role != AppRole::RemoteMonitor {
            plan.info.server_commands.iter().take(256).cloned().collect()
        } else { Vec::new() },
        runtime_bitrate: plan.info.capabilities.has(Feature::RuntimeBitrate) && role != AppRole::InputOnly,
        reconfigure: outcome.host_control_messages.contains(&RECONFIGURE_MESSAGE_TYPE)
            && !outcome.config.experimental_packed_444 && outcome.negotiated_video_format & VIDEO_FORMAT_MASK_PYROWAVE == 0 && role == AppRole::Stream,
        rate_change: rate_change::offered(&outcome.host_control_messages, role),
        usb: if settings.usb_forwarding {
            let native = usb_ask.map(|ask| ask.join().unwrap_or_else(|_| Err("the request panicked".into())));
            crate::usb::availability(&plan.info.capabilities, native)
        } else { Availability::Unknown },
        touch: outcome.sunshine_feature_flags & nightjar_core::control::negotiation::feature::PEN_TOUCH_EVENTS != 0
            && role != AppRole::RemoteMonitor,
        phase_lock: outcome.host_control_messages.contains(&rubylight_protocol::phase_lock::REPORT_MESSAGE_TYPE),
        early_decode: early_state(early, outcome.negotiated_video_format & VIDEO_FORMAT_MASK_PYROWAVE != 0,
            outcome.pyrowave_records_enabled) == EarlyState::On,
        early_without_records: early_state(early, outcome.negotiated_video_format & VIDEO_FORMAT_MASK_PYROWAVE != 0,
            outcome.pyrowave_records_enabled) == EarlyState::NoRecords,
        beam_race_off: beam.reason(),
        race_slices: slices,
        pace_mbps,
        beam_race: race_runs(slices, early_state(early, outcome.negotiated_video_format & VIDEO_FORMAT_MASK_PYROWAVE != 0,
            outcome.pyrowave_records_enabled)),
        color_notice: color_notice(&offer, outcome.negotiated_video_format),
        pyrowave_fallback: None,
        app_name: plan.app.name.clone(),
        host_name: plan.host_name.clone(),
    };
    let reported = plan.info.server_codec_mode_support;
    if let Some((log, notice)) = pyrowave_fallback(settings.codec, (scm, reported), offer.formats, outcome.negotiated_video_format) {
        eprintln!("{log}");
        // Shown once per PC (`pyrowave_notice`); the log says it on every stream.
        facts.pyrowave_fallback = Some(notice);
    }
    if facts.early_without_records {
        eprintln!("{EARLY_NO_RECORDS}");
    }
    if settings.prefer_yuv444 || settings.sdr_10bit {
        eprintln!("nightjar: 4:4:4 {:?}, 10-bit SDR {:?}, offered {:#x}, streaming {:#x}", offer.yuv444, offer.sdr10, offer.formats, facts.format);
    }
    // A pen's barrel roll goes only to a host with both the serverinfo element and the
    // DESCRIBE attribute; other Sunshine-family hosts may send the attribute alone.
    let pen_barrel_roll = plan.info.capabilities.has(Feature::PenBarrelRoll) && session.outcome().host_pen_barrel_roll;
    session.input(|q| q.set_pen_barrel_roll(pen_barrel_roll));
    Ok((session, facts))
}

/// Why a stream ended.
pub enum Exit {
    /// The user left (combo, controller combo, overlay button).
    User,
    /// The window was closed: leave Nightjar too.
    CloseWindow,
    Reconnect,
    Terminated(i32),
    /// The host ended it or the connection failed; Some carries the message.
    Ended(Option<String>),
    /// A lost network came back where the connection cannot carry on: reconnect at once.
    NetworkReturned,
}

impl Exit {
    /// Whether the stream reconnects on its own after this exit. Only once its video started
    /// (`video_started`, [`recovery::recovers_termination`]): a stream that never showed a picture
    /// failed to start, and its error shows at once instead of after a minute of retries.
    pub fn retryable(&self, video_started: bool) -> bool {
        match self {
            Self::NetworkReturned => video_started,
            Self::Terminated(code) => recovery::recovers_termination(*code, video_started),
            _ => false,
        }
    }
}

/// One-second statistics for the overlay.
#[derive(Default)]
struct Window {
    started: Option<Instant>,
    shown: u32,
    dropped_pacing: u32,
    late: u32,
    render_us: u64,
    hold_us: i64,
    latency_us: i64,
    latency_n: u32,
    /// The header's processing times (100 us units) and the frames that carried one, over this
    /// window and the one before it, filled in by `HostAverage` when the overlay is built.
    host_latency: u64,
    host_latency_n: u32,
    network_us: u64,
    network_n: u32,
    timing_host_us: u64,
    timing_encode_us: u64,
    timing_n: u32,
}

/// Artemis and Moonlight average the header's processing time over the frames received in the last
/// two statistics windows, whether or not they were shown. This keeps the per-window deltas of the
/// receive-side counters and sums the previous window with the current one.
#[derive(Default)]
struct HostAverage {
    seen: (u64, u64),
    previous: (u64, u64),
}

impl HostAverage {
    /// `total` and `frames` are the running counters; returns the two-window sum.
    fn update(&mut self, total: u64, frames: u64) -> (u64, u32) {
        let current = (total.saturating_sub(self.seen.0), frames.saturating_sub(self.seen.1));
        self.seen = (total, frames);
        let sum = (self.previous.0 + current.0, self.previous.1 + current.1);
        self.previous = current;
        (sum.0, u32::try_from(sum.1).unwrap_or(u32::MAX))
    }
}

/// The overlay's host lines. "Host processing" is the average of the video frame header's per-frame
/// processing time, the same number Artemis and Moonlight show. It must stay that: users compare it
/// with those clients. The host timing reports' present to last packet sent includes the network
/// send and reads several times higher, so it only ever goes under its own "Host to network" label,
/// and a report never replaces or feeds the "Host processing" value.
fn host_lines(w: &Window) -> Vec<(String, String)> {
    let avg = |sum: f64, n: u32| if n > 0 { sum / f64::from(n) } else { 0.0 };
    let mut lines = Vec::new();
    if w.host_latency_n > 0 {
        lines.push((
            "Host processing".into(),
            format!("{:.1} ms", avg(w.host_latency as f64, w.host_latency_n) / 10.0),
        ));
    }
    if w.timing_n > 0 {
        lines.push((
            "Host to network".into(),
            format!(
                "{:.1} ms present to send, {:.1} ms encode",
                avg(w.timing_host_us as f64, w.timing_n) / 1000.0,
                avg(w.timing_encode_us as f64, w.timing_n) / 1000.0
            ),
        ));
    }
    lines
}

/// What the hardware poll makes of the host's answer: a host that does not offer the readings
/// (not advertised, another schema version, 404, 403 or 409) is not asked again this stream.
fn hardware_answer(result: &Result<HardwareMetrics, HostApiError>) -> HardwareAnswer {
    use nightjar_core::http::Error;
    match result {
        Ok(m) => HardwareAnswer::Sample { sampled_at_unix_ms: m.sampled_at_unix_ms, interval_ms: m.interval_ms },
        Err(HostApiError::Unavailable { .. } | HostApiError::UnsupportedVersion) => HardwareAnswer::Unavailable,
        Err(HostApiError::Host(Error::NotFound(_) | Error::Status { code: 403 | 404 | 409, .. } | Error::Api { status: 403 | 404 | 409, .. })) => HardwareAnswer::Unavailable,
        Err(_) => HardwareAnswer::Failed,
    }
}

/// The overlay's text, all three modes kept up to date so a mode change shows at once.
#[derive(Default, Clone)]
pub struct Overlay {
    /// Debug: every line.
    pub lines: Vec<(String, String)>,
    pub statistics: Vec<(String, String)>,
    pub compact: overlay::Compact,
    /// The host's lowered-bitrate notice, under the Compact line.
    pub notice: Option<String>,
}

impl Overlay {
    /// The lines `mode` shows; Compact has its own line instead.
    pub fn lines(&self, mode: OverlayMode) -> &[(String, String)] {
        match mode {
            OverlayMode::Debug => &self.lines,
            OverlayMode::Statistics => &self.statistics,
            OverlayMode::Compact => &[],
        }
    }
}

/// Unwraps 90 kHz RTP timestamps into host microseconds.
#[derive(Default)]
struct RtpClock {
    last: Option<(u32, i64)>,
}

impl RtpClock {
    fn host_us(&mut self, rtp: u32) -> i64 {
        let ticks = match self.last {
            Some((last, ticks)) => ticks + i64::from(rtp.wrapping_sub(last) as i32),
            None => i64::from(rtp),
        };
        self.last = Some((rtp, ticks));
        ticks * 100 / 9
    }
}

/// What the raced presenter guesses of a stream's slice times before it has seen a frame.
fn seeded_predictor(facts: &Facts) -> race::Predictor {
    let frame_bytes = f64::from(facts.bitrate_kbps.max(1)) * 1000.0 / 8.0 / f64::from(facts.fps.max(1));
    race::Predictor::seeded(facts.race_slices, facts.height.max(1) as u32, frame_bytes, facts.pace_mbps)
}

/// A present of the texture: when the call was issued and returned (us), and the beam around it.
struct Flipped {
    issued_us: u64,
    returned_us: u64,
    scan: Option<race_probe::ScanReport>,
}

pub struct Streaming {
    pub session: Session,
    pub shared: Arc<Shared>,
    pub facts: Facts,
    pub pacer: Pacer,
    pub texture: VideoTexture,
    upscaler: Option<upscale::Upscaler>,
    /// AI enhancement (`config::Upscaling::Ai`) for the current picture shape and output size.
    #[cfg(windows)]
    enhancer: Option<enhance::Enhancer>,
    upscale_failed: bool,
    phase: phase::Phase,
    /// Whether racing the display's beam can work here, once the beam race is on and a picture is
    /// shown; on a VRR stream that tears, whether the display refreshes on present (`sync_vrr`).
    race_probe: Option<race_probe::RaceProbe>,
    /// The display check for VRR: the stream presents with tearing, and a display found to scan
    /// at its own rate gets the pacing it would have without VRR (off under `NIGHTJAR_VRR_CHECK=0`,
    /// for measurement runs); and `NIGHTJAR_VRR_SYNC` on a VRR stream.
    vrr: present::VrrPacing,
    /// Presents `VrrSync::Scan` held for a scan under way, and how long in all (ns).
    scan_waits: (u32, u64),
    /// The picture on the display's beam, while racing it is possible (see `sync_race`).
    race_scan: Option<race::Scan>,
    /// The frame in flight under the raced presenter, and what it plans from.
    racer: race::Racer<gpu_planes::Pin>,
    predictor: Option<race::Predictor>,
    latency: race::Latency,
    /// The user's margin ahead of the beam in ms, 0 for the automatic one.
    race_margin_ms: u32,
    /// False under `NIGHTJAR_BEAM_RACE_PRESENT=0`: the display is probed and the picture's age at the
    /// middle row is recorded, but whole pictures are presented (the A side of an A/B run).
    race_present: bool,
    /// The stream's setup as the last frame showed it, which slice draws need before a frame has.
    race_setup: Option<session::VideoSetup>,
    pub overlay_visible: bool,
    pub overlay_mode: OverlayMode,
    pub overlay_corner: OverlayCorner,
    /// The overlay as last kept in the settings; see `take_overlay_change`.
    overlay_kept: (bool, OverlayMode, OverlayCorner),
    pub overlay: Overlay,
    /// When to ask the host for its hardware readings, and the last sample while it may show.
    hardware_poll: HardwarePoll,
    hardware: Option<HardwareMetrics>,
    /// What the host last said about holding the bitrate down for the network; `None` while it has not.
    link_notice: Option<LinkNotice>,
    /// The host's latest pipeline frame rates; `None` until it sends them.
    pipeline_rates: Option<PipelineRatesReport>,
    pub exit: Option<Exit>,
    pub toast: Option<(String, Instant)>,
    /// The route toward the PC is gone: the lost-network notice stays until one returns.
    pub network_lost: bool,
    /// The next automatic reconnect starts without its delay (a network came back).
    retry_now: bool,
    pub host_id: u64,
    settings: Settings,
    plan: LaunchPlan,
    microphone: Option<microphone::Microphone>,
    /// USB forwarding, while the setting is on and the PC forwards this device's.
    pub usb: Option<crate::usb::Panel>,
    /// Why the setting is on but the stream controls offer no USB devices.
    pub usb_refused: Option<&'static str>,
    cursor: cursor::Cursor,
    cursor_warning: bool,
    text_field: text_field::TextField,
    /// The keyboard layout last reported to the host.
    keyboard_layout: Option<rubylight_protocol::keyboard_layout::KeyboardLayout>,
    clipboard_busy: bool,
    /// Clipboard sync, while the PC offered it.
    clipboard_sync: Option<clipboard_sync::Sync<clipboard_sync::Sdl>>,
    command_busy: bool,
    last_command: Option<Instant>,
    audio_status: ([u8; 16], Instant),
    controller_mouse_active: bool,
    pub pending_command: Option<usize>,
    pub bitrate_busy: bool,
    pub bitrate_kbps: u32,
    pub adaptive_bitrate: bool,
    bitrate_policy: Option<bitrate::Bitrate>,
    /// A Foundation PC's own bitrate controller, on in place of `bitrate_policy`.
    host_abr: Option<host_abr::HostAbr>,
    /// The request that turns it on, while the stream may end before the PC answers.
    host_abr_start: Option<host_abr::AbrStart>,
    /// The stream doctor: its report in the stream menu and after the stream, and the bitrate trial.
    doctor: doctor::Doctor,
    /// Rate changes over 0x553C, on a stream that takes them.
    pub rate_change: Option<rate_change::RateChange>,
    pub quality_mode: QualityMode,
    quality_policy: Option<quality::Quality>,
    quality_ceiling: Reconfigure,
    /// The decoder sampling window: its start on the session clock and the requested mode it counts.
    decode_window_us: u64,
    decode_generation: i64,
    pub requested_mode: Reconfigure,
    pending_mode: Option<(Reconfigure, Instant)>,
    display_change: Option<Instant>,
    last_arrival: Option<(u32, u64)>,
    jitter_ms: f64,
    focused: bool,
    pub microphone_muted: bool,
    pub export_statistics: bool,
    statistics: VecDeque<String>,
    pub menu_visible: bool,
    /// The widget that had keyboard focus in the controls last frame, to scroll to when it moves.
    pub menu_focus: Option<egui::Id>,
    /// The open "Session and graphics" page; its worker runs only while this is Some.
    pub session_page: Option<crate::session_page::Page>,
    session_link: String,
    /// The menu asked for the page; the main loop opens it (it owns the waker and the network).
    pub session_page_requested: bool,
    keys_down: HashSet<u16>,
    mouse_down: HashSet<u8>,
    touch: touch::Touch,
    touchpad: touchpad::Touchpad,
    captured: bool,
    absolute: bool,
    relative_remainder: (f32, f32),
    offset: ClockOffset,
    /// The client timing CSV (`NIGHTJAR_TIMING_CSV`), off by default.
    timing_csv: Option<crate::timing_csv::TimingCsv>,
    rtp_clock: RtpClock,
    last_probe: Instant,
    window: Window,
    last_counters: (u64, u64, u64, u64, u64, u64, u64),
    last_video_bytes: u64,
    last_wire_bytes: u64,
    host_average: HostAverage,
    /// Early frames and fallbacks at the last overlay update.
    last_early: (u64, u64),
    last_video: (u64, u64),
    started: Instant,
    first_frame: bool,
    /// Pictures shown at least once since the last overlay redraw.
    pub needs_redraw: bool,
    timings: VecDeque<(u32, u64)>,
}

/// What the main loop should do next.
pub enum Wait {
    /// Present now.
    Now,
    /// Wait for events up to this long.
    For(Duration),
}

/// Why the stream ends after SDL's render device event `kind`: the stream's textures and the
/// decoder's GPU planes are gone with the device, and the picture would freeze without a word.
fn render_device_ended(kind: SDL_EventType) -> Option<&'static str> {
    match kind {
        SDL_EVENT_RENDER_DEVICE_RESET => Some("The graphics driver was reset, which ended the stream. Connect again."),
        SDL_EVENT_RENDER_DEVICE_LOST => Some("The graphics device was lost (the driver stopped or the GPU was removed), which ended the stream. Restart Nightjar."),
        _ => None,
    }
}

/// [`Streaming::sends_controllers`] for `settings` in a session of `role`.
fn sends_controllers(settings: &Settings, role: AppRole) -> bool {
    settings.controllers && role != AppRole::RemoteMonitor
}

impl Streaming {
    pub fn new(connecting: Connecting, session: Session, facts: Facts, display: Display) -> Self {
        let min_interval = if display.refresh_hz > 0.0 {
            (1_000_000.0 / display.refresh_hz) as i64
        } else {
            0
        };
        crate::sdl::alt_f4_to_host(true);
        if facts.host_timing {
            let _ = session.send(ClientMessage::HostTimingRequest(HostTimingRequest { enabled: true }));
        }
        if facts.link_notice {
            let _ = session.send(ClientMessage::LinkNoticeRequest(LinkNoticeRequest { enabled: true }));
        }
        if facts.pipeline_rates {
            // Every second, as the overlay refreshes.
            let _ = session.send(ClientMessage::PipelineRatesRequest(PipelineRatesRequest { enabled: true, interval_ms: 1000 }));
        }
        if facts.text_input {
            if let Err(error) = session.send(ClientMessage::TextInputRequest(text_field::request())) {
                eprintln!("nightjar: text field: could not ask for notices: {error}");
            }
        }
        let settings = connecting.settings.clone();
        let microphone = if settings.microphone && !connecting.plan.microphone_muted && facts.role != AppRole::RemoteMonitor {
            if session.microphone_available() {
                match microphone::Microphone::open(connecting.shared.clone(), settings.mic_echo_cancel) {
                    Ok(microphone) => Some(microphone),
                    Err(error) => {
                        connecting.shared.notify(Notice::MicrophoneFailed(error));
                        None
                    }
                }
            } else if !settings.controller_audio {
                connecting.shared.notify(Notice::MicrophoneFailed("The PC did not offer an encrypted microphone stream.".into()));
                None
            } else { None }
        } else { None };
        let bitrate_kbps = facts.bitrate_kbps.max(1) as u32;
        let mode = Reconfigure { width: facts.width as u16, height: facts.height as u16, fps_millihz: facts.fps as u32 * 1000 };
        let mut quality_mode = if facts.reconfigure { settings.adaptive_quality } else { QualityMode::Off };
        let mut quality_failed = false;
        let quality_policy = match quality_mode {
            QualityMode::Off => None,
            _ => match quality::Quality::new(quality_mode, mode, settings.quality_min_short_edge, settings.quality_min_fps, 0) {
                Ok(policy) => Some(policy),
                Err(error) => {
                    eprintln!("nightjar: automatic quality off for {}x{}@{}: {error}", facts.width, facts.height, facts.fps);
                    quality_mode = QualityMode::Off;
                    quality_failed = true;
                    None
                }
            },
        };
        let rate_change = facts.rate_change.then(|| rate_change::RateChange::new(
            bitrate_kbps, facts.fps.max(1) as u32 * 1000, settings.client_hint_bitrate, connecting.plan.bitrate_limit_kbps, 0));
        let backlog = connecting.shared.pacer_backlog.clone();
        // The stream controls read the attachments each time they are drawn.
        let usb_refused = if settings.usb_forwarding { crate::usb::refusal(facts.usb) } else { None };
        if let Some(why) = usb_refused { eprintln!("nightjar: USB forwarding is on, but no USB devices are offered: {why}"); }
        let usb = if settings.usb_forwarding && usb_refused.is_none() { lock(&connecting.shared.host_client).clone() } else { None }
            .map(|host| crate::usb::Panel::new(session.usb_forwarder(host, || {})));
        let hdr_stream = present::hdr_negotiated(settings.hdr, facts.format);
        let vrr_fallback = present::without_vrr(&settings);
        let stream_fps = u32::try_from(facts.fps).unwrap_or(0);
        if settings.frame_pacing != config::FramePacing::Latency {
            eprintln!("nightjar: frame pacing {:?} without VRR at {stream_fps} fps", settings.frame_pacing);
        }
        let vrr_check = checks_display_for_vrr(connecting.plan.vrr, connecting.pacing, vrr_fallback, env_flag("NIGHTJAR_VRR_CHECK"));
        let vrr_sync = if connecting.plan.vrr { present::VrrSync::parse(std::env::var("NIGHTJAR_VRR_SYNC").ok().as_deref()) } else { present::VrrSync::Off };
        if vrr_sync != present::VrrSync::Off { eprintln!("nightjar: VRR sync {} (NIGHTJAR_VRR_SYNC)", vrr_sync.label()); }
        if vrr_sync == present::VrrSync::Scan && !vrr_check {
            eprintln!("nightjar: VRR sync scan reads the scan line; the display check's verdict changes no pacing on this stream");
        }
        let mut doctor = doctor::Doctor::default();
        doctor.receive_buffer = session.video_receive_buffer();
        let mut streaming = Self {
            usb,
            usb_refused,
            session,
            shared: connecting.shared,
            facts,
            pacer: Pacer::with_backlog(connecting.pacing, min_interval, backlog).with_frame_pacing(settings.frame_pacing, stream_fps),
            texture: VideoTexture::new(connecting.settings.upscaling == config::Upscaling::Bilinear, hdr_stream),
            upscaler: None,
            #[cfg(windows)]
            enhancer: None,
            upscale_failed: false,
            phase: phase::Phase::default(),
            race_probe: None,
            // The sync interval as `App::begin_stream` set it.
            vrr: present::VrrPacing::new(vrr_check, vrr_fallback, vrr_sync, connecting.pacing),
            scan_waits: (0, 0),
            race_scan: None,
            racer: race::Racer::default(),
            predictor: None,
            latency: race::Latency::default(),
            race_margin_ms: hint_override("NIGHTJAR_BEAM_RACE_MARGIN_MS", settings.beam_race_margin_ms),
            race_present: env_flag("NIGHTJAR_BEAM_RACE_PRESENT").unwrap_or(true),
            race_setup: None,
            overlay_visible: settings.overlay,
            overlay_mode: settings.overlay_mode,
            overlay_corner: settings.overlay_position,
            overlay_kept: (settings.overlay, settings.overlay_mode, settings.overlay_position),
            overlay: Overlay::default(),
            hardware_poll: HardwarePoll::default(),
            hardware: None,
            link_notice: None,
            pipeline_rates: None,
            exit: None,
            toast: None,
            network_lost: false,
            retry_now: false,
            host_id: connecting.host_id,
            absolute: settings.mouse_mode == MouseMode::Absolute,
            settings,
            microphone,
            cursor: cursor::Cursor::default(),
            cursor_warning: false,
            text_field: text_field::TextField::default(),
            keyboard_layout: None,
            clipboard_busy: false,
            clipboard_sync: None,
            command_busy: false,
            last_command: None,
            audio_status: ([0; 16], Instant::now() - Duration::from_secs(1)),
            controller_mouse_active: false,
            pending_command: None,
            bitrate_busy: false,
            bitrate_kbps,
            adaptive_bitrate: false,
            bitrate_policy: None,
            host_abr: None,
            host_abr_start: None,
            doctor,
            rate_change,
            quality_mode,
            quality_policy,
            quality_ceiling: mode,
            decode_window_us: session::now_us(),
            decode_generation: 0,
            requested_mode: mode,
            pending_mode: None,
            display_change: None,
            last_arrival: None,
            jitter_ms: -1.0,
            focused: false,
            microphone_muted: connecting.plan.microphone_muted,
            plan: connecting.plan,
            export_statistics: false,
            statistics: VecDeque::new(),
            menu_visible: false,
            menu_focus: None,
            session_page: None,
            session_link: String::new(),
            session_page_requested: false,
            keys_down: HashSet::new(),
            mouse_down: HashSet::new(),
            touch: touch::Touch::default(),
            touchpad: touchpad::Touchpad::default(),
            captured: false,
            relative_remainder: (0.0, 0.0),
            offset: ClockOffset::default(),
            timing_csv: open_timing_csv(),
            rtp_clock: RtpClock::default(),
            last_probe: Instant::now(),
            window: Window::default(),
            last_counters: (0, 0, 0, 0, 0, 0, 0),
            last_video_bytes: 0,
            last_wire_bytes: 0,
            host_average: HostAverage::default(),
            last_early: (0, 0),
            last_video: (0, 0),
            started: Instant::now(),
            first_frame: false,
            needs_redraw: true,
            timings: VecDeque::new(),
        };
        streaming.report_keyboard_layout();
        streaming.start_clipboard_sync();
        if quality_failed {
            streaming.show_toast(tr("Automatic quality stopped: the stream reported no size or frame rate."));
        }
        streaming
    }

    /// Sends the keyboard layout to a host that takes it, at the start and when it changed.
    fn report_keyboard_layout(&mut self) {
        if !self.facts.keyboard_layout {
            return;
        }
        let Some(layout) = text_field::current_layout() else {
            if self.keyboard_layout.is_none() {
                eprintln!("nightjar: keyboard layout unknown, not reported");
            }
            return;
        };
        if self.keyboard_layout.as_ref() == Some(&layout) {
            return;
        }
        match self.session.send(ClientMessage::KeyboardLayout(layout.clone())) {
            Ok(()) => {
                eprintln!("nightjar: keyboard layout {:?} {}", layout.source, layout.tag);
                self.keyboard_layout = Some(layout);
            }
            Err(error) => eprintln!("nightjar: could not report the keyboard layout {}: {error}", layout.tag),
        }
    }

    /// Starts clipboard sync with a PC that offered it, and says so when a Rubylight PC did not.
    fn start_clipboard_sync(&mut self) {
        if !self.settings.clipboard_sync || self.facts.role == AppRole::RemoteMonitor {
            return;
        }
        if !self.facts.clipboard_sync {
            eprintln!("nightjar: clipboard sync: the PC did not offer it");
            if self.plan.info.capabilities.family() == nightjar_core::http::Family::Rubylight {
                self.show_toast(tr("Clipboard sync is off: the PC did not offer it. It must allow this device to read and write its clipboard."));
            }
            return;
        }
        let (sync, hello) = clipboard_sync::Sync::start(clipboard_sync::Sdl);
        match self.session.send(hello) {
            Ok(()) => {
                eprintln!("nightjar: clipboard sync on");
                self.clipboard_sync = Some(sync);
            }
            Err(error) => {
                eprintln!("nightjar: clipboard sync: could not start: {error}");
                self.show_toast(trf("Clipboard sync could not start: {}", &[&error]));
            }
        }
    }

    fn clipboard_sync_step(&mut self, step: clipboard_sync::Step) {
        if let Some(message) = step.send {
            if let Err(error) = self.session.send(message) {
                eprintln!("nightjar: clipboard sync: could not send a copy: {error}");
            }
        }
        if let Some(toast) = step.toast {
            self.show_toast(toast);
        }
    }

    /// Follows the host's focused text field with SDL text input, once per frame.
    pub fn follow_text_field(&mut self, window: *mut SDL_Window) {
        self.text_field.apply(window, self.menu_visible);
    }

    pub fn show_toast(&mut self, text: impl Into<String>) {
        self.toast = Some((text.into(), Instant::now()));
        self.needs_redraw = true;
    }

    /// The route toward the PC went away: a frozen picture is never silent. The notice stays
    /// until a network returns; `away`: the window is minimized or hidden.
    fn network_lost(&mut self, away: bool) {
        eprintln!("nightjar: the stream's network was lost");
        self.network_lost = true;
        self.show_toast(tr(lost_notice(&self.settings, self.reconnects_on_its_own(away))));
    }

    /// A network is back after the route was gone; `change`: what it means for the session
    /// ([`reconnects_on_return`]).
    fn network_returned(&mut self, change: NetworkChange, metered: bool, away: bool) {
        if !std::mem::take(&mut self.network_lost) { return; }
        if self.toast.as_ref().is_some_and(|(text, _)| *text == tr(NETWORK_LOST) || *text == tr(NETWORK_LOST_RECONNECTING)) {
            self.toast = None;
            self.needs_redraw = true;
        }
        if change != NetworkChange::CannotContinue || self.exit.is_some() { return; }
        if !reconnects_on_return(change, metered, &self.settings, self.reconnects_on_its_own(away)) {
            eprintln!("nightjar: the network is back on another address the stream did not roam to; \
                its connection times out");
            return;
        }
        eprintln!("nightjar: the network is back on another address the stream did not roam to: reconnecting now");
        self.retry_now = true;
        self.exit = Some(Exit::NetworkReturned);
    }

    /// Whether a dropped stream would reconnect on its own now: its video started ([`Exit::retryable`])
    /// and [`automatic_rejoin`] has a plan with an attempt left in its budget. Spends nothing.
    fn reconnects_on_its_own(&self, away: bool) -> bool {
        self.first_frame && automatic_rejoin(&self.plan, self.facts.role, self.settings.auto_reconnect, away, self.connected_ms(), false)
            .and_then(|plan| plan.recovery)
            .is_some_and(|budget| budget.allows_attempt(recovery::now_ms()))
    }

    fn connected_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// The plan that starts the stream again; `automatic` after a drop, `away` while the window is
    /// minimized or hidden.
    pub fn reconnect_plan(&self, automatic: bool, away: bool) -> Option<LaunchPlan> {
        let mut plan = if automatic {
            automatic_rejoin(&self.plan, self.facts.role, self.settings.auto_reconnect, away, self.connected_ms(), self.retry_now)?
        } else {
            let mut plan = rejoin(self.plan.clone(), self.facts.role)?;
            plan.recovery = None;
            plan
        };
        plan.microphone_muted = self.microphone_muted;
        Some(plan)
    }

    pub fn display_changed(&mut self, renderer: *mut SDL_Renderer) {
        self.phase.reset();
        self.display_change = Some(Instant::now());
        if self.facts.display_caps && self.facts.role != AppRole::InputOnly {
            let _ = self.session.send(ClientMessage::DisplayCaps(crate::sdl::display_caps(renderer)));
        }
    }

    pub fn set_quality(&mut self, mode: QualityMode) {
        self.quality_mode = if self.facts.reconfigure { mode } else { QualityMode::Off };
        self.quality_policy = None;
        if self.quality_mode != QualityMode::Off {
            self.doctor.trial.clear();
            if !self.request_mode(self.quality_ceiling) { self.quality_mode = QualityMode::Off; return; }
            match quality::Quality::new(mode, self.quality_ceiling, self.settings.quality_min_short_edge,
                self.settings.quality_min_fps, self.started.elapsed().as_millis() as u64) {
                Ok(policy) => self.quality_policy = Some(policy),
                Err(error) => {
                    eprintln!("nightjar: automatic quality: {error}");
                    self.quality_mode = QualityMode::Off;
                    self.show_toast(tr("Automatic quality stopped: the stream reported no size or frame rate."));
                    return;
                }
            }
            if !self.adaptive_bitrate && !self.bitrate_busy { self.enable_adaptive_bitrate(true); }
        }
    }

    pub fn set_mode(&mut self, request: Reconfigure) {
        if self.request_mode(request) {
            self.quality_ceiling = request;
            self.settings.width = u32::from(request.width);
            self.settings.height = u32::from(request.height);
            self.settings.fps = request.fps_millihz.div_ceil(1000);
            self.plan.settings = self.settings.clone();
        }
    }

    fn request_mode(&mut self, request: Reconfigure) -> bool {
        if !self.facts.reconfigure || Reconfigure::decode(&request.encode()).is_none() {
            self.show_toast(tr("This stream cannot use the requested size or frame rate."));
            return false;
        }
        if let Err(error) = self.session.send(ClientMessage::Reconfigure(request)) {
            self.show_toast(trf("Could not change the stream: {}", &[&error]));
            return false;
        }
        self.requested_mode = request;
        self.pending_mode = Some((request, Instant::now()));
        self.facts.fps = request.fps_millihz.div_ceil(1000) as i32;
        self.shared.fps_millihz.store(request.fps_millihz, Ordering::Relaxed);
        *lock(&self.shared.decode_window) = pipeline::DecodeWindow::default();
        self.decode_window_us = session::now_us();
        self.decode_generation += 1;
        if let Some(rate) = &mut self.bitrate_policy {
            rate.set_mode(u32::from(request.width), u32::from(request.height), request.fps_millihz.div_ceil(1000));
        }
        if let Some(rates) = &mut self.rate_change { rates.fps_set(request.fps_millihz); }
        true
    }

    pub fn follow_display(&mut self, display: Display) {
        self.pacer.clock.settings.min_interval_us = (1_000_000.0 / display.refresh_hz.max(1.0)) as i64;
        self.plan.display = display;
        if !self.facts.reconfigure || !self.display_change.is_some_and(|at| at.elapsed() >= Duration::from_millis(300)) { return; }
        self.display_change = None;
        let (width, height) = if self.settings.width == 0 || self.settings.height == 0 {
            ((display.width.clamp(256, 8192) + 1) & !1, (display.height.clamp(256, 8192) + 1) & !1)
        } else { (u32::from(self.quality_ceiling.width), u32::from(self.quality_ceiling.height)) };
        let fps = if self.settings.fps == 0 { (display.refresh_hz * 1000.0).round() as u32 } else { self.quality_ceiling.fps_millihz };
        let Some(request) = quality::target(width, height, fps) else { return; };
        if request == self.quality_ceiling { return; }
        self.quality_ceiling = request;
        if self.quality_mode == QualityMode::Off { self.request_mode(request); }
        else if let Some(policy) = &mut self.quality_policy {
            let started = match policy.rebase(request, self.started.elapsed().as_millis() as u64) {
                Ok(Some(request)) => self.request_mode(request),
                other => {
                    eprintln!("nightjar: automatic quality: no request for {request:?}: {other:?}");
                    self.show_toast(tr("This stream cannot use the requested size or frame rate."));
                    false
                }
            };
            if !started {
                self.quality_policy = None;
                self.quality_mode = QualityMode::Off;
            }
        }
    }

    pub fn microphone_available(&self) -> bool {
        self.settings.microphone && (self.session.microphone_available() || self.controller_audio_available()) && self.facts.role != AppRole::RemoteMonitor
    }

    pub fn video_scaling(&self) -> (config::VideoScaling, bool) { (self.settings.video_scaling, self.settings.align_video_top) }

    pub fn set_video_scaling(&mut self, mode: config::VideoScaling, top: bool) {
        self.settings.video_scaling = mode;
        self.settings.align_video_top = top;
        self.session.input(|q| self.touch.release(q));
        self.needs_redraw = true;
    }

    pub fn controller_audio_available(&self) -> bool {
        self.sends_controllers() && self.settings.controller_audio
    }

    /// Whether this session tells the host about controllers: not with Controllers off, and never
    /// from a view-only remote monitor.
    fn sends_controllers(&self) -> bool {
        sends_controllers(&self.settings, self.facts.role)
    }

    pub fn mute_microphone(&mut self, muted: bool, pads: &mut Gamepads) {
        self.microphone_muted = muted;
        self.microphone = None;
        if !muted {
            for audio in pads.pads.iter_mut().filter_map(|pad| pad.audio.as_mut()) { audio.host_muted = false; }
        }
        self.route_microphones(pads);
    }

    fn route_microphones(&mut self, pads: &mut Gamepads) {
        let permitted = self.settings.microphone && self.facts.role != AppRole::RemoteMonitor;
        let mut controller_route = false;
        for pad in &mut pads.pads {
            let Some(audio) = &mut pad.audio else { continue; };
            if audio.host_muted { self.microphone_muted = true; }
            match audio.capture(permitted, self.microphone_muted, &self.shared) {
                Ok(active) => controller_route |= active,
                Err(error) => {
                    self.microphone_muted = true;
                    self.show_toast(error);
                }
            }
        }
        if self.microphone_muted {
            for audio in pads.pads.iter_mut().filter_map(|pad| pad.audio.as_mut()) {
                let _ = audio.capture(permitted, true, &self.shared);
            }
        }
        if !permitted || self.microphone_muted || controller_route {
            self.microphone = None;
        } else if self.microphone.is_none() && self.session.microphone_available() {
            match microphone::Microphone::open(self.shared.clone(), self.settings.mic_echo_cancel) {
                Ok(microphone) => self.microphone = Some(microphone),
                Err(error) => { self.microphone_muted = true; self.show_toast(error); }
            }
        }
        for pad in &mut pads.pads {
            let Some(audio) = &mut pad.audio else { continue; };
            while let Some(packet) = audio.microphone_packet(u16::from(pad.index)) {
                if let Err(error) = self.session.send(ClientMessage::ControllerMicrophone(packet)) {
                    self.shared.notify(Notice::MicrophoneFailed(format!("Controller microphone forwarding stopped: {error}")));
                    break;
                }
            }
        }
        pads.update_audio();
    }

    pub fn save_statistics(&mut self, directory: &std::path::Path) {
        self.export_statistics = false;
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
        let path = directory.join(format!("nightjar-{stamp}.csv"));
        let mut csv = String::from("elapsed_ms,received_fps,decoded_fps,shown_fps,loss_percent,rtt_ms,decode_ms,render_ms,host_to_network_ms,latency_ms,bitrate_kbps\n");
        for row in &self.statistics { csv.push_str(row); }
        match std::fs::create_dir_all(directory).and_then(|_| std::fs::write(&path, csv)) {
            Ok(()) => self.show_toast(trf("Statistics saved to {}", &[&path.display()])),
            Err(error) => self.show_toast(trf("Could not save statistics: {}", &[&error])),
        }
    }

    /// Mouse capture: relative motion with a hidden, confined pointer.
    pub fn set_capture(&mut self, window: *mut SDL_Window, on: bool) {
        self.focused = unsafe { SDL_GetWindowFlags(window) & SDL_WINDOW_INPUT_FOCUS != 0 };
        let on = on && self.facts.role != AppRole::RemoteMonitor;
        self.captured = on;
        let cursor_shapes = on && self.absolute && self.settings.cursor_shapes && self.facts.cursor_shapes;
        if cursor_shapes != self.cursor.enabled
            && self.session.send(ClientMessage::CursorShapeRequest { enabled: cursor_shapes }).is_ok() {
            self.cursor.clear();
            self.cursor.enabled = cursor_shapes;
        }
        // SAFETY: a live window.
        unsafe {
            if self.absolute {
                SDL_SetWindowRelativeMouseMode(window, false);
                if on && !self.cursor.enabled {
                    SDL_HideCursor();
                } else {
                    SDL_ShowCursor();
                }
            } else {
                SDL_SetWindowRelativeMouseMode(window, on);
            }
            let grab = on && self.settings.capture_system_keys;
            SDL_SetWindowKeyboardGrab(window, grab);
        }
    }

    pub fn raise_all_keys(&mut self) {
        let keys: Vec<u16> = self.keys_down.drain().collect();
        let buttons: Vec<u8> = self.mouse_down.drain().collect();
        self.session.input(|q| {
            self.touch.release(q);
            for key in keys {
                let _ = q.send_keyboard(0x8000 | key, false, 0, 0);
            }
            for button in buttons {
                let _ = q.send_mouse_button(button, false);
            }
        });
    }

    /// Handles queued notices from the session threads. Control events that need the
    /// controllers are applied to `pads`.
    /// `away`: the window is minimized or hidden.
    pub fn drain_notices(&mut self, pads: &mut Gamepads, away: bool) {
        let notices = std::mem::take(&mut *lock(&self.shared.notices));
        for notice in notices {
            match notice {
                Notice::Terminated(code) => {
                    self.exit.get_or_insert(Exit::Terminated(code));
                }
                Notice::DecoderFallback(text) => self.show_toast(text),
                Notice::PassthroughOpus => self.show_toast(tr("The PC did not offer PCM audio. This stream plays Opus.")),
                Notice::DecoderFailed(text) => {
                    self.exit.get_or_insert(Exit::Ended(Some(text)));
                }
                Notice::MicrophoneFailed(text) => {
                    self.microphone = None;
                    self.microphone_muted = true;
                    self.show_toast(text);
                }
                Notice::Clipboard(result) => {
                    self.clipboard_busy = false;
                    match result {
                        Ok(Some(text)) => {
                            let text = std::ffi::CString::new(text);
                            if text.as_ref().is_ok_and(|text| unsafe { SDL_SetClipboardText(text.as_ptr()) }) {
                                self.show_toast(tr("PC clipboard copied to this device."));
                            } else {
                                self.show_toast(tr("Could not copy the PC clipboard to this device."));
                            }
                        }
                        Ok(None) => self.show_toast(tr("Clipboard sent to the PC.")),
                        Err(error) => self.show_toast(error),
                    }
                }
                Notice::ClipboardImage(result) => {
                    self.clipboard_busy = false;
                    match result {
                        Ok(Some(image)) => match crate::clipboard::write(image) {
                            Ok(()) => self.show_toast(tr("PC image copied to this device.")),
                            Err(error) => self.show_toast(trf("Could not put the PC's image on this device's clipboard: {}", &[&error])),
                        },
                        Ok(None) => self.show_toast(tr("Image sent to the PC.")),
                        Err(error) => self.show_toast(error),
                    }
                }
                Notice::Control(event) => self.control_event(event, pads),
                Notice::ServerCommand(result) => {
                    self.command_busy = false;
                    match result {
                        Ok(payload) => match self.session.send(ClientMessage::ServerCommand(payload)) {
                            Ok(()) => self.show_toast(tr("Command sent to the PC.")),
                            Err(error) => self.show_toast(trf("Could not send the command: {}", &[&error])),
                        },
                        Err(error) => self.show_toast(error),
                    }
                }
                Notice::HostMenu(result) => {
                    if let Err(error) = result {
                        self.show_toast(error);
                    }
                }
                Notice::NetworkChanged(hint, after_loss) => {
                    // A session without roaming (off, or a host that does not roam) never roams,
                    // but still says whether its connection carries on.
                    let change = self.session.network_changed(hint);
                    if after_loss { self.network_returned(change, hint.metered, away); }
                }
                Notice::NetworkLost => self.network_lost(away),
                Notice::AdaptiveBitrate(result) => {
                    self.bitrate_busy = false;
                    match result {
                        Ok((ceiling, current)) => {
                            self.facts.bitrate_kbps = current as i32;
                            self.bitrate_kbps = current;
                            if let Some(rates) = &mut self.rate_change { rates.bitrate_set(current, self.started.elapsed().as_millis() as u64); }
                            self.bitrate_policy = Some(bitrate::Bitrate::new(self.facts.format, ceiling, current, self.facts.fps.max(1) as u32,
                                (u32::from(self.requested_mode.width), u32::from(self.requested_mode.height)),
                                self.started.elapsed().as_millis() as u64));
                        }
                        Err(error) => {
                            self.adaptive_bitrate = false;
                            self.show_toast(error);
                        }
                    }
                }
                Notice::HostAbr(result) => {
                    self.bitrate_busy = false;
                    match result {
                        Ok(Some(configured)) => {
                            let abr = host_abr::HostAbr::new(&configured, self.facts.bitrate_kbps.max(0) as u32,
                                self.started.elapsed().as_millis() as u64);
                            eprintln!("nightjar: the PC's automatic bitrate is on: {} kbps, at most {} kbps",
                                abr.current_kbps, configured.max_bitrate);
                            self.facts.bitrate_kbps = abr.current_kbps as i32;
                            self.bitrate_kbps = abr.current_kbps;
                            self.host_abr = Some(abr);
                            self.adaptive_bitrate = true;
                        }
                        Ok(None) => eprintln!("nightjar: the PC has no automatic bitrate"),
                        Err(error) => self.show_toast(error),
                    }
                }
                Notice::HostAbrAction(action) => {
                    if let Some(kbps) = self.host_abr.as_mut().and_then(|abr| abr.answered(action.as_ref())) {
                        if let Some(action) = &action { eprintln!("nightjar: the PC set {kbps} kbps: {}", action.reason); }
                        self.facts.bitrate_kbps = kbps as i32;
                        self.bitrate_kbps = kbps;
                    }
                }
                Notice::Bitrate { requested, result } => {
                    self.bitrate_busy = false;
                    match result {
                        Ok(kbps) => {
                            self.facts.bitrate_kbps = kbps as i32;
                            self.bitrate_kbps = kbps;
                            if let Some(rates) = &mut self.rate_change { rates.bitrate_set(kbps, self.started.elapsed().as_millis() as u64); }
                            if let Some(policy) = &mut self.bitrate_policy {
                                policy.applied(requested, kbps, self.started.elapsed().as_millis() as u64);
                            } else {
                                // A manual change replaces the doctor's trial.
                                self.doctor.trial.clear();
                                self.show_toast(trf("Bitrate set to {} kbps.", &[&kbps]));
                            }
                        }
                        Err(error) => {
                            self.adaptive_bitrate = false;
                            self.bitrate_policy = None;
                            self.show_toast(error);
                        }
                    }
                }
                Notice::HealthTrial(outcome) => {
                    self.bitrate_busy = false;
                    let (kbps, toast) = doctor::finished(&mut self.doctor.trial, outcome);
                    if let Some(kbps) = kbps.filter(|kbps| *kbps > 0) {
                        self.facts.bitrate_kbps = kbps as i32;
                        self.bitrate_kbps = kbps;
                        if let Some(rates) = &mut self.rate_change { rates.bitrate_set(kbps, self.started.elapsed().as_millis() as u64); }
                    }
                    // Automatic bitrate or quality started meanwhile: it owns the bitrate from here on.
                    if self.automatic_bitrate() { self.doctor.trial.clear(); }
                    self.show_toast(toast);
                }
                Notice::HardwareMetrics { generation, result } => {
                    let answer = hardware_answer(&result);
                    match &result {
                        Err(error) if answer == HardwareAnswer::Unavailable => eprintln!("nightjar: host hardware readings unavailable: {error}"),
                        Err(error) => eprintln!("nightjar: host hardware readings failed: {error}"),
                        Ok(_) => {}
                    }
                    if self.hardware_poll.answered(generation, self.started.elapsed().as_millis() as u64, answer) {
                        self.hardware = result.ok();
                    }
                }
                Notice::Stage(_) | Notice::Reconnecting(_) | Notice::Started => {}
            }
        }
        self.route_microphones(pads);
        *lock(&self.shared.controller_audio) = pads.pads.iter().filter_map(|p| p.audio.as_ref().map(|audio| Arc::downgrade(&audio.playback))).collect();
        self.controller_mouse_active = false;
        if self.settings.controllers && self.facts.role != AppRole::RemoteMonitor && !self.menu_visible && (self.focused || self.settings.background_controllers) {
            self.session.input(|q| self.controller_mouse_active = pads.update_mouse(q));
        }
        let mut status = [0u8; 16];
        if self.controller_audio_available() {
            for pad in &pads.pads {
                if let Some(audio) = &pad.audio { status[usize::from(pad.index)] = audio.status(self.settings.microphone, self.microphone_muted); }
            }
        }
        if status != self.audio_status.0 || self.audio_status.1.elapsed() >= Duration::from_secs(1) {
            for (index, &flags) in status.iter().enumerate() {
                if flags != 0 || self.audio_status.0[index] != 0 {
                    let _ = self.session.send(ClientMessage::ControllerAudioStatus { controller: index as u16, flags });
                }
            }
            self.audio_status = (status, Instant::now());
        }
    }

    fn control_event(&mut self, event: ControlEvent, pads: &mut Gamepads) {
        match event {
            ControlEvent::ControllerHaptics(pcm) if self.controller_audio_available() => pads.pcm(pcm, false),
            ControlEvent::ControllerSpeaker(pcm) if self.controller_audio_available() => pads.pcm(pcm, true),
            ControlEvent::ControllerAudioControl(payload) if self.controller_audio_available() => {
                pads.audio_control(&payload);
                if pads.pads.iter().any(|p| p.audio.as_ref().is_some_and(|a| a.host_muted)) {
                    self.microphone_muted = true;
                    self.microphone = None;
                }
            }
            ControlEvent::Rumble {
                controller,
                low_frequency,
                high_frequency,
            } => pads.rumble(controller, low_frequency, high_frequency),
            ControlEvent::RumbleTriggers { controller, left, right } => {
                pads.rumble_triggers(controller, left, right)
            }
            ControlEvent::SetControllerLed { controller, r, g, b } => pads.set_led(controller, r, g, b),
            ControlEvent::AdaptiveTriggers { controller, event_flags, type_left, type_right, left, right } => {
                pads.adaptive_triggers(controller, event_flags, type_left, type_right, &left, &right);
            }
            ControlEvent::SetMotionEventState {
                controller,
                motion_type,
                report_rate_hz,
            } => pads.set_motion(controller, motion_type, report_rate_hz),
            ControlEvent::HostTiming(records) => {
                if let Some(csv) = &self.timing_csv {
                    csv.reports(&records);
                }
                for (r, _) in records {
                    self.window.timing_host_us += u64::from(r.host_us);
                    self.window.timing_encode_us += u64::from(r.encode_us);
                    self.window.timing_n += 1;
                    self.doctor.host_sent(r.host_us);
                    self.timings.push_back((r.frame, u64::from(r.host_us)));
                    if self.timings.len() > 64 {
                        self.timings.pop_front();
                    }
                }
            }
            ControlEvent::LinkNotice(notice) => {
                self.link_notice = (notice.state != LinkState::Full).then_some(notice);
            }
            ControlEvent::PipelineRates(report) => self.pipeline_rates = Some(report),
            ControlEvent::RateChange(result) => self.rate_change_result(&result),
            ControlEvent::ClockSync { reply, received } => {
                let receive_us = session::now_us().saturating_sub(received.elapsed().as_micros() as u64);
                self.offset.add(&reply, receive_us);
            }
            ControlEvent::HdrMode { enabled, metadata } => {
                self.texture.set_hdr_mode(enabled, metadata);
                self.needs_redraw = true;
            }
            ControlEvent::CursorShape(shape) => {
                if let Err(error) = self.cursor.apply(shape) {
                    self.cursor_error(error);
                }
            }
            ControlEvent::CursorShapeRejected(reason) if self.cursor.enabled => {
                self.cursor.default_cursor();
                self.cursor_error(reason.to_owned());
            }
            // Steam Controller haptics and settings stay unhandled: no arrival advertises them.
            ControlEvent::SteamRejected { message_type, error } => {
                eprintln!(
                    "nightjar: dropping Steam Controller message {message_type:#06x}: {error:?}"
                );
            }
            ControlEvent::Roam(event) => eprintln!("nightjar: {event}"),
            ControlEvent::TextInput(notice) => self.text_field.notice(&notice),
            ControlEvent::ClipboardSync(frame) => {
                if let Some(step) = self.clipboard_sync.as_mut().map(|sync| sync.received(frame, Instant::now())) {
                    self.clipboard_sync_step(step);
                }
            }
            ControlEvent::ClipboardSyncRejected(bytes) => {
                if let Some(step) = self.clipboard_sync.as_mut().map(|sync| sync.rejected(bytes)) {
                    self.clipboard_sync_step(step);
                }
            }
            ControlEvent::Usb(message) => match &self.usb {
                Some(usb) => usb.on_message(message),
                None => eprintln!("nightjar: USB: ignored {message:?}: USB forwarding is off here"),
            },
            _ => {}
        }
    }

    fn cursor_error(&mut self, error: String) {
        eprintln!("nightjar: cursor: {error}");
        if !self.cursor_warning {
            self.cursor_warning = true;
            self.show_toast(tr("The PC's cursor could not be drawn; using the default pointer."));
        }
    }

    pub fn clipboard(&mut self, send: bool) {
        if !self.facts.clipboard || self.clipboard_busy { return; }
        let client = lock(&self.shared.host_client).clone().expect("connected HTTP client");
        let text = if send {
            unsafe {
                let text = SDL_GetClipboardText();
                if text.is_null() {
                    self.show_toast(tr("Could not read the local clipboard."));
                    return;
                }
                let value = std::ffi::CStr::from_ptr(text).to_string_lossy().into_owned();
                SDL_free(text.cast());
                Some(value)
            }
        } else { None };
        let shared = self.shared.clone();
        self.clipboard_busy = true;
        std::thread::spawn(move || {
            let result = client.server_info_https().map_err(|e| e.to_string()).and_then(|info| {
                let permission = if send { nightjar_core::http::PERMISSION_CLIPBOARD_SET } else { nightjar_core::http::PERMISSION_CLIPBOARD_READ };
                if !info.capabilities.has(Feature::Clipboard) || info.permission.is_some_and(|p| p & permission == 0) {
                    return Err("The PC denied clipboard access.".into());
                }
                match text {
                    Some(text) => client.send_clipboard(&text).map(|_| None),
                    None => client.clipboard().map(Some),
                }.map_err(|e| e.to_string())
            });
            shared.notify(Notice::Clipboard(result));
        });
    }

    /// Sends the image on this device's clipboard to the PC, or (`send` false) puts the PC's
    /// image on this device's clipboard. Every failure says what failed.
    pub fn clipboard_image(&mut self, send: bool) {
        if !self.facts.image_clipboard || self.clipboard_busy { return; }
        let client = lock(&self.shared.host_client).clone().expect("connected HTTP client");
        let local = if send {
            match crate::clipboard::read() {
                Ok(Some(local)) => Some(local),
                Ok(None) => {
                    self.show_toast(tr("The clipboard on this device holds no image."));
                    return;
                }
                Err(error) => {
                    self.show_toast(trf("Could not read the image on this device's clipboard: {}", &[&error]));
                    return;
                }
            }
        } else { None };
        let shared = self.shared.clone();
        self.clipboard_busy = true;
        std::thread::spawn(move || {
            let result = match local {
                Some(local) => crate::clipboard::to_png(local)
                    .map_err(|e| trf("Could not convert the image on this device's clipboard to PNG: {}", &[&e]))
                    .and_then(|png| client.send_clipboard_image(&png).map_err(|e| image_error(e, true)))
                    .map(|()| None),
                None => match client.clipboard_image() {
                    Ok(Some(png)) => crate::clipboard::from_png(&png)
                        .map(Some)
                        .map_err(|e| trf("Could not read the PC's image: {}", &[&e])),
                    Ok(None) => Err(tr("The PC's clipboard holds no image.").to_owned()),
                    Err(e) => Err(image_error(e, false)),
                },
            };
            shared.notify(Notice::ClipboardImage(result));
        });
    }

    pub fn server_command(&mut self, index: usize) {
        if self.command_busy || self.last_command.is_some_and(|at| at.elapsed() < Duration::from_secs(1)) {
            self.show_toast(tr("Wait one second between PC commands."));
            return;
        }
        let Some(name) = self.facts.server_commands.get(index).cloned() else { return; };
        let client = lock(&self.shared.host_client).clone().expect("connected HTTP client");
        let shared = self.shared.clone();
        self.command_busy = true;
        self.last_command = Some(Instant::now());
        std::thread::spawn(move || {
            let result = client.server_info_https().map_err(|e| e.to_string()).and_then(|info| {
                if !info.capabilities.has(Feature::ServerCommands)
                    || info.server_commands.get(index) != Some(&name) {
                    return Err("The command changed or permission was denied. Reconnect to refresh the PC commands.".into());
                }
                Ok(if info.capabilities.family() == nightjar_core::http::Family::Rubylight {
                    vec![index as u8]
                } else { vec![index as u8, 0, 0, 0] })
            });
            shared.notify(Notice::ServerCommand(result));
        });
    }

    /// Asks the host for its hardware readings on a worker; the answer comes back as a notice.
    fn request_hardware(&self, generation: u32) {
        let client = lock(&self.shared.host_client).clone().expect("connected HTTP client");
        let capabilities = self.plan.info.capabilities.clone();
        let shared = self.shared.clone();
        std::thread::spawn(move || {
            let result = client.hardware_metrics(&capabilities);
            shared.notify(Notice::HardwareMetrics { generation, result });
        });
    }

    pub fn set_bitrate(&mut self) {
        if !self.facts.runtime_bitrate || self.bitrate_busy { return; }
        let client = lock(&self.shared.host_client).clone().expect("connected HTTP client");
        let shared = self.shared.clone();
        self.bitrate_kbps = capped_bitrate(self.bitrate_kbps, self.plan.bitrate_limit_kbps);
        let bitrate = self.bitrate_kbps;
        self.bitrate_busy = true;
        std::thread::spawn(move || {
            let result = client.server_info_https().map_err(|e| e.to_string()).and_then(|info| {
                if !info.capabilities.has(Feature::RuntimeBitrate) {
                    return Err("The PC does not allow bitrate changes.".into());
                }
                client.set_bitrate(bitrate).map_err(|e| e.to_string())
            });
            shared.notify(Notice::Bitrate { requested: bitrate, result });
        });
    }

    /// Sends the stream menu's rate change; the host's answer comes back as a toast.
    pub fn change_rates(&mut self) {
        let Some(rates) = &mut self.rate_change else { return };
        match rates.menu_request(self.started.elapsed().as_millis() as u64) {
            Ok(request) => self.send_rate_change(request),
            Err(toast) => self.show_toast(toast),
        }
    }

    fn send_rate_change(&mut self, request: RateChangeRequest) {
        if let Err(error) = self.session.send(ClientMessage::RateChange(request)) {
            let toast = self.rate_change.as_mut().expect("rate changes offered").send_failed(&request, &error);
            self.show_toast(toast);
        }
    }

    fn rate_change_result(&mut self, result: &RateChangeResult) {
        let Some(rates) = &mut self.rate_change else { return };
        let answer = rates.result(result, self.started.elapsed().as_millis() as u64);
        if let Some(kbps) = answer.bitrate_kbps {
            self.facts.bitrate_kbps = kbps as i32;
            self.bitrate_kbps = kbps;
            let now = self.started.elapsed().as_millis() as u64;
            if let Some(policy) = &mut self.bitrate_policy { policy.applied(kbps, kbps, now); }
        }
        if let Some(fps_millihz) = answer.fps_millihz {
            self.facts.fps = fps_millihz.div_ceil(1000) as i32;
            self.shared.fps_millihz.store(fps_millihz, Ordering::Relaxed);
            *lock(&self.shared.decode_window) = pipeline::DecodeWindow::default();
            self.decode_window_us = session::now_us();
            self.decode_generation += 1;
            self.requested_mode.fps_millihz = fps_millihz;
            if let Some(rate) = &mut self.bitrate_policy {
                let mode = self.requested_mode;
                rate.set_mode(u32::from(mode.width), u32::from(mode.height), fps_millihz.div_ceil(1000));
            }
        }
        if let Some(toast) = answer.toast { self.show_toast(toast); }
    }

    /// Which bitrate and rate controls the stream menu shows.
    pub fn menu_controls(&self) -> rate_change::MenuControls {
        rate_change::menu_controls(self.facts.runtime_bitrate, self.facts.reconfigure, self.rate_change.is_some())
    }

    /// The client-hint controller steers this stream's bitrate in place of the PC's adaptive
    /// bitrate.
    pub fn client_hint_active(&self) -> bool {
        self.rate_change.as_ref().is_some_and(|rates| rates.hint_active())
    }

    /// Automatic bitrate, automatic quality or the client-hint controller owns the bitrate.
    fn automatic_bitrate(&self) -> bool {
        bitrate_automatic(self.adaptive_bitrate, self.quality_mode, self.rate_change.as_ref())
    }

    pub fn enable_adaptive_bitrate(&mut self, enabled: bool) {
        if self.bitrate_busy { return; }
        if enabled && self.client_hint_active() {
            eprintln!("nightjar: client-hint bitrate adapts this stream; the PC's adaptive bitrate stays off");
            return;
        }
        self.bitrate_policy = None;
        self.stop_host_abr();
        self.adaptive_bitrate = enabled && self.facts.runtime_bitrate && self.facts.role == AppRole::Stream;
        if !self.adaptive_bitrate {
            // A PC without Rubylight's runtime bitrate may run its own controller (Foundation).
            if enabled && self.facts.role == AppRole::Stream { self.start_host_abr(); }
            return;
        }
        self.doctor.trial.clear();
        let client = lock(&self.shared.host_client).clone().expect("connected HTTP client");
        let shared = self.shared.clone();
        self.bitrate_busy = true;
        let limit = self.plan.bitrate_limit_kbps;
        std::thread::spawn(move || {
            let result = client.server_info_https().map_err(|e| e.to_string()).and_then(|info| {
                if !info.capabilities.has(Feature::RuntimeBitrate) || info.encoder_bitrate_kbps == 0 || info.initial_encoder_bitrate_kbps == 0 {
                    return Err("The PC did not provide an encoder budget for automatic bitrate.".into());
                }
                let applied = client.set_bitrate(capped_bitrate(info.encoder_bitrate_kbps, limit)).map_err(|e| e.to_string())?;
                Ok((capped_bitrate(info.initial_encoder_bitrate_kbps.max(applied), limit), applied))
            });
            shared.notify(Notice::AdaptiveBitrate(result));
        });
    }

    /// Asks a Foundation PC to run its own bitrate controller up to the stream's bitrate, as
    /// Moonlight V+ PC's `startSunshineAbr` does. Other PCs answer 404: nothing changes there.
    /// The start may outlive the stream (only `finish` ends it while in flight: `bitrate_busy` holds
    /// the setting): its own client is not cancelled by `finish`, and `AbrStart` decides who turns
    /// the PC's controller off.
    fn start_host_abr(&mut self) {
        let Some(client) = lock(&self.shared.host_client).as_ref().map(|c| c.detached()) else { return };
        let shared = self.shared.clone();
        let max_kbps = self.facts.bitrate_kbps.max(0) as u32;
        let start = host_abr::AbrStart::default();
        self.host_abr_start = Some(start.clone());
        self.bitrate_busy = true;
        std::thread::spawn(move || {
            let result = match client.abr_capabilities() {
                Ok(caps) if caps.supported => client
                    .configure_abr(&AbrRequest::balanced_up_to(max_kbps))
                    .map(Some)
                    .map_err(|e| trf("The PC did not start its automatic bitrate: {}", &[&e])),
                Ok(_) | Err(HttpError::NotFound(_)) => Ok(None),
                Err(e) => Err(trf("The PC did not start its automatic bitrate: {}", &[&e])),
            };
            if matches!(result, Ok(Some(_))) && !start.turned_on() {
                // The stream ended while the PC turned it on: nobody else will turn it off.
                if let Err(e) = client.configure_abr(&AbrRequest::off()) {
                    eprintln!("nightjar: the PC's automatic bitrate did not stop: {e}");
                }
                return;
            }
            shared.notify(Notice::HostAbr(result));
        });
    }

    /// Turns a Foundation PC's controller off again (V+ PC's `stopSunshineAbr`, 1 s at most).
    fn stop_host_abr(&mut self) {
        if self.host_abr.take().is_some() { self.adaptive_bitrate = false; }
        // On, though its answer may not have come yet; a start still in flight turns it off itself.
        if !self.host_abr_start.take().is_some_and(|start| start.end()) { return; }
        if let Some(client) = lock(&self.shared.host_client).clone() {
            if let Err(e) = client.configure_abr(&AbrRequest::off()) {
                eprintln!("nightjar: the PC's automatic bitrate did not stop: {e}");
            }
        }
    }

    /// The trial the doctor offers with this report: none while automatic bitrate or quality owns
    /// the bitrate, the host cannot change it mid-stream or another bitrate request runs.
    fn trial_offer(&self, report: &nightjar_policy::health::Report) -> TrialOffer {
        let automatic = self.automatic_bitrate();
        self.doctor.trial.offer(report.can_suggest_bitrate(), self.facts.bitrate_kbps.max(0) as u32, self.facts.runtime_bitrate, automatic, self.bitrate_busy)
    }

    /// The stream doctor's report for the stream menu, and the trial it offers.
    pub fn health(&mut self) -> (String, TrialOffer) {
        let report = self.doctor.report(self.started.elapsed().as_millis() as u64);
        let offer = self.trial_offer(&report);
        let mut text = doctor::report_text(&report);
        if let Some(hint) = self.doctor.receive_buffer_hint() {
            text.push_str("\n\n");
            text.push_str(&hint);
        }
        if offer != TrialOffer::None {
            text.push_str("\n\n");
            text.push_str(doctor::trial_help());
        }
        (text, offer)
    }

    /// Runs the step `offer` names if the doctor still offers it: lowers the host's live encoder
    /// bitrate by a fifth, or goes back to the bitrate before the trial.
    pub fn health_trial(&mut self, offer: TrialOffer) {
        let report = self.doctor.report(self.started.elapsed().as_millis() as u64);
        if offer == TrialOffer::None || self.trial_offer(&report) != offer { return; }
        let client = lock(&self.shared.host_client).clone().expect("connected HTTP client");
        let shared = self.shared.clone();
        let trial = self.doctor.trial;
        self.bitrate_busy = true;
        std::thread::spawn(move || {
            let live = || {
                let info = client.server_info_https().map_err(|e| e.to_string())?;
                if !info.capabilities.has(Feature::RuntimeBitrate) {
                    return Err("The PC does not allow bitrate changes.".to_owned());
                }
                Ok(info.encoder_bitrate_kbps)
            };
            let outcome = doctor::run_trial(trial, offer == TrialOffer::Undo, live, |kbps| client.set_bitrate(kbps).map_err(|e| e.to_string()));
            shared.notify(Notice::HealthTrial(outcome));
        });
    }

    /// Copies the doctor's report to this device's clipboard.
    pub fn copy_health(&mut self) {
        let text = std::ffi::CString::new(self.health().0);
        if text.as_ref().is_ok_and(|text| unsafe { SDL_SetClipboardText(text.as_ptr()) }) {
            self.show_toast(tr("Report copied."));
        } else {
            self.show_toast(tr("Could not copy the report."));
        }
    }

    /// The report for the screen after the stream when it found something; the log gets it either way.
    pub fn health_summary(&mut self) -> Option<String> {
        if self.facts.role == AppRole::InputOnly { return None; }
        let report = self.doctor.report(self.started.elapsed().as_millis() as u64);
        eprintln!("nightjar: stream health: {}", doctor::log_line(&report));
        doctor::summary(&report).map(|summary| match self.doctor.receive_buffer_hint() {
            Some(hint) => format!("{summary}\n\n{hint}"),
            None => summary,
        })
    }

    pub fn toggle_menu(&mut self, window: *mut SDL_Window, pads: &mut Gamepads) {
        // While End or Close game runs, the Session page stays so its answer (or failure) shows.
        if self.menu_visible && self.session_page.as_ref().is_some_and(|p| p.busy()) {
            return;
        }
        self.menu_visible = !self.menu_visible;
        if !self.menu_visible {
            self.session_page = None;
            // The menu's own text field takes no more text once it closes.
            // SAFETY: a live window on the main thread.
            unsafe {
                if SDL_TextInputActive(window) && !SDL_StopTextInput(window) {
                    eprintln!("nightjar: could not stop the menu's text input");
                }
            }
        }
        self.text_field.apply(window, self.menu_visible);
        self.raise_all_keys();
        self.session.input(|q| pads.release(q));
        self.set_capture(window, !self.menu_visible);
        self.needs_redraw = true;
    }

    /// Whether the PC speaks the Rubylight native API, so the Session page has something to show.
    pub fn session_page_available(&self) -> bool {
        self.facts.role == AppRole::Stream && self.plan.info.capabilities.has(Feature::NativeApi)
    }

    /// Opens the Session page and starts its worker. `link` is the network description.
    pub fn open_session_page(&mut self, waker: Waker, link: String) {
        let identity = self.plan.identity.clone();
        let record = self.plan.record.clone();
        let address = record.address.clone();
        self.session_link = link;
        self.session_page_requested = false;
        self.session_page = Some(crate::session_page::Page::open(
            move || crate::hosts::client_for(&identity, &record, &address),
            waker,
        ));
        self.needs_redraw = true;
    }

    /// Closes Nightjar's menu first, so the stream (and the host's menu) gets the input again,
    /// then asks the host to open its menu. A refusal comes back as a toast.
    pub fn open_host_menu(&mut self, window: *mut SDL_Window, pads: &mut Gamepads) {
        if self.menu_visible {
            self.toggle_menu(window, pads);
        }
        if self.menu_visible {
            // An End or Close game is still running; the page stays for its answer.
            return;
        }
        let Some(client) = lock(&self.shared.host_client).clone() else {
            self.show_toast(tr("Cannot reach the host."));
            return;
        };
        let shared = self.shared.clone();
        std::thread::spawn(move || {
            shared.notify(Notice::HostMenu(crate::session_page::host_menu_result(client.couch_menu(true))));
        });
    }

    /// Closes the Session page; its worker ends and nothing polls.
    pub fn close_session_page(&mut self) {
        if self.session_page.as_ref().is_some_and(|p| p.busy()) {
            return;
        }
        self.session_page = None;
        self.needs_redraw = true;
    }

    /// What is known about this device's permissions before the page has read them: nothing,
    /// so nothing is offered until `/capabilities` has answered.
    pub fn session_hints(&self) -> crate::session_page::Hints {
        crate::session_page::Hints { launch: false, host_control: false }
    }

    /// The connection facts under the Session page's actions.
    pub fn connection_lines(&self) -> Vec<(&'static str, String)> {
        let network = self.overlay.lines.iter().find(|(k, _)| k == "Network").map(|(_, v)| v.as_str());
        crate::session_page::connection_lines(
            &self.facts.host_name,
            codec_name(self.facts.format),
            (self.facts.width, self.facts.height, self.facts.fps),
            self.facts.bitrate_kbps,
            &self.session_link,
            network,
        )
    }

    pub fn send_shortcut(&mut self, shortcut: &crate::shortcuts::Shortcut) {
        if !self.focused || !self.captured || self.facts.role == AppRole::RemoteMonitor { return; }
        if !self.settings.capture_system_keys && shortcut.keys.iter().any(|key| matches!(crate::shortcuts::key_code(key), Some(0x5B | 0x5C))) {
            self.show_toast(tr("Enable Send system keys to use shortcuts with the Windows key."));
            return;
        }
        let result = self.session.input(|q| crate::shortcuts::send(&shortcut.keys, &self.keys_down, q));
        if let Err(error) = result { self.show_toast(trf("Shortcut was not sent: {}", &[&error])); }
    }

    /// Moves decoded pictures into the pacer; sends a clock sync probe when due.
    pub fn pump(&mut self) {
        self.cursor.tick(Instant::now());
        if let Some(microphone) = &self.microphone {
            while let Ok(packet) = microphone.packets.try_recv() {
                if let Err(error) = self.session.send_microphone(&packet) {
                    self.shared.notify(Notice::MicrophoneFailed(format!("Microphone forwarding stopped: {error}")));
                    break;
                }
            }
        }
        // Frames first: the slices of every frame taken here were queued before the frame was.
        let frames: Vec<_> = lock(&self.shared.frames).drain(..).collect();
        // With the race off nothing queues a slice, and nothing here takes the lock.
        let slices: Vec<_> = if self.facts.beam_race { lock(&self.shared.slices).drain(..).collect() } else { Vec::new() };
        for decoded in frames {
            if decoded.setup.format & VIDEO_FORMAT_MASK_PYROWAVE != 0 { self.race_setup = Some(decoded.setup); }
            if let Some(frame) = decoded.picture.frame().filter(|_| decoded.setup.chroma_packing == 0 && decoded.setup.format & VIDEO_FORMAT_MASK_PYROWAVE == 0) {
                let info = frame.info();
                self.facts.width = info.width;
                self.facts.height = info.height;
                if self.pending_mode.is_some_and(|(mode, _)| i32::from(mode.width) == info.width && i32::from(mode.height) == info.height) {
                    self.pending_mode = None;
                }
            }
            if let Some((rtp, received)) = self.last_arrival {
                let ticks = decoded.rtp.wrapping_sub(rtp) as i32;
                if ticks > 0 && decoded.receive_us >= received {
                    let delta = ((decoded.receive_us - received) as f64 / 1000.0 - f64::from(ticks) / 90.0).abs();
                    self.jitter_ms = if self.jitter_ms < 0.0 { delta } else { self.jitter_ms + (delta - self.jitter_ms) / 16.0 };
                }
            }
            self.last_arrival = Some((decoded.rtp, decoded.receive_us));
            self.doctor.arrived(decoded.rtp, decoded.host_latency, decoded.receive_us, self.facts.fps.max(0) as u32);
            self.pacer.accept(decoded);
        }
        self.observe_slices(slices);
        if self.pending_mode.is_some_and(|(_, at)| at.elapsed() >= Duration::from_secs(5)) {
            self.pending_mode = None;
            self.quality_policy = None;
            self.quality_mode = QualityMode::Off;
            self.show_toast(tr("The PC did not confirm the requested size. Automatic quality stopped."));
        }
        if self.facts.clock_sync && self.last_probe.elapsed() >= Duration::from_millis(250) {
            self.last_probe = Instant::now();
            let _ = self.session.send_clock_sync_probe(session::now_us());
        }
    }

    /// How long the loop may sleep before the next picture is due.
    pub fn wait(&self) -> Wait {
        let racing = if self.pacer.mode == PacingMode::BeamRace { self.racer.due_us() } else { None };
        let due = match (self.pacer.next_due(), racing) {
            (Some(picture), Some(slice)) => Some(picture.min(slice)),
            (picture, slice) => picture.or(slice),
        };
        let wait = match due {
            Some(due) => {
                let now = session::now_us();
                if due <= now {
                    Wait::Now
                } else {
                    Wait::For(Duration::from_micros(due - now))
                }
            }
            None => Wait::For(Duration::from_millis(if self.controller_mouse_active || self.audio_status.0.iter().any(|flags| *flags != 0) { 20 } else if self.overlay_visible { 250 } else { 500 })),
        };
        // A hide from the PC waiting out its delay wakes the loop when due.
        match (wait, self.cursor.hide_in(Instant::now())) {
            (Wait::For(wait), Some(hide)) => Wait::For(wait.min(hide)),
            (wait, _) => wait,
        }
    }

    /// Works out whether the display allows racing slices and, if so, how the picture sits on its
    /// beam; switches the pacing between `Immediate` and `BeamRace` when that changes.
    fn sync_race(&mut self, renderer: *mut SDL_Renderer) {
        if !self.facts.beam_race { return; }
        let timing = self.race_probe.as_ref().and_then(race_probe::RaceProbe::racing);
        let size = self.texture.size();
        self.race_scan = timing.filter(|_| size.0 > 0 && size.1 > 0).map(|(beam, delta_ns)| {
            let (mut w, mut h) = (0, 0);
            // SAFETY: the live renderer belongs to the presenting thread.
            unsafe { SDL_GetCurrentRenderOutputSize(renderer, &mut w, &mut h) };
            let dst = viewport(size, (w, h), self.settings.video_scaling, self.settings.align_video_top);
            race::Scan::for_picture(&beam, delta_ns, size.1 as u32, dst.y.max(0.0) as u32, dst.h as u32)
        });
        match (self.pacer.mode, self.race_scan.is_some() && self.race_present) {
            (PacingMode::Immediate, true) => {
                self.pacer.mode = PacingMode::BeamRace;
                eprintln!("nightjar: beam race: presenting slices ahead of the beam");
            }
            (PacingMode::BeamRace, false) => {
                self.pacer.mode = PacingMode::Immediate;
                // The slices it held let go of their plane sets now, not at the next frame.
                self.racer.reset();
                eprintln!("nightjar: beam race: the display no longer allows racing, presenting whole pictures");
            }
            _ => {}
        }
    }

    /// Turns a VRR stream to the pacing it would have without VRR once the display check finds the
    /// display scanning at its own rate (`present::VrrPacing`), and gives the renderer the sync
    /// interval the pacing and `NIGHTJAR_VRR_SYNC` ask for.
    fn sync_vrr(&mut self, renderer: *mut SDL_Renderer) {
        let before = self.pacer.mode;
        let probe = self.race_probe.as_ref();
        let interval = self.vrr.step(&mut self.pacer.mode, probe.map(race_probe::RaceProbe::verdict));
        if self.pacer.mode != before {
            eprintln!("nightjar: pacing: {}, this display does not refresh on present (display check: {})",
                self.pacer.mode.label(), probe.map_or(String::new(), race_probe::RaceProbe::line));
        }
        if let Some(interval) = interval {
            // SAFETY: the live renderer belongs to the presenting thread.
            if !unsafe { SDL_SetRenderVSync(renderer, interval) } {
                eprintln!("nightjar: pacing: could not set the sync interval to {interval}: {}", crate::sdl::error());
            }
        }
    }

    /// `VrrSync::Scan`: while the beam is in the picture's rows, waits for it to leave the last one,
    /// so the present shows its picture whole. Returns how long it waited (ns).
    fn wait_out_scan(&mut self) -> u64 {
        // The tearing modes that present whole pictures; the beam race times its own presents.
        if self.vrr.sync != present::VrrSync::Scan || !matches!(self.pacer.mode, PacingMode::Immediate | PacingMode::HostTimes) { return 0; }
        let Some(probe) = &self.race_probe else { return 0 };
        let (Some(beam), Some(sample)) = (probe.beam(), probe.before()) else { return 0 };
        let Some(left) = beam.scan_left_ns(&sample) else { return 0 };
        // Spinning: the wait is under one scan (4.2 ms at 240 Hz), shorter than a sleep can be exact.
        let until = Instant::now() + Duration::from_nanos(left);
        while Instant::now() < until { std::hint::spin_loop(); }
        self.scan_waits.0 += 1;
        self.scan_waits.1 += left;
        left
    }

    /// Takes the slices the decoder finished since the last call into the racer.
    fn observe_slices(&mut self, slices: Vec<pyrowave::SliceReady>) {
        // Anything the racer does not take is dropped here, which lets go of its plane set.
        let (Some(scan), PacingMode::BeamRace, Some(_)) = (self.race_scan, self.pacer.mode, self.race_setup) else { return };
        let margin = self.latency.margin_us(self.race_margin_ms);
        let now = session::now_us();
        let facts = &self.facts;
        let predictor = self.predictor.get_or_insert_with(|| seeded_predictor(facts));
        for ready in slices {
            self.racer.observe(ready.seq, ready.band, ready.pin, now, &scan, predictor, &self.latency, margin);
        }
    }

    /// Presents the slices of the frame being received that are due: the first at the planned
    /// scan start, the others as they are ready. Returns whether it presented.
    #[cfg(windows)]
    fn present_slices(&mut self, renderer: *mut SDL_Renderer, draw_overlay: &mut impl FnMut(&mut Self, *mut SDL_Renderer), now: u64) -> bool {
        if self.pacer.mode != PacingMode::BeamRace { return false; }
        let (Some(scan), Some(setup)) = (self.race_scan, self.race_setup) else { return false };
        // The picture is decoded already: its own present shows the rows, a slice present first
        // would only keep it waiting (a present costs 0.4 ms of this thread).
        if self.pacer.front_race().is_some_and(|(seq, due)| seq != 0 && seq == self.racer.seq() && due <= now) { return false; }
        let batch = self.racer.take(now);
        if batch.is_empty() { return false; }
        for run in race::runs(batch.iter().map(|(band, pin)| (*band, pin.set()))) {
            let Some(rows) = race::scissor(run.first_row, run.end_row, setup.height as u32) else { continue };
            if !self.texture.upload_slice(renderer, setup, &batch[run.last].1, run.fence_value, rows) {
                let reason = self.texture.failure.take().unwrap_or_else(|| "PyroWave GPU reconstruction failed.".into());
                self.exit = Some(Exit::Ended(Some(reason)));
                return true;
            }
        }
        let flipped = self.render_flip(renderer, draw_overlay, true);
        self.needs_redraw = false;
        self.racer.presented(flipped.issued_us, &scan);
        for (band, _) in &batch {
            self.latency.record(self.racer.presentable_us(band.ready_us), flipped.issued_us, flipped.returned_us, flipped.returned_us);
        }
        true
    }

    #[cfg(not(windows))]
    fn present_slices(&mut self, _renderer: *mut SDL_Renderer, _draw_overlay: &mut impl FnMut(&mut Self, *mut SDL_Renderer), _now: u64) -> bool {
        false
    }

    /// The current picture through AI enhancement at `output`, or None to draw the picture itself:
    /// a picture it does not read (software-decoded, PyroWave, packed), or the mode failed, which
    /// is logged and shown once and leaves the stream on bilinear scaling.
    #[cfg(windows)]
    fn enhanced(&mut self, renderer: *mut SDL_Renderer, output: (i32, i32)) -> Option<*mut SDL_Texture> {
        if self.upscale_failed { return None; }
        let Some(source) = self.texture.enhance_source() else { self.enhancer = None; return None };
        if output.0 <= 0 || output.1 <= 0 || output.0 > 16384 || output.1 > 16384 { return None; }
        let key = enhance::Key { source, output, hdr_display: crate::sdl::hdr_available(renderer) };
        if !self.enhancer.as_ref().is_some_and(|enhancer| enhancer.matches(&key)) {
            self.enhancer = None;
            let device = crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_DEVICE_POINTER);
            let made = enhance::stream_probe(device)
                .and_then(|capability| capability.ok_or_else(|| "this GPU has neither video super resolution nor RTX Video HDR".to_owned()))
                .and_then(|capability| enhance::Enhancer::new(renderer, key, capability, self.facts.fps.max(1) as u32));
            match made {
                Ok(enhancer) => {
                    eprintln!("nightjar: {} for {}x{} to {}x{}", enhancer.label, source.picture.0, source.picture.1, output.0, output.1);
                    self.enhancer = Some(enhancer);
                }
                Err(error) => return self.enhance_failed(error),
            }
        }
        match self.enhancer.as_mut()?.draw(renderer) {
            Ok(texture) => Some(texture),
            Err(error) => self.enhance_failed(error),
        }
    }

    #[cfg(not(windows))]
    fn enhanced(&mut self, _renderer: *mut SDL_Renderer, _output: (i32, i32)) -> Option<*mut SDL_Texture> {
        None
    }

    #[cfg(windows)]
    fn enhance_failed(&mut self, error: String) -> Option<*mut SDL_Texture> {
        eprintln!("nightjar: AI enhancement failed: {error}; the stream continues with bilinear scaling");
        self.enhancer = None;
        self.upscale_failed = true;
        self.show_toast(tr("AI enhancement is unavailable; using bilinear scaling."));
        None
    }

    /// The stats line for the upscaling setting.
    fn upscaling_line(&self) -> String {
        if self.settings.upscaling == config::Upscaling::Ai {
            #[cfg(windows)]
            if let Some(enhancer) = &self.enhancer { return enhancer.label.clone(); }
            return if self.upscale_failed { "Bilinear (AI enhancement unavailable)".into() }
                else if cfg!(windows) { "Bilinear (not a hardware-decoded picture)".into() }
                else { "Bilinear (AI enhancement needs Direct3D 11)".into() };
        }
        if self.upscale_failed { "Bilinear (GPU unavailable)".into() }
        else if self.texture.is_hdr() { "Bilinear (HDR)".into() }
        else if self.upscaler.is_some() { self.settings.upscaling.label().into() }
        else { "Bilinear (no upscale needed)".into() }
    }

    /// Draws the picture itself into the back buffer (`direct`), or returns false for SDL to draw
    /// the video texture.
    #[cfg(windows)]
    fn draw_direct(&mut self, renderer: *mut SDL_Renderer, dst: &SDL_FRect) -> bool {
        self.texture.draw_direct(renderer, dst)
    }

    #[cfg(not(windows))]
    fn draw_direct(&mut self, _renderer: *mut SDL_Renderer, _dst: &SDL_FRect) -> bool {
        false
    }

    /// Draws the texture and the overlay and presents. `probe` reads the beam around the present.
    fn render_flip(&mut self, renderer: *mut SDL_Renderer, draw_overlay: &mut impl FnMut(&mut Self, *mut SDL_Renderer), probe: bool) -> Flipped {
        // SAFETY: drawing on a live renderer.
        unsafe {
            SDL_SetRenderDrawColor(renderer, 0, 0, 0, 255);
            SDL_RenderClear(renderer);
            let (mut w, mut h) = (0, 0);
            SDL_GetCurrentRenderOutputSize(renderer, &mut w, &mut h);
            let dst = viewport(self.texture.size(), (w, h), self.settings.video_scaling, self.settings.align_video_top);
            if !self.texture.texture.is_null() {
                let mut texture = self.texture.texture;
                let output = (dst.w as i32, dst.h as i32);
                let input = self.texture.size();
                let use_upscaler = self.settings.upscaling.is_shader() && !self.upscale_failed
                    && !self.texture.is_hdr() && output.0 >= input.0 && output.1 >= input.1 && output != input
                    && output.0 <= 16384 && output.1 <= 16384;
                // The upscalers and AI enhancement read the video texture.
                if self.settings.upscaling == config::Upscaling::Ai || use_upscaler { self.texture.materialize(); }
                if self.settings.upscaling == config::Upscaling::Ai {
                    self.upscaler = None;
                    if let Some(enhanced) = self.enhanced(renderer, output) { texture = enhanced; }
                } else if use_upscaler {
                    if !self.upscaler.as_ref().is_some_and(|u| u.matches(input, output, self.settings.upscaling)) {
                        self.upscaler = upscale::Upscaler::new(renderer, input, output, self.settings.upscaling);
                    }
                    match self.upscaler.as_ref().and_then(|u| u.draw(renderer, texture, self.settings.upscale_sharpness)) {
                        Some(upscaled) => texture = upscaled,
                        None => {
                            self.upscaler = None;
                            self.upscale_failed = true;
                            self.show_toast(tr("GPU upscaling is unavailable; using bilinear scaling."));
                        }
                    }
                } else { self.upscaler = None; }
                if !(texture == self.texture.texture && self.draw_direct(renderer, &dst)) {
                    if texture == self.texture.texture { self.texture.materialize(); }
                    SDL_RenderTexture(renderer, texture, std::ptr::null(), &dst);
                }
            } else if !self.texture.draw(renderer, &dst) {
                self.exit = Some(Exit::Ended(Some("Packed full-chroma presentation failed.".into())));
            }
        }
        draw_overlay(self, renderer);
        // The display probe reads the scan line around the present (nothing with the beam race off,
        // no VRR check and no VRR sync on the scan).
        if let (Some(topic), true, true) = (display_probe(self.facts.beam_race, &self.vrr), probe, self.race_probe.is_none()) {
            self.race_probe = Some(race_probe::RaceProbe::begin(renderer, self.pacer.mode != PacingMode::Vsync, f64::from(self.facts.fps), topic));
        }
        if probe { self.wait_out_scan(); }
        let beam_before = self.race_probe.as_ref().filter(|_| probe).and_then(race_probe::RaceProbe::before);
        let issued_us = session::now_us();
        // SAFETY: a live renderer.
        unsafe { SDL_RenderPresent(renderer) };
        let returned_us = session::now_us();
        let mode_tears = self.pacer.mode != PacingMode::Vsync;
        let fps = f64::from(self.facts.fps);
        let scan = self.race_probe.as_mut().filter(|_| probe).map(|probe| {
            probe.after_present(renderer, beam_before, mode_tears, fps);
            probe.report(beam_before)
        });
        Flipped { issued_us, returned_us, scan }
    }

    /// Shows the due picture, if any. Returns whether something was drawn.
    pub fn present(
        &mut self,
        renderer: *mut SDL_Renderer,
        mut draw_overlay: impl FnMut(&mut Self, *mut SDL_Renderer),
    ) -> bool {
        if self.texture.is_hdr() && !crate::sdl::hdr_available(renderer) {
            self.texture.destroy();
            self.exit = Some(Exit::Ended(Some("HDR presentation stopped because the current display/swapchain no longer supports HDR.".into())));
            self.needs_redraw = true;
        }
        self.sync_race(renderer);
        self.sync_vrr(renderer);
        // The slices of a frame still being received go out ahead of the beam.
        let sliced = self.present_slices(renderer, &mut draw_overlay, session::now_us());
        let now = session::now_us();
        let (picture, mut dropped) = self.pacer.take_due(now);
        // A newer frame's slices are on the screen: this older picture would step it back a frame.
        let picture = picture.filter(|p| {
            let older = self.racer.newer_on_screen(p.decoded.race_seq);
            dropped += u32::from(older);
            !older
        });
        self.window.dropped_pacing += dropped;
        self.shared.counters.replaced.fetch_add(u64::from(dropped), Ordering::Relaxed);
        let has_picture = picture.is_some();
        if !has_picture && !self.needs_redraw {
            return sliced;
        }
        let begin = session::now_us();
        // Whether the frame is under the racer, and which rows of it are left to draw.
        let mut raced = false;
        let mut rows = None;
        if let Some(p) = &picture {
            if self.facts.phase_lock && self.pacer.mode == PacingMode::Vsync && self.focused && !self.menu_visible {
                let timing = phase::timing(renderer, self.plan.display.refresh_hz);
                if let Some(report) = self.phase.sample(p.decoded.ready_us, begin, timing) {
                    let _ = self.session.send(ClientMessage::PhaseLockReport(report));
                }
            } else { self.phase.reset(); }
            if let (PacingMode::BeamRace, Some(scan), true) = (self.pacer.mode, self.race_scan, p.decoded.race_seq != 0) {
                raced = true;
                let first_row = p.decoded.bands.last().map_or(0, |band| band.first_row);
                if let race::Final::Rows(first, end) = self.racer.finish(p.decoded.race_seq, first_row, begin, &scan, &self.latency) {
                    rows = race::scissor(first, end, p.decoded.setup.height as u32);
                }
            }
            if !self.texture.upload(renderer, &p.decoded, rows) {
                if p.decoded.setup.format & VIDEO_FORMAT_MASK_PYROWAVE != 0 {
                    let reason = self.texture.failure.take().unwrap_or_else(|| "PyroWave GPU reconstruction failed.".into());
                    self.exit = Some(Exit::Ended(Some(reason)));
                } else if p.decoded.setup.chroma_packing != 0 {
                    self.exit = Some(Exit::Ended(Some("Packed full-chroma reconstruction failed: the decoded planes, GPU, or HDR output are unsupported.".into())));
                } else {
                    let format = p.decoded.picture.format_name();
                    let toast = match self.texture.failure.take() {
                        Some(reason) => trf("Cannot show {} pictures: {}", &[&format, &reason]),
                        None => trf("Cannot show {} pictures", &[&format]),
                    };
                    self.show_toast(toast);
                }
            }
            if !self.first_frame {
                self.first_frame = true;
            }
        }
        let flipped = self.render_flip(renderer, &mut draw_overlay, has_picture);
        let presented = flipped.returned_us;
        let scan = flipped.scan;
        let flip = if self.timing_csv.is_some() && picture.is_some() { phase::present_stats(renderer) } else { Default::default() };
        self.needs_redraw = false;
        if let Some(p) = picture {
            let timeline = self.finish_frame(&p.decoded, raced, flipped.issued_us, flipped.returned_us);
            let w = &mut self.window;
            w.shown += 1;
            w.render_us += presented - begin;
            w.hold_us += p.hold_us;
            w.late += u32::from(p.late);
            w.network_us += p.decoded.enqueue_us.saturating_sub(p.decoded.receive_us);
            w.network_n += 1;
            self.doctor.shown(p.decoded.host_latency, p.decoded.ready_us, presented);
            if let Some(csv) = &self.timing_csv {
                let d = &p.decoded;
                csv.row(crate::timing_csv::Row {
                    number: d.number,
                    rtp: d.rtp,
                    receive_us: d.receive_us,
                    enqueue_us: d.enqueue_us,
                    input_us: d.input_us,
                    output_us: d.ready_us,
                    render_us: presented,
                    host_latency: d.host_latency,
                    drops: self.shared.counters.replaced.load(Ordering::Relaxed),
                    clock: self.offset.offset_at(d.enqueue_us as i64).zip(self.offset.rtt_us()),
                    early: d.early,
                    first_block_us: d.first_block_us,
                    race: d.race,
                    scan: scan.unwrap_or_default(),
                    presented: timeline,
                    due_us: p.present_us,
                    begin_us: begin,
                    flip,
                });
            }
            let host_us = self.rtp_clock.host_us(p.decoded.rtp);
            if let Some(offset) = self.offset.offset_at(presented as i64) {
                let latency = presented as i64 - (host_us + offset);
                if (0..2_000_000).contains(&latency) {
                    w.latency_us += latency;
                    w.latency_n += 1;
                }
            }
        }
        true
    }

    /// A frame is on the screen: closes it in the racer, teaches the predictor its slice times and
    /// gives the timing CSV what the beam race columns need (empty with the race off).
    fn finish_frame(&mut self, decoded: &pipeline::Decoded, raced: bool, issued_us: u64, returned_us: u64) -> crate::timing_csv::Presented {
        let mut timeline = crate::timing_csv::Presented::default();
        if !self.facts.beam_race { return timeline; }
        if decoded.bands.len() >= 2 {
            let facts = &self.facts;
            self.predictor.get_or_insert_with(|| seeded_predictor(facts)).learn(&decoded.bands);
        }
        let mut presents = vec![issued_us];
        // The picture's own present costs what a slice's does, whether or not the frame was raced.
        self.latency.record_call(issued_us, returned_us, returned_us);
        if let (true, Some(scan)) = (raced, self.race_scan) {
            // Only the frame the racer follows: a picture of an older frame closes nothing.
            if let Some(record) = self.racer.close(decoded.race_seq, issued_us, &scan) {
                timeline.late_slices = Some(record.late);
                presents = record.presents_us;
            }
        }
        timeline.set_presents(&presents);
        if let Some(scan) = self.race_scan {
            let first = presents.first().copied().unwrap_or(issued_us);
            timeline.scan_start_us = scan.reaches_us(first, 0);
            timeline.mid_row_age_ns = race::mid_row_age_ns(&scan, first, &decoded.bands);
        }
        timeline
    }

    /// The overlay's value for the raced presenter: what it did, and why it did not when it did not.
    fn raced_line(&self) -> String {
        let totals = self.racer.totals;
        match self.pacer.mode {
            PacingMode::BeamRace => {
                let mut line = format!("{} raced, {} whole, {} late slices in {} frames", totals.raced, totals.whole, totals.late_slices, totals.late_frames);
                if totals.abandoned > 0 { line.push_str(&format!(", {} never finished", totals.abandoned)); }
                if let Some(skip) = totals.last_skip { line.push_str(&format!("; last whole: {}", skip.label())); }
                line
            }
            _ if self.race_probe.as_ref().is_some_and(|probe| probe.racing().is_some()) => format!("off, pacing is {}", self.pacer.mode.label()),
            _ => "off until the display check passes".into(),
        }
    }

    /// Recomputes the overlay text once a second. Returns whether it changed.
    pub fn update_overlay(&mut self) -> bool {
        let started = *self.window.started.get_or_insert_with(Instant::now);
        let elapsed = started.elapsed();
        if elapsed < Duration::from_secs(1) {
            return false;
        }
        let secs = elapsed.as_secs_f64();
        let stats = self.session.stats();
        let c = &self.shared.counters;
        let now = (
            stats.frames_submitted,
            c.decoded.load(Ordering::Relaxed),
            c.decode_us_total.load(Ordering::Relaxed),
            c.decode_errors.load(Ordering::Relaxed),
            c.audio_dropped.load(Ordering::Relaxed),
            c.audio_underruns.load(Ordering::Relaxed),
            c.audio_concealed.load(Ordering::Relaxed),
        );
        let last = self.last_counters;
        self.last_counters = now;
        let received = now.0 - last.0;
        let decoded = now.1 - last.1;
        let decode_ms = if decoded > 0 {
            (now.2 - last.2) as f64 / decoded as f64 / 1000.0
        } else {
            0.0
        };
        let video = (stats.video.frames_completed, stats.video.frame_drops);
        let (completed, drops) = (video.0 - self.last_video.0, video.1 - self.last_video.1);
        self.last_video = video;
        let loss = if completed + drops > 0 {
            drops as f64 * 100.0 / (completed + drops) as f64
        } else {
            0.0
        };
        let mut w = std::mem::take(&mut self.window);
        (w.host_latency, w.host_latency_n) = self.host_average.update(
            c.host_latency_total.load(Ordering::Relaxed), c.host_latency_frames.load(Ordering::Relaxed));
        let avg = |sum: f64, n: u32| if n > 0 { sum / f64::from(n) } else { 0.0 };
        let measured = |sum: f64, n: u32| if n > 0 { format!("{:.3}", sum / f64::from(n) / 1000.0) } else { String::new() };
        self.statistics.push_back(format!("{},{:.3},{:.3},{:.3},{loss:.3},{},{decode_ms:.3},{},{},{},{}\n",
            self.started.elapsed().as_millis(), received as f64 / secs, decoded as f64 / secs, f64::from(w.shown) / secs,
            stats.rtt.map_or_else(String::new, |(rtt, _)| format!("{:.3}", rtt.as_secs_f64() * 1000.0)),
            measured(w.render_us as f64, w.shown), measured(w.timing_host_us as f64, w.timing_n),
            measured(w.latency_us as f64, w.latency_n), self.facts.bitrate_kbps));
        if self.statistics.len() > 3600 { self.statistics.pop_front(); }
        let mut lines = Vec::new();
        let f = &self.facts;
        lines.push((
            "Stream".into(),
            format!("{}x{} {} fps, {}", f.width, f.height, f.fps, codec_name(f.format)),
        ));
        lines.push(("Decoder".into(), lock(&self.shared.decoder_name).clone()));
        if self.settings.upscaling != config::Upscaling::Bilinear {
            lines.push(("Upscaling".into(), self.upscaling_line()));
        }
        if self.quality_mode != QualityMode::Off { lines.push(("Quality".into(), self.quality_mode.label().into())); }
        lines.push((
            "Frames".into(),
            format!(
                "{:.0} in, {:.0} decoded, {:.0} shown",
                received as f64 / secs,
                decoded as f64 / secs,
                f64::from(w.shown) / secs
            ),
        ));
        lines.push((
            "Network".into(),
            match stats.rtt {
                Some((rtt, var)) => format!(
                    "{:.1} ms RTT (±{:.1}), {:.1}% frames lost",
                    rtt.as_secs_f64() * 1000.0,
                    var.as_secs_f64() * 1000.0,
                    loss
                ),
                None => format!("{loss:.1}% frames lost"),
            },
        ));
        if w.network_n > 0 && w.network_us / u64::from(w.network_n) >= 50 {
            lines.push((
                "Receive".into(),
                format!("{:.1} ms first packet to frame", avg(w.network_us as f64, w.network_n) / 1000.0),
            ));
        }
        lines.push((
            "Decode".into(),
            format!("{decode_ms:.2} ms{}", if now.3 > last.3 { ", errors" } else { "" }),
        ));
        if self.facts.early_without_records {
            lines.push(("Early decode".into(), "off, the host sent no PyroWave records".into()));
        }
        if self.facts.early_decode {
            let early = (c.early_frames.load(Ordering::Relaxed), c.early_fallbacks.load(Ordering::Relaxed));
            let last_early = std::mem::replace(&mut self.last_early, early);
            lines.push(("Early decode".into(), early_line(early, last_early, secs)));
        }
        if let Some(reason) = self.facts.beam_race_off {
            lines.push(("Beam race".into(), reason.into()));
        }
        if let (true, Some(probe)) = (self.facts.beam_race, &self.race_probe) {
            lines.push(("Beam race".into(), probe.line()));
        }
        if self.facts.beam_race {
            lines.push(("Race slices".into(), race_line(c.race_frames.load(Ordering::Relaxed), c.race_slices.load(Ordering::Relaxed),
                c.race_unordered.load(Ordering::Relaxed))));
        }
        if self.facts.beam_race {
            lines.push(("Raced presenter".into(), self.raced_line()));
        }
        lines.push((
            "Render".into(),
            format!("{:.2} ms", avg(w.render_us as f64, w.shown) / 1000.0),
        ));
        let mut pacing = pacing_label(self.pacer.mode, self.vrr.fell_back).to_owned();
        if self.vrr.sync != present::VrrSync::Off && self.pacer.mode != PacingMode::Vsync {
            pacing.push_str(&format!(", VRR sync {}", self.vrr.sync.label()));
            if self.scan_waits.0 > 0 {
                pacing.push_str(&format!(" ({} waits, {:.2} ms each)", self.scan_waits.0, self.scan_waits.1 as f64 / f64::from(self.scan_waits.0) / 1e6));
            }
        }
        if self.pacer.mode == PacingMode::HostTimes {
            if let Some(delay) = self.pacer.clock.delay_us() {
                pacing.push_str(&format!(
                    ", hold {:.1} ms, {} late",
                    avg(w.hold_us as f64, w.shown) / 1000.0,
                    w.late
                ));
                let _ = delay;
            }
        }
        if w.dropped_pacing > 0 {
            pacing.push_str(&format!(", {} replaced", w.dropped_pacing));
        }
        lines.push(("Pacing".into(), pacing));
        if self.facts.phase_lock && self.pacer.mode == PacingMode::Vsync {
            lines.push(("Phase lock".into(), self.phase.lead_us.map_or_else(|| "Waiting for display timing".into(), |lead| format!("{:.2} ms lead", f64::from(lead) / 1000.0))));
        }
        lines.extend(host_lines(&w));
        if w.latency_n > 0 {
            lines.push((
                "Latency".into(),
                format!(
                    "{:.1} ms PC frame to display",
                    avg(w.latency_us as f64, w.latency_n) / 1000.0
                ),
            ));
        }
        let channels = c.audio_channels.load(Ordering::Relaxed);
        let mut audio = match channels {
            0 => "no output device".to_owned(),
            2 => "stereo".to_owned(),
            6 => "5.1".to_owned(),
            8 => "7.1".to_owned(),
            n => format!("{n} channels"),
        };
        if channels > 0 {
            audio.push_str(&format!(
                ", {:.0} ms buffered",
                c.audio_queued_us.load(Ordering::Relaxed) as f64 / 1000.0
            ));
        }
        if now.4 > last.4 {
            audio.push_str(&format!(", {} packets dropped to stay in sync", now.4 - last.4));
        }
        if now.5 > last.5 {
            audio.push_str(&format!(", {} underruns", now.5 - last.5));
        }
        if now.6 > last.6 {
            audio.push_str(&format!(", {} packets concealed", now.6 - last.6));
        }
        lines.push(("Audio".into(), audio));
        lines.push(("Bitrate".into(), format!("{:.1} Mbps{}", f64::from(self.facts.bitrate_kbps) / 1000.0,
            if self.adaptive_bitrate || self.client_hint_active() { ", automatic" } else { "" })));
        if let Some(report) = &self.pipeline_rates {
            lines.push(("Host pipeline".into(), pipeline_rates_line(report)));
        }
        if let Some(notice) = &self.link_notice {
            lines.push(("Notice".into(), link_notice_line(notice)));
        }
        let video_bytes = c.video_bytes.load(Ordering::Relaxed);
        let wire_bytes = self.session.wire_bytes();
        let measures = overlay::Measures {
            rendered_fps: (w.shown > 0 || received > 0).then(|| (f64::from(w.shown) / secs) as f32),
            received_fps: (received > 0).then(|| (received as f64 / secs) as f32),
            video_mbps: (received > 0).then(|| overlay::video_mbps(video_bytes.saturating_sub(self.last_video_bytes), secs)).flatten(),
            wire_mbps: (wire_bytes > self.last_wire_bytes)
                .then(|| overlay::video_mbps(wire_bytes - self.last_wire_bytes, secs))
                .flatten(),
            loss_percent: (completed + drops > 0).then_some(loss as f32),
            network_ms: stats.rtt.map(|(rtt, _)| (rtt.as_secs_f64() * 1000.0) as f32),
            variance_ms: stats.rtt.map(|(_, var)| (var.as_secs_f64() * 1000.0) as f32),
            host_ms: (w.host_latency_n > 0).then(|| (avg(w.host_latency as f64, w.host_latency_n) / 10.0) as f32),
            host_to_network_ms: (w.timing_n > 0).then(|| (avg(w.timing_host_us as f64, w.timing_n) / 1000.0) as f32),
            decode_ms: (decoded > 0).then_some(decode_ms as f32),
            render_ms: (w.shown > 0).then(|| (avg(w.render_us as f64, w.shown) / 1000.0) as f32),
        };
        self.last_video_bytes = video_bytes;
        self.last_wire_bytes = wire_bytes;
        lines.extend(overlay::wire(&measures));
        let mut statistics = overlay::statistics(&lines[0].1, &measures);
        let now_ms = self.started.elapsed().as_millis() as u64;
        let wanted = self.overlay_visible && self.overlay_mode != OverlayMode::Compact && self.facts.role != AppRole::InputOnly;
        if let Some(generation) = self.hardware_poll.due(now_ms, wanted) {
            self.request_hardware(generation);
        }
        if !self.hardware_poll.fresh(now_ms) {
            self.hardware = None;
        }
        if let Some(metrics) = &self.hardware {
            statistics.extend(overlay::hardware(metrics, false));
            lines.extend(overlay::hardware(metrics, true));
        }
        self.overlay = Overlay {
            lines,
            statistics,
            compact: overlay::compact(&measures),
            notice: self.link_notice.as_ref().map(link_notice_line),
        };
        let timing = std::mem::take(&mut *lock(&self.shared.decode_window));
        let now_us = session::now_us();
        // The window since the last tick, or since the last requested mode, which reset it.
        let decode_timing = quality::decode_timing(self.decode_generation, self.decode_window_us, now_us, self.facts.fps.max(0) as u32, &timing);
        self.decode_window_us = now_us;
        let mut tick = Stats {
            now_ms: self.started.elapsed().as_millis() as u64,
            recent_video: decoded > 0 && self.last_arrival.is_some_and(|(_, at)| now_us.saturating_sub(at) < 500_000),
            poor_connection: stats.connection_status == Some(nightjar_core::control::ConnectionStatus::Poor),
            fps: self.facts.fps.max(0) as u32,
            loss_percent: (completed + drops > 0).then_some(loss as f32),
            decode_ms: (decoded > 0).then_some(decode_ms as f32),
            jitter_ms: (self.jitter_ms >= 0.0).then_some(self.jitter_ms as f32),
            queue_ms: (timing.samples > 0).then(|| (timing.queue_us as f64 / f64::from(timing.samples) / 1000.0) as f32),
            received_fps: Some((received as f64 / secs) as f32),
            released_fps: Some((decoded as f64 / secs) as f32),
            shown_fps: Some((f64::from(w.shown) / secs) as f32),
            ..Default::default()
        };
        self.doctor.timings(&mut tick);
        let client = self.host_abr.is_some().then(|| lock(&self.shared.host_client).clone()).flatten();
        if let (Some(abr), Some(client)) = (&mut self.host_abr, client) {
            abr.add(completed, drops, decoded);
            let rtt_ms = stats.rtt.map(|(rtt, _)| rtt.as_secs_f64() * 1000.0);
            if let Some(feedback) = abr.due(tick.now_ms, rtt_ms) {
                let shared = self.shared.clone();
                std::thread::spawn(move || {
                    let action = client.abr_feedback(&feedback)
                        .map_err(|e| eprintln!("nightjar: the PC's automatic bitrate report failed: {e}")).ok();
                    shared.notify(Notice::HostAbrAction(action));
                });
            }
        }
        // As on Android: no samples for input-only, or while a manual bitrate change or trial runs.
        if self.facts.role != AppRole::InputOnly && !(self.bitrate_busy && !self.adaptive_bitrate) {
            self.doctor.add(&tick);
        }
        let quality_target = if let Some(policy) = &mut self.quality_policy {
            if self.menu_visible || !self.focused || self.pending_mode.is_some() { policy.suspend(); None }
            else {
                let has_room = self.bitrate_busy || self.bitrate_policy.as_ref().is_some_and(|p| p.has_room())
                    || self.rate_change.as_ref().is_some_and(|rates| rates.hint_has_room());
                policy.sample(&tick, has_room, decode_timing.as_ref())
            }
        } else { None };
        if let Some(target) = quality_target {
            let applied = if self.request_mode(target) {
                self.quality_policy.as_mut().expect("active quality policy").applied(target, tick.now_ms)
                    .map_err(|error| eprintln!("nightjar: automatic quality: {target:?} not applied: {error}")).is_ok()
            } else { false };
            if !applied { self.quality_policy = None; self.quality_mode = QualityMode::Off; }
        }
        if !self.bitrate_busy {
            if let Some(policy) = &mut self.bitrate_policy {
                if self.menu_visible || !self.focused {
                    policy.suspend();
                } else {
                    let target = if self.facts.format & VIDEO_FORMAT_MASK_PYROWAVE != 0 {
                        // PyroWave also loses frames that are recovered without their records.
                        let record_loss = (timing.record_samples > 0)
                            .then(|| (timing.record_loss_milli as f64 / f64::from(timing.record_samples) / 1000.0) as f32);
                        let loss_percent = match (tick.loss_percent, record_loss) {
                            (Some(a), Some(b)) => Some(a.max(b)),
                            (a, b) => a.or(b),
                        };
                        policy.sample(&Stats { loss_percent, ..tick })
                    } else { policy.sample(&tick) };
                    if let Some(target) = target {
                        self.bitrate_kbps = target;
                        self.set_bitrate();
                    }
                }
            }
        }
        if let Some(toast) = self.rate_change.as_mut().and_then(|rates| rates.expire(self.started.elapsed().as_millis() as u64)) {
            self.show_toast(toast);
        }
        if !self.menu_visible && self.focused {
            if let Some(rates) = &mut self.rate_change {
                // PyroWave also loses records within frames, as for the PC's adaptive bitrate.
                let record_loss = (self.facts.format & VIDEO_FORMAT_MASK_PYROWAVE != 0 && timing.record_samples > 0)
                    .then(|| (timing.record_loss_milli as f64 / f64::from(timing.record_samples) / 1000.0) as f32);
                let loss_percent = match (tick.loss_percent, record_loss) { (Some(a), Some(b)) => Some(a.max(b)), (a, b) => a.or(b) };
                let request = rates.tick(&Stats {
                    now_ms: tick.now_ms,
                    recent_video: tick.recent_video,
                    poor_connection: tick.poor_connection,
                    loss_percent,
                    queue_ms: tick.queue_ms,
                    ..Default::default()
                });
                if let Some(request) = request { self.send_rate_change(request); }
            }
        }
        self.window.started = Some(Instant::now());
        true
    }

    /// The overlay as the user left it, once it differs from what the settings keep: the stream
    /// menu and the shortcut change the global setting too, as on Android.
    pub fn take_overlay_change(&mut self) -> Option<(bool, OverlayMode, OverlayCorner)> {
        let now = (self.overlay_visible, self.overlay_mode, self.overlay_corner);
        (now != self.overlay_kept).then(|| { self.overlay_kept = now; now })
    }

    /// Whether the stream has shown its first picture.
    pub fn has_picture(&self) -> bool {
        self.first_frame
    }

    /// The app this stream started or joined.
    pub fn app(&self) -> &App {
        &self.plan.app
    }

    /// Input-only through a small ordinary stream: video and audio arrive and are discarded.
    pub fn small_stream_input_only(&self) -> bool {
        self.plan.small_stream_input_only
    }

    pub fn since_start(&self) -> Duration {
        self.started.elapsed()
    }

    /// Handles one SDL event while streaming.
    pub fn handle(
        &mut self,
        event: &SDL_Event,
        window: *mut SDL_Window,
        renderer: *mut SDL_Renderer,
        pads: &mut Gamepads,
    ) {
        // SAFETY: union members read by event type.
        unsafe {
            let kind = SDL_EventType(event.r#type);
            if self.touchpad_swallows(event) {
                return;
            }
            match kind {
                SDL_EVENT_QUIT | SDL_EVENT_WINDOW_CLOSE_REQUESTED => {
                    self.exit = Some(Exit::CloseWindow);
                }
                SDL_EVENT_RENDER_DEVICE_RESET | SDL_EVENT_RENDER_DEVICE_LOST => {
                    let reason = render_device_ended(kind).expect("a render device event");
                    eprintln!("nightjar: {reason}");
                    self.exit.get_or_insert(Exit::Ended(Some(reason.into())));
                }
                SDL_EVENT_KEY_DOWN | SDL_EVENT_KEY_UP => self.key(&event.key, window, pads),
                SDL_EVENT_TEXT_INPUT if !event.text.text.is_null() => {
                    let text = std::ffi::CStr::from_ptr(event.text.text).to_string_lossy();
                    self.text(&text);
                }
                SDL_EVENT_KEYMAP_CHANGED => self.report_keyboard_layout(),
                // What this app put there came from the PC or its own controls.
                SDL_EVENT_CLIPBOARD_UPDATE if !event.clipboard.owner => {
                    if let Some(step) = self.clipboard_sync.as_mut().map(|sync| sync.changed(Instant::now())) {
                        self.clipboard_sync_step(step);
                    }
                }
                SDL_EVENT_WINDOW_FOCUS_LOST => {
                    self.focused = false;
                    self.raise_all_keys();
                    if !self.settings.background_controllers {
                        self.session.input(|q| pads.release(q));
                    }
                    if self.captured {
                        self.set_capture(window, false);
                        self.captured = true; // recapture on focus
                    }
                }
                SDL_EVENT_WINDOW_FOCUS_GAINED => {
                    self.focused = true;
                    if self.captured && !self.menu_visible {
                        self.set_capture(window, true);
                    }
                }
                SDL_EVENT_MOUSE_MOTION => {
                    let m = event.motion;
                    if self.facts.touch && self.captured && matches!(m.which, SDL_TOUCH_MOUSEID | SDL_PEN_MOUSEID) { return; }
                    if !self.captured {
                        return;
                    }
                    if self.absolute {
                        self.absolute_position(m.x, m.y, window, renderer);
                    } else {
                        let (rx, ry) = self.relative_remainder;
                        let speed = self.settings.mouse_speed.clamp(25, 400) as f32 / 100.0;
                        let (dx, dy) = (m.xrel * speed + rx, m.yrel * speed + ry);
                        let (ix, iy) = (dx.trunc(), dy.trunc());
                        self.relative_remainder = (dx - ix, dy - iy);
                        if ix != 0.0 || iy != 0.0 {
                            self.session.input(|q| {
                                let _ = q.send_mouse_move(ix as i16, iy as i16);
                            });
                        }
                    }
                }
                SDL_EVENT_MOUSE_BUTTON_DOWN | SDL_EVENT_MOUSE_BUTTON_UP => {
                    let b = event.button;
                    if self.facts.touch && self.captured && matches!(b.which, SDL_TOUCH_MOUSEID | SDL_PEN_MOUSEID) { return; }
                    if !self.captured {
                        // The first click only takes the pointer.
                        if b.down {
                            self.set_capture(window, true);
                            if self.absolute {
                                self.absolute_position(b.x, b.y, window, renderer);
                            }
                        }
                        return;
                    }
                    let button = match i32::from(b.button) {
                        SDL_BUTTON_LEFT => mouse_button::LEFT,
                        SDL_BUTTON_MIDDLE => mouse_button::MIDDLE,
                        SDL_BUTTON_RIGHT => mouse_button::RIGHT,
                        SDL_BUTTON_X1 if self.settings.mouse_nav_buttons => mouse_button::X1,
                        SDL_BUTTON_X2 if self.settings.mouse_nav_buttons => mouse_button::X2,
                        _ => return,
                    };
                    if b.down {
                        self.mouse_down.insert(button);
                    } else {
                        self.mouse_down.remove(&button);
                    }
                    self.session.input(|q| {
                        let _ = q.send_mouse_button(button, b.down);
                    });
                }
                SDL_EVENT_MOUSE_WHEEL => {
                    if !self.captured {
                        return;
                    }
                    let w = event.wheel;
                    let flip = if w.direction == SDL_MOUSEWHEEL_FLIPPED { -1.0 } else { 1.0 };
                    let (x, y) = (w.x * flip, w.y * flip);
                    self.session.input(|q| {
                        if y != 0.0 {
                            let _ = q.send_high_res_scroll((y * 120.0).clamp(-32000.0, 32000.0) as i16);
                        }
                        if x != 0.0 {
                            let _ = q.send_high_res_hscroll((x * 120.0).clamp(-32000.0, 32000.0) as i16);
                        }
                    });
                }
                SDL_EVENT_FINGER_DOWN | SDL_EVENT_FINGER_MOTION | SDL_EVENT_FINGER_UP | SDL_EVENT_FINGER_CANCELED
                | SDL_EVENT_PEN_MOTION | SDL_EVENT_PEN_DOWN | SDL_EVENT_PEN_UP | SDL_EVENT_PEN_AXIS
                | SDL_EVENT_PEN_BUTTON_DOWN | SDL_EVENT_PEN_BUTTON_UP | SDL_EVENT_PEN_PROXIMITY_OUT => {
                    if !self.facts.touch || !self.captured || !self.focused { return; }
                    let mut area = (0, 0);
                    SDL_GetCurrentRenderOutputSize(renderer, &mut area.0, &mut area.1);
                    let picture = if self.facts.role == AppRole::InputOnly { area } else { self.texture.size() };
                    let density = SDL_GetWindowPixelDensity(window).max(0.01);
                    let rect = viewport(picture, area, self.settings.video_scaling, self.settings.align_video_top);
                    self.session.input(|q| self.touch.handle(event, rect, area, density, q));
                }
                SDL_EVENT_GAMEPAD_ADDED => {
                    if let Err(why) = pads.added(event.gdevice.which) {
                        self.show_toast(why.message());
                    }
                    if self.controller_audio_available() { pads.auto_audio(); }
                    if self.sends_controllers() {
                        self.session.input(|q| pads.announce_new(q));
                    }
                }
                SDL_EVENT_GAMEPAD_REMOVED => {
                    self.session.input(|q| pads.release_mouse(event.gdevice.which, q));
                    if let Some(index) = pads.removed(event.gdevice.which) {
                        if self.sends_controllers() {
                            self.session.input(|q| pads.send_removed(index, q));
                        }
                    }
                }
                SDL_EVENT_AUDIO_DEVICE_ADDED => {
                    if self.controller_audio_available() {
                        pads.auto_audio();
                        self.session.input(|q| pads.announce_new(q));
                    }
                }
                SDL_EVENT_AUDIO_DEVICE_REMOVED => {
                    pads.removed_audio_device(event.adevice.which);
                    if self.sends_controllers() {
                        self.session.input(|q| pads.announce_new(q));
                    }
                }
                _ => {
                    let focused = SDL_GetWindowFlags(window) & SDL_WINDOW_INPUT_FOCUS != 0;
                    if self.facts.role == AppRole::RemoteMonitor || !self.settings.controllers || (!focused && !self.settings.background_controllers) {
                        return;
                    }
                    let mut action = None;
                    self.session.input(|q| action = pads.handle(event, q));
                    match action {
                        Some(Action::Quit) => self.exit = Some(Exit::User),
                        Some(Action::Menu) => self.toggle_menu(window, pads),
                        Some(Action::Mouse(active)) => self.show_toast(menu_extras::mouse_toast(pads, active, true)),
                        None => {}
                    }
                }
            }
        }
    }

    fn absolute_position(&self, x: f32, y: f32, window: *mut SDL_Window, renderer: *mut SDL_Renderer) {
        // SAFETY: live window and renderer.
        let (density, (w, h)) = unsafe {
            let mut size = (0, 0);
            SDL_GetCurrentRenderOutputSize(renderer, &mut size.0, &mut size.1);
            (SDL_GetWindowPixelDensity(window).max(0.01), size)
        };
        let picture = if self.facts.role == AppRole::InputOnly { (w, h) } else { self.texture.size() };
        let rect = viewport(picture, (w, h), self.settings.video_scaling, self.settings.align_video_top);
        let (px, py) = (x * density - rect.x, y * density - rect.y);
        let (rw, rh) = (rect.w.max(1.0), rect.h.max(1.0));
        let cx = px.clamp(0.0, rw - 1.0);
        let cy = py.clamp(0.0, rh - 1.0);
        let (sw, sh) = (rw.min(32767.0), rh.min(32767.0));
        self.session.input(|q| {
            let _ = q.send_mouse_position(
                (cx * sw / rw) as i16,
                (cy * sh / rh) as i16,
                sw as i16,
                sh as i16,
            );
        });
    }

    fn key(&mut self, k: &SDL_KeyboardEvent, window: *mut SDL_Window, pads: &mut Gamepads) {
        if k.repeat {
            // The PC repeats a held key itself; SDL's repeated text is that key's too.
            let held = key_input(k.scancode, k.r#mod, self.settings.capture_system_keys, self.settings.right_alt_as_meta)
                .is_some_and(|key| self.keys_down.contains(&key.code));
            if k.down && held {
                self.text_field.key_down(text_field::types(k.key, k.r#mod));
            }
            return;
        }
        if k.down && local_chord(k.r#mod) {
            match k.scancode {
                SDL_SCANCODE_TAB => {
                    self.toggle_menu(window, pads);
                    return;
                }
                SDL_SCANCODE_C => {
                    self.clipboard(false);
                    return;
                }
                SDL_SCANCODE_V => {
                    self.clipboard(true);
                    return;
                }
                SDL_SCANCODE_Q => {
                    self.exit = Some(Exit::User);
                    return;
                }
                SDL_SCANCODE_Z => {
                    let on = !self.captured;
                    self.raise_all_keys();
                    self.set_capture(window, on);
                    self.show_toast(if on {
                        tr("Input captured. Ctrl+Alt+Shift+Z releases it.")
                    } else {
                        tr("Input released. Click the stream to capture it.")
                    });
                    return;
                }
                SDL_SCANCODE_S => {
                    self.overlay_visible = !self.overlay_visible;
                    self.needs_redraw = true;
                    return;
                }
                SDL_SCANCODE_M => {
                    self.absolute = !self.absolute;
                    let on = self.captured;
                    self.set_capture(window, false);
                    self.set_capture(window, on);
                    self.show_toast(if self.absolute {
                        tr("Mouse: desktop pointer")
                    } else {
                        tr("Mouse: captured for games")
                    });
                    return;
                }
                SDL_SCANCODE_X => {
                    // SAFETY: a live window.
                    unsafe {
                        let fullscreen = SDL_GetWindowFlags(window) & SDL_WINDOW_FULLSCREEN != 0;
                        SDL_SetWindowFullscreen(window, !fullscreen);
                    }
                    self.raise_all_keys();
                    return;
                }
                SDL_SCANCODE_D => {
                    self.raise_all_keys();
                    // SAFETY: a live window.
                    unsafe { SDL_MinimizeWindow(window) };
                    return;
                }
                _ => {}
            }
        }
        if !self.captured {
            return;
        }
        let Some(KeyInput { code, modifiers, flags }) =
            key_input(k.scancode, k.r#mod, self.settings.capture_system_keys, self.settings.right_alt_as_meta)
        else {
            return;
        };
        if k.down {
            self.keys_down.insert(code);
            self.text_field.key_down(text_field::types(k.key, k.r#mod));
        } else {
            self.keys_down.remove(&code);
        }
        self.session.input(|q| {
            let _ = q.send_keyboard(0x8000 | code, k.down, modifiers, flags);
        });
    }

    /// Text from SDL text input: only what a touch keyboard typed goes to the PC, as UTF-8.
    fn text(&mut self, text: &str) {
        let Some(text) = self.text_field.text(text) else { return; };
        if !self.captured {
            return;
        }
        self.session.input(|q| {
            if let Err(error) = q.send_text(text) {
                eprintln!("nightjar: text field: could not send text: {error}");
            }
        });
    }

    /// Ends the stream: releases input, stops the session.
    pub fn finish(mut self, window: *mut SDL_Window, pads: &mut Gamepads) {
        self.microphone = None;
        self.stop_host_abr();
        if let Some(client) = &*lock(&self.shared.host_client) {
            client.cancel_pending_requests();
        }
        self.raise_all_keys();
        self.session.input(|q| pads.release(q));
        self.text_field.stop(window);
        self.set_capture(window, false);
        self.cursor.clear();
        crate::sdl::alt_f4_to_host(false);
        for controller in 0..16 {
            if self.audio_status.0[usize::from(controller)] != 0 {
                let _ = self.session.send(ClientMessage::ControllerAudioStatus { controller, flags: 0 });
            }
        }
        // SAFETY: a live window.
        unsafe { SDL_ShowCursor() };
        pads.reset();
        self.upscaler = None;
        #[cfg(windows)]
        { self.enhancer = None; }
        self.texture.destroy();
        if self.facts.format & VIDEO_FORMAT_MASK_PYROWAVE != 0 {
            eprintln!("nightjar: decoder {}", lock(&self.shared.decoder_name));
        }
        if let Some(summary) = self.texture.pyrowave_stages.summary() {
            eprintln!("nightjar: PyroWave presenter stages {summary} (CPU submit time)");
        }
        let stats = self.session.stats();
        eprintln!("{}", video_summary(stats.frames_submitted, stats.decode_queue_overflows,
            self.shared.counters.replaced.load(Ordering::Relaxed)));
        if self.facts.early_decode {
            let c = &self.shared.counters;
            eprintln!("{}", early_summary(c.early_frames.load(Ordering::Relaxed), c.early_fallbacks.load(Ordering::Relaxed),
                c.early_us_total.load(Ordering::Relaxed), stats.early_chunks_dropped));
        }
        if self.facts.beam_race {
            let c = &self.shared.counters;
            eprintln!("nightjar: beam race: {}", race_line(c.race_frames.load(Ordering::Relaxed), c.race_slices.load(Ordering::Relaxed),
                c.race_unordered.load(Ordering::Relaxed)));
            eprintln!("nightjar: beam race: presenter, {}", self.raced_line());
            eprintln!("nightjar: beam race: present timing, wake {} us, call p50 {} us p95 {} us (slowest {} us), spread {} us, margin {} us ahead of the beam",
                self.latency.wake_us(), self.latency.call_us(), self.latency.call_p95_us(), self.latency.slowest_call_us(), self.latency.spread_us(),
                self.latency.margin_us(self.race_margin_ms));
            let skipped = self.racer.totals.skipped;
            eprintln!("nightjar: beam race: frames not raced: {} with under 1 ms to gain, {} not in slices, {} with the first slice missed",
                skipped[0], skipped[1], skipped[2]);
        }
        if self.vrr.sync == present::VrrSync::Scan {
            let (waits, ns) = self.scan_waits;
            eprintln!("nightjar: VRR sync scan: {waits} presents waited for a scan under way, {:.2} ms each",
                if waits > 0 { ns as f64 / f64::from(waits) / 1e6 } else { 0.0 });
        }
        lock(&self.shared.roam_watch).take();
        self.session.stop();
    }
}

/// What the stream menu says when the PC's image clipboard refused or failed (`send`: this
/// device's image to the PC).
fn image_error(error: nightjar_core::http::ImageClipboardError, send: bool) -> String {
    use nightjar_core::http::ImageClipboardError as E;
    match error {
        E::TooLarge(why) => trf("The image is too large for the PC's clipboard: {}", &[&why]),
        E::NotPng(why) if send => trf("The image on this device's clipboard is no valid PNG: {}", &[&why]),
        E::NotPng(why) => trf("The PC sent no valid PNG image: {}", &[&why]),
        E::Unsupported if send => tr("The PC could not read this image.").to_owned(),
        E::Unsupported => tr("The PC could not convert the image on its clipboard.").to_owned(),
        E::Forbidden => tr("The PC refused its image clipboard to this device. Check the device's clipboard permissions on the PC; images move only during a stream of this device.").to_owned(),
        E::Host(e) => trf("The PC's image clipboard did not answer: {}", &[&e]),
    }
}

/// Ctrl+Alt+Shift is Nightjar's own chord; every other one, a host's own menu chord such as
/// Ctrl+Alt+Backspace included, goes to the PC like any key.
fn local_chord(modifiers: SDL_Keymod) -> bool {
    let has = |flag: SDL_Keymod| modifiers.0 & flag.0 != 0;
    has(SDL_KMOD_CTRL) && has(SDL_KMOD_ALT) && has(SDL_KMOD_SHIFT)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct KeyInput {
    code: u16,
    modifiers: u8,
    flags: u8,
}

/// What a key sends to the PC, or None for a key without a Windows code.
fn key_input(scancode: SDL_Scancode, held: SDL_Keymod, system_keys: bool, right_alt_as_meta: bool) -> Option<KeyInput> {
    let has = |flag: SDL_Keymod| held.0 & flag.0 != 0;
    let mapped = if right_alt_as_meta && scancode == SDL_SCANCODE_RALT { SDL_SCANCODE_RGUI } else { scancode };
    let (code, flags) = keymap::virtual_key(mapped, system_keys || (right_alt_as_meta && scancode == SDL_SCANCODE_RALT))?;
    let mut modifiers = 0u8;
    if has(SDL_KMOD_CTRL) {
        modifiers |= modifier::CTRL;
    }
    if has(SDL_KMOD_LALT) || (!right_alt_as_meta && has(SDL_KMOD_RALT)) {
        modifiers |= modifier::ALT;
    }
    if has(SDL_KMOD_SHIFT) {
        modifiers |= modifier::SHIFT;
    }
    if (has(SDL_KMOD_GUI) && system_keys) || (right_alt_as_meta && has(SDL_KMOD_RALT)) {
        modifiers |= modifier::META;
    }
    Some(KeyInput { code, modifiers, flags })
}

/// Whether something automatic owns the bitrate, so the doctor's trial stays out of its way:
/// the PC's adaptive bitrate, automatic quality, or the client-hint controller over 0x553C.
fn bitrate_automatic(adaptive_bitrate: bool, quality_mode: QualityMode, rate_change: Option<&rate_change::RateChange>) -> bool {
    adaptive_bitrate || quality_mode != QualityMode::Off || rate_change.is_some_and(|rates| rates.hint_active())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_doctor_offers_no_trial_while_the_client_hint_steers_the_bitrate() {
        let trial = nightjar_policy::health::BitrateTrial::default();
        let offer = |automatic| trial.offer(true, 20_000, true, automatic, false);
        let hint = rate_change::RateChange::new(20_000, 60_000, true, 0, 0);
        let manual = rate_change::RateChange::new(20_000, 60_000, false, 0, 0);
        assert_eq!(offer(bitrate_automatic(false, QualityMode::Off, Some(&hint))), TrialOffer::None);
        assert_eq!(offer(bitrate_automatic(false, QualityMode::Off, Some(&manual))), TrialOffer::Try);
        assert_eq!(offer(bitrate_automatic(false, QualityMode::Off, None)), TrialOffer::Try);
        assert_eq!(offer(bitrate_automatic(true, QualityMode::Off, None)), TrialOffer::None);
        assert_eq!(offer(bitrate_automatic(false, QualityMode::Motion, None)), TrialOffer::None);
    }

    #[test]
    fn each_image_clipboard_refusal_says_its_own_reason() {
        use nightjar_core::http::{Error, ImageClipboardError as E};
        let texts = [
            image_error(E::TooLarge("20 MB".into()), true),
            image_error(E::NotPng("bad header".into()), true),
            image_error(E::NotPng("bad header".into()), false),
            image_error(E::Unsupported, true),
            image_error(E::Unsupported, false),
            image_error(E::Forbidden, false),
            image_error(E::Host(Error::Cancelled), false),
        ];
        assert!(texts[0].contains("too large") && texts[0].contains("20 MB"));
        assert!(texts[1].contains("this device") && texts[1].contains("bad header"));
        assert!(texts[2].contains("The PC sent"));
        assert!(texts[6].contains(&Error::Cancelled.to_string()));
        let distinct: HashSet<&String> = texts.iter().collect();
        assert_eq!(distinct.len(), texts.len(), "{texts:#?}");
    }

    #[test]
    fn ctrl_alt_backspace_goes_to_the_pc_and_only_ctrl_alt_shift_stays_local() {
        let ctrl_alt = SDL_Keymod(SDL_KMOD_LCTRL.0 | SDL_KMOD_LALT.0);
        assert!(!local_chord(ctrl_alt), "a host's menu chord is not Nightjar's");
        for (system_keys, right_alt_as_meta) in [(false, false), (true, false), (false, true), (true, true)] {
            let sent = key_input(SDL_SCANCODE_BACKSPACE, ctrl_alt, system_keys, right_alt_as_meta).unwrap();
            assert_eq!((sent.code, sent.modifiers), (0x08, modifier::CTRL | modifier::ALT));
            // The modifier keys themselves go to the PC too.
            assert!(key_input(SDL_SCANCODE_LCTRL, ctrl_alt, system_keys, right_alt_as_meta).is_some());
            assert!(key_input(SDL_SCANCODE_LALT, ctrl_alt, system_keys, right_alt_as_meta).is_some());
        }
        assert!(local_chord(SDL_Keymod(ctrl_alt.0 | SDL_KMOD_LSHIFT.0)));
    }

    #[test]
    fn the_wireless_hint_goes_only_to_a_rubylight_host_on_a_wireless_route_when_the_setting_is_on() {
        use nightjar_core::http::Family;
        assert!(tells_host_wireless(true, Family::Rubylight, true));
        assert!(!tells_host_wireless(false, Family::Rubylight, true));
        assert!(!tells_host_wireless(true, Family::Rubylight, false));
        for other in [Family::GameStream, Family::Sunshine, Family::Apollo, Family::Vibepollo, Family::Unknown] {
            assert!(!tells_host_wireless(true, other, true), "{other:?}");
        }
    }

    #[test]
    fn the_link_speed_hint_is_left_out_when_the_setting_is_off_but_beam_race_and_pace_keep_the_real_speed() {
        // On: the host is told the routed speed.
        let on = link_hints(true, 2500, None, 2100, None);
        assert_eq!(on, LinkHints { link_mbps: 2500, sent_link_mbps: 2500, pace_mbps: 2100 });
        // Off: nothing is sent, the beam race still sees a wired link and the pace stays.
        let off = link_hints(false, 2500, None, 2100, None);
        assert_eq!(off, LinkHints { link_mbps: 2500, sent_link_mbps: 0, pace_mbps: 2100 });
        assert_eq!(beam_race_for(None, true, off.link_mbps, true), BeamRace::On);
        // An unknown speed is never sent, on or off.
        assert_eq!(link_hints(true, 0, None, 0, None).sent_link_mbps, 0);
        // A variable naming the speed wins over the setting, and the pace variable over the measurement.
        let forced = link_hints(false, 2500, Some(1000), 2100, Some(900));
        assert_eq!(forced, LinkHints { link_mbps: 1000, sent_link_mbps: 1000, pace_mbps: 900 });
    }

    #[test]
    fn only_a_remote_monitor_session_is_view_only() {
        let settings = Settings::default();
        let config = |role| session_config(&settings, role, StreamConfig::default(), Location::Auto, true);
        let monitor = config(AppRole::RemoteMonitor);
        assert!(monitor.remote_monitor && !monitor.input_only);
        let stream = config(AppRole::Stream);
        assert!(!stream.remote_monitor && !stream.input_only);
        let input = config(AppRole::InputOnly);
        assert!(!input.remote_monitor && input.input_only);
    }

    #[test]
    fn a_lost_or_reset_render_device_ends_the_stream_saying_why() {
        for kind in [SDL_EVENT_RENDER_DEVICE_RESET, SDL_EVENT_RENDER_DEVICE_LOST] {
            let reason = render_device_ended(kind).expect("ends the stream");
            assert!(!Exit::Ended(Some(reason.into())).retryable(true), "{reason}");
        }
        assert_eq!(render_device_ended(SDL_EVENT_RENDER_TARGETS_RESET), None);
    }

    #[test]
    fn controllers_reach_the_host_only_when_on_and_not_from_a_remote_monitor() {
        let on = Settings { controllers: true, ..Settings::default() };
        let off = Settings { controllers: false, ..Settings::default() };
        assert!(sends_controllers(&on, AppRole::Stream));
        assert!(sends_controllers(&on, AppRole::InputOnly));
        assert!(!sends_controllers(&on, AppRole::RemoteMonitor));
        assert!(!sends_controllers(&off, AppRole::Stream));
    }

    #[test]
    fn the_input_and_priority_settings_reach_the_engine() {
        let on = session_config(&Settings::default(), AppRole::Stream, StreamConfig::default(), Location::Local, true);
        assert!(on.unbatched_input && on.network_priority);
        assert_eq!((on.location, on.network_grace), (Location::Local, Duration::from_millis(3000)));
        for (unbatched_input, network_priority) in [(false, true), (true, false), (false, false)] {
            let settings = Settings { unbatched_input, network_priority, network_grace_ms: 0, ..Default::default() };
            let config = session_config(&settings, AppRole::Stream, StreamConfig::default(), Location::Auto, true);
            assert_eq!((config.unbatched_input, config.network_priority), (unbatched_input, network_priority));
            assert_eq!(config.network_grace, Duration::ZERO);
        }
    }

    #[test]
    fn roaming_follows_the_settings_and_needs_mobile_data_without_the_watcher() {
        let on = session_config(&Settings::default(), AppRole::Stream, StreamConfig::default(), Location::Auto, true).roaming;
        assert_eq!(on, RoamingConfig { enabled: true, metered: false, platform_hints: true });
        let metered = Settings { roaming_metered: true, ..Default::default() };
        assert_eq!(roaming_config(&metered, true), RoamingConfig { enabled: true, metered: true, platform_hints: true });
        // Without the watcher a metered network looks like any other.
        assert!(!roaming_config(&Settings::default(), false).enabled);
        assert_eq!(roaming_config(&metered, false), RoamingConfig { enabled: true, metered: true, platform_hints: false });
        let off = Settings { client_roaming: false, roaming_metered: true, ..Default::default() };
        assert!(!roaming_config(&off, true).enabled && !roaming_config(&off, false).enabled);
    }

    #[test]
    fn every_tearing_vrr_stream_checks_its_display_not_only_the_beam_race() {
        let checks = |settings: &Settings, tearing: bool, forced: Option<bool>| {
            let (pacing, vrr) = present::pacing(settings, tearing);
            checks_display_for_vrr(vrr, pacing, present::without_vrr(settings), forced)
        };
        // The default settings: VRR on, the swap chain tears, Immediate.
        assert!(checks(&Settings::default(), true, None), "a fixed refresh display would tear on every present unchecked");
        let host_times = Settings { present_at_host_times: true, ..Default::default() };
        assert!(checks(&host_times, true, None));
        assert!(checks(&Settings { vsync: false, ..host_times.clone() }, true, None), "host times turn immediate");
        // Nothing to check: V-Sync already waits, VRR is off, the chain cannot tear, or V-Sync is
        // off in the settings and the stream is immediate already.
        assert!(!checks(&Settings::default(), false, None));
        let off = Settings { vrr: false, vsync: false, ..Default::default() };
        assert_eq!(present::pacing(&off, true).0, PacingMode::Immediate);
        assert!(!checks(&off, true, None), "tearing asked for without VRR is the user's choice");
        assert!(!checks(&Settings { vsync: false, ..Default::default() }, true, None), "V-Sync off: immediate either way");
        assert!(!checks(&Settings::default(), true, Some(false)), "NIGHTJAR_VRR_CHECK=0");
        assert!(checks(&Settings::default(), true, Some(true)));
    }

    #[test]
    fn the_scan_line_is_read_for_the_beam_race_the_display_check_and_vrr_sync_scan() {
        use present::{VrrPacing, VrrSync};
        let vrr = |check, sync| VrrPacing::new(check, PacingMode::Vsync, sync, PacingMode::Immediate);
        assert_eq!(display_probe(false, &vrr(true, VrrSync::Off)), Some("display check"), "every VRR stream that tears");
        assert_eq!(display_probe(true, &vrr(true, VrrSync::Off)), Some("beam race"));
        assert_eq!(display_probe(true, &vrr(false, VrrSync::Off)), Some("beam race"));
        // NIGHTJAR_VRR_SYNC=scan waits on the beam, so it reads the scan line with the check off.
        assert_eq!(display_probe(false, &vrr(false, VrrSync::Scan)), Some("display check"));
        assert_eq!(display_probe(false, &vrr(false, VrrSync::Present1)), None, "present1 reads no beam");
        assert_eq!(display_probe(false, &vrr(false, VrrSync::Off)), None, "a default stream without VRR does none of it");
    }

    #[test]
    fn the_pacing_line_says_why_a_vrr_stream_waits_for_the_refresh() {
        assert_eq!(pacing_label(PacingMode::Vsync, true), "V-Sync: this display does not refresh on present");
        assert_eq!(pacing_label(PacingMode::Vsync, false), "V-Sync", "V-Sync the settings asked for");
        assert_eq!(pacing_label(PacingMode::Immediate, true), "Immediate: this display does not refresh on present", "V-Sync off in the settings");
        assert_eq!(pacing_label(PacingMode::Immediate, false), "Immediate");
    }

    #[test]
    fn the_network_watcher_runs_for_the_stream_and_stops_with_it() {
        use std::sync::mpsc::RecvTimeoutError;
        let shared = Shared::new(Arc::new(|| {}), 8);
        // Kept whether the host roams or not: a lost network's return reconnects every stream.
        let (watch, ended) = crate::network::RoamWatch::idle();
        keep_roam_watch(&shared, Some(watch));
        assert!(lock(&shared.roam_watch).is_some());
        assert_eq!(ended.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty));
        lock(&shared.roam_watch).take();
        assert_eq!(ended.recv_timeout(Duration::from_secs(5)), Err(RecvTimeoutError::Disconnected));
    }

    #[test]
    fn early_decode_without_records_is_reported_not_silent() {
        assert_eq!(early_state(false, true, true), EarlyState::Off);
        assert_eq!(early_state(true, false, false), EarlyState::Off);
        assert_eq!(early_state(true, true, true), EarlyState::On);
        // The 0-frames-decoded-ahead runs: asked for, PyroWave, but the host never offered records.
        assert_eq!(early_state(true, true, false), EarlyState::NoRecords);
    }

    #[test]
    fn beam_race_line_names_a_host_that_sends_no_row_order() {
        assert_eq!(race_line(100, 300, 0), "100 frames in 300 slices, 0 not in row order");
        assert!(race_line(0, 0, 7).contains("not sending the row order"));
        assert!(!race_line(90, 270, 10).contains("not sending"));
    }

    #[test]
    fn beam_race_needs_the_setting_and_a_wired_link() {
        // The environment override is for measurement runs; these cases are the settings.
        if std::env::var_os("NIGHTJAR_BEAM_RACE").is_some() { return; }
        assert_eq!(beam_race(false, 1000, true), BeamRace::Off);
        assert_eq!(beam_race(true, 1000, true), BeamRace::On);
        assert_eq!(beam_race(true, 0, true), BeamRace::NotWired);
    }

    #[test]
    fn beam_race_needs_the_gpu_decoder_and_says_why_it_is_off() {
        // The forced value is the measurement override; the GPU decoder is needed either way.
        assert_eq!(beam_race_for(None, true, 1000, false), BeamRace::NoGpuDecoder);
        assert_eq!(beam_race_for(Some(true), false, 0, false), BeamRace::NoGpuDecoder);
        assert_eq!(beam_race_for(Some(true), false, 0, true), BeamRace::On);
        assert_eq!(beam_race_for(Some(false), true, 1000, true), BeamRace::Off);
        assert_eq!(beam_race_for(None, false, 1000, false), BeamRace::Off, "the setting off says nothing");
        assert!(BeamRace::NoGpuDecoder.reason().unwrap().contains("CPU"));
        assert!(BeamRace::NotWired.reason().unwrap().contains("wired"));
        assert_eq!((BeamRace::On.reason(), BeamRace::Off.reason()), (None, None));
    }

    #[test]
    fn the_environment_switches_take_a_few_spellings_and_refuse_the_rest() {
        for on in ["1", "true", "TRUE", "yes", "On", " 1 "] { assert_eq!(flag_value(on), Some(true), "{on:?}"); }
        for off in ["0", "false", "No", "OFF"] { assert_eq!(flag_value(off), Some(false), "{off:?}"); }
        // Not a switch: said on stderr by `env_flag` and ignored, so the setting decides.
        for junk in ["", "2", "ture", "enable", "1 0"] { assert_eq!(flag_value(junk), None, "{junk:?}"); }
        assert_eq!(slices_override("4"), Some(4));
        assert_eq!(slices_override("7"), Some(MAX_RACE_SLICES));
        for bad in ["0", "8", "16", "four", "-1", ""] { assert_eq!(slices_override(bad), None, "{bad:?}"); }
        assert_eq!(beam_race_for(None, false, 1000, true), BeamRace::Off, "a refused value leaves the setting in charge");
    }

    #[test]
    fn the_beam_race_off_asks_for_nothing() {
        // Whatever turned it off (the setting, `NIGHTJAR_BEAM_RACE=0`, a link or decoder that cannot race):
        // no row order from the host, no slices (so no probe, no slice handoff), early decode as the user set it.
        for beam in [BeamRace::Off, BeamRace::NotWired, BeamRace::NoGpuDecoder] {
            for early in [false, true] {
                assert_eq!(race_request(beam, early, 1080), RaceRequest { row_order: false, early_decode: early, slices: 0 }, "{beam:?}");
            }
        }
        for early in [EarlyState::Off, EarlyState::On, EarlyState::NoRecords] {
            assert!(!race_runs(0, early), "no slices asked for: the presenter never probes or follows a slice ({early:?})");
        }
        assert!(race_runs(4, EarlyState::On));
        assert!(!race_runs(4, EarlyState::NoRecords), "asked for, but the host sends no records to slice");
        let on = race_request(BeamRace::On, false, 1080);
        assert!(on.row_order && on.early_decode && on.slices > 0, "on asks for all of it: {on:?}");
        // The default settings never turn it on, on any link.
        assert!(!crate::config::Settings::default().beam_race);
        assert_eq!(beam_race_for(None, false, 1000, true), BeamRace::Off);
    }

    #[test]
    fn beam_slices_are_four_or_three_at_4k() {
        if std::env::var_os("NIGHTJAR_BEAM_RACE_SLICES").is_some() { return; }
        assert_eq!(beam_slices(1080), 4);
        assert_eq!(beam_slices(2160), 3);
    }

    fn display() -> Display {
        Display {
            width: 2560,
            height: 1440,
            refresh_hz: 143.9,
        }
    }

    #[test]
    fn metered_limit_caps_fixed_auto_and_pyrowave_bitrates() {
        let settings = Settings { codec: CodecPreference::PyroWave, ..Default::default() };
        let display = Display { width: 1920, height: 1080, refresh_hz: 60.0 };
        let (_, _, _, bitrate) = stream_mode(&settings, display);
        assert!(bitrate > 5_000);
        assert_eq!(capped_bitrate(bitrate, 5_000), 5_000);
        assert_eq!(capped_bitrate(4_000, 5_000), 4_000);
        assert_eq!(capped_bitrate(bitrate, 0), bitrate);
    }

    #[test]
    fn only_a_new_stream_at_the_automatic_bitrate_tests_the_link_first() {
        let info = |paired: u8, version: &str| ServerInfo::parse(&format!("<root status_code=\"200\"><uniqueid>pc</uniqueid><state>SERVER_FREE</state><PairStatus>{paired}</PairStatus><BandwidthProbeVersion>{version}</BandwidthProbeVersion></root>"), 47989).unwrap();
        let offered = info(1, "1");
        assert!(!probes_first_bitrate(&Settings::default(), false, &offered), "the link test is off by default");
        let auto = Settings { link_probe: true, ..Default::default() };
        assert!(probes_first_bitrate(&auto, false, &offered));
        assert!(!probes_first_bitrate(&auto, true, &offered), "a resume keeps the session's bitrate");
        assert!(!probes_first_bitrate(&Settings { bitrate_kbps: 30_000, ..auto.clone() }, false, &offered));
        for codec in [CodecPreference::PyroWave, CodecPreference::PyroWave444] {
            assert!(!probes_first_bitrate(&Settings { codec, ..auto.clone() }, false, &offered));
        }
        assert!(!probes_first_bitrate(&auto, false, &info(1, "0")), "a PC that does not offer the test");
        assert!(!probes_first_bitrate(&auto, false, &info(0, "1")), "an unpaired PC");
    }

    #[test]
    fn native_mode_follows_the_display() {
        let (w, h, fps, kbps) = stream_mode(&Settings::default(), display());
        assert_eq!((w, h, fps), (2560, 1440, 144));
        assert_eq!(kbps, config::default_bitrate_kbps(2560, 1440, 144));
        let settings = Settings {
            width: 1920,
            height: 1080,
            fps: 60,
            bitrate_kbps: 30000,
            ..Default::default()
        };
        assert_eq!(stream_mode(&settings, display()), (1920, 1080, 60, 30000));
        let mut settings = Settings { codec: CodecPreference::PyroWave444, ..settings };
        assert_eq!(stream_mode(&settings, display()).3, 151_000);
        settings.bitrate_kbps = 0;
        assert_eq!(stream_mode(&settings, display()).3, 151_000);
        settings.bitrate_kbps = 2_000_000;
        assert_eq!(stream_mode(&settings, display()).3, 2_000_000);
    }

    #[test]
    fn formats_respect_the_host() {
        let all = SCM_H264 | SCM_HEVC | SCM_HEVC_MAIN10 | SCM_AV1_MAIN8 | SCM_AV1_MAIN10;
        assert_eq!(
            video_formats(CodecPreference::Auto, false, all),
            VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265
        );
        assert_eq!(
            video_formats(CodecPreference::Av1, true, all),
            VIDEO_FORMAT_H264 | VIDEO_FORMAT_AV1_MAIN8 | VIDEO_FORMAT_AV1_MAIN10
        );
        assert_eq!(video_formats(CodecPreference::Hevc, false, SCM_H264), VIDEO_FORMAT_H264);
        assert_eq!(video_formats(CodecPreference::H264, true, all), VIDEO_FORMAT_H264);
        assert_eq!(video_formats(CodecPreference::PyroWave, false, SCM_H264 | SCM_PYROWAVE), VIDEO_FORMAT_H264 | VIDEO_FORMAT_PYROWAVE);
        assert_eq!(video_formats(CodecPreference::PyroWave444, true, SCM_H264 | SCM_PYROWAVE_MAIN10_444), VIDEO_FORMAT_H264 | VIDEO_FORMAT_PYROWAVE_MAIN10_444);
        assert_eq!(video_formats(CodecPreference::PyroWave444, false, SCM_H264), VIDEO_FORMAT_H264);
    }

    const COLOR_SCM: u32 = SCM_H264 | SCM_HEVC | SCM_AV1_MAIN8 | SCM_HEVC_MAIN10 | SCM_AV1_MAIN10
        | SCM_H264_HIGH8_444 | SCM_HEVC_REXT8_444 | SCM_AV1_HIGH8_444;

    fn decodes_all() -> (u32, String) {
        (color::YUV444_FORMATS | color::SDR10_FORMATS, String::new())
    }

    #[test]
    fn the_launch_offers_what_the_4_4_4_and_10_bit_settings_add() {
        let hevc = Settings { codec: CodecPreference::Hevc, ..Default::default() };
        let plain = offered_formats(&hevc, COLOR_SCM, &decodes_all());
        assert_eq!(plain.formats, video_formats(CodecPreference::Hevc, false, COLOR_SCM), "both off: the codec choice alone");
        assert_eq!(color_notice(&plain, VIDEO_FORMAT_H265), None);
        let yuv444 = offered_formats(&Settings { prefer_yuv444: true, ..hevc.clone() }, COLOR_SCM, &decodes_all());
        assert_eq!(yuv444.formats, VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_REXT8_444);
        assert_eq!(color_notice(&yuv444, VIDEO_FORMAT_H265_REXT8_444).as_deref(), Some("Streaming in YUV 4:4:4."));
        let sdr10 = offered_formats(&Settings { sdr_10bit: true, ..hevc.clone() }, COLOR_SCM, &decodes_all());
        assert_eq!(sdr10.formats, VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10);
        assert_eq!(color_notice(&sdr10, VIDEO_FORMAT_H265_MAIN10).as_deref(), Some("Streaming 10-bit color (SDR)."));
        assert_eq!(color_notice(&sdr10, VIDEO_FORMAT_H265).as_deref(), Some("This PC didn’t accept 10-bit, so the stream is 8-bit."));
        // The probe's answer decides: what the decoder can't draw is not offered, and its reason is told.
        let none = (0, "HEVC 4:4:4: the decoder failed on the fixture".to_owned());
        let refused = offered_formats(&Settings { prefer_yuv444: true, ..hevc.clone() }, COLOR_SCM, &none);
        assert_eq!(refused.formats, plain.formats);
        assert_eq!(color_notice(&refused, VIDEO_FORMAT_H265).as_deref(),
            Some("YUV 4:4:4 unavailable; using 4:2:0: this device can’t decode or draw it (HEVC 4:4:4: the decoder failed on the fixture)."));
        // HDR takes the stream and needs no word; packed full chroma keeps its own offer.
        let hdr = offered_formats(&Settings { hdr: true, prefer_yuv444: true, sdr_10bit: true, ..hevc.clone() }, COLOR_SCM, &decodes_all());
        assert_eq!(hdr.formats, video_formats(CodecPreference::Hevc, true, COLOR_SCM));
        assert_eq!(color_notice(&hdr, VIDEO_FORMAT_H265_MAIN10), None);
        let packed = offered_formats(&Settings { experimental_packed_444: true, prefer_yuv444: true, ..hevc.clone() }, COLOR_SCM, &decodes_all());
        assert_eq!(packed.formats, video_formats(CodecPreference::Hevc, false, 0));
        assert_eq!(color_notice(&packed, VIDEO_FORMAT_H265).as_deref(),
            Some("YUV 4:4:4 unavailable; using 4:2:0: full chroma is on and has its own 4:4:4 and 10-bit handling."));
        // Both on: 4:4:4 wins and 10-bit says why.
        let both = offered_formats(&Settings { prefer_yuv444: true, sdr_10bit: true, ..hevc }, COLOR_SCM, &decodes_all());
        assert_eq!(color_notice(&both, VIDEO_FORMAT_H265_REXT8_444).as_deref(),
            Some("Streaming in YUV 4:4:4.\n10-bit SDR isn’t used, so the stream is 8-bit: 4:4:4 is on, and the PC takes 4:4:4 or 10-bit, not both."));
    }

    #[test]
    fn a_ten_bit_sdr_offer_launches_with_hdr_mode_zero() {
        // Hardware lane 4a: without hdrMode=0 the host streamed 10-bit SDR as HDR and the SDR
        // display ended the stream at once, for HEVC Main10 and AV1 alike.
        for codec in [CodecPreference::Hevc, CodecPreference::Av1] {
            let sdr10 = Settings { sdr_10bit: true, codec, ..Default::default() };
            let offer = offered_formats(&sdr10, COLOR_SCM, &decodes_all());
            assert!(launches_ten_bit_sdr(&sdr10, &offer), "{codec:?}");
            let request = LaunchRequest { ten_bit_sdr: launches_ten_bit_sdr(&sdr10, &offer), ..Default::default() };
            assert!(request.query(&nightjar_core::http::HostCapabilities::UNKNOWN, false).contains("&hdrMode=0&"));
            // HDR asks for HDR; plain 8-bit, and 10-bit the PC or the decoder can't do, say nothing.
            let hdr = Settings { hdr: true, ..sdr10.clone() };
            assert!(!launches_ten_bit_sdr(&hdr, &offered_formats(&hdr, COLOR_SCM, &decodes_all())));
            let plain = Settings { codec, ..Default::default() };
            assert!(!launches_ten_bit_sdr(&plain, &offered_formats(&plain, COLOR_SCM, &decodes_all())));
            assert!(!launches_ten_bit_sdr(&sdr10, &offered_formats(&sdr10, SCM_H264 | SCM_HEVC | SCM_AV1_MAIN8, &decodes_all())));
            assert!(!launches_ten_bit_sdr(&sdr10, &offered_formats(&sdr10, COLOR_SCM, &(0, "no".into()))));
        }
    }

    #[test]
    fn the_decoder_is_checked_only_when_a_setting_reaches_it() {
        let on = Settings { prefer_yuv444: true, ..Default::default() };
        assert!(!probes_color(&Settings::default()));
        assert!(probes_color(&on));
        assert!(probes_color(&Settings { sdr_10bit: true, ..Default::default() }));
        assert!(!probes_color(&Settings { hdr: true, ..on.clone() }));
        assert!(!probes_color(&Settings { experimental_packed_444: true, ..on.clone() }));
        assert!(!probes_color(&Settings { codec: CodecPreference::PyroWave, ..on }));
    }

    #[test]
    fn color_notices_are_german_in_german() {
        let offer = offered_formats(&Settings { sdr_10bit: true, codec: CodecPreference::Hevc, ..Default::default() }, SCM_H264 | SCM_HEVC, &decodes_all());
        let text = crate::strings::tests::with_lang(crate::strings::Lang::De, || color_notice(&offer, VIDEO_FORMAT_H265));
        assert_eq!(text.as_deref(), Some("10-Bit-SDR wird nicht verwendet, der Stream läuft mit 8 Bit: der PC codiert es für diesen Codec nicht."));
    }

    #[test]
    fn a_chosen_pyrowave_that_does_not_stream_is_logged_and_told() {
        let fallback = |codec: CodecPreference, scm: u32, negotiated: u32| {
            let offered = offered_formats(&Settings { codec, ..Default::default() }, scm, &decodes_all()).formats;
            pyrowave_fallback(codec, (scm, scm), offered, negotiated)
        };
        // The PC lists its codecs, and PyroWave is not among them: it was never offered.
        let (log, notice) = fallback(CodecPreference::PyroWave, SCM_H264 | SCM_HEVC, VIDEO_FORMAT_H265).expect("a fallback");
        assert_eq!(log, "nightjar: PyroWave was chosen, but the host does not offer it (codec support 0x101, reported 0x101); streaming HEVC");
        assert_eq!(notice, "This PC doesn’t offer PyroWave, so Nightjar uses HEVC.");
        // Offered, and the PC picked another codec.
        let (log, notice) = fallback(CodecPreference::PyroWave444, 0, VIDEO_FORMAT_H264).expect("a fallback");
        assert_eq!(log, "nightjar: PyroWave was chosen, but the host reports no trusted codec support and chose another codec (codec support 0x0, reported 0x0); streaming H.264");
        assert_eq!(notice, "This PC doesn’t offer PyroWave, so Nightjar uses H.264.");
        let (log, _) = fallback(CodecPreference::PyroWave, SCM_H264 | SCM_HEVC | SCM_PYROWAVE, VIDEO_FORMAT_H265).expect("a fallback");
        assert!(log.contains("but the host chose another codec (codec support"), "{log}");
        // The log keeps what serverinfo said when the trusted value stripped PyroWave.
        let (log, _) = pyrowave_fallback(CodecPreference::PyroWave, (SCM_H264 | SCM_HEVC, SCM_H264 | SCM_HEVC | SCM_PYROWAVE),
            video_formats(CodecPreference::PyroWave, false, SCM_H264 | SCM_HEVC), VIDEO_FORMAT_H265).expect("a fallback");
        assert!(log.contains(&format!("(codec support 0x101, reported {:#x})", SCM_H264 | SCM_HEVC | SCM_PYROWAVE)), "{log}");
        // PyroWave streams, or was not chosen: nothing to say.
        assert_eq!(fallback(CodecPreference::PyroWave, SCM_H264 | SCM_PYROWAVE, VIDEO_FORMAT_PYROWAVE), None);
        assert_eq!(fallback(CodecPreference::PyroWave444, SCM_H264 | SCM_PYROWAVE, VIDEO_FORMAT_PYROWAVE), None);
        assert_eq!(fallback(CodecPreference::Auto, SCM_H264 | SCM_HEVC, VIDEO_FORMAT_H265), None);
        let german = crate::strings::tests::with_lang(crate::strings::Lang::De, || {
            fallback(CodecPreference::PyroWave, SCM_H264 | SCM_HEVC, VIDEO_FORMAT_H265).map(|(_, notice)| notice)
        });
        assert_eq!(german.as_deref(), Some("Dieser PC bietet PyroWave nicht an, daher nutzt Nightjar HEVC."));
    }

    #[test]
    fn the_pyrowave_fallback_is_told_once_per_pc_until_pyrowave_streams_again() {
        let text = "This PC doesn’t offer PyroWave, so Nightjar uses HEVC.";
        let mut pc = HostRecord::default();
        let mut other = HostRecord::default();
        assert_eq!(pyrowave_notice(&mut pc, true, Some(text), VIDEO_FORMAT_H265), (Some(text.to_owned()), true), "the first time");
        // A reconnect, automatic or from the menu, and every later stream from this PC: the log only.
        assert_eq!(pyrowave_notice(&mut pc, true, Some(text), VIDEO_FORMAT_H265), (None, false));
        // Kept with the PC's record, so a restart of Nightjar stays quiet too.
        let mut saved: HostRecord = serde_json::from_str(&serde_json::to_string(&pc).unwrap()).unwrap();
        assert_eq!(pyrowave_notice(&mut saved, true, Some(text), VIDEO_FORMAT_H265), (None, false));
        assert_eq!(pyrowave_notice(&mut other, true, Some(text), VIDEO_FORMAT_H265), (Some(text.to_owned()), true), "another PC is told");
        // Streams that say nothing of PyroWave leave it as it is.
        assert_eq!(pyrowave_notice(&mut pc, false, None, VIDEO_FORMAT_H265), (None, false));
        assert_eq!(pyrowave_notice(&mut pc, true, None, VIDEO_FORMAT_H265), (None, false));
        // PyroWave streams from the PC again: losing it later is told again.
        assert_eq!(pyrowave_notice(&mut pc, true, None, VIDEO_FORMAT_PYROWAVE), (None, true));
        assert_eq!(pyrowave_notice(&mut pc, true, None, VIDEO_FORMAT_PYROWAVE), (None, false), "nothing to save then");
        assert_eq!(pyrowave_notice(&mut pc, true, Some(text), VIDEO_FORMAT_H265), (Some(text.to_owned()), true));
        // A record saved before the notices existed reads with none shown.
        let old: HostRecord = serde_json::from_str(r#"{"name":"PC","address":"192.0.2.1"}"#).unwrap();
        assert!(old.notices.is_empty());
    }

    #[test]
    fn rtp_clock_unwraps() {
        let mut c = RtpClock::default();
        let a = c.host_us(u32::MAX - 899);
        let b = c.host_us(600);
        // 1500 ticks of 90 kHz across the wrap, give or take the rounding.
        assert!((b - a - 16_667).abs() <= 1, "{}", b - a);
    }

    #[test]
    fn the_pipeline_line_says_each_stage_in_fps() {
        let report = PipelineRatesReport {
            short_window: false,
            window_ms: 1000,
            source_mhz: 119_880,
            capture_mhz: 240_000,
            submit_mhz: 120_000,
            output_mhz: 120_000,
            repeat_mhz: 0,
        };
        assert_eq!(pipeline_rates_line(&report), "119.9 source, 240.0 captured, 120.0 encoded, 120.0 sent, 0.0 repeated fps");
        let unknown = PipelineRatesReport { source_mhz: UNKNOWN, ..report };
        assert!(pipeline_rates_line(&unknown).starts_with("- source, 240.0 captured"));
        crate::strings::set(crate::strings::Lang::De);
        assert_eq!(pipeline_rates_line(&report), "119.9 Quelle, 240.0 erfasst, 120.0 kodiert, 120.0 gesendet, 0.0 wiederholt fps");
    }

    #[test]
    fn a_host_without_hardware_readings_is_not_asked_again_but_a_failed_request_is() {
        use nightjar_core::http::{Availability, Error};
        let sample = HardwareMetrics::parse(r#"{"version":1,"sampled_at_unix_ms":5,"interval_ms":1000,"cpu":{},"gpus":[]}"#).unwrap();
        assert_eq!(hardware_answer(&Ok(sample)), HardwareAnswer::Sample { sampled_at_unix_ms: 5, interval_ms: 1000 });
        let unavailable = [
            HostApiError::Unavailable { feature: Feature::HardwareMetrics, availability: Availability::Unknown },
            HostApiError::UnsupportedVersion,
            HostApiError::Host(Error::NotFound("api/hardware-metrics".into())),
            HostApiError::Host(Error::Status { code: 403, message: String::new() }),
            HostApiError::Host(Error::Status { code: 409, message: String::new() }),
        ];
        for error in unavailable {
            assert_eq!(hardware_answer(&Err(error)), HardwareAnswer::Unavailable);
        }
        for error in [Error::Status { code: 500, message: String::new() }, Error::Malformed("{".into()), Error::Cancelled] {
            assert_eq!(hardware_answer(&Err(HostApiError::Host(error))), HardwareAnswer::Failed, "tried again later");
        }
    }

    #[test]
    fn overlay_modes_show_their_own_lines() {
        let overlay = Overlay {
            lines: vec![("Decoder".into(), "d3d11va".into())],
            statistics: vec![("Frames".into(), "60".into())],
            ..Default::default()
        };
        assert_eq!(overlay.lines(OverlayMode::Debug)[0].0, "Decoder");
        assert_eq!(overlay.lines(OverlayMode::Statistics)[0].0, "Frames");
        assert!(overlay.lines(OverlayMode::Compact).is_empty(), "Compact draws its one line instead");
    }

    #[test]
    fn the_link_notice_line_says_the_lowered_bitrate() {
        use rubylight_protocol::control::LinkReason;
        let notice = |current_kbps| LinkNotice {
            state: LinkState::Limited,
            reason: LinkReason::Network,
            ceiling_kbps: 150_000,
            current_kbps,
        };
        assert_eq!(link_notice_line(&notice(37_500)), "Host lowered the bitrate to 38 Mbps: network");
        assert_eq!(link_notice_line(&notice(9_900)), "Host lowered the bitrate to 9.9 Mbps: network");
        crate::strings::set(crate::strings::Lang::De);
        assert_eq!(link_notice_line(&notice(4_500)), "Host hat die Bitrate auf 4.5 Mbit/s gesenkt: Netzwerk");
    }

    #[test]
    fn terminations_read_well() {
        assert!(termination_text(0).is_none());
        assert!(termination_text(-100).unwrap().contains("UDP"));
    }
}

#[cfg(test)]
mod host_line_tests {
    use super::*;

    #[test]
    fn host_processing_averages_received_frames_over_two_windows_like_artemis() {
        let mut average = HostAverage::default();
        // First second: 60 frames of 0.5 ms (5 units). Nothing earlier to add.
        assert_eq!(average.update(300, 60), (300, 60));
        // Second second: 60 frames of 1.0 ms. The overlay sums both seconds.
        assert_eq!(average.update(900, 120), (900, 120));
        // Third second: only the last two seconds count, the first has dropped out.
        assert_eq!(average.update(1200, 180), (900, 120));
        // A window without frames carrying a header value leaves the previous one standing.
        assert_eq!(average.update(1200, 180), (300, 60));
        assert_eq!(average.update(1200, 180), (0, 0));
        let w = Window { host_latency: 900, host_latency_n: 120, ..Default::default() };
        assert_eq!(host_lines(&w), vec![("Host processing".to_owned(), "0.8 ms".to_owned())]);
    }

    #[test]
    fn host_processing_is_the_header_average_and_a_host_report_never_changes_it() {
        // Header values are in 100 us units: 3 frames of 5, 5 and 5 are 0.5 ms.
        let header = Window { host_latency: 15, host_latency_n: 3, ..Default::default() };
        let lines = host_lines(&header);
        assert_eq!(lines, vec![("Host processing".to_owned(), "0.5 ms".to_owned())]);
        // Reports say 5.0 ms present to send: a separate line, the processing line is unchanged.
        let both = Window { timing_host_us: 15_000, timing_encode_us: 1_200, timing_n: 3, ..header };
        let lines = host_lines(&both);
        assert_eq!(lines[0], ("Host processing".to_owned(), "0.5 ms".to_owned()));
        assert_eq!(lines[1].0, "Host to network");
        assert_eq!(lines[1].1, "5.0 ms present to send, 0.4 ms encode");
        // Reports alone never produce a "Host processing" line.
        let reports = Window { timing_host_us: 15_000, timing_encode_us: 1_200, timing_n: 3, ..Default::default() };
        assert!(host_lines(&reports).iter().all(|(label, _)| label != "Host processing"));
    }
}

#[cfg(test)]
mod early_tests {
    use super::*;

    #[test]
    fn the_overlay_line_and_the_session_summary_carry_the_counters() {
        assert_eq!(early_line((130, 3), (70, 2), 2.0), "30 early/s, 1 whole in the last second, 3 fallbacks in all");
        let summary = early_summary(1200, 4, 8_500, 7);
        for part in ["1200 frames decoded ahead", "4 fell back", "7 chunks dropped", "8.5 ms"] {
            assert!(summary.contains(part), "{summary}");
        }
        let video = video_summary(3600, 12, 5);
        for part in ["3600 frames to the decoder", "12 dropped over the decoder queue", "5 replaced"] {
            assert!(video.contains(part), "{video}");
        }
    }
}

#[cfg(test)]
mod replace_tests {
    use super::*;

    fn launch_plan() -> LaunchPlan {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?><root status_code="200"><hostname>PC</hostname><uniqueid>uid</uniqueid><HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort><PairStatus>1</PairStatus><currentgame>7</currentgame><state>SUNSHINE_SERVER_BUSY</state><RustHostVersion>3.0.0</RustHostVersion></root>"#;
        LaunchPlan {
            host_id: 1,
            record: Default::default(),
            host_name: "PC".into(),
            info: ServerInfo::parse(xml, 47989).expect("the answer parses"),
            native: None,
            app: nightjar_core::http::App {
                name: "Zelda".into(),
                id: 9,
                hdr_supported: false,
                uuid: String::new(),
                host_index: None,
                art_version: String::new(),
                control: nightjar_core::http::AppControl::None,
                monitor_resume: false,
            },
            resume: false,
            small_stream_input_only: false,
            settings: Settings::default(),
            identity: Arc::new(ClientIdentity::generate().expect("an identity")),
            display: Display { width: 1920, height: 1080, refresh_hz: 60.0 },
            hardware: Hardware::None,
            dmabuf: false,
            gpu_decoder: None,
            packed_modes: 0,
            color_decodable: (0, String::new()),
            pacing: PacingMode::Immediate,
            vrr: false,
            gamepad_mask: 0,
            prepared_controllers: String::new(),
            microphone_muted: false,
            bitrate_limit_kbps: 0,
            network: Default::default(),
            pace: None,
            host_link_mbps: 0,
            recovery: None,
            confirm_replace: false,
        }
    }

    // The review's regression: roamed is false, but the same address came back (DHCP gave it
    // again) or there is no route yet. The connection resumes in place, so the stream goes on;
    // only a return it cannot carry on over reconnects, with the desktop's default settings too.
    #[test]
    fn only_a_network_back_the_connection_cannot_carry_on_over_reconnects_at_once() {
        let defaults = Settings::default();
        assert_eq!(defaults.network_grace_ms, 3000, "below the connection's own timeout: no tolerance");
        assert!(!reconnects_on_return(NetworkChange::CarriesOn, false, &defaults, true));
        assert!(!reconnects_on_return(NetworkChange::Roamed, false, &defaults, true));
        assert!(reconnects_on_return(NetworkChange::CannotContinue, false, &defaults, true));
        // Not when it would not reconnect on its own (minimized, not a game stream, budget used).
        assert!(!reconnects_on_return(NetworkChange::CannotContinue, false, &defaults, false));
        // Mobile data only with "Continue on mobile data".
        assert!(!reconnects_on_return(NetworkChange::CannotContinue, true, &defaults, true));
        let mobile = Settings { roaming_metered: true, ..Settings::default() };
        assert!(reconnects_on_return(NetworkChange::CannotContinue, true, &mobile, true));
        // A tolerance above the timeout holds the connection and decides; so does the user.
        let tolerant = Settings { network_grace_ms: 20_000, ..Settings::default() };
        assert!(!reconnects_on_return(NetworkChange::CannotContinue, false, &tolerant, true));
        let manual = Settings { auto_reconnect: false, ..Settings::default() };
        assert!(!reconnects_on_return(NetworkChange::CannotContinue, false, &manual, true));
        // The exit it takes reconnects, once the stream had video.
        assert!(Exit::NetworkReturned.retryable(true));
        assert!(!Exit::NetworkReturned.retryable(false));
    }

    #[test]
    fn a_vpn_route_streams_as_remote_with_small_packets() {
        assert_eq!(location_for_route(Location::Auto, true), (Location::Remote, 1024));
        assert_eq!(location_for_route(Location::Auto, false), (Location::Auto, 1392));
        // NIGHTJAR_STREAM_LOCATION decides over the route.
        assert_eq!(location_for_route(Location::Local, true), (Location::Local, 1392));
        assert_eq!(location_for_route(Location::Remote, false), (Location::Remote, 1392));
    }

    // A blocked media port: the stream never shows a picture and its error shows at once, not after
    // five reconnects and a minute (Moonlight V+ PC 6792aeed).
    #[test]
    fn a_stream_that_never_showed_video_does_not_reconnect() {
        use nightjar_core::session::error;
        for code in [error::NO_VIDEO_TRAFFIC, error::NO_VIDEO_FRAME, -1, 10054] {
            assert!(!Exit::Terminated(code).retryable(false), "{code}");
            assert!(Exit::Terminated(code).retryable(true), "{code}");
        }
        assert!(!Exit::Terminated(error::GRACEFUL).retryable(true));
        assert!(!Exit::Reconnect.retryable(true), "the user's own reconnect is not an automatic one");
    }

    #[test]
    fn the_lost_network_notice_promises_a_reconnect_only_when_one_follows() {
        let defaults = Settings::default();
        assert_eq!(lost_notice(&defaults, true), NETWORK_LOST_RECONNECTING);
        assert_eq!(lost_notice(&defaults, false), NETWORK_LOST);
        let tolerant = Settings { network_grace_ms: 20_000, ..Settings::default() };
        assert_eq!(lost_notice(&tolerant, true), NETWORK_LOST);
    }

    #[test]
    fn a_network_return_rejoins_without_the_first_delay_and_only_on_its_own() {
        let mut plan = launch_plan();
        plan.app.control = nightjar_core::http::AppControl::None;
        let now = recovery::now_ms();
        let mut rejoined = automatic_rejoin(&plan, AppRole::Stream, true, false, 0, true).expect("a plan");
        assert!(rejoined.resume);
        let budget = rejoined.recovery.as_mut().expect("a budget");
        assert!(budget.allows_attempt(now));
        assert_eq!(budget.next_delay(now), Some(0), "no delay, but it spends an attempt");
        assert_eq!(budget.attempts(), 1);
        // Without retry_now the usual first delay.
        let mut usual = automatic_rejoin(&plan, AppRole::Stream, true, false, 0, false).unwrap().recovery.unwrap();
        assert_eq!(usual.next_delay(now), Some(1000));
        // Minimized, off, or a stream that is not a game: the user decides.
        assert!(automatic_rejoin(&plan, AppRole::Stream, true, true, 0, true).is_none());
        assert!(automatic_rejoin(&plan, AppRole::Stream, false, false, 0, true).is_none());
        assert!(automatic_rejoin(&plan, AppRole::RemoteMonitor, true, false, 0, true).is_none());
        // A used-up budget allows no attempt, so the notice promises none.
        let mut spent = recovery::Backoff::default();
        while spent.next_delay(now).is_some() {}
        plan.recovery = Some(spent);
        let rejoined = automatic_rejoin(&plan, AppRole::Stream, true, false, 0, true).unwrap();
        assert!(!rejoined.recovery.unwrap().allows_attempt(now));
    }

    #[test]
    fn only_the_launch_the_user_answered_confirms_the_replace() {
        use nightjar_core::http::Error;
        let prompt = Error::Status {
            code: 410,
            message: "An app is already running. Launch this app again within 60 seconds to confirm that you want to close it.".into(),
        };
        let mut plan = launch_plan();
        assert!(asks_replace(&plan, AppRole::Stream, &prompt));
        // Another 410, a role that replaces nothing, or a resume is no prompt to answer.
        assert!(!asks_replace(&plan, AppRole::Stream, &Error::Status { code: 410, message: "Gone".into() }));
        assert!(!asks_replace(&plan, AppRole::InputOnly, &prompt));
        // A launch that already confirmed is not asked again: the refusal is shown.
        plan.confirm_replace = true;
        assert!(!asks_replace(&plan, AppRole::Stream, &prompt));
        // A reconnect resumes and never carries the answer.
        let rejoined = rejoin(plan.clone(), AppRole::Stream).expect("a plan");
        assert!(rejoined.resume && !rejoined.confirm_replace);
        assert!(!asks_replace(&rejoined, AppRole::Stream, &prompt));
        let monitor = rejoin(plan, AppRole::RemoteMonitor).expect("a plan");
        assert!(!monitor.resume && !monitor.confirm_replace);
    }

    #[test]
    fn a_dropped_stream_reconnects_without_focus_but_not_minimized() {
        // L4 on the laptop: a system dialog held the focus, so the drop never reconnected.
        assert!(reconnects_automatically(true, false, AppRole::Stream));
        assert!(!reconnects_automatically(true, true, AppRole::Stream), "minimized or hidden waits for the user");
        assert!(!reconnects_automatically(false, false, AppRole::Stream), "the setting is off");
        assert!(!reconnects_automatically(true, false, AppRole::InputOnly));
        assert!(!reconnects_automatically(true, false, AppRole::RemoteMonitor));
    }

    #[test]
    fn a_replace_prompt_hands_back_the_plan_to_launch_again() {
        for asks_replace in [true, false] {
            let plan = launch_plan();
            let (sender, result) = channel();
            let mut connecting = Connecting {
                shared: Shared::new(Arc::new(|| {}), 2),
                interrupt: Interrupt::default(),
                stage: String::new(),
                app_name: plan.app.name.clone(),
                host_name: plan.host_name.clone(),
                host_id: plan.host_id,
                pacing: plan.pacing,
                settings: plan.settings.clone(),
                plan,
                result,
            };
            sender.send(Err(ConnectError { message: "refused".into(), retryable: false, asks_replace })).unwrap();
            match connecting.poll() {
                Err(ConnectFailed::ConfirmReplace(plan)) => {
                    assert!(asks_replace);
                    assert!(plan.app.id == 9 && !plan.resume && !plan.confirm_replace);
                }
                Err(ConnectFailed::Message(message)) => {
                    assert!(!asks_replace);
                    assert_eq!(message, "refused");
                }
                Ok(_) => panic!("the launch failed"),
            }
        }
    }
}

#[cfg(test)]
mod audio_tests {
    use super::*;

    fn reports_714(xml: &str) -> bool {
        ServerInfo::parse(xml, 47989).expect("the answer parses").capabilities.has(Feature::Surround714)
    }

    #[test]
    fn only_a_pc_that_reports_7_1_4_is_asked_for_it() {
        let rubylight = include_str!("../../../../app/src/test/resources/protocol/rubylight-30-serverinfo.xml");
        let rubylight_without = include_str!("../../../../app/src/test/resources/protocol/rubylight-30-refused-serverinfo.xml");
        // Foundation Sunshine's serverinfo reads as stock Sunshine's.
        let sunshine = include_str!("../../../../app/src/test/resources/protocol/sunshine-serverinfo.xml");
        assert!(reports_714(rubylight));
        assert!(!reports_714(rubylight_without), "Rubylight with 7.1.4 off");
        assert!(!reports_714(sunshine));

        let asked = |host: &str, output: bool| audio_configuration(AudioChannels::Surround714, reports_714(host), output);
        assert_eq!(asked(rubylight, true), AudioConfiguration::SURROUND_714);
        assert_eq!(asked(sunshine, true), AudioConfiguration::SURROUND_71);
        assert_eq!(asked(rubylight_without, true), AudioConfiguration::SURROUND_71);
        // An output that plays fewer than 12 channels gets 7.1 from every PC.
        assert_eq!(asked(rubylight, false), AudioConfiguration::SURROUND_71);
        // The other choices are asked of every PC as picked.
        assert_eq!(audio_configuration(AudioChannels::Surround71, false, false), AudioConfiguration::SURROUND_71);
        assert_eq!(audio_configuration(AudioChannels::Surround51, false, false), AudioConfiguration::SURROUND_51);
        assert_eq!(audio_configuration(AudioChannels::Stereo, true, true), AudioConfiguration::STEREO);
    }

    #[test]
    fn the_7_1_4_choice_round_trips_through_the_settings_file() {
        let saved = serde_json::to_string(&AudioChannels::Surround714).unwrap();
        assert_eq!(serde_json::from_str::<AudioChannels>(&saved).unwrap(), AudioChannels::Surround714);
    }
}
