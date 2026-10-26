use super::*;
use crate::state::{Host, test_support::Fixture};
use axum::{
    body::to_bytes,
    http::{Request as HttpRequest, StatusCode},
};
use butterpollo_core::{session::Role, state::Client};
use std::sync::{Weak, atomic::AtomicUsize, atomic::Ordering};
use tower::ServiceExt;

/// A desktop with S3, unless the test sets other states. Counts suspends
/// and records whether the lifecycle gate was held. With `hold`, a suspend
/// waits for the test to let it return.
struct Fake {
    host: Weak<Host>,
    states: Mutex<SleepStates>,
    suspends: AtomicUsize,
    gate_held: std::sync::atomic::AtomicBool,
    hold: Mutex<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>>,
}
impl Backend for Fake {
    fn states(&self) -> Result<SleepStates> {
        Ok(*self.states.lock().unwrap())
    }
    fn suspend(&self) -> Result<()> {
        self.suspends.fetch_add(1, Ordering::SeqCst);
        let host = self.host.upgrade().unwrap();
        self.gate_held
            .store(host.launch_transition.try_lock().is_err(), Ordering::SeqCst);
        let hold = self.hold.lock().unwrap().take();
        if let Some((entered, release)) = hold {
            entered.send(()).unwrap();
            release.recv().unwrap();
        }
        Ok(())
    }
}
fn fake(f: &Fixture) -> Arc<Fake> {
    let fake = Arc::new(Fake {
        host: Arc::downgrade(&f.host),
        states: Mutex::new(DESKTOP),
        suspends: AtomicUsize::new(0),
        gate_held: Default::default(),
        hold: Mutex::new(None),
    });
    f.host.power.set_backend(fake.clone());
    fake
}
/// A paired device; `allow_sleep` as the console stores it.
fn device(f: &Fixture, allow_sleep: bool) -> Client {
    let client = f.client(u32::MAX);
    if allow_sleep {
        f.host.paired.write().unwrap().clients[0]
            .extra
            .insert("allow_sleep".into(), true.into());
    }
    client
}
struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    root: xmltree::Element,
}
impl Answer {
    fn field(&self, name: &str) -> String {
        self.root
            .get_child(name)
            .and_then(|e| e.get_text())
            .unwrap_or_default()
            .into_owned()
    }
    fn message(&self) -> &str {
        &self.root.attributes["status_message"]
    }
}
async fn call(f: &Fixture, https: bool, request: HttpRequest<Body>) -> Response {
    call_ending(f, https, &ConnectionEnd::default(), request).await
}
/// `end` stands in for the connection; the test says how it ended.
async fn call_ending(
    f: &Fixture,
    https: bool,
    end: &ConnectionEnd,
    request: HttpRequest<Body>,
) -> Response {
    crate::nvhttp::router(f.host.clone(), https)
        .layer(Extension(f.connection(https)))
        .layer(Extension(end.clone()))
        .oneshot(request)
        .await
        .unwrap()
}
async fn answer(response: Response) -> Answer {
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    assert_eq!(headers[header::CONTENT_LENGTH], bytes.len().to_string());
    let root = xmltree::Element::parse(bytes.as_ref()).unwrap();
    assert_eq!(root.attributes["status_code"], status.as_str());
    Answer {
        status,
        headers,
        root,
    }
}
async fn capabilities(f: &Fixture) -> Answer {
    let request = HttpRequest::get("/power-actions/v1")
        .body(Body::empty())
        .unwrap();
    answer(call(f, true, request).await).await
}
fn sleep_request(token: &str) -> HttpRequest<Body> {
    HttpRequest::post(format!("/power-actions/v1/sleep?token={token}"))
        .body(Body::empty())
        .unwrap()
}
/// A sleep request whose connection then closes cleanly.
async fn sleep(f: &Fixture, token: &str) -> Answer {
    let end = ConnectionEnd::default();
    let answer = answer(call_ending(f, true, &end, sleep_request(token)).await).await;
    end.finish(true);
    answer
}
/// The blocking half of an accepted request is done once it drops the gate.
fn wait_for_gate(f: &Fixture) {
    let started = Instant::now();
    while f.host.launch_transition.try_lock().is_err() {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "gate still held"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
const DESKTOP: SleepStates = SleepStates {
    s1: false,
    s2: false,
    s3: true,
    modern_standby: false,
    hibernate: true,
};
const ANY_TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[tokio::test]
async fn routes_are_https_only_and_need_a_paired_client() {
    let f = Fixture::new();
    let fake = fake(&f);
    for request in [
        HttpRequest::get("/power-actions/v1")
            .body(Body::empty())
            .unwrap(),
        sleep_request(ANY_TOKEN),
    ] {
        assert_eq!(
            call(&f, false, request).await.status(),
            StatusCode::NOT_FOUND
        );
    }
    // HTTPS, not paired.
    let unpaired = capabilities(&f).await;
    assert_eq!(unpaired.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        unpaired.message(),
        "A paired TLS client identity is required"
    );
    assert_eq!(sleep(&f, ANY_TOKEN).await.status, StatusCode::UNAUTHORIZED);
    // Only POST spends a token; any other method gets the XML refusal.
    let get = HttpRequest::get(format!("/power-actions/v1/sleep?token={ANY_TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let refused = answer(call(&f, true, get).await).await;
    assert_eq!(
        (refused.status, refused.message()),
        (
            StatusCode::METHOD_NOT_ALLOWED,
            "Use POST for a sleep request"
        )
    );
    assert_eq!(fake.suspends.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn sleep_is_off_until_the_device_is_allowed() {
    let f = Fixture::new();
    let fake = fake(&f);
    device(&f, false);
    let off = capabilities(&f).await;
    assert_eq!(off.status, StatusCode::OK);
    assert_eq!(
        (
            off.field("PowerActionVersion"),
            off.field("SleepSupported"),
            off.field("SleepAllowed"),
            off.field("SleepAvailable"),
            off.field("SleepToken"),
        ),
        (
            "1".into(),
            "1".into(),
            "0".into(),
            "0".into(),
            String::new()
        )
    );
    for (name, value) in [
        (header::CONTENT_TYPE, "application/xml"),
        (header::CACHE_CONTROL, "no-store"),
        (header::CONNECTION, "close"),
    ] {
        assert_eq!(off.headers[name], value);
    }
    let refused = sleep(&f, ANY_TOKEN).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    assert_eq!(
        refused.message(),
        "Sleep control is disabled for this paired client"
    );
    assert_eq!(fake.suspends.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn an_accepted_request_suspends_once_with_the_gate_held() {
    let f = Fixture::new();
    let fake = fake(&f);
    device(&f, true);
    let offer = capabilities(&f).await;
    assert_eq!(offer.field("SleepAvailable"), "1");
    assert_eq!(offer.field("SleepActiveSessionPolicy"), "idle-only");
    assert_eq!(offer.field("SleepTokenLifetimeSeconds"), "30");
    let token = offer.field("SleepToken");
    assert_eq!(token.len(), 64);
    let accepted = sleep(&f, &token).await;
    assert_eq!(accepted.status, StatusCode::ACCEPTED);
    assert_eq!(accepted.field("SleepAccepted"), "1");
    assert_eq!(accepted.headers[header::CONNECTION], "close");
    wait_for_gate(&f);
    assert_eq!(fake.suspends.load(Ordering::SeqCst), 1);
    assert!(fake.gate_held.load(Ordering::SeqCst));
    // A replay after wake is refused and does not suspend again.
    let replay = sleep(&f, &token).await;
    assert_eq!(replay.status, StatusCode::CONFLICT);
    assert_eq!(
        replay.message(),
        "Sleep token expired or already used; confirm the action again"
    );
    wait_for_gate(&f);
    assert_eq!(fake.suspends.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn an_undelivered_answer_or_a_revoked_device_does_not_suspend() {
    let f = Fixture::new();
    let fake = fake(&f);
    device(&f, true);
    // The connection failed after the answer was handed to hyper.
    let token = capabilities(&f).await.field("SleepToken");
    let end = ConnectionEnd::default();
    let accepted = answer(call_ending(&f, true, &end, sleep_request(&token)).await).await;
    assert_eq!(accepted.status, StatusCode::ACCEPTED);
    end.finish(false);
    wait_for_gate(&f);
    assert_eq!(fake.suspends.load(Ordering::SeqCst), 0);
    // Revoked while the answer is on its way: the opt-in is read again.
    let token = capabilities(&f).await.field("SleepToken");
    let end = ConnectionEnd::default();
    let response = call_ending(&f, true, &end, sleep_request(&token)).await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    f.host.paired.write().unwrap().clients[0]
        .extra
        .remove("allow_sleep");
    answer(response).await;
    end.finish(true);
    wait_for_gate(&f);
    assert_eq!(fake.suspends.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_busy_host_offers_no_token_and_refuses_sleep() {
    let f = Fixture::new();
    let fake = fake(&f);
    let client = device(&f, true);
    let token = capabilities(&f).await.field("SleepToken");
    let busy = "End all applications and sessions before requesting sleep";
    // A stream.
    let session = f.session(client.clone(), Role::Stream);
    let offer = capabilities(&f).await;
    assert_eq!(
        (offer.field("SleepAllowed"), offer.field("SleepAvailable")),
        ("1".into(), "0".into())
    );
    let refused = sleep(&f, &token).await;
    assert_eq!(
        (refused.status, refused.message()),
        (StatusCode::CONFLICT, busy)
    );
    f.host
        .sessions
        .lock()
        .unwrap()
        .active
        .remove(&session.launch.id);
    // A running app.
    let app = f.host.apps.read().unwrap()[0].clone();
    let running = crate::process::RunningApp::with_environment(&app, Default::default()).unwrap();
    *f.host.current_app.lock().unwrap() = Some(running);
    assert_eq!(sleep(&f, &token).await.message(), busy);
    let running = f.host.current_app.lock().unwrap().take();
    drop(running);
    // A launch in progress holds the gate.
    let request = Request::parse(true, Some(&format!("token={token}")), 0, false);
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let launching = f.host.launch_transition.lock().unwrap();
    sleep_blocking(
        &f.host,
        &f.connection(true),
        &ConnectionEnd::default(),
        &request,
        sender,
    );
    drop(launching);
    assert_eq!(answer(receiver.await.unwrap()).await.message(), busy);
    // None of these spent the token.
    assert_eq!(sleep(&f, &token).await.status, StatusCode::ACCEPTED);
    wait_for_gate(&f);
    assert_eq!(fake.suspends.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_launch_racing_an_accepted_sleep_waits_for_the_suspend() {
    let f = Fixture::new();
    let fake = fake(&f);
    device(&f, true);
    let (entered_tx, entered) = mpsc::channel();
    let (release, release_rx) = mpsc::channel();
    *fake.hold.lock().unwrap() = Some((entered_tx, release_rx));
    let token = capabilities(&f).await.field("SleepToken");
    assert_eq!(sleep(&f, &token).await.status, StatusCode::ACCEPTED);
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    // The paired list is free while the PC sleeps.
    assert!(f.host.paired.try_write().is_ok());
    let (launched_tx, launched) = mpsc::channel();
    let h = f.host.clone();
    let launch = std::thread::spawn(move || {
        let _transition = h.launch_transition.lock().unwrap();
        launched_tx.send(()).unwrap();
    });
    assert!(launched.recv_timeout(Duration::from_millis(200)).is_err());
    release.send(()).unwrap();
    launched.recv_timeout(Duration::from_secs(5)).unwrap();
    launch.join().unwrap();
    assert_eq!(fake.suspends.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn malformed_requests_are_refused_before_identity() {
    let f = Fixture::new();
    let fake = fake(&f);
    device(&f, true);
    let token = capabilities(&f).await.field("SleepToken");
    let shape = "Expected one sleep token and an empty body; browser origins are not accepted";
    let uri = format!("/power-actions/v1/sleep?token={token}");
    for request in [
        HttpRequest::post(&uri).body(Body::from("x")).unwrap(),
        HttpRequest::post(&uri)
            .header(header::ORIGIN, "https://example.com")
            .body(Body::empty())
            .unwrap(),
        HttpRequest::post(format!("{uri}&x=1"))
            .body(Body::empty())
            .unwrap(),
        HttpRequest::post("/power-actions/v1/sleep")
            .body(Body::empty())
            .unwrap(),
        // Past the router's 1 MiB limit too.
        HttpRequest::post(&uri)
            .body(Body::from(vec![b'x'; 2 << 20]))
            .unwrap(),
    ] {
        let refused = answer(call(&f, true, request).await).await;
        assert_eq!(
            (refused.status, refused.message()),
            (StatusCode::BAD_REQUEST, shape)
        );
    }
    assert_eq!(sleep(&f, &token).await.status, StatusCode::ACCEPTED);
    wait_for_gate(&f);
    assert_eq!(fake.suspends.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_connection_that_never_closes_does_not_suspend() {
    let f = Fixture::new();
    let fake = fake(&f);
    device(&f, true);
    let token = capabilities(&f).await.field("SleepToken");
    let end = ConnectionEnd::default();
    let started = Instant::now();
    let accepted = answer(call_ending(&f, true, &end, sleep_request(&token)).await).await;
    assert_eq!(accepted.status, StatusCode::ACCEPTED);
    // Still waiting for the connection to close, with the gate held.
    assert!(f.host.launch_transition.try_lock().is_err());
    wait_for_gate(&f);
    assert!(started.elapsed() >= DELIVERY_WAIT);
    assert_eq!(fake.suspends.load(Ordering::SeqCst), 0);
    // Too late: the request has given up.
    end.finish(true);
    assert_eq!(fake.suspends.load(Ordering::SeqCst), 0);
}

/// One request over a real TLS connection, served as the HTTPS port does.
/// The 64-byte pipe holds the answer back until the client reads it.
async fn over_tls(f: &Fixture, token: &str, read_answer: bool) -> Vec<u8> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let acceptor = crate::tls::acceptor(&f.host.identity, true).unwrap();
    let (near, far) = tokio::io::duplex(64);
    let connection = f.connection(true);
    let server = tokio::spawn(crate::tls::serve_connection(
        far,
        connection.peer,
        connection.local,
        crate::nvhttp::router(f.host.clone(), true),
        Some(acceptor),
    ));
    // TLS 1.2: no session tickets left in the pipe after the handshake.
    let client = crate::tls::tests::client(
        &rustls::version::TLS12,
        &f.host.identity.der,
        &f.host.identity.private_pem,
    );
    let mut stream = tokio_rustls::TlsConnector::from(Arc::new(client))
        .connect(
            rustls::pki_types::ServerName::try_from("localhost").unwrap(),
            near,
        )
        .await
        .unwrap();
    let request = format!(
        "POST /power-actions/v1/sleep?token={token} HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();
    let mut received = Vec::new();
    if read_answer {
        // The host may close without waiting for the client's close_notify.
        let _ = stream.read_to_end(&mut received).await;
    } else {
        // Accepted: the request holds the gate while its answer waits.
        let started = Instant::now();
        while f.host.launch_transition.try_lock().is_ok() {
            assert!(started.elapsed() < Duration::from_secs(5), "not accepted");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        drop(stream);
    }
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    received
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_reset_before_the_answer_is_flushed_does_not_suspend() {
    let f = Fixture::new();
    let fake = fake(&f);
    device(&f, true);
    let token = capabilities(&f).await.field("SleepToken");
    over_tls(&f, &token, false).await;
    wait_for_gate(&f);
    assert_eq!(fake.suspends.load(Ordering::SeqCst), 0);
    // A client that reads its answer is put to sleep.
    let token = capabilities(&f).await.field("SleepToken");
    let received = over_tls(&f, &token, true).await;
    assert!(
        received.starts_with(b"HTTP/1.1 202 "),
        "{}",
        String::from_utf8_lossy(&received)
    );
    wait_for_gate(&f);
    assert_eq!(fake.suspends.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn every_busy_reason_refuses_sleep() {
    let f = Fixture::new();
    let fake = fake(&f);
    let client = device(&f, true);
    let token = capabilities(&f).await.field("SleepToken");
    let busy = "End all applications and sessions before requesting sleep";
    // A stream still tearing down.
    f.host
        .sessions
        .lock()
        .unwrap()
        .teardown
        .insert("x".into(), Instant::now());
    assert_eq!(sleep(&f, &token).await.message(), busy);
    f.host.sessions.lock().unwrap().teardown.clear();
    // A WebRTC transport nobody took yet.
    let (handle, _) = crate::webrtc::media::tests::real_handle(Default::default());
    f.host.webrtc_links.put("x".into(), handle);
    assert_eq!(sleep(&f, &token).await.message(), busy);
    f.host.webrtc_links.discard("x");
    // A display kept for a client.
    f.host.monitors.lock().unwrap().insert(
        "x".into(),
        Arc::new(butterpollo_windows::display::Retained::inert("x")),
    );
    assert_eq!(sleep(&f, &token).await.message(), busy);
    f.host.monitors.lock().unwrap().clear();
    // A controller session in use; a parked one does not count.
    let controller = f
        .host
        .controller_sessions
        .acquire(&f.launch(client, Role::Stream), &Default::default())
        .unwrap();
    assert_eq!(sleep(&f, &token).await.message(), busy);
    drop(controller);
    assert!(!f.host.controller_sessions.in_use());
    assert_eq!(sleep(&f, &token).await.status, StatusCode::ACCEPTED);
    wait_for_gate(&f);
    assert_eq!(fake.suspends.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_modern_standby_only_pc_offers_and_allows_no_sleep() {
    let f = Fixture::new();
    let fake = fake(&f);
    device(&f, true);
    // Taken while the PC still had S3: the refusal is not about the token.
    let token = capabilities(&f).await.field("SleepToken");
    let unsupported = "Native sleep control is unavailable on this host";
    for hibernate in [true, false] {
        *fake.states.lock().unwrap() = SleepStates {
            modern_standby: true,
            hibernate,
            ..Default::default()
        };
        assert!(!f.host.power.supported());
        let offer = capabilities(&f).await;
        assert_eq!(
            (
                offer.status,
                offer.field("SleepSupported"),
                offer.field("SleepAllowed"),
                offer.field("SleepAvailable"),
                offer.field("SleepToken"),
            ),
            (
                StatusCode::OK,
                "0".into(),
                "1".into(),
                "0".into(),
                String::new()
            )
        );
        let refused = sleep(&f, &token).await;
        assert_eq!(
            (refused.status, refused.message()),
            (StatusCode::NOT_IMPLEMENTED, unsupported)
        );
    }
    assert_eq!(
        f.host.power.unavailable().as_deref(),
        Some("this PC only has Modern Standby, which Windows offers no call to enter")
    );
    *fake.states.lock().unwrap() = SleepStates {
        modern_standby: true,
        hibernate: true,
        ..Default::default()
    };
    assert_eq!(
        f.host.power.unavailable().as_deref(),
        Some("this PC only has Modern Standby, so Windows would hibernate it")
    );
    assert_eq!(fake.suspends.load(Ordering::SeqCst), 0);
    // S3 beside Modern Standby sleeps; the refusals did not spend the token.
    fake.states.lock().unwrap().s3 = true;
    assert_eq!(f.host.power.unavailable(), None);
    assert_eq!(sleep(&f, &token).await.status, StatusCode::ACCEPTED);
    wait_for_gate(&f);
    assert_eq!(fake.suspends.load(Ordering::SeqCst), 1);
    assert_eq!(capabilities(&f).await.field("SleepSupported"), "1");
}

#[test]
fn unreadable_power_capabilities_make_sleep_unavailable() {
    struct Unreadable;
    impl Backend for Unreadable {
        fn states(&self) -> Result<SleepStates> {
            anyhow::bail!("could not read this PC's power capabilities: denied")
        }
        fn suspend(&self) -> Result<()> {
            unreachable!()
        }
    }
    let f = Fixture::new();
    f.host.power.set_backend(Arc::new(Unreadable));
    assert!(!f.host.power.supported());
    assert_eq!(
        f.host.power.unavailable().as_deref(),
        Some("could not read this PC's power capabilities: denied")
    );
}
