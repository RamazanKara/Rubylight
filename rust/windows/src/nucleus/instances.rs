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
    /// Take away a window's title bar and sizing border, so its stream
    /// shows only the game.
    fn unframe(&mut self, window: u64) -> Result<()>;
    /// Keep Windows from throttling these background processes; the ones
    /// no longer listed get their settings back.
    fn unthrottle(&mut self, processes: &BTreeSet<u32>);
    /// The visible child window with this text in a window of `processes`:
    /// the profile's entry in Nucleus' list. An error when several match.
    fn label(&mut self, processes: &BTreeSet<u32>, text: &str) -> Result<Option<u64>>;
    /// The visible button a WinForms window of `processes` calls `name` in
    /// its program: Nucleus' Play shows only a picture.
    fn button(&mut self, processes: &BTreeSet<u32>, name: &str) -> Result<Option<u64>>;
    /// Click it as the mouse would; refused while another window covers it.
    fn click(&mut self, label: u64) -> Result<()>;
    /// Bring a window to the front, activated: whether it is in front.
    fn to_front(&mut self, window: u64) -> bool;
    /// Whether nobody uses the PC: no local input for a while, or only the
    /// desktop or taskbar in front. A window taken to the front then takes
    /// nobody's focus.
    fn pc_idle(&mut self) -> bool;
    /// The visible message boxes (`#32770` dialogs) of `processes`, and those
    /// Windows shows for any program (hard errors): with no processes, those
    /// alone.
    fn message_boxes(&mut self, processes: &BTreeSet<u32>) -> Result<Vec<MessageBox>>;
    /// Close the WinForms form of `processes` shown with `ShowDialog`, as
    /// its close button does: the visible, enabled one whose owner it
    /// disabled. The ones closed, with what they said.
    fn close_forms(&mut self, processes: &BTreeSet<u32>) -> Result<Vec<MessageBox>>;
    /// The process showing `window`.
    fn owner(&mut self, window: u64) -> Result<u32>;
    /// Whether Windows forces process `pid` to load its images at random
    /// addresses (Exploit protection's "Force randomization for images",
    /// Mandatory ASLR).
    fn forced_aslr(&mut self, pid: u32) -> Result<bool>;
}

/// A message box on the desktop: its window, its caption and its text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageBox {
    pub window: u64,
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

/// Passes in a row a window may leave its place before the keeper stops
/// moving it back on every pass (a game that re-centres itself on every
/// frame must not be fought for the whole stream). A game busy loading moves
/// late, so this allows several seconds.
const TRIES: u32 = 20;
/// After [`TRIES`], how many passes apart a window that keeps returning to
/// the PC's monitors, or minimising itself, is put back: never left there,
/// where the PC's keyboard and mouse reach it (#56).
const BACKOFF: u32 = 8;

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
    /// Each slot's instance: its root and every process it started.
    pub trees: Vec<BTreeSet<Identity>>,
    /// Slots whose instance exited in this pass.
    pub gone: Vec<usize>,
    /// Slots whose window keeps its own place or size off the PC's monitors,
    /// from this pass on left there.
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
                pass.trees.push(BTreeSet::new());
                continue;
            };
            slot.tree = tree(&processes, root);
            let pids: BTreeSet<u32> = slot.tree.iter().map(|id| id.pid).collect();
            processes_of_instances.extend(pids.iter().copied());
            let current = slot
                .window
                .and_then(|id| windows.iter().find(|w| w.id == id));
            // A game that minimised itself when it lost the focus stays its
            // instance's window, and is restored below; not once the
            // instance shows another (a launcher that minimised itself).
            let usable = current.is_some_and(|w| {
                !w.owned
                    && !w.tool
                    && pids.contains(&w.pid)
                    && (w.visible || (w.minimized && window::pick(&windows, &pids).is_none()))
            });
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
            pass.trees.push(slot.tree.clone());
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
            // A frame showed a title bar around Split Fiction's picture (#56).
            if !window.minimized
                && !window.framed
                && !window::strayed(window.rect, parked, &monitors)
            {
                slot.tries = 0;
                continue;
            }
            // Minimised or on a monitor, the stream shows nothing and the
            // PC's keyboard and mouse reach the game: it always goes back.
            let in_sight = window.minimized || window::on_monitor(window.rect, &monitors);
            if slot.stuck {
                if !in_sight {
                    continue;
                }
                slot.stuck = false;
                slot.tries = 0;
            }
            slot.tries = slot.tries.saturating_add(1);
            if slot.tries > TRIES {
                if !in_sight {
                    slot.stuck = true;
                    pass.stuck.push(index);
                    tracing::warn!(slot = index, rect = ?window.rect, framed = window.framed, "Nucleus Co-op window keeps its own place, size or frame beside the PC's desktop; left there");
                    continue;
                }
                if slot.tries == TRIES + 1 {
                    tracing::warn!(slot = index, rect = ?window.rect, minimized = window.minimized, "Nucleus Co-op window keeps returning to the PC's monitors or minimising; put back every {BACKOFF} passes");
                }
                if !(slot.tries - TRIES - 1).is_multiple_of(BACKOFF) {
                    continue;
                }
            }
            // Without its frame first: the place's size is then the game's.
            if window.framed
                && let Err(error) = desktop.unframe(window.id)
            {
                tracing::debug!(%error, slot = index, "Nucleus Co-op window's frame could not be taken away");
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
pub(crate) mod tests;
