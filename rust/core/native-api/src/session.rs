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

/// What a launcher (Steam, Epic, EA app, Ubisoft Connect) is showing in place of the game.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LauncherKind {
    /// An update, install or download is in the way.
    Update,
    /// The launcher wants a sign in, a verification code or a confirmation of the account.
    SignIn,
    /// A dialog the host cannot classify: a licence, a settings question, a dismissable
    /// notice. `window_title` names it.
    Prompt,
    /// A kind from a newer host: clients show it as `prompt`.
    #[serde(other)]
    Unknown,
}

/// The game was launched and a launcher window is up instead of it. The host never clicks
/// through: the person at the PC, or the client with the host's own remote input, does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LauncherNotice {
    /// `steam`, `epic`, `ea`, `ubisoft` or the launcher's id from the library.
    pub launcher: String,
    pub kind: LauncherKind,
    /// The launcher window's title, at most 120 characters.
    pub window_title: String,
    /// When the host first saw it.
    pub since_unix_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileState {
    /// The host is writing the settings.
    Applying,
    Applied,
    /// The settings were not applied; `message` says why. The game runs without them.
    Failed,
    #[serde(other)]
    Unknown,
}

/// The Radeon profile of the running game (`radeon-profile` in the app), as the host applied it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RadeonProfile {
    /// `competitive`, `smoothness`, `quality` or `custom`.
    pub profile: String,
    pub state: ProfileState,
    /// Why the profile failed, or what it left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// The setting ids the host wrote. They go back when the session ends.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub settings: Vec<String>,
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
    /// Present while a launcher shows a prompt or an update instead of the game; the owner
    /// only. Only when the host lists `session.launcher` in its features.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launcher: Option<LauncherNotice>,
    /// The Radeon profile of the app, while the session runs it; the owner only. Only when
    /// the host lists `radeon.profile` in its features.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radeon_profile: Option<RadeonProfile>,
    pub actions: Vec<Action>,
}
