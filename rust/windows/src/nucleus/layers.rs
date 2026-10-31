//! Nucleus Co-op's DPI setting, undone. As it starts each instance, Nucleus
//! Co-op 2.4.2 sets the instance's .exe to a DPI mode (`~ HIGHDPIAWARE`) in
//! the user's compatibility layers (`GameDpiAwareness.SetExeDpiAwareness`),
//! replacing the compatibility settings the user chose for that .exe, and
//! never puts them back. A session keeps its game's values from before
//! Nucleus starts and, when it ends however it ends, puts back each one
//! Nucleus' DPI mode still holds; a value the user changed meanwhile stays.
//! Only the game's own path and Nucleus' instance links count, never another
//! game's .exe of the same name. The values wait in a file beside Nucleus
//! until then, so what a host that stopped mid-session left comes back when
//! the service next sees the user sign in at the console or starts a host,
//! or before the next session. A lock file only SYSTEM and administrators
//! open keeps a restore off a running session's values.
use crate::text::to_wide;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;
use windows::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_PATH_NOT_FOUND,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_USERS, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_VALUE_TYPE,
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegEnumValueW, RegOpenKeyExW, RegQueryInfoKeyW,
    RegSetValueExW,
};
use windows::core::{PCWSTR, PWSTR};

/// The compatibility layers in a user's hive: one value per program path.
pub const KEY: &str = r"Software\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\Layers";

/// A registry value: its type and bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Value {
    pub kind: u32,
    pub data: Vec<u8>,
}

/// One user's compatibility layers: their hive on the PC, a map in tests.
pub trait Layers: Send {
    /// The user's SID.
    fn user(&self) -> &str;
    /// Every value by name; none without the key.
    fn values(&mut self) -> Result<BTreeMap<String, Value>>;
    fn set(&mut self, name: &str, value: &Value) -> Result<()>;
    /// Remove a value; one that is gone already is fine.
    fn delete(&mut self, name: &str) -> Result<()>;
}

/// What Nucleus Co-op 2.4.2 sets an instance's .exe to
/// (`GameDpiAwareness.SetExeDpiAwareness`): a value with other data is not
/// Nucleus' (any more) and is never touched.
pub const NUCLEUS_DPI: &str = "~ HIGHDPIAWARE";

/// The values of a game's .exe before Nucleus started.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Whose layers they are.
    pub user: String,
    /// The .exe's file name: Nucleus sets the value of each instance's path
    /// (the game's own, or its links in Nucleus' content folder).
    pub executable: String,
    /// The game's .exe as Nucleus starts it. None in a file an earlier build
    /// wrote, which counts every path of the .exe's file name.
    #[serde(default)]
    pub game: Option<String>,
    /// Nucleus' content folder, where it links each instance's files.
    #[serde(default)]
    pub content: Option<String>,
    pub values: BTreeMap<String, Value>,
}

/// A path as the registry compares value names: case aside, `\` only.
fn plain(path: &str) -> String {
    path.trim().replace('/', "\\").to_lowercase()
}

/// Whether value `name`, a program's path, is one of `executable`.
fn of(name: &str, executable: &str) -> bool {
    let file = |path: &str| path.rsplit(['\\', '/']).next().unwrap_or(path).to_string();
    file(name.trim()).eq_ignore_ascii_case(&file(executable.trim()))
}

impl Snapshot {
    /// Whether value `name` is one Nucleus sets for this session: the game's
    /// own path, or an instance's .exe below Nucleus' content folder.
    /// Another program with the same file name (every RPG Maker game is
    /// `Game.exe`) is not.
    fn covers(&self, name: &str) -> bool {
        if self.game.is_none() && self.content.is_none() {
            return of(name, &self.executable);
        }
        let name = plain(name);
        let game = self.game.as_deref().is_some_and(|game| plain(game) == name);
        let instance = self.content.as_deref().is_some_and(|content| {
            let content = plain(content);
            let content = content.trim_end_matches('\\');
            !content.is_empty()
                && name
                    .strip_prefix(content)
                    .is_some_and(|rest| rest.starts_with('\\'))
                && of(&name, &self.executable)
        });
        game || instance
    }

    /// The value the snapshot keeps for `name`, whatever its case.
    fn kept(&self, name: &str) -> Option<&Value> {
        self.values.get(name).or_else(|| {
            self.values
                .iter()
                .find(|(n, _)| plain(n) == plain(name))
                .map(|(_, v)| v)
        })
    }
}

/// Whether `value` is what Nucleus writes: [`NUCLEUS_DPI`] as REG_SZ.
fn nucleus_wrote(value: &Value) -> bool {
    value_text(value).is_some_and(|text| text == NUCLEUS_DPI)
}

/// A REG_SZ value's text; None for another type.
fn value_text(value: &Value) -> Option<String> {
    const REG_SZ: u32 = 1;
    if value.kind != REG_SZ || !value.data.len().is_multiple_of(2) {
        return None;
    }
    let text: Vec<u16> = value
        .data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&pair| u16::from_le_bytes(pair))
        .collect();
    let end = text.iter().rposition(|&c| c != 0).map_or(0, |at| at + 1);
    String::from_utf16(&text[..end]).ok()
}

/// The layers `values` give the program at `exe` (`~ RUNASADMIN`),
/// whatever the case of its name; None without one.
pub fn program_layers(values: &BTreeMap<String, Value>, exe: &str) -> Option<String> {
    let exe = plain(exe);
    values
        .iter()
        .find(|(name, _)| plain(name) == exe)
        .and_then(|(_, value)| value_text(value))
}

/// The machine's compatibility layers (`HKEY_LOCAL_MACHINE`), which apply
/// to every user; none without the key.
pub fn machine_values() -> Result<BTreeMap<String, Value>> {
    use windows::Win32::System::Registry::HKEY_LOCAL_MACHINE;
    let path = to_wide(KEY);
    let mut key = HKEY::default();
    // SAFETY: the path is terminated and outlives the call; `key` is an
    // out-pointer, closed by `Key` once opened.
    let opened = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(path.as_ptr()),
            Some(0),
            KEY_QUERY_VALUE,
            &mut key,
        )
    };
    if opened == ERROR_FILE_NOT_FOUND || opened == ERROR_PATH_NOT_FOUND {
        return Ok(BTreeMap::new());
    }
    opened
        .ok()
        .with_context(|| format!(r"opening HKEY_LOCAL_MACHINE\{KEY}"))?;
    read_values(&Key(key)).with_context(|| format!(r"reading HKEY_LOCAL_MACHINE\{KEY}"))
}

/// The values in `layers` now that Nucleus would set for `executable` at
/// `game`, with its instances below `content`.
pub fn snapshot(
    layers: &mut dyn Layers,
    executable: &str,
    game: &str,
    content: &Path,
) -> Result<Snapshot> {
    let mut snapshot = Snapshot {
        user: layers.user().to_string(),
        executable: executable.to_string(),
        game: Some(game.to_string()),
        content: Some(content.to_string_lossy().into_owned()),
        values: BTreeMap::new(),
    };
    snapshot.values = layers
        .values()?
        .into_iter()
        .filter(|(name, _)| snapshot.covers(name))
        .collect();
    Ok(snapshot)
}

/// Put `snapshot` back where Nucleus' DPI mode still is: such a value gets
/// its old data, or goes if it had none. A value the user changed or
/// removed since stays as they left it. Returns how many values it changed.
pub fn restore(layers: &mut dyn Layers, snapshot: &Snapshot) -> Result<usize> {
    let now = layers.values()?;
    let mut changed = 0;
    for (name, value) in now.iter().filter(|(name, _)| snapshot.covers(name)) {
        if !nucleus_wrote(value) {
            continue;
        }
        match snapshot.kept(name) {
            Some(old) if old == value => continue,
            Some(old) => layers.set(name, old)?,
            None => layers.delete(name)?,
        }
        changed += 1;
    }
    Ok(changed)
}

/// The file in Nucleus' `folder` that keeps `user`'s values until they are
/// restored.
fn journal(folder: &Path, user: &str) -> PathBuf {
    folder.join(format!("Rubylight-layers-{user}.json"))
}

/// Write `snapshot` to `journal` whole: a temporary file, on the disk before
/// it is renamed over it, so a crash or power loss never leaves half a file.
fn write_journal(journal: &Path, snapshot: &Snapshot) -> Result<()> {
    let text = serde_json::to_vec_pretty(snapshot)?;
    let temporary = journal.with_extension("json.tmp");
    let mut file = std::fs::File::create(&temporary)
        .with_context(|| format!("writing {}", temporary.display()))?;
    file.write_all(&text)
        .and_then(|()| file.sync_all())
        .with_context(|| format!("writing {}", temporary.display()))?;
    drop(file);
    std::fs::rename(&temporary, journal).with_context(|| format!("replacing {}", journal.display()))
}

/// Remove a file restored from; one already gone is fine.
fn remove_journal(journal: &Path) -> Result<()> {
    match std::fs::remove_file(journal) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(error).with_context(|| format!("removing {}", journal.display()))
        }
        _ => Ok(()),
    }
}

/// How long a session's start or check waits for a restore by the service
/// or another check, each a few registry calls.
pub(super) const LOCK_WAIT: Duration = Duration::from_secs(5);
/// How often a waiting start or check tries the lock again.
const LOCK_POLL: Duration = Duration::from_millis(50);
/// The folder below the data root ([`butterpollo_core::paths::data_root`])
/// that keeps the locks.
const LOCK_FOLDER: &str = "nucleus-locks";

/// Where the locks are kept, and who may own that folder: its owner may
/// change its permissions, and so hold or replace any lock in it.
struct Locks {
    folder: PathBuf,
    owners: Vec<String>,
    /// Creates `folder` with SYSTEM's and Administrators' permissions from the
    /// start, so nobody else can open it before they are set; AlreadyExists
    /// when it is there.
    create: fn(&Path) -> std::io::Result<()>,
    /// Leaves `folder` and the files in it to SYSTEM and Administrators.
    restrict: fn(&Path) -> Result<()>,
}

impl Locks {
    /// `%ProgramData%\Butterpollo\nucleus-locks`.
    #[cfg(not(test))]
    fn installed() -> Self {
        Self::administrators(butterpollo_core::paths::data_root().join(LOCK_FOLDER))
    }

    /// Tests keep theirs in a folder of their own.
    #[cfg(test)]
    fn installed() -> Self {
        tests::locks()
    }

    /// Locks in `folder`, which SYSTEM or Administrators own and alone may
    /// write.
    fn administrators(folder: PathBuf) -> Self {
        Self {
            folder,
            owners: vec!["S-1-5-18".into(), "S-1-5-32-544".into()],
            create: crate::process::create_folder_administrators_only,
            restrict: crate::process::restrict_to_administrators,
        }
    }

    /// The lock file of `journal`: one per file, the same in every process
    /// (FNV-1a of its path, case aside).
    fn file(&self, journal: &Path) -> PathBuf {
        let hash = journal
            .to_string_lossy()
            .to_lowercase()
            .bytes()
            .fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
                (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
            });
        self.folder.join(format!("{hash:016x}.lock"))
    }

    /// The lock file of `journal`, open without sharing; None while another
    /// handle has it open.
    fn open(&self, journal: &Path) -> Result<Option<std::fs::File>> {
        use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
        use windows::Win32::Foundation::ERROR_SHARING_VIOLATION;
        use windows::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
        const REPARSE_POINT: u32 = 0x400;
        let folder = &self.folder;
        // Made with its final permissions, never given them after: a user who
        // opened a folder that inherited the data root's read access in
        // between would hold every lock in it.
        match (self.create)(folder) {
            Err(error) if error.kind() != std::io::ErrorKind::AlreadyExists => {
                return Err(error).with_context(|| format!("creating {}", folder.display()));
            }
            _ => {}
        }
        // Open while the lock file is opened in it, so it is the folder
        // checked: it can't be renamed or deleted meanwhile.
        let open = crate::process::open_unfollowed(folder, true)
            .with_context(|| format!("opening {}", folder.display()))?;
        let metadata = open
            .metadata()
            .with_context(|| format!("reading {}", folder.display()))?;
        anyhow::ensure!(
            metadata.file_attributes() & REPARSE_POINT == 0,
            "{} is a link to elsewhere, not Rubylight's folder, so Rubylight starts no Nucleus Co-op session: delete it",
            folder.display()
        );
        let owner = crate::process::open_file_owner(&open)
            .with_context(|| format!("reading the owner of {}", folder.display()))?;
        anyhow::ensure!(
            self.owners.contains(&owner),
            "{} is owned by {owner}, not by SYSTEM or Administrators, so another program may hold or replace its locks; Rubylight starts no Nucleus Co-op session until it is deleted",
            folder.display()
        );
        // Also repairs the permissions of a lock file there already.
        (self.restrict)(folder)?;
        let path = self.file(journal);
        let opened = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .share_mode(0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(&path);
        let file = match opened {
            Ok(file) => file,
            Err(error) if error.raw_os_error() == Some(ERROR_SHARING_VIOLATION.0 as i32) => {
                return Ok(None);
            }
            Err(error) => return Err(error).with_context(|| format!("opening {}", path.display())),
        };
        let metadata = file
            .metadata()
            .with_context(|| format!("reading {}", path.display()))?;
        anyhow::ensure!(
            metadata.is_file() && metadata.file_attributes() & REPARSE_POINT == 0,
            "{} is a link or not a file, not Rubylight's lock, so Rubylight starts no Nucleus Co-op session: delete it",
            path.display()
        );
        drop(open);
        Ok(Some(file))
    }
}

/// A journal in use (a user's compatibility settings, or Nucleus'
/// `Settings.ini` values), by a session from before Nucleus starts until its
/// values are back, or by a restore: a restore never puts back a running
/// session's values, and two never restore the same file. A file below the
/// data root ([`Locks`]) open without sharing: only SYSTEM and
/// administrators may open it, and a name nobody else can make first, as
/// they could a named mutex's. Windows closes the handle of a host that
/// stopped, which lets go of it, whichever thread took it.
pub(super) struct Lock {
    _file: std::fs::File,
}

impl Lock {
    /// The lock, or None while a session or another restore holds it.
    pub(super) fn try_take(journal: &Path) -> Result<Option<Self>> {
        Self::acquire(&Locks::installed(), journal, Duration::ZERO)
    }

    /// The lock, waiting up to `wait` for a restore to finish.
    pub(super) fn take(journal: &Path, wait: Duration) -> Result<Self> {
        Self::acquire(&Locks::installed(), journal, wait)?.with_context(|| {
            format!(
                "{} stays in use: a Nucleus Co-op session or the Rubylight service is putting back the settings it keeps. Start the stream again once that session ends",
                journal.display()
            )
        })
    }

    fn acquire(locks: &Locks, journal: &Path, wait: Duration) -> Result<Option<Self>> {
        let started = std::time::Instant::now();
        loop {
            let file = locks.open(journal).with_context(|| {
                format!(
                    "taking the lock on {}, which keeps what a session changed",
                    journal.display()
                )
            })?;
            if let Some(file) = file {
                return Ok(Some(Self { _file: file }));
            }
            let left = wait.saturating_sub(started.elapsed());
            if left.is_zero() {
                return Ok(None);
            }
            std::thread::sleep(left.min(LOCK_POLL));
        }
    }
}

/// A session's values, restored once when it is dropped or
/// [`restore`](Guard::restore) is called. It holds its user's [`Lock`] until
/// then.
pub struct Guard {
    layers: Box<dyn Layers>,
    snapshot: Snapshot,
    journal: PathBuf,
    restored: bool,
    /// Held until the guard drops; tests let go of it to stand in for a
    /// host that stopped.
    _lock: Option<Lock>,
}

impl Guard {
    /// Before the Nucleus Co-op at `nucleus` starts `executable` (the file
    /// name) of the game at `game`: restore what a session the host never
    /// ended left, then keep the values now, in a file in `folder`, the
    /// host's copy (which is `nucleus` unless the session runs the user's
    /// own copy).
    pub fn take(
        mut layers: Box<dyn Layers>,
        folder: &Path,
        nucleus: &Path,
        executable: &str,
        game: &str,
    ) -> Result<Self> {
        let journal = journal(folder, layers.user());
        let lock = Lock::take(&journal, LOCK_WAIT)?;
        restore_left(layers.as_mut(), &journal)?;
        let snapshot = snapshot(layers.as_mut(), executable, game, &nucleus.join("content"))?;
        write_journal(&journal, &snapshot)?;
        Ok(Self {
            layers,
            snapshot,
            journal,
            restored: false,
            _lock: Some(lock),
        })
    }

    /// Put the values back, once. Should that fail, they stay in the file
    /// for the next session.
    pub fn restore(&mut self) {
        if std::mem::replace(&mut self.restored, true) {
            return;
        }
        match restore(self.layers.as_mut(), &self.snapshot) {
            Ok(changed) => {
                tracing::info!(
                    executable = self.snapshot.executable,
                    changed,
                    "Nucleus Co-op: the game's compatibility settings are back as they were"
                );
                if let Err(error) = remove_journal(&self.journal) {
                    tracing::warn!(
                        error = format!("{error:#}"),
                        file = %self.journal.display(),
                        "Nucleus Co-op: the restored compatibility settings' file was not removed"
                    );
                }
            }
            Err(error) => tracing::error!(
                error = format!("{error:#}"),
                file = %self.journal.display(),
                "Nucleus Co-op: the game's compatibility settings were not put back; the next session tries again"
            ),
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Restore the values in `journal`, which a session the host never ended
/// left, and remove it. A session must not start before: its own snapshot
/// would keep Nucleus' values as the user's. The caller holds the [`Lock`].
fn restore_left(layers: &mut dyn Layers, journal: &Path) -> Result<()> {
    let text = match std::fs::read(journal) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "reading {}, which keeps the compatibility settings an earlier Nucleus Co-op session changed",
                    journal.display()
                )
            });
        }
    };
    // Created but never written to the disk, as power loss can leave a
    // rename the file system logged before the data: nothing to put back.
    if text.iter().all(u8::is_ascii_whitespace) {
        tracing::warn!(
            file = %journal.display(),
            "Nucleus Co-op: the compatibility settings' file of an earlier session is empty; removing it"
        );
        return remove_journal(journal);
    }
    // Each error names the file: deleting it lets sessions start again, with
    // the settings it kept left as Nucleus set them.
    let left = serde_json::from_slice::<Snapshot>(&text).map_err(|error| {
        anyhow::anyhow!(
            "{} can't be read ({error}): delete it, then start the stream again. The compatibility settings it kept stay as Nucleus Co-op set them",
            journal.display()
        )
    })?;
    let changed = restore(layers, &left).with_context(|| {
        format!(
            "the compatibility settings an earlier Nucleus Co-op session changed were not put back from {}; deleting it lets sessions start without putting them back",
            journal.display()
        )
    })?;
    tracing::info!(
        executable = left.executable,
        changed,
        "Nucleus Co-op: an earlier session's compatibility settings are back as they were"
    );
    remove_journal(journal)
}

/// Restore what a session the host never ended left for the user of
/// `layers` in the Nucleus Co-op at `folder`. Checked before a session
/// closes the console's window; it waits for a restore by the service.
pub fn restore_left_in(layers: &mut dyn Layers, folder: &Path) -> Result<()> {
    let journal = journal(folder, layers.user());
    let _lock = Lock::take(&journal, LOCK_WAIT)?;
    restore_left(layers, &journal)
}

/// What [`restore_at_console`] did.
#[derive(Debug, PartialEq, Eq)]
pub enum AtConsole {
    /// Put back, or nothing was left for the user at the console.
    Done,
    /// Their session runs and puts its values back itself.
    Busy,
    /// Nobody is signed in at the console (`why`); the files in `waiting`
    /// wait for their users.
    NoUser { why: String, waiting: Vec<PathBuf> },
}

/// For the service, in session 0: restore what a session left for the user
/// signed in at the console, in the Nucleus Co-op `configured` names
/// (`nucleus_path`) or the host's own copy below `data`. A session of
/// theirs that runs keeps its values.
pub fn restore_at_console(data: &Path, configured: &str) -> Result<AtConsole> {
    let Some(folder) = super::install::installed(data, configured)? else {
        return Ok(AtConsole::Done);
    };
    restore_console_with(
        &folder,
        crate::process::console_session(),
        crate::process::session_user_sid,
        |sid| Box::new(UserHive::of(sid)),
    )
}

/// [`restore_at_console`] in the Nucleus Co-op at `folder`, with `console`
/// the console's session, `sid_of` the user signed in to a session and
/// `hive` their layers.
fn restore_console_with(
    folder: &Path,
    console: u32,
    sid_of: impl FnOnce(u32) -> Result<String>,
    hive: impl FnOnce(String) -> Box<dyn Layers>,
) -> Result<AtConsole> {
    // WTSGetActiveConsoleSessionId while the console is between sessions.
    const NO_SESSION: u32 = u32::MAX;
    let sid = if console == NO_SESSION {
        Err(anyhow::anyhow!("no session is attached to the console"))
    } else {
        sid_of(console).with_context(|| format!("nobody is signed in to session {console}"))
    };
    let sid = match sid {
        Ok(sid) => sid,
        Err(why) => {
            return Ok(AtConsole::NoUser {
                why: format!("{why:#}"),
                waiting: waiting(folder)?,
            });
        }
    };
    let mut layers = hive(sid);
    let journal = journal(folder, layers.user());
    if !journal.exists() {
        return Ok(AtConsole::Done);
    }
    let Some(_lock) = Lock::try_take(&journal)? else {
        return Ok(AtConsole::Busy);
    };
    restore_left(layers.as_mut(), &journal)?;
    Ok(AtConsole::Done)
}

/// The files in Nucleus' `folder` that keep a user's values.
fn waiting(folder: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in
        std::fs::read_dir(folder).with_context(|| format!("listing {}", folder.display()))?
    {
        let path = entry
            .with_context(|| format!("listing {}", folder.display()))?
            .path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if name.starts_with("Rubylight-layers-") && name.ends_with(".json") {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

/// A user's layers in their hive (`HKEY_USERS\<SID>`), which Nucleus Co-op,
/// running as them, sees as its own.
pub struct UserHive {
    sid: String,
    path: Vec<u16>,
}

impl UserHive {
    /// The user the host, in their session, runs Nucleus Co-op as.
    pub fn signed_in() -> Result<Self> {
        let sid = crate::process::user_sid().context(
            "finding the signed-in user, whose compatibility settings Nucleus Co-op changes",
        )?;
        Ok(Self::of(sid))
    }

    /// The user `sid`'s.
    pub fn of(sid: String) -> Self {
        let path = to_wide(&format!(r"{sid}\{KEY}"));
        Self { sid, path }
    }

    /// Whether the user's hive is loaded, as it is while they are signed in.
    /// Without it, their layers can't be read: none is not "no values".
    fn loaded(&self) -> Result<()> {
        let root = to_wide(&self.sid);
        let mut key = HKEY::default();
        // SAFETY: the SID is terminated and outlives the call; `key` is an
        // out-pointer, closed by `Key` once opened.
        let opened = unsafe {
            RegOpenKeyExW(
                HKEY_USERS,
                PCWSTR(root.as_ptr()),
                Some(0),
                KEY_QUERY_VALUE,
                &mut key,
            )
        };
        if opened == ERROR_FILE_NOT_FOUND || opened == ERROR_PATH_NOT_FOUND {
            anyhow::bail!(
                r"HKEY_USERS\{} is not loaded: the user is not signed in, so their compatibility settings wait until they are",
                self.sid
            );
        }
        opened
            .ok()
            .with_context(|| format!(r"opening HKEY_USERS\{}", self.sid))?;
        drop(Key(key));
        Ok(())
    }

    /// The key opened with `access`; None when it is not there in the
    /// user's loaded hive.
    fn open(&self, access: windows::Win32::System::Registry::REG_SAM_FLAGS) -> Result<Option<Key>> {
        let mut key = HKEY::default();
        // SAFETY: the path is terminated and outlives the call; `key` is an
        // out-pointer, closed by `Key` once opened.
        let opened = unsafe {
            RegOpenKeyExW(
                HKEY_USERS,
                PCWSTR(self.path.as_ptr()),
                Some(0),
                access,
                &mut key,
            )
        };
        if opened == ERROR_FILE_NOT_FOUND || opened == ERROR_PATH_NOT_FOUND {
            self.loaded()?;
            return Ok(None);
        }
        opened.ok().with_context(|| self.describe("opening"))?;
        Ok(Some(Key(key)))
    }

    fn describe(&self, what: &str) -> String {
        format!(r"{what} HKEY_USERS\{}\{KEY}", self.sid)
    }
}

/// An open registry key, closed on drop.
struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the key was opened and is closed once.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

impl Layers for UserHive {
    fn user(&self) -> &str {
        &self.sid
    }

    fn values(&mut self) -> Result<BTreeMap<String, Value>> {
        let Some(key) = self.open(KEY_QUERY_VALUE)? else {
            return Ok(BTreeMap::new());
        };
        read_values(&key).with_context(|| self.describe("reading"))
    }

    fn set(&mut self, name: &str, value: &Value) -> Result<()> {
        let mut key = HKEY::default();
        // SAFETY: the path is terminated and outlives the call; `key` is an
        // out-pointer, closed by `Key` once created.
        unsafe {
            RegCreateKeyExW(
                HKEY_USERS,
                PCWSTR(self.path.as_ptr()),
                None,
                None,
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                None,
                &mut key,
                None,
            )
        }
        .ok()
        .with_context(|| self.describe("opening"))?;
        let key = Key(key);
        let wide = to_wide(name);
        // SAFETY: the name is terminated and the data a live slice.
        unsafe {
            RegSetValueExW(
                key.0,
                PCWSTR(wide.as_ptr()),
                None,
                REG_VALUE_TYPE(value.kind),
                Some(&value.data),
            )
        }
        .ok()
        .with_context(|| self.describe(&format!("setting {name} in")))
    }

    fn delete(&mut self, name: &str) -> Result<()> {
        let Some(key) = self.open(KEY_SET_VALUE)? else {
            return Ok(());
        };
        let wide = to_wide(name);
        // SAFETY: the key is open and the name terminated.
        let deleted = unsafe { RegDeleteValueW(key.0, PCWSTR(wide.as_ptr())) };
        if deleted == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        deleted
            .ok()
            .with_context(|| self.describe(&format!("removing {name} from")))
    }
}

/// Every value of `key`. A value that grew while it was read is read again
/// with room for it.
fn read_values(key: &Key) -> Result<BTreeMap<String, Value>> {
    let (mut name_chars, mut data_bytes) = (0u32, 0u32);
    // SAFETY: the key is open; the counts are out-pointers.
    unsafe {
        RegQueryInfoKeyW(
            key.0,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(&mut name_chars),
            Some(&mut data_bytes),
            None,
            None,
        )
    }
    .ok()?;
    let mut name = vec![0u16; name_chars as usize + 1];
    let mut data = vec![0u8; (data_bytes as usize).max(1)];
    let mut values = BTreeMap::new();
    let mut index = 0;
    let mut retries = 0;
    loop {
        let mut name_len = name.len() as u32;
        let mut data_len = data.len() as u32;
        let mut kind = 0u32;
        // SAFETY: the key is open, and the buffers and their lengths are
        // live for the call.
        let result = unsafe {
            RegEnumValueW(
                key.0,
                index,
                Some(PWSTR(name.as_mut_ptr())),
                &mut name_len,
                None,
                Some(&mut kind),
                Some(data.as_mut_ptr()),
                Some(&mut data_len),
            )
        };
        if result == ERROR_NO_MORE_ITEMS {
            return Ok(values);
        }
        if result == ERROR_MORE_DATA && retries < 8 {
            retries += 1;
            name.resize(name.len() * 2, 0);
            data.resize((data_len as usize).max(data.len() * 2), 0);
            continue;
        }
        result.ok()?;
        values.insert(
            String::from_utf16_lossy(&name[..name_len as usize]),
            Value {
                kind,
                data: data[..data_len as usize].to_vec(),
            },
        );
        index += 1;
        retries = 0;
    }
}

#[cfg(test)]
pub mod tests;
