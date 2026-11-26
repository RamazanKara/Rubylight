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
pub(crate) mod recovery;
mod quality;
mod rate_change;
pub use rate_change::{own_lowering_at_start, store_keep};
pub mod menu_extras;
pub mod guests;
mod touch;
/// Touchpad passthrough: Raw Input on Windows, evdev on Linux, trackpad touches on macOS.
mod touchpad;
#[cfg(all(target_os = "macos", feature = "demo-library"))]
pub use present::macos_video_check::run as check_metal;

mod shader;
mod upscale;
#[cfg(target_os = "macos")]
mod metalfx;
pub fn metalfx_supported() -> bool {
    #[cfg(target_os = "macos")]
    return metalfx::supported();
    #[cfg(not(target_os = "macos"))]
    false
}
pub mod frame_warp;
pub mod frame_warp_v2;
pub mod enhance;
#[cfg(windows)]
mod amf_scaler;
#[cfg(windows)]
pub(crate) mod direct;
// The latency benchmark reads the launcher's vblanks with `phase::timing` too.
pub mod phase;
mod text_field;
mod clipboard_sync;
pub mod scanline;
pub mod race;
pub mod race_probe;
pub mod lfc;
pub mod scan_sync;
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
mod wlan;
pub mod gpu_planes;
pub mod pyrowave_sdk;
#[cfg(unix)]
mod planes_gpu;
// The overlay's meshes on an HDR10 swapchain image; its constants and module are tested everywhere.
#[cfg(any(target_os = "linux", test))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub mod ui_pq;
#[cfg(all(test, unix))]
mod vulkan_tests_unix;
mod negotiate;
pub(crate) mod connect;
pub mod decoder_cache;
mod stats;
mod input;
mod render;
mod notices;
mod microphone_watch;
use microphone_watch::{MICROPHONE_BUSY_EVERY, MicrophoneBusyWatch};

use std::collections::{HashSet, VecDeque};
use std::net::ToSocketAddrs;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nightjar_core::color;
use nightjar_core::control::{ClientMessage, ControlEvent};
use nightjar_core::http::foundation::AbrRequest;
use nightjar_core::http::{App, AppRole, Availability, Feature, HardwareMetrics, HostApiError, HostClient, Error as HttpError, LaunchRequest, ServerInfo};
#[cfg(test)]
use nightjar_core::http::ClientIdentity;
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
use crate::hosts_types::Waker;
use crate::strings::{en, tr, trf};
use crate::keymap;
use crate::media::Hardware;
use pipeline::{lock, Notice, Shared};
use present::{viewport, PacingMode, Pacer, VideoTexture};
use connect::*;
use negotiate::*;
use stats::*;
pub use connect::{end_all_before, host_ended_stream, offers_714, termination_text, connect_timeout_text, Attempts, ConnectFailed, Connecting, CONNECT_TIMEOUT, LEAVE_WAIT};
pub use connect::env_flag;
pub use negotiate::{base_probe_formats, codec_name, color_probe_formats, plain_formats, probes_color, pyrowave_bitrate_warning, pyrowave_minimum_notice, pyrowave_notices_if_stream, pyrowave_notice, stream_mode};
pub use frame_warp::frame_warp_notice;
mod adaptation;
mod controls;
mod lifecycle;
mod state;
mod types;

pub use types::{
    host_client_for, prune_teardowns, reap_session, sheet_chord, spawn_background, Display, Exit,
    Facts, LaunchPlan, Overlay, OverlayChange, Sheet, SheetChord, Teardown, Wait, LATENCY_CSV,
    TOAST_SHOWN,
};
use state::*;
use types::*;

// Kept in `stream/mod.rs` so `desktop-config`'s `strings::tests` scans every marked stream literal.
const MSG_AVG_DECODE_TIME: &str = en("Average decode time:");
const MSG_MS_UNIT: &str = en("{} ms");
const MSG_IMAGE_TOO_LARGE: &str = en("The image is too large for the PC’s clipboard: {}");
const MSG_LOCAL_NOT_PNG: &str = en("The image on this device’s clipboard is no valid PNG: {}");
const MSG_HOST_NOT_PNG: &str = en("The PC sent no valid PNG image: {}");
const MSG_HOST_UNREADABLE_IMAGE: &str = en("The PC could not read this image.");
const MSG_HOST_UNCONVERTIBLE_IMAGE: &str = en("The PC could not convert the image on its clipboard.");
const MSG_IMAGE_CLIPBOARD_FORBIDDEN: &str = en("The PC refused its image clipboard to this device. Check the device’s clipboard permissions on the PC; images move only during a stream of this device.");
const MSG_IMAGE_CLIPBOARD_NO_ANSWER: &str = en("The PC’s image clipboard did not answer: {}");
const MSG_SHORTCUT_NOT_TAKING_INPUT: &str = en("Shortcut was not sent: the stream isn’t taking input right now.");
const MSG_SHORTCUT_NEEDS_SYSTEM_KEYS: &str = en("Enable Send system keys to use shortcuts with the Windows key.");
const MSG_MIC_NOT_ACCEPTED: &str = en("{} did not accept the microphone");
const MSG_AUTO_QUALITY_NO_MODE: &str = en("Automatic quality stopped: the stream reported no size or frame rate.");
const MSG_CANNOT_USE_MODE: &str = en("This stream cannot use the requested size or frame rate.");
const MSG_COULD_NOT_CHANGE_STREAM: &str = en("Could not change the stream: {}");
const MSG_PC_DECIDES_NEXT_STREAM: &str = en("The PC decides from the next stream; this stream keeps its bitrate.");
const MSG_HOST_ABR_START_FAILED: &str = en("The PC did not start its automatic bitrate: {}");
const MSG_REPORT_COPIED: &str = en("Report copied.");
const MSG_REPORT_COPY_FAILED: &str = en("Could not copy the report.");
const MSG_STATS_SAVED: &str = en("Statistics saved to {}");
const MSG_STATS_SAVE_FAILED: &str = en("Could not save statistics: {}");
const MSG_LOCAL_CLIPBOARD_READ_FAILED: &str = en("Could not read the local clipboard.");
const MSG_LOCAL_CLIPBOARD_NO_TEXT: &str = en("The clipboard on this device holds no text.");
const MSG_LOCAL_CLIPBOARD_NO_IMAGE: &str = en("The clipboard on this device holds no image.");
const MSG_LOCAL_CLIPBOARD_IMAGE_READ_FAILED: &str = en("Could not read the image on this device’s clipboard: {}");
const MSG_LOCAL_CLIPBOARD_PNG_FAILED: &str = en("Could not convert the image on this device’s clipboard to PNG: {}");
const MSG_HOST_IMAGE_READ_FAILED: &str = en("Could not read the PC’s image: {}");
const MSG_HOST_CLIPBOARD_NO_IMAGE: &str = en("The PC’s clipboard holds no image.");
const MSG_STREAM_MENU: &str = en("Stream menu");
const MSG_SHORTCUT_NOT_SENT: &str = en("Shortcut was not sent: {}");

/// Where automatic bitrate starts after a reconnect and at first claim, shared with Android via
/// `nightjar-policy`.
pub(super) use nightjar_policy::recovery::{automatic_bitrate_start, reconnect_bitrate_start};

pub struct Streaming {
    pub session: Session,
    pub shared: Arc<Shared>,
    pub facts: Facts,
    pub pacer: Pacer,
    pub texture: VideoTexture,
    upscaler: Option<upscale::Upscaler>,
    #[cfg(target_os = "macos")]
    metalfx: Option<metalfx::Spatial>,
    /// Why MetalFX does not draw the current picture (`metalfx::blocker`), for the overlay.
    #[cfg(target_os = "macos")]
    metalfx_blocked: Option<&'static str>,
    /// AI enhancement (`config::Upscaling::Ai`) for the current picture shape and output size.
    #[cfg(windows)]
    enhancer: Option<enhance::Enhancer>,
    upscale_failed: bool,
    /// The swapchain as of the last pictures, logged once on a change.
    #[cfg(target_os = "linux")]
    kms_last_flip_us: Option<u64>,
    #[cfg(target_os = "linux")]
    phase_source_logged: Option<(&'static str, u64)>,
    #[cfg(target_os = "linux")]
    phase_missing_logged: bool,
    #[cfg(target_os = "linux")]
    phase_first_miss_us: Option<u64>,
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
    /// `present::deadline_before_refresh` (a held stream is aimed at its hold's lead instead,
    /// `Streaming::present`).
    present_deadline_us: Option<i64>,
    /// `NIGHTJAR_HELD_MARGIN_US` for measurement runs: the desktop's deadline a held picture's turn
    /// must make, which says which turn a late picture takes (`Streaming::held_margin_us`); the phase
    /// report aims the host's frames at the present lead instead (`phase::held_present_lead_us`).
    held_margin_us: Option<u64>,
    /// `NIGHTJAR_HELD_GUARD_US` for measurement runs: how long after the refresh that opens a held
    /// picture's turn its present goes out, and so how much of the refresh the swap chain has to
    /// latch it. Read once when the stream starts (`phase::held_guard_from_env`), 0.4 ms unless set,
    /// and clamped to half a refresh at use; the phase report aims the host's frames at that lead
    /// (`phase::held_present_lead_us`).
    held_guard_us: u64,
    /// `NIGHTJAR_HELD_REPORT=0` for measurement runs: a held stream's phase reports count to
    /// each refresh instead of each turn (60 fps on 120 Hz then missed 6.7% of turns, against 0.2%).
    held_report_refresh: bool,
    /// The refresh a held stream's last picture was shown at (`phase::HeldAnchor`).
    held_anchor: phase::HeldAnchor,
    /// How long the last picture fractional V-Sync held waited for its turn, for the timing CSV.
    held_wait_us: Option<u64>,
    /// How long the draws after a hold took, with Frame Warp (`present::DrawTime`).
    held_draw: present::DrawTime,
    /// The turns of a held stream on a display that tells the time it showed a picture, not its
    /// refresh (`phase::HeldClock`: Wayland's presentation feedback, a KMS commit).
    held_clock: phase::HeldClock,
    /// Fractional V-Sync was asked for where pictures can't be held, and the log said so.
    held_untimed_logged: bool,
    /// The hold the log last named.
    held_logged: u32,
    /// Whether the swap chain's statistics, which Windows' held turns count from, can be read.
    held_stats: phase::StatsWatch,
    /// Whether the last present that tried it could go without waiting: not on Linux, nor on a
    /// renderer without a DXGI swap chain.
    no_wait_present: bool,
    /// The picture skipped last, drawn after all if no newer one comes, and how long a refused
    /// present tries again (from `NIGHTJAR_PRESENT_HOLD_US` for measurement runs).
    skipped: present::SkippedPicture,
    /// That picture, counted as shown if it is drawn after all and as replaced if a newer one comes.
    skipped_picture: Option<present::Pending>,
    /// `VideoTexture::destroy_epoch` when `skipped_picture` was uploaded (or drawn into it).
    uploaded_epoch: u64,
    /// Finds pictures that missed their refresh, for `skipped`.
    miss_watch: present::MissWatch,
    /// How long a stream that skips keeps a new place from the host (`phase::SteadyReports`), from
    /// `NIGHTJAR_PHASE_STEADY_WINDOWS` for measurement runs, 0 to send every report as it is.
    phase_steady_windows: u32,
    /// Presents `VrrSync::Scan` held for a scan under way, and how long in all (ns).
    scan_waits: (u32, u64),
    /// What those presents met besides the beam (`scan_sync`): how long the drawing and the flip
    /// took, and the presents that found the one before still on its way.
    scan_tally: scan_sync::Tally,
    /// Cached D3D11 event query for `scan_sync::finish_drawing`, dropped when the stream ends.
    scan_query: scan_sync::DrawQuery,
    /// A `VrrSync::Scan` present has its drawing finished first and waits for the present before
    /// it to start its scan. `NIGHTJAR_VRR_SETTLE=0` leaves it to the beam's reading alone, as
    /// Nightjar 5.3.0 had it, and only counts (a measurement switch).
    scan_settles: bool,
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
    /// The flip history of `VrrSync::Blank`: its last present and how each present was latched.
    flip_guard: present::FlipGuard,
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
    /// The stream said once that the PC lowered the bitrate (`notices::link_lowered_toast`).
    link_lowered_told: bool,
    /// Keep my bitrate as the stream menu last turned it, for the main settings to keep; see
    /// `take_keep_change`.
    keep_change: Option<bool>,
    /// The host's latest pipeline frame rates; `None` until it sends them.
    pipeline_rates: Option<PipelineRatesReport>,
    pub exit: Option<Exit>,
    pub toast: Option<(String, Instant)>,
    /// The route toward the PC is gone: the lost-network notice stays until one returns.
    pub network_lost: bool,
    /// The network moved under this stream (a roam, or a route its connection cannot follow): a
    /// reconnect no longer takes the path the jumbo packets were tested on.
    network_moved: bool,
    /// The next automatic reconnect starts without its delay (a network came back).
    retry_now: bool,
    pub host_id: u64,
    settings: Settings,
    pub plan: LaunchPlan,
    /// The notice that Controllers is Off while a controller is connected, said once a stream.
    controllers_off: crate::app_types::ControllersOffNotice,
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
    /// The request a timeout cleared, kept until a picture of its size arrives late: the PC
    /// applied it after all, and the rate and settings it asked for are applied with the picture.
    late_mode: Option<Reconfigure>,
    /// Pictures whose RTP timestamps show the rate a reconfigure asked for (`shows_rate`), and
    /// the mode before it was asked, restored whole when the host does not apply it.
    fps_evidence: u32,
    mode_before: Option<ModeBefore>,
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
    /// Whether the UI's Session page is open (synced by the UI shell each frame).
    pub session_page_open: bool,
    /// Whether the UI's Session page has an active request in flight (keeps the page/menu open).
    pub session_page_busy: bool,
    /// Set by Ctrl+Alt+Shift+H when the Session page is open so the UI shell closes it first.
    pub close_session_page_requested: bool,
    /// Quit game was confirmed on a PC without the native API: leaving quits the game too.
    pub quit_on_leave: bool,
    /// The shortcut sheet over the stream controls, and how it was opened.
    pub shortcut_sheet: Option<Sheet>,
    keys_down: HashSet<u16>,
    /// A shortcut's chord held on the PC, and when it goes up.
    shortcut_held: Option<(crate::shortcuts_runtime::Held, Instant)>,
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
    /// Frames PyroWave's decoder passed over at the last overlay update, the seconds in a row it
    /// has been behind, and whether the stream has said so (`stats::fell_behind`).
    last_skipped: u64,
    behind: stats::Behind,
    started: Instant,
    first_frame: bool,
    /// Pictures in a row the presenter could not show.
    unshown: render::Unshown,
    /// Pictures shown at least once since the last overlay redraw.
    pub needs_redraw: bool,
    /// Whether an HDR stream is shown on HDR output, as it settles (`present` follows it); None
    /// for a stream without HDR.
    hdr_output: Option<HdrOutput>,
    timings: VecDeque<(u32, u64)>,
    /// Frame Warp's shift for each present, while the stream runs it (`Facts::frame_warp`).
    frame_warp: Option<frame_warp::Presenter>,
}

impl Streaming {
    /// Whether the compact overlay appends device battery percentage and local clock time.
    pub fn compact_overlay_device_info(&self) -> bool {
        self.settings.compact_overlay_device_info
    }

    /// Whether the pictures are on a display plane now, so the overlay draws into its own plane.
    pub fn kms_engaged(&self) -> bool {
        #[cfg(target_os = "linux")]
        return self.kms.as_ref().is_some_and(kms_scanout::Scanout::engaged);
        #[cfg(not(target_os = "linux"))]
        false
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

    fn clipboard_sync_step(&mut self, mut step: clipboard_sync::Step) {
        if let Some(message) = step.send.clone() {
            match self.session.send(message) {
                Ok(()) => {
                    if let Some(sync) = self.clipboard_sync.as_mut() {
                        sync.confirm_sent(&mut step);
                    }
                }
                Err(error) => eprintln!("nightjar: clipboard sync: could not send a copy: {error}"),
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
    pub fn add_toast(&mut self, text: impl Into<String>) {
        let text = text.into();
        let joined = joined_toast(self.toast.as_ref().map(|(shown, at)| (shown.as_str(), at.elapsed())), &text);
        self.show_toast(joined);
    }

    pub fn display_changed(&mut self, renderer: *mut SDL_Renderer) {
        // The swapchain's HDR answer may have changed with the display: ask SDL again.
        crate::sdl::forget_swapchain_answers();
        self.phase.reset();
        phase::reset_display_timing();
        #[cfg(target_os = "linux")]
        {
            self.kms_last_flip_us = None;
            self.phase_source_logged = None;
            self.phase_missing_logged = false;
            self.phase_first_miss_us = None;
        }
        // The landing is the display's: the wait's budget measures it afresh on another one.
        self.scan_tally.forget_landing();
        // Fractional V-Sync's turns count on the old display's refreshes: they start over.
        (self.held_anchor, self.held_clock, self.held_stats) = Default::default();
        self.display_change = Some(Instant::now());
        if self.facts.display_caps && self.facts.role != AppRole::InputOnly {
            let _ = self.session.send(ClientMessage::DisplayCaps(crate::sdl::display_caps(renderer)));
        }
    }

    /// Display vblank anchor and refresh period for phase lock and the V-Sync host-time grid. On
    /// Linux, resolves KMS flip timestamps and Wayland `wp_presentation` feedback for 1:1 streams
    /// before falling back to EGL/GLX sync control and unprivileged DRM vblank queries, logging
    /// the active source once per stream (or once after 1 s when none is available).
    pub(super) fn display_timing(&mut self, renderer: *mut SDL_Renderer, now_us: u64, refresh_hz: f32, report_held: u32, log_phase: bool) -> Option<(i64, i64)> {
        #[cfg(target_os = "linux")]
        {
            let resolved = if report_held == 1 {
                phase::glass_phase_timing(self.kms_last_flip_us, None, now_us, refresh_hz)
                    .map(|timing| (timing, phase::TimingSource::KmsFlip))
                    .or_else(|| self.glass.phase_timing(now_us, refresh_hz).map(|timing| (timing, phase::TimingSource::WaylandGlass)))
                    .or_else(|| phase::timing_with_source(renderer, refresh_hz))
            } else {
                phase::timing_with_source(renderer, refresh_hz)
            };
            if log_phase {
                match resolved {
                    Some(((_, period_ns), source)) => {
                        self.phase_first_miss_us = None;
                        const SOURCE_LOG_COOLDOWN_US: u64 = 30_000_000;
                        let should_log = match self.phase_source_logged {
                            None => true,
                            Some((prev, at_us)) => prev != source.label() && now_us.saturating_sub(at_us) >= SOURCE_LOG_COOLDOWN_US,
                        };
                        if should_log {
                            self.phase_source_logged = Some((source.label(), now_us));
                            self.phase_missing_logged = false;
                            eprintln!("nightjar: phase lock: display timing from {} ({:.2} ms refresh)", source.label(), period_ns as f64 / 1e6);
                        }
                    }
                    None if self.phase_source_logged.is_none() && !self.phase_missing_logged => {
                        let first = *self.phase_first_miss_us.get_or_insert(now_us);
                        if now_us.saturating_sub(first) >= 1_000_000 {
                            self.phase_missing_logged = true;
                            eprintln!("nightjar: phase lock: no display timing available on this renderer/display");
                        }
                    }
                    None => {}
                }
            }
            resolved.map(|(timing, _)| timing)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (now_us, log_phase);
            phase::timing(renderer, refresh_hz, report_held, &mut self.held_anchor)
        }
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
}

#[cfg(test)]
mod tests;
