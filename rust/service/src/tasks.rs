//! The service's duties whose code changes with the host, run as short
//! `butterpollo.exe` tasks from the installation (rust/host/src/service_tasks.rs):
//! as this service, SYSTEM in its own session, without a window. A task logs
//! to its standard error, which lands here in service.log at the level it
//! names; its exit code says how it went:
//!
//! - `--restore-nucleus-at-console --config-dir PROFILE`: put back what a
//!   Nucleus Co-op session left for the user at the console. 0: nothing
//!   failed (also when nothing was due, or it waits for the user); 1: a part
//!   failed. Run as the user signs in at the console while a host runs.
//! - `--virtual-display-installed`: 0: the host can use the virtual display
//!   driver (or it is switched off); 1: it cannot, and standard output says
//!   why. Run once the device is there and before a repair, never twice
//!   within a minute, as the driver watch says.
//!
//! Any other exit code, a task that cannot start or one that outlives its
//! time is an error here. No task runs on a timer. Each task runs in a job of
//! its own ([`crate::process::Job`]): what it leaves running ends with it, and
//! all of it ends as the service stops, so no task keeps the installation's
//! files open for the updater.
use crate::process::{Job, Signal};
use anyhow::{Context, Result, bail};
use std::{
    ffi::OsString,
    io::Read,
    os::windows::io::AsRawHandle,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// How a task ended: its exit code and standard output.
#[derive(Debug, PartialEq, Eq)]
pub struct Outcome {
    pub code: u32,
    pub output: String,
}

/// How a task's wait ended.
enum Ended {
    Exited,
    TimedOut,
    Stopped,
}

/// Run `butterpollo.exe` (`host`) with `args`, ended after `timeout` or as
/// `stop` is set (the service stops).
fn run(host: &Path, args: &[OsString], timeout: Duration, stop: &Signal) -> Result<Outcome> {
    use std::os::windows::process::CommandExt;
    use windows::Win32::{Foundation::*, System::Threading::*};
    let task = args
        .first()
        .map(|arg| arg.to_string_lossy().trim_start_matches('-').to_owned())
        .unwrap_or_default();
    let job = Job::new()?;
    // Suspended until it is in its job, so that nothing it starts escapes.
    let mut child = Command::new(host)
        .args(args)
        .current_dir(host.parent().context("the host has no folder")?)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags((CREATE_NO_WINDOW | CREATE_SUSPENDED).0)
        .spawn()
        .with_context(|| format!("starting {} --{task}", host.display()))?;
    if let Err(error) = job.adopt(&child) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error.context(format!("starting --{task} in its job")));
    }
    let (sender, received) = std::sync::mpsc::channel();
    let reader = |stream: usize, pipe: Option<Box<dyn Read + Send>>| {
        let sender = sender.clone();
        std::thread::spawn(move || {
            let mut text = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut text);
            }
            let _ = sender.send((stream, String::from_utf8_lossy(&text).into_owned()));
        });
    };
    reader(0, child.stdout.take().map(|p| Box::new(p) as _));
    reader(1, child.stderr.take().map(|p| Box::new(p) as _));
    drop(sender);
    let handles = [HANDLE(child.as_raw_handle()), stop.raw()];
    // SAFETY: the child and the stop event own these handles throughout the wait.
    let ended =
        match unsafe { WaitForMultipleObjects(&handles, false, crate::process::millis(timeout)) } {
            WAIT_OBJECT_0 => Ended::Exited,
            WAIT_TIMEOUT => Ended::TimedOut,
            event if event.0 == WAIT_OBJECT_0.0 + 1 => Ended::Stopped,
            _ => Err(windows::core::Error::from_thread()).context("waiting for a task")?,
        };
    // What the task left running ends with it, and the pipes it held close:
    // the readers below see the end of both.
    if let Err(error) = job.end() {
        tracing::warn!(task, %error, "what the task started could not be ended");
        // The task itself at least, so that the wait below returns.
        let _ = child.kill();
    }
    let status = child.wait()?;
    let mut texts = [String::new(), String::new()];
    let until = Instant::now() + Duration::from_secs(5);
    for _ in 0..texts.len() {
        match received.recv_timeout(until.saturating_duration_since(Instant::now())) {
            Ok((stream, text)) => texts[stream] = text,
            // Only a program outside the job could still hold a pipe.
            Err(_) => {
                tracing::warn!(
                    task,
                    "a program outside the task's job keeps its output open; the rest is not read"
                );
                break;
            }
        }
    }
    let [output, errors] = texts;
    for line in errors.lines() {
        relay(&task, line);
    }
    match ended {
        Ended::Exited => {}
        Ended::TimedOut => bail!("--{task} did not finish within {} s", timeout.as_secs()),
        Ended::Stopped => bail!("--{task} was ended as the service stops"),
    }
    Ok(Outcome {
        code: status.code().map_or(u32::MAX, |code| code as u32),
        output,
    })
}

/// A task's log line in service.log, at the level it starts with (the host
/// writes ` INFO message fields`, without a time).
fn relay(task: &str, line: &str) {
    match parse(line) {
        Some((tracing::Level::ERROR, message)) => tracing::error!(task, "{message}"),
        Some((tracing::Level::WARN, message)) => tracing::warn!(task, "{message}"),
        Some((tracing::Level::INFO, message)) => tracing::info!(task, "{message}"),
        Some((_, message)) => tracing::debug!(task, "{message}"),
        None if line.trim().is_empty() => {}
        // A panic or another line without a level.
        None => tracing::warn!(task, "{line}"),
    }
}
fn parse(line: &str) -> Option<(tracing::Level, &str)> {
    let (level, message) = line.trim_start().split_once(' ')?;
    let level = match level {
        "ERROR" => tracing::Level::ERROR,
        "WARN" => tracing::Level::WARN,
        "INFO" => tracing::Level::INFO,
        "DEBUG" => tracing::Level::DEBUG,
        "TRACE" => tracing::Level::TRACE,
        _ => return None,
    };
    Some((level, message.trim_start()))
}

/// Put back the compatibility settings and Nucleus Co-op settings a Nucleus
/// Co-op session left for the user at the console when its host stopped
/// mid-session, as the user signs in (a host does it itself as it starts). A
/// session of theirs that runs keeps its own. The task logs what it did and
/// what failed.
pub fn restore_nucleus(host: &Path, profile: &Path, stop: &Signal) {
    let args = [
        "--restore-nucleus-at-console".into(),
        "--config-dir".into(),
        profile.as_os_str().to_owned(),
    ];
    match run(host, &args, Duration::from_secs(120), stop) {
        Ok(Outcome { code: 0, .. }) => {}
        Ok(Outcome { code, .. }) => tracing::error!(
            code,
            "Nucleus Co-op: what an earlier session changed was not all put back"
        ),
        Err(error) => tracing::error!(
            error = %format!("{error:#}"),
            "Nucleus Co-op: what an earlier session changed was not put back"
        ),
    }
}

/// Whether the host can use the virtual display driver: its interface opens
/// and speaks the host's protocol, or its device is installed and switched
/// off, which the user may have done on purpose. Missing comes with the reason.
#[derive(Debug, PartialEq, Eq)]
pub enum Driver {
    Installed,
    Missing(String),
}
pub fn virtual_display(host: &Path, stop: &Signal) -> Result<Driver> {
    driver(run(
        host,
        &["--virtual-display-installed".into()],
        Duration::from_secs(60),
        stop,
    )?)
}
fn driver(outcome: Outcome) -> Result<Driver> {
    match outcome.code {
        0 => Ok(Driver::Installed),
        1 => Ok(Driver::Missing(outcome.output.trim().to_owned())),
        code => bail!("the virtual display driver check failed ({code})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn task_lines_keep_their_level() {
        assert_eq!(
            parse(" INFO Nucleus Co-op runs pid=42"),
            Some((tracing::Level::INFO, "Nucleus Co-op runs pid=42"))
        );
        assert_eq!(
            parse("ERROR Nucleus Co-op: the settings did not load error=denied"),
            Some((
                tracing::Level::ERROR,
                "Nucleus Co-op: the settings did not load error=denied"
            ))
        );
        assert_eq!(
            parse(" WARN Nucleus Co-op: settings kept path=\"x\""),
            Some((
                tracing::Level::WARN,
                "Nucleus Co-op: settings kept path=\"x\""
            ))
        );
        assert_eq!(parse("thread 'main' panicked at src/main.rs:1:1:"), None);
        assert_eq!(parse(""), None);
    }
    #[test]
    fn the_driver_check_says_missing_only_for_its_own_answer() {
        let outcome = |code, output: &str| Outcome {
            code,
            output: output.into(),
        };
        assert_eq!(driver(outcome(0, "")).unwrap(), Driver::Installed);
        assert_eq!(
            driver(outcome(1, "not installed\r\n")).unwrap(),
            Driver::Missing("not installed".into())
        );
        // A crash is not a missing driver: no repair starts from it.
        assert!(driver(outcome(0xC000_0409, "")).is_err());
        assert!(driver(outcome(2, "")).is_err());
    }
    const CMD: &str = r"C:\Windows\System32\cmd.exe";
    fn cmd(line: &str) -> Vec<OsString> {
        vec!["/d".into(), "/c".into(), line.into()]
    }
    #[test]
    fn a_task_reports_its_exit_code_and_output_and_is_ended_after_its_time() {
        let stop = Signal::new().unwrap();
        let outcome = run(
            Path::new(CMD),
            &cmd("echo missing& exit /b 1"),
            Duration::from_secs(30),
            &stop,
        )
        .unwrap();
        assert_eq!(outcome.code, 1);
        assert_eq!(outcome.output.trim(), "missing");
        let started = Instant::now();
        let error = run(
            Path::new(CMD),
            &cmd("ping -n 30 127.0.0.1 >nul"),
            Duration::from_millis(500),
            &stop,
        )
        .unwrap_err();
        assert!(error.to_string().contains("did not finish"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(20));
        assert!(
            run(
                Path::new(r"C:\missing\butterpollo.exe"),
                &[],
                Duration::from_secs(1),
                &stop
            )
            .is_err()
        );
    }
    #[test]
    fn what_a_task_leaves_running_ends_with_it_and_its_output_is_read() {
        // The task exits at once; the program it started would hold its
        // output open for 30 seconds.
        let stop = Signal::new().unwrap();
        let started = Instant::now();
        let outcome = run(
            Path::new(CMD),
            &cmd("start /b ping -n 30 127.0.0.1 & echo done"),
            Duration::from_secs(30),
            &stop,
        )
        .unwrap();
        assert_eq!(outcome.code, 0);
        assert!(outcome.output.contains("done"), "{:?}", outcome.output);
        // Not the 5 seconds a reader waits for a pipe someone keeps open.
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "{:?}",
            started.elapsed()
        );
    }
    #[test]
    fn a_task_ends_as_the_service_stops() {
        let stop = std::sync::Arc::new(Signal::new().unwrap());
        let setter = stop.clone();
        let started = Instant::now();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            setter.set().unwrap();
        });
        let error = run(
            Path::new(CMD),
            &cmd("ping -n 60 127.0.0.1 >nul"),
            Duration::from_secs(120),
            &stop,
        )
        .unwrap_err();
        assert!(error.to_string().contains("service stops"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(10));
    }
}
