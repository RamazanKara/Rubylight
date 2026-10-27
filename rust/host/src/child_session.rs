//! Background streaming in a Windows child session (Experimental, off by
//! default: `background_mode`). The console sets the PC up and restores it, a
//! viewer started as the signed-in user attaches the child session's desktop
//! (`--child-session-viewer`), and a second host streams that desktop from
//! inside the child session with its own ports and name. The rules are
//! `butterpollo_core::child_session`, ported from Annex (MIT).
use crate::state::Shared;
use anyhow::{Context, Result, bail};
use butterpollo_core::{
    child_session::{
        self as logic, DisplaySettings, LineReader, Mode, ModeDecision, ModeSetting, Phase,
        Registry, SetupState, SystemSettings, ViewerCommand, ViewerStatus,
    },
    config::Config,
    state::{Credentials, ProfileFiles, load_json, write_json},
};
use butterpollo_windows::{
    child_session as native,
    process::{PipedChild, Process, Target},
};
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    fs::File,
    io::{ErrorKind, Read, Write},
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard, PoisonError},
    time::Instant,
};

/// The child-session host's own profile, inside this host's.
const CHILD_PROFILE: &str = "child-session";

/// The PC's one record of what setup changed ([`logic::setup_record`]),
/// whichever Rubylight profile set it up: two profiles (the service's, a host
/// started by hand) each with their own would record each other's changes as
/// the originals. A profile's own record from before (in `legacy`) moves to it
/// once ([`Record::take_over`]).
#[derive(Clone)]
struct Record {
    path: PathBuf,
    legacy: PathBuf,
    /// Make SYSTEM and Administrators alone the folder's (or file's) owner
    /// and writers. Users may read it: it holds nothing secret, and a host
    /// without administrator rights reads it to say why child sessions are
    /// unavailable in it ([`unverified`]).
    restrict: fn(&Path) -> Result<()>,
    /// Err unless SYSTEM or Administrators own the open file or folder, which
    /// `path` names.
    trusted: fn(&File, &Path) -> Result<()>,
}
impl Record {
    /// This PC's record, and the profile in `directory`'s own from before.
    fn new(directory: &Path) -> Self {
        Self {
            path: logic::setup_record(),
            legacy: directory.join(logic::SETUP_FILE),
            restrict: butterpollo_windows::process::restrict_to_administrators_readable,
            trusted: administrators_own,
        }
    }
    fn folder(&self) -> &Path {
        self.path.parent().unwrap_or(&self.path)
    }
    /// Whether setup may have run: either record is there, or that can't be told.
    fn may_exist(&self) -> bool {
        !matches!(
            (self.path.try_exists(), self.legacy.try_exists()),
            (Ok(false), Ok(false))
        )
    }
    /// The record, the profile's own from before when the PC has none yet.
    /// None when setup never ran; Err for one SYSTEM or Administrators do not
    /// own (any program can write `%ProgramData%`), which is never used.
    fn load(&self) -> Result<Option<SetupState>> {
        match self.read(&self.path, true)? {
            Some(state) => Ok(Some(state)),
            None => self.read(&self.legacy, false),
        }
    }
    /// The record at `path` (in the PC's folder when `in_folder`), checked and
    /// read through one handle, so what is read is what was checked.
    fn read(&self, path: &Path, in_folder: bool) -> Result<Option<SetupState>> {
        // Held until the file is read: it can't be swapped for another folder meanwhile.
        let _folder = if in_folder {
            match self.open_folder()? {
                Some(folder) => Some(folder),
                None => return Ok(None),
            }
        } else {
            None
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
    /// Save it in a folder only SYSTEM and Administrators can write.
    fn save(&self, state: &SetupState) -> Result<()> {
        let folder = self.folder();
        if !folder.exists() {
            std::fs::create_dir_all(folder)
                .with_context(|| format!("creating {}", folder.display()))?;
            (self.restrict)(folder)?;
        }
        // Held while the record is written into it.
        let _folder = self
            .open_folder()?
            .with_context(|| format!("{} is gone", folder.display()))?;
        let temporary = self.path.with_extension("tmp");
        std::fs::write(&temporary, serde_json::to_vec_pretty(state)?)
            .with_context(|| format!("writing {}", temporary.display()))?;
        (self.restrict)(&temporary)?;
        std::fs::rename(&temporary, &self.path)
            .with_context(|| format!("saving {}", self.path.display()))
    }
    /// Forget it, once everything is restored.
    fn remove(&self) -> Result<()> {
        for path in [&self.path, &self.legacy] {
            match std::fs::remove_file(path) {
                Err(error) if error.kind() != ErrorKind::NotFound => {
                    return Err(error).with_context(|| format!("removing {}", path.display()));
                }
                _ => {}
            }
        }
        Ok(())
    }
    /// Move the profile's own record from before to the PC's, once, under the
    /// settings locks. The PC's record is never overwritten: with both there,
    /// the profile's goes, into the log.
    fn take_over(&self) -> Result<()> {
        let Some(old) = self.read(&self.legacy, false)? else {
            return Ok(());
        };
        if self.read(&self.path, true)?.is_some() {
            tracing::warn!(
                record = %self.path.display(),
                superseded = ?old,
                "the PC's child-session record is kept; this profile's own from before is deleted"
            );
        } else {
            self.save(&old)?;
            tracing::info!(
                from = %self.legacy.display(),
                to = %self.path.display(),
                "moved the child-session record to the PC's"
            );
        }
        std::fs::remove_file(&self.legacy)
            .with_context(|| format!("removing {}", self.legacy.display()))
    }
}
/// [`Record`]'s check on this PC, on the open `file` (`path` names it):
/// owned by SYSTEM or Administrators, and not a link elsewhere.
fn administrators_own(file: &File, path: &Path) -> Result<()> {
    use std::os::windows::fs::MetadataExt;
    let metadata = file
        .metadata()
        .with_context(|| format!("reading {}", path.display()))?;
    if metadata.file_attributes() & 0x400 != 0 {
        bail!(
            "{} is a link to elsewhere, not Rubylight's; Rubylight does not use it",
            path.display()
        );
    }
    let owner = butterpollo_windows::process::open_file_owner(file)
        .with_context(|| format!("reading the owner of {}", path.display()))?;
    if !matches!(owner.as_str(), "S-1-5-18" | "S-1-5-32-544") {
        bail!(
            "{} is owned by {owner}, not by SYSTEM or Administrators, so another program may have written it; Rubylight does not use it. Delete it, turn Remote Desktop off, then set up child sessions again",
            path.display()
        );
    }
    Ok(())
}

/// Which background mode this host runs and why. A `background_mode` the file
/// holds but that is not a mode keeps the default and says why.
pub(crate) fn decision(config: &Config) -> ModeDecision {
    let inside = config.boolean(logic::CHILD_HOST_KEY, false);
    let setting = ModeSetting::parse(config.get(logic::MODE_KEY, ""));
    let mut decision = logic::decide(
        *setting.as_ref().unwrap_or(&ModeSetting::VirtualDisplay),
        &native::edition_id(),
        inside,
    );
    if let (Err(error), false) = (&setting, inside) {
        decision.reason = format!("{error:#}; background streams use the isolated virtual display");
    }
    decision
}

/// The startup line naming the background mode. The child session's own host
/// also says which GPU renders its desktop, loudly when it can't be encoded.
pub(crate) fn log_mode(config: &Config) {
    let decision = decision(config);
    tracing::info!(mode = ?decision.mode, reason = %decision.reason, "{}", decision.label);
    if decision.mode != Mode::InsideChildSession {
        return;
    }
    match native::adapters() {
        Ok((all, render)) => {
            let verdict = logic::gpu_verdict(render.as_ref(), &all);
            match logic::gpu_problem(&verdict) {
                Some(problem) => tracing::error!(?verdict, "{problem}"),
                None => tracing::info!(?verdict, "the child session renders on this GPU"),
            }
        }
        Err(error) => tracing::error!(
            error = format!("{error:#}"),
            "the child-session host could not read its GPUs"
        ),
    }
}

/// From the host's start on, every [`WATCH_EVERY`], when child sessions are
/// set up here: RDP stays on until restore, so a firewall that no longer
/// keeps it on this PC turns RDP off and is said loudly (and [`start`]
/// refuses until setup runs again).
pub(crate) fn check_at_startup(h: &Shared) {
    let record = Record::new(&h.directory);
    let elevated = elevated();
    std::thread::spawn(move || {
        let mut last = None;
        loop {
            watch(
                &record,
                &mut last,
                elevated,
                native_enforce,
                std::thread::sleep,
                report,
            );
            std::thread::sleep(WATCH_EVERY);
        }
    });
}
/// How often the host checks the firewall while child sessions are set up:
/// one firewall read, off any stream's path.
const WATCH_EVERY: std::time::Duration = std::time::Duration::from_secs(60);
/// One [`secure_rdp`] round of [`check_at_startup`]: a problem is reported
/// when it is new (`last` holds the one before), not every round it stays.
/// A host that is not `elevated` checks nothing and changes nothing; while
/// setup ran here it says once that child sessions are unavailable in it.
fn watch(
    record: &Record,
    last: &mut Option<String>,
    elevated: bool,
    enforce: impl FnMut(Option<u16>, bool) -> Result<Vec<String>>,
    wait: impl FnMut(std::time::Duration),
    mut report: impl FnMut(String),
) {
    if let Some(reason) = unverified(record, elevated) {
        if last.as_ref() != Some(&reason) {
            tracing::warn!("{reason}");
        }
        *last = Some(reason);
        return;
    }
    let problem = secure_rdp(record, enforce, wait);
    if let Some(problem) = &problem
        && last.as_ref() != Some(problem)
    {
        report(problem.clone());
    }
    *last = problem;
}

/// Whether this host has administrator rights; when that can't be told, as
/// if it had, so [`secure_rdp`] checks and fails closed.
fn elevated() -> bool {
    butterpollo_windows::process::elevated().unwrap_or_else(|error| {
        tracing::warn!(
            "could not tell whether this host is elevated, so it checks the firewall: {error:#}"
        );
        true
    })
}
/// Why child sessions are unavailable in a host that is not `elevated` while
/// setup ran here (or that can't be told): it can't take the settings locks,
/// check the firewall or turn Remote Desktop off. None when it is elevated,
/// or setup never ran. Never a failure: the service, or a host run as
/// administrator, checks the firewall and starts them.
fn unverified(record: &Record, elevated: bool) -> Option<String> {
    if elevated || !record.may_exist() {
        return None;
    }
    let found = match record.load() {
        Ok(None) => return None,
        Ok(Some(_)) => "child sessions are set up on this PC".to_owned(),
        Err(error) => format!("Rubylight's child-session record could not be read ({error:#})"),
    };
    Some(format!(
        "{found}, but this host runs without administrator rights, so it can't check that the firewall keeps Remote Desktop on this PC: child sessions are unavailable in this host. The Rubylight service, or a host run as administrator, checks it and starts them."
    ))
}

/// Setup, restore and the firewall checks change the same Windows settings,
/// one at a time: a check that turned RDP off between restore's write of the
/// original value and its read-back would lose the user's own value. The
/// viewer's start holds it too, so no restore runs between its checks and the
/// viewer it starts.
///
/// Two locks, always taken in this order and let go of in reverse, so neither
/// waits on the other: the cross-process [`native::lock::setup_lock`] (the
/// uninstaller's `--child-session-restore`, another host), then this process's
/// own [`SETTINGS`]. [`RUNNING`] is only ever taken after both, or alone.
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

/// How often [`secure_rdp`] tries a check that could not run (Windows
/// Firewall or its rule script failing, as while Windows starts), and how long
/// it waits in between; the last try fails closed.
const CHECK_TRIES: u32 = 3;
const CHECK_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// When child sessions are set up here, [`logic::enforce_firewall`] for the
/// port setup recorded, through `enforce(port, last)`. A check that could not
/// run (`Err`, nothing changed) is tried again after `wait`, up to
/// [`CHECK_TRIES`]; on the last try it turns RDP off instead. What a check
/// finds is never tried again: it turns RDP off at once. It fails closed: a
/// record that cannot be read turns RDP off too (`enforce(None, _)`), and so
/// does a settings lock another process holds too long. The problem for the
/// log and the console, None when there is none or setup never ran.
fn secure_rdp(
    record: &Record,
    mut enforce: impl FnMut(Option<u16>, bool) -> Result<Vec<String>>,
    mut wait: impl FnMut(std::time::Duration),
) -> Option<String> {
    // Setup never ran here: nothing to check, and no lock to take, which an unelevated host may
    // not be allowed to. A setup that runs meanwhile is as if it ran right after this check.
    if !record.may_exist() {
        return None;
    }
    let mut found = Vec::new();
    for attempt in 1..=CHECK_TRIES {
        let last = attempt == CHECK_TRIES;
        // Only the try that ends this reports.
        found.clear();
        // Its wait was long enough not to try again. It fails closed: RDP goes off, as without
        // a record, even though a restore holding the lock may then lose the user's own value.
        let settings = match settings_lock() {
            Ok(settings) => settings,
            Err(error) => {
                found.push(format!("the firewall could not be checked ({error:#})"));
                match enforce(None, true) {
                    Ok(done) => found.extend(done),
                    Err(error) => found.push(format!("{error:#}")),
                }
                break;
            }
        };
        let port = match record.take_over().and_then(|()| record.load()) {
            Ok(None) => return None,
            Ok(Some(state)) => Some(state.rdp_port),
            Err(error) => {
                found.push(format!("{error:#}"));
                None
            }
        };
        match enforce(port, last) {
            Ok(done) => {
                found.extend(done);
                break;
            }
            Err(error) if !last => {
                // Setup and restore can run while this waits.
                drop(settings);
                tracing::warn!(
                    attempt,
                    "the child-session firewall check could not run, trying again in {} s: {error:#}",
                    CHECK_WAIT.as_secs()
                );
                wait(CHECK_WAIT);
            }
            Err(error) => found.push(format!("the firewall could not be checked ({error:#})")),
        }
    }
    (!found.is_empty()).then(|| {
        format!(
            "child sessions are set up, but the firewall may no longer keep Remote Desktop on this PC: {}. Fix that, then set up child sessions again (or choose Restore Windows settings); the child session does not start until then.",
            found.join("; ")
        )
    })
}
/// [`secure_rdp`]'s `enforce` on this PC.
fn native_enforce(port: Option<u16>, last: bool) -> Result<Vec<String>> {
    enforce_with(
        port,
        last,
        native::NativeSettings::new,
        &mut native::NativeRegistry,
    )
}
/// [`logic::check_firewall`] through the settings `open` makes; Err when it
/// could not run and this is not the `last` try. It fails closed: without a
/// record (`None`), RDP goes off at once, and on the last try a check that
/// cannot run turns it off too, through `registry` alone when those settings
/// (Windows Firewall) can't be opened.
fn enforce_with<S: SystemSettings>(
    port: Option<u16>,
    last: bool,
    open: impl FnOnce() -> Result<S>,
    registry: &mut dyn Registry,
) -> Result<Vec<String>> {
    let Some(port) = port else {
        return Ok(vec![logic::turn_rdp_off(registry)]);
    };
    match open() {
        Ok(mut system) if last => Ok(logic::enforce_firewall(&mut system, port)),
        Ok(mut system) => logic::check_firewall(&mut system, port),
        Err(error) if last => Ok(vec![
            format!(
                "Windows Firewall could not be opened ({error:#}), so Remote Desktop could not be checked"
            ),
            logic::turn_rdp_off(registry),
        ]),
        Err(error) => Err(error.context("opening Windows Firewall")),
    }
}

/// `butterpollo.exe --child-session-restore`, which the uninstaller runs: put
/// back the Windows settings child-session setup changed. 0 when they are back
/// or setup never ran here, 1 with the reason on stderr. It fails closed, as
/// [`secure_rdp`] does: a restore that failed, or could not get the settings
/// lock, turns RDP off, without the lock.
pub(crate) fn restore_exit_code(config_dir: Option<PathBuf>) -> i32 {
    let directory = config_dir.unwrap_or_else(butterpollo_core::paths::installed_profile);
    restore_exit_code_with(
        &Record::new(&directory),
        |record| restore_saved(record, native::NativeSettings::new),
        || logic::turn_rdp_off(&mut native::NativeRegistry),
    )
}
fn restore_exit_code_with(
    record: &Record,
    restore: impl FnOnce(&Record) -> Result<bool>,
    rdp_off: impl FnOnce() -> String,
) -> i32 {
    let restored = if record.may_exist() {
        restore_locked(record, restore)
    } else {
        // Setup never ran: nothing to restore, and no lock to take.
        Ok(false)
    };
    match restored {
        Ok(true) => {
            println!("Restored the Windows settings child-session setup changed.");
            0
        }
        Ok(false) => {
            println!("Child sessions were not set up by Rubylight here; nothing to restore.");
            0
        }
        Err(error) => {
            let off = rdp_off();
            eprintln!(
                "The Windows settings child-session setup changed were not restored: {error:#}. {off}. Rubylight keeps its record, so restoring again (Restore Windows settings in the console, or butterpollo.exe {}) puts back the rest.",
                logic::RESTORE_FLAG
            );
            1
        }
    }
}

/// `restore` ([`restore_saved`]) under the settings locks.
fn restore_locked(record: &Record, restore: impl FnOnce(&Record) -> Result<bool>) -> Result<bool> {
    let _settings = settings_lock()?;
    restore(record)
}
/// Put back what setup changed, through the settings `open` makes, then
/// forget it; false when setup never ran. The record stays when anything was
/// not restored, so a retry has it.
fn restore_saved<S: SystemSettings>(
    record: &Record,
    open: impl FnOnce() -> Result<S>,
) -> Result<bool> {
    record.take_over()?;
    let Some(state) = record.load()? else {
        return Ok(false);
    };
    logic::restore(&mut open()?, &state)?;
    record.remove()?;
    Ok(true)
}

/// The viewer and the child session's host while they run.
struct Running {
    viewer: Process,
    commands: File,
    status: Option<ViewerStatus>,
    /// The child session the host streams, and that host.
    host: Option<(u32, Process)>,
    /// A child session whose host could not start; it is not tried again.
    refused: Option<u32>,
}
static RUNNING: Mutex<Option<Running>> = Mutex::new(None);
/// The last failure, for the console.
static PROBLEM: Mutex<Option<String>> = Mutex::new(None);

fn running() -> MutexGuard<'static, Option<Running>> {
    RUNNING.lock().unwrap_or_else(PoisonError::into_inner)
}
fn report(problem: String) {
    tracing::error!("{problem}");
    *PROBLEM.lock().unwrap_or_else(PoisonError::into_inner) = Some(problem);
}

/// Set up child sessions for the account signed in at this PC's screen. Only
/// the main host in child-session mode does; the child session's own host
/// never touches the PC's settings.
pub(crate) fn setup(h: &Shared) -> Result<SetupState> {
    let config = h
        .config
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    refuse_inside(&config)?;
    let decision = decision(&config);
    if decision.mode != Mode::ChildSession {
        bail!(
            "{}: {}. Choose child sessions as the background mode first; nothing was changed.",
            decision.label,
            decision.reason
        );
    }
    let user = native::console_user()?;
    let state = setup_in(
        &Record::new(&h.directory),
        &user,
        native::NativeSettings::new,
    );
    // Even a setup that failed may have changed some.
    RULES.forget();
    let state = state?;
    tracing::info!("child sessions are set up; restore puts the Windows settings back");
    Ok(state)
}
/// [`logic::setup`] for `user` through the settings `open` makes, under the
/// settings locks.
fn setup_in<S: SystemSettings>(
    record: &Record,
    user: &str,
    open: impl FnOnce() -> Result<S>,
) -> Result<SetupState> {
    let _settings = settings_lock()?;
    record.take_over()?;
    let saved = record.load()?;
    logic::setup(
        &mut open()?,
        saved,
        user,
        |state| record.save(state),
        std::thread::sleep,
    )
}
/// The child session's own host sets nothing up and restores nothing: the
/// record is the PC's, and the main host's.
fn refuse_inside(config: &Config) -> Result<()> {
    if config.boolean(logic::CHILD_HOST_KEY, false) {
        bail!(
            "this is the child session's host: set up and restore child sessions in the main host's console"
        );
    }
    Ok(())
}

/// Stop the viewer and the child session's host, then put back what setup
/// changed. False when setup never ran here. Both under the settings locks,
/// so no start comes in between.
pub(crate) fn restore(h: &Shared) -> Result<bool> {
    refuse_inside(&h.config.read().unwrap_or_else(PoisonError::into_inner))?;
    let restored = restore_locked(&Record::new(&h.directory), |record| {
        quit();
        restore_saved(record, native::NativeSettings::new)
    });
    // Even a restore that failed may have put some back.
    RULES.forget();
    let restored = restored?;
    if restored {
        tracing::info!("the Windows settings child-session setup changed are restored");
    }
    Ok(restored)
}

/// Attach the child session's desktop, starting the viewer when it is not
/// running. `password` is a newly entered Windows password; None uses the
/// saved one.
pub(crate) fn start(h: &Shared, password: Option<logic::Secret>) -> Result<()> {
    let config = h
        .config
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let decision = decision(&config);
    if decision.mode != Mode::ChildSession {
        bail!("{}: {}", decision.label, decision.reason);
    }
    let display = DisplaySettings::parse(
        config.get(logic::RESOLUTION_KEY, ""),
        config.get(logic::SCALE_KEY, ""),
    )?;
    // The port setup recorded, which RDP and the block rule must still match;
    // on a mismatch, or a check that cannot run, RDP goes off.
    let record = Record::new(&h.directory);
    if let Some(reason) = unverified(&record, elevated()) {
        bail!("{reason}");
    }
    if let Some(problem) = secure_rdp(&record, native_enforce, std::thread::sleep) {
        report(problem.clone());
        bail!("{problem}");
    }
    // From the settings check to the viewer's command: no setup or restore in between.
    let _settings = settings_lock()?;
    let system = native::NativeSettings::new()?;
    let user = native::console_user()?;
    record.take_over()?;
    let missing = match record.load()? {
        Some(_) => logic::settings_missing(&system, &user)?,
        None => logic::missing(&system, &user, None)?,
    };
    if !missing.is_empty() {
        bail!("set up child sessions first: {}", missing.join("; "));
    }
    let command = start_command(&record, display, password)?;
    let mut running = running();
    let alive = running
        .as_ref()
        .is_some_and(|running| matches!(running.viewer.exit_code(), Ok(None)));
    if !alive {
        if let Some(mut gone) = running.take() {
            stop_host(&mut gone);
        }
        *running = Some(spawn_viewer(h)?);
    }
    *PROBLEM.lock().unwrap_or_else(PoisonError::into_inner) = None;
    let running = running
        .as_mut()
        .context("the child-session viewer is not running")?;
    running.refused = None;
    send_to(running, &command)
}
/// The viewer connects to the RDP port setup recorded, so a PC whose RDP
/// listens elsewhere is not left to the client's default.
fn start_command(
    record: &Record,
    display: DisplaySettings,
    password: Option<logic::Secret>,
) -> Result<ViewerCommand> {
    let state = record
        .load()?
        .context("set up child sessions first: Rubylight has no record of the setup here")?;
    Ok(ViewerCommand::Start {
        display,
        rdp_port: state.rdp_port,
        password,
    })
}

/// Send a command to the running viewer.
pub(crate) fn send(command: ViewerCommand) -> Result<()> {
    let mut running = running();
    let running = running
        .as_mut()
        .context("the child-session viewer is not running; start the child session first")?;
    send_to(running, &command)
}
fn send_to(running: &mut Running, command: &ViewerCommand) -> Result<()> {
    // The line can hold the password: it is wiped when dropped and never logged.
    let line = command.line()?;
    running
        .commands
        .write_all(line.as_bytes())
        .and_then(|()| running.commands.flush())
        .context("the child-session viewer did not take the command")
}

/// Quit the viewer and stop the child session's host; the child session
/// itself keeps running until it is signed out.
pub(crate) fn quit() {
    let Some(mut gone) = running().take() else {
        return;
    };
    let _ = send_to(&mut gone, &ViewerCommand::Quit);
    stop_host(&mut gone);
    // Dropping the viewer ends it if it has not quit by then.
    std::thread::spawn(move || {
        let _ = gone.viewer.wait(std::time::Duration::from_secs(5));
    });
}

fn spawn_viewer(h: &Shared) -> Result<Running> {
    let program = std::env::current_exe()?;
    let PipedChild {
        process,
        stdin,
        stdout,
        stderr,
    } = Process::spawn_piped(
        &program,
        &[OsString::from("--child-session-viewer")],
        Target::User { elevated: false },
    )
    .context("starting the child-session viewer as the signed-in user")?;
    let pid = process.pid;
    let commands = stdin.context("the child-session viewer has no command pipe")?;
    let output = stdout.context("the child-session viewer has no status pipe")?;
    if let Some(errors) = stderr {
        std::thread::spawn(move || read_errors(pid, errors));
    }
    let h = h.clone();
    std::thread::spawn(move || read_status(&h, pid, output));
    tracing::info!(
        pid,
        "started the child-session viewer as the signed-in user"
    );
    Ok(Running {
        viewer: process,
        commands,
        status: None,
        host: None,
        refused: None,
    })
}

/// The viewer's standard error, line by line into the log.
fn read_errors(pid: u32, errors: impl Read) {
    let mut lines = LineReader::new(errors, logic::STATUS_LINE_CAP);
    loop {
        match lines.read_line() {
            Ok(Some(line)) => {
                tracing::warn!(viewer = pid, "{}", String::from_utf8_lossy(line));
            }
            Ok(None) => break,
            // Nothing of the line itself: only that it was too long.
            Err(error) if error.kind() == ErrorKind::InvalidData => {
                tracing::warn!(viewer = pid, %error, "the child-session viewer wrote a line that is not logged");
            }
            Err(error) => {
                tracing::warn!(viewer = pid, %error, "the child-session viewer's error pipe failed");
                break;
            }
        }
    }
}

fn read_status(h: &Shared, pid: u32, output: File) {
    let mut lines = LineReader::new(output, logic::STATUS_LINE_CAP);
    loop {
        let line = match lines.read_line() {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(error) if error.kind() == ErrorKind::InvalidData => {
                tracing::warn!(%error, "the child-session viewer sent a line that is not a status");
                continue;
            }
            Err(error) => {
                tracing::warn!(%error, "the child-session viewer's status pipe failed");
                break;
            }
        };
        let status: ViewerStatus = match serde_json::from_slice(line) {
            Ok(status) => status,
            Err(error) => {
                tracing::warn!(%error, "the child-session viewer sent a line that is not a status");
                continue;
            }
        };
        let mut running = running();
        let Some(current) = running.as_mut().filter(|r| r.viewer.pid == pid) else {
            return;
        };
        if current.status.as_ref().map(|s| (s.phase, s.child)) != Some((status.phase, status.child))
        {
            tracing::info!(phase = ?status.phase, child = status.child, "child session: {}", status.message);
        }
        if let Some(notice) = &status.notice {
            tracing::warn!("child session: {notice}");
        }
        follow(h, current, &status);
        current.status = Some(status);
    }
    let mut running = running();
    if let Some(mut gone) = running.take_if(|r| r.viewer.pid == pid) {
        stop_host(&mut gone);
        report("the child-session viewer exited; start the child session again".into());
    }
}

/// Start the child session's host once the viewer has its desktop attached;
/// stop it when the desktop is gone or another child session is attached.
fn follow(h: &Shared, running: &mut Running, status: &ViewerStatus) {
    let attached =
        matches!(status.phase, Phase::Connected | Phase::Reconnecting) && status.child != 0;
    if running
        .host
        .as_ref()
        .is_some_and(|(child, _)| !attached || *child != status.child)
    {
        stop_host(running);
    }
    if status.phase != Phase::Connected
        || status.child == 0
        || running.host.is_some()
        || running.refused == Some(status.child)
    {
        return;
    }
    match start_host(h, status.child, running.viewer.pid) {
        Ok(process) => {
            tracing::info!(
                child = status.child,
                pid = process.pid,
                "started the child session's host"
            );
            running.host = Some((status.child, process));
        }
        Err(error) => {
            running.refused = Some(status.child);
            report(format!("the child session's host did not start: {error:#}"));
        }
    }
}

/// The child session's host: SYSTEM in that session, as the service runs the
/// main host, only after the session is checked to be the console user's own,
/// reported by the viewer at this PC's screen.
fn start_host(h: &Shared, child: u32, viewer: u32) -> Result<Process> {
    native::verify_child_owner(child, viewer)?;
    let (name, port) = {
        let config = h.config.read().unwrap_or_else(PoisonError::into_inner);
        (config.get("sunshine_name", "").to_owned(), config.port()?)
    };
    let credentials = h
        .credentials
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let profile = h.directory.join(CHILD_PROFILE);
    write_child_profile(&profile, &name, port, credentials.as_ref())?;
    let program = std::env::current_exe()?;
    let args = [
        OsString::from("--config-dir"),
        profile.into_os_string(),
        OsString::from("--no-tray"),
    ];
    Process::spawn_host(
        &program,
        &args,
        program.parent(),
        Target::SystemSession(child),
    )
    .with_context(|| format!("starting the host in child session {child}"))
}

/// The child host's settings over whatever its profile already holds, and the
/// main console's sign-in: a child host without one would let any program on
/// this PC claim its console (it runs as SYSTEM), so it never starts without.
fn write_child_profile(
    profile: &Path,
    name: &str,
    port: u16,
    credentials: Option<&Credentials>,
) -> Result<()> {
    let credentials = credentials
        .context("set the console's sign-in first: the child session's host uses the same one")?;
    std::fs::create_dir_all(profile).with_context(|| format!("creating {}", profile.display()))?;
    let file = profile.join("sunshine.conf");
    let mut config = if file.is_file() {
        Config::load(&file)?
    } else {
        Config::default()
    };
    for (key, value) in logic::child_host_config(name, port) {
        config.values.insert(key.to_owned(), value);
    }
    std::fs::write(&file, config.text()).with_context(|| format!("writing {}", file.display()))?;
    // Into whichever file the child profile keeps its sign-in in, beside what
    // that file holds (it can be the paired-devices file too).
    let path = ProfileFiles::new(&config, profile).credentials;
    let mut document = load_json(&path, json!({}))?;
    let Value::Object(fields) = serde_json::to_value(credentials)? else {
        bail!("the console's sign-in is not an object");
    };
    for (key, value) in fields {
        document[key] = value;
    }
    write_json(&path, &document)
}

fn stop_host(running: &mut Running) {
    let Some((child, host)) = running.host.take() else {
        return;
    };
    tracing::info!(child, pid = host.pid, "stopping the child session's host");
    std::thread::spawn(move || {
        if let Err(error) = host.shutdown_host() {
            tracing::warn!(
                child,
                error = format!("{error:#}"),
                "the child session's host did not stop"
            );
        }
    });
}

/// How long [`status`] keeps the firewall rules it read: each read starts
/// Windows PowerShell twice, and the console asks every few seconds.
const RULES_FOR: std::time::Duration = std::time::Duration::from_secs(30);
/// The firewall rules [`status`] read last, and when; setup and restore,
/// which change them, forget them.
struct RulesCache(Mutex<Option<(Instant, logic::FirewallRules)>>);
impl RulesCache {
    const fn new() -> Self {
        Self(Mutex::new(None))
    }
    /// The rules read less than [`RULES_FOR`] before `now`, or those `read`
    /// reads now. Held while reading, so status asked twice at once reads
    /// once. A read that fails is not kept.
    fn get(
        &self,
        now: Instant,
        read: impl FnOnce() -> Result<logic::FirewallRules>,
    ) -> Result<logic::FirewallRules> {
        let mut kept = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((at, rules)) = kept.as_ref()
            && now.saturating_duration_since(*at) < RULES_FOR
        {
            return Ok(rules.clone());
        }
        *kept = None;
        let rules = read()?;
        *kept = Some((now, rules.clone()));
        Ok(rules)
    }
    fn forget(&self) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }
}
static RULES: RulesCache = RulesCache::new();

/// What the console shows.
pub(crate) fn status(h: &Shared) -> Value {
    let config = h
        .config
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let decision = decision(&config);
    let record = Record::new(&h.directory);
    let missing = (|| {
        let rdp_port = record.load()?.map(|state| state.rdp_port);
        let system = native::NativeSettings::new()?;
        let rules = RULES.get(Instant::now(), || logic::FirewallRules::read(&system))?;
        logic::missing_with(&system, &native::console_user()?, rdp_port, &rules)
    })();
    let display = DisplaySettings::parse(
        config.get(logic::RESOLUTION_KEY, ""),
        config.get(logic::SCALE_KEY, ""),
    );
    let mut running = running();
    if let Some(current) = running.as_mut()
        && let Some((child, host)) = &current.host
        && let Ok(Some(code)) = host.exit_code()
    {
        let child = *child;
        current.host = None;
        current.refused = Some(child);
        report(format!(
            "the child session's host exited with code {code}; its log is in {}",
            h.directory.join(CHILD_PROFILE).join("logs").display()
        ));
    }
    let viewer = running.as_ref().map(|current| {
        json!({
            "status": current.status,
            "host_child": current.host.as_ref().map(|(child, _)| *child),
        })
    });
    let port = config.port().ok().and_then(|port| {
        logic::child_host_config("", port)
            .into_iter()
            .find(|(key, _)| *key == "port")
            .and_then(|(_, value)| value.parse::<u16>().ok())
    });
    json!({
        "mode": decision.mode,
        "label": decision.label,
        "reason": decision.reason,
        "setting": config.get(logic::MODE_KEY, ""),
        "edition": native::edition_id(),
        "missing": missing.as_ref().ok(),
        "missing_error": missing.as_ref().err().map(|error| format!("{error:#}")),
        "set_up": record.path.is_file() || record.legacy.is_file(),
        "display": display.as_ref().ok(),
        "display_error": display.as_ref().err().map(|error| format!("{error:#}")),
        "viewer": viewer,
        "host_port": port,
        "problem": *PROBLEM.lock().unwrap_or_else(PoisonError::into_inner),
        "limits": logic::LIMITS,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    thread_local! {
        /// [`settings_wait`] on this test thread, when not the host's.
        pub(super) static SETTINGS_WAIT: std::cell::Cell<Option<std::time::Duration>> =
            const { std::cell::Cell::new(None) };
    }

    #[test]
    fn the_mode_is_off_until_asked_for_and_a_bad_value_says_why() {
        let config = |text: &str| Config::parse(text).unwrap();
        assert_eq!(decision(&config("")).mode, Mode::VirtualDisplay);
        let bad = decision(&config("background_mode = rdp\n"));
        assert_eq!(bad.mode, Mode::VirtualDisplay);
        assert!(bad.reason.contains("is not a mode"), "{}", bad.reason);
        let inside = decision(&config(
            "background_mode = rdp\nchild_session_host = true\n",
        ));
        assert_eq!(inside.mode, Mode::InsideChildSession);
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
    /// The PC's record and a profile's own from before, in `directory`.
    fn record_in(directory: &Path) -> Record {
        let profile = directory.join("config");
        std::fs::create_dir_all(&profile).unwrap();
        Record {
            path: directory.join(logic::SETUP_FOLDER).join(logic::SETUP_FILE),
            legacy: profile.join(logic::SETUP_FILE),
            restrict: |_| Ok(()),
            trusted: |_, _| Ok(()),
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

    /// The Windows settings setup changes, shared by every [`Pc`] handle as
    /// one PC's are by every host on it.
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    struct Machine {
        values: std::collections::BTreeMap<(String, String, String), u32>,
        child_sessions: bool,
        /// Rule ID -> enabled, all in the Remote Desktop group.
        rules: std::collections::BTreeMap<String, bool>,
        block_rules: std::collections::BTreeMap<String, Vec<u16>>,
    }
    #[derive(Clone)]
    struct Pc(std::rc::Rc<std::cell::RefCell<Machine>>);
    impl Pc {
        fn new() -> Self {
            let mut machine = Machine::default();
            machine.values.insert(
                (
                    "LocalMachine".into(),
                    logic::TERMINAL_SERVER.into(),
                    "fDenyTSConnections".into(),
                ),
                1,
            );
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
            self.0.borrow_mut().child_sessions = enabled;
            Ok(())
        }
        fn firewall_problem(&self) -> Result<Option<String>> {
            Ok(None)
        }
        fn active_firewall(&self, group: &str, block_rule: &str) -> Result<logic::ActiveFirewall> {
            let ports = self.0.borrow().block_rules.get(block_rule).cloned();
            Ok(logic::ActiveFirewall {
                allowed: self.enabled_rules(group)?,
                blocked: ports.is_some(),
                block_ports: ports
                    .map(|ports| {
                        let ports: Vec<String> = ports.iter().map(u16::to_string).collect();
                        vec![ports.join(","); 2]
                    })
                    .unwrap_or_default(),
                unprotected: Vec::new(),
            })
        }
        fn enabled_rules(&self, _: &str) -> Result<Vec<String>> {
            Ok(self
                .0
                .borrow()
                .rules
                .iter()
                .filter(|(_, enabled)| **enabled)
                .map(|(id, _)| id.clone())
                .collect())
        }
        fn set_rule_enabled(&mut self, _: &str, id: &str, enabled: bool) -> Result<()> {
            *self
                .0
                .borrow_mut()
                .rules
                .get_mut(id)
                .context("no such rule")? = enabled;
            Ok(())
        }
        fn add_block_rule(&mut self, name: &str, ports: &[u16]) -> Result<()> {
            self.0
                .borrow_mut()
                .block_rules
                .insert(name.into(), ports.to_vec());
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
        fn rdp_listeners(&self) -> Result<logic::RdpListeners> {
            Ok(logic::RdpListeners {
                shared: false,
                listeners: vec![logic::ServiceListener {
                    port: 3389,
                    owner: None,
                }],
            })
        }
    }
    const USER: &str = "S-1-5-21-1";

    #[test]
    fn two_profiles_share_one_record_and_restore_leaves_rdp_off() {
        // Profile B (a host started by hand) sets up, then A (the service), then B restores,
        // then A. With a record per profile, A's held B's changes as the originals, and A's
        // restore turned RDP back on for good.
        let directory = scratch();
        let pc = Pc::new();
        let before = pc.now();
        let b = record_in(&directory.join("b"));
        let a = Record {
            legacy: directory.join("a").join(logic::SETUP_FILE),
            ..b.clone()
        };
        let first = setup_in(&b, USER, pc.open()).unwrap();
        assert_eq!(first.deny_ts_connections, Some(1));
        let set_up = pc.now();
        assert_ne!(set_up, before);
        // A finds the PC's record: a repair that keeps B's originals.
        assert_eq!(setup_in(&a, USER, pc.open()).unwrap(), first);
        assert_eq!(pc.now(), set_up);
        assert!(restore_saved(&b, pc.open()).unwrap());
        assert_eq!(pc.now(), before);
        assert!(!b.path.exists() && !a.path.exists());
        assert!(!restore_saved(&a, pc.open()).unwrap());
        assert_eq!(pc.now(), before, "RDP stays off");
        // A record of its own (from a build before the PC's) is refused, not filled with B's.
        let own = record_in(&directory.join("own"));
        setup_in(&b, USER, pc.open()).unwrap();
        let error = format!("{:#}", setup_in(&own, USER, pc.open()).unwrap_err());
        assert!(error.contains("restore it there"), "{error}");
        assert!(!own.path.exists());
        assert!(restore_saved(&b, pc.open()).unwrap());
        assert_eq!(pc.now(), before);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_record_another_account_owns_is_never_used() {
        let directory = scratch();
        let trusted = record_in(&directory);
        trusted.save(&recorded()).unwrap();
        let foreign = Record {
            trusted: |_, path| bail!("{} is owned by S-1-5-21-9", path.display()),
            ..trusted.clone()
        };
        let error = format!("{:#}", foreign.load().unwrap_err());
        assert!(error.contains("S-1-5-21-9"), "{error}");
        // Restore and setup refuse it; the firewall check turns RDP off, as without a record.
        let pc = Pc::new();
        assert!(restore_saved(&foreign, pc.open()).is_err());
        assert!(setup_in(&foreign, USER, pc.open()).is_err());
        assert_eq!(pc.now(), Pc::new().now());
        let mut ports = Vec::new();
        let problem = secure_rdp(
            &foreign,
            |port, _| {
                ports.push(port);
                Ok(vec!["Remote Desktop was turned off (fDenyTSConnections = 1), so it is not open to the network".into()])
            },
            no_wait,
        )
        .unwrap();
        assert_eq!(ports, [None]);
        assert!(problem.contains("S-1-5-21-9"), "{problem}");
        // A folder someone else made first is not saved into.
        std::fs::remove_dir_all(&directory).unwrap();
        let squatted = record_in(&directory);
        std::fs::create_dir_all(squatted.folder()).unwrap();
        let squatted = Record {
            trusted: foreign.trusted,
            ..squatted
        };
        assert!(squatted.save(&recorded()).is_err());
        assert!(!squatted.path.exists());
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
        static RESTRICTED: std::cell::RefCell<Vec<PathBuf>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    #[test]
    fn the_record_is_saved_for_administrators_only() {
        let directory = scratch();
        let record = Record {
            restrict: |path| {
                RESTRICTED.with_borrow_mut(|done| done.push(path.into()));
                Ok(())
            },
            ..record_in(&directory)
        };
        record.save(&recorded()).unwrap();
        record.save(&recorded()).unwrap();
        // The folder once, when it is made; every file before it takes the record's name.
        let temporary = record.path.with_extension("tmp");
        assert_eq!(
            RESTRICTED.take(),
            [record.folder().to_path_buf(), temporary.clone(), temporary]
        );
        assert_eq!(record.load().unwrap(), Some(recorded()));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_profiles_own_record_moves_to_the_pcs_once() {
        let directory = scratch();
        let record = record_in(&directory);
        let old = |port| SetupState {
            rdp_port: port,
            ..recorded()
        };
        std::fs::write(&record.legacy, serde_json::to_vec(&old(3391)).unwrap()).unwrap();
        // Read where it is until the settings locks are held.
        assert!(record.may_exist());
        assert_eq!(record.load().unwrap(), Some(old(3391)));
        assert!(record.legacy.is_file() && !record.path.exists());
        record.take_over().unwrap();
        assert!(!record.legacy.exists());
        assert_eq!(record.load().unwrap(), Some(old(3391)));
        // The PC's record is never overwritten by another profile's from before.
        std::fs::write(&record.legacy, serde_json::to_vec(&old(3392)).unwrap()).unwrap();
        record.take_over().unwrap();
        assert!(!record.legacy.exists());
        assert_eq!(record.load().unwrap(), Some(old(3391)));
        // Restore forgets both.
        std::fs::write(&record.legacy, serde_json::to_vec(&old(3392)).unwrap()).unwrap();
        record.remove().unwrap();
        assert!(!record.may_exist());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn the_record_is_the_pcs_not_the_profiles() {
        let directory = scratch();
        let record = Record::new(&directory);
        assert_eq!(record.path, logic::setup_record());
        assert_eq!(record.legacy, directory.join(logic::SETUP_FILE));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn the_child_profile_keeps_its_own_settings_under_the_child_host_ones() {
        let directory = scratch();
        let profile = directory.join(CHILD_PROFILE);
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(profile.join("sunshine.conf"), "port = 47989\nlocale = de\n").unwrap();
        let main = Credentials::new("admin".into(), "main-console").unwrap();
        write_child_profile(&profile, "Den", 47989, Some(&main)).unwrap();
        let written = Config::load(&profile.join("sunshine.conf")).unwrap();
        assert_eq!(written.get("locale", ""), "de");
        assert_eq!(written.get("port", ""), logic::CHILD_BASE_PORT.to_string());
        assert_eq!(written.get("sunshine_name", ""), "Den (background)");
        assert!(written.boolean(logic::CHILD_HOST_KEY, false));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn status_reads_the_firewall_rules_once_in_30_seconds_until_setup_or_restore() {
        let cache = RulesCache::new();
        let reads = std::cell::Cell::new(0);
        let rules = |id: &str| logic::FirewallRules {
            enabled: vec![id.to_owned()],
            active: logic::ActiveFirewall::default(),
        };
        let read = |id: &'static str| {
            let reads = &reads;
            move || {
                reads.set(reads.get() + 1);
                Ok(rules(id))
            }
        };
        let start = Instant::now();
        assert_eq!(cache.get(start, read("first")).unwrap(), rules("first"));
        let soon = start + RULES_FOR - std::time::Duration::from_millis(1);
        assert_eq!(cache.get(soon, read("second")).unwrap(), rules("first"));
        assert_eq!(reads.get(), 1);
        // Read again once they are 30 s old.
        assert_eq!(
            cache.get(start + RULES_FOR, read("third")).unwrap(),
            rules("third")
        );
        assert_eq!(reads.get(), 2);
        // Setup and restore forget them.
        cache.forget();
        assert_eq!(
            cache.get(start + RULES_FOR, read("fourth")).unwrap(),
            rules("fourth")
        );
        assert_eq!(reads.get(), 3);
        // A read that fails is said, and not kept: the next status reads again.
        let later = start + RULES_FOR * 3;
        let failed = cache.get(later, || bail!("powershell exited with code 1"));
        assert!(failed.is_err());
        assert_eq!(cache.get(later, read("fifth")).unwrap(), rules("fifth"));
        assert!(RULES_FOR <= std::time::Duration::from_secs(30));
    }

    #[test]
    fn the_firewall_is_watched_while_child_sessions_are_set_up() {
        let mut last = None;
        let mut reports = Vec::new();
        // Not set up: no check.
        let empty = scratch();
        let never = |_, _| -> Result<Vec<String>> { panic!("no setup, no check") };
        watch(&record_in(&empty), &mut last, true, never, no_wait, |p| {
            reports.push(p)
        });
        std::fs::remove_dir_all(empty).unwrap();
        // Set up: every round checks, and a problem is reported once while it stays.
        let (directory, record) = set_up_record();
        let rounds = [false, true, true, false, true];
        let mut checks = 0;
        for open in rounds {
            watch(
                &record,
                &mut last,
                true,
                |port, _| {
                    assert_eq!(port, Some(3390));
                    checks += 1;
                    Ok(if open {
                        vec!["the block rule is not in effect".into()]
                    } else {
                        Vec::new()
                    })
                },
                no_wait,
                |problem| reports.push(problem),
            );
        }
        assert_eq!(checks, rounds.len());
        assert_eq!(reports.len(), 2, "{reports:?}");
        assert!(reports[0].contains("the block rule is not in effect"));
        assert!(WATCH_EVERY <= std::time::Duration::from_secs(60));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_host_without_administrator_rights_says_child_sessions_are_unavailable_in_it() {
        let never = |_, _| -> Result<Vec<String>> { panic!("an unelevated host checks nothing") };
        let mut reports = Vec::new();
        // Not set up: nothing to say.
        let empty = scratch();
        let mut last = None;
        assert_eq!(unverified(&record_in(&empty), false), None);
        watch(&record_in(&empty), &mut last, false, never, no_wait, |p| {
            reports.push(p)
        });
        assert_eq!(last, None);
        std::fs::remove_dir_all(empty).unwrap();
        // Set up and readable: unavailable here, said in the log, never reported as a problem.
        let (directory, record) = set_up_record();
        let reason = unverified(&record, false).unwrap();
        assert!(
            reason.starts_with("child sessions are set up on this PC"),
            "{reason}"
        );
        assert!(reason.contains("unavailable in this host"), "{reason}");
        for _ in 0..2 {
            watch(&record, &mut last, false, never, no_wait, |p| {
                reports.push(p)
            });
        }
        assert_eq!(last.as_deref(), Some(reason.as_str()));
        // A record it can't read (an older one only administrators may read): the same, with why.
        let unreadable = Record {
            trusted: |_, path| bail!("Access is denied reading {}", path.display()),
            ..record.clone()
        };
        let reason = unverified(&unreadable, false).unwrap();
        assert!(
            reason.starts_with(
                "Rubylight's child-session record could not be read (Access is denied"
            ),
            "{reason}"
        );
        assert!(reason.contains("unavailable in this host"), "{reason}");
        watch(&unreadable, &mut last, false, never, no_wait, |p| {
            reports.push(p)
        });
        assert!(reports.is_empty(), "{reports:?}");
        // Elevated: it checks, as before.
        assert_eq!(unverified(&unreadable, true), None);
        let mut checks = 0;
        watch(
            &record,
            &mut last,
            true,
            |_, _| {
                checks += 1;
                Ok(Vec::new())
            },
            no_wait,
            |p| reports.push(p),
        );
        assert_eq!((checks, last), (1, None));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_set_up_pc_checks_its_firewall_at_the_hosts_start() {
        let directory = scratch();
        let record = record_in(&directory);
        let never = |_, _| -> Result<Vec<String>> { panic!("no setup, no check") };
        assert_eq!(secure_rdp(&record, never, no_wait), None);
        let state = SetupState {
            version: 1,
            user_sid: "S-1-5-21-1".into(),
            deny_ts_connections: Some(1),
            dwm_frame_interval: None,
            suppress_when_minimized: None,
            child_sessions: false,
            remote_desktop_rule_ids: Vec::new(),
            rdp_port: 3390,
        };
        record.save(&state).unwrap();
        // Checked for the port setup recorded.
        assert_eq!(
            secure_rdp(
                &record,
                |port, _| {
                    assert_eq!(port, Some(3390));
                    Ok(Vec::new())
                },
                no_wait
            ),
            None
        );
        let problem = secure_rdp(
            &record,
            |_, _| {
                Ok(vec![
                    "the block rule is not in effect".into(),
                    "Remote Desktop was turned off (fDenyTSConnections = 1), so it is not open to the network".into(),
                ])
            },
            no_wait,
        )
        .unwrap();
        assert!(
            problem.contains("the block rule is not in effect; Remote Desktop was turned off")
                && problem.contains("set up child sessions again")
                && !problem.contains("so Remote Desktop is on"),
            "{problem}"
        );
        // A record that cannot be read still turns RDP off, and says why.
        write_record(&record, "not json");
        let problem = secure_rdp(
            &record,
            |port, _| {
                assert_eq!(port, None);
                Ok(vec!["Remote Desktop was turned off (fDenyTSConnections = 1), so it is not open to the network".into()])
            },
            no_wait,
        )
        .unwrap();
        assert!(
            problem.contains(logic::SETUP_FILE)
                && problem.contains("Remote Desktop was turned off"),
            "{problem}"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    /// The registry alone, as [`enforce_with`] falls back to it.
    #[derive(Default)]
    struct Values {
        values: std::collections::BTreeMap<(String, String), u32>,
        fail_writes: bool,
    }
    impl Registry for Values {
        fn read_dword(&self, hive: &logic::Hive, path: &str, name: &str) -> Result<Option<u32>> {
            assert_eq!(*hive, logic::Hive::LocalMachine);
            Ok(self.values.get(&(path.into(), name.into())).copied())
        }
        fn write_dword(
            &mut self,
            hive: &logic::Hive,
            path: &str,
            name: &str,
            value: u32,
        ) -> Result<()> {
            assert_eq!(*hive, logic::Hive::LocalMachine);
            if self.fail_writes {
                bail!("access is denied");
            }
            self.values.insert((path.into(), name.into()), value);
            Ok(())
        }
    }
    fn no_wait(_: std::time::Duration) {}
    fn deny() -> (String, String) {
        (logic::TERMINAL_SERVER.into(), "fDenyTSConnections".into())
    }

    #[test]
    fn a_firewall_that_cannot_be_opened_still_turns_rdp_off() {
        let unopened = || -> Result<native::NativeSettings> { bail!("CoCreateInstance failed") };
        let mut registry = Values::default();
        registry.values.insert(deny(), 0);
        // Not the last try: nothing changes, so it can be tried again.
        let error = enforce_with(Some(3390), false, unopened, &mut registry).unwrap_err();
        assert!(
            format!("{error:#}").contains("CoCreateInstance failed"),
            "{error:#}"
        );
        assert_eq!(registry.values.get(&deny()), Some(&0));
        let found = enforce_with(Some(3390), true, unopened, &mut registry).unwrap();
        assert!(
            found[0].contains("Windows Firewall could not be opened")
                && found[0].contains("CoCreateInstance failed"),
            "{found:?}"
        );
        assert!(
            found[1].contains("Remote Desktop was turned off"),
            "{found:?}"
        );
        assert_eq!(registry.values.get(&deny()), Some(&1));
        // A write that fails says so: RDP may still be on.
        let mut stuck = Values {
            fail_writes: true,
            ..Values::default()
        };
        let found = enforce_with(Some(3390), true, unopened, &mut stuck).unwrap();
        assert!(
            found[1].contains("turning Remote Desktop off failed")
                && found[1].contains("access is denied"),
            "{found:?}"
        );
        // Without a record, RDP goes off without the firewall.
        let mut registry = Values::default();
        let unneeded = || -> Result<native::NativeSettings> { panic!("the firewall was opened") };
        let found = enforce_with(None, false, unneeded, &mut registry).unwrap();
        assert!(
            found.len() == 1 && found[0].contains("Remote Desktop was turned off"),
            "{found:?}"
        );
        assert_eq!(registry.values.get(&deny()), Some(&1));
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
        }
    }

    #[test]
    fn a_check_that_cannot_run_is_tried_again_then_fails_closed() {
        let (directory, record) = set_up_record();
        // Fails twice, then runs: no problem, after two waits without the settings lock.
        let mut tries = Vec::new();
        let mut waits = 0;
        let problem = secure_rdp(
            &record,
            |port, last| {
                tries.push((port, last));
                match tries.len() {
                    1 | 2 => bail!("the firewall rule script timed out"),
                    _ => Ok(Vec::new()),
                }
            },
            |wait| {
                assert_eq!(wait, CHECK_WAIT);
                // Another thread gets the lock (other tests may hold it a moment).
                let (locked, lock) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let _settings = settings_lock().unwrap();
                    locked.send(()).unwrap();
                });
                assert!(
                    lock.recv_timeout(std::time::Duration::from_secs(10))
                        .is_ok(),
                    "waits holding the settings lock"
                );
                waits += 1;
            },
        );
        assert_eq!(problem, None);
        assert_eq!(
            tries,
            [(Some(3390), false), (Some(3390), false), (Some(3390), true)]
        );
        assert_eq!(waits, 2);
        // Never runs: the last try is told so and turns RDP off.
        let mut tries = 0;
        let problem = secure_rdp(
            &record,
            |_, last| {
                tries += 1;
                if !last {
                    bail!("the firewall rule script timed out");
                }
                Ok(vec!["Remote Desktop was turned off (fDenyTSConnections = 1), so it is not open to the network".into()])
            },
            no_wait,
        )
        .unwrap();
        assert_eq!(tries, CHECK_TRIES);
        assert!(
            problem.contains("Remote Desktop was turned off"),
            "{problem}"
        );
        // What a check finds is never tried again.
        let mut tries = 0;
        let problem = secure_rdp(
            &record,
            |_, _| {
                tries += 1;
                Ok(vec![
                    "the block rule is not in effect".into(),
                    "Remote Desktop was turned off (fDenyTSConnections = 1), so it is not open to the network".into(),
                ])
            },
            |_| panic!("a real problem was tried again"),
        )
        .unwrap();
        assert_eq!(tries, 1);
        assert!(
            problem.contains("the block rule is not in effect"),
            "{problem}"
        );
        std::fs::remove_dir_all(directory).unwrap();
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
    fn setup_and_restore_wait_for_the_settings_lock() {
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
            // The console's restore.
            let config = record.clone();
            let restored = waits_for_the_settings_lock(hold, move |reached| {
                restore_locked(&config, |_| {
                    reached.send(()).unwrap();
                    Ok(false)
                })
            });
            assert!(!restored.unwrap());
            // Setup opens Windows' settings only under the lock.
            let config = record.clone();
            let error = waits_for_the_settings_lock(hold, move |reached| {
                setup_in(
                    &config,
                    "S-1-5-21-1",
                    || -> Result<native::NativeSettings> {
                        reached.send(()).unwrap();
                        bail!("not on this PC")
                    },
                )
            })
            .unwrap_err();
            assert!(format!("{error:#}").contains("not on this PC"), "{error:#}");
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn the_startup_check_waits_for_setup_and_restore() {
        let (directory, record) = set_up_record();
        for hold in HOLDERS {
            let (checked, checks) = std::sync::mpsc::channel();
            let config = record.clone();
            let problem = waits_for_the_settings_lock(hold, move |reached| {
                secure_rdp(
                    &config,
                    |port, _| {
                        checked.send(port).unwrap();
                        reached.send(()).unwrap();
                        Ok(Vec::new())
                    },
                    no_wait,
                )
            });
            assert_eq!(problem, None);
            assert_eq!(checks.try_recv().unwrap(), Some(3390));
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
    fn a_check_that_cannot_get_the_lock_fails_closed() {
        let (directory, record) = set_up_record();
        // Another process holds the cross-process lock for longer than the check waits.
        let lock = native::lock::setup_lock(settings_wait()).unwrap();
        let problem = std::thread::spawn({
            let record = record.clone();
            move || {
                SETTINGS_WAIT.set(Some(std::time::Duration::from_millis(300)));
                let mut tries = Vec::new();
                let problem = secure_rdp(
                    &record,
                    |port, last| {
                        tries.push((port, last));
                        Ok(vec!["Remote Desktop was turned off (fDenyTSConnections = 1), so it is not open to the network".into()])
                    },
                    |_| panic!("waited to try again"),
                );
                // Turned off without the firewall, as without a record.
                assert_eq!(tries, [(None, true)]);
                problem
            }
        })
        .join()
        .unwrap()
        .unwrap();
        drop(lock);
        assert!(
            problem.contains("the firewall could not be checked")
                && problem.contains("still setting up or restoring")
                && problem.contains("Remote Desktop was turned off"),
            "{problem}"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_viewer_line_over_the_cap_is_not_read_or_logged() {
        let directory = scratch();
        let path = directory.join("viewer.log");
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .with_ansi(false)
            .with_writer(Mutex::new(File::create(&path).unwrap()))
            .finish();
        let long = format!("{}SECRET\n", "x".repeat(logic::STATUS_LINE_CAP));
        let input = format!("first\n{long}after\n");
        tracing::subscriber::with_default(subscriber, || read_errors(7, input.as_bytes()));
        let log = std::fs::read_to_string(&path).unwrap();
        assert!(log.contains("first") && log.contains("after"), "{log}");
        assert!(log.contains("a line that is not logged"), "{log}");
        assert!(!log.contains("SECRET") && !log.contains("xxxx"), "{log}");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn the_viewer_connects_to_the_rdp_port_setup_recorded() {
        let directory = scratch();
        let record = record_in(&directory);
        let port = |record: &Record| match start_command(record, Default::default(), None) {
            Ok(ViewerCommand::Start { rdp_port, .. }) => Ok(rdp_port),
            Ok(other) => panic!("{other:?}"),
            Err(error) => Err(format!("{error:#}")),
        };
        let error = port(&record).unwrap_err();
        assert!(error.contains("set up child sessions first"), "{error}");
        let mut state = SetupState {
            version: 1,
            user_sid: "S-1-5-21-1".into(),
            deny_ts_connections: Some(1),
            dwm_frame_interval: None,
            suppress_when_minimized: None,
            child_sessions: false,
            remote_desktop_rule_ids: Vec::new(),
            rdp_port: 3390,
        };
        record.save(&state).unwrap();
        assert_eq!(port(&record), Ok(3390));
        // A record saved without a port: Windows' default.
        let mut text = serde_json::to_value(&state).unwrap();
        text.as_object_mut().unwrap().remove("rdp_port");
        write_record(&record, &text.to_string());
        assert_eq!(port(&record), Ok(3389));
        state.rdp_port = 3389;
        assert_eq!(record.load().unwrap(), Some(state));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn the_child_host_signs_in_like_the_main_console_and_never_starts_unclaimed() {
        let directory = scratch();
        let profile = directory.join(CHILD_PROFILE);
        let error = write_child_profile(&profile, "Den", 47989, None).unwrap_err();
        assert!(format!("{error:#}").contains("sign-in first"), "{error:#}");
        assert!(
            !profile.exists(),
            "nothing is written for a host that cannot start"
        );
        // A paired device in the file that also holds the sign-in stays.
        std::fs::create_dir_all(&profile).unwrap();
        let state = profile.join("sunshine_state.json");
        std::fs::write(&state, r#"{"root":{"named_devices":[1]},"username":"old"}"#).unwrap();
        let main = Credentials::new("admin".into(), "main-console").unwrap();
        write_child_profile(&profile, "Den", 47989, Some(&main)).unwrap();
        let config = Config::load(&profile.join("sunshine.conf")).unwrap();
        let files = ProfileFiles::new(&config, &profile);
        assert_eq!(files.credentials, state);
        let loaded = Credentials::load(&files.credentials).unwrap().unwrap();
        assert!(loaded == main);
        assert!(loaded.verifies("admin", "main-console"));
        assert_eq!(
            load_json(&state, json!({})).unwrap()["root"]["named_devices"][0],
            1
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
}
