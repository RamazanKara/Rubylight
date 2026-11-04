//! `GET /power`, `POST /power/restart` and `POST /power/shutdown`: restart or
//! shut down the PC, while the host lists [`crate::feature::POWER_RESTART`]
//! and [`crate::feature::POWER_SHUTDOWN`]. `GET /update` and
//! `POST /update/install`: install the host's pending update, while it lists
//! [`crate::feature::UPDATE`].
use serde::{Deserialize, Serialize};

/// Whether one action can run now, and why not.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Availability {
    pub available: bool,
    /// Plain English the client can show as it is; null when available.
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Power {
    pub restart: Availability,
    pub shutdown: Availability,
    /// A single-use token for either POST, null when neither is available.
    pub token: Option<String>,
    pub token_lifetime_s: u32,
    /// A wired wake target is ready, so a magic packet can likely wake the PC
    /// after a shutdown. Firmware settings Windows can't read may still stop it.
    pub wake_from_off: bool,
    /// Why this device's last accepted restart or shutdown did not happen
    /// (Windows refused, or the PC got busy before it), null when it did or
    /// after 10 minutes. Another device's refusal is never shown.
    pub last_refusal: Option<String>,
}

/// The body of `POST /power/restart` and `POST /power/shutdown`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PowerRequest {
    pub token: String,
}

/// The answer of an accepted (202) restart or shutdown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PowerAccepted {
    /// `restart` or `shutdown`.
    pub action: String,
}

/// `GET /update`, and the answer of an accepted (202) `POST /update/install`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Update {
    /// The running host's version.
    pub current_version: String,
    /// The newest release the last check found, null before a check or when
    /// this version is the newest.
    pub latest_version: Option<String>,
    /// `idle`, `waiting`, `downloading`, `ready`, `installing`, or a phase
    /// from a newer host.
    pub phase: String,
    /// The host can install updates itself (an installed Windows service).
    pub install_supported: bool,
    /// Why the last check or install failed, null when it did not.
    pub error: Option<String>,
    /// Why a device's install waits, for example
    /// `"Living room TV is streaming from this PC."`: it never ends another
    /// device's stream. Null when it does not wait for one; older hosts
    /// leave it out.
    #[serde(default)]
    pub waiting_for: Option<String>,
    /// The queued update is an Install now rather than one that waits for
    /// an idle host. Older hosts leave it out.
    #[serde(default)]
    pub install_now: bool,
}
