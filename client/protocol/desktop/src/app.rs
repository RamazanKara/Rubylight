//! What the app does: the methods of `App`, whose state and main loop are in main.rs.
//!
//! Here: saving, shutting down, pairing links and adding a PC. The rest by area: the launcher's events
//! and frames (`launcher`), what a page asks for (`command`), the window's renderer (`renderer`), starting
//! a stream (`launch`), a running stream (`session`) and what is drawn over the picture (`overlay`).

use super::*;

mod command;
mod control_center;
mod launch;
mod launch_probe;
mod launcher;
mod overlay;
mod renderer;
mod session;
mod win_renderer;

pub(crate) use launch::*;
pub(crate) use launch_probe::*;
pub(crate) use launcher::*;
pub(crate) use overlay::*;
pub(crate) use renderer::*;
pub(crate) use session::*;

/// The name of app `app` on `host`: its entry in the PC's list, else its game in the PC's library.
fn app_name_of(host: &hosts::Host, app: i32) -> Option<String> {
    if let Apps::Loaded(apps) = &host.apps
        && let Some(entry) = apps.iter().find(|a| a.id == app) {
            return Some(entry.name.clone());
        }
    host.library()?.games.iter().find(|g| g.app_id as i32 == app).map(|g| g.title.clone())
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

    fn save_hosts_and_place(&mut self) {
        self.save_hosts();
        // Under gamescope, or straight to the display, the window was never placed from the saved
        // place, so the desktop's stays. A fullscreen launcher keeps the windowed place saved
        // before it, and records the display it is on.
        if !handheld::fills_screen()
            && let Some(placement) = window_place::current(self.window, self.store.window_placement().as_ref())
                && let Err(e) = self.store.save_window_placement(&placement) { eprintln!("nightjar: window place not saved: {e}"); }
    }

    /// Before an update's hand-over, with nothing streaming: the next Nightjar can read these files
    /// as soon as it starts (a new AppImage, or the old program after a setup that stopped), and
    /// would overwrite with what it read what shutdown writes only later.
    pub(crate) fn save_for_next_run(&mut self) {
        self.save_edits();
        self.save_hosts_and_place();
    }

    pub(crate) fn shutdown(&mut self) {
        self.save_edits();
        self.network_test = None;
        // Its decoders may run on the renderer's device, which goes with the process.
        self.latency_bench.finish();
        self.launcher.latency_bench = self.latency_bench.state();
        // Its input closes now; the process going closes it too.
        if let Some(companion) = self.second_screen.take() {
            companion.end();
            self.second_screen_gone();
        }
        // The PC's automatic bitrate goes off after the window has gone, not with the process.
        self.gui_back_from_overlay();
        let mut abr_stops: Vec<_> = self.abr_stops.drain(..).map(|(_, stop)| stop).collect();
        let (stop, teardown) = match self.streaming.take() {
            Some((streaming, _)) => streaming.finish(self.window, &mut self.pads),
            None => (None, stream::Teardown::none()),
        };
        abr_stops.extend(stop);
        self.push_teardown(teardown);
        // Sessions an attempt set aside to stop (a recovery that gave up, a previous stop that
        // timed out) are the app's to join too.
        for ended in self.attempts.drain_teardowns() {
            self.push_teardown(ended);
        }
        self.save_hosts_and_place();
        self.pads.close_all();
        self.gui.destroy();
        let teardowns_pending = self.teardowns_busy();
        let quits_pending = self.hosts.has_pending_quits();
        if self.attempts.is_busy() || self.probe.is_some() || !self.ending_probes.is_empty() || teardowns_pending || quits_pending {
            // Ending a launch or its decoder checks can take seconds: the window goes now rather
            // than sitting frozen meanwhile.
            // SAFETY: the window run() created, still alive.
            unsafe { SDL_HideWindow(self.window) };
        }
        // The connect thread and the decoder checks may be using the renderer's GPU device (a
        // decoder being made on it), so every launch in progress or ending ends first, and a
        // session one started stops, before the renderer goes.
        let wait = stream::LEAVE_WAIT;
        let deadline = Instant::now() + wait;
        let mut checks_stopped = self.probe.take().is_none_or(|probe| probe.finish_until(deadline));
        for probe in std::mem::take(&mut self.ending_probes) {
            checks_stopped &= probe.finish_until(deadline);
        }
        let (renderer, window) = (self.renderer, self.window);
        // A session that stops from here on (a teardown, a launch ending into a reconnect) parks no
        // decoder for the reconnect: the renderer's device is going.
        stream::decoder_cache::retire();
        if !checks_stopped {
            eprintln!("nightjar: a decoder check did not end within {} ms of leaving; the window and its GPU device stay until the process exits, as it may still be using them", wait.as_millis());
        }
        // A session stopping on its own thread joins the decoder and audio threads that used the
        // renderer: they end before it goes, within the same leaving deadline.
        let teardowns_joined = std::mem::take(&mut self.teardowns)
            .into_iter()
            .all(|mut teardown| teardown.join_until(deadline));
        if !teardowns_joined {
            eprintln!("nightjar: a stream's teardown did not end within {} ms of leaving; the window and its GPU device stay until the process exits, as it may still be using them", wait.as_millis());
        }
        // The launches are cancelled and waited for either way; the renderer goes only when no
        // decoder check is still inside a call that may use it.
        stream::end_all_before(self.attempts.take_all(), deadline.saturating_duration_since(Instant::now()), || {
            // A decoder kept for a reconnect borrows or holds the renderer's device: it goes
            // before it, after the teardowns and launches above ended, which may have parked one.
            if checks_stopped && teardowns_joined && stream::decoder_cache::clear(deadline) {
                // SAFETY: tearing down what run() created, with nothing else using it any more.
                unsafe {
                    crate::sdl::destroy_renderer(renderer);
                    SDL_DestroyWindow(window);
                    // A controller's Bluetooth audio, dropped with the pads above, may still be
                    // inside a write on its own HID handle.
                    nightjar_desktop_media::dualsense_bt_audio::settle(deadline.saturating_duration_since(Instant::now()));
                    SDL_Quit();
                }
            }
        });
        for stop in abr_stops { stop.wait(); }
        self.hosts.wait_quits(deadline.saturating_duration_since(Instant::now()));
    }

    /// Drops the saved library of a PC that is unpaired or removed.
    pub(crate) fn forget_library(&self, host: u64) {
        if let Some(unique_id) = self.hosts.get(host).map(|h| h.record.unique_id.clone()).filter(|id| !id.is_empty()) {
            self.store.forget_library(&unique_id);
        }
    }

    pub(crate) fn save_hosts(&mut self) {
        if let Err(e) = self.host_store.save_records(&self.hosts.records()) {
            eprintln!("nightjar: saving hosts failed: {e}");
            self.save_failed();
        }
    }

    /// Says once a run that a save failed, with the folder Nightjar could not write to: without
    /// it a read-only config folder drops every change at the next start without a word. The
    /// change itself stays for this run. The log keeps every failure.
    pub(crate) fn save_failed(&mut self) {
        if !first_save_failure(&mut self.save_failed_told) {
            return;
        }
        self.launcher.notify(tr("Changes could not be saved"), save_failed_text(self.store.dir()));
    }

    /// Keeps a session's teardown for the joins before a renderer switch or the exit, dropping the
    /// ones that already ended instead of letting the list grow with every stream.
    pub(crate) fn push_teardown(&mut self, teardown: stream::Teardown) {
        self.teardowns.retain(|kept| !kept.finished());
        self.teardowns.push(teardown);
    }

    /// The sessions still stopping, for a launch to wait on: its connect thread joins them before
    /// its first request, as a reconnect joins the session it replaces, and hands back the ones
    /// still running after its wait (`Attempts::drain_teardowns`). While it holds them the attempt
    /// itself is what keeps a renderer switch and the exit waiting.
    pub(crate) fn take_stopping(&mut self) -> Vec<stream::Teardown> {
        self.teardowns.retain(|kept| !kept.finished());
        std::mem::take(&mut self.teardowns)
    }

    /// Whether a session is still stopping on the renderer's GPU device: finished teardowns are
    /// dropped, and the rest block a renderer switch or the exit as a start that has ended does.
    pub(crate) fn teardowns_busy(&mut self) -> bool {
        stream::prune_teardowns(&mut self.teardowns)
    }

    /// Waits up to `wait` for the sessions still stopping to end. False says one is still using the
    /// renderer's device; its handle is kept for the next wait or the exit, never dropped.
    pub(crate) fn join_teardowns(&mut self, wait: std::time::Duration) -> bool {
        let deadline = Instant::now() + wait;
        let mut still = Vec::new();
        let mut stopped = true;
        for mut teardown in std::mem::take(&mut self.teardowns) {
            if !teardown.join_until(deadline) {
                stopped = false;
                still.push(teardown);
            }
        }
        self.teardowns = still;
        stopped
    }

    /// Adds the PC a pairing link names and pairs with its one-time PIN and certificate.
    pub(crate) fn pair_link(&mut self, link: rubylight_protocol::pairing_uri::PairingUri) {
        let id = self.hosts.add_manual(&link.host, Some(link.port));
        self.save_hosts();
        self.launcher.dialog = Dialog::Pairing { host: id, pin: String::new() };
        self.hosts.pair_link(id, link);
    }

    /// Opens an invite to play over the internet: the PC is looked for, paired as a guest and
    /// added; the answer closes the progress dialog.
    pub(crate) fn redeem_invite(&mut self, link: rubylight_protocol::invite_uri::InviteUri) {
        let name = link.shown_name();
        let name = if name.is_empty() {
            tr("the PC").to_owned()
        } else {
            name
        };
        self.launcher.dialog = Dialog::Inviting { name };
        self.hosts.redeem_invite(link);
    }

    /// Shows a link the system opened with Nightjar, once nothing else is in the way: a stream
    /// (this runs only between them), a stream being started, an open dialog or the PIN screen.
    fn open_waiting_link(&mut self) {
        if self.attempts.is_connecting() || self.network_launch.is_some() || self.probe.is_some() || !self.launcher.can_open_link() {
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
            // Typed or pasted here on purpose: asked about all the same, as it can come from anyone.
            Ok(hosts::Entry::Invite(link)) => {
                self.launcher.dialog = Dialog::ConfirmInvite { link, shown: None }
            }
            // The link holds a PIN and passphrase: it is not shown back.
            Err(hosts::LINK_INVALID) => self
                .launcher
                .message(tr("Could not pair"), tr(hosts::LINK_INVALID)),
            Err(hosts::INVITE_INVALID) => self.launcher.message(
                tr("Could not join over the internet"),
                tr(hosts::INVITE_INVALID),
            ),
            Err(why) => self.launcher.message(tr(why), text.trim()),
        }
    }
}

/// Saves the PC list, logging a failure (the caller tells the player once: [`App::save_failed`]).
pub(crate) fn save_hosts(store: &config::Store, hosts: &Hosts) -> std::io::Result<()> {
    let saved = nightjar_core::hosts::HostStore::open(store.dir())
        .with_read_only(store.is_read_only())
        .save_records(&hosts.records());
    if let Err(e) = &saved {
        eprintln!("nightjar: saving hosts failed: {e}");
    }
    saved
}

/// The words of a failed save: what happened, that the change lasts only this run, and what to
/// check. The folder is the config folder Nightjar could not write to.
pub(crate) fn save_failed_text(dir: &std::path::Path) -> String {
    trf("Nightjar could not write to {}. Changes apply until Nightjar closes, then go back to how they were; check that the folder is writable.", &[&dir.display()])
}

/// Whether this is the run's first failed save, the one the player is told: `told` then stays
/// true, so a read-only folder is said once instead of at every change.
pub(crate) fn first_save_failure(told: &mut bool) -> bool {
    !std::mem::replace(told, true)
}

/// Keeps the name chosen for a PC on this device, and saves it.
pub(crate) fn rename_host(hosts: &mut Hosts, store: &config::Store, id: u64, alias: &str) -> std::io::Result<()> {
    if hosts.rename(id, alias) {
        save_hosts(store, hosts)?;
    }
    Ok(())
}

/// Forgets a PC: its saved library, covers, recent games and favourites go with it.
pub(crate) fn remove_host(
    hosts: &mut Hosts,
    store: &config::Store,
    recents: &mut Vec<config::Recent>,
    favourites: &mut Vec<config::Favourite>,
    open_game: &mut Option<config::OpenGame>,
    id: u64,
) -> std::io::Result<()> {
    if let Some(unique_id) = hosts.get(id).map(|h| h.record.unique_id.clone()).filter(|id| !id.is_empty()) {
        (*recents, *favourites) = store.forget_host(&unique_id, recents, favourites);
        *open_game = store.open_play().game;
    }
    hosts.remove(id);
    save_hosts(store, hosts)
}

/// Minimum analog stick/trigger magnitude treated as real controller activity for streaming
/// `PadPoll` active-hold (~3%, matching `direct_motion.rs`, Nightjar #179).
pub(crate) const GAMEPAD_ACTIVITY_AXIS_THRESHOLD: u16 = 1024;

/// Minimum analog stick/trigger magnitude that breaks an idle launcher out of
/// `wait_launcher_events` when no navigation stick is held (~12%, filters stick drift, #179).
pub(crate) const LAUNCHER_AXIS_WAKE_THRESHOLD: u16 = 4000;

/// Pure classification for [`is_gamepad_activity`], testable without constructing raw SDL unions.
pub(crate) fn is_gamepad_activity_parts(
    kind: SDL_EventType,
    axis_value: Option<i16>,
    gyro_peak: Option<f32>,
) -> bool {
    match kind {
        SDL_EVENT_GAMEPAD_BUTTON_DOWN
        | SDL_EVENT_GAMEPAD_BUTTON_UP
        | SDL_EVENT_GAMEPAD_TOUCHPAD_DOWN
        | SDL_EVENT_GAMEPAD_TOUCHPAD_MOTION
        | SDL_EVENT_GAMEPAD_TOUCHPAD_UP
        | SDL_EVENT_GAMEPAD_ADDED => true,
        SDL_EVENT_GAMEPAD_AXIS_MOTION => {
            axis_value.is_some_and(|v| v.unsigned_abs() > GAMEPAD_ACTIVITY_AXIS_THRESHOLD)
        }
        SDL_EVENT_GAMEPAD_SENSOR_UPDATE => stream::direct_motion::is_gyro_peak_active(gyro_peak),
        _ => false,
    }
}

/// Whether `event` is intentional controller input (or hotplug) that should switch streaming
/// polling into its fast active cadence.
pub(crate) fn is_gamepad_activity(event: &SDL_Event) -> bool {
    // SAFETY: union members read according to SDL event type.
    unsafe {
        let kind = SDL_EventType(event.r#type);
        let axis_value = if kind == SDL_EVENT_GAMEPAD_AXIS_MOTION {
            Some(event.gaxis.value)
        } else {
            None
        };
        let gyro_peak = (kind == SDL_EVENT_GAMEPAD_SENSOR_UPDATE
            && SDL_SensorType(event.gsensor.sensor) == SDL_SENSOR_GYRO)
            .then(|| {
                if event.gsensor.data.iter().all(|v| v.is_finite()) {
                    event.gsensor.data.into_iter().map(f32::abs).fold(0.0_f32, f32::max)
                } else {
                    f32::NAN
                }
            });
        is_gamepad_activity_parts(kind, axis_value, gyro_peak)
    }
}

/// Whether a launcher event should break out of [`App::wait_launcher_events`] and redraw a frame
/// immediately. Passive controller sensor ticks, raw joystick/update-complete events ignored by
/// the launcher, and sub-threshold stick noise stay inside the wait loop when Controller Test is
/// not open (Nightjar #179).
pub(crate) fn launcher_event_wakes_frame(
    event: &SDL_Event,
    controller_test: bool,
    stick_held: [bool; 4],
) -> bool {
    if controller_test {
        return true;
    }
    // SAFETY: union members read according to SDL event type.
    unsafe {
        match SDL_EventType(event.r#type) {
            SDL_EVENT_GAMEPAD_SENSOR_UPDATE
            | SDL_EVENT_SENSOR_UPDATE
            | SDL_EVENT_GAMEPAD_UPDATE_COMPLETE
            | SDL_EVENT_GAMEPAD_REMAPPED
            | SDL_EVENT_GAMEPAD_STEAM_HANDLE_UPDATED
            | SDL_EVENT_JOYSTICK_AXIS_MOTION
            | SDL_EVENT_JOYSTICK_BALL_MOTION
            | SDL_EVENT_JOYSTICK_HAT_MOTION
            | SDL_EVENT_JOYSTICK_BUTTON_DOWN
            | SDL_EVENT_JOYSTICK_BUTTON_UP
            | SDL_EVENT_JOYSTICK_ADDED
            | SDL_EVENT_JOYSTICK_REMOVED
            | SDL_EVENT_JOYSTICK_BATTERY_UPDATED
            | SDL_EVENT_JOYSTICK_UPDATE_COMPLETE => false,
            SDL_EVENT_GAMEPAD_AXIS_MOTION => {
                stick_held.iter().any(|&held| held)
                    || event.gaxis.value.unsigned_abs() > LAUNCHER_AXIS_WAKE_THRESHOLD
            }
            _ => true,
        }
    }
}

/// Rounds a positive wait up to whole milliseconds so a sub-millisecond remainder never truncates
/// to `0` and turns `SDL_WaitEventTimeout` into a non-blocking busy-spin (Nightjar #179).
pub(crate) fn ceil_ms(duration: Duration) -> i32 {
    duration.as_nanos().div_ceil(1_000_000).min(i32::MAX as u128) as i32
}

/// Next `SDL_WaitEventTimeout` slice inside [`App::wait_launcher_events`].
pub(crate) fn launcher_wait_slice_ms(remaining: Duration, pad_wait: Duration) -> i32 {
    ceil_ms(remaining.min(pad_wait))
}

/// Adaptive polling cadence for `SDL_UpdateJoysticks()` with `SDL_HINT_AUTO_UPDATE_JOYSTICKS=0`.
///
/// SDL3's automatic joystick update forces `SDL_WaitEventTimeout` to wake every 1 ms whenever
/// any controller is open (always true on Steam Deck and Windows handhelds), even in an idle
/// launcher or while streaming with keyboard and mouse. Polling explicitly keeps active gameplay
/// responsive while cutting idle wakeups dramatically.
#[derive(Debug, Default)]
pub(crate) struct PadPoll {
    last_poll: Option<Instant>,
    last_activity: Option<Instant>,
    /// Last monotonic µs timestamp observed from `DirectMotion::last_pad_activity_us()`.
    last_direct_us: u64,
}

impl PadPoll {
    /// Hotplug check when no controllers are open (or controllers are disabled for a stream).
    pub(crate) const ENUM_INTERVAL: Duration = Duration::from_secs(2);
    /// Launcher UI cadence when a controller is connected and the window is visible.
    pub(crate) const LAUNCHER_INTERVAL: Duration = Duration::from_millis(16);
    /// Launcher cadence while minimized or hidden.
    pub(crate) const LAUNCHER_HIDDEN_INTERVAL: Duration = Duration::from_millis(250);
    /// Stream cadence while a controller was used recently: 1000 Hz, as SDL's own poll.
    pub(crate) const STREAM_ACTIVE_INTERVAL: Duration = Duration::from_millis(1);
    /// Stream cadence while open controllers have been idle (mouse/keyboard play, cutscenes): the
    /// first input after a pause waits at most this long.
    pub(crate) const STREAM_IDLE_INTERVAL: Duration = Duration::from_millis(4);
    /// How long after the last controller input the stream stays on `STREAM_ACTIVE_INTERVAL`.
    pub(crate) const ACTIVE_HOLD: Duration = Duration::from_secs(1);

    /// Target polling interval for the current state.
    pub(crate) fn interval(&self, now: Instant, pads_open: bool, streaming: bool, sends_to_stream: bool, window_seen: bool) -> Duration {
        if !pads_open {
            return Self::ENUM_INTERVAL;
        }
        if !streaming {
            return if window_seen { Self::LAUNCHER_INTERVAL } else { Self::LAUNCHER_HIDDEN_INTERVAL };
        }
        if !sends_to_stream {
            return Self::ENUM_INTERVAL;
        }
        if self.last_activity.is_some_and(|at| now.saturating_duration_since(at) < Self::ACTIVE_HOLD) {
            Self::STREAM_ACTIVE_INTERVAL
        } else {
            Self::STREAM_IDLE_INTERVAL
        }
    }

    /// How long until the next `SDL_UpdateJoysticks()` call is due (`Duration::ZERO` when due now).
    pub(crate) fn next_in(&self, now: Instant, pads_open: bool, streaming: bool, sends_to_stream: bool, window_seen: bool) -> Duration {
        let Some(last) = self.last_poll else {
            return Duration::ZERO;
        };
        let interval = self.interval(now, pads_open, streaming, sends_to_stream, window_seen);
        interval.saturating_sub(now.saturating_duration_since(last))
    }

    /// Marks that `SDL_UpdateJoysticks()` just ran.
    pub(crate) fn polled(&mut self, now: Instant) {
        self.last_poll = Some(now);
    }

    /// Records controller input (or a controller connect) so streaming uses the active cadence.
    pub(crate) fn note_activity(&mut self, now: Instant) {
        self.last_activity = Some(now);
    }

    /// Syncs controller events handled directly by `DirectMotion`'s event filter during present
    /// (those events are consumed by the filter and never reach the main loop).
    pub(crate) fn sync_direct(&mut self, direct_pad_us: u64, now: Instant) {
        if direct_pad_us != 0 && direct_pad_us != self.last_direct_us {
            self.last_direct_us = direct_pad_us;
            self.note_activity(now);
        }
    }
}

/// How `stream_frame` waits out a short duration: `SDL_WaitEventTimeout` above 1.5 ms,
/// `SDL_DelayNS` (kernel high-resolution timer, no CPU busy-spin) for normal sub-1.5 ms tails,
/// and `SDL_DelayPrecise` only for Beam Race slices (`precise`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StreamSleep {
    /// Wait for SDL events up to this many milliseconds.
    EventTimeoutMs(i32),
    /// Kernel timer sleep for this many nanoseconds (clamped >= 1 us so Windows' waitable timer
    /// never rounds a positive sub-microsecond wait to zero).
    DelayNs(u64),
    /// High-precision spin-sleep for this many nanoseconds (Beam Race only).
    DelayPreciseNs(u64),
}

pub(crate) fn stream_sleep(duration: Duration, precise: bool, pad_wait: Option<Duration>) -> StreamSleep {
    if duration > Duration::from_micros(1500) {
        let frame_ms = (duration.as_micros() / 1000).saturating_sub(1).max(1).min(i32::MAX as u128) as i32;
        let ms = match pad_wait {
            Some(pad) => frame_ms.min(ceil_ms(pad).max(1)),
            None => frame_ms,
        };
        StreamSleep::EventTimeoutMs(ms)
    } else if precise {
        StreamSleep::DelayPreciseNs(duration.as_nanos() as u64)
    } else {
        StreamSleep::DelayNs((duration.as_nanos() as u64).max(1_000))
    }
}

impl App {
    fn poll_launcher_event(&mut self, event: &SDL_Event) -> bool {
        let controller_test =
            self.launcher.screen == Screen::Controllers && self.launcher.controller_test;
        let woke = launcher_event_wakes_frame(event, controller_test, self.stick_held);
        self.latency_input(event);
        self.handle_launcher_event(event);
        woke
    }

    /// Polls launcher events with adaptive joystick cadence instead of SDL's 1000 Hz wakeup.
    /// Loops internally across 16 ms joystick poll slices until either a waking event arrives or
    /// `timeout` elapses, avoiding redundant launcher redraws while a controller sits idle (#179).
    pub(super) fn wait_launcher_events(&mut self, timeout: i32, seen: bool) {
        let deadline = if timeout > 0 {
            Some(Instant::now() + Duration::from_millis(timeout as u64))
        } else {
            None
        };
        loop {
            let now = Instant::now();
            let pads_open = !self.pads.pads.is_empty();
            if self.pad_poll.next_in(now, pads_open, false, false, seen).is_zero() {
                // SAFETY: SDL joystick update on the main thread.
                unsafe { SDL_UpdateJoysticks() };
                self.pad_poll.polled(now);
            }
            // SAFETY: event polling on the main thread.
            let mut event: SDL_Event = unsafe { std::mem::zeroed() };
            let mut woke = false;
            while unsafe { SDL_PollEvent(&mut event) } {
                woke |= self.poll_launcher_event(&event);
            }
            if woke || !self.running {
                return;
            }
            let Some(deadline) = deadline else {
                return;
            };
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return;
            }
            let pad_wait =
                self.pad_poll.next_in(Instant::now(), !self.pads.pads.is_empty(), false, false, seen);
            if pad_wait.is_zero() {
                continue;
            }
            let slice = launcher_wait_slice_ms(remaining, pad_wait).max(1);
            // SAFETY: event polling on the main thread.
            if unsafe { SDL_WaitEventTimeout(&mut event, slice) } {
                woke |= self.poll_launcher_event(&event);
                while unsafe { SDL_PollEvent(&mut event) } {
                    woke |= self.poll_launcher_event(&event);
                }
                if woke || !self.running {
                    return;
                }
            }
        }
    }

    /// Waits for stream events or the next present time, using `SDL_DelayNS` for sub-1.5 ms tails
    /// (and `SDL_DelayPrecise` only for Beam Race) with adaptive joystick polling.
    pub(super) fn wait_stream_events(
        &mut self,
        wait: stream::Wait,
        menu_frame: Duration,
        menu_visible: bool,
        pad_active: bool,
        direct_pad_us: u64,
    ) {
        let now = Instant::now();
        self.pad_poll.sync_direct(direct_pad_us, now);
        let pads_open = !self.pads.pads.is_empty();
        if self.pad_poll.next_in(now, pads_open, true, pad_active, true).is_zero() {
            // SAFETY: SDL joystick update on the main thread.
            unsafe { SDL_UpdateJoysticks() };
            self.pad_poll.polled(now);
        }
        let pad_wait = (pads_open && pad_active)
            .then(|| self.pad_poll.next_in(now, pads_open, true, pad_active, true));
        // SAFETY: event calls on the main thread.
        unsafe {
            let mut event: SDL_Event = std::mem::zeroed();
            match wait {
                stream::Wait::Now => {}
                stream::Wait::For(duration) | stream::Wait::Precise(duration) => {
                    let precise = matches!(wait, stream::Wait::Precise(_));
                    // A held direction in the stream menu wakes the loop for its next step.
                    let duration = self.nav_repeat.next(now).map_or(duration, |next| duration.min(next + Duration::from_millis(1)));
                    // An animating menu wakes it for egui's next frame, at most once per refresh.
                    let duration = match self.gui.repaint_in(menu_frame) {
                        Some(wait) if menu_visible => duration.min(wait),
                        _ => duration,
                    };
                    match stream_sleep(duration, precise, pad_wait) {
                        StreamSleep::EventTimeoutMs(ms) => {
                            if SDL_WaitEventTimeout(&mut event, ms) {
                                self.dispatch_stream_event(&event);
                            }
                        }
                        StreamSleep::DelayNs(ns) => SDL_DelayNS(ns),
                        StreamSleep::DelayPreciseNs(ns) => SDL_DelayPrecise(ns),
                    }
                }
            }
            let after = Instant::now();
            if self.pad_poll.next_in(after, !self.pads.pads.is_empty(), true, pad_active, true).is_zero() {
                SDL_UpdateJoysticks();
                self.pad_poll.polled(after);
            }
            while SDL_PollEvent(&mut event) {
                self.dispatch_stream_event(&event);
            }
        }
    }
}

#[cfg(test)]
pub(crate) use win_renderer::windows_needs_linear;

#[cfg(test)]
mod tests;
