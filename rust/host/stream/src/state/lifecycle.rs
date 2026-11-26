//! Host startup loading, running-app shutdown, audio route release, and paused display
//! grace-period reaping.

use super::*;

/// How long the audio route may hold while the game closes, counted from the
/// stop's call; main's passed deadline can make it shorter. Past it the route
/// goes back even if the game ignores close.
pub(super) const AUDIO_RELEASE_WAIT: Duration = Duration::from_secs(6);

/// A client's display lease and when it last stopped being streamed.
pub type RetainedDisplay = (Arc<crate::display_session::Ready>, Option<Instant>);

/// The display a stream launch resumes: only this client's own retained
/// display, and only in the mode it asks for. Reuse leaves the entry (and the
/// display) untouched; a retained display in another mode is removed and
/// handed back so the caller drops it, restoring Windows, outside the lock.
pub fn take_retained<L: Clone>(
    displays: &mut BTreeMap<String, (L, Option<Instant>)>,
    client: &str,
    matches: impl FnOnce(&L) -> bool,
) -> Result<L, Option<L>> {
    match displays.get(client) {
        Some((lease, _)) if matches(lease) => Ok(lease.clone()),
        Some(_) => Err(displays.remove(client).map(|(lease, _)| lease)),
        None => Err(None),
    }
}

/// Why the running app is stopped, and the client that asked: each stop logs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppStop<'a> {
    /// A client's `/cancel`, Moonlight's Quit.
    Cancel(&'a str),
    /// A client's launch with the Terminate control.
    Terminate(&'a str),
    /// WebRTC's stop-host-session or switch-session.
    WebRtc(&'a str),
    /// Close game from the session controls.
    CloseGame(&'a str),
    /// A client's launch of another app (confirmed, when the host asks).
    Replaced(&'a str),
    /// Close in the web UI.
    WebUi,
    /// Quit app in the tray.
    Tray,
    /// The app exited by itself.
    Exited,
    /// The deferred app failed to start.
    LaunchFailed,
    /// The host shuts down; `deadline` is the end of its stop wait, less the
    /// margin main keeps for the route to go back.
    Shutdown { deadline: Instant },
    #[cfg(any(test, feature = "test-support"))]
    Test,
}
impl<'a> AppStop<'a> {
    pub fn reason(self) -> &'static str {
        match self {
            Self::Cancel(_) => "cancel",
            Self::Terminate(_) => "terminate",
            Self::WebRtc(_) => "webrtc",
            Self::CloseGame(_) => "close game",
            Self::Replaced(_) => "replaced",
            Self::WebUi => "web ui",
            Self::Tray => "tray",
            Self::Exited => "exited",
            Self::LaunchFailed => "launch failed",
            Self::Shutdown { .. } => "shutdown",
            #[cfg(any(test, feature = "test-support"))]
            Self::Test => "test",
        }
    }
    /// The uuid of the client that asked, as the app's owner is.
    pub fn client(self) -> Option<&'a str> {
        match self {
            Self::Cancel(client)
            | Self::Terminate(client)
            | Self::WebRtc(client)
            | Self::CloseGame(client)
            | Self::Replaced(client) => Some(client),
            _ => None,
        }
    }
}

impl Host {
    pub fn stop_app(&self, why: AppStop<'_>) {
        // Shutdown's whole stop runs under main's wait: the audio route may hold
        // only the earlier of its own wait and the deadline main passed, which
        // already leaves main's margin for the route to go back.
        let audio_deadline = match why {
            AppStop::Shutdown { deadline } => {
                Some((Instant::now() + AUDIO_RELEASE_WAIT).min(deadline))
            }
            _ => None,
        };
        // A game moved by Continue here whose new device's stream has arrived saves its settings
        // files for that device, also when it closes before the next housekeeping tick settles
        // the move. Callers that end the streams first settle before they do: a stopped stream
        // no longer counts as arrived.
        self.settle_move();
        let teardown = self.session_teardown(crate::session_control::Scope::Game);
        // Stopping the app waits for it to exit, up to its exit timeout and
        // undo commands: never under the lock every serverinfo request takes.
        let mut app = self.current_app.lock().unwrap().take();
        // The launcher watch is over with the app it watched: its prompt
        // notice goes here rather than waiting for a tick to notice the app
        // slot is empty, which on a resting host never comes.
        self.notes.end_launch();
        // At shutdown the wait runs under a budget. The display layout goes
        // back before the game is asked to close; its audio route stays until
        // its processes have stopped (before its undo commands), or until its
        // part of the wait runs out, for a game that ignores close.
        if let Some(deadline) = audio_deadline {
            let displays = std::mem::take(&mut *self.app_display.lock().unwrap());
            drop(displays);
            if let Some(mut running) = app.take() {
                let (stopped, stopped_rx) = std::sync::mpsc::channel();
                running.notify_when_processes_stop(stopped);
                std::thread::scope(|scope| {
                    scope.spawn(move || self.release_audio_by(stopped_rx, deadline));
                    self.end_app(Some(running), why);
                    drop(self.app_audio.lock().unwrap().take());
                    drop(std::mem::take(&mut *self.app_display.lock().unwrap()));
                });
            } else {
                drop(self.app_audio.lock().unwrap().take());
                self.end_app(None, why);
            }
            drop(teardown);
            return;
        }
        self.end_app(app, why);
        let audio = self.app_audio.lock().unwrap().take();
        let displays = std::mem::take(&mut *self.app_display.lock().unwrap());
        drop((audio, displays));
        drop(teardown);
    }

    /// Releases the app's audio route once its processes have stopped, or at
    /// `deadline` when the game ignores close, whichever comes first. Only the
    /// deadline warns: a normal close releases by its notice.
    pub(super) fn release_audio_by(
        &self,
        stopped: std::sync::mpsc::Receiver<()>,
        deadline: Instant,
    ) {
        let timed_out = matches!(
            stopped.recv_timeout(deadline.saturating_duration_since(Instant::now())),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        );
        let audio = self.app_audio.lock().unwrap().take();
        if timed_out && audio.is_some() {
            tracing::warn!(
                "the game's processes had not stopped when the shutdown's audio wait ran out; releasing its audio route"
            );
        }
        drop(audio);
    }

    /// Stops `app`, taken out of `current_app`: logs why, waits for it to
    /// exit and raises app.stopped. Never under the `current_app` lock.
    pub fn end_app(&self, app: Option<crate::process::RunningApp>, why: AppStop<'_>) {
        let mut stopped = None;
        if let Some(app) = &app {
            tracing::info!(
                reason = why.reason(),
                by = why.client(),
                app = %app.name,
                app_id = app.id,
                owner = %app.owner,
                "stopping the application"
            );
            // A placeholder that never started raised no app.started.
            stopped = app.started_event();
        }
        // Its game-only audio goes back while the game still runs, and is not
        // routed again until it stopped.
        let closing = app
            .as_ref()
            .and_then(|app| crate::stream::put_back(&self.directory, || app.game_pids()));
        drop(app);
        drop(closing);
        if let Some((id, name, owner)) = stopped {
            self.webhook(
                "app.stopped",
                json!({
                    "app": {"id": id, "name": name},
                    "device": self.webhook_device(&owner),
                    "reason": why.reason(),
                    "by": self.webhook_device(why.client().unwrap_or("")),
                }),
            );
        }
    }

    pub fn reap_paused_display(&self) {
        self.settle_move();
        // With `hold_sink_while_paused` a game left running keeps the virtual
        // speakers as the default until a device returns, End session or the
        // game stops, so the PC's own speakers stay silent meanwhile.
        let hold_audio = {
            let config = self.config.read().unwrap();
            config.boolean("hold_sink_while_paused", false)
        } && self.current_app.lock().unwrap().is_some();
        let audio = {
            let mut audio = self.app_audio.lock().unwrap();
            if !hold_audio
                && audio
                    .as_ref()
                    .is_some_and(|route| Arc::strong_count(route) == 1)
            {
                audio.take()
            } else {
                None
            }
        };
        drop(audio);
        if self.app_display.lock().unwrap().is_empty() {
            return;
        }
        let config = self.config.read().unwrap().clone();
        // A stream that moved to another device, which runs now, gives its display up at once.
        let moved = self.moves.displays_to_release();
        // A press waiting for its stream keeps the displays it displaced; their grace starts
        // over if it fails.
        let held = self.moves.displays_held();
        let expired: Vec<String> = {
            let mut displays = self.app_display.lock().unwrap();
            let mut expired = vec![];
            for (owner, (lease, paused)) in displays.iter_mut() {
                if Arc::strong_count(lease) > 1 || (held.contains(owner) && !moved.contains(owner))
                {
                    *paused = None;
                } else if moved.contains(owner)
                    || config
                        .display_disconnect_delay(lease.exclusive_layout())
                        .is_some_and(|delay| {
                            paused.get_or_insert_with(Instant::now).elapsed() >= delay
                        })
                {
                    expired.push(owner.clone());
                }
            }
            expired
        };
        if expired.is_empty() {
            return;
        }
        self.moves.released(&expired);
        // The grace period ran out: the session ends as End session does,
        // while the display is still held so the state shows the suspension.
        let teardown = self.session_teardown(crate::session_control::Scope::Displays(&expired));
        let released: Vec<_> = {
            let mut displays = self.app_display.lock().unwrap();
            let mut released = vec![];
            for owner in &expired {
                // A stream that took the display back in the meantime keeps it.
                if displays
                    .get(owner)
                    .is_some_and(|(lease, _)| Arc::strong_count(lease) == 1)
                {
                    released.extend(displays.remove(owner));
                }
            }
            released
        };
        // Display leases restore Windows settings; do that outside the lock.
        drop(released);
        drop(teardown);
    }

    pub fn load(directory: PathBuf, assets: PathBuf, port: Option<u16>) -> Result<Shared> {
        std::fs::create_dir_all(&directory)?;
        // A restore the console staged applies before anything is read
        // (docs/configuration.md, "Settings backup and restore").
        let restored = crate::settings_backup::apply_staged(&directory);
        let config_path = directory.join("sunshine.conf");
        let (mut config, retired) = Config::load_retiring(&config_path)?;
        let warnings = Warnings::default();
        crate::host_warnings::restore(&directory, &warnings);
        if let Err(message) = restored {
            crate::host_warnings::notice(&directory, &warnings, "settings_restore", &message);
        }
        if let Some(warning) = butterpollo_config::config::retired_warning(&retired) {
            // Written back, so the file no longer has them; the notice is saved
            // until the console dismisses it. A file that cannot be written
            // keeps the old values and warns again on the next start.
            if let Err(error) = butterpollo_session::state::atomic_write(
                &config_path,
                Config::load(&config_path)?.text().as_bytes(),
            ) {
                tracing::warn!(error = %format!("{error:#}"), "the replaced settings could not be saved");
            }
            crate::host_warnings::notice(&directory, &warnings, "settings_retired", &warning);
        }
        match butterpollo_session::webhook::migrate_from_config(&mut config, &directory) {
            Ok(true) => {
                if let Err(error) =
                    butterpollo_session::state::atomic_write(&config_path, config.text().as_bytes())
                {
                    tracing::warn!(error = %format!("{error:#}"), "migrated webhook secrets could not be saved back to config");
                }
            }
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(error = %format!("{error:#}"), "webhook secrets could not be migrated");
            }
        }
        if let Some(warning) = butterpollo_win_io::limiter::take_stale_nvidia(&directory) {
            crate::host_warnings::notice(&directory, &warnings, "nvidia_limiter_retired", &warning);
        }
        if let Some(port) = port {
            config.values.insert("port".into(), port.to_string());
        }
        config.ports()?;
        let ProfileFiles {
            paired: paired_path,
            credentials: credentials_path,
            apps: apps_path,
            aliases: aliases_path,
            certificate,
            key,
        } = ProfileFiles::new(&config, &directory);
        let mut aliases = butterpollo_session::state::load_json(&aliases_path, json!({"root":{}}))?;
        let mut paired = PairedState::load(&paired_path)?;
        // Vibepollo keeps the shared virtual display's GUID in
        // vibeshine_state.json; reuse it so Windows keeps that display's
        // settings.
        if paired.document["root"]["shared_virtual_display_guid"]
            .as_str()
            .is_none()
            && let Some(id) = aliases["root"]["shared_virtual_display_guid"]
                .as_str()
                .filter(|id| uuid::Uuid::parse_str(id).is_ok())
        {
            paired.document["root"]["shared_virtual_display_guid"] = id.into();
        }
        // Once per host: a saved amd_ltr_frames = 0 from before 2.0.0 is
        // that release's old default, not a choice; drop it from the file
        // too, so the console shows the current default.
        let upgraded = &mut paired.document["root"]["amd_ltr_frames_upgraded"];
        if upgraded.as_bool() != Some(true) {
            let mut stored = Config::load(&config_path)?;
            if stored.upgrade_ltr_default(false) {
                butterpollo_session::state::atomic_write(&config_path, stored.text().as_bytes())?;
                config.upgrade_ltr_default(false);
                tracing::info!(
                    "amd_ltr_frames = 0, the default before 2.0.0, was removed from sunshine.conf: AV1 now recovers lost frames without a keyframe. Set it to 0 again to turn that off"
                );
            }
            *upgraded = true.into();
        }
        // Once per host: an "extended" device layout from before the marker that names what
        // a stream's layout replaced is either one a stream learned (2.4.1 to 2.5.9) or a
        // pick made in Devices before 3.0.0, which look the same. A host that asks for the
        // primary wins over it, as the stream was after the main screen (issue #33);
        // otherwise it stays, learned over the host's layout, so the device follows that
        // layout until you change it and an app that asks for the primary still gets it. The
        // console names the devices that changed.
        if paired.document["root"]["legacy_extended_layout_migrated"].as_bool() != Some(true) {
            let host_layout = crate::display_session::legacy_host_layout(&config);
            let changed = crate::display_session::migrate_legacy_extended_layouts(
                &mut paired.clients,
                host_layout,
            );
            paired.document["root"]["legacy_extended_layout_migrated"] = true.into();
            if !changed.is_empty() {
                crate::host_warnings::notice(
                    &directory,
                    &warnings,
                    "display_layout_learned",
                    &crate::display_session::legacy_layout_notice(&changed, host_layout),
                );
            }
        }
        paired.save(&paired_path)?;
        let identity = Identity::load(&certificate, &key)?;
        let console =
            butterpollo_crypto::crypto::ConsoleCertificate::load(&directory.join("credentials"))?;
        let credentials = Credentials::load(&credentials_path)?;
        let credentials_stamp = file_stamp(&credentials_path);
        let mut app_document = load_library(
            &apps_path,
            // Vibepollo's default library.
            json!({"env":{},"apps":[
                {"name":"Desktop","image-path":"desktop.png","allow-client-commands":false},
                {"name":"Steam Big Picture","prep-cmd":[{"do":"","undo":"steam://close/bigpicture","elevated":false}],
                 "detached":["steam://open/bigpicture"],"image-path":"steam.png"}
            ]}),
        )?;
        if normalize_library(&mut app_document) {
            butterpollo_session::state::write_json(&apps_path, &app_document)?;
        }
        let apps_stamp = file_stamp(&apps_path);
        let mut apps: Vec<App> =
            serde_json::from_value(app_document.get("apps").cloned().unwrap_or(json!([])))?;
        butterpollo_session::catalog::assign(
            &mut apps,
            &mut aliases,
            assets.parent().unwrap_or(&assets),
        )?;
        crate::host_warnings::network_covers(&warnings, &apps);
        butterpollo_session::state::write_json(&aliases_path, &aliases)?;
        let web_sessions = web_sessions::load(
            &aliases,
            credentials
                .as_ref()
                .map(|c| c.username.as_str())
                .unwrap_or(""),
        );
        let session = crate::session_control::Control::new(&directory);
        let session_history = crate::session_history::History::load(&directory);
        let maintain_wake = Arc::new(tokio::sync::Notify::new());
        // A launch or a stream that starts ends the maintenance loop's idle wait.
        let sessions = Sessions::default().with_waker({
            let wake = maintain_wake.clone();
            move || wake.notify_one()
        });
        Ok(Arc::new(Self {
            #[cfg(any(test, feature = "test-support"))]
            reconnect_fixture: None,
            directory,
            config_path,
            paired_path,
            credentials_path,
            apps_path,
            aliases_path,
            aliases: Mutex::new(aliases),
            config: RwLock::new(config),
            config_snapshot: Mutex::new(None),
            config_version: std::sync::atomic::AtomicU64::new(0),
            config_save: Mutex::new(()),
            port_override: port,
            identity,
            console,
            paired: RwLock::new(paired),
            keyframe_loss_epochs: Default::default(),
            audio_memory: Default::default(),
            credentials: RwLock::new(credentials),
            credentials_stamp: Mutex::new(credentials_stamp),
            app_document: RwLock::new(app_document),
            apps: RwLock::new(apps),
            apps_stamp: Mutex::new(apps_stamp),
            sessions: Mutex::new(sessions),
            controller_sessions: Default::default(),
            hardware_metrics: Mutex::new(None),
            pairings: Mutex::new(Pairings::default()),
            pins: Mutex::new(BTreeMap::new()),
            approvals: Default::default(),
            webrtc_clients_epoch: Arc::new(tokio::sync::watch::channel(0).0),
            webrtc_links: Default::default(),
            guest_net: Default::default(),
            webrtc_service: Mutex::new(None),
            invites: Default::default(),
            otp: Mutex::new(OneTimePins::default()),
            web_sessions: Mutex::new(web_sessions),
            rotated_web_sessions: Mutex::new(HashMap::new()),
            stop: std::sync::atomic::AtomicBool::new(false),
            restart: std::sync::atomic::AtomicBool::new(false),
            stop_reason: Mutex::new(None),
            maintain_wake,
            codecs: std::sync::atomic::AtomicU32::new(0),
            encoder_probe_error: Mutex::new(None),
            encoder_mb_rates: Default::default(),
            packed_modes: std::sync::atomic::AtomicU8::new(0),
            nvidia_enhancements: std::sync::atomic::AtomicU8::new(0),
            nvidia_enhancement_errors: Mutex::new(Vec::new()),
            packed_publish: Mutex::new(()),
            packed_probe_pending: std::sync::atomic::AtomicU8::new(0),
            packed_session_waits: std::sync::atomic::AtomicU8::new(0),
            packed_codec_missing: std::sync::atomic::AtomicU8::new(0),
            packed_reprobing: std::sync::atomic::AtomicBool::new(false),
            optional_probe: Mutex::new(()),
            focus_support: Mutex::new(std::array::from_fn(|_| {
                butterpollo_media_policy::encoder_focus::Support::NotProbed
            })),
            probing_codecs: std::sync::atomic::AtomicBool::new(true),
            codec_probe_requested: std::sync::atomic::AtomicBool::new(false),
            startup_codecs: tokio::sync::watch::channel(false).0,
            startup_packed: tokio::sync::watch::channel(false).0,
            warnings,
            current_app: Mutex::new(None),
            launch_transition: Mutex::new(()),
            launch_cancel: Mutex::new(None),
            power: Default::default(),
            bandwidth_probes: Default::default(),
            pyrowave_probes: Mutex::new(butterpollo_media_policy::bandwidth_probe::Limiter::new(
                butterpollo_media_policy::bandwidth_probe::PYROWAVE,
            )),
            file_transfers: Default::default(),
            usb: Default::default(),
            session: Mutex::new(session),
            session_history,
            confirmations: Mutex::new(Default::default()),
            app_audio: Mutex::new(None),
            app_display: Mutex::new(BTreeMap::new()),
            monitors: Mutex::new(BTreeMap::new()),
            monitors_creating: Default::default(),
            updates: Mutex::new(
                json!({"status":true,"checking":false,"check_failed":false,"checked_at":0,"releases":[]}),
            ),
            assets,
            metadata: Mutex::new(None),
            serverinfo_cache: Mutex::new(Default::default()),
            client_agents: Mutex::new(BTreeMap::new()),
            last_failures: Mutex::new(BTreeMap::new()),
            probed_encoder: Mutex::new(""),
            codec_missing: Mutex::new(Vec::new()),
            radeon: crate::radeon_session::Radeon::default(),
            game_files: crate::game_files::GameFiles::default(),
            notes: crate::session_notes::SessionNotes::default(),
            moves: crate::continue_here::Moves::default(),
            wake: Default::default(),
            webhooks: crate::webhooks::Webhooks::new(crate::webhooks::Policy::DEFAULT)?,
            nucleus: crate::nucleus::Registry::on_pc(),
            neural: Default::default(),
            nvidia_component: Default::default(),
            device_guests: Default::default(),
        }))
    }
}
