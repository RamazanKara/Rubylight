//! The launcher: its events, controller navigation and frame, the latency bench's frame, and the file
//! dialogs of the shared folder browser.

use super::*;

/// What a file dialog of the shared folder browser was opened for, with where the browser was
/// then.
pub(crate) enum FolderPick {
    /// The folder to save the PC's file `name` in.
    Download { host: u64, at: folders_page::Place, name: String, size: u64 },
    /// The file to upload.
    Upload { host: u64, at: folders_page::Place },
}

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
    fn push_nav(&mut self, key: egui::Key) {
        let events = if self.streaming.is_none() { self.launcher.controller_key(&self.gui.ctx, key) } else { gui::tap(key).to_vec() };
        for e in events {
            self.gui.push(e);
        }
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
            if let Ok(url) = std::ffi::CStr::from_ptr(event.drop.data).to_str() {
                if cli::is_link(url) {
                    self.links.put(url.to_owned());
                    return true;
                }
            }
        }
        false
    }

    fn handle_launcher_event(&mut self, event: &SDL_Event) {
        #[cfg(target_os = "macos")]
        if self.take_pairing_url(event) { return; }
        log_window_hdr(event);
        self.track_visibility(event);
        // SAFETY: reading the event type.
        self.window_changed |= window_needs_a_frame(SDL_EventType(unsafe { event.r#type }));
        if let Some(source) = input_source::of(event, |id| self.pads.real_pad_type(id)) {
            self.launcher.input_source = source;
        }
        // SAFETY: reading the event type.
        let kind = SDL_EventType(unsafe { event.r#type });
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

    pub(crate) fn launcher_frame(&mut self) {
        #[cfg(target_os = "linux")]
        self.apply_renderer_choice();
        #[cfg(windows)]
        self.apply_graphics_card();
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
                self.latency_input(&event);
                self.handle_launcher_event(&event);
                while SDL_PollEvent(&mut event) {
                    self.latency_input(&event);
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
        let calibrate = self.network_test.as_ref().is_some_and(|test| test.calibrate);
        for event in test_events {
            match event {
                network_test::Event::Progress(stage) => self.launcher.dialog = Dialog::NetworkTest { stage, calibrate },
                network_test::Event::Done(result) => {
                    let host = self.network_test.take().unwrap().host;
                    match result {
                        // Its result replaces the running test's dialog, as Android's does.
                        Ok(network_test::Outcome::Connection(result)) => self.launcher.ask(Dialog::ConnectionTest { host, result }),
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
        // Before a start's own read, which is the newer of the two when both are in.
        self.network_watch.pace(self.launcher.screen.network_pace());
        if let Some(network) = self.network_watch.latest() {
            self.launcher.network = network;
        }
        if let Some(receiver) = &self.network_query {
            if let Ok(network) = receiver.try_recv() {
                self.launcher.network = network;
                self.network_query = None;
                if let Some((host, app, resume, kind)) = self.network_launch.take() { self.launch_ready(host, app, resume, kind); }
            }
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
                            let join = self.hosts.offers_session_guests(id);
                            self.launcher.ask_after(Dialog::ContinueHere { host: id, app: asked, resume: asked_resume, name, device, join })
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
                    } else if cfg!(windows) && connecting.plan().hardware != self.hardware(&connecting.plan().settings) {
                        // SDL made its Direct3D 11 device again while connecting (a driver reset):
                        // the decoder's pictures are on the old device, which nothing draws.
                        eprintln!("nightjar: the renderer's Direct3D 11 device changed while connecting; the stream ends");
                        session.stop();
                        self.launcher.message(tr("The stream could not start"), tr("The graphics driver was reset, which ended the stream. Connect again."));
                    } else {
                        self.begin_stream(connecting, session, facts);
                        return;
                    }
                }
                Ok(None) => {}
                Err(failed) => {
                    let gone = self.connecting.take();
                    if !std::mem::take(&mut self.cancelled) {
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
        }

        let connecting = self.connecting.as_ref().map(|c| ConnectingView {
            app: c.app_name.clone(),
            host: c.host_name.clone(),
            stage: c.stage.clone(),
            host_id: c.host_id,
            app_id: c.plan().app.id,
        }).or_else(|| self.network_launch.map(|(host, app, ..)| {
            let pc = self.hosts.get(host);
            ConnectingView {
                app: pc.and_then(|h| app_name_of(h, app)).unwrap_or_else(|| tr("Starting stream").into()),
                host: pc.map_or(String::new(), |h| h.display_name().to_owned()),
                stage: tr("Checking network profile").into(),
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
            sdl::blend_title_bar(self.window, self.title_bar, *theme::ON_SURFACE);
        }
        let renderer = self.renderer;
        // On a PQ renderer the launcher draws its white at the compositor's SDR white.
        #[cfg(target_os = "linux")]
        let rescaled = sdl::launcher_white(renderer, self.streaming.is_none());
        #[cfg(not(target_os = "linux"))]
        let rescaled = false;
        // The latency benchmark's frames each reach the screen, paced by V-Sync, as Android's
        // Choreographer frames are.
        let must_draw = std::mem::take(&mut self.window_changed) || self.screenshot_requested || rescaled || self.launcher.latency_bench.timing();
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
            self.launcher.latency_bench.presented(session_now_ns());
            static FIRST_FRAME_SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if let Some(line) = first_frame_line(&FIRST_FRAME_SAID, LAUNCH.get().map(|at| at.elapsed())) {
                eprintln!("{line}");
            }
        }
        for command in commands {
            self.command(command);
        }
    }

    /// The latency benchmark's part of a launcher frame (`latency_bench`): leaving its page, or a
    /// start under way, stops it; on the page the device it reports is read once, the test
    /// advances, and while the timing test runs this frame is a sample.
    fn latency_bench_frame(&mut self) {
        if self.launcher.screen != Screen::LatencyBenchmark || self.connecting.is_some() || self.network_launch.is_some() {
            let bench = &mut self.launcher.latency_bench;
            if bench.running() || bench.device.is_some() {
                bench.leave();
            }
            return;
        }
        if self.launcher.latency_bench.device.is_none() {
            self.launcher.latency_bench.device = Some(self.latency_device());
        }
        self.launcher.latency_bench.step(Instant::now());
        if self.launcher.latency_bench.timing() {
            let vblank = stream::phase::timing(self.renderer, display_of(self.window).refresh_hz).map(|(latch_us, _)| latch_us.saturating_mul(1000));
            self.launcher.latency_bench.frame(vblank, session_now_ns());
        }
    }

    /// What the latency benchmark reports about this device: the display mode's refresh rate, the
    /// default output's audio format and the renderer's GPU.
    pub(crate) fn latency_device(&self) -> latency_bench::Device {
        let display = display_of(self.window);
        #[cfg(windows)]
        let gpu = gpu_adapters::renderer_gpu(sdl::renderer_pointer(self.renderer, SDL_PROP_RENDERER_D3D11_DEVICE_POINTER));
        #[cfg(not(windows))]
        let gpu = None;
        latency_bench::Device { panel_hz: display.refresh_reported.then_some(display.refresh_hz), audio: latency_bench::audio_format(), gpu }
    }

    /// An input event's age for the latency benchmark while its test runs: SDL stamps events in
    /// `SDL_GetTicksNS` time.
    fn latency_input(&mut self, event: &SDL_Event) {
        if !self.launcher.latency_bench.timing() {
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
            self.launcher.latency_bench.input(age.min(i64::MAX as u64) as i64);
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
