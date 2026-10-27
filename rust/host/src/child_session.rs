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
};

/// The child-session host's own profile, inside this host's.
const CHILD_PROFILE: &str = "child-session";

pub(crate) fn setup_path(directory: &Path) -> PathBuf {
    directory.join(logic::SETUP_FILE)
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

/// At the host's start, when child sessions are set up here: RDP stays on until
/// restore, so a firewall that no longer keeps it on this PC turns RDP off and
/// is said loudly (and [`start`] refuses until setup runs again).
pub(crate) fn check_at_startup(h: &Shared) {
    let directory = h.directory.clone();
    std::thread::spawn(move || {
        if let Some(problem) = secure_rdp(&directory, native_enforce, std::thread::sleep) {
            report(problem);
        }
    });
}

/// Setup, restore and the firewall checks change the same Windows settings,
/// one at a time: a check that turned RDP off between restore's write of the
/// original value and its read-back would lose the user's own value.
static SETTINGS: Mutex<()> = Mutex::new(());
fn settings_lock() -> MutexGuard<'static, ()> {
    SETTINGS.lock().unwrap_or_else(PoisonError::into_inner)
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
/// record that cannot be read turns RDP off too (`enforce(None, _)`). The
/// problem for the log and the console, None when there is none or setup
/// never ran.
fn secure_rdp(
    directory: &Path,
    mut enforce: impl FnMut(Option<u16>, bool) -> Result<Vec<String>>,
    mut wait: impl FnMut(std::time::Duration),
) -> Option<String> {
    let mut found = Vec::new();
    for attempt in 1..=CHECK_TRIES {
        let last = attempt == CHECK_TRIES;
        let settings = settings_lock();
        // Only the try that ends this reports.
        found.clear();
        let port = match load_saved(directory) {
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
/// or setup never ran here, 1 with the reason on stderr.
pub(crate) fn restore_exit_code(config_dir: Option<PathBuf>) -> i32 {
    restore_exit_code_with(config_dir, restore_saved)
}
fn restore_exit_code_with(
    config_dir: Option<PathBuf>,
    restore: impl FnOnce(&Path) -> Result<bool>,
) -> i32 {
    let directory = config_dir.unwrap_or_else(butterpollo_core::paths::installed_profile);
    match restore_locked(&directory, restore) {
        Ok(true) => {
            println!("Restored the Windows settings child-session setup changed.");
            0
        }
        Ok(false) => {
            println!("Child sessions were not set up by Rubylight here; nothing to restore.");
            0
        }
        Err(error) => {
            eprintln!(
                "The Windows settings child-session setup changed were not restored: {error:#}"
            );
            1
        }
    }
}

fn load_saved(directory: &Path) -> Result<Option<SetupState>> {
    let path = setup_path(directory);
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .with_context(|| format!("reading {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}
fn save_state(directory: &Path, state: &SetupState) -> Result<()> {
    let path = setup_path(directory);
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, serde_json::to_vec_pretty(state)?)
        .with_context(|| format!("writing {}", temporary.display()))?;
    std::fs::rename(&temporary, &path).with_context(|| format!("saving {}", path.display()))
}
/// `restore` ([`restore_saved`]) under the settings lock.
fn restore_locked(directory: &Path, restore: impl FnOnce(&Path) -> Result<bool>) -> Result<bool> {
    let _settings = settings_lock();
    restore(directory)
}
/// Put back what setup changed, then forget it; false when setup never ran.
/// The record stays when anything was not restored, so a retry has it.
fn restore_saved(directory: &Path) -> Result<bool> {
    let Some(state) = load_saved(directory)? else {
        return Ok(false);
    };
    logic::restore(&mut native::NativeSettings::new()?, &state)?;
    let path = setup_path(directory);
    std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
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

/// Set up child sessions for the account signed in at this PC's screen.
pub(crate) fn setup(h: &Shared) -> Result<SetupState> {
    let user = native::console_user()?;
    let state = setup_in(&h.directory, &user, native::NativeSettings::new)?;
    tracing::info!("child sessions are set up; restore puts the Windows settings back");
    Ok(state)
}
/// [`logic::setup`] for `user` through the settings `open` makes, under the
/// settings lock.
fn setup_in<S: SystemSettings>(
    directory: &Path,
    user: &str,
    open: impl FnOnce() -> Result<S>,
) -> Result<SetupState> {
    let _settings = settings_lock();
    let saved = load_saved(directory)?;
    logic::setup(&mut open()?, saved, user, |state| {
        save_state(directory, state)
    })
}

/// Stop the viewer and the child session's host, then put back what setup
/// changed. False when setup never ran here.
pub(crate) fn restore(h: &Shared) -> Result<bool> {
    quit();
    let restored = restore_locked(&h.directory, restore_saved)?;
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
    if let Some(problem) = secure_rdp(&h.directory, native_enforce, std::thread::sleep) {
        report(problem.clone());
        bail!("{problem}");
    }
    let system = native::NativeSettings::new()?;
    let user = native::console_user()?;
    let missing = match load_saved(&h.directory)? {
        Some(_) => logic::settings_missing(&system, &user)?,
        None => logic::missing(&system, &user, None)?,
    };
    if !missing.is_empty() {
        bail!("set up child sessions first: {}", missing.join("; "));
    }
    let command = start_command(&h.directory, display, password)?;
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
    directory: &Path,
    display: DisplaySettings,
    password: Option<logic::Secret>,
) -> Result<ViewerCommand> {
    let state = load_saved(directory)?
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

/// What the console shows.
pub(crate) fn status(h: &Shared) -> Value {
    let config = h
        .config
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let decision = decision(&config);
    let missing = (|| {
        let rdp_port = load_saved(&h.directory)?.map(|state| state.rdp_port);
        logic::missing(
            &native::NativeSettings::new()?,
            &native::console_user()?,
            rdp_port,
        )
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
        "set_up": setup_path(&h.directory).is_file(),
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

    #[test]
    fn restore_without_a_setup_record_changes_nothing() {
        let directory = scratch();
        assert!(!restore_saved(&directory).unwrap());
        std::fs::write(setup_path(&directory), "not json").unwrap();
        let error = restore_saved(&directory).unwrap_err();
        assert!(
            format!("{error:#}").contains(logic::SETUP_FILE),
            "{error:#}"
        );
        assert!(setup_path(&directory).is_file());
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
    fn a_set_up_pc_checks_its_firewall_at_the_hosts_start() {
        let directory = scratch();
        let never = |_, _| -> Result<Vec<String>> { panic!("no setup, no check") };
        assert_eq!(secure_rdp(&directory, never, no_wait), None);
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
        save_state(&directory, &state).unwrap();
        // Checked for the port setup recorded.
        assert_eq!(
            secure_rdp(
                &directory,
                |port, _| {
                    assert_eq!(port, Some(3390));
                    Ok(Vec::new())
                },
                no_wait
            ),
            None
        );
        let problem = secure_rdp(
            &directory,
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
        std::fs::write(setup_path(&directory), "not json").unwrap();
        let problem = secure_rdp(
            &directory,
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

    fn set_up_record() -> PathBuf {
        let directory = scratch();
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
        save_state(&directory, &state).unwrap();
        directory
    }

    #[test]
    fn a_check_that_cannot_run_is_tried_again_then_fails_closed() {
        let directory = set_up_record();
        // Fails twice, then runs: no problem, after two waits without the settings lock.
        let mut tries = Vec::new();
        let mut waits = 0;
        let problem = secure_rdp(
            &directory,
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
                    let _settings = settings_lock();
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
            &directory,
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
            &directory,
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

    /// `run`, on another thread, signals only after this test lets go of the
    /// settings lock.
    fn waits_for_the_settings_lock<T: Send + 'static>(
        run: impl FnOnce(std::sync::mpsc::Sender<()>) -> T + Send + 'static,
    ) -> T {
        let settings = settings_lock();
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
        // The uninstaller's restore.
        let config = directory.clone();
        let code = waits_for_the_settings_lock(move |reached| {
            restore_exit_code_with(Some(config), |_| {
                reached.send(()).unwrap();
                Ok(true)
            })
        });
        assert_eq!(code, 0);
        // The console's restore.
        let config = directory.clone();
        let restored = waits_for_the_settings_lock(move |reached| {
            restore_locked(&config, |_| {
                reached.send(()).unwrap();
                Ok(false)
            })
        });
        assert!(!restored.unwrap());
        // Setup opens Windows' settings only under the lock.
        let config = directory.clone();
        let error = waits_for_the_settings_lock(move |reached| {
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
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn the_startup_check_waits_for_setup_and_restore() {
        let directory = scratch();
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
        save_state(&directory, &state).unwrap();
        let settings = settings_lock();
        let (checked, checks) = std::sync::mpsc::channel();
        let check = {
            let directory = directory.clone();
            std::thread::spawn(move || {
                secure_rdp(
                    &directory,
                    |port, _| {
                        checked.send(port).unwrap();
                        Ok(Vec::new())
                    },
                    no_wait,
                )
            })
        };
        assert!(
            checks
                .recv_timeout(std::time::Duration::from_millis(200))
                .is_err()
        );
        drop(settings);
        assert_eq!(
            checks
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap(),
            Some(3390)
        );
        assert_eq!(check.join().unwrap(), None);
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
        let port = |directory: &Path| match start_command(directory, Default::default(), None) {
            Ok(ViewerCommand::Start { rdp_port, .. }) => Ok(rdp_port),
            Ok(other) => panic!("{other:?}"),
            Err(error) => Err(format!("{error:#}")),
        };
        let error = port(&directory).unwrap_err();
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
        save_state(&directory, &state).unwrap();
        assert_eq!(port(&directory), Ok(3390));
        // A record saved without a port: Windows' default.
        let mut record = serde_json::to_value(&state).unwrap();
        record.as_object_mut().unwrap().remove("rdp_port");
        std::fs::write(setup_path(&directory), record.to_string()).unwrap();
        assert_eq!(port(&directory), Ok(3389));
        state.rdp_port = 3389;
        assert_eq!(load_saved(&directory).unwrap(), Some(state));
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
