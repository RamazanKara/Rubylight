//! Settings backup and restore API (docs/api.md, "Settings backup"). Console
//! only: no API token reaches it. `inspect` takes the archive as the raw
//! request body; the others take JSON.

use super::error;
use crate::settings_backup;
use crate::state::Shared;
use axum::{
    http::{Method, StatusCode, header},
    response::{IntoResponse, Json, Response},
};
use serde_json::Value;

pub(super) async fn handle(h: &Shared, method: &Method, path: &str, body: &[u8]) -> Response {
    if method != Method::POST {
        return error(StatusCode::METHOD_NOT_ALLOWED, "use POST");
    }
    let json = || -> Result<Value, Response> {
        if body.is_empty() {
            return Ok(Value::Object(Default::default()));
        }
        serde_json::from_slice(body).map_err(|_| error(StatusCode::BAD_REQUEST, "invalid JSON"))
    };
    let h = h.clone();
    match path {
        "/api/backup/export" => {
            let data = match json() {
                Ok(data) => data,
                Err(response) => return response,
            };
            // Secrets only when asked for in so many words.
            let secrets = data.get("secrets").and_then(Value::as_bool) == Some(true);
            let directory = h.directory.clone();
            match tokio::task::spawn_blocking(move || settings_backup::export(&directory, secrets))
                .await
            {
                Ok(Ok(bytes)) => (
                    [
                        (header::CONTENT_TYPE, "application/zip".to_owned()),
                        (
                            header::CONTENT_DISPOSITION,
                            format!(
                                "attachment; filename={}",
                                settings_backup::file_name(secrets)
                            ),
                        ),
                        (header::CACHE_CONTROL, "no-store".to_owned()),
                    ],
                    bytes,
                )
                    .into_response(),
                Ok(Err(e)) => error(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e:#}")),
                Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
            }
        }
        "/api/backup/inspect" => {
            if body.is_empty() {
                return error(
                    StatusCode::BAD_REQUEST,
                    "send the backup archive as the body",
                );
            }
            let bytes = body.to_vec();
            let directory = h.directory.clone();
            match tokio::task::spawn_blocking(move || settings_backup::inspect(&directory, &bytes))
                .await
            {
                Ok(Ok(answer)) => Json(answer).into_response(),
                Ok(Err(e)) => error(StatusCode::BAD_REQUEST, &format!("{e:#}")),
                Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
            }
        }
        "/api/backup/restore" => {
            let data = match json() {
                Ok(data) => data,
                Err(response) => return response,
            };
            let token = data
                .get("token")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            match tokio::task::spawn_blocking(move || settings_backup::restore(&h, &token)).await {
                Ok(Ok(answer)) => Json(answer).into_response(),
                Ok(Err(e)) if e.is::<settings_backup::Busy>() => {
                    error(StatusCode::CONFLICT, &e.to_string())
                }
                Ok(Err(e)) => error(StatusCode::BAD_REQUEST, &format!("{e:#}")),
                Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
            }
        }
        _ => error(StatusCode::NOT_FOUND, "unknown API endpoint"),
    }
}
