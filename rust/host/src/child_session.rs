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
        self as logic, DisplaySettings, Mode, ModeDecision, ModeSetting, Phase, SetupState,
        ViewerCommand, ViewerStatus,
    },
    config::Config,
};
use butterpollo_windows::{
    child_session as native,
    process::{PipedChild, Process, Target},
};
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    fs::File,
    io::{BufRead, BufReader, Write},
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

/// `butterpollo.exe --child-session-restore`, which the uninstaller runs: put
/// back the Windows settings child-session setup changed. 0 when they are back
/// or setup never ran here, 1 with the reason on stderr.
pub(crate) fn restore_exit_code(config_dir: Option<PathBuf>) -> i32 {
    let directory = config_dir.unwrap_or_else(butterpollo_core::paths::installed_profile);
    match restore_saved(&directory) {
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
    let saved = load_saved(&h.directory)?;
    let state = logic::setup(&mut native::NativeSettings::new()?, saved, &user, |state| {
        save_state(&h.directory, state)
    })?;
    tracing::info!("child sessions are set up; restore puts the Windows settings back");
    Ok(state)
}

/// Stop the viewer and the child session's host, then put back what setup
/// changed. False when setup never ran here.
pub(crate) fn restore(h: &Shared) -> Result<bool> {
    quit();
    let restored = restore_saved(&h.directory)?;
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
    let missing = logic::missing(&native::NativeSettings::new()?, &native::console_user()?)?;
    if !missing.is_empty() {
        bail!("set up child sessions first: {}", missing.join("; "));
    }
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
    send_to(running, &ViewerCommand::Start { display, password })
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
        std::thread::spawn(move || {
            for line in BufReader::new(errors).lines().map_while(Result::ok) {
                tracing::warn!(viewer = pid, "{line}");
            }
        });
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

fn read_status(h: &Shared, pid: u32, output: File) {
    for line in BufReader::new(output).lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                tracing::warn!(%error, "the child-session viewer's status pipe failed");
                break;
            }
        };
        let status: ViewerStatus = match serde_json::from_str(&line) {
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
    match start_host(h, status.child) {
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
/// main host, only after the session is checked to be the console user's own.
fn start_host(h: &Shared, child: u32) -> Result<Process> {
    native::verify_child_owner(child)?;
    let (name, port) = {
        let config = h.config.read().unwrap_or_else(PoisonError::into_inner);
        (config.get("sunshine_name", "").to_owned(), config.port()?)
    };
    let profile = h.directory.join(CHILD_PROFILE);
    write_child_profile(&profile, &name, port)?;
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

/// The child host's settings over whatever its profile already holds.
fn write_child_profile(profile: &Path, name: &str, port: u16) -> Result<()> {
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
    std::fs::write(&file, config.text()).with_context(|| format!("writing {}", file.display()))
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
    let missing = (|| logic::missing(&native::NativeSettings::new()?, &native::console_user()?))();
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
        write_child_profile(&profile, "Den", 47989).unwrap();
        let written = Config::load(&profile.join("sunshine.conf")).unwrap();
        assert_eq!(written.get("locale", ""), "de");
        assert_eq!(written.get("port", ""), logic::CHILD_BASE_PORT.to_string());
        assert_eq!(written.get("sunshine_name", ""), "Den (background)");
        assert!(written.boolean(logic::CHILD_HOST_KEY, false));
        std::fs::remove_dir_all(directory).unwrap();
    }
}
