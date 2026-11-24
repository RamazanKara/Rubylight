//! Starting a stream: waking the PC, the command line's and autostream's steps, the questions before a
//! start (replace, PIN, moved PC), and `launch_ready`, which resolves the settings and connects.

mod types;

use super::*;
pub(crate) use types::*;

impl App {
    /// Resends the wake packets while the PC stays away, and ends the wait when it answers or the
    /// time is up.
    pub(crate) fn wake_step(&mut self) {
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
                self.launcher.notify(
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
    pub(crate) fn autostream_step(&mut self) {
        let Some(auto) = self.autostream.as_mut() else { return };
        if auto.failed || self.attempts.is_connecting() || self.network_launch.is_some() || self.probe.is_some() {
            return;
        }
        if let Some(why) = self.launcher.message_shown() {
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
    pub(crate) fn refresh_plan(&mut self, plan: &mut LaunchPlan) {
        plan.display = display_of(self.window);
        // The renderer's device now: SDL makes it again after a driver reset, and the old one is
        // only kept alive, not drawn on.
        plan.hardware = self.hardware(&plan.settings);
        plan.gpu_decoder = plan.gpu_decoder.and(self.gpu_device());
        plan.device = media::DeviceRef::of_device(self.renderer_device());
        plan.display_luminance = sdl::launch_luminance(self.renderer);
        let mask = self.pads.launch_mask(&plan.settings);
        if mask != plan.gamepad_mask {
            let pads: Vec<&str> = self.pads.pads.iter().map(|pad| pad.name.as_str()).collect();
            eprintln!("nightjar: {}", controllers_line(&plan.settings, &plan.controllers_from, &pads, mask));
        }
        plan.gamepad_mask = mask;
        plan.prepared_controllers = self.pads.launch_controllers(&plan.settings);
    }

    /// The PC refused `plan` until the user confirms closing the app it runs: ask.
    pub(crate) fn ask_replace(&mut self, plan: Box<LaunchPlan>) {
        eprintln!("nightjar: the PC asks to confirm closing its running app for app {}", plan.app.id);
        let running = self.hosts.get(plan.host_id).map_or_else(|| tr("the running app").into(), |h| ui::running_name(&h.ui_view()));
        self.launcher.ask(Dialog::ConfirmReplace { app: plan.app.name.clone(), running });
        self.pending_replace = Some(plan);
    }

    /// A replace prompt closed without the answer (Cancel, Escape, or another dialog over it)
    /// cancels the launch.
    pub(crate) fn replace_step(&mut self) {
        if self.pending_replace.is_some() && !matches!(self.launcher.dialog, Dialog::ConfirmReplace { .. }) {
            self.pending_replace = None;
            eprintln!("nightjar: closing the running app was not confirmed; the launch is cancelled");
            if let Some(cli) = self.cli.as_mut() { cli.cancel(); }
        }
    }

    /// Drives a `--host` start from the command line (see `cli`); does nothing otherwise.
    pub(crate) fn cli_step(&mut self) {
        let pending_reconnect = self.has_pending_device_reconnect();
        let Some(cli) = self.cli.as_mut() else { return };
        if cli.failed.is_some() { return; }
        let message = self.launcher.message_shown();
        // The stream and any reconnect are over: --quit-on-exit closes Nightjar.
        if self.streaming.is_none() && !self.attempts.is_connecting() && !pending_reconnect && cli.stream_ended(message.clone()) {
            self.running = false;
            return;
        }
        let seen = cli::Seen {
            identity: self.hosts.identity().is_some(),
            host: cli.host_seen(&self.hosts),
            message,
            busy: self.streaming.is_some() || self.attempts.is_connecting() || self.network_launch.is_some() || self.probe.is_some(),
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
    pub(crate) fn read_then_start(&mut self, host: u64, app: i32, resume: bool, too: bool) {
        if !self.hosts.offers_continue_here(host) {
            self.launch(host, app, resume, LaunchKind::Stream, LaunchFrom::Launcher);
        } else if !self.select_read {
            self.select_read = self.hosts.read_before_start(host, app, resume, too);
            if !self.select_read {
                self.launch(host, app, resume, LaunchKind::Stream, LaunchFrom::Launcher);
            }
        }
    }

    pub(crate) fn launch(&mut self, host_id: u64, app_id: i32, resume: bool, kind: LaunchKind, from: LaunchFrom) {
        let busy = self.attempts.is_connecting() || self.network_launch.is_some() || self.probe.is_some();
        // A new start asks for full chroma as its settings say, and waits for the HDR report anew.
        if !busy { self.full_chroma_off = None; self.hdr_wait = None; }
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

    /// A start that failed before its first frame: the launch scene turns into its card, which
    /// offers the same start again, the stream's settings (`mode`, else as this PC's settings make
    /// it) and a test of the network. `name`: the game's, when the PC's list may no longer have it.
    pub(crate) fn start_failed(&mut self, launch: (u64, i32, bool, LaunchKind), name: Option<String>, mode: Option<(u32, u32, u32, u32)>, body: String) {
        let (host_id, app_id, ..) = launch;
        let host = self.hosts.get(host_id);
        let mode = mode.or_else(|| {
            let host = host?;
            let mut settings = host.record.settings(&self.launcher.profiles.settings(&self.launcher.settings, self.launcher.network.kind(&self.launcher.profiles.home_wifi)));
            if let Apps::Loaded(apps) = &host.apps
                && let Some(overrides) = apps.iter().find(|a| a.id == app_id).and_then(|app| host.record.app_settings(&app.uuid, app.id)) {
                    settings = overrides.apply(&settings);
                }
            if let Some(cli) = self.cli.as_ref().filter(|cli| cli.host == host_id) { cli.launch.options.apply(&mut settings); }
            Some(stream::stream_mode(&settings, display_of(self.window)))
        });
        let view = ConnectingView {
            app: name.or_else(|| host.and_then(|h| app_name_of(h, app_id))).unwrap_or_default(),
            host: host.map_or(String::new(), |h| h.display_name().to_owned()),
            stage: String::new(),
            host_id,
            app_id,
        };
        self.failed_launch = Some(launch);
        self.launcher.start_failed(view, body, mode.map(ui::settings_line));
    }

    /// The PC refused full chroma after `plan`'s launch, so its app runs there: the PC is
    /// remembered as refusing it, and the same start is made once more without it, resuming that
    /// app. That start does not ask for it, so it cannot be refused again.
    pub(crate) fn restart_without_full_chroma(&mut self, plan: &LaunchPlan) {
        if let Some(host) = self.hosts.get_mut(plan.host_id)
            && full_chroma::refused(&mut host.record) { self.save_hosts(); }
        let kind = LaunchKind::of_plan(plan.join, plan.small_stream_input_only, plan.mic_only);
        eprintln!("nightjar: the PC refused full chroma after the launch; starting again without it{}", kind.log_suffix());
        self.full_chroma_off = Some(plan.host_id);
        // A microphone-only start launches nothing to resume.
        self.launch_ready(plan.host_id, plan.app.id, kind != LaunchKind::MicOnly, kind);
    }

    pub(crate) fn forget_pin_grant(&mut self) {
        if let Some(pin) = self.launcher.pin.as_mut() {
            pin.forget_grant();
        }
    }

    /// "Now playing on ...", watched while it is open: a move that fails gives the game back to
    /// this device, which has a minute to come back before close-on-disconnect applies.
    pub(crate) fn show_moved(&mut self, host: u64, app: i32, device: String) {
        self.launcher.ask_after(Dialog::NowPlaying { host, app, device });
        self.move_watch.open();
        self.watch_moved_prompt();
    }

    /// The next read for the open prompt. None once it closed, or it has asked enough.
    pub(crate) fn watch_moved_prompt(&mut self) {
        let Some((host, app)) = self.launcher.moved_prompt() else {
            return;
        };
        if let Some(token) = self.move_watch.next()
            && !self.hosts.watch_stream_move(host, app, token) {
                self.move_watch.not_started();
            }
    }

    /// Whether the PIN lock holds a stream start on `host` back; it then asks for the PIN and
    /// keeps `pending` until the PIN screen answers (Command::PinPassed or PinCancelled).
    pub(crate) fn pin_holds(&mut self, host: u64, pending: PinPending) -> bool {
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

    /// The renderer's device for a decoder check's plane shader: Direct3D 11 on Windows, the SDL
    /// GPU device on Unix (null for a renderer without one, which the check then refuses). Read on
    /// the main thread; the worker gets it with a `DeviceRef` that holds it while it runs.
    fn probe_device(&self) -> *mut std::ffi::c_void {
        #[cfg(windows)]
        {
            self.renderer_device()
        }
        #[cfg(unix)]
        {
            crate::sdl::renderer_pointer(self.renderer, SDL_PROP_RENDERER_GPU_DEVICE_POINTER)
        }
        #[cfg(not(any(windows, unix)))]
        {
            std::ptr::null_mut()
        }
    }

    /// Runs the start's decoder checks on a worker (`LaunchProbe`): the connecting screen shows
    /// them and Cancel ends them. The start goes on when `probe_step` reads their result.
    fn decoder_probe(&mut self, work: ProbeWork, hardware: Hardware, launch: (u64, i32, bool, LaunchKind), app_name: String) {
        eprintln!("nightjar: checking this device's decoders before the start, on a worker thread");
        self.probe = Some(LaunchProbe::start(work, media::DeviceRef::of_device(self.renderer_device()), launch, app_name, hardware));
    }

    /// Reads a decoder check's result and goes on with its start; reaps checks let go of after a
    /// Cancel, a timeout or a lost device. Runs once a frame, before the launcher's steps.
    pub(crate) fn probe_step(&mut self) {
        self.ending_probes.retain_mut(|probe| !probe.ended());
        let Some(mut probe) = self.probe.take() else { return };
        match probe.poll() {
            ProbeStep::Waiting => self.probe = Some(probe),
            ProbeStep::Overdue => {
                let (launch, app_name) = (probe.launch(), probe.app_name().to_owned());
                eprintln!("nightjar: the decoder checks did not answer within {} s; the start ends", PROBE_TIMEOUT.as_secs());
                self.ending_probes.push(probe);
                self.start_failed(launch, Some(app_name), None, stream::connect_timeout_text());
            }
            ProbeStep::Failed => {
                let (launch, app_name) = (probe.launch(), probe.app_name().to_owned());
                eprintln!("nightjar: a decoder check ended without an answer");
                self.start_failed(launch, Some(app_name), None, tr("The decoder checks could not finish. Connect again.").to_owned());
            }
            ProbeStep::Done(result) => {
                let (host, app, resume, kind) = probe.launch();
                match result {
                    ProbeResult::ColorAndBase { color, base } => {
                        if let Some((key, found)) = color { self.color_probed = Some((key, found)); }
                        if let Some((key, found)) = base { self.base_probed = Some((key, found)); }
                    }
                    ProbeResult::Packed(decoded) => {
                        // The draw and readback belong to the renderer's own thread: its targets do.
                        let hardware = probe.hardware();
                        let found = decoded
                            .and_then(|decoded| stream::packed444::draw_decoded(self.renderer, decoded))
                            .map_err(|reason| {
                                eprintln!("nightjar: full chroma: no decoder passed the check: {reason}");
                                reason
                            });
                        self.packed_probed = Some((hardware, found));
                    }
                }
                self.launch_ready(host, app, resume, kind);
            }
        }
    }

    pub(crate) fn launch_ready(&mut self, host_id: u64, app_id: i32, resume: bool, kind: LaunchKind) {
        let small = kind == LaunchKind::SmallInputOnly;
        let mic_only = kind == LaunchKind::MicOnly;
        if self.attempts.is_connecting() {
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
                    let why = unreadable_list(&self.hosts, host_id);
                    self.start_failed((host_id, app_id, resume, kind), None, None, why.to_owned());
                }
                return;
            }
            LaunchApp::Failed(why) => {
                self.start_failed((host_id, app_id, resume, kind), None, None, trf("The app list could not be loaded. {}", &[&why]));
                return;
            }
            LaunchApp::Missing => {
                self.start_failed((host_id, app_id, resume, kind), None, None, tr("The PC’s app list has no entry for this game.").to_owned());
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
        // moonlight-qt's stream options from the command line, for this run's streams from its PC.
        let cli_options = self.cli.as_ref().filter(|cli| cli.host == host_id).map(|cli| &cli.launch.options);
        if let Some(options) = cli_options { options.apply(&mut settings); }
        let mut controllers_from = ControllersFrom::of_layers(!settings.controllers,
            self.launcher.profiles.profile(self.launcher.network.kind(&self.launcher.profiles.home_wifi)),
            host.record.settings.as_ref(), host.record.app_settings(&app.uuid, app.id),
            cli_options.is_some_and(|options| options.multi_controller.is_some()));
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
        // Both kinds leave the controllers to another stream or to none: nothing the player chose.
        if mic_only || second_screen::this_process() { controllers_from = ControllersFrom::StreamKind; }
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
        // Sharper colored text without the GPU renderer starts without it (`full_chroma::decide`), as
        // on a decoder without it; anything else that needs the renderer refuses the stream.
        self.kms_notes.clear();
        self.switch_for_stream(&settings);
        #[cfg(unix)]
        let packed_without_gpu = {
            // Straight to the display the GPU renderer could not be made: what needs it plays
            // without it and the start says so, instead of a refused stream.
            if kms::active() && self.gpu_failure.is_some() && sdl::renderer_name(self.renderer) != "gpu" {
                let renderer = self.renderer;
                for need in kms::without_gpu(&mut settings, |s| kms_planned(s, renderer)) {
                    eprintln!("nightjar: kms: {need} is off for this stream: the GPU renderer could not be made");
                    self.kms_notes.push(trf(KMS_GPU_OFF, &[&tr(config::gpu_need_label(need))]));
                }
                self.gpu_failure = None;
            }
            // HDR and PyroWave say it in their own checks below, with what they found.
            // HDR and 10-bit pictures on video planes (KMS) need nothing of the renderer.
            let planes = kms_planned(&settings, self.renderer);
            let for_renderer = config::Settings { hdr: settings.hdr && !planes, sdr_10bit: settings.sdr_10bit && !planes, ..settings.clone() };
            match full_chroma::gpu_check(&for_renderer, |need| self.gpu_missing(need)) {
                Ok(left_out) => left_out,
                Err(advice) => {
                    let mode = stream::stream_mode(&settings, display_of(self.window));
                    self.start_failed((host_id, app_id, resume, kind), Some(app.name.clone()), Some(mode), advice);
                    return;
                }
            }
        };
        #[cfg(not(unix))]
        let packed_without_gpu = false;
        self.hdr_note = false;
        let hdr_output = sdl::hdr_available(self.renderer)
            || (settings.hdr && kms_planned(&settings, self.renderer) && kms_display_hdr(self.window));
        // The window's HDR report can come a moment after it is shown. The start waits for it
        // over the next frames and comes back here (`network_launch`), so the launcher keeps
        // drawing and Cancel works; at the wait's end it goes on with what the window reports.
        if !hdr_output && settings.hdr && !handheld::gamescope() && hdr_wait_goes_on(&mut self.hdr_wait, Instant::now(), sdl::window_hdr_may_come(self.renderer)) {
            self.network_launch = Some((host_id, app_id, resume, kind));
            return;
        }
        self.hdr_wait = None;
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
        self.pyrowave_note = None;
        // The renderer's device, when a PyroWave decoder needs it: the Mac check below probes the
        // plane shader on it too, so a shader that does not build falls back before the stream.
        let gpu_device = if settings.codec.is_pyrowave() { self.gpu_device() } else { None };
        // A Mac decodes on the processor where the GPU decoder cannot run, and says why.
        #[cfg(target_os = "macos")]
        if settings.codec.is_pyrowave() && settings.pyrowave_decoder == config::PyroWaveDecoder::Gpu {
            let device = gpu_device.map_or(std::ptr::null_mut(), |device| device.0);
            if let stream::pyrowave_mode::MacChoice::Cpu(reason) = stream::pyrowave_mode::this_mac(settings.pyrowave_decoder, settings.hdr, device) {
                let reason = reason.unwrap_or_default();
                eprintln!("nightjar: PyroWave decodes on the processor: {reason}");
                self.pyrowave_note = Some(trf("PyroWave decodes on the processor: {}", &[&reason]));
                settings.pyrowave_decoder = config::PyroWaveDecoder::Cpu;
            }
        }
        if settings.codec.is_pyrowave() {
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
                    let advice = self.gpu_missing(strings::en("PyroWave")).map_or_else(|| format!("{reason}."), |why| format!("{reason}. {why}"));
                    self.launcher.message(tr("PyroWave output unavailable"), advice);
                    return;
                }
                (Err(reason), config::PyroWaveDecoder::Gpu) => {
                    // The CPU decoder draws with the GPU renderer too: without it, switching would not help.
                    let advice = match self.gpu_missing(strings::en("PyroWave")) {
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
        media::use_videotoolbox_rfi_setting(settings.videotoolbox_rfi);
        // Before the launch: a stream that cannot have sharper colored text starts without it,
        // instead of launching the game and being refused in the handshake.
        let refused_now = self.full_chroma_off == Some(host_id);
        let (hardware, size) = (self.hardware(&settings), (settings.width, settings.height));
        // It tries the codec's own 4:4:4 first: what this device decodes of it, and of the plain
        // formats, as a stream without packing has them. Offers follow what the decoder decodes
        // here, not what it is said to. The checks decode on a worker (`LaunchProbe`): a driver
        // reset inside a decoder call must not hold this frame, which draws the window and carries
        // Cancel and the deadline. Their results are kept per decoder and renderer device.
        let native = full_chroma::native_probe_settings(&settings);
        let color_wanted = stream::color_probe_formats(&native);
        let color_key = probe_key(hardware, color_wanted, &native);
        let color_asked = !packed_without_gpu && stream::probes_color(&native);
        let base_wanted = stream::base_probe_formats(&native);
        let base_key = probe_key(hardware, base_wanted, &native);
        let cached_color = self.color_probed.as_ref().filter(|(key, _)| *key == color_key).map(|(_, found)| found.clone());
        let cached_base = self.base_probed.as_ref().filter(|(key, _)| *key == base_key).map(|(_, found)| found.clone());
        let color_missing = color_asked && cached_color.is_none();
        let base_missing = hardware != Hardware::None && cached_base.is_none();
        if color_missing || base_missing {
            self.decoder_probe(
                ProbeWork::ColorAndBase {
                    color: color_missing.then_some((color_key, color_wanted)),
                    base: base_missing.then_some((base_key, base_wanted)),
                    device: self.probe_device(),
                },
                hardware,
                (host_id, app_id, resume, kind),
                app.name.clone(),
            );
            return;
        }
        let color_decodable = if color_asked { cached_color.unwrap_or_default() } else { (0, String::new()) };
        let plain_decodable = match hardware {
            Hardware::None => stream::color_probe::Base::software(),
            _ => cached_base.unwrap_or_else(stream::color_probe::Base::software),
        };
        // An update on either side since the PC refused full chroma: it is asked again. Written
        // before the packed check, whose answer the renewed record changes.
        let renewed = {
            let Some(host) = self.hosts.get_mut(host_id) else { return };
            full_chroma::automatic(&settings) && full_chroma::renew(&mut host.record, VERSION, info.capabilities.host_version())
        };
        if renewed { self.save_hosts(); }
        let known = full_chroma::Known {
            serverinfo_modes: info.packed444_modes,
            host_scm: info.trusted_codec_mode_support(),
            color_decodable: &color_decodable,
            hardware_decodable: plain_decodable.decodable,
            renderer_missing: packed_without_gpu,
        };
        // The packed check decodes too, and only some launches need it: the same decision says so
        // before the decode is started (`full_chroma::needs_device_modes`).
        let packed_cached = self.packed_probed.as_ref().filter(|(device, _)| *device == hardware).map(|(_, found)| found.clone());
        let packed_missing = packed_cached.is_none() && {
            let Some(host) = self.hosts.get(host_id) else { return };
            full_chroma::needs_device_modes(&settings, &host.record, &known, refused_now)
        };
        if packed_missing {
            self.decoder_probe(ProbeWork::Packed { hardware }, hardware, (host_id, app_id, resume, kind), app.name.clone());
            return;
        }
        let Some(host) = self.hosts.get_mut(host_id) else { return };
        let chroma = full_chroma::decide(&mut settings, &mut host.record, &known, refused_now, || {
            if packed_without_gpu {
                eprintln!("nightjar: full chroma: no GPU renderer for its shader pass");
                return (0, None);
            }
            let modes = match packed_cached {
                Some(Ok(modes)) => modes,
                Some(Err(reason)) => {
                    eprintln!("nightjar: full chroma: no decoder passed the check: {reason}");
                    0
                }
                None => 0,
            };
            packed_modes_that_fit(hardware, modes, size)
        });
        if chroma.record_changed || renewed {
            self.save_hosts();
        }
        // The restart after a refusal decided without full chroma; the next start is a new one.
        if refused_now { self.full_chroma_off = None; }
        self.packed_note = chroma.note;
        let packed_modes = chroma.modes;
        // Packed full chroma checks its own formats (`stream::packed444::decode_all`, drawn and
        // read back on this thread) and offers the plain ones whatever the decoder check found.
        let base_decodable = if settings.experimental_packed_444 && hardware != Hardware::None {
            stream::color_probe::Base { decodable: 0, reason: String::new(), backends: Vec::new() }
        } else {
            plain_decodable
        };
        let (Some(identity), Some(host)) = (identity, self.hosts.get(host_id)) else {
            return;
        };
        let display = display_of(self.window);
        log_display(self.window, display);
        self.launcher.display = display;
        // SAFETY: a live window.
        self.launcher.fullscreen_modes = fullscreen_modes(unsafe { SDL_GetDisplayForWindow(self.window) });
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
            negotiated_packet_size: 0,
            join: kind == LaunchKind::JoinRunning,
            small_stream_input_only: small,
            mic_only,
            settings: settings.clone(),
            identity,
            display,
            hardware,
            // Both decoders' device: D3D11VA's (`hardware`) and PyroWave's (`gpu_decoder`).
            device: media::DeviceRef::of_device(self.renderer_device()),
            dmabuf: self.dmabuf_import(),
            kms_planes: kms_planned(&settings, self.renderer),
            gpu_decoder,
            packed_modes,
            color_decodable,
            base_decodable,
            pacing,
            vrr,
            gamepad_mask: self.pads.launch_mask(&settings),
            prepared_controllers: self.pads.launch_controllers(&settings),
            controllers_from,
            network: self.launcher.network.clone(),
            pace: host.record.pace.clone(),
            host_link_mbps: host.info.as_ref().map_or(0, |info| info.pyrowave_host_link_mbps),
            bitrate_limit_kbps: self.launcher.profiles.bitrate_limit(self.launcher.network.kind(&self.launcher.profiles.home_wifi)),
            microphone_muted: false,
            recovery: None,
            video_shown: false,
            confirm_replace: false,
            display_luminance: sdl::launch_luminance(self.renderer),
            bitrate_before_manual: None,
            resume_bitrate_kbps: None,
            resume_quality: None,
        };
        let pads: Vec<&str> = self.pads.pads.iter().map(|pad| pad.name.as_str()).collect();
        eprintln!("nightjar: {}", controllers_line(&plan.settings, &plan.controllers_from, &pads, plan.gamepad_mask));
        // A stream that just ended may still be stopping: this launch's first request waits for it.
        let stopping = self.take_stopping();
        self.attempts.begin(Connecting::start(plan, self.waker.clone(), stopping));
    }
}

/// The microphone-only screen's title. An unavailable microphone says so rather than "muted";
/// `busy`: the PC plays another device's microphone for now.
pub(crate) fn mic_only_title(muted: bool, unavailable: bool, busy: bool) -> &'static str {
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

/// The app a microphone-only session stands for in the stream's title and logs.
pub(crate) fn mic_only_app() -> nightjar_core::http::App {
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

pub(crate) use crate::app_types::{controllers_line, ControllersFrom};
#[cfg(test)]
pub(crate) use crate::app_types::{controllers_notice, ControllersOffNotice};

/// Why no read of the list a launch waits for could start: the PC was removed, or this device's
/// key is still being made.
pub(crate) fn unreadable_list(hosts: &Hosts, host: u64) -> &'static str {
    if hosts.get(host).is_none() { tr("This PC is no longer in your list.") } else { tr("This device’s pairing key is not ready yet. Try again in a moment.") }
}
