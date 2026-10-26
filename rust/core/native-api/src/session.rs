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

/// `POST /session/couch-menu`: open or close the menu the host draws into the caller's
/// stream. Idempotent. Only when the host lists `session.couch_menu` in its features.
/// A request: unknown fields are rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CouchMenuRequest {
    pub open: bool,
}

/// The reply to [`CouchMenuRequest`]: whether the menu is open now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CouchMenu {
    pub open: bool,
}

#[cfg(test)]
mod tests {
    use super::{CouchMenu, CouchMenuRequest};
    use crate::capabilities::Capabilities;
    use crate::feature;

    #[test]
    fn couch_menu_request_round_trips_and_stays_strict() {
        let request: CouchMenuRequest = serde_json::from_str(r#"{"open":true}"#).unwrap();
        assert_eq!(request, CouchMenuRequest { open: true });
        let text = serde_json::to_string(&CouchMenuRequest { open: false }).unwrap();
        assert_eq!(text, r#"{"open":false}"#);
        for bad in [r#"{"open":true,"page":"radeon"}"#, "{}", r#"{"open":"yes"}"#] {
            assert!(serde_json::from_str::<CouchMenuRequest>(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn couch_menu_reply_round_trips_and_ignores_new_fields() {
        let reply: CouchMenu = serde_json::from_str(r#"{"open":false}"#).unwrap();
        assert_eq!(reply, CouchMenu { open: false });
        let text = serde_json::to_string(&CouchMenu { open: true }).unwrap();
        assert_eq!(text, r#"{"open":true}"#);
        let newer: CouchMenu = serde_json::from_str(r#"{"open":true,"page":"main"}"#).unwrap();
        assert!(newer.open);
    }

    #[test]
    fn capabilities_of_a_1_0_host_still_parse_without_the_feature() {
        let old = r#"{"api_version":1,"host":{"name":"PC","version":"2.6.0","unique_id":"X"},
            "features":["library","session","session.close_game","radeon.read","radeon.write"],
            "permissions":{"list":true,"view":true,"launch":true,"host_control":false}}"#;
        let capabilities: Capabilities = serde_json::from_str(old).unwrap();
        assert!(!capabilities.features.iter().any(|f| f == feature::COUCH_MENU));
        assert_eq!(feature::COUCH_MENU, "session.couch_menu");
    }
}
