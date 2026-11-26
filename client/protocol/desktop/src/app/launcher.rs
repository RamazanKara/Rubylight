//! The launcher: its events, controller navigation and frame, the latency bench's frame, and the file
//! dialogs of the shared folder browser.

use super::*;
use nightjar_desktop_media::shortcuts_runtime::ButtonSdlExt;

mod helpers;
pub(crate) use helpers::{
    animations_seen, effective_launcher_window_mode, pointer_input, window_shown, FolderPick,
};
use helpers::decodes_on_device;

impl App {
    /// Gamepad buttons and the left stick become arrow keys, Enter and Escape for egui.
    pub(crate) fn navigate(&mut self, event: &SDL_Event) {
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
                                if self.streaming.is_none() && matches!(self.launcher.dialog, Dialog::None) && !self.attempts.is_connecting() && self.probe.is_none() {
                                    if let Some(command) = self.launcher.screen.save_command() { self.command(command); }
                                    self.launcher.open_settings();
                                }
                                None
                            }
                            Some(action @ (Action::PreviousTab | Action::NextTab)) => {
                                if self.streaming.is_none() && !self.attempts.is_connecting() && self.probe.is_none() {
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

    pub(super) fn handle_launcher_event(&mut self, event: &SDL_Event) {
        #[cfg(target_os = "macos")]
        if self.take_pairing_url(event) { return; }
        log_window_hdr(event);
        self.track_visibility(event);
        // SAFETY: reading the event type.
        self.window_changed |= window_needs_a_frame(SDL_EventType(unsafe { event.r#type }));
        if let Some(source) = input_source::of(event, |id| self.pads.real_pad_type(id)) {
            if matches!(source, input_source::InputSource::Pad(_)) {
                self.screen_awake.active(Instant::now());
            }
            self.launcher.input_source = source;
        }
        // SAFETY: reading the event type.
        let kind = SDL_EventType(unsafe { event.r#type });
        if is_gamepad_activity(event) {
            self.pad_poll.note_activity(Instant::now());
        }
        self.note_render_device(kind);
        if kind == SDL_EVENT_SYSTEM_THEME_CHANGED {
            // The System theme follows at once: the next frame has the other look.
            self.launcher.system_light = sdl::system_light();
            self.window_changed = true;
        }
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
                        self.launcher.notify(tr("Controller not connected"), why.message());
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
                SDL_EVENT_KEY_DOWN if self.launcher.recording_hotkey() && !hotkey_sdl::moves_in_hotkey_dialog(event.key.scancode, event.key.r#mod) => {
                    if let Some(command) = self.launcher.hotkey_input(hotkey_sdl::hotkey_input(event.key.scancode, event.key.r#mod, event.key.repeat)) {
                        self.command(command);
                    }
                }
                SDL_EVENT_KEY_DOWN if shortcuts_runtime::key_action(Scope::Launcher, event.key.scancode, event.key.r#mod) == Some(Action::Screenshot) => {
                    self.screenshot_requested = true;
                }
                SDL_EVENT_KEY_DOWN if shortcuts_runtime::key_action(Scope::Launcher, event.key.scancode, event.key.r#mod) == Some(Action::Fullscreen) => {
                    let fullscreen = SDL_GetWindowFlags(self.window) & SDL_WINDOW_FULLSCREEN != 0;
                    SDL_SetWindowFullscreen(self.window, !fullscreen);
                }
                _ => {
                    self.gui.handle(event, scale);
                }
            }
        }
    }

    pub(crate) fn launcher_frame(&mut self) {
        // Ending attempts that their workers let go of since the last frame (a stuck one stays):
        // the settings below must see what is still in use, not what ended moments ago.
        self.attempts.reap();
        for ended in self.attempts.drain_teardowns() {
            self.push_teardown(ended);
        }
        // A controller paused while it was forwarded over USB comes back after the stream.
        if self.pads.arbitration_due() {
            for why in self.pads.arbitrate(None) {
                self.launcher.notify(tr("Controller not connected"), why.message());
            }
        }
        #[cfg(target_os = "linux")]
        self.apply_renderer_choice();
        #[cfg(windows)]
        self.apply_graphics_card();
        // Wait for input or a worker; animate the spinner while something runs.
        let busy = self.attempts.is_connecting() || self.waking.host().is_some() || self.network_launch.is_some() || self.network_test.is_some()
            || self.probe.is_some()
            || self.launcher.remote.running() || self.launcher.files.busy()
            || matches!(self.launcher.dialog, Dialog::Pairing { .. })
            || self.identity.is_some() || self.autostream.is_some()
            || self.launcher.pin.as_ref().is_some_and(pin_ui::PinLock::busy);
        let timeout = if busy { 16 } else { 250 };
        // A held direction wakes the loop for its next step.
        let timeout = self.nav_repeat.next(Instant::now()).map_or(timeout, |next| timeout.min(next.as_millis().min(250) as i32 + 1));
        // So does egui while it animates or settles a layout, at most once per refresh: without
        // this, a scroll, a hover fade or an opening list moved only when the next input came.
        // Not while nobody can see it: minimized, hidden, covered or in the background, the loop
        // keeps its own wake for workers and input.
        let frame = Duration::from_secs_f32(1.0 / display_of(self.window).refresh_hz.max(30.0));
        // SAFETY: a live window, read on the main thread.
        let seen = animations_seen(unsafe { SDL_GetWindowFlags(self.window) });
        let timeout = if self.is_renderer_lost() { 0 } else if seen { self.gui.repaint_in(frame).map_or(timeout, |wait| timeout.min(ceil_ms(wait))) } else { timeout };
        self.wait_launcher_events(timeout, seen);
        if !self.running {
            return;
        }
        // Before a stream this frame may start, which holds the screen itself.
        self.screen_awake.apply(Instant::now());
        // Before anything this frame launches or draws: SDL lost its device (`recover_renderer`).
        #[cfg(windows)]
        if self.is_renderer_lost() {
            self.recover_renderer();
        }
        self.repeat_navigation();
        self.open_waiting_link();
        #[cfg(feature = "demo-library")]
        self.demo_steps();
        let test_events: Vec<_> = self.network_test.as_ref().map(|test| test.events.try_iter().collect()).unwrap_or_default();
        let calibrate = self.network_test.as_ref().is_some_and(|test| test.calibrate);
        for event in test_events {
            match event {
                network_test::Event::Progress(stage) => self.launcher.dialog = Dialog::NetworkTest { stage, calibrate },
                network_test::Event::Done(result) => {
                    let host = self.network_test.take().unwrap().host;
                    match result {
                        // Its result replaces the running test's dialog, as Android's does.
                        Ok(network_test::Outcome::Connection(result)) => self.launcher.ask(Dialog::ConnectionTest { host, result }),
                        // A pace confirmed twice is a notice; one past some loss limits says why in a dialog.
                        Ok(network_test::Outcome::Pace(calibration, _)) if calibration.qualified => {
                            self.launcher.toast(trf("Saved {} Mbps frame pace", &[&calibration.pace_mbps]));
                            if let Some(host) = self.hosts.get_mut(host) { host.record.pace = Some(calibration); self.save_hosts(); }
                        }
                        Ok(network_test::Outcome::Pace(calibration, measurement)) => {
                            let text = trf("Saved {} Mbps frame pace ({}). Packet loss {}%, worst 100 ms loss {}%, missing frame tails {}%. The calibration applies while the network and endpoint link speeds match.",
                                &[&calibration.pace_mbps, &if calibration.qualified { tr("confirmed twice") } else { tr("best measured result; some loss limits exceeded") },
                                &format!("{:.2}", measurement.loss), &format!("{:.2}", measurement.worst_window_loss), &format!("{:.2}", measurement.tail_loss())]);
                            if let Some(host) = self.hosts.get_mut(host) { host.record.pace = Some(calibration); self.save_hosts(); }
                            self.launcher.notify(tr("PyroWave calibration"), text);
                        }
                        Err(error) => self.launcher.notify(tr("Network test failed"), error),
                    }
                }
            }
        }
        // Before a start's own read, which is the newer of the two when both are in.
        self.network_watch.pace(self.launcher.screen.network_pace());
        if let Some(network) = self.network_watch.latest() {
            self.launcher.network = network;
        }
        // A start's decoder checks, run on a worker since the last frame.
        self.probe_step();
        if let Some(receiver) = &self.network_query
            && let Ok(network) = receiver.try_recv() {
                self.launcher.network = network;
                self.network_query = None;
                if let Some((host, app, resume, kind)) = self.network_launch.take() { self.launch_ready(host, app, resume, kind); }
            }
        if let Some((export, receiver)) = &self.shortcut_file
            && let Ok(result) = receiver.try_recv() {
                let export = *export;
                self.shortcut_file = None;
                match result {
                    Ok(Some(path)) if export => {
                        match std::fs::write(&path, self.launcher.shortcuts.encode()) {
                            Ok(()) => self.launcher.toast(tr("Shortcuts exported")),
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
        if let Some(result) = self.latency_csv_file.as_ref().and_then(|receiver| receiver.try_recv().ok()) {
            self.latency_csv_file = None;
            let kept = self.store.dir().join("statistics").join(stream::LATENCY_CSV);
            match result.map(|chosen| chosen.map(|path| std::fs::copy(&kept, &path))) {
                Ok(None) => {}
                Ok(Some(Ok(_))) => self.launcher.toast(tr("Latency CSV saved.")),
                Ok(Some(Err(e))) => {
                    eprintln!("nightjar: latency CSV: {e}");
                    self.launcher.toast(tr("Could not save the latency CSV."));
                }
                Err(e) => self.launcher.message(tr("File dialog unavailable"), e),
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
                self.launcher.notify(tr(offer.action.failed()), why);
            }
        }
        if let Some(result) = self.folder_pick.as_ref().and_then(|(_, receiver)| receiver.try_recv().ok())
            && let Some((pick, _)) = self.folder_pick.take()
                && let Some((title, body)) = finish_folder_pick(&mut self.launcher.folders, pick, result) {
                    self.launcher.message(title, body);
                }
        if let Some(receiver) = &self.identity
            && let Ok(result) = receiver.try_recv() {
                self.identity = None;
                match result {
                    Ok(identity) => {
                        let saved = nightjar_core::hosts::IdentityFile {
                            unique_id: identity.unique_id().to_owned(),
                            certificate_pem: identity.certificate_pem().to_owned(),
                            private_key_pem: identity.private_key_pem().to_owned(),
                        };
                        if let Err(e) = self.identity_store.save_identity(&saved) {
                            eprintln!("nightjar: saving the pairing key failed: {e}");
                            self.launcher.notify(
                                crate::strings::tr("Could not save this choice"),
                                crate::strings::trf(
                                    "The pairing key could not be saved, so the PCs paired with this device have to be paired again after a restart: {}",
                                    &[&e],
                                ),
                            );
                        }
                        self.hosts.set_identity(Arc::new(identity));
                        self.launcher.identity_ready = true;
                        self.hosts.poll(true, None);
                    }
                    Err(e) => self.launcher.notify(tr("No pairing key"), e),
                }
            }
        // While the window is minimized or hidden only the PC being woken and those just quit or
        // left are polled: the others are asked again, all at once, when it is back.
        if !self.attempts.is_connecting() {
            // SAFETY: a live window, read on the main thread.
            let shown = window_shown(unsafe { SDL_GetWindowFlags(self.window) });
            self.hosts.poll(shown, self.waking.host());
        }
        let (events, changed) = self.hosts.drain(&self.gui.ctx);
        if changed {
            self.save_hosts();
        }
        self.show_pending_end();
        self.save_host_caches();
        for event in events {
            match event {
                HostEvent::Invited { result, id } => {
                    if matches!(self.launcher.dialog, Dialog::Inviting { .. }) {
                        self.launcher.dialog = Dialog::None;
                    }
                    match (result, id) {
                        (Ok(redeemed), Some(id)) => {
                            self.save_hosts();
                            self.hosts.load_apps(id);
                            self.launcher.open(Screen::Apps(id));
                            self.launcher.notify(
                                tr("Invite accepted"),
                                trf("{} is on your list now. Join a game someone plays there; they let you in.", &[&redeemed.name]),
                            );
                        }
                        (Ok(_), None) => {}
                        (Err(error), _) => self.launcher.notify(
                            tr("Could not join over the internet"),
                            crate::hosts::invite_error(&error),
                        ),
                    }
                }
                HostEvent::Paired { id, result, .. } => {
                    let id = self.hosts.canonical_id(id);
                    if let Dialog::Pairing { host, .. } = &mut self.launcher.dialog
                        && self.hosts.canonical_id(*host) == id
                    {
                        *host = id;
                    }
                    // The first pairing on this install: Home says what Auto set up for this PC.
                    if result.is_ok() && self.launcher.offer_setup_card(id, self.hosts.paired_with(id))
                        && let Err(e) = self.store.save_settings(&self.launcher.settings) {
                            eprintln!("nightjar: saving settings failed: {e}");
                        }
                    if self.launcher.pairing_finished(id, result.map(|_| ())) {
                        self.save_hosts();
                        self.hosts.load_apps(id);
                        self.launcher.open(Screen::Apps(id));
                    }
                }
                HostEvent::Quit { id, result } => match result {
                    Err(e) if self.quitting_on_leave.as_ref().is_some_and(|(host, _)| *host == id) => {
                        let (_, name) = self.quitting_on_leave.take().expect("checked above");
                        self.launcher.notify(tr("Could not quit the game"), crate::hosts::quit_failure(&e, &name));
                    }
                    Ok(()) => {
                        if self.quitting_on_leave.as_ref().is_some_and(|(host, _)| *host == id) { self.quitting_on_leave = None; }
                        if self.pending_launch.is_some_and(|(host, _)| host == id) {
                            let (_, app) = self.pending_launch.take().expect("checked above");
                            self.launch(id, app, false, LaunchKind::Stream, LaunchFrom::Replace);
                        }
                    }
                    Err(e) => {
                        if self.pending_launch.is_some_and(|(host, _)| host == id) {
                            self.pending_launch = None;
                        }
                        let name = self.hosts.get(id).map_or_else(|| tr("the running app").to_owned(), |h| crate::ui::running_name(&h.ui_view()));
                        self.launcher.notify(tr("Could not quit the game"), crate::hosts::quit_failure(&e, &name));
                    }
                },
                // A packet that could not be sent at all (no network, no route) ends the wait.
                // Only the running wake ends: a send of a cancelled one may fail after a new one began.
                HostEvent::Wake { id, run, result: Err(e) } => {
                    if self.waking.failed(id, run) {
                        self.launcher.notify(tr("Could not wake the PC"), e);
                    }
                }
                HostEvent::Sleep { id, ask, name, confirmed, result } => self.launcher.sleep_answered(id, ask, &name, confirmed, result.map(|s| s.into_ui(&mut self.pending_power_handed, ask))),
                HostEvent::PcPower { id, ask, name, action, confirmed, result } => self.launcher.pc_power_answered(id, ask, &name, action, confirmed, result.map(|s| s.into_ui(&mut self.pending_power_handed, ask))),
                HostEvent::HostUpdate { id, name, result } => self.launcher.update_answered(id, &name, result.map(hosts::UpdateStep::into_ui)),
                HostEvent::PairingAnswer { id, request, name, denied, result } => self.launcher.pairing_answered(id, &request, &name, denied, result),
                HostEvent::Unpaired { id, result } => {
                    if crate::hosts::now_unpaired(&result) {
                        self.forget_library(id);
                    }
                    if let Err(e) = result {
                        self.launcher.notify(tr("Could not unpair"), e);
                    }
                }
                HostEvent::Session { action, result: Err(e), .. } => {
                    let title = match action {
                        ActionId::CloseGame => tr("Could not quit the game"),
                        ActionId::End => tr("Could not end the session"),
                        _ => tr("The session action failed"),
                    };
                    self.launcher.notify(title, e);
                }
                // Nothing changed on the PC: the game keeps running and the prompt says why not. No
                // stream follows to use the PIN's grant.
                HostEvent::ContinueHere { result: Err(e), .. } => {
                    self.forget_pin_grant();
                    self.launcher.notify(tr("Could not continue here"), e);
                }
                HostEvent::ContinueHere { id, app, result: Ok(()) } => self.launch(id, app, true, LaunchKind::Stream, LaunchFrom::Launcher),
                HostEvent::SelectRead { id, app, resume, too, read } => self.handle_select_read(id, app, resume, too, read),
                HostEvent::StreamMoved { id, app, device: Some(device) } if self.streaming.is_none() && !self.attempts.is_connecting() => {
                    eprintln!("nightjar: the stream moved to {device}");
                    self.show_moved(id, app, device);
                }
                HostEvent::MoveWatched { id, app, token, read } => self.handle_move_watched(id, app, token, read),
                HostEvent::Action { result: Err(e), .. } => self.launcher.notify(tr("The PC action failed"), e),
                _ => {}
            }
        }
        // A launch without a network query in flight is waiting for its app list (see
        // `launch_ready`); it goes on once the list is no longer loading.
        if self.network_query.is_none()
            && let Some((host, app, resume, kind)) = self.network_launch
                && !matches!(self.hosts.get(host).map(|h| &h.apps), Some(Apps::Loading)) {
                    self.network_launch = None;
                    self.launch_ready(host, app, resume, kind);
                }
        self.wake_step();
        self.replace_step();
        self.autostream_step();
        self.cli_step();
        self.second_screen_step();
        // A cancelled start's worker may still be inside a call that cannot be stopped; it ends on
        // its own, and a stream it makes late is stopped there.
        self.attempts.reap();
        for ended in self.attempts.drain_teardowns() {
            self.push_teardown(ended);
        }
        // The worker's answer comes first: one that arrives just as its deadline passes is a
        // result, not silence (a start that keeps reporting stages moves its deadline, `poll`).
        let mut answered = false;
        if let Some(c) = self.attempts.current_mut() {
            match c.poll() {
                Ok(Some((session, facts))) => {
                    answered = true;
                    let connecting = self.attempts.take().expect("connecting");
                    for ended in connecting.take_teardowns() {
                        self.push_teardown(ended);
                    }
                    if decodes_on_device(connecting.plan()) && !connecting.plan().device.holds(self.renderer_device()) {
                        // SDL made its Direct3D 11 device again while connecting (a driver reset):
                        // the decoder's pictures are on the old device, which nothing draws.
                        let why = connecting.plan().device.removed_reason().map_or(String::new(), |why| format!(" (the old one was removed: {why})"));
                        eprintln!("nightjar: the renderer's Direct3D 11 device changed while connecting{why}; the stream ends");
                        self.push_teardown(stream::reap_session(session));
                        self.launcher.notify(tr("The stream could not start"), tr("The graphics driver was reset, which ended the stream. Connect again."));
                    } else {
                        self.begin_stream(connecting, session, facts);
                        return;
                    }
                }
                Ok(None) => {}
                Err(failed) => {
                    answered = true;
                    // A recovery that gave up may have set a session aside to stop: keep its
                    // teardown before the attempt goes.
                    for ended in self.attempts.drain_teardowns() {
                        self.push_teardown(ended);
                    }
                    let gone = self.attempts.take();
                    match failed {
                        ConnectFailed::Message(e) => match gone {
                            Some(c) => {
                                let plan = c.plan();
                                let launch = (plan.host_id, plan.app.id, plan.resume, LaunchKind::of_plan(plan.join, plan.small_stream_input_only, plan.mic_only));
                                let mode = stream::stream_mode(&c.settings, plan.display);
                                self.start_failed(launch, Some(c.app_name.clone()), Some(mode), e);
                            }
                            None => self.launcher.message(tr("The stream could not start"), e),
                        },
                        ConnectFailed::ConfirmReplace(plan) => {
                            if !self.open_play_game_running() {
                                self.ask_replace(plan);
                            }
                        }
                        ConnectFailed::Moved { host, app, device } => self.show_moved(host, app, device),
                        ConnectFailed::PackedRefused(plan) => self.restart_without_full_chroma(&plan),
                    }
                }
            }
        }
        if !answered && self.attempts.overdue(Instant::now()) {
            // The worker stopped answering (a decoder made or a device created while the driver
            // resets): the screen ends the attempt and says so instead of waiting for ever.
            let connecting = self.attempts.take().expect("an overdue start");
            eprintln!("nightjar: the start did not answer within {} s; the attempt ends", stream::CONNECT_TIMEOUT.as_secs());
            let plan = connecting.plan();
            let launch = (plan.host_id, plan.app.id, plan.resume, LaunchKind::of_plan(plan.join, plan.small_stream_input_only, plan.mic_only));
            let mode = stream::stream_mode(&connecting.settings, plan.display);
            let name = connecting.app_name.clone();
            self.attempts.end(connecting);
            self.start_failed(launch, Some(name), Some(mode), stream::connect_timeout_text());
        }

        let connecting = self.attempts.current().map(|c| ConnectingView {
            app: c.app_name.clone(),
            host: c.host_name.clone(),
            stage: c.stage.clone(),
            host_id: c.host_id,
            app_id: c.plan().app.id,
        }).or_else(|| self.probe.as_ref().map(|probe| {
            let (host, app, ..) = probe.launch();
            let pc = self.hosts.get(host);
            ConnectingView {
                app: pc.and_then(|h| app_name_of(h, app)).unwrap_or_else(|| probe.app_name().to_owned()),
                host: pc.map_or(String::new(), |h| h.display_name().to_owned()),
                stage: strings::en("Connecting").into(),
                host_id: host,
                app_id: app,
            }
        })).or_else(|| self.network_launch.map(|(host, app, ..)| {
            let pc = self.hosts.get(host);
            ConnectingView {
                app: pc.and_then(|h| app_name_of(h, app)).unwrap_or_else(|| tr("Starting stream").into()),
                host: pc.map_or(String::new(), |h| h.display_name().to_owned()),
                // What the player waits for; the network read behind it is the log's business.
                stage: strings::en("Connecting").into(),
                host_id: host,
                app_id: app,
            }
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
        if let Some(toast) = self.updates.answer().and_then(update_check::Answer::toast) {
            self.launcher.toast(toast);
        }
        self.latency_bench_frame();
        #[cfg(feature = "demo-library")]
        let demo_scrolling = self.demo_scrolling();
        let mut commands = Vec::new();
        let launcher = &mut self.launcher;
        let hosts_view = self.hosts.ui_view();
        let pads_view = self.pads.view();
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
                    hosts: &hosts_view,
                    pads: &pads_view,
                    connecting: connecting.clone(),
                    waking: waking.clone(),
                },
            );
        });
        // Before the frame's commands, so a cover it asks for is sized for this window.
        self.hosts.covers_shown(&self.gui.ctx, &self.store);
        self.gui.platform(self.window, &output.platform_output);
        if self.title_bar != *theme::NIGHT {
            self.title_bar = *theme::NIGHT;
            crate::blend_title_bar(self.window, self.title_bar, *theme::ON_SURFACE);
        }
        let renderer = self.renderer;
        // On a PQ renderer the launcher draws its white at the compositor's SDR white.
        #[cfg(target_os = "linux")]
        let rescaled = sdl::launcher_white(renderer, self.streaming.is_none());
        #[cfg(not(target_os = "linux"))]
        let rescaled = false;
        // The latency benchmark's frames each reach the screen, paced by V-Sync, as Android's
        // Choreographer frames are.
        let must_draw = std::mem::take(&mut self.window_changed) || self.screenshot_requested || rescaled || self.latency_bench.timing();
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
        let mut present_failed = false;
        if drew && !self.is_renderer_lost() {
            #[cfg(feature = "demo-library")]
            self.demo_drew();
            self.take_screenshot("launcher");
            // SAFETY: a live renderer.
            present_failed = !unsafe { SDL_RenderPresent(self.renderer) };
            if !present_failed {
                self.latency_bench.presented(session_now_ns());
                self.launcher.latency_bench = self.latency_bench.state();
            }
            static FIRST_FRAME_SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if let Some(line) = first_frame_line(&FIRST_FRAME_SAID, LAUNCH.get().map(|at| at.elapsed())) {
                eprintln!("{line}");
            }
        }
        // Recover a lost Windows D3D11 device before a command launches on it.
        self.recover_after_launcher_present(present_failed);
        for command in commands {
            self.command(command);
        }
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

/// Opens the dialog choosing where the shared folder browser of `host` saves a download, or
/// what it uploads. One dialog at a time: with one open, the page says so.
pub(crate) fn start_folder_pick(
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
pub(crate) fn finish_folder_pick(
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
