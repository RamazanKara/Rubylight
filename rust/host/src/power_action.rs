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
use butterpollo_core::power_action::{
    self, Action, Authority, Busy, Request, SleepStates, Tokens, Unconfirmed,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, RwLock, mpsc},
    time::{Duration, Instant},
};

/// How long an accepted request waits for its connection to close cleanly
/// after the answer before it gives up without suspending.
const DELIVERY_WAIT: Duration = Duration::from_secs(2);
/// How long `GET /power` reports a device's last refusal.
pub(crate) const REFUSAL_KEPT: Duration = Duration::from_secs(10 * 60);
/// A sleep request has an empty body; reading stops past this many bytes.
const BODY_LIMIT: usize = 1024;

/// The sleep states this PC reports, the suspend, and Restart and Shut down.
pub trait Backend: Send + Sync {
    fn states(&self) -> Result<SleepStates>;
    fn suspend(&self) -> Result<()>;
    /// Asks Windows to restart or shut down, without closing apps by force.
    /// Returns once Windows took the request or refused it.
    fn power_off(&self, action: Action) -> Result<()>;
    /// Whether this backend reaches Windows; only the one outside tests does.
    fn native(&self) -> bool {
        false
    }
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
    fn power_off(&self, action: Action) -> Result<()> {
        butterpollo_windows::power::power_off(action == Action::Restart)
    }
    fn native(&self) -> bool {
        true
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
    fn power_off(&self, action: Action) -> Result<()> {
        panic!("a test reached the native {}", action.name())
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
    fn power_off(&self, action: Action) -> Result<()> {
        panic!("a test reached the {} of a fixed backend", action.name())
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
    /// Restart and Shut down's own tokens.
    pub(crate) power_tokens: Mutex<Tokens>,
    /// Why each device's last accepted restart or shutdown did not happen,
    /// whether that is only Windows not confirming it, and when.
    last_refusal: Mutex<HashMap<String, (String, bool, Instant)>>,
    backend: RwLock<Arc<dyn Backend>>,
}
impl Default for PowerActions {
    fn default() -> Self {
        Self {
            tokens: Mutex::default(),
            power_tokens: Mutex::default(),
            last_refusal: Mutex::default(),
            backend: RwLock::new(Arc::new(Native)),
        }
    }
}
impl PowerActions {
    pub(crate) fn backend(&self) -> Arc<dyn Backend> {
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
    /// Why `uuid`'s last accepted restart or shutdown did not happen, for
    /// [`REFUSAL_KEPT`] or until its next accepted request, and whether
    /// Windows only did not confirm it ([`Unconfirmed`]): the PC may still go.
    pub(crate) fn last_refusal(&self, uuid: &str, now: Instant) -> Option<(String, bool)> {
        let mut refusals = self.last_refusal.lock().unwrap();
        refusals.retain(|_, (_, _, at)| now.saturating_duration_since(*at) < REFUSAL_KEPT);
        refusals
            .get(uuid)
            .map(|(refusal, unconfirmed, _)| (refusal.clone(), *unconfirmed))
    }
    pub(crate) fn set_last_refusal(&self, uuid: &str, refusal: Option<String>) {
        self.set_refusal(uuid, refusal.map(|refusal| (refusal, false)));
    }
    /// Windows' answer to an accepted request: its refusal, or no answer in time.
    pub(crate) fn set_power_off_failure(&self, uuid: &str, error: &anyhow::Error) {
        let unconfirmed = error.downcast_ref::<Unconfirmed>().is_some();
        self.set_refusal(uuid, Some((format!("{error:#}"), unconfirmed)));
    }
    fn set_refusal(&self, uuid: &str, refusal: Option<(String, bool)>) {
        let mut refusals = self.last_refusal.lock().unwrap();
        match refusal {
            Some((refusal, unconfirmed)) => {
                refusals.insert(uuid.to_owned(), (refusal, unconfirmed, Instant::now()))
            }
            None => refusals.remove(uuid),
        };
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
        allow_restart_shutdown: client.allows_restart_shutdown(),
    })
}
/// The lifecycle gate, taken without waiting, while nothing runs: no app,
/// stream, pending launch, teardown, display monitor, WebRTC link or
/// controller session in use. Also gates resetting display persistence.
pub(crate) fn idle(h: &Shared) -> Option<MutexGuard<'_, ()>> {
    idle_or_busy(h).ok()
}
/// [`idle`], or what keeps the PC busy, naming the devices that stream.
pub(crate) fn idle_or_busy(h: &Shared) -> Result<MutexGuard<'_, ()>, Busy> {
    let gate = h
        .launch_transition
        .try_lock()
        .map_err(|_| Busy::Transition)?;
    match busy_beside_gate(h) {
        Some(busy) => Err(busy),
        None => Ok(gate),
    }
}
/// What [`idle_or_busy`] checks once it holds the gate. Browser guests and
/// controller sessions don't take the gate, so an accepted request checks
/// this again right before it goes ahead.
pub(crate) fn busy_beside_gate(h: &Shared) -> Option<Busy> {
    if let Some(busy) = streaming(h, None) {
        return Some(busy);
    }
    let ending = !h.sessions.lock().unwrap().teardown.is_empty();
    if let Some(app) = h.current_app.lock().unwrap().as_ref() {
        return Some(Busy::App(app.name.clone()));
    }
    if ending || !h.monitors.lock().unwrap().is_empty() {
        return Some(Busy::Ending);
    }
    if h.webrtc_links.len() != 0 {
        return Some(Busy::Guest);
    }
    if h.controller_sessions.in_use() {
        return Some(Busy::Controller);
    }
    None
}
/// The devices that stream or start a stream, but `except` (with both its screens).
pub(crate) fn streaming(h: &Shared, except: Option<&str>) -> Option<Busy> {
    use butterpollo_core::state::device_uuid;
    let sessions = h.sessions.lock().unwrap();
    let names = |clients: Vec<&butterpollo_core::state::Client>| {
        let mut names: Vec<String> = clients
            .into_iter()
            .filter(|c| except.is_none_or(|except| device_uuid(except) != device_uuid(&c.uuid)))
            .map(|c| c.name.clone())
            .collect();
        names.sort();
        names.dedup();
        names
    };
    let active = names(sessions.active.values().map(|s| &s.launch.client).collect());
    if !active.is_empty() {
        return Some(Busy::Streaming(active));
    }
    let pending = names(sessions.pending.values().map(|l| &l.client).collect());
    (!pending.is_empty()).then_some(Busy::Starting(pending))
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
    if let Some(busy) = busy_beside_gate(h) {
        let busy = busy.message();
        tracing::warn!(client, %peer, busy, "Sleep PC: the PC got busy before the suspend; not sleeping");
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
