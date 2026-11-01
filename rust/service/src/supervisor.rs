//! Service supervisor; the child is the Rust streaming executable in the active session.

use crate::{
    process::{Process, Signal},
    tasks,
};
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
/// The service's name, kept from Apollo for migration compatibility; setup
/// and the host use the same (`butterpollo_windows::service::NAME`).
pub const NAME: &str = "ApolloService";
/// The exit code with which the host asks to be started again at once
/// (`butterpollo_windows::service::RESTART_EXIT_CODE`).
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

fn shutdown_child(process: Process) {
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
///
/// Whether the device is there the service reads itself ([`crate::driver`]),
/// on every poll. Whether the host can use it (a driver of another protocol
/// left by an update that did not finish its driver step) only the host can
/// say, so the host is asked ([`Step::AskHost`]) on two occasions only: once
/// per service run, the first time the device is there, and before a repair
/// for a device it refused, in case that was passing. Never twice within a
/// minute, and the second at most once an hour.
#[derive(Default)]
struct DriverWatch {
    missing_since: Option<Instant>,
    quiet_until: Option<Instant>,
    /// The host was asked this run.
    asked: bool,
    /// Its last answer: it could not use the device that is there.
    refused: bool,
    /// It is being asked again before a repair.
    confirming: bool,
}
/// What a poll of the [`DriverWatch`] asks for.
#[derive(Debug, PartialEq, Eq)]
enum Step {
    Wait,
    /// Ask the host whether it can use the driver, then poll again at once.
    AskHost,
    /// Set the driver up again.
    Repair,
}
impl DriverWatch {
    const GRACE: Duration = Duration::from_secs(60);
    const QUIET: Duration = Duration::from_secs(3600);
    /// `present`: whether the device is there now.
    fn poll(&mut self, present: bool, now: Instant) -> Step {
        if present && !self.asked {
            return Step::AskHost;
        }
        if present && !self.refused {
            self.missing_since = None;
            self.confirming = false;
            return Step::Wait;
        }
        if self.quiet_until.is_some_and(|until| now < until) {
            return Step::Wait;
        }
        let since = *self.missing_since.get_or_insert(now);
        if now.duration_since(since) < Self::GRACE {
            return Step::Wait;
        }
        if present && !self.confirming {
            self.confirming = true;
            return Step::AskHost;
        }
        self.missing_since = None;
        self.quiet_until = Some(now + Self::QUIET);
        self.confirming = false;
        Step::Repair
    }
    /// The host's answer to [`Step::AskHost`]: whether it can use the driver,
    /// None when the check failed. A failed check decides nothing; one before a
    /// repair keeps the watch quiet for an hour instead of repairing.
    fn answered(&mut self, usable: Option<bool>, now: Instant) {
        self.asked = true;
        match usable {
            Some(usable) => self.refused = !usable,
            None if self.confirming => {
                self.missing_since = None;
                self.quiet_until = Some(now + Self::QUIET);
                self.confirming = false;
            }
            None => {}
        }
    }
    /// How long to wait before the next poll: a minute while the driver is
    /// there, which is the common case for the whole life of the service, and
    /// 5 seconds while its minute of grace runs. Polls read Plug and Play;
    /// only [`Step::AskHost`] starts a process.
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
/// The service reads whether the device is there; the host's
/// `--virtual-display-installed` task says whether it can use it, asked as
/// [`DriverWatch`] says.
fn repair_display_driver(install: &std::path::Path, host: &std::path::Path, stop: Arc<Signal>) {
    use std::os::windows::process::CommandExt;
    let setup = install.join("uninstall.exe");
    if !setup.is_file()
        || !install
            .join("drivers\\rubylight-display\\install-local.ps1")
            .is_file()
    {
        return;
    }
    let host = host.to_owned();
    std::thread::spawn(move || {
        let mut watch = DriverWatch::default();
        // Why the host refused the device, for the repair's log line.
        let mut refusal = String::new();
        // Until the service stops, which also ends a task that runs.
        let stopped = |interval| stop.wait(interval).unwrap_or(true);
        loop {
            // A check that failed decides nothing: the next one comes after
            // the same interval.
            let present = match crate::driver::present() {
                Ok(present) => present,
                Err(error) => {
                    tracing::error!(error = %format!("{error:#}"), "the virtual display driver could not be checked");
                    if stopped(watch.interval()) {
                        break;
                    }
                    continue;
                }
            };
            match watch.poll(present, Instant::now()) {
                Step::Wait => {}
                Step::AskHost => {
                    let usable = match tasks::virtual_display(&host, &stop) {
                        Ok(tasks::Driver::Installed) => Some(true),
                        Ok(tasks::Driver::Missing(reason)) => {
                            tracing::warn!(%reason, "the host cannot use the virtual display driver");
                            refusal = reason;
                            Some(false)
                        }
                        // The service stops, which ended the task.
                        Err(_) if stopped(Duration::ZERO) => break,
                        Err(error) => {
                            tracing::error!(error = %format!("{error:#}"), "the host could not check the virtual display driver");
                            None
                        }
                    };
                    watch.answered(usable, Instant::now());
                    continue;
                }
                Step::Repair => {
                    let reason = if present { refusal.as_str() } else { "" };
                    tracing::warn!(
                        reason,
                        present,
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
            }
            if stopped(watch.interval()) {
                break;
            }
        }
    });
}
/// Whether a change of `session` may be the user signing in at the console
/// (session `console`), when what a Nucleus Co-op session left is put back
/// ([`tasks::restore_nucleus`]). Another user signing in remotely leaves the
/// console's session as it is.
fn signs_in(reason: SessionChangeReason, session: u32, console: u32) -> bool {
    session == console
        && matches!(
            reason,
            SessionChangeReason::SessionLogon | SessionChangeReason::ConsoleConnect
        )
}
/// When the Nucleus Co-op restore ([`tasks::restore_nucleus`]) that a sign-in
/// asked for runs: on a worker, so that a stop, the host's exit or a session
/// change is handled while it runs, and only while a host runs. A request
/// waits for a restore that runs to finish, then runs again, since the
/// sign-in came after that one began. A host launched while one is due puts
/// it back itself as it starts (rust/host/src/service_tasks.rs), which serves
/// the request.
#[derive(Default)]
struct RestoreDue(bool);
impl RestoreDue {
    fn requested(&mut self) {
        self.0 = true;
    }
    fn launched(&mut self) {
        self.0 = false;
    }
    /// Whether to start the restore now: it is due, a host runs (`host`) and no
    /// restore runs (`running`).
    fn start(&mut self, host: bool, running: bool) -> bool {
        let start = self.0 && host && !running;
        if start {
            self.0 = false;
        }
        start
    }
}

fn supervise() -> Result<()> {
    let stop = Arc::new(AtomicBool::new(false));
    let event_stop = stop.clone();
    let wake = Arc::new(Signal::new()?);
    let event_wake = wake.clone();
    // Set once as the service stops, never reset: it ends the tasks that run
    // and the driver watch.
    let stopping = Arc::new(Signal::new()?);
    let event_stopping = stopping.clone();
    // Due as the user signs in at the console while a host runs; a host puts
    // back what an earlier one left as it starts (service_tasks.rs).
    let restore = Arc::new(AtomicBool::new(false));
    let event_restore = restore.clone();
    let reporter = service_control_handler::register(NAME, move |control| match control {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            event_stop.store(true, Ordering::Release);
            let _ = event_stopping.set();
            let _ = event_wake.set();
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::SessionChange(change) => {
            let console = crate::process::console_session();
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
    let config = crate::installed_profile();
    // Interfaces come from bind_address and address_family (IPv4 by default).
    let args = vec![
        OsString::from("--config-dir"),
        config.clone().into_os_string(),
    ];
    reporter.set_service_status(status(ServiceState::Running, 0))?;
    tracing::info!(
        version = crate::SERVICE_VERSION,
        "Rubylight service started"
    );
    // An update that power loss or a crash interrupted is rolled back before
    // a host starts from a mix of two versions.
    if let Some(install) = executable.parent() {
        match crate::update_recovery::recover(&config, install) {
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
        repair_display_driver(install, &executable, stopping.clone());
    }
    let mut child: Option<Process> = None;
    let mut session = u32::MAX;
    let mut restart = RestartPolicy::default();
    let mut retry_at = Instant::now();
    let mut due = RestoreDue::default();
    // The restore's worker, and whether it still runs: it clears the flag,
    // then wakes this loop.
    let mut restorer: Option<std::thread::JoinHandle<()>> = None;
    let restoring = Arc::new(AtomicBool::new(false));
    loop {
        wake.reset()?;
        if stop.load(Ordering::Acquire) {
            break;
        }
        let current = crate::process::console_session();
        if current != session {
            if let Some(process) = child.take() {
                shutdown_child(process);
            }
            session = current;
            restart = RestartPolicy::default();
            retry_at = Instant::now();
        }
        if let Some(process) = &child
            && let Some(code) = process.exit_code()?
        {
            child = None;
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
        // As the user signs in while a host runs: a session it runs holds its
        // file's lock, and the restore skips it. Without a host, the next one
        // puts it back as it starts.
        if restore.swap(false, Ordering::AcqRel) {
            due.requested();
        }
        let running = restoring.load(Ordering::Acquire);
        if !running && let Some(worker) = restorer.take() {
            let _ = worker.join();
        }
        if due.start(child.is_some(), running) {
            restoring.store(true, Ordering::Release);
            let (host, profile) = (executable.clone(), config.clone());
            let (stopping, wake, restoring) = (stopping.clone(), wake.clone(), restoring.clone());
            restorer = Some(std::thread::spawn(move || {
                tasks::restore_nucleus(&host, &profile, &stopping);
                restoring.store(false, Ordering::Release);
                let _ = wake.set();
            }));
        }
        // A stop can arrive while a host shuts down above; a new host would
        // only delay it.
        if child.is_none()
            && session != u32::MAX
            && Instant::now() >= retry_at
            && !stop.load(Ordering::Acquire)
        {
            match Process::spawn_host(&executable, &args, executable.parent(), session) {
                Ok(process) => {
                    tracing::info!(pid = process.pid, session, "service host launched");
                    child = Some(process);
                    due.launched();
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
    // Also when the host's exit stopped the service: the tasks that run end.
    let _ = stopping.set();
    if let Some(process) = child.take() {
        shutdown_child(process);
    }
    // The restore's task has ended with its job; its worker reads what is left.
    if let Some(worker) = restorer.take() {
        let started = Instant::now();
        while !worker.is_finished() && started.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(50));
        }
        if worker.is_finished() {
            let _ = worker.join();
        } else {
            tracing::warn!("the Nucleus Co-op restore did not end with the service");
        }
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
    fn the_restore_runs_only_with_a_host_and_no_request_is_lost() {
        let mut due = RestoreDue::default();
        assert!(!due.start(true, false), "nothing asked for it");
        // A sign-in while no host runs: no restore without a host.
        due.requested();
        assert!(!due.start(false, false));
        assert!(!due.start(false, false));
        // It stays due until the host that is launched next, which puts it
        // back itself as it starts.
        assert!(due.0);
        due.launched();
        assert!(!due.start(true, false));
        // A sign-in while a host runs starts it once.
        due.requested();
        assert!(due.start(true, false));
        assert!(!due.start(true, true));
        // Another while it runs waits for it, then runs again.
        due.requested();
        assert!(!due.start(true, true));
        assert!(!due.start(true, true));
        assert!(due.start(true, false));
        assert!(!due.start(true, false));
        // One that comes while a restore runs and the host restarts goes to
        // the new host (restores take the journal's lock in turn).
        due.requested();
        assert!(!due.start(false, true));
        assert!(!due.start(false, false));
        due.launched();
        assert!(!due.start(true, false));
    }
    /// Drives a [`DriverWatch`] as the watch thread does, from second 0 to
    /// `end`: `present` is the Plug and Play answer at a second, `host` the
    /// host's answer to an ask. The seconds at which the host was asked and a
    /// repair started.
    fn drive(
        end: u64,
        present: impl Fn(u64) -> bool,
        mut host: impl FnMut(u64) -> Option<bool>,
    ) -> (Vec<u64>, Vec<u64>) {
        let start = Instant::now();
        let mut watch = DriverWatch::default();
        let (mut asks, mut repairs) = (Vec::new(), Vec::new());
        let mut second = 0;
        while second < end {
            let now = start + Duration::from_secs(second);
            match watch.poll(present(second), now) {
                Step::Wait => {}
                Step::AskHost => {
                    asks.push(second);
                    assert!(asks.len() < 100, "the host is asked in a loop");
                    watch.answered(host(second), now);
                    continue;
                }
                Step::Repair => repairs.push(second),
            }
            second += watch.interval().as_secs();
        }
        (asks, repairs)
    }
    fn spaced(seconds: &[u64], at_least: u64) -> bool {
        seconds.windows(2).all(|pair| pair[1] - pair[0] >= at_least)
    }
    #[test]
    fn a_driver_removed_while_the_service_runs_is_repaired_after_a_minute() {
        // Present for ten minutes, then removed (pnputil /remove-device): the
        // host is asked once, at the start; the removal needs no host.
        let (asks, repairs) = drive(5000, |second| second < 600, |_| Some(true));
        assert_eq!(asks, [0]);
        // A repair that left it missing is not started again every few
        // seconds; the next try comes an hour later.
        assert_eq!(repairs, [660, 660 + 3600 + 60]);
        // A blip that heals within the minute never repairs; the next
        // removal gets its own minute.
        let present = |second| !(60..110).contains(&second) && second < 200;
        let (_, repairs) = drive(300, present, |_| Some(true));
        assert_eq!(repairs, [290]);
        // A minute apart while the driver is there, 5 seconds while it is not.
        let start = Instant::now();
        let mut watch = DriverWatch::default();
        assert_eq!(watch.poll(true, start), Step::AskHost);
        watch.answered(Some(true), start);
        assert_eq!(watch.poll(true, start), Step::Wait);
        assert_eq!(watch.interval(), Duration::from_secs(60));
        watch.poll(false, start + Duration::from_secs(60));
        assert_eq!(watch.interval(), Duration::from_secs(5));
        assert_eq!(
            watch.poll(false, start + Duration::from_secs(120)),
            Step::Repair
        );
        // Missing through the quiet hour: back to a minute.
        assert_eq!(watch.interval(), Duration::from_secs(60));
    }
    #[test]
    fn the_host_is_asked_once_unless_it_refuses_and_never_on_a_timer() {
        // A day with the driver there and usable: one ask, at the start.
        let (asks, repairs) = drive(86_400, |_| true, |_| Some(true));
        assert_eq!(asks, [0]);
        assert!(repairs.is_empty());
        // At boot the device comes up after the service: the ask waits for it.
        let (asks, repairs) = drive(86_400, |second| second >= 30, |_| Some(true));
        assert_eq!(asks, [30]);
        assert!(repairs.is_empty());
        // A day with a device the host keeps refusing (another protocol):
        // asked again a minute later before the repair, then once an hour.
        let (asks, repairs) = drive(86_400, |_| true, |_| Some(false));
        assert_eq!(&asks[..3], [0, 60, 60 + 3600 + 60]);
        assert_eq!(&repairs[..2], [60, 60 + 3600 + 60]);
        assert!(spaced(&asks[1..], 3600), "{asks:?}");
        assert!(asks.len() <= 25 && repairs.len() <= 24);
        // A refusal that passed (the device was still starting) repairs nothing.
        let (asks, repairs) = drive(86_400, |_| true, |second| Some(second > 0));
        assert_eq!(asks, [0, 60]);
        assert!(repairs.is_empty());
        // A host that cannot answer decides nothing: no repair from its
        // silence, and no ask more than once an hour.
        let (asks, repairs) = drive(86_400, |_| true, |_| None);
        assert_eq!(asks, [0]);
        assert!(repairs.is_empty());
        let (asks, repairs) = drive(86_400, |_| true, |second| (second == 0).then_some(false));
        assert!(repairs.is_empty());
        assert!(spaced(&asks, 60) && spaced(&asks[1..], 3600), "{asks:?}");
        // Whatever the answers, never twice within a minute.
        for host in [Some(true), Some(false), None] {
            let (asks, _) = drive(86_400, |second| (second / 600) % 2 == 0, |_| host);
            assert!(spaced(&asks, 60), "{host:?}: {asks:?}");
        }
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
