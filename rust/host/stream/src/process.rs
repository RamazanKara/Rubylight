//! Application launch, process tracking, foreground focus, and command hooks.
use anyhow::{Result, bail};
use butterpollo_session::state::{App, PrepCommand};
use butterpollo_win_io::process::{CommandCancellation, Process};
#[cfg(test)]
use std::collections::BTreeSet;
use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    sync::Arc,
    time::Duration,
};

mod commands;
mod focus;
mod launch;
mod lifecycle;
#[cfg(test)]
mod parity_tests;

pub use commands::{ClientCommands, MenuRun, app_bool, exit_timeout, pc_command, pc_command_names};
use commands::{StateCommands, directory, inferred_working_dir, start_state_commands};
#[cfg(test)]
use commands::{client_stream_environment, expand};

pub(crate) use focus::store_client;
#[cfg(test)]
use focus::{
    ExitAction, exit_action, focus_candidates, foreign_game, started_from_desktop, watches_program,
};
use focus::{Focus, game_trees, only_opens_launchers};
pub use focus::{FocusSnapshot, ForegroundCheck, ForegroundVerdict, ProcessScan};

#[cfg(test)]
use launch::launch_with_environment;
pub use launch::{BACKGROUND_ARG, LAUNCH_STOPPED, launch};
use launch::{prep_failure, wait_failure};

pub struct RunningApp {
    pub id: u32,
    pub name: String,
    pub child: Option<Process>,
    pub owner: String,
    /// The identity of the virtual display the game is on, as streams lease it:
    /// not the owner's as such, since a press moves the owner while the game
    /// stays. None on a physical display, or before a stream showed it (a game
    /// the console started).
    pub display: Option<String>,
    pub generation: String,
    pub background: bool,
    undo: Vec<PrepCommand>,
    working: String,
    environment: BTreeMap<String, String>,
    started: std::time::Instant,
    auto_detach: bool,
    wait_all: bool,
    detached: bool,
    pub allow_client_commands: bool,
    /// The app's own stream-menu commands (`menu-cmd`), listed after the host-wide
    /// `server_cmd` ones, and whether the app hides those (`exclude-global-menu-cmd`).
    pub menu_commands: Vec<butterpollo_input_policy::menu_commands::MenuCommand>,
    pub exclude_global_menu: bool,
    /// The app's menu commands that ran, undone when the app stops.
    menu_undo: butterpollo_input_policy::menu_commands::UndoLog,
    terminate_on_pause: bool,
    connected: bool,
    state_events: Option<StateCommands>,
    exit_timeout: Duration,
    /// The launching device's game settings files, swapped in; they go back once the app's
    /// processes are closed.
    settings_swap: Option<crate::game_settings::Swap>,
    /// Told once the app's processes have stopped, before its undo commands.
    processes_stopped: Option<std::sync::mpsc::Sender<()>>,
    /// A test's pause before that notice, standing in for a game whose close
    /// takes longer than the shutdown's audio wait.
    #[cfg(test)]
    pub stop_delay: Option<std::time::Duration>,
    /// When the game's programs were last recorded for [`Self::settings_swap`].
    settings_watch: std::time::Instant,
    /// An app launched before anyone signed in, started once a user does.
    pub deferred: Option<(App, HashMap<String, String>)>,
    deferred_check: std::time::Instant,
    /// A game Steam starts, followed through its install folder.
    steam: Option<butterpollo_session::steam::Tracker>,
    steam_check: std::time::Instant,
    /// A game Playnite starts.
    playnite: Option<crate::playnite::Launch>,
    /// Playnite's fullscreen mode, closed when the app stops.
    playnite_fullscreen: bool,
    /// Lossless Scaling for the game.
    lossless: Option<crate::lossless::Session>,
    /// Whether the app starts a program, and when its own command started, after the
    /// preparation commands (100 ns since 1601, as process creation times).
    launches: bool,
    launched_at: u64,
    /// Fullscreen games on the stream's display that another program, such
    /// as a store client, started after the launch (pid → creation time).
    /// Quitting closes them like the app's own processes.
    foreign: BTreeMap<u32, u64>,
    /// Fullscreen programs on that display found not to be such a game (pid →
    /// program), such as one the desktop started: never closed, and not
    /// looked at again.
    not_foreign: BTreeMap<u32, String>,
    /// The process whose [`ForegroundCheck`] runs.
    foreground_check: Option<u32>,
    /// The game's own process or window has been seen, so a launcher window is no longer
    /// standing in for it (`launcher_watch`).
    game_seen: bool,
    /// Every program the app starts only opens a launcher's own window (Steam Big Picture): the
    /// launcher is the app, so its windows are no prompt standing in for a game.
    launcher_only: bool,
    /// The launched game's window first seen, and the brings-back since (issue #131).
    focus: Option<Focus>,
}
/// The processes of this host's user session, among which a running app's
/// game processes are found. Listing them takes milliseconds: list them
/// before locking `current_app`, which console and launch requests wait on.
pub fn session_processes() -> Result<Vec<butterpollo_session::steam::Process>> {
    let session = butterpollo_win_io::process::current_session()?;
    let mut processes = butterpollo_win_io::process::processes()?;
    // A launcher may report the same executable in several signed-in
    // sessions. Game ownership never crosses this host's user session.
    processes.retain(|p| butterpollo_win_io::process::process_session(p.pid).ok() == Some(session));
    Ok(processes)
}
impl RunningApp {
    /// Take the place of the placeholder a launch before sign-in left, as the same
    /// session: its generation, and the display the game's stream is on.
    pub fn take_place_of(&mut self, placeholder: &mut RunningApp, owner: String) {
        self.generation = placeholder.generation.clone();
        self.owner = owner;
        self.display = placeholder.display.take();
    }
    /// Id, name and owner for `app.started`, or none for a placeholder that
    /// waits for a sign-in to start.
    pub fn started_event(&self) -> Option<(u32, String, String)> {
        self.deferred
            .is_none()
            .then(|| (self.id, self.name.clone(), self.owner.clone()))
    }
    /// Only established launch ownership can choose a game-audio target. In
    /// particular, never capture Steam/Playnite itself or an arbitrary foreground
    /// window when its game process has not been identified.
    /// `processes` is [`session_processes`], listed before the caller locked
    /// `current_app`.
    pub fn audio_root(
        &self,
        processes: &[butterpollo_session::steam::Process],
    ) -> Result<butterpollo_session::audio_scope::Identity> {
        butterpollo_session::audio_scope::root(&self.owned_processes(processes)?, processes)
    }
    /// The verified game processes and every process they started, whose
    /// devices game-only audio sets. None for an app without one (Desktop, a
    /// detached command).
    pub fn game_pids(&self) -> Result<Vec<u32>> {
        if !self.has_game_process() {
            return Ok(Vec::new());
        }
        let (owned, processes) = self.game_processes()?;
        Ok(game_trees(&owned, &processes))
    }
    /// The processes whose windows a background stream keeps on its display,
    /// identified as for game-only audio.
    pub fn window_processes(&self) -> Result<std::collections::BTreeSet<u32>> {
        let processes = session_processes()?;
        self.window_processes_in(&processes)
    }
    /// Same as [`window_processes`], using a process list captured outside the
    /// `current_app` lock.
    pub fn window_processes_in(
        &self,
        processes: &[butterpollo_session::steam::Process],
    ) -> Result<std::collections::BTreeSet<u32>> {
        let owned = self.owned_processes(processes)?;
        Ok(butterpollo_session::background::window_owners(
            &owned, processes,
        ))
    }
    /// Its game settings files' swap, for [`crate::game_settings::hand_over`].
    pub fn settings_handle(&self) -> Option<crate::game_settings::Handle> {
        self.settings_swap
            .as_ref()
            .map(crate::game_settings::Swap::handle)
    }
    /// Whether the app is the desktop: it starts nothing, so every stream of it
    /// shows its own display.
    pub fn is_desktop(&self) -> bool {
        !self.launches && !self.has_game_process()
    }
    /// The verified game processes (pid → creation time) among `processes`
    /// that Quick Suspend starts from; none for Desktop or a detached command.
    pub(crate) fn suspend_roots(
        &self,
        processes: &[butterpollo_session::steam::Process],
    ) -> Result<BTreeMap<u32, u64>> {
        if !self.has_game_process() {
            return Ok(BTreeMap::new());
        }
        self.owned_processes(processes)
    }
    pub(crate) fn has_game_process(&self) -> bool {
        self.steam.is_some()
            || self.playnite.is_some()
            || self.child.is_some()
            || !self.foreign.is_empty()
    }
    /// The processes whose windows may come back after a display's departure
    /// minimised them. None for a background game, whose windows stay out of the
    /// PC's way, or an app with no game process to name (Desktop, a detached command).
    pub fn departure_windows(&self) -> Result<std::collections::BTreeSet<u32>> {
        if self.background || !self.has_game_process() {
            return Ok(Default::default());
        }
        self.window_processes()
    }
    /// The verified game processes (pid → creation time) and this session's
    /// process list.
    fn game_processes(
        &self,
    ) -> Result<(BTreeMap<u32, u64>, Vec<butterpollo_session::steam::Process>)> {
        let processes = session_processes()?;
        Ok((self.owned_processes(&processes)?, processes))
    }
    /// The verified game processes (pid → creation time) among `processes`.
    fn owned_processes(
        &self,
        processes: &[butterpollo_session::steam::Process],
    ) -> Result<BTreeMap<u32, u64>> {
        let owned = if let Some(tracker) = &self.steam {
            tracker.tracked.clone()
        } else if let Some(playnite) = &self.playnite {
            playnite.audio_processes(processes)?
        } else {
            let mut owned: BTreeMap<u32, u64> = if let Some(child) = &self.child {
                let ids = child.process_ids()?;
                processes
                    .iter()
                    .filter(|p| ids.contains(&p.pid))
                    .map(|p| (p.pid, p.started))
                    .collect()
            } else if self.foreign.is_empty() {
                anyhow::bail!(
                    "this app has no owned game process; game-only audio and background streaming are unavailable for Desktop or detached commands"
                );
            } else {
                BTreeMap::new()
            };
            owned.extend(self.foreign.iter().map(|(&pid, &started)| (pid, started)));
            owned
        };
        Ok(owned)
    }
    pub fn with_environment(app: &App, environment: BTreeMap<String, String>) -> Result<Self> {
        Self::with_environment_cancellable(app, environment, None)
    }
    /// As `with_environment`; `cancel` ends a running preparation command and fails the
    /// launch with [`LAUNCH_STOPPED`] before the next one or the app starts. The commands that
    /// finished are undone, as on any failed launch.
    pub fn with_environment_cancellable(
        app: &App,
        environment: BTreeMap<String, String>,
        cancel: Option<&CommandCancellation>,
    ) -> Result<Self> {
        let stopped = || cancel.is_some_and(CommandCancellation::requested);
        let mut running = Self {
            id: app.id(),
            name: app.name.clone(),
            child: None,
            owner: environment
                .get("SUNSHINE_CLIENT_UUID")
                .cloned()
                .unwrap_or_default(),
            display: None,
            generation: uuid::Uuid::new_v4().to_string(),
            background: false,
            undo: vec![],
            working: if app.working_dir.trim().is_empty() {
                inferred_working_dir(&app.cmd)
            } else {
                app.working_dir.clone()
            },
            environment,
            started: std::time::Instant::now(),
            auto_detach: app_bool(app, "auto-detach", true),
            wait_all: app_bool(app, "wait-all", true),
            detached: false,
            allow_client_commands: app_bool(app, "allow-client-commands", true),
            menu_commands: app
                .extra
                .get(butterpollo_input_policy::menu_commands::APP_KEY)
                .map(|list| butterpollo_input_policy::menu_commands::entries(list, true))
                .unwrap_or_default(),
            exclude_global_menu: app_bool(
                app,
                butterpollo_input_policy::menu_commands::EXCLUDE_GLOBAL_KEY,
                false,
            ),
            menu_undo: Default::default(),
            terminate_on_pause: app_bool(app, "terminate-on-pause", false),
            connected: false,
            state_events: None,
            deferred: None,
            deferred_check: std::time::Instant::now(),
            steam: None,
            steam_check: std::time::Instant::now(),
            playnite: None,
            playnite_fullscreen: false,
            lossless: None,
            launches: !app.cmd.trim().is_empty()
                || app
                    .extra
                    .get("detached")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|commands| !commands.is_empty()),
            launched_at: butterpollo_win_io::process::now(),
            foreign: BTreeMap::new(),
            not_foreign: BTreeMap::new(),
            foreground_check: None,
            game_seen: false,
            launcher_only: only_opens_launchers(app),
            focus: None,
            exit_timeout: exit_timeout(app, &butterpollo_config::config::Config::default()),
            settings_swap: None,
            settings_watch: std::time::Instant::now(),
            processes_stopped: None,
            #[cfg(test)]
            stop_delay: None,
        };
        let commands: Vec<PrepCommand> = serde_json::from_value(
            app.extra
                .get("state-cmd")
                .cloned()
                .unwrap_or_else(|| serde_json::json!([])),
        )?;
        if !commands.is_empty() {
            let (sender, receiver) = std::sync::mpsc::channel::<bool>();
            let cancel = Arc::new(CommandCancellation::new()?);
            let stopping = cancel.clone();
            let mut environment = running.environment.clone();
            let working = running.working.clone();
            let worker = std::thread::Builder::new()
                .name("app-state-commands".into())
                .spawn(move || {
                    for active in receiver {
                        if stopping.requested() {
                            break;
                        }
                        environment.insert(
                            "APOLLO_APP_STATUS".into(),
                            if active { "RESUMING" } else { "PAUSING" }.into(),
                        );
                        start_state_commands(&commands, active, &environment, |cmd, elevated| {
                            Process::shell_detached(
                                cmd,
                                directory(&working),
                                elevated,
                                &environment,
                            )
                        });
                    }
                })?;
            running.state_events = Some(StateCommands {
                sender: Some(sender),
                cancel,
                worker: Some(worker),
            });
        }
        for (index, prep) in app.prep.iter().enumerate() {
            if stopped() {
                bail!(LAUNCH_STOPPED);
            }
            if !prep.r#do.is_empty() {
                let child = Process::shell_leaving_children(
                    &prep.r#do,
                    directory(&running.working),
                    prep.elevated,
                    &running.environment,
                )
                .map_err(|error| {
                    prep_failure(&app.name, index, &format!("could not start ({error:#})"))
                })?;
                let waited = match cancel {
                    Some(cancel) => child.wait_cancellable(Duration::from_secs(120), cancel),
                    None => child.wait(Duration::from_secs(120)).map(Some),
                };
                let code = waited.map_err(|error| {
                    tracing::warn!(%error, app = %app.name, "preparation command wait failed");
                    wait_failure(&app.name, index, stopped())
                })?;
                // Ended by a stop: this command's own undo does not run, the earlier ones' do.
                let Some(code) = code else {
                    bail!(LAUNCH_STOPPED);
                };
                // Failed: the launch is cancelled and this command's undo is never queued, so what
                // it started ends with it when `child` drops, like after a stop or a timeout.
                if code != 0 {
                    return Err(prep_failure(
                        &app.name,
                        index,
                        &format!("ended with exit code {code}"),
                    ));
                }
                // Ended by itself with exit code 0: what it started and left running stays
                // (issue 135).
                child.leave_children_running_or_warn();
            }
            running.undo.push(prep.clone());
        }
        if stopped() {
            bail!(LAUNCH_STOPPED);
        }
        // A program the preparation commands left running is never taken for a game another
        // program started.
        running.launched_at = butterpollo_win_io::process::now();
        let imported = app
            .extra
            .get("launcher-launch")
            .filter(|_| {
                app.extra
                    .get("launcher-command")
                    .and_then(serde_json::Value::as_str)
                    == Some(app.cmd.as_str())
            })
            .map(|action| {
                serde_json::from_value::<butterpollo_session::library::Launch>(action.clone())
            })
            .transpose()?;
        // Launchers start games outside this app's job. Track only new game
        // processes so closing a stream cannot terminate a shared launcher.
        if let Some(folder) = app
            .extra
            .get("steam-install-dir")
            .or_else(|| app.extra.get("launcher-install-dir"))
            .and_then(serde_json::Value::as_str)
            .filter(|folder| !folder.trim().is_empty())
            .filter(|_| {
                app.extra
                    .get("steam-id")
                    .is_some_and(serde_json::Value::is_string)
                    || imported.is_some()
            })
        {
            match butterpollo_win_io::process::processes() {
                Ok(before) => {
                    running.steam = Some(butterpollo_session::steam::Tracker::new(
                        &before,
                        folder,
                        Duration::from_secs(15),
                    ))
                }
                Err(error) => tracing::warn!(%error, "Game tracking unavailable"),
            }
        }
        if let Some(commands) = app
            .extra
            .get("detached")
            .and_then(serde_json::Value::as_array)
        {
            for command in commands {
                let command = command
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("detached command must be a string"))?;
                Process::shell_detached(
                    command,
                    directory(&running.working),
                    app_bool(app, "elevated", false),
                    &running.environment,
                )?;
            }
        }
        if let Some(action) = imported {
            butterpollo_win_io::library::launch(
                &action,
                directory(&running.working),
                app_bool(app, "elevated", false),
                &running.environment,
            )?;
        } else if !app.cmd.is_empty() {
            let target = butterpollo_win_io::process::command_target(&app.cmd);
            // The app starts with the user's token; a SYSTEM host cannot
            // judge a mapped drive or network share the user can open.
            if target.to_ascii_lowercase().ends_with(".exe")
                && !target.contains(['%', '!', '^'])
                && Path::new(target).is_absolute()
                && butterpollo_win_io::files::local_volume(Path::new(target))
                && !Path::new(target).is_file()
            {
                bail!("application executable not found: {target}");
            }
            let command = if let Some(output) = app
                .extra
                .get("output")
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.is_empty())
            {
                if output.contains(['\0', '\r', '\n', '"', '%', '!']) {
                    bail!("application output path contains shell metacharacters");
                }
                let output = if output == "null" { "NUL" } else { output };
                if output != "NUL"
                    && let Some(parent) = Path::new(output)
                        .parent()
                        .filter(|p| !p.as_os_str().is_empty())
                {
                    std::fs::create_dir_all(parent)?;
                }
                redirected_command(&app.cmd, output)
            } else {
                app.cmd.clone()
            };
            running.child = Some(Process::shell(
                &command,
                directory(&running.working),
                app_bool(app, "elevated", false),
                &running.environment,
            )?);
        }
        running.started = std::time::Instant::now();
        Ok(running)
    }
    /// A Steam game looked for in `install_dir`, with its first look due.
    #[cfg(test)]
    pub fn track_steam(&mut self, install_dir: &str) {
        self.steam = Some(butterpollo_session::steam::Tracker::new(
            &[],
            install_dir,
            Duration::ZERO,
        ));
        self.steam_check = std::time::Instant::now() - Duration::from_secs(2);
    }
    /// As an app that started a program, without starting one.
    #[cfg(test)]
    pub fn mark_launching(&mut self) {
        self.launches = true;
    }
    /// The processes the Steam game's tracker follows.
    #[cfg(test)]
    pub fn steam_tracked(&self) -> Vec<u32> {
        self.steam.as_ref().map_or_else(Vec::new, |tracker| {
            tracker.tracked.keys().copied().collect()
        })
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn mark_game_seen(&mut self) {
        self.game_seen = true;
    }
}

/// Wraps `cmd` with output redirection after applying `shell_command` so URI
/// schemes (`steam://`, `ms-settings:`) and `.lnk`/document targets still open
/// through `cmd /c start` instead of having a leading `(` break scheme parsing.
pub(crate) fn redirected_command(cmd: &str, output: &str) -> String {
    let inner = butterpollo_win_io::process::shell_command(cmd);
    format!("({inner}) 1>>\"{output}\" 2>&1")
}

#[cfg(test)]
mod tests;
