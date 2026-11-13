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
mod mic_dsp;
#[cfg(windows)]
mod voice_capture;
mod cursor;
pub mod direct_motion;
mod system_keys;
mod bitrate;
pub mod host_abr;
pub mod doctor;
mod gateway;
pub mod overlay;
mod recovery;
mod quality;
mod rate_change;
pub mod menu_extras;
pub mod guests;
mod touch;
/// Touchpad passthrough: Raw Input on Windows, evdev on Linux, trackpad touches on macOS.
mod touchpad;
#[cfg(all(target_os = "macos", feature = "demo-library"))]
pub(crate) use present::macos_video_check::run as check_metal;

mod shader;
mod upscale;
pub mod frame_warp;
pub mod enhance;
#[cfg(windows)]
mod amf_scaler;
#[cfg(windows)]
pub(crate) mod direct;
// The latency benchmark reads the launcher's vblanks with `phase::timing` too.
pub(crate) mod phase;
mod text_field;
mod clipboard_sync;
pub mod scanline;
pub mod race;
pub mod race_probe;
pub mod lfc;
#[cfg(any(target_os = "linux", test))]
pub mod drm_vrr;
#[cfg(target_os = "linux")]
pub mod kms_scanout;
// The timing CSV names its flags everywhere; the feedback itself is Wayland's.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub mod wl_presentation;
// Direct scanout on Wayland; its decisions and log text are tested everywhere.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub mod wl_scanout;
pub(crate) mod pyrowave;
pub mod pyrowave_gpu;
pub mod pyrowave_mode;
pub(crate) mod stage_times;
// amdgpu's sysfs files are Linux's; the parsing is tested everywhere.
#[cfg(any(target_os = "linux", test))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) mod gpu_clocks;
// The GPU hold asks amdgpu for a stable pstate on Linux's render nodes; its sysfs reading is tested everywhere.
#[cfg(any(target_os = "linux", test))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) mod gpu_hold;
#[cfg(windows)]
mod d3d11;
#[cfg(windows)]
pub(crate) mod direct_planes;
#[cfg(windows)]
mod wasapi;
#[cfg(windows)]
mod wlan;
pub mod gpu_planes;
pub(crate) mod pyrowave_sdk;
#[cfg(unix)]
mod planes_gpu;
// The overlay's meshes on an HDR10 swapchain image; its constants and module are tested everywhere.
#[cfg(any(target_os = "linux", test))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub mod ui_pq;
#[cfg(all(test, unix))]
mod vulkan_tests_unix;
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
pub use connect::{end_before, host_ended_stream, offers_714, termination_text, ConnectFailed, Connecting, LEAVE_WAIT};
pub(crate) use connect::env_flag;
pub use negotiate::{base_probe_formats, codec_name, color_probe_formats, plain_formats, probes_color, pyrowave_bitrate_warning, pyrowave_notice, stream_mode};
pub use frame_warp::frame_warp_notice;

/// What the display offers, for "native" resolution and "display" refresh.
#[derive(Clone, Copy, Debug)]
pub struct Display {
    pub width: u32,
    pub height: u32,
    /// The refresh rate pacing uses: the display's, else 60 Hz.
    pub refresh_hz: f32,
    /// The display reported `refresh_hz`; otherwise it is the 60 Hz fallback.
    pub refresh_reported: bool,
}

impl Display {
    /// The refresh rate the overlay judges frames against: only a reported one.
    pub fn reported_refresh_hz(&self) -> Option<f32> {
        self.refresh_reported.then_some(self.refresh_hz)
    }
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
    /// Whether `app` is an entry of the PC's list; a resume of a game it does not list is not.
    pub listed: bool,
    pub resume: bool,
    /// This resume joins the game the PC runs although the PC reports no game of this device's: the
    /// user chose Stream here too. `/resume` starts nothing, so a closed game stays closed.
    /// Only that start: a reconnect of it checks as any other (`rejoin`).
    pub join: bool,
    /// Input-only on a PC without its own mode: an ordinary small stream, shown and played nowhere.
    pub small_stream_input_only: bool,
    /// An input-only session that carries only this device's microphone (`Feature::MicOnly`):
    /// nothing is shown or played, and no input goes to the PC.
    pub mic_only: bool,
    pub settings: Settings,
    pub identity: Arc<ClientIdentity>,
    pub display: Display,
    pub hardware: Hardware,
    /// VAAPI pictures go to the renderer as DMA-BUFs (`crate::dmabuf`), not through memory.
    pub dmabuf: bool,
    /// Pictures go straight onto a display plane (`kms_scanout`): VAAPI exports them as DMA-BUFs
    /// in 10-bit too, since no renderer has to import them.
    pub kms_planes: bool,
    /// The renderer's Direct3D 11 device when PyroWave decodes on the GPU (`pyrowave_decoder`).
    pub gpu_decoder: Option<pipeline::GpuDevice>,
    pub packed_modes: u8,
    /// The 4:4:4 and 10-bit formats this device decodes and draws, and why the others are not
    /// (`color_probe::probe`); `(0, "")` when neither setting asks for them.
    pub color_decodable: (u32, String),
    /// The plain HEVC and AV1 formats (and Main10) the stream's decoder decodes, and by which
    /// backend (`color_probe::probe_base`).
    pub base_decodable: color_probe::Base,
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
    /// The chosen codec isn't decoded here and the stream runs another (`codec_fallback`): what
    /// to tell the player.
    pub codec_fallback: Option<String>,
    pub app_name: String,
    pub host_name: String,
    /// Frame Warp's state, shared with the session, while the PC offered it to a stream with the
    /// setting on (`connect::frame_warp_for`).
    pub frame_warp: Option<Arc<nightjar_core::frame_warp::FrameWarp>>,
    /// The setting is on for a played stream and the PC does not offer Frame Warp.
    pub frame_warp_refused: bool,
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
    /// Nightjar's own drawing of the frames shown (`render::frame_shares`).
    render_us: u64,
    /// Their present calls, which are the system's.
    present_us: u64,
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

/// How the shortcut sheet was opened, which is where Escape or B goes from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sheet {
    /// From the stream controls: back to them.
    FromMenu,
    /// By its chord over the game: back to the game.
    FromChord,
}

/// What Ctrl+Alt+Shift+H does from where the stream is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SheetChord {
    /// The sheet opens, over the controls (closing the Session page first) or over the game.
    Open(Sheet),
    /// The sheet was open: it and the controls close.
    Close,
    /// End or Close game runs on the Session page, which stays for its answer.
    Stay,
}

/// The chord's step for the menu's state; `session_page` is whether the Session page is open and
/// busy. The sheet draws only without the Session page, so the chord there leaves the page for it.
pub fn sheet_chord(menu_visible: bool, sheet: Option<Sheet>, session_page: Option<bool>) -> SheetChord {
    if !menu_visible {
        return SheetChord::Open(Sheet::FromChord);
    }
    match (session_page, sheet) {
        (Some(true), _) => SheetChord::Stay,
        (Some(false), _) | (None, None) => SheetChord::Open(Sheet::FromMenu),
        (None, Some(_)) => SheetChord::Close,
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
    /// False under `NIGHTJAR_PRESENT_SKIP=0`: every present waits behind the last, the A side of an
    /// A/B run (`present::skips_instead_of_waiting`).
    skip_over_waiting: bool,
    /// `NIGHTJAR_PRESENT_DEADLINE_US` for measurement runs, in place of
    /// `present::deadline_before_refresh`.
    present_deadline_us: Option<i64>,
    /// Whether the last present that tried it could go without waiting: not on Linux, nor on a
    /// renderer without a DXGI swap chain.
    no_wait_present: bool,
    /// The picture skipped last, drawn after all if no newer one comes, and how long a refused
    /// present tries again (from `NIGHTJAR_PRESENT_HOLD_US` for measurement runs).
    skipped: present::SkippedPicture,
    /// That picture, counted as shown if it is drawn after all and as replaced if a newer one comes.
    skipped_picture: Option<present::Pending>,
    /// Finds pictures that missed their refresh, for `skipped`.
    miss_watch: present::MissWatch,
    /// How long a stream that skips keeps a new place from the host (`phase::SteadyReports`), from
    /// `NIGHTJAR_PHASE_STEADY_WINDOWS` for measurement runs, 0 to send every report as it is.
    phase_steady_windows: u32,
    /// Presents `VrrSync::Scan` held for a scan under way, and how long in all (ns).
    scan_waits: (u32, u64),
    /// Repeats the held picture before a VRR panel refreshes by itself (`lfc`), while the display
    /// check has the display's scan and found it refreshing on present.
    keeper: Option<lfc::Keeper>,
    /// The keeper repeats now: the stream tears whole pictures onto a display the check found
    /// refreshing on present.
    keeper_on: bool,
    /// True under `NIGHTJAR_VRR_LFC=1` only. On screen the repeats spared pictures a scan under way
    /// on the steadiest content alone and the waits cost cadence everywhere (README, "Low frame
    /// rates on a VRR display"), so there is no keeper by default; under `count` it counts the
    /// pictures that met a scan, repeats none and no picture waits for a scan's end.
    keeper_repeats: bool,
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
    /// The pictures on a display plane when Nightjar draws straight to the display.
    #[cfg(target_os = "linux")]
    pub kms: Option<kms_scanout::Scanout>,
    /// The AMD GPUs held at their peak power state for this stream (`high_performance_gpu_power`);
    /// dropped with the stream, which releases them.
    #[cfg(target_os = "linux")]
    _gpu_hold: Option<gpu_hold::GpuHold>,
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
    /// A Guide held for the setting's time asks for the stream menu; main opens it with the window.
    guide_menu: bool,
    /// A Guide press waits on the loop's tick, so the loop wakes often.
    guide_waiting: bool,
    /// The PC's night mode and headphone surround switches in the stream controls.
    pub host_audio: menu_extras::HostAudio,
    /// Join requests and guests of this device's game, on a PC that offers them.
    pub guests: guests::Guests,
    pub pending_command: Option<usize>,
    pub bitrate_busy: bool,
    /// The toast that packed full chroma's copies leave the decoder behind was shown (`doctor::packed_hint`).
    packed_hint_shown: bool,
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
    /// The sizes decoded pictures are cropped to past an encoder's alignment padding.
    stream_sizes: render::StreamSizes,
    display_change: Option<Instant>,
    last_arrival: Option<(u32, u64)>,
    /// Emptied buffers `pump` swaps with the decoder's queues, so taking a frame allocates nothing
    /// and holds the decoder's lock only for the swap.
    taken_frames: VecDeque<pipeline::Decoded>,
    taken_slices: VecDeque<pyrowave::SliceReady>,
    jitter_ms: f64,
    focused: bool,
    pub microphone_muted: bool,
    /// Microphone only: the microphone could not be opened or stopped working, so nothing goes.
    pub microphone_unavailable: bool,
    /// Microphone only: asks the PC whether another device's microphone plays instead.
    microphone_busy: Option<MicrophoneBusyWatch>,
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
    /// A way out confirmed in the stream menu, run as the Session page opens.
    pending_exit: Option<crate::session_page::HostExit>,
    /// This device's permissions as the launcher last read them from the PC; the main loop keeps
    /// them current.
    pub hints: crate::session_page::Hints,
    /// Quit game was confirmed on a PC without the native API: leaving quits the game too.
    pub quit_on_leave: bool,
    /// The shortcut sheet over the stream controls, and how it was opened.
    pub shortcut_sheet: Option<Sheet>,
    /// The stream menu's page: the grid, or one of the pages its tiles open.
    pub menu_page: crate::stream_menu::Page,
    /// A controller button pressed for a grid tile, run when the menu draws next.
    pub menu_hotkey: Option<nightjar_policy::stream_menu::Hotkey>,
    keys_down: HashSet<u16>,
    /// A shortcut's chord held on the PC, and when it goes up.
    shortcut_held: Option<(crate::shortcuts::Held, Instant)>,
    mouse_down: HashSet<u8>,
    touch: touch::Touch,
    touchpad: touchpad::Touchpad,
    captured: bool,
    /// Whether the window was full screen at the last pass (`follow_fullscreen`).
    fullscreen: bool,
    absolute: bool,
    /// Captured motion sent from the thread that reads the mouse.
    pub direct_motion: direct_motion::DirectMotion,
    /// Wi-Fi in media streaming mode for the stream.
    #[cfg(windows)]
    _wlan: Option<wlan::StreamingMode>,
    offset: ClockOffset,
    /// The client timing CSV (`NIGHTJAR_TIMING_CSV`), off by default.
    timing_csv: Option<crate::timing_csv::TimingCsv>,
    /// What reached the glass: wp_presentation feedback of each picture (Wayland).
    #[cfg(target_os = "linux")]
    glass: wl_presentation::Glass,
    /// The stream's presents, and how many the GPU renderer drew straight into the swapchain.
    #[cfg(target_os = "linux")]
    window_target: WindowTargetCount,
    /// The HDR mastering metadata last handed to the HDR10 swapchain (`sdl::set_swapchain_hdr_metadata`):
    /// None before any, `Some(None)` for unknown.
    #[cfg(target_os = "linux")]
    swapchain_hdr_metadata: Option<Option<crate::sdl::SwapchainHdrMetadata>>,
    rtp_clock: RtpClock,
    last_probe: Instant,
    window: Window,
    last_counters: (u64, u64, u64, u64, u64, u64, u64),
    last_video_bytes: u64,
    last_wire_bytes: u64,
    host_average: HostAverage,
    /// Early frames and fallbacks at the last overlay update.
    last_early: (u64, u64),
    last_video: (u64, u64, u64),
    started: Instant,
    first_frame: bool,
    /// Pictures shown at least once since the last overlay redraw.
    pub needs_redraw: bool,
    /// An HDR stream is shown on HDR output: `display_changed` says when that changes.
    hdr_output: bool,
    timings: VecDeque<(u32, u64)>,
    /// Frame Warp's shift for each present, while the stream runs it (`Facts::frame_warp`).
    frame_warp: Option<frame_warp::Presenter>,
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

/// Whether an HDR stream's output changed, and to what. A stream without HDR is never on HDR output.
fn hdr_output_change(was: bool, now: bool) -> Option<bool> {
    (was != now).then_some(now)
}

/// The end-of-stream line for how the GPU renderer's frames reached the swapchain: `direct` of the
/// stream's `presents` were drawn straight into it, the rest through the back buffer and a copy
/// (the PQ present, a screenshot's read-back, a resize, or another renderer).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn window_target_line(direct: i64, presents: u64) -> String {
    let direct = u64::try_from(direct).unwrap_or(0).min(presents);
    format!("window target: {direct} of {presents} presents drawn straight into the swapchain, {} through the back buffer", presents - direct)
}

/// [`window_target_line`] for the stream's last renderer, `renderer` (SDL's name). Only the GPU
/// renderer counts its frames; any other one (OpenGL, Linux's default) has no such path, and its
/// "0 of N" would read as a copy that never happened.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn window_target_report(renderer: &str, direct: i64, presents: u64) -> String {
    if renderer == "gpu" {
        window_target_line(direct, presents)
    } else {
        format!("window target: not counted on the {renderer} renderer, which draws through its own back buffer ({presents} presents)")
    }
}

/// The stream's presents and how many of them the GPU renderer drew straight into the swapchain,
/// from its count (`sdl::window_target_frames`) read around each present. A renderer made again
/// mid-stream (a PQ or scRGB switch) counts from zero: its count starts afresh, the old one's kept.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct WindowTargetCount {
    renderer: usize,
    start: i64,
    last: i64,
    carried: i64,
    presents: u64,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
impl WindowTargetCount {
    /// Before a present on `renderer`, whose count is `frames`. A count below the last one is
    /// another renderer at the old one's address.
    fn before_present(&mut self, renderer: usize, frames: i64) {
        if self.presents == 0 || renderer != self.renderer || frames < self.last {
            if self.presents > 0 {
                self.carried += self.last - self.start;
            }
            self.renderer = renderer;
            self.start = frames;
        }
        self.last = frames;
        self.presents += 1;
    }

    /// After that present: its count now.
    fn after_present(&mut self, frames: i64) {
        self.last = frames.max(self.last);
    }

    fn direct(&self) -> i64 {
        self.carried + self.last - self.start
    }
}

/// What the HDR10 swapchain's metadata changes to at a present, if anything. `stream`: the
/// stream's own while it is HDR (`Some(None)` when what the host sent is no usable metadata);
/// `sent`: what was handed last (`None` before anything). Once HDR stops the swapchain gets
/// unknown metadata (`Some(None)`) instead of keeping the last stream's.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn swapchain_metadata_change<M: Copy + PartialEq>(stream: Option<Option<M>>, sent: Option<Option<M>>) -> Option<Option<M>> {
    stream.or(sent.map(|_| None)).filter(|wanted| sent != Some(*wanted))
}

impl Streaming {
    /// Whether the pictures are on a display plane now, so the overlay draws into its own plane.
    pub fn kms_engaged(&self) -> bool {
        #[cfg(target_os = "linux")]
        return self.kms.as_ref().is_some_and(kms_scanout::Scanout::engaged);
        #[cfg(not(target_os = "linux"))]
        false
    }

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
        let mic_refused = connecting.plan.mic_only && !session.microphone_available();
        let facts_host = facts.host_name.clone();
        let microphone = if settings.microphone && !connecting.plan.microphone_muted && facts.role != AppRole::RemoteMonitor {
            if session.microphone_available() {
                match microphone::Microphone::open(connecting.shared.clone(), microphone::echo_for(connecting.plan.mic_only, settings.mic_echo_cancel), settings.mic_gain, mic_dsp::Choice::of(&settings)) {
                    Ok(microphone) => Some(microphone),
                    Err(error) => {
                        connecting.shared.notify(Notice::MicrophoneFailed(error));
                        None
                    }
                }
            } else if !settings.controller_audio && !mic_refused {
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
            .map(|host| crate::usb::Panel::new(session.usb_forwarder(host, || {}), settings.usb_auto.clone()));
        let hdr_stream = present::hdr_negotiated(settings.hdr, facts.format);
        let vrr_fallback = present::without_vrr(&settings);
        let stream_fps = u32::try_from(facts.fps).unwrap_or(0);
        if settings.frame_pacing != config::FramePacing::Latency {
            eprintln!("nightjar: frame pacing {:?} without VRR at {stream_fps} fps", settings.frame_pacing);
        }
        let vrr_check = checks_display_for_vrr(connecting.plan.vrr, connecting.pacing, vrr_fallback, env_flag("NIGHTJAR_VRR_CHECK"));
        #[cfg(target_os = "linux")]
        let keeps_mailbox = !present::prefers_immediate(true, crate::sdl::video_driver() == "wayland", crate::handheld::gamescope());
        #[cfg(not(target_os = "linux"))]
        let keeps_mailbox = false;
        let present_check = checks_display_for_present_mode(connecting.plan.vrr, connecting.pacing, vrr_check, keeps_mailbox, env_flag("NIGHTJAR_VRR_CHECK"));
        let vrr_sync = if connecting.plan.vrr { present::VrrSync::parse(std::env::var("NIGHTJAR_VRR_SYNC").ok().as_deref()) } else { present::VrrSync::Off };
        if vrr_sync != present::VrrSync::Off { eprintln!("nightjar: VRR sync {} (NIGHTJAR_VRR_SYNC)", vrr_sync.label()); }
        // `NIGHTJAR_VRR_LFC=1` has the stream repeat pictures for a VRR panel itself
        // (`lfc::Keeper`), `count` only counts the pictures that meet a scan under way; without
        // it the stream reads no scan line for this. The scan line is read on Windows.
        let lfc_mode = std::env::var("NIGHTJAR_VRR_LFC").ok();
        let lfc_repeats = lfc_mode.as_deref().is_some_and(|mode| connect::flag_value(mode) == Some(true));
        let lfc_counts = lfc_mode.as_deref().is_some_and(|mode| mode.trim().eq_ignore_ascii_case("count"));
        let vrr_lfc = cfg!(windows) && connecting.plan.vrr && connecting.pacing != PacingMode::Vsync && (lfc_repeats || lfc_counts);
        if vrr_sync == present::VrrSync::Scan && !vrr_check {
            eprintln!("nightjar: VRR sync scan reads the scan line; the display check's verdict changes no pacing on this stream");
        }
        let mut doctor = doctor::Doctor::default();
        doctor.receive_buffer = session.video_receive_buffer();
        let direct_motion = direct_motion::DirectMotion::new(session.input_handle(), facts.touch);
        let warp_presenter = facts.frame_warp.clone().map(frame_warp::Presenter::new);
        let mut streaming = Self {
            usb,
            usb_refused,
            direct_motion,
            #[cfg(windows)]
            _wlan: wlan::StreamingMode::begin(),
            session,
            shared: connecting.shared,
            facts,
            pacer: Pacer::with_backlog(connecting.pacing, min_interval, backlog).with_frame_pacing(settings.frame_pacing, stream_fps).with_drop_late_frames(settings.drop_late_frames),
            texture: VideoTexture::new(connecting.settings.upscaling == config::Upscaling::Bilinear, hdr_stream),
            upscaler: None,
            #[cfg(windows)]
            enhancer: None,
            upscale_failed: false,
            phase: phase::Phase::default(),
            race_probe: None,
            // The sync interval as `App::begin_stream` set it.
            vrr: present::VrrPacing::new(vrr_check, vrr_fallback, vrr_sync, connecting.pacing).with_present_check(present_check).with_lfc(vrr_lfc),
            skip_over_waiting: env_flag("NIGHTJAR_PRESENT_SKIP").unwrap_or(true),
            present_deadline_us: std::env::var("NIGHTJAR_PRESENT_DEADLINE_US").ok().and_then(|us| us.parse().ok()),
            no_wait_present: false,
            skipped: present::SkippedPicture::new(std::env::var("NIGHTJAR_PRESENT_HOLD_US").ok().and_then(|us| us.parse().ok()).map_or(present::REFUSED_HOLD, Duration::from_micros)),
            skipped_picture: None,
            miss_watch: present::MissWatch::default(),
            phase_steady_windows: std::env::var("NIGHTJAR_PHASE_STEADY_WINDOWS").ok().and_then(|windows| windows.parse().ok()).unwrap_or(phase::STEADY_WINDOWS),
            scan_waits: (0, 0),
            keeper: None,
            keeper_on: false,
            keeper_repeats: lfc_repeats,
            race_scan: None,
            racer: race::Racer::default(),
            predictor: None,
            latency: race::Latency::default(),
            race_margin_ms: hint_override("NIGHTJAR_BEAM_RACE_MARGIN_MS", settings.beam_race_margin_ms),
            race_present: env_flag("NIGHTJAR_BEAM_RACE_PRESENT").unwrap_or(true),
            race_setup: None,
            overlay_visible: settings.overlay,
            #[cfg(target_os = "linux")]
            kms: None,
            #[cfg(target_os = "linux")]
            _gpu_hold: settings.high_performance_gpu_power.then(gpu_hold::acquire),
            overlay_mode: settings.overlay_mode,
            overlay_corner: settings.overlay_position,
            overlay_kept: (settings.overlay, settings.overlay_mode, settings.overlay_position),
            overlay: Overlay::default(),
            hardware_poll: HardwarePoll::default(),
            hardware: None,
            link_notice: None,
            pipeline_rates: None,
            // A PC that set up no microphone gives Microphone only nothing to do.
            exit: mic_refused.then(|| Exit::Ended(Some(trf("{} did not accept the microphone", &[&facts_host])))),
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
            guide_menu: false,
            guide_waiting: false,
            host_audio: Default::default(),
            guests: Default::default(),
            pending_command: None,
            bitrate_busy: false,
            packed_hint_shown: false,
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
            stream_sizes: render::StreamSizes::new((i32::from(mode.width), i32::from(mode.height))),
            display_change: None,
            last_arrival: None,
            taken_frames: VecDeque::new(),
            taken_slices: VecDeque::new(),
            jitter_ms: -1.0,
            focused: false,
            microphone_muted: connecting.plan.microphone_muted,
            microphone_unavailable: false,
            microphone_busy: None,
            plan: connecting.plan,
            export_statistics: false,
            statistics: VecDeque::new(),
            menu_visible: false,
            menu_focus: None,
            session_page: None,
            session_link: String::new(),
            session_page_requested: false,
            pending_exit: None,
            hints: crate::session_page::Hints { launch: false, host_control: false },
            quit_on_leave: false,
            shortcut_sheet: None,
            menu_page: crate::stream_menu::Page::Grid,
            menu_hotkey: None,
            keys_down: HashSet::new(),
            shortcut_held: None,
            mouse_down: HashSet::new(),
            touch: touch::Touch::default(),
            touchpad: touchpad::Touchpad::default(),
            captured: false,
            fullscreen: false,
            offset: ClockOffset::default(),
            timing_csv: open_timing_csv(),
            #[cfg(target_os = "linux")]
            glass: wl_presentation::Glass::default(),
            #[cfg(target_os = "linux")]
            window_target: WindowTargetCount::default(),
            #[cfg(target_os = "linux")]
            swapchain_hdr_metadata: None,
            rtp_clock: RtpClock::default(),
            last_probe: Instant::now(),
            window: Window::default(),
            last_counters: (0, 0, 0, 0, 0, 0, 0),
            last_video_bytes: 0,
            last_wire_bytes: 0,
            host_average: HostAverage::default(),
            last_early: (0, 0),
            last_video: (0, 0, 0),
            started: Instant::now(),
            first_frame: false,
            needs_redraw: true,
            hdr_output: hdr_stream,
            timings: VecDeque::new(),
            frame_warp: warp_presenter,
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

    /// Shows `text` below the notice still showing, for as long again, so a notice that comes
    /// right after the start's (the video planes giving up at the first picture) loses neither.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn add_toast(&mut self, text: impl Into<String>) {
        let text = text.into();
        let joined = joined_toast(self.toast.as_ref().map(|(shown, at)| (shown.as_str(), at.elapsed())), &text);
        self.show_toast(joined);
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
        // The swapchain's HDR answer may have changed with the display: ask SDL again.
        crate::sdl::forget_swapchain_answers();
        self.phase.reset();
        self.display_change = Some(Instant::now());
        if self.facts.display_caps && self.facts.role != AppRole::InputOnly {
            let _ = self.session.send(ClientMessage::DisplayCaps(crate::sdl::display_caps(renderer)));
        }
        let shown = present::hdr_negotiated(self.settings.hdr, self.facts.format) && crate::sdl::hdr_available(renderer);
        if let Some(on) = hdr_output_change(self.hdr_output, shown) {
            self.hdr_output = shown;
            eprintln!("nightjar: an HDR stream's output is now {}", if on { "HDR" } else { "SDR" });
            self.show_toast(if on { tr("HDR is back on this display.") } else { tr("This display stopped showing HDR. The stream is drawn in SDR.") });
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
        self.stream_sizes.ask((i32::from(request.width), i32::from(request.height)));
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

    /// A new rate from the host: whether the stream has refreshes to spare, and what the hold learned.
    fn stream_rate_changed(&mut self, fps: u32) {
        self.pacer.set_fps(fps);
        self.skipped.restart();
    }

    pub fn follow_display(&mut self, display: Display) {
        self.pacer.clock.settings.min_interval_us = (1_000_000.0 / display.refresh_hz.max(1.0)) as i64;
        if display.refresh_hz != self.plan.display.refresh_hz { self.skipped.restart(); }
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
            match microphone::Microphone::open(self.shared.clone(), microphone::echo_for(self.plan.mic_only, self.settings.mic_echo_cancel), self.settings.mic_gain, mic_dsp::Choice::of(&self.settings)) {
                Ok(microphone) => {
                    self.microphone = Some(microphone);
                    self.microphone_unavailable = false;
                }
                Err(error) => {
                    self.microphone_muted = true;
                    self.microphone_unavailable = self.plan.mic_only;
                    self.show_toast(error);
                }
            }
        }
        for pad in &mut pads.pads {
            // The gamepad number the PC knows the controller as (player 1 for each with One player).
            let sent_as = u16::from(pad.sent_as());
            let Some(audio) = &mut pad.audio else { continue; };
            while let Some(packet) = audio.microphone_packet(sent_as) {
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
        let csv = statistics_csv(&self.statistics);
        match std::fs::create_dir_all(directory).and_then(|_| std::fs::write(&path, csv)) {
            Ok(()) => self.show_toast(trf("Statistics saved to {}", &[&path.display()])),
            Err(error) => self.show_toast(trf("Could not save statistics: {}", &[&error])),
        }
    }

    /// Keeps this stream's once-a-second measurements at `path` for Settings → Advanced → Export
    /// latency CSV, as Android keeps its last stream's latency CSV. Written once as the stream
    /// ends, from rows the overlay's window collects anyway: no work during the stream. A stream
    /// without a second of measurements leaves the last one's file. False when nothing was written.
    pub fn keep_latency_csv(&self, path: &std::path::Path) -> std::io::Result<bool> {
        if self.statistics.is_empty() {
            return Ok(false);
        }
        if let Some(directory) = path.parent() {
            std::fs::create_dir_all(directory)?;
        }
        std::fs::write(path, statistics_csv(&self.statistics)).map(|()| true)
    }

    /// Settings → Advanced → Show decode time after streaming: Android's post-stream toast, the
    /// average decode time of the stream's frames and its codec, from the counters the overlay
    /// reads. None before a frame was decoded.
    pub fn decode_time_line(&self) -> Option<String> {
        let c = &self.shared.counters;
        decode_time_text(c.decode_us_total.load(Ordering::Relaxed), c.decoded.load(Ordering::Relaxed), negotiate::codec_name(self.facts.format))
    }

    /// A stream that opened behind another window captures input (and with it the keyboard
    /// grab) once it comes to the front, as after a focus loss, not only at the first click.
    pub fn capture_on_focus(&mut self) {
        self.captured = self.takes_input();
    }

    /// Once per pass: the hook keeps the Tab of a stream hotkey Windows would take, while that
    /// hotkey can act and the grab does not have the keyboard.
    #[cfg(windows)]
    pub fn follow_tab_chord(&mut self, window: *mut SDL_Window) {
        // SAFETY: a live window.
        let focused = unsafe { SDL_GetWindowFlags(window) & SDL_WINDOW_INPUT_FOCUS != 0 };
        let grab = system_keys::keyboard_grab(self.captured, self.system_keys());
        let on = system_keys::tab_guard(system_keys::tab_chord_action(), focused, grab, self.menu_visible, self.plan.mic_only);
        self.direct_motion.tab_guard(window, on);
    }

    /// "Send system keys" as it holds now: with "System keys only in full screen", only while
    /// the window is full screen, which the Fullscreen and the Borderless window mode both are.
    fn system_keys(&self) -> bool {
        system_keys::sent(self.settings.capture_system_keys, self.settings.system_keys_fullscreen_only, self.fullscreen)
    }

    /// Once per pass: with "System keys only in full screen", the window going full screen or
    /// back to a window takes the system keys with it.
    pub fn follow_fullscreen(&mut self, window: *mut SDL_Window) {
        // SAFETY: a live window.
        let fullscreen = unsafe { SDL_GetWindowFlags(window) & SDL_WINDOW_FULLSCREEN != 0 };
        let before = self.system_keys();
        self.fullscreen = fullscreen;
        if self.system_keys() != before {
            // A key held across the change would stay down on the PC.
            self.raise_all_keys();
            self.set_capture(window, self.captured);
        }
    }

    /// Whether this stream captures the mouse and keyboard: not a view-only remote monitor, and
    /// not Microphone only, which sends nothing but the microphone.
    fn takes_input(&self) -> bool {
        self.facts.role != AppRole::RemoteMonitor && !self.plan.mic_only
    }

    /// Mouse capture: relative motion with a hidden, confined pointer.
    pub fn set_capture(&mut self, window: *mut SDL_Window, on: bool) {
        self.focused = unsafe { SDL_GetWindowFlags(window) & SDL_WINDOW_INPUT_FOCUS != 0 };
        let on = on && self.takes_input();
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
            let grab = system_keys::keyboard_grab(on, self.system_keys());
            let sdl_grab = self.direct_motion.keyboard_grab(window, grab);
            SDL_SetWindowKeyboardGrab(window, sdl_grab);
        }
        self.direct_motion.set_active(on && !self.absolute && self.focused, self.settings.mouse_speed);
    }

    pub fn raise_all_keys(&mut self) {
        self.release_shortcut();
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
                    self.show_toast(trf("Could not read the image on this device’s clipboard: {}", &[&error]));
                    return;
                }
            }
        } else { None };
        let shared = self.shared.clone();
        self.clipboard_busy = true;
        std::thread::spawn(move || {
            let result = match local {
                Some(local) => crate::clipboard::to_png(local)
                    .map_err(|e| trf("Could not convert the image on this device’s clipboard to PNG: {}", &[&e]))
                    .and_then(|png| client.send_clipboard_image(&png).map_err(|e| image_error(e, true)))
                    .map(|()| None),
                None => match client.clipboard_image() {
                    Ok(Some(png)) => crate::clipboard::from_png(&png)
                        .map(Some)
                        .map_err(|e| trf("Could not read the PC’s image: {}", &[&e])),
                    Ok(None) => Err(tr("The PC’s clipboard holds no image.").to_owned()),
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
            self.stream_rate_changed(self.facts.fps as u32);
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
        let packed = doctor::packed_hint(&report, *lock(&self.shared.packed_from_memory));
        for hint in [packed, self.doctor.receive_buffer_hint()].into_iter().flatten() {
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
        let packed = doctor::packed_hint(&report, *lock(&self.shared.packed_from_memory));
        if let Some(hint) = &packed {
            eprintln!("nightjar: {hint}");
        }
        // "Hide connection warnings" leaves the connection out, the receive buffer's note on lost video with it.
        let hide_connection = self.settings.hide_connection_warnings;
        let buffer = if hide_connection { None } else { self.doctor.receive_buffer_hint() };
        doctor::summary(&report, hide_connection).map(|summary| {
            [packed, buffer].into_iter().flatten().fold(summary, |text, hint| format!("{text}\n\n{hint}"))
        })
    }

    /// Opens the stream menu when a held Guide asked for it on the last tick.
    pub fn open_guide_menu(&mut self, window: *mut SDL_Window, pads: &mut Gamepads) {
        if std::mem::take(&mut self.guide_menu) && !self.menu_visible {
            self.toggle_menu(window, pads);
        }
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
            self.shortcut_sheet = None;
            self.menu_page = crate::stream_menu::Page::Grid;
            self.menu_hotkey = None;
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
        let page = crate::session_page::Page::open(move || crate::hosts::client_for(&identity, &record, &address), waker);
        if let Some(exit) = self.pending_exit.take() {
            // Confirmed in the stream menu already: the page shows it running, and why it failed.
            page.exit(exit);
        }
        self.session_page = Some(page);
        self.needs_redraw = true;
    }

    /// The ways out beside Disconnect that end the PC's session or game: on a PC with the native
    /// API the Session page's (End session, then Close game with Host control or Quit game), on
    /// any other Quit game (`/cancel`). A remote monitor or input session has none here.
    pub fn game_exits(&self) -> Vec<crate::session_page::HostExit> {
        if self.facts.role != AppRole::Stream {
            return Vec::new();
        }
        if !self.session_page_available() {
            return vec![crate::session_page::HostExit::QuitApp];
        }
        crate::session_page::actions(self.hints.launch, self.hints.host_control, None).into_iter().map(|entry| entry.exit).collect()
    }

    /// Runs a way out the stream menu confirmed: through the Session page where the PC has one,
    /// so it shows running and why it failed; otherwise by leaving the stream and quitting the
    /// game as a launch set to quit on leaving does.
    pub fn end_on_host(&mut self, exit: crate::session_page::HostExit) {
        if self.session_page_available() {
            self.pending_exit = Some(exit);
            self.session_page_requested = true;
        } else {
            self.quit_on_leave = true;
            self.exit = Some(Exit::User);
        }
    }

    /// The stream menu's header on its first page: the game's name over the stream's mode
    /// (`1440p · 144 FPS · HEVC HDR`, nightjar_policy::stream_menu::mode_line), the PC's name while
    /// the mode is not known. A remote monitor or input session keeps the menu's title.
    pub fn menu_header(&self) -> (String, String) {
        let place = format!("{} · {}", self.facts.app_name, self.facts.host_name);
        if self.facts.role != AppRole::Stream || self.facts.app_name.trim().is_empty() {
            return (tr("Stream menu").to_owned(), place);
        }
        let size = |v: i32| u32::try_from(v).unwrap_or(0);
        let hdr = present::hdr_negotiated(self.settings.hdr, self.facts.format);
        let mode = nightjar_policy::stream_menu::mode_line(size(self.facts.width), size(self.facts.height), size(self.facts.fps), self.facts.format, hdr);
        (self.facts.app_name.clone(), mode.unwrap_or_else(|| self.facts.host_name.clone()))
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
            self.show_toast(tr("Cannot reach the PC."));
            return;
        };
        let shared = self.shared.clone();
        std::thread::spawn(move || {
            shared.notify(Notice::HostMenu(crate::session_page::host_menu_result(client.couch_menu(true))));
        });
    }

    /// Ctrl+Alt+Shift+H, or Shortcuts & help in the stream controls: the shortcut sheet opens over
    /// the stream (from the controls when they or the Session page are open), and the chord again
    /// closes it and the controls. It draws only while shown, like the controls.
    pub fn toggle_shortcut_sheet(&mut self, window: *mut SDL_Window, pads: &mut Gamepads) {
        match sheet_chord(self.menu_visible, self.shortcut_sheet, self.session_page.as_ref().map(|p| p.busy())) {
            SheetChord::Stay => {}
            SheetChord::Close => self.toggle_menu(window, pads),
            SheetChord::Open(from) => {
                if self.menu_visible {
                    self.close_session_page();
                } else {
                    self.toggle_menu(window, pads);
                }
                if self.menu_visible && self.session_page.is_none() {
                    self.shortcut_sheet = Some(from);
                    self.needs_redraw = true;
                }
            }
        }
    }

    /// Escape or B on the shortcut sheet: back to the controls it was opened from, or to the game
    /// when its chord opened it.
    pub fn close_shortcut_sheet(&mut self, window: *mut SDL_Window, pads: &mut Gamepads) {
        match self.shortcut_sheet.take() {
            Some(Sheet::FromChord) => self.toggle_menu(window, pads),
            Some(Sheet::FromMenu) | None => self.needs_redraw = true,
        }
    }

    /// Closes the Session page; its worker ends and nothing polls.
    pub fn close_session_page(&mut self) {
        if self.session_page.as_ref().is_some_and(|p| p.busy()) {
            return;
        }
        self.session_page = None;
        self.needs_redraw = true;
    }

    /// What is known about this device's permissions before the page has read them: what the
    /// launcher last read from `/capabilities`, nothing before it answered.
    pub fn session_hints(&self) -> crate::session_page::Hints {
        self.hints
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
        let ready = self.focused && self.captured && self.facts.role != AppRole::RemoteMonitor;
        if let Some(why) = shortcut_refusal(ready, self.settings.capture_system_keys, &shortcut.keys) {
            self.show_toast(tr(why));
            return;
        }
        // A shortcut still held goes up first, so this one is pressed in full.
        self.release_shortcut();
        if self.shortcut_held.is_some() {
            self.show_toast(trf("Shortcut was not sent: {}", &[&nightjar_core::input::InputError::QueueFull]));
            return;
        }
        match self.session.input(|q| crate::shortcuts::press(&shortcut.keys, &self.keys_down, q)) {
            Ok(chord) => {
                self.shortcut_held = Some((chord, Instant::now() + Duration::from_millis(crate::shortcuts::HOLD_MS)));
            }
            Err(error) => self.show_toast(trf("Shortcut was not sent: {}", &[&error])),
        }
    }

    /// Lets go of the held shortcut's keys once its hold is over; `pump` calls this.
    pub(super) fn release_shortcut_when_due(&mut self, now: Instant) {
        if self.shortcut_held.as_ref().is_some_and(|(_, due)| now >= *due) { self.release_shortcut(); }
    }

    /// When the held shortcut's keys go up, for the loop's wait.
    pub(super) fn shortcut_release_in(&self, now: Instant) -> Option<Duration> {
        self.shortcut_held.as_ref().map(|(_, due)| due.saturating_duration_since(now))
    }

    fn release_shortcut(&mut self) {
        let Some((mut chord, due)) = self.shortcut_held.take() else { return };
        match self.session.input(|q| crate::shortcuts::release(&mut chord, q)) {
            Ok(()) => {}
            // Keys left down would stay down on the PC: the queue drains, so the next pump tries again.
            Err(nightjar_core::input::InputError::QueueFull) => self.shortcut_held = Some((chord, due)),
            Err(error) => eprintln!("nightjar: shortcut keys not released: {error}"),
        }
    }

    /// What the user changed of the overlay since the settings last kept it: the stream menu and
    /// the shortcut change the global setting too, as on Android. Only the changed values, since
    /// the others may be this PC's, game's or profile's own and are not the global ones.
    pub fn take_overlay_change(&mut self) -> Option<OverlayChange> {
        let now = (self.overlay_visible, self.overlay_mode, self.overlay_corner);
        let change = overlay_change(self.overlay_kept, now)?;
        self.overlay_kept = now;
        Some(change)
    }

    /// Whether the stream has shown its first picture.
    pub fn has_picture(&self) -> bool {
        self.first_frame
    }

    /// The app this stream started or joined.
    pub fn app(&self) -> &App {
        &self.plan.app
    }

    /// Whether this stream joined the game the PC runs, another device's too (`LaunchPlan::join`).
    pub fn joined(&self) -> bool {
        self.plan.join
    }

    /// Input-only through a small ordinary stream: video and audio arrive and are discarded.
    pub fn small_stream_input_only(&self) -> bool {
        self.plan.small_stream_input_only
    }

    /// Only this device's microphone goes to the PC.
    pub fn mic_only(&self) -> bool {
        self.plan.mic_only
    }

    /// Microphone only: starts asking the PC every few seconds whether another device's
    /// microphone plays (`GET /session`); `wake` redraws when that changes.
    pub fn watch_microphone_busy(&mut self, wake: Waker) {
        if !self.plan.mic_only || !self.plan.info.capabilities.has(Feature::MicOnly) {
            return;
        }
        let identity = self.plan.identity.clone();
        let record = self.plan.record.clone();
        self.microphone_busy = MicrophoneBusyWatch::start(
            move || crate::hosts::client_for(&identity, &record, &record.address).map(|c| move || c.microphone_busy().map_err(|e| e.to_string())),
            wake,
            MICROPHONE_BUSY_EVERY,
        );
    }

    /// Whether the PC said, last time it was asked, that another device's microphone plays.
    pub fn microphone_busy(&self) -> bool {
        self.microphone_busy.as_ref().is_some_and(MicrophoneBusyWatch::busy)
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
        let (direct, queued) = self.direct_motion.counts();
        if direct + queued > 0 {
            eprintln!("nightjar: mouse moves: {direct} sent as they arrived, {queued} through the main loop");
        }
        let input = self.direct_motion.input_count();
        if input > 0 {
            eprintln!("nightjar: keys, clicks and controller events: {input} sent during the present");
        }
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
        #[cfg(target_os = "linux")]
        {
            self.glass.poll(session::now_us());
            for line in self.glass.lines() { eprintln!("nightjar: {line}"); }
            // SAFETY: a live window; its renderer, if any, too.
            let renderer = unsafe { SDL_GetRenderer(window) };
            if self.window_target.presents > 0 && !renderer.is_null() {
                eprintln!("nightjar: {}", window_target_report(&crate::sdl::renderer_name(renderer), self.window_target.direct(), self.window_target.presents));
            }
            // The window outlives the stream: the next one (or the launcher) must not inherit this
            // stream's mastering metadata.
            if matches!(self.swapchain_hdr_metadata, Some(Some(_))) {
                crate::sdl::set_swapchain_hdr_metadata(window, None);
            }
        }
        let stats = self.session.stats();
        eprintln!("{}", video_summary(stats.frames_submitted, stats.frames_skipped, stats.decode_queue_overflows,
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
        if self.vrr.sync == present::VrrSync::Scan || self.keeper.is_some() {
            let (waits, ns) = self.scan_waits;
            eprintln!("nightjar: VRR: {waits} presents waited for a scan under way, {:.2} ms each",
                if waits > 0 { ns as f64 / f64::from(waits) / 1e6 } else { 0.0 });
        }
        if let Some(keeper) = &self.keeper { eprintln!("nightjar: VRR low frame rate: {}", keeper.line()); }
        lock(&self.shared.roam_watch).take();
        self.session.stop();
        self.host_abr_stop.take()
    }
}

/// The file Settings → Advanced → Export latency CSV saves, in the statistics folder.
pub const LATENCY_CSV: &str = "nightjar-latency.csv";

/// The once-a-second measurements as CSV, with their header.
fn statistics_csv(rows: &VecDeque<String>) -> String {
    let mut csv = String::from("elapsed_ms,received_fps,decoded_fps,shown_fps,loss_percent,rtt_ms,decode_ms,render_ms,host_to_network_ms,latency_ms,bitrate_kbps\n");
    for row in rows {
        csv.push_str(row);
    }
    csv
}

/// Android's post-stream toast: "Average decode time: 3 ms [HEVC]". Whole milliseconds as
/// Android shows them, a tenth below 10 ms, where a GPU decoder spends under one.
fn decode_time_text(total_us: u64, frames: u64, codec: &str) -> Option<String> {
    if frames == 0 {
        return None;
    }
    let ms = total_us as f64 / frames as f64 / 1000.0;
    let number = if ms < 10.0 { format!("{ms:.1}") } else { format!("{ms:.0}") };
    let number = if crate::strings::current().decimal_comma() { number.replace('.', ",") } else { number };
    Some(format!("{} {} [{codec}]", tr("Average decode time:"), trf("{} ms", &[&number])))
}

/// What the stream menu says when the PC's image clipboard refused or failed (`send`: this
/// device's image to the PC).
fn image_error(error: nightjar_core::http::ImageClipboardError, send: bool) -> String {
    use nightjar_core::http::ImageClipboardError as E;
    match error {
        E::TooLarge(why) => trf("The image is too large for the PC’s clipboard: {}", &[&why]),
        E::NotPng(why) if send => trf("The image on this device’s clipboard is no valid PNG: {}", &[&why]),
        E::NotPng(why) => trf("The PC sent no valid PNG image: {}", &[&why]),
        E::Unsupported if send => tr("The PC could not read this image.").to_owned(),
        E::Unsupported => tr("The PC could not convert the image on its clipboard.").to_owned(),
        E::Forbidden => tr("The PC refused its image clipboard to this device. Check the device’s clipboard permissions on the PC; images move only during a stream of this device.").to_owned(),
        E::Host(e) => trf("The PC’s image clipboard did not answer: {}", &[&e]),
    }
}

/// Whether something automatic owns the bitrate, so the doctor's trial stays out of its way:
/// the PC's adaptive bitrate, automatic quality, or the client-hint controller over 0x553C.
fn bitrate_automatic(adaptive_bitrate: bool, quality_mode: QualityMode, rate_change: Option<&rate_change::RateChange>) -> bool {
    adaptive_bitrate || quality_mode != QualityMode::Off || rate_change.is_some_and(|rates| rates.hint_active())
}

/// Why a shortcut cannot go to the PC now, as its toast says it; `None` sends it. A shortcut
/// picked while the window had lost focus used to vanish without a word.
fn shortcut_refusal(ready: bool, system_keys: bool, keys: &[String]) -> Option<&'static str> {
    if !ready {
        return Some(en("Shortcut was not sent: the stream isn’t taking input right now."));
    }
    if !system_keys && keys.iter().any(|key| matches!(crate::shortcuts::key_code(key), Some(0x5B | 0x5C))) {
        return Some(en("Enable Send system keys to use shortcuts with the Windows key."));
    }
    None
}

/// The overlay values a stream changed: whether it shows, its mode and its corner. `None` for
/// one the user left alone.
pub type OverlayChange = (Option<bool>, Option<OverlayMode>, Option<OverlayCorner>);

/// What differs between the overlay the settings keep and the one on the stream; `None` when
/// nothing does.
fn overlay_change(kept: (bool, OverlayMode, OverlayCorner), now: (bool, OverlayMode, OverlayCorner)) -> Option<OverlayChange> {
    (now != kept).then(|| ((now.0 != kept.0).then_some(now.0), (now.1 != kept.1).then_some(now.1), (now.2 != kept.2).then_some(now.2)))
}

/// How long a stream's notice shows (the lost-network notice stays until a network returns).
pub const TOAST_SHOWN: Duration = Duration::from_secs(4);

/// The notice `text` makes beside `showing` (its text and age): below it while it still shows,
/// once only, else alone.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn joined_toast(showing: Option<(&str, Duration)>, text: &str) -> String {
    match showing {
        Some((shown, age)) if age <= TOAST_SHOWN && !shown.is_empty() => {
            if shown.lines().any(|line| line == text) { shown.to_owned() } else { format!("{shown}\n{text}") }
        }
        _ => text.to_owned(),
    }
}

#[cfg(test)]
mod tests;

/// How often Microphone only asks the PC whether another device's microphone plays.
const MICROPHONE_BUSY_EVERY: Duration = Duration::from_secs(3);

/// A thread that asks the PC whether this device's microphone waits for another's, until
/// dropped with the stream.
struct MicrophoneBusyWatch {
    busy: Arc<std::sync::atomic::AtomicBool>,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl MicrophoneBusyWatch {
    /// `connect` makes the question; `wake` runs when the answer changes.
    fn start<Q, F>(connect: F, wake: Waker, every: Duration) -> Option<Self>
    where
        F: FnOnce() -> Result<Q, String> + Send + 'static,
        Q: Fn() -> Result<bool, String>,
    {
        use std::sync::atomic::{AtomicBool, Ordering};
        let busy = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let (seen, stopped) = (busy.clone(), stop.clone());
        let spawned = std::thread::Builder::new().name("microphone-busy".into()).spawn(move || {
            let ask = match connect() {
                Ok(ask) => ask,
                Err(error) => {
                    eprintln!("nightjar: microphone only: the PC's session cannot be read: {error}");
                    return;
                }
            };
            let mut last_error = None;
            while !stopped.load(Ordering::Acquire) {
                match ask() {
                    Ok(now) => {
                        last_error = None;
                        if seen.swap(now, Ordering::AcqRel) != now {
                            eprintln!("nightjar: microphone only: {}", if now { "another device's microphone plays on the PC" } else { "the PC plays this microphone again" });
                            wake();
                        }
                    }
                    // Said once per kind of failure; the next answer counts again.
                    Err(error) if last_error.as_ref() != Some(&error) => {
                        eprintln!("nightjar: microphone only: the PC's session could not be read: {error}");
                        last_error = Some(error);
                    }
                    Err(_) => {}
                }
                let until = Instant::now() + every;
                while !stopped.load(Ordering::Acquire) && Instant::now() < until {
                    std::thread::sleep(Duration::from_millis(100).min(every));
                }
            }
        });
        match spawned {
            Ok(_) => Some(Self { busy, stop }),
            Err(error) => {
                eprintln!("nightjar: microphone only: no thread to ask the PC about the microphone: {error}");
                None
            }
        }
    }

    fn busy(&self) -> bool {
        self.busy.load(std::sync::atomic::Ordering::Acquire)
    }
}

impl Drop for MicrophoneBusyWatch {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
    }
}

#[cfg(test)]
mod microphone_busy_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    #[test]
    fn the_watch_follows_the_pcs_answer_and_wakes_on_each_change() {
        let answers = Arc::new(Mutex::new(vec![Ok(true), Err("offline".to_owned()), Ok(true), Ok(false)]));
        let wakes = Arc::new(AtomicUsize::new(0));
        let woke = wakes.clone();
        let queue = answers.clone();
        let watch = MicrophoneBusyWatch::start(
            move || Ok(move || queue.lock().unwrap().pop().unwrap_or(Ok(false))),
            Arc::new(move || { woke.fetch_add(1, Ordering::SeqCst); }),
            Duration::from_millis(5),
        )
        .expect("a watch");
        // Answers pop from the end: false, true, an error, true; then false for good.
        let deadline = Instant::now() + Duration::from_secs(5);
        while !answers.lock().unwrap().is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(Duration::from_millis(50));
        assert!(!watch.busy());
        // false (no change), true (wake), error (none), true (none), false (wake).
        assert_eq!(wakes.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_pc_that_cannot_be_reached_leaves_it_not_busy() {
        let watch = MicrophoneBusyWatch::start(
            || Err::<fn() -> Result<bool, String>, _>("no address".to_owned()),
            Arc::new(|| {}),
            Duration::from_millis(5),
        )
        .expect("a watch");
        std::thread::sleep(Duration::from_millis(30));
        assert!(!watch.busy());
    }
}
