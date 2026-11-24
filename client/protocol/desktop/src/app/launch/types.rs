//! Launch configuration helpers, stream-kind enums and app-list target resolution.

use super::super::*;

/// Input-only on a PC without its own mode: the smallest stream every host takes, as on Android.
/// It leaves the PC's display mode and its speakers alone: no virtual display or VRR, and the
/// PC keeps playing its sound.
/// A session whose picture nobody sees (input only, microphone only): the plainest stream, and
/// nothing that needs the GPU renderer (Linux would switch to it, or refuse with OpenGL set).
pub(crate) fn input_only_settings(settings: &mut config::Settings) {
    settings.hdr = false;
    settings.experimental_packed_444 = false;
    (settings.prefer_yuv444, settings.sdr_10bit) = (false, false);
    (settings.upscaling, settings.frame_warp) = (config::Upscaling::Bilinear, false);
    settings.codec = config::CodecPreference::H264;
    settings.width = 1280;
    settings.height = 720;
    settings.fps = 60;
}

pub(crate) fn small_stream(settings: &mut config::Settings) {
    use nightjar_policy::launch::{SMALL_BITRATE_KBPS, SMALL_FPS, SMALL_HEIGHT, SMALL_WIDTH};
    (settings.width, settings.height, settings.fps, settings.bitrate_kbps) = (SMALL_WIDTH, SMALL_HEIGHT, SMALL_FPS, SMALL_BITRATE_KBPS);
    settings.audio = config::AudioChannels::Stereo;
    (settings.virtual_display, settings.vrr, settings.play_audio_on_host) = (false, false, true);
    // Nothing is shown, so nothing is offered beyond plain 4:2:0 or said about it, or scaled.
    (settings.prefer_yuv444, settings.sdr_10bit) = (false, false);
    (settings.upscaling, settings.frame_warp) = (config::Upscaling::Bilinear, false);
}

/// What a decoder check's result holds for: the decoder, the formats it checked, and "Vulkan video
/// first", which changes the backend order both checks decode through (`Hardware::order`).
pub(crate) type ProbeKey = (Hardware, u32, bool);

pub(crate) fn probe_key(hardware: Hardware, wanted: u32, settings: &config::Settings) -> ProbeKey {
    (hardware, wanted, settings.vulkan_video_first)
}

/// Whether a start goes on waiting for the window's HDR report: while it can still come, until
/// the deadline its first call sets in `wait`.
pub(crate) fn hdr_wait_goes_on(wait: &mut Option<Instant>, now: Instant, may_come: bool) -> bool {
    let deadline = *wait.get_or_insert(now + sdl::WINDOW_HDR_REPORT_WAIT);
    may_come && now < deadline
}

/// `probe`'s answer for `hardware`, from `cache` when it was the last decoder checked.
#[cfg(test)]
pub(crate) fn probed_once<K: PartialEq, T: Clone>(cache: &mut Option<(K, T)>, hardware: K, probe: impl FnOnce() -> T) -> T {
    match cache {
        Some((probed, result)) if *probed == hardware => result.clone(),
        _ => cache.insert((hardware, probe())).1.clone(),
    }
}

/// A stream start the PIN lock holds until the PIN is entered.
pub(crate) enum PinPending {
    Launch { host: u64, app: i32, resume: bool, kind: LaunchKind, from: LaunchFrom },
    QuitAndLaunch { host: u64, app: i32, named: Option<i32> },
    /// Continue here's press, and with it the stream start it leads to.
    ContinueHere { host: u64, app: i32 },
}

/// What asked for a launch. The log names it, so it shows what had the PC start or replace a game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LaunchFrom {
    /// A tile, Continue playing or Resume in the launcher.
    Launcher,
    /// The launcher's prompt to quit the running app and start this one.
    Replace,
    /// `--autostream`.
    Autostream,
    /// A `--host` command line.
    CommandLine,
}

impl LaunchFrom {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            LaunchFrom::Launcher => "launcher",
            LaunchFrom::Replace => "launcher, quit and start",
            LaunchFrom::Autostream => "autostream",
            LaunchFrom::CommandLine => "command line",
        }
    }
}

/// The packed picture is twice the stream's height: past the decoder's largest, that mode goes,
/// and the start says why when none is left (`linux_diagnosis::packed_offer`).
#[cfg(target_os = "linux")]
pub(crate) fn packed_modes_that_fit(hardware: Hardware, modes: u8, (width, height): (u32, u32)) -> (u8, Option<String>) {
    linux_diagnosis::packed_offer(hardware, modes, width, height)
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn packed_modes_that_fit(_hardware: Hardware, modes: u8, _size: (u32, u32)) -> (u8, Option<String>) {
    (modes, None)
}

/// What a launch starts besides an ordinary stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LaunchKind {
    Stream,
    /// Input-only through a small ordinary stream, on a PC without its own mode.
    SmallInputOnly,
    /// Only this device's microphone, on a PC that lists `Feature::MicOnly`.
    MicOnly,
    /// A resume of the game the PC runs, also another device's (`LaunchPlan::join`).
    JoinRunning,
}

impl LaunchKind {
    /// The kind of a start a plan made: a join, input only through a small stream, microphone
    /// only, else an ordinary stream.
    pub(crate) fn of_plan(join: bool, small_stream_input_only: bool, mic_only: bool) -> Self {
        if mic_only { LaunchKind::MicOnly } else if small_stream_input_only { LaunchKind::SmallInputOnly } else if join { LaunchKind::JoinRunning } else { LaunchKind::Stream }
    }

    pub(crate) fn log_suffix(self) -> &'static str {
        match self {
            LaunchKind::Stream => "",
            LaunchKind::SmallInputOnly => ", input only through a small stream",
            LaunchKind::MicOnly => ", microphone only",
            LaunchKind::JoinRunning => ", joining the running game",
        }
    }

    /// Stream here too: a game selected by its tile launches beside the running one as it always
    /// did; a resume would stop here, since the PC reports the game to its owner only.
    pub(crate) fn stream_here_too(resume: bool) -> Self {
        if resume { LaunchKind::JoinRunning } else { LaunchKind::Stream }
    }
}

/// What a start does once the PC answered who plays.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SelectStep {
    /// Another device plays this game: "X is playing on Y", with this title and device.
    Ask(String, String),
    Join,
    Start,
}

pub(crate) fn select_step(resume: bool, too: bool, playing: Option<(String, String)>, press_failed: bool) -> SelectStep {
    if resume && nightjar_policy::continue_here::joins_after_read(too, playing.is_some(), press_failed) {
        return SelectStep::Join;
    }
    match playing {
        Some((name, device)) => SelectStep::Ask(name, device),
        None => SelectStep::Start,
    }
}

/// A microphone-only session's settings: the microphone on, no controllers, and the PC keeps
/// its own sound.
pub(crate) fn mic_only_settings(settings: &mut config::Settings) {
    settings.microphone = true;
    settings.controllers = false;
    settings.play_audio_on_host = true;
    settings.clipboard_sync = false;
    settings.usb_forwarding = false;
}

/// The app id a microphone-only launch sends: it names no app, and the PC starts none.
pub(crate) const MIC_ONLY_APP_ID: i32 = 0;

pub(crate) fn launch_line(from: LaunchFrom, app_id: i32, resume: bool, busy: bool) -> String {
    format!(
        "launch from {}: app {app_id}{}{}",
        from.as_str(),
        if resume { ", resume" } else { "" },
        if busy { ", ignored: another stream is starting" } else { "" }
    )
}

/// What a launch can do with a PC's app list as it is now.
pub(crate) enum LaunchApp {
    Ready(nightjar_core::http::App),
    /// The list is being read; the launch waits for it.
    Wait,
    /// No list was read yet.
    Gone,
    /// The list could not be read; the reason is shown.
    Failed(String),
    /// The list has no such app.
    Missing,
}

pub(crate) fn launch_app(apps: &Apps, app_id: i32) -> LaunchApp {
    match apps {
        Apps::Loading => LaunchApp::Wait,
        Apps::Loaded(apps) => apps.iter().find(|app| app.id == app_id).cloned().map_or(LaunchApp::Missing, LaunchApp::Ready),
        Apps::NotLoaded => LaunchApp::Gone,
        Apps::Failed(why) => LaunchApp::Failed(why.clone()),
    }
}

/// What a launch starts on this PC. A resume names the running session and needs no entry of the
/// list: the stream start asks for the id and UUID the PC reports, and checks them against a fresh
/// read. A tile that happens to carry the running id is never its action.
pub(crate) fn launch_target(host: &hosts::Host, app_id: i32, resume: bool) -> LaunchApp {
    let listed = launch_app(&host.apps, app_id);
    if !resume {
        return listed;
    }
    let running = host.running_app() == Some(app_id);
    match listed {
        LaunchApp::Wait => LaunchApp::Wait,
        LaunchApp::Ready(app) if !app.is_control_action() => LaunchApp::Ready(app),
        _ if running => LaunchApp::Ready(running_app_unlisted(host, app_id)),
        LaunchApp::Ready(_) => LaunchApp::Missing,
        other => other,
    }
}

/// The session the PC runs, for a resume its list has no entry for.
fn running_app_unlisted(host: &hosts::Host, app_id: i32) -> nightjar_core::http::App {
    nightjar_core::http::App {
        name: ui::at_sentence_start(&ui::running_name(&host.ui_view())),
        id: app_id,
        hdr_supported: false,
        uuid: host.info.as_ref().and_then(|info| info.current_game_uuid.clone()).unwrap_or_default(),
        host_index: None,
        art_version: String::new(),
        control: nightjar_core::http::AppControl::None,
        monitor_resume: false,
    }
}

/// Starts reading the list a launch waits for; false when no read could start.
pub(crate) fn read_list_for_launch(hosts: &mut Hosts, host: u64) -> bool {
    hosts.load_apps(host);
    matches!(hosts.get(host).map(|h| &h.apps), Some(Apps::Loading))
}
