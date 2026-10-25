//! A stream: launching or resuming the app over HTTPS, starting the session on a worker
//! thread, then on the UI thread showing pictures, playing nothing (audio has its own
//! thread), sending keyboard, mouse and controller input, and drawing the overlay.
//!
//! Input handling follows moonlight-qt's SdlInputHandler (GPL-3.0, Cameron Gutman and
//! contributors): positional keys, the Ctrl+Alt+Shift combos, raising every key when focus
//! is lost, relative capture with absolute fallback.

pub mod packed444;
pub mod pipeline;
pub mod present;
mod microphone;
mod cursor;
mod bitrate;
mod recovery;
mod quality;
mod touch;
mod upscale;
mod phase;
pub mod scanline;
pub mod race;
pub mod race_probe;
pub(crate) mod pyrowave;
pub mod pyrowave_gpu;
pub mod pyrowave_mode;
mod stage_times;
#[cfg(windows)]
mod d3d11;
pub mod gpu_planes;
mod pyrowave_sdk;
#[cfg(unix)]
mod planes_gpu;

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nightjar_core::control::{ClientMessage, ControlEvent};
use nightjar_core::http::{App, AppRole, ClientIdentity, Feature, HostClient, LaunchRequest, ServerInfo};
use nightjar_core::input::packets::{modifier, mouse_button};
use nightjar_core::sdp::*;
use nightjar_core::session::{self, HostInfo, Interrupt, Location, Session, SessionConfig, Stage};
use rubylight_protocol::control::{HostTimingRequest, LinkNotice, LinkNoticeRequest, LinkState, Reconfigure, CLOCK_SYNC_MESSAGE_TYPE, HOST_TIMING_MESSAGE_TYPE, LINK_NOTICE_MESSAGE_TYPE, RECONFIGURE_MESSAGE_TYPE};
use rubylight_protocol::present_clock::ClockOffset;
use sdl3_sys::everything::*;

use crate::config::{self, AudioChannels, CodecPreference, HostRecord, MouseMode, QualityMode, Settings};
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
    pub info: ServerInfo,
    pub app: App,
    pub resume: bool,
    pub settings: Settings,
    pub identity: Arc<ClientIdentity>,
    pub display: Display,
    pub hardware: Hardware,
    /// The renderer's Direct3D 11 device when PyroWave decodes on the GPU (`pyrowave_decoder`).
    pub gpu_decoder: Option<pipeline::GpuDevice>,
    pub packed_modes: u8,
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
    pub recovery: Option<recovery::Recovery>,
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
    pub clock_sync: bool,
    pub display_caps: bool,
    pub cursor_shapes: bool,
    pub clipboard: bool,
    pub server_commands: Vec<String>,
    pub runtime_bitrate: bool,
    pub reconfigure: bool,
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
        if let Some(recovery) = &mut plan.recovery { recovery.begin(Instant::now()); }
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

    /// Ok(Some) once streaming, Err with a message on failure.
    pub fn poll(&mut self) -> Result<Option<(Session, Facts)>, String> {
        let expired = self.plan.recovery.as_ref().is_some_and(|r| r.remaining(Instant::now()).is_zero());
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
                    session.stop();
                    Err("Automatic recovery stopped after one minute.".into())
                } else {
                    self.plan = plan;
                    Ok(Some((session, facts)))
                }
            }
            Ok(Err(e)) => Err(if expired { "Automatic recovery stopped after one minute.".into() } else { e.message }),
            Err(std::sync::mpsc::TryRecvError::Empty) => Ok(None),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("The connection attempt ended.".into()),
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
}

impl From<String> for ConnectError {
    fn from(message: String) -> Self { Self { message, retryable: false } }
}

impl From<&str> for ConnectError {
    fn from(message: &str) -> Self { message.to_owned().into() }
}

fn connect_with_recovery(plan: &mut LaunchPlan, shared: Arc<Shared>, interrupt: &Interrupt) -> Result<(Session, Facts), ConnectError> {
    loop {
        if let Some(recovery) = &mut plan.recovery {
            let delay = recovery.next_delay(Instant::now()).ok_or("Automatic recovery exhausted its retry budget.")?;
            shared.notify(Notice::Reconnecting(recovery.attempts));
            let until = Instant::now() + delay;
            while Instant::now() < until {
                if interrupt.is_interrupted() { return Err("Cancelled.".into()); }
                std::thread::sleep(Duration::from_millis(20).min(until.saturating_duration_since(Instant::now())));
            }
        }
        match connect(plan, shared.clone(), interrupt) {
            Err(error) if error.retryable && plan.recovery.is_some() && !interrupt.is_interrupted() => {
                lock(&shared.frames).clear();
                lock(&shared.notices).clear();
            }
            result => return result,
        }
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

/// The link speed for `x-ss-video[0].pyrowaveLinkMbps`: zero (no attribute) when the setting is off.
/// The beam-race check and the calibrated pace still use the real speed.
fn link_speed_hint(setting: bool, link_mbps: u32) -> u32 {
    if setting { link_mbps } else { 0 }
}

/// A network hint the stream sends the host, unless the variable names another value (measurement
/// runs on loopback, where the real link is not the one under test). A value that is not a number
/// is said so on stderr and ignored.
fn hint_override(name: &str, value: u32) -> u32 {
    let Ok(text) = std::env::var(name) else { return value };
    text.trim().parse().unwrap_or_else(|_| {
        eprintln!("nightjar: {name}={text:?} is not a number, so it is ignored");
        value
    })
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

/// The setting, unless `NIGHTJAR_EARLY_DECODE` is `1` or `0` (for measurement runs).
fn early_decode(setting: bool) -> bool {
    match std::env::var("NIGHTJAR_EARLY_DECODE").as_deref() {
        Ok("1") => true,
        Ok("0") => false,
        _ => setting,
    }
}

fn connect(plan: &mut LaunchPlan, shared: Arc<Shared>, interrupt: &Interrupt) -> Result<(Session, Facts), ConnectError> {
    let settings = &plan.settings;
    let role = plan.app.role(false);
    let (width, height, fps, bitrate) = stream_mode(settings, plan.display);
    let bitrate = capped_bitrate(bitrate, plan.bitrate_limit_kbps);
    let audio = match settings.audio {
        AudioChannels::Stereo => AudioConfiguration::STEREO,
        AudioChannels::Surround51 => AudioConfiguration::SURROUND_51,
        AudioChannels::Surround71 => AudioConfiguration::SURROUND_71,
    };
    let ri_key: [u8; 16] = random_bytes();
    let key_id_bytes: [u8; 4] = random_bytes();
    let ri_key_id = i32::from_be_bytes(key_id_bytes) & 0x7FFF_FFFF;
    let refresh_x100 = (plan.display.refresh_hz * 100.0).round() as u32;
    let request = LaunchRequest {
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
        prepared_controllers: if role != AppRole::RemoteMonitor && plan.info.capabilities.has(Feature::ControllerPreparation) {
            plan.prepared_controllers.clone()
        } else { String::new() },
        virtual_display: settings.virtual_display,
        virtual_display_scale: settings.virtual_display_scale,
        host_display: settings.host_display.into(),
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
            retryable: recovery::transient_http(&e), message: format!("Checking the running app failed: {e}"),
        })?;
        if !plan.info.paired || !plan.app.matches_running_app(plan.info.current_game, plan.info.current_game_uuid.as_deref()) {
            return Err("The original app is no longer running on the PC. Choose an app to start a new stream.".into());
        }
    }
    let launched = if plan.resume {
        client.resume(&request, &plan.info)
    } else {
        client.launch(&request, &plan.info)
    };
    let launched = match launched {
        Ok(Some(launched)) => launched,
        Ok(None) => return Err("The PC declined to start the app.".into()),
        Err(nightjar_core::http::Error::Status { code, message }) => {
            return Err(format!("The PC could not start the app ({code}: {message}).").into())
        }
        Err(e) => return Err(ConnectError { retryable: recovery::transient_http(&e), message: format!("Starting the app failed: {e}") }),
    };
    if interrupt.is_interrupted() {
        return Err("Cancelled.".into());
    }
    let scm = plan
        .info
        .capabilities
        .trusted_codec_mode_support(plan.info.server_codec_mode_support);
    // The interface the system sends this host's traffic over decides, not the internet
    // connection; the connection is the fallback when that lookup tells nothing.
    let toward = crate::network::toward(&plan.record.address);
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
    let link_mbps = hint_override("NIGHTJAR_PYROWAVE_LINK_MBPS", routed_link_mbps);
    let measured_pace = plan.pace.as_ref().map_or(0, |pace| pace.for_network(&network, plan.host_link_mbps));
    let pace_mbps = hint_override("NIGHTJAR_PYROWAVE_PACE_MBPS", measured_pace);
    let beam = beam_race(settings.beam_race, link_mbps, plan.gpu_decoder.is_some());
    match beam {
        BeamRace::NotWired => eprintln!("{BEAM_RACE_NOT_WIRED}"),
        BeamRace::NoGpuDecoder => eprintln!("{BEAM_RACE_NO_GPU}"),
        BeamRace::On => eprintln!("nightjar: beam race is on (experimental): asking the host for the row order"),
        BeamRace::Off => {}
    }
    let race_ask = race_request(beam, early_decode(settings.early_decode), height);
    let mut iv = [0u8; 16];
    iv[..4].copy_from_slice(&ri_key_id.to_be_bytes());
    let stream = StreamConfig {
        width: width as i32,
        height: height as i32,
        fps: fps as i32,
        bitrate_kbps: bitrate as i32,
        packet_size: 1392,
        audio,
        supported_video_formats: video_formats(
            settings.codec,
            settings.hdr,
            if settings.experimental_packed_444 {
                0
            } else {
                scm
            },
        ),
        experimental_packed_444: settings.experimental_packed_444,
        packed444_modes: plan.packed_modes,
        client_refresh_rate_x100: refresh_x100 as i32,
        color_space: if settings.hdr { COLORSPACE_REC_2020 } else { COLORSPACE_REC_709 },
        color_range: if settings.full_range { COLOR_RANGE_FULL } else { COLOR_RANGE_LIMITED },
        microphone: settings.microphone && role != AppRole::RemoteMonitor,
        haptics_pcm: settings.controllers && settings.controller_audio && role != AppRole::RemoteMonitor,
        pyrowave_link_mbps: link_speed_hint(settings.tell_host_link_speed, link_mbps) as i32,
        pyrowave_pace_mbps: pace_mbps as i32,
        client_wireless,
        pyrowave_records_supported: plan.info.capabilities.has(Feature::PyroWaveRecords),
        pyrowave_row_order: race_ask.row_order,
        encryption_flags: ENCFLG_ALL,
        remote_input_aes_key: ri_key,
        remote_input_aes_iv: iv,
        video_capabilities: 0,
        audio_capabilities: CAPABILITY_DIRECT_SUBMIT,
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
    let session = Session::start(
        &host,
        SessionConfig {
            stream,
            location: location_override(),
            network_grace: Duration::from_millis(u64::from(settings.network_grace_ms)),
            input_only: role == AppRole::InputOnly,
            ..Default::default()
        },
        Box::new(pipeline::Decoder::new(shared.clone(), plan.hardware).with_early_decode(early).with_beam_race(slices).with_gpu_decoder(plan.gpu_decoder)),
        Box::new(pipeline::Audio::new(shared.clone())),
        Arc::new(pipeline::Events { shared }),
        interrupt,
    )
    .map_err(|e| ConnectError { retryable: recovery::transient_start(e.stage, e.code), message: format!("Could not start the stream: {e}") })?;
    let outcome = session.outcome();
    let facts = Facts {
        role,
        width: outcome.config.width,
        height: outcome.config.height,
        fps: outcome.config.fps,
        bitrate_kbps: outcome.config.bitrate_kbps,
        format: outcome.negotiated_video_format,
        host_timing: outcome.host_control_messages.contains(&HOST_TIMING_MESSAGE_TYPE),
        link_notice: outcome.host_control_messages.contains(&LINK_NOTICE_MESSAGE_TYPE),
        clock_sync: outcome.host_control_messages.contains(&CLOCK_SYNC_MESSAGE_TYPE),
        display_caps: outcome.host_control_messages.contains(&rubylight_protocol::control::DISPLAY_CAPS_MESSAGE_TYPE),
        cursor_shapes: plan.info.capabilities.has(Feature::CursorShape) && outcome.host_control_messages.contains(&0x5540),
        clipboard: plan.info.capabilities.has(Feature::Clipboard) && role != AppRole::RemoteMonitor,
        server_commands: if plan.info.capabilities.has(Feature::ServerCommands) && role != AppRole::RemoteMonitor {
            plan.info.server_commands.iter().take(256).cloned().collect()
        } else { Vec::new() },
        runtime_bitrate: plan.info.capabilities.has(Feature::RuntimeBitrate) && role != AppRole::InputOnly,
        reconfigure: outcome.host_control_messages.contains(&RECONFIGURE_MESSAGE_TYPE)
            && !outcome.config.experimental_packed_444 && outcome.negotiated_video_format & VIDEO_FORMAT_MASK_PYROWAVE == 0 && role == AppRole::Stream,
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
        app_name: plan.app.name.clone(),
        host_name: plan.host_name.clone(),
    };
    if facts.early_without_records {
        eprintln!("{EARLY_NO_RECORDS}");
    }
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
}

impl Exit {
    pub fn retryable(&self) -> bool {
        matches!(self, Self::Terminated(code) if recovery::transient_termination(*code))
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

#[derive(Default, Clone)]
pub struct Overlay {
    pub lines: Vec<(String, String)>,
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
    upscale_failed: bool,
    phase: phase::Phase,
    /// Whether racing the display's beam can work here, once the beam race is on and a picture is shown.
    race_probe: Option<race_probe::RaceProbe>,
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
    pub overlay: Overlay,
    /// What the host last said about holding the bitrate down for the network; `None` while it has not.
    link_notice: Option<LinkNotice>,
    pub exit: Option<Exit>,
    pub toast: Option<(String, Instant)>,
    pub host_id: u64,
    settings: Settings,
    plan: LaunchPlan,
    microphone: Option<microphone::Microphone>,
    cursor: cursor::Cursor,
    cursor_warning: bool,
    clipboard_busy: bool,
    command_busy: bool,
    last_command: Option<Instant>,
    audio_status: ([u8; 16], Instant),
    controller_mouse_active: bool,
    pub pending_command: Option<usize>,
    pub bitrate_busy: bool,
    pub bitrate_kbps: u32,
    pub adaptive_bitrate: bool,
    bitrate_policy: Option<bitrate::Bitrate>,
    pub quality_mode: QualityMode,
    quality_policy: Option<quality::Quality>,
    quality_ceiling: Reconfigure,
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
    captured: bool,
    absolute: bool,
    relative_remainder: (f32, f32),
    offset: ClockOffset,
    /// The client timing CSV (`NIGHTJAR_TIMING_CSV`), off by default.
    timing_csv: Option<crate::timing_csv::TimingCsv>,
    rtp_clock: RtpClock,
    last_probe: Instant,
    window: Window,
    last_counters: (u64, u64, u64, u64, u64, u64),
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

impl Streaming {
    pub fn new(connecting: Connecting, session: Session, facts: Facts, display: Display) -> Self {
        let min_interval = if display.refresh_hz > 0.0 {
            (1_000_000.0 / display.refresh_hz) as i64
        } else {
            0
        };
        if facts.host_timing {
            let _ = session.send(ClientMessage::HostTimingRequest(HostTimingRequest { enabled: true }));
        }
        if facts.link_notice {
            let _ = session.send(ClientMessage::LinkNoticeRequest(LinkNoticeRequest { enabled: true }));
        }
        let settings = connecting.settings.clone();
        let microphone = if settings.microphone && !connecting.plan.microphone_muted && facts.role != AppRole::RemoteMonitor {
            if session.microphone_available() {
                match microphone::Microphone::open(connecting.shared.clone()) {
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
        let quality_mode = if facts.reconfigure { settings.adaptive_quality } else { QualityMode::Off };
        let quality_policy = (quality_mode != QualityMode::Off).then(|| quality::Quality::new(
            quality_mode, mode, settings.quality_min_short_edge, settings.quality_min_fps, 0));
        let backlog = connecting.shared.pacer_backlog.clone();
        Self {
            session,
            shared: connecting.shared,
            facts,
            pacer: Pacer::with_backlog(connecting.pacing, min_interval, backlog),
            texture: VideoTexture::new(connecting.settings.upscaling == config::Upscaling::Bilinear),
            upscaler: None,
            upscale_failed: false,
            phase: phase::Phase::default(),
            race_probe: None,
            race_scan: None,
            racer: race::Racer::default(),
            predictor: None,
            latency: race::Latency::default(),
            race_margin_ms: hint_override("NIGHTJAR_BEAM_RACE_MARGIN_MS", settings.beam_race_margin_ms),
            race_present: env_flag("NIGHTJAR_BEAM_RACE_PRESENT").unwrap_or(true),
            race_setup: None,
            overlay_visible: settings.overlay,
            overlay: Overlay::default(),
            link_notice: None,
            exit: None,
            toast: None,
            host_id: connecting.host_id,
            absolute: settings.mouse_mode == MouseMode::Absolute,
            settings,
            microphone,
            cursor: cursor::Cursor::default(),
            cursor_warning: false,
            clipboard_busy: false,
            command_busy: false,
            last_command: None,
            audio_status: ([0; 16], Instant::now() - Duration::from_secs(1)),
            controller_mouse_active: false,
            pending_command: None,
            bitrate_busy: false,
            bitrate_kbps,
            adaptive_bitrate: false,
            bitrate_policy: None,
            quality_mode,
            quality_policy,
            quality_ceiling: mode,
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
            captured: false,
            relative_remainder: (0.0, 0.0),
            offset: ClockOffset::default(),
            timing_csv: open_timing_csv(),
            rtp_clock: RtpClock::default(),
            last_probe: Instant::now(),
            window: Window::default(),
            last_counters: (0, 0, 0, 0, 0, 0),
            host_average: HostAverage::default(),
            last_early: (0, 0),
            last_video: (0, 0),
            started: Instant::now(),
            first_frame: false,
            needs_redraw: true,
            timings: VecDeque::new(),
        }
    }

    pub fn show_toast(&mut self, text: impl Into<String>) {
        self.toast = Some((text.into(), Instant::now()));
        self.needs_redraw = true;
    }

    pub fn reconnect_plan(&self, automatic: bool) -> Option<LaunchPlan> {
        if automatic && (!self.settings.auto_reconnect || !self.focused || self.facts.role != AppRole::Stream) {
            return None;
        }
        let mut plan = self.plan.clone();
        plan.resume = self.facts.role == AppRole::Stream;
        if plan.resume && plan.app.control != nightjar_core::http::AppControl::None {
            if plan.info.current_game == 0 { return None; }
            plan.app.id = plan.info.current_game;
            plan.app.uuid = plan.info.current_game_uuid.clone().unwrap_or_default();
            plan.app.control = nightjar_core::http::AppControl::None;
        }
        plan.microphone_muted = self.microphone_muted;
        if automatic {
            let recovery = plan.recovery.get_or_insert_with(Default::default);
            recovery.reset_after(self.started.elapsed());
        } else {
            plan.recovery = None;
        }
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
            if !self.request_mode(self.quality_ceiling) { self.quality_mode = QualityMode::Off; return; }
            self.quality_policy = Some(quality::Quality::new(mode, self.quality_ceiling,
                self.settings.quality_min_short_edge, self.settings.quality_min_fps, self.started.elapsed().as_millis() as u64));
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
        if let Some(rate) = &mut self.bitrate_policy { rate.set_fps(request.fps_millihz); }
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
            let request = policy.rebase(self.quality_mode, request, self.settings.quality_min_short_edge,
                self.settings.quality_min_fps, self.started.elapsed().as_millis() as u64);
            if !self.request_mode(request) { self.quality_policy = None; self.quality_mode = QualityMode::Off; }
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
        self.settings.controllers && self.settings.controller_audio && self.facts.role != AppRole::RemoteMonitor
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
            match microphone::Microphone::open(self.shared.clone()) {
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
    pub fn drain_notices(&mut self, pads: &mut Gamepads) {
        let notices = std::mem::take(&mut *lock(&self.shared.notices));
        for notice in notices {
            match notice {
                Notice::Terminated(code) => {
                    self.exit.get_or_insert(Exit::Terminated(code));
                }
                Notice::DecoderFallback(text) => self.show_toast(text),
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
                Notice::AdaptiveBitrate(result) => {
                    self.bitrate_busy = false;
                    match result {
                        Ok((ceiling, current)) => {
                            self.facts.bitrate_kbps = current as i32;
                            self.bitrate_kbps = current;
                            self.bitrate_policy = Some(bitrate::Bitrate::new(ceiling, current, self.facts.fps, self.started.elapsed().as_millis() as u64));
                        }
                        Err(error) => {
                            self.adaptive_bitrate = false;
                            self.show_toast(error);
                        }
                    }
                }
                Notice::Bitrate { requested, result } => {
                    self.bitrate_busy = false;
                    match result {
                        Ok(kbps) => {
                            self.facts.bitrate_kbps = kbps as i32;
                            self.bitrate_kbps = kbps;
                            if let Some(policy) = &mut self.bitrate_policy {
                                policy.applied(requested, kbps, self.started.elapsed().as_millis() as u64);
                            } else {
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
                    self.timings.push_back((r.frame, u64::from(r.host_us)));
                    if self.timings.len() > 64 {
                        self.timings.pop_front();
                    }
                }
            }
            ControlEvent::LinkNotice(notice) => {
                self.link_notice = (notice.state != LinkState::Full).then_some(notice);
            }
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

    pub fn enable_adaptive_bitrate(&mut self, enabled: bool) {
        if self.bitrate_busy { return; }
        self.bitrate_policy = None;
        self.adaptive_bitrate = enabled && self.facts.runtime_bitrate && self.facts.role == AppRole::Stream;
        if !self.adaptive_bitrate { return; }
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

    pub fn toggle_menu(&mut self, window: *mut SDL_Window, pads: &mut Gamepads) {
        // While End or Close game runs, the Session page stays so its answer (or failure) shows.
        if self.menu_visible && self.session_page.as_ref().is_some_and(|p| p.busy()) {
            return;
        }
        self.menu_visible = !self.menu_visible;
        if !self.menu_visible {
            self.session_page = None;
        }
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
        match due {
            Some(due) => {
                let now = session::now_us();
                if due <= now {
                    Wait::Now
                } else {
                    Wait::For(Duration::from_micros(due - now))
                }
            }
            None => Wait::For(Duration::from_millis(if self.controller_mouse_active || self.audio_status.0.iter().any(|flags| *flags != 0) { 20 } else if self.overlay_visible { 250 } else { 500 })),
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
                let use_upscaler = self.settings.upscaling != config::Upscaling::Bilinear && !self.upscale_failed
                    && !self.texture.is_hdr() && output.0 >= input.0 && output.1 >= input.1 && output != input
                    && output.0 <= 16384 && output.1 <= 16384;
                if use_upscaler {
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
                SDL_RenderTexture(renderer, texture, std::ptr::null(), &dst);
            } else if !self.texture.draw(renderer, &dst) {
                self.exit = Some(Exit::Ended(Some("Packed full-chroma presentation failed.".into())));
            }
        }
        draw_overlay(self, renderer);
        // The beam race probe reads the scan line around the present (nothing when the race is off).
        if self.facts.beam_race && probe && self.race_probe.is_none() {
            self.race_probe = Some(race_probe::RaceProbe::begin(renderer, self.pacer.mode != PacingMode::Vsync, f64::from(self.facts.fps)));
        }
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
            lines.push(("Upscaling".into(), if self.upscale_failed { "Bilinear (GPU unavailable)".into() }
                else if self.texture.is_hdr() { "Bilinear (HDR)".into() }
                else if self.upscaler.is_some() { self.settings.upscaling.label().into() }
                else { "Bilinear (no upscale needed)".into() }));
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
        if let Some(probe) = &self.race_probe {
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
        let mut pacing = self.pacer.mode.label().to_owned();
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
        lines.push(("Audio".into(), audio));
        lines.push(("Bitrate".into(), format!("{:.1} Mbps{}", f64::from(self.facts.bitrate_kbps) / 1000.0,
            if self.adaptive_bitrate { ", automatic" } else { "" })));
        if let Some(notice) = &self.link_notice {
            lines.push(("Notice".into(), link_notice_line(notice)));
        }
        self.overlay = Overlay { lines };
        let timing = std::mem::take(&mut *lock(&self.shared.decode_window));
        let recent = decoded > 0 && self.last_arrival.is_some_and(|(_, at)| session::now_us().saturating_sub(at) < 500_000);
        let poor = stats.connection_status == Some(nightjar_core::control::ConnectionStatus::Poor);
        let quality_target = if let Some(policy) = &mut self.quality_policy {
            if self.menu_visible || !self.focused || self.pending_mode.is_some() { policy.suspend(); None }
            else { policy.sample(quality::Sample {
                now: self.started.elapsed().as_millis() as u64, recent, poor,
                loss: if completed + drops > 0 { loss } else { -1.0 },
                decode_ms: if decoded > 0 { decode_ms } else { -1.0 }, jitter_ms: self.jitter_ms,
                queue_ms: if timing.samples > 0 { timing.queue_us as f64 / f64::from(timing.samples) / 1000.0 } else { -1.0 },
                bitrate_has_room: self.bitrate_busy || self.bitrate_policy.as_ref().is_some_and(|p| p.has_room()),
                samples: timing.samples, late: timing.late, span_us: timing.last_us.saturating_sub(timing.first_us),
            }) }
        } else { None };
        if let Some(target) = quality_target {
            if self.request_mode(target) {
                self.quality_policy.as_mut().expect("active quality policy").applied(target, self.started.elapsed().as_millis() as u64);
            } else { self.quality_policy = None; self.quality_mode = QualityMode::Off; }
        }
        if !self.bitrate_busy {
            if let Some(policy) = &mut self.bitrate_policy {
                if self.menu_visible || !self.focused {
                    policy.suspend();
                } else {
                    let recent = decoded > 0 && self.last_arrival.is_some_and(|(_, at)| session::now_us().saturating_sub(at) < 500_000);
                    let poor = stats.connection_status == Some(nightjar_core::control::ConnectionStatus::Poor);
                    let loss = if completed + drops > 0 { loss } else { -1.0 };
                    let decode_ms = if decoded > 0 { decode_ms } else { -1.0 };
                    let now = self.started.elapsed().as_millis() as u64;
                    let target = if self.facts.format & VIDEO_FORMAT_MASK_PYROWAVE != 0 {
                        let record_loss = if timing.record_samples > 0 { timing.record_loss_milli as f64 / f64::from(timing.record_samples) / 1000.0 } else { -1.0 };
                        policy.pyrowave_sample(now, recent, poor, [loss.max(record_loss),
                            if timing.samples > 0 { timing.queue_us as f64 / f64::from(timing.samples) / 1000.0 } else { -1.0 },
                            decode_ms, self.jitter_ms])
                    } else { policy.sample(now, recent, poor, loss, decode_ms, self.jitter_ms) };
                    if let Some(target) = target {
                        self.bitrate_kbps = target;
                        self.set_bitrate();
                    }
                }
            }
        }
        self.window.started = Some(Instant::now());
        true
    }

    /// Whether the stream has shown its first picture.
    pub fn has_picture(&self) -> bool {
        self.first_frame
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
            match kind {
                SDL_EVENT_QUIT | SDL_EVENT_WINDOW_CLOSE_REQUESTED => {
                    self.exit = Some(Exit::CloseWindow);
                }
                SDL_EVENT_KEY_DOWN | SDL_EVENT_KEY_UP => self.key(&event.key, window, pads),
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
                    pads.added(event.gdevice.which);
                    if self.controller_audio_available() { pads.auto_audio(); }
                    if self.settings.controllers && self.facts.role != AppRole::RemoteMonitor {
                        self.session.input(|q| pads.announce_new(q));
                    }
                }
                SDL_EVENT_GAMEPAD_REMOVED => {
                    self.session.input(|q| pads.release_mouse(event.gdevice.which, q));
                    if let Some(index) = pads.removed(event.gdevice.which) {
                        if self.settings.controllers && self.facts.role != AppRole::RemoteMonitor {
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
                    self.session.input(|q| pads.announce_new(q));
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
                        Some(Action::Mouse(active)) => self.show_toast(if active { tr("Controller mouse enabled. A/B click; D-pad scrolls. Hold Start to return to gamepad.") } else { tr("Controller mouse disabled.") }),
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
            return;
        }
        let m = k.r#mod.0;
        let has = |flag: SDL_Keymod| m & flag.0 != 0;
        if k.down && has(SDL_KMOD_CTRL) && has(SDL_KMOD_ALT) && has(SDL_KMOD_SHIFT) {
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
        let system_keys = self.settings.capture_system_keys;
        let right_alt_as_meta = self.settings.right_alt_as_meta;
        let scancode = if right_alt_as_meta && k.scancode == SDL_SCANCODE_RALT { SDL_SCANCODE_RGUI } else { k.scancode };
        let Some((code, flags)) = keymap::virtual_key(scancode, system_keys || (right_alt_as_meta && k.scancode == SDL_SCANCODE_RALT)) else {
            return;
        };
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
        if k.down {
            self.keys_down.insert(code);
        } else {
            self.keys_down.remove(&code);
        }
        self.session.input(|q| {
            let _ = q.send_keyboard(0x8000 | code, k.down, modifiers, flags);
        });
    }

    /// Ends the stream: releases input, stops the session.
    pub fn finish(mut self, window: *mut SDL_Window, pads: &mut Gamepads) {
        self.microphone = None;
        if let Some(client) = &*lock(&self.shared.host_client) {
            client.cancel_pending_requests();
        }
        self.raise_all_keys();
        self.session.input(|q| pads.release(q));
        self.set_capture(window, false);
        self.cursor.clear();
        for controller in 0..16 {
            if self.audio_status.0[usize::from(controller)] != 0 {
                let _ = self.session.send(ClientMessage::ControllerAudioStatus { controller, flags: 0 });
            }
        }
        // SAFETY: a live window.
        unsafe { SDL_ShowCursor() };
        pads.reset();
        self.upscaler = None;
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
        self.session.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn the_link_speed_hint_is_left_out_when_the_setting_is_off() {
        assert_eq!(link_speed_hint(true, 1000), 1000);
        assert_eq!(link_speed_hint(false, 1000), 0);
        assert_eq!(link_speed_hint(true, 0), 0);
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

    #[test]
    fn rtp_clock_unwraps() {
        let mut c = RtpClock::default();
        let a = c.host_us(u32::MAX - 899);
        let b = c.host_us(600);
        // 1500 ticks of 90 kHz across the wrap, give or take the rounding.
        assert!((b - a - 16_667).abs() <= 1, "{}", b - a);
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
