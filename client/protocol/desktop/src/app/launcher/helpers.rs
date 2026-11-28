//! Launcher helper methods: gamepad navigation repeat, window mode/visibility tracking,
//! host cache saving, stream-move event handling, latency benchmark hooks and pointer/window predicates.

use super::super::*;

/// What a file dialog of the shared folder browser was opened for, with where the browser was
/// then.
pub(crate) enum FolderPick {
    /// The folder to save the PC's file `name` in.
    Download { host: u64, at: folders_page::Place, name: String, size: u64 },
    /// The file to upload.
    Upload { host: u64, at: folders_page::Place },
}

/// Per-gamepad signed left-stick deflection (`-1`, `0`, `+1`) for `[LEFTX, LEFTY]` UI navigation.
/// Tracking signed direction per `SDL_JoystickID` guarantees rapid opposite flips (`-30000` ->
/// `+30000`) emit immediately, independent pads never block one another, and unplugging a tilted
/// pad clears its entry cleanly (`remove_pad`).
#[derive(Default)]
pub(crate) struct StickNav {
    held: std::collections::HashMap<u32, [i8; 2]>,
}

impl StickNav {
    const ENGAGE: i32 = 24_000;
    const RELEASE: i32 = 12_000;

    /// Updates `slot` (`0` = X, `1` = Y) for `pad` and returns `(previous_sign, new_sign)`.
    pub(crate) fn update(&mut self, pad: u32, slot: usize, value: i16) -> (i8, i8) {
        let prev = self.held.get(&pad).map_or(0, |axes| axes[slot]);
        let v = i32::from(value);
        let next = if v > Self::ENGAGE {
            1
        } else if v < -Self::ENGAGE {
            -1
        } else if prev > 0 && v > Self::RELEASE {
            1
        } else if prev < 0 && v < -Self::RELEASE {
            -1
        } else {
            0
        };
        if next != prev {
            let entry = self.held.entry(pad).or_insert([0, 0]);
            entry[slot] = next;
            if *entry == [0, 0] {
                self.held.remove(&pad);
            }
        }
        (prev, next)
    }

    pub(crate) fn remove_pad(&mut self, pad: u32) {
        self.held.remove(&pad);
    }

    pub(crate) fn any_held(&self) -> bool {
        self.held.values().any(|&[x, y]| x != 0 || y != 0)
    }
}

/// The launcher's target window mode from the Window and Windowed launcher settings.
pub(crate) fn effective_launcher_window_mode(window_mode: WindowMode, windowed_launcher: bool) -> WindowMode {
    if windowed_launcher { WindowMode::Windowed } else { window_mode }
}

impl App {
    /// Steps a held D-pad direction or stick again (nav_repeat.rs). A hold whose release went to
    /// the stream, or whose controller left, is checked against the controller and dropped.
    /// True when it stepped.
    pub(crate) fn repeat_navigation(&mut self) -> bool {
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
    pub(super) fn push_nav(&mut self, key: egui::Key) {
        let events = if self.streaming.is_none() { self.launcher.controller_key(&self.gui.ctx, key) } else { gui::tap(key).to_vec() };
        for e in events {
            self.gui.push(e);
        }
    }

    /// Puts the launcher's window in the effective launcher mode: Windowed when Windowed launcher
    /// is on or Window is Windowed, otherwise fullscreen on the desktop's mode without a stream's
    /// exclusive mode. Under gamescope, straight to the display, and on the second screen the
    /// window stays as the start made it.
    pub(crate) fn apply_launcher_window_mode(&mut self) {
        let wanted = effective_launcher_window_mode(
            self.launcher.settings.window_mode,
            self.launcher.settings.windowed_launcher,
        );
        self.launcher_window_mode = wanted;
        if handheld::fills_screen() || second_screen::this_process() { return; }
        let windowed = wanted == WindowMode::Windowed;
        // SAFETY: window calls on the main thread.
        unsafe {
            let fullscreen = SDL_GetWindowFlags(self.window) & SDL_WINDOW_FULLSCREEN != 0;
            if windowed && fullscreen {
                SDL_SetWindowFullscreen(self.window, false);
            } else if !windowed && (!fullscreen || !SDL_GetWindowFullscreenMode(self.window).is_null()) {
                SDL_SetWindowFullscreenMode(self.window, std::ptr::null());
                SDL_SetWindowFullscreen(self.window, true);
            }
        }
        self.window_changed = true;
    }

    /// Minimizing or hiding the window is leaving Nightjar for the PIN lock's grace time.
    pub(crate) fn track_visibility(&mut self, event: &SDL_Event) {
        // SAFETY: reading the event type.
        let kind = SDL_EventType(unsafe { event.r#type });
        let Some(pin) = self.launcher.pin.as_mut() else { return };
        match kind {
            SDL_EVENT_WINDOW_MINIMIZED | SDL_EVENT_WINDOW_HIDDEN => pin.left(),
            SDL_EVENT_WINDOW_RESTORED | SDL_EVENT_WINDOW_SHOWN => pin.returned(),
            _ => {}
        }
    }

    /// Cocoa delivers a URL opened by Launch Services as an SDL drop event.
    /// Queue it for the same confirmation and certificate checks as a command-line link.
    #[cfg(target_os = "macos")]
    pub(crate) fn take_pairing_url(&self, event: &SDL_Event) -> bool {
        unsafe {
            if SDL_EventType(event.r#type) != SDL_EVENT_DROP_FILE || event.drop.data.is_null() {
                return false;
            }
            if let Ok(url) = std::ffi::CStr::from_ptr(event.drop.data).to_str()
                && cli::is_link(url)
            {
                self.links.put(url.to_owned());
                return true;
            }
        }
        false
    }

    /// Saves updated native libraries and classic app lists drained from the host worker.
    pub(super) fn save_host_caches(&mut self) {
        for (unique_id, library) in self.hosts.take_library_saves() {
            if let Err(e) = self.store.save_library(&unique_id, &library) {
                eprintln!("nightjar: saving the library failed: {e}");
            }
        }
        for (unique_id, apps) in self.hosts.take_classic_saves() {
            match apps {
                Some(apps) => {
                    if let Err(e) = self.store.save_classic_apps(&unique_id, &apps) {
                        eprintln!("nightjar: saving the app list failed: {e}");
                    }
                }
                None => self.store.forget_classic_apps(&unique_id),
            }
        }
    }

    /// Handles the PC's answer to who currently plays a selected game.
    pub(super) fn handle_select_read(
        &mut self,
        id: u64,
        app: i32,
        resume: bool,
        too: bool,
        read: Option<nightjar_core::http::StreamMove>,
    ) {
                    self.select_read = false;
                    if self.streaming.is_some() || self.attempts.is_connecting() {
                        eprintln!("nightjar: another stream started while the PC said who plays; this start is dropped");
                        return;
                    }
                    let playing = read.as_ref().and_then(|read| self.hosts.get(id).and_then(|h| ui::playing_elsewhere(&h.ui_view(), app, read)));
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
                            let join = self.hosts.offers_session_guests(id);
                            self.launcher.ask_after(Dialog::ContinueHere { host: id, app: asked, resume: asked_resume, name, device, join })
                        }
                        SelectStep::Join => self.launch(id, target, true, LaunchKind::JoinRunning, LaunchFrom::Launcher),
                        SelectStep::Start => self.launch(id, app, resume, LaunchKind::Stream, LaunchFrom::Launcher),
                    }
    }

    /// Handles a move-watch poll answer while the "Now playing on" dialog is open.
    pub(super) fn handle_move_watched(
        &mut self,
        id: u64,
        app: i32,
        token: u64,
        read: Result<nightjar_core::http::StreamMove, String>,
    ) {
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

    /// The latency benchmark's part of a launcher frame (`latency_bench`): leaving its page, or a
    /// start under way, stops it; on the page the device it reports is read once, the test
    /// advances, and while the timing test runs this frame is a sample.
    pub(super) fn latency_bench_frame(&mut self) {
        if self.launcher.screen != Screen::LatencyBenchmark || self.attempts.is_connecting() || self.network_launch.is_some() || self.probe.is_some() {
            if self.latency_bench.running() || self.latency_bench.device.is_some() {
                self.latency_bench.leave();
            }
            self.launcher.latency_bench = self.latency_bench.state();
            return;
        }
        if self.latency_bench.device.is_none() {
            self.latency_bench.device = Some(self.latency_device());
        }
        self.latency_bench.step(Instant::now());
        if self.latency_bench.timing() {
            let vblank = stream::phase::timing(self.renderer, display_of(self.window).refresh_hz, 1, &mut Default::default()).map(|(latch_us, _)| latch_us.saturating_mul(1000));
            self.latency_bench.frame(vblank, session_now_ns());
        }
        self.launcher.latency_bench = self.latency_bench.state();
    }

    /// What the latency benchmark reports about this device: the display mode's refresh rate, the
    /// default output's audio format and the renderer's GPU.
    pub(crate) fn latency_device(&self) -> latency_bench_state::Device {
        let display = display_of(self.window);
        #[cfg(windows)]
        let gpu = gpu_adapters::renderer_gpu(self.renderer_device());
        #[cfg(not(windows))]
        let gpu = None;
        latency_bench_state::Device { panel_hz: display.refresh_reported.then_some(display.refresh_hz), audio: latency_bench::audio_format(), gpu }
    }

    /// An input event's age for the latency benchmark while its test runs: SDL stamps events in
    /// `SDL_GetTicksNS` time.
    pub(crate) fn latency_input(&mut self, event: &SDL_Event) {
        if !self.latency_bench.timing() {
            return;
        }
        // SAFETY: every event starts with its type and timestamp.
        let (kind, stamp) = unsafe { (SDL_EventType(event.r#type), event.common.timestamp) };
        let input = matches!(
            kind,
            SDL_EVENT_KEY_DOWN | SDL_EVENT_KEY_UP | SDL_EVENT_MOUSE_MOTION | SDL_EVENT_MOUSE_BUTTON_DOWN | SDL_EVENT_MOUSE_BUTTON_UP
                | SDL_EVENT_MOUSE_WHEEL | SDL_EVENT_GAMEPAD_AXIS_MOTION | SDL_EVENT_GAMEPAD_BUTTON_DOWN | SDL_EVENT_GAMEPAD_BUTTON_UP
                | SDL_EVENT_FINGER_DOWN | SDL_EVENT_FINGER_UP | SDL_EVENT_FINGER_MOTION | SDL_EVENT_PEN_DOWN | SDL_EVENT_PEN_UP
                | SDL_EVENT_PEN_MOTION
        );
        if input && stamp > 0 {
            // SAFETY: SDL is up.
            let age = unsafe { SDL_GetTicksNS() }.saturating_sub(stamp);
            self.latency_bench.input(age.min(i64::MAX as u64) as i64);
        }
    }

    pub(super) fn take_screenshot(&mut self, label: &str) {
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
}

/// Whether `plan`'s decoder makes its pictures on the renderer's device (D3D11VA, PyroWave on the
/// GPU), so that a stream whose device changed while connecting cannot show them.
pub(super) fn decodes_on_device(plan: &LaunchPlan) -> bool {
    matches!(plan.hardware, Hardware::D3d11(_)) || plan.gpu_decoder.is_some()
}

/// A stick or trigger past this counts as controller use for the pointer; jitter below it does not.
const POINTER_AXIS_THRESHOLD: i16 = 12000;

/// What a launcher event is for the pointer (handheld::Pointer). Touch and pen arrive as mouse
/// events too, but a tap is not the trackpad, so they leave the pointer as it is.
pub(crate) fn pointer_input(event: &SDL_Event) -> handheld::Input {
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

/// Whether anyone can see the launcher animate: its window shown, not minimized or covered, and in
/// front. Otherwise no animation asks for frames.
pub(crate) fn animations_seen(flags: SDL_WindowFlags) -> bool {
    flags & (SDL_WINDOW_MINIMIZED | SDL_WINDOW_HIDDEN | SDL_WINDOW_OCCLUDED) == 0 && flags & SDL_WINDOW_INPUT_FOCUS != 0
}

/// Whether the window is on screen at all: not minimized or hidden.
pub(crate) fn window_shown(flags: SDL_WindowFlags) -> bool {
    flags & (SDL_WINDOW_MINIMIZED | SDL_WINDOW_HIDDEN) == 0
}
