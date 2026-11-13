//! `GET /session/guests` and its actions (`docs/native-api.md`, "Guests"): the device playing,
//! or one with Host control, answers joins and manages the running game's guests from its own
//! screen. The state is `crate::multiclient`'s.
use super::{Failure, Reply, caller, json, read_body, require_view_or_launch};
use crate::{
    multiclient::{ActionError, View},
    state::Shared,
    tls::Connection,
};
use axum::{
    Extension,
    extract::{Request, State},
    http::StatusCode,
};
use butterpollo_core::{
    multiclient::{MAX_LIFETIME, MIN_LIFETIME, Permission},
    native_api::{
        error::ErrorCode,
        guests::{
            GuestApprove, GuestId, GuestKeyboardMouse, GuestPermission, JoinRequest, SessionGuest,
            SessionGuests,
        },
    },
    state::{Client, device_uuid},
};
use serde::de::DeserializeOwned;
use std::time::Duration;

/// The list as `client` sees it.
pub(crate) fn project(view: View, client: &Client, manage: bool) -> SessionGuests {
    let device = device_uuid(&client.uuid);
    let you = view
        .guests
        .iter()
        .find(|g| g.device == device)
        .map(|g| g.id);
    SessionGuests {
        policy: view.policy.as_str().into(),
        default_permission: view.default_permission.as_str().into(),
        manage,
        you,
        owner: view.owner_name,
        max_guests: u32::try_from(view.max_guests).unwrap_or(u32::MAX),
        version: view.version,
        keyboard_mouse: view.keyboard_mouse,
        guests: view
            .guests
            .into_iter()
            .map(|g| SessionGuest {
                id: g.id,
                name: g.name,
                permission: g.permission.as_str().into(),
                streaming: g.streaming,
                since: g.since,
                expires: g.expires,
            })
            .collect(),
        waiting: view
            .waiting
            .into_iter()
            .map(|r| JoinRequest {
                id: r.id,
                name: r.name,
                seconds_left: r.seconds_left,
            })
            .collect(),
    }
}

fn answer(h: &Shared, client: &Client) -> Reply {
    let manage = h.device_guests.may_manage(h, client);
    Ok(json(
        StatusCode::OK,
        &project(h.device_guests.view(h), client, manage),
    ))
}

pub async fn list(State(h): State<Shared>, Extension(c): Extension<Connection>) -> Reply {
    let client = caller(&h, &c)?;
    require_view_or_launch(&client)?;
    answer(&h, &client)
}

/// The caller, when it may manage guests, and the body.
async fn manager<T: DeserializeOwned>(
    h: &Shared,
    c: &Connection,
    request: Request,
) -> Result<(Client, T), Failure> {
    let client = caller(h, c)?;
    require_view_or_launch(&client)?;
    if !h.device_guests.may_manage(h, &client) {
        return Err(Failure::new(
            ErrorCode::NotSessionOwner,
            "Only the device playing, or one with Host control, manages guests.",
        ));
    }
    let body = serde_json::from_slice(&read_body(request).await?).map_err(|error| {
        Failure::new(ErrorCode::InvalidRequest, format!("Invalid body: {error}."))
    })?;
    Ok((client, body))
}

fn permission(text: &str) -> Result<Permission, Failure> {
    Permission::parse(text).ok_or_else(|| {
        Failure::new(
            ErrorCode::InvalidValue,
            "A guest's permission is view, controller or controller-keyboard-mouse.",
        )
    })
}

fn failed(error: ActionError) -> Failure {
    match error {
        ActionError::NotFound => Failure::new(
            ErrorCode::NotFound,
            "That guest or request is no longer there.",
        ),
        ActionError::Invalid => Failure::new(
            ErrorCode::InvalidValue,
            "The host does not take that for this guest.",
        ),
    }
}

pub async fn approve(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    let (client, body): (_, GuestApprove) = manager(&h, &c, request).await?;
    let permission = match &body.permission {
        Some(text) => permission(text)?,
        None => butterpollo_core::multiclient::default_permission(&h.config_snapshot()),
    };
    let lifetime = match body.seconds {
        Some(seconds) => {
            let lifetime = Duration::from_secs(seconds);
            if !(MIN_LIFETIME..=MAX_LIFETIME).contains(&lifetime) {
                return Err(Failure::new(
                    ErrorCode::InvalidValue,
                    "A guest's time is 5 minutes to 7 days, or until the game ends.",
                ));
            }
            Some(lifetime)
        }
        None => None,
    };
    h.device_guests
        .approve(body.id, permission, lifetime)
        .map_err(failed)?;
    tracing::info!(by = %client.name, request = body.id, "join approved from a device");
    answer(&h, &client)
}

pub async fn deny(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    let (client, body): (_, GuestId) = manager(&h, &c, request).await?;
    h.device_guests.deny(body.id).map_err(failed)?;
    answer(&h, &client)
}

pub async fn kick(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    let (client, body): (_, GuestId) = manager(&h, &c, request).await?;
    h.device_guests.kick(&h, body.id).map_err(failed)?;
    answer(&h, &client)
}

pub async fn revoke(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    let (client, body): (_, GuestId) = manager(&h, &c, request).await?;
    h.device_guests.revoke(&h, body.id).map_err(failed)?;
    answer(&h, &client)
}

pub async fn set_permission(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    let (client, body): (_, GuestPermission) = manager(&h, &c, request).await?;
    let permission = permission(&body.permission)?;
    h.device_guests
        .set_permission(&h, body.id, permission)
        .map_err(failed)?;
    answer(&h, &client)
}

pub async fn keyboard_mouse(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    let (client, body): (_, GuestKeyboardMouse) = manager(&h, &c, request).await?;
    h.device_guests
        .give_keyboard_mouse(body.id)
        .map_err(failed)?;
    answer(&h, &client)
}
