//! Stream DVR (`stream.dvr`, `docs/native-api.md`, "Stream DVR"): the replay buffer's state,
//! saving a clip, recording, and the saved clips. Saving and recording need Launch and the
//! device whose stream has the buffer; deleting a clip needs Host control. A refusal is 409
//! `dvr_refused` with the reason in plain English.
use super::{
    Failure, Reply, caller, json, read_body, require, require_host_control, require_view_or_launch,
};
use axum::{
    Extension,
    extract::{Path, RawQuery, Request, State},
    http::StatusCode,
    response::IntoResponse,
};
use butterpollo_session::{native::LAUNCH, state::Client};
use butterpollo_stream::{dvr_store, state::Shared, stream::dvr, tls::Connection};
use rubylight_native_api::{
    dvr::{ClipRequest, ClipSaved, RecordAction, RecordRequest, RecordResult},
    error::ErrorCode,
};
use rubylight_protocol::stream_dvr::Reason;
use std::sync::PoisonError;

/// The 409 for `reason`, pointing at the setting or a retry where one helps.
pub fn refused(reason: Reason) -> Failure {
    let failure = Failure::new(ErrorCode::DvrRefused, dvr::reason_text(reason));
    match reason {
        Reason::Disabled => failure.for_setting("dvr_enabled"),
        Reason::Busy | Reason::NoKeyframeYet => failure.retry_after(1000),
        _ => failure,
    }
}

async fn blocking<T: Send + 'static>(
    what: &'static str,
    run: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Failure> {
    tokio::task::spawn_blocking(run)
        .await
        .map_err(|error| Failure::internal(what, error))
}

/// Launch, and the device of the stream that has (or would get) the replay buffer.
fn may_save(h: &Shared, client: &Client) -> Result<(), Failure> {
    require(client, LAUNCH, "Launch applications")?;
    if dvr::owned_by(h, &client.uuid) {
        Ok(())
    } else {
        Err(refused(Reason::PermissionDenied))
    }
}

fn json_body<T: serde::de::DeserializeOwned + Default>(bytes: &[u8]) -> Result<T, Failure> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    serde_json::from_slice(bytes).map_err(|error| {
        Failure::new(
            ErrorCode::InvalidRequest,
            format!("The body is not a valid request: {error}."),
        )
    })
}

/// `GET /dvr`.
pub async fn status(State(h): State<Shared>, Extension(c): Extension<Connection>) -> Reply {
    let client = caller(&h, &c)?;
    require_view_or_launch(&client)?;
    let status = blocking("read the replay buffer's state", move || dvr::status(&h)).await?;
    Ok(json(StatusCode::OK, &status))
}

/// `POST /dvr/clip`: blocks until the clip is written.
pub async fn clip(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    let client = caller(&h, &c)?;
    may_save(&h, &client)?;
    let body: ClipRequest = json_body(&read_body(request).await?)?;
    let saved = blocking("save the clip", move || {
        dvr::save_clip(&h, body.duration_secs)
    })
    .await?
    .map_err(refused)?;
    Ok(json(StatusCode::OK, &ClipSaved { clip: saved }))
}

/// `POST /dvr/record`: a stop blocks until the recording is finished.
pub async fn record(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    let client = caller(&h, &c)?;
    may_save(&h, &client)?;
    let bytes = read_body(request).await?;
    let body: RecordRequest = serde_json::from_slice(&bytes).map_err(|error| {
        Failure::new(
            ErrorCode::InvalidRequest,
            format!("Send {{\"action\": \"start\"}} or {{\"action\": \"stop\"}}: {error}."),
        )
    })?;
    let result = blocking("change the recording", move || {
        dvr::record(&h, body.action == RecordAction::Start).map(|clip| RecordResult {
            status: dvr::status(&h),
            clip,
        })
    })
    .await?
    .map_err(refused)?;
    Ok(json(StatusCode::OK, &result))
}

/// `GET /dvr/clips?limit=&cursor=`.
pub async fn clips(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    RawQuery(query): RawQuery,
) -> Reply {
    let client = caller(&h, &c)?;
    require_view_or_launch(&client)?;
    let (mut limit, mut cursor) = (None, None);
    for pair in query.as_deref().unwrap_or_default().split('&') {
        match pair.split_once('=') {
            Some(("limit", value)) => {
                limit = Some(value.parse::<usize>().map_err(|_| {
                    Failure::new(ErrorCode::InvalidRequest, "limit is not a number.")
                })?);
            }
            Some(("cursor", value)) if !value.is_empty() => cursor = Some(value.to_owned()),
            _ => {}
        }
    }
    let list = blocking("list the clips", move || {
        let config = h
            .config
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let dir = dvr_store::clips_dir(&h.config_path, &config);
        dvr_store::page(
            dvr_store::list(&dir),
            limit,
            cursor.as_deref(),
            dvr_store::quota_bytes(&config),
        )
    })
    .await?;
    Ok(json(StatusCode::OK, &list))
}

/// `DELETE /dvr/clips/{id}`.
pub async fn delete(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    Path(id): Path<String>,
) -> Reply {
    let client = caller(&h, &c)?;
    require_host_control(&client)?;
    let deleted = blocking("delete the clip", move || {
        let config = h
            .config
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let dir = dvr_store::clips_dir(&h.config_path, &config);
        h.dvr.delete(&dir, &id)
    })
    .await?
    .map_err(|error| Failure::internal("delete the clip", error))?;
    if deleted {
        Ok(StatusCode::NO_CONTENT.into_response())
    } else {
        Err(Failure::new(ErrorCode::NotFound, "There is no such clip on this host.").into())
    }
}
