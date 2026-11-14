//! Starting a stream: waking the PC, the command line's and autostream's steps, the questions before a
//! start (replace, PIN, moved PC), and `launch_ready`, which resolves the settings and connects.

use super::*;

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
    pub(crate) fn autostream_step(&mut self) {
        let Some(auto) = self.autostream.as_mut() else { return };
        if auto.failed || self.connecting.is_some() || self.network_launch.is_some() {
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
        plan.gamepad_mask = self.pads.launch_mask(&plan.settings);
        plan.prepared_controllers = self.pads.launch_controllers(&plan.settings);
    }

    /// The PC refused `plan` until the user confirms closing the app it runs: ask.
    pub(crate) fn ask_replace(&mut self, plan: Box<LaunchPlan>) {
        eprintln!("nightjar: the PC asks to confirm closing its running app for app {}", plan.app.id);
        let running = self.hosts.get(plan.host_id).map_or_else(|| tr("the running app").into(), ui::running_name);
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
        let Some(cli) = self.cli.as_mut() else { return };
        if cli.failed.is_some() {
            return;
        }
        let message = self.launcher.message_shown();
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
        let busy = self.connecting.is_some() || self.network_launch.is_some();
        // A new start asks for full chroma as its settings say.
        if !busy { self.full_chroma_off = None; }
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
            if let Apps::Loaded(apps) = &host.apps {
                if let Some(overrides) = apps.iter().find(|a| a.id == app_id).and_then(|app| host.record.app_settings(&app.uuid, app.id)) {
                    settings = overrides.apply(&settings);
                }
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
        if let Some(host) = self.hosts.get_mut(plan.host_id) {
            if full_chroma::refused(&mut host.record) { self.save_hosts(); }
        }
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
        if let Some(token) = self.move_watch.next() {
            if !self.hosts.watch_stream_move(host, app, token) {
                self.move_watch.not_started();
            }
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

    pub(crate) fn launch_ready(&mut self, host_id: u64, app_id: i32, resume: bool, kind: LaunchKind) {
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
                    self.start_failed((host_id, app_id, resume, kind), None, None, trf("The app list could not be loaded. {}", &[&tr("Creating this device’s pairing key…")]));
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
        if let Some(cli) = self.cli.as_ref().filter(|cli| cli.host == host_id) { cli.launch.options.apply(&mut settings); }
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
        // Sharper colored text without the GPU renderer starts without it (`full_chroma::decide`), as
        // on a decoder without it; anything else that needs the renderer refuses the stream.
        self.kms_notes.clear();
        #[cfg(unix)]
        let packed_without_gpu = {
            self.switch_for_stream(&settings);
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
            || (settings.hdr && kms_planned(&settings, self.renderer) && kms_display_hdr(self.window))
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
        // Before the launch: a stream that cannot have sharper colored text starts without it,
        // instead of launching the game and being refused in the handshake.
        let refused_now = self.full_chroma_off.take() == Some(host_id);
        let (hardware, renderer, size) = (self.hardware(&settings), self.renderer, (settings.width, settings.height));
        // It tries the codec's own 4:4:4 first: what this device decodes of it, and of the plain
        // formats, as a stream without packing has them. Offers follow what the decoder decodes
        // here, not what it is said to.
        let native = full_chroma::native_probe_settings(&settings);
        let color_decodable = if !packed_without_gpu && stream::probes_color(&native) {
            let wanted = stream::color_probe_formats(&native);
            probed_once(&mut self.color_probed, probe_key(hardware, wanted, &native), || {
                stream::color_probe::probe(hardware, wanted, |ten_bit| stream::color_probe::planes_drawable(renderer, ten_bit))
            })
        } else {
            (0, String::new())
        };
        let plain_decodable = match (hardware, stream::base_probe_formats(&native)) {
            (Hardware::None, _) => stream::color_probe::Base::software(),
            (hardware, wanted) => {
                let key = probe_key(hardware, wanted, &native);
                probed_once(&mut self.base_probed, key, || stream::color_probe::probe_base(hardware, wanted))
            }
        };
        let Some(host) = self.hosts.get_mut(host_id) else { return };
        // An update on either side since the PC refused full chroma: it is asked again.
        let renewed = full_chroma::automatic(&settings)
            && full_chroma::renew(&mut host.record, VERSION, info.capabilities.host_version());
        let known = full_chroma::Known {
            serverinfo_modes: info.packed444_modes,
            host_scm: info.trusted_codec_mode_support(),
            color_decodable: &color_decodable,
            hardware_decodable: plain_decodable.decodable,
            renderer_missing: packed_without_gpu,
        };
        let chroma = full_chroma::decide(&mut settings, &mut host.record, &known, refused_now, || {
            if packed_without_gpu {
                eprintln!("nightjar: full chroma: no GPU renderer for its shader pass");
                return (0, None);
            }
            let modes = stream::packed444::probe(renderer, hardware).unwrap_or_else(|reason| {
                eprintln!("nightjar: full chroma: no decoder passed the check: {reason}");
                0
            });
            packed_modes_that_fit(hardware, modes, size)
        });
        if chroma.record_changed || renewed {
            self.save_hosts();
        }
        self.packed_note = chroma.note;
        let packed_modes = chroma.modes;
        // Packed full chroma checks its own formats (`stream::packed444::probe`) and offers the
        // plain ones whatever the decoder check found.
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
}

/// Input-only on a PC without its own mode: the smallest stream every host takes, as on Android.
/// It leaves the PC's display mode and its speakers alone: no virtual display or VRR, and the
/// PC keeps playing its sound.
/// A session whose picture nobody sees (input only, microphone only): the plainest stream, and
/// nothing that needs the GPU renderer (Linux would switch to it, or refuse with OpenGL set).
pub(crate) fn input_only_settings(settings: &mut config::Settings) {
    settings.hdr = false;
    settings.experimental_packed_444 = false;
    (settings.prefer_yuv444, settings.sdr_10bit) = (false, false);
    (settings.upscaling, settings.frame_warp) = (config::Upscaling::Bilinear, false);
    settings.codec = config::CodecPreference::H264;
    settings.width = 1280;
    settings.height = 720;
    settings.fps = 60;
}

pub(crate) fn small_stream(settings: &mut config::Settings) {
    use nightjar_policy::launch::{SMALL_BITRATE_KBPS, SMALL_FPS, SMALL_HEIGHT, SMALL_WIDTH};
    (settings.width, settings.height, settings.fps, settings.bitrate_kbps) = (SMALL_WIDTH, SMALL_HEIGHT, SMALL_FPS, SMALL_BITRATE_KBPS);
    settings.audio = config::AudioChannels::Stereo;
    (settings.virtual_display, settings.vrr, settings.play_audio_on_host) = (false, false, true);
    // Nothing is shown, so nothing is offered beyond plain 4:2:0 or said about it, or scaled.
    (settings.prefer_yuv444, settings.sdr_10bit) = (false, false);
    (settings.upscaling, settings.frame_warp) = (config::Upscaling::Bilinear, false);
}

/// What a decoder check's result holds for: the decoder, the formats it checked, and "Vulkan video
/// first", which changes the backend order both checks decode through (`Hardware::order`).
pub(crate) type ProbeKey = (Hardware, u32, bool);

pub(crate) fn probe_key(hardware: Hardware, wanted: u32, settings: &config::Settings) -> ProbeKey {
    (hardware, wanted, settings.vulkan_video_first)
}

/// `probe`'s answer for `hardware`, from `cache` when it was the last decoder checked.
pub(crate) fn probed_once<K: PartialEq, T: Clone>(cache: &mut Option<(K, T)>, hardware: K, probe: impl FnOnce() -> T) -> T {
    match cache {
        Some((probed, result)) if *probed == hardware => result.clone(),
        _ => cache.insert((hardware, probe())).1.clone(),
    }
}

/// A stream start the PIN lock holds until the PIN is entered.
pub(crate) enum PinPending {
    Launch { host: u64, app: i32, resume: bool, kind: LaunchKind, from: LaunchFrom },
    QuitAndLaunch { host: u64, app: i32, named: Option<i32> },
    /// Continue here's press, and with it the stream start it leads to.
    ContinueHere { host: u64, app: i32 },
}

/// What asked for a launch. The log names it, so it shows what had the PC start or replace a game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LaunchFrom {
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
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            LaunchFrom::Launcher => "launcher",
            LaunchFrom::Replace => "launcher, quit and start",
            LaunchFrom::Autostream => "autostream",
            LaunchFrom::CommandLine => "command line",
        }
    }
}

/// The packed picture is twice the stream's height: past the decoder's largest, that mode goes,
/// and the start says why when none is left (`linux_diagnosis::packed_offer`).
#[cfg(target_os = "linux")]
pub(crate) fn packed_modes_that_fit(hardware: Hardware, modes: u8, (width, height): (u32, u32)) -> (u8, Option<String>) {
    linux_diagnosis::packed_offer(hardware, modes, width, height)
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn packed_modes_that_fit(_hardware: Hardware, modes: u8, _size: (u32, u32)) -> (u8, Option<String>) {
    (modes, None)
}

/// What a launch starts besides an ordinary stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LaunchKind {
    Stream,
    /// Input-only through a small ordinary stream, on a PC without its own mode.
    SmallInputOnly,
    /// Only this device's microphone, on a PC that lists `Feature::MicOnly`.
    MicOnly,
    /// A resume of the game the PC runs, also another device's (`LaunchPlan::join`).
    JoinRunning,
}

impl LaunchKind {
    /// The kind of a start a plan made: a join, input only through a small stream, microphone
    /// only, else an ordinary stream.
    pub(crate) fn of_plan(join: bool, small_stream_input_only: bool, mic_only: bool) -> Self {
        if mic_only { LaunchKind::MicOnly } else if small_stream_input_only { LaunchKind::SmallInputOnly } else if join { LaunchKind::JoinRunning } else { LaunchKind::Stream }
    }

    pub(crate) fn log_suffix(self) -> &'static str {
        match self {
            LaunchKind::Stream => "",
            LaunchKind::SmallInputOnly => ", input only through a small stream",
            LaunchKind::MicOnly => ", microphone only",
            LaunchKind::JoinRunning => ", joining the running game",
        }
    }

    /// Stream here too: a game selected by its tile launches beside the running one as it always
    /// did; a resume would stop here, since the PC reports the game to its owner only.
    pub(crate) fn stream_here_too(resume: bool) -> Self {
        if resume { LaunchKind::JoinRunning } else { LaunchKind::Stream }
    }
}

/// What a start does once the PC answered who plays.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SelectStep {
    /// Another device plays this game: "X is playing on Y", with this title and device.
    Ask(String, String),
    Join,
    Start,
}

pub(crate) fn select_step(resume: bool, too: bool, playing: Option<(String, String)>, press_failed: bool) -> SelectStep {
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
pub(crate) fn mic_only_settings(settings: &mut config::Settings) {
    settings.microphone = true;
    settings.controllers = false;
    settings.play_audio_on_host = true;
    settings.clipboard_sync = false;
    settings.usb_forwarding = false;
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

/// The app id a microphone-only launch sends: it names no app, and the PC starts none.
pub(crate) const MIC_ONLY_APP_ID: i32 = 0;

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

pub(crate) fn launch_line(from: LaunchFrom, app_id: i32, resume: bool, busy: bool) -> String {
    format!(
        "launch from {}: app {app_id}{}{}",
        from.as_str(),
        if resume { ", resume" } else { "" },
        if busy { ", ignored: another stream is starting" } else { "" }
    )
}

/// What a launch can do with a PC's app list as it is now.
pub(crate) enum LaunchApp {
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

pub(crate) fn launch_app(apps: &Apps, app_id: i32) -> LaunchApp {
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
pub(crate) fn launch_target(host: &hosts::Host, app_id: i32, resume: bool) -> LaunchApp {
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
pub(crate) fn read_list_for_launch(hosts: &mut Hosts, host: u64) -> bool {
    hosts.load_apps(host);
    matches!(hosts.get(host).map(|h| &h.apps), Some(Apps::Loading))
}
