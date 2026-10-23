use anyhow::{Context, Result, bail};
#[cfg(test)]
mod parity_tests;
use butterpollo_core::state::{App, PrepCommand};
use butterpollo_windows::process::{CommandCancellation, Process};
use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    sync::Arc,
    time::Duration,
};
#[derive(Clone)]
struct ClientCommand {
    cmd: String,
    elevated: bool,
}
/// A device's command list as Apollo and the C++ host store it too: an empty
/// list may be "", and `elevated` may be "true"/"false" or a number. An entry
/// without a command line is an error: no command may run if its undo list
/// cannot be read.
fn client_commands(value: Option<&serde_json::Value>, key: &str) -> Result<Vec<ClientCommand>> {
    use serde_json::Value;
    let entries = match value {
        None | Some(Value::Null) => return Ok(vec![]),
        Some(Value::String(text)) if text.trim().is_empty() => return Ok(vec![]),
        Some(Value::Array(entries)) => entries,
        Some(_) => bail!("the device's {key} commands are not a list"),
    };
    entries
        .iter()
        .map(|entry| {
            let Some(cmd) = entry.get("cmd").and_then(Value::as_str) else {
                bail!("a device {key} command has no command line");
            };
            Ok(ClientCommand {
                cmd: cmd.to_owned(),
                elevated: match entry.get("elevated") {
                    Some(Value::Bool(flag)) => *flag,
                    Some(Value::Number(number)) => number.as_i64().is_some_and(|n| n != 0),
                    Some(Value::String(text)) => {
                        matches!(
                            text.trim().to_ascii_lowercase().as_str(),
                            "true" | "1" | "yes"
                        )
                    }
                    _ => false,
                },
            })
        })
        .collect()
}
/// Administrator-configured hooks run once for each connected transport, with
/// its environment saved for disconnect even if the application exits first.
pub struct ClientCommands {
    undo: Vec<ClientCommand>,
    environment: BTreeMap<String, String>,
}
impl ClientCommands {
    pub fn start(h: &crate::state::Shared, s: &crate::state::Session) -> Result<Self> {
        if s.launch.role == butterpollo_core::session::Role::InputOnly
            || !s.launch.client.allows_commands()
            || h.current_app
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|app| !app.allow_client_commands)
        {
            return Ok(Self {
                undo: vec![],
                environment: BTreeMap::new(),
            });
        }
        let environment = h
            .current_app
            .lock()
            .unwrap()
            .as_ref()
            .filter(|app| app.deferred.is_none())
            .map(|app| app.environment.clone());
        let mut environment = match environment {
            Some(environment) => environment,
            None => match butterpollo_windows::process::user_environment() {
                Ok(environment) => environment,
                Err(error) => {
                    tracing::info!(%error, "no signed-in user; client commands skipped");
                    return Ok(Self {
                        undo: vec![],
                        environment: BTreeMap::new(),
                    });
                }
            },
        };
        environment.insert(
            "SUNSHINE_CLIENT_NAME".into(),
            butterpollo_core::pairing::command_device_name(&s.launch.client.name),
        );
        environment.insert("SUNSHINE_CLIENT_UUID".into(), s.launch.client.uuid.clone());
        environment.insert("SUNSHINE_CLIENT_WIDTH".into(), s.config.width.to_string());
        environment.insert("SUNSHINE_CLIENT_HEIGHT".into(), s.config.height.to_string());
        environment.insert(
            "SUNSHINE_CLIENT_FPS".into(),
            if h.config
                .read()
                .unwrap()
                .boolean("envvar_compatibility_mode", false)
            {
                s.config.fps.to_string()
            } else {
                butterpollo_core::framegen::Rate(s.config.fps_millihz()).to_string()
            },
        );
        environment.insert("SUNSHINE_CLIENT_HDR".into(), s.config.hdr.to_string());
        for (key, value) in environment.clone() {
            if let Some(suffix) = key.strip_prefix("SUNSHINE_") {
                environment.insert(format!("APOLLO_{suffix}"), value);
            }
        }
        // A device whose commands cannot be read streams without them, as in
        // the C++ host, instead of failing every stream.
        Ok(
            Self::with_environment(&s.launch.client.extra, environment).unwrap_or_else(|error| {
                tracing::warn!(
                    client = %s.launch.client.name,
                    error = format!("{error:#}"),
                    "device commands cannot be used; streaming without them"
                );
                Self {
                    undo: vec![],
                    environment: BTreeMap::new(),
                }
            }),
        )
    }
    fn with_environment(
        extra: &BTreeMap<String, serde_json::Value>,
        environment: BTreeMap<String, String>,
    ) -> Result<Self> {
        let parse = |key: &str| -> Result<Vec<ClientCommand>> {
            let commands = client_commands(extra.get(key), key)?;
            if commands.len() > 64
                || commands
                    .iter()
                    .any(|c| c.cmd.len() > 32767 || c.cmd.contains('\0'))
            {
                bail!("client command list exceeds its limits");
            }
            Ok(commands)
        };
        // Validate both lists before any command is started.
        let commands = parse("do")?;
        let hooks = Self {
            undo: parse("undo")?,
            environment,
        };
        hooks.run(&commands);
        Ok(hooks)
    }
    fn run(&self, commands: &[ClientCommand]) {
        for command in commands.iter().filter(|c| !c.cmd.is_empty()) {
            if let Err(error) = expand(&command.cmd, &self.environment).and_then(|cmd| {
                Process::shell_detached(&cmd, None, command.elevated, &self.environment)
            }) {
                tracing::warn!(%error, "client connection command failed");
            }
        }
    }
}
impl Drop for ClientCommands {
    fn drop(&mut self) {
        self.run(&self.undo);
    }
}
struct StateCommands {
    sender: Option<std::sync::mpsc::Sender<bool>>,
    cancel: Arc<CommandCancellation>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl StateCommands {
    fn send(&self, active: bool) {
        if let Some(sender) = &self.sender {
            let _ = sender.send(active);
        }
    }
}
impl Drop for StateCommands {
    fn drop(&mut self) {
        if let Err(error) = self.cancel.cancel() {
            tracing::warn!(%error, "application state command cancellation failed");
        }
        // Wake an idle receiver as well as a running command. The worker
        // checks cancellation before consuming any queued transition.
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub struct RunningApp {
    pub id: u32,
    pub name: String,
    pub child: Option<Process>,
    pub owner: String,
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
    terminate_on_pause: bool,
    connected: bool,
    state_events: Option<StateCommands>,
    exit_timeout: Duration,
    /// An app launched before anyone signed in, started once a user does.
    pub deferred: Option<(App, HashMap<String, String>)>,
    deferred_check: std::time::Instant,
    /// A game Steam starts, followed through its install folder.
    steam: Option<butterpollo_core::steam::Tracker>,
    steam_check: std::time::Instant,
    /// A game Playnite starts.
    playnite: Option<crate::playnite::Launch>,
    /// Playnite's fullscreen mode, closed when the app stops.
    playnite_fullscreen: bool,
    /// Lossless Scaling for the game.
    lossless: Option<crate::lossless::Session>,
    /// Whether the app starts a program, and when it was launched (100 ns
    /// since 1601, as process creation times).
    launches: bool,
    launched_at: u64,
    /// Fullscreen games on the stream's display that another program, such
    /// as a store client, started after the launch (pid → creation time).
    /// Quitting closes them like the app's own processes.
    foreign: BTreeMap<u32, u64>,
    /// Fullscreen programs on that display the desktop started (pid,
    /// creation time): never closed, and not looked at again.
    desktop: std::collections::BTreeSet<(u32, u64)>,
    /// The game's own process or window has been seen, so a launcher window is no longer
    /// standing in for it (`launcher_watch`).
    game_seen: bool,
}
/// The folder of the program an app starts, as Vibepollo uses when the app
/// has no working directory: games often load files relative to it.
fn inferred_working_dir(command: &str) -> String {
    let target = butterpollo_windows::process::command_target(command);
    if target.contains("://") {
        return String::new();
    }
    let path = Path::new(target);
    path.is_absolute()
        .then(|| path.parent())
        .flatten()
        .filter(|parent| parent.is_dir())
        .map(|parent| parent.to_string_lossy().into_owned())
        .unwrap_or_default()
}
fn directory(s: &str) -> Option<&Path> {
    if s.is_empty() {
        None
    } else {
        Some(Path::new(s))
    }
}
impl RunningApp {
    /// Only established launch ownership can choose a game-audio target. In
    /// particular, never capture Steam/Playnite itself or an arbitrary foreground
    /// window when its game process has not been identified.
    pub fn audio_process(&self) -> Result<butterpollo_windows::process::AudioProcess> {
        let (owned, processes) = self.game_processes()?;
        butterpollo_windows::process::AudioProcess::open(butterpollo_core::audio_scope::root(
            &owned, &processes,
        )?)
    }
    /// The processes whose windows a background stream keeps on its display,
    /// identified as for game-only audio.
    pub fn window_processes(&self) -> Result<std::collections::BTreeSet<u32>> {
        let (owned, processes) = self.game_processes()?;
        Ok(butterpollo_core::background::window_owners(
            &owned, &processes,
        ))
    }
    /// The verified game processes (pid → creation time) and this session's
    /// process list.
    fn game_processes(
        &self,
    ) -> Result<(BTreeMap<u32, u64>, Vec<butterpollo_core::steam::Process>)> {
        let session = butterpollo_windows::process::current_session()?;
        let mut processes = butterpollo_windows::process::processes()?;
        // A launcher may report the same executable in several signed-in
        // sessions. Game ownership never crosses this host's user session.
        processes
            .retain(|p| butterpollo_windows::process::process_session(p.pid).ok() == Some(session));
        let owned = if let Some(tracker) = &self.steam {
            tracker.tracked.clone()
        } else if let Some(playnite) = &self.playnite {
            playnite.audio_processes(&processes)?
        } else {
            let ids = self.child.as_ref().context("this app has no owned game process; game-only audio and background streaming are unavailable for Desktop or detached commands")?.process_ids()?;
            processes
                .iter()
                .filter(|p| ids.contains(&p.pid))
                .map(|p| (p.pid, p.started))
                .collect()
        };
        Ok((owned, processes))
    }
    pub fn with_environment(app: &App, environment: BTreeMap<String, String>) -> Result<Self> {
        let mut running = Self {
            id: app.id(),
            name: app.name.clone(),
            child: None,
            owner: environment
                .get("SUNSHINE_CLIENT_UUID")
                .cloned()
                .unwrap_or_default(),
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
            launched_at: butterpollo_windows::process::now(),
            foreign: BTreeMap::new(),
            desktop: Default::default(),
            game_seen: false,
            exit_timeout: Duration::from_secs(
                app.extra
                    .get("exit-timeout")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(10)
                    .min(300),
            ),
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
                        for command in &commands {
                            if stopping.requested() {
                                return;
                            }
                            let cmd = if active { &command.r#do } else { &command.undo };
                            if cmd.is_empty() {
                                continue;
                            }
                            match expand(cmd, &environment)
                                .and_then(|cmd| {
                                    Process::shell(
                                        &cmd,
                                        directory(&working),
                                        command.elevated,
                                        &environment,
                                    )
                                })
                                .and_then(|process| {
                                    process.wait_cancellable(Duration::from_secs(120), &stopping)
                                }) {
                                Ok(Some(0)) => {}
                                Ok(None) => return,
                                Ok(Some(code)) => {
                                    tracing::warn!(code, "application state command failed");
                                    break;
                                }
                                Err(error) => {
                                    tracing::warn!(%error,"application state command failed");
                                    break;
                                }
                            }
                        }
                    }
                })?;
            running.state_events = Some(StateCommands {
                sender: Some(sender),
                cancel,
                worker: Some(worker),
            });
        }
        for (index, prep) in app.prep.iter().enumerate() {
            if !prep.r#do.is_empty() {
                let child = Process::shell(
                    &prep.r#do,
                    directory(&running.working),
                    prep.elevated,
                    &running.environment,
                )
                .with_context(|| format!("preparation command {} could not start", index + 1))?;
                let code = child.wait(Duration::from_secs(120)).with_context(|| {
                    format!(
                        "preparation command {} did not finish within 120 seconds",
                        index + 1
                    )
                })?;
                if code != 0 {
                    bail!("preparation command {} exited with {code}", index + 1);
                }
            }
            running.undo.push(prep.clone());
        }
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
                serde_json::from_value::<butterpollo_core::library::Launch>(action.clone())
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
            match butterpollo_windows::process::processes() {
                Ok(before) => {
                    running.steam = Some(butterpollo_core::steam::Tracker::new(
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
            butterpollo_windows::library::launch(
                &action,
                directory(&running.working),
                app_bool(app, "elevated", false),
                &running.environment,
            )?;
        } else if !app.cmd.is_empty() {
            let target = butterpollo_windows::process::command_target(&app.cmd);
            // The app starts with the user's token; a SYSTEM host cannot
            // judge a mapped drive or network share the user can open.
            if target.to_ascii_lowercase().ends_with(".exe")
                && !target.contains(['%', '!', '^'])
                && Path::new(target).is_absolute()
                && butterpollo_windows::files::local_volume(Path::new(target))
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
                format!("({}) 1>>\"{}\" 2>&1", app.cmd, output)
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
    pub fn stop(&mut self) {
        self.state_events.take();
        if let Some(lossless) = self.lossless.take() {
            lossless.stop();
        }
        if let Some(tracker) = self.steam.take() {
            butterpollo_windows::process::stop_processes(&tracker.tracked, self.exit_timeout);
        }
        let foreign = std::mem::take(&mut self.foreign);
        if !foreign.is_empty() {
            butterpollo_windows::process::stop_processes(&foreign, self.exit_timeout);
        }
        if let Some(launch) = self.playnite.take() {
            launch.stop(self.exit_timeout);
        }
        if std::mem::take(&mut self.playnite_fullscreen) {
            crate::playnite::close_fullscreen(self.exit_timeout);
        }
        if let Some(child) = self.child.take()
            && let Err(error) = child.stop_graceful(self.exit_timeout)
        {
            tracing::warn!(%error, "application could not exit gracefully");
            let _ = child.stop();
        }
        for prep in self.undo.drain(..).rev() {
            if !prep.undo.is_empty() {
                match Process::shell(
                    &prep.undo,
                    directory(&self.working),
                    prep.elevated,
                    &self.environment,
                )
                .and_then(|p| p.wait(Duration::from_secs(20)))
                {
                    Ok(0) => {}
                    Ok(code) => tracing::warn!(code, "undo command failed"),
                    Err(e) => tracing::warn!(error=%e,"undo command failed"),
                }
            }
        }
    }
    /// The deferred app, once a user has signed in (checked once a second).
    pub fn take_ready_deferred(&mut self) -> Option<(App, HashMap<String, String>)> {
        if self.deferred.is_none() || self.deferred_check.elapsed() < Duration::from_secs(1) {
            return None;
        }
        self.deferred_check = std::time::Instant::now();
        butterpollo_windows::process::user_signed_in()
            .then(|| self.deferred.take())
            .flatten()
    }
    pub fn exited(&mut self) -> Result<bool> {
        if let Some(launch) = &self.playnite {
            return Ok(launch.finished());
        }
        if let Some(tracker) = &mut self.steam {
            use butterpollo_core::steam::Tracked;
            if self.steam_check.elapsed() < Duration::from_secs(1) {
                return Ok(false);
            }
            self.steam_check = std::time::Instant::now();
            let processes = butterpollo_windows::process::processes()?;
            match tracker.update(&processes, butterpollo_windows::process::image_path) {
                Tracked::Waiting => return Ok(false),
                Tracked::Running => {
                    self.game_seen = true;
                    return Ok(false);
                }
                Tracked::Exited => {
                    tracing::info!(app = %self.name, "the game exited");
                    return Ok(true);
                }
                Tracked::Unknown => {
                    if !self.detached {
                        tracing::info!(app = %self.name, "waiting for the game to appear; tracking continues while the stream stays open");
                        self.detached = true;
                    }
                    return Ok(false);
                }
            }
        }
        if self.detached {
            return Ok(false);
        }
        let Some(child) = self.child.as_ref() else {
            return Ok(false);
        };
        let code = child.exit_code()?;
        let waiting = self.wait_all && child.active_processes()? != 0;
        match exit_action(self.auto_detach, self.started.elapsed(), code, waiting) {
            ExitAction::Running => Ok(false),
            ExitAction::Detached(error) => {
                self.detached = true;
                tracing::info!(app=%self.name,"application launcher exited; retaining its streaming session");
                if let Some(code) = error {
                    butterpollo_windows::tray::launch_error(&self.name, code);
                }
                Ok(false)
            }
            ExitAction::Finished => {
                if code == Some(9009) {
                    butterpollo_windows::tray::launch_error(&self.name, 9009);
                }
                Ok(true)
            }
        }
    }
    /// A fullscreen window on the stream's display belongs to process `pid`.
    /// A store client (Xbox, Epic, EA, Ubisoft, Battle.net) starts its games
    /// itself, outside the processes this app owns, so quitting never closed
    /// them. Such a game is remembered when the app launched a program, no
    /// Steam or Playnite tracking covers it, and its process started after the
    /// launch; quitting the plain desktop never closes what a user opened.
    pub fn observe_foreground(&mut self, pid: u32, program: &str) {
        let is_launcher = butterpollo_core::launcher_prompt::launcher_of(program).is_some();
        if !is_launcher
            && self
                .child
                .as_ref()
                .and_then(|child| child.process_ids().ok())
                .is_some_and(|owned| owned.contains(&pid))
        {
            self.game_seen = true;
        }
        if !self.launches
            || self.steam.is_some()
            || self.playnite.is_some()
            || self.foreign.contains_key(&pid)
            || self.foreign.len() >= 8
            || pid == std::process::id()
            || self
                .child
                .as_ref()
                .and_then(|child| child.process_ids().ok())
                .is_some_and(|owned| owned.contains(&pid))
        {
            return;
        }
        let created = butterpollo_windows::process::creation_time(pid);
        if self.desktop.contains(&(pid, created))
            || !foreign_game(program, created, self.launched_at)
        {
            return;
        }
        // A program the user opened on the streamed display (its parent is
        // explorer.exe) is theirs, not the app's. Remembered, so the process
        // list is not read again every second on the stream's thread.
        if butterpollo_windows::process::processes()
            .is_ok_and(|processes| started_from_desktop(pid, &processes))
        {
            self.desktop.insert((pid, created));
            return;
        }
        tracing::info!(app = %self.name, program, pid, "game started by another program; quitting the app closes it");
        self.foreign.insert(pid, created);
        self.game_seen = true;
    }
    /// Whether the game itself is up, and how long ago the app was launched, for an app that
    /// starts a program. A launcher window is a prompt only while the game is not up.
    pub fn launch_progress(&self) -> Option<(bool, Duration)> {
        self.launches
            .then(|| (self.game_seen, self.started.elapsed()))
    }
    #[cfg(test)]
    pub fn mark_game_seen(&mut self) {
        self.game_seen = true;
    }
    /// Return true when the last game transport leaves an app that closes on pause.
    pub fn connection_state(&mut self, connected: bool) -> bool {
        if self.connected == connected {
            return false;
        }
        self.connected = connected;
        if !connected && self.terminate_on_pause {
            return true;
        }
        if let Some(events) = &self.state_events {
            events.send(connected);
        }
        false
    }
}
#[derive(Debug, PartialEq, Eq)]
enum ExitAction {
    Running,
    Detached(Option<u32>),
    Finished,
}
fn exit_action(
    auto_detach: bool,
    elapsed: Duration,
    code: Option<u32>,
    waiting: bool,
) -> ExitAction {
    let Some(code) = code.filter(|_| !waiting) else {
        return ExitAction::Running;
    };
    if auto_detach && code != 9009 && elapsed < Duration::from_secs(5) {
        ExitAction::Detached((code != 0).then_some(code))
    } else {
        ExitAction::Finished
    }
}
/// Whether a fullscreen `program`, created at `created`, is a game a store
/// client started for an app launched at `launched_at`. The store clients
/// themselves never are: quitting a Big Picture app that started Steam must
/// not close Steam.
/// Whether explorer.exe, the desktop, started `pid`: a program the user
/// opened themselves. A parent created after the child is an unrelated
/// process that reused the parent's id.
fn started_from_desktop(pid: u32, processes: &[butterpollo_core::steam::Process]) -> bool {
    let Some(child) = processes.iter().find(|p| p.pid == pid) else {
        return false;
    };
    processes.iter().any(|p| {
        p.pid == child.parent
            && p.started <= child.started
            && p.name.eq_ignore_ascii_case("explorer.exe")
    })
}
fn foreign_game(program: &str, created: u64, launched_at: u64) -> bool {
    const CLIENTS: [&str; 14] = [
        "steam.exe",
        "steamwebhelper.exe",
        "epicgameslauncher.exe",
        "eadesktop.exe",
        "eabackgroundservice.exe",
        "upc.exe",
        "ubisoftconnect.exe",
        "battle.net.exe",
        "xboxpcapp.exe",
        "gamingservices.exe",
        "galaxyclient.exe",
        "playnite.fullscreenapp.exe",
        "playnite.desktopapp.exe",
        "butterpollo.exe",
    ];
    let name = program.rsplit(['/', '\\']).next().unwrap_or(program);
    created != 0
        && created >= launched_at
        && !CLIENTS
            .iter()
            .any(|client| name.eq_ignore_ascii_case(client))
}
pub fn app_bool(app: &App, name: &str, default: bool) -> bool {
    app.extra.get(name).map_or(default, |v| match v {
        serde_json::Value::Bool(b) => *b,
        serde_json::Value::String(s) => matches!(s.as_str(), "true" | "1" | "yes"),
        _ => default,
    })
}
fn expand(value: &str, environment: &BTreeMap<String, String>) -> Result<String> {
    let mut output = String::new();
    let mut rest = value;
    while let Some(index) = rest.find('$') {
        output.push_str(&rest[..index]);
        rest = &rest[index..];
        if let Some(tail) = rest.strip_prefix("$$") {
            output.push('$');
            rest = tail;
        } else if let Some(tail) = rest.strip_prefix("$(") {
            let end = tail
                .find(')')
                .ok_or_else(|| anyhow::anyhow!("unterminated environment variable"))?;
            output.push_str(
                environment
                    .get(&tail[..end].to_uppercase())
                    .map_or("", String::as_str),
            );
            rest = &tail[end + 1..];
        } else {
            output.push('$');
            rest = &rest[1..];
        }
    }
    output.push_str(rest);
    Ok(output)
}
/// Whether starting the app runs any command, which needs the user's
/// environment and token.
fn runs_commands(h: &crate::state::Shared, app: &App) -> bool {
    !app.cmd.trim().is_empty()
        || !app.prep.is_empty()
        || app
            .extra
            .get("playnite-id")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|id| !id.trim().is_empty())
        || playnite_fullscreen(app)
        || app
            .extra
            .get("detached")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|commands| !commands.is_empty())
        || (!app_bool(app, "exclude-global-prep-cmd", false)
            && h.config.read().unwrap().get("global_prep_cmd", "[]").trim() != "[]")
}
/// The launch argument the host sets for a background stream's app; see
/// [`butterpollo_core::background`].
pub const BACKGROUND_ARG: &str = "rubylightBackgroundStream";
pub fn launch(
    h: &crate::state::Shared,
    app: &App,
    args: &HashMap<String, String>,
) -> Result<RunningApp> {
    launch_with_environment(
        h,
        app,
        args,
        butterpollo_windows::process::user_environment(),
    )
}
fn launch_with_environment(
    h: &crate::state::Shared,
    app: &App,
    args: &HashMap<String, String>,
    environment: Result<BTreeMap<String, String>>,
) -> Result<RunningApp> {
    let background = args.get(BACKGROUND_ARG).is_some_and(|v| v == "1");
    let terminate_on_pause = app_bool(
        app,
        "terminate-on-pause",
        h.config
            .read()
            .unwrap()
            .boolean("terminate_on_pause", false),
    );
    let scale_factor = args
        .get("scaleFactor")
        .map(|v| v.parse::<u32>())
        .transpose()?
        .unwrap_or(100);
    let gcmap = args
        .get("gcmap")
        .map(|v| v.parse::<u32>())
        .transpose()?
        .unwrap_or(0);
    for key in ["surroundParams", "surroundAudioInfo"] {
        if let Some(value) = args.get(key) {
            value.parse::<u64>()?;
        }
    }
    let mut environment = match environment {
        Ok(environment) => environment,
        Err(error) => {
            // Before anyone signs in (a service after a reboot) the sign-in
            // screen can still be streamed. As in Vibepollo, the app's
            // commands wait until a user signs in.
            let mut placeholder = app.clone();
            placeholder.cmd.clear();
            placeholder.prep.clear();
            placeholder.extra.remove("detached");
            placeholder.extra.remove("state-cmd");
            placeholder
                .extra
                .insert("exclude-global-prep-cmd".into(), true.into());
            placeholder
                .extra
                .insert("exclude-global-state-cmd".into(), true.into());
            let mut system = BTreeMap::new();
            // The launching client owns the session, as with a started app.
            for (arg, name) in [("clientUuid", "UUID"), ("clientName", "NAME")] {
                if let Some(value) = args.get(arg) {
                    let value = if arg == "clientName" {
                        butterpollo_core::pairing::command_device_name(value)
                    } else {
                        value.clone()
                    };
                    system.insert(format!("SUNSHINE_CLIENT_{name}"), value.clone());
                    system.insert(format!("APOLLO_CLIENT_{name}"), value);
                }
            }
            let mut running = RunningApp::with_environment(&placeholder, system)?;
            running.background = background;
            running.terminate_on_pause = terminate_on_pause;
            if runs_commands(h, app) {
                tracing::info!(app = %app.name, %error, "no signed-in user; the application starts after sign-in");
                running.deferred = Some((app.clone(), args.clone()));
            }
            return Ok(running);
        }
    };
    let mut app = app.clone();
    let document = h.app_document.read().unwrap();
    if let Some(values) = document.get("env").and_then(serde_json::Value::as_object) {
        for (key, value) in values {
            environment.insert(
                key.to_uppercase(),
                expand(
                    value.as_str().ok_or_else(|| {
                        anyhow::anyhow!("application environment must contain strings")
                    })?,
                    &environment,
                )?,
            );
        }
    }
    drop(document);
    if background {
        for (key, value) in butterpollo_core::background::ENVIRONMENT {
            environment.insert(key.into(), value.into());
        }
    }
    if !app_bool(&app, "exclude-global-state-cmd", false) {
        let mut global: Vec<PrepCommand> =
            serde_json::from_str(h.config.read().unwrap().get("global_state_cmd", "[]"))?;
        let mut local: Vec<PrepCommand> = serde_json::from_value(
            app.extra
                .get("state-cmd")
                .cloned()
                .unwrap_or_else(|| serde_json::json!([])),
        )?;
        global.append(&mut local);
        app.extra
            .insert("state-cmd".into(), serde_json::to_value(global)?);
    }
    let mode: Vec<_> = args
        .get("mode")
        .map_or("1920x1080x60", String::as_str)
        .split('x')
        .collect();
    let (render_width, render_height) = butterpollo_core::display_policy::render_dimensions(
        mode.first()
            .and_then(|value| value.parse().ok())
            .unwrap_or(1920),
        mode.get(1)
            .and_then(|value| value.parse().ok())
            .unwrap_or(1080),
        i64::from(scale_factor),
        app.extra
            .get("scale-factor")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(100),
    );
    for (name, value) in [
        ("SUNSHINE_APP_ID", app.id().to_string()),
        ("SUNSHINE_APP_NAME", app.name.clone()),
        ("SUNSHINE_CLIENT_WIDTH", render_width.to_string()),
        ("SUNSHINE_CLIENT_HEIGHT", render_height.to_string()),
        (
            "SUNSHINE_CLIENT_FPS",
            mode.get(2).unwrap_or(&"60").to_string(),
        ),
        (
            "SUNSHINE_CLIENT_HDR",
            if args.get("hdrMode").is_some_and(|v| v == "1") {
                "true"
            } else {
                "false"
            }
            .into(),
        ),
        ("SUNSHINE_CLIENT_GCMAP", gcmap.to_string()),
        (
            "SUNSHINE_CLIENT_HOST_AUDIO",
            if args.get("localAudioPlayMode").is_some_and(|v| v == "1") {
                "true"
            } else {
                "false"
            }
            .into(),
        ),
        (
            "SUNSHINE_CLIENT_ENABLE_SOPS",
            if args.get("sops").is_some_and(|v| v == "1") {
                "true"
            } else {
                "false"
            }
            .into(),
        ),
    ] {
        environment.insert(name.into(), value);
    }
    if let Some(rate) = mode.get(2) {
        let rate = if rate.contains('.') {
            butterpollo_core::framegen::Rate::parse(rate)?
        } else {
            butterpollo_core::framegen::Rate::from_client(rate.parse()?)
        };
        environment.insert(
            "SUNSHINE_CLIENT_FPS".into(),
            if h.config
                .read()
                .unwrap()
                .boolean("envvar_compatibility_mode", false)
            {
                rate.rounded().to_string()
            } else {
                rate.to_string()
            },
        );
    }
    for (key, value) in args {
        let name = match key.as_str() {
            "clientName" => Some("NAME"),
            "clientUuid" => Some("UUID"),
            "surroundParams" => Some("AUDIO_SURROUND_PARAMS"),
            "surroundAudioInfo" => Some("AUDIO_CONFIGURATION"),
            _ => None,
        };
        if let Some(name) = name {
            let value = if key == "clientName" {
                butterpollo_core::pairing::command_device_name(value)
            } else {
                value.clone()
            };
            environment.insert(format!("SUNSHINE_CLIENT_{name}"), value);
        }
    }
    for (key, value) in environment.clone() {
        if let Some(suffix) = key.strip_prefix("SUNSHINE_") {
            environment.insert(format!("APOLLO_{suffix}"), value);
        }
    }
    // Apollo/Vibepollo additions: the app's UUID, the client's own mode
    // before render scaling, and its scale factor.
    for (name, value) in [
        (
            "APOLLO_APP_UUID",
            app.extra
                .get("uuid")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        ),
        (
            "APOLLO_CLIENT_RENDER_WIDTH",
            mode.first()
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(1920)
                .to_string(),
        ),
        (
            "APOLLO_CLIENT_RENDER_HEIGHT",
            mode.get(1)
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(1080)
                .to_string(),
        ),
        ("APOLLO_CLIENT_SCALE_FACTOR", scale_factor.to_string()),
        ("APOLLO_APP_STATUS", "STARTING".to_owned()),
    ] {
        environment.insert(name.into(), value);
    }
    if !app_bool(&app, "exclude-global-prep-cmd", false) {
        let mut global: Vec<PrepCommand> =
            serde_json::from_str(h.config.read().unwrap().get("global_prep_cmd", "[]"))?;
        global.append(&mut app.prep);
        app.prep = global;
    }
    app.cmd = expand(&app.cmd, &environment)?;
    app.working_dir = expand(&app.working_dir, &environment)?;
    for prep in &mut app.prep {
        prep.r#do = expand(&prep.r#do, &environment)?;
        prep.undo = expand(&prep.undo, &environment)?;
    }
    if let Some(commands) = app
        .extra
        .get_mut("detached")
        .and_then(serde_json::Value::as_array_mut)
    {
        for command in commands {
            *command = serde_json::Value::String(expand(
                command
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("detached command must be a string"))?,
                &environment,
            )?);
        }
    }
    // An app linked to a Playnite game has no command: Playnite starts it.
    let playnite_id = app
        .extra
        .get("playnite-id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty() && app.cmd.trim().is_empty())
        .map(str::to_owned);
    let fullscreen = app.cmd.trim().is_empty() && playnite_fullscreen(&app);
    let config = h.config.read().unwrap().clone();
    // Lossless Scaling scales the focused window; it would pull the game in
    // front of the person using the PC.
    let lossless = if fullscreen || background {
        None
    } else {
        let fps = mode
            .get(2)
            .and_then(|rate| rate.parse::<f64>().ok())
            .map_or(60., |rate| if rate >= 1000. { rate / 1000. } else { rate });
        butterpollo_core::lossless::options(&serde_json::to_value(&app)?, &config, fps)
    };
    // Lossless Scaling looks for the game among the processes that start
    // after this.
    let baseline = lossless
        .as_ref()
        .map(|_| butterpollo_windows::process::processes().unwrap_or_default());
    let mut running = RunningApp::with_environment(&app, environment)?;
    running.background = background;
    running.terminate_on_pause = terminate_on_pause;
    if fullscreen || (playnite_id.is_some() && config.boolean("playnite_enabled", true)) {
        running.playnite = Some(crate::playnite::Launch::start(
            h,
            if fullscreen {
                None
            } else {
                playnite_id.as_deref()
            },
            &running.environment,
            background,
        )?);
    }
    running.playnite_fullscreen = fullscreen;
    if let (Some(options), Some(baseline)) = (lossless, baseline) {
        let folder: Box<dyn Fn() -> Option<String> + Send> = match &running.playnite {
            Some(launch) => Box::new(launch.folder()),
            None => {
                let steam = app
                    .extra
                    .get("steam-install-dir")
                    .and_then(serde_json::Value::as_str)
                    .filter(|folder| !folder.trim().is_empty())
                    .map(str::to_owned)
                    .or_else(|| (!running.working.is_empty()).then(|| running.working.clone()));
                Box::new(move || steam.clone())
            }
        };
        running.lossless = Some(crate::lossless::Session::start(
            options,
            config.get("lossless_scaling_path", "").to_owned(),
            baseline,
            running.child.as_ref().map_or(0, |child| child.pid),
            folder,
        ));
    }
    Ok(running)
}
fn playnite_fullscreen(app: &App) -> bool {
    match app.extra.get("playnite-fullscreen") {
        Some(serde_json::Value::Number(n)) => n.as_i64() == Some(1),
        _ => app_bool(app, "playnite-fullscreen", false),
    }
}

impl Drop for RunningApp {
    fn drop(&mut self) {
        self.stop();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn early_exit_notification_requires_auto_detach_and_a_nonzero_exit_before_five_seconds() {
        let early = Duration::from_millis(4999);
        assert_eq!(
            exit_action(true, early, Some(1), false),
            ExitAction::Detached(Some(1))
        );
        assert_eq!(
            exit_action(true, early, Some(0xc0000005), false),
            ExitAction::Detached(Some(0xc0000005))
        );
        assert_eq!(
            exit_action(true, early, Some(0), false),
            ExitAction::Detached(None)
        );
        assert_eq!(
            exit_action(false, early, Some(1), false),
            ExitAction::Finished
        );
        assert_eq!(
            exit_action(true, Duration::from_secs(5), Some(1), false),
            ExitAction::Finished
        );
        assert_eq!(
            exit_action(true, Duration::from_secs(6), Some(1), false),
            ExitAction::Finished
        );
        assert_eq!(exit_action(true, early, None, false), ExitAction::Running);
        assert_eq!(exit_action(true, early, Some(1), true), ExitAction::Running);
        assert_eq!(exit_action(true, early, Some(0), true), ExitAction::Running);
        assert_eq!(
            exit_action(true, early, Some(9009), false),
            ExitAction::Finished
        );
    }

    #[test]
    fn a_missing_executable_fails_instead_of_detaching_a_desktop_stream() {
        let mut app = App::desktop();
        app.cmd = format!(
            "\"{}\"",
            std::env::temp_dir()
                .join(format!("missing-{}.exe", uuid::Uuid::new_v4()))
                .display()
        );
        let result = RunningApp::with_environment(&app, BTreeMap::new());
        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("application executable not found")
        );
    }

    #[test]
    fn a_slow_steam_launch_keeps_its_process_tracker() {
        let mut app = RunningApp::with_environment(&App::desktop(), BTreeMap::new()).unwrap();
        app.steam = Some(butterpollo_core::steam::Tracker::new(
            &[],
            "C:\\not-a-game",
            Duration::ZERO,
        ));
        app.steam_check = std::time::Instant::now() - Duration::from_secs(2);
        assert!(!app.exited().unwrap());
        assert!(app.steam.is_some());
    }
    #[test]
    fn quitting_closes_a_store_launched_game_but_not_the_store_or_older_programs() {
        let launched = 1_000;
        let game = r"C:\XboxGames\Forza Horizon 5\Content\ForzaHorizon5.exe";
        assert!(foreign_game(game, launched + 1, launched));
        // A program that was already running before the launch stays open.
        assert!(!foreign_game(game, launched - 1, launched));
        // So does a process whose creation time cannot be read.
        assert!(!foreign_game(game, 0, launched));
        for client in [
            r"C:\Program Files (x86)\Steam\steam.exe",
            r"C:\Program Files (x86)\Epic Games\Launcher\Portal\Binaries\Win64\EpicGamesLauncher.exe",
            r"C:\Program Files\Butterpollo\butterpollo.exe",
        ] {
            assert!(!foreign_game(client, launched + 1, launched), "{client}");
        }
    }
    #[test]
    fn existing_environment_syntax_is_case_insensitive_and_checked() {
        let env = BTreeMap::from([("PATH".into(), "C:\\tools".into())]);
        assert_eq!(
            expand("$(Path);$$;$(MISSING);$literal$", &env).unwrap(),
            "C:\\tools;$;;$literal$"
        );
        assert!(expand("$(Path", &env).is_err());
    }
    #[test]
    fn a_program_the_desktop_started_is_the_users_not_the_apps() {
        use butterpollo_core::steam::Process;
        let process = |pid, parent, started, name: &str| Process {
            pid,
            parent,
            started,
            name: name.into(),
        };
        let processes = [
            process(10, 1, 100, "explorer.exe"),
            process(20, 10, 500, "player.exe"),
            process(30, 40, 500, "game.exe"),
            process(40, 1, 200, "EpicGamesLauncher.exe"),
            // Parent id 10 reused after the child started: not its parent.
            process(50, 60, 300, "old.exe"),
            process(60, 1, 900, "explorer.exe"),
        ];
        assert!(started_from_desktop(20, &processes));
        assert!(!started_from_desktop(30, &processes));
        assert!(!started_from_desktop(50, &processes));
        assert!(!started_from_desktop(99, &processes));
    }
    #[test]
    fn device_commands_in_the_formats_apollo_writes_are_accepted() {
        let parse = |value: serde_json::Value| {
            client_commands(Some(&value), "do").map(|commands| {
                commands
                    .into_iter()
                    .map(|c| (c.cmd, c.elevated))
                    .collect::<Vec<_>>()
            })
        };
        assert!(parse(serde_json::json!("")).unwrap().is_empty());
        assert!(client_commands(None, "do").unwrap().is_empty());
        assert_eq!(
            parse(serde_json::json!([
                {"cmd": "a", "elevated": "false"},
                {"cmd": "b", "elevated": "true"},
                {"cmd": "c", "elevated": 1},
                {"cmd": "d"},
                {"cmd": "e", "elevated": true}
            ]))
            .unwrap(),
            [
                ("a".to_owned(), false),
                ("b".to_owned(), true),
                ("c".to_owned(), true),
                ("d".to_owned(), false),
                ("e".to_owned(), true)
            ]
        );
        assert!(parse(serde_json::json!([{"elevated": true}])).is_err());
        assert!(parse(serde_json::json!({"cmd": "x"})).is_err());
    }
    #[test]
    fn client_hooks_use_the_saved_environment_on_disconnect() {
        let elevated = butterpollo_windows::process::user_environment().is_err();
        let path = std::env::temp_dir().join(format!(
            "butterpollo-client-hooks-{}.txt",
            uuid::Uuid::new_v4()
        ));
        let environment = BTreeMap::from([("MARKER".into(), path.to_string_lossy().into_owned())]);
        let extra = BTreeMap::from([
            (
                "do".into(),
                serde_json::json!([{"cmd":"echo connected>\"$(MARKER)\"","elevated":elevated}]),
            ),
            (
                "undo".into(),
                serde_json::json!([{"cmd":"echo disconnected>>\"$(MARKER)\"","elevated":elevated}]),
            ),
        ]);
        let wait = |needle: &str| {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while !std::fs::read_to_string(&path)
                .unwrap_or_default()
                .contains(needle)
            {
                assert!(
                    std::time::Instant::now() < deadline,
                    "client hook did not write {needle}"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        let hooks = ClientCommands::with_environment(&extra, environment.clone()).unwrap();
        wait("connected");
        assert!(
            !std::fs::read_to_string(&path)
                .unwrap()
                .contains("disconnected")
        );
        drop(hooks);
        wait("disconnected");
        assert_eq!(
            std::fs::read_to_string(&path)
                .unwrap()
                .lines()
                .collect::<Vec<_>>(),
            ["connected", "disconnected"]
        );
        std::fs::remove_file(&path).unwrap();
        let invalid = BTreeMap::from([
            ("do".into(), extra["do"].clone()),
            ("undo".into(), serde_json::json!([{"cmd":null}])),
        ]);
        assert!(ClientCommands::with_environment(&invalid, environment).is_err());
        assert!(!path.exists());
    }
}
