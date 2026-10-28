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
        tracing::warn!("{reason}");
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
/// there: it can't restore.
const NOT_ELEVATED: &str = "Windows settings from the removed child-session feature are still changed; the Rubylight service restores them at its next start, or run butterpollo.exe --child-session-restore as administrator.";

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
mod tests {
    use super::*;
    use butterpollo_core::child_session::Registry;
    use std::cell::Cell;

    thread_local! {
        /// [`settings_wait`] on this test thread, when not the host's.
        pub(super) static SETTINGS_WAIT: std::cell::Cell<Option<std::time::Duration>> =
            const { std::cell::Cell::new(None) };
    }

    #[test]
    fn the_uninstallers_restore_flag_is_the_hosts() {
        use clap::Parser;
        let args = crate::Args::try_parse_from(["butterpollo.exe", logic::RESTORE_FLAG]).unwrap();
        assert!(args.child_session_restore);
    }

    fn scratch() -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("butterpollo-child-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }
    /// The PC's record, in `directory`.
    fn record_in(directory: &Path) -> Record {
        Record {
            path: directory.join(logic::SETUP_FOLDER).join(logic::SETUP_FILE),
            create_folder: |path| std::fs::create_dir(path),
            create_file: |path| {
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)
            },
            trusted: |_, _| Ok(()),
            trusted_root: |_, _| Ok(()),
            elevated: || Ok(true),
        }
    }
    fn write_record(record: &Record, text: &str) {
        std::fs::create_dir_all(record.folder()).unwrap();
        std::fs::write(&record.path, text).unwrap();
    }
    fn unopened() -> Result<native::NativeSettings> {
        panic!("Windows' settings were opened")
    }

    #[test]
    fn restore_without_a_setup_record_changes_nothing() {
        let directory = scratch();
        let record = record_in(&directory);
        assert!(!restore_saved(&record, unopened).unwrap());
        write_record(&record, "not json");
        let error = restore_saved(&record, unopened).unwrap_err();
        assert!(
            format!("{error:#}").contains(logic::SETUP_FILE),
            "{error:#}"
        );
        assert!(record.path.is_file());
        std::fs::remove_dir_all(directory).unwrap();
    }

    /// The Windows settings setup changed, shared by every [`Pc`] handle as
    /// one PC's are by every host on it.
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    struct Machine {
        values: std::collections::BTreeMap<(String, String, String), u32>,
        child_sessions: bool,
        /// Rule ID -> enabled, all in the Remote Desktop group.
        rules: std::collections::BTreeMap<String, bool>,
        block_rules: std::collections::BTreeSet<String>,
        /// Turning child sessions on or off fails.
        child_sessions_fail: bool,
    }
    #[derive(Clone)]
    struct Pc(std::rc::Rc<std::cell::RefCell<Machine>>);
    impl Pc {
        fn new() -> Self {
            let mut machine = Machine::default();
            machine.values.insert(deny(), 1);
            machine
                .rules
                .insert("RemoteDesktop-UserMode-In-TCP".into(), true);
            Self(std::rc::Rc::new(std::cell::RefCell::new(machine)))
        }
        fn now(&self) -> Machine {
            self.0.borrow().clone()
        }
        fn open(&self) -> impl FnOnce() -> Result<Pc> + use<> {
            let pc = self.clone();
            move || Ok(pc)
        }
        fn rdp(&self) -> Option<u32> {
            self.0.borrow().values.get(&deny()).copied()
        }
    }
    fn deny() -> (String, String, String) {
        hive_key(
            &logic::Hive::LocalMachine,
            logic::TERMINAL_SERVER,
            "fDenyTSConnections",
        )
    }
    fn hive_key(hive: &logic::Hive, path: &str, name: &str) -> (String, String, String) {
        let hive = match hive {
            logic::Hive::LocalMachine => "LocalMachine".to_string(),
            logic::Hive::User(sid) => sid.clone(),
        };
        (hive, path.into(), name.into())
    }
    impl Registry for Pc {
        fn read_dword(&self, hive: &logic::Hive, path: &str, name: &str) -> Result<Option<u32>> {
            Ok(self
                .0
                .borrow()
                .values
                .get(&hive_key(hive, path, name))
                .copied())
        }
        fn write_dword(
            &mut self,
            hive: &logic::Hive,
            path: &str,
            name: &str,
            value: u32,
        ) -> Result<()> {
            self.0
                .borrow_mut()
                .values
                .insert(hive_key(hive, path, name), value);
            Ok(())
        }
    }
    impl SystemSettings for Pc {
        fn delete_value(&mut self, hive: &logic::Hive, path: &str, name: &str) -> Result<()> {
            self.0
                .borrow_mut()
                .values
                .remove(&hive_key(hive, path, name));
            Ok(())
        }
        fn child_sessions_enabled(&self) -> Result<bool> {
            Ok(self.0.borrow().child_sessions)
        }
        fn set_child_sessions(&mut self, enabled: bool) -> Result<()> {
            let mut machine = self.0.borrow_mut();
            anyhow::ensure!(!machine.child_sessions_fail, "the call failed");
            machine.child_sessions = enabled;
            Ok(())
        }
        fn block_rule_present(&self, name: &str) -> Result<bool> {
            Ok(self.0.borrow().block_rules.contains(name))
        }
        fn enable_rule(&mut self, _: &str, id: &str) -> Result<()> {
            *self
                .0
                .borrow_mut()
                .rules
                .get_mut(id)
                .context("no such rule")? = true;
            Ok(())
        }
        fn remove_rule(&mut self, name: &str) -> Result<()> {
            self.0.borrow_mut().block_rules.remove(name);
            Ok(())
        }
        fn load_user_hive(&mut self, _: &str) -> Result<bool> {
            Ok(false)
        }
        fn unload_user_hive(&mut self, _: &str) -> Result<()> {
            Ok(())
        }
    }

    /// What the removed setup recorded in `record` and changed on `pc`: RDP
    /// on behind its block rule, the frame interval, child sessions on.
    fn set_up(pc: &Pc, record: &Record) {
        let mut machine = pc.0.borrow_mut();
        let state = SetupState {
            remote_desktop_rule_ids: machine
                .rules
                .iter()
                .filter(|(_, on)| **on)
                .map(|(id, _)| id.clone())
                .collect(),
            ..recorded()
        };
        record.save(&state).unwrap();
        machine.values.insert(deny(), 0);
        machine.values.insert(
            hive_key(
                &logic::Hive::LocalMachine,
                logic::WINSTATIONS,
                "DWMFRAMEINTERVAL",
            ),
            8,
        );
        for on in machine.rules.values_mut() {
            *on = false;
        }
        machine.block_rules.insert(logic::BLOCK_RULE.into());
        machine.child_sessions = true;
    }

    #[test]
    fn a_restore_that_stops_after_rdp_leaves_rdp_to_the_user() {
        let directory = scratch();
        let record = record_in(&directory);
        let pc = Pc::new();
        let before = pc.now();
        set_up(&pc, &record);
        // Child sessions, the last step, fail: RDP and the firewall are back, and the record
        // says so.
        pc.0.borrow_mut().child_sessions_fail = true;
        let error = format!("{:#}", restore_saved(&record, pc.open()).unwrap_err());
        assert!(error.contains("child sessions"), "{error}");
        let saved = record.load().unwrap().unwrap();
        assert!(saved.rdp_restored && saved.remote_desktop_rule_ids.is_empty());
        assert!(pc.now().block_rules.is_empty());
        // The user turns RDP on since. Restoring again finishes without writing RDP's value
        // again.
        pc.0.borrow_mut().values.insert(deny(), 0);
        pc.0.borrow_mut().child_sessions_fail = false;
        assert!(restore_saved(&record, pc.open()).unwrap());
        assert!(!record.path.exists());
        let mut expected = before;
        expected.values.insert(deny(), 0);
        assert_eq!(pc.now(), expected);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_record_another_account_owns_is_never_used() {
        let directory = scratch();
        let trusted = record_in(&directory);
        trusted.save(&recorded()).unwrap();
        /// [`administrators_own`]'s refusal of `path`.
        fn foreign(path: &Path) -> Result<()> {
            Err(Foreign {
                folder: path.extension().is_none(),
                problem: format!("{} is owned by S-1-5-21-9", path.display()),
            }
            .into())
        }
        let foreign_file = Record {
            trusted: |_, path| match path.extension() {
                None => Ok(()),
                Some(_) => foreign(path),
            },
            ..trusted.clone()
        };
        let foreign_folder = Record {
            trusted: |_, path| foreign(path),
            ..trusted.clone()
        };
        // A folder that can't be checked is no proof that setup never made it.
        let unchecked_folder = Record {
            trusted: |_, path| match path.extension() {
                None => bail!("reading the owner of {}: access denied", path.display()),
                Some(_) => Ok(()),
            },
            ..trusted.clone()
        };
        for record in [&foreign_file, &foreign_folder, &unchecked_folder] {
            let error = format!("{:#}", record.load().unwrap_err());
            assert!(
                error.contains("S-1-5-21-9") || error.contains("access denied"),
                "{error}"
            );
            // Restore refuses it.
            let pc = Pc::new();
            assert!(restore_saved(record, pc.open()).is_err());
            assert_eq!(pc.now(), Pc::new().now());
        }
        // A record another account wrote into setup's folder, or a folder that can't be
        // checked: RDP goes off, as after any restore that failed.
        for record in [&foreign_file, &unchecked_folder] {
            let fake = Fake::new(Pc::new());
            fake.pc.0.borrow_mut().values.insert(deny(), 0);
            let left = retire(record, &directory, &fake);
            assert_eq!((fake.rdp_offs.get(), fake.pc.rdp()), (1, Some(1)));
            assert!(
                left.problems[0].contains("Remote Desktop was turned off"),
                "{left:?}"
            );
        }
        // A folder setup never made holds no setup of Rubylight's: any user could make one in
        // a data root they can write, so RDP, the user's own, is left as it is. Still said.
        let fake = Fake::new(Pc::new());
        fake.pc.0.borrow_mut().values.insert(deny(), 0);
        let left = retire(&foreign_folder, &directory, &fake);
        assert_eq!((fake.rdp_offs.get(), fake.pc.rdp()), (0, Some(0)));
        assert!(
            left.problems[0].contains("is owned by S-1-5-21-9")
                && left.problems[0].contains("Remote Desktop is left as it is"),
            "{left:?}"
        );
        // A folder someone else made first is not saved into.
        std::fs::remove_dir_all(&directory).unwrap();
        let squatted = record_in(&directory);
        std::fs::create_dir_all(squatted.folder()).unwrap();
        let squatted = Record {
            trusted: foreign_folder.trusted,
            ..squatted
        };
        assert!(squatted.save(&recorded()).is_err());
        assert!(!squatted.path.exists());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn this_pcs_check_tells_a_folder_setup_never_made_from_a_file() {
        // Under %TEMP%, which its user can write: never SYSTEM's and Administrators' alone,
        // whether this test runs elevated or not.
        let directory = scratch();
        let file = directory.join(logic::SETUP_FILE);
        std::fs::write(&file, "{}").unwrap();
        for (path, folder) in [(&directory, true), (&file, false)] {
            let open = butterpollo_windows::process::open_unfollowed(path, folder).unwrap();
            match administrators_own(&open, path) {
                Ok(()) => panic!("{} passed as administrators' own", path.display()),
                Err(error) => {
                    let foreign = error.downcast_ref::<Foreign>().unwrap();
                    assert_eq!(foreign.folder, folder, "{error:#}");
                    assert_eq!(foreign_folder(&error), folder);
                }
            }
        }
        // Checked through a context too.
        let error = administrators_own(
            &butterpollo_windows::process::open_unfollowed(&directory, true).unwrap(),
            &directory,
        )
        .unwrap_err()
        .context("opening the record");
        assert!(foreign_folder(&error));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn the_record_read_is_the_one_whose_owner_was_checked() {
        // Checked by its path, then read by its path, another file could take its place in
        // between. The check here swaps one in: what is read is still the file checked.
        let directory = scratch();
        let record = Record {
            trusted: |file, path| {
                if path.extension().is_none() {
                    // The folder stays where it is while open.
                    assert!(std::fs::rename(path, path.with_extension("moved")).is_err());
                    return Ok(());
                }
                let swapped = path.with_extension("swap");
                let other = SetupState {
                    rdp_port: 3391,
                    ..recorded()
                };
                std::fs::write(&swapped, serde_json::to_vec(&other).unwrap()).unwrap();
                std::fs::rename(&swapped, path).unwrap();
                assert!(file.metadata().unwrap().len() > 0);
                Ok(())
            },
            ..record_in(&directory)
        };
        record.save(&recorded()).unwrap();
        assert_eq!(record.load().unwrap(), Some(recorded()));
        // The next read finds the file swapped in, through its own check.
        assert_eq!(record.load().unwrap().unwrap().rdp_port, 3391);
        std::fs::remove_dir_all(directory).unwrap();
    }

    thread_local! {
        static CREATED: std::cell::RefCell<Vec<PathBuf>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    #[test]
    fn the_record_is_made_for_administrators_only() {
        let directory = scratch();
        let record = Record {
            create_folder: |path| {
                CREATED.with_borrow_mut(|done| done.push(path.into()));
                (record_in(path).create_folder)(path)
            },
            create_file: |path| {
                CREATED.with_borrow_mut(|done| done.push(path.into()));
                (record_in(path).create_file)(path)
            },
            ..record_in(&directory)
        };
        record.save(&recorded()).unwrap();
        record.save(&recorded()).unwrap();
        // Made with their permissions: the data root and the folder each time (there the
        // second), every file before it takes the record's name.
        let temporary = record.path.with_extension("tmp");
        let folder = record.folder().to_path_buf();
        assert_eq!(
            CREATED.take(),
            [
                directory.clone(),
                folder.clone(),
                temporary.clone(),
                directory.clone(),
                folder,
                temporary.clone()
            ]
        );
        assert_eq!(record.load().unwrap(), Some(recorded()));
        assert!(!temporary.exists());
        // A data root that is not there is made with them too, never with its parents.
        let root = directory.join("data");
        let rooted = Record {
            create_folder: record.create_folder,
            create_file: record.create_file,
            ..record_in(&root)
        };
        rooted.save(&recorded()).unwrap();
        assert_eq!(CREATED.take()[..2], [root.clone(), rooted.folder().into()]);
        let orphan = record_in(&directory.join("gone").join("data"));
        assert!(orphan.save(&recorded()).is_err());
        assert!(!directory.join("gone").exists());
        // A folder that could not be made is not saved into.
        let unmade = Record {
            create_folder: |_| Err(ErrorKind::PermissionDenied.into()),
            ..record_in(&directory.join("unmade"))
        };
        assert!(unmade.save(&recorded()).is_err());
        assert!(!unmade.folder().exists());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_data_root_administrators_do_not_own_is_never_saved_into() {
        // Its owner could change its permissions and swap the record's folder in it.
        let directory = scratch();
        let foreign = Record {
            trusted_root: |_, path| bail!("{} is owned by S-1-5-21-9", path.display()),
            ..record_in(&directory)
        };
        let error = format!("{:#}", foreign.save(&recorded()).unwrap_err());
        assert!(error.contains("S-1-5-21-9"), "{error}");
        assert!(!foreign.folder().exists());
        // A link is opened itself, never followed, and refused on this PC's check.
        let target = directory.join("elsewhere");
        std::fs::create_dir(&target).unwrap();
        let link = directory.join("linked");
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "mklink /J failed");
        let linked = Record {
            trusted_root: administrators_own_root,
            ..record_in(&link)
        };
        let error = format!("{:#}", linked.save(&recorded()).unwrap_err());
        assert!(error.contains("is a link to elsewhere"), "{error}");
        assert_eq!(std::fs::read_dir(&target).unwrap().count(), 0);
        // A folder: refused unless SYSTEM or Administrators own it, as this test's own does
        // only when it runs elevated, and alone may write it, which %TEMP%'s user may too.
        let own = Record {
            trusted_root: administrators_own_root,
            ..record_in(&target)
        };
        let open = butterpollo_windows::process::open_unfollowed(&target, true).unwrap();
        let owner = butterpollo_windows::process::open_file_owner(&open).unwrap();
        let writers = butterpollo_windows::process::open_file_writers(&open).unwrap();
        drop(open);
        let administrators = |sid: &str| matches!(sid, "S-1-5-18" | "S-1-5-32-544");
        let error = format!("{:#}", own.save(&recorded()).unwrap_err());
        if administrators(&owner) {
            let other = writers
                .iter()
                .find(|sid| !administrators(sid.as_str()))
                .unwrap();
            assert!(
                error.contains(&format!("can be written by {other}")),
                "{error}"
            );
        } else {
            assert!(error.contains(&format!("is owned by {owner}")), "{error}");
        }
        assert!(!own.folder().exists());
        std::fs::remove_dir(&link).unwrap();
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_temporary_record_a_cut_short_setup_left_is_replaced() {
        // A setup cut short between making and renaming it left one: it is never written into
        // (it is replaced by one made new), and never holds off the next setup.
        let directory = scratch();
        let record = record_in(&directory);
        let temporary = record.path.with_extension("tmp");
        std::fs::create_dir_all(record.folder()).unwrap();
        std::fs::write(&temporary, "left by a setup cut short").unwrap();
        record.save(&recorded()).unwrap();
        assert!(!temporary.exists());
        assert_eq!(record.load().unwrap(), Some(recorded()));
        // Replaced once: one that can't be made new even then is a failure that says so.
        std::fs::write(&temporary, "left by a setup cut short").unwrap();
        let stuck = Record {
            create_file: |path| {
                std::fs::write(path, "made again")?;
                Err(std::io::ErrorKind::AlreadyExists.into())
            },
            ..record.clone()
        };
        let error = format!("{:#}", stuck.save(&recorded()).unwrap_err());
        assert!(
            error.contains("creating") && error.contains(".tmp"),
            "{error}"
        );
        assert_eq!(record.load().unwrap(), Some(recorded()));
        // One that can't be removed (held open, sharing nothing) is a failure that says so.
        let held = {
            use std::os::windows::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(&temporary)
                .unwrap()
        };
        let error = format!("{:#}", record.save(&recorded()).unwrap_err());
        assert!(error.contains("which a setup cut short left"), "{error}");
        drop(held);
        std::fs::remove_file(&temporary).unwrap();
        record.remove().unwrap();
        // One this save made and could not write goes, so it holds off no later setup.
        let unwritable = Record {
            create_file: |path| {
                std::fs::write(path, "")?;
                File::open(path)
            },
            ..record.clone()
        };
        assert!(unwritable.save(&recorded()).is_err());
        assert!(!temporary.exists() && !record.path.exists());
        record.save(&recorded()).unwrap();
        assert_eq!(record.load().unwrap(), Some(recorded()));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn restore_needs_administrator_rights() {
        let directory = scratch();
        let pc = Pc::new();
        let elevated = record_in(&directory);
        let before = pc.now();
        set_up(&pc, &elevated);
        let changed = pc.now();
        for unelevated in [
            Record {
                elevated: || Ok(false),
                ..elevated.clone()
            },
            Record {
                elevated: || anyhow::bail!("no token"),
                ..elevated.clone()
            },
        ] {
            let error = format!(
                "{:#}",
                restore_locked(&unelevated, |_| -> Result<bool> { panic!("restored") })
                    .unwrap_err()
            );
            assert!(error.contains("administrator rights"), "{error}");
            assert_eq!(pc.now(), changed);
            assert!(elevated.path.is_file());
        }
        assert!(restore_locked(&elevated, |record| restore_saved(record, pc.open())).unwrap());
        assert_eq!(pc.now(), before);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_planted_profile_record_is_ignored() {
        // A profile folder is writable by its user; a record there would
        // make restore put back what that user wrote.
        let directory = scratch();
        let record = record_in(&directory);
        let profile = directory.join("config");
        std::fs::create_dir_all(&profile).unwrap();
        // It would turn RDP on at restore.
        let planted = SetupState {
            deny_ts_connections: Some(0),
            rdp_port: 3391,
            ..recorded()
        };
        std::fs::write(
            profile.join(logic::SETUP_FILE),
            serde_json::to_vec(&planted).unwrap(),
        )
        .unwrap();
        assert!(!record.may_exist());
        assert_eq!(record.load().unwrap(), None);
        assert!(!restore_saved(&record, unopened).unwrap());
        let fake = Fake::new(Pc::new());
        assert_eq!(retire(&record, &profile, &fake), Left::default());
        assert_eq!((fake.restores.get(), fake.pc.rdp()), (0, Some(1)));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn the_record_is_the_pcs_not_the_profiles() {
        assert_eq!(Record::new().path, logic::setup_record());
    }

    fn set_up_record() -> (PathBuf, Record) {
        let directory = scratch();
        let record = record_in(&directory);
        record.save(&recorded()).unwrap();
        (directory, record)
    }
    fn recorded() -> SetupState {
        SetupState {
            version: 1,
            user_sid: "S-1-5-21-1".into(),
            deny_ts_connections: Some(1),
            dwm_frame_interval: None,
            suppress_when_minimized: None,
            child_sessions: false,
            remote_desktop_rule_ids: Vec::new(),
            rdp_port: 3390,
            rdp_restored: false,
        }
    }

    /// The settings locks as this process holds them, or only the
    /// cross-process one, as the uninstaller or another host holds it.
    type Hold = fn() -> Box<dyn std::any::Any>;
    const HOLDERS: [Hold; 2] = [
        || Box::new(settings_lock().unwrap()),
        || Box::new(native::lock::setup_lock(settings_wait()).unwrap()),
    ];
    /// `run`, on another thread, signals only after this test lets go of what
    /// `hold` took.
    fn waits_for_the_settings_lock<T: Send + 'static>(
        hold: Hold,
        run: impl FnOnce(std::sync::mpsc::Sender<()>) -> T + Send + 'static,
    ) -> T {
        let settings = hold();
        let (reached, reach) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || run(reached));
        assert!(
            reach
                .recv_timeout(std::time::Duration::from_millis(200))
                .is_err(),
            "ran while the settings lock was held"
        );
        drop(settings);
        reach
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        thread.join().unwrap()
    }

    #[test]
    fn restore_and_the_unrecorded_check_wait_for_the_settings_lock() {
        let directory = scratch();
        let record = record_in(&directory);
        for hold in HOLDERS {
            // The uninstaller's restore.
            let config = record.clone();
            record.save(&recorded()).unwrap();
            let code = waits_for_the_settings_lock(hold, move |reached| {
                restore_exit_code_with(
                    &config,
                    |_| {
                        reached.send(()).unwrap();
                        Ok(true)
                    },
                    || panic!("restored"),
                )
            });
            assert_eq!(code, 0);
            // The host's restore.
            let config = record.clone();
            let restored = waits_for_the_settings_lock(hold, move |reached| {
                restore_locked(&config, |_| {
                    reached.send(()).unwrap();
                    Ok(false)
                })
            });
            assert!(!restored.unwrap());
            // The check without a record.
            record.remove().unwrap();
            let config = record.clone();
            let found = waits_for_the_settings_lock(hold, move |reached| {
                secure_unrecorded(&config, || {
                    reached.send(()).unwrap();
                    Ok(Unrecorded::default())
                })
            });
            assert_eq!(found.unwrap(), Unrecorded::default());
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn the_uninstallers_restore_fails_closed() {
        use std::sync::atomic::{AtomicU32, Ordering::SeqCst};
        let off = AtomicU32::new(0);
        let rdp_off = || {
            off.fetch_add(1, SeqCst);
            "Remote Desktop was turned off (fDenyTSConnections = 1), so it is not open to the network".to_string()
        };
        // Setup never ran: nothing to restore, RDP left as the user has it.
        let empty = scratch();
        let never = |_: &Record| -> Result<bool> { panic!("nothing to restore") };
        assert_eq!(
            restore_exit_code_with(&record_in(&empty), never, rdp_off),
            0
        );
        assert_eq!(off.load(SeqCst), 0);
        std::fs::remove_dir_all(empty).unwrap();
        // A restore that fails turns RDP off.
        let (directory, record) = set_up_record();
        let failed = |_: &Record| -> Result<bool> { bail!("access denied") };
        assert_eq!(restore_exit_code_with(&record, failed, rdp_off), 1);
        assert_eq!(off.load(SeqCst), 1);
        // So does a settings lock another process keeps.
        let lock = native::lock::setup_lock(settings_wait()).unwrap();
        let code = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    SETTINGS_WAIT.set(Some(std::time::Duration::from_millis(200)));
                    restore_exit_code_with(
                        &record,
                        |_| panic!("restored without the lock"),
                        rdp_off,
                    )
                })
                .join()
                .unwrap()
        });
        drop(lock);
        assert_eq!(code, 1);
        assert_eq!(off.load(SeqCst), 2);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_partly_restored_remote_desktop_is_left_as_the_user_had_it() {
        use std::sync::atomic::{AtomicU32, Ordering::SeqCst};
        let off = AtomicU32::new(0);
        let rdp_off = || {
            off.fetch_add(1, SeqCst);
            "Remote Desktop was turned off (fDenyTSConnections = 1), so it is not open to the network".to_string()
        };
        // This restore puts RDP back, then fails on the rest: RDP is the user's own again.
        let (directory, record) = set_up_record();
        let partly = |record: &Record| -> Result<bool> {
            let mut state = record.load()?.unwrap();
            state.rdp_restored = true;
            record.save(&state)?;
            bail!("child sessions could not be turned off: access denied")
        };
        assert_eq!(
            restore_exit_code_with(&record, partly, rdp_off),
            logic::RESTORE_PARTLY_EXIT
        );
        assert_eq!(off.load(SeqCst), 0);
        // An earlier restore put it back; this one fails before the lock or the record.
        let failed = |_: &Record| -> Result<bool> { bail!("access denied") };
        assert_eq!(
            restore_exit_code_with(&record, failed, rdp_off),
            logic::RESTORE_PARTLY_EXIT
        );
        assert_eq!(off.load(SeqCst), 0);
        // Without that mark it still fails closed.
        let mut state = record.load().unwrap().unwrap();
        state.rdp_restored = false;
        record.save(&state).unwrap();
        assert_eq!(restore_exit_code_with(&record, failed, rdp_off), 1);
        assert_eq!(off.load(SeqCst), 1);
        assert_ne!(logic::RESTORE_PARTLY_EXIT, 1);
        std::fs::remove_dir_all(directory).unwrap();
    }

    /// [`Leftovers`] over a [`Pc`], counting what [`retire`] calls; saved
    /// passwords in `profiles`, wiped as on this PC.
    struct Fake {
        pc: Pc,
        restores: Cell<u32>,
        /// The restore fails before it changes anything.
        restore_fails: bool,
        rdp_offs: Cell<u32>,
        unrecorded: Cell<u32>,
        profiles: Vec<PathBuf>,
        session: Option<u32>,
    }
    impl Fake {
        fn new(pc: Pc) -> Self {
            Self {
                pc,
                restores: Cell::new(0),
                restore_fails: false,
                rdp_offs: Cell::new(0),
                unrecorded: Cell::new(0),
                profiles: Vec::new(),
                session: None,
            }
        }
    }
    impl Leftovers for Fake {
        fn restore(&self, record: &Record) -> Result<bool> {
            self.restores.set(self.restores.get() + 1);
            if self.restore_fails {
                bail!("access denied");
            }
            restore_saved(record, self.pc.open())
        }
        fn rdp_off(&self) -> String {
            self.rdp_offs.set(self.rdp_offs.get() + 1);
            logic::turn_rdp_off(&mut self.pc.clone())
        }
        fn unrecorded(&self) -> Result<Unrecorded> {
            self.unrecorded.set(self.unrecorded.get() + 1);
            logic::enforce_unrecorded(&mut self.pc.clone())
        }
        fn password_folders(&self) -> (Vec<PathBuf>, Vec<String>) {
            (logic::password_folders(&self.profiles, &[]), Vec::new())
        }
        fn wipe(&self, path: &Path) -> std::io::Result<bool> {
            native::wipe_file(path)
        }
        fn remove_empty(&self, path: &Path) -> std::io::Result<()> {
            native::remove_empty_folder(path)
        }
        fn child_session(&self) -> Option<u32> {
            self.session
        }
    }

    #[test]
    fn the_host_restores_a_set_up_pc_at_its_start_once() {
        let directory = scratch();
        let record = record_in(&directory);
        let fake = Fake::new(Pc::new());
        let before = fake.pc.now();
        set_up(&fake.pc, &record);
        let left = retire(&record, &directory, &fake);
        assert_eq!(left, Left::default());
        assert_eq!(fake.pc.now(), before);
        assert!(!record.path.exists());
        assert_eq!(
            status_with(&record, &left),
            json!({"pending": false, "problem": null})
        );
        // The next start finds nothing to do.
        let fake = Fake {
            unrecorded: Cell::new(0),
            ..fake
        };
        assert_eq!(retire(&record, &directory, &fake), Left::default());
        assert_eq!((fake.restores.get(), fake.unrecorded.get()), (1, 1));
        assert_eq!(fake.pc.now(), before);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_restore_that_fails_at_the_start_turns_rdp_off_and_keeps_the_record() {
        let (directory, record) = set_up_record();
        let fake = Fake {
            restore_fails: true,
            ..Fake::new(Pc::new())
        };
        fake.pc.0.borrow_mut().values.insert(deny(), 0);
        let left = retire(&record, &directory, &fake);
        assert_eq!((fake.restores.get(), fake.rdp_offs.get()), (1, 1));
        assert_eq!(fake.pc.rdp(), Some(1));
        assert!(record.path.is_file());
        let problem = &left.problems[0];
        assert!(
            problem.starts_with("Child sessions were removed from Rubylight, and the Windows settings their setup changed were not restored: access denied. Remote Desktop was turned off")
                && problem.ends_with("Restore again under Maintenance, or run butterpollo.exe --child-session-restore as administrator."),
            "{problem}"
        );
        let status = status_with(&record, &left);
        assert_eq!(
            (&status["pending"], &status["problem"]),
            (&json!(true), &json!(problem))
        );
        // RDP back as before setup already: it stays the user's own.
        let mut state = record.load().unwrap().unwrap();
        state.rdp_restored = true;
        record.save(&state).unwrap();
        let fake = Fake {
            restore_fails: true,
            ..Fake::new(Pc::new())
        };
        fake.pc.0.borrow_mut().values.insert(deny(), 0);
        let left = retire(&record, &directory, &fake);
        assert_eq!((fake.rdp_offs.get(), fake.pc.rdp()), (0, Some(0)));
        assert!(
            left.problems[0].contains("Remote Desktop was left as it is"),
            "{left:?}"
        );
        assert_eq!(
            restore_exit_code_with(
                &record,
                |_| -> Result<bool> { bail!("access denied") },
                || panic!("RDP off")
            ),
            logic::RESTORE_PARTLY_EXIT
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_host_without_administrator_rights_changes_nothing_and_says_so_once() {
        let (directory, record) = set_up_record();
        let unelevated = Record {
            elevated: || Ok(false),
            ..record.clone()
        };
        let profile = directory.join(CHILD_PROFILE);
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(profile.join("sunshine.conf"), "child_session_host = true\n").unwrap();
        let fake = Fake::new(Pc::new());
        fake.pc.0.borrow_mut().values.insert(deny(), 0);
        let before = fake.pc.now();
        let left = retire(&unelevated, &directory, &fake);
        assert_eq!(left.problems, [NOT_ELEVATED]);
        assert_eq!(
            (
                fake.restores.get(),
                fake.rdp_offs.get(),
                fake.unrecorded.get()
            ),
            (0, 0, 0)
        );
        assert_eq!(fake.pc.now(), before);
        assert!(record.path.is_file() && profile.is_dir());
        // Never set up: nothing to say.
        record.remove().unwrap();
        assert_eq!(retire(&unelevated, &directory, &fake), Left::default());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn without_a_record_setups_traces_are_checked_once() {
        let directory = scratch();
        let record = record_in(&directory);
        // Set up, then the record went: RDP goes off, then the block rule.
        let fake = Fake::new(Pc::new());
        set_up(&fake.pc, &record);
        record.remove().unwrap();
        let left = retire(&record, &directory, &fake);
        assert_eq!(left, Left::default());
        assert_eq!((fake.unrecorded.get(), fake.restores.get()), (1, 0));
        assert_eq!(fake.pc.rdp(), Some(1));
        assert!(fake.pc.now().block_rules.is_empty());
        // A block rule that stays (RDP can't be turned off) is pending, and said.
        let fake = Fake::new(Pc::new());
        set_up(&fake.pc, &record);
        record.remove().unwrap();
        fake.pc.0.borrow_mut().values.insert(
            hive_key(
                &logic::Hive::LocalMachine,
                logic::TERMINAL_SERVICES_POLICY,
                "fDenyTSConnections",
            ),
            0,
        );
        let left = retire(&record, &directory, &fake);
        assert!(left.block_rule, "{left:?}");
        assert!(left.problems[0].contains("is still on this PC"), "{left:?}");
        assert_eq!(status_with(&record, &left)["pending"], json!(true));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn the_saved_passwords_go_from_every_profile() {
        let directory = scratch();
        let profiles = [directory.join("ramaz"), directory.join("guest")];
        let folder = |profile: &Path| {
            profile
                .join("AppData")
                .join("Local")
                .join(logic::PASSWORD_FOLDER)
        };
        for profile in &profiles {
            std::fs::create_dir_all(folder(profile)).unwrap();
            for name in logic::PASSWORD_FILES {
                std::fs::write(folder(profile).join(name), "DPAPI blob").unwrap();
            }
        }
        // The guest's folder holds something else too, and stays.
        std::fs::write(folder(&profiles[1]).join("other.txt"), "kept").unwrap();
        // A profile with no saved password is no failure; a relative one, and one through `..`,
        // are skipped.
        let empty = directory.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let kept = directory.join("kept");
        std::fs::create_dir_all(folder(&kept)).unwrap();
        std::fs::write(folder(&kept).join(logic::PASSWORD_FILES[0]), "DPAPI blob").unwrap();
        let fake = Fake {
            profiles: vec![
                profiles[0].clone(),
                profiles[1].clone(),
                empty,
                PathBuf::from("relative"),
                directory.join("x").join("..").join("kept"),
            ],
            ..Fake::new(Pc::new())
        };
        assert!(delete_saved_passwords(&fake).is_empty());
        assert!(!folder(&profiles[0]).exists());
        assert_eq!(
            std::fs::read_dir(folder(&profiles[1])).unwrap().count(),
            1,
            "only other.txt stays"
        );
        assert!(folder(&kept).join(logic::PASSWORD_FILES[0]).is_file());
        // A file that can't be deleted (held open) is said with its path; the others go.
        for profile in &profiles {
            std::fs::create_dir_all(folder(profile)).unwrap();
            for name in logic::PASSWORD_FILES {
                std::fs::write(folder(profile).join(name), "DPAPI blob").unwrap();
            }
        }
        let held_path = folder(&profiles[0]).join(logic::PASSWORD_FILES[0]);
        let held = {
            use std::os::windows::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(&held_path)
                .unwrap()
        };
        let problems = delete_saved_passwords(&fake);
        drop(held);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            problems[0].contains(&held_path.display().to_string())
                && problems[0].contains("delete it by hand"),
            "{problems:?}"
        );
        assert!(!folder(&profiles[0]).join(logic::PASSWORD_FILES[1]).exists());
        for name in logic::PASSWORD_FILES {
            assert!(!folder(&profiles[1]).join(name).exists());
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn the_child_hosts_profile_goes_only_when_it_is_that_hosts() {
        let directory = scratch();
        let profile = directory.join(CHILD_PROFILE);
        assert_eq!(remove_child_profile(&directory), Ok(false));
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(profile.join("sunshine.conf"), "port = 47989\n").unwrap();
        assert_eq!(remove_child_profile(&directory), Ok(false));
        assert!(profile.is_dir());
        std::fs::write(
            profile.join("sunshine.conf"),
            "port = 47999\n child_session_host = True \n",
        )
        .unwrap();
        std::fs::write(
            profile.join("sunshine_state.json"),
            r#"{"username":"admin"}"#,
        )
        .unwrap();
        assert_eq!(remove_child_profile(&directory), Ok(true));
        assert!(!profile.exists());
        std::fs::remove_dir_all(directory).unwrap();
    }
}
