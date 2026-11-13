//! Nightjar desktop: a Moonlight-protocol game-streaming client on
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
mod app;
mod autostart;
mod autostream;
mod cli;
mod clipboard;
mod config;
mod conn_test;
#[cfg(windows)]
mod crash_dump;
#[cfg(feature = "demo-library")]
mod demo_driver;
mod console;
mod coop_page;
mod folders_page;
mod full_chroma;
mod controller_audio;
mod controller_mouse;
mod dmabuf;
mod gamepad;
mod glyphs;
#[cfg(windows)]
mod gpu_adapters;
mod gui;
mod handheld;
mod host_page;
mod hosts;
mod input_source;
mod keymap;
mod kms;
mod live_glass;
mod logfile;
mod link_channel;
#[cfg(any(target_os = "linux", test))]
mod linux_diagnosis;
#[cfg(any(target_os = "linux", test))]
mod linux_video;
mod latency_bench;
mod logpipe;
// The display's refresh range is NSScreen's; its tests for a variable range run everywhere.
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod mac_display;
mod media;
mod mp4;
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
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
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
use stream::present::pacing;
use stream::{ConnectFailed, Connecting, Display, Exit, LaunchPlan, Streaming};
use strings::{tr, trf};
use ui::{Combo, Command, ConnectingView, Dialog, Launcher, Screen, View};
use app::*;

struct App {
    window: *mut SDL_Window,
    renderer: *mut SDL_Renderer,
    gui: Gui,
    /// The software renderer of a stream's overlay plane (`kms_scanout`) the gui's textures are
    /// on; null while they are on `renderer`.
    gui_overlay: *mut SDL_Renderer,
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
    /// Where Export latency CSV saves the last stream's measurements, being chosen.
    latency_csv_file: Option<sdl::FileChoice>,
    /// A folder to download into, or a file to upload, being chosen for the shared folder browser.
    folder_pick: Option<(FolderPick, sdl::FileChoice)>,
    /// The network read for a start, which waits for it.
    network_query: Option<Receiver<network::Network>>,
    /// The network as the launcher shows and uses it between starts.
    network_watch: network::Watch,
    /// What this PC's power allows, being asked once at start.
    power_probe: Option<Receiver<Vec<power::Offer>>>,
    /// A shut down, restart or sleep under way, and the system's answer to it.
    power_request: Option<(power::Offer, Receiver<Result<(), String>>)>,
    network_test: Option<network_test::Test>,
    /// Host, app, resume, and Input-only through a small stream.
    network_launch: Option<(u64, i32, bool, LaunchKind)>,
    /// The start a failed start's card offers to make again (`Command::RetryLaunch`).
    failed_launch: Option<(u64, i32, bool, LaunchKind)>,
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
    /// What the stream being started plays without, straight to the display without the GPU renderer
    /// (`kms::without_gpu`): its start says so.
    kms_notes: Vec<String>,
    /// Full chroma was asked for and the stream being started runs without it: its start says so.
    packed_note: Option<String>,
    /// The PC (by id) refused full chroma after this start's launch: the start made again for it
    /// runs without it (`full_chroma::decide`).
    full_chroma_off: Option<u64>,
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
    #[cfg(target_os = "linux")]
    renderer_state: RendererState,
    /// Why this stream's switch to the GPU renderer failed, for the check that refuses it.
    gpu_failure: Option<String>,
    /// The Graphics card setting last applied: a change is applied once, with nothing streaming.
    #[cfg(windows)]
    graphics_card: config::GraphicsCard,
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

/// Now on the session clock (`nightjar_core::session::now_us`), in nanoseconds, the clock
/// `stream::phase::timing` reports vblanks on.
fn session_now_ns() -> i64 {
    i64::try_from(nightjar_core::session::now_us()).unwrap_or(i64::MAX / 1000).saturating_mul(1000)
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
    #[cfg(all(target_os = "macos", feature = "demo-library"))]
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--check-metal")) {
        stream::check_metal();
        return;
    }
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
    let raw: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    // A start Nightjar made itself to move to another GPU goes on in the same log.
    #[cfg(windows)]
    let (raw, graphics_restarted) = gpu_adapters::split_restarted(raw);
    #[cfg(not(windows))]
    let graphics_restarted = false;
    let (args, after_setup) = self_update::split_after_setup(raw.clone());
    if self_update::install_running(after_setup) {
        // Its progress window is up, and it starts Nightjar when it ends.
        eprintln!("nightjar: the update is installing");
        std::process::exit(75);
    }
    let (args, at_sign_in) = autostart::split_sign_in(args);
    let (args, second_screen) = second_screen::split_flag(args);
    let (args, kms_flag) = kms::split_flag(args);
    kms::set_flag(kms_flag);
    let (args, second_screen_join) = second_screen::split_join(args);
    let usage_box = cli::shows_usage_box(cli::HAS_CONSOLE, &args);
    let (mut launch, link) = match cli::parse(args) {
        Ok(cli::Parsed::Gui) => (None, None),
        Ok(cli::Parsed::Launch(launch)) => (Some(launch), None),
        // moonlight-qt's `list`: the app names on stdout, then out, without a window or the log.
        Ok(cli::Parsed::List(list)) => {
            cli::use_parent_console();
            let code = match Store::open() {
                Ok(store) => cli::run_list(&list, &store),
                Err(e) => {
                    eprintln!("nightjar: no config directory: {e}");
                    cli::LIST_NO_PC
                }
            };
            std::process::exit(code);
        }
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
    logpipe::start(graphics_restarted);
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
    // The Graphics card setting moved Nightjar to another GPU, which Windows applies at a start.
    #[cfg(windows)]
    if let Some(replay) = gpu_adapters::restart_requested() {
        logpipe::shutdown();
        if let Err(e) = gpu_adapters::restart(if replay { &raw } else { &[] }) {
            message_box(SDL_MESSAGEBOX_ERROR, &format!("Nightjar could not start again: {e}"));
        }
    }
}

fn message_box(flags: SDL_MessageBoxFlags, text: &str) {
    let text = std::ffi::CString::new(text).unwrap_or_default();
    // SAFETY: NUL-terminated strings; a null parent window is allowed.
    unsafe { SDL_ShowSimpleMessageBox(flags, c"Nightjar".as_ptr(), text.as_ptr(), std::ptr::null_mut()) };
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
    #[cfg(target_os = "linux")]
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

    // Straight to the display (KMS) or through the desktop, decided before SDL picks its video driver.
    #[cfg(target_os = "linux")]
    {
        let setting = Store::open().map(|store| store.read_only().settings().display_output).unwrap_or_default();
        let session = kms::Session::read(|name| std::env::var_os(name), std::path::Path::exists);
        let driver = std::env::var("SDL_VIDEO_DRIVER").ok();
        let decision = kms::decide(kms::flag(), setting, session, driver.as_deref());
        eprintln!("nightjar: kms: {} ({})", if decision.backend == kms::Backend::Kms { "drawing straight to the display" } else { "drawing through the desktop" }, decision.why);
        if let Some(drivers) = decision.driver_hint {
            let drivers = std::ffi::CString::new(drivers).expect("driver names have no NUL");
            sdl::set_hint(SDL_HINT_VIDEO_DRIVER, &drivers);
        }
        kms::set(decision.backend);
    }
    #[cfg(not(target_os = "linux"))]
    if let Some(flag) = kms::flag() {
        eprintln!("nightjar: kms: {} is ignored: only Linux draws straight to the display", if flag { kms::KMS_ARG } else { kms::NO_KMS_ARG });
    }
    // The second screen's stream takes no controllers: they stay with the first.
    let gamepads = if second_screen::this_process() { SDL_InitFlags::default() } else { SDL_INIT_GAMEPAD };
    let subsystems = SDL_INIT_VIDEO | gamepads | SDL_INIT_EVENTS;
    // On macOS the first stream opens Core Audio. Enumerating a cold audio driver
    // can otherwise hold the Cocoa thread before the launcher has drawn a window.
    #[cfg(not(target_os = "macos"))]
    let subsystems = subsystems | SDL_INIT_AUDIO;
    // SAFETY: SDL initialisation on the main thread.
    unsafe {
        if !SDL_Init(subsystems) {
            #[cfg(target_os = "linux")]
            {
                if kms::active() {
                    let index = std::env::var("SDL_KMSDRM_DEVICE_INDEX").ok().and_then(|i| i.trim().parse().ok());
                    return Err(kms::start_failure(&sdl::error(), kms::drm::connected_displays(index)));
                }
                let session = kms::Session::read(|name| std::env::var_os(name), std::path::Path::exists);
                if !session.any() {
                    return Err(kms::desktop_failure(&sdl::error()));
                }
            }
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
    #[cfg(target_os = "macos")]
    let gpu = false; // Start the launcher in SDR. Its stream chooses its own Metal path.
    #[cfg(not(target_os = "macos"))]
    let gpu = settings.needs_gpu() || hosts.iter()
        .any(|host| host.settings.as_ref().is_some_and(config::Overrides::needs_gpu) || host.app_settings.values().any(config::Overrides::needs_gpu))
        || profiles.entries.iter().any(|profile| profile.settings.needs_gpu());
    #[cfg(target_os = "linux")]
    let vulkan_choice = vulkan_video::Choice::from_environment();
    #[cfg(target_os = "linux")]
    let vulkan_video = vulkan_video_possible(any_hardware_decoding(&settings, hosts.iter()
        .flat_map(|host| host.settings.iter().chain(host.app_settings.values()))
        .chain(profiles.entries.iter().map(|profile| &profile.settings))), vulkan_choice, vaapi_map_switched_on());
    // The Renderer setting decides, and at Auto what needs the GPU renderer.
    #[cfg(target_os = "linux")]
    let gpu = handheld::starts_on_gpu(settings.renderer, settings.renderer == config::Renderer::Auto && (gpu || gpu_for_vulkan_video(vulkan_video, vulkan_choice)));
    // Straight to the display, OpenGL keeps DRM master where the video planes can share it.
    #[cfg(target_os = "linux")]
    let gpu = match kms::active().then(|| kms::renderer_for(settings.renderer, settings.video_planes)).flatten() {
        Some(gpu) => gpu,
        None => gpu,
    };
    #[cfg(not(target_os = "linux"))]
    let vulkan_video = false;

    // The GPU before the renderer: Windows applies Nightjar's graphics preference at a start, so
    // a preference that changed now ends this start for a new one (once).
    #[cfg(windows)]
    if !second_screen::this_process()
        && gpu_adapters::choose_at_start(settings.graphics_card, any_yuv444_asked(&store, &settings), store.dir())
    {
        return Ok(());
    }
    let hdr_asked = any_hdr_asked(&settings, hosts.iter()
        .flat_map(|host| host.settings.iter().chain(host.app_settings.values()))
        .chain(profiles.entries.iter().map(|profile| &profile.settings)));
    sdl::set_hdr_asked(hdr_asked);
    // Before the renderer: on Linux its device gets PyroWave's compute queue only with this on.
    stream::pyrowave_sdk::queue::use_async_compute(any_async_compute_asked(&settings, hosts.iter()
        .flat_map(|host| host.settings.iter().chain(host.app_settings.values()))
        .chain(profiles.entries.iter().map(|profile| &profile.settings))));

    // Steam's Game Mode, and straight to the display: fullscreen without a border from the start
    // (handheld.rs), at the display's current mode in KMS mode.
    let start = handheld::start_window(handheld::fills_screen());
    if handheld::gamescope() {
        eprintln!("nightjar: gamescope: starting fullscreen without a border; Start+Back opens the stream menu");
    }
    // SAFETY: display queries on the main thread after SDL_Init; the mode is SDL's and read at once.
    let screen = unsafe {
        let mode = SDL_GetDesktopDisplayMode(SDL_GetPrimaryDisplay());
        (!mode.is_null()).then(|| ((*mode).w, (*mode).h))
    };
    let (width, height) = handheld::start_size(kms::active(), screen);
    // SAFETY: window and renderer creation on the main thread.
    let (window, renderer, wake_event) = unsafe {
        let window = SDL_CreateWindow(
            c"Nightjar".as_ptr(),
            width,
            height,
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
    // The System theme's light or dark; SDL tells of a change (SDL_EVENT_SYSTEM_THEME_CHANGED).
    launcher.system_light = sdl::system_light();
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
    #[cfg(target_os = "linux")]
    {
        launcher.tearing_supported = true;
    }
    #[cfg(target_os = "macos")]
    {
        // SAFETY: SDL's live window, on the main thread.
        let timing = unsafe {
            mac_display::of_window(SDL_GetPointerProperty(SDL_GetWindowProperties(window), SDL_PROP_WINDOW_COCOA_WINDOW_POINTER, std::ptr::null_mut()))
        };
        let variable = timing.is_some_and(|timing| timing.variable_refresh());
        match timing {
            Some(timing) => {
                let (slowest, fastest) = timing.range_hz();
                eprintln!("nightjar: the display refreshes at {slowest:.0} to {fastest:.0} Hz{}", if variable { ", a variable refresh rate" } else { "" });
            }
            None => eprintln!("nightjar: the display reports no refresh range"),
        }
        // Real VRR on a Mac is built and was not seen on a display: `NIGHTJAR_MAC_VRR=1` turns
        // it on for a display with a refresh range, where a stream then presents synchronised at
        // the PC's frame times (`stream::present::sync_interval`).
        launcher.tearing_supported = variable && stream::env_flag("NIGHTJAR_MAC_VRR") == Some(true);
        if launcher.tearing_supported { eprintln!("nightjar: Real VRR on this display (NIGHTJAR_MAC_VRR)"); }
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
    #[cfg(windows)]
    if let Some(gpu) = gpu_adapters::renderer_gpu(sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_DEVICE_POINTER)) {
        eprintln!("nightjar: the renderer draws and D3D11VA decodes on {gpu}");
    }

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
        gui_overlay: std::ptr::null_mut(),
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
        latency_csv_file: None,
        folder_pick: None,
        network_query: None,
        network_watch: network::Watch::start(waker.clone()),
        power_probe: Some(power::probe(waker.clone())),
        power_request: None,
        network_test: None,
        network_launch: None,
        failed_launch: None,
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
        kms_notes: Vec::new(),
        packed_note: None,
        full_chroma_off: None,
        running: true,
        autostream: autostream::Autostream::from_env(),
        updates,
        open_play,
        cli,
        second_screen: None,
        color_probed: None,
        base_probed: None,
        #[cfg(target_os = "linux")]
        renderer_state: RendererState::for_launcher(
            settings.renderer,
            vulkan_video,
            (gpu && sdl::renderer_name(renderer) != "gpu")
                .then(|| sdl::gpu_renderer_failure().unwrap_or_else(|| format!("SDL made {:?} instead", sdl::renderer_name(renderer)))),
        ),
        gpu_failure: None,
        #[cfg(windows)]
        graphics_card: settings.graphics_card,
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
            // The stream watches the network itself (`RoamWatch`), and the launcher's pages are
            // not on screen to show it.
            app.network_watch.pace(network::Pace::Paused);
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

/// A Wayland window learns its HDR state after it is mapped: the log says when, launcher or stream.
fn log_window_hdr(event: &SDL_Event) {
    static LAST: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);
    // SAFETY: union members read by event type.
    unsafe {
        if SDL_EventType(event.r#type) == SDL_EVENT_WINDOW_HDR_STATE_CHANGED
            && LAST.swap(event.window.data1, std::sync::atomic::Ordering::Relaxed) != event.window.data1 {
            eprintln!("nightjar: the compositor reports window HDR {} ({})", if event.window.data1 != 0 { "on" } else { "off" }, sdl::window_hdr_levels(event.window.windowID));
        }
    }
}

/// The launcher window's flags for how it opens (handheld.rs).
fn start_flags(start: handheld::StartWindow) -> SDL_WindowFlags {
    SDL_WINDOW_RESIZABLE | SDL_WINDOW_HIGH_PIXEL_DENSITY | SDL_WINDOW_HIDDEN
        | if start.fullscreen { SDL_WINDOW_FULLSCREEN } else { SDL_WindowFlags(0) }
        | if start.borderless { SDL_WINDOW_BORDERLESS } else { SDL_WindowFlags(0) }
}

// Test only, and declared down here: `strings.rs` reads this file up to its first test item.
#[cfg(test)]
mod shot;

#[cfg(test)]
mod tests;
