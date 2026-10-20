//! Nightjar for Windows and Linux: a Moonlight-protocol game-streaming client on
//! nightjar-core. Copyright (C) 2026 RamazanKara, GPL-3.0-only.
//!
//! One SDL3 window and renderer serve the launcher (egui) and the stream (the decoded
//! video, with the overlay drawn by egui on top). The UI thread owns SDL; host requests,
//! the session, decoding and audio run on their own threads and wake the UI with an SDL
//! user event.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod config;
mod gamepad;
mod gui;
mod hosts;
mod keymap;
mod media;
mod sdl;
mod stream;
mod theme;
mod ui;

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::Duration;

use nightjar_core::http::ClientIdentity;
use sdl3_sys::everything::*;

use config::{DecoderPreference, Store, WindowMode};
use gamepad::Gamepads;
use gui::{Gui, Scale};
use hosts::{Apps, Event as HostEvent, Hosts};
use media::Hardware;
use stream::present::PacingMode;
use stream::{Connecting, Display, Exit, LaunchPlan, Streaming};
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
    waker: hosts::Waker,
    wake_event: u32,
    screenshot_dir: PathBuf,
    screenshot_requested: bool,
    stick_held: [bool; 4],
    running: bool,
}

fn display_of(window: *mut SDL_Window) -> Display {
    // SAFETY: a live window; the mode pointer is owned by SDL.
    unsafe {
        let id = SDL_GetDisplayForWindow(window);
        let mode = SDL_GetDesktopDisplayMode(id);
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
        std::process::exit(1);
    }
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
            sdl::create_renderer(window, settings.experimental_packed_444 || settings.hdr);
        if renderer.is_null() {
            return Err(format!("No renderer: {}", sdl::error()));
        }
        SDL_SetRenderVSync(renderer, 1);
        (window, renderer, SDL_RegisterEvents(1))
    };
    sdl::set_icon(window, &theme::logo_rgba(64), 64);
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
    let mut launcher = Launcher::new(settings.clone(), display);
    #[cfg(windows)]
    {
        launcher.tearing_supported =
            media::swapchain_allows_tearing(sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_SWAPCHAIN_POINTER));
    }
    #[cfg(not(windows))]
    {
        launcher.tearing_supported = true;
    }
    eprintln!(
        "nightjar: renderer {}, display {}x{} at {:.2} Hz, tearing {}",
        sdl::renderer_name(renderer),
        display.width,
        display.height,
        display.refresh_hz,
        launcher.tearing_supported
    );

    let hosts = Hosts::new(store.hosts(), waker.clone());
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
    let mut app = App {
        window,
        renderer,
        gui,
        store,
        hosts,
        launcher,
        pads: Gamepads {
            swap_face_buttons: settings.swap_face_buttons,
            motion: settings.controller_motion,
            ..Default::default()
        },
        connecting: None,
        cancelled: false,
        streaming: None,
        pending_launch: None,
        identity: None,
        waker,
        wake_event,
        screenshot_dir,
        screenshot_requested: false,
        stick_held: [false; 4],
        running: true,
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
    Ok(())
}

impl App {
    fn shutdown(&mut self) {
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

    fn save_hosts(&self) {
        if let Err(e) = self.store.save_hosts(&self.hosts.records()) {
            eprintln!("nightjar: saving hosts failed: {e}");
        }
    }

    /// Gamepad buttons and the left stick become arrow keys, Enter and Escape for egui.
    fn navigate(&mut self, event: &SDL_Event) {
        // SAFETY: union members read by event type.
        unsafe {
            match SDL_EventType(event.r#type) {
                SDL_EVENT_GAMEPAD_BUTTON_DOWN => {
                    let swap = self.launcher.settings.swap_face_buttons;
                    let key = match SDL_GamepadButton(i32::from(event.gbutton.button)) {
                        SDL_GAMEPAD_BUTTON_DPAD_UP => Some(egui::Key::ArrowUp),
                        SDL_GAMEPAD_BUTTON_DPAD_DOWN => Some(egui::Key::ArrowDown),
                        SDL_GAMEPAD_BUTTON_DPAD_LEFT => Some(egui::Key::ArrowLeft),
                        SDL_GAMEPAD_BUTTON_DPAD_RIGHT => Some(egui::Key::ArrowRight),
                        SDL_GAMEPAD_BUTTON_SOUTH => Some(if swap { egui::Key::Escape } else { egui::Key::Enter }),
                        SDL_GAMEPAD_BUTTON_EAST => Some(if swap { egui::Key::Enter } else { egui::Key::Escape }),
                        SDL_GAMEPAD_BUTTON_BACK => Some(egui::Key::Escape),
                        SDL_GAMEPAD_BUTTON_START => {
                            if matches!(self.launcher.dialog, Dialog::None) && self.connecting.is_none() {
                                self.launcher.go(Screen::Settings);
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
                SDL_EVENT_GAMEPAD_BUTTON_DOWN | SDL_EVENT_GAMEPAD_AXIS_MOTION => self.navigate(event),
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
        let busy = self.connecting.is_some()
            || matches!(self.launcher.dialog, Dialog::Pairing { .. })
            || self.identity.is_some();
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
                    Err(e) => self.launcher.message("No pairing key", e),
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
        for event in events {
            match event {
                HostEvent::Paired { id, result } => {
                    if !matches!(self.launcher.dialog, Dialog::Pairing { host, .. } if host == id) {
                        continue;
                    }
                    match result {
                        Ok(_) => {
                            self.launcher.dialog = Dialog::None;
                            self.save_hosts();
                            self.hosts.load_apps(id);
                            self.launcher.go(Screen::Apps(id));
                        }
                        Err(e) => self.launcher.message("Pairing failed", e),
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
                        self.launcher.message("Could not quit the app", e);
                    }
                },
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
                self.hosts.request_art(id, app, &self.store);
            }
        }
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
                        self.launcher.message("The stream could not start", e);
                    }
                }
            }
        }

        let connecting = self.connecting.as_ref().map(|c| ConnectingView {
            app: c.app_name.clone(),
            host: c.host_name.clone(),
            stage: c.stage.clone(),
        });
        let mut commands = Vec::new();
        let launcher = &mut self.launcher;
        let hosts = &self.hosts;
        let output = self.gui.run(self.window, self.renderer, |ui| {
            commands = launcher.ui(
                ui,
                View {
                    hosts,
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
            Command::QuitApp(host) => self.hosts.quit_app(host),
            Command::QuitAndLaunch { host, app } => {
                self.pending_launch = Some((host, app));
                self.hosts.quit_app(host);
            }
            Command::Pair(host) => {
                let pin = hosts::random_pin();
                self.launcher.dialog = Dialog::Pairing {
                    host,
                    pin: pin.clone(),
                };
                self.hosts.pair(host, pin);
            }
            Command::CancelPair => self.hosts.cancel_pairing(),
            Command::AddHost(text) => match config::parse_address(&text) {
                Some((address, port)) => {
                    self.hosts.add_manual(&address, port);
                    self.save_hosts();
                }
                None => self.launcher.message("That address is not valid", text),
            },
            Command::Unpair(host) => {
                self.hosts.unpair(host);
                self.save_hosts();
            }
            Command::Remove(host) => {
                self.hosts.remove(host);
                self.save_hosts();
            }
            Command::Refresh(host) => {
                self.hosts.poll_one(host);
                if self.launcher.screen == Screen::Apps(host) {
                    self.hosts.load_apps(host);
                }
            }
            Command::OpenApps(host) => {
                if !matches!(self.hosts.get(host).map(|h| &h.apps), Some(Apps::Loaded(_))) {
                    self.hosts.load_apps(host);
                }
                self.launcher.go(Screen::Apps(host));
            }
            Command::SaveSettings => {
                let s = &self.launcher.settings;
                self.pads.swap_face_buttons = s.swap_face_buttons;
                self.pads.motion = s.controller_motion;
                if let Err(e) = self.store.save_settings(s) {
                    eprintln!("nightjar: saving settings failed: {e}");
                }
            }
            Command::CancelConnect => {
                if let Some(c) = &self.connecting {
                    c.cancel();
                    self.cancelled = true;
                }
            }
        }
    }

    fn pacing(&self) -> PacingMode {
        let s = &self.launcher.settings;
        if s.vrr && self.launcher.tearing_supported {
            if s.present_at_host_times {
                PacingMode::HostTimes
            } else {
                PacingMode::Immediate
            }
        } else if s.vsync {
            PacingMode::Vsync
        } else {
            PacingMode::Immediate
        }
    }

    fn hardware(&self) -> Hardware {
        if self.launcher.settings.decoder == DecoderPreference::Software {
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

    fn launch(&mut self, host_id: u64, app_id: i32, resume: bool) {
        if self.connecting.is_some() {
            return;
        }
        if self.launcher.settings.hdr && !sdl::hdr_available(self.renderer) {
            self.launcher.message("HDR output unavailable", "Enable HDR on this display and restart Nightjar. The current compositor or swapchain does not support HDR output.");
            return;
        }
        let packed_modes = if self.launcher.settings.experimental_packed_444 {
            match stream::packed444::probe(self.renderer, self.hardware()) {
                Ok(modes) => modes,
                Err(reason) => {
                    self.launcher.message("Full chroma unavailable", reason);
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
            self.launcher.message("The PC is offline", "Nightjar has not reached this PC yet.");
            return;
        };
        let Apps::Loaded(apps) = &host.apps else {
            return;
        };
        let Some(app) = apps.iter().find(|a| a.id == app_id).cloned() else {
            return;
        };
        let display = display_of(self.window);
        self.launcher.display = display;
        let plan = LaunchPlan {
            host_id,
            record: host.record.clone(),
            info,
            app,
            resume,
            settings: self.launcher.settings.clone(),
            identity,
            display,
            hardware: self.hardware(),
            packed_modes,
            pacing: self.pacing(),
        };
        self.cancelled = false;
        self.connecting = Some(Connecting::start(plan, self.waker.clone()));
    }

    fn begin_stream(&mut self, connecting: Connecting, session: nightjar_core::session::Session, facts: stream::Facts) {
        let settings = connecting.settings.clone();
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
            match settings.window_mode {
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
        let mut streaming = Streaming::new(connecting, session, facts, display);
        streaming.session.input(|q| self.pads.announce_all(q));
        // SAFETY: a live window.
        let focused = unsafe { SDL_GetWindowFlags(self.window) & SDL_WINDOW_INPUT_FOCUS != 0 };
        if focused {
            streaming.set_capture(self.window, true);
        }
        streaming.show_toast("Ctrl+Alt+Shift+Q leaves the stream, +Z releases input");
        self.streaming = Some((streaming, TimerResolution::begin()));
    }

    fn end_stream(&mut self, exit: Exit) {
        let Some((streaming, _timer)) = self.streaming.take() else {
            return;
        };
        let host = streaming.host_id;
        eprintln!(
            "nightjar: stream ended ({})",
            match &exit {
                Exit::User => "left by the user".to_owned(),
                Exit::CloseWindow => "window closed".to_owned(),
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
        match exit {
            Exit::CloseWindow => self.running = false,
            Exit::User | Exit::Ended(None) => {}
            Exit::Ended(Some(message)) => self.launcher.message("The stream ended", message),
        }
        // egui forgets held modifiers from before the stream.
        self.gui.push(egui::Event::WindowFocused(true));
        self.launcher.refocus();
    }

    fn stream_frame(&mut self) {
        let window = self.window;
        let renderer = self.renderer;
        let Some((streaming, _)) = self.streaming.as_mut() else {
            return;
        };
        streaming.pump();
        streaming.drain_notices(&self.pads);
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
        let screenshot = std::mem::take(&mut self.screenshot_requested);
        let dir = self.screenshot_dir.clone();
        streaming.present(renderer, |s, renderer| {
            draw_overlay(gui, window, renderer, s);
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
        if !streaming.has_picture() && streaming.since_start() > Duration::from_secs(12) && streaming.exit.is_none() {
            streaming.exit = Some(Exit::Ended(Some(
                "No video arrived from the PC. Check its firewall for UDP ports 47998-48010.".into(),
            )));
        }
        if let Some(exit) = streaming.exit.take() {
            self.end_stream(exit);
        }
    }

    fn dispatch_stream_event(&mut self, event: &SDL_Event) {
        // SAFETY: reading the event type.
        let kind = SDL_EventType(unsafe { event.r#type });
        if kind.0 == self.wake_event {
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
        streaming.handle(event, self.window, self.renderer, &mut self.pads);
    }
}

/// The overlay and toasts, drawn with egui over the picture.
fn draw_overlay(gui: &mut Gui, window: *mut SDL_Window, renderer: *mut SDL_Renderer, s: &mut Streaming) {
    let toast = s.toast.as_ref().map(|(t, _)| t.clone());
    if !s.overlay_visible && toast.is_none() {
        return;
    }
    let lines = s.overlay.lines.clone();
    let visible = s.overlay_visible;
    let title = format!("{} · {}", s.facts.app_name, s.facts.host_name);
    let output = gui.run(window, renderer, |ui| {
        let ctx = ui.ctx().clone();
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
                                    ui.label(egui::RichText::new("Measuring…").color(theme::ON_SURFACE_VARIANT));
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
    gui.paint(renderer, output);
}
