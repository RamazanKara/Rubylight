//! Settings backup and restore API (docs/api.md, "Settings backup"). Console
//! only: no API token reaches it. `inspect` takes the archive as the raw
//! request body; the others take JSON.

use super::error;
use axum::{
    body::Body,
    extract::State,
    http::{Method, StatusCode, header},
    response::{IntoResponse, Json, Response},
};
use butterpollo_session::settings_backup::MAX_ARCHIVE;
use butterpollo_stream::settings_backup;
use butterpollo_stream::state::Shared;
use hyper::body::Body as _;
use serde_json::Value;
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;

/// Deletes the temporary upload file if streaming or validation fails or is cancelled.
struct RemoveTemp(Option<PathBuf>);

impl RemoveTemp {
    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for RemoveTemp {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Streams `POST /api/backup/inspect` straight to a temp file under `backups/` while enforcing
/// `MAX_ARCHIVE` incrementally, then validates and summarizes entries one by one from disk.
pub(super) async fn inspect(State(h): State<Shared>, mut body: Body) -> Response {
    let dir = h.directory.clone();
    let folder = match tokio::task::spawn_blocking(move || {
        settings_backup::ensure_backups_dir(&dir)
    })
    .await
    {
        Ok(Ok(folder)) => folder,
        Ok(Err(e)) => return error(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e:#}")),
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let temp_path = folder.join(format!(".inspect-{}.tmp", uuid::Uuid::new_v4()));
    let mut guard = RemoveTemp(Some(temp_path.clone()));
    let mut total = 0u64;
    {
        let mut file = match tokio::fs::File::create(&temp_path).await {
            Ok(file) => file,
            Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        };
        while let Some(frame) =
            std::future::poll_fn(|cx| std::pin::Pin::new(&mut body).poll_frame(cx)).await
        {
            let frame = match frame {
                Ok(frame) => frame,
                Err(e) => {
                    return error(
                        StatusCode::BAD_REQUEST,
                        &format!("backup upload failed: {e}"),
                    );
                }
            };
            if let Ok(chunk) = frame.into_data() {
                total = total.saturating_add(chunk.len() as u64);
                if total > MAX_ARCHIVE {
                    return error(StatusCode::PAYLOAD_TOO_LARGE, "the archive is too large");
                }
                if let Err(e) = file.write_all(&chunk).await {
                    return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
                }
            }
        }
        if let Err(e) = file.flush().await {
            return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
        }
        if let Err(e) = file.sync_all().await {
            return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
        }
        // Close the OS handle synchronously before `rename` / `remove_file` on Windows.
        let _ = file.into_std().await;
    }
    if total == 0 {
        return error(
            StatusCode::BAD_REQUEST,
            "send the backup archive as the body",
        );
    }
    let directory = h.directory.clone();
    match tokio::task::spawn_blocking(move || settings_backup::inspect_file(&directory, &temp_path))
        .await
    {
        Ok(Ok(answer)) => {
            guard.disarm();
            Json(answer).into_response()
        }
        Ok(Err(e)) => error(StatusCode::BAD_REQUEST, &format!("{e:#}")),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

pub(super) async fn handle(h: &Shared, method: &Method, path: &str, body: &[u8]) -> Response {
    if method != Method::POST {
        return error(StatusCode::METHOD_NOT_ALLOWED, "use POST");
    }
    // None for a body that is not JSON; the caller answers 400 (a Response is too large an Err).
    let json = || -> Option<Value> {
        if body.is_empty() {
            return Some(Value::Object(Default::default()));
        }
        serde_json::from_slice(body).ok()
    };
    let h = h.clone();
    match path {
        "/api/backup/export" => {
            let Some(data) = json() else {
                return error(StatusCode::BAD_REQUEST, "invalid JSON");
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
        "/api/backup/restore" => {
            let Some(data) = json() else {
                return error(StatusCode::BAD_REQUEST, "invalid JSON");
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
