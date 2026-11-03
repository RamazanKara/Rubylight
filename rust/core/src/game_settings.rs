//! Game settings files per device (issues #72 and #69): a game's own settings files (its
//! resolution and quality) swapped for the copy of the device that streams it.
//!
//! An app lists whole files in `game-settings-files`: absolute paths, `%VAR%` names from the
//! Windows user's environment allowed (`%LOCALAPPDATA%\Game\Settings.ini`). When a device
//! starts the app, the host keeps the PC's own files (the originals) and puts that device's
//! saved copies in their place. When the app stops (End session, Close game, the game exiting)
//! the files, with whatever the game changed, become the device's copies and the originals go
//! back. A device's first launch keeps the files as they are and saves them as its copies at the
//! end. Finding a game's settings files is up to the user; the host does not look for them.
//!
//! Nothing is lost on the way: the originals and every device copy live under the host's
//! config folder (`game-settings`, [`FOLDER`]), a device's previous copy is kept beside its
//! new one (`.previous`), and the journal ([`JOURNAL_FILE`]) records each file's step before
//! the step is taken. A swap a crash or a failure left unfinished is finished later from the
//! journal ([`Store::recover`]): a file still holding the device's copy is saved as that copy
//! first, then the original goes back. A file that is missing, locked or unreadable fails the
//! launch with the reason, before the game starts; one that cannot be saved or put back when
//! the app stops stays journaled, and nothing of it is overwritten until it can be.
//!
//! One swap holds a file at a time. A launch whose file another swap still holds finishes that
//! swap first; while that swap's app runs, or when it cannot be finished, the launch fails.
//!
//! Each swap keeps its own originals (`apps/<app>/swaps/<id>/`): a later launch never
//! overwrites an original that is not back yet. Once every file is back, they move to
//! `apps/<app>/original/` as the backup of the PC's last own files. A swap also records the
//! game's programs as the host sees them ([`Store::watch`]); while one of them still runs, no
//! file is saved or put back (the game would write over the PC's file as it exits), and the
//! swap stays journaled until it has closed.
//!
//! The host runs as SYSTEM: the game's files are read and written through an
//! [`Access`] (as the signed-in Windows user, as for managed game files), so a link in the
//! user's folders leads nowhere the user could not write anyway. The store is the host's own.
use crate::{
    game_files::{self, Access},
    state,
};
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard, PoisonError},
};

/// The app setting: a list of paths.
pub const SETTING: &str = "game-settings-files";
/// The host's folder for the copies, in its config folder.
pub const FOLDER: &str = "game-settings";
/// The journal, in [`FOLDER`].
pub const JOURNAL_FILE: &str = "journal.json";
/// Files per app.
pub const MAX_FILES: usize = 16;
/// The largest file taken: settings, not saves or caches.
pub const MAX_FILE_BYTES: u64 = 16 << 20;
const MAX_PATH: usize = 1024;

/// Held through every journal change, so two swaps never interleave.
static LOCK: Mutex<()> = Mutex::new(());
fn lock() -> MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The paths an app lists, checked; none when it lists none.
pub fn paths(extra: &BTreeMap<String, Value>) -> Result<Vec<String>> {
    let list = match extra.get(SETTING) {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(list)) => list,
        Some(_) => bail!("{SETTING} must be a list of paths"),
    };
    let mut paths = Vec::new();
    let mut seen = BTreeSet::new();
    for path in list {
        let path = path
            .as_str()
            .with_context(|| format!("{SETTING} must be a list of paths"))?
            .trim();
        if path.is_empty() {
            continue;
        }
        check_path(path)?;
        if !seen.insert(normal(path)) {
            bail!("{path} is listed twice");
        }
        paths.push(path.to_owned());
    }
    if paths.len() > MAX_FILES {
        bail!("at most {MAX_FILES} game settings files per app");
    }
    Ok(paths)
}

/// Check a path as written: one line, a drive letter or a `%VAR%` first, no network or device
/// path, no `..`, and every `%` part of a `%NAME%`.
pub fn check_path(path: &str) -> Result<()> {
    let path = path.trim();
    if path.is_empty() {
        bail!("no file is named");
    }
    if path.chars().count() > MAX_PATH || path.chars().any(char::is_control) {
        bail!("{path}: a path is one line of at most {MAX_PATH} characters");
    }
    let names = variables(path)?;
    if !path.starts_with('%') {
        check_absolute(path)?;
    } else if names.is_empty() {
        bail!(
            "{path}: start with a drive letter, such as C:\\, or a %NAME% such as %LOCALAPPDATA%"
        );
    }
    let bare = names.iter().fold(path.to_owned(), |rest, name| {
        rest.replace(&format!("%{name}%"), "x")
    });
    if bare.get(2..).is_some_and(|rest| rest.contains(':')) {
        bail!("{path}: a colon belongs only after the drive letter");
    }
    if path.split(['\\', '/']).any(|part| part.trim() == "..") {
        bail!("{path}: .. is not taken; name the file's own folder");
    }
    if path.ends_with(['\\', '/']) {
        bail!("{path} names a folder; name the settings file in it");
    }
    Ok(())
}

/// The `%NAME%` parts of `path`, in order. An unpaired `%` is an error.
fn variables(path: &str) -> Result<Vec<String>> {
    let parts: Vec<&str> = path.split('%').collect();
    if parts.len().is_multiple_of(2) {
        bail!("{path}: a % has no closing %; environment variables are written %NAME%");
    }
    parts
        .iter()
        .skip(1)
        .step_by(2)
        .map(|name| {
            if name.is_empty()
                || !name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '(' | ')' | '-'))
            {
                bail!("{path}: %{name}% is not an environment variable name");
            }
            Ok((*name).to_owned())
        })
        .collect()
}

/// A drive letter, a colon and a slash first.
fn check_absolute(path: &str) -> Result<()> {
    let bytes = path.as_bytes();
    if matches!(bytes.first(), Some(b'\\' | b'/')) {
        bail!("{path}: network and device paths are not taken; start with a drive letter");
    }
    let drive = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/');
    if !drive {
        bail!(
            "{path}: start with a drive letter, such as C:\\, or a %NAME% such as %LOCALAPPDATA%"
        );
    }
    Ok(())
}

/// `path` with its `%NAME%` parts replaced by `lookup` (the Windows user's environment), checked
/// again as a file on a drive. A name that is not set is an error.
pub fn expand(path: &str, lookup: impl Fn(&str) -> Option<String>) -> Result<PathBuf> {
    check_path(path)?;
    let path = path.trim();
    let mut expanded = String::new();
    for (index, part) in path.split('%').enumerate() {
        if index % 2 == 0 {
            expanded.push_str(part);
        } else {
            let value = lookup(part)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| anyhow!("{path}: %{part}% is not set for the Windows user"))?;
            expanded.push_str(value.trim().trim_end_matches(['\\', '/']));
        }
    }
    check_absolute(&expanded).with_context(|| format!("{path} becomes {expanded}"))?;
    if expanded.get(2..).is_some_and(|rest| rest.contains(':'))
        || expanded.split(['\\', '/']).any(|part| part.trim() == "..")
    {
        bail!("{path} becomes {expanded}, which is not a plain file path");
    }
    Ok(PathBuf::from(expanded.replace('/', "\\")))
}

/// A path compared without regard to ASCII case or slashes.
fn normal(path: &str) -> String {
    path.trim().replace('/', "\\").to_ascii_lowercase()
}

/// A name for the store: letters, digits, `.`, `-` and `_` only, with a hash of the original when
/// anything had to change.
fn store_name(name: &str) -> String {
    let clean: String = name
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect();
    let clean = clean.trim_matches('.').to_owned();
    if !clean.is_empty() && clean == name {
        clean
    } else {
        format!("{clean}-{:08x}", crc32fast::hash(name.as_bytes()))
    }
}

/// The store's name for a file as the app lists it: its file name and a hash of the whole path.
pub fn file_key(path: &str) -> String {
    let normal = normal(path);
    let name = normal.rsplit('\\').next().unwrap_or_default();
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect();
    format!("{clean}-{:08x}", crc32fast::hash(normal.as_bytes()))
}

/// The store's name for an app: its `uuid`, else its id.
pub fn app_key(app: &state::App) -> String {
    app.extra
        .get("uuid")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|uuid| !uuid.is_empty())
        .map_or_else(|| app.id().to_string(), str::to_ascii_lowercase)
}

/// Where a swapped file is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    /// The original is kept; the device's copy may be going in. Finishing puts the original back.
    Placing,
    /// The file is the device's (its copy, or on its first launch the file as it was). Finishing
    /// saves it as the device's copy, then puts the original back.
    Playing,
    /// Saved as the device's copy; the original goes back.
    Saved,
    Done,
}

/// A file of a swap.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSwap {
    /// As the app lists it.
    pub written: String,
    /// Expanded.
    pub path: PathBuf,
    pub key: String,
    pub step: Step,
}

/// A launch's swap, until every file is back.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Swap {
    pub id: String,
    pub app: String,
    #[serde(default)]
    pub app_name: String,
    pub device: String,
    #[serde(default)]
    pub device_name: String,
    pub files: Vec<FileSwap>,
    /// The game's programs (full paths) seen while it ran: the files stay while one runs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub programs: Vec<String>,
    /// Why the last try to finish it failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
impl Swap {
    fn holds(&self, path: &Path) -> bool {
        let path = normal(&path.to_string_lossy());
        self.files
            .iter()
            .any(|file| file.step != Step::Done && normal(&file.path.to_string_lossy()) == path)
    }
    fn who(&self) -> String {
        let device = if self.device_name.is_empty() {
            &self.device
        } else {
            &self.device_name
        };
        format!("{} from {device}", self.app_name)
    }
}

/// The swaps not finished yet.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Journal {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub swaps: Vec<Swap>,
}

/// What a launch or a finish did, by file as listed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// The device's copy went in.
    pub placed: Vec<String>,
    /// The device had no copy: the file stays and becomes its copy at the end.
    pub first: Vec<String>,
    /// Saved as the device's copy.
    pub saved: Vec<String>,
    /// The original is back.
    pub restored: Vec<String>,
    /// Done, but worth saying loudly (a file the game removed).
    pub warnings: Vec<String>,
}

/// A launch's swap, begun.
#[derive(Debug)]
pub struct Begun {
    pub id: String,
    pub report: Report,
}

/// Which of a swap's programs still runs, if one does: its path, or why that is not known.
pub type Running = Box<dyn Fn(&[String]) -> Option<String> + Send + Sync>;

/// The copies and the journal under a config folder.
pub struct Store {
    root: PathBuf,
    access: Access,
    running: Running,
}

impl Store {
    /// A store that takes every recorded program as closed; see [`Self::running`].
    pub fn new(directory: &Path, access: Access) -> Self {
        Self {
            root: directory.join(FOLDER),
            access,
            running: Box::new(|_| None),
        }
    }
    /// Tell the store which recorded programs still run.
    pub fn running(mut self, running: Running) -> Self {
        self.running = running;
        self
    }
    fn journal_path(&self) -> PathBuf {
        self.root.join(JOURNAL_FILE)
    }
    fn app_folder(&self, app: &str) -> PathBuf {
        self.root.join("apps").join(store_name(app))
    }
    /// A device's saved copy of a file.
    pub fn device_copy(&self, app: &str, device: &str, key: &str) -> PathBuf {
        self.app_folder(app)
            .join("devices")
            .join(store_name(device))
            .join(key)
    }
    /// The PC's last own file put back, kept as a backup.
    pub fn original(&self, app: &str, key: &str) -> PathBuf {
        self.app_folder(app).join("original").join(key)
    }
    /// The folder of swap `id`'s originals, until all are back.
    fn swap_folder(&self, app: &str, id: &str) -> PathBuf {
        self.app_folder(app).join("swaps").join(store_name(id))
    }
    /// Whether `device` has a saved copy of the file the app lists as `written`.
    pub fn saved(&self, app: &str, device: &str, written: &str) -> bool {
        self.device_copy(app, device, &file_key(written)).is_file()
    }

    /// The journal. Missing is empty; unreadable is an error, so nothing is swapped while the
    /// originals in it could be forgotten.
    pub fn journal(&self) -> Result<Journal> {
        let path = self.journal_path();
        match std::fs::read(&path) {
            Ok(bytes) => match state::json_bytes(&bytes) {
                None => Ok(Journal::default()),
                Some(json) => serde_json::from_slice(json)
                    .with_context(|| format!("invalid game settings journal {}", path.display())),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Journal::default()),
            Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
        }
    }
    fn save(&self, journal: &Journal) -> Result<()> {
        state::write_json(
            &self.journal_path(),
            &Journal {
                version: 1,
                swaps: journal.swaps.clone(),
            },
        )
    }

    /// `device` (its name for messages) starts `app`: keep the originals of `files` (as listed,
    /// expanded) and put the device's copies in, as swap `id`. A swap that still holds one of
    /// the files and is not in `live` (running apps' swaps, read under the store's lock, so a
    /// swap registered before its `begin` is never taken for a stale one) is finished first.
    /// Fails, with every file as it was, when a file is missing or cannot be read or written,
    /// or another swap holds it.
    #[allow(clippy::too_many_arguments)]
    pub fn begin(
        &self,
        id: &str,
        app: &str,
        app_name: &str,
        device: &str,
        device_name: &str,
        files: &[(String, PathBuf)],
        live: &dyn Fn() -> BTreeSet<String>,
    ) -> Result<Begun> {
        let mut seen: BTreeMap<String, &str> = BTreeMap::new();
        for (written, path) in files {
            if let Some(other) = seen.insert(normal(&path.to_string_lossy()), written) {
                bail!(
                    "{other} and {written} are the same file ({})",
                    path.display()
                );
            }
        }
        let _lock = lock();
        let live = live();
        let mut journal = self.journal()?;
        if journal.swaps.iter().any(|swap| swap.id == id) {
            bail!("game settings swap {id} exists already");
        }
        let stale: Vec<String> = journal
            .swaps
            .iter()
            .filter(|swap| files.iter().any(|(_, path)| swap.holds(path)))
            .map(|swap| swap.id.clone())
            .collect();
        for id in stale {
            let swap = journal.swaps.iter().find(|swap| swap.id == id).unwrap();
            let who = swap.who();
            if live.contains(&id) {
                bail!("the settings files are in use by {who}, which is still running");
            }
            self.finish_in(&mut journal, &id).with_context(|| {
                format!("the settings files {who} used could not be put back first")
            })?;
        }
        // The originals are kept first, in this swap's own folder: from here a crash only ever
        // puts them back, and no other swap's are touched.
        let originals = self.swap_folder(app, id);
        let mut swapped = Vec::new();
        for (written, path) in files {
            let key = file_key(written);
            let kept = self.read_game(written, path).and_then(|bytes| {
                state::atomic_write(&originals.join(&key), &bytes)
                    .with_context(|| format!("keeping the original of {written}"))
            });
            if let Err(error) = kept {
                let _ = std::fs::remove_dir_all(&originals);
                return Err(error);
            }
            swapped.push(FileSwap {
                written: written.clone(),
                path: path.clone(),
                key,
                step: Step::Placing,
            });
        }
        let id = id.to_owned();
        journal.swaps.push(Swap {
            id: id.clone(),
            app: app.to_owned(),
            app_name: app_name.to_owned(),
            device: device.to_owned(),
            device_name: device_name.to_owned(),
            files: swapped,
            programs: Vec::new(),
            error: None,
        });
        self.save(&journal)?;
        let index = journal.swaps.len() - 1;
        let mut report = Report::default();
        let mut failure = None;
        for file in 0..journal.swaps[index].files.len() {
            let FileSwap {
                written, path, key, ..
            } = journal.swaps[index].files[file].clone();
            let copy = self.device_copy(app, device, &key);
            let placed = match read_store(&copy) {
                Ok(Some(bytes)) => self
                    .write_game(&path, &bytes)
                    .map(|()| report.placed.push(written.clone()))
                    .with_context(|| {
                        format!(
                            "{written} ({}) could not be replaced by the copy of {device_name}",
                            path.display()
                        )
                    }),
                Ok(None) => {
                    report.first.push(written.clone());
                    Ok(())
                }
                Err(error) => Err(anyhow!(error))
                    .with_context(|| format!("reading the copy of {written} for {device_name}")),
            };
            match placed {
                Ok(()) => {
                    journal.swaps[index].files[file].step = Step::Playing;
                    self.save(&journal)?;
                }
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            }
        }
        let Some(error) = failure else {
            return Ok(Begun { id, report });
        };
        // The game never ran: the copies that went in are unchanged, so only the originals go back.
        for file in &mut journal.swaps[index].files {
            if file.step == Step::Playing {
                file.step = Step::Saved;
            }
        }
        self.save(&journal)?;
        match self.finish_in(&mut journal, &id) {
            Ok(_) => Err(error),
            Err(again) => Err(error.context(format!(
                "and the originals could not all be put back yet ({again:#}); the host tries again"
            ))),
        }
    }

    /// The app of swap `id` stopped: save its files as the device's copies and put the
    /// originals back. A file that fails stays journaled for [`Self::recover`].
    pub fn finish(&self, id: &str) -> Result<Report> {
        let _lock = lock();
        let mut journal = self.journal()?;
        self.finish_in(&mut journal, id)
    }

    /// The launch of swap `id` failed after its files went in, before the game ran: only the
    /// originals go back; the device's copies stay as they were.
    pub fn cancel(&self, id: &str) -> Result<Report> {
        let _lock = lock();
        let mut journal = self.journal()?;
        let swap = journal
            .swaps
            .iter_mut()
            .find(|swap| swap.id == id)
            .with_context(|| format!("no game settings swap {id}"))?;
        for file in &mut swap.files {
            if file.step == Step::Playing {
                file.step = Step::Saved;
            }
        }
        self.save(&journal)?;
        self.finish_in(&mut journal, id)
    }

    /// Record `programs` (full paths) as swap `id`'s game: its files stay while one runs.
    pub fn watch(&self, id: &str, programs: &[String]) -> Result<()> {
        let _lock = lock();
        let mut journal = self.journal()?;
        let Some(swap) = journal.swaps.iter_mut().find(|swap| swap.id == id) else {
            return Ok(());
        };
        let mut changed = false;
        for program in programs {
            let program = program.trim();
            if !program.is_empty()
                && !swap
                    .programs
                    .iter()
                    .any(|known| normal(known) == normal(program))
            {
                swap.programs.push(program.to_owned());
                changed = true;
            }
        }
        if changed {
            self.save(&journal)?;
        }
        Ok(())
    }

    /// Finish every journaled swap not in `live` (read under the store's lock): at host start,
    /// and again while one fails or its game still runs.
    pub fn recover(
        &self,
        live: &dyn Fn() -> BTreeSet<String>,
    ) -> Result<Vec<(Swap, Result<Report>)>> {
        let _lock = lock();
        let live = live();
        let mut journal = self.journal()?;
        let ids: Vec<String> = journal
            .swaps
            .iter()
            .filter(|swap| !live.contains(&swap.id))
            .map(|swap| swap.id.clone())
            .collect();
        let mut results = Vec::new();
        for id in ids {
            let swap = journal
                .swaps
                .iter()
                .find(|swap| swap.id == id)
                .cloned()
                .unwrap();
            let result = self.finish_in(&mut journal, &id);
            results.push((swap, result));
        }
        Ok(results)
    }

    fn finish_in(&self, journal: &mut Journal, id: &str) -> Result<Report> {
        let index = journal
            .swaps
            .iter()
            .position(|swap| swap.id == id)
            .with_context(|| format!("no game settings swap {id}"))?;
        let (app, device, who) = {
            let swap = &journal.swaps[index];
            (swap.app.clone(), swap.device.clone(), swap.who())
        };
        // A game still running writes its settings as it exits: over the PC's file, were it
        // back, and past the device's copy. Nothing moves until it has closed.
        if let Some(program) = (self.running)(&journal.swaps[index].programs) {
            let text = format!("{program} is still running; the files go back once it closes");
            journal.swaps[index].error = Some(text.clone());
            self.save(journal)?;
            return Err(anyhow!(StillRunning).context(format!("{who}: {text}")));
        }
        let originals = self.swap_folder(&app, id);
        let mut report = Report::default();
        let mut failures = Vec::new();
        for file in 0..journal.swaps[index].files.len() {
            let FileSwap {
                written,
                path,
                key,
                step,
            } = journal.swaps[index].files[file].clone();
            if step == Step::Playing {
                match self.read_game(&written, &path) {
                    Ok(bytes) => {
                        if let Err(error) = self.keep_copy(&app, &device, &key, &bytes) {
                            // The file is the only place the session's changes are: it stays.
                            failures.push(format!("{written}: saving the copy failed: {error:#}"));
                            continue;
                        }
                        report.saved.push(written.clone());
                    }
                    Err(error) if is_missing(&error) => report.warnings.push(format!(
                        "{written} ({}) was gone when {who} ended, so the saved copy stays as it was; the PC's own file is put back",
                        path.display()
                    )),
                    Err(error) => {
                        failures.push(format!("{error:#}"));
                        continue;
                    }
                }
                journal.swaps[index].files[file].step = Step::Saved;
                self.save(journal)?;
            }
            if matches!(step, Step::Playing | Step::Saved | Step::Placing) {
                let original = originals.join(&key);
                let restored = match read_store(&original) {
                    Ok(Some(bytes)) => self.write_game(&path, &bytes).with_context(|| {
                        format!("{written} ({}) could not be put back", path.display())
                    }),
                    Ok(None) => Err(anyhow!(
                        "{written}: the kept original {} is missing",
                        original.display()
                    )),
                    Err(error) => Err(anyhow!(error))
                        .with_context(|| format!("reading {}", original.display())),
                };
                match restored {
                    Ok(()) => {
                        report.restored.push(written.clone());
                        journal.swaps[index].files[file].step = Step::Done;
                        self.save(journal)?;
                    }
                    Err(error) => failures.push(format!("{error:#}")),
                }
            }
        }
        if failures.is_empty() {
            let swap = journal.swaps.remove(index);
            self.save(journal)?;
            // Every file is back: its original becomes the backup of the PC's last own file.
            let mut all_moved = true;
            for file in &swap.files {
                let kept = originals.join(&file.key);
                if !kept.is_file() {
                    continue;
                }
                let backup = self.original(&app, &file.key);
                let moved = backup
                    .parent()
                    .map_or(Ok(()), std::fs::create_dir_all)
                    .and_then(|()| std::fs::rename(&kept, &backup));
                if let Err(error) = moved {
                    all_moved = false;
                    report.warnings.push(format!(
                        "{}: the original stays in {}, as it could not become the backup {}: {error}",
                        file.written,
                        kept.display(),
                        backup.display()
                    ));
                }
            }
            if all_moved {
                let _ = std::fs::remove_dir_all(&originals);
            }
            return Ok(report);
        }
        let text = failures.join("; ");
        journal.swaps[index].error = Some(text.clone());
        self.save(journal)?;
        bail!("{who}: {text}")
    }

    /// Save `bytes` as the device's copy, its previous copy kept beside it.
    fn keep_copy(&self, app: &str, device: &str, key: &str, bytes: &[u8]) -> Result<()> {
        let copy = self.device_copy(app, device, key);
        if copy.is_file() {
            let mut previous = copy.clone().into_os_string();
            previous.push(".previous");
            std::fs::copy(&copy, PathBuf::from(previous))
                .with_context(|| format!("keeping the previous copy {}", copy.display()))?;
        }
        state::atomic_write(&copy, bytes)
    }

    /// Read a game's file as the user, with why it cannot be read.
    fn read_game(&self, written: &str, path: &Path) -> Result<Vec<u8>> {
        let read = game_files::reach(self.access, || read_file(path)).and_then(|read| read);
        read.map_err(|error| {
            let missing = error.kind() == std::io::ErrorKind::NotFound;
            let why = if missing {
                "does not exist. Start the game once on this PC so it writes its settings, or correct the path under Game settings files".to_owned()
            } else if matches!(error.raw_os_error(), Some(32 | 33)) {
                "is in use by another program (is the game still running?)".to_owned()
            } else {
                format!("could not be read as the Windows user: {error}")
            };
            anyhow!(Missing(missing)).context(format!("{written} ({}) {why}", path.display()))
        })
    }

    /// Write a game's file as the user: replaced in one step when it exists, created when the
    /// game removed it meanwhile.
    fn write_game(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        game_files::reach(self.access, || {
            if std::fs::symlink_metadata(path).is_ok() {
                game_files::replace(path, bytes)
            } else {
                let mut out = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)?;
                out.write_all(bytes)?;
                out.sync_all()
            }
        })
        .and_then(|written| written)
        .map_err(|error| {
            if matches!(error.raw_os_error(), Some(32 | 33)) {
                anyhow!("it is in use by another program (is the game still running?)")
            } else {
                anyhow!(error)
            }
        })
    }
}

/// Marks a finish refused because the swap's game still runs.
#[derive(Debug)]
pub struct StillRunning;
impl std::fmt::Display for StillRunning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the game still runs")
    }
}
impl std::error::Error for StillRunning {}
/// Whether a finish was refused because the swap's game still runs.
pub fn still_running(error: &anyhow::Error) -> bool {
    error.downcast_ref::<StillRunning>().is_some()
}

/// Marks a [`Store::read_game`] error for a file that does not exist.
#[derive(Debug)]
struct Missing(bool);
impl std::fmt::Display for Missing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0 { "missing" } else { "unreadable" })
    }
}
impl std::error::Error for Missing {}
fn is_missing(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<Missing>()
        .is_some_and(|missing| missing.0)
}

/// A settings file: a file of at most [`MAX_FILE_BYTES`].
fn read_file(path: &Path) -> std::io::Result<Vec<u8>> {
    let file = std::fs::File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other("it is not a file"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(std::io::Error::other(
            "it is larger than 16 MiB, more than a settings file",
        ));
    }
    Ok(bytes)
}

/// A file of the store, or None when there is none.
fn read_store(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests;
