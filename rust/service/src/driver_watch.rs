//! The service's watch over Rubylight's virtual display and virtual gamepad
//! drivers. Setup installs both, but a failed driver step, an uninstalled
//! device or a driver package Windows dropped can leave one missing: Artemis
//! then reports its "SudoVDA" display driver as not installed, or controllers
//! do not appear. When a driver is missing for a minute, whether since the
//! service started or after it was removed while the service runs, setup sets
//! it up again from this installation, in one run for the drivers due
//! together. Each driver has a watch of its own, so one that keeps failing
//! never holds the other back. Setup does that at most once a day per driver
//! and version (driver-repair.json), never during a stream and never for a
//! driver left out at install, for which the service starts no setup either;
//! the driver scripts may stop and restart this service. A device the user
//! switched off counts as there.
//!
//! The service reads whether a device is there itself ([`crate::driver`]),
//! without starting a process. Whether the host can use the display driver
//! the host's `--virtual-display-installed` task says, asked as
//! [`DriverWatch`] says; the gamepad driver's watch never asks.
use crate::{
    driver::{self, Driver},
    process::Signal,
    tasks,
};
use serde_json::Value;
use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

/// When a driver counts as missing: it has been unavailable for a whole
/// minute (it can come up after the service at boot), at the service's start
/// or any time later, such as a device removed while the service runs. After
/// a repair attempt the watch stays quiet for an hour; setup also limits
/// itself, and a repair that failed must not start a new setup process every
/// few seconds.
///
/// Whether the device is there the service reads itself, on every poll.
/// Whether the host can use the display driver (a driver of another protocol
/// left by an update that did not finish its driver step) only the host can
/// say, so the host is asked ([`Step::AskHost`]) on two occasions only: once
/// per service run, the first time the device is there, and before a repair
/// for a device it refused, in case that was passing. Never twice within a
/// minute, and the second at most once an hour. An ask that got no answer
/// (the task failed) is asked again 10 minutes later, then hourly, until one
/// comes; an answer is never asked for again on a timer.
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
    /// Asks in a row that got no answer, and when to ask again.
    unanswered: u32,
    ask_again: Option<Instant>,
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
    /// The first ask again after one that got no answer; later ones wait
    /// [`Self::QUIET`].
    const RETRY: Duration = Duration::from_secs(600);
    /// The watch of `driver`: only the display driver's asks the host.
    fn new(driver: Driver) -> Self {
        Self {
            // As if answered: a device that is there is usable.
            asked: driver != Driver::Display,
            ..Self::default()
        }
    }
    /// `present`: whether the device is there now.
    fn poll(&mut self, present: bool, now: Instant) -> Step {
        if present && (!self.asked || self.ask_again.is_some_and(|at| now >= at)) {
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
    /// repair keeps the watch quiet for an hour instead of repairing, any
    /// other is asked again later.
    fn answered(&mut self, usable: Option<bool>, now: Instant) {
        self.asked = true;
        match usable {
            Some(usable) => {
                self.refused = !usable;
                self.unanswered = 0;
                self.ask_again = None;
            }
            None if self.confirming => {
                self.missing_since = None;
                self.quiet_until = Some(now + Self::QUIET);
                self.confirming = false;
            }
            None => {
                self.unanswered += 1;
                let wait = if self.unanswered == 1 {
                    Self::RETRY
                } else {
                    Self::QUIET
                };
                self.ask_again = Some(now + wait);
            }
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
/// The error of a check that runs again and again, such as every minute:
/// logged once, then again only when it changes, and once more as it clears.
#[derive(Default)]
struct Repeated(Option<String>);
impl Repeated {
    /// The check failed with `error`; whether to log it.
    fn failed(&mut self, error: &str) -> bool {
        let new = self.0.as_deref() != Some(error);
        if new {
            self.0 = Some(error.to_owned());
        }
        new
    }
    /// The check passed; whether that ends an error logged before.
    fn passed(&mut self) -> bool {
        self.0.take().is_some()
    }
}

/// One round of the watch at the time `clock` tells: each driver's watch is
/// polled with whether its device is there (`present`; None when Plug and
/// Play could not say, which decides nothing), the host is asked through
/// `ask` (None: no answer) when a watch wants it, and the drivers due for a
/// repair come back with whether their device was there, for one setup run.
fn round(
    watches: &mut [(Driver, DriverWatch)],
    mut present: impl FnMut(Driver) -> Option<bool>,
    mut ask: impl FnMut(Driver) -> Option<bool>,
    clock: impl Fn() -> Instant,
) -> Vec<(Driver, bool)> {
    let mut due = Vec::new();
    for (driver, watch) in watches.iter_mut() {
        let Some(there) = present(*driver) else {
            continue;
        };
        // Each answer is followed by another poll at once, as the watch
        // wants: at most two asks (the first and the one before a repair),
        // then the repair.
        for _ in 0..3 {
            match watch.poll(there, clock()) {
                Step::Wait => break,
                Step::AskHost => {
                    let usable = ask(*driver);
                    watch.answered(usable, clock());
                }
                Step::Repair => {
                    due.push((*driver, there));
                    break;
                }
            }
        }
    }
    due
}
/// The arguments of the setup run that repairs `due`: plain --repair-drivers
/// is the limited, quiet run; only a person adds --now.
fn repair_args(due: &[Driver]) -> Vec<&'static str> {
    let mut args = vec!["--repair-drivers"];
    for driver in due {
        args.extend(["--driver", setup_name(*driver)]);
    }
    args
}
/// The driver's name in setup's `--driver`.
fn setup_name(driver: Driver) -> &'static str {
    match driver {
        Driver::Display => "display",
        Driver::Gamepad => "gamepad",
    }
}
/// The installer of the driver in the installation; a package without it
/// has nothing to set up again.
fn script(driver: Driver) -> &'static str {
    match driver {
        Driver::Display => "drivers\\rubylight-display\\install-local.ps1",
        Driver::Gamepad => "drivers\\rubylight-gamepad\\install-local.ps1",
    }
}
/// Whether setup's record of driver repairs says `driver` was left out at
/// install (--no-display-driver, --no-gamepad-driver): setup would not set it
/// up again, so no setup starts for it. The display driver's entry is the
/// record's top level, the gamepad driver's under "gamepad"; setup writes the
/// record (rust/setup/src/install/driver_repair.rs), keep the two in step.
fn declined(record: &Value, driver: Driver) -> bool {
    let entry = match driver {
        Driver::Display => record,
        Driver::Gamepad => &record["gamepad"],
    };
    entry["declined"] == true
}

/// Watch the drivers whose installer `install` holds, until `stop` is set;
/// None when there is nothing to watch.
pub fn watch(
    install: &Path,
    host: &Path,
    stop: Arc<Signal>,
) -> Option<std::thread::JoinHandle<()>> {
    use std::os::windows::process::CommandExt;
    let setup = install.join("uninstall.exe");
    let drivers: Vec<Driver> = Driver::ALL
        .into_iter()
        .filter(|driver| install.join(script(*driver)).is_file())
        .collect();
    if !setup.is_file() || drivers.is_empty() {
        return None;
    }
    let host = host.to_owned();
    let record = crate::installed_profile().join("driver-repair.json");
    Some(std::thread::spawn(move || {
        let mut watches: Vec<_> = drivers
            .iter()
            .map(|driver| (*driver, DriverWatch::new(*driver)))
            .collect();
        // Per driver, as Driver::ALL orders them.
        let mut checks = [Repeated::default(), Repeated::default()];
        let mut told = [false; 2];
        // Why the host refused the display device, for the repair's log line.
        let mut refusal = String::new();
        // Until the service stops, which also ends a task that runs.
        let stopped = |interval| {
            stop.wait(interval).unwrap_or_else(|error| {
                tracing::error!(%error, "the driver watch cannot wait; it stops");
                true
            })
        };
        loop {
            // A check that failed decides nothing: the next one comes after
            // the same interval.
            let present = |driver: Driver| {
                let check = &mut checks[driver as usize];
                match driver::present(driver) {
                    Ok(present) => {
                        if check.passed() {
                            tracing::info!(
                                driver = driver.name(),
                                "the driver can be checked again"
                            );
                        }
                        Some(present)
                    }
                    Err(error) => {
                        let error = format!("{error:#}");
                        if check.failed(&error) {
                            tracing::error!(%error, driver = driver.name(), "the driver could not be checked");
                        }
                        None
                    }
                }
            };
            let ask = |_: Driver| match tasks::virtual_display(&host, &stop) {
                Ok(tasks::Driver::Installed) => Some(true),
                Ok(tasks::Driver::Missing(reason)) => {
                    tracing::warn!(%reason, "the host cannot use the virtual display driver");
                    refusal = reason;
                    Some(false)
                }
                // The service stops, which ended the task.
                Err(_) if stop.wait(Duration::ZERO).unwrap_or(true) => None,
                Err(error) => {
                    tracing::error!(error = %format!("{error:#}"), "the host could not check the virtual display driver");
                    None
                }
            };
            let mut due = round(&mut watches, present, ask, Instant::now);
            if stopped(Duration::ZERO) {
                break;
            }
            if !due.is_empty() {
                let record = crate::state::load_json(&record, Value::Null).unwrap_or(Value::Null);
                due.retain(|(driver, _)| {
                    let left_out = declined(&record, *driver);
                    if left_out && !std::mem::replace(&mut told[*driver as usize], true) {
                        tracing::info!(
                            driver = driver.name(),
                            "the driver is missing, but it was left out at install, so it is not set up again"
                        );
                    }
                    !left_out
                });
            }
            for (driver, present) in &due {
                let reason = if *present { refusal.as_str() } else { "" };
                tracing::warn!(
                    driver = driver.name(),
                    reason,
                    present,
                    "the driver is missing; setting it up again"
                );
            }
            if !due.is_empty() {
                let due: Vec<Driver> = due.into_iter().map(|(driver, _)| driver).collect();
                // Not a child that dies with the service: a driver script may
                // stop it.
                if let Err(error) = std::process::Command::new(&setup)
                    .args(repair_args(&due))
                    .creation_flags(windows::Win32::System::Threading::CREATE_NO_WINDOW.0)
                    .spawn()
                {
                    tracing::warn!(%error, "the driver repair could not start");
                }
            }
            let interval = watches
                .iter()
                .map(|(_, watch)| watch.interval())
                .min()
                .unwrap_or(DriverWatch::GRACE);
            if stopped(interval) {
                break;
            }
        }
    }))
}

#[cfg(test)]
mod tests;
