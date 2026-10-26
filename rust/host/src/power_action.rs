//! Sleep PC routes, in Vibepollo's wire format (`docs/native-api.md` does
//! not cover them): `GET /power-actions/v1` hands an opted-in paired client
//! a 30 s token while the host is idle, `POST /power-actions/v1/sleep`
//! spends it. HTTPS only. The decisions live in
//! `butterpollo_core::power_action`; this module holds the lifecycle gate
//! (`launch_transition`) from the idle check through the suspend, so a
//! racing launch waits, and suspends only once the 202 left the host: the
//! answer closes its connection, and hyper finished writing and closing it.
use crate::{
    state::Shared,
    tls::{Connection, ConnectionEnd},
};
use anyhow::Result;
use axum::{
    Extension,
    body::Body,
    extract::State,
    http::{HeaderMap, Method, Uri, header},
    response::{IntoResponse, Response},
};
use butterpollo_core::power_action::{self, Authority, Request, SleepStates, Tokens};
use std::{
    sync::{Arc, Mutex, MutexGuard, RwLock, mpsc},
    time::{Duration, Instant},
};

/// How long an accepted request waits for its connection to close cleanly
/// after the answer before it gives up without suspending.
const DELIVERY_WAIT: Duration = Duration::from_secs(2);
/// A sleep request has an empty body; reading stops past this many bytes.
const BODY_LIMIT: usize = 1024;

/// The sleep states this PC reports, and the suspend itself.
pub trait Backend: Send + Sync {
    fn states(&self) -> Result<SleepStates>;
    fn suspend(&self) -> Result<()>;
}
#[cfg(not(test))]
struct Native;
#[cfg(not(test))]
impl Backend for Native {
    fn states(&self) -> Result<SleepStates> {
        butterpollo_windows::power::sleep_states()
    }
    fn suspend(&self) -> Result<()> {
        butterpollo_windows::power::suspend()
    }
}
/// Tests never put the PC running them to sleep: without a fake backend
/// the host reports no sleep support.
#[cfg(test)]
struct Native;
#[cfg(test)]
impl Backend for Native {
    fn states(&self) -> Result<SleepStates> {
        Ok(SleepStates::default())
    }
    fn suspend(&self) -> Result<()> {
        panic!("a test reached the native suspend")
    }
}
/// A PC with these sleep states, for tests that never suspend.
#[cfg(test)]
pub struct FixedStates(pub SleepStates);
#[cfg(test)]
impl Backend for FixedStates {
    fn states(&self) -> Result<SleepStates> {
        Ok(self.0)
    }
    fn suspend(&self) -> Result<()> {
        panic!("a test reached the suspend of a fixed backend")
    }
}
/// Why Sleep PC is unavailable, `None` on a PC with S1, S2 or S3.
fn unavailable(states: &Result<SleepStates>) -> Option<String> {
    match states {
        Ok(states) => states.unavailable().map(str::to_owned),
        Err(error) => Some(format!("{error:#}")),
    }
}

pub struct PowerActions {
    tokens: Mutex<Tokens>,
    backend: RwLock<Arc<dyn Backend>>,
}
impl Default for PowerActions {
    fn default() -> Self {
        Self {
            tokens: Mutex::default(),
            backend: RwLock::new(Arc::new(Native)),
        }
    }
}
impl PowerActions {
    fn backend(&self) -> Arc<dyn Backend> {
        self.backend.read().unwrap().clone()
    }
    /// `SleepSupported` in serverinfo: only a PC with S1, S2 or S3.
    pub fn supported(&self) -> bool {
        self.unavailable().is_none()
    }
    /// Why Sleep PC is unavailable here, for the console; `None` when it is.
    pub fn unavailable(&self) -> Option<String> {
        unavailable(&self.backend().states())
    }
    /// Once at startup: whether Sleep PC is available, and why not.
    pub fn log_availability(&self) {
        match self.backend().states() {
            Err(error) => tracing::warn!("Sleep PC is unavailable: {error:#}"),
            Ok(states) => match states.unavailable() {
                Some(reason) => {
                    tracing::info!(states = %states.describe(), "Sleep PC is unavailable: {reason}")
                }
                None => tracing::info!(states = %states.describe(), "Sleep PC is available"),
            },
        }
    }
    #[cfg(test)]
    pub fn set_backend(&self, backend: Arc<dyn Backend>) {
        *self.backend.write().unwrap() = backend;
    }
}

/// The caller, by its paired, enabled TLS certificate.
fn authority(h: &Shared, connection: &Connection) -> Option<Authority> {
    let der = connection
        .certificate
        .as_deref()
        .filter(|_| connection.tls)?;
    let paired = h.paired.read().unwrap();
    let client = paired.client_by_certificate(der)?;
    Some(Authority {
        uuid: client.uuid.clone(),
        allow_sleep: client.allows_sleep(),
    })
}
/// The lifecycle gate, taken without waiting, while nothing runs: no app,
/// stream, pending launch, teardown, display monitor, WebRTC link or
/// controller session in use.
fn idle(h: &Shared) -> Option<MutexGuard<'_, ()>> {
    let gate = h.launch_transition.try_lock().ok()?;
    let busy = crate::updater::busy(h)
        || !h.sessions.lock().unwrap().teardown.is_empty()
        || h.webrtc_links.len() != 0
        || h.controller_sessions.in_use();
    (!busy).then_some(gate)
}
fn reply(answer: &power_action::Response, body: Body) -> Response {
    let status = axum::http::StatusCode::from_u16(answer.status).unwrap();
    let length = answer.xml().len();
    (
        status,
        [
            (header::CONTENT_TYPE, "application/xml".to_owned()),
            (header::CACHE_CONTROL, "no-store".to_owned()),
            (header::CONNECTION, "close".to_owned()),
            (header::CONTENT_LENGTH, length.to_string()),
        ],
        body,
    )
        .into_response()
}

pub async fn capabilities(
    State(h): State<Shared>,
    Extension(connection): Extension<Connection>,
) -> Response {
    let caller = authority(&h, &connection);
    let supported = caller.is_some() && h.power.supported();
    let answer = h.power.tokens.lock().unwrap().capabilities(
        caller.as_ref(),
        supported,
        || idle(&h),
        Instant::now(),
        power_action::os_random,
    );
    if answer.status != 200 {
        tracing::warn!(status = answer.status, message = answer.message, peer = %connection.peer, "Sleep PC capabilities refused");
    }
    reply(&answer, Body::from(answer.xml()))
}

/// Every method reaches this, so a GET is refused with Vibepollo's 405 and
/// a body of any size with its 400, both logged.
pub async fn sleep(
    State(h): State<Shared>,
    Extension(connection): Extension<Connection>,
    Extension(end): Extension<ConnectionEnd>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Body,
) -> Response {
    // Too large or unreadable counts as a body.
    let body_len = axum::body::to_bytes(body, BODY_LIMIT)
        .await
        .map_or(BODY_LIMIT + 1, |bytes| bytes.len());
    let request = Request::parse(
        method == Method::POST,
        uri.query(),
        body_len,
        headers.contains_key(header::ORIGIN),
    );
    let (sender, receiver) = tokio::sync::oneshot::channel();
    tokio::task::spawn_blocking(move || sleep_blocking(&h, &connection, &end, &request, sender));
    match receiver.await {
        Ok(response) => response,
        Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
/// Runs on the blocking pool: the lifecycle gate (a std mutex) is held here
/// through the response and the suspend, never across an `.await`.
fn sleep_blocking(
    h: &Shared,
    connection: &Connection,
    end: &ConnectionEnd,
    request: &Request,
    respond: tokio::sync::oneshot::Sender<Response>,
) {
    let caller = authority(h, connection);
    let backend = h.power.backend();
    let decision = h.power.tokens.lock().unwrap().sleep(
        request,
        caller.as_ref(),
        || unavailable(&backend.states()).is_none(),
        || idle(h),
        Instant::now(),
    );
    let answer = decision.response;
    let client = caller.as_ref().map_or("", |c| c.uuid.as_str());
    let peer = connection.peer;
    let Some(gate) = decision.gate else {
        tracing::warn!(status = answer.status, message = answer.message, client, %peer, "Sleep PC request refused");
        let _ = respond.send(reply(&answer, Body::from(answer.xml())));
        return;
    };
    tracing::warn!(client, %peer, "Sleep PC request accepted");
    // Watched before the answer goes out, so its end cannot be missed. The
    // answer says `Connection: close`: hyper ends the connection cleanly
    // only after writing and flushing it, and a client gone before that
    // ends it with an error.
    let delivery = end.watch();
    if respond
        .send(reply(&answer, Body::from(answer.xml())))
        .is_err()
    {
        tracing::warn!(client, %peer, "Sleep PC client left before its answer; not sleeping");
        return;
    }
    match delivery.recv_timeout(DELIVERY_WAIT) {
        Ok(true) => {}
        outcome => {
            let outcome = match outcome {
                Ok(_) => "the connection failed",
                Err(mpsc::RecvTimeoutError::Timeout) => "the connection stayed open",
                Err(mpsc::RecvTimeoutError::Disconnected) => "the connection was dropped",
            };
            tracing::warn!(client, %peer, outcome, "Sleep PC answer was not delivered; not sleeping");
            return;
        }
    }
    // The opt-in is checked again: an administrator who just revoked it is
    // obeyed. The paired list is not held through the suspend, which
    // returns only after the PC wakes.
    let allowed = {
        let paired = h.paired.read().unwrap();
        connection
            .certificate
            .as_deref()
            .and_then(|der| paired.client_by_certificate(der))
            .is_some_and(|c| c.uuid == client && c.allows_sleep())
    };
    if !allowed {
        tracing::warn!(client, %peer, "Sleep PC was revoked before the suspend; not sleeping");
        return;
    }
    match backend.suspend() {
        Ok(()) => tracing::warn!(client, %peer, "Sleep PC suspend call returned"),
        Err(error) => {
            tracing::warn!(client, %peer, error = %format!("{error:#}"), "Sleep PC failed")
        }
    }
    drop(gate);
}

#[cfg(test)]
mod tests;
