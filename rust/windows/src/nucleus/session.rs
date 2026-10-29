//! A Nucleus session on the PC: Nucleus Co-op started for one game with the
//! host's profile, each player's instance found and parked beside the
//! desktop, each player's pad and audio, and all of it stopped again. A
//! failure ends the session with the reason the stream and the console
//! show; nothing falls back to another mode.
use super::desktop::Win32;
use super::files;
use super::instances::{Desktop, Keeper, MessageBox};
use super::layers::{self, Guard, Layers};
use super::pads::{self, Dropped, Pads, XInput};
use crate::input::PadReport;
use anyhow::{Context, Result, bail, ensure};
use butterpollo_core::audio_scope::Identity;
use butterpollo_core::input::Input;
use butterpollo_core::nucleus::coop::{self, End, Phase};
use butterpollo_core::nucleus::handler::Handler;
use butterpollo_core::nucleus::install::EXECUTABLE;
use butterpollo_core::nucleus::profile::{Player, TITLE};
use butterpollo_core::nucleus::window;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How long Nucleus may take to show the instances, counted again from
/// each step it makes (the profile clicked, an instance, a window).
pub const START_TIMEOUT: Duration = Duration::from_secs(60);
/// How often the session looks at the desktop.
const PERIOD: Duration = Duration::from_millis(250);
/// Passes in a row that may fail (the desktop switching for a UAC prompt)
/// before the session ends with the error.
const FAILED_PASSES: u32 = 8;
/// How long Nucleus and the instances get to close before they are ended.
const STOP_TIMEOUT: Duration = Duration::from_secs(10);
/// Process loopback capture of one process tree came with this build.
pub const PROCESS_AUDIO_BUILD: u32 = 20348;

/// Nucleus Co-op as a session runs it: the elevated process on the PC, a
/// fake in tests.
pub trait Nucleus: Send {
    /// When it started (FILETIME): an older process is not its instance.
    fn since(&self) -> u64;
    /// Its exit code, once it exited.
    fn exit_code(&mut self) -> Result<Option<u32>>;
    /// The processes in its job: Nucleus and what it started.
    fn job(&mut self) -> Result<BTreeSet<u32>>;
    /// Close Nucleus, then the `instances` (pid → start time) it started;
    /// what has not closed in time is ended.
    fn stop(&mut self, instances: &BTreeMap<u32, u64>);
}

/// Starts Nucleus Co-op: elevated on the PC, a fake in tests.
pub trait Launcher {
    /// Start it for game `guid` with `environment` added to the user's: the
    /// game it starts gets the same.
    fn launch(
        &mut self,
        folder: &Path,
        guid: &str,
        environment: &BTreeMap<String, String>,
    ) -> Result<Box<dyn Nucleus>>;
}

/// What Nucleus Co-op starts with for a game found from Steam app
/// `steam_app`. A Steam game started by its .exe restarts through Steam
/// unless Steam started it; Steam's own launch sets these, and Nucleus
/// starts a handler's game with its own environment unless the handler sets
/// `UseNucleusEnvironment`.
pub fn steam_environment(steam_app: Option<u32>) -> BTreeMap<String, String> {
    steam_app
        .into_iter()
        .flat_map(|app| ["SteamAppId", "SteamGameId"].map(|key| (key.to_string(), app.to_string())))
        .collect()
}

/// Nucleus Co-op as the signed-in user, elevated: it makes symbolic links.
pub struct Elevated;
impl Launcher for Elevated {
    fn launch(
        &mut self,
        folder: &Path,
        guid: &str,
        extra: &BTreeMap<String, String>,
    ) -> Result<Box<dyn Nucleus>> {
        require_administrator(crate::process::user_runs_elevated()?)?;
        let environment = environment(extra)?;
        let since = crate::process::now();
        let program = folder.join(EXECUTABLE);
        // Nucleus reads Settings.ini from its working folder, and its window
        // must be visible for the profile click.
        let process = crate::process::Process::spawn(
            &program,
            &[guid.into()],
            Some(folder),
            crate::process::Target::User { elevated: true },
            &environment,
            false,
        )
        .with_context(|| format!("starting {} elevated", program.display()))?;
        tracing::info!(
            pid = process.pid,
            guid,
            ?environment,
            "Nucleus Co-op started"
        );
        Ok(Box::new(Started { process, since }))
    }
}

/// Open Nucleus Co-op's own window on the PC's screen, as the signed-in user
/// and elevated as a session starts it, for the user to add a handler or a
/// game. It runs outside the host's job, so the host ending does not close
/// it; the [`Opened`] it returns closes it before a session starts.
pub fn open(folder: &Path) -> Result<Opened> {
    require_administrator(crate::process::user_runs_elevated()?)?;
    let program = folder.join(EXECUTABLE);
    let line = crate::process::quote(program.as_os_str())?;
    let process = crate::process::Process::spawn_command(
        &program,
        &line,
        Some(folder),
        crate::process::Target::User { elevated: true },
        &environment(&BTreeMap::new())?,
        false,
        true,
    )
    .with_context(|| format!("starting {} elevated", program.display()))?;
    tracing::info!(pid = process.pid, folder = %folder.display(), "Nucleus Co-op opened for the user");
    Ok(Opened(process))
}

/// WebView2 starts its browser without administrator rights even for an
/// elevated Nucleus Co-op, so that browser could not write the hub view's
/// data folder (`webview\cache` in Nucleus' Administrators-only folder) and
/// Nucleus stayed on its loading screen with "Microsoft Edge can't read and
/// write to its data directory" (#59). `WEBVIEW2_USER_DATA_FOLDER` replaces
/// the folder Nucleus passes: one in the signed-in user's own AppData\Local.
/// `extra` (a Steam game's app id) is added to it.
fn environment(extra: &BTreeMap<String, String>) -> Result<BTreeMap<String, String>> {
    let local = crate::child_session::console_local_app_data()?
        .context("Nucleus Co-op needs a Windows account signed in at this PC's screen")?;
    Ok(webview_environment(&local, extra))
}
fn webview_environment(
    local_app_data: &Path,
    extra: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let folder = local_app_data.join("Butterpollo").join("nucleus-webview");
    let mut environment = BTreeMap::from([(
        "WEBVIEW2_USER_DATA_FOLDER".to_string(),
        folder.to_string_lossy().into_owned(),
    )]);
    environment.extend(extra.clone());
    environment
}

/// Without an elevated token for the signed-in user (a standard account, or
/// Windows' Administrator protection, which elevates only into a separate
/// account after a Windows Hello prompt) Nucleus Co-op would run with
/// standard rights: it can't write to
/// its folder, which only administrators can change, and stops on its
/// loading screen ("Microsoft Edge can't read and write to its data
/// directory").
fn require_administrator(elevated: bool) -> Result<()> {
    ensure!(
        elevated,
        "Nucleus Co-op needs administrator rights, and Windows gives the account signed in on this PC none to start it with (a standard account, or Windows' Administrator protection)"
    );
    Ok(())
}

/// The Nucleus Co-op [`open`] started: the only one the host closes. It is
/// every NucleusCoop.exe in the job [`open`] made, so one that restarted
/// itself (elevating, updating) is still this one; the job's members keep
/// their ids from naming other processes.
pub struct Opened(crate::process::Process);
impl Opened {
    /// The process [`open`] started.
    pub fn pid(&self) -> u32 {
        self.0.pid
    }
    /// Whether a NucleusCoop.exe of it still runs. What it started (a
    /// browser from a hub link, a game) is the user's, and doesn't count.
    pub fn running(&self) -> Result<bool> {
        self.0.running_image(EXECUTABLE)
    }
    /// Ask its NucleusCoop.exe windows to close and wait up to `timeout`.
    /// Nothing is ended, and what it started is left alone: one still open (a
    /// question it asks the user) is an error.
    pub fn close(&self, timeout: Duration) -> Result<()> {
        ensure!(
            self.0.close_image(EXECUTABLE, timeout)?,
            "Nucleus Co-op (process {}) did not close within {} s: close it on the PC",
            self.0.pid,
            timeout.as_secs()
        );
        tracing::info!(pid = self.0.pid, "Nucleus Co-op closed");
        Ok(())
    }
}

/// A Nucleus Co-op process running on the PC, whoever started it.
pub fn running() -> Result<Option<u32>> {
    Ok(crate::process::processes()?
        .into_iter()
        .find(|p| p.name.eq_ignore_ascii_case(EXECUTABLE))
        .map(|p| p.pid))
}

/// Why a session does not start while Nucleus Co-op process `pid` runs.
pub fn already_running(pid: u32) -> String {
    format!(
        "Nucleus Co-op is already running on the PC (process {pid}): close it, then start the stream again"
    )
}

/// Nucleus started by [`Elevated`]. Its job ends what it started when this
/// is dropped.
struct Started {
    process: crate::process::Process,
    since: u64,
}
impl Nucleus for Started {
    fn since(&self) -> u64 {
        self.since
    }
    fn exit_code(&mut self) -> Result<Option<u32>> {
        self.process.exit_code()
    }
    fn job(&mut self) -> Result<BTreeSet<u32>> {
        Ok(self.process.process_ids()?.into_iter().collect())
    }
    fn stop(&mut self, instances: &BTreeMap<u32, u64>) {
        if let Err(error) = self.process.stop_graceful(STOP_TIMEOUT) {
            tracing::warn!(error = format!("{error:#}"), "Nucleus Co-op did not stop");
        }
        // Instances Nucleus started outside its job.
        crate::process::stop_processes(instances, STOP_TIMEOUT);
    }
}

/// Nucleus' `crashlog.txt` in its working folder, where it appends each
/// exception it catches (`[time] exception`, then its stack) and goes on,
/// as it was when the session launched it.
struct Crashlog {
    path: PathBuf,
    /// Its length at the launch: what follows is this session's.
    from: u64,
}

impl Crashlog {
    /// The longest line quoted.
    const LIMIT: usize = 300;

    fn at_launch(folder: &Path) -> Crashlog {
        let path = folder.join("crashlog.txt");
        let from = match std::fs::metadata(&path) {
            Ok(metadata) => metadata.len(),
            Err(error) => {
                if error.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(error = %error, path = %path.display(), "Nucleus Co-op's crashlog not read");
                }
                0
            }
        };
        Crashlog { path, from }
    }

    /// The newest exception written since the launch: its first line.
    fn newest(&self) -> Option<String> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) => {
                if error.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(error = %error, path = %self.path.display(), "Nucleus Co-op's crashlog not read");
                }
                return None;
            }
        };
        // A shorter file was started again since.
        let from = usize::try_from(self.from)
            .ok()
            .filter(|&from| from <= bytes.len())
            .unwrap_or(0);
        let text = String::from_utf8_lossy(&bytes[from..]);
        let lines: Vec<&str> = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        let line = lines
            .iter()
            .rev()
            .find(|line| line.starts_with('['))
            .or(lines.last())?;
        Some(match line.char_indices().nth(Self::LIMIT) {
            Some((at, _)) => format!("{}...", &line[..at]),
            None => line.to_string(),
        })
    }
}

/// What the session thread and the host share.
struct Shared {
    session: Mutex<coop::Session>,
    /// Each slot's stream size: the first player's until the slot's own.
    sizes: Mutex<Vec<(u32, u32)>>,
    /// Each slot's instance root, for its audio.
    roots: Mutex<Vec<Option<Identity>>>,
}

/// The session thread's state: one [`step`](Runner::step) per period.
struct Runner {
    shared: Arc<Shared>,
    desktop: Box<dyn Desktop>,
    nucleus: Box<dyn Nucleus>,
    keeper: Keeper,
    title: String,
    players: usize,
    clicked: bool,
    /// Why the last click on the profile was refused.
    unclicked: Option<String>,
    /// While the profile is not shown: Nucleus' message boxes at the last
    /// look, every one logged so far, whether it showed any window, and why
    /// the last look failed.
    showing: Vec<MessageBox>,
    logged: Vec<MessageBox>,
    shown: bool,
    unseen: Option<String>,
    /// Nucleus' crashlog.txt as it was at the launch. None for a runner
    /// tested alone.
    crashlog: Option<Crashlog>,
    /// Nucleus Co-op's folder and the game's GUID, to tell whether Nucleus
    /// lists the game. None for a runner tested alone.
    game: Option<(PathBuf, String)>,
    /// Clicked, instances found, windows found.
    progress: (bool, usize, usize),
    windows: Vec<Option<u64>>,
    deadline: Instant,
    exited: bool,
    failed: u32,
    /// The user's Nucleus settings the session changed, then the game's
    /// compatibility settings from before Nucleus started; last, so they
    /// drop after Nucleus. None for a runner tested alone.
    settings: Option<files::SettingsGuard>,
    layers: Option<Guard>,
}

impl Runner {
    /// One look at the desktop. Some when the session ends, with why.
    fn step(&mut self, now: Instant) -> Option<End> {
        {
            let mut session = self.shared.session.lock().unwrap();
            for event in session.tick(now) {
                if let coop::Event::Freed { slot, device } = event {
                    tracing::info!(slot, device, "Nucleus Co-op player did not return");
                }
            }
            if let Phase::Ended(why) = &session.phase {
                return Some(why.clone());
            }
        }
        let job = match self.nucleus.job() {
            Ok(job) => job,
            Err(error) => {
                return self.failed_pass(error.context("listing Nucleus Co-op's processes"));
            }
        };
        // Once an instance runs, Nucleus has loaded a profile: a second
        // click could start the game again.
        if !self.clicked
            && self.progress.1 == 0
            && let Some(end) = self.click(&job)
        {
            return Some(end);
        }
        let sizes = self.shared.sizes.lock().unwrap().clone();
        let pass = match self.keeper.pass(self.desktop.as_mut(), &job, &sizes) {
            Ok(pass) => pass,
            Err(error) => return self.failed_pass(error),
        };
        if let Some(&slot) = pass.gone.first() {
            let why = format!("Player {}'s instance of {} exited", slot + 1, self.title);
            return Some(self.failure(why));
        }
        *self.shared.roots.lock().unwrap() = pass.roots.clone();
        let roots = pass.roots.iter().flatten().count();
        let windows = pass.windows.iter().flatten().count();
        let progress = (self.clicked, roots, windows);
        if progress != self.progress {
            self.progress = progress;
            self.deadline = now + START_TIMEOUT;
        }
        self.windows = pass.windows.clone();
        let running = {
            let mut session = self.shared.session.lock().unwrap();
            let running = session.phase == Phase::Running;
            // While it runs, a window that closed is gone from its slot at once.
            if running || windows == self.players {
                if !running {
                    tracing::info!(title = self.title, "Nucleus Co-op session running");
                }
                session.running(&pass.windows);
            }
            session.phase == Phase::Running
        };
        if !self.exited {
            let code = match self.nucleus.exit_code() {
                Ok(code) => code,
                Err(error) => {
                    return self.failed_pass(error.context("reading Nucleus Co-op's exit code"));
                }
            };
            if let Some(code) = code {
                self.exited = true;
                if !running && roots == 0 {
                    return Some(End::Failed(format!(
                        "Nucleus Co-op exited with code {code} before starting {}",
                        self.title
                    )));
                }
                tracing::warn!(
                    code,
                    "Nucleus Co-op exited; the session goes on with the instances it started"
                );
            }
        }
        self.failed = 0;
        if !running && roots == 0 && self.clicked {
            // A box Nucleus shows after the click (its End-key reminder, a
            // check of the game) holds the start as one before it does.
            self.look(&job);
        }
        if !running && now >= self.deadline {
            return Some(End::Failed(self.timeout()));
        }
        None
    }

    /// A pass that could not look at the desktop: the session ends after
    /// [`FAILED_PASSES`] in a row.
    fn failed_pass(&mut self, error: anyhow::Error) -> Option<End> {
        self.failed += 1;
        let why = format!("{error:#}");
        tracing::warn!(
            error = why,
            failed = self.failed,
            "Nucleus Co-op pass failed"
        );
        (self.failed >= FAILED_PASSES).then(|| self.failure(why))
    }

    /// Load the host's profile: Nucleus starts the game by itself then.
    fn click(&mut self, job: &BTreeSet<u32>) -> Option<End> {
        let label = match self.desktop.label(job, TITLE) {
            Ok(Some(label)) => label,
            Ok(None) => {
                self.look(job);
                return None;
            }
            Err(error) => return Some(End::Failed(format!("{error:#}"))),
        };
        match self.desktop.click(label) {
            Ok(()) => {
                tracing::info!("Nucleus Co-op profile {TITLE} clicked");
                self.clicked = true;
                self.unclicked = None;
            }
            Err(error) => {
                let why = format!("{error:#}");
                if self.unclicked.as_ref() != Some(&why) {
                    tracing::warn!(why, "Nucleus Co-op profile {TITLE} not clicked");
                }
                self.unclicked = Some(why);
                // What covers the list may be Nucleus' own message box.
                self.look(job);
            }
        }
        None
    }

    /// What Nucleus shows instead of the profile: a message box it shows
    /// before its window (its folder check, a missing runtime) waits for an
    /// answer no one gives during a background stream (#59).
    fn look(&mut self, job: &BTreeSet<u32>) {
        let looked = match self.desktop.message_boxes(job) {
            Ok(boxes) => {
                self.showing = boxes;
                self.desktop
                    .windows()
                    .map(|windows| windows.iter().any(|w| w.visible && job.contains(&w.pid)))
            }
            Err(error) => {
                self.showing.clear();
                Err(error)
            }
        };
        match looked {
            Ok(shown) => {
                self.shown |= shown;
                self.unseen = None;
            }
            Err(error) => {
                let why = format!("{error:#}");
                if self.unseen.as_ref() != Some(&why) {
                    tracing::warn!(why, "Nucleus Co-op's windows not listed");
                }
                self.unseen = Some(why);
            }
        }
        for shown in &self.showing {
            if !self.logged.contains(shown) {
                tracing::warn!(
                    "Nucleus Co-op shows \"{}\" on the PC's screen",
                    shown.summary()
                );
                self.logged.push(shown.clone());
            }
        }
    }

    /// Why Nucleus did not show the profile.
    fn unshown(&self) -> String {
        if let Some(shown) = self.showing.first() {
            return format!(
                "Nucleus Co-op shows \"{}\" on the PC's screen",
                shown.summary()
            );
        }
        let why = if self.shown {
            self.unlisted().unwrap_or_else(|| {
                format!(
                    "Nucleus Co-op shows no profile {TITLE} for {}; open Nucleus Co-op and load the profile {TITLE} once",
                    self.title
                )
            })
        } else if let Some(unseen) = &self.unseen {
            format!("Nucleus Co-op's windows could not be listed: {unseen}")
        } else {
            "Nucleus Co-op showed no window".to_string()
        };
        match self.crashlog.as_ref().and_then(Crashlog::newest) {
            Some(line) => format!("{why}. Its crashlog.txt says: {line}"),
            None => why,
        }
    }

    /// Why Nucleus Co-op shows no entry for the game, when it doesn't: no
    /// profile of a game it doesn't list ever appears (#59).
    fn unlisted(&self) -> Option<String> {
        let (folder, guid) = self.game.as_ref()?;
        let list = files::list(folder);
        match files::listed_by_nucleus(folder) {
            Ok(listed) if listed.contains(guid) => None,
            Ok(_) => Some(format!(
                "Nucleus Co-op does not list {}: {} has no entry for it with its handler and an .exe that is there",
                self.title,
                list.display()
            )),
            Err(error) => Some(format!(
                "Nucleus Co-op's game list can't be read: {error:#}"
            )),
        }
    }

    fn failure(&self, why: String) -> End {
        if self.shared.session.lock().unwrap().phase == Phase::Running {
            End::Crashed(why)
        } else {
            End::Failed(why)
        }
    }

    fn timeout(&self) -> String {
        let (clicked, roots, _) = self.progress;
        let hint = format!("open Nucleus Co-op and load the profile {TITLE} once");
        match (&self.unclicked, clicked, roots) {
            // The box is why: it covers the list or holds the start.
            (_, _, 0) if !self.showing.is_empty() => self.unshown(),
            (Some(why), false, 0) => format!("Nucleus Co-op did not start {}: {why}", self.title),
            (None, false, 0) => self.unshown(),
            (_, _, 0) => format!("Nucleus Co-op did not start {}; {hint}", self.title),
            _ if roots < self.players => format!(
                "Nucleus Co-op started {roots} of {} instances of {}; {hint}",
                self.players, self.title
            ),
            _ => {
                let slot = self.windows.iter().position(Option::is_none).unwrap_or(0);
                format!(
                    "Player {}'s instance of {} showed no window",
                    slot + 1,
                    self.title
                )
            }
        }
    }

    /// End the session with `end` (unless it ended already), and stop
    /// Nucleus and the instances.
    fn finish(mut self, end: End) {
        let phase = {
            let mut session = self.shared.session.lock().unwrap();
            session.end(end);
            session.phase.clone()
        };
        match phase {
            Phase::Ended(End::Ended) => {
                tracing::info!(title = self.title, "Nucleus Co-op session ended")
            }
            Phase::Ended(why) => {
                tracing::warn!(title = self.title, "Nucleus Co-op session ended: {why}")
            }
            _ => {}
        }
        let instances = self.keeper.processes();
        self.nucleus.stop(&instances);
        // Nucleus sets them as it starts each instance: back once all stopped.
        if let Some(layers) = &mut self.layers {
            layers.restore();
        }
        if let Some(settings) = &mut self.settings {
            settings.restore();
        }
    }
}

fn run(mut runner: Runner, stop: mpsc::Receiver<()>) {
    let end = loop {
        if let Some(end) = runner.step(Instant::now()) {
            break end;
        }
        match stop.recv_timeout(PERIOD) {
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break End::Ended,
        }
    };
    runner.finish(end);
}

/// Why per-player audio cannot be captured on this Windows build. An
/// unknown build is taken as new enough, as `os_gate` does.
pub fn audio_limit(build: Option<u32>) -> Option<String> {
    let build = build?;
    (build < PROCESS_AUDIO_BUILD).then(|| {
        format!(
            "Per-player audio needs Windows build {PROCESS_AUDIO_BUILD} or newer and this PC runs build {build}: update Windows to hear each player's own game"
        )
    })
}

/// What starts a session.
pub struct Start<'a> {
    pub id: u32,
    pub kind: coop::Kind,
    /// Nucleus Co-op's folder.
    pub folder: &'a Path,
    pub handler: &'a Handler,
    /// The game's .exe as Nucleus starts it: the user's added game's, or the
    /// one Rubylight's handler is for.
    pub exe_path: &'a str,
    /// The Steam app the game was found from, for Rubylight's own handler
    /// ([`environment`]).
    pub steam_app: Option<u32>,
    /// The game as the stream names it.
    pub title: &'a str,
    pub players: usize,
    /// The first player's device id and name.
    pub owner: (&'a str, &'a str),
    /// The first player's stream size.
    pub size: (u32, u32),
}

/// A session that runs until [`stop`](Running::stop) or drop. Its calls
/// never wait for a stop, which can take seconds.
pub struct Running {
    shared: Arc<Shared>,
    pads: Box<dyn Pads>,
    stop: Mutex<Option<mpsc::Sender<()>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
    players: usize,
}

/// Start a session on the PC: virtual Xbox pads, XInput, the user's desktop
/// and compatibility settings, and Nucleus Co-op elevated.
pub fn start_on_pc(
    start: &Start,
    policy: butterpollo_core::input_policy::Policy,
    warnings: Arc<butterpollo_core::session::Warnings>,
) -> Result<Running> {
    let pads = pads::virtual_pads(policy, warnings)?;
    let mut xinput = pads::System::open()?;
    self::start(
        start,
        Box::new(pads),
        &mut xinput,
        Box::new(Win32::default()),
        Box::new(layers::UserHive::signed_in()?),
        &mut Elevated,
    )
}

/// What starting a session needs that doesn't need Nucleus Co-op closed:
/// administrator rights for the signed-in user, their AppData folder and
/// their account, and the compatibility settings an earlier session at
/// `folder` left put back. Checked before a session closes the console's
/// window, so a launch refused for one of these leaves the window open.
pub fn ready(folder: &Path) -> Result<()> {
    require_administrator(crate::process::user_runs_elevated()?)?;
    environment(&BTreeMap::new())?;
    layers::restore_left_in(&mut layers::UserHive::signed_in()?, folder)?;
    Ok(())
}

/// Start a session: refused while Nucleus Co-op already runs, then one pad
/// per player, the profile, the game's compatibility settings kept
/// ([`layers`]), and Nucleus.
pub fn start(
    start: &Start,
    pads: Box<dyn Pads>,
    xinput: &mut dyn XInput,
    mut desktop: Box<dyn Desktop>,
    layers: Box<dyn Layers>,
    launcher: &mut dyn Launcher,
) -> Result<Running> {
    // Its profile list would show the user's state, and its instances could
    // not be told from the session's.
    if let Some(process) = desktop
        .processes()?
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(EXECUTABLE))
    {
        bail!(already_running(process.pid));
    }
    let free = pads::free(xinput)?;
    ensure!(
        start.players <= free,
        "{} players need {} free XInput controller slots and the PC's own controllers leave {free}: disconnect one of the PC's controllers or use fewer players",
        start.players,
        start.players
    );
    let indices = pads::plug(pads.as_ref(), xinput, start.players, pads::PLUG_TIMEOUT)?;
    let session = coop::Session::new(
        start.id,
        start.kind,
        &start.handler.guid,
        start.title,
        &indices,
        start.owner,
    )
    .map_err(anyhow::Error::msg)?;
    let screen = desktop.virtual_screen();
    let screens = desktop.monitors()?;
    let players = indices
        .iter()
        .enumerate()
        .map(|(slot, &xinput)| {
            Ok(Player {
                xinput,
                bounds: window::parking(screen, slot, start.size).map_err(anyhow::Error::msg)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    // Put back when the session ends, or here should Nucleus not start.
    let settings = files::write_settings(start.folder)?;
    let number = files::write_profile(start.folder, &start.handler.guid, &players, &screens)?;
    tracing::info!(
        number,
        game = start.handler.guid,
        players = start.players,
        "Nucleus Co-op profile {TITLE} written"
    );
    // Restored when the session ends, or here should Nucleus not start.
    let layers = Guard::take(
        layers,
        start.folder,
        &start.handler.executable,
        start.exe_path,
    )
    .context("keeping the game's compatibility settings, which Nucleus Co-op changes")?;
    let crashlog = Crashlog::at_launch(start.folder);
    let nucleus = launcher.launch(
        start.folder,
        &start.handler.guid,
        &steam_environment(start.steam_app),
    )?;
    let content = start.folder.join("content");
    let keeper = Keeper::new(
        start.players,
        std::slice::from_ref(&start.handler.executable),
        &content.to_string_lossy(),
        nucleus.since(),
    );
    let shared = Arc::new(Shared {
        session: Mutex::new(session),
        sizes: Mutex::new(vec![start.size; start.players]),
        roots: Mutex::new(vec![None; start.players]),
    });
    let runner = Runner {
        shared: shared.clone(),
        desktop,
        nucleus,
        keeper,
        title: start.title.to_string(),
        players: start.players,
        clicked: false,
        unclicked: None,
        showing: Vec::new(),
        logged: Vec::new(),
        shown: false,
        unseen: None,
        crashlog: Some(crashlog),
        game: Some((start.folder.to_path_buf(), start.handler.guid.clone())),
        progress: (false, 0, 0),
        windows: vec![None; start.players],
        deadline: Instant::now() + START_TIMEOUT,
        exited: false,
        failed: 0,
        settings: Some(settings),
        layers: Some(layers),
    };
    let (stop, stopped) = mpsc::channel();
    // Should the thread not start, dropping the runner ends Nucleus' job.
    let thread = std::thread::Builder::new()
        .name("nucleus".into())
        .spawn(move || run(runner, stopped))
        .context("starting the Nucleus Co-op session thread")?;
    Ok(Running {
        shared,
        pads,
        stop: Mutex::new(Some(stop)),
        thread: Mutex::new(Some(thread)),
        players: start.players,
    })
}

impl Running {
    /// Read or change the session: players joining, leaving, kicked, ended.
    pub fn with_session<R>(&self, f: impl FnOnce(&mut coop::Session) -> R) -> R {
        f(&mut self.shared.session.lock().unwrap())
    }

    /// Each slot's game window while the session runs.
    pub fn windows(&self) -> Vec<Option<u64>> {
        let session = self.shared.session.lock().unwrap();
        session.slots.iter().map(|s| s.window).collect()
    }

    /// Slot `slot`'s capture target: its game window, captured on the GPU
    /// of `display`. A window Nucleus replaced gives a new target, which the
    /// stream reopens on; an error while the slot has no window.
    pub fn capture_target(&self, slot: usize, display: &str) -> Result<String> {
        let window = self
            .windows()
            .get(slot)
            .copied()
            .with_context(|| format!("the session has {} players, not {}", self.players, slot + 1))?
            .with_context(|| format!("Player {}'s game window is not open", slot + 1))?;
        Ok(crate::capture::WindowTarget::new(window, display).to_string())
    }

    /// Park slot `slot`'s window at its player's stream size from the next
    /// pass on.
    pub fn resize(&self, slot: usize, size: (u32, u32)) -> Result<()> {
        let mut sizes = self.shared.sizes.lock().unwrap();
        let players = sizes.len();
        let entry = sizes
            .get_mut(slot)
            .with_context(|| format!("the session has {players} players, not {}", slot + 1))?;
        *entry = size;
        Ok(())
    }

    /// A device's input: its controller drives its slot's pad while it
    /// streams; anything else is dropped, with the limit to show.
    pub fn input(&self, device: &str, event: Input) -> Result<(), Dropped> {
        let slot = {
            let session = self.shared.session.lock().unwrap();
            match session.slot_of(device) {
                Some(slot) if session.route(device).is_some() => slot,
                _ => return Err(Dropped::Ignored),
            }
        };
        self.pads.send(pads::route(event, slot, self.players)?);
        Ok(())
    }

    /// Rumble and other feedback since the last call, per slot, for its
    /// player's first controller.
    pub fn feedback(&self) -> Vec<(usize, PadReport)> {
        self.pads
            .reports()
            .into_iter()
            .filter_map(|report| pads::feedback(report, self.players))
            .collect()
    }

    /// Slot `slot`'s instance, for its audio. An error on a Windows build
    /// without process loopback capture and while the instance is not there.
    pub fn audio(&self, slot: usize) -> Result<crate::process::AudioProcess> {
        self.audio_on(crate::display::windows_build(), slot)
    }

    fn audio_on(&self, build: Option<u32>, slot: usize) -> Result<crate::process::AudioProcess> {
        if let Some(limit) = audio_limit(build) {
            bail!(limit);
        }
        let root = self
            .shared
            .roots
            .lock()
            .unwrap()
            .get(slot)
            .copied()
            .flatten()
            .with_context(|| format!("Player {}'s game instance is not running", slot + 1))?;
        crate::process::AudioProcess::open(root)
            .with_context(|| format!("Player {}'s game instance", slot + 1))
    }

    /// End the session and wait until Nucleus and the instances closed; a
    /// second call waits for the first.
    pub fn stop(&self) {
        drop(self.stop.lock().unwrap().take());
        let mut thread = self.thread.lock().unwrap();
        if let Some(thread) = thread.take()
            && thread.join().is_err()
        {
            tracing::error!("the Nucleus Co-op session thread panicked");
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::super::instances::tests::{Fake, ON_SCREEN, fake, process, window};
    use super::super::pads::tests::{Fake as FakeXInput, Recorder};
    use super::*;
    use butterpollo_core::nucleus::window::{Rect, Window};
    use butterpollo_core::steam::Process;

    #[test]
    fn webview2_keeps_its_data_in_the_users_own_folder() {
        // WebView2's browser runs without administrator rights and could not
        // write Nucleus' Administrators-only webview\cache (#59).
        let local = Path::new(r"C:\Users\Player\AppData\Local");
        let environment = webview_environment(local, &BTreeMap::new());
        assert_eq!(environment.len(), 1);
        assert_eq!(
            environment["WEBVIEW2_USER_DATA_FOLDER"],
            r"C:\Users\Player\AppData\Local\Butterpollo\nucleus-webview"
        );
    }

    #[test]
    fn a_steam_games_app_id_joins_webview2s_folder() {
        // Nucleus needs both: WebView2's folder for its window, and the app
        // id so a Steam game it starts by its .exe doesn't restart via Steam.
        let local = Path::new(r"C:\Users\Player\AppData\Local");
        let environment = webview_environment(local, &steam_environment(Some(400)));
        assert_eq!(
            environment.keys().collect::<Vec<_>>(),
            ["SteamAppId", "SteamGameId", "WEBVIEW2_USER_DATA_FOLDER"]
        );
        assert_eq!(environment["SteamAppId"], "400");
    }

    #[test]
    fn an_account_without_administrator_rights_is_refused_before_nucleus_starts() {
        // A standard user's Nucleus Co-op hung on its loading screen: it
        // could not write its WebView2 cache into the Administrators-only folder.
        assert!(require_administrator(true).is_ok());
        let refused = format!("{:#}", require_administrator(false).unwrap_err());
        assert!(refused.contains("needs administrator rights"), "{refused}");
        // Both ways Windows has no elevated token for an account are named.
        assert!(refused.contains("a standard account"), "{refused}");
        assert!(refused.contains("Administrator protection"), "{refused}");
    }

    /// A fake desktop the test changes while the session runs.
    #[derive(Clone)]
    struct Screen(Arc<Mutex<Fake>>);
    impl Desktop for Screen {
        fn processes(&mut self) -> Result<Vec<Process>> {
            self.0.lock().unwrap().processes()
        }
        fn image_path(&mut self, pid: u32) -> Option<String> {
            self.0.lock().unwrap().image_path(pid)
        }
        fn windows(&mut self) -> Result<Vec<Window>> {
            self.0.lock().unwrap().windows()
        }
        fn monitors(&mut self) -> Result<Vec<Rect>> {
            self.0.lock().unwrap().monitors()
        }
        fn virtual_screen(&mut self) -> Rect {
            self.0.lock().unwrap().virtual_screen()
        }
        fn place(&mut self, window: u64, to: Rect) -> Result<()> {
            self.0.lock().unwrap().place(window, to)
        }
        fn unthrottle(&mut self, processes: &BTreeSet<u32>) {
            self.0.lock().unwrap().unthrottle(processes)
        }
        fn label(&mut self, processes: &BTreeSet<u32>, text: &str) -> Result<Option<u64>> {
            self.0.lock().unwrap().label(processes, text)
        }
        fn click(&mut self, label: u64) -> Result<()> {
            self.0.lock().unwrap().click(label)
        }
        fn message_boxes(&mut self, processes: &BTreeSet<u32>) -> Result<Vec<MessageBox>> {
            self.0.lock().unwrap().message_boxes(processes)
        }
    }

    const NUCLEUS: u32 = 10;
    const SINCE: u64 = 100;

    #[derive(Clone, Default)]
    struct Probe {
        exit: Arc<Mutex<Option<u32>>>,
        stopped: Arc<Mutex<Option<BTreeMap<u32, u64>>>>,
        launched: Arc<Mutex<Vec<String>>>,
        environments: Arc<Mutex<Vec<BTreeMap<String, String>>>>,
        /// The user's compatibility settings.
        layers: super::super::layers::tests::Map,
        /// Why Nucleus does not start, when it does not.
        refused: Arc<Mutex<Option<String>>>,
    }
    struct FakeNucleus {
        probe: Probe,
        screen: Screen,
    }
    impl Nucleus for FakeNucleus {
        fn since(&self) -> u64 {
            SINCE
        }
        fn exit_code(&mut self) -> Result<Option<u32>> {
            Ok(*self.probe.exit.lock().unwrap())
        }
        fn job(&mut self) -> Result<BTreeSet<u32>> {
            // Nucleus and every process it started, as its job would hold them.
            let desktop = self.screen.0.lock().unwrap();
            let exited = self.probe.exit.lock().unwrap().is_some();
            Ok(desktop
                .processes
                .iter()
                .filter(|p| (p.pid == NUCLEUS && !exited) || p.parent == NUCLEUS)
                .map(|p| p.pid)
                .collect())
        }
        fn stop(&mut self, instances: &BTreeMap<u32, u64>) {
            *self.probe.stopped.lock().unwrap() = Some(instances.clone());
        }
    }
    impl Launcher for (Probe, Screen) {
        fn launch(
            &mut self,
            _folder: &Path,
            guid: &str,
            environment: &BTreeMap<String, String>,
        ) -> Result<Box<dyn Nucleus>> {
            if let Some(why) = self.0.refused.lock().unwrap().clone() {
                bail!(why);
            }
            self.0.launched.lock().unwrap().push(guid.into());
            self.0
                .environments
                .lock()
                .unwrap()
                .push(environment.clone());
            self.1
                .0
                .lock()
                .unwrap()
                .processes
                .push(process(NUCLEUS, 1, SINCE, EXECUTABLE));
            Ok(Box::new(FakeNucleus {
                probe: self.0.clone(),
                screen: self.1.clone(),
            }))
        }
    }

    fn handler() -> Handler {
        Handler {
            file: "Game.js".into(),
            guid: "Game".into(),
            name: "Game".into(),
            executable: "Game.exe".into(),
            max_players: 4,
            supports_keyboard: false,
            hides_taskbar: false,
            own: false,
            owner: None,
            admin: Vec::new(),
        }
    }

    fn kind(players: usize) -> coop::Kind {
        if players == 1 {
            coop::Kind::Background
        } else {
            coop::Kind::Coop
        }
    }

    fn runner(players: usize, screen: &Screen, probe: &Probe, t0: Instant) -> Runner {
        let pads: Vec<u8> = (0..players as u8).collect();
        let session =
            coop::Session::new(1, kind(players), "Game", "Game", &pads, ("phone", "Phone"))
                .unwrap();
        screen
            .0
            .lock()
            .unwrap()
            .processes
            .push(process(NUCLEUS, 1, SINCE, EXECUTABLE));
        Runner {
            shared: Arc::new(Shared {
                session: Mutex::new(session),
                sizes: Mutex::new(vec![(1280, 720); players]),
                roots: Mutex::new(vec![None; players]),
            }),
            desktop: Box::new(screen.clone()),
            nucleus: Box::new(FakeNucleus {
                probe: probe.clone(),
                screen: screen.clone(),
            }),
            keeper: Keeper::new(players, &["Game.exe".into()], r"C:\Nucleus\content", SINCE),
            title: "Game".into(),
            players,
            clicked: false,
            unclicked: None,
            showing: Vec::new(),
            logged: Vec::new(),
            shown: false,
            unseen: None,
            crashlog: None,
            game: None,
            progress: (false, 0, 0),
            windows: vec![None; players],
            deadline: t0 + START_TIMEOUT,
            exited: false,
            failed: 0,
            settings: None,
            layers: None,
        }
    }

    fn screen() -> Screen {
        Screen(Arc::new(Mutex::new(Fake {
            obey: true,
            ..fake(vec![], vec![])
        })))
    }

    /// Nucleus starts player `slot`'s instance with a window on the monitor.
    fn instance(screen: &Screen, slot: u32) {
        let mut desktop = screen.0.lock().unwrap();
        let pid = 20 + slot;
        desktop.processes.push(process(
            pid,
            NUCLEUS,
            SINCE + 1 + u64::from(slot),
            "Game.exe",
        ));
        desktop
            .windows
            .push(window(u64::from(pid) * 10, pid, ON_SCREEN));
    }

    fn label(screen: &Screen) {
        screen
            .0
            .lock()
            .unwrap()
            .labels
            .push((NUCLEUS, TITLE.into(), 7));
    }

    const HINT: &str = "open Nucleus Co-op and load the profile Rubylight once";

    #[test]
    fn the_profile_is_clicked_and_the_session_runs_with_every_window() {
        let t0 = Instant::now();
        let (screen, probe) = (screen(), Probe::default());
        let mut runner = runner(2, &screen, &probe, t0);
        assert_eq!(runner.step(t0), None);
        assert!(screen.0.lock().unwrap().clicks.is_empty());
        label(&screen);
        assert_eq!(runner.step(t0), None);
        assert_eq!(screen.0.lock().unwrap().clicks, vec![7]);
        // Clicked once only.
        assert_eq!(runner.step(t0), None);
        assert_eq!(screen.0.lock().unwrap().clicks.len(), 1);
        instance(&screen, 0);
        assert_eq!(runner.step(t0), None);
        assert_eq!(runner.shared.session.lock().unwrap().phase, Phase::Starting);
        instance(&screen, 1);
        assert_eq!(runner.step(t0), None);
        let session = runner.shared.session.lock().unwrap().clone();
        assert_eq!(session.phase, Phase::Running);
        assert_eq!(session.slots[0].window, Some(200));
        assert_eq!(session.slots[1].window, Some(210));
        assert!(runner.shared.roots.lock().unwrap()[1].is_some_and(|r| r.pid == 21));
        // Both windows were parked beside the desktop.
        let placed = screen.0.lock().unwrap().placed.clone();
        assert!(
            placed
                .iter()
                .any(|(id, to)| *id == 210 && i64::from(to.x) > ON_SCREEN.right())
        );
        // Running, the session never times out.
        assert_eq!(runner.step(t0 + 10 * START_TIMEOUT), None);
    }

    #[test]
    fn no_profile_entry_in_time_fails_says_what_to_do_and_stops_nucleus() {
        let t0 = Instant::now();
        let (screen, probe) = (screen(), Probe::default());
        let mut runner = runner(1, &screen, &probe, t0);
        assert_eq!(runner.step(t0 + START_TIMEOUT / 2), None);
        let end = runner.step(t0 + START_TIMEOUT).unwrap();
        assert_eq!(end, End::Failed("Nucleus Co-op showed no window".into()));
        let shared = runner.shared.clone();
        runner.finish(end.clone());
        assert_eq!(shared.session.lock().unwrap().phase, Phase::Ended(end));
        assert_eq!(*probe.stopped.lock().unwrap(), Some(BTreeMap::new()));
    }

    #[test]
    fn a_message_box_nucleus_shows_instead_of_the_profile_is_named() {
        // Nucleus' check of a Documents folder in OneDrive waits for an
        // answer before its window opens; nobody gives it during a
        // background stream (#59).
        let t0 = Instant::now();
        let (screen, probe) = (screen(), Probe::default());
        let mut runner = runner(1, &screen, &probe, t0);
        let shown = MessageBox {
            caption: "OneDrive must be disabled".into(),
            text: "Your Documents folder\r\nis in OneDrive.".into(),
        };
        screen
            .0
            .lock()
            .unwrap()
            .boxes
            .push((NUCLEUS, shown.clone()));
        // Another program's box is not Nucleus'.
        screen.0.lock().unwrap().boxes.push((
            99,
            MessageBox {
                caption: "Other".into(),
                text: String::new(),
            },
        ));
        assert_eq!(runner.step(t0), None);
        assert_eq!(runner.step(t0), None);
        assert_eq!(runner.logged, [shown]);
        assert_eq!(
            runner.step(t0 + START_TIMEOUT),
            Some(End::Failed(
                "Nucleus Co-op shows \"OneDrive must be disabled: Your Documents folder is in OneDrive.\" on the PC's screen".into()
            ))
        );
    }

    #[test]
    fn a_window_without_the_profile_is_named_with_the_sessions_crash() {
        let t0 = Instant::now();
        let (screen, probe) = (screen(), Probe::default());
        let mut runner = runner(1, &screen, &probe, t0);
        let folder = tempfile::tempdir().unwrap();
        let log = folder.path().join("crashlog.txt");
        std::fs::write(&log, "[01.10.2026 10:00:00] System.Exception: earlier\n").unwrap();
        runner.crashlog = Some(Crashlog::at_launch(folder.path()));
        let mut later = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
        std::io::Write::write_all(
            &mut later,
            b"[29.10.2026 12:00:00] System.NullReferenceException: no game\n   at ChangeGame()\n",
        )
        .unwrap();
        screen
            .0
            .lock()
            .unwrap()
            .windows
            .push(window(5, NUCLEUS, ON_SCREEN));
        assert_eq!(runner.step(t0), None);
        assert_eq!(
            runner.step(t0 + START_TIMEOUT),
            Some(End::Failed(format!(
                "Nucleus Co-op shows no profile Rubylight for Game; {HINT}. Its crashlog.txt says: [29.10.2026 12:00:00] System.NullReferenceException: no game"
            )))
        );
    }

    #[test]
    fn a_game_nucleus_does_not_list_is_named_with_its_list() {
        let t0 = Instant::now();
        let (screen, probe) = (screen(), Probe::default());
        let folder = tempfile::tempdir().unwrap();
        let exe = folder.path().join("Game.exe");
        std::fs::write(&exe, b"MZ").unwrap();
        let setup =
            butterpollo_core::nucleus::generic::Setup::new(&exe.to_string_lossy(), "Game", None);
        let own = files::write_own(folder.path(), &setup).unwrap();
        screen
            .0
            .lock()
            .unwrap()
            .windows
            .push(window(5, NUCLEUS, ON_SCREEN));
        let timeout = || {
            let mut runner = runner(1, &screen, &probe, t0);
            runner.game = Some((folder.path().to_path_buf(), own.guid.clone()));
            assert_eq!(runner.step(t0), None);
            runner.step(t0 + START_TIMEOUT)
        };
        // Listed: the profile is what's missing.
        assert_eq!(
            timeout(),
            Some(End::Failed(format!(
                "Nucleus Co-op shows no profile Rubylight for Game; {HINT}"
            )))
        );
        // Not in the list Nucleus reads (#59).
        let list = files::list(folder.path());
        std::fs::remove_file(&list).unwrap();
        assert_eq!(
            timeout(),
            Some(End::Failed(format!(
                "Nucleus Co-op does not list Game: {} has no entry for it with its handler and an .exe that is there",
                list.display()
            )))
        );
    }

    #[test]
    fn only_crashes_written_after_the_launch_are_quoted() {
        let folder = tempfile::tempdir().unwrap();
        let log = folder.path().join("crashlog.txt");
        // No file at the launch or after it.
        let none = Crashlog::at_launch(folder.path());
        assert_eq!(none.from, 0);
        assert_eq!(none.newest(), None);
        std::fs::write(&log, "[1] System.Exception: before\n   at Old()\n").unwrap();
        let crashlog = Crashlog::at_launch(folder.path());
        assert_eq!(crashlog.newest(), None);
        let append = |text: &str| {
            let mut file = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
            std::io::Write::write_all(&mut file, text.as_bytes()).unwrap();
        };
        append(
            "[2] System.Exception: first\n   at A()\n[3] System.Exception: second\n   at B()\n\n",
        );
        assert_eq!(
            crashlog.newest().as_deref(),
            Some("[3] System.Exception: second")
        );
        // A file started again since is read whole.
        std::fs::write(&log, "[4] new\n").unwrap();
        assert_eq!(crashlog.newest().as_deref(), Some("[4] new"));
        // A long line is cut.
        append(&format!("[5] {}\n", "x".repeat(400)));
        let cut = Crashlog::at_launch(folder.path());
        append(&format!("[6] {}\n", "y".repeat(400)));
        let line = cut.newest().unwrap();
        assert!(
            line.starts_with("[6] yyy") && line.ends_with("..."),
            "{line}"
        );
        assert_eq!(line.chars().count(), Crashlog::LIMIT + 3);
        // None written since the launch: nothing is quoted.
        assert_eq!(Crashlog::at_launch(folder.path()).newest(), None);
    }

    #[test]
    fn a_covered_profile_list_is_named_when_it_times_out() {
        let t0 = Instant::now();
        let (screen, probe) = (screen(), Probe::default());
        let mut runner = runner(1, &screen, &probe, t0);
        label(&screen);
        screen.0.lock().unwrap().covered = true;
        assert_eq!(runner.step(t0), None);
        // Tried again on every pass.
        assert_eq!(runner.step(t0), None);
        assert_eq!(
            runner.step(t0 + START_TIMEOUT),
            Some(End::Failed(
                "Nucleus Co-op did not start Game: covered".into()
            ))
        );
    }

    fn reminder() -> MessageBox {
        MessageBox {
            caption: "Nucleus Co-op".into(),
            text: "Press the End key to unlock the input.".into(),
        }
    }
    const REMINDED: &str = "Nucleus Co-op shows \"Nucleus Co-op: Press the End key to unlock the input.\" on the PC's screen";

    #[test]
    fn a_box_covering_the_profile_list_is_named_instead_of_the_cover() {
        let t0 = Instant::now();
        let (screen, probe) = (screen(), Probe::default());
        let mut runner = runner(1, &screen, &probe, t0);
        label(&screen);
        {
            let mut desktop = screen.0.lock().unwrap();
            desktop.covered = true;
            desktop.boxes.push((NUCLEUS, reminder()));
        }
        assert_eq!(runner.step(t0), None);
        assert_eq!(runner.logged, [reminder()]);
        assert_eq!(
            runner.step(t0 + START_TIMEOUT),
            Some(End::Failed(REMINDED.into()))
        );
    }

    #[test]
    fn a_box_nucleus_shows_after_the_click_is_named() {
        let t0 = Instant::now();
        let (screen, probe) = (screen(), Probe::default());
        let mut runner = runner(1, &screen, &probe, t0);
        label(&screen);
        assert_eq!(runner.step(t0), None);
        assert_eq!(screen.0.lock().unwrap().clicks, vec![7]);
        screen.0.lock().unwrap().boxes.push((NUCLEUS, reminder()));
        assert_eq!(runner.step(t0), None);
        assert_eq!(runner.logged, [reminder()]);
        assert_eq!(
            runner.step(t0 + START_TIMEOUT),
            Some(End::Failed(REMINDED.into()))
        );
    }

    #[test]
    fn a_box_gone_from_the_list_is_not_quoted_and_a_failed_list_is_named() {
        let t0 = Instant::now();
        let (screen, probe) = (screen(), Probe::default());
        let mut runner = runner(1, &screen, &probe, t0);
        screen.0.lock().unwrap().boxes.push((NUCLEUS, reminder()));
        assert_eq!(runner.step(t0), None);
        assert_eq!(runner.showing, [reminder()]);
        // The box may be gone; a list that failed doesn't say it's still up.
        screen.0.lock().unwrap().unlisted = true;
        assert_eq!(runner.step(t0), None);
        assert_eq!(runner.showing, []);
        assert_eq!(
            runner.step(t0 + START_TIMEOUT),
            Some(End::Failed(
                "Nucleus Co-op's windows could not be listed: access denied".into()
            ))
        );
    }

    #[test]
    fn each_step_nucleus_makes_gives_it_the_timeout_again() {
        let t0 = Instant::now();
        let (screen, probe) = (screen(), Probe::default());
        let mut runner = runner(2, &screen, &probe, t0);
        label(&screen);
        let clicked = t0 + START_TIMEOUT / 2;
        assert_eq!(runner.step(clicked), None);
        // Past the first deadline, but the click came since.
        let first = clicked + START_TIMEOUT / 2 + PERIOD;
        instance(&screen, 0);
        assert_eq!(runner.step(first), None);
        assert_eq!(runner.step(first + START_TIMEOUT - PERIOD), None);
        assert_eq!(
            runner.step(first + START_TIMEOUT),
            Some(End::Failed(format!(
                "Nucleus Co-op started 1 of 2 instances of Game; {HINT}"
            )))
        );
    }

    #[test]
    fn an_instance_without_a_window_is_named() {
        let t0 = Instant::now();
        let (screen, probe) = (screen(), Probe::default());
        let mut runner = runner(1, &screen, &probe, t0);
        label(&screen);
        instance(&screen, 0);
        screen.0.lock().unwrap().windows.clear();
        assert_eq!(runner.step(t0), None);
        assert_eq!(
            runner.step(t0 + START_TIMEOUT),
            Some(End::Failed(
                "Player 1's instance of Game showed no window".into()
            ))
        );
    }

    #[test]
    fn nucleus_exiting_before_the_game_fails_with_its_code() {
        let t0 = Instant::now();
        let (screen, probe) = (screen(), Probe::default());
        let mut runner = runner(1, &screen, &probe, t0);
        assert_eq!(runner.step(t0), None);
        *probe.exit.lock().unwrap() = Some(3);
        assert_eq!(
            runner.step(t0),
            Some(End::Failed(
                "Nucleus Co-op exited with code 3 before starting Game".into()
            ))
        );
    }

    #[test]
    fn nucleus_exiting_after_the_instances_started_leaves_them_running() {
        let t0 = Instant::now();
        let (screen, probe) = (screen(), Probe::default());
        let mut runner = runner(1, &screen, &probe, t0);
        label(&screen);
        instance(&screen, 0);
        assert_eq!(runner.step(t0), None);
        *probe.exit.lock().unwrap() = Some(0);
        assert_eq!(runner.step(t0), None);
        assert_eq!(runner.step(t0), None);
        assert_eq!(runner.shared.session.lock().unwrap().phase, Phase::Running);
    }

    #[test]
    fn an_instance_that_exits_ends_the_session_and_everything_stops() {
        let t0 = Instant::now();
        let (screen, probe) = (screen(), Probe::default());
        let mut runner = runner(2, &screen, &probe, t0);
        label(&screen);
        instance(&screen, 0);
        instance(&screen, 1);
        assert_eq!(runner.step(t0), None);
        let shared = runner.shared.clone();
        assert_eq!(shared.session.lock().unwrap().phase, Phase::Running);
        screen.0.lock().unwrap().processes.retain(|p| p.pid != 21);
        let end = runner.step(t0).unwrap();
        assert_eq!(
            end,
            End::Crashed("Player 2's instance of Game exited".into())
        );
        runner.finish(end.clone());
        assert_eq!(shared.session.lock().unwrap().phase, Phase::Ended(end));
        // Player 1's instance is stopped with Nucleus.
        assert_eq!(
            *probe.stopped.lock().unwrap(),
            Some(BTreeMap::from([(20, SINCE + 1)]))
        );
    }

    #[test]
    fn an_instance_that_exits_while_starting_fails_the_launch() {
        let t0 = Instant::now();
        let (screen, probe) = (screen(), Probe::default());
        let mut runner = runner(2, &screen, &probe, t0);
        label(&screen);
        instance(&screen, 0);
        assert_eq!(runner.step(t0), None);
        screen.0.lock().unwrap().processes.retain(|p| p.pid != 20);
        assert_eq!(
            runner.step(t0),
            Some(End::Failed("Player 1's instance of Game exited".into()))
        );
    }

    #[test]
    fn a_session_ended_by_the_host_keeps_its_reason() {
        let t0 = Instant::now();
        let (screen, probe) = (screen(), Probe::default());
        let mut runner = runner(1, &screen, &probe, t0);
        let shared = runner.shared.clone();
        shared
            .session
            .lock()
            .unwrap()
            .end(End::Crashed("kicked".into()));
        let end = runner.step(t0).unwrap();
        runner.finish(End::Ended);
        assert_eq!(end, End::Crashed("kicked".into()));
        assert_eq!(
            shared.session.lock().unwrap().phase,
            Phase::Ended(End::Crashed("kicked".into()))
        );
        assert!(probe.stopped.lock().unwrap().is_some());
    }

    #[test]
    fn a_window_that_cannot_be_parked_ends_the_session_after_some_passes() {
        let t0 = Instant::now();
        let (screen, probe) = (screen(), Probe::default());
        let mut runner = runner(1, &screen, &probe, t0);
        *runner.shared.sizes.lock().unwrap() = vec![(40000, 720)];
        label(&screen);
        instance(&screen, 0);
        for _ in 1..FAILED_PASSES {
            assert_eq!(runner.step(t0), None);
        }
        let end = runner.step(t0).unwrap();
        assert!(
            matches!(&end, End::Failed(why) if why.contains("does not fit beside the PC's desktop")),
            "{end:?}"
        );
    }

    #[test]
    fn a_good_pass_resets_the_failed_passes() {
        let t0 = Instant::now();
        let (screen, probe) = (screen(), Probe::default());
        let mut runner = runner(1, &screen, &probe, t0);
        label(&screen);
        instance(&screen, 0);
        for _ in 0..2 {
            *runner.shared.sizes.lock().unwrap() = vec![(40000, 720)];
            for _ in 1..FAILED_PASSES {
                assert_eq!(runner.step(t0), None);
            }
            *runner.shared.sizes.lock().unwrap() = vec![(1280, 720)];
            assert_eq!(runner.step(t0), None);
        }
        assert_eq!(runner.shared.session.lock().unwrap().phase, Phase::Running);
    }

    #[test]
    fn several_profile_entries_fail_at_once() {
        let t0 = Instant::now();
        let (screen, probe) = (screen(), Probe::default());
        let mut runner = runner(1, &screen, &probe, t0);
        label(&screen);
        label(&screen);
        assert_eq!(runner.step(t0), Some(End::Failed("two labels".into())));
    }

    #[test]
    fn per_player_audio_needs_build_20348() {
        let limit = audio_limit(Some(19045)).unwrap();
        assert!(
            limit.starts_with(
                "Per-player audio needs Windows build 20348 or newer and this PC runs build 19045"
            ),
            "{limit}"
        );
        assert_eq!(audio_limit(Some(20348)), None);
        assert_eq!(audio_limit(None), None);
    }

    /// The session's pads: a recorder the test keeps reading.
    struct Recorded(Arc<Recorder>);
    impl Pads for Recorded {
        fn send(&self, event: Input) {
            self.0.send(event)
        }
        fn wait_ready(&self, mask: u16, timeout: Duration) -> bool {
            self.0.wait_ready(mask, timeout)
        }
        fn reports(&self) -> Vec<PadReport> {
            self.0.reports()
        }
    }

    fn start_with(
        folder: &Path,
        players: usize,
        taken: [bool; 4],
        screen: &Screen,
        probe: &Probe,
    ) -> (Result<Running>, Arc<Recorder>) {
        start_steam(folder, players, taken, screen, probe, None)
    }

    fn start_steam(
        folder: &Path,
        players: usize,
        taken: [bool; 4],
        screen: &Screen,
        probe: &Probe,
        steam_app: Option<u32>,
    ) -> (Result<Running>, Arc<Recorder>) {
        let recorder = Arc::new(Recorder::default());
        let handler = handler();
        let mut xinput = FakeXInput {
            pads: &recorder,
            taken,
            hidden: false,
            intruder: false,
        };
        let request = Start {
            id: 1,
            kind: kind(players),
            folder,
            handler: &handler,
            exe_path: r"D:\Games\Game\Game.exe",
            steam_app,
            title: "Game",
            players,
            owner: ("phone", "Phone"),
            size: (1280, 720),
        };
        let running = start(
            &request,
            Box::new(Recorded(recorder.clone())),
            &mut xinput,
            Box::new(screen.clone()),
            Box::new(probe.layers.clone()),
            &mut (probe.clone(), screen.clone()),
        );
        (running, recorder)
    }

    /// Wait until the session thread got `running` to `phase`.
    fn wait_for(running: &Running, phase: impl Fn(&Phase) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !running.with_session(|s| phase(&s.phase)) {
            assert!(
                Instant::now() < deadline,
                "{:?}",
                running.with_session(|s| s.phase.clone())
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_session_starts_runs_and_stops_everything() {
        let folder = tempfile::tempdir().unwrap();
        let (screen, probe) = (screen(), Probe::default());
        label(&screen);
        let (running, recorder) = start_with(
            folder.path(),
            2,
            [false, true, false, false],
            &screen,
            &probe,
        );
        let running = running.unwrap();
        let error = running.capture_target(0, r"\\.\DISPLAY1").unwrap_err();
        assert_eq!(error.to_string(), "Player 1's game window is not open");
        // One pad per player, skipping the PC's own at index 1.
        assert_eq!(
            running.with_session(|s| s.slots.iter().map(|s| s.pad).collect::<Vec<_>>()),
            [0, 2]
        );
        assert_eq!(recorder.sent.lock().unwrap().len(), 2);
        assert_eq!(*probe.launched.lock().unwrap(), ["Game"]);
        // Found from no Steam app: Nucleus gets the user's environment alone.
        assert_eq!(*probe.environments.lock().unwrap(), [BTreeMap::new()]);
        let profile = std::fs::read_to_string(
            folder
                .path()
                .join("game profiles")
                .join("Game")
                .join("Profile[1].json"),
        )
        .unwrap();
        assert!(profile.contains(TITLE), "{profile}");
        assert!(folder.path().join("Settings.ini").is_file());
        // Nucleus loads the profile and starts both instances.
        let deadline = Instant::now() + Duration::from_secs(10);
        while screen.0.lock().unwrap().clicks.is_empty() {
            assert!(Instant::now() < deadline, "the profile was never clicked");
            std::thread::sleep(Duration::from_millis(10));
        }
        instance(&screen, 0);
        instance(&screen, 1);
        wait_for(&running, |p| *p == Phase::Running);
        assert_eq!(running.windows(), [Some(200), Some(210)]);
        // Each player's stream captures that player's window, which capture
        // reads back as the same window and display.
        let target = running.capture_target(1, r"\\.\DISPLAY2").unwrap();
        assert_eq!(target, r"window:210@\\.\DISPLAY2");
        assert_eq!(
            crate::capture::WindowTarget::parse(&target).unwrap(),
            Some(crate::capture::WindowTarget::new(210, r"\\.\DISPLAY2"))
        );
        assert_eq!(
            running.capture_target(0, r"\\.\DISPLAY1").unwrap(),
            r"window:200@\\.\DISPLAY1"
        );
        let error = running.capture_target(2, r"\\.\DISPLAY1").unwrap_err();
        assert_eq!(error.to_string(), "the session has 2 players, not 3");
        // Player 2's game replaced its window: the target follows it.
        {
            let mut desktop = screen.0.lock().unwrap();
            desktop.windows.retain(|w| w.id != 210);
            desktop.windows.push(window(211, 21, ON_SCREEN));
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while running.capture_target(1, r"\\.\DISPLAY2").ok().as_deref()
            != Some(r"window:211@\\.\DISPLAY2")
        {
            assert!(Instant::now() < deadline, "{:?}", running.windows());
            std::thread::sleep(Duration::from_millis(10));
        }
        running.stop();
        assert_eq!(
            running.with_session(|s| s.phase.clone()),
            Phase::Ended(End::Ended)
        );
        assert_eq!(
            *probe.stopped.lock().unwrap(),
            Some(BTreeMap::from([(20, SINCE + 1), (21, SINCE + 2)]))
        );
    }

    #[test]
    fn a_game_found_from_steam_starts_with_its_app_id() {
        let folder = tempfile::tempdir().unwrap();
        let (screen, probe) = (screen(), Probe::default());
        let (running, _) = start_steam(folder.path(), 1, [false; 4], &screen, &probe, Some(242050));
        running.unwrap().stop();
        let app = || "242050".to_string();
        assert_eq!(
            *probe.environments.lock().unwrap(),
            [BTreeMap::from([
                ("SteamAppId".to_string(), app()),
                ("SteamGameId".to_string(), app()),
            ])]
        );
        assert_eq!(steam_environment(None), BTreeMap::new());
    }

    #[test]
    fn the_games_compatibility_settings_are_back_after_a_session_or_a_failed_start() {
        use super::super::layers::tests::text;
        let folder = tempfile::tempdir().unwrap();
        let (screen, probe) = (screen(), Probe::default());
        let game = r"D:\Games\Game\Game.exe";
        let set = |value: &str| {
            probe
                .layers
                .values
                .lock()
                .unwrap()
                .insert(game.into(), text(value))
        };
        set("~ RUNASADMIN");
        let ini = folder.path().join("Settings.ini");
        std::fs::write(&ini, "[Dev]\r\nShowFavoriteOnly=True\r\n").unwrap();
        let favourites = || {
            let text = std::fs::read(&ini).unwrap();
            butterpollo_core::nucleus::profile::ini_value(&text, "Dev", "ShowFavoriteOnly").unwrap()
        };
        let (running, _) = start_with(folder.path(), 1, [false; 4], &screen, &probe);
        let running = running.unwrap();
        assert_eq!(favourites(), b"False");
        // Nucleus sets the instance's DPI mode as it starts it.
        set("~ HIGHDPIAWARE");
        running.stop();
        // The user's own Nucleus settings are back too.
        assert_eq!(favourites(), b"True");
        assert_eq!(
            *probe.layers.values.lock().unwrap(),
            BTreeMap::from([(game.to_string(), text("~ RUNASADMIN"))])
        );
        // Nucleus did not start: nothing left behind either.
        *probe.refused.lock().unwrap() = Some("access denied".into());
        let (running, _) = start_with(folder.path(), 1, [false; 4], &self::screen(), &probe);
        assert_eq!(running.err().unwrap().to_string(), "access denied");
        assert_eq!(
            *probe.layers.values.lock().unwrap(),
            BTreeMap::from([(game.to_string(), text("~ RUNASADMIN"))])
        );
        assert_eq!(favourites(), b"True");
        // No journal of the settings is left either.
        let mut files: Vec<_> = std::fs::read_dir(folder.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        files.sort();
        assert_eq!(files, ["Settings.ini", "game profiles"]);
    }

    #[test]
    fn a_session_that_fails_on_its_own_stops_nucleus() {
        let folder = tempfile::tempdir().unwrap();
        let (screen, probe) = (screen(), Probe::default());
        let (running, _) = start_with(folder.path(), 1, [false; 4], &screen, &probe);
        let running = running.unwrap();
        *probe.exit.lock().unwrap() = Some(1);
        wait_for(&running, |p| matches!(p, Phase::Ended(_)));
        drop(running);
        assert!(probe.stopped.lock().unwrap().is_some());
    }

    #[test]
    fn a_running_nucleus_or_too_few_controller_slots_refuse_the_start() {
        let folder = tempfile::tempdir().unwrap();
        let (screen, probe) = (screen(), Probe::default());
        screen
            .0
            .lock()
            .unwrap()
            .processes
            .push(process(42, 1, 1, "nucleuscoop.exe"));
        let error = start_with(folder.path(), 1, [false; 4], &screen, &probe)
            .0
            .err()
            .unwrap()
            .to_string();
        assert!(
            error.starts_with("Nucleus Co-op is already running on the PC (process 42)"),
            "{error}"
        );
        let screen = self::screen();
        let (result, recorder) =
            start_with(folder.path(), 2, [true, true, true, false], &screen, &probe);
        let error = result.err().unwrap().to_string();
        assert!(
            error.starts_with(
                "2 players need 2 free XInput controller slots and the PC's own controllers leave 1"
            ),
            "{error}"
        );
        // Nothing was plugged, written or started.
        assert!(recorder.sent.lock().unwrap().is_empty());
        assert!(!folder.path().join("game profiles").exists());
        assert!(probe.launched.lock().unwrap().is_empty());
    }

    fn state(id: u16, active: u16, buttons: u32) -> Input {
        Input::Controller {
            id,
            active,
            buttons,
            left_trigger: 0,
            right_trigger: 0,
            sticks: [0; 4],
        }
    }

    #[test]
    fn each_device_reaches_its_own_slot_s_pad_only() {
        let folder = tempfile::tempdir().unwrap();
        let (screen, probe) = (screen(), Probe::default());
        let (running, recorder) = start_with(folder.path(), 2, [false; 4], &screen, &probe);
        let running = running.unwrap();
        assert_eq!(
            running.with_session(|s| s.join("laptop", "Laptop", true)),
            Ok(coop::Joined::Seated(1))
        );
        recorder.sent.lock().unwrap().clear();
        assert_eq!(running.input("laptop", state(0, 1, 0x1000)), Ok(()));
        assert_eq!(running.input("phone", state(0, 1, 0x10)), Ok(()));
        assert_eq!(
            *recorder.sent.lock().unwrap(),
            [state(1, 0b11, 0x1000), state(0, 0b11, 0x10)]
        );
        // Keyboards, second controllers, strangers and away players reach nothing.
        let key = Input::Keyboard {
            key: 0x41,
            modifiers: 0,
            flags: 0,
            down: true,
        };
        assert_eq!(running.input("phone", key), Err(Dropped::KeyboardMouse));
        assert_eq!(
            running.input("phone", state(1, 0b11, 0)),
            Err(Dropped::SecondController)
        );
        assert_eq!(running.input("tv", state(0, 1, 0)), Err(Dropped::Ignored));
        running.with_session(|s| s.leave("laptop", Instant::now()));
        assert_eq!(
            running.input("laptop", state(0, 1, 0)),
            Err(Dropped::Ignored)
        );
        assert_eq!(recorder.sent.lock().unwrap().len(), 2);
        // Pad 1's rumble goes to the laptop's first controller.
        recorder.reports.lock().unwrap().push(PadReport::Feedback {
            id: 1,
            kind: 1,
            data: vec![9],
        });
        assert_eq!(
            running.feedback(),
            [(
                1,
                PadReport::Feedback {
                    id: 0,
                    kind: 1,
                    data: vec![9]
                }
            )]
        );
        running.resize(1, (1920, 1080)).unwrap();
        assert_eq!(running.shared.sizes.lock().unwrap()[1], (1920, 1080));
        assert!(running.resize(2, (1920, 1080)).is_err());
    }

    #[test]
    fn audio_is_refused_on_old_builds_and_before_the_instance_runs() {
        let folder = tempfile::tempdir().unwrap();
        let (screen, probe) = (screen(), Probe::default());
        let running = start_with(folder.path(), 1, [false; 4], &screen, &probe)
            .0
            .unwrap();
        let error = running.audio_on(Some(19045), 0).err().unwrap().to_string();
        assert!(
            error.starts_with("Per-player audio needs Windows build 20348"),
            "{error}"
        );
        let error = running.audio_on(Some(26100), 0).err().unwrap().to_string();
        assert_eq!(error, "Player 1's game instance is not running");
    }

    /// The archive downloaded beforehand, for the install's own checks.
    struct Downloaded(std::path::PathBuf);
    impl super::super::install::Fetch for Downloaded {
        fn fetch(&mut self, _url: &str, limit: u64) -> Result<Vec<u8>> {
            let bytes =
                std::fs::read(&self.0).with_context(|| format!("reading {}", self.0.display()))?;
            ensure!(
                bytes.len() as u64 <= limit,
                "{} is too large",
                self.0.display()
            );
            Ok(bytes)
        }
    }

    fn env<T: std::str::FromStr>(name: &str, default: T) -> Result<T>
    where
        T::Err: std::error::Error + Send + Sync + 'static,
    {
        std::env::var(name).map_or(Ok(default), |v| {
            v.parse().with_context(|| format!("{name}={v}"))
        })
    }

    /// A real session on this PC (hw-queue.md, "Nucleus Co-op session"):
    /// Nucleus Co-op installed below `RUBYLIGHT_NUCLEUS_DATA` (from
    /// `RUBYLIGHT_NUCLEUS_ZIP`) or at `RUBYLIGHT_NUCLEUS_PATH`, the game of
    /// handler `RUBYLIGHT_NUCLEUS_HANDLER` started for
    /// `RUBYLIGHT_NUCLEUS_PLAYERS`, held `RUBYLIGHT_NUCLEUS_HOLD` seconds and
    /// stopped; `RUBYLIGHT_NUCLEUS_STEAM_APP` is the Steam app a game was
    /// found from. The user's compatibility settings must be as before
    /// afterwards. Run as SYSTEM, like the service.
    #[test]
    #[ignore = "requires Nucleus Co-op, a handler with its game, the virtual gamepad driver and SYSTEM"]
    fn a_session_on_this_pc() -> Result<()> {
        let data = std::path::PathBuf::from(
            std::env::var("RUBYLIGHT_NUCLEUS_DATA").context("set RUBYLIGHT_NUCLEUS_DATA")?,
        );
        let configured = std::env::var("RUBYLIGHT_NUCLEUS_PATH").unwrap_or_default();
        let folder = match std::env::var("RUBYLIGHT_NUCLEUS_ZIP") {
            Ok(zip) => {
                super::super::install::ensure(&data, &configured, &mut Downloaded(zip.into()))?
            }
            Err(_) => super::super::install::installed(&data, &configured)?
                .context("Nucleus Co-op is not installed: set RUBYLIGHT_NUCLEUS_ZIP")?,
        };
        let steam_app: Option<u32> = std::env::var("RUBYLIGHT_NUCLEUS_STEAM_APP")
            .ok()
            .map(|app| app.parse().context("RUBYLIGHT_NUCLEUS_STEAM_APP"))
            .transpose()?;
        // A hub handler and the game the user added with it. Rubylight's own
        // handler goes through admission: the host's nucleus::tests::
        // a_background_stream_without_a_hub_handler_on_this_pc.
        let file = std::env::var("RUBYLIGHT_NUCLEUS_HANDLER")
            .context("set RUBYLIGHT_NUCLEUS_HANDLER to a hub handler's file")?;
        let path = folder.join("handlers").join(&file);
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let handler =
            butterpollo_core::nucleus::handler::parse(&file, &text).map_err(anyhow::Error::msg)?;
        let exe_path = files::catalog(&folder, true)?
            .games
            .into_iter()
            .find(|game| game.guid == handler.guid)
            .with_context(|| format!("add {}'s game in Nucleus Co-op first", handler.name))?
            .exe_path;
        println!("handler {} for {}", handler.file, handler.executable);
        let layers_before = layers::UserHive::signed_in()?.values()?;
        let players = env("RUBYLIGHT_NUCLEUS_PLAYERS", 1usize)?;
        let hold = Duration::from_secs(env("RUBYLIGHT_NUCLEUS_HOLD", 20u64)?);
        let since = crate::process::now();
        let warnings = Arc::new(butterpollo_core::session::Warnings::default());
        let request = Start {
            id: 1,
            kind: kind(players),
            folder: &folder,
            handler: &handler,
            exe_path: &exe_path,
            steam_app,
            title: &handler.name,
            players,
            owner: ("hw-queue", "hw-queue"),
            size: (1280, 720),
        };
        let policy = butterpollo_core::input_policy::Policy::resolve(&Default::default())?;
        let running = start_on_pc(&request, policy, warnings.clone())?;
        println!(
            "pads {:?}",
            running.with_session(|s| s.slots.iter().map(|s| s.pad).collect::<Vec<_>>())
        );
        let started = Instant::now();
        loop {
            match running.with_session(|s| s.phase.clone()) {
                Phase::Running => break,
                Phase::Ended(why) => bail!("{why}"),
                Phase::Starting => std::thread::sleep(PERIOD),
            }
        }
        println!(
            "running after {:?}, windows {:?}",
            started.elapsed(),
            running.windows()
        );
        // Each parked window streams through WGC, on the primary display's GPU.
        let display = crate::capture::primary_display().context("no display")?;
        let _com = crate::capture::ComGuard::new()?;
        for slot in 0..players {
            let target = running.capture_target(slot, &display)?;
            let mut capture = crate::capture::Capture::open_for_stream_reported(
                &target,
                "wgc",
                false,
                &Default::default(),
                warnings.clone(),
            )?;
            let deadline = Instant::now() + Duration::from_secs(5);
            let image = loop {
                if let Some(image) = capture.next_gpu()? {
                    break image;
                }
                ensure!(
                    Instant::now() < deadline,
                    "Player {}'s window gave no frame within 5 s",
                    slot + 1
                );
                std::thread::sleep(Duration::from_millis(5));
            };
            println!(
                "player {} capture {target}: {} {}x{}",
                slot + 1,
                capture.backend(),
                image.width,
                image.height
            );
        }
        for slot in 0..players {
            match running.audio(slot) {
                Ok(audio) => println!("player {} audio pid {}", slot + 1, audio.identity.pid),
                Err(error) => println!("player {} audio: {error:#}", slot + 1),
            }
        }
        // The first player's pad: A held, then released, through its slot.
        for buttons in [0x1000, 0] {
            running
                .input("hw-queue", state(0, 1, buttons))
                .map_err(|dropped| anyhow::anyhow!("the pad input was dropped: {dropped:?}"))?;
            std::thread::sleep(Duration::from_millis(200));
        }
        println!("pad reports {:?}", running.pads.reports());
        std::thread::sleep(hold);
        let phase = running.with_session(|s| s.phase.clone());
        ensure!(phase == Phase::Running, "after {hold:?}: {phase:?}");
        let stopping = Instant::now();
        running.stop();
        println!("stopped in {:?}", stopping.elapsed());
        for warning in warnings.snapshot() {
            println!("warning {}: {}", warning.code, warning.message);
        }
        let left: Vec<String> = Win32::default()
            .processes()?
            .into_iter()
            .filter(|p| p.started >= since)
            .filter(|p| {
                p.name.eq_ignore_ascii_case(EXECUTABLE)
                    || p.name.eq_ignore_ascii_case(&handler.executable)
            })
            .map(|p| format!("{} {}", p.name, p.pid))
            .collect();
        ensure!(left.is_empty(), "still running after the stop: {left:?}");
        let layers_after = layers::UserHive::signed_in()?.values()?;
        ensure!(
            layers_after == layers_before,
            "the compatibility settings changed: {layers_before:?} before, {layers_after:?} after"
        );
        println!("compatibility settings as before");
        Ok(())
    }
}
