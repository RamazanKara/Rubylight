//! Waking a PC, putting it to sleep, shutting it down or restarting it, and installing its update.
//!
//! Core token/state-machine logic lives in `nightjar_core::hosts::power` and
//! `nightjar_policy::hosts::power`; this module translates typed results into Desktop UI events and
//! localized strings (`en`/`trf`) cleanly.

use super::*;

pub use nightjar_core::hosts::power::{
    Handed, PcPowerFailure, PcPowerHost, PowerHost, SleepFailure, UpdateHost, pc_power_refusal,
};
use nightjar_policy::hosts::{
    PcPowerStepAction, SleepUnavailableReason, UPDATE_MAX_WAIT_MS, UPDATE_POLL_MS, UPDATE_WAIT_MS,
    UpdateStepAction,
};

pub enum SleepStep {
    /// The PC offers to sleep: confirm, then spend `token`, which the PC handed out at `at`.
    Offer { token: String, at: Handed },
    /// The PC took the request (202). It does not confirm that the PC suspended.
    Accepted,
}

pub enum PcPowerStep {
    /// The PC offers it: confirm, then spend `token`. `wake_from_off`: a wired wake target is
    /// ready, so the PC can likely be woken after a shutdown.
    Offer { token: String, at: Handed, wake_from_off: bool },
    /// The PC took the request (202).
    Accepted,
    /// After the 202, Windows did not confirm it in time: the PC may still go down. The host's words.
    Unconfirmed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateStep {
    /// The host is installing `version`.
    Started(String),
    /// The install waits for another device's stream, in the host's words.
    Waiting(String),
    /// The PC answers with `version` again.
    Back(String),
}

impl SleepStep {
    pub(crate) fn into_ui(self, handed: &mut HashMap<u64, Handed>, ask: u64) -> crate::hosts_model::SleepStep {
        match self {
            Self::Offer { token, at } => {
                handed.insert(ask, at);
                crate::hosts_model::SleepStep::Offer {
                    token,
                    at: crate::hosts_model::Handed(at.at),
                }
            }
            Self::Accepted => crate::hosts_model::SleepStep::Accepted,
        }
    }
}

impl PcPowerStep {
    pub(crate) fn into_ui(
        self,
        handed: &mut HashMap<u64, Handed>,
        ask: u64,
    ) -> crate::hosts_model::PcPowerStep {
        match self {
            Self::Offer {
                token,
                at,
                wake_from_off,
            } => {
                handed.insert(ask, at);
                crate::hosts_model::PcPowerStep::Offer {
                    token,
                    at: crate::hosts_model::Handed(at.at),
                    wake_from_off,
                }
            }
            Self::Accepted => crate::hosts_model::PcPowerStep::Accepted,
            Self::Unconfirmed(s) => crate::hosts_model::PcPowerStep::Unconfirmed(s),
        }
    }
}

impl UpdateStep {
    pub(crate) fn into_ui(self) -> crate::hosts_model::UpdateStep {
        match self {
            Self::Started(v) => crate::hosts_model::UpdateStep::Started(v),
            Self::Waiting(r) => crate::hosts_model::UpdateStep::Waiting(r),
            Self::Back(v) => crate::hosts_model::UpdateStep::Back(v),
        }
    }
}

const UPDATE_WAIT: Duration = Duration::from_millis(UPDATE_WAIT_MS);
pub(super) const UPDATE_MAX_WAIT: Duration = Duration::from_millis(UPDATE_MAX_WAIT_MS);
const UPDATE_POLL: Duration = Duration::from_millis(UPDATE_POLL_MS);
const POWER_REFUSAL_POLL: Duration =
    Duration::from_millis(nightjar_policy::hosts::PC_POWER_REFUSAL_POLL_MS);
const POWER_REFUSAL_READS: u32 = nightjar_policy::hosts::PC_POWER_REFUSAL_READS as u32;
pub(super) const UPDATE_GONE: &str = en("The PC has no update to install.");
pub(super) const UPDATE_FAILED: &str = en("The update failed on the PC.");

pub(super) fn update_target(update: &HostUpdate) -> Result<String, String> {
    nightjar_core::hosts::power::update_target(update).map_err(|_| UPDATE_GONE.to_owned())
}

pub(super) fn follow_update(
    host: &impl UpdateHost,
    target: &str,
    polls: u32,
    most: u32,
    pause: impl Fn(),
    mut said: impl FnMut(UpdateStep),
) -> Result<UpdateStep, String> {
    match nightjar_core::hosts::power::follow_update(host, target, polls, most, pause, |reason| {
        said(UpdateStep::Waiting(reason));
    }) {
        UpdateStepAction::Back { version } => Ok(UpdateStep::Back(version)),
        UpdateStepAction::UpdateFailed { error } => {
            Err(error.unwrap_or_else(|| UPDATE_FAILED.to_owned()))
        }
        UpdateStepAction::CameBackOn {
            current_version,
            error,
        } => {
            let back = trf(
                "The PC came back on version {}; the update did not install.",
                &[&current_version],
            );
            Err(error.map_or(back.clone(), |e| format!("{back} {e}")))
        }
        UpdateStepAction::TimedOut { target } => Err(trf(
            "The PC did not come back with version {} in time.",
            &[&target],
        )),
        UpdateStepAction::Continue | UpdateStepAction::Waiting { .. } => {
            unreachable!("follow_update returns only terminal actions")
        }
    }
}

/// The PC's own words (with its title, next step and technical detail when it sent them), else
/// what failed.
pub(super) fn native_error(error: &nightjar_core::http::Error) -> String {
    match error.api_error().filter(|api| !api.message.is_empty()) {
        Some(api) => nightjar_desktop_config::host_error::host_text(api),
        None => describe(error),
    }
}

fn format_pc_power_failure(err: PcPowerFailure) -> String {
    match err {
        PcPowerFailure::Unavailable(reason) if reason.is_empty() => PC_POWER_BUSY.into(),
        PcPowerFailure::Unavailable(reason) => reason,
        PcPowerFailure::Transport(e) => native_error(&e),
    }
}

pub(super) fn pc_power_token(host: &impl PcPowerHost, action: PcPower) -> Result<PcPowerStep, String> {
    nightjar_core::hosts::power::pc_power_token(host, action)
        .map(|(token, at, wake_from_off)| PcPowerStep::Offer {
            token,
            at,
            wake_from_off,
        })
        .map_err(format_pc_power_failure)
}

pub(super) fn pc_power_confirmed(
    host: &impl PcPowerHost,
    action: PcPower,
    token: String,
    at: Handed,
) -> Result<PcPowerStep, String> {
    match nightjar_core::hosts::power::pc_power_confirmed(host, action, token, at) {
        Ok(PcPowerStepAction::Accepted) | Ok(_) => Ok(PcPowerStep::Accepted),
        Err(e) => Err(format_pc_power_failure(e)),
    }
}

fn format_sleep_failure(err: SleepFailure) -> String {
    match err {
        SleepFailure::Unavailable(SleepUnavailableReason::Unsupported) => SLEEP_UNSUPPORTED.into(),
        SleepFailure::Unavailable(SleepUnavailableReason::NotAllowed) => SLEEP_NOT_ALLOWED.into(),
        SleepFailure::Unavailable(SleepUnavailableReason::Busy) => SLEEP_BUSY.into(),
        SleepFailure::Refused { message, .. } if !message.is_empty() => message,
        SleepFailure::Refused { status, .. } => trf("The PC refused with status {}.", &[&status]),
        SleepFailure::Transport(e) => describe(&e),
    }
}

pub(super) fn sleep_token(host: &impl PowerHost) -> Result<(String, Handed), String> {
    nightjar_core::hosts::power::sleep_token(host).map_err(format_sleep_failure)
}

pub(super) fn sleep_confirmed(
    host: &impl PowerHost,
    token: String,
    at: Handed,
) -> Result<SleepStep, String> {
    nightjar_core::hosts::power::sleep_confirmed(host, token, at)
        .map(|()| SleepStep::Accepted)
        .map_err(format_sleep_failure)
}

impl Hosts {
    pub fn wake(&self, id: u64, run: u64) {
        let Some(host) = self.get(id) else { return };
        let record = host.record.clone();
        self.spawn("wake", move || {
            Some(Event::Wake {
                id,
                run,
                result: crate::wol::wake(&record),
            })
        });
    }

    pub fn sleep_offer(&self, id: u64, ask: u64) {
        self.sleep_step(id, ask, false, |client| {
            sleep_token(client).map(|(token, at)| SleepStep::Offer { token, at })
        });
    }

    pub fn sleep(&self, id: u64, ask: u64, token: String, at: Handed) {
        self.sleep_step(id, ask, true, move |client| sleep_confirmed(client, token, at));
    }

    fn sleep_step(
        &self,
        id: u64,
        ask: u64,
        confirmed: bool,
        work: impl FnOnce(&HostClient) -> Result<SleepStep, String> + Send + 'static,
    ) {
        let Some(host) = self.get(id) else { return };
        let record = host.connect_record();
        let name = host.display_name().to_owned();
        let identity = self.identity.clone();
        self.spawn("sleep", move || {
            let result = identity
                .ok_or_else(|| {
                    en("Nightjar has no client identity yet. Restart Nightjar and try again.")
                        .to_owned()
                })
                .and_then(|identity| client_for(&identity, &record, &record.address))
                .and_then(|client| work(&client));
            Some(Event::Sleep {
                id,
                ask,
                name,
                confirmed,
                result,
            })
        });
    }

    pub fn pc_power_offer(&self, id: u64, ask: u64, action: PcPower) {
        self.pc_power_step(id, ask, action, false, move |client| pc_power_token(client, action));
    }

    pub fn pc_power(&self, id: u64, ask: u64, action: PcPower, token: String, at: Handed) {
        self.pc_power_step(id, ask, action, true, move |client| {
            pc_power_confirmed(client, action, token, at)
        });
    }

    fn pc_power_step(
        &self,
        id: u64,
        ask: u64,
        action: PcPower,
        confirmed: bool,
        work: impl FnOnce(&HostClient) -> Result<PcPowerStep, String> + Send + 'static,
    ) {
        let Some(host) = self.get(id) else { return };
        let record = host.connect_record();
        let name = host.display_name().to_owned();
        let identity = self.identity.clone();
        let sender = self.sender.clone();
        let waker = self.waker.clone();
        self.spawn("pc-power", move || {
            let Some(identity) = identity else {
                return Some(Event::PcPower {
                    id,
                    ask,
                    name,
                    action,
                    confirmed,
                    result: Err(en(
                        "Nightjar has no client identity yet. Restart Nightjar and try again.",
                    )
                    .to_owned()),
                });
            };
            let client = match client_for(&identity, &record, &record.address) {
                Ok(client) => client,
                Err(e) => {
                    return Some(Event::PcPower {
                        id,
                        ask,
                        name,
                        action,
                        confirmed,
                        result: Err(e),
                    });
                }
            };
            let result = work(&client);
            let follow = confirmed && matches!(result, Ok(PcPowerStep::Accepted));
            let _ = sender.send(Event::PcPower {
                id,
                ask,
                name: name.clone(),
                action,
                confirmed,
                result,
            });
            waker();
            if follow && let Some(refusal) = pc_power_refusal(&*client, POWER_REFUSAL_READS, || {
                std::thread::sleep(POWER_REFUSAL_POLL);
            }) {
                let message = match refusal {
                    LateRefusal::Refused(m) if !m.is_empty() => Err(m),
                    LateRefusal::Refused(_) => Err(PC_POWER_BUSY.into()),
                    LateRefusal::Unconfirmed(m) => Ok(PcPowerStep::Unconfirmed(m)),
                };
                return Some(Event::PcPower {
                    id,
                    ask,
                    name,
                    action,
                    confirmed: true,
                    result: message,
                });
            }
            None
        });
    }

    pub fn install_update(&self, id: u64) {
        let Some(host) = self.get(id) else { return };
        let record = host.connect_record();
        let name = host.display_name().to_owned();
        let Some(identity) = self.identity.clone() else {
            return;
        };
        let sender = self.sender.clone();
        let waker = self.waker.clone();
        self.spawn("host-update", move || {
            let client = match client_for(&identity, &record, &record.address) {
                Ok(client) => client,
                Err(e) => {
                    return Some(Event::HostUpdate {
                        id,
                        name,
                        result: Err(e),
                    });
                }
            };
            let target = match client
                .install_host_update()
                .map_err(|e| native_error(&e))
                .and_then(|u| update_target(&u))
            {
                Ok(target) => target,
                Err(e) => {
                    return Some(Event::HostUpdate {
                        id,
                        name,
                        result: Err(e),
                    });
                }
            };
            let _ = sender.send(Event::HostUpdate {
                id,
                name: name.clone(),
                result: Ok(UpdateStep::Started(target.clone())),
            });
            waker();
            let polls = (UPDATE_WAIT.as_secs() / UPDATE_POLL.as_secs()) as u32;
            let most = (UPDATE_MAX_WAIT.as_secs() / UPDATE_POLL.as_secs()) as u32;
            let result = follow_update(
                &*client,
                &target,
                polls,
                most,
                || std::thread::sleep(UPDATE_POLL),
                |step| {
                    let _ = sender.send(Event::HostUpdate {
                        id,
                        name: name.clone(),
                        result: Ok(step),
                    });
                    waker();
                },
            );
            Some(Event::HostUpdate { id, name, result })
        });
    }
}
