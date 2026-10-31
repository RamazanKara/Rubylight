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
mod hlg;
mod microphone;
#[cfg(windows)]
mod voice_capture;
mod cursor;
mod bitrate;
pub mod host_abr;
pub mod doctor;
pub mod overlay;
mod recovery;
mod quality;
mod rate_change;
pub mod menu_extras;
mod touch;
/// Precision touchpad passthrough is Windows only; its frame logic is tested everywhere.
#[cfg(any(windows, test))]
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
mod negotiate;
mod connect;
mod stats;
mod input;
mod render;
mod notices;

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
use connect::*;
use negotiate::*;
use stats::*;
pub use connect::{offers_714, termination_text, ConnectFailed, Connecting};
pub use negotiate::{codec_name, probes_color, pyrowave_bitrate_warning, pyrowave_notice, stream_mode};

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
    /// An earlier connection of this automatic reconnect chain showed video: a reconnect that has
    /// no picture yet when it drops again still reconnects ([`Streaming::video_started`]).
    pub video_shown: bool,
    /// The user confirmed closing the running app for this launch (`rlConfirmReplace`).
    pub confirm_replace: bool,
    /// The display's luminance for hosts other than Rubylight (`sdl::launch_luminance`).
    pub display_luminance: Option<nightjar_core::http::DisplayLuminance>,
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
    /// The PC's night mode and headphone surround switches in the stream controls.
    pub host_audio: menu_extras::HostAudio,
    pub pending_command: Option<usize>,
    pub bitrate_busy: bool,
    pub bitrate_kbps: u32,
    pub adaptive_bitrate: bool,
    bitrate_policy: Option<bitrate::Bitrate>,
    /// A Foundation PC's own bitrate controller, on in place of `bitrate_policy`.
    host_abr: Option<host_abr::HostAbr>,
    /// The request that turns it on, while the stream may end before the PC answers.
    host_abr_start: Option<host_abr::AbrStart>,
    /// The request that turns it off, while it may still be in flight.
    host_abr_stop: Option<host_abr::AbrStop>,
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
    /// Emptied buffers `pump` swaps with the decoder's queues, so taking a frame allocates nothing
    /// and holds the decoder's lock only for the swap.
    taken_frames: VecDeque<pipeline::Decoded>,
    taken_slices: VecDeque<pyrowave::SliceReady>,
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
    #[cfg(windows)]
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
            host_audio: Default::default(),
            pending_command: None,
            bitrate_busy: false,
            bitrate_kbps,
            adaptive_bitrate: false,
            bitrate_policy: None,
            host_abr: None,
            host_abr_start: None,
            host_abr_stop: None,
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
            taken_frames: VecDeque::new(),
            taken_slices: VecDeque::new(),
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
            #[cfg(windows)]
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
        self.video_started() && automatic_rejoin(&self.plan, self.facts.role, self.settings.auto_reconnect, away, self.connected_ms(), false, true)
            .and_then(|plan| plan.recovery)
            .is_some_and(|budget| budget.allows_attempt(recovery::now_ms()))
    }

    /// Whether video started: this connection showed a picture, or an earlier one of its automatic
    /// reconnect chain did ([`LaunchPlan::video_shown`]). A reconnect after a real drop that has no
    /// picture yet when it drops again keeps reconnecting until its budget is spent; a stream that
    /// never showed video does not reconnect (Moonlight V+ PC 6792aeed).
    pub fn video_started(&self) -> bool {
        self.first_frame || self.plan.video_shown
    }

    fn connected_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// The plan that starts the stream again; `automatic` after a drop, `away` while the window is
    /// minimized or hidden.
    pub fn reconnect_plan(&self, automatic: bool, away: bool) -> Option<LaunchPlan> {
        let mut plan = if automatic {
            automatic_rejoin(&self.plan, self.facts.role, self.settings.auto_reconnect, away, self.connected_ms(), self.retry_now, self.video_started())?
        } else {
            let mut plan = rejoin(self.plan.clone(), self.facts.role)?;
            plan.recovery = None;
            plan.video_shown = false;
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
        let stopping = self.host_abr_stop.take();
        std::thread::spawn(move || {
            // An off sent just before must not land after this on.
            if let Some(stop) = stopping { stop.wait(); }
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

    /// Turns a Foundation PC's controller off again (V+ PC's `stopSunshineAbr`), on its own
    /// thread and client: `finish` cancels the stream's requests right after, and a dead network
    /// would hold the window for the request's timeouts.
    fn stop_host_abr(&mut self) {
        if self.host_abr.take().is_some() { self.adaptive_bitrate = false; }
        // On, though its answer may not have come yet; a start still in flight turns it off itself.
        if !self.host_abr_start.take().is_some_and(|start| start.end()) { return; }
        let Some(client) = lock(&self.shared.host_client).as_ref().map(|c| c.detached()) else { return };
        self.host_abr_stop = Some(host_abr::AbrStop::spawn(move || {
            if let Err(e) = client.configure_abr(&AbrRequest::off()) {
                eprintln!("nightjar: the PC's automatic bitrate did not stop: {e}");
            }
        }));
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
        if self.menu_visible {
            self.refresh_host_audio();
        } else {
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

    /// Ends the stream: releases input, stops the session. Returns the request that turns the
    /// PC's automatic bitrate off while it runs on; one closing Nightjar waits for it.
    pub fn finish(mut self, window: *mut SDL_Window, pads: &mut Gamepads) -> Option<host_abr::AbrStop> {
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
        self.host_abr_stop.take()
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

/// Whether something automatic owns the bitrate, so the doctor's trial stays out of its way:
/// the PC's adaptive bitrate, automatic quality, or the client-hint controller over 0x553C.
fn bitrate_automatic(adaptive_bitrate: bool, quality_mode: QualityMode, rate_change: Option<&rate_change::RateChange>) -> bool {
    adaptive_bitrate || quality_mode != QualityMode::Off || rate_change.is_some_and(|rates| rates.hint_active())
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::input::*;

    /// Desktop never asked for an HDR format, so no host sent it Dolby Vision RPUs and the draw
    /// pass's RPU peak was unreachable. It asks as Android does, and stock hosts see nothing new.
    #[test]
    fn hdr_streams_ask_a_rubylight_host_for_dolby_vision_where_the_rpus_are_read() {
        use rubylight_protocol::hdr::{ALL, DV_101, DV_81, DV_84, HLG, PQ};
        let both = VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10 | VIDEO_FORMAT_AV1_MAIN8 | VIDEO_FORMAT_AV1_MAIN10;
        let hevc = VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10;
        let host = i32::from(ALL);
        // AV1 first, as the handshake takes it, then HEVC; each only where the host offers it.
        assert_eq!(hdr_format(true, host, both, false, true), DV_101);
        assert_eq!(hdr_format(true, host, hevc, false, true), DV_81);
        assert_eq!(hdr_format(true, i32::from(PQ | HLG | DV_81), both, false, true), DV_81);
        // Never profile 8.4 (an HLG base layer) nor HLG: every desktop HDR output takes PQ.
        assert_eq!(hdr_format(true, i32::from(PQ | HLG | DV_84), both, false, true), 0);
        // Stock HDR10 for a stock host (no HdrFormats: -1), an SDR stream, packed full chroma,
        // 8-bit formats only, and where nothing reads the RPUs (Linux, SDL drawing on Windows).
        assert_eq!(hdr_format(true, -1, both, false, true), 0);
        assert_eq!(hdr_format(true, 0, both, false, true), 0);
        assert_eq!(hdr_format(false, host, both, false, true), 0);
        assert_eq!(hdr_format(true, host, both, true, true), 0);
        assert_eq!(hdr_format(true, host, VIDEO_FORMAT_H265 | VIDEO_FORMAT_AV1_MAIN8, false, true), 0);
        assert_eq!(hdr_format(true, host, both, false, false), 0);
        #[cfg(not(windows))]
        assert!(!reads_dolby_vision(), "only the Windows draw pass reads them");
    }

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
        // PyroWave chosen: a chosen bitrate stays, below its floor too (the settings warn), and the
        // automatic one is PyroWave's default (what the settings show).
        let mut settings = Settings { codec: CodecPreference::PyroWave444, ..settings };
        assert_eq!(stream_mode(&settings, display()).3, 30000);
        settings.bitrate_kbps = 0;
        assert_eq!(stream_mode(&settings, display()).3, 398_132);
        settings.bitrate_kbps = 250_000;
        assert_eq!(stream_mode(&settings, display()).3, 250_000);
        settings.bitrate_kbps = 2_000_000;
        assert_eq!(stream_mode(&settings, display()).3, 2_000_000);
    }

    // User report: PyroWave started at the H.264 default (20 Mbps at 1080p60), far below its floor,
    // because the automatic bitrate did not know the codec.
    #[test]
    fn a_pyrowave_stream_at_the_automatic_bitrate_starts_at_pyrowaves_default() {
        let auto = Settings::default();
        // With the link unknown the default stays within 900 Mbps, unless the floor is higher.
        for (w, h, fps, kbps) in [(1920, 1080, 60, 398_132), (2560, 1440, 120, 900_000), (3840, 2160, 60, 900_000), (3840, 2160, 120, 1_492_992)] {
            assert_eq!(start_bitrate(&auto, w, h, fps, true, 0), kbps, "{w}x{h}@{fps}");
            assert_eq!(start_bitrate(&auto, w, h, fps, false, 0), config::default_bitrate_kbps(w, h, fps));
            assert!(kbps >= nightjar_policy::pyrowave::minimum_kbps(w, h, fps));
            // The settings show the same for PyroWave chosen.
            let chosen = Settings { width: w, height: h, fps, codec: CodecPreference::PyroWave, ..Default::default() };
            assert_eq!(stream_mode(&chosen, display()).3, kbps);
        }
        // Chosen bitrates stay as they are, whatever the link, also below the floor (the settings warn).
        let chosen = |kbps| Settings { bitrate_kbps: kbps, ..Default::default() };
        assert_eq!(start_bitrate(&chosen(300_000), 1920, 1080, 60, true, 0), 300_000);
        assert_eq!(start_bitrate(&chosen(900_000), 3840, 2160, 60, true, 0), 900_000);
        assert_eq!(start_bitrate(&chosen(1_500_000), 3840, 2160, 60, true, 1000), 1_500_000);
        assert_eq!(start_bitrate(&chosen(20_000), 1920, 1080, 60, true, 0), 20_000);
        assert_eq!(start_bitrate(&chosen(20_000), 1920, 1080, 60, false, 0), 20_000);
    }

    // A default over gigabit broke a first PyroWave stream on ordinary Ethernet and Wi-Fi: the
    // default stays within 90% of a known wired link, never below the floor.
    #[test]
    fn pyrowaves_default_stays_within_the_wired_link() {
        let auto = Settings::default();
        assert_eq!(start_bitrate(&auto, 3840, 2160, 60, true, 1000), 900_000);
        assert_eq!(start_bitrate(&auto, 3840, 2160, 60, true, 2500), 1_592_525);
        assert_eq!(start_bitrate(&auto, 2560, 1440, 120, true, 2500), 1_415_578);
        assert_eq!(start_bitrate(&auto, 1920, 1080, 60, true, 100), 186_624);
        assert_eq!(start_bitrate(&auto, 1920, 1080, 60, false, 100), config::default_bitrate_kbps(1920, 1080, 60));
        // 10G gets the full clean target at 4K60, 1G about 900 Mbps at 1440p120, unknown 900.
        assert_eq!(start_bitrate(&auto, 3840, 2160, 60, true, 10_000), 1_592_525);
        assert_eq!(start_bitrate(&auto, 2560, 1440, 120, true, 1000), 900_000);
        assert_eq!(start_bitrate(&auto, 2560, 1440, 120, true, 0), 900_000);
    }

    // The bitrate follows what the launch offers the PC, not the codec choice alone: a PyroWave
    // choice the PC lacks streams HEVC at the conventional default, and any offer the PC can answer
    // with PyroWave starts at PyroWave's (the automatic choice included, should it offer PyroWave).
    #[test]
    fn the_launch_starts_pyrowave_bitrates_only_when_the_offer_streams_pyrowave() {
        let with = SCM_H264 | SCM_HEVC | SCM_PYROWAVE;
        let offered = |codec, scm| offered_formats(&Settings { codec, ..Default::default() }, scm, &decodes_all()).formats;
        assert!(offers_pyrowave(offered(CodecPreference::PyroWave, with), with));
        assert!(offers_pyrowave(offered(CodecPreference::PyroWave444, SCM_H264 | SCM_PYROWAVE_444), SCM_H264 | SCM_PYROWAVE_444));
        assert!(!offers_pyrowave(offered(CodecPreference::PyroWave, SCM_H264 | SCM_HEVC), SCM_H264 | SCM_HEVC), "the PC has no PyroWave");
        assert!(!offers_pyrowave(offered(CodecPreference::PyroWave, 0), 0), "no trusted codec support: the handshake never picks PyroWave");
        assert!(!offers_pyrowave(offered(CodecPreference::Auto, with), with), "automatic offers HEVC and H.264 only");
        assert!(offers_pyrowave(VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_PYROWAVE, with), "an automatic offer that has PyroWave");
        assert_eq!(start_bitrate(&Settings::default(), 1920, 1080, 60, offers_pyrowave(VIDEO_FORMAT_H264 | VIDEO_FORMAT_PYROWAVE, with), 0), 398_132);
    }

    // A chosen bitrate below PyroWave's minimum is kept; the bitrate setting warns, naming the
    // minimum for the mode.
    #[test]
    fn a_chosen_bitrate_below_pyrowaves_minimum_is_warned_in_the_settings() {
        let low = Settings { codec: CodecPreference::PyroWave, bitrate_kbps: 20_000, ..Default::default() };
        assert_eq!(pyrowave_bitrate_warning(&low, 1920, 1080, 60).as_deref(),
            Some("Below PyroWave’s minimum of 187 Mbps at 1920x1080, 60 fps: the picture will break up."));
        let german = crate::strings::tests::with_lang(crate::strings::Lang::De, || pyrowave_bitrate_warning(&low, 1920, 1080, 60));
        assert_eq!(german.as_deref(), Some("Unter dem PyroWave-Minimum von 187 Mbit/s bei 1920x1080, 60 fps: Das Bild wird zerfallen."));
        assert_eq!(pyrowave_bitrate_warning(&Settings { bitrate_kbps: 186_624, ..low.clone() }, 1920, 1080, 60), None);
        assert_eq!(pyrowave_bitrate_warning(&Settings { bitrate_kbps: 0, ..low.clone() }, 1920, 1080, 60), None, "automatic");
        assert_eq!(pyrowave_bitrate_warning(&Settings { codec: CodecPreference::Hevc, ..low.clone() }, 1920, 1080, 60), None);
        assert!(pyrowave_bitrate_warning(&Settings { codec: CodecPreference::PyroWave444, ..low }, 1920, 1080, 60).is_some());
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
