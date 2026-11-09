//! Nightjar for Windows and Linux: a Moonlight-protocol game-streaming client on
//! nightjar-core. Copyright (C) 2026 RamazanKara, GPL-3.0-only.
//!
//! One SDL3 window and renderer serve the launcher (egui) and the stream (the decoded
//! video, with the overlay drawn by egui on top). The UI thread owns SDL; host requests,
//! the session, decoding and audio run on their own threads and wake the UI with an SDL
//! user event.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod a11y;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod amd_decode;
#[allow(dead_code, reason = "glass foundation: the pages start using it in the next change")]
mod ambient;
mod autostart;
mod autostream;
mod cli;
mod clipboard;
mod config;
#[cfg(windows)]
mod crash_dump;
#[cfg(feature = "demo-library")]
mod demo_driver;
mod console;
mod coop_page;
mod folders_page;
mod controller_audio;
mod controller_mouse;
mod dmabuf;
mod gamepad;
mod glyphs;
mod gui;
mod handheld;
mod host_page;
mod hosts;
mod input_source;
mod keymap;
mod live_glass;
mod logfile;
mod link_channel;
#[cfg(any(target_os = "linux", test))]
mod linux_diagnosis;
#[cfg(any(target_os = "linux", test))]
mod linux_video;
mod logpipe;
mod media;
mod menu_style;
mod nav_repeat;
mod move_watch;
mod network;
mod network_test;
mod open_play;
mod pin_lock;
mod pin_ui;
mod power;
mod radeon;
mod sdl;
mod second_screen;
mod session_page;
mod session_ui;
mod shortcuts;
mod stream;
mod stream_menu;
mod strings;
mod theme;
mod timing_csv;
mod ui;
mod usb;
#[cfg(target_os = "linux")]
mod usb_linux;
// Windows reads its drivers here; the parsing is tested everywhere.
#[cfg(any(windows, test))]
mod usb_winusb;
// build.rs uses the parser; here only its tests do.
#[cfg_attr(not(test), allow(dead_code))]
mod version_name;
mod self_update;
mod update_check;
mod update_net;
// Linux only, and in tests everywhere with zero_copy, which plans its extensions.
#[cfg(any(unix, test))]
#[cfg_attr(not(unix), allow(dead_code))]
mod vulkan_video;
mod window_place;
mod wol;

/// The release version: the `versionName` of the Android app, embedded by build.rs.
const VERSION: &str = env!("NIGHTJAR_VERSION");

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nightjar_core::http::native_api::session::ActionId;
use nightjar_core::http::ClientIdentity;
use sdl3_sys::everything::*;

use config::{DecoderPreference, Store, WindowMode};
use gamepad::Gamepads;
use gui::{Gui, Scale};
use hosts::{Apps, Event as HostEvent, Hosts};
use media::Hardware;
use shortcuts::{Action, Scope};
use stream::present::{pacing, PacingMode};
use stream::{ConnectFailed, Connecting, Display, Exit, LaunchPlan, Streaming};
use strings::{tr, trf};
use ui::{Combo, Command, ConnectingView, Dialog, Launcher, Screen, View};

#[cfg(windows)]
#[link(name = "winmm")]
extern "system" {
    fn timeBeginPeriod(period: u32) -> u32;
    fn timeEndPeriod(period: u32) -> u32;
}

/// 1 ms scheduler resolution while streaming, so pictures go out on time.
struct TimerResolution;

impl TimerResolution {
    fn begin() -> Self {
        #[cfg(windows)]
        // SAFETY: paired with timeEndPeriod in Drop.
        unsafe {
            timeBeginPeriod(1);
        }
        Self
    }
}

impl Drop for TimerResolution {
    fn drop(&mut self) {
        #[cfg(windows)]
        // SAFETY: paired with timeBeginPeriod.
        unsafe {
            timeEndPeriod(1);
        }
    }
}

/// What a file dialog of the shared folder browser was opened for, with where the browser was
/// then.
enum FolderPick {
    /// The folder to save the PC's file `name` in.
    Download { host: u64, at: folders_page::Place, name: String, size: u64 },
    /// The file to upload.
    Upload { host: u64, at: folders_page::Place },
}

/// How long the message of a stream the PC failed waits for the PC's reason.
const REASON_WAIT: Duration = Duration::from_secs(5);

/// The end of a stream the PC failed, shown once the poll `seq` of PC `host` answered.
struct PendingEnd {
    host: u64,
    seq: u64,
    ended: Option<String>,
    health: Option<String>,
    until: Instant,
}

struct App {
    window: *mut SDL_Window,
    renderer: *mut SDL_Renderer,
    gui: Gui,
    store: Store,
    hosts: Hosts,
    launcher: Launcher,
    pads: Gamepads,
    connecting: Option<Connecting>,
    cancelled: bool,
    /// The PC being woken and the clock of its resends ("Waking <PC>…").
    waking: wol::Wakes,
    streaming: Option<(Streaming, TimerResolution)>,
    pending_launch: Option<(u64, i32)>,
    /// The launch the PC refused until the user confirms closing the app it runs, while the
    /// launcher asks (`Dialog::ConfirmReplace`).
    pending_replace: Option<Box<LaunchPlan>>,
    /// The PC and game being quit because the player chose quit on leave for it.
    quitting_on_leave: Option<(u64, String)>,
    /// The message for a stream the PC failed, until the PC said why (`show_pending_end`).
    pending_end: Option<PendingEnd>,
    identity: Option<Receiver<Result<ClientIdentity, String>>>,
    shortcut_file: Option<(bool, sdl::FileChoice)>,
    /// A folder to download into, or a file to upload, being chosen for the shared folder browser.
    folder_pick: Option<(FolderPick, sdl::FileChoice)>,
    network_query: Option<Receiver<network::Network>>,
    /// What this PC's power allows, being asked once at start.
    power_probe: Option<Receiver<Vec<power::Offer>>>,
    /// A shut down, restart or sleep under way, and the system's answer to it.
    power_request: Option<(power::Offer, Receiver<Result<(), String>>)>,
    network_test: Option<network_test::Test>,
    /// Host, app, resume, and Input-only through a small stream.
    network_launch: Option<(u64, i32, bool, LaunchKind)>,
    next_network_check: Instant,
    /// A pairing link from the system, until the launcher can ask about it.
    links: Arc<link_channel::Inbox>,
    #[cfg(feature = "demo-library")]
    demo: demo_driver::Driver,
    waker: hosts::Waker,
    wake_event: u32,
    screenshot_dir: PathBuf,
    screenshot_requested: bool,
    stick_held: [bool; 4],
    nav_repeat: nav_repeat::NavRepeat,
    /// Under gamescope, hidden while a controller is in use outside streams.
    pointer: handheld::Pointer,
    /// The stream being started plays in SDR although HDR was asked for (gamescope without HDR): its start says so.
    hdr_note: bool,
    /// Full chroma was asked for but its packed picture is too large for the decoder: its start says so.
    packed_note: Option<String>,
    running: bool,
    autostream: Option<autostream::Autostream>,
    cli: Option<cli::CliLaunch>,
    /// Dual screen mode: the second screen's process, for as long as this stream runs.
    second_screen: Option<second_screen::Companion>,
    /// The 4:4:4 and 10-bit decoder check for a decoder and the formats it checked, done at the
    /// first launch that asks: the decoder stays the same while Nightjar runs, and a renderer
    /// switch drops it.
    color_probed: Option<(ProbeKey, (u32, String))>,
    /// The plain HEVC and AV1 decoder check (`color_probe::probe_base`), kept the same way for
    /// the decoder and the formats it checked.
    base_probed: Option<(ProbeKey, stream::color_probe::Base)>,
    /// Linux's renderer as it was made ([`RendererState`]).
    #[cfg(unix)]
    renderer_state: RendererState,
    /// Why this stream's switch to the GPU renderer failed, for the check that refuses it.
    gpu_failure: Option<String>,
    /// The stream start that waits for the PIN screen.
    pin_pending: Option<PinPending>,
    /// Selecting a game waits for the PC to say who plays it.
    select_read: bool,
    /// The reads for the open "Now playing on ...".
    move_watch: move_watch::MoveWatch,
    /// This launch's update check (Windows); command-line and test streams make none.
    updates: update_check::Updates,
    open_play: open_play::OpenPlay,
    /// The background the title bar was last given (High contrast turns it black).
    title_bar: egui::Color32,
    /// The window changed since the last launcher frame: it needs one, whatever egui paints.
    window_changed: bool,
}

/// What Linux's renderer was made for, beyond what SDL says of it.
#[cfg(unix)]
#[derive(Clone, Debug)]
struct RendererState {
    /// The Renderer setting last applied: a change is applied once, with nothing streaming.
    choice: config::Renderer,
    /// Made for an HDR stream with a display's HDR on (`handheld::RendererNow`).
    made_for_hdr: bool,
    /// Made with Vulkan video's extensions and queues, when it is the GPU renderer.
    vulkan_video: bool,
    /// Why the GPU renderer could not be made this run. A stream does not try it again (each try
    /// makes a working renderer again, and one that fails then ends Nightjar); a changed Renderer
    /// setting does.
    gpu_failed: Option<String>,
}

fn display_of(window: *mut SDL_Window) -> Display {
    // SAFETY: a live window; the mode pointer is owned by SDL.
    unsafe {
        let id = SDL_GetDisplayForWindow(window);
        let mode = SDL_GetCurrentDisplayMode(id);
        if mode.is_null() {
            return display_from_mode(None);
        }
        let m = &*mode;
        display_from_mode(Some((m.w, m.h, m.pixel_density, m.refresh_rate)))
    }
}

/// Whether the window now stands on another display, or its display on another mode.
fn another_display(kind: SDL_EventType) -> bool {
    matches!(kind, SDL_EVENT_WINDOW_DISPLAY_CHANGED | SDL_EVENT_DISPLAY_CURRENT_MODE_CHANGED)
}

/// The display from SDL's current mode (width, height, pixel density, refresh rate), if it gave
/// one. Without a mode or a refresh rate, pacing assumes 1920x1080 at 60 Hz, but the rate is not
/// marked reported, so the overlay does not judge frames against a guess.
fn display_from_mode(mode: Option<(i32, i32, f32, f32)>) -> Display {
    let Some((w, h, pixel_density, refresh_rate)) = mode else {
        return Display { width: 1920, height: 1080, refresh_hz: 60.0, refresh_reported: false };
    };
    let (width, height) = mode_pixels(w, h, pixel_density);
    let reported = refresh_rate.is_finite() && refresh_rate > 0.0;
    Display { width, height, refresh_hz: if reported { refresh_rate } else { 60.0 }, refresh_reported: reported }
}

/// A display mode's size in pixels. SDL 3 gives a mode's size in its own units and
/// `pixel_density` turns them into pixels: Windows (per-monitor DPI aware, which SDL declares)
/// and X11 give pixels at density 1 whatever the desktop scaling, Wayland and macOS give points
/// and the scale. The display's scaling (`SDL_GetDisplayContentScale`) is not part of it.
fn mode_pixels(w: i32, h: i32, pixel_density: f32) -> (u32, u32) {
    let density = if pixel_density > 0.0 { pixel_density } else { 1.0 };
    let pixels = |points: i32| (points.max(0) as f32 * density).round() as u32;
    (pixels(w), pixels(h))
}

/// The refresh rate exclusive fullscreen asks the display for. With VRR the display's top rate
/// (`modes`: the refresh rates of its fullscreen modes), whatever the desktop runs at: the mode
/// closest to it at the stream's size is that size's fastest, the panel follows the frames up to
/// it, and a mode at the stream's rate would cap the range (a 60 fps stream would put a 240 Hz
/// panel at 60 Hz). Without VRR the stream's rate, so each frame gets whole refreshes.
fn fullscreen_refresh_hz(vrr: bool, modes: &[f32], stream_fps: i32) -> f32 {
    let top = modes.iter().copied().fold(0.0, f32::max);
    if vrr && top > 0.0 { top } else { stream_fps as f32 }
}

/// The refresh rates of a display's fullscreen modes, empty when SDL has none.
fn mode_refresh_rates(id: SDL_DisplayID) -> Vec<f32> {
    let mut count = 0;
    // SAFETY: SDL allocates the list of mode pointers, which is read here and then freed.
    unsafe {
        let modes = SDL_GetFullscreenDisplayModes(id, &mut count);
        if modes.is_null() {
            return Vec::new();
        }
        let rates = (0..count.max(0) as usize).filter_map(|i| (*modes.add(i)).as_ref().map(|mode| mode.refresh_rate)).collect();
        SDL_free(modes.cast());
        rates
    }
}

/// What the display reported, for the log: the stream's "native" resolution comes from it.
fn log_display(window: *mut SDL_Window, display: Display) {
    // SAFETY: a live window; the mode pointer is owned by SDL.
    let (mode, scale) = unsafe {
        let id = SDL_GetDisplayForWindow(window);
        let mode = SDL_GetCurrentDisplayMode(id).as_ref().map(|m| (m.w, m.h, m.pixel_density));
        (mode, SDL_GetDisplayContentScale(id))
    };
    let mode = mode.map_or("no mode".to_owned(), |(w, h, d)| format!("mode {w}x{h} at density {d}"));
    eprintln!(
        "nightjar: display {}x{}@{} ({mode}, scaling {scale})",
        display.width, display.height, display.refresh_hz
    );
}

/// When this process entered `main`, for the launch line.
static LAUNCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// The events after which the window has to be drawn even if egui paints the same: it moved, was
/// resized, exposed or restored, or the renderer lost what it held.
fn window_needs_a_frame(kind: SDL_EventType) -> bool {
    (SDL_EVENT_WINDOW_FIRST.0..=SDL_EVENT_WINDOW_LAST.0).contains(&kind.0)
        || matches!(kind, SDL_EVENT_RENDER_TARGETS_RESET | SDL_EVENT_RENDER_DEVICE_RESET | SDL_EVENT_RENDER_DEVICE_LOST)
}

/// The launch line, once: the launcher's first frame is on the screen.
fn first_frame_line(already: &std::sync::atomic::AtomicBool, since: Option<std::time::Duration>) -> Option<String> {
    let since = since?;
    (!already.swap(true, std::sync::atomic::Ordering::Relaxed))
        .then(|| format!("nightjar: launch to first frame {} ms", since.as_millis()))
}

fn main() {
    LAUNCH.get_or_init(std::time::Instant::now);
    // Every request to a host says which Nightjar asks: `User-Agent: Nightjar/<version>`.
    nightjar_core::http::set_app_version(VERSION);
    // Package checks run without a window: `nightjar --check-pyrowave <report file>`.
    let mut args = std::env::args_os().skip(1);
    if args.next().is_some_and(|flag| flag == "--check-pyrowave") {
        let Some(report) = args.next() else {
            eprintln!("nightjar: --check-pyrowave needs a report file");
            std::process::exit(64);
        };
        std::process::exit(stream::pyrowave_mode::package_check(std::path::Path::new(&report)));
    }
    // Frontends start a stream directly (see `cli`); a release build has no console to print to.
    let (args, after_setup) = self_update::split_after_setup(std::env::args_os().skip(1).collect());
    if self_update::install_running(after_setup) {
        // Its progress window is up, and it starts Nightjar when it ends.
        eprintln!("nightjar: the update is installing");
        std::process::exit(75);
    }
    let (args, at_sign_in) = autostart::split_sign_in(args);
    let (args, second_screen) = second_screen::split_flag(args);
    let (args, second_screen_join) = second_screen::split_join(args);
    let usage_box = cli::shows_usage_box(cli::HAS_CONSOLE, &args);
    let (mut launch, link) = match cli::parse(args) {
        Ok(cli::Parsed::Gui) => (None, None),
        Ok(cli::Parsed::Launch(launch)) => (Some(launch), None),
        // An open window takes the link; this start ends here, before it touches the log.
        Ok(cli::Parsed::Link(link)) => {
            if link_channel::send(&link) {
                return;
            }
            (None, Some(link))
        }
        Ok(cli::Parsed::Help) => {
            println!("{}", cli::USAGE);
            if usage_box {
                message_box(SDL_MESSAGEBOX_INFORMATION, cli::USAGE);
            }
            return;
        }
        // No box: install checks run it without a console and must not wait on a click.
        Ok(cli::Parsed::Version) => {
            cli::use_parent_console();
            // Not println!: a reader that is gone already (PowerShell does not wait for a window
            // program behind `>`) is no reason to panic.
            let _ = std::io::Write::write_all(&mut std::io::stdout(), format!("Nightjar {VERSION}\n").as_bytes());
            return;
        }
        Err(e) => {
            let text = format!("{e}\n{}", cli::USAGE);
            eprintln!("nightjar: {text}");
            if usage_box {
                message_box(SDL_MESSAGEBOX_ERROR, &text);
            }
            std::process::exit(64);
        }
    };
    // The second screen's stream is a frontend start of the first's game; it ends with its stream.
    let second_screen = second_screen_join.and_then(|join| match (second_screen, join) {
        (Ok(None), Some(_)) => Err("--second-screen-join needs --second-screen".to_owned()),
        (point, join) => point.map(|p| p.map(|p| (p, join))),
    });
    match (second_screen, launch.as_mut()) {
        (Ok(None), _) => {}
        (Ok(Some((point, join))), Some(launch)) if launch.app.is_some() => {
            second_screen::mark(point);
            launch.quit_on_exit = true;
            launch.join = join;
        }
        (Ok(Some(_)), _) => {
            eprintln!("nightjar: --second-screen needs --host and --app");
            std::process::exit(64);
        }
        (Err(e), _) => {
            eprintln!("nightjar: {e}");
            std::process::exit(64);
        }
    }
    // After the package check, which must not write to the user's log.
    logpipe::start();
    let _log = logpipe::Guard;
    #[cfg(windows)]
    match logfile::logs_dir() {
        Ok(dir) => crash_dump::install(&dir),
        Err(e) => eprintln!("nightjar: crash dumps off: no logs folder: {e}"),
    }
    let error_box = cli::shows_error_box(launch.as_ref());
    if let Err(e) = run(launch, open_play::Started { at_sign_in, after_setup }, link) {
        eprintln!("nightjar: {e}");
        if error_box {
            message_box(SDL_MESSAGEBOX_ERROR, &e);
        }
        logpipe::shutdown();
        std::process::exit(1);
    }
}

fn message_box(flags: SDL_MessageBoxFlags, text: &str) {
    let text = std::ffi::CString::new(text).unwrap_or_default();
    // SAFETY: NUL-terminated strings; a null parent window is allowed.
    unsafe { SDL_ShowSimpleMessageBox(flags, c"Nightjar".as_ptr(), text.as_ptr(), std::ptr::null_mut()) };
}

/// The renderer's device PyroWave GPU decoding is tied to: the Direct3D 11 device on Windows, the
/// SDL GPU device (the "gpu" renderer, Vulkan) elsewhere.
fn renderer_gpu_device(renderer: *mut SDL_Renderer) -> Option<stream::pipeline::GpuDevice> {
    let (name, property) = if cfg!(windows) {
        ("direct3d11", SDL_PROP_RENDERER_D3D11_DEVICE_POINTER)
    } else {
        ("gpu", SDL_PROP_RENDERER_GPU_DEVICE_POINTER)
    };
    if sdl::renderer_name(renderer) != name { return None; }
    let device = sdl::renderer_pointer(renderer, property);
    (!device.is_null()).then_some(stream::pipeline::GpuDevice(device))
}

/// Whether Linux needs the GPU renderer for Vulkan video, which decodes on its Vulkan device: when
/// VAAPI cannot open a device (NVIDIA's driver without a VAAPI one) and the GPU SDL picks has the
/// video extensions, or `NIGHTJAR_VULKAN_VIDEO=1` (`choice`); not when no stream can decode with it
/// (`vulkan_video_possible`) or `NIGHTJAR_VULKAN_VIDEO=0`. A machine with VAAPI keeps its
/// renderer, unless its VA driver wraps another API (`linux_video::vaapi_plan`). The reason goes
/// to the log.
#[cfg(unix)]
fn gpu_for_vulkan_video(possible: bool, choice: vulkan_video::Choice) -> bool {
    use vulkan_video::Choice;
    if !possible || choice == Choice::Off {
        return false;
    }
    let why = match (choice, media::vaapi_device(None)) {
        (Choice::First, _) => format!("{}=1", vulkan_video::ENVIRONMENT),
        (_, Err(error)) => format!("VAAPI is unavailable ({error})"),
        // A wrapper VA driver (nvidia-vaapi-driver) is left out of the order: Vulkan video
        // decodes instead, on the GPU renderer.
        (_, Ok(identity)) => match linux_video::vaapi_plan(Ok(&identity.vendor), stream::env_flag(linux_video::FORCE_VAAPI_ENVIRONMENT) == Some(true)) {
            linux_video::VaapiPlan { skip: true, why: Some(why) } => why,
            _ => return false,
        },
    };
    match stream::pyrowave_sdk::zero_copy::video_devices() {
        Ok(_) => {
            vulkan_video::note(&format!("{why}: the GPU renderer is used for Vulkan video decoding"));
            true
        }
        Err(error) => {
            vulkan_video::note(&format!("{why}, and Vulkan video cannot be used either: {error}"));
            false
        }
    }
}

/// Whether a stream can decode with Vulkan video or map VAAPI's pictures into Vulkan images, so the
/// GPU renderer's device is made with the extensions for them (video ones and decode queues unless
/// `NIGHTJAR_VULKAN_VIDEO` is 0, `choice`; DMA-BUF import ones unless `NIGHTJAR_VAAPI_VULKAN_MAP`
/// is 0, `map`): on Linux, when a stream may decode in hardware (`any_hardware_decoding`).
#[cfg(any(unix, test))]
fn vulkan_video_possible(hardware_decoding: bool, choice: vulkan_video::Choice, map: bool) -> bool {
    cfg!(unix) && hardware_decoding && (choice != vulkan_video::Choice::Off || map)
}

/// `NIGHTJAR_VAAPI_VULKAN_MAP` from the environment (`vulkan_video::map_switched_on`).
#[cfg(unix)]
fn vaapi_map_switched_on() -> bool {
    vulkan_video::map_switched_on(std::env::var_os(vulkan_video::MAP_ENVIRONMENT).as_deref())
}

/// Whether a stream may decode in hardware: the global decoder setting, or a profile's, a host's
/// or an app's override of it, applied over the global settings as a stream applies it. The
/// renderer is chosen once for every stream, so only when all of them say Software is it chosen
/// as for software decoding.
#[cfg_attr(not(unix), allow(dead_code))]
fn any_hardware_decoding<'a>(settings: &config::Settings, overrides: impl IntoIterator<Item = &'a config::Overrides>) -> bool {
    settings.decoder != DecoderPreference::Software
        || overrides.into_iter().any(|own| own.apply(settings).decoder != DecoderPreference::Software)
}

/// `settings` or an override of it (`any_hardware_decoding`) asks for HDR.
fn any_hdr_asked<'a>(settings: &config::Settings, overrides: impl IntoIterator<Item = &'a config::Overrides>) -> bool {
    settings.hdr || overrides.into_iter().any(|own| own.apply(settings).hdr)
}

/// Whether any layer (the global settings, a PC, a game or a profile) turns on PyroWave's compute
/// queue: on Linux the renderer's shared device gets the queue only when it is made.
fn any_async_compute_asked<'a>(settings: &config::Settings, overrides: impl IntoIterator<Item = &'a config::Overrides>) -> bool {
    settings.pyrowave_async_compute || overrides.into_iter().any(|own| own.apply(settings).pyrowave_async_compute)
}

fn run(launch: Option<cli::Launch>, started: open_play::Started, link: Option<String>) -> Result<(), String> {
    sdl::set_hint(SDL_HINT_APP_NAME, c"Nightjar");
    sdl::set_hint(SDL_HINT_APP_ID, c"io.github.ramazankara.nightjar");
    #[cfg(windows)]
    {
        // The decoder shares the renderer's D3D11 device from its own thread.
        sdl::set_hint(SDL_HINT_RENDER_DRIVER, c"direct3d11");
        sdl::set_hint(SDL_HINT_RENDER_DIRECT3D_THREADSAFE, c"1");
    }
    // VAAPI pictures reach the OpenGL renderer as EGL images (`dmabuf`), which needs an EGL
    // context on X11 as well (Wayland's always is), as moonlight-qt forces it.
    #[cfg(unix)]
    sdl::set_hint(SDL_HINT_VIDEO_FORCE_EGL, c"1");
    // Keep Alt+Tab and the Windows key for the host while the keyboard is grabbed.
    sdl::set_hint(SDL_HINT_ALLOW_ALT_TAB_WHILE_GRABBED, c"0");
    sdl::set_hint(SDL_HINT_VIDEO_MINIMIZE_ON_FOCUS_LOSS, c"0");
    sdl::set_hint(SDL_HINT_AUDIO_DEVICE_SAMPLE_FRAMES, c"480");
    sdl::set_hint(SDL_HINT_JOYSTICK_ALLOW_BACKGROUND_EVENTS, c"1");
    sdl::set_hint(SDL_HINT_MOUSE_RELATIVE_MODE_CENTER, c"1");
    // The second screen's window opens beside the first stream without taking its keyboard and mouse.
    if second_screen::this_process() {
        sdl::set_hint(SDL_HINT_WINDOW_ACTIVATE_WHEN_SHOWN, c"0");
        sdl::set_hint(SDL_HINT_WINDOW_ACTIVATE_WHEN_RAISED, c"0");
    }
    // Before Vulkan loads: gamescope's layer without its socket would block the swapchain.
    if handheld::gamescope() {
        let socket = handheld::gamescope_socket(std::env::var_os("XDG_RUNTIME_DIR").as_deref(), std::env::var_os("GAMESCOPE_WAYLAND_DISPLAY").as_deref());
        if handheld::gamescope_layer_off(socket.as_deref(), std::path::Path::exists) {
            eprintln!("nightjar: gamescope: its socket {} is not reachable here, so gamescope's Vulkan layer is off (DISABLE_GAMESCOPE_WSI=1) and the output is SDR", socket.unwrap_or_default().display());
            std::env::set_var("DISABLE_GAMESCOPE_WSI", "1");
        }
    }
    // Before SDL, VAAPI or Vulkan load Mesa.
    #[cfg(target_os = "linux")]
    amd_decode::request();

    // The second screen's stream takes no controllers: they stay with the first.
    let gamepads = if second_screen::this_process() { SDL_InitFlags::default() } else { SDL_INIT_GAMEPAD };
    // SAFETY: SDL initialisation on the main thread.
    unsafe {
        if !SDL_Init(SDL_INIT_VIDEO | SDL_INIT_AUDIO | gamepads | SDL_INIT_EVENTS) {
            return Err(format!("SDL could not start: {}", sdl::error()));
        }
    }
    #[cfg(feature = "demo-library")]
    demo_driver::require_sandbox()?;
    let store = Store::open().map_err(|e| format!("No config directory: {e}"))?;
    // The first screen's process owns the files; the second only reads them.
    let store = if second_screen::this_process() { store.read_only() } else { store };
    let settings = store.settings();
    let profiles = store.profiles();
    let mappings = store.controller_mappings();
    let hosts = store.hosts();
    let gpu = settings.needs_gpu() || hosts.iter()
        .any(|host| host.settings.as_ref().is_some_and(config::Overrides::needs_gpu) || host.app_settings.values().any(config::Overrides::needs_gpu))
        || profiles.entries.iter().any(|profile| profile.settings.needs_gpu());
    #[cfg(unix)]
    let vulkan_choice = vulkan_video::Choice::from_environment();
    #[cfg(unix)]
    let vulkan_video = vulkan_video_possible(any_hardware_decoding(&settings, hosts.iter()
        .flat_map(|host| host.settings.iter().chain(host.app_settings.values()))
        .chain(profiles.entries.iter().map(|profile| &profile.settings))), vulkan_choice, vaapi_map_switched_on());
    // The Renderer setting decides, and at Auto what needs the GPU renderer.
    #[cfg(unix)]
    let gpu = handheld::starts_on_gpu(settings.renderer, settings.renderer == config::Renderer::Auto && (gpu || gpu_for_vulkan_video(vulkan_video, vulkan_choice)));
    #[cfg(not(unix))]
    let vulkan_video = false;

    let hdr_asked = any_hdr_asked(&settings, hosts.iter()
        .flat_map(|host| host.settings.iter().chain(host.app_settings.values()))
        .chain(profiles.entries.iter().map(|profile| &profile.settings)));
    sdl::set_hdr_asked(hdr_asked);
    // Before the renderer: on Linux its device gets PyroWave's compute queue only with this on.
    stream::pyrowave_sdk::queue::use_async_compute(any_async_compute_asked(&settings, hosts.iter()
        .flat_map(|host| host.settings.iter().chain(host.app_settings.values()))
        .chain(profiles.entries.iter().map(|profile| &profile.settings))));

    // Steam's Game Mode: fullscreen without a border from the start (handheld.rs).
    let start = handheld::start_window(handheld::gamescope());
    if handheld::gamescope() {
        eprintln!("nightjar: gamescope: starting fullscreen without a border; Start+Back opens the stream menu");
    }
    // SAFETY: window and renderer creation on the main thread.
    let (window, renderer, wake_event) = unsafe {
        let window = SDL_CreateWindow(
            c"Nightjar".as_ptr(),
            1280,
            800,
            start_flags(start),
        );
        if window.is_null() {
            return Err(format!("No window: {}", sdl::error()));
        }
        window_place::set_minimum_size(window);
        if let Some(point) = second_screen::point() {
            if !window_place::move_to_display_at(window, point) {
                return Err(format!("No monitor at {},{} for the second screen: {}", point.0, point.1, sdl::error()));
            }
        } else if start.restore_place {
            window_place::apply(window, store.window_placement().as_ref());
        }
        let renderer =
            sdl::create_renderer_with(window, gpu, vulkan_video);
        if renderer.is_null() {
            return Err(format!("No renderer: {}", sdl::error()));
        }
        SDL_SetRenderVSync(renderer, 1);
        (window, renderer, SDL_RegisterEvents(1))
    };
    // Windows windows take the executable's icon (build.rs) in every size the shell asks for.
    #[cfg(not(windows))]
    sdl::set_icon(window, &theme::logo_rgba(64), 64);
    sdl::exclude_from_capture_if_asked(window);
    // Before the window shows, so its first frame already has the dark bar; again when the palette changes.
    sdl::blend_title_bar(window, *theme::NIGHT, *theme::ON_SURFACE);

    let waker: hosts::Waker = Arc::new(move || {
        // SAFETY: SDL_PushEvent is thread-safe; the event is plain data.
        unsafe {
            let mut event: SDL_Event = std::mem::zeroed();
            event.r#type = wake_event;
            SDL_PushEvent(&mut event);
        }
    });

    let mut gui = Gui::new();
    // Before the window shows: UI Automation's adapter has to be on it from the start.
    if let Err(e) = gui.attach_accessibility(window, waker.clone()) {
        eprintln!("nightjar: screen readers cannot read this window: {e}");
    }
    // SAFETY: a live window.
    unsafe { SDL_ShowWindow(window) };
    theme::apply(&gui.ctx);
    let display = display_of(window);
    gui.set_refresh(display.refresh_hz);
    shortcuts::set_hotkeys(store.hotkeys());
    let mut launcher = Launcher::new(settings.clone(), profiles, store.shortcuts(), display);
    launcher.recents = store.recents();
    launcher.pin = Some(if second_screen::this_process() {
        let hand_over = second_screen::follow_first(|| {
            // SAFETY: SDL_PushEvent is thread-safe; the event is plain data.
            unsafe {
                let mut event: SDL_Event = std::mem::zeroed();
                event.r#type = SDL_EVENT_QUIT.into();
                SDL_PushEvent(&mut event);
            }
        });
        let token = hand_over.recv_timeout(second_screen::HAND_OVER_WAIT).ok();
        pin_ui::PinLock::open_second_screen(store.dir(), token.as_deref())
    } else {
        pin_ui::PinLock::open(store.dir())
    });
    launcher.favourites = store.favourites();
    launcher.hidden_games = store.hidden_games();
    launcher.system_languages = sdl::preferred_languages();
    #[cfg(windows)]
    {
        launcher.tearing_supported =
            media::swapchain_allows_tearing(sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_SWAPCHAIN_POINTER));
    }
    #[cfg(not(windows))]
    {
        launcher.tearing_supported = true;
    }
    launcher.gpu_device = renderer_gpu_device(renderer);
    // Off Windows the swapchain's present mode decides tearing, logged when a stream starts.
    eprintln!(
        "nightjar: renderer {}, display {}x{} at {:.2} Hz{}",
        sdl::renderer_name(renderer),
        display.width,
        display.height,
        display.refresh_hz,
        if cfg!(windows) { format!(", tearing {}", launcher.tearing_supported) } else { String::new() }
    );

    let opened_by_link = link.is_some();
    let links = Arc::new(link_channel::Inbox::default());
    if let Some(link) = link {
        links.put(link);
    }
    // A frontend's start ends with its stream, and a link waiting behind it would be lost: the
    // link starts its own window then.
    if launch.is_none() && !cfg!(feature = "demo-library") {
        if let Err(e) = link_channel::listen(links.clone(), waker.clone()) {
            eprintln!("nightjar: pairing links do not come to this window: {e}");
        }
    }
    #[cfg(not(feature = "demo-library"))]
    let records = store.hosts();
    #[cfg(feature = "demo-library")]
    let records = ui::demo::records();
    // The demo library looks for nobody, and the second screen streams from the first's PC.
    let mut hosts = if cfg!(feature = "demo-library") || second_screen::this_process() { Hosts::without_discovery(records, waker.clone()) } else { Hosts::new(records, waker.clone()) };
    let cli = match launch {
        Some(_) if std::env::var_os("NIGHTJAR_AUTOSTREAM").is_some() => {
            return Err("--host and NIGHTJAR_AUTOSTREAM both ask for a stream; give one".into());
        }
        Some(launch) => Some(cli::CliLaunch::new(launch, &hosts)?),
        None => None,
    };
    hosts.load_cached_libraries(&store);
    {
        // Open on Home when it is on and some PC has a library to show.
        let sources: Vec<console::Source> = hosts.list.iter().map(console::Source::of).collect();
        if console::opens_on_home(settings.console_home, &sources) {
            launcher.go(Screen::Home);
        }
    }
    #[cfg(feature = "demo-library")]
    ui::demo::fill(&mut hosts, &mut launcher, &gui.ctx);
    // A stream started from the command line waits for the next ordinary start.
    if cli.is_none() && std::env::var_os("NIGHTJAR_AUTOSTREAM").is_none() && launcher.show_kept_settings_once(&hosts.list) {
        if let Err(e) = store.save_settings(&launcher.settings) {
            eprintln!("nightjar: saving settings failed: {e}");
        }
    }
    let identity = match store.identity().filter(|_| !cfg!(feature = "demo-library")) {
        Some(saved) => match ClientIdentity::from_pem(saved.unique_id, &saved.certificate_pem, &saved.private_key_pem) {
            Ok(identity) => {
                launcher.identity_ready = true;
                Some(identity)
            }
            Err(e) => {
                eprintln!("nightjar: stored identity unusable ({e}); creating a new one");
                None
            }
        },
        None => None,
    };
    let screenshot_dir = std::env::var_os("NIGHTJAR_SCREENSHOT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| store.dir().join("screenshots"));
    let mut pads = Gamepads::default();
    pads.mappings = mappings;
    pads.configure(&settings);
    let updates = update_check::Updates::start(
        settings.check_updates && !cfg!(feature = "demo-library") && cli.is_none() && std::env::var_os("NIGHTJAR_AUTOSTREAM").is_none(),
        waker.clone(),
    );
    let open_play = open_play::at_launch(
        &store,
        &mut launcher,
        &hosts,
        open_play::opened_by_player(cli.is_some(), std::env::var_os("NIGHTJAR_AUTOSTREAM").is_some(), opened_by_link),
        started,
    );
    let mut app = App {
        window,
        renderer,
        gui,
        store,
        hosts,
        launcher,
        pads,
        connecting: None,
        cancelled: false,
        waking: wol::Wakes::default(),
        streaming: None,
        pending_launch: None,
        pending_replace: None,
        quitting_on_leave: None,
        pending_end: None,
        identity: None,
        shortcut_file: None,
        folder_pick: None,
        network_query: Some(network::query(waker.clone())),
        power_probe: Some(power::probe(waker.clone())),
        power_request: None,
        network_test: None,
        network_launch: None,
        next_network_check: Instant::now(),
        links,
        #[cfg(feature = "demo-library")]
        demo: demo_driver::start(waker.clone()),
        waker,
        wake_event,
        screenshot_dir,
        screenshot_requested: false,
        stick_held: [false; 4],
        nav_repeat: nav_repeat::NavRepeat::default(),
        pointer: handheld::Pointer::new(handheld::gamescope()),
        hdr_note: false,
        packed_note: None,
        running: true,
        autostream: autostream::Autostream::from_env(),
        updates,
        open_play,
        cli,
        second_screen: None,
        color_probed: None,
        base_probed: None,
        #[cfg(unix)]
        renderer_state: RendererState {
            choice: settings.renderer,
            made_for_hdr: gpu && hdr_asked && handheld::hdr_display(handheld::gamescope(), sdl::any_display_hdr()),
            vulkan_video,
            gpu_failed: (gpu && sdl::renderer_name(renderer) != "gpu")
                .then(|| sdl::gpu_renderer_failure().unwrap_or_else(|| format!("SDL made {:?} instead", sdl::renderer_name(renderer)))),
        },
        gpu_failure: None,
        pin_pending: None,
        select_read: false,
        move_watch: Default::default(),
        title_bar: *theme::NIGHT,
        window_changed: true,
    };
    match identity {
        Some(identity) => {
            app.hosts.set_identity(Arc::new(identity));
            app.hosts.poll();
        }
        // The demo library has no identity: without one nothing can ask a PC.
        None if cfg!(feature = "demo-library") => {}
        None => {
            let (sender, receiver) = channel();
            let waker = app.waker.clone();
            std::thread::spawn(move || {
                let _ = sender.send(ClientIdentity::generate().map_err(|e| e.to_string()));
                waker();
            });
            app.identity = Some(receiver);
        }
    }

    while app.running {
        app.updates.hold(app.streaming.is_some() || app.connecting.is_some() || app.network_launch.is_some());
        if app.streaming.is_some() {
            app.stream_frame();
        } else {
            app.launcher_frame();
        }
        app.open_play_step();
    }
    eprintln!("nightjar: leaving");
    app.shutdown();
    if app.autostream.as_ref().is_some_and(|a| a.failed) {
        logpipe::shutdown();
        std::process::exit(2);
    }
    match app.cli.and_then(|c| c.failed) {
        Some(why) => Err(why),
        None => Ok(()),
    }
}

impl App {
    fn save_edits(&mut self) {
        // Closing Nightjar is leaving it: a start within the grace time stays unlocked.
        if let Some(pin) = self.launcher.pin.as_mut() {
            pin.left();
            pin.flush();
        }
        if let Some(command) = self.launcher.screen.save_command() { self.command(command); }
    }

    fn save_hosts_and_place(&self) {
        if let Err(e) = self.store.save_hosts(&self.hosts.records()) { eprintln!("nightjar: PCs not saved: {e}"); }
        // Under gamescope the window was never placed from the saved place, so Desktop Mode's stays.
        if handheld::start_window(handheld::gamescope()).restore_place {
            if let Some(placement) = window_place::current(self.window, self.store.window_placement().as_ref()) {
                if let Err(e) = self.store.save_window_placement(&placement) { eprintln!("nightjar: window place not saved: {e}"); }
            }
        }
    }

    /// Before an update's hand-over, with nothing streaming: the next Nightjar can read these files
    /// as soon as it starts (a new AppImage, or the old program after a setup that stopped), and
    /// would overwrite with what it read what shutdown writes only later.
    fn save_for_next_run(&mut self) {
        self.save_edits();
        self.save_hosts_and_place();
    }

    fn shutdown(&mut self) {
        self.save_edits();
        self.network_test = None;
        // Its input closes now; the process going closes it too.
        if let Some(companion) = self.second_screen.take() {
            companion.end();
            self.second_screen_gone();
        }
        // The PC's automatic bitrate goes off after the window has gone, not with the process.
        let abr_stop = self.streaming.take().and_then(|(streaming, _)| streaming.finish(self.window, &mut self.pads));
        if let Some(c) = &self.connecting {
            c.cancel();
        }
        self.save_hosts_and_place();
        self.pads.close_all();
        self.gui.destroy();
        if self.connecting.is_some() {
            // Ending a launch can take seconds: the window goes now rather than sitting frozen meanwhile.
            // SAFETY: the window run() created, still alive.
            unsafe { SDL_HideWindow(self.window) };
        }
        // The connect thread may be using the renderer's GPU device (a decoder being made on it),
        // so a launch in progress ends, and a session it started stops, before the renderer goes.
        let (renderer, window) = (self.renderer, self.window);
        // SAFETY: tearing down what run() created, with nothing else using it any more.
        stream::end_before(self.connecting.take(), stream::LEAVE_WAIT, || unsafe {
            SDL_DestroyRenderer(renderer);
            SDL_DestroyWindow(window);
            SDL_Quit();
        });
        if let Some(stop) = abr_stop { stop.wait(); }
    }

    /// Drops the saved library of a PC that is unpaired or removed.
    fn forget_library(&self, host: u64) {
        if let Some(unique_id) = self.hosts.get(host).map(|h| h.record.unique_id.clone()).filter(|id| !id.is_empty()) {
            self.store.forget_library(&unique_id);
        }
    }

    fn save_hosts(&self) {
        save_hosts(&self.store, &self.hosts);
    }

    /// Adds the PC a pairing link names and pairs with its one-time PIN and certificate.
    fn pair_link(&mut self, link: rubylight_protocol::pairing_uri::PairingUri) {
        let id = self.hosts.add_manual(&link.host, Some(link.port));
        self.save_hosts();
        self.launcher.dialog = Dialog::Pairing { host: id, pin: String::new() };
        self.hosts.pair_link(id, link);
    }

    /// Shows a link the system opened with Nightjar, once nothing else is in the way: a stream
    /// (this runs only between them), a stream being started, an open dialog or the PIN screen.
    fn open_waiting_link(&mut self) {
        if self.connecting.is_some() || self.network_launch.is_some() || !self.launcher.can_open_link() {
            return;
        }
        let Some(link) = self.links.take() else { return };
        // SAFETY: window calls on the main thread with a live window.
        unsafe {
            if SDL_GetWindowFlags(self.window) & SDL_WINDOW_MINIMIZED != 0 {
                SDL_RestoreWindow(self.window);
            }
            SDL_RaiseWindow(self.window);
        }
        self.launcher.open_link(&link);
    }

    /// Add PC: an address adds the PC; a QR pairing link adds it and pairs with the link's
    /// one-time PIN and certificate, at once: it was typed or pasted here on purpose.
    fn add_entry(&mut self, text: &str) {
        match hosts::parse_entry(text) {
            Ok(hosts::Entry::Address(address, port)) => {
                self.hosts.add_manual(&address, port);
                self.save_hosts();
            }
            Ok(hosts::Entry::Link(link)) => self.pair_link(*link),
            // The link holds a PIN and passphrase: it is not shown back.
            Err(hosts::LINK_INVALID) => self.launcher.message(tr("Could not pair"), tr(hosts::LINK_INVALID)),
            Err(why) => self.launcher.message(tr(why), text.trim()),
        }
    }

    /// Gamepad buttons and the left stick become arrow keys, Enter and Escape for egui.
    fn navigate(&mut self, event: &SDL_Event) {
        // SAFETY: union members read by event type.
        unsafe {
            match SDL_EventType(event.r#type) {
                SDL_EVENT_GAMEPAD_BUTTON_DOWN => {
                    let swap = if self.streaming.is_some() { self.pads.swap_face_buttons } else { self.launcher.settings.swap_face_buttons };
                    let button = SDL_GamepadButton(i32::from(event.gbutton.button));
                    let key = match button {
                        SDL_GAMEPAD_BUTTON_DPAD_UP => Some(egui::Key::ArrowUp),
                        SDL_GAMEPAD_BUTTON_DPAD_DOWN => Some(egui::Key::ArrowDown),
                        SDL_GAMEPAD_BUTTON_DPAD_LEFT => Some(egui::Key::ArrowLeft),
                        SDL_GAMEPAD_BUTTON_DPAD_RIGHT => Some(egui::Key::ArrowRight),
                        // What the other buttons do is the binding list's (shortcuts.rs, which Help shows).
                        _ => match shortcuts::Button::from_sdl(button).and_then(|b| shortcuts::pad_action(b, swap)) {
                            Some(Action::OpenSettings) => {
                                if self.streaming.is_none() && matches!(self.launcher.dialog, Dialog::None) && self.connecting.is_none() {
                                    if let Some(command) = self.launcher.screen.save_command() { self.command(command); }
                                    self.launcher.open_settings();
                                }
                                None
                            }
                            Some(action @ (Action::PreviousTab | Action::NextTab)) => {
                                if self.streaming.is_none() && self.connecting.is_none() {
                                    let mut commands = Vec::new();
                                    self.launcher.step_destination(&self.gui.ctx, action == Action::NextTab, &mut commands);
                                    for command in commands { self.command(command); }
                                }
                                None
                            }
                            Some(action) => nav_key(action),
                            None => None,
                        },
                    };
                    if let Some(key) = key {
                        self.push_nav(key);
                        if matches!(key, egui::Key::ArrowUp | egui::Key::ArrowDown | egui::Key::ArrowLeft | egui::Key::ArrowRight) {
                            self.nav_repeat.hold(event.gbutton.which.0, nav_repeat::Input::Button(event.gbutton.button), key, Instant::now());
                        }
                    }
                }
                SDL_EVENT_GAMEPAD_BUTTON_UP => {
                    self.nav_repeat.release(event.gbutton.which.0, nav_repeat::Input::Button(event.gbutton.button));
                }
                SDL_EVENT_GAMEPAD_AXIS_MOTION => {
                    let a = event.gaxis;
                    let (slot, key_neg, key_pos) = match SDL_GamepadAxis(i32::from(a.axis)) {
                        SDL_GAMEPAD_AXIS_LEFTX => (0, egui::Key::ArrowLeft, egui::Key::ArrowRight),
                        SDL_GAMEPAD_AXIS_LEFTY => (1, egui::Key::ArrowUp, egui::Key::ArrowDown),
                        _ => return,
                    };
                    let held = i32::from(a.value).abs() > 24000;
                    if held && !self.stick_held[slot] {
                        let key = if a.value < 0 { key_neg } else { key_pos };
                        self.push_nav(key);
                        self.nav_repeat.hold(a.which.0, nav_repeat::Input::Axis(a.axis, a.value > 0), key, Instant::now());
                    }
                    self.stick_held[slot] = i32::from(a.value).abs() > 12000 && (held || self.stick_held[slot]);
                    if !self.stick_held[slot] {
                        self.nav_repeat.release(a.which.0, nav_repeat::Input::Axis(a.axis, a.value > 0));
                    }
                }
                _ => {}
            }
        }
    }

    /// Steps a held D-pad direction or stick again (nav_repeat.rs). A hold whose release went to
    /// the stream, or whose controller left, is checked against the controller and dropped.
    /// True when it stepped.
    fn repeat_navigation(&mut self) -> bool {
        if self.launcher.screen == Screen::Controllers && self.launcher.controller_test {
            self.nav_repeat.clear();
            return false;
        }
        let keys = self.nav_repeat.due(Instant::now(), |pad, input| {
            // SAFETY: SDL gamepad queries on the main thread; a null gamepad has left.
            unsafe {
                let gamepad = SDL_GetGamepadFromID(SDL_JoystickID(pad));
                if gamepad.is_null() {
                    return false;
                }
                match input {
                    nav_repeat::Input::Button(button) => SDL_GetGamepadButton(gamepad, SDL_GamepadButton(i32::from(button))),
                    nav_repeat::Input::Axis(axis, positive) => {
                        let value = i32::from(SDL_GetGamepadAxis(gamepad, SDL_GamepadAxis(i32::from(axis))));
                        if positive { value > 12000 } else { value < -12000 }
                    }
                }
            }
        });
        for key in &keys {
            self.push_nav(*key);
        }
        !keys.is_empty()
    }

    /// A controller's navigation key for egui. In the launcher a press with nothing focused
    /// focuses the page's first item (`Launcher::controller_key`); the stream menu focuses Return
    /// to stream itself.
    fn push_nav(&mut self, key: egui::Key) {
        let events = if self.streaming.is_none() { self.launcher.controller_key(&self.gui.ctx, key) } else { gui::tap(key).to_vec() };
        for e in events {
            self.gui.push(e);
        }
    }

    /// Minimizing or hiding the window is leaving Nightjar for the PIN lock's grace time.
    fn track_visibility(&mut self, event: &SDL_Event) {
        // SAFETY: reading the event type.
        let kind = SDL_EventType(unsafe { event.r#type });
        let Some(pin) = self.launcher.pin.as_mut() else { return };
        match kind {
            SDL_EVENT_WINDOW_MINIMIZED | SDL_EVENT_WINDOW_HIDDEN => pin.left(),
            SDL_EVENT_WINDOW_RESTORED | SDL_EVENT_WINDOW_SHOWN => pin.returned(),
            _ => {}
        }
    }

    fn handle_launcher_event(&mut self, event: &SDL_Event) {
        log_window_hdr(event);
        self.track_visibility(event);
        // SAFETY: reading the event type.
        self.window_changed |= window_needs_a_frame(SDL_EventType(unsafe { event.r#type }));
        if let Some(source) = input_source::of(event, |id| self.pads.real_pad_type(id)) {
            self.launcher.input_source = source;
        }
        // SAFETY: reading the event type.
        let kind = SDL_EventType(unsafe { event.r#type });
        if kind == SDL_EVENT_WINDOW_DISPLAY_SCALE_CHANGED || another_display(kind) {
            window_place::set_minimum_size(self.window);
        }
        if another_display(kind) {
            self.gui.set_refresh(display_of(self.window).refresh_hz);
        }
        // SAFETY: SDL cursor calls on the main thread.
        match self.pointer.input(pointer_input(event)) {
            Some(true) => unsafe { SDL_ShowCursor(); },
            Some(false) => unsafe { SDL_HideCursor(); },
            None => {}
        }
        let scale = Scale::of(self.window);
        // SAFETY: union members read by event type.
        unsafe {
            match SDL_EventType(event.r#type) {
                SDL_EVENT_QUIT | SDL_EVENT_WINDOW_CLOSE_REQUESTED => self.running = false,
                SDL_EVENT_GAMEPAD_ADDED => {
                    if let Err(why) = self.pads.added(event.gdevice.which) {
                        self.launcher.message(tr("Controller not connected"), why.message());
                    }
                }
                SDL_EVENT_GAMEPAD_REMOVED => {
                    self.pads.removed(event.gdevice.which);
                }
                SDL_EVENT_GAMEPAD_BUTTON_DOWN | SDL_EVENT_GAMEPAD_BUTTON_UP | SDL_EVENT_GAMEPAD_AXIS_MOTION => {
                    if self.launcher.screen != Screen::Controllers || !self.launcher.controller_test { self.navigate(event); }
                }
                // A stream hotkey being recorded takes every key, F11 and F12 too; an arrow or Enter
                // alone goes on to egui and moves in the dialog.
                SDL_EVENT_KEY_DOWN if self.launcher.recording_hotkey() && !ui::moves_in_hotkey_dialog(event.key.scancode, event.key.r#mod) => {
                    if let Some(command) = self.launcher.hotkey_key_down(event.key.scancode, event.key.r#mod, event.key.repeat) {
                        self.command(command);
                    }
                }
                SDL_EVENT_KEY_DOWN if shortcuts::key_action(Scope::Launcher, event.key.scancode, event.key.r#mod) == Some(Action::Screenshot) => {
                    self.screenshot_requested = true;
                }
                SDL_EVENT_KEY_DOWN if shortcuts::key_action(Scope::Launcher, event.key.scancode, event.key.r#mod) == Some(Action::Fullscreen) => {
                    let fullscreen = SDL_GetWindowFlags(self.window) & SDL_WINDOW_FULLSCREEN != 0;
                    SDL_SetWindowFullscreen(self.window, !fullscreen);
                }
                _ => {
                    self.gui.handle(event, scale);
                }
            }
        }
    }

    fn launcher_frame(&mut self) {
        #[cfg(unix)]
        self.apply_renderer_choice();
        // Wait for input or a worker; animate the spinner while something runs.
        let busy = self.connecting.is_some() || self.waking.host().is_some() || self.network_launch.is_some() || self.network_test.is_some()
            || self.launcher.remote.running() || self.launcher.files.busy()
            || matches!(self.launcher.dialog, Dialog::Pairing { .. })
            || self.identity.is_some() || self.autostream.is_some()
            || self.launcher.pin.as_ref().is_some_and(pin_ui::PinLock::busy);
        let timeout = if busy { 16 } else { 250 };
        // A held direction wakes the loop for its next step.
        let timeout = self.nav_repeat.next(Instant::now()).map_or(timeout, |next| timeout.min(next.as_millis().min(250) as i32 + 1));
        // So does egui while it animates or settles a layout, at most once per refresh: without
        // this, a scroll, a hover fade or an opening list moved only when the next input came.
        let frame = Duration::from_secs_f32(1.0 / display_of(self.window).refresh_hz.max(30.0));
        let timeout = self.gui.repaint_in(frame).map_or(timeout, |wait| timeout.min(wait.as_millis() as i32));
        // SAFETY: event polling on the main thread.
        unsafe {
            let mut event: SDL_Event = std::mem::zeroed();
            if SDL_WaitEventTimeout(&mut event, timeout) {
                self.handle_launcher_event(&event);
                while SDL_PollEvent(&mut event) {
                    self.handle_launcher_event(&event);
                }
            }
        }
        if !self.running {
            return;
        }
        self.repeat_navigation();
        self.open_waiting_link();
        #[cfg(feature = "demo-library")]
        self.demo_steps();
        let test_events: Vec<_> = self.network_test.as_ref().map(|test| test.events.try_iter().collect()).unwrap_or_default();
        for event in test_events {
            match event {
                network_test::Event::Progress(stage) => self.launcher.dialog = Dialog::NetworkTest { stage },
                network_test::Event::Done(result) => {
                    let host = self.network_test.take().unwrap().host;
                    match result {
                        Ok(network_test::Outcome::Bandwidth(mbps)) => self.launcher.message(tr("Bandwidth test"), trf("{} Mbps received from the PC over pinned HTTPS. This measures transfer speed, not a recommended video bitrate.", &[&format!("{mbps:.1}")])),
                        Ok(network_test::Outcome::Pace(calibration, measurement)) => {
                            let text = trf("Saved {} Mbps frame pace ({}). Packet loss {}%, worst 100 ms loss {}%, missing frame tails {}%. The calibration applies while the network and endpoint link speeds match.",
                                &[&calibration.pace_mbps, &if calibration.qualified { tr("confirmed twice") } else { tr("best measured result; some loss limits exceeded") },
                                &format!("{:.2}", measurement.loss), &format!("{:.2}", measurement.worst_window_loss), &format!("{:.2}", measurement.tail_loss())]);
                            if let Some(host) = self.hosts.get_mut(host) { host.record.pace = Some(calibration); self.save_hosts(); }
                            self.launcher.message(tr("PyroWave calibration"), text);
                        }
                        Err(error) => self.launcher.message(tr("Network test failed"), error),
                    }
                }
            }
        }
        if let Some(receiver) = &self.network_query {
            if let Ok(network) = receiver.try_recv() {
                self.launcher.network = network;
                self.network_query = None;
                self.next_network_check = Instant::now() + Duration::from_secs(5);
                if let Some((host, app, resume, kind)) = self.network_launch.take() { self.launch_ready(host, app, resume, kind); }
            }
        } else if Instant::now() >= self.next_network_check {
            self.network_query = Some(network::query(self.waker.clone()));
        }
        if let Some((export, receiver)) = &self.shortcut_file {
            if let Ok(result) = receiver.try_recv() {
                let export = *export;
                self.shortcut_file = None;
                match result {
                    Ok(Some(path)) if export => {
                        match std::fs::write(&path, self.launcher.shortcuts.encode()) {
                            Ok(()) => self.launcher.message(tr("Shortcuts exported"), path.display().to_string()),
                            Err(e) => self.launcher.message(tr("Could not export shortcuts"), e.to_string()),
                        }
                    }
                    Ok(Some(path)) => match shortcuts::read(&path) {
                        Ok(entries) => self.launcher.dialog = Dialog::ImportShortcuts(entries),
                        Err(e) => self.launcher.message(tr("Could not import shortcuts"), e),
                    },
                    Ok(None) => {}
                    Err(e) => self.launcher.message(tr("File dialog unavailable"), e),
                }
            }
        }
        if let Some(offers) = self.power_probe.as_ref().and_then(|receiver| receiver.try_recv().ok()) {
            self.launcher.power = offers;
            self.power_probe = None;
        }
        if let Some((offer, result)) = self.power_request.as_ref().and_then(|(offer, receiver)| receiver.try_recv().ok().map(|r| (*offer, r))) {
            self.power_request = None;
            if let Err(why) = result {
                eprintln!("nightjar: {:?} failed: {why}", offer.action);
                self.launcher.message(tr(offer.action.failed()), why);
            }
        }
        if let Some(result) = self.folder_pick.as_ref().and_then(|(_, receiver)| receiver.try_recv().ok()) {
            if let Some((pick, _)) = self.folder_pick.take() {
                if let Some((title, body)) = finish_folder_pick(&mut self.launcher.folders, pick, result) {
                    self.launcher.message(title, body);
                }
            }
        }
        if let Some(receiver) = &self.identity {
            if let Ok(result) = receiver.try_recv() {
                self.identity = None;
                match result {
                    Ok(identity) => {
                        let _ = self.store.save_identity(&config::Identity {
                            unique_id: identity.unique_id().to_owned(),
                            certificate_pem: identity.certificate_pem().to_owned(),
                            private_key_pem: identity.private_key_pem().to_owned(),
                        });
                        self.hosts.set_identity(Arc::new(identity));
                        self.launcher.identity_ready = true;
                        self.hosts.poll();
                    }
                    Err(e) => self.launcher.message(tr("No pairing key"), e),
                }
            }
        }
        if self.connecting.is_none() {
            self.hosts.poll();
        }
        let (events, changed) = self.hosts.drain(&self.gui.ctx);
        if changed {
            self.save_hosts();
        }
        self.show_pending_end();
        for (unique_id, library) in self.hosts.take_library_saves() {
            if let Err(e) = self.store.save_library(&unique_id, &library) {
                eprintln!("nightjar: saving the library failed: {e}");
            }
        }
        for event in events {
            match event {
                HostEvent::Paired { id, result, .. } => {
                    if self.launcher.pairing_finished(id, result.map(|_| ())) {
                        self.save_hosts();
                        self.hosts.load_apps(id);
                        self.launcher.open(Screen::Apps(id));
                    }
                }
                HostEvent::Quit { id, result } => match result {
                    Err(e) if self.quitting_on_leave.as_ref().is_some_and(|(host, _)| *host == id) => {
                        let (_, name) = self.quitting_on_leave.take().expect("checked above");
                        self.launcher.message(tr("Could not quit the app"), trf("Couldn’t quit {} on the PC, so it is still running. {}", &[&name, &e]));
                    }
                    Ok(()) => {
                        if self.quitting_on_leave.as_ref().is_some_and(|(host, _)| *host == id) { self.quitting_on_leave = None; }
                        if let Some((host, app)) = self.pending_launch.take() {
                            if host == id {
                                self.launch(host, app, false, LaunchKind::Stream, LaunchFrom::Replace);
                            }
                        }
                    }
                    Err(e) => {
                        self.pending_launch = None;
                        self.launcher.message(tr("Could not quit the app"), e);
                    }
                },
                // A packet that could not be sent at all (no network, no route) ends the wait.
                // Only the running wake ends: a send of a cancelled one may fail after a new one began.
                HostEvent::Wake { id, run, result: Err(e) } => {
                    if self.waking.failed(id, run) {
                        self.launcher.message(tr("Could not wake the PC"), e);
                    }
                }
                HostEvent::Sleep { id, ask, name, confirmed, result } => self.launcher.sleep_answered(id, ask, &name, confirmed, result),
                HostEvent::PcPower { id, ask, name, action, confirmed, result } => {
                    self.launcher.pc_power_answered(id, ask, &name, action, confirmed, result)
                }
                HostEvent::HostUpdate { id, name, result } => self.launcher.update_answered(id, &name, result),
                HostEvent::PairingAnswer { id, request, name, denied, result } => {
                    self.launcher.pairing_answered(id, &request, &name, denied, result);
                }
                HostEvent::Unpaired { id, result } => {
                    if crate::hosts::now_unpaired(&result) {
                        self.forget_library(id);
                    }
                    if let Err(e) = result {
                        self.launcher.message(tr("Could not unpair"), e);
                    }
                }
                HostEvent::Session { action, result: Err(e), .. } => {
                    let title = match action {
                        ActionId::CloseGame => tr("Could not close the game"),
                        ActionId::End => tr("Could not end the session"),
                        _ => tr("The session action failed"),
                    };
                    self.launcher.message(title, e);
                }
                // Nothing changed on the PC: the game keeps running and the prompt says why not. No
                // stream follows to use the PIN's grant.
                HostEvent::ContinueHere { result: Err(e), .. } => {
                    self.forget_pin_grant();
                    self.launcher.message(tr("Could not continue here"), e);
                }
                HostEvent::ContinueHere { id, app, result: Ok(()) } => self.launch(id, app, true, LaunchKind::Stream, LaunchFrom::Launcher),
                HostEvent::SelectRead { id, app, resume, too, read } => {
                    self.select_read = false;
                    if self.streaming.is_some() || self.connecting.is_some() {
                        eprintln!("nightjar: another stream started while the PC said who plays; this start is dropped");
                        continue;
                    }
                    let playing = read.as_ref().and_then(|read| self.hosts.get(id).and_then(|h| ui::playing_elsewhere(h, app, read)));
                    let press_failed = read.as_ref().is_some_and(|read| read.press_failed);
                    let known = self.hosts.get(id).and_then(|h| match &h.apps {
                        hosts::Apps::Loaded(apps) => apps.iter().find(|known| known.id == app),
                        _ => None,
                    });
                    let playing_on = read.as_ref().and_then(|read| read.playing_on.as_ref());
                    let target = ui::join_target(known, app, playing_on);
                    let (asked, asked_resume) = ui::asked_start(known, app, resume, playing_on);
                    match select_step(resume, too, playing, press_failed) {
                        SelectStep::Ask(name, device) => {
                            self.launcher.ask_after(Dialog::ContinueHere { host: id, app: asked, resume: asked_resume, name, device })
                        }
                        SelectStep::Join => self.launch(id, target, true, LaunchKind::JoinRunning, LaunchFrom::Launcher),
                        SelectStep::Start => self.launch(id, app, resume, LaunchKind::Stream, LaunchFrom::Launcher),
                    }
                }
                HostEvent::StreamMoved { id, app, device: Some(device) } if self.streaming.is_none() && self.connecting.is_none() => {
                    eprintln!("nightjar: the stream moved to {device}");
                    self.show_moved(id, app, device);
                }
                HostEvent::MoveWatched { id, app, token, read } => {
                    if self.move_watch.answered(token) {
                        match read {
                            // Still moved, as far as the PC says.
                            Ok(read) if read.moved_to.is_some() => {}
                            Ok(read) => {
                                self.move_watch.done();
                                if nightjar_policy::continue_here::came_back(false, read.owns_game) {
                                    eprintln!("nightjar: the move failed; the game is this device's again");
                                    self.launcher.game_came_back(id, app);
                                } else if read.press_failed {
                                    eprintln!("nightjar: the move failed; the game stayed with another device");
                                    self.launcher.move_failed(id, app);
                                }
                            }
                            Err(e) => eprintln!("nightjar: could not ask the PC whether the move failed: {e}"),
                        }
                        self.watch_moved_prompt();
                    }
                }
                HostEvent::Action { result: Err(e), .. } => self.launcher.message(tr("The PC action failed"), e),
                _ => {}
            }
        }
        // Box art for the library on screen.
        if let Screen::Apps(id) = self.launcher.screen {
            let ids: Vec<i32> = match self.hosts.get(id).map(|h| &h.apps) {
                Some(Apps::Loaded(apps)) => apps.iter().map(|a| a.id).collect(),
                _ => Vec::new(),
            };
            for app in ids {
                self.hosts.request_art(id, app, "", &self.store);
            }
        }
        // A launch without a network query in flight is waiting for its app list (see
        // `launch_ready`); it goes on once the list is no longer loading.
        if self.network_query.is_none() {
            if let Some((host, app, resume, kind)) = self.network_launch {
                if !matches!(self.hosts.get(host).map(|h| &h.apps), Some(Apps::Loading)) {
                    self.network_launch = None;
                    self.launch_ready(host, app, resume, kind);
                }
            }
        }
        self.wake_step();
        self.replace_step();
        self.autostream_step();
        self.cli_step();
        self.second_screen_step();
        if let Some(c) = &mut self.connecting {
            match c.poll() {
                Ok(Some((session, facts))) => {
                    let connecting = self.connecting.take().expect("connecting");
                    if self.cancelled {
                        session.stop();
                        self.cancelled = false;
                    } else {
                        self.begin_stream(connecting, session, facts);
                        return;
                    }
                }
                Ok(None) => {}
                Err(failed) => {
                    self.connecting = None;
                    if !std::mem::take(&mut self.cancelled) {
                        match failed {
                            ConnectFailed::Message(e) => self.launcher.message(tr("The stream could not start"), e),
                            ConnectFailed::ConfirmReplace(plan) => {
                                if !self.open_play_game_running() {
                                    self.ask_replace(plan);
                                }
                            }
                            ConnectFailed::Moved { host, app, device } => self.show_moved(host, app, device),
                        }
                    }
                }
            }
        }

        let connecting = self.connecting.as_ref().map(|c| ConnectingView {
            app: c.app_name.clone(),
            host: c.host_name.clone(),
            stage: c.stage.clone(),
        }).or_else(|| self.network_launch.map(|(host, ..)| ConnectingView {
            app: tr("Starting stream").into(),
            host: self.hosts.get(host).map_or(String::new(), |h| h.display_name().to_owned()),
            stage: tr("Checking network profile").into(),
        }));
        let waking = self.waking.host().and_then(|id| self.hosts.get(id)).map(|h| h.display_name().to_owned());
        // Outside the frame: egui can run it twice, and the answer comes only once.
        if let Some(part) = self.updates.step() {
            self.save_for_next_run();
            if self.updates.hand_over(&part) {
                self.running = false;
            } else if let Some(pin) = self.launcher.pin.as_mut() {
                // Nightjar stays open after all, so it wasn't left.
                pin.returned();
            }
        }
        #[cfg(feature = "demo-library")]
        let demo_scrolling = self.demo_scrolling();
        let mut commands = Vec::new();
        let launcher = &mut self.launcher;
        let hosts = &self.hosts;
        let pads = &self.pads;
        let updates = &mut self.updates;
        self.gui.set_readable(true);
        self.gui.set_over_video(false);
        // The stream menu's demo pages stand in for a stream.
        #[cfg(feature = "demo-library")]
        if launcher.demo_menu.is_some() {
            self.gui.set_over_video(true);
        }
        let output = self.gui.run(self.window, self.renderer, |ui| {
            #[cfg(feature = "demo-library")]
            if demo_scrolling {
                ui.ctx().request_repaint();
            }
            updates.show(ui.ctx());
            commands = launcher.ui(
                ui,
                View {
                    hosts,
                    pads,
                    connecting: connecting.as_ref().map(|c| ConnectingView {
                        app: c.app.clone(),
                        host: c.host.clone(),
                        stage: c.stage.clone(),
                    }),
                    waking: waking.clone(),
                },
            );
        });
        self.gui.platform(self.window, &output.platform_output);
        if self.title_bar != *theme::NIGHT {
            self.title_bar = *theme::NIGHT;
            sdl::blend_title_bar(self.window, self.title_bar, *theme::ON_SURFACE);
        }
        let renderer = self.renderer;
        let must_draw = std::mem::take(&mut self.window_changed) || self.screenshot_requested;
        // A screenshot or capture reads the frame back, and so does live glass's timed chain: the
        // frame stays in the GPU renderer's back buffer.
        #[cfg(feature = "demo-library")]
        let read_back = self.screenshot_requested || self.demo_capture_pending() || self.gui.glass_timing();
        #[cfg(not(feature = "demo-library"))]
        let read_back = self.screenshot_requested || self.gui.glass_timing();
        sdl::read_back_frames(renderer, read_back);
        // SAFETY: drawing on the live renderer.
        let drew = self.gui.paint_if_changed(renderer, output, must_draw, || unsafe {
            SDL_SetRenderDrawColor(renderer, 0x14, 0x16, 0x1F, 255);
            SDL_RenderClear(renderer);
        });
        if drew {
            #[cfg(feature = "demo-library")]
            self.demo_drew();
            self.take_screenshot("launcher");
            // SAFETY: a live renderer.
            unsafe { SDL_RenderPresent(self.renderer) };
            static FIRST_FRAME_SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if let Some(line) = first_frame_line(&FIRST_FRAME_SAID, LAUNCH.get().map(|at| at.elapsed())) {
                eprintln!("{line}");
            }
        }
        for command in commands {
            self.command(command);
        }
    }

    fn take_screenshot(&mut self, label: &str) {
        if !std::mem::take(&mut self.screenshot_requested) {
            return;
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        let path = self.screenshot_dir.join(format!("nightjar-{label}-{stamp}.png"));
        match sdl::screenshot(self.renderer, &path) {
            Ok(()) => eprintln!("nightjar: screenshot {}", path.display()),
            Err(e) => eprintln!("nightjar: screenshot failed: {e}"),
        }
    }

    fn command(&mut self, command: Command) {
        match command {
            Command::PinPassed => match self.pin_pending.take() {
                Some(PinPending::Launch { host, app, resume, kind, from }) => self.launch(host, app, resume, kind, from),
                Some(PinPending::QuitAndLaunch { host, app, named }) => self.command(Command::QuitAndLaunch { host, app, named }),
                Some(PinPending::ContinueHere { host, app }) => self.hosts.continue_here(host, app),
                None => {}
            },
            Command::PinCancelled => {
                eprintln!("nightjar: stream start cancelled at the PIN");
                if let Some(PinPending::Launch { from: LaunchFrom::CommandLine, .. }) = self.pin_pending.take() {
                    if let Some(cli) = self.cli.as_mut() { cli.cancel(); }
                }
            }
            Command::Quit => self.running = false,
            Command::OpenOnStart(game) => self.open_play_choose(game),
            Command::SetSignIn(on) => self.sign_in_set(on),
            Command::Power(offer) => {
                // One at a time: a second confirm while logind waits on its password is dropped.
                if self.power_request.is_none() {
                    eprintln!("nightjar: {:?} this PC", offer.action);
                    self.power_request = Some((offer, power::request(offer, self.waker.clone())));
                }
            }
            Command::Launch { host, app, resume } => self.read_then_start(host, app, resume, false),
            Command::LaunchUnasked { host, app, resume } => self.launch(host, app, resume, LaunchKind::Stream, LaunchFrom::Launcher),
            Command::StreamTooAfterFailedMove { host, app } => self.read_then_start(host, app, true, true),
            Command::StreamHereToo { host, app, resume } => self.launch(host, app, resume, LaunchKind::stream_here_too(resume), LaunchFrom::Launcher),
            // The press ends another device's stream: the PIN comes before it, not at the start after it.
            Command::ContinueHere { host, app } => {
                if !self.pin_holds(host, PinPending::ContinueHere { host, app }) {
                    self.hosts.continue_here(host, app);
                }
            }
            Command::LaunchInputOnly { host, app, resume } => self.launch(host, app, resume, LaunchKind::SmallInputOnly, LaunchFrom::Launcher),
            Command::MicOnly { host } => self.launch(host, MIC_ONLY_APP_ID, false, LaunchKind::MicOnly, LaunchFrom::Launcher),
            Command::ToggleFavourite(entry) => {
                config::toggle_favourite(&mut self.launcher.favourites, entry);
                if let Err(e) = self.store.save_favourites(&self.launcher.favourites) {
                    eprintln!("nightjar: could not save favourites: {e}");
                }
            }
            Command::SetHidden { games, hidden } => {
                if config::set_hidden(&mut self.launcher.hidden_games, games, hidden) {
                    if let Err(e) = self.store.save_hidden_games(&self.launcher.hidden_games) {
                        eprintln!("nightjar: could not save hidden games: {e}");
                    }
                }
            }
            Command::QuitApp(host) => self.hosts.quit_app(host),
            Command::SetQuitOnLeave { host, uuid, id, enabled } => {
                if let Some(h) = self.hosts.get_mut(host) {
                    h.record.set_quit_on_leave(&uuid, id, enabled);
                    self.save_hosts();
                }
            }
            Command::ControlAction { host, app } => self.hosts.control_action(host, app),
            Command::QuitAndLaunch { host, app, named } => {
                // Quitting the running game is part of this start: the PIN comes first.
                if self.pin_holds(host, PinPending::QuitAndLaunch { host, app, named }) {
                    return;
                }
                // The PC may run another app than the prompt named: look again before closing anything.
                let now = self.hosts.get(host).and_then(|h| h.running_app());
                match console::recheck_running(named, now) {
                    console::Running::Changed => self.launcher.message(
                        tr("The running app changed"),
                        tr("The PC runs another app now. Check the screen and try again."),
                    ),
                    console::Running::Gone => self.launch(host, app, false, LaunchKind::Stream, LaunchFrom::Replace),
                    console::Running::Same => {
                        self.pending_launch = Some((host, app));
                        self.hosts.quit_app(host);
                    }
                }
            }
            Command::ConfirmReplace => {
                if let Some(mut plan) = self.pending_replace.take() {
                    if self.connecting.is_some() || self.network_launch.is_some() {
                        eprintln!("nightjar: another launch is under way; not launching app {} again", plan.app.id);
                        return;
                    }
                    eprintln!("nightjar: launching app {} again, closing the running app as confirmed", plan.app.id);
                    plan.confirm_replace = true;
                    self.refresh_plan(&mut plan);
                    self.cancelled = false;
                    self.connecting = Some(Connecting::start(*plan, self.waker.clone()));
                }
            }
            Command::Pair(host) => {
                if let Some(why) = self.hosts.get(host).and_then(|h| h.pairing_blocked()) {
                    self.launcher.pair_failed(host, why, false);
                    return;
                }
                let pin = hosts::random_pin();
                self.launcher.dialog = Dialog::Pairing {
                    host,
                    pin: pin.clone(),
                };
                self.hosts.pair(host, pin, None);
            }
            Command::PairOtp { host, pin, passphrase } => {
                if let Some(why) = self.hosts.get(host).and_then(|h| h.pairing_blocked()) {
                    self.launcher.pair_failed(host, why, true);
                    return;
                }
                self.launcher.dialog = Dialog::Pairing { host, pin: String::new() };
                self.hosts.pair(host, pin, Some(passphrase));
            }
            Command::ApprovePairing { host, request, name, pin } => self.hosts.approve_pairing(host, request, name, pin),
            Command::DenyPairing { host, request, name } => self.hosts.deny_pairing(host, request, name),
            Command::Wake(host) => {
                if self.waking.start(host, Instant::now()) {
                    self.wake_step();
                }
            }
            Command::CancelWake => self.waking.end(),
            Command::SleepOffer { host, ask } => self.hosts.sleep_offer(host, ask),
            Command::Sleep { host, ask, token, at } => self.hosts.sleep(host, ask, token, at),
            Command::PcPowerOffer { host, ask, action } => self.hosts.pc_power_offer(host, ask, action),
            Command::PcPower { host, ask, action, token, at } => self.hosts.pc_power(host, ask, action, token, at),
            Command::InstallUpdate { host } => self.hosts.install_update(host),
            Command::Rename { host, alias } => rename_host(&mut self.hosts, &self.store, host, &alias),
            Command::PinAddress { host, address } => {
                if self.hosts.pin_address(host, address.as_deref()) {
                    save_hosts(&self.store, &self.hosts);
                    self.hosts.poll_one(host);
                }
            }
            Command::HostSettings(id) => {
                if let Some(host) = self.hosts.get(id) {
                    self.launcher.open_host_settings(id, host);
                }
            }
            Command::SaveHostSettings(id) => {
                if let Some(host) = self.hosts.get_mut(id) {
                    host.record.settings = (!self.launcher.edit_overrides.is_empty()).then(|| self.launcher.edit_overrides.clone());
                    self.save_hosts();
                }
            }
            Command::AppSettings { host: id, app } => {
                if let Some(host) = self.hosts.get(id) {
                    self.launcher.open_app_settings(id, host, app);
                }
            }
            Command::SaveAppSettings(id) => {
                if let (Some(host), Some(app)) = (self.hosts.get_mut(id), self.launcher.editing_app.as_ref()) {
                    host.record.save_app_settings(&app.uuid, app.id, self.launcher.edit_overrides.clone());
                    self.save_hosts();
                }
            }
            Command::ResetKeptSettings => {
                self.hosts.list.iter_mut().for_each(|host| host.record.clear_own_settings());
                self.save_hosts();
            }
            Command::ProfileSettings(id) => {
                if let Some(profile) = self.launcher.profiles.entries.iter().find(|p| p.id == id) {
                    self.launcher.inherited_settings = self.launcher.settings.clone();
                    self.launcher.edit_overrides = profile.settings.clone();
                    self.launcher.edit_settings = profile.settings.apply(&self.launcher.inherited_settings);
                    self.launcher.inherited_from = Vec::new();
                    self.launcher.edit_name = trf("{} settings", &[&profile.name]);
                    self.launcher.go(Screen::ProfileSettings(id));
                }
            }
            Command::SaveProfile(id) => {
                if let Some(profile) = self.launcher.profiles.entries.iter_mut().find(|p| p.id == id) {
                    profile.settings = self.launcher.edit_overrides.clone();
                    self.command(Command::SaveProfiles);
                }
            }
            Command::SaveProfiles => {
                if let Err(e) = self.store.save_profiles(&self.launcher.profiles) {
                    self.launcher.message(tr("Could not save profiles"), e.to_string());
                }
            }
            Command::NetworkTest { host: id, calibrate } => {
                if self.network_test.is_some() { return; }
                if self.launcher.screen == Screen::HostSettings(id) { self.command(Command::SaveHostSettings(id)); }
                let Some(host) = self.hosts.get(id) else { return; };
                let Some(identity) = self.hosts.identity() else { return; };
                let client = match hosts::client_for(identity, &host.record, host.connect_address()) {
                    Ok(client) => client,
                    Err(error) => { self.launcher.message(tr("Network test unavailable"), error); return; }
                };
                let settings = host.record.settings(&self.launcher.profiles.settings(&self.launcher.settings, self.launcher.network.kind(&self.launcher.profiles.home_wifi)));
                let (_, _, fps, bitrate) = stream::stream_mode(&settings, display_of(self.window));
                self.network_test = Some(network_test::Test::start(id, client, self.launcher.network.clone(), host.connect_address().to_owned(), (bitrate, fps), calibrate, self.waker.clone()));
                self.launcher.dialog = Dialog::NetworkTest { stage: if calibrate { tr("Preparing frame-shaped UDP probes") } else { tr("Receiving 32 MiB from the PC") }.into() };
            }
            Command::CancelNetworkTest => { self.network_test = None; self.launcher.dialog = Dialog::None; }
            Command::ClearPace(id) => {
                if let Some(host) = self.hosts.get_mut(id) { host.record.pace = None; self.save_hosts(); }
            }
            Command::ControllerSettings => {
                self.command(Command::SaveSettings);
                self.launcher.open(Screen::Controllers);
                if self.launcher.controller_key.is_empty() {
                    if let Some(pad) = self.pads.pads.first() {
                        self.command(Command::EditController { key: pad.key.clone(), name: pad.name.clone() });
                    }
                }
            }
            Command::EditController { key, name } => {
                self.command(Command::SaveControllerMapping);
                self.launcher.controller_mapping = self.pads.mappings.get(&key).cloned().unwrap_or_default();
                self.launcher.controller_mapping.name = name;
                self.launcher.controller_key = key;
            }
            Command::SaveControllerMapping => {
                if !self.launcher.controller_key.is_empty() {
                    let key = &self.launcher.controller_key;
                    let mapping = &self.launcher.controller_mapping;
                    for pad in &mut self.pads.pads {
                        if &pad.key == key { pad.mapping = mapping.clone(); }
                    }
                    self.pads.mappings.insert(key.clone(), mapping.clone());
                    if let Err(e) = self.store.save_controller_mappings(&self.pads.mappings) {
                        self.launcher.message(tr("Could not save controller mapping"), e.to_string());
                    }
                }
            }
            Command::Shortcuts => {
                self.command(Command::SaveSettings);
                self.launcher.open(Screen::Shortcuts);
            }
            Command::SaveShortcuts => {
                if let Err(e) = self.store.save_shortcuts(&self.launcher.shortcuts) {
                    self.launcher.message(tr("Could not save shortcuts"), e.to_string());
                }
            }
            Command::StreamHotkeys => {
                self.command(Command::SaveSettings);
                self.launcher.open(Screen::StreamHotkeys);
            }
            Command::SaveHotkeys => {
                if let Err(e) = self.store.save_hotkeys(&shortcuts::hotkey_bindings()) {
                    self.launcher.message(tr("Could not save shortcuts"), e.to_string());
                }
            }
            Command::ImportShortcuts | Command::ExportShortcuts => {
                if self.shortcut_file.is_none() {
                    let export = matches!(command, Command::ExportShortcuts);
                    self.shortcut_file = Some((export, sdl::choose_json_file(self.window, export)));
                }
            }
            Command::OpenLogs => {
                if let Err(e) = logfile::open_folder() {
                    self.launcher.message(tr("Could not open the logs folder"), e);
                }
            }
            Command::RemoteCheck(id) => {
                if let Some(host) = self.hosts.get(id) {
                    host_page::start_check(&mut self.launcher.remote, self.hosts.identity(), host, self.waker.clone());
                }
            }
            Command::SharedFolders(id) => {
                if let Some(host) = self.hosts.get(id) {
                    folders_page::start(&mut self.launcher.folders, self.hosts.identity(), host, self.waker.clone());
                }
            }
            Command::FolderDownload { host, name, size } => {
                let window = self.window;
                start_folder_pick(&mut self.folder_pick, &mut self.launcher.folders, host, |at| FolderPick::Download { host, at, name, size }, || sdl::choose_path(window, true));
            }
            Command::FolderUpload(host) => {
                let window = self.window;
                start_folder_pick(&mut self.folder_pick, &mut self.launcher.folders, host, |at| FolderPick::Upload { host, at }, || sdl::choose_path(window, false));
            }
            Command::Coop(id) => {
                if let Some(host) = self.hosts.get(id) {
                    coop_page::start(&mut self.launcher.coop, self.hosts.identity(), host, self.waker.clone());
                }
            }
            Command::OpenFiles(id) => {
                if let Some(host) = self.hosts.get(id) {
                    ui::files::open(&mut self.launcher, self.hosts.identity(), host, self.waker.clone());
                }
            }
            Command::ChooseFiles => {
                let window = self.window;
                self.launcher.files.choose(|done| sdl::choose_files(window, done));
            }
            Command::OpenWebUi(url) => {
                if let Some((title, body)) = host_page::open_web_ui(&url, sdl::open_url) {
                    self.launcher.message(title, body);
                }
            }
            Command::OpenGuide(page) => {
                if let Some((title, body)) = host_page::open_guide(page, sdl::open_url) {
                    self.launcher.message(title, body);
                }
            }
            Command::CancelPair => self.hosts.cancel_pairing(),
            Command::AddHost(text) => self.add_entry(&text),
            Command::PairLink(link) => self.pair_link(*link),
            Command::AddFromClipboard => match gui::clipboard_text() {
                Some(text) if rubylight_protocol::pairing_uri::PairingUri::is_link(text.trim()) => self.add_entry(&text),
                _ => self.launcher.message(tr("No pairing link to paste"), tr(hosts::CLIPBOARD_NO_LINK)),
            },
            Command::Unpair(host) => {
                // The library and the certificate go when the PC has answered (see the event).
                if let Err(e) = self.hosts.unpair(host) {
                    self.launcher.message(tr("Could not unpair"), e);
                }
            }
            Command::Remove(host) => {
                remove_host(&mut self.hosts, &self.store, &mut self.launcher.recents, &mut self.launcher.favourites, &mut self.launcher.open_game, host);
            }
            Command::Refresh(host) => {
                self.hosts.poll_one(host);
                match self.launcher.screen {
                    Screen::Apps(id) if id == host => self.hosts.load_apps(host),
                    // Play needs the classic app list; a failed one is read again.
                    Screen::Game { host: id, .. }
                        if id == host && !matches!(self.hosts.get(host).map(|h| &h.apps), Some(Apps::Loaded(_))) =>
                    {
                        self.hosts.load_apps(host);
                    }
                    _ => {}
                }
            }
            Command::ReadAppsAgain(host) => {
                self.hosts.poll_one(host);
                self.hosts.load_apps(host);
            }
            Command::RequestArt { host, app, version } => self.hosts.request_art(host, app, &version, &self.store),
            Command::CloseGame(host) => self.hosts.session_action(host, ActionId::CloseGame),
            Command::LoadApps(host) => {
                if matches!(self.hosts.get(host).map(|h| &h.apps), Some(Apps::NotLoaded)) {
                    self.hosts.load_apps(host);
                }
            }
            Command::OpenApps(host) => {
                if !matches!(self.hosts.get(host).map(|h| &h.apps), Some(Apps::Loaded(_))) {
                    self.hosts.load_apps(host);
                }
                self.launcher.open(Screen::Apps(host));
            }
            Command::SaveSettings => {
                let s = &self.launcher.settings;
                self.pads.configure(s);
                if let Err(e) = self.store.save_settings(s) {
                    eprintln!("nightjar: saving settings failed: {e}");
                }
            }
            Command::CancelConnect => {
                if let Some(cli) = self.cli.as_mut() { cli.cancel(); }
                self.network_launch = None;
                if let Some(c) = &self.connecting {
                    c.cancel();
                    self.cancelled = true;
                }
            }
        }
    }

    /// Why the GPU renderer `need` asks for is missing, when a failed switch or the Renderer setting
    /// at OpenGL is why; None otherwise (Windows, or the GPU renderer is there).
    fn gpu_missing(&self, need: &'static str) -> Option<String> {
        if let Some(why) = &self.gpu_failure {
            return Some(trf("The GPU renderer could not be made: {}.", &[why]));
        }
        (cfg!(unix) && self.launcher.settings.renderer == config::Renderer::OpenGl && sdl::renderer_name(self.renderer) != "gpu")
            .then(|| trf("{} needs the GPU renderer. Set Renderer to Automatic or GPU (Vulkan).", &[&tr(config::gpu_need_label(need))]))
    }

    /// Before a stream's checks on Linux: switches to the GPU renderer when the stream needs it
    /// (`handheld::renderer_switch`). A failure keeps a renderer of the old kind and leaves why in
    /// `gpu_failure`, which the check that refuses the stream then says.
    #[cfg(unix)]
    fn switch_for_stream(&mut self, settings: &config::Settings) {
        self.apply_renderer_choice();
        let gpu = sdl::renderer_name(self.renderer) == "gpu";
        let state = self.renderer_state.clone();
        // A GPU renderer that failed this run: the checks that refuse the stream say why.
        self.gpu_failure.clone_from(&state.gpu_failed);
        let now = handheld::RendererNow {
            gpu,
            hdr_swapchain: sdl::swapchain_takes_hdr(self.renderer),
            made_for_hdr: state.made_for_hdr,
            gpu_failed: state.gpu_failed.is_some(),
            pq_unused: gpu && handheld::pq_unused(handheld::gamescope(), sdl::output_decision(), sdl::hdr10_offered(self.renderer), sdl::pq_failed()),
            hdr_unreachable: gpu && sdl::pq_failed() && !sdl::scrgb_offered(self.renderer),
        };
        let vulkan_choice = vulkan_video::Choice::from_environment();
        let vulkan_video = state.vulkan_video || vulkan_video_possible(settings.decoder != DecoderPreference::Software, vulkan_choice, vaapi_map_switched_on());
        // A stream decoding in hardware where every saved one decoded in software: Vulkan video
        // may need the GPU renderer, as at start.
        let need = settings.gpu_need().or_else(|| {
            (!gpu && !state.vulkan_video && vulkan_video && state.choice == config::Renderer::Auto && gpu_for_vulkan_video(vulkan_video, vulkan_choice))
                .then_some("Vulkan video")
        });
        let display_hdr = settings.hdr && handheld::hdr_display(handheld::gamescope(), sdl::any_display_hdr());
        // The quit-and-start prompt's plan holds the renderer's device.
        let busy = self.streaming.is_some() || self.connecting.is_some() || self.pending_replace.is_some();
        let Some(switch) = handheld::renderer_switch(state.choice, now, need, settings.hdr, display_hdr, busy) else { return };
        if let Err(why) = self.rebuild_renderer(true, settings.hdr, display_hdr, vulkan_video, switch.reason()) {
            self.renderer_state.gpu_failed = Some(why.clone());
            self.gpu_failure = Some(why);
        }
    }

    /// A changed Renderer setting, applied once with nothing streaming (`handheld::renderer_for_choice`).
    #[cfg(unix)]
    fn apply_renderer_choice(&mut self) {
        let choice = self.launcher.settings.renderer;
        if choice == self.renderer_state.choice || self.streaming.is_some() || self.connecting.is_some() || self.pending_replace.is_some() {
            return;
        }
        self.renderer_state.choice = choice;
        let Some(gpu) = handheld::renderer_for_choice(choice, sdl::renderer_name(self.renderer) == "gpu") else { return };
        match self.rebuild_renderer(gpu, false, false, self.renderer_state.vulkan_video, "the Renderer setting") {
            Ok(()) if gpu => self.renderer_state.gpu_failed = None,
            Ok(()) => {}
            Err(why) => {
                if gpu {
                    self.renderer_state.gpu_failed = Some(why.clone());
                }
                self.launcher.message(tr("Renderer"), trf("The renderer could not be made: {}.", &[&why]));
            }
        }
    }

    /// Makes Linux's renderer again on the same window, as the GPU renderer (`gpu`) or OpenGL, with
    /// nothing streaming or launching on it: what the GUI made on the old one goes first and comes
    /// back on the new one, and what run() took from the renderer is taken again. `hdr`: for an HDR
    /// stream (`display_hdr`: a display has HDR on). When the renderer asked for cannot be made, one
    /// of the old kind is (the GPU renderer once more without Vulkan video's options, never OpenGL in
    /// its place), and the error says why; with none at all Nightjar ends, loudly.
    #[cfg(unix)]
    fn rebuild_renderer(&mut self, gpu: bool, hdr: bool, display_hdr: bool, vulkan_video: bool, reason: &str) -> Result<(), String> {
        let from = sdl::renderer_name(self.renderer);
        let wanted = if gpu { "gpu" } else { "opengl" };
        // Waits for a settings probe still on the old device, and forgets what it found.
        stream::pyrowave_mode::settings_probe_reset();
        self.gui.release_renderer();
        // SAFETY: the renderer run() made, its textures freed, with no stream or launch using it.
        unsafe { SDL_DestroyRenderer(self.renderer) };
        if hdr {
            sdl::set_hdr_asked(true);
        }
        // Only an HDR stream's renderer may present as HDR10 PQ; the launcher and SDR streams keep
        // the window target.
        sdl::set_hdr_stream(hdr && display_hdr);
        let mut made = sdl::create_renderer_with(self.window, gpu, vulkan_video);
        let mut vulkan_video = vulkan_video;
        let mut failure = if made.is_null() {
            Some(sdl::error())
        } else if gpu && sdl::renderer_name(made) != "gpu" {
            Some(sdl::gpu_renderer_failure().unwrap_or_else(|| format!("SDL made {:?} instead", sdl::renderer_name(made))))
        } else {
            None
        };
        if failure.is_some() && gpu && from == "gpu" {
            // SDL's own fallback is OpenGL: from the GPU renderer, the GPU renderer once more, plainly.
            if !made.is_null() {
                // SAFETY: the renderer just made, nothing on it yet.
                unsafe { SDL_DestroyRenderer(made) };
            }
            made = sdl::create_renderer_with(self.window, true, false);
            if !made.is_null() && sdl::renderer_name(made) == "gpu" {
                eprintln!("nightjar: the GPU renderer for {reason} is made without Vulkan video: {}", failure.take().unwrap_or_default());
                vulkan_video = false;
            }
        }
        if made.is_null() {
            made = sdl::create_renderer_with(self.window, from == "gpu", self.renderer_state.vulkan_video);
        }
        if made.is_null() && from == "gpu" {
            made = sdl::create_renderer_with(self.window, false, false);
        }
        sdl::set_hdr_stream(false);
        if made.is_null() {
            let why = format!("Nightjar has no renderer any more: the {wanted:?} renderer for {reason} and the {from:?} one before it could not be made ({})", sdl::error());
            eprintln!("nightjar: {why}");
            message_box(SDL_MESSAGEBOX_ERROR, &why);
            logpipe::shutdown();
            std::process::exit(1);
        }
        // SAFETY: the renderer just made.
        unsafe { SDL_SetRenderVSync(made, 1) };
        self.renderer = made;
        self.gui.renderer_changed();
        self.hosts.renderer_changed();
        #[cfg(feature = "demo-library")]
        ui::demo::covers(&mut self.hosts, &self.gui.ctx);
        self.launcher.renderer_changed(&self.gui.ctx);
        self.launcher.gpu_device = renderer_gpu_device(made);
        self.color_probed = None;
        self.base_probed = None;
        self.window_changed = true;
        let to = sdl::renderer_name(made);
        self.renderer_state.made_for_hdr = to == "gpu" && hdr && display_hdr;
        if failure.is_none() {
            self.renderer_state.vulkan_video = vulkan_video;
        }
        match failure {
            None => {
                eprintln!("nightjar: renderer switched from {from:?} to {to:?} for {reason}");
                Ok(())
            }
            Some(why) => {
                eprintln!("nightjar: renderer {from:?} could not switch to {wanted:?} for {reason} ({why}); drawing with {to:?}");
                Err(why)
            }
        }
    }

    /// The renderer's device, for PyroWave decoding on the GPU.
    fn gpu_device(&self) -> Option<stream::pipeline::GpuDevice> {
        renderer_gpu_device(self.renderer)
    }

    fn hardware(&self, settings: &config::Settings) -> Hardware {
        if settings.decoder == DecoderPreference::Software {
            return Hardware::None;
        }
        if cfg!(windows) {
            if sdl::renderer_name(self.renderer) == "direct3d11" {
                let device = sdl::renderer_pointer(self.renderer, SDL_PROP_RENDERER_D3D11_DEVICE_POINTER);
                if !device.is_null() {
                    return Hardware::D3d11(device);
                }
            }
            Hardware::None
        } else {
            let gpu = renderer_gpu_device(self.renderer).map_or(std::ptr::null_mut(), |device| device.0);
            Hardware::Auto(gpu, if gpu.is_null() { self.display_vaapi_node() } else { None })
        }
    }

    /// The render node VAAPI opens on for the OpenGL renderer: the GPU its EGL display runs on,
    /// when that is not the default device (`dmabuf::renderer_vaapi_node`).
    fn display_vaapi_node(&self) -> Option<u32> {
        #[cfg(unix)]
        {
            dmabuf::renderer_vaapi_node(self.renderer)
        }
        #[cfg(not(unix))]
        None
    }

    /// Whether VAAPI pictures go to the renderer as DMA-BUFs (`dmabuf`): its OpenGL renderer on
    /// EGL, unless `NIGHTJAR_DMABUF=0`.
    fn dmabuf_import(&self) -> bool {
        #[cfg(unix)]
        {
            dmabuf::enabled_by_environment(std::env::var_os("NIGHTJAR_DMABUF").as_deref()) && dmabuf::renderer_imports(self.renderer)
        }
        #[cfg(not(unix))]
        false
    }

    /// Resends the wake packets while the PC stays away, and ends the wait when it answers or the
    /// time is up.
    fn wake_step(&mut self) {
        let Some(waking) = self.waking.current() else { return };
        let (id, run) = (waking.host, waking.run);
        // A PC removed meanwhile has nothing left to wake.
        let Some(host) = self.hosts.get(id) else {
            self.waking.end();
            return;
        };
        match waking.wait.next(host.reach == hosts::Reach::Online, Instant::now()) {
            wol::Step::Send => self.hosts.wake(id, run),
            wol::Step::Wait => {}
            wol::Step::Online => self.waking.end(),
            wol::Step::TimedOut => {
                let name = host.display_name().to_owned();
                self.waking.end();
                self.launcher.message(
                    tr("The PC did not wake up"),
                    trf(
                        "{} did not answer within {} seconds. Check that Wake-on-LAN is on in its firmware and its network adapter settings.",
                        &[&name, &wol::WakeWait::TIMEOUT.as_secs()],
                    ),
                );
            }
        }
    }

    /// Drives the unattended measurement start (see `autostream`); does nothing otherwise.
    fn autostream_step(&mut self) {
        let Some(auto) = self.autostream.as_mut() else { return };
        if auto.failed || self.connecting.is_some() || self.network_launch.is_some() {
            return;
        }
        if let Dialog::Message { title, body } = &self.launcher.dialog {
            let why = format!("{title}: {body}");
            auto.fail(&why);
            self.running = false;
            return;
        }
        let host = auto.host_seen(&self.hosts);
        let seen = autostream::Seen {
            identity: self.hosts.identity().is_some(),
            host,
            pair_error: matches!(self.launcher.dialog, Dialog::PairError { .. }),
        };
        match auto.next(&seen) {
            autostream::Step::Wait => {}
            autostream::Step::AddHost => {
                let id = self.hosts.add_manual(&auto.address, auto.port);
                auto.host_added(id);
            }
            autostream::Step::Pair(id) => {
                eprintln!("nightjar: autostream pairing, enter PIN {} on the PC", auto.pin);
                self.launcher.dialog = Dialog::Pairing { host: id, pin: auto.pin.clone() };
                self.hosts.pair(id, auto.pin.clone(), None);
            }
            autostream::Step::LoadApps(id) => self.hosts.load_apps(id),
            autostream::Step::Launch(host, app) => self.launch(host, app, false, LaunchKind::Stream, LaunchFrom::Autostream),
            autostream::Step::Fail(why) => {
                auto.fail(&why);
                self.running = false;
            }
        }
    }

    /// A plan made earlier, with this moment's display and controllers.
    fn refresh_plan(&mut self, plan: &mut LaunchPlan) {
        plan.display = display_of(self.window);
        plan.gamepad_mask = if plan.settings.controllers { u32::from(self.pads.active_mask()) } else { 0 };
        plan.prepared_controllers = if plan.settings.controllers { self.pads.prepare(plan.settings.controller_motion) } else { String::new() };
    }

    /// The PC refused `plan` until the user confirms closing the app it runs: ask.
    fn ask_replace(&mut self, plan: Box<LaunchPlan>) {
        eprintln!("nightjar: the PC asks to confirm closing its running app for app {}", plan.app.id);
        let running = self.hosts.get(plan.host_id).map_or_else(|| tr("the running app").into(), ui::running_name);
        self.launcher.ask(Dialog::ConfirmReplace { app: plan.app.name.clone(), running });
        self.pending_replace = Some(plan);
    }

    /// A replace prompt closed without the answer (Cancel, Escape, or another dialog over it)
    /// cancels the launch.
    fn replace_step(&mut self) {
        if self.pending_replace.is_some() && !matches!(self.launcher.dialog, Dialog::ConfirmReplace { .. }) {
            self.pending_replace = None;
            eprintln!("nightjar: closing the running app was not confirmed; the launch is cancelled");
            if let Some(cli) = self.cli.as_mut() { cli.cancel(); }
        }
    }

    /// Drives a `--host` start from the command line (see `cli`); does nothing otherwise.
    fn cli_step(&mut self) {
        let Some(cli) = self.cli.as_mut() else { return };
        if cli.failed.is_some() {
            return;
        }
        let message = match &self.launcher.dialog {
            Dialog::Message { title, body } => Some(format!("{title}: {body}")),
            _ => None,
        };
        // The stream and any reconnect are over: --quit-on-exit closes Nightjar.
        if self.streaming.is_none() && self.connecting.is_none() && cli.stream_ended(message.clone()) {
            self.running = false;
            return;
        }
        let seen = cli::Seen {
            identity: self.hosts.identity().is_some(),
            host: cli.host_seen(&self.hosts),
            message,
            busy: self.streaming.is_some() || self.connecting.is_some() || self.network_launch.is_some(),
            confirming: self.pending_replace.is_some() || self.pin_pending.is_some(),
        };
        match cli.next(&seen) {
            cli::Step::Wait => {}
            cli::Step::OpenApps(id) => self.command(Command::OpenApps(id)),
            cli::Step::LoadApps(id) => self.hosts.load_apps(id),
            cli::Step::Launch(host, app, resume) => {
                let kind = if cli.launch.join == Some(app) { LaunchKind::JoinRunning } else { LaunchKind::Stream };
                self.launch(host, app, resume, kind, LaunchFrom::CommandLine)
            }
            cli::Step::Fail(why) => {
                cli.fail(&why);
                self.running = false;
            }
            cli::Step::Close => {
                eprintln!("nightjar: --host start cancelled");
                self.running = false;
            }
        }
    }

    /// Who plays is read now, not taken from the last poll. One at a time: a second tap while the
    /// PC answers changes nothing.
    fn read_then_start(&mut self, host: u64, app: i32, resume: bool, too: bool) {
        if !self.hosts.offers_continue_here(host) {
            self.launch(host, app, resume, LaunchKind::Stream, LaunchFrom::Launcher);
        } else if !self.select_read {
            self.select_read = self.hosts.read_before_start(host, app, resume, too);
            if !self.select_read {
                self.launch(host, app, resume, LaunchKind::Stream, LaunchFrom::Launcher);
            }
        }
    }

    fn launch(&mut self, host_id: u64, app_id: i32, resume: bool, kind: LaunchKind, from: LaunchFrom) {
        let busy = self.connecting.is_some() || self.network_launch.is_some();
        eprintln!("nightjar: {}{}", launch_line(from, app_id, resume, busy), kind.log_suffix());
        if busy {
            // A grant left for this start is not used by it.
            self.forget_pin_grant();
            return;
        }
        // The quit-and-start prompt's start was let through with its quit.
        if from != LaunchFrom::Replace && self.pin_holds(host_id, PinPending::Launch { host: host_id, app: app_id, resume, kind, from }) {
            return;
        }
        self.network_launch = Some((host_id, app_id, resume, kind));
        self.network_query = Some(network::query(self.waker.clone()));
    }

    fn forget_pin_grant(&mut self) {
        if let Some(pin) = self.launcher.pin.as_mut() {
            pin.forget_grant();
        }
    }

    /// "Now playing on ...", watched while it is open: a move that fails gives the game back to
    /// this device, which has a minute to come back before close-on-disconnect applies.
    fn show_moved(&mut self, host: u64, app: i32, device: String) {
        self.launcher.ask_after(Dialog::NowPlaying { host, app, device });
        self.move_watch.open();
        self.watch_moved_prompt();
    }

    /// The next read for the open prompt. None once it closed, or it has asked enough.
    fn watch_moved_prompt(&mut self) {
        let Some((host, app)) = self.launcher.moved_prompt() else {
            return;
        };
        if let Some(token) = self.move_watch.next() {
            if !self.hosts.watch_stream_move(host, app, token) {
                self.move_watch.not_started();
            }
        }
    }

    /// Whether the PIN lock holds a stream start on `host` back; it then asks for the PIN and
    /// keeps `pending` until the PIN screen answers (Command::PinPassed or PinCancelled).
    fn pin_holds(&mut self, host: u64, pending: PinPending) -> bool {
        let Some(pin) = self.launcher.pin.as_mut() else { return false };
        let (uuid, name) = self.hosts.get(host).map_or((String::new(), String::new()), |h| (h.record.unique_id.clone(), h.display_name().to_owned()));
        let key = pin_lock::stream_key(&uuid, host);
        // A press leaves a grant to the stream it starts, which uses it up.
        let needed = if matches!(pending, PinPending::ContinueHere { .. }) { pin.locks_stream(&key) } else { pin.needs_pin_for_stream(&key) };
        if !needed {
            return false;
        }
        eprintln!("nightjar: the PIN lock asks for the PIN before this start");
        pin.ask_for_stream(&key, &name);
        self.pin_pending = Some(pending);
        true
    }

    fn launch_ready(&mut self, host_id: u64, app_id: i32, resume: bool, kind: LaunchKind) {
        let small = kind == LaunchKind::SmallInputOnly;
        let mic_only = kind == LaunchKind::MicOnly;
        if self.connecting.is_some() {
            return;
        }
        let Some(host) = self.hosts.get(host_id) else {
            return;
        };
        // A microphone-only session names no app of the PC's list.
        let app = if mic_only { mic_only_app() } else { match launch_target(host, app_id, resume) {
            LaunchApp::Ready(app) => app,
            LaunchApp::Wait => {
                // The list is being read again (the first native read of a run reloads it).
                // Starting over once it lands keeps the launch instead of dropping it unseen.
                self.network_launch = Some((host_id, app_id, resume, kind));
                return;
            }
            LaunchApp::Gone => {
                // Nothing has read the list yet: read it now and go on when it lands.
                if read_list_for_launch(&mut self.hosts, host_id) {
                    self.network_launch = Some((host_id, app_id, resume, kind));
                } else {
                    self.launcher.message(tr("The stream could not start"), trf("The app list could not be loaded. {}", &[&tr("Creating this device's pairing key…")]));
                }
                return;
            }
            LaunchApp::Failed(why) => {
                self.launcher.message(tr("The stream could not start"), trf("The app list could not be loaded. {}", &[&why]));
                return;
            }
            LaunchApp::Missing => {
                self.launcher.message(tr("The stream could not start"), tr("The PC's app list has no entry for this game.").to_owned());
                return;
            }
        } };
        if app.is_control_action() {
            self.hosts.control_action(host_id, app_id);
            return;
        }
        let listed = matches!(&host.apps, Apps::Loaded(apps) if apps.contains(&app));
        let mut settings = host.record.settings(&self.launcher.profiles.settings(&self.launcher.settings, self.launcher.network.kind(&self.launcher.profiles.home_wifi)));
        if let Some(overrides) = host.record.app_settings(&app.uuid, app.id) { settings = overrides.apply(&settings); }
        if settings.codec.is_pyrowave() { settings.experimental_packed_444 = false; }
        if mic_only || app.role(small) == nightjar_core::http::AppRole::InputOnly {
            input_only_settings(&mut settings);
        }
        if small {
            small_stream(&mut settings);
        }
        if mic_only {
            mic_only_settings(&mut settings);
        }
        if second_screen::this_process() {
            second_screen::stream_settings(&mut settings);
        }
        // Before the renderer may be made again for this stream: a launch that cannot go out makes none.
        // The demo library has no identity: its launch goes as far as the checks.
        let identity = self.hosts.identity().cloned();
        if identity.is_none() && !cfg!(feature = "demo-library") {
            return;
        }
        let Some(info) = self.hosts.get(host_id).and_then(|host| host.info.clone()) else {
            self.launcher.message(tr("The PC is offline"), tr("Nightjar has not reached this PC yet."));
            return;
        };
        self.gpu_failure = None;
        // Before the renderer may be made again for this stream and before its decoder.
        stream::pyrowave_sdk::queue::use_async_compute(settings.pyrowave_async_compute);
        #[cfg(unix)]
        {
            self.switch_for_stream(&settings);
            // HDR and PyroWave say it in their own checks below, with what they found.
            if let Some(advice) = settings.gpu_need().filter(|&need| handheld::refused_without_gpu(need)).and_then(|need| self.gpu_missing(need)) {
                self.launcher.message(tr("The stream could not start"), advice);
                return;
            }
        }
        self.hdr_note = false;
        let hdr_output = sdl::hdr_available(self.renderer)
            || (settings.hdr && !handheld::gamescope() && sdl::await_window_hdr(self.renderer));
        match handheld::hdr_stream(settings.hdr, hdr_output, handheld::gamescope()) {
            handheld::HdrStream::Refuse => {
                let why = sdl::output_decision().and_then(Result::err).map_or("the compositor reports no HDR for this window", handheld::SdrReason::says);
                eprintln!("nightjar: HDR asked for, refused: {why}");
                let advice = match sdl::hdr_refusal(self.renderer) {
                    handheld::HdrRefusal::TurnHdrOn => tr("Turn HDR on for this display, then start the stream again.").to_owned(),
                    handheld::HdrRefusal::RendererSdr => self.gpu_missing(strings::en("HDR"))
                        .unwrap_or_else(|| trf("This display has HDR on, but the renderer cannot present HDR: {}.", &[&why])),
                    handheld::HdrRefusal::MoveWindow => tr("Move this window to the display with HDR on, then start the stream again.").to_owned(),
                };
                self.launcher.message(tr("HDR output unavailable"), advice);
                return;
            }
            handheld::HdrStream::SdrWithNote => {
                let why = sdl::output_decision().and_then(Result::err).map_or("no HDR output", handheld::SdrReason::says);
                eprintln!("nightjar: HDR asked for, streaming in SDR: {why}");
                settings.hdr = false;
                self.hdr_note = true;
            }
            handheld::HdrStream::Hdr | handheld::HdrStream::Sdr => {}
        }
        let mut gpu_decoder = None;
        if settings.codec.is_pyrowave() {
            let gpu_device = self.gpu_device();
            let probed = stream::pyrowave_mode::availability(
                settings.pyrowave_decoder,
                || stream::pyrowave_gpu::probe(self.renderer, settings.hdr),
                || match gpu_device {
                    Some(device) => stream::pyrowave_mode::gpu_probe(device.0, settings.hdr),
                    None => Err(if cfg!(windows) { "PyroWave GPU decoding needs the Direct3D 11 renderer" } else { "PyroWave GPU decoding needs the SDL GPU renderer" }.to_owned()),
                },
            );
            match (probed, settings.pyrowave_decoder) {
                (Ok(()), config::PyroWaveDecoder::Gpu) => gpu_decoder = gpu_device,
                (Ok(()), config::PyroWaveDecoder::Cpu) => {}
                (Err(reason), config::PyroWaveDecoder::Cpu) => {
                    let advice = self.gpu_missing("PyroWave").map_or_else(|| format!("{reason}."), |why| format!("{reason}. {why}"));
                    self.launcher.message(tr("PyroWave output unavailable"), advice);
                    return;
                }
                (Err(reason), config::PyroWaveDecoder::Gpu) => {
                    // The CPU decoder draws with the GPU renderer too: without it, switching would not help.
                    let advice = match self.gpu_missing("PyroWave") {
                        Some(why) => format!("{reason}. {why}"),
                        None => trf("{}. Switch the PyroWave decoder to CPU in the settings, or pick another codec.", &[&reason]),
                    };
                    self.launcher.message(tr("PyroWave GPU decoding unavailable"), advice);
                    return;
                }
            }
        }
        // Before any decoder probe: the Vulkan video Debug settings change the backend order.
        media::use_vulkan_video_settings(settings.vulkan_video_first, settings.vulkan_rfi);
        let packed_modes = if settings.experimental_packed_444 {
            match stream::packed444::probe(self.renderer, self.hardware(&settings)) {
                Ok(modes) => modes,
                Err(reason) => {
                    self.launcher.message(tr("Full chroma unavailable"), reason);
                    return;
                }
            }
        } else {
            0
        };
        // The packed picture is twice the stream's height: past the decoder's largest, that mode goes.
        #[cfg(target_os = "linux")]
        let packed_modes = {
            let (modes, note) = linux_diagnosis::packed_offer(self.hardware(&settings), packed_modes, settings.width, settings.height);
            self.packed_note = note;
            modes
        };
        let color_decodable = if stream::probes_color(&settings) {
            let (hardware, renderer) = (self.hardware(&settings), self.renderer);
            let wanted = stream::color_probe_formats(&settings);
            probed_once(&mut self.color_probed, probe_key(hardware, wanted, &settings), || {
                stream::color_probe::probe(hardware, wanted, |ten_bit| stream::color_probe::planes_drawable(renderer, ten_bit))
            })
        } else {
            (0, String::new())
        };
        // Offers follow what the decoder decodes here, not what it is said to.
        let base_decodable = match (self.hardware(&settings), stream::base_probe_formats(&settings)) {
            (Hardware::None, _) => stream::color_probe::Base::software(),
            (hardware, wanted) => {
                let key = probe_key(hardware, wanted, &settings);
                probed_once(&mut self.base_probed, key, || stream::color_probe::probe_base(hardware, wanted))
            }
        };
        let (Some(identity), Some(host)) = (identity, self.hosts.get(host_id)) else {
            return;
        };
        let display = display_of(self.window);
        log_display(self.window, display);
        self.launcher.display = display;
        let (pacing, vrr) = pacing(&settings, self.launcher.tearing_supported);
        let plan = LaunchPlan {
            host_id,
            record: host.connect_record(),
            host_name: host.display_name().to_owned(),
            info,
            native: host.native.capabilities.clone(),
            app,
            listed,
            resume,
            join: kind == LaunchKind::JoinRunning,
            small_stream_input_only: small,
            mic_only,
            settings: settings.clone(),
            identity,
            display,
            hardware: self.hardware(&settings),
            dmabuf: self.dmabuf_import(),
            gpu_decoder,
            packed_modes,
            color_decodable,
            base_decodable,
            pacing,
            vrr,
            gamepad_mask: if settings.controllers { u32::from(self.pads.active_mask()) } else { 0 },
            prepared_controllers: if settings.controllers { self.pads.prepare(settings.controller_motion) } else { String::new() },
            network: self.launcher.network.clone(),
            pace: host.record.pace.clone(),
            host_link_mbps: host.info.as_ref().map_or(0, |info| info.pyrowave_host_link_mbps),
            bitrate_limit_kbps: self.launcher.profiles.bitrate_limit(self.launcher.network.kind(&self.launcher.profiles.home_wifi)),
            microphone_muted: false,
            recovery: None,
            video_shown: false,
            confirm_replace: false,
            display_luminance: sdl::launch_luminance(self.renderer),
        };
        self.cancelled = false;
        self.connecting = Some(Connecting::start(plan, self.waker.clone()));
    }

    fn begin_stream(&mut self, connecting: Connecting, session: nightjar_core::session::Session, facts: stream::Facts) {
        if let Some(auto) = self.autostream.as_mut() { auto.streaming_began(); }
        if let Some(cli) = self.cli.as_mut() { cli.streaming_began(); }
        let settings = connecting.settings.clone();
        self.pads.configure(&settings);
        let display = display_of(self.window);
        let pacing = connecting.pacing;
        eprintln!(
            "nightjar: streaming {}x{}@{} {} at {} kbps, pacing {:?}",
            facts.width,
            facts.height,
            facts.fps,
            stream::codec_name(facts.format),
            facts.bitrate_kbps,
            pacing
        );
        // Without V-Sync the GPU renderer tears where it can (`prefers_immediate`); the property
        // holds for the stream, so the VRR pacing's later interval changes follow it too.
        #[cfg(target_os = "linux")]
        sdl::prefer_immediate(self.renderer, stream::present::prefers_immediate(connecting.vrr(), sdl::video_driver() == "wayland", handheld::gamescope()));
        // SAFETY: window and renderer calls on the main thread.
        unsafe {
            SDL_SetRenderVSync(self.renderer, if pacing == PacingMode::Vsync { 1 } else { 0 });
            #[cfg(target_os = "linux")]
            eprintln!("nightjar: present mode {}", sdl::present_mode_name(sdl::present_mode(self.renderer)));
            let mode = stream_window_mode(settings.window_mode, handheld::gamescope());
            match if facts.role == nightjar_core::http::AppRole::InputOnly { WindowMode::Windowed } else { mode } {
                WindowMode::Fullscreen => {
                    let mut mode: SDL_DisplayMode = std::mem::zeroed();
                    let id = SDL_GetDisplayForWindow(self.window);
                    if SDL_GetClosestFullscreenDisplayMode(
                        id,
                        facts.width,
                        facts.height,
                        fullscreen_refresh_hz(connecting.vrr(), &mode_refresh_rates(id), facts.fps),
                        true,
                        &mut mode,
                    ) {
                        SDL_SetWindowFullscreenMode(self.window, &mode);
                    } else {
                        SDL_SetWindowFullscreenMode(self.window, std::ptr::null());
                    }
                    SDL_SetWindowFullscreen(self.window, true);
                }
                WindowMode::Borderless => {
                    SDL_SetWindowFullscreenMode(self.window, std::ptr::null());
                    SDL_SetWindowFullscreen(self.window, true);
                }
                WindowMode::Windowed => {}
            }
            SDL_SyncWindow(self.window);
        }
        self.remember_recent(&connecting);
        let mut streaming = Streaming::new(connecting, session, facts, display);
        streaming.watch_microphone_busy(self.waker.clone());
        self.pads.prepare_audio(settings.controllers && settings.controller_audio && streaming.facts.role != nightjar_core::http::AppRole::RemoteMonitor);
        streaming.display_changed(self.renderer);
        if settings.adaptive_bitrate || streaming.quality_mode != config::QualityMode::Off { streaming.enable_adaptive_bitrate(true); }
        if settings.controllers && streaming.facts.role != nightjar_core::http::AppRole::RemoteMonitor {
            streaming.session.input(|q| self.pads.announce_all(q));
        }
        // SAFETY: a live window.
        let focused = unsafe { SDL_GetWindowFlags(self.window) & SDL_WINDOW_INPUT_FOCUS != 0 };
        if focused {
            streaming.set_capture(self.window, true);
        } else {
            streaming.capture_on_focus();
        }
        let (pyrowave, notices_changed) = match self.hosts.get_mut(streaming.host_id) {
            Some(host) => stream::pyrowave_notice(&mut host.record, settings.codec.is_pyrowave(),
                streaming.facts.pyrowave_fallback.as_deref(), streaming.facts.format),
            None => (None, false),
        };
        if notices_changed {
            self.save_hosts();
        }
        let hdr_note = std::mem::take(&mut self.hdr_note).then(|| tr(HDR_OFF_NOTE));
        #[cfg(target_os = "linux")]
        let decoder_notes = self.linux_stream_start(&settings, pacing == PacingMode::Vsync);
        #[cfg(not(target_os = "linux"))]
        let decoder_notes: Vec<String> = Vec::new();
        let packed_note = self.packed_note.take();
        let notices: Vec<&str> = hdr_note.into_iter().chain(packed_note.as_deref()).chain(decoder_notes.iter().map(String::as_str)).chain(pyrowave.as_deref())
            .chain(streaming.facts.codec_fallback.as_deref()).chain(streaming.facts.color_notice.as_deref()).collect();
        let notices = notices.join("\n");
        let network = &self.launcher.network;
        let profiles = &self.launcher.profiles;
        let selected = profiles.selected(network.kind(&profiles.home_wifi));
        let warning = if network.error.is_some() && (profiles.automatic || profiles.mobile_limit_kbps > 0) {
            Some(tr("Network details unavailable; automatic profiles and the mobile data limit could not be applied."))
        } else if selected.is_some_and(|id| !profiles.entries.iter().any(|p| p.id == id)) {
            Some(tr("The selected profile is missing; main settings and PC/game overrides apply."))
        } else if profiles.automatic && profiles.home.is_some() && !profiles.home_wifi.is_empty() && network.wifi && network.ssid.is_none() {
            Some(tr("Wi-Fi name unavailable; using the other Wi-Fi profile."))
        } else { None };
        let toast = start_toast(warning, (!notices.is_empty()).then_some(notices.as_str()), handheld::gamescope());
        streaming.show_toast(toast);
        self.streaming = Some((streaming, TimerResolution::begin()));
        self.start_second_screen(&settings);
    }

    /// The `linux:` line, the present mode for the `video path:` line, and the toasts naming what
    /// hardware decoding is missing (`linux_diagnosis`).
    #[cfg(target_os = "linux")]
    fn linux_stream_start(&self, settings: &config::Settings, vsync: bool) -> Vec<String> {
        let renderer = sdl::renderer_name(self.renderer);
        let device = sdl::renderer_pointer(self.renderer, SDL_PROP_RENDERER_GPU_DEVICE_POINTER);
        let present = if renderer == "gpu" && !device.is_null() {
            // SAFETY: The renderer's live GPU device and its window, on the main thread.
            let supports = |mode| unsafe { SDL_WindowSupportsGPUPresentMode(device.cast(), self.window, mode) };
            format!("present {}", linux_diagnosis::gpu_present_mode(vsync, supports(SDL_GPU_PRESENTMODE_MAILBOX), supports(SDL_GPU_PRESENTMODE_IMMEDIATE)))
        } else {
            format!("{renderer} renderer, V-Sync {}", if vsync { "on" } else { "off" })
        };
        linux_diagnosis::stream_start(self.hardware(settings), &present)
    }

    /// Dual screen mode: a stream of a game, with a second monitor and a PC that keeps a second
    /// stream beside the first, starts the second screen's process (`second_screen`).
    fn start_second_screen(&mut self, settings: &config::Settings) {
        if !settings.dual_screen || second_screen::this_process() || self.second_screen.is_some() {
            return;
        }
        let Some((streaming, _)) = &self.streaming else { return };
        if streaming.mic_only() || streaming.facts.role != nightjar_core::http::AppRole::Stream {
            return;
        }
        let Some(host) = self.hosts.get(streaming.host_id) else { return };
        let target = if !host.info.as_ref().is_some_and(|i| i.capabilities.has(nightjar_core::http::Feature::SecondScreen)) {
            Err("this PC does not keep a second stream beside the first".to_owned())
        } else {
            // SAFETY: a live window.
            let main = unsafe { SDL_GetDisplayForWindow(self.window) };
            let apps = match &host.apps { Apps::Loaded(apps) => apps.as_slice(), _ => &[] };
            // A joined game is another device's, which the PC does not report as running to this one.
            let joined = streaming.joined().then(|| streaming.app().id);
            let running = second_screen::joined_running(host.running_app().map(|id| (id, host.info.as_ref().and_then(|i| i.current_game_uuid.as_deref()))), joined);
            second_screen::pick_display(&window_place::display_bounds(), main)
                .ok_or_else(|| "no second monitor".to_owned())
                .and_then(|point| {
                    let (pc, game) = second_screen::target(&host.record, streaming.app(), apps, running)?;
                    // This stream got past the PIN lock: the second screen's does not ask again.
                    let hand_over = match self.launcher.pin.as_ref().map(pin_ui::PinLock::hand_over) {
                        Some(Ok(Some(token))) => token,
                        Some(Ok(None)) | None => String::new(),
                        Some(Err(e)) => {
                            eprintln!("nightjar: dual screen: the PIN lock cannot be handed to the second screen ({e}); it asks for the PIN there");
                            String::new()
                        }
                    };
                    second_screen::Companion::start(&pc, &game, point, joined, &hand_over)
                        .map(|companion| (companion, pc, game, point))
                        .map_err(|e| format!("could not start: {e}"))
                })
        };
        match target {
            Ok((companion, pc, game, point)) => {
                eprintln!("nightjar: dual screen: the second screen streams {game} from {pc} on the monitor at {},{}", point.0, point.1);
                self.second_screen = Some(companion);
            }
            Err(why) => {
                eprintln!("nightjar: dual screen: no second screen: {why}");
                if let Some((streaming, _)) = self.streaming.as_mut() {
                    streaming.show_toast(tr("The second screen could not be opened."));
                }
            }
        }
    }

    /// The second screen's process has ended or is ending: the PIN lock it was handed opens
    /// nothing any more.
    fn second_screen_gone(&self) {
        if let Some(pin) = &self.launcher.pin {
            pin.end_hand_over();
        }
    }

    /// The second screen ends with the first screen's stream, a reconnect aside, and a second
    /// screen that failed says so.
    fn second_screen_step(&mut self) {
        let Some(companion) = self.second_screen.as_mut() else { return };
        if self.streaming.is_none() && self.connecting.is_none() {
            if let Some(companion) = self.second_screen.take() {
                companion.end();
                self.second_screen_gone();
            }
            return;
        }
        let failed = match companion.exited() {
            None => return,
            Some(Ok(status)) if status.success() => {
                eprintln!("nightjar: dual screen: the second screen closed");
                false
            }
            Some(Ok(status)) => {
                eprintln!("nightjar: dual screen: the second screen failed ({status}); its log is in logs/second-screen");
                true
            }
            Some(Err(e)) => {
                eprintln!("nightjar: dual screen: the second screen cannot be watched: {e}");
                true
            }
        };
        self.second_screen = None;
        self.second_screen_gone();
        if failed {
            if let Some((streaming, _)) = self.streaming.as_mut() {
                streaming.show_toast(tr("The second screen could not be opened."));
            }
        }
    }

    /// Notes the game of a stream that just started, for Home's "Recently played".
    fn remember_recent(&mut self, connecting: &Connecting) {
        let Some(entry) = connecting.recent_game() else { return };
        config::remember(&mut self.launcher.recents, entry);
        if let Err(e) = self.store.save_recents(&self.launcher.recents) {
            eprintln!("nightjar: could not save recent games: {e}");
        }
    }

    /// Shows the message of a stream the PC failed once the poll after it answered, with the
    /// PC's reason, or after `REASON_WAIT` without it.
    fn show_pending_end(&mut self) {
        let Some(pending) = &self.pending_end else { return };
        let answered = self.hosts.failure_after(pending.host, pending.seq);
        if answered.is_none() && Instant::now() < pending.until {
            return;
        }
        let Some(pending) = self.pending_end.take() else { return };
        if let Some((title, body)) = stream::doctor::after_stream(pending.ended, pending.health) {
            // "Now playing on" may have come first; this stream's report waits for it.
            self.launcher.ask_after(Dialog::Message { title: title.into(), body: stream::doctor::with_host_reason(body, answered.flatten().as_deref()) });
        }
    }

    fn end_stream(&mut self, exit: Exit) {
        let Some((mut streaming, _timer)) = self.streaming.take() else {
            return;
        };
        let host = streaming.host_id;
        // The launcher didn't see the window move while the stream had it.
        self.gui.set_refresh(display_of(self.window).refresh_hz);
        self.open_play.left_by_player = matches!(exit, Exit::User | Exit::CloseWindow);
        // Microphone only has no video; it is going once connected.
        let reconnect = if matches!(exit, Exit::Reconnect) || exit.retryable(streaming.video_started() || streaming.mic_only()) {
            // SAFETY: the window lives as long as the app.
            let away = unsafe { SDL_GetWindowFlags(self.window) & (SDL_WINDOW_MINIMIZED | SDL_WINDOW_HIDDEN) != 0 };
            streaming.reconnect_plan(!matches!(exit, Exit::Reconnect), away)
        } else { None };
        // The stream doctor's report when it found something, unless the stream goes on.
        let health = streaming.health_summary().filter(|_| reconnect.is_none());
        eprintln!(
            "nightjar: stream ended ({})",
            match &exit {
                Exit::User => "left by the user".to_owned(),
                Exit::CloseWindow => "window closed".to_owned(),
                Exit::Reconnect => "reconnecting".to_owned(),
                Exit::NetworkReturned => "the network is back where the connection cannot carry on, reconnecting".to_owned(),
                Exit::Terminated(code) => format!("host or transport error {code}"),
                Exit::Ended(None) => "ended by the PC".to_owned(),
                Exit::Ended(Some(m)) => m.clone(),
            }
        );
        // Quit on leave: only a leave the player asked for quits the game; a stream that is still
        // up when it ends that way was connected. The second screen's leave never does: the
        // game is the first screen's too.
        if matches!(exit, Exit::User) && !second_screen::this_process() {
            let app = streaming.app();
            // Quit game confirmed in the stream menu, or the launch's own choice to quit on leaving.
            let chosen = streaming.quit_on_leave || self.hosts.get(host).is_some_and(|h| h.record.quits_on_leave(&app.uuid, app.id));
            if nightjar_policy::launch::quits_on_leave(chosen, ui::policy_role(streaming.facts.role), true) {
                eprintln!("nightjar: quitting {} on the PC, as chosen for leaving its stream", app.name);
                self.launcher.toast(trf("Quitting {} on the PC…", &[&app.name]));
                self.quitting_on_leave = Some((host, app.name.clone()));
                self.hosts.quit_app(host);
            }
        }
        let streaming_app = streaming.app().id;
        streaming.finish(self.window, &mut self.pads);
        // The stream's cursor shows the pointer again on leaving.
        self.pointer.shown();
        // SAFETY: window and renderer calls on the main thread.
        unsafe {
            if handheld::start_window(handheld::gamescope()).windowed_after_stream {
                SDL_SetWindowFullscreen(self.window, false);
            }
            SDL_SetRenderVSync(self.renderer, 1);
        }
        self.hosts.refresh_after_exit(host);
        // A reconnect asks the PC itself before it launches.
        if reconnect.is_none() && self.running {
            self.hosts.read_stream_move(host, streaming_app, matches!(exit, Exit::User | Exit::CloseWindow));
        }
        if let Some(mut plan) = reconnect {
            self.refresh_plan(&mut plan);
            self.cancelled = false;
            self.connecting = Some(Connecting::start(plan, self.waker.clone()));
        } else {
        // A failure the PC ended the stream on: the message waits for the poll just started,
        // which carries the PC's own words for it where it says them.
        let reason_poll = match exit {
            Exit::Terminated(code) if stream::host_ended_stream(code) => self.hosts.polling_seq(host),
            _ => None,
        };
        let ended = match exit {
            Exit::CloseWindow => { self.running = false; None }
            Exit::User | Exit::Reconnect | Exit::Ended(None) => None,
            Exit::Terminated(code) => stream::termination_text(code),
            // Not reconnecting after all (minimized since the network came back): the connection is lost.
            Exit::NetworkReturned => stream::termination_text(nightjar_core::session::error::FAILED),
            Exit::Ended(Some(message)) => Some(message),
        };
        match reason_poll {
            Some(seq) if self.running => {
                self.pending_end = Some(PendingEnd { host, seq, ended, health, until: Instant::now() + REASON_WAIT });
            }
            _ => {
                if let Some((title, body)) = stream::doctor::after_stream(ended, health).filter(|_| self.running) {
                    self.launcher.message(title, body);
                }
            }
        }
        }
        // egui forgets held modifiers from before the stream.
        self.gui.push(egui::Event::WindowFocused(true));
        self.launcher.refocus();
    }

    fn stream_frame(&mut self) {
        let window = self.window;
        let renderer = self.renderer;
        let display = display_of(window);
        // The stream menu animates at most once per refresh.
        let menu_frame = Duration::from_secs_f32(1.0 / display.refresh_hz.max(30.0));
        let Some((streaming, _)) = self.streaming.as_mut() else {
            return;
        };
        // SAFETY: the window lives as long as the app.
        let away = unsafe { SDL_GetWindowFlags(window) & (SDL_WINDOW_MINIMIZED | SDL_WINDOW_HIDDEN) != 0 };
        streaming.drain_notices(&mut self.pads, away);
        streaming.open_guide_menu(window, &mut self.pads);
        streaming.follow_text_field(window);
        #[cfg(windows)]
        streaming.follow_tab_chord(window);
        streaming.pump();
        streaming.follow_display(display);
        if streaming.export_statistics {
            streaming.save_statistics(&self.store.dir().join("statistics"));
        }
        // Wait for input, a decoded picture or the next present time.
        // SAFETY: event calls on the main thread.
        unsafe {
            let mut event: SDL_Event = std::mem::zeroed();
            match streaming.wait() {
                stream::Wait::Now => {}
                stream::Wait::For(duration) => {
                    // A held direction in the stream menu wakes the loop for its next step.
                    let duration = self.nav_repeat.next(Instant::now()).map_or(duration, |next| duration.min(next + Duration::from_millis(1)));
                    // An animating menu wakes it for egui's next frame, at most once per refresh.
                    let duration = match self.gui.repaint_in(menu_frame) {
                        Some(wait) if streaming.menu_visible => duration.min(wait),
                        _ => duration,
                    };
                    if duration > Duration::from_micros(1500) {
                        let ms = (duration.as_micros() / 1000).saturating_sub(1).max(1) as i32;
                        if SDL_WaitEventTimeout(&mut event, ms) {
                            self.dispatch_stream_event(&event);
                        }
                    } else {
                        SDL_DelayPrecise(duration.as_nanos() as u64);
                    }
                }
            }
            while SDL_PollEvent(&mut event) {
                self.dispatch_stream_event(&event);
            }
        }
        // A held D-pad or stick steps the stream menu again, as in the launcher.
        if self.streaming.as_ref().is_some_and(|(s, _)| s.menu_visible) {
            if self.repeat_navigation() {
                if let Some((s, _)) = self.streaming.as_mut() {
                    s.needs_redraw = true;
                }
            }
        } else {
            self.nav_repeat.clear();
        }
        let Some((streaming, _)) = self.streaming.as_mut() else {
            return;
        };
        // Without a new picture (a still desktop, a paused game) the menu draws when egui asks.
        if streaming.menu_visible && self.gui.repaint_in(menu_frame).is_some_and(|wait| wait.is_zero()) {
            streaming.needs_redraw = true;
        }
        #[cfg(windows)]
        streaming.pump_touchpad(window);
        streaming.pump();
        if let Some(host) = self.hosts.get(streaming.host_id) {
            streaming.hints = host.native.session_hints();
        }
        if streaming.session_page_requested {
            let link = self.launcher.network.description(&self.launcher.profiles.home_wifi);
            streaming.open_session_page(self.waker.clone(), link);
        }
        if streaming.update_overlay() && streaming.overlay_visible {
            streaming.needs_redraw = true;
        }
        if let Some((shown, mode, corner)) = streaming.take_overlay_change() {
            let s = &mut self.launcher.settings;
            (s.overlay, s.overlay_mode, s.overlay_position) = (shown, mode, corner);
            if let Err(e) = self.store.save_settings(s) {
                eprintln!("nightjar: saving settings failed: {e}");
            }
        }
        if let Some((_, at)) = &streaming.toast {
            // The lost-network notice stays until a network returns.
            if at.elapsed() > Duration::from_secs(4) && !streaming.network_lost {
                streaming.toast = None;
                streaming.needs_redraw = true;
            }
        }
        let gui = &mut self.gui;
        // The input filter sends controller input while the present waits (direct_motion.rs).
        let pads = streaming.direct_motion.lend(std::mem::take(&mut self.pads));
        let shortcuts = &mut self.launcher.shortcuts;
        let screenshot = std::mem::take(&mut self.screenshot_requested);
        sdl::read_back_frames(renderer, screenshot);
        let dir = self.screenshot_dir.clone();
        let mut shortcuts_changed = false;
        streaming.present(renderer, |s, renderer| {
            shortcuts_changed = draw_overlay(gui, window, renderer, s, &mut pads.pads(), shortcuts);
            if screenshot {
                let stamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis());
                let path = dir.join(format!("nightjar-stream-{stamp}.png"));
                match sdl::screenshot(renderer, &path) {
                    Ok(()) => eprintln!("nightjar: screenshot {}", path.display()),
                    Err(e) => eprintln!("nightjar: screenshot failed: {e}"),
                }
            }
        });
        self.pads = pads.take_back();
        // A shortcut saved or deleted in the stream menu lands in the same file as the settings' list.
        if shortcuts_changed {
            if let Err(e) = self.store.save_shortcuts(&self.launcher.shortcuts) {
                streaming.show_toast(trf("Could not save shortcuts: {}", &[&e]));
            }
        }
        // No picture for 12 s after the start means the stream never came up.
        if streaming.facts.role != nightjar_core::http::AppRole::InputOnly
            && !streaming.has_picture() && streaming.since_start() > Duration::from_secs(12) && streaming.exit.is_none() {
            streaming.exit = Some(Exit::Terminated(nightjar_core::session::error::NO_VIDEO_TRAFFIC));
        }
        if let Some(auto) = &self.autostream {
            if streaming.exit.is_none() && auto.ran_its_time() {
                streaming.exit = Some(Exit::User);
            }
        }
        if let Some(exit) = streaming.exit.take() {
            let planned = matches!(exit, Exit::User);
            self.end_stream(exit);
            // A dropped stream may reconnect (end_stream started it); a failed reconnect ends the
            // run through its message, in autostream_step.
            let reconnecting = self.connecting.is_some();
            if let Some(auto) = self.autostream.as_mut() {
                if !auto.stream_ended(planned, reconnecting) { self.running = false; }
            }
        }
    }

    fn dispatch_stream_event(&mut self, event: &SDL_Event) {
        self.dispatch_stream_event_inner(event);
        // A click or wheel turn has gone out (or the menu took it): later motion may pass it.
        if let Some((s, _)) = self.streaming.as_ref() {
            s.direct_motion.dispatched(event);
        }
    }

    fn dispatch_stream_event_inner(&mut self, event: &SDL_Event) {
        self.track_visibility(event);
        // SAFETY: reading the event type.
        let kind = SDL_EventType(unsafe { event.r#type });
        if kind.0 == self.wake_event {
            // The Session page's worker published something new.
            if let Some((s, _)) = self.streaming.as_mut() {
                if s.session_page.is_some() { s.needs_redraw = true; }
            }
            return;
        }
        // SAFETY: a key event's members.
        let key_action = (kind == SDL_EVENT_KEY_DOWN).then(|| unsafe { shortcuts::key_action(Scope::Stream, event.key.scancode, event.key.r#mod) }).flatten();
        // SAFETY: a key event's members.
        let repeat = kind == SDL_EVENT_KEY_DOWN && unsafe { event.key.repeat };
        if key_action == Some(Action::Screenshot) {
            self.screenshot_requested = true;
            if let Some((s, _)) = self.streaming.as_mut() {
                s.needs_redraw = true;
            }
            return;
        }
        log_window_hdr(event);
        if matches!(
            kind,
            SDL_EVENT_WINDOW_RESIZED
                | SDL_EVENT_WINDOW_PIXEL_SIZE_CHANGED
                | SDL_EVENT_WINDOW_EXPOSED
                | SDL_EVENT_WINDOW_HDR_STATE_CHANGED
                | SDL_EVENT_WINDOW_DISPLAY_CHANGED
        ) {
            if let Some((s, _)) = self.streaming.as_mut() {
                s.needs_redraw = true;
            }
        }
        let Some((streaming, _)) = self.streaming.as_mut() else {
            return;
        };
        if matches!(kind, SDL_EVENT_WINDOW_HDR_STATE_CHANGED | SDL_EVENT_WINDOW_DISPLAY_CHANGED | SDL_EVENT_DISPLAY_CURRENT_MODE_CHANGED) {
            streaming.display_changed(self.renderer);
        }
        if streaming.menu_visible {
            let back_button = if self.pads.swap_face_buttons { SDL_GAMEPAD_BUTTON_SOUTH } else { SDL_GAMEPAD_BUTTON_EAST };
            // SAFETY: union members read by event type.
            let back = (kind == SDL_EVENT_KEY_DOWN && unsafe { event.key.scancode } == SDL_SCANCODE_ESCAPE)
                || (kind == SDL_EVENT_GAMEPAD_BUTTON_DOWN && unsafe {
                    event.gbutton.button == back_button.0 as u8 || event.gbutton.button == SDL_GAMEPAD_BUTTON_BACK.0 as u8
                });
            let page = streaming.menu_page != stream_menu::Page::Grid;
            match menu_step(key_action, back, repeat, streaming.shortcut_sheet.is_some(), streaming.session_page.is_some(), page) {
                MenuStep::Pass => {}
                MenuStep::Swallow => return,
                MenuStep::ToggleSheet => return streaming.toggle_shortcut_sheet(self.window, &mut self.pads),
                MenuStep::CloseSessionPage => return streaming.close_session_page(),
                MenuStep::CloseSheet => return streaming.close_shortcut_sheet(self.window, &mut self.pads),
                MenuStep::ClosePage => {
                    streaming.menu_page = stream_menu::Page::Grid;
                    streaming.needs_redraw = true;
                    return;
                }
                MenuStep::CloseMenu => return streaming.toggle_menu(self.window, &mut self.pads),
            }
            // Y, X and the shoulders run their tiles while the grid shows (nightjar_policy::stream_menu).
            if kind == SDL_EVENT_GAMEPAD_BUTTON_DOWN && !page && streaming.session_page.is_none() && streaming.shortcut_sheet.is_none() {
                // SAFETY: union member read by event type.
                let button = SDL_GamepadButton(i32::from(unsafe { event.gbutton.button }));
                if let Some(key) = stream_menu::hotkey(button) {
                    streaming.menu_hotkey = Some(key);
                    streaming.needs_redraw = true;
                    return;
                }
            }
            if matches!(kind, SDL_EVENT_GAMEPAD_BUTTON_DOWN | SDL_EVENT_GAMEPAD_AXIS_MOTION) {
                streaming.needs_redraw = true;
                self.navigate(event);
                return;
            }
            if !matches!(kind, SDL_EVENT_QUIT | SDL_EVENT_WINDOW_CLOSE_REQUESTED
                | SDL_EVENT_WINDOW_FOCUS_LOST | SDL_EVENT_WINDOW_FOCUS_GAINED
                | SDL_EVENT_GAMEPAD_ADDED | SDL_EVENT_GAMEPAD_REMOVED
                | SDL_EVENT_AUDIO_DEVICE_ADDED | SDL_EVENT_AUDIO_DEVICE_REMOVED
                | SDL_EVENT_RENDER_DEVICE_RESET | SDL_EVENT_RENDER_DEVICE_LOST) {
                self.gui.handle(event, Scale::of(self.window));
                streaming.needs_redraw = true;
                return;
            }
        }
        // Microphone only's window has its own buttons; its chords still reach the stream.
        if streaming.mic_only() && !matches!(kind, SDL_EVENT_QUIT | SDL_EVENT_WINDOW_CLOSE_REQUESTED
            | SDL_EVENT_WINDOW_FOCUS_LOST | SDL_EVENT_WINDOW_FOCUS_GAINED
            | SDL_EVENT_AUDIO_DEVICE_ADDED | SDL_EVENT_AUDIO_DEVICE_REMOVED
            | SDL_EVENT_RENDER_DEVICE_RESET | SDL_EVENT_RENDER_DEVICE_LOST) {
            self.gui.handle(event, Scale::of(self.window));
            streaming.needs_redraw = true;
        }
        streaming.handle(event, self.window, self.renderer, &mut self.pads);
    }
}

/// What a key or button does while the stream menu is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuStep {
    /// Not the menu's: egui or the controller navigation gets it.
    Pass,
    /// A held key's repeat of a toggle: holding it doesn't flip the menu open and shut.
    Swallow,
    ToggleSheet,
    CloseSessionPage,
    CloseSheet,
    /// Back from a page a tile opened to the grid.
    ClosePage,
    CloseMenu,
}

/// The step for a stream chord (`action`), Escape or B (`back`), with the menu's pages. Back acts
/// on what is drawn: the Session page first (the sheet never draws over it), then the sheet, then
/// a page a tile opened (`page`).
fn menu_step(action: Option<Action>, back: bool, repeat: bool, sheet: bool, session_page: bool, page: bool) -> MenuStep {
    let controls = action == Some(Action::StreamControls);
    if action != Some(Action::StreamShortcuts) && !back && !controls {
        return MenuStep::Pass;
    }
    if repeat {
        return MenuStep::Swallow;
    }
    if action == Some(Action::StreamShortcuts) {
        MenuStep::ToggleSheet
    } else if session_page {
        MenuStep::CloseSessionPage
    } else if back && sheet {
        MenuStep::CloseSheet
    } else if back && page {
        MenuStep::ClosePage
    } else {
        MenuStep::CloseMenu
    }
}

/// The key egui gets for a controller button's launcher action (`shortcuts::PAD`).
fn nav_key(action: Action) -> Option<egui::Key> {
    match action {
        Action::Select => Some(egui::Key::Enter),
        Action::Back => Some(egui::Key::Escape),
        Action::Options => Some(egui::Key::F2),
        _ => None,
    }
}

/// The width of a list in the stream menu's rows.
const MENU_LIST: f32 = 190.0;

/// The overlay and toasts, drawn with egui over the picture.
/// Returns whether the user's shortcuts changed (saved or deleted in the stream menu) and want writing.
fn draw_overlay(gui: &mut Gui, window: *mut SDL_Window, renderer: *mut SDL_Renderer, s: &mut Streaming, pads: &mut Gamepads, shortcuts: &mut shortcuts::Shortcuts) -> bool {
    // Glass holds nothing while a stream plays, and draws no live blur over its video.
    gui.set_over_video(true);
    // A stream started from the command line never drew the launcher, which loads these too.
    theme::fonts_for(&gui.ctx, strings::current());
    let mut shortcuts_changed = false;
    let toast = s.toast.as_ref().map(|(t, _)| t.clone());
    let input_only = s.facts.role == nightjar_core::http::AppRole::InputOnly;
    let small_stream = s.small_stream_input_only();
    let mic_only = s.mic_only();
    if !s.overlay_visible && toast.is_none() && !input_only && !s.menu_visible {
        return false;
    }
    let mode = s.overlay_mode;
    let lines = s.overlay.lines(mode).to_vec();
    let compact = (mode == config::OverlayMode::Compact).then(|| (s.overlay.compact.clone(), s.overlay.notice.clone()));
    let (anchor, offset) = overlay_anchor(s.overlay_corner);
    let visible = s.overlay_visible;
    let title = format!("{} · {}", s.facts.app_name, s.facts.host_name);
    // Screen readers get the menu and the Remote Input screen, not every frame of the overlay.
    gui.set_readable(s.menu_visible || input_only);
    let output = gui.run(window, renderer, |ui| {
        let ctx = ui.ctx().clone();
        if mic_only {
            egui::Area::new(egui::Id::unique("mic-only"))
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(&ctx, |ui| {
                    ui.label(egui::RichText::new(tr(mic_only_title(s.microphone_muted, s.microphone_unavailable, s.microphone_busy()))).font(theme::semibold(theme::text::TITLE)).color(theme::MOON));
                    ui.label(&s.facts.host_name);
                    ui.horizontal(|ui| {
                        // Unmuting also tries an unavailable microphone again.
                        let mute = if s.microphone_muted { tr("Unmute") } else { tr("Mute") };
                        if ui.button(mute).clicked() {
                            let muted = !s.microphone_muted;
                            s.mute_microphone(muted, pads);
                            s.needs_redraw = true;
                        }
                        if ui.button(tr("Stop")).clicked() {
                            s.exit = Some(Exit::User);
                        }
                    });
                });
        } else if input_only {
            egui::Area::new(egui::Id::unique("input-only"))
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(&ctx, |ui| {
                    ui.label(egui::RichText::new(tr("Remote Input connected")).font(theme::semibold(theme::text::TITLE)).color(theme::MOON));
                    ui.label(&title);
                    ui.label(tr("Keyboard, mouse and controllers control the PC."));
                    if let Some(keys) = shortcuts::chord_for(Scope::Stream, Action::Disconnect) {
                        ui.label(trf("{} disconnects", &[&keys.text()]));
                    }
                    if small_stream {
                        let mbps = format!("{:.1}", f64::from(nightjar_policy::launch::estimated_kbps()) / 1000.0);
                        ui.label(trf("The PC still streams small video (about {} Mbit/s); it isn’t shown.", &[&mbps]));
                    }
                });
        }
        if s.menu_visible && s.session_page.is_some() {
            let lines = s.connection_lines();
            let hints = s.session_hints();
            let mut outcome = session_ui::Outcome::Stay;
            if let Some(page) = s.session_page.as_mut() {
                menu_style::show(&ctx, tr("Session and graphics"), "session-and-graphics", |ui| menu_style::body(ui, |ui| {
                        gui::follow_focus(ui, &mut s.menu_focus);
                        menu_style::header(ui, tr("Session and graphics"), &title);
                        outcome = session_ui::show(ui, page, &lines, hints);
                    }));
            }
            match outcome {
                session_ui::Outcome::Stay => {}
                session_ui::Outcome::Resume => s.toggle_menu(window, pads),
                session_ui::Outcome::HostMenu => s.open_host_menu(window, pads),
                session_ui::Outcome::Back => s.close_session_page(),
                session_ui::Outcome::Disconnect | session_ui::Outcome::Leave => s.exit = Some(Exit::User),
            }
        }
        if s.menu_visible && s.session_page.is_none() && s.shortcut_sheet.is_some() {
            let from_menu = s.shortcut_sheet == Some(stream::Sheet::FromMenu);
            let style = glyphs::Style::of(pads);
            let offered = s.controller_mouse_offered(pads);
            let mut back = false;
            menu_style::show(&ctx, tr("Shortcuts & help"), "stream-controls", |ui| menu_style::body(ui, |ui| {
                    gui::follow_focus(ui, &mut s.menu_focus);
                    menu_style::header(ui, tr("Shortcuts & help"), &title);
                    back = menu_style::sheet(ui, style, pads.quit_combo(), pads.back_opens_menu(), offered, from_menu);
                }));
            if back {
                s.close_shortcut_sheet(window, pads);
            }
        } else if s.menu_visible && s.session_page.is_none() {
            let available = stream_menu::Available {
                mouse: s.controller_mouse_offered(pads).then(|| pads.mouse_active()),
                overlay: s.overlay_visible,
                shortcuts: s.facts.role != nightjar_core::http::AppRole::RemoteMonitor,
                microphone: s.microphone_available().then_some(!s.microphone_muted),
                quality: s.facts.runtime_bitrate || s.rate_change.is_some() || s.menu_controls().size_editor
                    || s.facts.role != nightjar_core::http::AppRole::InputOnly,
                clipboard: s.facts.clipboard,
                image_clipboard: s.facts.image_clipboard,
                commands: !s.facts.server_commands.is_empty(),
                session: s.session_page_available(),
                host_audio: !s.host_audio.switches().is_empty() || s.host_audio.problem().is_some(),
                controller_audio: s.controller_audio_available(),
                usb: s.usb.is_some() || s.usb_refused.is_some(),
                exits: s.game_exits(),
            };
            let layout = stream_menu::layout(&available, shortcuts.data.len(), pads.menu_buttons());
            let mut page = std::mem::take(&mut s.menu_page);
            let mut next = None;
            let grid_page = page == stream_menu::Page::Grid;
            let (heading, line) = if grid_page { s.menu_header() } else { (page.title().to_owned(), title.clone()) };
            let button = if grid_page { s.menu_hotkey.and_then(|key| stream_menu::button_leave(&layout, key)) } else { None };
            let mut leave = None;
            menu_style::show(&ctx, tr("Stream controls"), "stream-controls", |ui| {
                // On the grid the head and the ways out stay in view; only the tiles scroll.
                if grid_page && menu_style::head(ui, &heading, &line) {
                    leave = Some(stream_menu::Leave::Close);
                }
                let reserve = if grid_page { menu_style::pinned_room(&ctx, "stream-menu-footer") } else { 0.0 };
                menu_style::body_above(ui, reserve, |ui| {
                    use glyphs::Icon;
                    gui::follow_focus(ui, &mut s.menu_focus);
                    if !grid_page {
                        menu_style::header(ui, &heading, &line);
                    }
                    // Nothing focused: the menu just opened or a page changed. The first tile takes
                    // the focus, so A right after opening never leaves the stream.
                    let focus_first = ctx.memory(|m| m.focused().is_none());
                    match &mut page {
                        stream_menu::Page::Grid => {
                            match stream_menu::grid(ui, &layout, shortcuts, s.menu_hotkey.take(), focus_first) {
                                Some(stream_menu::Chosen::Tile(id)) => match id {
                                    "controller_mouse" => s.toggle_controller_mouse(pads),
                                    "overlay" => s.overlay_visible = !s.overlay_visible,
                                    "shortcuts" => next = Some(stream_menu::Page::Shortcuts),
                                    "microphone" => s.mute_microphone(!s.microphone_muted, pads),
                                    "bitrate" => next = Some(stream_menu::Page::Quality),
                                    "clipboard_send" => s.clipboard(true),
                                    "clipboard_receive" => s.clipboard(false),
                                    "server_commands" => next = Some(stream_menu::Page::Commands),
                                    "native_page" => s.session_page_requested = true,
                                    _ => next = Some(stream_menu::Page::More),
                                },
                                Some(stream_menu::Chosen::Shortcut(index)) => {
                                    let shortcut = shortcuts.data[index].clone();
                                    s.toggle_menu(window, pads);
                                    s.send_shortcut(&shortcut);
                                }
                                Some(stream_menu::Chosen::Add) => next = Some(stream_menu::Page::Editor(stream_menu::Draft::default())),
                                None => {}
                            }
                        }
                        stream_menu::Page::Confirm(exit) => match stream_menu::confirm(ui, *exit, focus_first) {
                            Some(true) => {
                                s.end_on_host(*exit);
                                next = Some(stream_menu::Page::Grid);
                            }
                            Some(false) => next = Some(stream_menu::Page::Grid),
                            None => {}
                        },
                        stream_menu::Page::ConfirmDisconnect => match stream_menu::confirm_disconnect(ui, focus_first) {
                            Some(true) => s.exit = Some(Exit::User),
                            Some(false) => next = Some(stream_menu::Page::Grid),
                            None => {}
                        },
                        stream_menu::Page::Shortcuts => match stream_menu::shortcuts_page(ui, shortcuts, focus_first) {
                            Some(stream_menu::ShortcutChoice::Send(shortcut)) => {
                                s.toggle_menu(window, pads);
                                s.send_shortcut(&shortcut);
                            }
                            Some(stream_menu::ShortcutChoice::Edit(index)) => {
                                next = Some(stream_menu::Page::Editor(stream_menu::Draft::edit(&shortcuts.data[index])));
                            }
                            Some(stream_menu::ShortcutChoice::Add) => next = Some(stream_menu::Page::Editor(stream_menu::Draft::default())),
                            None => {}
                        },
                        stream_menu::Page::Editor(draft) => match stream_menu::editor(ui, draft, shortcuts, focus_first) {
                            stream_menu::Edited::Stay => {}
                            stream_menu::Edited::Changed => {
                                shortcuts_changed = true;
                                next = Some(stream_menu::Page::Shortcuts);
                            }
                            stream_menu::Edited::Cancel => next = Some(stream_menu::Page::Shortcuts),
                        },
                        stream_menu::Page::Commands => {
                            for (index, name) in s.facts.server_commands.iter().enumerate() {
                                if !name.is_empty() && menu_style::action(ui, Icon::Terminal, name, None).clicked() {
                                    s.pending_command = Some(index);
                                }
                            }
                            if let Some(index) = s.pending_command {
                                menu_style::text_block(ui, &trf("Run ‘{}’ on the PC?", &[&s.facts.server_commands[index]]));
                                ui.horizontal(|ui| {
                                    if menu_style::button(ui, tr("Run command")).clicked() {
                                        s.pending_command = None;
                                        s.server_command(index);
                                    }
                                    if menu_style::button(ui, tr("Cancel")).clicked() { s.pending_command = None; }
                                });
                            }
                        }
                        stream_menu::Page::Quality => menu_style::group(ui, |ui| {
                            let controls = s.menu_controls();
                            if s.facts.runtime_bitrate {
                                if s.facts.role == nightjar_core::http::AppRole::Stream && !s.client_hint_active() {
                                    let mut enabled = s.adaptive_bitrate;
                                    if ui.add_enabled_ui(!s.bitrate_busy, |ui| menu_style::switch(ui, Icon::Gauge, tr("Device controls bitrate"), &mut enabled)).inner.changed() {
                                        s.enable_adaptive_bitrate(enabled);
                                    }
                                }
                                if controls.http_bitrate && stream::menu_extras::bitrate_slider(ui, &mut s.bitrate_kbps, !s.adaptive_bitrate && !s.bitrate_busy) {
                                    s.set_bitrate();
                                }
                            }
                            if let Some(rates) = s.rate_change.as_mut().filter(|_| controls.rate_change) {
                                let busy = rates.menu_busy();
                                let mut apply = stream::menu_extras::bitrate_slider(ui, &mut rates.menu_kbps, !busy);
                                menu_style::fold(ui, "rate-change", Icon::Gauge, tr("Change bitrate / frame rate"), |ui| {
                                    let busy = rates.menu_busy();
                                    ui.add_enabled_ui(!busy, |ui| {
                                        ui.horizontal(|ui| {
                                            theme::step_value(ui, &mut rates.menu_kbps, 500..=800_000, 1000.0, |d| d.speed(100).suffix(" kbps"));
                                            theme::step_value(ui, &mut rates.menu_fps, 10.0..=500.0, 1.0, |d| d.suffix(" fps").max_decimals(3));
                                        });
                                        apply |= menu_style::button(ui, tr("Apply bitrate and frame rate")).clicked();
                                    });
                                    if busy { menu_style::text_block(ui, tr("Waiting for the PC…")); }
                                });
                                if apply { s.change_rates(); }
                            }
                            if s.facts.role != nightjar_core::http::AppRole::InputOnly {
                                menu_style::fold(ui, "stream-health", Icon::Pulse, tr("Why did my stream stutter?"), |ui| {
                                    let (text, offer) = s.health();
                                    menu_style::text_block(ui, &text);
                                    ui.add_space(theme::space::S);
                                    ui.horizontal(|ui| {
                                        if let Some(label) = stream::doctor::offer_label(offer) {
                                            if ui.add_enabled_ui(!s.bitrate_busy, |ui| menu_style::button(ui, label)).inner.clicked() { s.health_trial(offer); }
                                        }
                                        if menu_style::button(ui, tr("Copy report")).clicked() { s.copy_health(); }
                                    });
                                });
                            }
                            if controls.size_editor {
                                menu_style::fold(ui, "stream-size", Icon::Screen, tr("Stream size"), |ui| {
                                    let mut quality = s.quality_mode;
                                    Combo::from_id_salt("stream-quality").selected_text(tr(quality.label())).show_ui(ui, |ui| {
                                        for mode in [config::QualityMode::Off, config::QualityMode::Motion, config::QualityMode::Detail, config::QualityMode::Battery] {
                                            ui.selectable_value(&mut quality, mode, tr(mode.label()));
                                        }
                                    });
                                    if quality != s.quality_mode { s.set_quality(quality); }
                                    ui.add_enabled_ui(s.quality_mode == config::QualityMode::Off, |ui| {
                                        ui.horizontal(|ui| {
                                            theme::step_value(ui, &mut s.requested_mode.width, 256..=8192, 16.0, |d| d.speed(2));
                                            ui.label("×");
                                            theme::step_value(ui, &mut s.requested_mode.height, 256..=8192, 16.0, |d| d.speed(2));
                                            // With 0x553C the rate is set above and the size keeps the current one.
                                            if controls.size_editor_fps {
                                                let mut fps = s.requested_mode.fps_millihz as f64 / 1000.0;
                                                if theme::step_value(ui, &mut fps, 10.0..=500.0, 1.0, |d| d.suffix(" fps").max_decimals(3)).changed() {
                                                    s.requested_mode.fps_millihz = (fps * 1000.0).round() as u32;
                                                }
                                            }
                                        });
                                        let apply = if controls.size_editor_fps { tr("Apply stream size and rate") } else { tr("Apply stream size") };
                                        if menu_style::button(ui, apply).clicked() { s.set_mode(s.requested_mode); }
                                    });
                                });
                            }
                        }),
                        stream_menu::Page::More => menu_style::group(ui, |ui| {
                            // The model's order (nightjar_policy::stream_menu::TILES), one row or group per ID.
                            for id in &layout.more {
                                match *id {
                                    "picture" => {
                                        menu_style::row(ui, Icon::Overlay, tr("Performance overlay"), |ui| {
                                            let shown = if s.overlay_visible { tr(s.overlay_mode.label()) } else { tr("Off") };
                                            Combo::from_id_salt("stream-overlay").width(MENU_LIST).selected_text(shown).show_ui(ui, |ui| {
                                                if ui.selectable_label(!s.overlay_visible, tr("Off")).clicked() { s.overlay_visible = false; }
                                                for mode in config::OverlayMode::ALL {
                                                    if ui.selectable_label(s.overlay_visible && s.overlay_mode == mode, tr(mode.label())).clicked() {
                                                        (s.overlay_visible, s.overlay_mode) = (true, mode);
                                                    }
                                                }
                                            });
                                        });
                                        if s.overlay_visible {
                                            menu_style::row(ui, Icon::Corner, tr("Overlay position"), |ui| {
                                                Combo::from_id_salt("stream-overlay-corner").width(MENU_LIST).selected_text(tr(s.overlay_corner.label())).show_ui(ui, |ui| {
                                                    for corner in config::OverlayCorner::ALL {
                                                        ui.selectable_value(&mut s.overlay_corner, corner, tr(corner.label()));
                                                    }
                                                });
                                            });
                                        }
                                        let current = s.video_scaling();
                                        let (mut scaling, top) = current;
                                        menu_style::row(ui, Icon::Screen, tr("Video size"), |ui| {
                                            Combo::from_id_salt("stream-scaling").width(MENU_LIST).selected_text(tr(scaling.label())).show_ui(ui, |ui| {
                                                use config::VideoScaling;
                                                for mode in [VideoScaling::Fit, VideoScaling::Fill, VideoScaling::Stretch, VideoScaling::Native] {
                                                    ui.selectable_value(&mut scaling, mode, tr(mode.label()));
                                                }
                                            });
                                        });
                                        if scaling != current.0 { s.set_video_scaling(scaling, top); }
                                    }
                                    // The PC's night mode and headphone surround, as it answered when the menu opened.
                                    "night_mode" => {
                                        for switch in s.host_audio.switches() {
                                            let mut on = switch.on;
                                            let label = stream::menu_extras::audio_label(switch.id);
                                            if ui.add_enabled_ui(switch.enabled, |ui| menu_style::switch(ui, Icon::Speaker, label, &mut on)).inner.changed() { s.set_host_audio(switch.id, on); }
                                            if let Some(note) = &switch.note { menu_style::note(ui, note); }
                                        }
                                        if let Some(problem) = s.host_audio.problem() { menu_style::note(ui, problem); }
                                    }
                                    "image_clipboard_send" => if menu_style::action(ui, Icon::Image, tr("Send image to PC"), None).clicked() { s.clipboard_image(true); },
                                    "image_clipboard_receive" => if menu_style::action(ui, Icon::Image, tr("Copy PC image here"), None).clicked() { s.clipboard_image(false); },
                                    "controller_audio" => {
                                        menu_style::fold(ui, "controller-audio", Icon::Controller, tr("DualSense USB audio and waveform haptics"), |ui| {
                                        let outputs = controller_audio::outputs();
                                        let inputs = controller_audio::inputs();
                                        let mut selections = Vec::new();
                                        let mut input_selections = Vec::new();
                                        for pad in &mut pads.pads {
                                            if !pad.supports_audio() { continue; }
                                            let current = pad.audio.as_ref().map(|a| a.device);
                                            let mut selected = current;
                                            let label = current.and_then(|id| outputs.iter().find(|(device, _)| *device == id)).map_or(tr("Off"), |(_, name)| name.as_str());
                                            Combo::from_id_salt(("controller-audio", pad.index)).selected_text(trf("Controller {}: {}", &[&(pad.index + 1), &label])).show_ui(ui, |ui| {
                                                ui.selectable_value(&mut selected, None, tr("Off"));
                                                for (id, name) in &outputs { ui.selectable_value(&mut selected, Some(*id), name); }
                                            });
                                            if selected != current { selections.push((pad.index, selected)); }
                                            if let Some(audio) = &mut pad.audio {
                                                let mut input = audio.input;
                                                let label = input.and_then(|id| inputs.iter().find(|(device, _)| *device == id)).map_or(tr("Off"), |(_, name)| name.as_str());
                                                ui.add_enabled_ui(s.microphone_available(), |ui| {
                                                    Combo::from_id_salt(("controller-microphone", pad.index)).selected_text(trf("Microphone: {}", &[&label])).show_ui(ui, |ui| {
                                                        ui.selectable_value(&mut input, None, tr("Off"));
                                                        for (id, name) in &inputs { ui.selectable_value(&mut input, Some(*id), name); }
                                                    });
                                                });
                                                if input != audio.input { input_selections.push((pad.index, input)); }
                                                Combo::from_id_salt(("controller-headphones", pad.index))
                                                    .selected_text(match audio.headphone_override { None => tr("Automatic headset detection"), Some(true) => tr("Headset"), Some(false) => tr("Controller speaker") })
                                                    .show_ui(ui, |ui| {
                                                        ui.selectable_value(&mut audio.headphone_override, None, tr("Automatic headset detection"));
                                                        ui.selectable_value(&mut audio.headphone_override, Some(true), tr("Headset"));
                                                        ui.selectable_value(&mut audio.headphone_override, Some(false), tr("Controller speaker"));
                                                    });
                                                if audio.headphone_override.is_none() && audio.detected_headphones.is_none() {
                                                    ui.weak(tr("Headset detection unavailable. Select Headset when using the controller jack."));
                                                }
                                            }
                                        }
                                        for (index, device) in selections {
                                            match pads.set_audio(index, device) {
                                                Ok(()) => s.session.input(|q| pads.announce_new(q)),
                                                Err(error) => s.show_toast(trf("Controller audio unavailable: {}", &[&error])),
                                            }
                                        }
                                        for (index, device) in input_selections {
                                            if let Err(error) = pads.set_audio_input(index, device) { s.show_toast(error); }
                                        }
                                        });
                                    }
                                    "usb_devices" => {
                                        if let Some(panel) = s.usb.as_mut() {
                                            menu_style::fold(ui, "usb", Icon::Usb, tr("USB devices"), |ui| usb::show(ui, panel));
                                        } else if let Some(why) = s.usb_refused {
                                            // Why there are no devices shows without opening anything.
                                            usb::show_refusal(ui, why);
                                        }
                                    }
                                    "export_statistics" => if menu_style::action(ui, Icon::Download, tr("Export statistics (last hour)"), None).clicked() { s.export_statistics = true; },
                                    "help" => {
                                        if menu_style::action(ui, Icon::Help, tr("Shortcuts & help"), Some(shortcuts::Action::StreamShortcuts)).clicked() {
                                            s.shortcut_sheet = Some(stream::Sheet::FromMenu);
                                            ctx.memory_mut(|m| m.surrender_focus(m.focused().unwrap_or(egui::Id::NULL)));
                                        }
                                    }
                                    "reconnect" => if menu_style::action(ui, Icon::Reconnect, tr("Reconnect"), None).clicked() { s.exit = Some(Exit::Reconnect); },
                                    other => unreachable!("More has no row for {other}"),
                                }
                            }
                        }),
                    }
                    if !matches!(page, stream_menu::Page::Grid | stream_menu::Page::Confirm(_) | stream_menu::Page::ConfirmDisconnect) {
                        ui.add_space(theme::space::M);
                        if menu_style::action_keys(ui, Icon::Back, tr("Back"), Some(crate::shortcuts::Key::Escape.cap().as_ref())).clicked() {
                            next = Some(stream_menu::Page::Grid);
                        }
                    }
                });
                if grid_page {
                    leave = leave.or(menu_style::pinned(ui, "stream-menu-footer", |ui| stream_menu::footer(ui, &layout)));
                }
                });
            // From a button Disconnect asks first, as the ways out that end the game do.
            let leave = match (leave, button) {
                (None, Some(stream_menu::Leave::Disconnect)) => {
                    next = Some(stream_menu::Page::ConfirmDisconnect);
                    None
                }
                (None, button) => button,
                (clicked, _) => clicked,
            };
            match leave {
                Some(stream_menu::Leave::Close) => s.toggle_menu(window, pads),
                Some(stream_menu::Leave::Disconnect) => s.exit = Some(Exit::User),
                Some(stream_menu::Leave::Exit(exit)) => next = Some(stream_menu::Page::Confirm(exit)),
                None => {}
            }
            if let Some(next) = next {
                // The new page starts with nothing focused, so its first tile or row takes the focus.
                ctx.memory_mut(|m| m.surrender_focus(m.focused().unwrap_or(egui::Id::NULL)));
                page = next;
            }
            // Closing from a page (Disconnect, a shortcut) already reset it to the grid.
            if s.menu_visible {
                s.menu_page = page;
            }
        } else {
            s.settle_bitrate_slider(&ctx);
        }
        if visible {
            egui::Area::new(egui::Id::unique("overlay"))
                .anchor(anchor, offset)
                .show(&ctx, |ui| {
                    // The stream's HUD glass, painted under the text once its size is known; no blur under it.
                    let place = ui.painter().add(egui::Shape::Noop);
                    let frame = egui::Frame::new()
                        .inner_margin(egui::Margin::symmetric(14, 10))
                        .show(ui, |ui| {
                            if let Some((line, notice)) = &compact {
                                ui.horizontal(|ui| {
                                    let [r, g, b] = stream::overlay::health_rgb(line.health);
                                    ui.label(egui::RichText::new(line.health.glyph()).font(theme::semibold(theme::text::LABEL)).color(egui::Color32::from_rgb(r, g, b)));
                                    let text = if line.text.is_empty() { tr("Waiting for video").to_owned() } else { line.text.clone() };
                                    ui.label(egui::RichText::new(text).font(theme::regular(theme::text::LABEL)).color(theme::ON_SURFACE));
                                });
                                if let Some(notice) = notice {
                                    ui.label(egui::RichText::new(notice).font(theme::regular(theme::text::CAPTION)).color(theme::ON_SURFACE_VARIANT));
                                }
                                return;
                            }
                            ui.label(egui::RichText::new(&title).font(theme::semibold(theme::text::LABEL)).color(theme::MOON));
                            ui.add_space(2.0);
                            ui.spacing_mut().interact_size.y = 16.0;
                            egui::Grid::new("stats").num_columns(2).spacing([14.0, 3.0]).show(ui, |ui| {
                                if lines.is_empty() {
                                    ui.label(egui::RichText::new(tr("Measuring…")).color(theme::ON_SURFACE_VARIANT));
                                    ui.end_row();
                                }
                                for (k, v) in &lines {
                                    ui.label(egui::RichText::new(k).font(theme::regular(theme::text::CAPTION)).color(theme::OUTLINE));
                                    ui.label(egui::RichText::new(v).font(theme::regular(theme::text::CAPTION)).color(theme::ON_SURFACE));
                                    ui.end_row();
                                }
                            });
                        });
                    let mut glass = Vec::new();
                    ui::glass::hud_surface(&mut glass, ui.ctx(), frame.response.rect, 12.0);
                    ui.painter().set(place, egui::Shape::Vec(glass));
                });
        }
        if let Some(text) = &toast {
            egui::Area::new(egui::Id::unique("toast"))
                .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -40.0))
                .show(&ctx, |ui| {
                    let place = ui.painter().add(egui::Shape::Noop);
                    let frame = egui::Frame::new()
                        .inner_margin(egui::Margin::symmetric(18, 10))
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new(text).font(theme::regular(theme::text::LABEL)).color(theme::ON_SURFACE));
                        });
                    let mut glass = Vec::new();
                    ui::glass::hud_surface(&mut glass, ui.ctx(), frame.response.rect, 18.0);
                    ui.painter().set(place, egui::Shape::Vec(glass));
                });
        }
    });
    if s.menu_visible {
        gui.platform(window, &output.platform_output);
    }
    gui.paint(renderer, output);
    shortcuts_changed
}

/// Input-only on a PC without its own mode: the smallest stream every host takes, as on Android.
/// It leaves the PC's display mode and its speakers alone: no virtual display or VRR, and the
/// PC keeps playing its sound.
/// A session whose picture nobody sees (input only, microphone only): the plainest stream, and
/// nothing that needs the GPU renderer (Linux would switch to it, or refuse with OpenGL set).
fn input_only_settings(settings: &mut config::Settings) {
    settings.hdr = false;
    settings.experimental_packed_444 = false;
    (settings.prefer_yuv444, settings.sdr_10bit) = (false, false);
    settings.upscaling = config::Upscaling::Bilinear;
    settings.codec = config::CodecPreference::H264;
    settings.width = 1280;
    settings.height = 720;
    settings.fps = 60;
}

fn small_stream(settings: &mut config::Settings) {
    use nightjar_policy::launch::{SMALL_BITRATE_KBPS, SMALL_FPS, SMALL_HEIGHT, SMALL_WIDTH};
    (settings.width, settings.height, settings.fps, settings.bitrate_kbps) = (SMALL_WIDTH, SMALL_HEIGHT, SMALL_FPS, SMALL_BITRATE_KBPS);
    settings.audio = config::AudioChannels::Stereo;
    (settings.virtual_display, settings.vrr, settings.play_audio_on_host) = (false, false, true);
    // Nothing is shown, so nothing is offered beyond plain 4:2:0 or said about it, or scaled.
    (settings.prefer_yuv444, settings.sdr_10bit) = (false, false);
    settings.upscaling = config::Upscaling::Bilinear;
}

/// `probe`'s answer for `hardware`, from `cache` when it was the last decoder checked.
/// What a decoder check's result holds for: the decoder, the formats it checked, and "Vulkan video
/// first", which changes the backend order both checks decode through (`Hardware::order`).
type ProbeKey = (Hardware, u32, bool);

fn probe_key(hardware: Hardware, wanted: u32, settings: &config::Settings) -> ProbeKey {
    (hardware, wanted, settings.vulkan_video_first)
}

fn probed_once<K: PartialEq, T: Clone>(cache: &mut Option<(K, T)>, hardware: K, probe: impl FnOnce() -> T) -> T {
    match cache {
        Some((probed, result)) if *probed == hardware => result.clone(),
        _ => cache.insert((hardware, probe())).1.clone(),
    }
}

/// A Wayland window learns its HDR state after it is mapped: the log says when, launcher or stream.
fn log_window_hdr(event: &SDL_Event) {
    // SAFETY: union members read by event type.
    unsafe {
        if SDL_EventType(event.r#type) == SDL_EVENT_WINDOW_HDR_STATE_CHANGED {
            eprintln!("nightjar: the compositor reports window HDR {}", if event.window.data1 != 0 { "on" } else { "off" });
        }
    }
}

/// The launcher window's flags for how it opens (handheld.rs).
fn start_flags(start: handheld::StartWindow) -> SDL_WindowFlags {
    SDL_WINDOW_RESIZABLE | SDL_WINDOW_HIGH_PIXEL_DENSITY | SDL_WINDOW_HIDDEN
        | if start.fullscreen { SDL_WINDOW_FULLSCREEN } else { SDL_WindowFlags(0) }
        | if start.borderless { SDL_WINDOW_BORDERLESS } else { SDL_WindowFlags(0) }
}

/// A stick or trigger past this counts as controller use for the pointer; jitter below it does not.
const POINTER_AXIS_THRESHOLD: i16 = 12000;

/// What a launcher event is for the pointer (handheld::Pointer). Touch and pen arrive as mouse
/// events too, but a tap is not the trackpad, so they leave the pointer as it is.
fn pointer_input(event: &SDL_Event) -> handheld::Input {
    let synthetic = |which: SDL_MouseID| matches!(which, SDL_TOUCH_MOUSEID | SDL_PEN_MOUSEID);
    // SAFETY: union members read by event type.
    unsafe {
        match SDL_EventType(event.r#type) {
            SDL_EVENT_GAMEPAD_BUTTON_DOWN => handheld::Input::Controller,
            SDL_EVENT_GAMEPAD_AXIS_MOTION if event.gaxis.value.unsigned_abs() > POINTER_AXIS_THRESHOLD as u16 => handheld::Input::Controller,
            SDL_EVENT_MOUSE_MOTION if !synthetic(event.motion.which) => handheld::Input::Mouse,
            SDL_EVENT_MOUSE_BUTTON_DOWN if !synthetic(event.button.which) => handheld::Input::Mouse,
            SDL_EVENT_MOUSE_WHEEL if !synthetic(event.wheel.which) => handheld::Input::Mouse,
            _ => handheld::Input::Other,
        }
    }
}

/// The stream's window mode: gamescope scales the window itself, so no exclusive mode change there.
fn stream_window_mode(mode: WindowMode, gamescope: bool) -> WindowMode {
    match mode {
        WindowMode::Fullscreen if gamescope => WindowMode::Borderless,
        mode => mode,
    }
}

/// An HDR stream under gamescope without HDR output plays in SDR and says so (handheld.rs).
const HDR_OFF_NOTE: &str = strings::en("HDR is off: gamescope offers no HDR output here, so this stream is in SDR.");

/// The toast a stream starts with: a network warning, the PyroWave fallback and what became of
/// 4:4:4 and 10-bit SDR, else the way to the stream menu (under gamescope, where the
/// controller is the input and Steam keeps the Guide button, Start+Back's).
fn start_toast(warning: Option<&str>, color: Option<&str>, gamescope: bool) -> String {
    let lines: Vec<&str> = warning.into_iter().chain(color).collect();
    if !lines.is_empty() {
        lines.join("\n")
    } else if gamescope {
        tr("Start+Back opens the stream menu").to_owned()
    } else {
        controls_hint()
    }
}

/// The stream hotkeys that open the controls, disconnect and release input, as they are bound.
fn controls_hint() -> String {
    use shortcuts::{chord_for, Chord};
    let keys = |action| chord_for(Scope::Stream, action).map(Chord::text);
    let parts = [
        keys(Action::StreamControls).map(|k| trf("{} opens controls", &[&k])),
        keys(Action::Disconnect).map(|k| trf("{} disconnects", &[&k])),
        keys(Action::Capture).map(|k| trf("{} releases input", &[&k])),
    ];
    parts.into_iter().flatten().collect::<Vec<_>>().join(", ")
}

/// Where the overlay sits: its corner, 16 points in from both edges.
fn overlay_anchor(corner: config::OverlayCorner) -> (egui::Align2, egui::Vec2) {
    use config::OverlayCorner;
    match corner {
        OverlayCorner::TopLeft => (egui::Align2::LEFT_TOP, egui::vec2(16.0, 16.0)),
        OverlayCorner::TopRight => (egui::Align2::RIGHT_TOP, egui::vec2(-16.0, 16.0)),
        OverlayCorner::BottomLeft => (egui::Align2::LEFT_BOTTOM, egui::vec2(16.0, -16.0)),
        OverlayCorner::BottomRight => (egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -16.0)),
    }
}

fn save_hosts(store: &config::Store, hosts: &Hosts) {
    if let Err(e) = store.save_hosts(&hosts.records()) {
        eprintln!("nightjar: saving hosts failed: {e}");
    }
}

/// Keeps the name chosen for a PC on this device, and saves it.
fn rename_host(hosts: &mut Hosts, store: &config::Store, id: u64, alias: &str) {
    if hosts.rename(id, alias) {
        save_hosts(store, hosts);
    }
}

/// Forgets a PC: its saved library, covers, recent games and favourites go with it.
fn remove_host(
    hosts: &mut Hosts,
    store: &config::Store,
    recents: &mut Vec<config::Recent>,
    favourites: &mut Vec<config::Favourite>,
    open_game: &mut Option<config::OpenGame>,
    id: u64,
) {
    if let Some(unique_id) = hosts.get(id).map(|h| h.record.unique_id.clone()).filter(|id| !id.is_empty()) {
        (*recents, *favourites) = store.forget_host(&unique_id, recents, favourites);
        *open_game = store.open_play().game;
    }
    hosts.remove(id);
    save_hosts(store, hosts);
}

/// A stream start the PIN lock holds until the PIN is entered.
enum PinPending {
    Launch { host: u64, app: i32, resume: bool, kind: LaunchKind, from: LaunchFrom },
    QuitAndLaunch { host: u64, app: i32, named: Option<i32> },
    /// Continue here's press, and with it the stream start it leads to.
    ContinueHere { host: u64, app: i32 },
}

/// What asked for a launch. The log names it, so it shows what had the PC start or replace a game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LaunchFrom {
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
    fn as_str(self) -> &'static str {
        match self {
            LaunchFrom::Launcher => "launcher",
            LaunchFrom::Replace => "launcher, quit and start",
            LaunchFrom::Autostream => "autostream",
            LaunchFrom::CommandLine => "command line",
        }
    }
}

/// What a launch starts besides an ordinary stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LaunchKind {
    Stream,
    /// Input-only through a small ordinary stream, on a PC without its own mode.
    SmallInputOnly,
    /// Only this device's microphone, on a PC that lists `Feature::MicOnly`.
    MicOnly,
    /// A resume of the game the PC runs, also another device's (`LaunchPlan::join`).
    JoinRunning,
}

impl LaunchKind {
    fn log_suffix(self) -> &'static str {
        match self {
            LaunchKind::Stream => "",
            LaunchKind::SmallInputOnly => ", input only through a small stream",
            LaunchKind::MicOnly => ", microphone only",
            LaunchKind::JoinRunning => ", joining the running game",
        }
    }

    /// Stream here too: a game selected by its tile launches beside the running one as it always
    /// did; a resume would stop here, since the PC reports the game to its owner only.
    fn stream_here_too(resume: bool) -> Self {
        if resume { LaunchKind::JoinRunning } else { LaunchKind::Stream }
    }
}

/// What a start does once the PC answered who plays.
#[derive(Debug, PartialEq, Eq)]
enum SelectStep {
    /// Another device plays this game: "X is playing on Y", with this title and device.
    Ask(String, String),
    Join,
    Start,
}

fn select_step(resume: bool, too: bool, playing: Option<(String, String)>, press_failed: bool) -> SelectStep {
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
fn mic_only_settings(settings: &mut config::Settings) {
    settings.microphone = true;
    settings.controllers = false;
    settings.play_audio_on_host = true;
    settings.clipboard_sync = false;
    settings.usb_forwarding = false;
}

/// The microphone-only screen's title. An unavailable microphone says so rather than "muted";
/// `busy`: the PC plays another device's microphone for now.
fn mic_only_title(muted: bool, unavailable: bool, busy: bool) -> &'static str {
    if unavailable {
        strings::en("Microphone unavailable")
    } else if muted {
        strings::en("Microphone muted")
    } else if busy {
        strings::en("Microphone in use on another device")
    } else {
        strings::en("Microphone on")
    }
}

/// The app id a microphone-only launch sends: it names no app, and the PC starts none.
const MIC_ONLY_APP_ID: i32 = 0;

/// The app a microphone-only session stands for in the stream's title and logs.
fn mic_only_app() -> nightjar_core::http::App {
    nightjar_core::http::App {
        name: tr("Microphone only").to_owned(),
        id: MIC_ONLY_APP_ID,
        hdr_supported: false,
        uuid: String::new(),
        host_index: None,
        art_version: String::new(),
        control: nightjar_core::http::AppControl::None,
        monitor_resume: false,
    }
}

fn launch_line(from: LaunchFrom, app_id: i32, resume: bool, busy: bool) -> String {
    format!(
        "launch from {}: app {app_id}{}{}",
        from.as_str(),
        if resume { ", resume" } else { "" },
        if busy { ", ignored: another stream is starting" } else { "" }
    )
}

/// What a launch can do with a PC's app list as it is now.
enum LaunchApp {
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

fn launch_app(apps: &Apps, app_id: i32) -> LaunchApp {
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
fn launch_target(host: &hosts::Host, app_id: i32, resume: bool) -> LaunchApp {
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
        name: ui::at_sentence_start(&ui::running_name(host)),
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
fn read_list_for_launch(hosts: &mut Hosts, host: u64) -> bool {
    hosts.load_apps(host);
    matches!(hosts.get(host).map(|h| &h.apps), Some(Apps::Loading))
}

/// Opens the dialog choosing where the shared folder browser of `host` saves a download, or
/// what it uploads. One dialog at a time: with one open, the page says so.
fn start_folder_pick(
    slot: &mut Option<(FolderPick, sdl::FileChoice)>,
    folders: &mut folders_page::SharedFolders,
    host: u64,
    pick: impl FnOnce(folders_page::Place) -> FolderPick,
    choose: impl FnOnce() -> sdl::FileChoice,
) {
    // Only the browser's own page asks.
    let Some(at) = folders.state_for(host).map(|open| open.place.clone()) else { return };
    if slot.is_some() {
        folders.transfer_refused(host, tr("A file dialog is already open. Finish it first.").to_owned());
        return;
    }
    *slot = Some((pick(at), choose()));
}

/// Starts what a dialog of the shared folder browser chose; the message to show, if any.
fn finish_folder_pick(
    folders: &mut folders_page::SharedFolders,
    pick: FolderPick,
    result: Result<Option<PathBuf>, String>,
) -> Option<(String, String)> {
    let chosen = match result {
        Err(e) => return Some((tr("File dialog unavailable").to_owned(), e)),
        // Cancelled.
        Ok(None) => return None,
        Ok(Some(chosen)) => chosen,
    };
    let taken = match pick {
        FolderPick::Download { host, at, name, size } => folders.download(host, &at, &name, size, chosen),
        FolderPick::Upload { host, at } => folders.upload(host, &at, chosen),
    };
    (!taken).then(|| {
        (
            tr("Transfer not started").to_owned(),
            tr("The shared folders were closed while the file dialog was open. Open them again to transfer the file.").to_owned(),
        )
    })
}

// Test only, and declared down here: `strings.rs` reads this file up to its first test item.
#[cfg(test)]
mod shot;

#[cfg(test)]
mod tests {
    use super::{first_frame_line, window_needs_a_frame};

    /// A PC, game or profile that turns the compute queue on asks for it before the renderer, as HDR does.
    #[test]
    fn any_layer_turning_on_the_compute_queue_asks_for_it() {
        let settings = crate::config::Settings::default();
        let none: Vec<crate::config::Overrides> = vec![crate::config::Overrides::default()];
        assert!(!super::any_async_compute_asked(&settings, &none));
        let own: crate::config::Overrides = serde_json::from_str("{\"pyrowave_async_compute\": true}").unwrap();
        assert!(super::any_async_compute_asked(&settings, &[own]));
        let global = crate::config::Settings { pyrowave_async_compute: true, ..Default::default() };
        assert!(super::any_async_compute_asked(&global, &none));
    }

    /// A launcher at rest presents nothing, but what the window itself lost has to come back.
    #[test]
    fn the_window_changing_asks_for_a_frame_and_a_worker_or_the_pointer_does_not() {
        for kind in [SDL_EVENT_WINDOW_EXPOSED, SDL_EVENT_WINDOW_RESIZED, SDL_EVENT_WINDOW_PIXEL_SIZE_CHANGED, SDL_EVENT_WINDOW_RESTORED, SDL_EVENT_RENDER_TARGETS_RESET, SDL_EVENT_RENDER_DEVICE_LOST] {
            assert!(window_needs_a_frame(kind), "{}", kind.0);
        }
        for kind in [SDL_EVENT_MOUSE_MOTION, SDL_EVENT_USER, SDL_EVENT_KEY_DOWN] {
            assert!(!window_needs_a_frame(kind), "{}", kind.0);
        }
    }

    #[test]
    fn the_launch_line_comes_once_with_the_time() {
        let said = std::sync::atomic::AtomicBool::new(false);
        assert_eq!(first_frame_line(&said, None), None, "no start, no number");
        let line = first_frame_line(&said, Some(std::time::Duration::from_millis(412)));
        assert_eq!(line.as_deref(), Some("nightjar: launch to first frame 412 ms"));
        assert_eq!(first_frame_line(&said, Some(std::time::Duration::from_secs(9))), None, "once");
    }

    /// Stream here too on a resume joins; a PC that did not answer never does.
    #[test]
    fn who_plays_decides_how_a_start_goes_on() {
        use super::{select_step, LaunchKind, SelectStep};
        let tv = || Some(("Game".to_owned(), "TV".to_owned()));
        // Resume on "came back", and a selected game.
        assert_eq!(select_step(true, false, tv(), false), SelectStep::Ask("Game".into(), "TV".into()));
        assert_eq!(select_step(true, false, tv(), true), SelectStep::Ask("Game".into(), "TV".into()), "it asks first");
        assert_eq!(select_step(false, false, tv(), true), SelectStep::Ask("Game".into(), "TV".into()));
        assert_eq!(select_step(true, false, None, true), SelectStep::Join);
        assert_eq!(select_step(true, false, None, false), SelectStep::Start);
        assert_eq!(select_step(false, false, None, true), SelectStep::Start, "a plain start launches beside it");
        // Stream here too on "did not move".
        assert_eq!(select_step(true, true, tv(), false), SelectStep::Join);
        assert_eq!(select_step(true, true, None, true), SelectStep::Join);
        assert_eq!(select_step(true, true, None, false), SelectStep::Start);
        assert_eq!(LaunchKind::stream_here_too(true), LaunchKind::JoinRunning);
        assert_eq!(LaunchKind::stream_here_too(false), LaunchKind::Stream);
    }

    /// Vulkan video's renderer is left out only when every decoder setting a stream can use says
    /// Software: a host's, an app's or a profile's override counts as the global setting does.
    #[test]
    fn hardware_decoding_counts_every_override_of_the_decoder_setting() {
        use super::{any_hardware_decoding, config::{DecoderPreference, Overrides, Settings}};
        let decoder = |decoder| {
            let mut own = Overrides::default();
            own.capture(&Settings { decoder, ..Default::default() }, &["decoder"]);
            own
        };
        let (software, auto) = (Settings { decoder: DecoderPreference::Software, ..Default::default() }, Settings::default());
        let other = Overrides::default();
        assert!(!any_hardware_decoding(&software, []), "Software everywhere");
        assert!(!any_hardware_decoding(&software, [&other, &decoder(DecoderPreference::Software)]), "overrides that leave it Software");
        assert!(any_hardware_decoding(&software, [&other, &decoder(DecoderPreference::Auto)]), "a host that decodes in hardware");
        assert!(any_hardware_decoding(&auto, []));
        assert!(any_hardware_decoding(&auto, [&decoder(DecoderPreference::Software)]), "other hosts still use the global setting");
    }

    /// The GPU renderer's device gets Vulkan video's extensions and decode queues, or the DMA-BUF
    /// import ones VAAPI's pictures are mapped with, only when a stream can use them: on Linux,
    /// with hardware decoding somewhere and `NIGHTJAR_VULKAN_VIDEO` or `NIGHTJAR_VAAPI_VULKAN_MAP`
    /// not 0. Otherwise it is made as before Vulkan video (PyroWave's plan alone).
    #[test]
    fn the_renderer_asks_for_vulkan_video_only_when_a_stream_can_decode_with_it() {
        use super::{vulkan_video::Choice, vulkan_video_possible};
        for choice in [Choice::Default, Choice::First] {
            for map in [false, true] {
                assert_eq!(vulkan_video_possible(true, choice, map), cfg!(unix), "{choice:?}");
                assert!(!vulkan_video_possible(false, choice, map), "{choice:?} with Software everywhere");
            }
        }
        assert!(!vulkan_video_possible(true, Choice::Off, false), "both switched off");
        assert_eq!(vulkan_video_possible(true, Choice::Off, true), cfg!(unix), "NIGHTJAR_VULKAN_VIDEO=0 still maps VAAPI's pictures");
    }

    #[test]
    fn input_only_through_a_small_stream_asks_for_the_smallest_stream_and_leaves_the_pc_alone() {
        let mut settings = config::Settings { width: 3840, height: 2160, fps: 120, bitrate_kbps: 80_000, vrr: true,
            virtual_display: true, audio: config::AudioChannels::Surround71, ..Default::default() };
        settings.prefer_yuv444 = true;
        settings.sdr_10bit = true;
        settings.upscaling = config::Upscaling::Fsr1;
        super::small_stream(&mut settings);
        assert!(!settings.prefer_yuv444 && !settings.sdr_10bit, "a stream nobody sees offers no 4:4:4 or 10-bit");
        assert_eq!(settings.upscaling, config::Upscaling::Bilinear, "nor upscaling");
        assert_eq!((settings.width, settings.height, settings.fps, settings.bitrate_kbps), (640, 360, 30, 500));
        assert_eq!(settings.audio, config::AudioChannels::Stereo);
        assert!(!settings.virtual_display && !settings.vrr && settings.play_audio_on_host, "the PC's display and speakers stay as they are");
    }

    /// Input-only and microphone-only sessions need no GPU renderer: Linux neither switches to it
    /// for them nor refuses them with Renderer set to OpenGL.
    #[test]
    fn a_session_nobody_sees_needs_no_gpu_renderer() {
        let mut settings = config::Settings { hdr: true, prefer_yuv444: true, sdr_10bit: true, experimental_packed_444: true,
            upscaling: config::Upscaling::Fsr1, codec: config::CodecPreference::PyroWave, ..Default::default() };
        assert!(settings.needs_gpu());
        super::input_only_settings(&mut settings);
        assert_eq!(settings.gpu_need(), None);
        assert_eq!((settings.codec, settings.width, settings.height, settings.fps), (config::CodecPreference::H264, 1280, 720, 60));
    }

    #[test]
    fn microphone_only_sends_the_microphone_and_names_no_app() {
        let mut settings = config::Settings { microphone: false, controllers: true, clipboard_sync: true, ..Default::default() };
        super::mic_only_settings(&mut settings);
        assert!(settings.microphone, "the microphone goes, whatever the stream setting says");
        assert!(!settings.controllers && !settings.clipboard_sync && !settings.usb_forwarding);
        assert!(settings.play_audio_on_host, "the PC keeps its sound");
        let app = super::mic_only_app();
        assert_eq!(app.id, super::MIC_ONLY_APP_ID);
        assert!(!app.is_control_action());
        assert_eq!(super::LaunchKind::MicOnly.log_suffix(), ", microphone only");
        assert_eq!(super::LaunchKind::JoinRunning.log_suffix(), ", joining the running game");
        assert_eq!(super::mic_only_title(false, false, false), "Microphone on");
        assert_eq!(super::mic_only_title(true, false, false), "Microphone muted");
        // A failed microphone is not called muted, also once it was muted for the failure.
        assert_eq!(super::mic_only_title(true, true, false), "Microphone unavailable");
        // The PC plays another device's microphone; muted says more.
        assert_eq!(super::mic_only_title(false, false, true), "Microphone in use on another device");
        assert_eq!(super::mic_only_title(true, false, true), "Microphone muted");
    }

    /// Holding Ctrl+Alt+Shift+H or Escape doesn't flip the sheet or the menu with each repeat,
    /// and Back acts on the page that is drawn: the Session page before a sheet it hides.
    #[test]
    fn menu_keys_ignore_repeats_and_back_leaves_the_drawn_page() {
        use super::{menu_step, MenuStep};
        use shortcuts::Action;
        let sheet = Some(Action::StreamShortcuts);
        let controls = Some(Action::StreamControls);
        assert_eq!(menu_step(sheet, false, false, false, false, false), MenuStep::ToggleSheet);
        assert_eq!(menu_step(sheet, false, true, true, false, false), MenuStep::Swallow);
        assert_eq!(menu_step(controls, false, true, false, false, false), MenuStep::Swallow);
        assert_eq!(menu_step(None, true, true, false, true, false), MenuStep::Swallow, "a held Escape");
        assert_eq!(menu_step(None, true, false, true, true, false), MenuStep::CloseSessionPage);
        assert_eq!(menu_step(controls, false, false, false, true, false), MenuStep::CloseSessionPage);
        assert_eq!(menu_step(None, true, false, true, false, false), MenuStep::CloseSheet);
        assert_eq!(menu_step(controls, false, false, true, false, false), MenuStep::CloseMenu);
        assert_eq!(menu_step(None, true, false, false, false, false), MenuStep::CloseMenu);
        assert_eq!(menu_step(Some(Action::SendClipboard), false, true, false, false, false), MenuStep::Pass);
        assert_eq!(menu_step(None, false, false, false, false, false), MenuStep::Pass);
        // Back from a page a tile opened goes to the grid; the sheet and the Session page draw over it first.
        assert_eq!(menu_step(None, true, false, false, false, true), MenuStep::ClosePage);
        assert_eq!(menu_step(None, true, false, true, false, true), MenuStep::CloseSheet);
        assert_eq!(menu_step(None, true, false, false, true, true), MenuStep::CloseSessionPage);
        assert_eq!(menu_step(controls, false, false, false, false, true), MenuStep::CloseMenu, "the controls chord closes from any page");
    }

    #[test]
    fn the_overlay_sits_in_its_corner() {
        use config::OverlayCorner;
        assert_eq!(super::overlay_anchor(OverlayCorner::TopLeft), (egui::Align2::LEFT_TOP, egui::vec2(16.0, 16.0)));
        assert_eq!(super::overlay_anchor(OverlayCorner::BottomRight), (egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -16.0)));
        assert_eq!(super::overlay_anchor(OverlayCorner::TopRight).0, egui::Align2::RIGHT_TOP);
        assert_eq!(super::overlay_anchor(OverlayCorner::BottomLeft).0, egui::Align2::LEFT_BOTTOM);
    }

    use super::*;
    use std::sync::Arc;

    #[test]
    fn exclusive_fullscreen_keeps_a_vrr_panels_top_refresh() {
        // A 240 Hz panel whose desktop runs at 120 Hz.
        let modes = [240.0, 165.0, 144.0, 120.0, 60.0, 120.0, 60.0];
        assert_eq!(fullscreen_refresh_hz(true, &modes, 60), 240.0, "a 60 fps stream gets the 240 Hz mode, not the desktop's 120");
        assert_eq!(fullscreen_refresh_hz(false, &modes, 60), 60.0, "without VRR a mode at the stream's rate");
        assert_eq!(fullscreen_refresh_hz(true, &[], 120), 120.0, "no modes known");
    }

    #[test]
    fn native_resolution_is_the_display_in_pixels_at_any_scaling() {
        // Windows and X11: SDL gives pixels at density 1 at any scaling (4K at 200% and 175%).
        assert_eq!(mode_pixels(3840, 2160, 1.0), (3840, 2160));
        // Wayland at 200%, and at 175% through fractional scaling (3840 / 2194 points).
        assert_eq!(mode_pixels(1920, 1080, 2.0), (3840, 2160));
        assert_eq!(mode_pixels(2194, 1234, 3840.0 / 2194.0), (3840, 2160));
        // macOS: a 14" MacBook Pro looks like 1512x982 at density 2.
        assert_eq!(mode_pixels(1512, 982, 2.0), (3024, 1964));
        // No density reported counts as 1.
        assert_eq!(mode_pixels(2560, 1440, 0.0), (2560, 1440));
    }

    // Regression (review): a display without a reported rate paced at the 60 Hz fallback, and the
    // overlay judged frames against it and could say "60 Hz screen".
    #[test]
    fn the_overlay_only_gets_a_refresh_rate_the_display_reported() {
        let reported = display_from_mode(Some((2560, 1440, 1.0, 143.9)));
        assert_eq!((reported.refresh_hz, reported.reported_refresh_hz()), (143.9, Some(143.9)));
        for refresh in [0.0, -1.0, f32::NAN] {
            let fallback = display_from_mode(Some((2560, 1440, 1.0, refresh)));
            assert_eq!((fallback.refresh_hz, fallback.reported_refresh_hz()), (60.0, None), "pacing keeps 60 Hz");
        }
        let none = display_from_mode(None);
        assert_eq!((none.width, none.refresh_hz, none.reported_refresh_hz()), (1920, 60.0, None));
    }

    #[test]
    fn the_decoder_checks_run_again_when_vulkan_video_first_changes() {
        let mut settings = config::Settings::default();
        let mut cache = None;
        let mut runs = 0;
        let mut probe = |settings: &config::Settings| probed_once(&mut cache, probe_key(Hardware::Vaapi, 1, settings), || { runs += 1; runs });
        assert_eq!(probe(&settings), 1);
        settings.vulkan_rfi = true;
        assert_eq!(probe(&settings), 1, "recovery does not change what decodes");
        settings.vulkan_video_first = true;
        assert_eq!(probe(&settings), 2, "the backend order the check decodes through changed");
        assert_eq!(probe(&settings), 2);
    }

    #[test]
    fn the_color_probe_runs_once_per_decoder() {
        let mut cache = None;
        let mut runs = 0;
        let mut probe = |hardware| probed_once(&mut cache, hardware, || { runs += 1; (runs, format!("run {runs}")) });
        assert_eq!(probe(Hardware::None), (1, "run 1".to_owned()));
        assert_eq!(probe(Hardware::None), (1, "run 1".to_owned()), "the same decoder is not checked again");
        assert_eq!(probe(Hardware::Vaapi), (2, "run 2".to_owned()), "another decoder is");
        assert_eq!(probe(Hardware::None), (3, "run 3".to_owned()));
    }

    #[test]
    fn a_stream_starts_with_what_became_of_4_4_4_and_10_bit_next_to_the_warning() {
        assert_eq!(start_toast(None, None, false), "Ctrl+Alt+Shift+Tab opens controls, Ctrl+Alt+Shift+Q disconnects, Ctrl+Alt+Shift+Z releases input");
        // It names the key the layout has there: the key labelled Y on QWERTZ.
        shortcuts::use_layout(&[(sdl3_sys::everything::SDL_SCANCODE_Z, sdl3_sys::everything::SDLK_Y)]);
        assert_eq!(start_toast(None, None, false), "Ctrl+Alt+Shift+Tab opens controls, Ctrl+Alt+Shift+Q disconnects, Ctrl+Alt+Shift+Y releases input");
        shortcuts::use_layout(&[]);
        // The hint follows the hotkeys as bound.
        shortcuts::record_hotkey("quit", &["CTRL".into(), "F9".into()]).unwrap();
        shortcuts::turn_off_hotkey("capture").unwrap();
        assert_eq!(start_toast(None, None, false), "Ctrl+Alt+Shift+Tab opens controls, Ctrl+F9 disconnects");
        shortcuts::set_hotkeys(Default::default());
        assert_eq!(start_toast(None, Some("Streaming in YUV 4:4:4."), false), "Streaming in YUV 4:4:4.");
        assert_eq!(start_toast(Some("Wi-Fi name unavailable."), Some("Streaming in YUV 4:4:4."), false), "Wi-Fi name unavailable.\nStreaming in YUV 4:4:4.");
        assert_eq!(start_toast(Some("Wi-Fi name unavailable."), None, false), "Wi-Fi name unavailable.");
    }

    #[test]
    fn under_gamescope_the_launcher_window_opens_borderless_fullscreen() {
        let deck = start_flags(handheld::start_window(true));
        assert!(deck.0 & SDL_WINDOW_FULLSCREEN.0 != 0 && deck.0 & SDL_WINDOW_BORDERLESS.0 != 0);
        assert!(deck.0 & SDL_WINDOW_HIDDEN.0 != 0, "shown after the GUI is ready");
        let desktop = start_flags(handheld::start_window(false));
        assert_eq!(desktop.0, (SDL_WINDOW_RESIZABLE | SDL_WINDOW_HIGH_PIXEL_DENSITY | SDL_WINDOW_HIDDEN).0);
        assert_eq!(stream_window_mode(WindowMode::Fullscreen, true), WindowMode::Borderless, "no exclusive mode under gamescope");
        assert_eq!(stream_window_mode(WindowMode::Fullscreen, false), WindowMode::Fullscreen);
        assert_eq!(stream_window_mode(WindowMode::Windowed, true), WindowMode::Windowed);
    }

    #[test]
    fn the_pointer_follows_real_controller_and_mouse_input_only() {
        // SAFETY: plain C unions, zeroed then filled by type.
        let event = |kind: SDL_EventType, fill: &dyn Fn(&mut SDL_Event)| unsafe {
            let mut e: SDL_Event = std::mem::zeroed();
            e.r#type = kind.0;
            fill(&mut e);
            e
        };
        let axis = |value: i16| event(SDL_EVENT_GAMEPAD_AXIS_MOTION, &|e| e.gaxis.value = value);
        let motion = |which: SDL_MouseID| event(SDL_EVENT_MOUSE_MOTION, &|e| e.motion.which = which);
        let button = |which: SDL_MouseID| event(SDL_EVENT_MOUSE_BUTTON_DOWN, &|e| e.button.which = which);
        assert_eq!(pointer_input(&event(SDL_EVENT_GAMEPAD_BUTTON_DOWN, &|_| {})), handheld::Input::Controller);
        assert_eq!(pointer_input(&axis(20000)), handheld::Input::Controller);
        assert_eq!(pointer_input(&axis(-20000)), handheld::Input::Controller);
        assert_eq!(pointer_input(&axis(1500)), handheld::Input::Other, "stick or trigger jitter");
        assert_eq!(pointer_input(&axis(-1500)), handheld::Input::Other);
        assert_eq!(pointer_input(&motion(SDL_MouseID(1))), handheld::Input::Mouse, "the trackpad");
        assert_eq!(pointer_input(&button(SDL_MouseID(1))), handheld::Input::Mouse);
        assert_eq!(pointer_input(&motion(SDL_TOUCH_MOUSEID)), handheld::Input::Other, "a touch tap");
        assert_eq!(pointer_input(&button(SDL_TOUCH_MOUSEID)), handheld::Input::Other);
        assert_eq!(pointer_input(&button(SDL_PEN_MOUSEID)), handheld::Input::Other);
        assert_eq!(pointer_input(&event(SDL_EVENT_MOUSE_WHEEL, &|e| e.wheel.which = SDL_TOUCH_MOUSEID)), handheld::Input::Other);
        assert_eq!(pointer_input(&event(SDL_EVENT_KEY_DOWN, &|_| {})), handheld::Input::Other);
    }

    #[test]
    fn under_gamescope_a_stream_starts_with_start_back_and_the_hdr_note() {
        assert_eq!(start_toast(None, None, true), "Start+Back opens the stream menu");
        assert_eq!(start_toast(None, Some(HDR_OFF_NOTE), true), HDR_OFF_NOTE, "a note replaces the hint");
        assert_eq!(shortcuts::stream_controls_mask(), 1 << SDL_GAMEPAD_BUTTON_START.0 | 1 << SDL_GAMEPAD_BUTTON_BACK.0,
            "the hint's chord is the one the stream opens its menu with");
    }

    #[test]
    fn a_launch_logs_what_asked_for_it() {
        assert_eq!(launch_line(LaunchFrom::Launcher, 5, false, false), "launch from launcher: app 5");
        assert_eq!(launch_line(LaunchFrom::Launcher, 5, true, false), "launch from launcher: app 5, resume");
        assert_eq!(
            launch_line(LaunchFrom::Replace, 7, false, true),
            "launch from launcher, quit and start: app 7, ignored: another stream is starting"
        );
        assert_eq!(launch_line(LaunchFrom::Autostream, 1, false, false), "launch from autostream: app 1");
        assert_eq!(launch_line(LaunchFrom::CommandLine, 2, true, false), "launch from command line: app 2, resume");
    }

    #[test]
    fn a_launch_waits_for_an_app_list_being_read_again_instead_of_dropping() {
        let app = nightjar_core::http::App {
            name: "Desktop".into(),
            id: 5,
            hdr_supported: false,
            uuid: String::new(),
            host_index: None,
            art_version: String::new(),
            control: nightjar_core::http::AppControl::None,
            monitor_resume: false,
        };
        assert!(matches!(launch_app(&Apps::Loading, 5), LaunchApp::Wait), "a reload in flight keeps the launch");
        assert!(matches!(launch_app(&Apps::Loaded(vec![app.clone()]), 5), LaunchApp::Ready(a) if a.id == 5));
        assert!(matches!(launch_app(&Apps::Loaded(vec![app]), 6), LaunchApp::Missing));
        assert!(matches!(launch_app(&Apps::NotLoaded, 5), LaunchApp::Gone));
        // A reload that ends in an error is shown, not dropped.
        assert!(matches!(launch_app(&Apps::Failed("x".into()), 5), LaunchApp::Failed(why) if why == "x"));
    }

    #[test]
    fn the_window_on_another_display_or_mode_is_told_apart() {
        assert!(another_display(SDL_EVENT_WINDOW_DISPLAY_CHANGED));
        assert!(another_display(SDL_EVENT_DISPLAY_CURRENT_MODE_CHANGED));
        assert!(!another_display(SDL_EVENT_WINDOW_MOVED) && !another_display(SDL_EVENT_WINDOW_RESIZED));
    }

    #[test]
    fn a_second_file_dialog_is_refused_on_the_page_and_a_late_choice_is_reported() {
        use folders_page::tests::{calls, settle, waker, Fake};
        let fake = Arc::new(Fake::default());
        let mut folders = folders_page::SharedFolders::default();
        folders.open(7, fake.clone(), waker());
        settle(&mut folders, 7);
        let mut slot = None;
        let (_sender, receiver) = std::sync::mpsc::channel();
        start_folder_pick(&mut slot, &mut folders, 7, |at| FolderPick::Upload { host: 7, at }, || receiver);
        assert!(slot.is_some());
        start_folder_pick(&mut slot, &mut folders, 7, |_| panic!("no second pick"), || panic!("no second dialog"));
        let said = folders.state_for(7).unwrap().transfer_error.clone();
        assert_eq!(said.as_deref(), Some("A file dialog is already open. Finish it first."));

        let (pick, _) = slot.take().unwrap();
        assert_eq!(finish_folder_pick(&mut folders, pick, Ok(None)), None, "cancelled");
        let pick = FolderPick::Download { host: 7, at: folders_page::Place::Folders, name: "a.txt".into(), size: 3 };
        let (title, body) = finish_folder_pick(&mut folders, pick, Err("no portal".into())).unwrap();
        assert_eq!((title.as_str(), body.as_str()), ("File dialog unavailable", "no portal"));

        // The page was left while the dialog was open.
        folders.poll(None);
        let at = folders_page::Place::Folders;
        let pick = FolderPick::Upload { host: 7, at };
        let (title, body) = finish_folder_pick(&mut folders, pick, Ok(Some(std::env::temp_dir()))).unwrap();
        assert_eq!(title, "Transfer not started");
        assert!(body.contains("closed while the file dialog was open"), "{body}");
        assert_eq!(calls(&fake), ["folders"]);
    }

    fn record(unique_id: &str) -> config::HostRecord {
        config::HostRecord {
            unique_id: unique_id.into(),
            name: format!("{unique_id} on the PC"),
            address: "192.0.2.7".into(),
            manual: true,
            ..Default::default()
        }
    }

    fn temp_store(tag: &str) -> (config::Store, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("nightjar-main-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        (config::Store::at(dir.clone()).unwrap(), dir)
    }

    #[test]
    fn a_name_chosen_for_a_pc_is_saved_and_comes_back_after_a_restart() {
        let (store, dir) = temp_store("rename");
        let mut hosts = Hosts::without_discovery(vec![record("a")], Arc::new(|| {}));
        let id = hosts.list[0].id;
        rename_host(&mut hosts, &store, id, "  Living room  ");
        assert_eq!(store.hosts()[0].alias.as_deref(), Some("Living room"), "saved trimmed");
        let restarted = Hosts::without_discovery(store.hosts(), Arc::new(|| {}));
        assert_eq!(restarted.list[0].display_name(), "Living room");
        rename_host(&mut hosts, &store, id, "");
        assert_eq!(store.hosts()[0].alias, None, "an empty name brings the PC's own back");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn removing_a_pc_leaves_nothing_of_it_behind() {
        let (store, dir) = temp_store("remove");
        let mut hosts = Hosts::without_discovery(vec![record("gone"), record("kept")], Arc::new(|| {}));
        let gone = hosts.list[0].id;
        let recent = |host: &str| config::Recent { host: host.into(), install_id: String::new(), app_id: 1, title: "G".into(), at: 1 };
        let mut recents = vec![recent("gone"), recent("kept")];
        let star = |host: &str| config::Favourite { host: host.into(), install_id: String::new(), app_id: 1, title: "G".into() };
        let mut favourites = vec![star("gone"), star("kept")];
        store.save_favourites(&favourites).unwrap();
        for host in ["gone", "kept"] {
            let cover = store.art_path(host, 1, "");
            std::fs::create_dir_all(cover.parent().unwrap()).unwrap();
            std::fs::write(cover, b"x").unwrap();
        }
        save_hosts(&store, &hosts);
        let game = config::OpenGame { host: "gone".into(), install_id: String::new(), app_id: 1, title: "G".into() };
        store.save_open_play(&config::OpenPlayState { game: Some(game.clone()), armed: false }).unwrap();
        let mut open_game = Some(game);
        remove_host(&mut hosts, &store, &mut recents, &mut favourites, &mut open_game, gone);
        assert_eq!(open_game, None, "the settings page and game menu forget its chosen game");
        assert_eq!(store.open_play().game, None);
        assert_eq!(store.hosts().len(), 1);
        assert_eq!(store.hosts()[0].unique_id, "kept");
        assert_eq!(recents.len(), 1);
        assert_eq!(store.recents().len(), 1, "recent.json is filtered");
        assert_eq!(favourites.len(), 1);
        assert_eq!(store.favourites()[0].host, "kept", "favourites.json is filtered");
        assert!(!store.art_path("gone", 1, "").exists());
        assert!(store.art_path("kept", 1, "").exists());
        std::fs::remove_dir_all(dir).ok();
    }
}
