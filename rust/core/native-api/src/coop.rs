//! `GET /coop` and `POST /coop/join`: Nucleus Co-op sessions a device can
//! join, while the host lists [`crate::feature::COOP`].
use serde::{Deserialize, Serialize};

/// A co-op library entry: an app whose exe has a Nucleus Co-op handler.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Game {
    /// The entry's id in `/applist` and `/library`; `/launch` it to start a
    /// session or join the running one.
    pub app_id: u32,
    /// `Co-op (Nucleus): <app name>`.
    pub title: String,
    /// The handler's GUID.
    pub game: String,
    pub max_players: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Nucleus Co-op is starting the instances.
    Starting,
    Running,
    Ended,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    /// Ended from the console or by the host.
    Ended,
    /// Every player left and none returned in time.
    Empty,
    /// An instance or Nucleus Co-op exited while the session ran.
    Crashed,
    /// Nucleus Co-op did not start the game.
    Failed,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct End {
    pub reason: EndReason,
    /// What stopped, for `crashed` and `failed`.
    pub detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Player {
    pub slot: u32,
    /// The device's name; `null` for a free slot.
    pub name: Option<String>,
    /// The caller's own slot.
    pub you: bool,
    /// The device's stream is gone; it keeps the slot for a while.
    pub away: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub id: u32,
    /// The co-op entry the session runs.
    pub app_id: u32,
    pub title: String,
    pub game: String,
    pub phase: Phase,
    /// Always present: `null` until the session ended.
    pub end: Option<End>,
    pub players: Vec<Player>,
    pub free_slots: u32,
    /// A join by the caller would succeed now.
    pub joinable: bool,
    /// Why the caller cannot join, as the client can show it; `null` when
    /// `joinable`.
    pub refusal: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Coop {
    pub available: bool,
    /// Why co-op is not available; `null` when it is.
    pub reason: Option<String>,
    #[serde(default)]
    pub games: Vec<Game>,
    /// Co-op sessions only: a background stream is private.
    #[serde(default)]
    pub sessions: Vec<Session>,
}

/// The body of `POST /coop/join`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JoinRequest {
    pub session: u32,
}

/// The answer to `POST /coop/join`: the slot is held for the device until it
/// launches `app_id`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Joined {
    pub session: u32,
    pub slot: u32,
    pub app_id: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_listing_reads_back_and_tolerates_newer_values() {
        let json = r#"{"available":true,"reason":null,
            "games":[{"app_id":7,"title":"Co-op (Nucleus): Portal 2","game":"g","max_players":2}],
            "sessions":[{"id":3,"app_id":7,"title":"Portal 2","game":"g","phase":"paused",
              "end":{"reason":"melted","detail":null},
              "players":[{"slot":0,"name":"Fold7","you":true,"away":false,"extra":1}],
              "free_slots":1,"joinable":true,"refusal":null}]}"#;
        let coop: Coop = serde_json::from_str(json).unwrap();
        assert_eq!(coop.games[0].app_id, 7);
        assert_eq!(coop.sessions[0].phase, Phase::Unknown);
        assert_eq!(
            coop.sessions[0].end.as_ref().unwrap().reason,
            EndReason::Unknown
        );
        assert!(coop.sessions[0].players[0].you);
    }

    #[test]
    fn a_join_request_refuses_unknown_fields() {
        let ok: JoinRequest = serde_json::from_str(r#"{"session":3}"#).unwrap();
        assert_eq!(ok.session, 3);
        assert!(serde_json::from_str::<JoinRequest>(r#"{"session":3,"slot":1}"#).is_err());
    }
}
