//! The console's Stream DVR routes (`/api/dvr`): the replay buffer's state with the clips
//! folder, saving a clip, recording, the saved clips, a clip's video for the preview (one byte
//! range at a time) and as a download, deleting one, and opening the folder on the PC's desktop.
//!
//! A refusal is 409 `{"status": false, "error": <plain English>, "reason": <snake_case>}`.
use super::error;
use axum::{
    Json,
    body::Body,
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use butterpollo_stream::{dvr_store, state::Shared, stream::dvr};
use rubylight_protocol::stream_dvr::Reason;
use serde_json::{Value, json};
use std::io::SeekFrom;
use std::path::PathBuf;
use std::sync::PoisonError;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

/// Handles `path` under `/api/dvr`.
pub(super) async fn handle(
    h: &Shared,
    method: &Method,
    path: &str,
    query: Option<&str>,
    headers: &HeaderMap,
    data: &Value,
) -> Response {
    let rest = path.strip_prefix("/api/dvr").unwrap_or_default();
    let h = h.clone();
    match (method.as_str(), rest) {
        ("GET", "") => {
            blocking(move || {
                let config = h
                    .config
                    .read()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone();
                let mut status =
                    serde_json::to_value(dvr::status(&h)).unwrap_or_else(|_| json!({}));
                status["enabled"] = json!(config.dvr_enabled());
                status["clips_dir"] = json!(clips_dir(&h).display().to_string());
                Json(status).into_response()
            })
            .await
        }
        ("GET", "/clips") => {
            let (limit, cursor) = page_query(query);
            blocking(move || {
                let config = h
                    .config
                    .read()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone();
                let list = dvr_store::page(
                    dvr_store::list(&clips_dir(&h)),
                    limit,
                    cursor.as_deref(),
                    dvr_store::quota_bytes(&config),
                );
                Json(list).into_response()
            })
            .await
        }
        ("POST", "/clip") => {
            let secs = data
                .get("duration_secs")
                .and_then(Value::as_u64)
                .map(|secs| secs.min(u64::from(u16::MAX)) as u16);
            blocking(move || match dvr::save_clip(&h, secs) {
                Ok(clip) => Json(json!({"status": true, "clip": clip})).into_response(),
                Err(reason) => refused(reason),
            })
            .await
        }
        ("POST", "/record") => {
            let begin = match data.get("action").and_then(Value::as_str) {
                Some("start") => true,
                Some("stop") => false,
                _ => return error(StatusCode::BAD_REQUEST, "action must be start or stop"),
            };
            blocking(move || match dvr::record(&h, begin) {
                Ok(clip) => Json(json!({"status": true, "dvr": dvr::status(&h), "clip": clip}))
                    .into_response(),
                Err(reason) => refused(reason),
            })
            .await
        }
        ("POST", "/open-folder") => blocking(move || open_folder(&h)).await,
        (method, rest) => match clip_route(rest) {
            Some((id, "")) if method == "DELETE" => {
                let id = id.to_owned();
                blocking(move || match h.dvr.delete(&clips_dir(&h), &id) {
                    Ok(true) => Json(json!({"status": true})).into_response(),
                    Ok(false) => error(StatusCode::NOT_FOUND, "There is no such clip."),
                    Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
                })
                .await
            }
            Some((id, "/video")) if method == "GET" => video(&h, id, headers, false).await,
            Some((id, "/download")) if method == "GET" => video(&h, id, headers, true).await,
            _ => error(StatusCode::NOT_FOUND, "unknown endpoint"),
        },
    }
}

/// `/clips/<id><tail>` as the id and its tail, for a valid clip id.
fn clip_route(rest: &str) -> Option<(&str, &str)> {
    let rest = rest.strip_prefix("/clips/")?;
    let (id, tail) = rest.find('/').map_or((rest, ""), |at| rest.split_at(at));
    butterpollo_media_policy::dvr::quota::valid_clip_id(id).then_some((id, tail))
}

fn clips_dir(h: &Shared) -> PathBuf {
    let config = h.config.read().unwrap_or_else(PoisonError::into_inner);
    dvr_store::clips_dir(&h.config_path, &config)
}

fn page_query(query: Option<&str>) -> (Option<usize>, Option<String>) {
    let (mut limit, mut cursor) = (None, None);
    for pair in query.unwrap_or_default().split('&') {
        match pair.split_once('=') {
            Some(("limit", value)) => limit = value.parse().ok(),
            Some(("cursor", value)) if !value.is_empty() => cursor = Some(value.to_owned()),
            _ => {}
        }
    }
    (limit, cursor)
}

fn refused(reason: Reason) -> Response {
    (
        StatusCode::CONFLICT,
        Json(
            json!({"status": false, "error": dvr::reason_text(reason), "reason": reason.as_str()}),
        ),
    )
        .into_response()
}

async fn blocking(run: impl FnOnce() -> Response + Send + 'static) -> Response {
    tokio::task::spawn_blocking(run)
        .await
        .unwrap_or_else(|e| error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))
}

fn open_folder(h: &Shared) -> Response {
    let dir = clips_dir(h);
    if !dvr_store::writable(&dir) {
        return refused(Reason::IoError);
    }
    let windows =
        std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    match butterpollo_win_io::process::Process::spawn_detached(
        &windows.join("explorer.exe"),
        &[dir.into_os_string()],
        butterpollo_win_io::process::Target::User { elevated: false },
    ) {
        Ok(()) => Json(json!({"status": true})).into_response(),
        Err(e) => error(
            StatusCode::CONFLICT,
            &format!("The clips folder could not be opened on the PC's desktop: {e:#}"),
        ),
    }
}

/// One `Range: bytes=` range of a `len`-byte file: `None` for the whole file (no header, or
/// one this does not serve, such as several ranges), `Err` when it lies outside the file.
pub(super) fn byte_range(value: Option<&str>, len: u64) -> Option<Result<(u64, u64), ()>> {
    let spec = value?.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (start, end) = spec.split_once('-')?;
    let (start, end) = (start.trim(), end.trim());
    let range = if start.is_empty() {
        let suffix: u64 = end.parse().ok()?;
        (suffix > 0 && len > 0).then(|| (len.saturating_sub(suffix), len - 1))
    } else {
        let start: u64 = start.parse().ok()?;
        let end = if end.is_empty() {
            len.checked_sub(1)
        } else {
            end.parse::<u64>()
                .ok()
                .map(|end| end.min(len.saturating_sub(1)))
        };
        end.filter(|end| start <= *end && start < len)
            .map(|end| (start, end))
    };
    Some(range.ok_or(()))
}

async fn video(h: &Shared, id: &str, headers: &HeaderMap, download: bool) -> Response {
    let path = dvr_store::video_path(&clips_dir(h), id);
    let mut file = match tokio::fs::File::open(&path).await {
        Ok(file) => file,
        Err(_) => return error(StatusCode::NOT_FOUND, "There is no such clip."),
    };
    let len = match file.metadata().await {
        Ok(meta) => meta.len(),
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let range = if download {
        None
    } else {
        byte_range(
            headers.get(header::RANGE).and_then(|v| v.to_str().ok()),
            len,
        )
    };
    let disposition = if download { "attachment" } else { "inline" };
    let mut response = match range {
        Some(Err(())) => {
            let mut response = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
            if let Ok(value) = HeaderValue::from_str(&format!("bytes */{len}")) {
                response.headers_mut().insert(header::CONTENT_RANGE, value);
            }
            return response;
        }
        Some(Ok((start, end))) => {
            if let Err(e) = file.seek(SeekFrom::Start(start)).await {
                return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
            }
            let body = Body::from_stream(tokio_util::io::ReaderStream::new(
                file.take(end - start + 1),
            ));
            let mut response = (StatusCode::PARTIAL_CONTENT, body).into_response();
            if let Ok(value) = HeaderValue::from_str(&format!("bytes {start}-{end}/{len}")) {
                response.headers_mut().insert(header::CONTENT_RANGE, value);
            }
            response
                .headers_mut()
                .insert(header::CONTENT_LENGTH, HeaderValue::from(end - start + 1));
            response
        }
        None => {
            let body = Body::from_stream(tokio_util::io::ReaderStream::new(file));
            let mut response = (StatusCode::OK, body).into_response();
            response
                .headers_mut()
                .insert(header::CONTENT_LENGTH, HeaderValue::from(len));
            response
        }
    };
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("video/mp4"));
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Ok(value) = HeaderValue::from_str(&format!("{disposition}; filename=\"{id}.mp4\"")) {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_ranges_follow_rfc_9110() {
        assert_eq!(byte_range(None, 100), None);
        assert_eq!(byte_range(Some("bytes=0-"), 100), Some(Ok((0, 99))));
        assert_eq!(byte_range(Some("bytes=10-19"), 100), Some(Ok((10, 19))));
        assert_eq!(byte_range(Some("bytes=90-500"), 100), Some(Ok((90, 99))));
        assert_eq!(byte_range(Some("bytes=-10"), 100), Some(Ok((90, 99))));
        assert_eq!(byte_range(Some("bytes=-500"), 100), Some(Ok((0, 99))));
        assert_eq!(byte_range(Some("bytes=100-"), 100), Some(Err(())));
        assert_eq!(byte_range(Some("bytes=20-10"), 100), Some(Err(())));
        assert_eq!(byte_range(Some("bytes=0-"), 0), Some(Err(())));
        assert_eq!(
            byte_range(Some("bytes=0-1,5-6"), 100),
            None,
            "several ranges: the whole file"
        );
        assert_eq!(byte_range(Some("items=0-1"), 100), None);
    }

    #[test]
    fn clip_routes_take_only_clip_ids() {
        assert_eq!(
            clip_route("/clips/20261126-120000-replay-ab12"),
            Some(("20261126-120000-replay-ab12", ""))
        );
        assert_eq!(
            clip_route("/clips/20261126-120000-replay-ab12/video"),
            Some(("20261126-120000-replay-ab12", "/video"))
        );
        assert_eq!(clip_route("/clips/..%2F..%2Fwin.ini/video"), None);
        assert_eq!(clip_route("/clips/short"), None);
        assert_eq!(clip_route("/clipsX"), None);
    }

    #[test]
    fn page_queries_are_lenient() {
        assert_eq!(page_query(None), (None, None));
        assert_eq!(
            page_query(Some("limit=20&cursor=abc")),
            (Some(20), Some("abc".into()))
        );
        assert_eq!(page_query(Some("limit=x&cursor=")), (None, None));
    }
}
