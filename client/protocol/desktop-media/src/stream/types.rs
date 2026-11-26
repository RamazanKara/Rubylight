//! Public stream data types, teardown handles, and pure stream-control helpers.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nightjar_core::http::{App, AppRole, Availability, ClientIdentity, NativeCapabilities, ServerInfo};
use nightjar_core::session::Session;
use rubylight_protocol::control::Reconfigure;

use super::{
    color_probe, overlay, pipeline, present::PacingMode, rate_change, recovery,
    MSG_AVG_DECODE_TIME, MSG_HOST_NOT_PNG, MSG_HOST_UNCONVERTIBLE_IMAGE,
    MSG_HOST_UNREADABLE_IMAGE, MSG_IMAGE_CLIPBOARD_FORBIDDEN, MSG_IMAGE_CLIPBOARD_NO_ANSWER,
    MSG_IMAGE_TOO_LARGE, MSG_LOCAL_NOT_PNG, MSG_MS_UNIT, MSG_SHORTCUT_NEEDS_SYSTEM_KEYS,
    MSG_SHORTCUT_NOT_TAKING_INPUT,
};
use crate::config::{HostRecord, OverlayCorner, OverlayMode, QualityMode, Settings};
use crate::media::Hardware;
use crate::strings::{tr, trf};

pub use nightjar_desktop_config::stream_types::Display;

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
    /// The whole video packet size the session being resumed asked the PC for, encryption header
    /// included (0 when none is known): a reconnect carries it into the resume, which probes
    /// nothing while the old session may run. It is the launch's request, never the ANNOUNCE's
    /// reduced size, so three reconnects keep the same packets.
    pub negotiated_packet_size: u32,
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
    /// Keeps `hardware`'s device alive for the launch and its decoder ([`media::DeviceRef`]).
    pub device: crate::media::DeviceRef,
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
    /// The PlayStation controllers among the gamepads (`psmap`).
    pub playstation_mask: u32,
    pub prepared_controllers: String,
    /// The settings layer that made the "Controllers" choice, for the start's notice when it is Off.
    pub controllers_from: crate::app_types::ControllersFrom,
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
    /// The player set the bitrate in the stream menu: `settings` start at it with automatic
    /// bitrate off, and automatic quality does not turn automatic bitrate on for itself
    /// ([`Streaming::keep_manual_bitrate`]). Holds the settings' bitrate from before, which
    /// turning automatic bitrate on again restores.
    pub bitrate_before_manual: Option<u32>,
    /// An automatic reconnect's starting point for automatic bitrate: the rate it had cut to
    /// ([`reconnect_bitrate_start`]), below the stream's start bitrate it may climb back to. The
    /// client-hint controller starts from it the same way when it owned the rate.
    pub resume_bitrate_kbps: Option<u32>,
    /// An automatic reconnect's starting point for automatic quality: the step it had reached,
    /// below the stream's own mode it may climb back to.
    pub resume_quality: Option<Reconfigure>,
}

/// The stream's negotiated shape.
#[derive(Clone, Debug)]
pub struct Facts {
    pub role: AppRole,
    pub width: i32,
    pub height: i32,
    pub fps: i32,
    pub bitrate_kbps: i32,
    /// The whole video packet size this launch asked the PC for, encryption header included: what
    /// an automatic reconnect carries. Not the ANNOUNCE's size, which has the 32-byte header taken
    /// off already and would lose another 32 on every reconnect.
    pub packet_size: i32,
    pub format: u32,
    pub host_timing: bool,
    /// The host says when it lowers the video bitrate because of the network (0x5538).
    pub link_notice: bool,
    /// The host reports its pipeline frame rates (0x553D).
    pub pipeline_rates: bool,
    /// The host pauses and resumes the game on request (Quick Suspend, 0x554B).
    pub quick_suspend: bool,
    pub clock_sync: bool,
    pub display_caps: bool,
    pub cursor_shapes: bool,
    /// The host says when one of its text fields takes or loses focus (0x553B), once asked.
    pub text_input: bool,
    /// The controller slots this host takes: four on GFE's GameStream, 16 on Sunshine and its
    /// forks ([`nightjar_core::input::InputQueue::max_controllers`]). Controllers past it are not
    /// announced and say so once (`Gamepads::set_limit`).
    pub controller_limit: u8,
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
    /// PyroWave is about to start on a link this mode's rate cannot fit: what to tell the player
    /// before the first picture, without stopping the stream (`pyrowave_link_notice`).
    pub pyrowave_link: Option<String>,
    /// PyroWave's bitrate is below the level where the project's test scenes kept detail: what to
    /// tell the player at the start, as the host's own warning does.
    pub pyrowave_minimum: Option<String>,
    /// Jumbo video packets were asked for but did not pass the pre-flight or the PC's offer, so
    /// the stream uses normal packets: what to tell the player.
    pub jumbo_fallback: Option<String>,
    /// The chosen codec isn't decoded here and the stream runs another (`codec_fallback`): what
    /// to tell the player.
    pub codec_fallback: Option<String>,
    pub app_name: String,
    pub host_name: String,
    /// Frame Warp's state, shared with the session, while the PC offered it to a stream with the
    /// setting on (`connect::frame_warp_for`).
    pub frame_warp: Option<Arc<nightjar_core::frame_warp::FrameWarp>>,
    /// Why a played stream with the setting on runs without Frame Warp: the PC does not offer
    /// it, or its streaming app needs updating (`Refusal::None` otherwise).
    pub frame_warp_refused: super::frame_warp_v2::Refusal,
    #[cfg_attr(not(windows), allow(dead_code))] pub wlan_streaming: bool,
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
    /// The render device was lost mid-stream; Windows recovers it and reconnects.
    RenderDeviceLost(String),
}

impl Exit {
    /// Whether the stream reconnects on its own after this exit. Only once its video started
    /// (`video_started`, [`recovery::recovers_termination`]): a stream that never showed a picture
    /// failed to start, and its error shows at once instead of after a minute of retries.
    pub fn retryable(&self, video_started: bool) -> bool {
        match self {
            Self::NetworkReturned => video_started,
            Self::RenderDeviceLost(_) => cfg!(windows) && video_started,
            Self::Terminated(code) => recovery::recovers_termination(*code, video_started),
            _ => false,
        }
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

/// What the main loop should do next (`Precise` is Beam Race only).
pub enum Wait { Now, For(Duration), Precise(Duration) }

/// A session's stop on a thread of its own, with the handle that lets an owner wait for it where
/// it must: before the renderer it decodes on goes, or before the next launch's HTTP request. Every
/// drop is a choice, so the handle is handed to the app and never silently detached.
#[must_use = "a dropped teardown is a session still stopping on the renderer's device"]
pub struct Teardown(Option<std::thread::JoinHandle<()>>);

impl Teardown {
    pub fn none() -> Self {
        Self(None)
    }

    /// Whether the teardown thread has ended (a handle that was never started counts as ended).
    pub fn finished(&self) -> bool {
        self.0.as_ref().is_none_or(std::thread::JoinHandle::is_finished)
    }

    /// Waits for the work to end, at most until `deadline`; true once it has (or there was none).
    /// A teardown thread that panicked is finished too, and logged. One still running at the
    /// deadline keeps its handle, so its owner can hand it on or join it later.
    pub fn join_until(&mut self, deadline: Instant) -> bool {
        let Some(handle) = self.0.take() else { return true };
        while !handle.is_finished() {
            if Instant::now() >= deadline {
                self.0 = Some(handle);
                return false;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        if handle.join().is_err() {
            eprintln!("nightjar: a stream's teardown thread panicked");
        }
        true
    }
}

/// Runs `work` on a thread of its own, for teardown that may block on a socket, a decoder or an
/// audio device. The UI thread does not wait for it; a [`Teardown`] owner does where it must.
pub fn spawn_background(work: impl FnOnce() + Send + 'static) -> Teardown {
    match std::thread::Builder::new().name("nightjar teardown".into()).spawn(work) {
        Ok(handle) => Teardown(Some(handle)),
        Err(e) => {
            eprintln!("nightjar: the teardown thread cannot start: {e}");
            Teardown::none()
        }
    }
}

/// Stops a stream on a reaper thread (`spawn_background`), for a session that is not `finish`ed.
pub fn reap_session(session: Session) -> Teardown {
    spawn_background(move || session.stop())
}

/// Drops the teardowns that have ended and says whether any is still stopping: the renderer, its
/// GPU device and a launch may not go while one is.
pub fn prune_teardowns(teardowns: &mut Vec<Teardown>) -> bool {
    teardowns.retain(|teardown| !teardown.finished());
    !teardowns.is_empty()
}

/// The file Settings → Advanced → Export latency CSV saves, in the statistics folder.
pub const LATENCY_CSV: &str = "nightjar-latency.csv";

/// The once-a-second measurements as CSV, with their header.
pub(super) fn statistics_csv(rows: &VecDeque<String>) -> String {
    let mut csv = String::from("elapsed_ms,received_fps,decoded_fps,shown_fps,loss_percent,rtt_ms,decode_ms,render_ms,host_to_network_ms,latency_ms,bitrate_kbps\n");
    for row in rows {
        csv.push_str(row);
    }
    csv
}

/// Android's post-stream toast: "Average decode time: 3 ms [HEVC]". Whole milliseconds as
/// Android shows them, a tenth below 10 ms, where a GPU decoder spends under one.
pub(super) fn decode_time_text(total_us: u64, frames: u64, codec: &str) -> Option<String> {
    if frames == 0 {
        return None;
    }
    let ms = total_us as f64 / frames as f64 / 1000.0;
    let number = if ms < 10.0 { format!("{ms:.1}") } else { format!("{ms:.0}") };
    let number = if crate::strings::current().decimal_comma() { number.replace('.', ",") } else { number };
    Some(format!("{} {} [{codec}]", tr(MSG_AVG_DECODE_TIME), trf(MSG_MS_UNIT, &[&number])))
}

/// What the stream menu says when the PC's image clipboard refused or failed (`send`: this
/// device's image to the PC).
pub(super) fn image_error(error: nightjar_core::http::ImageClipboardError, send: bool) -> String {
    use nightjar_core::http::ImageClipboardError as E;
    match error {
        E::TooLarge(why) => trf(MSG_IMAGE_TOO_LARGE, &[&why]),
        E::NotPng(why) if send => trf(MSG_LOCAL_NOT_PNG, &[&why]),
        E::NotPng(why) => trf(MSG_HOST_NOT_PNG, &[&why]),
        E::Unsupported if send => tr(MSG_HOST_UNREADABLE_IMAGE).to_owned(),
        E::Unsupported => tr(MSG_HOST_UNCONVERTIBLE_IMAGE).to_owned(),
        E::Forbidden => tr(MSG_IMAGE_CLIPBOARD_FORBIDDEN).to_owned(),
        E::Host(e) => trf(MSG_IMAGE_CLIPBOARD_NO_ANSWER, &[&e]),
    }
}

/// Whether something automatic owns the bitrate, so the doctor's trial stays out of its way:
/// the PC's adaptive bitrate, automatic quality, or the client-hint controller over 0x553C.
pub(super) fn bitrate_automatic(adaptive_bitrate: bool, quality_mode: QualityMode, rate_change: Option<&rate_change::RateChange>) -> bool {
    adaptive_bitrate || quality_mode != QualityMode::Off || rate_change.is_some_and(|rates| rates.hint_active())
}

/// Why a shortcut cannot go to the PC now, as its toast says it; `None` sends it. A shortcut
/// picked while the window had lost focus used to vanish without a word.
pub(super) fn shortcut_refusal(ready: bool, system_keys: bool, keys: &[String]) -> Option<&'static str> {
    if !ready {
        return Some(MSG_SHORTCUT_NOT_TAKING_INPUT);
    }
    if !system_keys && keys.iter().any(|key| matches!(crate::shortcuts::key_code(key), Some(0x5B | 0x5C))) {
        return Some(MSG_SHORTCUT_NEEDS_SYSTEM_KEYS);
    }
    None
}

/// The overlay values a stream changed: whether it shows, its mode and its corner. `None` for
/// one the user left alone.
pub type OverlayChange = (Option<bool>, Option<OverlayMode>, Option<OverlayCorner>);

/// What differs between the overlay the settings keep and the one on the stream; `None` when
/// nothing does.
pub(super) fn overlay_change(kept: (bool, OverlayMode, OverlayCorner), now: (bool, OverlayMode, OverlayCorner)) -> Option<OverlayChange> {
    (now != kept).then(|| ((now.0 != kept.0).then_some(now.0), (now.1 != kept.1).then_some(now.1), (now.2 != kept.2).then_some(now.2)))
}

/// How long a stream's notice shows (the lost-network notice stays until a network returns).
pub const TOAST_SHOWN: Duration = Duration::from_secs(4);

/// The notice `text` makes beside `showing` (its text and age): below it while it still shows,
/// once only, else alone.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(super) fn joined_toast(showing: Option<(&str, Duration)>, text: &str) -> String {
    match showing {
        Some((shown, age)) if age <= TOAST_SHOWN && !shown.is_empty() => {
            if shown.lines().any(|line| line == text) { shown.to_owned() } else { format!("{shown}\n{text}") }
        }
        _ => text.to_owned(),
    }
}

/// Builds a TLS-pinned [`HostClient`] from a [`HostRecord`] for stream workers.
pub fn host_client_for(
    identity: &Arc<ClientIdentity>,
    record: &crate::config::HostRecord,
    address: &str,
) -> Result<nightjar_core::http::HostClient, String> {
    let port = if record.http_port == 0 {
        nightjar_core::http::DEFAULT_HTTP_PORT
    } else {
        record.http_port
    };
    let mut client = nightjar_core::http::HostClient::new(address, port, identity.clone())
        .map_err(|e| e.to_string())?
        .with_device_name(crate::hosts_types::device_name());
    if crate::second_screen::this_process() {
        client = client.with_second_screen();
    }
    if record.https_port != 0 {
        client = client.with_https_port(record.https_port);
    }
    if let Some(pem) = &record.server_certificate {
        client = client
            .with_server_certificate_pem(pem)
            .map_err(|e| e.to_string())?;
    }
    Ok(client)
}
