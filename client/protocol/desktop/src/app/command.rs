//! What a launcher page asks the app to do: `App::command`, one arm per `ui::Command`.

use super::*;

impl App {
    pub(crate) fn command(&mut self, command: Command) {
        match command {
            Command::PinPassed => match self.pin_pending.take() {
                Some(PinPending::Launch { host, app, resume, kind, from }) => self.launch(host, app, resume, kind, from),
                Some(PinPending::QuitAndLaunch { host, app, named }) => self.command(Command::QuitAndLaunch { host, app, named }),
                Some(PinPending::ContinueHere { host, app }) => self.hosts.continue_here(host, app),
                None => {}
            },
            Command::PinCancelled => {
                eprintln!("nightjar: stream start cancelled at the PIN");
                if let Some(PinPending::Launch { from: LaunchFrom::CommandLine, .. }) = self.pin_pending.take() {
                    if let Some(cli) = self.cli.as_mut() { cli.cancel(); }
                }
            }
            Command::Quit => self.running = false,
            Command::OpenOnStart(game) => self.open_play_choose(game),
            Command::SetSignIn(on) => self.sign_in_set(on),
            Command::Power(offer) => {
                // One at a time: a second confirm while logind waits on its password is dropped.
                if self.power_request.is_none() {
                    eprintln!("nightjar: {:?} this PC", offer.action);
                    self.power_request = Some((offer, power::request(offer, self.waker.clone())));
                }
            }
            Command::Launch { host, app, resume } => self.read_then_start(host, app, resume, false),
            Command::LaunchUnasked { host, app, resume } => self.launch(host, app, resume, LaunchKind::Stream, LaunchFrom::Launcher),
            Command::StreamTooAfterFailedMove { host, app } => self.read_then_start(host, app, true, true),
            Command::StreamHereToo { host, app, resume } => self.launch(host, app, resume, LaunchKind::stream_here_too(resume), LaunchFrom::Launcher),
            // The press ends another device's stream: the PIN comes before it, not at the start after it.
            Command::ContinueHere { host, app } => {
                if !self.pin_holds(host, PinPending::ContinueHere { host, app }) {
                    self.hosts.continue_here(host, app);
                }
            }
            Command::LaunchInputOnly { host, app, resume } => self.launch(host, app, resume, LaunchKind::SmallInputOnly, LaunchFrom::Launcher),
            Command::MicOnly { host } => self.launch(host, MIC_ONLY_APP_ID, false, LaunchKind::MicOnly, LaunchFrom::Launcher),
            Command::ToggleFavourite(entry) => {
                config::toggle_favourite(&mut self.launcher.favourites, entry);
                if let Err(e) = self.store.save_favourites(&self.launcher.favourites) {
                    eprintln!("nightjar: could not save favourites: {e}");
                }
            }
            Command::SetHidden { games, hidden } => {
                if config::set_hidden(&mut self.launcher.hidden_games, games, hidden) {
                    if let Err(e) = self.store.save_hidden_games(&self.launcher.hidden_games) {
                        eprintln!("nightjar: could not save hidden games: {e}");
                    }
                }
            }
            Command::QuitApp(host) => self.hosts.quit_app(host),
            Command::SetQuitOnLeave { host, uuid, id, enabled } => {
                if let Some(h) = self.hosts.get_mut(host) {
                    h.record.set_quit_on_leave(&uuid, id, enabled);
                    self.save_hosts();
                }
            }
            Command::ControlAction { host, app } => self.hosts.control_action(host, app),
            Command::QuitAndLaunch { host, app, named } => {
                // Quitting the running game is part of this start: the PIN comes first.
                if self.pin_holds(host, PinPending::QuitAndLaunch { host, app, named }) {
                    return;
                }
                // The PC may run another app than the prompt named: look again before closing anything.
                let now = self.hosts.get(host).and_then(|h| h.running_app());
                match console::recheck_running(named, now) {
                    console::Running::Changed => self.launcher.message(
                        tr("The running app changed"),
                        tr("The PC runs another app now. Check the screen and try again."),
                    ),
                    console::Running::Gone => self.launch(host, app, false, LaunchKind::Stream, LaunchFrom::Replace),
                    console::Running::Same => {
                        self.pending_launch = Some((host, app));
                        self.hosts.quit_app(host);
                    }
                }
            }
            Command::ConfirmReplace => {
                if let Some(mut plan) = self.pending_replace.take() {
                    if self.connecting.is_some() || self.network_launch.is_some() {
                        eprintln!("nightjar: another launch is under way; not launching app {} again", plan.app.id);
                        return;
                    }
                    eprintln!("nightjar: launching app {} again, closing the running app as confirmed", plan.app.id);
                    plan.confirm_replace = true;
                    self.refresh_plan(&mut plan);
                    self.cancelled = false;
                    self.connecting = Some(Connecting::start(*plan, self.waker.clone()));
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
            Command::ApprovePairing { host, request, name, pin } => self.hosts.approve_pairing(host, request, name, pin),
            Command::DenyPairing { host, request, name } => self.hosts.deny_pairing(host, request, name),
            Command::Wake(host) => {
                if self.waking.start(host, Instant::now()) {
                    self.wake_step();
                }
            }
            Command::CancelWake => self.waking.end(),
            Command::SleepOffer { host, ask } => self.hosts.sleep_offer(host, ask),
            Command::Sleep { host, ask, token, at } => self.hosts.sleep(host, ask, token, at),
            Command::PcPowerOffer { host, ask, action } => self.hosts.pc_power_offer(host, ask, action),
            Command::PcPower { host, ask, action, token, at } => self.hosts.pc_power(host, ask, action, token, at),
            Command::InstallUpdate { host } => self.hosts.install_update(host),
            Command::Rename { host, alias } => rename_host(&mut self.hosts, &self.store, host, &alias),
            Command::WakeAddresses { host, macs } => {
                if self.hosts.set_own_wake_macs(host, macs) {
                    save_hosts(&self.store, &self.hosts);
                }
            }
            Command::PinAddress { host, address } => {
                if self.hosts.pin_address(host, address.as_deref()) {
                    save_hosts(&self.store, &self.hosts);
                    self.hosts.poll_one(host);
                }
            }
            Command::HostSettings(id) => {
                if let Some(host) = self.hosts.get(id) {
                    self.launcher.open_host_settings(id, host);
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
                    self.launcher.open_app_settings(id, host, app);
                }
            }
            Command::SaveAppSettings(id) => {
                if let (Some(host), Some(app)) = (self.hosts.get_mut(id), self.launcher.editing_app.as_ref()) {
                    host.record.save_app_settings(&app.uuid, app.id, self.launcher.edit_overrides.clone());
                    self.save_hosts();
                }
            }
            Command::ResetKeptSettings => {
                self.hosts.list.iter_mut().for_each(|host| host.record.clear_own_settings());
                self.save_hosts();
            }
            Command::ProfileSettings(id) => {
                if let Some(profile) = self.launcher.profiles.entries.iter().find(|p| p.id == id) {
                    self.launcher.inherited_settings = self.launcher.settings.clone();
                    self.launcher.edit_overrides = profile.settings.clone();
                    self.launcher.edit_settings = profile.settings.apply(&self.launcher.inherited_settings);
                    self.launcher.inherited_from = Vec::new();
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
                let client = match hosts::client_for(identity, &host.record, host.connect_address()) {
                    Ok(client) => client,
                    Err(error) => { self.launcher.message(tr("Network test unavailable"), error); return; }
                };
                let settings = host.record.settings(&self.launcher.profiles.settings(&self.launcher.settings, self.launcher.network.kind(&self.launcher.profiles.home_wifi)));
                let (_, _, fps, bitrate) = stream::stream_mode(&settings, display_of(self.window));
                self.network_test = Some(network_test::Test::start(id, client, self.launcher.network.clone(), host.connect_address().to_owned(), (bitrate, fps), calibrate, host.bandwidth_test(), self.waker.clone()));
                let stage = if calibrate { tr("Preparing frame-shaped UDP probes") } else { strings::en("Measuring ping, jitter and loss") };
                self.launcher.dialog = Dialog::NetworkTest { stage: stage.into(), calibrate };
            }
            Command::CancelNetworkTest => { self.network_test = None; self.launcher.dialog = Dialog::None; }
            Command::ClearPace(id) => {
                if let Some(host) = self.hosts.get_mut(id) { host.record.pace = None; self.save_hosts(); }
            }
            Command::AskFullChromaAgain => {
                let mut changed = false;
                for host in &mut self.hosts.list {
                    changed |= full_chroma::ask_again(&mut host.record);
                }
                if changed { self.save_hosts(); }
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
            Command::StreamHotkeys => {
                self.command(Command::SaveSettings);
                self.launcher.open(Screen::StreamHotkeys);
            }
            Command::SaveHotkeys => {
                if let Err(e) = self.store.save_hotkeys(&shortcuts::hotkey_bindings()) {
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
            // Android's ProblemReport: the redacted logs as one file, shown in the logs folder.
            Command::ReportProblem => {
                let header = format!("Nightjar {VERSION} / {} {}
Redacted logs, oldest line first; addresses, PC names, PINs and credentials are removed.",
                    std::env::consts::OS, std::env::consts::ARCH);
                match logfile::logs_dir().and_then(|dir| logfile::write_report(&dir, &header)) {
                    Ok(_) => {
                        self.launcher.toast(tr("Problem report saved."));
                        if let Err(e) = logfile::open_folder() {
                            self.launcher.message(tr("Could not open the logs folder"), e);
                        }
                    }
                    Err(e) => self.launcher.message(tr("Could not save the report"), e.to_string()),
                }
            }
            // The latency benchmark page (`latency_bench`).
            Command::StartLatencyBenchmark => {
                let hardware = self.hardware(&self.launcher.settings);
                self.launcher.latency_bench.device = Some(self.latency_device());
                self.launcher.latency_bench.start(hardware, self.waker.clone());
            }
            Command::StopLatencyBenchmark => self.launcher.latency_bench.stop(),
            // Android's Export latency CSV: the last stream's file, saved where the dialog says.
            Command::ExportLatencyCsv => {
                if !self.store.dir().join("statistics").join(stream::LATENCY_CSV).is_file() {
                    self.launcher.toast(tr("Finish a stream before exporting its timings."));
                } else if self.latency_csv_file.is_none() {
                    self.latency_csv_file = Some(sdl::choose_csv_file(self.window));
                }
            }
            Command::OpenSource => {
                if let Err(e) = sdl::open_url(ui::ABOUT_SOURCE) {
                    self.launcher.message(tr("No browser could open the page. Its address is below, to open on another device."), format!("{}\n\n{e}", ui::ABOUT_SOURCE));
                }
            }
            // Android's UpdateFlow.checkNow: a toast says it asks; the answer is a toast or the notice.
            Command::CheckUpdatesNow => {
                if self.updates.check_now() {
                    self.launcher.toast(tr("Checking for updates…"));
                }
            }
            Command::RemoteCheck(id) => {
                if let Some(host) = self.hosts.get(id) {
                    host_page::start_check(&mut self.launcher.remote, self.hosts.identity(), host, self.waker.clone());
                }
            }
            Command::SharedFolders(id) => {
                if let Some(host) = self.hosts.get(id) {
                    folders_page::start(&mut self.launcher.folders, self.hosts.identity(), host, self.waker.clone());
                }
            }
            Command::FolderDownload { host, name, size } => {
                let window = self.window;
                start_folder_pick(&mut self.folder_pick, &mut self.launcher.folders, host, |at| FolderPick::Download { host, at, name, size }, || sdl::choose_path(window, true));
            }
            Command::FolderUpload(host) => {
                let window = self.window;
                start_folder_pick(&mut self.folder_pick, &mut self.launcher.folders, host, |at| FolderPick::Upload { host, at }, || sdl::choose_path(window, false));
            }
            Command::Coop(id) => {
                if let Some(host) = self.hosts.get(id) {
                    coop_page::start(&mut self.launcher.coop, self.hosts.identity(), host, self.waker.clone());
                }
            }
            Command::OpenFiles(id) => {
                if let Some(host) = self.hosts.get(id) {
                    ui::files::open(&mut self.launcher, self.hosts.identity(), host, self.waker.clone());
                }
            }
            Command::ChooseFiles => {
                let window = self.window;
                self.launcher.files.choose(|done| sdl::choose_files(window, done));
            }
            Command::OpenWebUi(url) => {
                if let Some((title, body)) = host_page::open_web_ui(&url, sdl::open_url) {
                    self.launcher.message(title, body);
                }
            }
            Command::OpenGuide(page) => {
                if let Some((title, body)) = host_page::open_guide(page, sdl::open_url) {
                    self.launcher.message(title, body);
                }
            }
            Command::CancelPair => self.hosts.cancel_pairing(),
            Command::AddHost(text) => self.add_entry(&text),
            Command::PairLink(link) => self.pair_link(*link),
            Command::AddFromClipboard => match gui::clipboard_text() {
                Some(text) if rubylight_protocol::pairing_uri::PairingUri::is_link(text.trim()) => self.add_entry(&text),
                _ => self.launcher.message(tr("No pairing link to paste"), tr(hosts::CLIPBOARD_NO_LINK)),
            },
            Command::Unpair(host) => {
                // The library and the certificate go when the PC has answered (see the event).
                if let Err(e) = self.hosts.unpair(host) {
                    self.launcher.message(tr("Could not unpair"), e);
                }
            }
            Command::Remove(host) => {
                remove_host(&mut self.hosts, &self.store, &mut self.launcher.recents, &mut self.launcher.favourites, &mut self.launcher.open_game, host);
            }
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
            Command::ReadAppsAgain(host) => {
                self.hosts.poll_one(host);
                self.hosts.load_apps(host);
            }
            Command::RequestArt { host, app, version } => self.hosts.request_art(host, app, &version, &self.store),
            Command::CloseGame(host) => self.hosts.session_action(host, ActionId::CloseGame),
            Command::LoadApps(host) => {
                if matches!(self.hosts.get(host).map(|h| &h.apps), Some(Apps::NotLoaded)) {
                    self.hosts.load_apps(host);
                }
            }
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
            Command::RetryLaunch => {
                if let Some((host, app, resume, kind)) = self.failed_launch.take() {
                    self.launch(host, app, resume, kind, LaunchFrom::Launcher);
                }
            }
            Command::TestConnection => self.launcher.test_connection(self.waker.clone()),
            Command::SearchAgain => {
                if !self.hosts.search_again() {
                    eprintln!("nightjar: looking for PCs on the network could not start again");
                }
            }
            Command::CancelConnect => {
                if let Some(cli) = self.cli.as_mut() { cli.cancel(); }
                self.network_launch = None;
                if let Some(c) = &self.connecting {
                    c.cancel();
                    self.cancelled = true;
                }
            }
        }
    }
}
