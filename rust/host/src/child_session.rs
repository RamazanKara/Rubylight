//! What is left of background streaming in a Windows child session, which
//! Rubylight removed: Windows signs a child session in only with a Windows
//! password, and a Microsoft account that uses only Windows Hello has none.
//! Development builds set it up on some PCs, so at every start the host puts
//! back the Windows settings that setup changed, deletes the password those
//! builds saved and the child host's profile ([`retire_at_startup`]); the
//! console's Restore again does the same ([`restore_again`]), and so does
//! `--child-session-restore`, which the uninstaller runs. The rules are
//! `butterpollo_core::child_session`, ported from Annex (MIT).
use crate::state::Shared;
use anyhow::{Context, Result, bail};
use butterpollo_core::child_session::{self as logic, SetupState, SystemSettings, Unrecorded};
use butterpollo_windows::child_session as native;
use serde_json::{Value, json};
use std::{
    fs::File,
    io::{ErrorKind, Read, Write},
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard, PoisonError},
};

/// The removed child session's own host kept its profile here, inside this
/// host's.
const CHILD_PROFILE: &str = "child-session";

/// The PC's one record of what setup changed ([`logic::setup_record`]),
/// whichever Rubylight profile set it up: two profiles (the service's, a host
/// started by hand) each with their own would record each other's changes as
/// the originals. A record in a profile folder, which its user can write, is
/// never read: restore would put back what it says.
#[derive(Clone)]
struct Record {
    path: PathBuf,
    /// Create the new folder, and the new file open for writing alone, with
    /// SYSTEM and Administrators their only owner and writers from the start;
    /// AlreadyExists when anything has the name, which is left as it is.
    /// Users may read both: they hold nothing secret, and a host without
    /// administrator rights reads the record to say that it is still there.
    /// Only restore saves it now, to note what is back already.
    create_folder: fn(&Path) -> std::io::Result<()>,
    create_file: fn(&Path) -> std::io::Result<File>,
    /// Err unless SYSTEM or Administrators own the open file or folder, which
    /// `path` names, and alone may write it; [`Foreign`] when it is not theirs.
    trusted: fn(&File, &Path) -> Result<()>,
    /// Err unless SYSTEM or Administrators own the open data root the folder
    /// is made in (`%ProgramData%\Butterpollo`), which `path` names, and alone
    /// may write it, and it is no link elsewhere.
    trusted_root: fn(&File, &Path) -> Result<()>,
    /// Whether this process has administrator rights, which restore needs.
    elevated: fn() -> Result<bool>,
}
impl Record {
    /// This PC's record.
    fn new() -> Self {
        Self {
            path: logic::setup_record(),
            create_folder: butterpollo_windows::process::create_folder_administrators_readable,
            create_file: butterpollo_windows::process::create_file_administrators_readable,
            trusted: administrators_own,
            trusted_root: administrators_own_root,
            elevated: butterpollo_windows::process::elevated,
        }
    }
    fn folder(&self) -> &Path {
        self.path.parent().unwrap_or(&self.path)
    }
    /// Whether setup may have run: the record is there, or that can't be told.
    fn may_exist(&self) -> bool {
        !matches!(self.path.try_exists(), Ok(false))
    }
    /// The record; None when setup never ran. Err for one SYSTEM or
    /// Administrators do not own (any program can write `%ProgramData%`),
    /// which is never used. Checked and read through one handle, so what is
    /// read is what was checked.
    fn load(&self) -> Result<Option<SetupState>> {
        let path = &self.path;
        // Held until the file is read: it can't be swapped for another folder meanwhile.
        let Some(_folder) = self.open_folder()? else {
            return Ok(None);
        };
        let mut file = match butterpollo_windows::process::open_unfollowed(path, false) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
        };
        (self.trusted)(&file, path)?;
        let mut text = String::new();
        file.read_to_string(&mut text)
            .with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&text)
            .map(Some)
            .with_context(|| format!("reading {}", path.display()))
    }
    /// The PC's folder, open and checked; None when there is none. It can't
    /// be renamed or deleted while open.
    fn open_folder(&self) -> Result<Option<File>> {
        let folder = self.folder();
        match butterpollo_windows::process::open_unfollowed(folder, true) {
            Ok(open) => {
                (self.trusted)(&open, folder)?;
                Ok(Some(open))
            }
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).with_context(|| format!("opening {}", folder.display())),
        }
    }
    /// Save it in a folder only SYSTEM and Administrators can write, in a
    /// data root they own. The folder and file are made with those
    /// permissions, never given them after: in between, another program could
    /// open them for writing.
    fn save(&self, state: &SetupState) -> Result<()> {
        let text = serde_json::to_vec_pretty(state)?;
        let folder = self.folder();
        // Held while the folder is made in it.
        let _root = folder
            .parent()
            .map(|root| self.open_root(root))
            .transpose()?;
        match (self.create_folder)(folder) {
            // One that was there is checked as it is opened.
            Err(error) if error.kind() != ErrorKind::AlreadyExists => {
                return Err(error).with_context(|| format!("creating {}", folder.display()));
            }
            _ => {}
        }
        // Held while the record is written into it.
        let _folder = self
            .open_folder()?
            .with_context(|| format!("{} is gone", folder.display()))?;
        let temporary = self.path.with_extension("tmp");
        // One already there is never written into, only replaced: only SYSTEM and Administrators
        // can write the folder (checked as it was opened) and setups wait for each other, so a
        // setup cut short between making and renaming it left it.
        let mut file = match (self.create_file)(&temporary) {
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                std::fs::remove_file(&temporary).with_context(|| {
                    format!(
                        "removing {}, which a setup cut short left",
                        temporary.display()
                    )
                })?;
                (self.create_file)(&temporary)
            }
            created => created,
        }
        .with_context(|| format!("creating {}", temporary.display()))?;
        let written = file
            .write_all(&text)
            .and_then(|()| file.sync_all())
            .with_context(|| format!("writing {}", temporary.display()));
        // Closed first: it is shared with nothing, not even the rename.
        drop(file);
        let saved = written.and_then(|()| {
            std::fs::rename(&temporary, &self.path)
                .with_context(|| format!("saving {}", self.path.display()))
        });
        if saved.is_err() {
            // Ours, and it would hold off the next setup.
            let _ = std::fs::remove_file(&temporary);
        }
        saved
    }
    /// The data root `root`, open and checked. Made as the installer makes it
    /// when there is none (SYSTEM and Administrators its owner and only
    /// writers), never with its parents: `%ProgramData%` is there. It can't be
    /// renamed or deleted while open.
    fn open_root(&self, root: &Path) -> Result<File> {
        match (self.create_folder)(root) {
            // One that was there is checked as it is opened.
            Err(error) if error.kind() != ErrorKind::AlreadyExists => {
                return Err(error).with_context(|| format!("creating {}", root.display()));
            }
            _ => {}
        }
        let open = butterpollo_windows::process::open_unfollowed(root, true)
            .with_context(|| format!("opening {}", root.display()))?;
        (self.trusted_root)(&open, root)?;
        Ok(open)
    }
    /// Forget it, once everything is restored.
    fn remove(&self) -> Result<()> {
        match std::fs::remove_file(&self.path) {
            Err(error) if error.kind() != ErrorKind::NotFound => {
                Err(error).with_context(|| format!("removing {}", self.path.display()))
            }
            _ => Ok(()),
        }
    }
}
/// What [`Record`]'s `trusted` refuses: a link, or a file or folder SYSTEM
/// and Administrators do not own and alone write, which setup never made.
/// One that can't be checked is a plain error.
#[derive(Debug)]
struct Foreign {
    /// The record's folder: setup makes it for SYSTEM and Administrators
    /// alone, in a data root only they can write, so one that is not holds no
    /// setup of Rubylight's.
    folder: bool,
    problem: String,
}
impl std::fmt::Display for Foreign {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.problem)
    }
}
impl std::error::Error for Foreign {}
/// Whether `error` is the record's folder refused as [`Foreign`].
fn foreign_folder(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<Foreign>()
        .is_some_and(|foreign| foreign.folder)
}
/// [`Record`]'s check on this PC, on the open `file` (`path` names it):
/// owned by SYSTEM or Administrators, which alone may write it, and not a
/// link elsewhere; [`Foreign`] when it is not.
fn administrators_own(file: &File, path: &Path) -> Result<()> {
    use std::os::windows::fs::MetadataExt;
    let metadata = file
        .metadata()
        .with_context(|| format!("reading {}", path.display()))?;
    let foreign = |problem: String| -> Result<()> {
        Err(Foreign {
            folder: metadata.is_dir(),
            problem,
        }
        .into())
    };
    if metadata.file_attributes() & 0x400 != 0 {
        return foreign(format!(
            "{} is a link to elsewhere, not Rubylight's; Rubylight does not use it",
            path.display()
        ));
    }
    let owner = butterpollo_windows::process::open_file_owner(file)
        .with_context(|| format!("reading the owner of {}", path.display()))?;
    if !matches!(owner.as_str(), "S-1-5-18" | "S-1-5-32-544") {
        return foreign(format!(
            "{} is owned by {owner}, not by SYSTEM or Administrators, so another program may have written it; Rubylight does not use it. Delete it, and turn Remote Desktop off if you don't use it",
            path.display()
        ));
    }
    let writers = butterpollo_windows::process::open_file_writers(file)
        .with_context(|| format!("reading the permissions of {}", path.display()))?;
    if let Some(other) = writers
        .iter()
        .find(|sid| !matches!(sid.as_str(), "S-1-5-18" | "S-1-5-32-544"))
    {
        return foreign(format!(
            "{} can be written by {other}, not only by SYSTEM and Administrators, so another program may have written it; Rubylight does not use it. Delete it, and turn Remote Desktop off if you don't use it",
            path.display()
        ));
    }
    Ok(())
}
/// [`Record`]'s check of the data root on this PC, on the open `file`
/// (`path` names it): owned by SYSTEM or Administrators, which alone may
/// write it, as the installer leaves it, and not a link elsewhere. An owner
/// may change its permissions and move the record's folder out of it; a
/// writer may make that folder first.
fn administrators_own_root(file: &File, path: &Path) -> Result<()> {
    use std::os::windows::fs::MetadataExt;
    let metadata = file
        .metadata()
        .with_context(|| format!("reading {}", path.display()))?;
    if metadata.file_attributes() & 0x400 != 0 {
        bail!(
            "{} is a link to elsewhere, not Rubylight's folder, so Rubylight does not keep its child-session record in it; nothing was changed",
            path.display()
        );
    }
    let owner = butterpollo_windows::process::open_file_owner(file)
        .with_context(|| format!("reading the owner of {}", path.display()))?;
    if !matches!(owner.as_str(), "S-1-5-18" | "S-1-5-32-544") {
        bail!(
            "{} is owned by {owner}, not by SYSTEM or Administrators, so another program may control it; Rubylight does not keep its child-session record in it and nothing was changed. Run Rubylight's setup again, which gives it to Administrators, then restore again",
            path.display()
        );
    }
    let writers = butterpollo_windows::process::open_file_writers(file)
        .with_context(|| format!("reading the permissions of {}", path.display()))?;
    if let Some(other) = writers
        .iter()
        .find(|sid| !matches!(sid.as_str(), "S-1-5-18" | "S-1-5-32-544"))
    {
        bail!(
            "{} can be written by {other}, not only by SYSTEM and Administrators, so another program may make its own folders in it; Rubylight does not keep its child-session record in it and nothing was changed. Run Rubylight's setup again, which leaves it to Administrators, then restore again",
            path.display()
        );
    }
    Ok(())
}

/// Child sessions were removed: once, at the host's start, on a thread off
/// every stream's path, [`retire`] what their setup left on this PC. Nothing
/// retries on a timer: the next start does, or Restore again under
/// Maintenance ([`restore_again`]).
pub(crate) fn retire_at_startup(config_dir: PathBuf) {
    std::thread::spawn(move || {
        let left = retire(&Record::new(), &config_dir, &Native);
        *left_over() = left;
    });
}
/// Restore again, from Maintenance: [`retire`] once more, now.
pub(crate) fn restore_again(h: &Shared) {
    let left = retire(&Record::new(), &h.directory, &Native);
    *left_over() = left;
}
/// What the console shows: whether Windows settings the removed feature's
/// setup changed are still changed (its record is there, or its block rule
/// stayed), and why the last try failed.
pub(crate) fn status() -> Value {
    status_with(&Record::new(), &left_over())
}
fn status_with(record: &Record, left: &Left) -> Value {
    json!({
        "pending": left.block_rule || record.may_exist(),
        "problem": (!left.problems.is_empty()).then(|| left.problems.join(" ")),
    })
}

/// What [`retire`] could not clean, for the console.
#[derive(Debug, Default, PartialEq, Eq)]
struct Left {
    /// Setup's block rule is still there, without a record.
    block_rule: bool,
    /// Each failure, with what to do about it.
    problems: Vec<String>,
}
static LEFT: Mutex<Left> = Mutex::new(Left {
    block_rule: false,
    problems: Vec::new(),
});
fn left_over() -> MutexGuard<'static, Left> {
    LEFT.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What [`retire`] changes on this PC; the tests fake it.
trait Leftovers {
    /// [`restore_saved`] through Windows' settings.
    fn restore(&self, record: &Record) -> Result<bool>;
    /// [`logic::turn_rdp_off`], after a restore that failed.
    fn rdp_off(&self) -> String;
    /// [`logic::enforce_unrecorded`].
    fn unrecorded(&self) -> Result<Unrecorded>;
    /// Every folder a saved password can be in ([`logic::password_folders`]),
    /// and why some could not be listed.
    fn password_folders(&self) -> (Vec<PathBuf>, Vec<String>);
    /// [`native::wipe_file`].
    fn wipe(&self, path: &Path) -> std::io::Result<bool>;
    /// [`native::remove_empty_folder`].
    fn remove_empty(&self, path: &Path) -> std::io::Result<()>;
    /// The child session still signed in, if any.
    fn child_session(&self) -> Option<u32>;
}
/// [`Leftovers`] on this PC.
struct Native;
impl Leftovers for Native {
    fn restore(&self, record: &Record) -> Result<bool> {
        restore_saved(record, native::NativeSettings::new)
    }
    fn rdp_off(&self) -> String {
        logic::turn_rdp_off(&mut native::NativeRegistry)
    }
    fn unrecorded(&self) -> Result<Unrecorded> {
        logic::enforce_unrecorded(&mut native::NativeSettings::new()?)
    }
    fn password_folders(&self) -> (Vec<PathBuf>, Vec<String>) {
        let mut problems = Vec::new();
        let profiles = native::profile_folders().unwrap_or_else(|error| {
            problems.push(format!(
                "The profile folders could not be listed, so a child-session password Rubylight had saved in one may be left: {error:#}."
            ));
            Vec::new()
        });
        let local: Vec<PathBuf> = native::console_local_app_data()
            .unwrap_or_else(|error| {
                problems.push(format!(
                    "AppData\\Local of the account signed in at this PC's screen could not be read, so a child-session password Rubylight had saved there may be left: {error:#}."
                ));
                None
            })
            .into_iter()
            .collect();
        (logic::password_folders(&profiles, &local), problems)
    }
    fn wipe(&self, path: &Path) -> std::io::Result<bool> {
        native::wipe_file(path)
    }
    fn remove_empty(&self, path: &Path) -> std::io::Result<()> {
        native::remove_empty_folder(path)
    }
    fn child_session(&self) -> Option<u32> {
        native::child_session_id()
    }
}

/// One cleanup of what the removed child-session feature left on this PC,
/// each step logged and each failure kept for the console. Without
/// administrator rights it changes nothing and says once that a record is
/// still there. With them: the restore when there is a record
/// ([`restore_with`]), else setup's traces ([`secure_unrecorded`]); the
/// saved passwords in every profile ([`delete_saved_passwords`]); the child
/// host's profile in `config_dir` ([`remove_child_profile`]); and a child
/// session still signed in is said, never signed out, which would close its
/// apps without saving. Run again, with nothing left, it logs nothing.
fn retire(record: &Record, config_dir: &Path, pc: &dyn Leftovers) -> Left {
    let mut left = Left::default();
    let problem = |left: &mut Left, problem: String| {
        tracing::error!("{problem}");
        left.problems.push(problem);
    };
    if matches!((record.elevated)(), Ok(false)) {
        let reason = match record.load() {
            Ok(None) => return left,
            Ok(Some(_)) => NOT_ELEVATED.to_owned(),
            Err(error) => format!(
                "{NOT_ELEVATED} (Rubylight's record of that setup could not be read here: {error:#})"
            ),
        };
        tracing::warn!(
            "Windows settings from the removed child-session feature are still changed: {reason}"
        );
        left.problems.push(reason);
        return left;
    }
    if record.may_exist() {
        if let Err(failed) = restore_with(record, |record| pc.restore(record), || pc.rdp_off()) {
            problem(&mut left, failed);
        }
    } else {
        match secure_unrecorded(record, || pc.unrecorded()) {
            Ok(Unrecorded { found, .. }) if found.is_empty() => {}
            Ok(Unrecorded { found, left: false }) => tracing::warn!(
                "Child sessions were removed from Rubylight; their setup's record ({}) was gone, but not its traces: {}.",
                record.path.display(),
                found.join("; ")
            ),
            Ok(Unrecorded { found, left: true }) => {
                left.block_rule = true;
                problem(
                    &mut left,
                    format!(
                        "Child sessions were removed from Rubylight, and their setup's firewall rule \"{}\" is still on this PC: {}. Restore again under Maintenance.",
                        logic::BLOCK_RULE,
                        found.join("; ")
                    ),
                );
            }
            Err(error) => problem(
                &mut left,
                format!(
                    "Child sessions were removed from Rubylight, and whether their setup's firewall rule \"{}\" is still on this PC could not be told: {error:#}. Nothing was changed; restart the Rubylight service to check again.",
                    logic::BLOCK_RULE
                ),
            ),
        }
    }
    for failed in delete_saved_passwords(pc) {
        problem(&mut left, failed);
    }
    if let Err(failed) = remove_child_profile(config_dir) {
        problem(&mut left, failed);
    }
    if let Some(session) = pc.child_session() {
        tracing::warn!(
            "A child session from the removed feature is still signed in (session {session}); its apps keep running until you sign it out in Task Manager › Users."
        );
    }
    left
}
/// What a host without administrator rights says while setup's record is
/// there: it can't restore. The console shows it under a title saying the
/// settings are still changed, so it doesn't repeat that.
const NOT_ELEVATED: &str = "This host runs without administrator rights, so it can't restore them. The Rubylight service restores them at its next start, or run butterpollo.exe --child-session-restore as administrator.";

/// [`logic::delete_passwords`] in every folder a saved password can be in,
/// each deleted file logged (its path, never what it held); the failures.
fn delete_saved_passwords(pc: &dyn Leftovers) -> Vec<String> {
    let (folders, mut problems) = pc.password_folders();
    let cleanup =
        logic::delete_passwords(&folders, |path| pc.wipe(path), |path| pc.remove_empty(path));
    for path in &cleanup.deleted {
        tracing::info!(
            "Deleted the child-session password Rubylight had saved (DPAPI-encrypted) at {}.",
            path.display()
        );
    }
    problems.extend(cleanup.failed);
    problems
}

/// The removed child session's own host kept its profile in
/// `<config_dir>\child-session\`, with a copy of the console's sign-in:
/// deleted, and logged, but only when its `sunshine.conf` says it is that
/// host's (`child_session_host = true`). Read as text: the setting is no
/// longer one, so [`butterpollo_core::config::Config`] drops it. Ok(false)
/// when there is none.
fn remove_child_profile(config_dir: &Path) -> Result<bool, String> {
    let profile = config_dir.join(CHILD_PROFILE);
    let conf = profile.join("sunshine.conf");
    let text = match std::fs::read_to_string(&conf) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(format!(
                "{} could not be read, so the profile of the removed child session's host, which holds a copy of the console's sign-in, may be left: {error}.",
                conf.display()
            ));
        }
    };
    let child_host = text.lines().any(|line| {
        line.split_once('=').is_some_and(|(key, value)| {
            key.trim() == "child_session_host" && value.trim().eq_ignore_ascii_case("true")
        })
    });
    if !child_host {
        return Ok(false);
    }
    std::fs::remove_dir_all(&profile).map_err(|error| {
        format!(
            "The profile of the removed child session's host at {} holds a copy of the console's sign-in and could not be deleted: {error}; delete it by hand.",
            profile.display()
        )
    })?;
    tracing::info!(
        "Deleted the profile of the removed child session's host at {}, which held a copy of the console's sign-in.",
        profile.display()
    );
    Ok(true)
}

/// When there is no record, [`logic::enforce_unrecorded`] through
/// `unrecorded`, under the settings locks: a restore running meanwhile is not
/// taken for a setup whose record is gone. Nothing found when there is a
/// record now. Err when that could not be told, with nothing changed.
fn secure_unrecorded(
    record: &Record,
    unrecorded: impl FnOnce() -> Result<Unrecorded>,
) -> Result<Unrecorded> {
    let _settings = settings_lock()?;
    if record.may_exist() {
        return Ok(Unrecorded::default());
    }
    unrecorded()
}

/// Restore and the unrecorded check change the same Windows settings, one at
/// a time: a check that turned RDP off between restore's write of the
/// original value and its read-back would lose the user's own value.
///
/// Two locks, always taken in this order and let go of in reverse, so neither
/// waits on the other: the cross-process [`native::lock::setup_lock`] (the
/// uninstaller's `--child-session-restore`, another host), then this process's
/// own [`SETTINGS`].
static SETTINGS: Mutex<()> = Mutex::new(());
/// Both settings locks; the fields drop in their order, so [`SETTINGS`] first.
struct SettingsLock {
    _settings: MutexGuard<'static, ()>,
    _system: native::lock::NamedLock,
}
/// How long [`settings_lock`] waits for another process.
fn settings_wait() -> std::time::Duration {
    #[cfg(test)]
    if let Some(wait) = tests::SETTINGS_WAIT.get() {
        return wait;
    }
    native::lock::SETUP_LOCK_TIMEOUT
}
/// Both settings locks, Err when another process holds the cross-process one
/// longer than [`settings_wait`].
fn settings_lock() -> Result<SettingsLock> {
    let system = native::lock::setup_lock(settings_wait())?;
    Ok(SettingsLock {
        _settings: SETTINGS.lock().unwrap_or_else(PoisonError::into_inner),
        _system: system,
    })
}

/// What a restore of the PC's record came to.
enum Restored {
    /// Everything is back, and the record is gone.
    Done,
    /// Setup never ran here.
    Never,
    /// It failed after it had put Remote Desktop back as it was before setup
    /// ([`SetupState::rdp_restored`]): RDP is the user's own again, and left.
    Partly(anyhow::Error),
    /// The record's folder is not one setup made ([`Foreign`]), so it holds
    /// no setup of Rubylight's: RDP is left as it is.
    Foreign(anyhow::Error),
    /// It failed; `off` is what was done to turn Remote Desktop off.
    Failed { error: anyhow::Error, off: String },
}
/// Restore the PC's record through `restore` ([`restore_saved`]) under the
/// settings locks. It fails closed: a restore that failed, or could not get
/// the settings lock, turns RDP off through `rdp_off`, without the lock,
/// unless RDP is back as before setup already or the folder is not setup's.
fn restore_outcome(
    record: &Record,
    restore: impl FnOnce(&Record) -> Result<bool>,
    rdp_off: impl FnOnce() -> String,
) -> Restored {
    let restored = if record.may_exist() {
        restore_locked(record, restore)
    } else {
        // Setup never ran: nothing to restore, and no lock to take.
        Ok(false)
    };
    match restored {
        Ok(true) => Restored::Done,
        Ok(false) => Restored::Never,
        // Remote Desktop is back as it was before setup (this restore or an earlier one put
        // it back): it is the user's own again, and a later restore skips it, so turning it
        // off now would lose it for good.
        Err(error) if matches!(record.load(), Ok(Some(state)) if state.rdp_restored) => {
            Restored::Partly(error)
        }
        // Setup never made that folder, so no setup of Rubylight's turned RDP on: turned off
        // for it, any user who makes one would turn an administrator's own RDP off.
        Err(error) if foreign_folder(&error) => Restored::Foreign(error),
        Err(error) => Restored::Failed {
            error,
            off: rdp_off(),
        },
    }
}
/// [`restore_outcome`] at the host's start or from Maintenance, logged: the
/// problem for the console when anything is left to restore.
fn restore_with(
    record: &Record,
    restore: impl FnOnce(&Record) -> Result<bool>,
    rdp_off: impl FnOnce() -> String,
) -> Result<(), String> {
    let not_restored = |error: &anyhow::Error, rdp: &str| {
        format!(
            "Child sessions were removed from Rubylight, and the Windows settings their setup changed were not restored: {error:#}. {rdp}. Restore again under Maintenance, or run butterpollo.exe {} as administrator.",
            logic::RESTORE_FLAG
        )
    };
    match restore_outcome(record, restore, rdp_off) {
        Restored::Done => {
            tracing::info!(
                "Child sessions were removed from Rubylight; the Windows settings their setup changed are restored (Remote Desktop, its firewall rules, the session frame rate, child sessions)."
            );
            Ok(())
        }
        Restored::Never => Ok(()),
        Restored::Partly(error) => Err(not_restored(
            &error,
            "Remote Desktop and its firewall rules are back as they were before setup, so Remote Desktop was left as it is",
        )),
        Restored::Foreign(error) => Err(format!(
            "{error:#}. Setup makes that folder for SYSTEM and Administrators alone, so it holds no setup of Rubylight's: Remote Desktop is left as it is. Delete the folder."
        )),
        Restored::Failed { error, off } => Err(not_restored(&error, &off)),
    }
}

/// `butterpollo.exe --child-session-restore`, which the uninstaller runs: put
/// back the Windows settings child-session setup changed, then delete the
/// passwords it saved. 0 when the settings are back or setup never ran here,
/// 1 with the reason on stderr, after which RDP is off
/// ([`restore_outcome`]); [`logic::RESTORE_PARTLY_EXIT`] when RDP was left
/// as it is because it is back as before setup. A password that could not be
/// deleted is said on stderr; the uninstaller deletes them too.
pub(crate) fn restore_exit_code() -> i32 {
    let code = restore_exit_code_with(
        &Record::new(),
        |record| restore_saved(record, native::NativeSettings::new),
        || logic::turn_rdp_off(&mut native::NativeRegistry),
    );
    for failed in delete_saved_passwords(&Native) {
        eprintln!("{failed}");
    }
    code
}
fn restore_exit_code_with(
    record: &Record,
    restore: impl FnOnce(&Record) -> Result<bool>,
    rdp_off: impl FnOnce() -> String,
) -> i32 {
    match restore_outcome(record, restore, rdp_off) {
        Restored::Done => {
            println!("Restored the Windows settings child-session setup changed.");
            0
        }
        Restored::Never => {
            println!("Child sessions were not set up by Rubylight here; nothing to restore.");
            0
        }
        Restored::Partly(error) => {
            eprintln!(
                "The Windows settings child-session setup changed were partly restored: {error:#}. Remote Desktop and its firewall rules are back as they were before setup, so Remote Desktop was left as it is. Rubylight keeps its record, so restoring again (Restore again under Maintenance, or butterpollo.exe {}) puts back the rest.",
                logic::RESTORE_FLAG
            );
            logic::RESTORE_PARTLY_EXIT
        }
        Restored::Foreign(error) => {
            eprintln!(
                "The Windows settings child-session setup changed were not restored: {error:#}. Setup makes that folder for SYSTEM and Administrators alone, so it holds no setup of Rubylight's; Remote Desktop was left as it is."
            );
            1
        }
        Restored::Failed { error, off } => {
            eprintln!(
                "The Windows settings child-session setup changed were not restored: {error:#}. {off}. Rubylight keeps its record, so restoring again (Restore again under Maintenance, or butterpollo.exe {}) puts back the rest.",
                logic::RESTORE_FLAG
            );
            1
        }
    }
}

/// `restore` ([`restore_saved`]) under the settings locks; refused without
/// administrator rights.
fn restore_locked(record: &Record, restore: impl FnOnce(&Record) -> Result<bool>) -> Result<bool> {
    require_elevated(record, "restoring the Windows settings")?;
    let _settings = settings_lock()?;
    restore(record)
}
/// Err unless this process has administrator rights, as restore needs:
/// without them it would make the record's folder with the permissions it
/// inherits. Not knowing is a refusal too.
fn require_elevated(record: &Record, what: &str) -> Result<()> {
    match (record.elevated)() {
        Ok(true) => Ok(()),
        Ok(false) => bail!(
            "{what} needs administrator rights, and this host runs without them; nothing was changed. Use the console of the Rubylight service, or of a host run as administrator"
        ),
        Err(error) => Err(error.context(format!(
            "{what} needs administrator rights, and whether this host has them could not be told; nothing was changed"
        ))),
    }
}
/// Put back what setup changed, through the settings `open` makes, then
/// forget it; false when setup never ran. The record stays when anything was
/// not restored, so a retry has it, saying what is back already.
fn restore_saved<S: SystemSettings>(
    record: &Record,
    open: impl FnOnce() -> Result<S>,
) -> Result<bool> {
    let Some(mut state) = record.load()? else {
        return Ok(false);
    };
    logic::restore(&mut open()?, &mut state, |state| record.save(state))?;
    record.remove()?;
    Ok(true)
}

#[cfg(test)]
mod tests;
