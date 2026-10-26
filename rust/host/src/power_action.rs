//! Sleep PC routes, in Vibepollo's wire format (`docs/native-api.md` does
//! not cover them): `GET /power-actions/v1` hands an opted-in paired client
//! a 30 s token while the host is idle, `POST /power-actions/v1/sleep`
//! spends it. HTTPS only. The decisions live in
//! `butterpollo_core::power_action`; this module holds the lifecycle gate
//! (`launch_transition`) from the idle check through the suspend, so a
//! racing launch waits, and suspends only once the 202 left the host.
use crate::{state::Shared, tls::Connection};
use anyhow::Result;
use axum::{
    Extension,
    body::{Body, Bytes},
    extract::State,
    http::{HeaderMap, Uri, header},
    response::{IntoResponse, Response},
};
use butterpollo_core::power_action::{self, Authority, Request, Tokens};
use std::{
    pin::Pin,
    sync::{Arc, Mutex, MutexGuard, RwLock, mpsc},
    task::{Context, Poll},
    time::{Duration, Instant},
};

/// How long an accepted request waits for its response to leave before it
/// gives up without suspending.
const DELIVERY_WAIT: Duration = Duration::from_secs(2);

/// Whether this PC can sleep, and the suspend itself.
pub trait Backend: Send + Sync {
    fn supported(&self) -> bool;
    fn suspend(&self) -> Result<()>;
}
#[cfg(not(test))]
struct Native;
#[cfg(not(test))]
impl Backend for Native {
    fn supported(&self) -> bool {
        butterpollo_windows::power::sleep_supported()
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
    fn supported(&self) -> bool {
        false
    }
    fn suspend(&self) -> Result<()> {
        panic!("a test reached the native suspend")
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
    /// `SleepSupported` in serverinfo.
    pub fn supported(&self) -> bool {
        self.backend().supported()
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

pub async fn sleep(
    State(h): State<Shared>,
    Extension(connection): Extension<Connection>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = Request::parse(
        true,
        uri.query(),
        body.len(),
        headers.contains_key(header::ORIGIN),
    );
    let (sender, receiver) = tokio::sync::oneshot::channel();
    tokio::task::spawn_blocking(move || sleep_blocking(&h, &connection, &request, sender));
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
    request: &Request,
    respond: tokio::sync::oneshot::Sender<Response>,
) {
    let caller = authority(h, connection);
    let backend = h.power.backend();
    let decision = h.power.tokens.lock().unwrap().sleep(
        request,
        caller.as_ref(),
        || backend.supported(),
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
    let (delivered, delivery) = mpsc::sync_channel(1);
    let body = Body::from_stream(tokio_util::io::ReaderStream::new(Delivery {
        data: answer.xml().into_bytes(),
        sent: 0,
        delivered: Some(delivered),
    }));
    if respond.send(reply(&answer, body)).is_err() {
        tracing::warn!(client, %peer, "Sleep PC client left before its answer; not sleeping");
        return;
    }
    if delivery.recv_timeout(DELIVERY_WAIT) != Ok(true) {
        tracing::warn!(client, %peer, "Sleep PC answer was not delivered; not sleeping");
        return;
    }
    // The opt-in is checked again, and the paired list stays locked through
    // the suspend: an administrator who just revoked it is obeyed.
    let paired = h.paired.read().unwrap();
    let allowed = connection
        .certificate
        .as_deref()
        .and_then(|der| paired.client_by_certificate(der))
        .is_some_and(|c| c.uuid == client && c.allows_sleep());
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
    drop(paired);
    drop(gate);
}

/// The body of a 202. Dropped, it tells the waiting request whether hyper
/// took every byte, which it then writes and closes; a client gone before
/// that leaves it short, and the host does not sleep.
struct Delivery {
    data: Vec<u8>,
    sent: usize,
    delivered: Option<mpsc::SyncSender<bool>>,
}
impl tokio::io::AsyncRead for Delivery {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = &mut *self;
        let n = buf.remaining().min(this.data.len() - this.sent);
        buf.put_slice(&this.data[this.sent..this.sent + n]);
        this.sent += n;
        Poll::Ready(Ok(()))
    }
}
impl Drop for Delivery {
    fn drop(&mut self) {
        if let Some(delivered) = self.delivered.take() {
            let _ = delivered.try_send(self.sent == self.data.len());
        }
    }
}

#[cfg(test)]
mod tests;
