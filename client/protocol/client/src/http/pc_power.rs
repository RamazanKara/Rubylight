//! Restart and shut down the PC (native `GET /power`, `POST /power/restart` and
//! `/power/shutdown`), and install the host's pending update (`GET /update`,
//! `POST /update/install`), offered while the host lists the features `power.restart`,
//! `power.shutdown` and `update`. The pinned native API types predate them, so they live here,
//! read as tolerantly as the host's own.
//!
//! Restart and Shut down spend a single-use token from `GET /power`, valid 30 s, as Sleep PC
//! does on `/power-actions/v1`. The host refuses while anything runs on the PC and says what,
//! naming the device that streams; the refusal's message is shown as it is.

use super::Error;
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const FEATURE_RESTART: &str = "power.restart";
pub const FEATURE_SHUTDOWN: &str = "power.shutdown";
pub const FEATURE_UPDATE: &str = "update";
/// A token older than this is asked for again before it is spent: the PC keeps it 30 s.
pub const POWER_TOKEN_REFRESH: Duration = Duration::from_secs(25);

/// What the client asks the PC to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PcPower {
    Restart,
    ShutDown,
}

impl PcPower {
    /// The route under `power/`.
    pub fn path(self) -> &'static str {
        match self {
            PcPower::Restart => "power/restart",
            PcPower::ShutDown => "power/shutdown",
        }
    }
    pub fn feature(self) -> &'static str {
        match self {
            PcPower::Restart => FEATURE_RESTART,
            PcPower::ShutDown => FEATURE_SHUTDOWN,
        }
    }
}

/// Whether one action can run now, and the host's words for why not.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PowerAvailability {
    #[serde(default)]
    pub available: bool,
    #[serde(default)]
    pub reason: Option<String>,
}

/// `GET /power`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PcPowerOffer {
    #[serde(default)]
    pub restart: PowerAvailability,
    #[serde(default)]
    pub shutdown: PowerAvailability,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub token_lifetime_s: u32,
    /// A wired wake target is ready; without it a PC that is off may not wake again.
    #[serde(default)]
    pub wake_from_off: bool,
    /// Why Windows refused the last accepted request.
    #[serde(default)]
    pub last_refusal: Option<String>,
}

impl PcPowerOffer {
    pub fn availability(&self, action: PcPower) -> &PowerAvailability {
        match action {
            PcPower::Restart => &self.restart,
            PcPower::ShutDown => &self.shutdown,
        }
    }

    /// The token to confirm `action` with, or the host's reason it can't run.
    pub fn token_for(&self, action: PcPower) -> Result<&str, String> {
        let availability = self.availability(action);
        match (&self.token, availability.available) {
            (Some(token), true) => Ok(token),
            _ => Err(availability.reason.clone().unwrap_or_default()),
        }
    }
}

#[derive(Serialize)]
pub(super) struct PowerRequest<'a> {
    pub token: &'a str,
}

/// `GET /update`, and the answer of an accepted `POST /update/install`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostUpdate {
    #[serde(default)]
    pub current_version: String,
    #[serde(default)]
    pub latest_version: Option<String>,
    /// `idle`, `waiting`, `downloading`, `ready`, `installing`, or a newer host's phase.
    #[serde(default)]
    pub phase: String,
    #[serde(default)]
    pub install_supported: bool,
    #[serde(default)]
    pub error: Option<String>,
    /// Why this device's install waits, in the host's words ("TV is streaming from this PC."):
    /// it never ends another device's stream. Older hosts leave it out.
    #[serde(default)]
    pub waiting_for: Option<String>,
}

impl HostUpdate {
    /// An update is queued, downloading or installing: nothing to offer again meanwhile.
    pub fn in_progress(&self) -> bool {
        matches!(self.phase.as_str(), "waiting" | "downloading" | "ready" | "installing")
    }

    /// The PC's updater failed, in its own words when it gave some.
    pub fn failure(&self) -> Option<Option<&str>> {
        (self.phase == "failed").then(|| self.error.as_deref().filter(|e| !e.is_empty()))
    }

    /// The version to offer, without a leading `v`, when there is one to install.
    pub fn offered(&self) -> Option<&str> {
        let latest = self.latest_version.as_deref()?.trim_start_matches('v');
        (self.install_supported && !latest.is_empty() && latest != self.current_version.trim_start_matches('v'))
            .then_some(latest)
    }
}

/// Whether `/serverinfo`'s version shows the update arrived: the PC is back on `target`.
pub fn update_arrived(running: &str, target: &str) -> bool {
    let running = running.trim().trim_start_matches('v');
    !running.is_empty() && running == target.trim_start_matches('v')
}

/// The host's own words for a refused power or update request, to show as they are; `None`
/// when the answer carries none, so the client words the error itself.
pub fn host_words(error: &Error) -> Option<String> {
    error.api_error().map(|api| api.message.clone()).filter(|m| !m.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_offer_reads_as_the_host_writes_it_and_ignores_newer_fields() {
        let offer: PcPowerOffer = serde_json::from_str(
            r#"{"restart":{"available":true,"reason":null},"shutdown":{"available":true,"reason":null},
                "token":"ab","token_lifetime_s":30,"wake_from_off":false,"last_refusal":null,"later":1}"#,
        )
        .unwrap();
        assert_eq!(offer.token_for(PcPower::Restart), Ok("ab"));
        assert!(!offer.wake_from_off);
        let busy: PcPowerOffer = serde_json::from_str(
            r#"{"restart":{"available":false,"reason":"TV is streaming from this PC."},
                "shutdown":{"available":false,"reason":"TV is streaming from this PC."},"token":null}"#,
        )
        .unwrap();
        assert_eq!(busy.token_for(PcPower::ShutDown), Err("TV is streaming from this PC.".into()));
        // A token without availability is not spent.
        let odd = PcPowerOffer { token: Some("ab".into()), ..Default::default() };
        assert!(odd.token_for(PcPower::Restart).is_err());
        assert_eq!((PcPower::Restart.path(), PcPower::ShutDown.feature()), ("power/restart", "power.shutdown"));
    }

    #[test]
    fn an_update_is_offered_only_when_newer_and_installable() {
        let update = |latest: Option<&str>, supported| HostUpdate {
            current_version: "3.3.0".into(),
            latest_version: latest.map(Into::into),
            phase: "idle".into(),
            install_supported: supported,
            ..Default::default()
        };
        assert_eq!(update(Some("v3.4.0"), true).offered(), Some("3.4.0"));
        assert_eq!(update(Some("v3.4.0"), false).offered(), None);
        assert_eq!(update(None, true).offered(), None);
        assert_eq!(update(Some("3.3.0"), true).offered(), None);
        assert!(update_arrived("3.4.0", "v3.4.0"));
        assert!(!update_arrived("3.3.0", "3.4.0"));
        assert!(!update_arrived("", "3.4.0"));
    }

    #[test]
    fn an_update_reads_its_phase_failure_and_wait() {
        let read: HostUpdate = serde_json::from_str(
            r#"{"current_version":"3.3.0","latest_version":"v3.4.0","phase":"ready","install_supported":true,
                "error":null,"waiting_for":"TV is streaming from this PC."}"#,
        )
        .unwrap();
        assert!(read.in_progress());
        assert_eq!(read.waiting_for.as_deref(), Some("TV is streaming from this PC."));
        assert_eq!(read.failure(), None);
        let phase = |phase: &str, error: Option<&str>| HostUpdate { phase: phase.into(), error: error.map(Into::into), ..Default::default() };
        assert!(!phase("idle", None).in_progress() && !phase("failed", None).in_progress());
        assert_eq!(phase("failed", Some("Installer verification failed")).failure(), Some(Some("Installer verification failed")));
        assert_eq!(phase("failed", None).failure(), Some(None));
        // An older host: no wait reported.
        let older: HostUpdate = serde_json::from_str(r#"{"current_version":"3.3.0","phase":"idle"}"#).unwrap();
        assert_eq!(older.waiting_for, None);
    }

    #[test]
    fn a_refusal_shows_the_hosts_words() {
        let body = r#"{"error":{"code":"host_in_use","message":"TV is streaming from this PC.","setting":null,"retry_after_ms":null}}"#;
        let error = Error::Api { status: 409, error: serde_json::from_str::<super::super::native_api::error::ErrorBody>(body).unwrap().error };
        assert_eq!(host_words(&error).as_deref(), Some("TV is streaming from this PC."));
        assert_eq!(host_words(&Error::NotFound("/x".into())), None);
    }
}
