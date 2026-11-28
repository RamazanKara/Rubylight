//! Serializable view models and action error types for [`super::DeviceGuests`].

use butterpollo_session::multiclient::{JoinPolicy, Permission};

/// Why a guest action failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionError {
    /// No such guest or waiting join.
    NotFound,
    /// A value the host does not take.
    Invalid,
}

/// The guests and the joins waiting, as the console and devices see them.
#[derive(Clone, Debug, serde::Serialize)]
pub struct View {
    #[serde(serialize_with = "policy_text")]
    pub policy: JoinPolicy,
    #[serde(serialize_with = "permission_text")]
    pub default_permission: Permission,
    /// The device whose game it is, when a game runs.
    pub owner: Option<String>,
    pub owner_name: Option<String>,
    pub max_guests: usize,
    pub version: u64,
    /// The guest holding the keyboard and mouse; none when the owner does.
    pub keyboard_mouse: Option<u32>,
    pub guests: Vec<GuestView>,
    pub waiting: Vec<WaitingView>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct GuestView {
    pub id: u32,
    /// The guest device's uuid, to tell a device which guest it is; never sent.
    #[serde(skip)]
    pub device: String,
    pub name: String,
    #[serde(serialize_with = "permission_text")]
    pub permission: Permission,
    pub streaming: bool,
    /// Unix seconds.
    pub since: u64,
    pub expires: Option<u64>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct WaitingView {
    pub id: u32,
    pub name: String,
    pub seconds_left: u64,
}

fn policy_text<S: serde::Serializer>(policy: &JoinPolicy, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(policy.as_str())
}

fn permission_text<S: serde::Serializer>(permission: &Permission, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(permission.as_str())
}
