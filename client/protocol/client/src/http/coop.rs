//! Nucleus Co-op sessions (native `GET /coop` and `POST /coop/join`), offered while the host
//! lists the `coop` feature: [`Feature::Coop`](super::Feature::Coop). The pinned native API types
//! predate them, so they live here, read as tolerantly as the host's own.
//!
//! Joining seats the device and holds its slot for two minutes; the stream itself is the normal
//! launch of the session's `app_id`, whose refusals (Nucleus Co-op missing, no handler, busy)
//! come back from the launch as they do for any app.

use super::native_api::error::ErrorCode;
use super::Error;
use serde::{Deserialize, Serialize};

/// A co-op library entry: an app whose exe has a Nucleus Co-op handler.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoopGame {
    /// The entry's id in the app list; launching it starts a session or joins the running one.
    pub app_id: u32,
    /// `<app name> (Co-op)`.
    pub title: String,
    /// The handler's GUID.
    pub game: String,
    pub max_players: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoopPhase {
    /// Nucleus Co-op is starting the game instances.
    Starting,
    Running,
    Ended,
    /// A phase from a newer host.
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoopEndReason {
    Ended,
    Empty,
    Crashed,
    Failed,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoopEnd {
    pub reason: CoopEndReason,
    /// What stopped, for `crashed` and `failed`.
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoopPlayer {
    pub slot: u32,
    /// The device's name; `None` for a free slot.
    #[serde(default)]
    pub name: Option<String>,
    /// This device's own slot.
    #[serde(default)]
    pub you: bool,
    /// The device's stream is gone; it keeps the slot for a while.
    #[serde(default)]
    pub away: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoopSession {
    pub id: u32,
    /// The co-op entry to launch once joined.
    pub app_id: u32,
    pub title: String,
    #[serde(default)]
    pub game: String,
    pub phase: CoopPhase,
    #[serde(default)]
    pub end: Option<CoopEnd>,
    #[serde(default)]
    pub players: Vec<CoopPlayer>,
    #[serde(default)]
    pub free_slots: u32,
    /// A join by this device would succeed now.
    #[serde(default)]
    pub joinable: bool,
    /// Why this device cannot join, as the host words it; `None` when it can.
    #[serde(default)]
    pub refusal: Option<String>,
}

impl CoopSession {
    /// This device already holds a slot here (and may be away from it).
    pub fn seated(&self) -> bool {
        self.players.iter().any(|player| player.you)
    }
}

/// `GET /coop`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Coop {
    pub available: bool,
    /// Why co-op is not available; `None` when it is.
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub games: Vec<CoopGame>,
    /// Co-op sessions only: a background stream is private.
    #[serde(default)]
    pub sessions: Vec<CoopSession>,
}

#[derive(Serialize)]
pub(super) struct JoinRequest {
    pub session: u32,
}

/// `POST /coop/join`: the slot is held for this device until it launches `app_id`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoopJoined {
    pub session: u32,
    pub slot: u32,
    pub app_id: u32,
}

/// Why a co-op call failed, for the words the UI shows. Each keeps the host's message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CoopProblem {
    /// 403 `permission_required`: Join co-op is off for this device on the host.
    Permission(String),
    /// 409 `coop_refused`: full, ended, private, or this device was removed.
    Refused(String),
    /// 409 `no_session`: the session is gone.
    Gone(String),
    /// 503: the host cannot run co-op now (Nucleus Co-op missing, not the service, mode).
    Unavailable(String),
    /// 409 `session_busy`: another session holds Nucleus Co-op.
    Busy(String),
    /// Anything else, as the error reads.
    Other(String),
}

impl CoopProblem {
    pub fn of(error: &Error) -> Self {
        let message = error.to_string();
        let code = error.api_error().map(|api| api.code);
        match (error.http_status(), code) {
            (_, Some(ErrorCode::PermissionRequired)) | (Some(403), _) => Self::Permission(message),
            (_, Some(ErrorCode::NoSession)) => Self::Gone(message),
            (_, Some(ErrorCode::SessionBusy)) => Self::Busy(message),
            (_, Some(ErrorCode::TemporarilyUnavailable)) | (Some(503), _) => {
                Self::Unavailable(message)
            }
            // `coop_refused` is newer than the pinned codes: it reads as Unknown with a 409.
            (Some(409), _) => Self::Refused(message),
            _ => Self::Other(message),
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Self::Permission(m)
            | Self::Refused(m)
            | Self::Gone(m)
            | Self::Unavailable(m)
            | Self::Busy(m)
            | Self::Other(m) => m,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::native_api::error::ApiError;

    const LISTING: &str = r#"{"available":true,"reason":null,
        "games":[{"app_id":7,"title":"Portal 2 (Co-op)","game":"g","max_players":2}],
        "sessions":[{"id":3,"app_id":7,"title":"Portal 2","game":"g","phase":"paused",
          "end":{"reason":"melted","detail":null},
          "players":[{"slot":0,"name":"Fold7","you":true,"away":false,"extra":1},
                     {"slot":1,"name":null,"you":false,"away":false}],
          "free_slots":1,"joinable":true,"refusal":null,"newer":true}]}"#;

    #[test]
    fn a_listing_reads_back_and_tolerates_newer_values() {
        let coop: Coop = serde_json::from_str(LISTING).unwrap();
        assert_eq!(coop.games[0].app_id, 7);
        let session = &coop.sessions[0];
        assert_eq!(session.phase, CoopPhase::Unknown);
        assert_eq!(session.end.as_ref().unwrap().reason, CoopEndReason::Unknown);
        assert!(session.seated() && session.joinable);
        assert_eq!(session.players[1].name, None);
        // An unavailable host may leave the lists out.
        let off: Coop = serde_json::from_str(
            r#"{"available":false,"reason":"Nucleus Co-op is not installed"}"#,
        )
        .unwrap();
        assert!(off.sessions.is_empty() && off.games.is_empty());
        assert_eq!(
            off.reason.as_deref(),
            Some("Nucleus Co-op is not installed")
        );
    }

    #[test]
    fn the_join_body_is_the_session_alone() {
        assert_eq!(
            serde_json::to_string(&JoinRequest { session: 3 }).unwrap(),
            r#"{"session":3}"#
        );
        let joined: CoopJoined =
            serde_json::from_str(r#"{"session":3,"slot":1,"app_id":7}"#).unwrap();
        assert_eq!((joined.slot, joined.app_id), (1, 7));
    }

    fn api(status: u16, code: ErrorCode, message: &str) -> Error {
        Error::Api {
            status,
            error: ApiError {
                code,
                message: message.into(),
                setting: None,
                retry_after_ms: None,
            },
        }
    }

    #[test]
    fn every_refusal_keeps_the_hosts_words() {
        let permission = "Turn on Join co-op for this device in the Rubylight console.";
        assert_eq!(
            CoopProblem::of(&api(403, ErrorCode::PermissionRequired, permission)),
            CoopProblem::Permission(permission.into())
        );
        assert_eq!(
            CoopProblem::of(&api(409, ErrorCode::NoSession, "No such session")),
            CoopProblem::Gone("No such session".into())
        );
        assert_eq!(
            CoopProblem::of(&api(409, ErrorCode::Unknown, "The session is full")),
            CoopProblem::Refused("The session is full".into())
        );
        assert_eq!(
            CoopProblem::of(&api(
                409,
                ErrorCode::SessionBusy,
                "Nucleus Co-op runs another game"
            )),
            CoopProblem::Busy("Nucleus Co-op runs another game".into())
        );
        let missing = "Nucleus Co-op could not be installed: the download has another SHA-256";
        assert_eq!(
            CoopProblem::of(&api(503, ErrorCode::TemporarilyUnavailable, missing)).message(),
            missing
        );
        assert!(matches!(
            CoopProblem::of(&Error::Status {
                code: 503,
                message: "Service Unavailable".into()
            }),
            CoopProblem::Unavailable(_)
        ));
        assert!(matches!(
            CoopProblem::of(&Error::Cancelled),
            CoopProblem::Other(_)
        ));
    }
}
