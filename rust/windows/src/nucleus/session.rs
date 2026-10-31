//! A Nucleus session on the PC: Nucleus Co-op started for one game with the
//! host's profile, each player's instance found and parked beside the
//! desktop, each player's pad and audio, and all of it stopped again. A
//! failure ends the session with the reason the stream and the console
//! show; nothing falls back to another mode.
use super::desktop::Win32;
use super::instances::{Desktop, Keeper, MessageBox};
use super::layers::{self, Guard, Layers};
use super::pads::{self, Dropped, Pads, XInput};
use super::{files, user_copy};
use crate::input::PadReport;
use anyhow::{Context, Result, bail, ensure};
use butterpollo_core::audio_scope::Identity;
use butterpollo_core::input::Input;
use butterpollo_core::nucleus::coop::{self, End, Phase};
use butterpollo_core::nucleus::elevation::{Account, Refused, Run};
use butterpollo_core::nucleus::generic::{self, Inject};
use butterpollo_core::nucleus::handler::Handler;
use butterpollo_core::nucleus::install::EXECUTABLE;
use butterpollo_core::nucleus::profile::{Player, TITLE};
use butterpollo_core::nucleus::window;
use std::collections::{BTreeMap, BTreeSet};
use std::os::windows::io::OwnedHandle;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

mod launch;

pub use launch::{
    Elevated, Opened, Unelevated, already_running, needs_administrator, open, running,
    steam_environment,
};

use launch::*;

/// How long Nucleus may take to show the instances, counted again from
/// each step it makes (the profile clicked, an instance, a window).
pub const START_TIMEOUT: Duration = Duration::from_secs(60);
/// How often the session looks at the desktop.
const PERIOD: Duration = Duration::from_millis(250);
/// How long after a click on the profile it is clicked again while Nucleus
/// still lists it: a loaded profile with every player's controller plays at
/// once and hides the list within about a second, and a click before its
/// setup screen is made fails.
const RECLICK: Duration = Duration::from_secs(5);
/// Passes in a row that may fail (the desktop switching for a UAC prompt)
/// before the session ends with the error.
const FAILED_PASSES: u32 = 8;
/// How long an instance of Rubylight's own handler's game that exited
/// before the session runs waits for Nucleus' box saying the game refused
/// its hooks: one Nucleus fails to inject into as it starts it is ended
/// before the box shows (#59).
const REFUSAL_GRACE: Duration = Duration::from_secs(2);
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

/// Starts Nucleus Co-op: elevated or as the user on the PC, a fake in tests.
pub trait Launcher {
    /// Start it for game `guid` with `environment` added to the user's: the
    /// game it starts gets the same.
    fn launch(
        &mut self,
        folder: &Path,
        guid: &str,
        environment: &BTreeMap<String, String>,
    ) -> Result<Box<dyn Nucleus>>;
    /// Run `work`, which writes Nucleus' settings and profile, as the
    /// account whose copy it writes to: the host for its own copy, the user
    /// for theirs.
    fn write(&self, work: &mut dyn FnMut() -> Result<()>) -> Result<()> {
        work()
    }
    /// Run `work` on the profile folder of the user Nucleus runs as, as
    /// that user: Nucleus keeps its per-game settings there
    /// ([`files::index_pads`]). Nothing runs for a launcher that has none.
    fn in_profile(&self, _work: &mut dyn FnMut(&Path) -> Result<()>) -> Result<()> {
        Ok(())
    }
    /// The account the folder it runs from is read as while the session
    /// runs: None for the host's own, the user for their copy.
    fn reader(&self) -> Result<Option<OwnedHandle>> {
        Ok(None)
    }
}

/// Nucleus' `crashlog.txt` in the folder it runs from, where it appends
/// each exception it catches (`[time] exception`, then its stack) and goes
/// on, as it was when the session launched it. It is read as the account
/// that reads that folder: the user for their own copy, which is theirs to
/// change, so the host never reads through a link they made there.
struct Crashlog {
    path: PathBuf,
    /// Its length at the launch: what follows is this session's.
    from: u64,
}

impl Crashlog {
    /// The longest line quoted.
    const LIMIT: usize = 300;
    /// The most read, from its end: the newest exception is there, and a
    /// file made huge is never read whole.
    const TAIL: u64 = 64 * 1024;

    /// As it is at the launch, read as `reader` (None: as the host).
    fn at_launch(folder: &Path, reader: Option<&OwnedHandle>) -> Crashlog {
        let path = folder.join("crashlog.txt");
        let from = Self::read(reader, &path, || std::fs::metadata(&path).map(|m| m.len()));
        Crashlog {
            from: from.unwrap_or(0),
            path,
        }
    }

    /// `work` on the file at `path` as `reader`: None when there is no
    /// file, or with a warning when it could not be read.
    fn read<T>(
        reader: Option<&OwnedHandle>,
        path: &Path,
        work: impl FnOnce() -> std::io::Result<T>,
    ) -> Option<T> {
        let why = match super::user_copy::as_user(reader, work) {
            Ok(Ok(value)) => return Some(value),
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => return None,
            Ok(Err(error)) => error.to_string(),
            Err(error) => format!("{error:#}"),
        };
        tracing::warn!(error = why, path = %path.display(), "Nucleus Co-op's crashlog not read");
        None
    }

    /// What was written since the launch, at most [`Self::TAIL`] from its
    /// end; a shorter file was started again since, and counts whole.
    fn since_launch(&self) -> std::io::Result<Vec<u8>> {
        use std::io::{Read, Seek, SeekFrom};
        let mut file = std::fs::File::open(&self.path)?;
        let length = file.metadata()?.len();
        let from = if self.from <= length { self.from } else { 0 };
        file.seek(SeekFrom::Start(from.max(length.saturating_sub(Self::TAIL))))?;
        let mut bytes = Vec::new();
        file.take(Self::TAIL).read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    /// The newest exception written since the launch, read as `reader`:
    /// its first line.
    fn newest(&self, reader: Option<&OwnedHandle>) -> Option<String> {
        let bytes = Self::read(reader, &self.path, || self.since_launch())?;
        let text = String::from_utf8_lossy(&bytes);
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
    /// When the profile was last clicked.
    clicked_at: Option<Instant>,
    /// Whether Nucleus hid its profile list after a click: it plays then,
    /// and the profile is never clicked again.
    hid: bool,
    /// The forms Nucleus showed over its window after a click, closed;
    /// whether one was closed since the last click; the last one.
    closed: BTreeSet<u64>,
    boxed: bool,
    last_form: Option<MessageBox>,
    /// Why the last click on the profile was refused.
    unclicked: Option<String>,
    /// While the profile is not shown: Nucleus' message boxes at the last
    /// look, every one logged so far, whether it showed any window, and why
    /// the last look failed. A box Windows showed for another program
    /// before the launch (`before`, its windows) is not Nucleus'.
    showing: Vec<MessageBox>,
    logged: Vec<MessageBox>,
    before: BTreeSet<u64>,
    shown: bool,
    unseen: Option<String>,
    /// Nucleus' crashlog.txt as it was at the launch. None for a runner
    /// tested alone.
    crashlog: Option<Crashlog>,
    /// The folder Nucleus Co-op runs from and the game's GUID, to tell
    /// whether Nucleus lists the game, read as `reader` (the user, for their
    /// own copy). None for a runner tested alone, and for a Nucleus Co-op
    /// whose game list Rubylight doesn't know.
    game: Option<(PathBuf, String)>,
    reader: Option<OwnedHandle>,
    /// Clicked, instances found, windows found.
    progress: (bool, usize, usize),
    windows: Vec<Option<u64>>,
    deadline: Instant,
    exited: bool,
    failed: u32,
    /// The game's folder: walked again after a failed start
    /// ([`super::elevation::forget`]).
    game_folder: Option<PathBuf>,
    /// For Rubylight's own handler: the host's Nucleus folder and the
    /// game's .exe, kept there when the game refuses ProtoInput's hooks
    /// ([`files::refuse_inject`]).
    own_game: Option<(PathBuf, String)>,
    /// Until when Nucleus may still show that box: it injects once, as it
    /// starts the game or right after. Set at the first window.
    inject_by: Option<Instant>,
    /// An instance that exited while that box may still come, and when
    /// ([`REFUSAL_GRACE`]).
    exited_at: Option<(usize, Instant)>,
    /// The user's Nucleus settings the session changed (None from the
    /// user's own copy, whose stay), then the game's compatibility settings
    /// from before Nucleus started; last, so they drop after Nucleus. None
    /// for a runner tested alone.
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
            && let Some(end) = self.click(&job, now)
        {
            return Some(end);
        }
        let sizes = self.shared.sizes.lock().unwrap().clone();
        let pass = match self.keeper.pass(self.desktop.as_mut(), &job, &sizes) {
            Ok(pass) => pass,
            Err(error) => return self.failed_pass(error),
        };
        let windows = pass.windows.iter().flatten().count();
        if windows > 0 && self.inject_by.is_none() {
            self.inject_by = Some(now + START_TIMEOUT);
        }
        let injecting = self.inject_by.is_none_or(|by| now < by);
        // From the click: a game Nucleus injects into as it starts it is
        // refused before its process runs, and ended before the box shows.
        if self.clicked
            && injecting
            && let Some(end) = self.refused_hooks(&job)
        {
            return Some(end);
        }
        if let Some(&slot) = pass.gone.first() {
            let running = self.shared.session.lock().unwrap().phase == Phase::Running;
            if self.own_game.is_some() && injecting && !running {
                self.exited_at.get_or_insert((slot, now));
            } else {
                return Some(self.exited(slot));
            }
        }
        if let Some((slot, at)) = self.exited_at
            && now >= at + REFUSAL_GRACE
        {
            return Some(self.exited(slot));
        }
        *self.shared.roots.lock().unwrap() = pass.roots.clone();
        let roots = pass.roots.iter().flatten().count();
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
        if !running && roots == 0 && self.clicked {
            // A click that comes while Nucleus 2.4.2 still makes its setup
            // screen fails to load the profile, and its box saying so
            // disables the window, so no later click lands (#59). Nobody
            // answers it during a background stream: it is closed, and the
            // profile clicked again.
            match self.desktop.close_forms(&job) {
                Ok(closed) => {
                    for form in closed {
                        // One that takes a few passes to close is told once.
                        if self.closed.insert(form.window) {
                            tracing::warn!(
                                "Nucleus Co-op showed \"{}\" over its window after the profile {TITLE} was clicked; closed it to click the profile again",
                                form.summary()
                            );
                        }
                        self.last_form = Some(form);
                        self.boxed = true;
                    }
                }
                Err(error) => {
                    return self.failed_pass(error.context("closing Nucleus Co-op's windows"));
                }
            }
            // A box Nucleus shows after the click (its End-key reminder, a
            // check of the game) holds the start as one before it does.
            self.look(&job);
            // Nucleus hides the list when it plays the profile: a list that
            // comes back means no second click, which could start the game
            // again.
            if !self.hid {
                match self.desktop.label(&job, TITLE) {
                    Ok(Some(label)) => {
                        // Not as the start times out: what the last click
                        // led to says why.
                        if self.showing.is_empty()
                            && self.clicked_at.is_some_and(|at| now >= at + RECLICK)
                            && now < self.deadline
                        {
                            // Still no instance: clicked again.
                            self.press(&job, label, now);
                        }
                    }
                    Ok(None) => self.hid = true,
                    Err(error) => {
                        return self.failed_pass(
                            error.context("looking for Nucleus Co-op's profile list"),
                        );
                    }
                }
            }
        }
        self.failed = 0;
        if !running && now >= self.deadline {
            return Some(End::Failed(self.timeout()));
        }
        None
    }

    /// Rubylight's own handler's game refused ProtoInput's hooks: Nucleus
    /// shows [`generic::INJECT_FAILED`] as it starts the game or once the
    /// game runs, and ends it when the box is answered, which nobody does
    /// during a background stream (#59). The game is kept so its next
    /// handler injects them the next way ([`generic::Inject`]), or none
    /// after the last, and the session ends saying so.
    fn refused_hooks(&mut self, job: &BTreeSet<u32>) -> Option<End> {
        self.own_game.as_ref()?;
        let boxes = match self.desktop.message_boxes(job) {
            Ok(boxes) => boxes,
            Err(error) => {
                let why = format!("{error:#}");
                if self.unseen.as_ref() != Some(&why) {
                    tracing::warn!(
                        why,
                        "the PC's message boxes not listed, so a game refusing Nucleus Co-op's input hooks is not seen"
                    );
                }
                self.unseen = Some(why);
                return None;
            }
        };
        let shown = boxes.iter().find(|shown| {
            !self.before.contains(&shown.window)
                && generic::INJECT_FAILED
                    .iter()
                    .any(|failed| shown.text.contains(failed))
        })?;
        let said = shown.summary();
        let (folder, exe) = self.own_game.as_ref()?;
        let why = match files::refuse_inject(folder, exe) {
            Ok(Inject::None) => format!(
                "{} refused Nucleus Co-op's input hooks (it showed \"{said}\"), and Nucleus Co-op ends a game it can't hook: start the stream again, and Rubylight starts it without them",
                self.title
            ),
            Ok(_) => format!(
                "{} refused Nucleus Co-op's input hooks (it showed \"{said}\"), and Nucleus Co-op ends a game it can't hook: start the stream again, and Rubylight has Nucleus Co-op hook it another way",
                self.title
            ),
            Err(error) => format!(
                "{} refused Nucleus Co-op's input hooks (it showed \"{said}\"), and Nucleus Co-op ends a game it can't hook; Rubylight could not keep that for its next start: {error:#}",
                self.title
            ),
        };
        Some(self.failure(why))
    }

    /// Player `slot`'s instance exited.
    fn exited(&mut self, slot: usize) -> End {
        let why = format!("Player {}'s instance of {} exited", slot + 1, self.title);
        self.failure(why)
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
    fn click(&mut self, job: &BTreeSet<u32>, now: Instant) -> Option<End> {
        let label = match self.desktop.label(job, TITLE) {
            Ok(Some(label)) => label,
            Ok(None) => {
                self.look(job);
                return None;
            }
            Err(error) => return Some(End::Failed(format!("{error:#}"))),
        };
        self.press(job, label, now);
        None
    }

    /// Click the profile's entry in Nucleus' list.
    fn press(&mut self, job: &BTreeSet<u32>, label: u64, now: Instant) {
        match self.desktop.click(label) {
            Ok(()) => {
                if self.clicked {
                    tracing::info!(
                        "Nucleus Co-op profile {TITLE} clicked again: it still lists it"
                    );
                } else {
                    tracing::info!("Nucleus Co-op profile {TITLE} clicked");
                }
                self.clicked = true;
                self.clicked_at = Some(now);
                self.boxed = false;
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
    }

    /// What Nucleus shows instead of the profile: a message box it shows
    /// before its window (its folder check, a missing runtime) waits for an
    /// answer no one gives during a background stream (#59).
    fn look(&mut self, job: &BTreeSet<u32>) {
        let looked = match self.desktop.message_boxes(job) {
            Ok(mut boxes) => {
                boxes.retain(|shown| !self.before.contains(&shown.window));
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
        self.quoting_crashlog(why)
    }

    /// Why Nucleus did not start the game after the profile was clicked.
    /// Nucleus plays a loaded profile by itself once each of its players
    /// has a controller (`GameProfile.FindProfilePlayers`), and hides the
    /// profile list then.
    /// What came after the last click says why: a form closed since it, or
    /// else whether the list is still there.
    fn unstarted(&self) -> String {
        let boxed = self.last_form.as_ref().filter(|_| self.boxed);
        let why = if let Some(form) = boxed {
            let said = match form.summary() {
                summary if summary.is_empty() => "a window".to_string(),
                summary => format!("\"{summary}\""),
            };
            format!(
                "Nucleus Co-op did not start {}: after the last click on the profile {TITLE} it showed {said} over its window ({} closed since the launch)",
                self.title,
                self.closed.len()
            )
        } else if !self.hid {
            format!(
                "Nucleus Co-op did not start {}: it still shows its setup screen with the profile {TITLE}, which starts the game once every player has a controller there",
                self.title
            )
        } else {
            format!(
                "Nucleus Co-op did not start {} after loading the profile {TITLE}",
                self.title
            )
        };
        self.quoting_crashlog(why)
    }

    /// `why`, with the newest exception of Nucleus' crashlog.txt since the
    /// launch.
    fn quoting_crashlog(&self, why: String) -> String {
        let reader = self.reader.as_ref();
        match self.crashlog.as_ref().and_then(|log| log.newest(reader)) {
            Some(line) => format!("{why}. Its crashlog.txt says: {line}"),
            None => why,
        }
    }

    /// Why Nucleus Co-op shows no entry for the game, when it doesn't: no
    /// profile of a game it doesn't list ever appears (#59).
    fn unlisted(&self) -> Option<String> {
        let (folder, guid) = self.game.as_ref()?;
        let list = files::list(folder);
        let listed =
            super::user_copy::as_user(self.reader.as_ref(), || files::listed_by_nucleus(folder))
                .and_then(|listed| listed);
        match listed {
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
        match (&self.unclicked, clicked, roots) {
            // The box is why: it covers the list or holds the start.
            (_, _, 0) if !self.showing.is_empty() => self.unshown(),
            (Some(why), false, 0) => format!("Nucleus Co-op did not start {}: {why}", self.title),
            (None, false, 0) => self.unshown(),
            (_, _, 0) => self.unstarted(),
            _ if roots < self.players => self.quoting_crashlog(format!(
                "Nucleus Co-op started {roots} of {} instances of {}",
                self.players, self.title
            )),
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
                tracing::warn!(title = self.title, "Nucleus Co-op session ended: {why}");
                if let (End::Failed(_), Some(game)) = (why, &self.game_folder) {
                    super::elevation::forget(game);
                }
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
    /// The host's Nucleus Co-op folder, which keeps the compatibility
    /// settings' file.
    pub folder: &'a Path,
    /// The signed-in user's own copy ([`super::user_copy`]) when the
    /// session runs it as them without administrator rights; None runs
    /// `folder` elevated.
    pub user_copy: Option<&'a Path>,
    /// Whether it is the release whose game list Rubylight knows
    /// ([`files::list`]): the host's own install of 2.4.2 (or the user's
    /// copy of it), not a `nucleus_path` copy. Only then does a start that
    /// times out say whether the game is listed.
    pub known_list: bool,
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
    /// The crashlog.txt a start that times out quotes.
    #[cfg(test)]
    crashlog: PathBuf,
}

/// The stream's warning for a game Rubylight's handler starts without
/// ProtoInput's hooks ([`unhooked`]).
pub const UNHOOKED: &str = "nucleus_unhooked";

/// What the player should know when Rubylight's own handler starts the game
/// without ProtoInput's hooks, because it refused every way (#59).
fn unhooked(start: &Start) -> Result<Option<String>> {
    if !start.handler.own || files::inject_for(start.folder, start.exe_path)? != Inject::None {
        return Ok(None);
    }
    Ok(Some(format!(
        "{} refused every way Nucleus Co-op injects its input hooks, so it runs without them: a keyboard, mouse or pad on the PC reaches it too",
        start.title
    )))
}

/// Start a session on the PC: virtual Xbox pads, XInput, the user's desktop
/// and compatibility settings, and Nucleus Co-op, elevated or from the
/// user's own copy.
pub fn start_on_pc(
    start: &Start,
    policy: butterpollo_core::input_policy::Policy,
    warnings: Arc<butterpollo_core::session::Warnings>,
) -> Result<Running> {
    let mut launcher: Box<dyn Launcher> = match start.user_copy {
        Some(_) => Box::new(Unelevated::signed_in()?),
        None => Box::new(Elevated),
    };
    if let Some(why) = unhooked(start)? {
        warnings.set(UNHOOKED, why);
    }
    let pads = pads::virtual_pads(policy, warnings)?;
    let mut xinput = pads::System::open()?;
    self::start(
        start,
        Box::new(pads),
        &mut xinput,
        Box::new(Win32::default()),
        Box::new(layers::UserHive::signed_in()?),
        launcher.as_mut(),
    )
}

/// The folder of the game at `exe`, which [`super::elevation::folder_need`]
/// walks.
fn game_folder(exe: &str) -> Option<PathBuf> {
    Path::new(exe).parent().map(Path::to_path_buf)
}

/// What starting a session needs that doesn't need Nucleus Co-op closed:
/// administrator rights for the signed-in user when it runs elevated, their
/// AppData folder and their account, and the compatibility settings an
/// earlier session left in the host's `folder` put back. Checked before a
/// session closes the console's window, so a launch refused for one of
/// these leaves the window open.
pub fn ready(folder: &Path, run: Run) -> Result<()> {
    if run == Run::Elevated {
        require_administrator(crate::process::user_account()?, Refused::Game)?;
    }
    environment(&BTreeMap::new())?;
    layers::restore_left_in(&mut layers::UserHive::signed_in()?, folder)?;
    Ok(())
}

/// Start a session: refused while Nucleus Co-op already runs, then one pad
/// per player, the profile, the game's compatibility settings kept
/// ([`layers`]), and Nucleus. Should it fail, the game's folder is walked
/// again next time ([`super::elevation::forget`]).
pub fn start(
    start: &Start,
    pads: Box<dyn Pads>,
    xinput: &mut dyn XInput,
    desktop: Box<dyn Desktop>,
    layers: Box<dyn Layers>,
    launcher: &mut dyn Launcher,
) -> Result<Running> {
    begin(start, pads, xinput, desktop, layers, launcher).inspect_err(|_| {
        if let Some(game) = game_folder(start.exe_path) {
            super::elevation::forget(&game);
        }
    })
}

fn begin(
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
    // The copy Nucleus runs from, written as the account it runs as. The
    // host's own settings are put back when the session ends, or here should
    // Nucleus not start; the user's copy keeps the session's, and nothing in
    // it is written as the host.
    let folder = start.user_copy.unwrap_or(start.folder);
    let settings = match start.user_copy {
        None => Some(files::write_settings(start.folder)?),
        Some(_) => None,
    };
    let mut number = 0;
    launcher.write(&mut || {
        if start.user_copy.is_some() {
            files::write_copy_settings(folder)?;
        }
        number = files::write_profile(folder, &start.handler.guid, &players, &screens)?;
        Ok(())
    })?;
    // Nucleus takes the pads' assignment from there, not from the profile:
    // by button press, it would wait for a press no one gives (#56).
    launcher
        .in_profile(&mut |profile| {
            files::index_pads(&files::meta_info(profile), &start.handler.guid)
        })
        .context("giving the game its pads by XInput index in Nucleus Co-op's per-game settings")?;
    tracing::info!(
        number,
        game = start.handler.guid,
        players = start.players,
        folder = %folder.display(),
        "Nucleus Co-op profile {TITLE} written"
    );
    // Restored when the session ends, or here should Nucleus not start.
    let layers = Guard::take(
        layers,
        start.folder,
        folder,
        &start.handler.executable,
        start.exe_path,
    )
    .context("keeping the game's compatibility settings, which Nucleus Co-op changes")?;
    // In the folder Nucleus runs from, read as the account that reads it.
    let reader = launcher.reader()?;
    let crashlog = Crashlog::at_launch(folder, reader.as_ref());
    #[cfg(test)]
    let crashlog_path = crashlog.path.clone();
    // Windows' boxes up now are another program's: Nucleus isn't running.
    let before = desktop
        .message_boxes(&BTreeSet::new())
        .context("listing the message boxes on the PC's screen before Nucleus Co-op starts")?
        .into_iter()
        .map(|shown| shown.window)
        .collect();
    let nucleus = launcher.launch(
        folder,
        &start.handler.guid,
        &steam_environment(start.steam_app),
    )?;
    let content = folder.join("content");
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
        clicked_at: None,
        hid: false,
        closed: BTreeSet::new(),
        boxed: false,
        last_form: None,
        unclicked: None,
        showing: Vec::new(),
        logged: Vec::new(),
        before,
        shown: false,
        unseen: None,
        crashlog: Some(crashlog),
        game: start
            .known_list
            .then(|| (folder.to_path_buf(), start.handler.guid.clone())),
        reader,
        progress: (false, 0, 0),
        windows: vec![None; start.players],
        deadline: Instant::now() + START_TIMEOUT,
        exited: false,
        failed: 0,
        game_folder: game_folder(start.exe_path),
        own_game: start
            .handler
            .own
            .then(|| (start.folder.to_path_buf(), start.exe_path.to_string())),
        inject_by: None,
        exited_at: None,
        settings,
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
        #[cfg(test)]
        crashlog: crashlog_path,
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
mod tests;
