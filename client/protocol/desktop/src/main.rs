//! Nightjar desktop: a Moonlight-protocol game-streaming client on
//! nightjar-core. Copyright (C) 2026 RamazanKara, GPL-3.0-only.
//!
//! One SDL3 window and renderer serve the launcher (egui) and the stream (the decoded
//! video, with the overlay drawn by egui on top). The UI thread owns SDL; host requests,
//! the session, decoding and audio run on their own threads and wake the UI with an SDL
//! user event.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod a11y;
mod app;
mod autostart;
mod autostream;
mod cli;
mod conn_test;
#[cfg(windows)]
mod crash_dump;
#[cfg(feature = "demo-library")]
mod demo_driver;
mod font_loader;
mod game_info;
mod gui;
mod hosts;
mod hotkey_sdl;
mod input_source;
mod link_channel;
mod live_glass;
mod logfile;
mod logpipe;
mod move_watch;
mod network_test;
mod open_play;
mod page_hosts;
mod pin_clock;
mod power;
mod self_update;
mod startup;
mod update_check;
mod update_net;
mod window_place;
mod wol;

mod crates;
pub use crates::*;

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
pub(crate) use startup::blend_title_bar;
use startup::*;

struct App {
    window: *mut SDL_Window,
    renderer: *mut SDL_Renderer,
    /// Windows: D3D11 device-loss recovery, remake guard, lost devices, and deferred reconnect.
    #[cfg(windows)]
    recovery: render_device::Recovery,
    gui: Gui,
    /// The software renderer of a stream's overlay plane (`kms_scanout`) the gui's textures are
    /// on; null while they are on `renderer`.
    gui_overlay: *mut SDL_Renderer,
    store: Store,
    host_store: nightjar_core::hosts::HostStore,
    identity_store: nightjar_core::hosts::IdentityStore,
    hosts: Hosts,
    launcher: Launcher,
    latency_bench: latency_bench::Bench,
    pads: Gamepads,
    pad_poll: PadPoll,
    /// The starts connecting and those just cancelled or timed out, whose workers may still run.
    attempts: stream::Attempts,
    /// A start's decoder checks, on a worker thread (`LaunchProbe`); the start waits for them.
    probe: Option<LaunchProbe>,
    /// The packed full-chroma check's answer per decoder and renderer device, as `color_probed`
    /// keeps its own; None until the first launch that needs it.
    packed_probed: Option<(Hardware, Result<u8, String>)>,
    /// Decoder checks let go of (cancelled, timed out, or their device was lost) whose workers run on.
    ending_probes: Vec<LaunchProbe>,
    /// The PC being woken and the clock of its resends ("Waking <PC>…").
    waking: wol::Wakes,
    streaming: Option<(Streaming, TimerResolution)>,
    overlay: StreamOverlay,
    pending_power_handed: std::collections::HashMap<u64, hosts::Handed>,
    /// The request that turns the PC's automatic bitrate off after the last stream, while it may
    /// still be in flight, each with the PC it went to: the next stream to that PC (a reconnect
    /// too) turns it on only after it, and the app's end waits for them all.
    abr_stops: Vec<(u64, stream::host_abr::AbrStop)>,
    /// Sessions stopping on their own threads. Joined before the renderer they decoded on goes
    /// (exit) or before the next connection's launch request (a reconnect carries its own).
    teardowns: Vec<stream::Teardown>,
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
    /// Until when the start in `network_launch` waits for the window's HDR report.
    hdr_wait: Option<Instant>,
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
    /// The launcher keeps the screen awake for a while after controller input or a stream.
    screen_awake: sdl::LauncherAwake,
    /// What the stream being started plays without, straight to the display without the GPU renderer
    /// (`kms::without_gpu`): its start says so.
    kms_notes: Vec<String>,
    /// Full chroma was asked for and the stream being started runs without it: its start says so.
    packed_note: Option<String>,
    /// macOS: why PyroWave decodes on the processor for the stream starting, for its start notices.
    pyrowave_note: Option<String>,
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
    /// the decoder and the formats it checked (one-shot when V4L2 M2M had a transient failure).
    base_probed: Option<app::CachedBaseProbe>,
    /// Keys whose transient V4L2 base probe already retried once so a second failure stays cached.
    base_probe_retried: Option<ProbeKey>,
    /// Linux's renderer as it was made ([`RendererState`]).
    #[cfg(target_os = "linux")]
    renderer_state: RendererState,
    /// Why this stream's switch to the GPU renderer failed, for the check that refuses it.
    gpu_failure: Option<String>,
    /// The Graphics card setting last applied: a change is applied once, with nothing streaming.
    #[cfg(windows)]
    graphics_card: config::GraphicsCard,
    /// A Graphics card change a start that is still ending blocks, said once.
    #[cfg(windows)]
    graphics_card_blocked: Option<config::GraphicsCard>,
    /// A Renderer change a start that is still ending blocks, said once.
    #[cfg(target_os = "linux")]
    renderer_choice_blocked: Option<config::Renderer>,
    /// When a Renderer change that found a decoder still dropping asks again.
    #[cfg(target_os = "linux")]
    renderer_choice_retry: Option<Instant>,
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
    /// The launcher window's mode last applied from the Window setting: a changed setting moves the
    /// window once, and the Fullscreen hotkey keeps its own toggle in between
    /// (`apply_launcher_window_mode`).
    launcher_window_mode: WindowMode,
    /// The window changed since the last launcher frame: it needs one, whatever egui paints.
    window_changed: bool,
    /// A save failed this run and the player was told: the one notice a run gives for it.
    save_failed_told: bool,
}

fn main() {
    // Before any thread starts and anything loads Vulkan: the loader sees the bundled driver only.
    #[cfg(target_os = "macos")]
    stream::pyrowave_sdk::own_vulkan::prepare_environment();
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
    // Before `logpipe::start` spawns threads: `std::env::set_var` is only sound while
    // single-threaded on POSIX.
    let env_notes = startup::prepare_process_env();
    // After the package check, which must not write to the user's log.
    logpipe::start(graphics_restarted);
    let _log = logpipe::Guard;
    for line in env_notes {
        eprintln!("{line}");
    }
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

fn run(launch: Option<cli::Launch>, started: open_play::Started, link: Option<String>) -> Result<(), String> {
    startup::register_hooks();
    startup::init_sdl()?;
    #[cfg(feature = "demo-library")]
    demo_driver::require_sandbox()?;
    let store = Store::open().map_err(|e| format!("No config directory: {e}"))?;
    // The first screen's process owns the files; the second only reads them.
    let store = if second_screen::this_process() { store.read_only() } else { store };
    let host_store = nightjar_core::hosts::HostStore::open(store.dir()).with_read_only(store.is_read_only());
    let identity_store = nightjar_core::hosts::IdentityStore::open(store.dir()).with_read_only(store.is_read_only());
    let settings = store.settings();
    // Whole frames under VRR Automatic starts on Windows V-Sync for a monitor an earlier run found
    // not refreshing on present, and this run keeps the monitors it finds (`stream::scan_sync`).
    // The folder comes first: the restore drops the output names older versions kept.
    if !second_screen::this_process() {
        stream::scan_sync::keep_in(store.dir());
    }
    // Devices Nightjar shared through usbipd-win and could not unshare before it stopped (a
    // crash, a cancelled prompt) go back to Windows. Nothing runs without its journal.
    #[cfg(windows)]
    if !second_screen::this_process() && !store.is_read_only() {
        nightjar_desktop_media::usb_windows::recover_in_background();
    }
    stream::scan_sync::restore(&store.scan_untrusted().displays);
    let profiles = store.profiles();
    let mappings = store.controller_mappings();
    let hosts = host_store.load_records::<config::HostRecord, _>(config::keep_readable_settings_value);
    let (gpu, vulkan_video) = startup::choose_renderer_gpu(&settings, &hosts, &profiles);

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
    let (window, renderer, wake_event, launcher_mode) = startup::open_window_and_renderer(&store, &settings, gpu, vulkan_video)?;

    let waker: hosts::Waker = Arc::new(move || {
        // SAFETY: SDL_PushEvent is thread-safe; the event is plain data.
        unsafe {
            let mut event: SDL_Event = std::mem::zeroed();
            event.r#type = wake_event;
            SDL_PushEvent(&mut event);
        }
    });

    let mut gui = Gui::new();
    startup::prime_gui_touch(&mut gui);
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
    launcher.hdr_display = sdl::any_display_hdr();
    launcher.hdr_display_probe = Some(sdl::any_display_hdr);
    // SAFETY: a live window.
    launcher.fullscreen_modes = fullscreen_modes(unsafe { SDL_GetDisplayForWindow(window) });
    // The System theme's light or dark; SDL tells of a change (SDL_EVENT_SYSTEM_THEME_CHANGED).
    launcher.system_light = sdl::system_light();
    // SAFETY: a query without arguments, on the main thread after SDL_Init.
    launcher.screen_keyboard = unsafe { SDL_HasScreenKeyboardSupport() };
    #[cfg(target_os = "linux")]
    {
        launcher.wayland = sdl::video_driver() == "wayland";
    }
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
    startup::configure_display_tearing(window, renderer, &mut launcher, display);

    let opened_by_link = link.is_some();
    let links = Arc::new(link_channel::Inbox::default());
    if let Some(link) = link {
        links.put(link);
    }
    // A frontend's start ends with its stream, and a link waiting behind it would be lost: the
    // link starts its own window then.
    if launch.is_none() && !cfg!(feature = "demo-library")
        && let Err(e) = link_channel::listen(links.clone(), waker.clone()) {
            eprintln!("nightjar: pairing links do not come to this window: {e}");
        }
    #[cfg(not(feature = "demo-library"))]
    let records = host_store.load_records::<config::HostRecord, _>(config::keep_readable_settings_value);
    #[cfg(feature = "demo-library")]
    let records = ui::demo::records();
    // An install from before Home's setup card that has a PC paired made its first pairing long ago.
    if records.iter().any(|record| record.server_certificate.is_some()) && launcher.paired_before_setup_card()
        && let Err(e) = store.save_settings(&launcher.settings) {
            eprintln!("nightjar: saving settings failed: {e}");
        }
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
    // SteamGridDB and IGDB with the player's keys; the demo and the second screen stay offline.
    if !cfg!(feature = "demo-library") && !second_screen::this_process() {
        let agent = format!("Nightjar/{} ({})", VERSION, std::env::consts::OS);
        let keys = game_info::load_keys(store.dir());
        hosts.set_game_info(std::sync::Arc::new(game_info::GameInfo::new(store.dir(), keys, Box::new(game_info::Https(update_net::Native::new(agent))))));
    }
    launcher.game_info_keys = hosts.game_info_keys();
    {
        // Open on Home when it is on and some PC has a library to show.
        let hosts_view = hosts.ui_view();
        let sources: Vec<console::Source> = hosts_view.list.iter().map(console::Source::of).collect();
        if console::opens_on_home(settings.console_home, &sources) {
            launcher.go(Screen::Home);
        }
    }
    #[cfg(feature = "demo-library")]
    {
        let mut hosts_view = hosts.ui_view();
        ui::demo::fill(&mut hosts_view, &mut launcher, &gui.ctx);
        hosts.apply_ui_demo(hosts_view);
    }
    // A stream started from the command line waits for the next ordinary start.
    let mut save_failed_told = false;
    if cli.is_none()
        && std::env::var_os("NIGHTJAR_AUTOSTREAM").is_none()
        && launcher.show_kept_settings_once(&hosts.ui_view().list)
        && let Err(e) = store.save_settings(&launcher.settings) {
            eprintln!("nightjar: saving settings failed: {e}");
            save_failed_told = true;
            launcher.notify(crate::strings::tr("Changes could not be saved"), app::save_failed_text(store.dir()));
        }
    // A file that is there but cannot be read is not an identity to create a new one over: its
    // pairing key may still be recovered, so it stays as it is and the player is told.
    let mut identity_unreadable = false;
    let identity = if cfg!(feature = "demo-library") {
        None
    } else {
        match identity_store.identity() {
            Ok(Some(saved)) => match ClientIdentity::from_pem(saved.unique_id, &saved.certificate_pem, &saved.private_key_pem) {
                Ok(identity) => {
                    launcher.identity_ready = true;
                    Some(identity)
                }
                Err(e) => {
                    eprintln!("nightjar: stored identity unusable ({e}); not creating a new one over it");
                    let reason = format!("{} ({e})", crate::strings::tr(crate::strings::PAIRING_KEY_UNREADABLE));
                    launcher.notify(crate::strings::tr("No pairing key"), reason.clone());
                    launcher.identity_error = Some(reason);
                    identity_unreadable = true;
                    None
                }
            },
            Ok(None) => None,
            Err(e) => {
                eprintln!("nightjar: {e}; not creating a new pairing key");
                let reason = format!("{} ({e})", crate::strings::tr(crate::strings::PAIRING_KEY_UNREADABLE));
                launcher.notify(crate::strings::tr("No pairing key"), reason.clone());
                launcher.identity_error = Some(reason);
                identity_unreadable = true;
                None
            }
        }
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
        store.dir().join("update-skipped.txt"),
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
        #[cfg(windows)]
        recovery: render_device::Recovery::default(),
        gui,
        gui_overlay: std::ptr::null_mut(),
        store,
        host_store,
        identity_store,
        hosts,
        launcher,
        latency_bench: Default::default(),
        pads,
        pad_poll: PadPoll::default(),
        attempts: stream::Attempts::default(),
        probe: None,
        packed_probed: None,
        ending_probes: Vec::new(),
        waking: wol::Wakes::default(),
        streaming: None,
        overlay: StreamOverlay::default(),
        pending_power_handed: std::collections::HashMap::new(),
        abr_stops: Vec::new(),
        teardowns: Vec::new(),
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
        hdr_wait: None,
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
        screen_awake: sdl::LauncherAwake::default(),
        kms_notes: Vec::new(),
        packed_note: None,
        pyrowave_note: None,
        full_chroma_off: None,
        running: true,
        autostream: autostream::Autostream::from_env(),
        updates,
        open_play,
        cli,
        second_screen: None,
        color_probed: None,
        base_probed: None,
        base_probe_retried: None,
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
        #[cfg(windows)]
        graphics_card_blocked: None,
        #[cfg(target_os = "linux")]
        renderer_choice_blocked: None,
        #[cfg(target_os = "linux")]
        renderer_choice_retry: None,
        pin_pending: None,
        select_read: false,
        move_watch: Default::default(),
        title_bar: *theme::NIGHT,
        launcher_window_mode: launcher_mode,
        window_changed: true,
        save_failed_told,
    };
    match identity {
        Some(identity) => {
            app.hosts.set_identity(Arc::new(identity));
            app.hosts.poll(true, None);
        }
        // The demo library has no identity: without one nothing can ask a PC.
        None if cfg!(feature = "demo-library") => {}
        // A file that could not be read keeps its place; a new identity would replace it.
        None if identity_unreadable => {}
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
        app.updates.hold(app.streaming.is_some() || app.attempts.is_connecting() || app.network_launch.is_some() || app.probe.is_some());
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

// Test only, and declared down here: `strings.rs` reads this file up to its first test item.
#[cfg(test)]
mod ui_sdl_tests;

#[cfg(test)]
mod tests;
