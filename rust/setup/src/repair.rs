//! `rubylight-setup.exe --repair`: a one-shot repair of the installed
//! Rubylight, made only of what setup already does elsewhere. It starts the
//! service again when it is stopped or its host does not answer, sets the
//! virtual display and gamepad drivers up again from the installation
//! (without the service's daily limit, but never a driver left out at
//! install), and puts back the Windows Firewall rule and the Start menu
//! shortcut. A step that fails does not stop the next; the result lists what
//! was done and what failed.
use crate::{
    detect,
    install::{self, Driver, RepairRun},
    log::line,
    system, ui,
    ui::Progress,
};
use anyhow::{Context, Result};
use std::path::Path;

/// The Rubylight service as the repair finds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Service {
    /// It runs, and its host answers.
    Answers,
    /// It runs (or is starting), but its host does not answer within the
    /// time an installation gives it to start.
    Silent,
    /// It is stopped, or stuck stopping.
    Stopped,
}
/// A step of the repair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Start the service (unless it is disabled), once a stop that hangs has
    /// ended, and wait for its host.
    StartService,
    /// Stop the service whose host does not answer, then start it again.
    RestartService,
    /// Set the drivers up again ([`install::repair_drivers_in`]).
    Drivers,
    /// The firewall rule and the Start menu shortcut
    /// ([`install::refresh_entries`]).
    Entries,
}
/// The steps of a repair, in order. The service comes first: the driver
/// repair refuses a host that runs but does not answer.
pub fn plan(service: Service) -> Vec<Step> {
    let mut steps = match service {
        Service::Answers => Vec::new(),
        Service::Silent => vec![Step::RestartService],
        Service::Stopped => vec![Step::StartService],
    };
    steps.extend([Step::Drivers, Step::Entries]);
    steps
}

/// What the repair did.
#[derive(Debug, Default)]
pub struct Outcome {
    /// Windows must restart to finish.
    pub restart: bool,
    /// What was done, in order.
    pub done: Vec<String>,
    /// What failed, in order.
    pub problems: Vec<String>,
}
impl Outcome {
    /// The heading of the result.
    pub fn heading(&self) -> &'static str {
        if self.problems.is_empty() {
            "Rubylight is repaired"
        } else {
            "Rubylight could not be repaired completely"
        }
    }
    /// The result's text: what was done, then what failed.
    pub fn text(&self) -> String {
        let mut parts = self.done.clone();
        parts.extend(self.problems.iter().cloned());
        if self.restart {
            parts.push("Restart Windows to finish setting up the drivers.".into());
        }
        parts.join("\n\n")
    }
    /// Setup's exit code: 1 when a step failed, 3010 when Windows must
    /// restart, else 0.
    pub fn exit_code(&self) -> i32 {
        if !self.problems.is_empty() {
            1
        } else if self.restart {
            3010
        } else {
            0
        }
    }
}

/// `--repair`, with the progress window unless `quiet`; returns setup's exit
/// code. Setup has already elevated itself, as for every other mode.
pub fn run_repair(quiet: bool) -> i32 {
    match ui::progress(
        crate::TITLE,
        "Repairing Rubylight",
        quiet,
        move |progress| repair(&progress),
    ) {
        Ok(outcome) => {
            for line_text in outcome.done.iter().chain(&outcome.problems) {
                line(format!("repair: {line_text}"));
            }
            if !quiet {
                ui::finished(
                    crate::TITLE,
                    outcome.heading(),
                    &outcome.text(),
                    outcome.problems.is_empty(),
                    None,
                );
            }
            outcome.exit_code()
        }
        Err(error) => crate::failed(quiet, "Rubylight could not be repaired", &error),
    }
}
/// Find the installation from its service and run the [`plan`].
fn repair(progress: &Progress) -> Result<Outcome> {
    let service = detect::service();
    let program = system::service_program(service)
        .context("Rubylight is not installed here; run the Rubylight installer")?;
    let install = system::win32_path(program.parent().context("the service has no folder")?)?;
    let profile = install::profile();
    // Read as an administrator below: verified and locked down first.
    install::prepare_profile(&profile, true)
        .context("checking the Rubylight settings folder before repairing")?;
    progress.set("Checking the Rubylight service…");
    let found = if !system::service_running(service) {
        Service::Stopped
    } else if install::wait_ready(install::probe(&profile), None).is_ok() {
        Service::Answers
    } else {
        Service::Silent
    };
    line(format!("repair: the service {service} is {found:?}"));
    let mut outcome = Outcome::default();
    for step in plan(found) {
        let done = match step {
            Step::StartService | Step::RestartService => {
                progress.set("Starting the Rubylight service…");
                // Ends a stop that hangs, or the service whose host is silent.
                system::stop_service(service)
                    .and_then(|()| start(service, &profile, &mut outcome.problems))
                    .map(|started| {
                        started.then(|| "The Rubylight service was started again.".to_owned())
                    })
            }
            Step::Drivers => {
                install::repair_drivers_in(&install, progress, RepairRun::Repair, &Driver::ALL).map(
                    |repair| {
                        outcome.restart |= repair.restart;
                        Some(repair.text())
                    },
                )
            }
            Step::Entries => {
                progress.set("Repairing the firewall rule and the Start menu shortcut…");
                let before = outcome.problems.len();
                install::refresh_entries(&install, &mut outcome.problems);
                Ok((outcome.problems.len() == before).then(|| {
                    "The Windows Firewall rule and the Start menu shortcut are in place.".to_owned()
                }))
            }
        };
        match done {
            Ok(done) => outcome.done.extend(done),
            Err(error) => outcome.problems.push(format!("{error:#}")),
        }
    }
    Ok(outcome)
}
/// Start the service unless it is disabled (a note says so) and wait for its
/// host to answer. Returns whether it was started.
fn start(service: &str, profile: &Path, problems: &mut Vec<String>) -> Result<bool> {
    if !system::start_service_unless_disabled(service)? {
        problems.push(crate::update::DISABLED_WARNING.into());
        return Ok(false);
    }
    install::wait_ready(install::probe(profile), None)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_repair_starts_a_stuck_service_before_it_repairs_the_drivers() {
        assert_eq!(
            plan(Service::Answers),
            [Step::Drivers, Step::Entries],
            "a service whose host answers is left running"
        );
        assert_eq!(
            plan(Service::Stopped),
            [Step::StartService, Step::Drivers, Step::Entries]
        );
        assert_eq!(
            plan(Service::Silent),
            [Step::RestartService, Step::Drivers, Step::Entries]
        );
    }
    #[test]
    fn the_result_says_what_was_done_and_what_failed() {
        let mut outcome = Outcome {
            restart: false,
            done: vec!["The virtual display and gamepad drivers were set up again.".into()],
            problems: Vec::new(),
        };
        assert_eq!(outcome.heading(), "Rubylight is repaired");
        assert_eq!(outcome.exit_code(), 0);
        outcome.restart = true;
        assert_eq!(outcome.exit_code(), 3010);
        assert!(
            outcome
                .text()
                .ends_with("Restart Windows to finish setting up the drivers.")
        );
        outcome
            .problems
            .push("The Windows Firewall rule could not be updated: denied".into());
        assert_eq!(outcome.exit_code(), 1);
        assert_eq!(
            outcome.heading(),
            "Rubylight could not be repaired completely"
        );
        let text = outcome.text();
        assert!(text.starts_with("The virtual display"), "{text}");
        assert!(text.contains("\n\nThe Windows Firewall rule"), "{text}");
    }
}
