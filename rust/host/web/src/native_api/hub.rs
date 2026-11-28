//! Host Hub Sync routes (`/hub/snapshot`, `/hub/profiles/{id}`, `/hub/covers/{app_id}`):
//! per-device isolation (`#screen2` normalized via `device_uuid`), optimistic `If-Match` CAS
//! with `X-Hub-Current-Revision`, 64 KiB JSON cap returning `413 too_large`, and SHA-256 ETag
//! `304 Not Modified` caching.
use super::{
    ApiFailure, Failure, Reply, build_capabilities, caller, json, read_body_limit, require,
};
use axum::{
    Extension,
    body::Body,
    extract::{Path, Query, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use butterpollo_session::{
    hub_sync::{HubError, etag_matches, parse_if_match_revision},
    native::{self, LAUNCH, LIST},
};
use butterpollo_stream::{hub_store::CoverRead, state::Shared, tls::Connection};
use rubylight_native_api::{
    error::ErrorCode,
    hub::{
        HEADER_CURRENT_REVISION, HubCapabilitiesSnapshot, HubSnapshot, MAX_PROFILE_BODY_BYTES,
        ProfileWrite, SNAPSHOT_SCHEMA_VERSION,
    },
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::sync::{PoisonError, atomic::Ordering};

#[derive(Debug, Default, Deserialize)]
pub(super) struct SnapshotQuery {
    pub all_devices: Option<u8>,
}

pub(super) async fn snapshot(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    Query(query): Query<SnapshotQuery>,
    headers: HeaderMap,
) -> Reply {
    let client = caller(&h, &c)?;
    require(&client, LIST, "Read synced hub state")?;
    let all_devices = query.all_devices.unwrap_or(0) != 0;
    let host_control = native::host_control(&client);
    if all_devices && !host_control {
        return Err(Failure::new(
            ErrorCode::HostControlRequired,
            "Viewing all devices' synced profiles requires Host control.",
        )
        .into());
    }

    let if_none_match = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);

    let caps = build_capabilities(&h, &client).await;
    let client_for_job = client.clone();
    let h_for_job = h.clone();
    let (projection, codecs, virtual_display, hdr) = tokio::task::spawn_blocking(move || {
        h_for_job.reload_apps();
        let apps = h_for_job
            .apps
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let assets = h_for_job.assets.parent().unwrap_or(&h_for_job.assets);
        let projection =
            h_for_job
                .hub
                .project(&apps, assets, &client_for_job, all_devices, host_control)?;
        let mask = h_for_job.codecs.load(Ordering::Acquire);
        let codecs = codec_labels(mask);
        let windows_11 = butterpollo_win_display::display::virtual_display_os();
        let vd = h_for_job
            .config
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .virtual_display_mode(windows_11)
            != "disabled";
        let hdr = mask & (0x200 | 0x20000 | 0x2000000 | 0x4000000) != 0;
        Ok::<_, anyhow::Error>((projection, codecs, vd, hdr))
    })
    .await
    .map_err(|error| Failure::internal("build hub snapshot", error))?
    .map_err(|error| Failure::internal("persist hub cover digests", error))?;

    let mut snapshot = HubSnapshot {
        schema_version: SNAPSHOT_SCHEMA_VERSION,
        snapshot_revision: projection.snapshot_revision,
        host_unique_id: caps.host.unique_id,
        generated_unix_ms: 0,
        caller_device_uuid: projection.caller_device_uuid,
        capabilities_snapshot: HubCapabilitiesSnapshot {
            api_version: caps.api_version,
            features: caps.features,
            permissions: caps.permissions,
            codecs,
            virtual_display,
            hdr,
        },
        covers: projection.covers,
        profiles: projection.profiles,
        input_decks: projection.input_decks,
        host_overrides_summary: projection.host_overrides_summary,
    };
    let etag = snapshot_etag(&snapshot, all_devices);
    if let Some(ref inm) = if_none_match
        && etag_matches(inm, &etag)
    {
        return Ok((
            StatusCode::NOT_MODIFIED,
            [
                (header::ETAG, etag),
                (header::CACHE_CONTROL, "no-store".to_string()),
            ],
            Body::empty(),
        )
            .into_response());
    }
    snapshot.generated_unix_ms = projection.generated_unix_ms;
    let mut response = json(StatusCode::OK, &snapshot);
    if let Ok(val) = HeaderValue::from_str(&etag) {
        response.headers_mut().insert(header::ETAG, val);
    }
    Ok(response)
}

pub(super) async fn put_profile(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    Path(id): Path<String>,
    request: Request,
) -> Reply {
    let client = caller(&h, &c)?;
    require(&client, LAUNCH, "Save synced profiles")?;
    let host_control = native::host_control(&client);
    let if_match_header = request
        .headers()
        .get(header::IF_MATCH)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);

    let bytes = read_body_limit(request, MAX_PROFILE_BODY_BYTES).await?;
    let expected_rev = match if_match_header {
        Some(ref raw) => parse_if_match_revision(raw).map_err(hub_failure_response)?,
        None => {
            return Err(Failure::new(
                ErrorCode::InvalidRequest,
                "Send an If-Match header with the profile revision (use \"0\" to create).",
            )
            .into());
        }
    };
    let write: ProfileWrite = serde_json::from_slice(&bytes).map_err(|error| {
        Failure::new(
            ErrorCode::InvalidRequest,
            format!("Invalid synced profile JSON: {error}."),
        )
    })?;

    let outcome = tokio::task::spawn_blocking(move || {
        h.hub
            .put_profile(&id, &client, host_control, expected_rev, write)
    })
    .await
    .map_err(|error| Failure::internal("save synced profile", error))?
    .map_err(|error| Failure::internal("write hub-sync.json", error))?;

    match outcome {
        Ok(outcome) => {
            let status = if outcome.created {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            };
            let etag = format!("\"{}\"", outcome.profile.revision);
            let mut response = json(status, &outcome.profile);
            if let Ok(val) = HeaderValue::from_str(&etag) {
                response.headers_mut().insert(header::ETAG, val);
            }
            Ok(response)
        }
        Err(hub_err) => Ok(hub_error_http(hub_err)),
    }
}

pub(super) async fn delete_profile(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Reply {
    let client = caller(&h, &c)?;
    require(&client, LAUNCH, "Delete synced profiles")?;
    let host_control = native::host_control(&client);
    let expected_rev = match headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok()) {
        Some(raw) => parse_if_match_revision(raw).map_err(hub_failure_response)?,
        None => {
            return Err(Failure::new(
                ErrorCode::InvalidRequest,
                "Send an If-Match header with the profile revision to delete.",
            )
            .into());
        }
    };

    let result = tokio::task::spawn_blocking(move || {
        h.hub
            .delete_profile(&id, &client, host_control, expected_rev)
    })
    .await
    .map_err(|error| Failure::internal("delete synced profile", error))?
    .map_err(|error| Failure::internal("write hub-sync.json", error))?;

    match result {
        Ok(deleted) => Ok(json(StatusCode::OK, &deleted)),
        Err(hub_err) => Ok(hub_error_http(hub_err)),
    }
}

pub(super) async fn cover(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    Path(app_id): Path<u32>,
    headers: HeaderMap,
) -> Reply {
    let client = caller(&h, &c)?;
    require(&client, LIST, "Read game cover art")?;
    let if_none_match = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);

    let read = tokio::task::spawn_blocking(move || {
        h.reload_apps();
        let apps = h
            .apps
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let assets = h.assets.parent().unwrap_or(&h.assets);
        h.hub
            .read_cover(&apps, assets, app_id, if_none_match.as_deref())
    })
    .await
    .map_err(|error| Failure::internal("read cover art", error))?;

    match read {
        None => Err(Failure::new(
            ErrorCode::NotFound,
            "This game has no custom cover image on the PC.",
        )
        .into()),
        Some(CoverRead::NotModified { etag }) => Ok((
            StatusCode::NOT_MODIFIED,
            [
                (header::ETAG, format!("\"{etag}\"")),
                (
                    header::CACHE_CONTROL,
                    "private, max-age=0, must-revalidate".to_string(),
                ),
            ],
            Body::empty(),
        )
            .into_response()),
        Some(CoverRead::Body { etag, mime, bytes }) => Ok((
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, mime),
                (header::ETAG, format!("\"{etag}\"")),
                (
                    header::CACHE_CONTROL,
                    "private, max-age=0, must-revalidate".to_string(),
                ),
            ],
            bytes,
        )
            .into_response()),
    }
}

fn snapshot_etag(snapshot: &HubSnapshot, all_devices: bool) -> String {
    let mut hasher = Sha256::new();
    hasher.update([u8::from(all_devices)]);
    hasher.update(serde_json::to_vec(snapshot).unwrap_or_default());
    let digest = hex::encode(hasher.finalize());
    format!("\"rev:{}:{}\"", snapshot.snapshot_revision, &digest[..16])
}

fn codec_labels(mask: u32) -> Vec<String> {
    let mut out = Vec::with_capacity(5);
    if mask & 0x1 != 0 {
        out.push("h264".into());
    }
    if mask & 0x100 != 0 {
        out.push("hevc".into());
    }
    if mask & 0x200 != 0 {
        out.push("hevc10".into());
    }
    if mask & 0x10000 != 0 {
        out.push("av1".into());
    }
    if mask & 0x20000 != 0 {
        out.push("av1_10".into());
    }
    out
}

fn hub_failure_response(err: HubError) -> Failure {
    let code = err.code();
    let msg = err.message().to_string();
    let mut f = Failure::new(code, msg);
    f.setting = err.setting().map(str::to_owned);
    f
}

fn hub_error_http(err: HubError) -> Response {
    let current_rev = err.current_revision();
    let mut response = ApiFailure(hub_failure_response(err)).into_response();
    if let Some(rev) = current_rev
        && let Ok(header_name) = header::HeaderName::from_bytes(HEADER_CURRENT_REVISION.as_bytes())
        && let Ok(val) = HeaderValue::from_str(&rev.to_string())
    {
        response.headers_mut().insert(header_name, val);
    }
    response
}
