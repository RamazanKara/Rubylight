//! The Stream page's guests (docs/browser-guests.md). `GET /api/guests` lists the invite links
//! and the guests watching, never a token. `POST /api/guests/invite` makes a link lasting
//! `seconds` with a `permission` and answers with its token, the only time the host has it, and
//! this PC's LAN addresses for a console opened on this PC. `POST /api/guests/revoke` withdraws
//! a link and disconnects whoever joined with it; `POST /api/guests/kick` disconnects one guest,
//! whose link keeps working. Console-only: API tokens cannot reach them, and nothing under
//! `/guest/` leads here.

use super::error;
use crate::{state::Shared, webrtc::guests::InviteInfo};
use axum::{
    Json,
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};
use butterpollo_core::webrtc::guests::{InviteError, Permission};
use serde_json::{Value, json};
use std::time::Duration;

pub(super) fn handle(h: &Shared, method: &Method, path: &str, data: &Value) -> Response {
    match (method.as_str(), path) {
        ("GET", "/api/guests") => Json(h.guests.status()).into_response(),
        ("POST", "/api/guests/invite") => {
            let Some(permission) = data
                .get("permission")
                .and_then(Value::as_str)
                .and_then(Permission::parse)
            else {
                return error(
                    StatusCode::BAD_REQUEST,
                    "permission must be view, controller or controller-keyboard-mouse",
                );
            };
            let Some(seconds) = data.get("seconds").and_then(Value::as_u64) else {
                return error(
                    StatusCode::BAD_REQUEST,
                    "seconds must be how long the link lasts",
                );
            };
            match h.guests.create(permission, Duration::from_secs(seconds)) {
                Ok((invite, token)) => {
                    tracing::info!(
                        invite = %invite.id,
                        permission = permission.as_str(),
                        expires = invite.expires,
                        "guest invite created"
                    );
                    Json(json!({
                        "invite": InviteInfo::from(&invite),
                        "token": token,
                        "addresses": addresses(),
                    }))
                    .into_response()
                }
                Err(refusal @ InviteError::Lifetime) => {
                    error(StatusCode::BAD_REQUEST, &refusal.to_string())
                }
                Err(refusal @ InviteError::Full) => {
                    error(StatusCode::CONFLICT, &refusal.to_string())
                }
            }
        }
        ("POST", "/api/guests/revoke") => {
            let Some(id) = data.get("id").and_then(Value::as_str) else {
                return error(
                    StatusCode::BAD_REQUEST,
                    "id must be the invite the panel showed",
                );
            };
            if !h.guests.revoke(id) {
                return error(StatusCode::NOT_FOUND, "There is no such invite link.");
            }
            tracing::info!(invite = %id, "guest invite revoked");
            Json(json!({})).into_response()
        }
        ("POST", "/api/guests/kick") => {
            let Some(id) = data.get("id").and_then(Value::as_u64) else {
                return error(
                    StatusCode::BAD_REQUEST,
                    "id must be the guest the panel showed",
                );
            };
            if !h.guests.kick(id) {
                return error(StatusCode::NOT_FOUND, "That guest is not watching.");
            }
            tracing::info!(guest = id, "guest kicked");
            Json(json!({})).into_response()
        }
        _ => error(StatusCode::BAD_REQUEST, "unknown API endpoint"),
    }
}

/// This PC's LAN addresses, for a link made on a console opened at `localhost`, which no guest
/// can open. Empty, and logged, when Windows does not list them.
fn addresses() -> Vec<String> {
    butterpollo_windows::net::lan_addresses().unwrap_or_else(|error| {
        tracing::warn!(error = %format!("{error:#}"), "the LAN addresses for a guest link could not be read");
        Vec::new()
    })
}
