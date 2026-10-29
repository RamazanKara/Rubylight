//! The game page's Neural Rendering switch. `GET /api/neural?uuid=` shows
//! whether this PC can have it and why not, whether the user accepted the
//! notice, and the app's install with its progress. `POST /api/neural/accept`
//! records the acceptance, `POST /api/neural/enable` (`uuid`, optional `exe`)
//! downloads and installs, `POST /api/neural/disable` removes. Console-only:
//! API tokens cannot reach them.

use super::error;
use crate::{
    neural::{self, Refusal},
    state::Shared,
};
use axum::{
    Json,
    http::{Method, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};

pub(super) async fn handle(
    h: &Shared,
    method: &Method,
    path: &str,
    uri: &Uri,
    data: &Value,
) -> Response {
    let h = h.clone();
    let uuid = data["uuid"].as_str().unwrap_or_default().to_owned();
    let answer = match (method.as_str(), path) {
        ("GET", "/api/neural") => {
            let uuid = url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes())
                .find(|(key, _)| key == "uuid")
                .map(|(_, value)| value.into_owned())
                .unwrap_or_default();
            return match tokio::task::spawn_blocking(move || neural::status(&h, &uuid)).await {
                Ok(status) => Json(status).into_response(),
                Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
            };
        }
        ("POST", "/api/neural/accept") => {
            return match neural::accept(&h) {
                Ok(()) => Json(json!({"status": true})).into_response(),
                Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e:#}")),
            };
        }
        ("POST", "/api/neural/enable") => {
            let exe = data["exe"]
                .as_str()
                .map(str::trim)
                .filter(|exe| !exe.is_empty())
                .map(std::path::PathBuf::from);
            if exe.as_ref().is_some_and(|exe| {
                !exe.is_absolute()
                    || !exe
                        .extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
            }) {
                return error(
                    StatusCode::BAD_REQUEST,
                    "exe must be the full path of a .exe",
                );
            }
            tokio::task::spawn_blocking(move || neural::enable(&h, &uuid, exe)).await
        }
        ("POST", "/api/neural/disable") => {
            tokio::task::spawn_blocking(move || neural::disable(&h, &uuid)).await
        }
        _ => return error(StatusCode::NOT_FOUND, "not found"),
    };
    match answer {
        Ok(Ok(())) => (StatusCode::ACCEPTED, Json(json!({"status": true}))).into_response(),
        Ok(Err(Refusal::NotAccepted)) => error(
            StatusCode::CONFLICT,
            "accept the Neural Rendering notice first",
        ),
        Ok(Err(Refusal::Unavailable(reason))) => error(StatusCode::CONFLICT, &reason),
        Ok(Err(Refusal::NotFound)) => error(StatusCode::NOT_FOUND, "application not found"),
        Ok(Err(Refusal::Busy)) => error(
            StatusCode::CONFLICT,
            "Neural Rendering is busy with this game",
        ),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}
