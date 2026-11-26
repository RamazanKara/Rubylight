//! `GET /session/suspend`, `POST /session/suspend` and `POST /session/resume`:
//! pause the running game's processes on the host and resume them, while the
//! host lists [`crate::feature::QUICK_SUSPEND`]. The same state travels on the
//! control stream as message `0x554B` (`rubylight_protocol::quick_suspend`).
use serde::{Deserialize, Serialize};

/// The game's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuspendState {
    /// Quick Suspend is off, or nothing runs that it could pause; `reason`
    /// says which.
    Unavailable,
    /// The game runs and can be suspended.
    Ready,
    /// The game's processes are suspended.
    Suspended,
    /// A resume is under way.
    Resuming,
    /// Only in a POST's answer: the request was refused, `reason` and
    /// `message` say why, and the game's state did not change.
    Refused,
    /// A state from a newer host.
    #[serde(other)]
    Unknown,
}

/// `GET /session/suspend`, and the answer of both POSTs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickSuspendStatus {
    pub state: SuspendState,
    /// The stable name of why it is unavailable or refused (`none`,
    /// `disabled_for_app`, `no_owned_process`, `anti_cheat_detected`,
    /// `protected_process`, `denylisted_process`, `guests_active`,
    /// `coop_session`, `not_owner`, `session_busy`, `no_session`,
    /// `io_or_os_error`, or one from a newer host).
    pub reason: String,
    /// Plain English the client can show as it is; null with `none`.
    pub message: Option<String>,
    /// The running or suspended game, null without one.
    pub app_title: Option<String>,
    /// How many processes are suspended (0 unless suspended).
    pub suspended_count: u32,
    /// When the suspend started (Unix ms), null unless suspended.
    pub suspended_since_unix_ms: Option<u64>,
    /// The device that suspended it, null unless suspended.
    pub suspended_by: Option<String>,
}

/// The body of `POST /session/suspend` and `POST /session/resume`: empty, an
/// unknown field is refused.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuickSuspendRequest {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_state_from_a_newer_host_still_parses() {
        let status: QuickSuspendStatus = serde_json::from_str(
            r#"{"state":"hibernated","reason":"later","message":null,"app_title":null,
            "suspended_count":0,"suspended_since_unix_ms":null,"suspended_by":null,"extra":1}"#,
        )
        .unwrap();
        assert_eq!(status.state, SuspendState::Unknown);
        assert!(serde_json::from_str::<QuickSuspendRequest>(r#"{"disconnect":true}"#).is_err());
        assert!(serde_json::from_str::<QuickSuspendRequest>("{}").is_ok());
    }
}
