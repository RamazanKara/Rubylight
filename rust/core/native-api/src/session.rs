//! `GET /session` and the session actions.
use serde::{Deserialize, Serialize};

use crate::error::ErrorCode;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Idle,
    Preparing,
    Launching,
    Streaming,
    Suspended,
    Stopping,
    /// A state from a newer host: clients treat it as `idle` with no actions.
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Owner {
    None,
    You,
    OtherDevice,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionId {
    Resume,
    Disconnect,
    End,
    CloseGame,
    /// An action from a newer host: clients hide it.
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Action {
    pub id: ActionId,
    pub allowed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<ErrorCode>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionApp {
    pub install_id: String,
    pub app_id: u32,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub revision: u64,
    pub state: State,
    pub owner: Owner,
    /// `null` for other devices and without View or Launch permission.
    pub app: Option<SessionApp>,
    pub game_running: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suspended_until_unix_ms: Option<u64>,
    pub actions: Vec<Action>,
}
