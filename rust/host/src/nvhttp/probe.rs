//! The bandwidth tests a device runs before a stream: `/bandwidth-probe` and PyroWave's paired UDP probe.

use super::*;

pub(super) async fn pyrowave_bandwidth(
    State(h): State<Shared>,
    Extension(connection): Extension<Connection>,
) -> Response {
    if authenticated(&h, &connection, 0).is_err() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    static PAYLOAD: std::sync::OnceLock<Bytes> = std::sync::OnceLock::new();
    let payload = PAYLOAD
        .get_or_init(|| Bytes::from(vec![0xa5; 32 * 1024 * 1024]))
        .clone();
    (
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        payload,
    )
        .into_response()
}
/// `GET /bandwidth-probe?bytes=N` (`butterpollo_core::bandwidth_probe`): an
/// incompressible body a paired client times before it picks its first
/// bitrate, for every codec. Refused while a stream runs or starts, cut short
/// when one starts meanwhile, and past each client's cooldown and budget.
pub(super) async fn bandwidth_probe(
    State(h): State<Shared>,
    Extension(connection): Extension<Connection>,
    Query(args): Query<Args>,
) -> Response {
    use butterpollo_core::bandwidth_probe as probe;
    let offered = {
        let config = h.config.read().unwrap();
        crate::extensions::bandwidth_probe(&h, &config)
    };
    if !offered {
        return StatusCode::NOT_FOUND.into_response();
    }
    let client = match authenticated(&h, &connection, 0) {
        Ok(client) => client,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };
    let bytes = match probe::parse(args.get("bytes").map(String::as_str)) {
        Ok(bytes) => bytes,
        Err(error) => return (StatusCode::BAD_REQUEST, error.to_string()).into_response(),
    };
    let busy = {
        let mut sessions = h.sessions.lock().unwrap();
        let expired = sessions.expire();
        let busy = streaming(&sessions);
        drop(sessions);
        drop(expired);
        busy
    };
    if busy {
        return (
            StatusCode::CONFLICT,
            "Stop streaming before testing bandwidth",
        )
            .into_response();
    }
    let admitted = h
        .bandwidth_probes
        .lock()
        .unwrap()
        .admit(&client.uuid, bytes, Instant::now());
    if let Err(wait) = admitted {
        let seconds = wait.as_secs() + u64::from(wait.subsec_nanos() > 0);
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, seconds.max(1).to_string())],
            "Bandwidth probe limit reached",
        )
            .into_response();
    }
    tracing::info!(client = %client.name, bytes, "bandwidth probe");
    (
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        axum::body::Body::from_stream(tokio_util::io::ReaderStream::with_capacity(
            ProbeBody::new(
                bytes as usize,
                Instant::now() + probe::MAX_DURATION,
                stream_starts(&h),
            ),
            64 * 1024,
        )),
    )
        .into_response()
}
/// Whether a stream runs or starts, which a probe must not compete with.
pub(super) fn streaming(sessions: &crate::state::Sessions) -> bool {
    !sessions.active.is_empty() || !sessions.pending.is_empty()
}
/// Whether a stream runs or starts on `h` now, for a probe body to stop at.
pub(super) fn stream_starts(h: &Shared) -> impl Fn() -> bool + Send + Unpin + 'static {
    let h = h.clone();
    move || streaming(&h.sessions.lock().unwrap())
}
/// The bandwidth probe's body: random bytes, which no compression on the path
/// can shrink, until `remaining` is sent, the deadline passes or `busy` says a
/// stream runs or starts.
pub(super) struct ProbeBody<F> {
    pub(super) remaining: usize,
    offset: usize,
    pub(super) deadline: Instant,
    busy: F,
}
impl<F: Fn() -> bool> ProbeBody<F> {
    pub(super) fn new(remaining: usize, deadline: Instant, busy: F) -> Self {
        Self {
            remaining,
            offset: 0,
            deadline,
            busy,
        }
    }
    pub(super) fn noise() -> &'static [u8] {
        static NOISE: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
        NOISE.get_or_init(|| {
            use rand::RngCore;
            let mut bytes = vec![0; butterpollo_core::bandwidth_probe::MAX_BYTES as usize];
            rand::thread_rng().fill_bytes(&mut bytes);
            bytes
        })
    }
}
impl<F: Fn() -> bool + Unpin> tokio::io::AsyncRead for ProbeBody<F> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        // The body ends early at the deadline, or when a stream runs or
        // starts: the client times what arrived.
        if self.remaining > 0 && (self.busy)() {
            tracing::info!(
                unsent = self.remaining,
                "bandwidth probe stopped for a stream"
            );
            self.remaining = 0;
        }
        if self.remaining > 0 && Instant::now() < self.deadline {
            let noise = Self::noise();
            let n = self
                .remaining
                .min(buf.remaining())
                .min(noise.len() - self.offset);
            buf.put_slice(&noise[self.offset..self.offset + n]);
            self.offset = (self.offset + n) % noise.len();
            self.remaining -= n;
        }
        std::task::Poll::Ready(Ok(()))
    }
}
/// Vibeshine's paired UDP probe: two seconds of numbered packets from this
/// host to the client's own address, so the client can count what a rate, or
/// whole frames at a pace, lose on its path. Refused while anything streams.
pub(super) async fn pyrowave_udp_probe(
    State(h): State<Shared>,
    Extension(connection): Extension<Connection>,
    Query(args): Query<Args>,
) -> Response {
    static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if authenticated(&h, &connection, 0).is_err() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(request) =
        butterpollo_core::pyrowave_probe::parse(|key| args.get(key).map(String::as_str))
    else {
        return (StatusCode::BAD_REQUEST, "Invalid UDP probe parameters").into_response();
    };
    let streaming = {
        let mut sessions = h.sessions.lock().unwrap();
        let expired = sessions.expire();
        let busy = !sessions.active.is_empty() || !sessions.pending.is_empty();
        drop(sessions);
        drop(expired);
        busy
    };
    if streaming || RUNNING.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return (
            StatusCode::BAD_REQUEST,
            "Stop streaming before testing bandwidth",
        )
            .into_response();
    }
    struct Release;
    impl Drop for Release {
        fn drop(&mut self) {
            RUNNING.store(false, std::sync::atomic::Ordering::Release);
        }
    }
    let release = Release;
    let peer = std::net::SocketAddr::new(connection.peer.ip().to_canonical(), request.port);
    let socket = match crate::pyrowave_probe::bind(connection.local.ip().to_canonical()) {
        Ok(socket) => socket,
        Err(error) => {
            drop(release);
            tracing::warn!(error = %format!("{error:#}"), "PyroWave UDP probe could not bind");
            return (StatusCode::INTERNAL_SERVER_ERROR, "UDP probe failed").into_response();
        }
    };
    let port = socket.local_addr().map_or(0, |a| a.port());
    let handshake = request.handshake;
    let probe = tokio::task::spawn_blocking(move || {
        let _release = release;
        let outcome = crate::pyrowave_probe::send(&socket, peer, &request);
        match &outcome {
            Ok(o) => tracing::info!(
                kbps = request.kbps,
                burst_fps = request.burst.map_or(0, |b| b.fps),
                pace_kbps = request.burst.map_or(0, |b| b.pace_kbps),
                sent = o.sent,
                expected = o.expected,
                elapsed_ms = o.elapsed.as_secs_f64() * 1000.,
                send_retries = o.send_retries,
                last_send_error = o.last_send_error,
                "PyroWave UDP probe"
            ),
            Err(error) => {
                tracing::warn!(error = %format!("{error:#}"), "PyroWave UDP probe failed")
            }
        }
        outcome
    });
    let body = |outcome: Result<crate::pyrowave_probe::Outcome>| match outcome {
        Ok(o) => (
            200,
            vec![
                ("expected", o.expected.to_string()),
                ("sent", o.sent.to_string()),
                (
                    "elapsedMs",
                    format!("{:.3}", o.elapsed.as_secs_f64() * 1000.),
                ),
                ("sendRetries", o.send_retries.to_string()),
                ("lastSendError", o.last_send_error.to_string()),
            ],
            None,
        ),
        Err(error) => (500, vec![], Some(format!("{error:#}"))),
    };
    if !handshake {
        let (code, fields, error) = body(probe.await.unwrap_or_else(|e| Err(e.into())));
        if code != 200 {
            return (StatusCode::INTERNAL_SERVER_ERROR, "UDP probe failed").into_response();
        }
        return xml(code, &fields, error);
    }
    // The client learns the host's UDP port from the headers, sends its token
    // there, and reads the result once the probe has run.
    let (mut writer, reader) = tokio::io::duplex(4096);
    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        let (code, fields, error) = body(probe.await.unwrap_or_else(|e| Err(e.into())));
        let response = xml(code, &fields, error);
        if let Ok(bytes) = axum::body::to_bytes(response.into_body(), 64 * 1024).await {
            let _ = writer.write_all(&bytes).await;
        }
    });
    (
        [
            (
                header::CONTENT_TYPE,
                "application/xml; charset=utf-8".to_owned(),
            ),
            (header::CACHE_CONTROL, "no-store".to_owned()),
            (
                header::HeaderName::from_static("x-pyrowave-udp-port"),
                port.to_string(),
            ),
        ],
        axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(reader)),
    )
        .into_response()
}
