//! Quick Suspend over the native API (`docs/native-api.md`, "Quick Suspend"): the running
//! game's paused state, and pausing or resuming it, while the host lists
//! `session.quick_suspend` ([`crate::http::Feature::QuickSuspend`]). A refusal is an answer, not
//! an error: `state` `refused` with the host's `reason` and `message`. A stream asks on the
//! control stream instead (0x554B, [`crate::control::ClientMessage::QuickSuspend`]).

use super::native::{ACTION_READ_TIMEOUT, EMPTY_BODY, MAX_JSON_BYTES};
use super::native_api::quick_suspend::QuickSuspendStatus;
use super::{Error, HostClient, READ_TIMEOUT};

impl HostClient {
    /// `GET /session/suspend`: whether the running game can be paused, is paused, and by whom.
    /// Needs View or Launch.
    pub fn quick_suspend(&self) -> Result<QuickSuspendStatus, Error> {
        self.native("session/suspend", None, MAX_JSON_BYTES, READ_TIMEOUT)
    }

    /// `POST /session/suspend`: pauses the running game's processes. Needs Launch or Host
    /// control, and the session unless the device has Host control.
    pub fn suspend_game(&self) -> Result<QuickSuspendStatus, Error> {
        self.native(
            "session/suspend",
            Some(EMPTY_BODY),
            MAX_JSON_BYTES,
            ACTION_READ_TIMEOUT,
        )
    }

    /// `POST /session/resume`: resumes the paused game, from this or another device.
    pub fn resume_game(&self) -> Result<QuickSuspendStatus, Error> {
        self.native(
            "session/resume",
            Some(EMPTY_BODY),
            MAX_JSON_BYTES,
            ACTION_READ_TIMEOUT,
        )
    }
}
