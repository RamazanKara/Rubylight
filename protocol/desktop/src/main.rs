//! Nightjar for Windows and Linux: a Moonlight-protocol game-streaming client on
//! nightjar-core. Copyright (C) 2026 RamazanKara, GPL-3.0-only.
//!
//! One SDL3 window and renderer serve the launcher (egui) and the stream (the decoded
//! video, with the overlay drawn by egui on top). The UI thread owns SDL; host requests,
//! the session, decoding and audio run on their own threads and wake the UI with an SDL
//! user event.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod autostream;
mod config;
mod console;
mod controller_audio;
mod controller_mouse;
mod gamepad;
mod gui;
mod hosts;
mod keymap;
mod logfile;
mod logpipe;
mod media;
mod network;
mod network_test;
mod radeon;
mod sdl;
mod session_page;
mod session_ui;
mod shortcuts;
mod stream;
mod strings;
mod theme;
mod timing_csv;
mod ui;
mod wol;

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
use stream::present::{pacing, PacingMode};
use stream::{Connecting, Display, Exit, LaunchPlan, Streaming};
use strings::{tr, trf};
use ui::{Command, ConnectingView, Dialog, Launcher, Screen, View};

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
    streaming: Option<(Streaming, TimerResolution)>,
    pending_launch: Option<(u64, i32)>,
    identity: Option<Receiver<Result<ClientIdentity, String>>>,
    shortcut_file: Option<(bool, sdl::FileChoice)>,
    network_query: Option<Receiver<network::Network>>,
    network_test: Option<network_test::Test>,
    network_launch: Option<(u64, i32, bool)>,
    next_network_check: Instant,
    waker: hosts::Waker,
    wake_event: u32,
    screenshot_dir: PathBuf,
    screenshot_requested: bool,
    stick_held: [bool; 4],
    running: bool,
    autostream: Option<autostream::Autostream>,
}

fn display_of(window: *mut SDL_Window) -> Display {
    // SAFETY: a live window; the mode pointer is owned by SDL.
    unsafe {
        let id = SDL_GetDisplayForWindow(window);
        let mode = SDL_GetCurrentDisplayMode(id);
        if mode.is_null() {
            return Display {
                width: 1920,
                height: 1080,
                refresh_hz: 60.0,
            };
        }
        let m = &*mode;
        let density = if m.pixel_density > 0.0 { m.pixel_density } else { 1.0 };
        Display {
            width: (m.w as f32 * density).round() as u32,
            height: (m.h as f32 * density).round() as u32,
            refresh_hz: if m.refresh_rate > 0.0 { m.refresh_rate } else { 60.0 },
        }
    }
}

fn main() {
    // Package checks run without a window: `nightjar --check-pyrowave <report file>`.
    let mut args = std::env::args_os().skip(1);
    if args.next().is_some_and(|flag| flag == "--check-pyrowave") {
        let Some(report) = args.next() else {
            eprintln!("nightjar: --check-pyrowave needs a report file");
            std::process::exit(64);
        };
        std::process::exit(stream::pyrowave_mode::package_check(std::path::Path::new(&report)));
    }
    // After the package check, which must not write to the user's log.
    logpipe::start();
    let _log = logpipe::Guard;
    if let Err(e) = run() {
        eprintln!("nightjar: {e}");
        let text = std::ffi::CString::new(e).unwrap_or_default();
        // SAFETY: NUL-terminated strings; a null parent window is allowed.
        unsafe {
            SDL_ShowSimpleMessageBox(
                SDL_MESSAGEBOX_ERROR,
                c"Nightjar".as_ptr(),
                text.as_ptr(),
                std::ptr::null_mut(),
            )
        };
        logpipe::shutdown();
        std::process::exit(1);
    }
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

fn run() -> Result<(), String> {
    sdl::set_hint(SDL_HINT_APP_NAME, c"Nightjar");
    sdl::set_hint(SDL_HINT_APP_ID, c"io.github.ramazankara.nightjar");
    #[cfg(windows)]
    {
        // The decoder shares the renderer's D3D11 device from its own thread.
        sdl::set_hint(SDL_HINT_RENDER_DRIVER, c"direct3d11");
        sdl::set_hint(SDL_HINT_RENDER_DIRECT3D_THREADSAFE, c"1");
    }
    // Keep Alt+Tab and the Windows key for the host while the keyboard is grabbed.
    sdl::set_hint(SDL_HINT_ALLOW_ALT_TAB_WHILE_GRABBED, c"0");
    sdl::set_hint(SDL_HINT_VIDEO_MINIMIZE_ON_FOCUS_LOSS, c"0");
    sdl::set_hint(SDL_HINT_AUDIO_DEVICE_SAMPLE_FRAMES, c"480");
    sdl::set_hint(SDL_HINT_JOYSTICK_ALLOW_BACKGROUND_EVENTS, c"1");
    sdl::set_hint(SDL_HINT_MOUSE_RELATIVE_MODE_CENTER, c"1");

    // SAFETY: SDL initialisation on the main thread.
    unsafe {
        if !SDL_Init(SDL_INIT_VIDEO | SDL_INIT_AUDIO | SDL_INIT_GAMEPAD | SDL_INIT_EVENTS) {
            return Err(format!("SDL could not start: {}", sdl::error()));
        }
    }
    let store = Store::open().map_err(|e| format!("No config directory: {e}"))?;
    let settings = store.settings();
    let profiles = store.profiles();
    let mappings = store.controller_mappings();
    let gpu = settings.experimental_packed_444 || settings.hdr || settings.codec.is_pyrowave() || settings.upscaling != config::Upscaling::Bilinear || store.hosts().iter()
        .any(|host| host.settings.as_ref().is_some_and(config::Overrides::needs_gpu) || host.app_settings.values().any(config::Overrides::needs_gpu))
        || profiles.entries.iter().any(|profile| profile.settings.needs_gpu());

    // SAFETY: window and renderer creation on the main thread.
    let (window, renderer, wake_event) = unsafe {
        let window = SDL_CreateWindow(
            c"Nightjar".as_ptr(),
            1280,
            800,
            SDL_WINDOW_RESIZABLE | SDL_WINDOW_HIGH_PIXEL_DENSITY | SDL_WINDOW_HIDDEN,
        );
        if window.is_null() {
            return Err(format!("No window: {}", sdl::error()));
        }
        SDL_SetWindowMinimumSize(window, 720, 480);
        let renderer =
            sdl::create_renderer(window, gpu);
        if renderer.is_null() {
            return Err(format!("No renderer: {}", sdl::error()));
        }
        SDL_SetRenderVSync(renderer, 1);
        (window, renderer, SDL_RegisterEvents(1))
    };
    sdl::set_icon(window, &theme::logo_rgba(64), 64);
    sdl::exclude_from_capture_if_asked(window);
    // SAFETY: a live window.
    unsafe { SDL_ShowWindow(window) };

    let waker: hosts::Waker = Arc::new(move || {
        // SAFETY: SDL_PushEvent is thread-safe; the event is plain data.
        unsafe {
            let mut event: SDL_Event = std::mem::zeroed();
            event.r#type = wake_event;
            SDL_PushEvent(&mut event);
        }
    });

    let gui = Gui::new();
    theme::apply(&gui.ctx);
    let display = display_of(window);
    let mut launcher = Launcher::new(settings.clone(), profiles, store.shortcuts(), display);
    launcher.recents = store.recents();
    launcher.favourites = store.favourites();
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
    eprintln!(
        "nightjar: renderer {}, display {}x{} at {:.2} Hz, tearing {}",
        sdl::renderer_name(renderer),
        display.width,
        display.height,
        display.refresh_hz,
        launcher.tearing_supported
    );

    let mut hosts = Hosts::new(store.hosts(), waker.clone());
    hosts.load_cached_libraries(&store);
    {
        // Open on Home when it is on and some PC has a library to show.
        let sources: Vec<console::Source> = hosts.list.iter().map(console::Source::of).collect();
        if console::opens_on_home(settings.console_home, &sources) {
            launcher.go(Screen::Home);
        }
    }
    let identity = match store.identity() {
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
        streaming: None,
        pending_launch: None,
        identity: None,
        shortcut_file: None,
        network_query: Some(network::query(waker.clone())),
        network_test: None,
        network_launch: None,
        next_network_check: Instant::now(),
        waker,
        wake_event,
        screenshot_dir,
        screenshot_requested: false,
        stick_held: [false; 4],
        running: true,
        autostream: autostream::Autostream::from_env(),
    };
    match identity {
        Some(identity) => {
            app.hosts.set_identity(Arc::new(identity));
            app.hosts.poll();
        }
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
        if app.streaming.is_some() {
            app.stream_frame();
        } else {
            app.launcher_frame();
        }
    }
    eprintln!("nightjar: leaving");
    app.shutdown();
    if app.autostream.as_ref().is_some_and(|a| a.failed) {
        logpipe::shutdown();
        std::process::exit(2);
    }
    Ok(())
}

impl App {
    fn shutdown(&mut self) {
        self.network_test = None;
        if let Some(command) = self.launcher.screen.save_command() { self.command(command); }
        if let Some((streaming, _)) = self.streaming.take() {
            streaming.finish(self.window, &mut self.pads);
        }
        if let Some(c) = &self.connecting {
            c.cancel();
        }
        let _ = self.store.save_hosts(&self.hosts.records());
        self.pads.close_all();
        self.gui.destroy();
        // SAFETY: tearing down what run() created.
        unsafe {
            SDL_DestroyRenderer(self.renderer);
            SDL_DestroyWindow(self.window);
            SDL_Quit();
        }
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

    /// Gamepad buttons and the left stick become arrow keys, Enter and Escape for egui.
    fn navigate(&mut self, event: &SDL_Event) {
        // SAFETY: union members read by event type.
        unsafe {
            match SDL_EventType(event.r#type) {
                SDL_EVENT_GAMEPAD_BUTTON_DOWN => {
                    let swap = if self.streaming.is_some() { self.pads.swap_face_buttons } else { self.launcher.settings.swap_face_buttons };
                    let key = match SDL_GamepadButton(i32::from(event.gbutton.button)) {
                        SDL_GAMEPAD_BUTTON_DPAD_UP => Some(egui::Key::ArrowUp),
                        SDL_GAMEPAD_BUTTON_DPAD_DOWN => Some(egui::Key::ArrowDown),
                        SDL_GAMEPAD_BUTTON_DPAD_LEFT => Some(egui::Key::ArrowLeft),
                        SDL_GAMEPAD_BUTTON_DPAD_RIGHT => Some(egui::Key::ArrowRight),
                        SDL_GAMEPAD_BUTTON_SOUTH => Some(if swap { egui::Key::Escape } else { egui::Key::Enter }),
                        SDL_GAMEPAD_BUTTON_EAST => Some(if swap { egui::Key::Enter } else { egui::Key::Escape }),
                        SDL_GAMEPAD_BUTTON_BACK => Some(egui::Key::Escape),
                        SDL_GAMEPAD_BUTTON_WEST => Some(egui::Key::F2),
                        SDL_GAMEPAD_BUTTON_NORTH => Some(egui::Key::F3),
                        SDL_GAMEPAD_BUTTON_START => {
                            if self.streaming.is_none() && matches!(self.launcher.dialog, Dialog::None) && self.connecting.is_none() {
                                if let Some(command) = self.launcher.screen.save_command() { self.command(command); }
                                self.launcher.open_settings();
                            }
                            None
                        }
                        _ => None,
                    };
                    if let Some(key) = key {
                        for e in gui::tap(key) {
                            self.gui.push(e);
                        }
                    }
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
                        for e in gui::tap(key) {
                            self.gui.push(e);
                        }
                    }
                    self.stick_held[slot] = i32::from(a.value).abs() > 12000 && (held || self.stick_held[slot]);
                }
                _ => {}
            }
        }
    }

    fn handle_launcher_event(&mut self, event: &SDL_Event) {
        let scale = Scale::of(self.window);
        // SAFETY: union members read by event type.
        unsafe {
            match SDL_EventType(event.r#type) {
                SDL_EVENT_QUIT | SDL_EVENT_WINDOW_CLOSE_REQUESTED => self.running = false,
                SDL_EVENT_GAMEPAD_ADDED => self.pads.added(event.gdevice.which),
                SDL_EVENT_GAMEPAD_REMOVED => {
                    self.pads.removed(event.gdevice.which);
                }
                SDL_EVENT_GAMEPAD_BUTTON_DOWN | SDL_EVENT_GAMEPAD_AXIS_MOTION => {
                    if self.launcher.screen != Screen::Controllers || !self.launcher.controller_test { self.navigate(event); }
                }
                SDL_EVENT_KEY_DOWN if event.key.scancode == SDL_SCANCODE_F12 => {
                    self.screenshot_requested = true;
                }
                SDL_EVENT_KEY_DOWN if event.key.scancode == SDL_SCANCODE_F11 => {
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
        // Wait for input or a worker; animate the spinner while something runs.
        let busy = self.connecting.is_some() || self.network_launch.is_some() || self.network_test.is_some()
            || matches!(self.launcher.dialog, Dialog::Pairing { .. })
            || self.identity.is_some() || self.autostream.is_some();
        let timeout = if busy { 16 } else { 250 };
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
                if let Some((host, app, resume)) = self.network_launch.take() { self.launch_ready(host, app, resume); }
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
                    Ok(Some(path)) => match shortcuts::Shortcuts::read(&path) {
                        Ok(entries) => self.launcher.dialog = Dialog::ImportShortcuts(entries),
                        Err(e) => self.launcher.message(tr("Could not import shortcuts"), e),
                    },
                    Ok(None) => {}
                    Err(e) => self.launcher.message(tr("File dialog unavailable"), e),
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
                    Ok(()) => {
                        if let Some((host, app)) = self.pending_launch.take() {
                            if host == id {
                                self.launch(host, app, false);
                            }
                        }
                    }
                    Err(e) => {
                        self.pending_launch = None;
                        self.launcher.message(tr("Could not quit the app"), e);
                    }
                },
                HostEvent::Wake { result, .. } => match result {
                    Ok(()) => self.launcher.message(tr("Wake request sent"), tr("The PC should appear online shortly if Wake-on-LAN is enabled.")),
                    Err(e) => self.launcher.message(tr("Could not wake the PC"), e),
                },
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
            if let Some((host, app, resume)) = self.network_launch {
                if !matches!(self.hosts.get(host).map(|h| &h.apps), Some(Apps::Loading)) {
                    self.network_launch = None;
                    self.launch_ready(host, app, resume);
                }
            }
        }
        self.autostream_step();
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
                Err(e) => {
                    self.connecting = None;
                    if !std::mem::take(&mut self.cancelled) {
                        self.launcher.message(tr("The stream could not start"), e);
                    }
                }
            }
        }

        let connecting = self.connecting.as_ref().map(|c| ConnectingView {
            app: c.app_name.clone(),
            host: c.host_name.clone(),
            stage: c.stage.clone(),
        }).or_else(|| self.network_launch.map(|(host, _, _)| ConnectingView {
            app: tr("Starting stream").into(),
            host: self.hosts.get(host).map_or(String::new(), |h| h.display_name().to_owned()),
            stage: tr("Checking network profile").into(),
        }));
        let mut commands = Vec::new();
        let launcher = &mut self.launcher;
        let hosts = &self.hosts;
        let pads = &self.pads;
        let output = self.gui.run(self.window, self.renderer, |ui| {
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
                },
            );
        });
        self.gui.platform(self.window, &output.platform_output);
        // SAFETY: drawing on the live renderer.
        unsafe {
            SDL_SetRenderDrawColor(self.renderer, 0x14, 0x16, 0x1F, 255);
            SDL_RenderClear(self.renderer);
        }
        self.gui.paint(self.renderer, output);
        self.take_screenshot("launcher");
        // SAFETY: a live renderer.
        unsafe { SDL_RenderPresent(self.renderer) };
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
            Command::Launch { host, app, resume } => self.launch(host, app, resume),
            Command::ToggleFavourite(entry) => {
                config::toggle_favourite(&mut self.launcher.favourites, entry);
                if let Err(e) = self.store.save_favourites(&self.launcher.favourites) {
                    eprintln!("nightjar: could not save favourites: {e}");
                }
            }
            Command::QuitApp(host) => self.hosts.quit_app(host),
            Command::ControlAction { host, app } => self.hosts.control_action(host, app),
            Command::QuitAndLaunch { host, app, named } => {
                // The PC may run another app than the prompt named: look again before closing anything.
                let now = self.hosts.get(host).and_then(|h| h.running_app());
                match console::recheck_running(named, now) {
                    console::Running::Changed => self.launcher.message(
                        tr("The running app changed"),
                        tr("The PC runs another app now. Check the screen and try again."),
                    ),
                    console::Running::Gone => self.launch(host, app, false),
                    console::Running::Same => {
                        self.pending_launch = Some((host, app));
                        self.hosts.quit_app(host);
                    }
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
            Command::Wake(host) => self.hosts.wake(host),
            Command::Rename { host, alias } => rename_host(&mut self.hosts, &self.store, host, &alias),
            Command::HostSettings(id) => {
                if let Some(host) = self.hosts.get(id) {
                    self.launcher.inherited_settings = self.launcher.profiles.settings(&self.launcher.settings, self.launcher.network.kind(&self.launcher.profiles.home_wifi));
                    self.launcher.edit_overrides = host.record.settings.clone().unwrap_or_default();
                    self.launcher.edit_settings = self.launcher.edit_overrides.apply(&self.launcher.inherited_settings);
                    self.launcher.edit_name = trf("{} settings", &[&host.display_name()]);
                    self.launcher.go(Screen::HostSettings(id));
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
                    self.launcher.inherited_settings = host.record.settings(&self.launcher.profiles.settings(&self.launcher.settings, self.launcher.network.kind(&self.launcher.profiles.home_wifi)));
                    self.launcher.edit_overrides = host.record.app_settings(&app.uuid, app.id).cloned().unwrap_or_default();
                    self.launcher.edit_settings = self.launcher.edit_overrides.apply(&self.launcher.inherited_settings);
                    self.launcher.edit_name = trf("{} settings", &[&app.name]);
                    self.launcher.editing_app = Some(app);
                    self.launcher.go(Screen::AppSettings(id));
                }
            }
            Command::SaveAppSettings(id) => {
                if let (Some(host), Some(app)) = (self.hosts.get_mut(id), self.launcher.editing_app.as_ref()) {
                    host.record.save_app_settings(&app.uuid, app.id, self.launcher.edit_overrides.clone());
                    self.save_hosts();
                }
            }
            Command::ProfileSettings(id) => {
                if let Some(profile) = self.launcher.profiles.entries.iter().find(|p| p.id == id) {
                    self.launcher.inherited_settings = self.launcher.settings.clone();
                    self.launcher.edit_overrides = profile.settings.clone();
                    self.launcher.edit_settings = profile.settings.apply(&self.launcher.inherited_settings);
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
                let client = match hosts::client_for(identity, &host.record, &host.record.address) {
                    Ok(client) => client,
                    Err(error) => { self.launcher.message(tr("Network test unavailable"), error); return; }
                };
                let settings = host.record.settings(&self.launcher.profiles.settings(&self.launcher.settings, self.launcher.network.kind(&self.launcher.profiles.home_wifi)));
                let (_, _, fps, bitrate) = stream::stream_mode(&settings, display_of(self.window));
                self.network_test = Some(network_test::Test::start(id, client, self.launcher.network.clone(), (bitrate, fps), calibrate, self.waker.clone()));
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
            Command::CancelPair => self.hosts.cancel_pairing(),
            Command::AddHost(text) => match config::parse_address(&text) {
                Some((address, port)) => {
                    self.hosts.add_manual(&address, port);
                    self.save_hosts();
                }
                None => self.launcher.message(tr("That address is not valid"), text),
            },
            Command::Unpair(host) => {
                // The library and the certificate go when the PC has answered (see the event).
                if let Err(e) = self.hosts.unpair(host) {
                    self.launcher.message(tr("Could not unpair"), e);
                }
            }
            Command::Remove(host) => remove_host(&mut self.hosts, &self.store, &mut self.launcher.recents, &mut self.launcher.favourites, host),
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
            Command::RequestArt { host, app, version } => self.hosts.request_art(host, app, &version, &self.store),
            Command::CloseGame(host) => self.hosts.session_action(host, ActionId::CloseGame),
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
                self.network_launch = None;
                if let Some(c) = &self.connecting {
                    c.cancel();
                    self.cancelled = true;
                }
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
            Hardware::Vaapi
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
            autostream::Step::Launch(host, app) => self.launch(host, app, false),
            autostream::Step::Fail(why) => {
                auto.fail(&why);
                self.running = false;
            }
        }
    }

    fn launch(&mut self, host_id: u64, app_id: i32, resume: bool) {
        if self.connecting.is_some() || self.network_launch.is_some() { return; }
        self.network_launch = Some((host_id, app_id, resume));
        self.network_query = Some(network::query(self.waker.clone()));
    }

    fn launch_ready(&mut self, host_id: u64, app_id: i32, resume: bool) {
        if self.connecting.is_some() {
            return;
        }
        let Some(host) = self.hosts.get(host_id) else {
            return;
        };
        let app = match launch_app(&host.apps, app_id) {
            LaunchApp::Ready(app) => app,
            LaunchApp::Wait => {
                // The list is being read again (the first native read of a run reloads it).
                // Starting over once it lands keeps the launch instead of dropping it unseen.
                self.network_launch = Some((host_id, app_id, resume));
                return;
            }
            LaunchApp::Gone => return,
            LaunchApp::Failed(why) => {
                self.launcher.message(tr("The stream could not start"), trf("The app list could not be loaded. {}", &[&why]));
                return;
            }
            LaunchApp::Missing => {
                self.launcher.message(tr("The stream could not start"), tr("The PC's app list has no entry for this game.").to_owned());
                return;
            }
        };
        if app.is_control_action() {
            self.hosts.control_action(host_id, app_id);
            return;
        }
        let mut settings = host.record.settings(&self.launcher.profiles.settings(&self.launcher.settings, self.launcher.network.kind(&self.launcher.profiles.home_wifi)));
        if let Some(overrides) = host.record.app_settings(&app.uuid, app.id) { settings = overrides.apply(&settings); }
        if settings.codec.is_pyrowave() { settings.experimental_packed_444 = false; }
        if app.role(false) == nightjar_core::http::AppRole::InputOnly {
            settings.hdr = false;
            settings.experimental_packed_444 = false;
            settings.codec = config::CodecPreference::H264;
            settings.width = 1280;
            settings.height = 720;
            settings.fps = 60;
        }
        if settings.hdr && !sdl::hdr_available(self.renderer) {
            self.launcher.message(tr("HDR output unavailable"), tr("Enable HDR on this display and restart Nightjar. The current compositor or swapchain does not support HDR output."));
            return;
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
                    self.launcher.message(tr("PyroWave output unavailable"), trf("{}. Restart Nightjar after selecting PyroWave so it can initialize the GPU renderer.", &[&reason]));
                    return;
                }
                (Err(reason), config::PyroWaveDecoder::Gpu) => {
                    self.launcher.message(tr("PyroWave GPU decoding unavailable"), trf("{}. Switch the PyroWave decoder to CPU in the settings, or pick another codec.", &[&reason]));
                    return;
                }
            }
        }
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
        let Some(identity) = self.hosts.identity().cloned() else {
            return;
        };
        let Some(host) = self.hosts.get(host_id) else {
            return;
        };
        let Some(info) = host.info.clone() else {
            self.launcher.message(tr("The PC is offline"), tr("Nightjar has not reached this PC yet."));
            return;
        };
        let display = display_of(self.window);
        self.launcher.display = display;
        let (pacing, vrr) = pacing(&settings, self.launcher.tearing_supported);
        let plan = LaunchPlan {
            host_id,
            record: host.record.clone(),
            host_name: host.display_name().to_owned(),
            info,
            app,
            resume,
            settings: settings.clone(),
            identity,
            display,
            hardware: self.hardware(&settings),
            gpu_decoder,
            packed_modes,
            pacing,
            vrr,
            gamepad_mask: if settings.controllers { u32::from(self.pads.active_mask()) } else { 0 },
            prepared_controllers: if settings.controllers { self.pads.prepare(settings.controller_motion) } else { String::new() },
            pyrowave_link_mbps: self.launcher.network.wired_link_mbps(),
            pyrowave_pace_mbps: host.record.pace.as_ref().map_or(0, |pace| pace.for_network(&self.launcher.network, host.info.as_ref().map_or(0, |info| info.pyrowave_host_link_mbps))),
            bitrate_limit_kbps: self.launcher.profiles.bitrate_limit(self.launcher.network.kind(&self.launcher.profiles.home_wifi)),
            microphone_muted: false,
            recovery: None,
        };
        self.cancelled = false;
        self.connecting = Some(Connecting::start(plan, self.waker.clone()));
    }

    fn begin_stream(&mut self, connecting: Connecting, session: nightjar_core::session::Session, facts: stream::Facts) {
        if let Some(auto) = self.autostream.as_mut() { auto.streaming_began(); }
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
        // SAFETY: window and renderer calls on the main thread.
        unsafe {
            SDL_SetRenderVSync(self.renderer, if pacing == PacingMode::Vsync { 1 } else { 0 });
            match if facts.role == nightjar_core::http::AppRole::InputOnly { WindowMode::Windowed } else { settings.window_mode } {
                WindowMode::Fullscreen => {
                    let mut mode: SDL_DisplayMode = std::mem::zeroed();
                    let id = SDL_GetDisplayForWindow(self.window);
                    if SDL_GetClosestFullscreenDisplayMode(
                        id,
                        facts.width,
                        facts.height,
                        facts.fps as f32,
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
        }
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
        streaming.show_toast(warning.unwrap_or(tr("Ctrl+Alt+Shift+Tab opens controls, +Q disconnects, +Z releases input")));
        self.streaming = Some((streaming, TimerResolution::begin()));
    }

    /// Notes the game of a stream that just started, for Home's "Recently played".
    fn remember_recent(&mut self, connecting: &Connecting) {
        let Some(entry) = connecting.recent_game() else { return };
        config::remember(&mut self.launcher.recents, entry);
        if let Err(e) = self.store.save_recents(&self.launcher.recents) {
            eprintln!("nightjar: could not save recent games: {e}");
        }
    }

    fn end_stream(&mut self, exit: Exit) {
        let Some((streaming, _timer)) = self.streaming.take() else {
            return;
        };
        let host = streaming.host_id;
        let reconnect = if matches!(exit, Exit::Reconnect) || exit.retryable() {
            streaming.reconnect_plan(!matches!(exit, Exit::Reconnect))
        } else { None };
        eprintln!(
            "nightjar: stream ended ({})",
            match &exit {
                Exit::User => "left by the user".to_owned(),
                Exit::CloseWindow => "window closed".to_owned(),
                Exit::Reconnect => "reconnecting".to_owned(),
                Exit::Terminated(code) => format!("host or transport error {code}"),
                Exit::Ended(None) => "ended by the PC".to_owned(),
                Exit::Ended(Some(m)) => m.clone(),
            }
        );
        streaming.finish(self.window, &mut self.pads);
        // SAFETY: window and renderer calls on the main thread.
        unsafe {
            SDL_SetWindowFullscreen(self.window, false);
            SDL_SetRenderVSync(self.renderer, 1);
        }
        self.hosts.poll_one(host);
        if let Some(mut plan) = reconnect {
            plan.display = display_of(self.window);
            plan.gamepad_mask = if plan.settings.controllers { u32::from(self.pads.active_mask()) } else { 0 };
            plan.prepared_controllers = if plan.settings.controllers { self.pads.prepare(plan.settings.controller_motion) } else { String::new() };
            self.cancelled = false;
            self.connecting = Some(Connecting::start(plan, self.waker.clone()));
        } else {
        match exit {
            Exit::CloseWindow => self.running = false,
            Exit::User | Exit::Reconnect | Exit::Ended(None) => {}
            Exit::Terminated(code) => {
                if let Some(message) = stream::termination_text(code) {
                    self.launcher.message(tr("The stream ended"), message);
                }
            }
            Exit::Ended(Some(message)) => self.launcher.message(tr("The stream ended"), message),
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
        let Some((streaming, _)) = self.streaming.as_mut() else {
            return;
        };
        streaming.drain_notices(&mut self.pads);
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
        let Some((streaming, _)) = self.streaming.as_mut() else {
            return;
        };
        streaming.pump();
        if streaming.session_page_requested {
            let link = self.launcher.network.description(&self.launcher.profiles.home_wifi);
            streaming.open_session_page(self.waker.clone(), link);
        }
        if streaming.update_overlay() && streaming.overlay_visible {
            streaming.needs_redraw = true;
        }
        if let Some((_, at)) = &streaming.toast {
            if at.elapsed() > Duration::from_secs(4) {
                streaming.toast = None;
                streaming.needs_redraw = true;
            }
        }
        let gui = &mut self.gui;
        let pads = &mut self.pads;
        let shortcuts = &self.launcher.shortcuts;
        let screenshot = std::mem::take(&mut self.screenshot_requested);
        let dir = self.screenshot_dir.clone();
        streaming.present(renderer, |s, renderer| {
            draw_overlay(gui, window, renderer, s, pads, shortcuts);
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
        // No picture for 12 s after the start means the stream never came up.
        if streaming.facts.role != nightjar_core::http::AppRole::InputOnly
            && !streaming.has_picture() && streaming.since_start() > Duration::from_secs(12) && streaming.exit.is_none() {
            streaming.exit = Some(Exit::Terminated(nightjar_core::session::error::NO_VIDEO_TRAFFIC));
        }
        if let Some(auto) = &self.autostream {
            if streaming.exit.is_none() && streaming.since_start() >= auto.length() {
                streaming.exit = Some(Exit::User);
            }
        }
        if let Some(exit) = streaming.exit.take() {
            let planned = matches!(exit, Exit::User);
            self.end_stream(exit);
            if let Some(auto) = self.autostream.as_mut() {
                if !planned { auto.fail("the stream ended before its time"); }
                self.running = false;
            }
        }
    }

    fn dispatch_stream_event(&mut self, event: &SDL_Event) {
        // SAFETY: reading the event type.
        let kind = SDL_EventType(unsafe { event.r#type });
        if kind.0 == self.wake_event {
            // The Session page's worker published something new.
            if let Some((s, _)) = self.streaming.as_mut() {
                if s.session_page.is_some() { s.needs_redraw = true; }
            }
            return;
        }
        if kind == SDL_EVENT_KEY_DOWN && unsafe { event.key.scancode } == SDL_SCANCODE_F12 {
            self.screenshot_requested = true;
            if let Some((s, _)) = self.streaming.as_mut() {
                s.needs_redraw = true;
            }
            return;
        }
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
            let close_menu = (kind == SDL_EVENT_KEY_DOWN && unsafe {
                let key = event.key;
                key.scancode == SDL_SCANCODE_ESCAPE || (key.scancode == SDL_SCANCODE_TAB
                    && key.r#mod.0 & SDL_KMOD_CTRL.0 != 0 && key.r#mod.0 & SDL_KMOD_ALT.0 != 0
                    && key.r#mod.0 & SDL_KMOD_SHIFT.0 != 0)
            }) || (kind == SDL_EVENT_GAMEPAD_BUTTON_DOWN && unsafe {
                event.gbutton.button == back_button.0 as u8 || event.gbutton.button == SDL_GAMEPAD_BUTTON_BACK.0 as u8
            });
            if close_menu {
                // Escape or B on the Session page goes back to the controls first.
                if streaming.session_page.is_some() {
                    streaming.close_session_page();
                } else {
                    streaming.toggle_menu(self.window, &mut self.pads);
                }
                return;
            }
            if matches!(kind, SDL_EVENT_GAMEPAD_BUTTON_DOWN | SDL_EVENT_GAMEPAD_AXIS_MOTION) {
                streaming.needs_redraw = true;
                self.navigate(event);
                return;
            }
            if !matches!(kind, SDL_EVENT_QUIT | SDL_EVENT_WINDOW_CLOSE_REQUESTED
                | SDL_EVENT_WINDOW_FOCUS_LOST | SDL_EVENT_WINDOW_FOCUS_GAINED
                | SDL_EVENT_GAMEPAD_ADDED | SDL_EVENT_GAMEPAD_REMOVED
                | SDL_EVENT_AUDIO_DEVICE_ADDED | SDL_EVENT_AUDIO_DEVICE_REMOVED) {
                self.gui.handle(event, Scale::of(self.window));
                streaming.needs_redraw = true;
                return;
            }
        }
        streaming.handle(event, self.window, self.renderer, &mut self.pads);
    }
}

/// The overlay and toasts, drawn with egui over the picture.
fn draw_overlay(gui: &mut Gui, window: *mut SDL_Window, renderer: *mut SDL_Renderer, s: &mut Streaming, pads: &mut Gamepads, shortcuts: &shortcuts::Shortcuts) {
    let toast = s.toast.as_ref().map(|(t, _)| t.clone());
    let input_only = s.facts.role == nightjar_core::http::AppRole::InputOnly;
    if !s.overlay_visible && toast.is_none() && !input_only && !s.menu_visible {
        return;
    }
    let lines = s.overlay.lines.clone();
    let visible = s.overlay_visible;
    let title = format!("{} · {}", s.facts.app_name, s.facts.host_name);
    let output = gui.run(window, renderer, |ui| {
        let ctx = ui.ctx().clone();
        if input_only {
            egui::Area::new(egui::Id::unique("input-only"))
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(&ctx, |ui| {
                    ui.label(egui::RichText::new(tr("Remote Input connected")).font(theme::semibold(26.0)).color(theme::MOON));
                    ui.label(&title);
                    ui.label(tr("Keyboard, mouse and controllers control the PC. Ctrl+Alt+Shift+Q disconnects."));
                });
        }
        if s.menu_visible && s.session_page.is_some() {
            let lines = s.connection_lines();
            let hints = s.session_hints();
            let mut outcome = session_ui::Outcome::Stay;
            if let Some(page) = s.session_page.as_mut() {
                egui::Window::new(tr("Session and graphics"))
                    .id(egui::Id::unique("session-and-graphics"))
                    .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                    .collapsible(false).resizable(false).vscroll(true).default_width(440.0)
                    .show(&ctx, |ui| {
                        gui::follow_focus(ui, &mut s.menu_focus);
                        outcome = session_ui::show(ui, page, &title, &lines, hints);
                    });
            }
            match outcome {
                session_ui::Outcome::Stay => {}
                session_ui::Outcome::Resume => s.toggle_menu(window, pads),
                session_ui::Outcome::Back => s.close_session_page(),
                session_ui::Outcome::Disconnect | session_ui::Outcome::Leave => s.exit = Some(Exit::User),
            }
        }
        if s.menu_visible && s.session_page.is_none() {
            egui::Window::new(tr("Stream controls"))
                .id(egui::Id::unique("stream-controls"))
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .collapsible(false).resizable(false).vscroll(true)
                .show(&ctx, |ui| {
                    gui::follow_focus(ui, &mut s.menu_focus);
                    ui.label(&title);
                    let back = ui.button(tr("Return to stream"));
                    if ctx.memory(|m| m.focused().is_none()) { back.request_focus(); }
                    if back.clicked() { s.toggle_menu(window, pads); }
                    ui.checkbox(&mut s.overlay_visible, tr("Performance overlay"));
                    let current = s.video_scaling();
                    let (mut scaling, mut top) = current;
                    egui::ComboBox::from_id_salt("stream-scaling").selected_text(tr(scaling.label())).show_ui(ui, |ui| {
                        use config::VideoScaling;
                        for mode in [VideoScaling::Fit, VideoScaling::Fill, VideoScaling::Stretch, VideoScaling::Native] {
                            ui.selectable_value(&mut scaling, mode, tr(mode.label()));
                        }
                    });
                    ui.checkbox(&mut top, tr("Align video to top"));
                    if (scaling, top) != current { s.set_video_scaling(scaling, top); }
                    if ui.button(tr("Export statistics (last hour)")).clicked() { s.export_statistics = true; }
                    if s.microphone_available() {
                        let mut muted = s.microphone_muted;
                        if ui.checkbox(&mut muted, tr("Mute microphone")).changed() { s.mute_microphone(muted, pads); }
                    }
                    ui.add_enabled_ui(s.facts.clipboard, |ui| {
                        if ui.button(tr("Send clipboard to PC")).clicked() { s.clipboard(true); }
                        if ui.button(tr("Copy PC clipboard here")).clicked() { s.clipboard(false); }
                    });
                    if s.facts.role != nightjar_core::http::AppRole::RemoteMonitor && !shortcuts.data.is_empty() {
                        egui::CollapsingHeader::new(tr("Keyboard shortcuts")).id_salt("keyboard-shortcuts").show(ui, |ui| {
                            for shortcut in &shortcuts.data {
                                if ui.button(format!("{} · {}", shortcut.name, shortcut.keys.join(" + "))).clicked() {
                                    s.toggle_menu(window, pads);
                                    s.send_shortcut(shortcut);
                                }
                            }
                        });
                    }
                    if s.facts.runtime_bitrate {
                        if s.facts.role == nightjar_core::http::AppRole::Stream {
                            let mut enabled = s.adaptive_bitrate;
                            if ui.add_enabled(!s.bitrate_busy, egui::Checkbox::new(&mut enabled, tr("Adapt bitrate to the connection"))).changed() {
                                s.enable_adaptive_bitrate(enabled);
                            }
                        }
                        ui.horizontal(|ui| {
                            ui.add_enabled_ui(!s.adaptive_bitrate && !s.bitrate_busy, |ui| theme::step_value(ui, &mut s.bitrate_kbps, 1..=2_000_000, 1000.0, |d| d.speed(100).suffix(" kbps")));
                            if ui.add_enabled(!s.adaptive_bitrate && !s.bitrate_busy, egui::Button::new(tr("Apply bitrate"))).clicked() {
                                s.set_bitrate();
                            }
                        });
                    }
                    if s.facts.reconfigure {
                        ui.separator();
                        let mut quality = s.quality_mode;
                        egui::ComboBox::from_id_salt("stream-quality").selected_text(tr(quality.label())).show_ui(ui, |ui| {
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
                                let mut fps = s.requested_mode.fps_millihz as f64 / 1000.0;
                                if theme::step_value(ui, &mut fps, 10.0..=500.0, 1.0, |d| d.suffix(" fps").max_decimals(3)).changed() {
                                    s.requested_mode.fps_millihz = (fps * 1000.0).round() as u32;
                                }
                            });
                            if ui.button(tr("Apply stream size and rate")).clicked() { s.set_mode(s.requested_mode); }
                        });
                    }
                    if s.controller_audio_available() {
                        ui.separator();
                        ui.label(tr("DualSense USB audio and waveform haptics"));
                        let outputs = controller_audio::outputs();
                        let inputs = controller_audio::inputs();
                        let mut selections = Vec::new();
                        let mut input_selections = Vec::new();
                        for pad in &mut pads.pads {
                            if !pad.supports_audio() { continue; }
                            let current = pad.audio.as_ref().map(|a| a.device);
                            let mut selected = current;
                            let label = current.and_then(|id| outputs.iter().find(|(device, _)| *device == id)).map_or(tr("Off"), |(_, name)| name.as_str());
                            egui::ComboBox::from_id_salt(("controller-audio", pad.index)).selected_text(trf("Controller {}: {}", &[&(pad.index + 1), &label])).show_ui(ui, |ui| {
                                ui.selectable_value(&mut selected, None, tr("Off"));
                                for (id, name) in &outputs { ui.selectable_value(&mut selected, Some(*id), name); }
                            });
                            if selected != current { selections.push((pad.index, selected)); }
                            if let Some(audio) = &mut pad.audio {
                                let mut input = audio.input;
                                let label = input.and_then(|id| inputs.iter().find(|(device, _)| *device == id)).map_or(tr("Off"), |(_, name)| name.as_str());
                                ui.add_enabled_ui(s.microphone_available(), |ui| {
                                    egui::ComboBox::from_id_salt(("controller-microphone", pad.index)).selected_text(trf("Microphone: {}", &[&label])).show_ui(ui, |ui| {
                                        ui.selectable_value(&mut input, None, tr("Off"));
                                        for (id, name) in &inputs { ui.selectable_value(&mut input, Some(*id), name); }
                                    });
                                });
                                if input != audio.input { input_selections.push((pad.index, input)); }
                                egui::ComboBox::from_id_salt(("controller-headphones", pad.index))
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
                    }
                    if !s.facts.server_commands.is_empty() {
                        ui.separator();
                        ui.label(tr("PC commands"));
                        for (index, name) in s.facts.server_commands.iter().enumerate() {
                            if !name.is_empty() && ui.button(name).clicked() {
                                s.pending_command = Some(index);
                            }
                        }
                        if let Some(index) = s.pending_command {
                            ui.label(trf("Run ‘{}’ on the PC?", &[&s.facts.server_commands[index]]));
                            ui.horizontal(|ui| {
                                if ui.button(tr("Run command")).clicked() {
                                    s.pending_command = None;
                                    s.server_command(index);
                                }
                                if ui.button(tr("Cancel")).clicked() { s.pending_command = None; }
                            });
                        }
                    }
                    ui.separator();
                    if s.session_page_available() && ui.button(tr("Session and graphics")).clicked() {
                        s.session_page_requested = true;
                    }
                    if ui.button(tr("Reconnect")).clicked() { s.exit = Some(Exit::Reconnect); }
                    if ui.button(tr("Disconnect")).clicked() { s.exit = Some(Exit::User); }
                    ui.label(tr("Ctrl+Alt+Shift+Tab or Start+Select opens this menu."));
                });
        }
        if visible {
            egui::Area::new(egui::Id::unique("overlay"))
                .fixed_pos(egui::pos2(16.0, 16.0))
                .show(&ctx, |ui| {
                    egui::Frame::new()
                        .fill(egui::Color32::from_rgba_unmultiplied(0x0E, 0x10, 0x17, 215))
                        .corner_radius(egui::CornerRadius::same(10))
                        .inner_margin(egui::Margin::symmetric(14, 10))
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new(&title).font(theme::semibold(14.0)).color(theme::MOON));
                            ui.add_space(2.0);
                            ui.spacing_mut().interact_size.y = 16.0;
                            egui::Grid::new("stats").num_columns(2).spacing([14.0, 3.0]).show(ui, |ui| {
                                if lines.is_empty() {
                                    ui.label(egui::RichText::new(tr("Measuring…")).color(theme::ON_SURFACE_VARIANT));
                                    ui.end_row();
                                }
                                for (k, v) in &lines {
                                    ui.label(egui::RichText::new(k).font(theme::regular(13.0)).color(theme::OUTLINE));
                                    ui.label(egui::RichText::new(v).font(theme::regular(13.0)).color(theme::ON_SURFACE));
                                    ui.end_row();
                                }
                            });
                        });
                });
        }
        if let Some(text) = &toast {
            egui::Area::new(egui::Id::unique("toast"))
                .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -40.0))
                .show(&ctx, |ui| {
                    egui::Frame::new()
                        .fill(egui::Color32::from_rgba_unmultiplied(0x28, 0x2A, 0x36, 235))
                        .corner_radius(egui::CornerRadius::same(18))
                        .inner_margin(egui::Margin::symmetric(18, 10))
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new(text).font(theme::regular(14.5)).color(theme::ON_SURFACE));
                        });
                });
        }
    });
    if s.menu_visible {
        gui.platform(window, &output.platform_output);
    }
    gui.paint(renderer, output);
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
    id: u64,
) {
    if let Some(unique_id) = hosts.get(id).map(|h| h.record.unique_id.clone()).filter(|id| !id.is_empty()) {
        (*recents, *favourites) = store.forget_host(&unique_id, recents, favourites);
    }
    hosts.remove(id);
    save_hosts(store, hosts);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

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
        remove_host(&mut hosts, &store, &mut recents, &mut favourites, gone);
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
