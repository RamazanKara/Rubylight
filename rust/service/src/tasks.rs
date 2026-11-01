//! The service's duties whose code changes with the host, run as short
//! `butterpollo.exe` tasks from the installation (rust/host/src/service_tasks.rs):
//! as this service, SYSTEM in its own session, without a window. A task logs
//! to its standard error, which lands here in service.log at the level it
//! names; its exit code says how it went:
//!
//! - `--restore-nucleus-at-console --config-dir PROFILE`: put back what a
//!   Nucleus Co-op session left for the user at the console. 0: nothing
//!   failed (also when nothing was due, or it waits for the user); 1: a part
//!   failed.
//! - `--virtual-display-installed`: 0: the virtual display driver is there
//!   (or switched off); 1: it is missing, and standard output says why.
//!
//! Any other exit code, a task that cannot start or one that outlives its
//! time is an error here.
use anyhow::{Context, Result, bail};
use std::{
    ffi::OsString,
    io::Read,
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

/// Run `butterpollo.exe` (`host`) with `args`, ended after `timeout`.
fn run(host: &Path, args: &[OsString], timeout: Duration) -> Result<Outcome> {
    use std::os::windows::process::CommandExt;
    let task = args
        .first()
        .map(|arg| arg.to_string_lossy().trim_start_matches('-').to_owned())
        .unwrap_or_default();
    let mut child = Command::new(host)
        .args(args)
        .current_dir(host.parent().context("the host has no folder")?)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(windows::Win32::System::Threading::CREATE_NO_WINDOW.0)
        .spawn()
        .with_context(|| format!("starting {} --{task}", host.display()))?;
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
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    // The pipes close with the task, unless something it started keeps
    // them open: the rest of its output then stays unread.
    let mut texts = [String::new(), String::new()];
    let until = Instant::now() + Duration::from_secs(5);
    for _ in 0..texts.len() {
        match received.recv_timeout(until.saturating_duration_since(Instant::now())) {
            Ok((stream, text)) => texts[stream] = text,
            Err(_) => {
                tracing::warn!(
                    task,
                    "a program the task started keeps its output open; the rest is not read"
                );
                break;
            }
        }
    }
    let [output, errors] = texts;
    for line in errors.lines() {
        relay(&task, line);
    }
    let Some(status) = status else {
        bail!("--{task} did not finish within {} s", timeout.as_secs());
    };
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
/// mid-session: before a host starts, and as the user signs in. A session of
/// theirs that runs keeps its own. The task logs what it did and what failed.
pub fn restore_nucleus(host: &Path, profile: &Path) {
    let args = [
        "--restore-nucleus-at-console".into(),
        "--config-dir".into(),
        profile.as_os_str().to_owned(),
    ];
    match run(host, &args, Duration::from_secs(120)) {
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

/// Whether the virtual display driver counts as there for the repair watch:
/// its interface opens, or its device is installed and switched off, which
/// the user may have done on purpose. Missing comes with the reason.
#[derive(Debug, PartialEq, Eq)]
pub enum Driver {
    Installed,
    Missing(String),
}
pub fn virtual_display(host: &Path) -> Result<Driver> {
    driver(run(
        host,
        &["--virtual-display-installed".into()],
        Duration::from_secs(60),
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
    #[test]
    fn a_task_reports_its_exit_code_and_output_and_is_ended_after_its_time() {
        let cmd = Path::new(r"C:\Windows\System32\cmd.exe");
        let outcome = run(
            cmd,
            &["/d".into(), "/c".into(), "echo missing& exit /b 1".into()],
            Duration::from_secs(30),
        )
        .unwrap();
        assert_eq!(outcome.code, 1);
        assert_eq!(outcome.output.trim(), "missing");
        let started = Instant::now();
        let error = run(
            cmd,
            &["/d".into(), "/c".into(), "ping -n 30 127.0.0.1 >nul".into()],
            Duration::from_millis(500),
        )
        .unwrap_err();
        assert!(error.to_string().contains("did not finish"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(20));
        assert!(
            run(
                Path::new(r"C:\missing\butterpollo.exe"),
                &[],
                Duration::from_secs(1)
            )
            .is_err()
        );
    }
}
