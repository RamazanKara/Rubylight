//! The native API v1 (plan item A3): capabilities, library and session state
//! and actions, served on the streaming HTTPS port. The contract is
//! `docs/native-api.md`; the wire types live in `rubylight_native_api`
//! and the projections in `butterpollo_session::native`.
//!
//! Every request identifies its caller from the TLS certificate and reads the
//! device's permissions and Host control flag from `h.paired` then, never from
//! a copy taken at launch: a grant or revoke in the console applies to the
//! next request.
mod audio;
pub mod coop;
mod core_routes;
mod curtain;
mod doctor;
pub mod dvr;
pub mod files;
mod folders;
mod guests;
mod neural;
pub mod pairing;
mod power;
mod quick_suspend;
pub mod radeon;
pub mod wake;
mod wake_relay;

use axum::{
    Extension, Router,
    body::{Bytes, HttpBody, to_bytes},
    extract::{DefaultBodyLimit, Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{any, get, post, put},
};
use butterpollo_session::{
    native::{self, FILE_DOWNLOAD, FILE_UPLOAD, LAUNCH, LIST, VIEW},
    state::Client,
};
pub use butterpollo_stream::session_actions::{
    Action, Failure, permission_required, require, require_host_control, require_view_or_launch,
    session_action,
};
use butterpollo_stream::{
    extensions, network, neural as stream_neural, state::Shared, stream, tls::Connection, updater,
};
use rubylight_native_api as native_api;
use rubylight_native_api::{
    capabilities::{Capabilities, HostInfo, Permissions},
    error::{ErrorBody, ErrorCode},
    feature,
};
use serde::Serialize;
use std::{pin::Pin, sync::PoisonError};

/// POST bodies are JSON of at most this many bytes.
const MAX_BODY: usize = 16 * 1024;

/// Wrapper for [`Failure`] implementing `axum::response::IntoResponse`.
#[derive(Debug)]
pub struct ApiFailure(pub Failure);

impl<E: Into<Failure>> From<E> for ApiFailure {
    fn from(error: E) -> Self {
        Self(error.into())
    }
}

impl IntoResponse for ApiFailure {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.0.code.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        json(
            status,
            &ErrorBody {
                error: self.0.into_api_error(),
            },
        )
    }
}
type Reply = Result<Response, ApiFailure>;

/// A JSON answer. Every answer says `no-store`: the state is live.
pub(super) fn json<T: Serialize>(status: StatusCode, body: &T) -> Response {
    match serde_json::to_vec(body) {
        Ok(bytes) => (
            status,
            [
                (header::CONTENT_TYPE, "application/json; charset=utf-8"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            bytes,
        )
            .into_response(),
        Err(error) => {
            tracing::error!(%error, "native API: a response could not be serialized");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                [
                    (header::CONTENT_TYPE, "application/json; charset=utf-8"),
                    (header::CACHE_CONTROL, "no-store"),
                ],
                r#"{"error":{"code":"internal","message":"The host failed to build the answer.","setting":null,"retry_after_ms":null}}"#,
            )
                .into_response()
        }
    }
}

/// The router for the HTTPS service.
pub fn router() -> Router<Shared> {
    let path = |rest: &str| format!("{}{rest}", native_api::PREFIX);
    Router::new()
        .route(&path("/capabilities"), get(capabilities).fallback(unknown))
        .route(
            &path("/library"),
            get(core_routes::library).fallback(unknown),
        )
        .route(
            &path("/session"),
            get(core_routes::session).fallback(unknown),
        )
        .route(
            &path("/session/disconnect"),
            post(core_routes::disconnect).fallback(unknown),
        )
        .route(
            &path("/session/end"),
            post(core_routes::end).fallback(unknown),
        )
        .route(
            &path("/session/continue-here"),
            post(core_routes::continue_here).fallback(unknown),
        )
        .route(
            &path("/session/close-game"),
            post(core_routes::close_game).fallback(unknown),
        )
        .route(
            &path("/session/couch-menu"),
            post(core_routes::couch_menu).fallback(unknown),
        )
        .route(
            &path("/session/suspend"),
            get(quick_suspend::read)
                .post(quick_suspend::suspend)
                .fallback(unknown),
        )
        .route(
            &path("/session/resume"),
            post(quick_suspend::resume).fallback(unknown),
        )
        .route(
            &path("/session/curtain"),
            get(curtain::read).fallback(unknown),
        )
        .route(
            &path("/session/curtain/dismiss"),
            post(curtain::dismiss).fallback(unknown),
        )
        .route(&path("/radeon"), get(radeon::read).fallback(unknown))
        .route(
            &path("/radeon/settings"),
            post(radeon::write).fallback(unknown),
        )
        .route(
            &path("/radeon/restore"),
            post(radeon::restore).fallback(unknown),
        )
        .route(&path("/neural"), get(neural::read).fallback(unknown))
        .route(
            &path("/neural/settings"),
            post(neural::write).fallback(unknown),
        )
        // An upload streams to disk: the inner limit replaces the 1 MiB one
        // `nvhttp::router` sets, and the route checks Content-Length itself.
        .route(
            &path("/files"),
            get(files::list)
                .merge(post(files::upload).layer(DefaultBodyLimit::disable()))
                .fallback(unknown),
        )
        .route(
            &path("/files/content"),
            get(files::content).fallback(unknown),
        )
        .route(&path("/folders"), get(folders::list).fallback(unknown))
        .route(
            &path("/folders/{id}/entries"),
            get(folders::entries).fallback(unknown),
        )
        .route(
            &path("/folders/{id}/content"),
            get(folders::content)
                .merge(put(folders::upload).layer(DefaultBodyLimit::disable()))
                .fallback(unknown),
        )
        .route(&path("/wake"), get(wake::read).fallback(unknown))
        .route(&path("/stream/doctor"), get(doctor::read).fallback(unknown))
        .route(
            &path("/stream/doctor/apply"),
            post(doctor::apply).fallback(unknown),
        )
        .route(&path("/dvr"), get(dvr::status).fallback(unknown))
        .route(&path("/dvr/clip"), post(dvr::clip).fallback(unknown))
        .route(&path("/dvr/record"), post(dvr::record).fallback(unknown))
        .route(&path("/dvr/clips"), get(dvr::clips).fallback(unknown))
        .route(
            &path("/dvr/clips/{id}"),
            axum::routing::delete(dvr::delete).fallback(unknown),
        )
        .route(
            &path("/wake/relay"),
            get(wake_relay::read).fallback(unknown),
        )
        .route(
            &path("/wake/relay/enroll"),
            post(wake_relay::enroll).fallback(unknown),
        )
        .route(&path("/audio"), get(audio::read).fallback(unknown))
        .route(
            &path("/audio/settings"),
            post(audio::write).fallback(unknown),
        )
        .route(&path("/power"), get(power::read).fallback(unknown))
        .route(
            &path("/power/restart"),
            post(power::restart).fallback(unknown),
        )
        .route(
            &path("/power/shutdown"),
            post(power::shutdown).fallback(unknown),
        )
        .route(&path("/update"), get(power::update).fallback(unknown))
        .route(
            &path("/update/install"),
            post(power::install).fallback(unknown),
        )
        .route(
            &path("/pairing/requests"),
            get(pairing::requests).fallback(unknown),
        )
        .route(
            &path("/pairing/approve"),
            post(pairing::approve).fallback(unknown),
        )
        .route(
            &path("/pairing/deny"),
            post(pairing::deny).fallback(unknown),
        )
        .route(
            &path("/session/guests"),
            get(guests::list).fallback(unknown),
        )
        .route(
            &path("/session/guests/approve"),
            post(guests::approve).fallback(unknown),
        )
        .route(
            &path("/session/guests/deny"),
            post(guests::deny).fallback(unknown),
        )
        .route(
            &path("/session/guests/kick"),
            post(guests::kick).fallback(unknown),
        )
        .route(
            &path("/session/guests/revoke"),
            post(guests::revoke).fallback(unknown),
        )
        .route(
            &path("/session/guests/permission"),
            post(guests::set_permission).fallback(unknown),
        )
        .route(
            &path("/session/guests/keyboard-mouse"),
            post(guests::keyboard_mouse).fallback(unknown),
        )
        .route(
            &path("/session/guests/invites"),
            get(guests::invites).post(guests::invite).fallback(unknown),
        )
        .route(
            &path("/session/guests/invites/revoke"),
            post(guests::revoke_invite).fallback(unknown),
        )
        .route(&path("/coop"), get(coop::list).fallback(unknown))
        .route(&path("/coop/join"), post(coop::join).fallback(unknown))
        .route(
            &path(native_api::usb::TUNNEL_PATH),
            get(core_routes::usb_tunnel).fallback(unknown),
        )
        .route(native_api::PREFIX, any(unknown))
        .route(&path("/"), any(unknown))
        .route(&path("/{*rest}"), any(unknown))
}
async fn unknown() -> ApiFailure {
    Failure::new(
        ErrorCode::NotFound,
        "This host has no such native API endpoint.",
    )
    .into()
}

/// The paired device behind this connection, as the console has it now; its second screen for a
/// request of that screen ([`nvhttp::as_requested`]).
pub(super) fn caller(h: &Shared, connection: &Connection) -> Result<Client, Failure> {
    let not_paired = || {
        Failure::new(
            ErrorCode::NotPaired,
            "This device is not paired with this host, or it is turned off. Pair it first.",
        )
    };
    let der = connection
        .certificate
        .as_deref()
        .filter(|_| connection.tls)
        .ok_or_else(not_paired)?;
    h.paired
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .client_by_certificate(der)
        .cloned()
        .map(|client| butterpollo_stream::nvhttp::as_requested(client, connection))
        .ok_or_else(not_paired)
}

fn validate_json_content_type(request: &Request) -> Result<(), Failure> {
    let content_type = request
        .headers()
        .get(header::CONTENT_TYPE)
        .map(|value| value.to_str().unwrap_or("?"))
        .map(|value| {
            value
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
        });
    if !matches!(
        content_type.as_deref(),
        None | Some("" | "application/json" | "text/plain")
    ) {
        return Err(Failure::new(
            ErrorCode::InvalidRequest,
            "Send the body as application/json.",
        ));
    }
    Ok(())
}

/// The body of a POST: JSON as `application/json`, `text/plain` or without a content type, of at
/// most 16 KiB.
pub(super) async fn read_body(request: Request) -> Result<axum::body::Bytes, Failure> {
    validate_json_content_type(&request)?;
    to_bytes(request.into_body(), MAX_BODY).await.map_err(|_| {
        Failure::new(
            ErrorCode::InvalidRequest,
            "The body is larger than 16 KiB or could not be read.",
        )
    })
}

/// Reads a JSON body up to `max` bytes, returning [`ErrorCode::TooLarge`] (HTTP 413) when
/// `Content-Length` or streamed frame bytes exceed `max`.
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "shared 64 KiB JSON body reader for Wave 1+ native routes"
    )
)]
pub(super) async fn read_body_limit(
    request: Request,
    max: usize,
) -> Result<axum::body::Bytes, Failure> {
    validate_json_content_type(&request)?;
    if let Some(len) = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        && len > max as u64
    {
        return Err(Failure::new(
            ErrorCode::TooLarge,
            format!("The body exceeds the {max}-byte limit."),
        ));
    }
    let mut body = request.into_body();
    let mut out = Vec::new();
    while let Some(frame) = std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await {
        let frame = frame.map_err(|_| {
            Failure::new(
                ErrorCode::InvalidRequest,
                "The request body could not be read.",
            )
        })?;
        if let Ok(chunk) = frame.into_data() {
            if out.len().saturating_add(chunk.len()) > max {
                return Err(Failure::new(
                    ErrorCode::TooLarge,
                    format!("The body exceeds the {max}-byte limit."),
                ));
            }
            out.extend_from_slice(&chunk);
        }
    }
    Ok(Bytes::from(out))
}

/// An action POST carries no data. Its body is empty or a JSON object.
pub(super) async fn expect_empty_body(request: Request) -> Result<(), Failure> {
    let invalid = |message: &str| Failure::new(ErrorCode::InvalidRequest, message);
    let bytes = read_body(request).await?;
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(());
    }
    serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(&bytes)
        .map(drop)
        .map_err(|error| invalid(&format!("The body is not a JSON object: {error}.")))
}

async fn capabilities(State(h): State<Shared>, Extension(c): Extension<Connection>) -> Reply {
    let client = caller(&h, &c)?;
    let file_transfer = h
        .config
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .file_transfer();
    // The device's own settings can turn the couch menu off.
    let couch_menu = stream::device_config(&h, &client).map_or_else(
        |error| {
            tracing::warn!(client = %client.name, error = %format!("{error:#}"), "couch menu: the device's configuration did not load; not offering the menu");
            false
        },
        |config| config.couch_menu(),
    );
    let (name, extension_features, pairing, guests) = {
        let config = h.config.read().unwrap_or_else(PoisonError::into_inner);
        (
            network::host_name(&config),
            extensions::native_features(&h, &config),
            config.boolean("enable_pairing", true),
            butterpollo_session::multiclient::JoinPolicy::from_config(&config)
                != butterpollo_session::multiclient::JoinPolicy::Off,
        )
    };
    let coop = h.nucleus.unavailable().is_none();
    let update = updater::install_supported(&h);
    // Frame generation only where one of the PC's GPUs is AMD's or NVIDIA's,
    // by its DXGI vendor ID (no probe; GPUs that can't be listed keep it offered).
    let neural = {
        let h = h.clone();
        tokio::task::spawn_blocking(move || stream_neural::offered(&h))
            .await
            .unwrap_or(true)
    };
    Ok(json(
        StatusCode::OK,
        &Capabilities {
            api_version: native_api::API_VERSION,
            host: HostInfo {
                name,
                version: env!("CARGO_PKG_VERSION").into(),
                unique_id: h
                    .paired
                    .read()
                    .unwrap_or_else(PoisonError::into_inner)
                    .unique_id
                    .clone(),
            },
            features: [
                feature::LIBRARY,
                feature::SESSION,
                feature::CLOSE_GAME,
                feature::SESSION_LAUNCHER,
                feature::WAKE,
                // Night mode and headphone surround of the caller's own stream.
                feature::AUDIO_NIGHT_MODE,
                feature::AUDIO_HEADPHONE_SURROUND,
                // Microphone-only sessions; launching one needs Launch apps.
                feature::MIC_ONLY,
                // Taking over the game's stream; pressing it needs View and Launch apps.
                feature::CONTINUE_HERE,
                // Each device needs its own Restart and shut down PC grant.
                feature::POWER_RESTART,
                feature::POWER_SHUTDOWN,
            ]
            .into_iter()
            // A game's frame generation; reading needs View streams, a write
            // Host control too. Whether this PC's GPU can run it is in the
            // answer, which the first time of a host start asks.
            .chain(neural.then_some(feature::NEURAL_FRAME_GENERATION))
            // The Radeon endpoints answer only where the host started its Radeon worker.
            .chain(
                h.radeon
                    .worker()
                    .is_some()
                    .then_some([
                        feature::RADEON_READ,
                        feature::RADEON_WRITE,
                        feature::RADEON_PROFILE,
                    ])
                    .into_iter()
                    .flatten(),
            )
            // File transfer answers only while the host's setting is on.
            .chain(
                file_transfer
                    .then_some([feature::FILES_UPLOAD, feature::FILES_DOWNLOAD])
                    .into_iter()
                    .flatten(),
            )
            // The couch menu, for a device that may see its sessions.
            .chain(
                (couch_menu && (client.allows(VIEW) || client.allows(LAUNCH)))
                    .then_some(feature::COUCH_MENU),
            )
            // Stream Doctor 2.0, for a device that may see its stream.
            .chain(
                (client.allows(VIEW) || client.allows(LAUNCH)).then_some(feature::STREAM_DOCTOR_V2),
            )
            // While this host can run Nucleus Co-op sessions.
            .chain(coop.then_some(feature::COOP))
            // Where the host installs its own updates; installing needs Host control.
            .chain(update.then_some(feature::UPDATE))
            // While new devices may pair; approving needs Host control.
            .chain(pairing.then_some(feature::PAIRING_APPROVE))
            // While another device may join the running game as a guest.
            .chain(guests.then_some(feature::SESSION_GUESTS))
            .chain(pairing.then_some(feature::GUEST_INVITES))
            .chain(extension_features)
            .map(String::from)
            .collect(),
            permissions: Permissions {
                list: client.allows(LIST),
                view: client.allows(VIEW),
                launch: client.allows(LAUNCH),
                host_control: native::host_control(&client),
                file_upload: client.allows(FILE_UPLOAD),
                file_download: client.allows(FILE_DOWNLOAD),
                usb: client.allows_usb(),
                shared_folders: client.has_folders(),
                coop_join: client.allows_coop_join(),
                restart_shutdown: client.allows_restart_shutdown(),
                pairing_approve: client.allows_pairing_approve(),
            },
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use std::io::Cursor;
    use tokio_util::io::ReaderStream;

    const LIMIT: usize = 64 * 1024;

    #[tokio::test]
    async fn read_body_limit_accepts_payload_under_limit() {
        let payload = vec![b'a'; LIMIT - 1];
        let request = Request::builder()
            .method("POST")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::CONTENT_LENGTH, payload.len().to_string())
            .body(Body::from(payload.clone()))
            .unwrap();

        let bytes = read_body_limit(request, LIMIT).await.unwrap();
        assert_eq!(bytes.as_ref(), payload.as_slice());
    }

    #[tokio::test]
    async fn read_body_limit_rejects_excessive_content_length() {
        let request = Request::builder()
            .method("POST")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::CONTENT_LENGTH, (LIMIT + 1).to_string())
            .body(Body::from("{}"))
            .unwrap();

        let error = read_body_limit(request, LIMIT).await.unwrap_err();
        assert_eq!(error.code, ErrorCode::TooLarge);
    }

    #[tokio::test]
    async fn read_body_limit_rejects_streamed_chunks_over_limit() {
        let payload = vec![b'a'; LIMIT + 1];
        let stream = ReaderStream::with_capacity(Cursor::new(payload), 16 * 1024);
        let request = Request::builder()
            .method("POST")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from_stream(stream))
            .unwrap();

        let error = read_body_limit(request, LIMIT).await.unwrap_err();
        assert_eq!(error.code, ErrorCode::TooLarge);
    }
}
