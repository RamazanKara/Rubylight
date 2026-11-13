use super::*;
use crate::state::test_support::Fixture;
use butterpollo_core::webrtc::auth::proof_hex;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

#[test]
fn a_pages_connection_failure_reaches_a_default_log_once_per_session() {
    let mut throttle = Throttle::new();
    let mut reported = false;
    assert_eq!(
        client_line_is_loud("decoder started", &mut reported, &mut throttle),
        None
    );
    assert!(!reported);
    assert_eq!(
        client_line_is_loud(
            "connection failed: no-answer, offered 192.168.4.69:8001, Firefox/159.0",
            &mut reported,
            &mut throttle
        ),
        Some(0)
    );
    // The limit is past: the same session still says it once.
    throttle.last = Some(Instant::now() - Duration::from_secs(11));
    assert_eq!(
        client_line_is_loud(
            "connection failed: the same page again",
            &mut reported,
            &mut throttle
        ),
        None
    );
}

#[test]
fn a_page_reconnecting_in_a_loop_writes_one_warning_in_ten_seconds() {
    let mut throttle = Throttle::new();
    let failure = |throttle: &mut Throttle| {
        // Every reconnect is a new connection, whose flag is clear.
        client_line_is_loud("connection failed: no-answer", &mut false, throttle)
    };
    assert_eq!(failure(&mut throttle), Some(0));
    assert_eq!(failure(&mut throttle), None);
    assert_eq!(failure(&mut throttle), None);
    throttle.last = Some(Instant::now() - Duration::from_secs(11));
    assert_eq!(failure(&mut throttle), Some(2));
}

#[tokio::test]
async fn a_page_that_tries_again_on_its_socket_can_report_its_failure_again() {
    let f = Fixture::new();
    let h = f.host.clone();
    let peer = Peer {
        address: ([192, 168, 1, 9], 5000).into(),
        local: ([192, 168, 1, 2], 8000).into(),
        this_pc: false,
    };
    let Registered {
        id,
        closed: _closed,
    } = h.webrtc.open(peer.address.ip()).unwrap();
    let mut state = Connection::new(h.clone(), Limits::default(), peer, id);
    let device = h.webrtc.console_device(&h, "owner").unwrap();
    state.client = Some(Authenticated {
        client_id: device.webrtc().unwrap().0.to_owned(),
        name: device.name.clone(),
    });
    let mut out = Vec::new();
    let failure = json!({"version":2,"type":"client-log","lines":["connection failed: no-try"]});
    let settings = r#""appId":"0","video":{"width":1920,"height":1080,"fps":60,"codec":"hevc","bitrateKbps":20000,"hdr":false},"audio":{"channels":2}"#;
    for kind in ["start-session", "switch-session"] {
        state.handle(&failure.to_string(), &mut out).await;
        assert!(state.reported_failure, "{out:?}");
        let again = format!(r#"{{"version":2,"type":"{kind}",{settings}}}"#);
        state.handle(&again, &mut out).await;
        assert!(!state.reported_failure, "{kind}: {out:?}");
    }
}

#[test]
fn a_refusal_log_is_one_line_in_ten_seconds_with_the_skipped_count() {
    let mut throttle = Throttle::new();
    assert_eq!(throttle.allow(), Some(0));
    assert_eq!(throttle.allow(), None);
    assert_eq!(throttle.allow(), None);
    throttle.last = Some(Instant::now() - Duration::from_secs(11));
    assert_eq!(throttle.allow(), Some(2));
    assert_eq!(throttle.allow(), None);
}

#[test]
fn a_pairing_name_may_not_spell_a_pin_in_any_digits() {
    for name in [
        "Living room TV - PIN 4821",
        "TV 4-8-2-1",
        "Xbox, PIN ④⑧②①",
        "TV \u{ff14}",
        "TV ²",
        "TV \u{1d7d2}",
        "TV \u{0664}\u{0668}",
    ] {
        assert!(name_problem("4821", name).is_some(), "{name}");
    }
    for name in [
        "Living room TV 2024",
        "Xbox",
        "Edge on Windows",
        "SM-F966B",
        "Fernseher Wohnzimmer 📺",
        "客厅电视",
        "Télé du salon",
    ] {
        assert_eq!(name_problem("4821", name), None, "{name}");
    }
}

#[test]
fn a_pairing_name_may_not_hide_reorder_or_break_its_text() {
    for c in [
        '\u{00ad}',
        '\u{061c}',
        '\u{180e}',
        '\u{200b}',
        '\u{200c}',
        '\u{200d}',
        '\u{200e}',
        '\u{200f}',
        '\u{2028}',
        '\u{2029}',
        '\u{202a}',
        '\u{202b}',
        '\u{202c}',
        '\u{202d}',
        '\u{202e}',
        '\u{2060}',
        '\u{2066}',
        '\u{2067}',
        '\u{2068}',
        '\u{2069}',
        '\u{feff}',
        '\u{e0001}',
        '\u{e0041}',
    ] {
        let name = format!("{c}Pairing request from 192.168.4.20");
        assert_eq!(
            name_problem("4821", &name),
            Some("clientName may not contain format or line separator characters"),
            "U+{:04X}",
            c as u32
        );
    }
}

/// A minimal WebSocket client: enough to talk to the server in tests.
pub(crate) struct Ws {
    stream: TcpStream,
    /// The response to the upgrade request.
    pub(crate) head: String,
    /// The code and reason in the server's close frame, once it sent one with them.
    pub(crate) close_code: Option<u16>,
    pub(crate) close_reason: Option<String>,
}

impl Ws {
    pub(crate) async fn connect(address: SocketAddr) -> Self {
        Self::connect_with(address, "/", "").await
    }

    /// Upgrade `path`, adding `headers` (each line ending in `\r\n`) to the request.
    pub(crate) async fn connect_with(address: SocketAddr, path: &str, headers: &str) -> Self {
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream
            .write_all(
                format!("GET {path} HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n{headers}\r\n")
                    .as_bytes(),
            )
            .await
            .unwrap();
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            head.push(stream.read_u8().await.unwrap());
        }
        let head = String::from_utf8_lossy(&head).into_owned();
        assert!(head.starts_with("HTTP/1.1 101"), "{head}");
        Self {
            stream,
            head,
            close_code: None,
            close_reason: None,
        }
    }

    pub(crate) async fn send(&mut self, message: Value) {
        self.send_text(message.to_string().into_bytes()).await;
    }

    /// One masked text frame carrying `payload` as it is.
    async fn send_text(&mut self, payload: Vec<u8>) {
        let mut frame = vec![0x81u8];
        match payload.len() {
            n if n < 126 => frame.push(0x80 | n as u8),
            n if n <= u16::MAX as usize => {
                frame.push(0x80 | 126);
                frame.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                frame.push(0x80 | 127);
                frame.extend_from_slice(&(n as u64).to_be_bytes());
            }
        }
        let mask = [0x12u8, 0x34, 0x56, 0x78];
        frame.extend_from_slice(&mask);
        frame.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        // The server may close the socket before it has read it all.
        let _ = self.stream.write_all(&frame).await;
    }

    /// The next message; None once the server closed the socket.
    pub(crate) async fn next(&mut self) -> Option<Value> {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let first = self.stream.read_u8().await.ok()?;
                let second = self.stream.read_u8().await.ok()?;
                let length = match second & 0x7f {
                    126 => self.stream.read_u16().await.ok()? as usize,
                    127 => self.stream.read_u64().await.ok()? as usize,
                    n => n as usize,
                };
                let mut payload = vec![0; length];
                self.stream.read_exact(&mut payload).await.ok()?;
                match first & 0x0f {
                    1 => return serde_json::from_slice(&payload).ok(),
                    8 => {
                        self.close_code =
                            payload.get(..2).map(|c| u16::from_be_bytes([c[0], c[1]]));
                        self.close_reason = payload
                            .get(2..)
                            .map(|r| String::from_utf8_lossy(r).into_owned());
                        return None;
                    }
                    _ => {}
                }
            }
        })
        .await
        .expect("a message from the server")
    }

    /// The next message, which must have this type.
    pub(crate) async fn expect(&mut self, kind: &str) -> Value {
        let message = self
            .next()
            .await
            .unwrap_or_else(|| panic!("closed, wanted {kind}"));
        assert_eq!(message["type"], kind, "{message}");
        assert_eq!(message["version"], 2);
        message
    }

    pub(crate) async fn closed(&mut self) {
        assert!(
            self.next().await.is_none(),
            "the socket should have been closed"
        );
    }
}

struct Server {
    f: Fixture,
    address: SocketAddr,
    _running: Running,
}

impl Server {
    fn start() -> Self {
        Self::start_with(Limits::default())
    }

    fn start_with(limits: Limits) -> Self {
        let f = Fixture::new();
        let running =
            start_with(f.host.clone(), [127, 0, 0, 1].into(), 0, 0, None, limits).expect("starts");
        let address = f.host.webrtc.status().signalling.expect("listening");
        Self {
            f,
            address,
            _running: running,
        }
    }

    async fn connect(&self) -> (Ws, String) {
        let mut ws = Ws::connect(self.address).await;
        let nonce = ws.expect("auth-required").await["nonce"]
            .as_str()
            .unwrap()
            .to_owned();
        (ws, nonce)
    }

    /// Pair a device the way a TV does, approving through the console API.
    async fn pair(&self, name: &str) -> (String, String) {
        let (mut ws, _) = self.connect().await;
        ws.send(json!({"version":2,"type":"request-pairing","pin":"4321","clientName":name}))
            .await;
        let request = self.pending_request().await;
        crate::web::update_clients(
            &self.f.host,
            "POST",
            "/api/pin",
            &json!({"pin":"4321","uniqueid":request}),
        )
        .unwrap();
        let paired = ws.expect("paired").await;
        (
            paired["clientId"].as_str().unwrap().to_owned(),
            paired["clientSecret"].as_str().unwrap().to_owned(),
        )
    }

    async fn pending_request(&self) -> String {
        for _ in 0..200 {
            let pending = crate::web::update_clients(
                &self.f.host,
                "GET",
                "/api/clients/pending",
                &Value::Null,
            )
            .unwrap();
            if let Some(request) = pending["requests"].as_array().and_then(|r| r.first()) {
                assert_eq!(request["webrtc"], true);
                return request["uniqueid"].as_str().unwrap().to_owned();
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("no pairing request reached the console");
    }

    fn api(&self, path: &str, data: Value) -> Value {
        let method = if path.ends_with("list") {
            "GET"
        } else {
            "POST"
        };
        crate::web::update_clients(&self.f.host, method, path, &data).unwrap()
    }
}

fn proof(secret: &str, nonce: &str) -> String {
    proof_hex(secret, nonce).unwrap()
}

async fn sign_in(server: &Server, client_id: &str, secret: &str) -> Ws {
    let (mut ws, nonce) = server.connect().await;
    ws.send(json!({"version":2,"type":"authenticate","clientId":client_id,"proof":proof(secret, &nonce)}))
        .await;
    ws.expect("authenticated").await;
    ws.expect("gateway-status").await;
    ws.expect("capabilities").await;
    ws
}

#[tokio::test]
async fn a_tv_pairs_through_the_console_pin_page_and_signs_in_with_its_secret() {
    let server = Server::start();
    let (mut ws, _) = server.connect().await;
    ws.send(
        json!({"version":2,"type":"request-pairing","pin":"4321","clientName":"Living room TV"}),
    )
    .await;
    let request = server.pending_request().await;
    // A wrong PIN is refused, once; the request is spent.
    crate::web::update_clients(
        &server.f.host,
        "POST",
        "/api/pin",
        &json!({"pin":"1111","uniqueid":request}),
    )
    .unwrap();
    let refused = ws.expect("error").await;
    assert_eq!(refused["code"], "incorrect-pin");
    assert_eq!(refused["requestType"], "request-pairing");
    assert!(server.f.host.paired.read().unwrap().clients.is_empty());
    ws.send(
        json!({"version":2,"type":"request-pairing","pin":"4321","clientName":"Living room TV"}),
    )
    .await;
    let request = server.pending_request().await;
    crate::web::update_clients(
        &server.f.host,
        "POST",
        "/api/pin",
        &json!({"pin":"4321","uniqueid":request,"name":"Den TV"}),
    )
    .unwrap();
    let paired = ws.expect("paired").await;
    ws.expect("gateway-status").await;
    ws.expect("capabilities").await;
    let client_id = paired["clientId"].as_str().unwrap();
    let secret = paired["clientSecret"].as_str().unwrap();
    assert_eq!(client_id.len(), 32);
    assert_eq!(secret.len(), 64);
    {
        let state = server.f.host.paired.read().unwrap();
        let client = &state.clients[0];
        assert_eq!(client.name, "Den TV");
        assert!(client.cert.is_empty());
        assert_eq!(client.webrtc(), Some((client_id, secret)));
    }
    // The credentials never reach the console.
    let list = server.api("/api/clients/list", Value::Null);
    assert_eq!(list["clients"][0]["webrtc"], json!({"clientId": client_id}));
    assert!(!list.to_string().contains(secret));
    // The same device signs in on a new connection.
    let mut again = sign_in(&server, client_id, secret).await;
    again.send(json!({"version":2,"type":"get-apps"})).await;
    let apps = again.expect("apps").await;
    assert!(!apps["apps"].as_array().unwrap().is_empty(), "{apps}");
    assert!(
        apps["apps"][0]["id"]
            .as_str()
            .unwrap()
            .parse::<u32>()
            .is_ok()
    );
}

#[tokio::test]
async fn a_wrong_proof_fails_but_the_connection_can_still_ask_to_pair() {
    let server = Server::start();
    let (client_id, secret) = server.pair("TV").await;
    let (mut ws, nonce) = server.connect().await;
    ws.send(json!({"version":2,"type":"authenticate","clientId":client_id,"proof":proof(&"0".repeat(64), &nonce)}))
        .await;
    assert_eq!(ws.expect("error").await["code"], "authentication-failed");
    // The nonce is spent: even the right proof fails now.
    ws.send(json!({"version":2,"type":"authenticate","clientId":client_id,"proof":proof(&secret, &nonce)}))
        .await;
    assert_eq!(ws.expect("error").await["code"], "authentication-failed");
    // An unknown device gets the same answer as a wrong secret.
    let (mut unknown, nonce) = server.connect().await;
    unknown
        .send(json!({"version":2,"type":"authenticate","clientId":"ab".repeat(16),"proof":proof(&secret, &nonce)}))
        .await;
    assert_eq!(
        unknown.expect("error").await["code"],
        "authentication-failed"
    );
    // Nothing but pairing works before signing in.
    unknown.send(json!({"version":2,"type":"get-apps"})).await;
    assert_eq!(unknown.expect("error").await["code"], "not-authenticated");
    unknown
        .send(json!({"version":2,"type":"request-pairing","pin":"1234","clientName":"Other"}))
        .await;
    server.pending_request().await;
}

#[tokio::test]
async fn disabling_or_unpairing_drops_a_live_connection_and_blocks_the_next() {
    let server = Server::start();
    let (client_id, secret) = server.pair("TV").await;
    let uuid = server.f.host.paired.read().unwrap().clients[0].uuid.clone();
    let mut live = sign_in(&server, &client_id, &secret).await;
    server.api("/api/clients/update", json!({"uuid":uuid,"enabled":false}));
    assert_eq!(live.expect("error").await["code"], "client-disabled");
    live.closed().await;
    // A disabled device is told so when it signs in again.
    let (mut ws, nonce) = server.connect().await;
    ws.send(json!({"version":2,"type":"authenticate","clientId":client_id,"proof":proof(&secret, &nonce)}))
        .await;
    assert_eq!(ws.expect("error").await["code"], "client-disabled");
    // Enabled again, it signs in; unpaired, it is dropped and refused.
    server.api("/api/clients/update", json!({"uuid":uuid,"enabled":true}));
    let mut live = sign_in(&server, &client_id, &secret).await;
    server.api("/api/clients/unpair", json!({"uuid":uuid}));
    assert_eq!(live.expect("error").await["code"], "authentication-failed");
    live.closed().await;
    let (mut ws, nonce) = server.connect().await;
    ws.send(json!({"version":2,"type":"authenticate","clientId":client_id,"proof":proof(&secret, &nonce)}))
        .await;
    assert_eq!(ws.expect("error").await["code"], "authentication-failed");
}

#[tokio::test]
async fn the_console_cannot_overwrite_a_devices_credentials() {
    let server = Server::start();
    let (client_id, secret) = server.pair("TV").await;
    let uuid = server.f.host.paired.read().unwrap().clients[0].uuid.clone();
    server.api(
        "/api/clients/update",
        json!({"uuid":uuid,"webrtc":{"clientId":"ab".repeat(16),"secret":"cd".repeat(32)}}),
    );
    server.api("/api/clients/update", json!({"uuid":uuid,"webrtc":null}));
    assert_eq!(
        server.f.host.paired.read().unwrap().clients[0].webrtc(),
        Some((client_id.as_str(), secret.as_str()))
    );
}

#[tokio::test]
async fn a_fifth_unauthenticated_connection_from_one_peer_pushes_out_its_oldest() {
    let server = Server::start();
    let mut sockets = Vec::new();
    for _ in 0..4 {
        sockets.push(server.connect().await.0);
    }
    let mut fifth = server.connect().await.0;
    sockets[0].closed().await;
    // The others stay open: a probe connection may idle.
    fifth.send(json!({"version":2,"type":"get-apps"})).await;
    assert_eq!(fifth.expect("error").await["code"], "not-authenticated");
    sockets[1]
        .send(json!({"version":2,"type":"get-apps"}))
        .await;
    assert_eq!(
        sockets[1].expect("error").await["code"],
        "not-authenticated"
    );
}

fn short(handshake: u64, unauthenticated: u64, pairing: u64) -> Limits {
    Limits {
        handshake: Duration::from_millis(handshake),
        unauthenticated: Duration::from_millis(unauthenticated),
        pairing: Duration::from_millis(pairing),
    }
}

#[tokio::test]
async fn a_tcp_connection_that_sends_nothing_is_closed() {
    let server = Server::start_with(short(300, 60_000, 60_000));
    let mut silent = TcpStream::connect(server.address).await.unwrap();
    let mut byte = [0u8; 1];
    let read = tokio::time::timeout(Duration::from_secs(5), silent.read(&mut byte))
        .await
        .expect("the server closed the idle connection");
    assert!(matches!(read, Ok(0) | Err(_)));
    // A peer that starts a request and stalls is closed too.
    let mut slow = TcpStream::connect(server.address).await.unwrap();
    slow.write_all(b"GET / HTTP/1.1\r\nHost: te").await.unwrap();
    let read = tokio::time::timeout(Duration::from_secs(5), slow.read(&mut byte))
        .await
        .expect("the server closed the stalled request");
    assert!(matches!(read, Ok(0) | Err(_)));
}

#[tokio::test]
async fn an_unauthenticated_websocket_that_stays_silent_is_closed() {
    let server = Server::start_with(short(10_000, 400, 60_000));
    let (mut ws, _) = server.connect().await;
    ws.closed().await;
}

#[tokio::test]
async fn a_paired_connection_outlives_the_handshake_and_idle_limits() {
    // Both limits are far shorter than the wait below.
    let server = Server::start_with(short(300, 300, 60_000));
    let (client_id, secret) = server.pair("TV").await;
    let mut ws = sign_in(&server, &client_id, &secret).await;
    tokio::time::sleep(Duration::from_millis(900)).await;
    ws.send(json!({"version":2,"type":"get-apps"})).await;
    ws.expect("apps").await;
}

#[tokio::test]
async fn a_pairing_request_keeps_its_socket_until_it_expires_and_then_says_so() {
    let server = Server::start_with(short(10_000, 200, 700));
    let (mut ws, _) = server.connect().await;
    ws.send(json!({"version":2,"type":"request-pairing","pin":"4321","clientName":"TV"}))
        .await;
    server.pending_request().await;
    // Past the idle limit, the request holds the socket open.
    let error = ws.expect("error").await;
    assert_eq!(error["code"], "pairing-expired");
    assert_eq!(error["requestType"], "request-pairing");
    assert!(server.f.host.pins.lock().unwrap().is_empty());
    ws.closed().await;
}

#[tokio::test]
async fn pairing_turned_off_refuses_a_request_without_a_console_entry_or_notice() {
    let server = Server::start();
    server
        .f
        .host
        .config
        .write()
        .unwrap()
        .values
        .insert("enable_pairing".into(), "false".into());
    let (mut ws, _) = server.connect().await;
    ws.send(json!({"version":2,"type":"request-pairing","pin":"4321","clientName":"TV"}))
        .await;
    let error = ws.expect("error").await;
    assert_eq!(error["code"], "pairing-failed");
    assert!(server.f.host.pins.lock().unwrap().is_empty());
    // No notification was raised.
    assert_eq!(server.f.host.webrtc.notices(), 0);
    assert!(server.f.host.paired.read().unwrap().clients.is_empty());
    // The connection is still usable, and pairing works again once it is on.
    server
        .f
        .host
        .config
        .write()
        .unwrap()
        .values
        .insert("enable_pairing".into(), "true".into());
    ws.send(json!({"version":2,"type":"request-pairing","pin":"4321","clientName":"TV"}))
        .await;
    server.pending_request().await;
}

#[tokio::test]
async fn asking_again_does_not_raise_another_notice() {
    let server = Server::start();
    // No throttle window, so only the connection's own `again` check can
    // hold back the second notice.
    server.f.host.webrtc.set_notify_every(Duration::ZERO);
    let (mut ws, _) = server.connect().await;
    ws.send(json!({"version":2,"type":"request-pairing","pin":"4321","clientName":"TV"}))
        .await;
    server.pending_request().await;
    assert_eq!(server.f.host.webrtc.notices(), 1);
    ws.send(json!({"version":2,"type":"request-pairing","pin":"8765","clientName":"TV"}))
        .await;
    ws.send(json!({"version":2,"type":"get-apps"})).await;
    assert_eq!(ws.expect("error").await["code"], "not-authenticated");
    assert_eq!(server.f.host.pins.lock().unwrap().len(), 1);
    assert_eq!(server.f.host.webrtc.notices(), 1);
}

#[tokio::test]
async fn a_sixth_pairing_request_in_a_minute_is_refused_with_no_console_entry() {
    let server = Server::start();
    let (mut ws, _) = server.connect().await;
    for n in 0..crate::webrtc::clients::MAX_PAIRING_REQUESTS {
        ws.send(json!({"version":2,"type":"request-pairing","pin":format!("{:04}", 1000 + n),"clientName":"TV"}))
            .await;
    }
    ws.send(json!({"version":2,"type":"request-pairing","pin":"9999","clientName":"TV"}))
        .await;
    let refused = ws.expect("error").await;
    assert_eq!(refused["code"], "pairing-failed");
    assert_eq!(refused["requestType"], "request-pairing");
    assert!(
        refused["message"]
            .as_str()
            .unwrap()
            .contains("wait a minute")
    );
    // Each request replaced the one before: the fifth waits.
    let request = server.pending_request().await;
    // A new socket of the same peer is refused too; the request already waiting still works.
    let (mut again, _) = server.connect().await;
    again
        .send(json!({"version":2,"type":"request-pairing","pin":"4321","clientName":"TV"}))
        .await;
    assert_eq!(again.expect("error").await["code"], "pairing-failed");
    let pins = server
        .f
        .host
        .pins
        .lock()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(pins, std::slice::from_ref(&request));
    crate::web::update_clients(
        &server.f.host,
        "POST",
        "/api/pin",
        &json!({"pin":"1004","uniqueid":request}),
    )
    .unwrap();
    ws.expect("paired").await;
}

#[tokio::test]
async fn a_connection_task_that_panics_or_is_aborted_frees_its_slots() {
    let f = Fixture::new();
    let h = f.host.clone();
    let peer = Peer {
        address: ([192, 168, 1, 9], 5000).into(),
        local: ([192, 168, 1, 2], 8000).into(),
        this_pc: false,
    };
    let ip = peer.address.ip();
    // A connection with a pairing request, as a TV waiting for its PIN has.
    let claim = |h: &Shared| {
        let Registered { id, closed } = h.webrtc.open(ip).expect("a slot");
        let mut state = Connection::new(h.clone(), Limits::default(), peer, id);
        let (request_id, entered) = h.webrtc.begin_pairing(h, "4321", "TV", ip);
        state.pairing = Some(Pairing {
            request_id,
            entered,
            deadline: tokio::time::Instant::now() + Duration::from_secs(60),
        });
        (state, closed)
    };
    let (state, closed) = claim(&h);
    assert_eq!(h.webrtc.slots(), 1);
    assert_eq!(h.pins.lock().unwrap().len(), 1);
    let panicked = tokio::spawn(async move {
        let _held = (state, closed);
        panic!("a connection task failed");
    });
    assert!(panicked.await.unwrap_err().is_panic());
    assert_eq!(h.webrtc.slots(), 0);
    assert_eq!(h.webrtc.connections(), 0);
    assert!(h.pins.lock().unwrap().is_empty());

    let (state, closed) = claim(&h);
    let waiting = tokio::spawn(async move {
        let _held = (state, closed);
        std::future::pending::<()>().await;
    });
    tokio::task::yield_now().await;
    assert_eq!(h.webrtc.slots(), 1);
    waiting.abort();
    assert!(waiting.await.unwrap_err().is_cancelled());
    assert_eq!(h.webrtc.slots(), 0);
    assert_eq!(h.webrtc.connections(), 0);
    assert!(h.pins.lock().unwrap().is_empty());
    // The peer has its whole share again.
    let held: Vec<_> = (0..4).map(|_| h.webrtc.open(ip).unwrap()).collect();
    assert_eq!(held.len(), 4);
}

#[tokio::test]
async fn a_message_over_256_kib_closes_the_connection() {
    let server = Server::start();
    let (mut ws, _) = server.connect().await;
    // Just under the limit is read, and answered as the bad message it is.
    ws.send_text(vec![b' '; MAX_MESSAGE - 16]).await;
    assert_eq!(ws.expect("error").await["code"], "invalid-json");
    ws.send_text(vec![b' '; MAX_MESSAGE + 1]).await;
    ws.closed().await;
}

#[tokio::test]
async fn the_first_device_of_either_kind_gets_everything_and_a_later_tv_view_only() {
    let server = Server::start();
    let (first, _) = server.pair("First TV").await;
    let (second, _) = server.pair("Second TV").await;
    let perm = |id: &str| {
        server
            .f
            .host
            .paired
            .read()
            .unwrap()
            .webrtc_client(id)
            .unwrap()
            .perm
    };
    assert_eq!(
        perm(&first),
        butterpollo_core::state::FIRST_DEVICE_PERMISSION
    );
    assert_eq!(
        perm(&second),
        butterpollo_core::state::LATER_DEVICE_PERMISSION
    );
    // Behind a GameStream phone, a TV is a later device too.
    let phone = Server::start();
    phone
        .f
        .client(butterpollo_core::state::FIRST_DEVICE_PERMISSION);
    let (tv, secret) = phone.pair("TV").await;
    assert_eq!(
        phone
            .f
            .host
            .paired
            .read()
            .unwrap()
            .webrtc_client(&tv)
            .unwrap()
            .perm,
        butterpollo_core::state::LATER_DEVICE_PERMISSION
    );
    // Viewing the library is part of view-only.
    let mut ws = sign_in(&phone, &tv, &secret).await;
    ws.send(json!({"version":2,"type":"get-apps"})).await;
    ws.expect("apps").await;
}

#[tokio::test]
async fn a_second_connection_from_the_same_device_replaces_the_first() {
    let server = Server::start();
    let (client_id, secret) = server.pair("TV").await;
    let mut first = sign_in(&server, &client_id, &secret).await;
    let mut second = sign_in(&server, &client_id, &secret).await;
    first.closed().await;
    assert_eq!(first.close_code, Some(REPLACED));
    second.send(json!({"version":2,"type":"get-apps"})).await;
    second.expect("apps").await;
    assert_eq!(server.f.host.webrtc.connections(), 1);
}

#[tokio::test]
async fn a_server_without_a_stream_pipeline_refuses_session_requests_and_bad_messages_get_errors() {
    let server = Server::start();
    let (client_id, secret) = server.pair("TV").await;
    let mut ws = sign_in(&server, &client_id, &secret).await;
    ws.send(json!({"version":2,"type":"start-session","appId":"0","video":{"width":1920,"height":1080,"fps":60,"codec":"hevc","bitrateKbps":20000,"hdr":false},"audio":{"channels":2}}))
        .await;
    let error = ws.expect("error").await;
    assert_eq!(error["code"], "internal-error");
    assert_eq!(error["requestType"], "start-session");
    ws.send(json!({"version":2,"type":"stop-host-session"}))
        .await;
    let error = ws.expect("error").await;
    assert_eq!(error["code"], "internal-error");
    assert_eq!(error["requestType"], "stop-host-session");
    // Nothing is running, so stopping reports idle.
    ws.send(json!({"version":2,"type":"stop-session"})).await;
    assert_eq!(ws.expect("session-status").await["state"], "idle");
    ws.send(json!({"version":2,"type":"no-such-message"})).await;
    ws.expect("error").await;
    ws.send(json!({"version":2,"type":"get-app-artwork","appId":"999999"}))
        .await;
    let artwork = ws.expect("app-artwork").await;
    assert_eq!(artwork["available"], false);
    // The connection still works.
    ws.send(json!({"version":2,"type":"get-apps"})).await;
    ws.expect("apps").await;
}

#[tokio::test]
async fn an_unanswered_pairing_request_leaves_the_console_when_the_tv_goes() {
    let server = Server::start();
    let (mut ws, _) = server.connect().await;
    ws.send(json!({"version":2,"type":"request-pairing","pin":"4321","clientName":"TV"}))
        .await;
    server.pending_request().await;
    drop(ws);
    for _ in 0..200 {
        if server.f.host.pins.lock().unwrap().is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the request stayed in the console");
}

#[tokio::test]
async fn a_taken_port_is_reported_not_moved() {
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = taken.local_addr().unwrap().port();
    let f = Fixture::new();
    assert!(start_on(f.host.clone(), [127, 0, 0, 1].into(), port, 0, None).is_none());
    let status = f.host.webrtc.status();
    assert!(status.enabled);
    assert!(status.signalling.is_none());
    assert!(status.error.unwrap().contains(&port.to_string()));
    assert!(f.host.warnings.snapshot().iter().any(|w| w.code == WARNING));
}

#[tokio::test]
async fn with_the_setting_off_nothing_listens() {
    let f = Fixture::new();
    assert!(!f.host.config.read().unwrap().webrtc_server());
    assert!(start(f.host.clone(), [127, 0, 0, 1].into(), None).is_none());
    let status = f.host.webrtc.status();
    assert!(!status.enabled);
    assert!(status.signalling.is_none() && status.discovery.is_none());
    assert!(!f.host.warnings.snapshot().iter().any(|w| w.code == WARNING));
}

#[test]
fn only_the_local_network_may_connect() {
    for peer in [
        "127.0.0.1",
        "10.0.0.5",
        "172.16.1.1",
        "192.168.1.9",
        "169.254.1.1",
        "100.64.0.1",
        "::1",
        "fd12::1",
        "fe80::1",
        "::ffff:192.168.1.9",
    ] {
        assert!(local_network(peer.parse().unwrap()), "{peer}");
    }
    for peer in [
        "8.8.8.8",
        "172.32.0.1",
        "100.128.0.1",
        "2001:db8::1",
        "::ffff:8.8.8.8",
    ] {
        assert!(!local_network(peer.parse().unwrap()), "{peer}");
    }
}

/// A test host can stream rates that are not offered by default; nothing else changes them.
#[test]
fn the_frame_rate_setting_replaces_the_offer_for_every_mode() {
    let mut config = butterpollo_core::config::Config::default();
    assert_eq!(frame_rate_offer(&config), FrameRateOffer::default());
    let saved = serde_json::json!({"webrtc_frame_rates": "30, 60,120,x"});
    config.update(saved.as_object().unwrap()).unwrap();
    let offer = frame_rate_offer(&config);
    assert_eq!(offer, FrameRateOffer::uniform(&[30, 60, 120]));
    assert_ne!(offer, FrameRateOffer::default());
}

/// An authenticated connection hears `capabilities` again when the probe or the rate setting
/// changes what is on offer, and only then.
#[test]
fn capabilities_are_sent_again_when_the_probe_or_the_rate_setting_changes() {
    let f = Fixture::new();
    let h = f.host.clone();
    let peer = Peer {
        address: ([192, 168, 1, 9], 5000).into(),
        local: ([192, 168, 1, 2], 8000).into(),
        this_pc: false,
    };
    let Registered {
        id,
        closed: _closed,
    } = h.webrtc.open(peer.address.ip()).unwrap();
    let mut state = Connection::new(h.clone(), Limits::default(), peer, id);
    let refresh = |state: &mut Connection| {
        let mut out = Vec::new();
        state.refresh_capabilities(&mut out);
        out
    };
    // Nothing before authentication.
    assert!(refresh(&mut state).is_empty());
    state.client = Some(Authenticated {
        client_id: "client".into(),
        name: "TV".into(),
    });
    let mut out = Vec::new();
    state.startup(&mut out);
    assert!(matches!(out.last(), Some(ServerMessage::Capabilities(_))));
    assert!(refresh(&mut state).is_empty(), "nothing changed");
    let rates = |values: &str| {
        let saved = json!({ "webrtc_frame_rates": values });
        h.config
            .write()
            .unwrap()
            .update(saved.as_object().unwrap())
            .unwrap();
    };
    rates("30,60");
    assert_ne!(
        frame_rate_offer(&h.config.read().unwrap()),
        FrameRateOffer::default()
    );
    let again = refresh(&mut state);
    assert!(
        matches!(again.as_slice(), [ServerMessage::Capabilities(_)]),
        "{again:?}"
    );
    assert!(refresh(&mut state).is_empty(), "sent once");
    // A setting that parses to the same rates changes nothing on offer.
    rates("30, 60");
    assert!(refresh(&mut state).is_empty());
    // The probe finds AV1, or loses it.
    h.codecs
        .fetch_xor(0x10000, std::sync::atomic::Ordering::AcqRel);
    assert!(matches!(
        refresh(&mut state).as_slice(),
        [ServerMessage::Capabilities(_)]
    ));
    assert!(refresh(&mut state).is_empty());
}

/// The host's AMF throughput caps gate the frame rates it lists and accepts.
#[test]
fn the_offer_follows_the_amf_throughput() {
    let mbs = |w: u64, h: u64, fps: u64| w.div_ceil(16) * h.div_ceil(16) * fps;
    let rates = |probe: EncoderProbe, height: u32| {
        HostCapabilities::new(probe, FrameRateOffer::default())
            .capabilities()
            .video_modes
            .into_iter()
            .find(|m| m.height == height)
            .map(|m| m.frame_rates)
    };
    // No cap reported: 60 fps and below, as before the probe.
    let unknown = encoder_probe(0x10000 | 0x20000 | 0x200, [0; 3]);
    assert_eq!(rates(unknown, 2160), Some(vec![30, 60]));
    assert_eq!(rates(unknown, 720), Some(vec![30, 60]));
    let full = encoder_probe(0x10000 | 0x20000 | 0x200, [mbs(3840, 2160, 120); 3]);
    assert_eq!(rates(full, 2160), Some(vec![30, 60, 90, 120]));
    let slow = encoder_probe(0x10000 | 0x20000 | 0x200, [mbs(3840, 2160, 60); 3]);
    assert_eq!(slow.hevc_mb_rate, mbs(3840, 2160, 60));
    assert_eq!(rates(slow, 2160), Some(vec![30, 60]));
    assert_eq!(rates(slow, 1440), Some(vec![30, 60, 90, 120]));
}

/// The capabilities list follows the host's AMF probe bits, not the protocol table.
#[test]
fn capabilities_follow_the_amf_probe_bits() {
    let listed = |bits: u32| {
        let body = HostCapabilities::new(encoder_probe(bits, [0; 3]), FrameRateOffer::default())
            .capabilities();
        let value: Value =
            serde_json::from_str(&ServerMessage::Capabilities(body).to_json()).unwrap();
        let all = |field: &str| -> Vec<(String, Vec<String>)> {
            value["videoModes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|m| {
                    (
                        m["height"].to_string(),
                        m[field]
                            .as_array()
                            .map(|a| a.iter().map(|c| c.as_str().unwrap().into()).collect())
                            .unwrap_or_default(),
                    )
                })
                .collect()
        };
        (all("codecs"), all("hdrCodecs"))
    };
    let has = |modes: &[(String, Vec<String>)], codec: &str| {
        modes
            .iter()
            .any(|(_, codecs)| codecs.iter().any(|c| c == codec))
    };
    // H.264 and HEVC only: no AV1 anywhere.
    let (codecs, hdr) = listed(0x101);
    assert!(has(&codecs, "hevc") && !has(&codecs, "av1"));
    assert!(!has(&hdr, "av1") && !has(&hdr, "hevc"), "no Main10, no HDR");
    // HEVC Main10 adds HEVC HDR only.
    let (_, hdr) = listed(0x301);
    assert!(has(&hdr, "hevc") && !has(&hdr, "av1"));
    // AV1 8-bit lists AV1 but not as HDR.
    let (codecs, hdr) = listed(0x10101);
    assert!(has(&codecs, "av1") && !has(&hdr, "av1"));
    // AV1 with 10-bit is HDR too.
    let (codecs, hdr) = listed(0x30101);
    assert!(has(&codecs, "av1") && has(&hdr, "av1"));
}
