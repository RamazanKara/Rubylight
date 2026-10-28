//! A Nucleus session on the PC: Nucleus Co-op started for one game with the
//! host's profile, each player's instance found and parked beside the
//! desktop, each player's pad and audio, and all of it stopped again. A
//! failure ends the session with the reason the stream and the console
//! show; nothing falls back to another mode.
use super::desktop::Win32;
use super::files;
use super::instances::{Desktop, Keeper};
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
use std::path::Path;
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
    fn launch(&mut self, folder: &Path, guid: &str) -> Result<Box<dyn Nucleus>>;
}

/// Nucleus Co-op as the signed-in user, elevated: it makes symbolic links.
pub struct Elevated;
impl Launcher for Elevated {
    fn launch(&mut self, folder: &Path, guid: &str) -> Result<Box<dyn Nucleus>> {
        let since = crate::process::now();
        let program = folder.join(EXECUTABLE);
        // Nucleus reads Settings.ini from its working folder, and its window
        // must be visible for the profile click.
        let process = crate::process::Process::spawn(
            &program,
            &[guid.into()],
            Some(folder),
            crate::process::Target::User { elevated: true },
            &BTreeMap::new(),
            false,
        )
        .with_context(|| format!("starting {} elevated", program.display()))?;
        tracing::info!(pid = process.pid, guid, "Nucleus Co-op started");
        Ok(Box::new(Started { process, since }))
    }
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
    /// Clicked, instances found, windows found.
    progress: (bool, usize, usize),
    windows: Vec<Option<u64>>,
    deadline: Instant,
    exited: bool,
    failed: u32,
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
            Ok(None) => return None,
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
            }
        }
        None
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
            (Some(why), false, 0) => format!("Nucleus Co-op did not start {}: {why}", self.title),
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
    /// The game as the stream names it.
    pub title: &'a str,
    pub players: usize,
    /// The first player's device id and name.
    pub owner: (&'a str, &'a str),
    /// The first player's stream size.
    pub size: (u32, u32),
}

/// A session that runs until [`stop`](Running::stop) or drop.
pub struct Running {
    shared: Arc<Shared>,
    pads: Box<dyn Pads>,
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
    players: usize,
}

/// Start a session on the PC: virtual Xbox pads, XInput, the user's desktop
/// and Nucleus Co-op elevated.
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
        &mut Elevated,
    )
}

/// Start a session: refused while Nucleus Co-op already runs, then one pad
/// per player, the profile, and Nucleus.
pub fn start(
    start: &Start,
    pads: Box<dyn Pads>,
    xinput: &mut dyn XInput,
    mut desktop: Box<dyn Desktop>,
    launcher: &mut dyn Launcher,
) -> Result<Running> {
    // Its profile list would show the user's state, and its instances could
    // not be told from the session's.
    if let Some(process) = desktop
        .processes()?
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(EXECUTABLE))
    {
        bail!(
            "Nucleus Co-op is already running on the PC (process {}): close it, then start the stream again",
            process.pid
        );
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
    files::write_settings(start.folder)?;
    let number = files::write_profile(start.folder, &start.handler.guid, &players, &screens)?;
    tracing::info!(
        number,
        game = start.handler.guid,
        players = start.players,
        "Nucleus Co-op profile {TITLE} written"
    );
    let nucleus = launcher.launch(start.folder, &start.handler.guid)?;
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
        progress: (false, 0, 0),
        windows: vec![None; start.players],
        deadline: Instant::now() + START_TIMEOUT,
        exited: false,
        failed: 0,
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
        stop: Some(stop),
        thread: Some(thread),
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

    /// End the session and wait until Nucleus and the instances closed.
    pub fn stop(&mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take()
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
    }

    const NUCLEUS: u32 = 10;
    const SINCE: u64 = 100;

    #[derive(Clone, Default)]
    struct Probe {
        exit: Arc<Mutex<Option<u32>>>,
        stopped: Arc<Mutex<Option<BTreeMap<u32, u64>>>>,
        launched: Arc<Mutex<Vec<String>>>,
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
        fn launch(&mut self, _folder: &Path, guid: &str) -> Result<Box<dyn Nucleus>> {
            self.0.launched.lock().unwrap().push(guid.into());
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
            progress: (false, 0, 0),
            windows: vec![None; players],
            deadline: t0 + START_TIMEOUT,
            exited: false,
            failed: 0,
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
        assert_eq!(
            end,
            End::Failed(format!("Nucleus Co-op did not start Game; {HINT}"))
        );
        let shared = runner.shared.clone();
        runner.finish(end.clone());
        assert_eq!(shared.session.lock().unwrap().phase, Phase::Ended(end));
        assert_eq!(*probe.stopped.lock().unwrap(), Some(BTreeMap::new()));
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
        let mut running = running.unwrap();
        let error = running.capture_target(0, r"\\.\DISPLAY1").unwrap_err();
        assert_eq!(error.to_string(), "Player 1's game window is not open");
        // One pad per player, skipping the PC's own at index 1.
        assert_eq!(
            running.with_session(|s| s.slots.iter().map(|s| s.pad).collect::<Vec<_>>()),
            [0, 2]
        );
        assert_eq!(recorder.sent.lock().unwrap().len(), 2);
        assert_eq!(*probe.launched.lock().unwrap(), ["Game"]);
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
    /// stopped. Run as SYSTEM, like the service.
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
        let file =
            std::env::var("RUBYLIGHT_NUCLEUS_HANDLER").context("set RUBYLIGHT_NUCLEUS_HANDLER")?;
        let path = folder.join("handlers").join(&file);
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let handler =
            butterpollo_core::nucleus::handler::parse(&file, &text).map_err(anyhow::Error::msg)?;
        let players = env("RUBYLIGHT_NUCLEUS_PLAYERS", 1usize)?;
        let hold = Duration::from_secs(env("RUBYLIGHT_NUCLEUS_HOLD", 20u64)?);
        let since = crate::process::now();
        let warnings = Arc::new(butterpollo_core::session::Warnings::default());
        let request = Start {
            id: 1,
            kind: kind(players),
            folder: &folder,
            handler: &handler,
            title: &handler.name,
            players,
            owner: ("hw-queue", "hw-queue"),
            size: (1280, 720),
        };
        let policy = butterpollo_core::input_policy::Policy::resolve(&Default::default())?;
        let mut running = start_on_pc(&request, policy, warnings.clone())?;
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
        Ok(())
    }
}
