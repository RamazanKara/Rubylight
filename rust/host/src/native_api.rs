//! The native API v1 (plan item A3): capabilities, library and session state
//! and actions, served on the streaming HTTPS port. The contract is
//! `docs/native-api.md`; the wire types live in `butterpollo_core::native_api`
//! and the projections in `butterpollo_core::native`.
//!
//! Every request identifies its caller from the TLS certificate and reads the
//! device's permissions and Host control flag from `h.paired` then, never from
//! a copy taken at launch: a grant or revoke in the console applies to the
//! next request.
pub(crate) mod files;
pub(crate) mod radeon;
pub(crate) mod wake;

use crate::{state::Shared, tls::Connection};
use axum::{
    Extension, Router,
    body::to_bytes,
    extract::{DefaultBodyLimit, Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{any, get, post},
};
use butterpollo_core::{
    native::{self, FILE_DOWNLOAD, FILE_UPLOAD, LAUNCH, LIST, VIEW},
    native_api::{
        self,
        capabilities::{Capabilities, HostInfo, Permissions},
        error::{ApiError, ErrorBody, ErrorCode},
        feature,
    },
    session_state::Refusal,
    state::{App, Client},
};
use serde::Serialize;
use std::{path::Path, sync::PoisonError, time::UNIX_EPOCH};

/// POST bodies are JSON of at most this many bytes.
const MAX_BODY: usize = 16 * 1024;
/// How soon a client retries after `session_busy`.
const BUSY_RETRY_MS: u64 = 1000;

/// A refused or failed request, answered with the error body of the spec.
#[derive(Debug)]
pub struct Failure {
    code: ErrorCode,
    message: String,
    setting: Option<String>,
    retry_after_ms: Option<u64>,
}
impl Failure {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            setting: None,
            retry_after_ms: None,
        }
    }
    pub(crate) fn internal(what: &str, error: impl std::fmt::Display) -> Self {
        tracing::error!(%error, "native API: {what}");
        Self::new(ErrorCode::Internal, format!("The host failed to {what}."))
    }
    pub(crate) fn code(&self) -> ErrorCode {
        self.code
    }
    pub(crate) fn message(&self) -> &str {
        &self.message
    }
    /// Name the setting the error is about.
    fn for_setting(mut self, id: &str) -> Self {
        self.setting = Some(id.to_owned());
        self
    }
    fn retry_after(mut self, milliseconds: u64) -> Self {
        self.retry_after_ms = Some(milliseconds);
        self
    }
}
impl From<Refusal> for Failure {
    fn from(refusal: Refusal) -> Self {
        match refusal {
            Refusal::NoSession => Self::new(ErrorCode::NoSession, "There is no session."),
            Refusal::Busy => Self::new(
                ErrorCode::SessionBusy,
                "A launch or a stop is in progress. Try again in a moment.",
            )
            .retry_after(BUSY_RETRY_MS),
            Refusal::NotOwner => Self::new(
                ErrorCode::NotSessionOwner,
                "This session belongs to another device.",
            ),
        }
    }
}
impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.code.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        json(
            status,
            &ErrorBody {
                error: ApiError {
                    code: self.code,
                    message: self.message,
                    setting: self.setting,
                    retry_after_ms: self.retry_after_ms,
                },
            },
        )
    }
}
type Reply = Result<Response, Failure>;

/// A JSON answer. Every answer says `no-store`: the state is live.
fn json<T: Serialize>(status: StatusCode, body: &T) -> Response {
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
        .route(&path("/library"), get(library).fallback(unknown))
        .route(&path("/session"), get(session).fallback(unknown))
        .route(
            &path("/session/disconnect"),
            post(disconnect).fallback(unknown),
        )
        .route(&path("/session/end"), post(end).fallback(unknown))
        .route(
            &path("/session/close-game"),
            post(close_game).fallback(unknown),
        )
        .route(
            &path("/session/couch-menu"),
            post(couch_menu).fallback(unknown),
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
        .route(&path("/wake"), get(wake::read).fallback(unknown))
        .route(native_api::PREFIX, any(unknown))
        .route(&path("/"), any(unknown))
        .route(&path("/{*rest}"), any(unknown))
}
async fn unknown() -> Failure {
    Failure::new(
        ErrorCode::NotFound,
        "This host has no such native API endpoint.",
    )
}

/// The paired device behind this connection, as the console has it now.
fn caller(h: &Shared, connection: &Connection) -> Result<Client, Failure> {
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
        .ok_or_else(not_paired)
}
fn permission_required(name: &str) -> Failure {
    Failure::new(
        ErrorCode::PermissionRequired,
        format!("Turn on {name} for this device in the Rubylight console."),
    )
}
fn require(client: &Client, bits: u32, name: &str) -> Result<(), Failure> {
    client
        .allows(bits)
        .then_some(())
        .ok_or_else(|| permission_required(name))
}
/// View streams or Launch applications: what seeing and leaving a session needs.
fn require_view_or_launch(client: &Client) -> Result<(), Failure> {
    require(client, VIEW, "View streams")
        .or_else(|_| require(client, LAUNCH, "Launch applications"))
}
pub(crate) fn require_host_control(client: &Client) -> Result<(), Failure> {
    native::host_control(client).then_some(()).ok_or_else(|| {
        Failure::new(
            ErrorCode::HostControlRequired,
            "Turn on Host control for this device in the Rubylight console.",
        )
    })
}

/// The body of a POST: JSON as `application/json`, `text/plain` or without a content type, of at
/// most 16 KiB.
async fn read_body(request: Request) -> Result<axum::body::Bytes, Failure> {
    let invalid = |message: &str| Failure::new(ErrorCode::InvalidRequest, message);
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
        return Err(invalid("Send the body as application/json."));
    }
    to_bytes(request.into_body(), MAX_BODY)
        .await
        .map_err(|_| invalid("The body is larger than 16 KiB or could not be read."))
}
/// An action POST carries no data. Its body is empty or a JSON object.
async fn expect_empty_body(request: Request) -> Result<(), Failure> {
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
    let couch_menu = crate::stream::device_config(&h, &client).map_or_else(
        |error| {
            tracing::warn!(client = %client.name, error = %format!("{error:#}"), "couch menu: the device's configuration did not load; not offering the menu");
            false
        },
        |config| config.couch_menu(),
    );
    let (name, extension_features) = {
        let config = h.config.read().unwrap_or_else(PoisonError::into_inner);
        (
            crate::network::host_name(&config),
            crate::extensions::native_features(&h, &config),
        )
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
            ]
            .into_iter()
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
            },
        },
    ))
}

async fn library(State(h): State<Shared>, Extension(c): Extension<Connection>) -> Reply {
    require(&caller(&h, &c)?, LIST, "List applications")?;
    let library = tokio::task::spawn_blocking(move || library_of(&h))
        .await
        .map_err(|error| Failure::internal("read the library", error))?;
    Ok(json(StatusCode::OK, &library))
}
fn library_of(h: &Shared) -> native_api::library::Library {
    let apps = h
        .apps
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let current = h
        .current_app
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .map(|app| app.id);
    let scan = crate::library::installed(h);
    // The same directory `/appasset` serves covers from.
    let assets = h.assets.parent().unwrap_or(&h.assets);
    native::library_with(&apps, scan.as_ref(), current, |app| {
        cover_fingerprint(app, assets)
    })
}
/// What identifies a cover image file: its path, size and modification time.
fn cover_fingerprint(app: &App, assets: &Path) -> String {
    let path = butterpollo_core::catalog::artwork(app, assets);
    let (size, modified) = std::fs::metadata(&path).map_or((0, 0), |meta| {
        let modified = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |elapsed| elapsed.as_nanos());
        (meta.len(), modified)
    });
    format!("{}|{size}|{modified}", path.display())
}

async fn session(State(h): State<Shared>, Extension(c): Extension<Connection>) -> Reply {
    let client = caller(&h, &c)?;
    let snapshot = h.session_snapshot();
    Ok(json(
        StatusCode::OK,
        &native::session_view(&snapshot, &client),
    ))
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Action {
    Disconnect,
    End,
    CloseGame,
}
async fn disconnect(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    act(h, c, request, Action::Disconnect).await
}
async fn end(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    act(h, c, request, Action::End).await
}
async fn close_game(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    act(h, c, request, Action::CloseGame).await
}
async fn act(h: Shared, c: Connection, request: Request, action: Action) -> Reply {
    caller(&h, &c)?;
    expect_empty_body(request).await?;
    // End session and Close game wait up to five seconds for the streams.
    tokio::task::spawn_blocking(move || run(&h, &c, action))
        .await
        .map_err(|error| Failure::internal("run the action", error))?
}
/// Check the device, then run the action and answer with the new session.
/// The permission checks use the device as the console has it now, right
/// before the action runs.
fn run(h: &Shared, connection: &Connection, action: Action) -> Reply {
    let client = caller(h, connection)?;
    session_action(h, &client, action)?;
    let client = caller(h, connection).unwrap_or(client);
    Ok(json(
        StatusCode::OK,
        &native::session_view(&h.session_snapshot(), &client),
    ))
}
/// Checks `client`'s permissions, then runs the action. The endpoints above
/// and the couch menu's rows both run session actions through here; End and
/// Close game block for up to five seconds.
pub(crate) fn session_action(h: &Shared, client: &Client, action: Action) -> Result<(), Failure> {
    match action {
        Action::Disconnect => require_view_or_launch(client)?,
        Action::End => require(client, LAUNCH, "Launch applications")?,
        Action::CloseGame => {
            require(client, LAUNCH, "Launch applications")?;
            require_host_control(client)?;
        }
    }
    // The actions check the session's owner themselves, under the launch lock.
    // With no session, Close game closes a game End session left running.
    let device = client.uuid.as_str();
    match action {
        Action::Disconnect => h.disconnect_session(Some(device)),
        Action::End => h.end_session(Some(device)),
        Action::CloseGame => h.close_game(device),
    }?;
    Ok(())
}

/// Opens or closes the couch menu in the caller's stream
/// (`docs/couch-menu-design.md` section 7.1). Idempotent.
async fn couch_menu(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    let client = caller(&h, &c)?;
    let body = read_body(request).await?;
    let request: native_api::session::CouchMenuRequest =
        serde_json::from_slice(&body).map_err(|error| {
            Failure::new(
                ErrorCode::InvalidRequest,
                format!("The body is not a valid couch menu request: {error}."),
            )
        })?;
    require_view_or_launch(&client)?;
    let open =
        tokio::task::spawn_blocking(move || crate::couch_menu::request(&h, &client, request.open))
            .await
            .map_err(|error| Failure::internal("open the couch menu", error))??;
    Ok(json(
        StatusCode::OK,
        &native_api::session::CouchMenu { open },
    ))
}
