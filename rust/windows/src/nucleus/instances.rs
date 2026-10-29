//! A session's game instances: which processes Nucleus started for which
//! player, each instance's game window, and keeping those windows parked
//! beside the PC's desktop at their stream's size.
use anyhow::Result;
use butterpollo_core::audio_scope::Identity;
use butterpollo_core::nucleus::window::{self, Rect, Window};
use butterpollo_core::steam::Process;
use std::collections::{BTreeMap, BTreeSet};

/// What a session reads and changes on the desktop: Win32 on the PC, fakes
/// in tests.
pub trait Desktop: Send {
    fn processes(&mut self) -> Result<Vec<Process>>;
    fn image_path(&mut self, pid: u32) -> Option<String>;
    /// Every top-level window.
    fn windows(&mut self) -> Result<Vec<Window>>;
    fn monitors(&mut self) -> Result<Vec<Rect>>;
    /// The virtual screen: the rectangle around every monitor.
    fn virtual_screen(&mut self) -> Rect;
    /// Move and size a window without activating it.
    fn place(&mut self, window: u64, to: Rect) -> Result<()>;
    /// Keep Windows from throttling these background processes; the ones
    /// no longer listed get their settings back.
    fn unthrottle(&mut self, processes: &BTreeSet<u32>);
    /// The visible child window with this text in a window of `processes`:
    /// the profile's entry in Nucleus' list. An error when several match.
    fn label(&mut self, processes: &BTreeSet<u32>, text: &str) -> Result<Option<u64>>;
    /// Click it as the mouse would; refused while another window covers it.
    fn click(&mut self, label: u64) -> Result<()>;
    /// The visible message boxes (`#32770` dialogs) of `processes`, and those
    /// Windows shows for a program (hard errors).
    fn message_boxes(&mut self, processes: &BTreeSet<u32>) -> Result<Vec<MessageBox>>;
}

/// A message box on the desktop: its caption and its text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageBox {
    pub caption: String,
    pub text: String,
}

impl MessageBox {
    /// What it says on one line, as a failure quotes it: `caption: text`,
    /// spaces collapsed and cut to a few sentences.
    pub fn summary(&self) -> String {
        const LIMIT: usize = 300;
        let plain = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
        let (caption, text) = (plain(&self.caption), plain(&self.text));
        let line = match (caption.is_empty(), text.is_empty()) {
            (false, false) => format!("{caption}: {text}"),
            (true, _) => text,
            (false, true) => caption,
        };
        match line.char_indices().nth(LIMIT) {
            Some((at, _)) => format!("{}...", line[..at].trim_end()),
            None => line,
        }
    }
}

/// Passes in a row a window may leave its place before the keeper leaves it
/// alone (a game that re-centres itself on every frame must not be fought
/// for the whole stream). A game busy loading moves late, so this allows
/// several seconds.
const TRIES: u32 = 20;

/// Which Nucleus instance folder a process runs from: Nucleus starts
/// instance k from `...\Instance<k>\...` when it copies or links the game.
pub fn instance_number(path: &str) -> Option<usize> {
    let path = path.replace('/', "\\");
    let mut folders: Vec<&str> = path.split('\\').collect();
    folders.pop();
    folders.iter().find_map(|folder| {
        let rest = folder
            .get(..8)?
            .eq_ignore_ascii_case("instance")
            .then(|| &folder[8..])?;
        (!rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())).then(|| rest.parse().ok())?
    })
}

fn identity(process: &Process) -> Identity {
    Identity {
        pid: process.pid,
        started: process.started,
    }
}

/// `root` and every process it started, by pid. A parent started after its
/// "child" is a reused pid, not a parent.
pub fn tree(processes: &[Process], root: Identity) -> BTreeSet<Identity> {
    let mut found = BTreeSet::from([root]);
    let mut pending = vec![root];
    while let Some(parent) = pending.pop() {
        for child in processes {
            let child_id = identity(child);
            if child.parent == parent.pid
                && child.started >= parent.started
                && child.pid != parent.pid
                && found.insert(child_id)
            {
                pending.push(child_id);
            }
        }
    }
    found
}

#[derive(Debug)]
struct Slot {
    root: Option<Identity>,
    /// The instance's processes at the last pass.
    tree: BTreeSet<Identity>,
    window: Option<u64>,
    tries: u32,
    stuck: bool,
    gone: bool,
}

/// What one pass found.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Pass {
    /// Each slot's game window.
    pub windows: Vec<Option<u64>>,
    /// Each slot's instance root.
    pub roots: Vec<Option<Identity>>,
    /// Slots whose instance exited in this pass.
    pub gone: Vec<usize>,
    /// Slots whose window keeps leaving its place, from this pass on left alone.
    pub stuck: Vec<usize>,
}

/// Finds a session's instances and keeps their windows parked.
pub struct Keeper {
    /// The handler's executable names, lower case.
    executables: Vec<String>,
    /// Nucleus' `content` folder, lower case with a trailing backslash: the
    /// copies of the game it starts live below it.
    content: String,
    /// When Nucleus started (FILETIME): an older process is not an instance.
    since: u64,
    slots: Vec<Slot>,
}

impl Keeper {
    pub fn new(players: usize, executables: &[String], content: &str, since: u64) -> Self {
        let mut content = content.replace('/', "\\").to_lowercase();
        if !content.ends_with('\\') {
            content.push('\\');
        }
        Self {
            executables: executables.iter().map(|e| e.to_lowercase()).collect(),
            content,
            since,
            slots: (0..players)
                .map(|_| Slot {
                    root: None,
                    tree: BTreeSet::new(),
                    window: None,
                    tries: 0,
                    stuck: false,
                    gone: false,
                })
                .collect(),
        }
    }

    /// Every instance's processes at the last pass (pid → start time), to
    /// stop with the session.
    pub fn processes(&self) -> BTreeMap<u32, u64> {
        self.slots
            .iter()
            .flat_map(|s| s.tree.iter().map(|id| (id.pid, id.started)))
            .collect()
    }

    fn is_game(&self, process: &Process) -> bool {
        self.executables
            .iter()
            .any(|e| process.name.eq_ignore_ascii_case(e))
    }

    /// Instances started since the last pass: the handler's executables in
    /// Nucleus' job, or below its `content` folder, that no known instance
    /// started, and whose ancestors are not instances themselves (a game
    /// that restarts itself stays one instance). Each with its instance
    /// folder's number when it has one, oldest first.
    fn discover(
        &self,
        processes: &[Process],
        job: &BTreeSet<u32>,
        desktop: &mut dyn Desktop,
    ) -> Vec<(Identity, Option<usize>)> {
        let known: BTreeSet<Identity> = self
            .slots
            .iter()
            .flat_map(|s| s.tree.iter().copied())
            .collect();
        let mut found: BTreeMap<Identity, Option<usize>> = BTreeMap::new();
        for process in processes {
            let id = identity(process);
            if !self.is_game(process) || known.contains(&id) || process.started < self.since {
                continue;
            }
            let path = desktop.image_path(process.pid);
            let below_content = path.as_deref().is_some_and(|p| {
                p.replace('/', "\\")
                    .to_lowercase()
                    .starts_with(&self.content)
            });
            if job.contains(&process.pid) || below_content {
                found.insert(id, path.as_deref().and_then(instance_number));
            }
        }
        let by_pid: BTreeMap<u32, &Process> = processes.iter().map(|p| (p.pid, p)).collect();
        let mut roots: Vec<(Identity, Option<usize>)> = found
            .iter()
            .filter(|(id, _)| {
                // Walk the ancestors; one that is a found or known instance owns this one.
                let mut current = by_pid.get(&id.pid).copied();
                for _ in 0..64 {
                    let Some(process) = current else { break };
                    let Some(parent) = by_pid.get(&process.parent).copied() else {
                        break;
                    };
                    if parent.started > process.started || parent.pid == process.pid {
                        break;
                    }
                    let parent_id = identity(parent);
                    if found.contains_key(&parent_id) || known.contains(&parent_id) {
                        return false;
                    }
                    current = Some(parent);
                }
                true
            })
            .map(|(id, number)| (*id, *number))
            .collect();
        roots.sort_by_key(|(id, _)| (id.started, id.pid));
        roots
    }

    /// One pass: follow the instances, choose their windows, park them at
    /// `sizes` (each slot's stream size). An error when a window cannot be
    /// parked at all (the stream is too large to fit beside the desktop).
    pub fn pass(
        &mut self,
        desktop: &mut dyn Desktop,
        job: &BTreeSet<u32>,
        sizes: &[(u32, u32)],
    ) -> Result<Pass> {
        let processes = desktop.processes()?;
        let alive: BTreeSet<Identity> = processes.iter().map(identity).collect();
        let mut pass = Pass::default();
        for (index, slot) in self.slots.iter_mut().enumerate() {
            let Some(root) = slot.root else { continue };
            if slot.gone || alive.contains(&root) {
                continue;
            }
            // A launcher that started the game and exited leaves the game.
            let heir = processes
                .iter()
                .filter(|p| slot.tree.contains(&identity(p)))
                .min_by_key(|p| {
                    let game = self
                        .executables
                        .iter()
                        .any(|e| p.name.eq_ignore_ascii_case(e));
                    (!game, p.started, p.pid)
                })
                .map(identity);
            match heir {
                Some(heir) => slot.root = Some(heir),
                None => {
                    slot.gone = true;
                    slot.window = None;
                    slot.tree.clear();
                    pass.gone.push(index);
                }
            }
        }
        for (root, number) in self.discover(&processes, job, desktop) {
            let free = |slot: &Slot| slot.root.is_none();
            let index = number
                .filter(|&k| self.slots.get(k).is_some_and(free))
                .or_else(|| self.slots.iter().position(free));
            match index {
                Some(index) => {
                    tracing::info!(
                        slot = index,
                        pid = root.pid,
                        "Nucleus Co-op instance started"
                    );
                    self.slots[index].root = Some(root);
                }
                None => {
                    tracing::debug!(
                        pid = root.pid,
                        "Nucleus Co-op started more instances than players"
                    )
                }
            }
        }
        let windows = desktop.windows()?;
        let monitors = desktop.monitors()?;
        let screen = desktop.virtual_screen();
        let mut processes_of_instances = BTreeSet::new();
        for (index, slot) in self.slots.iter_mut().enumerate() {
            let Some(root) = slot.root.filter(|_| !slot.gone) else {
                pass.windows.push(None);
                pass.roots.push(None);
                continue;
            };
            slot.tree = tree(&processes, root);
            let pids: BTreeSet<u32> = slot.tree.iter().map(|id| id.pid).collect();
            processes_of_instances.extend(pids.iter().copied());
            let current = slot
                .window
                .and_then(|id| windows.iter().find(|w| w.id == id));
            let usable =
                current.is_some_and(|w| w.visible && !w.owned && !w.tool && pids.contains(&w.pid));
            if !usable {
                let picked = window::pick(&windows, &pids);
                if picked != slot.window {
                    tracing::info!(slot = index, window = ?picked, "Nucleus Co-op instance window");
                    slot.tries = 0;
                    slot.stuck = false;
                }
                slot.window = picked;
            }
            pass.windows.push(slot.window);
            pass.roots.push(Some(root));
            let Some(window) = slot
                .window
                .and_then(|id| windows.iter().find(|w| w.id == id))
            else {
                continue;
            };
            let Some(&(width, height)) = sizes.get(index).or(sizes.first()) else {
                continue;
            };
            // Spaced for the widest stream so windows of different sizes
            // never overlap.
            let widest = sizes.iter().map(|s| s.0).max().unwrap_or(width);
            let mut parked =
                window::parking(screen, index, (widest, height)).map_err(anyhow::Error::msg)?;
            parked.width = i32::try_from(width).map_err(anyhow::Error::msg)?;
            if !window::strayed(window.rect, parked, &monitors) {
                slot.tries = 0;
                continue;
            }
            if slot.stuck {
                continue;
            }
            slot.tries += 1;
            if slot.tries > TRIES {
                slot.stuck = true;
                pass.stuck.push(index);
                tracing::warn!(slot = index, rect = ?window.rect, "Nucleus Co-op window keeps leaving its place; left alone");
                continue;
            }
            if let Err(error) = desktop.place(window.id, parked) {
                tracing::debug!(%error, slot = index, "Nucleus Co-op window could not be moved");
            }
        }
        desktop.unthrottle(&processes_of_instances);
        Ok(pass)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const SCREEN: Rect = Rect {
        x: 0,
        y: 0,
        width: 2560,
        height: 1440,
    };

    /// A desktop that is only data: tests change it between passes.
    #[derive(Default)]
    pub(crate) struct Fake {
        pub processes: Vec<Process>,
        pub paths: BTreeMap<u32, String>,
        pub windows: Vec<Window>,
        pub placed: Vec<(u64, Rect)>,
        pub unthrottled: BTreeSet<u32>,
        pub labels: Vec<(u32, String, u64)>,
        pub clicks: Vec<u64>,
        pub covered: bool,
        /// Message boxes and the process showing each.
        pub boxes: Vec<(u32, MessageBox)>,
        /// Listing message boxes fails.
        pub unlisted: bool,
        /// Placing a window moves it there, as a cooperative game does.
        pub obey: bool,
    }
    impl Desktop for Fake {
        fn processes(&mut self) -> Result<Vec<Process>> {
            Ok(self.processes.clone())
        }
        fn image_path(&mut self, pid: u32) -> Option<String> {
            self.paths.get(&pid).cloned()
        }
        fn windows(&mut self) -> Result<Vec<Window>> {
            Ok(self.windows.clone())
        }
        fn monitors(&mut self) -> Result<Vec<Rect>> {
            Ok(vec![SCREEN])
        }
        fn virtual_screen(&mut self) -> Rect {
            SCREEN
        }
        fn place(&mut self, window: u64, to: Rect) -> Result<()> {
            self.placed.push((window, to));
            if self.obey
                && let Some(w) = self.windows.iter_mut().find(|w| w.id == window)
            {
                w.rect = to;
            }
            Ok(())
        }
        fn unthrottle(&mut self, processes: &BTreeSet<u32>) {
            self.unthrottled = processes.clone();
        }
        fn label(&mut self, processes: &BTreeSet<u32>, text: &str) -> Result<Option<u64>> {
            let mut found = self
                .labels
                .iter()
                .filter(|(pid, t, _)| processes.contains(pid) && t == text);
            let first = found.next().map(|(_, _, id)| *id);
            anyhow::ensure!(found.next().is_none(), "two labels");
            Ok(first)
        }
        fn click(&mut self, label: u64) -> Result<()> {
            anyhow::ensure!(!self.covered, "covered");
            self.clicks.push(label);
            Ok(())
        }
        fn message_boxes(&mut self, processes: &BTreeSet<u32>) -> Result<Vec<MessageBox>> {
            anyhow::ensure!(!self.unlisted, "access denied");
            Ok(self
                .boxes
                .iter()
                .filter(|(pid, _)| processes.contains(pid))
                .map(|(_, shown)| shown.clone())
                .collect())
        }
    }

    #[test]
    fn a_message_box_is_quoted_on_one_short_line() {
        let shown = |caption: &str, text: &str| MessageBox {
            caption: caption.into(),
            text: text.into(),
        };
        assert_eq!(
            shown(
                "OneDrive must be disabled",
                "Your Documents folder\r\n  is in OneDrive."
            )
            .summary(),
            "OneDrive must be disabled: Your Documents folder is in OneDrive."
        );
        assert_eq!(shown("", "Text only").summary(), "Text only");
        assert_eq!(shown("Caption only", " ").summary(), "Caption only");
        let long = shown("Error", &"é".repeat(400)).summary();
        assert_eq!(long.chars().count(), 303);
        assert!(long.ends_with("é..."), "{long}");
    }

    pub(crate) fn fake(processes: Vec<Process>, windows: Vec<Window>) -> Fake {
        Fake {
            processes,
            windows,
            ..Default::default()
        }
    }
    pub(crate) fn process(pid: u32, parent: u32, started: u64, name: &str) -> Process {
        Process {
            pid,
            parent,
            started,
            name: name.into(),
        }
    }
    pub(crate) fn window(id: u64, pid: u32, rect: Rect) -> Window {
        Window {
            id,
            pid,
            rect,
            visible: true,
            owned: false,
            tool: false,
        }
    }
    pub(crate) const ON_SCREEN: Rect = Rect {
        x: 100,
        y: 100,
        width: 1280,
        height: 720,
    };
    const SIZES: [(u32, u32); 2] = [(1920, 1080), (1280, 720)];

    fn keeper(players: usize) -> Keeper {
        Keeper::new(players, &["Game.exe".into()], r"C:\Nucleus\content", 100)
    }

    #[test]
    fn instance_folders_name_their_player() {
        assert_eq!(
            instance_number(r"C:\Nucleus\content\BL2\Instance1\Binaries\Game.exe"),
            Some(1)
        );
        assert_eq!(
            instance_number(r"c:/n/content/x/instance12/game.exe"),
            Some(12)
        );
        assert_eq!(instance_number(r"D:\Games\BL2\Game.exe"), None);
        assert_eq!(instance_number(r"D:\Instances\Game.exe"), None);
        assert_eq!(instance_number(r"D:\x\Instance2.exe"), None);
    }

    #[test]
    fn instances_take_their_folders_slot_and_keep_their_children() {
        let mut keeper = keeper(2);
        // Nucleus (10) started instance 1 first, then instance 0; each game
        // started a helper of the same name (a crash reporter), and the PC
        // user's own copy of the game (5, older than Nucleus) runs too.
        let mut desktop = Fake {
            processes: vec![
                process(5, 1, 50, "Game.exe"),
                process(10, 1, 100, "NucleusCoop.exe"),
                process(20, 10, 110, "Game.exe"),
                process(21, 20, 111, "Game.exe"),
                process(30, 10, 120, "game.exe"),
            ],
            paths: BTreeMap::from([
                (5, r"D:\Games\Game.exe".into()),
                (20, r"C:\Nucleus\content\G\Instance1\Game.exe".into()),
                (21, r"C:\Nucleus\content\G\Instance1\Game.exe".into()),
                (30, r"C:\Nucleus\content\G\Instance0\Game.exe".into()),
            ]),
            windows: vec![
                window(1, 5, ON_SCREEN),
                window(2, 21, ON_SCREEN),
                window(3, 30, ON_SCREEN),
            ],
            ..Default::default()
        };
        let job = BTreeSet::from([10, 20, 21, 30]);
        let pass = keeper.pass(&mut desktop, &job, &SIZES).unwrap();
        assert_eq!(pass.windows, [Some(3), Some(2)]);
        assert_eq!(
            pass.roots,
            [
                Some(Identity {
                    pid: 30,
                    started: 120
                }),
                Some(Identity {
                    pid: 20,
                    started: 110
                })
            ]
        );
        assert_eq!(desktop.unthrottled, BTreeSet::from([20, 21, 30]));
        // The session stops these, never the PC user's own copy.
        assert_eq!(
            keeper.processes(),
            BTreeMap::from([(20, 110), (21, 111), (30, 120)])
        );
        // Parked beside the desktop at each player's stream size; the PC
        // user's window is left alone.
        assert_eq!(
            desktop.placed,
            [
                (
                    3,
                    Rect {
                        x: 2624,
                        y: 0,
                        width: 1920,
                        height: 1080
                    }
                ),
                (
                    2,
                    Rect {
                        x: 2560 + 64 + 1920 + 64,
                        y: 0,
                        width: 1280,
                        height: 720
                    }
                )
            ]
        );
    }

    #[test]
    fn without_instance_folders_instances_fill_slots_in_start_order() {
        let mut keeper = keeper(2);
        let mut desktop = fake(
            vec![
                process(10, 1, 100, "NucleusCoop.exe"),
                process(31, 10, 130, "Game.exe"),
                process(30, 10, 120, "Game.exe"),
            ],
            vec![window(4, 31, ON_SCREEN), window(3, 30, ON_SCREEN)],
        );
        let job = BTreeSet::from([10, 30, 31]);
        let pass = keeper.pass(&mut desktop, &job, &SIZES).unwrap();
        assert_eq!(pass.windows, [Some(3), Some(4)]);
        // Outside the job and not below Nucleus' content folder: not an instance.
        let mut keeper = self::keeper(1);
        desktop.processes = vec![process(40, 1, 140, "Game.exe")];
        desktop.windows = vec![window(5, 40, ON_SCREEN)];
        let pass = keeper.pass(&mut desktop, &BTreeSet::new(), &SIZES).unwrap();
        assert_eq!(pass.windows, [None]);
    }

    #[test]
    fn a_closed_window_is_replaced_and_an_exited_instance_reported() {
        let mut keeper = keeper(1);
        let mut desktop = Fake {
            obey: true,
            ..fake(
                vec![process(20, 10, 110, "Game.exe")],
                vec![window(2, 20, ON_SCREEN)],
            )
        };
        let job = BTreeSet::from([20]);
        assert_eq!(
            keeper.pass(&mut desktop, &job, &SIZES).unwrap().windows,
            [Some(2)]
        );
        // The game recreates its window (a mode change): the new one is followed and parked.
        desktop.windows = vec![window(7, 20, ON_SCREEN)];
        assert_eq!(
            keeper.pass(&mut desktop, &job, &SIZES).unwrap().windows,
            [Some(7)]
        );
        assert_eq!(desktop.placed.last().unwrap().0, 7);
        // No window for a while (loading) is not an exit.
        desktop.windows.clear();
        let pass = keeper.pass(&mut desktop, &job, &SIZES).unwrap();
        assert_eq!((pass.windows, pass.gone), (vec![None], vec![]));
        desktop.processes.clear();
        let pass = keeper.pass(&mut desktop, &job, &SIZES).unwrap();
        assert_eq!(pass.gone, [0]);
        assert_eq!(pass.windows, [None]);
        // Reported once.
        assert!(
            keeper
                .pass(&mut desktop, &job, &SIZES)
                .unwrap()
                .gone
                .is_empty()
        );
    }

    #[test]
    fn a_launcher_that_exits_leaves_the_game_as_the_instance() {
        let mut keeper = Keeper::new(
            1,
            &["Launcher.exe".into(), "Game.exe".into()],
            r"C:\N\content",
            100,
        );
        let mut desktop = fake(
            vec![
                process(20, 10, 110, "Launcher.exe"),
                process(21, 20, 111, "Game.exe"),
            ],
            vec![window(2, 21, ON_SCREEN)],
        );
        let job = BTreeSet::from([20, 21]);
        let pass = keeper.pass(&mut desktop, &job, &SIZES).unwrap();
        assert_eq!(
            pass.roots,
            [Some(Identity {
                pid: 20,
                started: 110
            })]
        );
        desktop.processes.remove(0);
        let pass = keeper.pass(&mut desktop, &job, &SIZES).unwrap();
        assert!(pass.gone.is_empty());
        assert_eq!(
            pass.roots,
            [Some(Identity {
                pid: 21,
                started: 111
            })]
        );
        assert_eq!(pass.windows, [Some(2)]);
    }

    #[test]
    fn a_reused_pid_is_not_the_instance() {
        let mut keeper = keeper(1);
        let mut desktop = fake(vec![process(20, 10, 110, "Game.exe")], Vec::new());
        let job = BTreeSet::from([20]);
        keeper.pass(&mut desktop, &job, &SIZES).unwrap();
        // The instance exited and Windows gave its pid to an unrelated process.
        desktop.processes = vec![process(20, 3, 500, "Other.exe")];
        assert_eq!(keeper.pass(&mut desktop, &job, &SIZES).unwrap().gone, [0]);
    }

    #[test]
    fn a_window_that_keeps_returning_is_left_alone_after_a_while() {
        let mut keeper = keeper(1);
        let mut desktop = fake(
            vec![process(20, 10, 110, "Game.exe")],
            vec![window(2, 20, ON_SCREEN)],
        );
        let job = BTreeSet::from([20]);
        let mut stuck = Vec::new();
        for _ in 0..TRIES + 5 {
            stuck.extend(keeper.pass(&mut desktop, &job, &SIZES).unwrap().stuck);
        }
        assert_eq!(stuck, [0]);
        assert_eq!(desktop.placed.len(), TRIES as usize);
        // A window that stays where it was parked is moved once.
        let mut keeper = self::keeper(1);
        desktop.obey = true;
        desktop.placed.clear();
        for _ in 0..3 {
            keeper.pass(&mut desktop, &job, &SIZES).unwrap();
        }
        assert_eq!(desktop.placed.len(), 1);
    }

    #[test]
    fn a_stream_too_large_to_park_is_an_error() {
        let mut keeper = keeper(1);
        let mut desktop = fake(
            vec![process(20, 10, 110, "Game.exe")],
            vec![window(2, 20, ON_SCREEN)],
        );
        let error = keeper
            .pass(&mut desktop, &BTreeSet::from([20]), &[(40000, 1080)])
            .unwrap_err()
            .to_string();
        assert!(error.contains("does not fit"), "{error}");
    }
}
