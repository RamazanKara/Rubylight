//! Guests from other devices (native `GET /session/guests` and its POSTs, docs/multiclient.md in
//! the host repo): a paired device that joins the game another device plays waits for that
//! device to let it in, and the device playing manages its guests. Offered while the host lists
//! the feature `session.guests`. The pinned native API types predate them, so they live here.

use serde::{Deserialize, Serialize};

pub const FEATURE_SESSION_GUESTS: &str = "session.guests";

/// What a guest may do, as the wire spells it.
pub const GUEST_VIEW: &str = "view";
pub const GUEST_CONTROLLER: &str = "controller";
pub const GUEST_KEYBOARD_MOUSE: &str = "controller-keyboard-mouse";
pub const GUEST_PERMISSIONS: [&str; 3] = [GUEST_VIEW, GUEST_CONTROLLER, GUEST_KEYBOARD_MOUSE];

/// How often a client reads the list while it streams: the host's approval wait is 45 s.
pub const GUESTS_POLL_MS: u64 = 2_000;

/// The refusals and endings the host sends a guest, in its words. A client stops recovering a
/// stream that ended with one of [`GUEST_ENDINGS`]: the player chose it.
pub const KICKED: &str = "The player removed you from the game";
pub const REVOKED: &str = "The player does not let you join this game";
pub const EXPIRED: &str = "Your time as a guest in this game ended";
pub const GAME_OVER: &str = "The game you joined ended";
pub const GUEST_ENDINGS: [&str; 4] = [KICKED, REVOKED, EXPIRED, GAME_OVER];

/// `GET /session/guests`, and the answer of every action.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionGuests {
    /// The host's `guest_join`: `ask`, `allow` or `off`.
    #[serde(default)]
    pub policy: String,
    #[serde(default)]
    pub default_permission: String,
    /// Whether this device may answer joins and manage guests.
    #[serde(default)]
    pub manage: bool,
    /// This device's own guest id, when it is a guest of the running game.
    #[serde(default)]
    pub you: Option<u32>,
    /// The name of the device playing.
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub max_guests: u32,
    #[serde(default)]
    pub version: u64,
    /// The guest holding the keyboard and mouse; none while the device playing holds them.
    #[serde(default)]
    pub keyboard_mouse: Option<u32>,
    #[serde(default)]
    pub guests: Vec<SessionGuest>,
    #[serde(default)]
    pub waiting: Vec<GuestRequest>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionGuest {
    pub id: u32,
    pub name: String,
    pub permission: String,
    #[serde(default)]
    pub streaming: bool,
    #[serde(default)]
    pub since: u64,
    #[serde(default)]
    pub expires: Option<u64>,
}

/// A device waiting for an answer to its join.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuestRequest {
    pub id: u32,
    pub name: String,
    #[serde(default)]
    pub seconds_left: u64,
}

/// `POST /session/guests/approve`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuestApprove {
    pub id: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seconds: Option<u64>,
}

/// `POST /session/guests/deny`, `/kick` and `/revoke`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuestId {
    pub id: u32,
}

/// `POST /session/guests/permission`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuestPermissionChange {
    pub id: u32,
    pub permission: String,
}

/// `POST /session/guests/keyboard-mouse`: to guest `id`, or back to the device playing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuestKeyboardMouse {
    pub id: Option<u32>,
}

/// An action on a guest or a waiting device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GuestAction {
    Approve { id: u32, permission: Option<String> },
    Deny(u32),
    Kick(u32),
    Revoke(u32),
    Permission { id: u32, permission: String },
    KeyboardMouse(Option<u32>),
}

/// The Android bridge's form of an action: `{"action": "approve" | "deny" | "kick" | "revoke" |
/// "permission" | "keyboard_mouse", "id": n | null, "permission": "..."}`.
#[derive(Deserialize)]
struct ActionForm {
    action: String,
    #[serde(default)]
    id: Option<u32>,
    #[serde(default)]
    permission: Option<String>,
}

impl GuestAction {
    /// Reads the bridge's JSON form; a missing id, an unknown action or a permission the host
    /// would refuse never leaves this device.
    pub fn from_json(text: &str) -> Result<Self, String> {
        let form: ActionForm = serde_json::from_str(text).map_err(|e| format!("guest action: {e}"))?;
        let id = || {
            form.id
                .ok_or_else(|| format!("guest action {:?} needs an id", form.action))
        };
        let known = |permission: &Option<String>| match permission {
            Some(p) if !GUEST_PERMISSIONS.contains(&p.as_str()) => Err(format!("unknown guest permission: {p:?}")),
            _ => Ok(()),
        };
        known(&form.permission)?;
        Ok(match form.action.as_str() {
            "approve" => Self::Approve {
                id: id()?,
                permission: form.permission.clone(),
            },
            "deny" => Self::Deny(id()?),
            "kick" => Self::Kick(id()?),
            "revoke" => Self::Revoke(id()?),
            "permission" => Self::Permission {
                id: id()?,
                permission: form
                    .permission
                    .clone()
                    .ok_or("a permission change needs a permission")?,
            },
            "keyboard_mouse" => Self::KeyboardMouse(form.id),
            other => return Err(format!("unknown guest action: {other:?}")),
        })
    }

    /// The path under `/api/native/v1` and the body.
    pub fn request(&self) -> (&'static str, Vec<u8>) {
        let json = |value: serde_json::Result<Vec<u8>>| value.expect("guest actions serialize");
        match self {
            Self::Approve { id, permission } => (
                "session/guests/approve",
                json(serde_json::to_vec(&GuestApprove {
                    id: *id,
                    permission: permission.clone(),
                    seconds: None,
                })),
            ),
            Self::Deny(id) => ("session/guests/deny", json(serde_json::to_vec(&GuestId { id: *id }))),
            Self::Kick(id) => ("session/guests/kick", json(serde_json::to_vec(&GuestId { id: *id }))),
            Self::Revoke(id) => ("session/guests/revoke", json(serde_json::to_vec(&GuestId { id: *id }))),
            Self::Permission { id, permission } => (
                "session/guests/permission",
                json(serde_json::to_vec(&GuestPermissionChange {
                    id: *id,
                    permission: permission.clone(),
                })),
            ),
            Self::KeyboardMouse(id) => (
                "session/guests/keyboard-mouse",
                json(serde_json::to_vec(&GuestKeyboardMouse { id: *id })),
            ),
        }
    }
}

impl SessionGuests {
    /// The join requests this device should be asked about: only the device that manages them,
    /// and each request once (the caller keeps `answered`).
    pub fn new_requests<'a>(&'a self, answered: &'a [u32]) -> impl Iterator<Item = &'a GuestRequest> + 'a {
        self.waiting
            .iter()
            .filter(move |r| self.manage && !answered.contains(&r.id))
    }
}

/// Whether a stream that ended with `reason` (the host's `RustHostLastFailure`) ended because the
/// player or the game ended the guest's place: no recovery, no reconnect.
pub fn guest_ended(reason: &str) -> bool {
    let reason = reason.trim();
    GUEST_ENDINGS.iter().any(|ending| reason.starts_with(ending))
}

/// Whether a launch refusal is a join the player turned down or the host refused; the caller
/// shows the message as it is and does not retry.
pub fn join_refused(message: &str) -> bool {
    const REFUSALS: [&str; 5] = [
        "This game has as many guests as it takes",
        "The player did not let you join",
        "Nobody answered your request to join",
        "The game ended before you could join",
        REVOKED,
    ];
    let message = message.trim();
    REFUSALS.iter().any(|refusal| message.starts_with(refusal))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{"policy":"ask","default_permission":"controller","manage":true,"you":null,
        "owner":"Living room PC","max_guests":4,"version":7,"keyboard_mouse":2,
        "guests":[{"id":2,"name":"Phone","permission":"controller-keyboard-mouse","streaming":true,"since":10,"expires":null}],
        "waiting":[{"id":3,"name":"Tablet","seconds_left":40}]}"#;

    #[test]
    fn reads_the_hosts_shape() {
        let read: SessionGuests = serde_json::from_str(FIXTURE).unwrap();
        assert!(read.manage);
        assert_eq!(read.owner.as_deref(), Some("Living room PC"));
        assert_eq!(read.keyboard_mouse, Some(2));
        assert_eq!(read.guests[0].permission, GUEST_KEYBOARD_MOUSE);
        assert_eq!(read.waiting[0].seconds_left, 40);
        // A newer host's extra fields and an older one's missing ones both read.
        let sparse: SessionGuests = serde_json::from_str(r#"{"policy":"allow","extra":1}"#).unwrap();
        assert!(sparse.guests.is_empty() && !sparse.manage);
    }

    #[test]
    fn only_the_manager_is_asked_and_each_request_once() {
        let mut read: SessionGuests = serde_json::from_str(FIXTURE).unwrap();
        assert_eq!(read.new_requests(&[]).map(|r| r.id).collect::<Vec<_>>(), [3]);
        assert_eq!(read.new_requests(&[3]).count(), 0);
        read.manage = false;
        assert_eq!(read.new_requests(&[]).count(), 0);
    }

    #[test]
    fn actions_name_their_route_and_body() {
        let (path, body) = GuestAction::Approve {
            id: 3,
            permission: None,
        }
        .request();
        assert_eq!(path, "session/guests/approve");
        assert_eq!(String::from_utf8(body).unwrap(), r#"{"id":3}"#);
        let (path, body) = GuestAction::Permission {
            id: 2,
            permission: GUEST_VIEW.into(),
        }
        .request();
        assert_eq!(path, "session/guests/permission");
        assert_eq!(String::from_utf8(body).unwrap(), r#"{"id":2,"permission":"view"}"#);
        let (path, body) = GuestAction::KeyboardMouse(None).request();
        assert_eq!(path, "session/guests/keyboard-mouse");
        assert_eq!(String::from_utf8(body).unwrap(), r#"{"id":null}"#);
        assert_eq!(GuestAction::Kick(2).request().0, "session/guests/kick");
        assert_eq!(GuestAction::Revoke(2).request().0, "session/guests/revoke");
        assert_eq!(GuestAction::Deny(3).request().0, "session/guests/deny");
    }

    #[test]
    fn the_bridge_form_reads_and_bad_forms_stay_here() {
        assert_eq!(
            GuestAction::from_json(r#"{"action":"approve","id":3,"permission":"view"}"#),
            Ok(GuestAction::Approve {
                id: 3,
                permission: Some(GUEST_VIEW.into())
            })
        );
        assert_eq!(
            GuestAction::from_json(r#"{"action":"kick","id":2}"#),
            Ok(GuestAction::Kick(2))
        );
        assert_eq!(
            GuestAction::from_json(r#"{"action":"keyboard_mouse","id":null}"#),
            Ok(GuestAction::KeyboardMouse(None))
        );
        for bad in [
            r#"{"action":"kick"}"#,
            r#"{"action":"permission","id":2}"#,
            r#"{"action":"permission","id":2,"permission":"admin"}"#,
            r#"{"action":"promote","id":2}"#,
            "nonsense",
        ] {
            assert!(GuestAction::from_json(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_hosts_endings_stop_recovery_and_others_do_not() {
        assert!(guest_ended(KICKED));
        assert!(guest_ended(" The game you joined ended "));
        assert!(!guest_ended("Encoder failed"));
        assert!(!guest_ended(""));
        assert!(join_refused("The player did not let you join"));
        assert!(join_refused(REVOKED));
        assert!(!join_refused("Launch failed"));
    }
}
