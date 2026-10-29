//! Service supervisor; the child is the Rust streaming executable in the active session.

use anyhow::{Context, Result};
use std::{
    ffi::OsString,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use windows::Win32::Foundation::DBG_TERMINATE_PROCESS;
use windows_service::{
    define_windows_service,
    service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType, SessionChangeReason,
    },
    service_control_handler::{self, ServiceControlHandlerResult},
    service_dispatcher,
};
pub const NAME: &str = "ApolloService";
pub const RESTART_EXIT_CODE: u32 = 75;

#[derive(Default)]
struct RestartPolicy {
    failures: std::collections::VecDeque<Instant>,
}
impl RestartPolicy {
    fn exited(&mut self, code: u32, now: Instant) -> Option<Duration> {
        match code {
            0 => None,
            RESTART_EXIT_CODE => Some(Duration::ZERO),
            code if code == DBG_TERMINATE_PROCESS.0 as u32 => Some(Duration::from_secs(1)),
            _ => {
                self.failures
                    .retain(|failure| now.duration_since(*failure) < Duration::from_secs(60));
                self.failures.push_back(now);
                Some(if self.failures.len() >= 3 {
                    Duration::from_secs(30)
                } else {
                    Duration::from_secs(1)
                })
            }
        }
    }
}

fn shutdown_child(process: crate::process::Process) {
    let started = Instant::now();
    match process.shutdown_host() {
        Ok(outcome) => tracing::info!(
            pid = process.pid,
            elapsed_ms = started.elapsed().as_millis(),
            ?outcome,
            "service host stopped"
        ),
        Err(error) => {
            tracing::error!(pid=process.pid, %error, "service host shutdown failed; closing its owned job")
        }
    }
}
define_windows_service!(ffi_main, service_main);
pub fn run() -> Result<()> {
    service_dispatcher::start(NAME, ffi_main)?;
    Ok(())
}
fn service_main(_args: Vec<OsString>) {
    if let Err(e) = supervise() {
        tracing::error!(error=%e,"Windows service failed");
    }
}
fn status(state: ServiceState, checkpoint: u32) -> ServiceStatus {
    ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: if state == ServiceState::Running {
            ServiceControlAccept::STOP
                | ServiceControlAccept::SHUTDOWN
                | ServiceControlAccept::SESSION_CHANGE
        } else {
            ServiceControlAccept::empty()
        },
        exit_code: ServiceExitCode::Win32(0),
        checkpoint,
        wait_hint: if checkpoint != 0 {
            Duration::from_secs(25)
        } else {
            Duration::ZERO
        },
        process_id: None,
    }
}
/// When the virtual display driver counts as missing: it has been
/// unavailable for a whole minute (it can come up after the service at boot),
/// at the service's start or any time later, such as a device removed while
/// the service runs. After a repair attempt the watch stays quiet for an hour;
/// setup also limits itself, and a repair that failed must not start a new
/// setup process every few seconds.
#[derive(Default)]
struct DriverWatch {
    missing_since: Option<Instant>,
    quiet_until: Option<Instant>,
}
impl DriverWatch {
    const GRACE: Duration = Duration::from_secs(60);
    const QUIET: Duration = Duration::from_secs(3600);
    /// Whether to run the repair now.
    fn poll(&mut self, available: bool, now: Instant) -> bool {
        if available {
            self.missing_since = None;
            return false;
        }
        if self.quiet_until.is_some_and(|until| now < until) {
            return false;
        }
        let since = *self.missing_since.get_or_insert(now);
        if now.duration_since(since) < Self::GRACE {
            return false;
        }
        self.missing_since = None;
        self.quiet_until = Some(now + Self::QUIET);
        true
    }
    /// How long to wait before the next poll: a minute while the driver is
    /// there, which is the common case for the whole life of the service, and
    /// 5 seconds while its minute of grace runs.
    fn interval(&self) -> Duration {
        if self.missing_since.is_some() {
            Duration::from_secs(5)
        } else {
            Duration::from_secs(60)
        }
    }
}
/// Setup installs Rubylight's virtual display driver, but a failed driver
/// step, an uninstalled device or a driver package Windows dropped can leave
/// it missing, and Artemis then reports its "SudoVDA" driver as not installed.
/// When the driver is missing for a minute, whether since the service started
/// or after it was removed while the service runs, setup sets it up again from
/// this installation. Setup does that at most once a day per version, never
/// during a stream and never after --no-display-driver; the driver script stops
/// and restarts this service. A device the user switched off counts as there.
fn repair_display_driver(install: &std::path::Path, stop: Arc<AtomicBool>) {
    use std::os::windows::process::CommandExt;
    let setup = install.join("uninstall.exe");
    if !setup.is_file()
        || !install
            .join("drivers\\rubylight-display\\install-local.ps1")
            .is_file()
    {
        return;
    }
    std::thread::spawn(move || {
        let mut watch = DriverWatch::default();
        while !stop.load(Ordering::Acquire) {
            if watch.poll(crate::display::virtual_display_installed(), Instant::now()) {
                let status = crate::display::virtual_display_status();
                tracing::warn!(
                    reason = status["reason"].as_str().unwrap_or(""),
                    "the virtual display driver is missing; setting it up again"
                );
                // Not a child that dies with the service: the script stops it.
                // Plain --repair-drivers is the limited, quiet run; only a
                // person adds --now.
                if let Err(error) = std::process::Command::new(&setup)
                    .arg("--repair-drivers")
                    .creation_flags(windows::Win32::System::Threading::CREATE_NO_WINDOW.0)
                    .spawn()
                {
                    tracing::warn!(%error, "the driver repair could not start");
                }
            }
            std::thread::sleep(watch.interval());
        }
    });
}
/// Whether a change of `session` may be the user signing in at the console
/// (session `console`), when what a Nucleus Co-op session left is put back
/// ([`restore_nucleus_layers`]). Another user signing in remotely leaves the
/// console's session as it is.
fn signs_in(reason: SessionChangeReason, session: u32, console: u32) -> bool {
    session == console
        && matches!(
            reason,
            SessionChangeReason::SessionLogon | SessionChangeReason::ConsoleConnect
        )
}

/// Put back the compatibility settings a Nucleus Co-op session left for the
/// user at the console when its host stopped mid-session
/// (`nucleus::layers`): before a host starts, and as the user signs in. A
/// session of theirs that runs keeps its own.
fn restore_nucleus_layers(config: &std::path::Path) {
    use crate::nucleus::layers::AtConsole;
    // The host's settings file, for nucleus_path.
    let configured = match butterpollo_core::config::Config::load(&config.join("sunshine.conf")) {
        Ok(config) => config
            .nucleus_path()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default(),
        Err(error) => {
            tracing::error!(error = %format!("{error:#}"), "Nucleus Co-op: the settings did not load, so the compatibility settings an earlier session changed were not put back");
            return;
        }
    };
    restore_nucleus_settings(&configured);
    match crate::nucleus::layers::restore_at_console(
        &butterpollo_core::paths::data_root(),
        &configured,
    ) {
        Ok(AtConsole::Done) => {}
        Ok(AtConsole::Busy) => tracing::info!(
            "Nucleus Co-op: a session runs for the user at the console and puts its compatibility settings back itself"
        ),
        Ok(AtConsole::NoUser { why, waiting }) => {
            if !waiting.is_empty() {
                let files = waiting
                    .iter()
                    .map(|file| file.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                tracing::warn!(
                    why,
                    files,
                    "Nucleus Co-op: compatibility settings an earlier session changed wait for their user to sign in at the console"
                );
            }
        }
        Err(error) => {
            tracing::error!(error = %format!("{error:#}"), "Nucleus Co-op: the compatibility settings an earlier session changed were not put back")
        }
    }
}

/// Put back the user's Nucleus Co-op settings a session changed when its
/// host stopped mid-session (`nucleus::files::restore_settings_left`). Not
/// while Nucleus Co-op runs: a session's own puts them back as it ends.
fn restore_nucleus_settings(configured: &str) {
    let restored = (|| -> Result<usize> {
        let Some(folder) =
            crate::nucleus::install::installed(&butterpollo_core::paths::data_root(), configured)?
        else {
            return Ok(0);
        };
        if let Some(pid) = crate::nucleus::session::running()? {
            tracing::info!(
                pid,
                "Nucleus Co-op runs; the settings an earlier session changed are put back before the next"
            );
            return Ok(0);
        }
        crate::nucleus::files::restore_settings_left(&folder)
    })();
    match restored {
        Ok(0) => {}
        Ok(changed) => tracing::info!(
            changed,
            "Nucleus Co-op: the settings an earlier session changed are back"
        ),
        Err(error) => tracing::error!(
            error = %format!("{error:#}"),
            "Nucleus Co-op: the settings an earlier session changed were not put back"
        ),
    }
}

fn supervise() -> Result<()> {
    let stop = Arc::new(AtomicBool::new(false));
    let event_stop = stop.clone();
    let wake = Arc::new(crate::timing::Signal::new()?);
    let event_wake = wake.clone();
    // Due at the start, after a host stops, and as the user signs in.
    let restore = Arc::new(AtomicBool::new(true));
    let event_restore = restore.clone();
    let reporter = service_control_handler::register(NAME, move |control| match control {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            event_stop.store(true, Ordering::Release);
            let _ = event_wake.set();
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::SessionChange(change) => {
            // SAFETY: WTSGetActiveConsoleSessionId takes no arguments and has no preconditions.
            let console =
                unsafe { windows::Win32::System::RemoteDesktop::WTSGetActiveConsoleSessionId() };
            if signs_in(change.reason, change.notification.session_id, console) {
                event_restore.store(true, Ordering::Release);
            }
            let _ = event_wake.set();
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })?;
    struct ReportStopped(
        windows_service::service_control_handler::ServiceStatusHandle,
        bool,
    );
    impl Drop for ReportStopped {
        fn drop(&mut self) {
            if !self.1 {
                let mut failed = status(ServiceState::Stopped, 0);
                failed.exit_code = ServiceExitCode::Win32(1067);
                let _ = self.0.set_service_status(failed);
            }
        }
    }
    let mut cleanup = ReportStopped(reporter, false);
    reporter.set_service_status(status(ServiceState::StartPending, 1))?;
    let executable = std::env::current_exe()?
        .parent()
        .context("service directory unavailable")?
        .join("butterpollo.exe");
    let config = butterpollo_core::paths::installed_profile();
    // Interfaces come from bind_address and address_family (IPv4 by default).
    let args = vec![
        OsString::from("--config-dir"),
        config.clone().into_os_string(),
    ];
    reporter.set_service_status(status(ServiceState::Running, 0))?;
    // An update that power loss or a crash interrupted is rolled back before
    // a host starts from a mix of two versions.
    if let Some(install) = executable.parent() {
        match butterpollo_core::update_recovery::recover(&config, install) {
            Ok(Some(outcome)) => tracing::warn!(outcome, "interrupted update rolled back"),
            Ok(None) => {}
            // The host still starts: it serves the console that reports the
            // failure, and the next start or setup tries again. Stopping the
            // service here would leave no host at all, even for an unreadable
            // record.
            Err(error) => tracing::error!(
                error = %format!("{error:#}"),
                "rolling back an interrupted update failed"
            ),
        }
    }
    if let Some(install) = executable.parent() {
        repair_display_driver(install, stop.clone());
    }
    let mut child: Option<crate::process::Process> = None;
    let mut session = u32::MAX;
    let mut restart = RestartPolicy::default();
    let mut retry_at = Instant::now();
    loop {
        wake.reset()?;
        if stop.load(Ordering::Acquire) {
            break;
        }
        // SAFETY: WTSGetActiveConsoleSessionId takes no arguments and has no preconditions.
        let current =
            unsafe { windows::Win32::System::RemoteDesktop::WTSGetActiveConsoleSessionId() };
        if current != session {
            if let Some(process) = child.take() {
                shutdown_child(process);
                restore.store(true, Ordering::Release);
            }
            session = current;
            restart = RestartPolicy::default();
            retry_at = Instant::now();
        }
        if let Some(process) = &child
            && let Some(code) = process.exit_code()?
        {
            child = None;
            restore.store(true, Ordering::Release);
            let now = Instant::now();
            let Some(delay) = restart.exited(code, now) else {
                stop.store(true, Ordering::Release);
                break;
            };
            if code == RESTART_EXIT_CODE {
                tracing::info!("host requested a restart");
            } else if code == DBG_TERMINATE_PROCESS.0 as u32 {
                tracing::info!(
                    code,
                    retry_seconds = delay.as_secs(),
                    "Windows terminated the host; retrying the console session"
                );
            } else {
                tracing::warn!(
                    code,
                    retry_seconds = delay.as_secs(),
                    "host exited unexpectedly"
                );
            }
            retry_at = now + delay;
        }
        // Before a host starts, and as the user signs in while one runs: a
        // session it runs holds its file's lock, and the restore skips it.
        if restore.swap(false, Ordering::AcqRel) {
            restore_nucleus_layers(&config);
        }
        // A stop can arrive while a host shuts down above; a new host would
        // only delay it.
        if child.is_none()
            && session != u32::MAX
            && Instant::now() >= retry_at
            && !stop.load(Ordering::Acquire)
        {
            match crate::process::Process::spawn_host(
                &executable,
                &args,
                executable.parent(),
                crate::process::Target::SystemSession(session),
            ) {
                Ok(process) => {
                    tracing::info!(pid = process.pid, session, "service host launched");
                    child = Some(process);
                }
                Err(e) => {
                    tracing::warn!(error=%e,"service host launch failed");
                    retry_at = Instant::now() + Duration::from_secs(5);
                }
            }
        }
        let mut handles = vec![wake.raw()];
        let timeout = if let Some(process) = &child {
            handles.push(process.handle());
            u32::MAX
        } else if session == u32::MAX {
            1000
        } else {
            retry_at
                .saturating_duration_since(Instant::now())
                .as_millis()
                .min(u128::from(u32::MAX - 1)) as u32
        };
        // SAFETY: The event and optional child own these handles throughout the wait.
        if unsafe {
            windows::Win32::System::Threading::WaitForMultipleObjects(&handles, false, timeout)
        } == windows::Win32::Foundation::WAIT_FAILED
        {
            return Err(windows::core::Error::from_thread().into());
        }
    }
    reporter.set_service_status(status(ServiceState::StopPending, 1))?;
    if let Some(process) = child.take() {
        shutdown_child(process);
    }
    // All process and job handles are released before SCM observes STOPPED.
    reporter.set_service_status(status(ServiceState::Stopped, 0))?;
    cleanup.1 = true;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_sign_in_at_the_console_puts_back_what_nucleus_left() {
        assert!(signs_in(SessionChangeReason::SessionLogon, 1, 1));
        assert!(signs_in(SessionChangeReason::ConsoleConnect, 2, 2));
        // Someone signing in over Remote Desktop.
        assert!(!signs_in(SessionChangeReason::SessionLogon, 3, 1));
        for reason in [
            SessionChangeReason::SessionLock,
            SessionChangeReason::SessionUnlock,
            SessionChangeReason::SessionLogoff,
            SessionChangeReason::RemoteConnect,
            SessionChangeReason::ConsoleDisconnect,
        ] {
            assert!(!signs_in(reason, 1, 1), "{reason:?}");
        }
    }
    #[test]
    fn a_driver_removed_while_the_service_runs_is_repaired_after_a_minute() {
        let mut watch = DriverWatch::default();
        let start = Instant::now();
        let at = |seconds| start + Duration::from_secs(seconds);
        // Present for ten minutes, then removed (pnputil /remove-device).
        for second in (0..600).step_by(5) {
            assert!(!watch.poll(true, at(second)));
        }
        assert!(!watch.poll(false, at(600)));
        assert!(!watch.poll(false, at(655)));
        assert!(watch.poll(false, at(660)));
        // A repair that left it missing is not started again every few
        // seconds; the next try comes an hour later.
        assert!(!watch.poll(false, at(665)));
        assert!(!watch.poll(false, at(660 + 3599)));
        assert!(!watch.poll(false, at(660 + 3600)));
        assert!(watch.poll(false, at(660 + 3600 + 60)));
        // A minute apart while the driver is there, 5 seconds while it is not.
        let mut watch = DriverWatch::default();
        watch.poll(true, at(0));
        assert_eq!(watch.interval(), Duration::from_secs(60));
        watch.poll(false, at(60));
        assert_eq!(watch.interval(), Duration::from_secs(5));
        assert!(watch.poll(false, at(120)));
        // Missing through the quiet hour: back to a minute.
        assert_eq!(watch.interval(), Duration::from_secs(60));
        // A blip that heals within the minute never repairs.
        let mut watch = DriverWatch::default();
        assert!(!watch.poll(false, at(0)));
        assert!(!watch.poll(true, at(30)));
        assert!(!watch.poll(false, at(40)));
        assert!(!watch.poll(false, at(95)));
        assert!(watch.poll(false, at(100)));
    }
    #[test]
    fn windows_termination_does_not_trigger_or_reset_crash_backoff() {
        let mut policy = RestartPolicy::default();
        let now = Instant::now();
        for second in 0..5 {
            assert_eq!(
                policy.exited(
                    DBG_TERMINATE_PROCESS.0 as u32,
                    now + Duration::from_secs(second)
                ),
                Some(Duration::from_secs(1))
            );
        }
        assert!(policy.failures.is_empty());
        for second in 5..7 {
            assert_eq!(
                policy.exited(1, now + Duration::from_secs(second)),
                Some(Duration::from_secs(1))
            );
        }
        assert_eq!(
            policy.exited(DBG_TERMINATE_PROCESS.0 as u32, now + Duration::from_secs(7)),
            Some(Duration::from_secs(1))
        );
        assert_eq!(
            policy.exited(1, now + Duration::from_secs(8)),
            Some(Duration::from_secs(30))
        );
    }
    #[test]
    fn requested_restarts_do_not_trigger_crash_backoff_or_stop_the_service() {
        let mut policy = RestartPolicy::default();
        let now = Instant::now();
        for second in 0..10 {
            assert_eq!(
                policy.exited(RESTART_EXIT_CODE, now + Duration::from_secs(second)),
                Some(Duration::ZERO)
            );
        }
        assert!(policy.failures.is_empty());
        assert_eq!(
            policy.exited(1, now + Duration::from_secs(10)),
            Some(Duration::from_secs(1))
        );
        assert_eq!(policy.exited(0, now + Duration::from_secs(11)), None);
    }
    #[test]
    fn repeated_crashes_back_off_and_old_failures_expire() {
        let mut policy = RestartPolicy::default();
        let now = Instant::now();
        for second in 0..3 {
            let expected = if second == 2 { 30 } else { 1 };
            assert_eq!(
                policy.exited(1, now + Duration::from_secs(second)),
                Some(Duration::from_secs(expected))
            );
        }
        assert_eq!(
            policy.exited(RESTART_EXIT_CODE, now + Duration::from_secs(3)),
            Some(Duration::ZERO)
        );
        assert_eq!(policy.failures.len(), 3);
        assert_eq!(
            policy.exited(1, now + Duration::from_secs(62)),
            Some(Duration::from_secs(1))
        );
        assert_eq!(policy.failures.len(), 1);
    }
}
