//! Which process tree is the game Steam launched.
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// A process as the Steam tracker sees it.
#[derive(Clone, Debug)]
pub struct Process {
    pub pid: u32,
    pub parent: u32,
    /// Creation time, which tells a reused process ID apart.
    pub started: u64,
    pub name: String,
}
/// What the tracker knows about a game started through Steam.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tracked {
    /// No game process yet.
    Waiting,
    Running,
    /// Its processes have all exited.
    Exited,
    /// None appeared in time; the stream stays until the user ends it.
    Unknown,
}
/// Finds the processes of a game Steam starts: new processes whose program
/// lies in the game's install folder, and their children.
pub struct Tracker {
    baseline: HashMap<u32, u64>,
    install_dir: String,
    deadline: std::time::Instant,
    /// pid → creation time.
    pub tracked: BTreeMap<u32, u64>,
    checked: BTreeSet<(u32, u64)>,
    seen: bool,
    /// Since when no tracked process is left, and how long that may last:
    /// a game that restarts itself through Steam has none for a moment.
    empty_since: Option<std::time::Instant>,
    pub(super) grace: std::time::Duration,
}
/// Children a game starts that are not the game: a browser it opens, a store
/// client. Following them kept the app running after the game, and quitting
/// the app closed them.
const NOT_THE_GAME: &[&str] = &[
    "chrome.exe",
    "msedge.exe",
    "firefox.exe",
    "opera.exe",
    "brave.exe",
    "iexplore.exe",
    "upc.exe",
    "ubisoftconnect.exe",
    "epicgameslauncher.exe",
    "eadesktop.exe",
    "battle.net.exe",
    "battle.net launcher.exe",
    "agent.exe",
    "ealauncher.exe",
    "eabackgroundservice.exe",
    "galaxyclientservice.exe",
    "galaxycommunication.exe",
    "upcwebcore.exe",
    "epicwebhelper.exe",
    "origin.exe",
    "galaxyclient.exe",
    "werfault.exe",
];
const STEAM_PROCESSES: &[&str] = &[
    "steam.exe",
    "steamwebhelper.exe",
    "steamservice.exe",
    "steamerrorreporter.exe",
    "steamerrorreporter64.exe",
    "gameoverlayui.exe",
    "gameoverlayui64.exe",
];
impl Tracker {
    /// `before` is the process list taken just before the launch.
    pub fn new(before: &[Process], install_dir: &str, wait: std::time::Duration) -> Self {
        Self {
            baseline: before.iter().map(|p| (p.pid, p.started)).collect(),
            install_dir: normalized(install_dir),
            deadline: std::time::Instant::now() + wait,
            tracked: BTreeMap::new(),
            checked: BTreeSet::new(),
            seen: false,
            empty_since: None,
            grace: std::time::Duration::from_secs(10),
        }
    }
    /// How long the game may have no process before it counts as exited.
    pub fn with_exit_grace(mut self, grace: std::time::Duration) -> Self {
        self.grace = grace;
        self
    }
    /// The processes in `now` whose program path the next [`Self::update`]
    /// may ask for: new ones that are not Steam's and not looked at yet. A
    /// caller reads their paths before it takes whatever guards the tracker.
    pub fn unchecked(&self, now: &[Process]) -> Vec<u32> {
        now.iter()
            .filter(|process| {
                let name = process.name.to_ascii_lowercase();
                self.tracked.get(&process.pid) != Some(&process.started)
                    && self.baseline.get(&process.pid) != Some(&process.started)
                    && !STEAM_PROCESSES.contains(&name.as_str())
                    && !NOT_THE_GAME.contains(&name.as_str())
                    && !self.checked.contains(&(process.pid, process.started))
            })
            .map(|process| process.pid)
            .collect()
    }
    /// Update from the current process list; `image` gives a process's full
    /// program path (asked once per new process).
    pub fn update(
        &mut self,
        now: &[Process],
        mut image: impl FnMut(u32) -> Option<String>,
    ) -> Tracked {
        let alive: HashMap<u32, u64> = now.iter().map(|p| (p.pid, p.started)).collect();
        self.tracked
            .retain(|pid, started| alive.get(pid) == Some(started));
        loop {
            let mut added = false;
            for process in now {
                let key = (process.pid, process.started);
                if self.tracked.contains_key(&process.pid)
                    || self.baseline.get(&process.pid) == Some(&process.started)
                    || STEAM_PROCESSES.contains(&process.name.to_ascii_lowercase().as_str())
                    || NOT_THE_GAME.contains(&process.name.to_ascii_lowercase().as_str())
                {
                    continue;
                }
                let child = self.tracked.contains_key(&process.parent);
                let inside = !child && !self.checked.contains(&key) && {
                    self.checked.insert(key);
                    image(process.pid).is_some_and(|path| {
                        let path = normalized(&path);
                        !self.install_dir.is_empty()
                            && path.starts_with(&self.install_dir)
                            && path[self.install_dir.len()..].starts_with('\\')
                    })
                };
                if child || inside {
                    self.tracked.insert(process.pid, process.started);
                    added = true;
                }
            }
            if !added {
                break;
            }
        }
        if !self.tracked.is_empty() {
            self.seen = true;
            self.empty_since = None;
            Tracked::Running
        } else if self.seen {
            let since = *self.empty_since.get_or_insert_with(std::time::Instant::now);
            if since.elapsed() >= self.grace {
                Tracked::Exited
            } else {
                Tracked::Running
            }
        } else if std::time::Instant::now() >= self.deadline {
            Tracked::Unknown
        } else {
            Tracked::Waiting
        }
    }
}
fn normalized(path: &str) -> String {
    path.replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
}
