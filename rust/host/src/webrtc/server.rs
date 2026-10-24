//! The WebRTC signalling server: a WebSocket on `webrtc_port`, on its own
//! listener, for TVs and browsers that speak the Moonlight WebRTC protocol
//! (version 2). It pairs devices, authenticates them, lists apps and answers
//! `capabilities`. `start-session` and `switch-session` are not wired to a
//! stream yet and answer `unsupported-settings`.
//!
//! The WebSocket is plain `ws://`. Authentication keeps other devices on the
//! network from using or interrupting a stream; it hides nothing. Peers
//! outside the local network are closed before the first message.

use super::{
    clients::{Close, Registered, Status},
    discovery,
};
use crate::{
    network::{Reach, reach},
    state::{Host, Shared},
};
use axum::{
    Extension, Router,
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::Response,
    routing::get,
};
use butterpollo_core::{
    catalog,
    webrtc::{
        auth::{Challenge, PAIRING_LIFETIME},
        capabilities::{EncoderProbe, FrameRateOffer, HostCapabilities},
        protocol::{
            AppEntry, ClientMessage, ErrorCode, ProtocolError, ServerMessage, SessionState,
            parse_client_message,
        },
    },
};
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto::Builder,
    service::TowerToHyperService,
};
use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{sync::oneshot, task::JoinHandle};

/// The console warning code for a server that could not start.
pub const WARNING: &str = "webrtc_server";

/// The most one client message may hold. `client-log` and an SDP answer are
/// the largest messages a client sends.
const MAX_MESSAGE: usize = 256 * 1024;
/// Sockets accepted at once, authenticated or not. A flood beyond this is
/// closed at once.
const MAX_SOCKETS: usize = 128;
/// Artwork larger than this is not sent over the WebSocket.
const MAX_ARTWORK: u64 = 4 * 1024 * 1024;
/// How often the connection looks for a changed encoder probe.
const PROBE_CHECK: Duration = Duration::from_secs(5);
/// "List applications", the permission bit GameStream's `applist` needs.
const LIST_APPS: u32 = 1 << 24;

/// The running server. Dropping it stops the listener, discovery and every
/// connection.
pub struct Running {
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for Running {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// Start the server if `webrtc_server` is on; otherwise open no socket and
/// start no thread. A port that can't be bound is reported in the console and
/// the log, never moved.
pub fn start(h: Shared, bind: IpAddr) -> Option<Running> {
    let config = h.config.read().unwrap().clone();
    if !config.webrtc_server() {
        h.webrtc.set_status(Status::default());
        h.warnings.clear(WARNING);
        return None;
    }
    start_on(h, bind, config.webrtc_port(), discovery::PORT)
}

/// [`start`] on chosen ports (tests use ephemeral ones; `0` picks one).
pub fn start_on(h: Shared, bind: IpAddr, port: u16, discovery_port: u16) -> Option<Running> {
    let mut status = Status {
        enabled: true,
        ..Status::default()
    };
    let listener = match crate::network::tcp((bind, port).into()) {
        Ok(listener) => listener,
        Err(error) => {
            let message = format!(
                "The WebRTC server could not listen on {bind} port {port}: {error:#}. Another program may be using the port; choose another in Settings, Network."
            );
            tracing::error!("{message}");
            h.warnings.set(WARNING, &message);
            status.error = Some(message);
            h.webrtc.set_status(status);
            return None;
        }
    };
    let signalling = listener.local_addr().unwrap_or((bind, port).into());
    status.signalling = Some(signalling);
    let mut tasks = Vec::new();
    let mut discovery_address = None;
    match discovery::bind((bind, discovery_port).into()) {
        Ok(socket) => {
            discovery_address = socket.local_addr().ok();
            tasks.push(tokio::spawn(discovery::serve(
                h.clone(),
                socket,
                signalling.port(),
            )));
        }
        Err(error) => {
            let message = format!(
                "WebRTC discovery could not listen on {bind} UDP port {discovery_port}: {error:#}. TVs will not find this PC on their own; add it by address."
            );
            tracing::error!("{message}");
            h.warnings.set(WARNING, &message);
            status.error = Some(message);
        }
    }
    if status.error.is_none() {
        h.warnings.clear(WARNING);
    }
    status.discovery = discovery_address;
    h.webrtc.set_status(status);
    tracing::info!(%signalling, discovery = ?discovery_address, "WebRTC server listening (unencrypted ws://, local network only)");
    tasks.push(tokio::spawn(accept(h, listener)));
    Some(Running { tasks })
}

#[derive(Clone)]
struct Context {
    h: Shared,
}

/// Where a connection came from and arrived at.
#[derive(Clone, Copy)]
struct Peer {
    address: SocketAddr,
    local: SocketAddr,
}

/// Logs refused peers, but not every one of a scan.
struct Throttle {
    last: Option<Instant>,
    skipped: u32,
}

impl Throttle {
    fn refused(&mut self, peer: SocketAddr) {
        let now = Instant::now();
        if self
            .last
            .is_some_and(|at| now.duration_since(at) < Duration::from_secs(10))
        {
            self.skipped += 1;
            return;
        }
        tracing::warn!(
            %peer,
            more = self.skipped,
            "WebRTC signalling closed a connection from outside the local network"
        );
        self.last = Some(now);
        self.skipped = 0;
    }
}

/// Loopback, private, link-local, ULA and carrier-grade NAT addresses; nothing the internet can reach.
fn local_network(peer: IpAddr) -> bool {
    reach(peer.to_canonical()) <= Reach::Lan
}

async fn accept(h: Shared, listener: tokio::net::TcpListener) {
    let router = Router::new()
        .fallback(get(upgrade))
        .with_state(Context { h });
    let sockets = Arc::new(tokio::sync::Semaphore::new(MAX_SOCKETS));
    let mut refused = Throttle {
        last: None,
        skipped: 0,
    };
    loop {
        let (socket, address) = crate::network::accept(&listener).await;
        if !local_network(address.ip()) {
            refused.refused(address);
            continue;
        }
        let Ok(permit) = sockets.clone().try_acquire_owned() else {
            tracing::debug!(%address, "WebRTC signalling is full; connection closed");
            continue;
        };
        let _ = socket.set_nodelay(true);
        let Ok(local) = socket.local_addr() else {
            continue;
        };
        let local = SocketAddr::new(local.ip().to_canonical(), local.port());
        let router = router.clone().layer(Extension(Peer { address, local }));
        tokio::spawn(async move {
            let _permit = permit;
            let _ = Builder::new(TokioExecutor::new())
                .serve_connection_with_upgrades(
                    TokioIo::new(socket),
                    TowerToHyperService::new(router),
                )
                .await;
        });
    }
}

async fn upgrade(
    State(context): State<Context>,
    Extension(peer): Extension<Peer>,
    socket: WebSocketUpgrade,
) -> Response {
    socket
        .max_message_size(MAX_MESSAGE)
        .max_frame_size(MAX_MESSAGE)
        .on_upgrade(move |socket| connection(context.h, peer, socket))
}

/// What a connection does after a message.
#[derive(Debug, PartialEq, Eq)]
enum Flow {
    Continue,
    Close,
}

struct Authenticated {
    client_id: String,
    name: String,
}

struct Pairing {
    request_id: String,
    entered: oneshot::Receiver<(String, String)>,
    deadline: tokio::time::Instant,
}

/// What came of a pairing request.
enum Outcome {
    Entered(Result<(String, String), oneshot::error::RecvError>),
    Expired,
}

struct Connection {
    h: Shared,
    id: u64,
    peer: Peer,
    challenge: Challenge,
    client: Option<Authenticated>,
    pairing: Option<Pairing>,
    probe: Option<EncoderProbe>,
}

async fn connection(h: Shared, peer: Peer, mut socket: WebSocket) {
    let Registered { id, mut closed } = h.webrtc.open();
    let mut state = Connection {
        challenge: h.webrtc.challenge(),
        h: h.clone(),
        id,
        peer,
        client: None,
        pairing: None,
        probe: None,
    };
    tracing::debug!(peer = %peer.address, id, "WebRTC connection opened");
    let mut changed = h.webrtc.subscribe();
    let mut tick = tokio::time::interval(PROBE_CHECK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut out = vec![state.auth_required()];
    let mut flow = Flow::Continue;
    loop {
        if !send(&mut socket, &mut out).await {
            break;
        }
        if flow == Flow::Close {
            let _ = socket.send(Message::Close(None)).await;
            break;
        }
        tokio::select! {
            message = socket.recv() => match message {
                Some(Ok(Message::Text(text))) => {
                    flow = state.handle(text.as_str(), &mut out).await;
                }
                Some(Ok(Message::Binary(_))) => out.push(ServerMessage::error(
                    "unknown",
                    ErrorCode::InvalidMessage,
                    "Messages must be text",
                )),
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
            Some(reason) = closed.recv() => {
                tracing::debug!(peer = %peer.address, id, ?reason, "WebRTC connection closed by the host");
                match reason {
                    Close::Replaced | Close::TooManyUnauthenticated => flow = Flow::Close,
                }
            }
            Ok(()) = changed.changed() => flow = state.recheck(&mut out),
            outcome = state.pairing_outcome() => flow = state.finish_pairing(outcome, &mut out),
            _ = tick.tick() => state.refresh_capabilities(&mut out),
        }
    }
    state.end();
    tracing::debug!(peer = %peer.address, id, "WebRTC connection closed");
}

/// Send what is queued. False when the socket is gone.
async fn send(socket: &mut WebSocket, out: &mut Vec<ServerMessage>) -> bool {
    for message in out.drain(..) {
        if socket
            .send(Message::Text(message.to_json().into()))
            .await
            .is_err()
        {
            return false;
        }
    }
    true
}

/// The request type to name in an error for a message that did not parse.
fn request_type(text: &str) -> String {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|value| value.get("type")?.as_str().map(str::to_owned))
        .filter(|kind| {
            !kind.is_empty()
                && kind.len() <= 32
                && kind.bytes().all(|b| b.is_ascii_lowercase() || b == b'-')
        })
        .unwrap_or_else(|| "unknown".into())
}

impl Connection {
    fn auth_required(&self) -> ServerMessage {
        let mac = discovery::mac_address(self.peer.local.ip());
        ServerMessage::auth_required(self.challenge.nonce().unwrap_or_default(), mac)
    }

    fn error(code: ErrorCode, request_type: &str, message: &str) -> ServerMessage {
        ProtocolError::new(code, message).into_message(request_type)
    }

    /// The device this connection authenticated as, if it is still paired and enabled.
    fn recheck(&mut self, out: &mut Vec<ServerMessage>) -> Flow {
        let Some(client) = &self.client else {
            return Flow::Continue;
        };
        match self.h.webrtc.verify(&self.h, &client.client_id) {
            Ok(_) => Flow::Continue,
            Err(code) => {
                tracing::info!(device = %client.name, %code, "WebRTC device disconnected");
                out.push(Self::error(
                    code,
                    "authenticate",
                    match code {
                        ErrorCode::ClientDisabled => "This device is disabled on the host",
                        _ => "This device is no longer paired",
                    },
                ));
                Flow::Close
            }
        }
    }

    async fn handle(&mut self, text: &str, out: &mut Vec<ServerMessage>) -> Flow {
        let message = match parse_client_message(text) {
            Ok(message) => message,
            Err(error) => {
                out.push(error.into_message(&request_type(text)));
                return Flow::Continue;
            }
        };
        let kind = message.type_name();
        if self.client.is_some() && self.recheck(out) == Flow::Close {
            return Flow::Close;
        }
        match message {
            ClientMessage::Authenticate { client_id, proof } => {
                self.authenticate(&client_id, &proof, out)
            }
            ClientMessage::RequestPairing { pin, client_name } => {
                self.request_pairing(&pin, &client_name, out)
            }
            _ if self.client.is_none() => out.push(Self::error(
                ErrorCode::NotAuthenticated,
                kind,
                "Authenticate or pair first",
            )),
            ClientMessage::GetApps => {
                if !self.allows(LIST_APPS) {
                    out.push(Self::error(
                        ErrorCode::NotAuthenticated,
                        kind,
                        "This device may not list applications",
                    ));
                } else {
                    let h = self.h.clone();
                    let apps = tokio::task::spawn_blocking(move || app_list(&h))
                        .await
                        .unwrap_or_default();
                    out.push(ServerMessage::Apps { apps });
                }
            }
            ClientMessage::GetAppArtwork { app_id } => {
                if !self.allows(LIST_APPS) {
                    out.push(Self::error(
                        ErrorCode::NotAuthenticated,
                        kind,
                        "This device may not list applications",
                    ));
                } else {
                    let h = self.h.clone();
                    let id = app_id.clone();
                    let artwork = tokio::task::spawn_blocking(move || artwork(&h, &id))
                        .await
                        .ok()
                        .flatten();
                    out.push(match artwork {
                        Some((mime_type, data)) => ServerMessage::AppArtwork {
                            app_id,
                            available: true,
                            mime_type: Some(mime_type.into()),
                            data: Some(data),
                        },
                        None => ServerMessage::AppArtwork {
                            app_id,
                            available: false,
                            mime_type: None,
                            data: None,
                        },
                    });
                }
            }
            ClientMessage::StartSession { .. } | ClientMessage::SwitchSession { .. } => {
                out.push(Self::error(
                    ErrorCode::UnsupportedSettings,
                    kind,
                    "Streaming over WebRTC is not available in this build yet",
                ));
            }
            ClientMessage::StopSession => {
                out.push(ServerMessage::session_status(SessionState::Idle, None));
            }
            ClientMessage::StopHostSession => out.push(Self::error(
                ErrorCode::UnsupportedMessage,
                kind,
                "stop-host-session is not available in this build yet",
            )),
            // Late signalling for a session that is not there.
            ClientMessage::Answer { .. } | ClientMessage::Candidate { .. } => {
                tracing::debug!(
                    id = self.id,
                    kind,
                    "WebRTC signalling without a session ignored"
                );
            }
            ClientMessage::ClientLog { lines } => {
                let name = self.client.as_ref().map_or("", |c| c.name.as_str());
                for line in lines {
                    tracing::debug!(device = name, "WebRTC client: {line}");
                }
            }
        }
        Flow::Continue
    }

    fn allows(&self, permission: u32) -> bool {
        self.client.as_ref().is_some_and(|authenticated| {
            self.h
                .webrtc
                .verify(&self.h, &authenticated.client_id)
                .is_ok_and(|client| client.allows(permission))
        })
    }

    fn authenticate(&mut self, client_id: &str, proof: &str, out: &mut Vec<ServerMessage>) {
        if self.client.is_some() {
            out.push(Self::error(
                ErrorCode::AlreadyAuthenticated,
                "authenticate",
                "This connection is already authenticated",
            ));
            return;
        }
        let client_id = client_id.to_ascii_lowercase();
        // The proof is checked first, so a device that does not hold the
        // secret learns nothing about the paired list.
        let secret = self.h.webrtc.secret(&self.h, &client_id);
        if !self.challenge.verify(secret.as_deref(), proof) {
            tracing::info!(peer = %self.peer.address, "WebRTC authentication failed");
            out.push(Self::error(
                ErrorCode::AuthenticationFailed,
                "authenticate",
                "Authentication failed",
            ));
            return;
        }
        let client = match self.h.webrtc.verify(&self.h, &client_id) {
            Ok(client) => client,
            Err(code) => {
                out.push(Self::error(
                    code,
                    "authenticate",
                    "This device is disabled on the host",
                ));
                return;
            }
        };
        if let Err(code) = self.h.webrtc.authenticate(self.id, &client_id) {
            out.push(Self::error(code, "authenticate", "Could not authenticate"));
            return;
        }
        self.cancel_pairing();
        tracing::info!(device = %client.name, peer = %self.peer.address, "WebRTC device authenticated");
        self.client = Some(Authenticated {
            client_id,
            name: client.name,
        });
        out.push(ServerMessage::Authenticated);
        self.startup(out);
    }

    fn request_pairing(&mut self, pin: &str, name: &str, out: &mut Vec<ServerMessage>) {
        if self.client.is_some() {
            out.push(Self::error(
                ErrorCode::AlreadyAuthenticated,
                "request-pairing",
                "This connection is already authenticated",
            ));
            return;
        }
        let name = if name.trim().is_empty() {
            "WebRTC client"
        } else {
            name.trim()
        };
        if butterpollo_core::pairing::validate_device_name(name).is_err() {
            out.push(Self::error(
                ErrorCode::InvalidMessage,
                "request-pairing",
                "clientName is not a valid device name",
            ));
            return;
        }
        // A new request replaces the connection's old one.
        self.cancel_pairing();
        let (request_id, entered) = self.h.webrtc.begin_pairing(&self.h, pin, name);
        self.pairing = Some(Pairing {
            request_id,
            entered,
            deadline: tokio::time::Instant::now() + PAIRING_LIFETIME,
        });
        tracing::info!(client = name, peer = %self.peer.address, "WebRTC pairing PIN required in the web interface");
        butterpollo_windows::tray::notify(
            "Pairing request",
            &format!("{name} wants to pair. Enter the PIN it shows in the Rubylight console."),
        );
    }

    fn cancel_pairing(&mut self) {
        if let Some(pairing) = self.pairing.take() {
            self.h.webrtc.cancel_pairing(&self.h, &pairing.request_id);
        }
    }

    /// Resolves when the administrator entered the PIN, the request was
    /// withdrawn, or it ran out; never without a request.
    async fn pairing_outcome(&mut self) -> Outcome {
        match &mut self.pairing {
            None => std::future::pending().await,
            Some(pairing) => tokio::select! {
                entered = &mut pairing.entered => Outcome::Entered(entered),
                _ = tokio::time::sleep_until(pairing.deadline) => Outcome::Expired,
            },
        }
    }

    fn finish_pairing(&mut self, outcome: Outcome, out: &mut Vec<ServerMessage>) -> Flow {
        let Some(pairing) = self.pairing.take() else {
            return Flow::Continue;
        };
        let failure = |code, text: &str| Self::error(code, "request-pairing", text);
        match outcome {
            Outcome::Expired => {
                self.h.webrtc.cancel_pairing(&self.h, &pairing.request_id);
                out.push(failure(
                    ErrorCode::PairingExpired,
                    "The pairing request expired",
                ));
            }
            // The console entry was removed without a PIN.
            Outcome::Entered(Err(_)) => {
                self.h.webrtc.cancel_pairing(&self.h, &pairing.request_id);
                out.push(failure(
                    ErrorCode::PairingCancelled,
                    "The pairing request was cancelled",
                ));
            }
            Outcome::Entered(Ok((entered, name))) => {
                match self
                    .h
                    .webrtc
                    .finish_pairing(&self.h, &pairing.request_id, &entered, &name)
                {
                    Err(code) => {
                        let text = match code {
                            ErrorCode::IncorrectPin => "The PIN does not match the one shown",
                            ErrorCode::PairingExpired => "The pairing request expired",
                            _ => "Pairing failed",
                        };
                        tracing::info!(peer = %self.peer.address, %code, "WebRTC pairing did not complete");
                        out.push(failure(code, text));
                    }
                    Ok(credentials) => {
                        self.challenge.discard();
                        out.push(ServerMessage::paired(
                            credentials.client_id.clone(),
                            credentials.secret.clone(),
                        ));
                        if self
                            .h
                            .webrtc
                            .authenticate(self.id, &credentials.client_id)
                            .is_ok()
                        {
                            self.client = Some(Authenticated {
                                client_id: credentials.client_id,
                                name,
                            });
                            self.startup(out);
                        }
                    }
                }
            }
        }
        Flow::Continue
    }

    /// The messages that follow `authenticated` and `paired`.
    fn startup(&mut self, out: &mut Vec<ServerMessage>) {
        out.push(gateway_status(&self.h, self.peer.local.ip()));
        let (capabilities, probe) = capabilities(&self.h);
        self.probe = Some(probe);
        out.push(capabilities);
    }

    /// `capabilities` again when the encoder probe finished or changed.
    fn refresh_capabilities(&mut self, out: &mut Vec<ServerMessage>) {
        if self.client.is_none() {
            return;
        }
        let (capabilities, probe) = capabilities(&self.h);
        if self.probe != Some(probe) {
            self.probe = Some(probe);
            out.push(capabilities);
        }
    }

    fn end(&mut self) {
        self.cancel_pairing();
        self.h.webrtc.close(self.id);
    }
}

fn gateway_status(h: &Host, local: IpAddr) -> ServerMessage {
    let name = crate::network::host_name(&h.config.read().unwrap());
    let running = h.current_app.lock().unwrap().as_ref().map(|app| app.id);
    ServerMessage::GatewayStatus {
        gateway_name: name,
        sunshine_detected: true,
        sunshine_paired: true,
        session_active: !h.sessions.lock().unwrap().idle(),
        running_app_id: running.map(|id| id.to_string()),
        mac_address: discovery::mac_address(local),
    }
}

/// The encoder probe from the host's codec probe: AV1, AV1 10-bit and HEVC Main10.
fn encoder_probe(codecs: u32) -> EncoderProbe {
    EncoderProbe {
        av1: codecs & 0x10000 != 0,
        av1_10bit: codecs & 0x20000 != 0,
        hevc_main10: codecs & 0x200 != 0,
    }
}

fn capabilities(h: &Host) -> (ServerMessage, EncoderProbe) {
    let probe = encoder_probe(h.codecs.load(std::sync::atomic::Ordering::Acquire));
    let body = HostCapabilities::new(probe, FrameRateOffer::default()).capabilities();
    (ServerMessage::Capabilities(body), probe)
}

/// The configured apps, `id` being the GameStream application ID.
fn app_list(h: &Host) -> Vec<AppEntry> {
    let running = h.current_app.lock().unwrap().as_ref().map(|app| app.id);
    let assets = h.assets.parent().unwrap_or(&h.assets).to_owned();
    h.apps
        .read()
        .unwrap()
        .iter()
        .map(|app| AppEntry {
            id: app.id().to_string(),
            title: app.name.clone(),
            artwork_available: std::fs::metadata(catalog::artwork(app, &assets))
                .is_ok_and(|m| m.len() <= MAX_ARTWORK),
            running: running == Some(app.id()),
        })
        .collect()
}

/// The app's cover as `(mime type, base64)`, if it is a PNG, JPEG or WebP of a sane size.
fn artwork(h: &Host, app_id: &str) -> Option<(&'static str, String)> {
    use base64::Engine;
    let id: u32 = app_id.parse().ok()?;
    let app = h
        .apps
        .read()
        .unwrap()
        .iter()
        .find(|a| a.id() == id || a.aliases.contains(&id))
        .cloned()?;
    let assets = h.assets.parent().unwrap_or(&h.assets);
    let path = catalog::artwork(&app, assets);
    if std::fs::metadata(&path).ok()?.len() > MAX_ARTWORK {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let mime = if bytes.starts_with(b"\x89PNG") {
        "image/png"
    } else if bytes.starts_with(b"\xff\xd8") {
        "image/jpeg"
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else {
        return None;
    };
    Some((
        mime,
        base64::engine::general_purpose::STANDARD.encode(bytes),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::Fixture;
    use butterpollo_core::webrtc::auth::proof_hex;
    use serde_json::{Value, json};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpStream,
    };

    /// A minimal WebSocket client: enough to talk to the server in tests.
    struct Ws {
        stream: TcpStream,
    }

    impl Ws {
        async fn connect(address: SocketAddr) -> Self {
            let mut stream = TcpStream::connect(address).await.unwrap();
            stream
                .write_all(
                    b"GET / HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
                )
                .await
                .unwrap();
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                head.push(stream.read_u8().await.unwrap());
            }
            assert!(
                head.starts_with(b"HTTP/1.1 101"),
                "{}",
                String::from_utf8_lossy(&head)
            );
            Self { stream }
        }

        async fn send(&mut self, message: Value) {
            let payload = message.to_string().into_bytes();
            let mut frame = vec![0x81u8];
            match payload.len() {
                n if n < 126 => frame.push(0x80 | n as u8),
                n => {
                    frame.push(0x80 | 126);
                    frame.extend_from_slice(&(n as u16).to_be_bytes());
                }
            }
            let mask = [0x12u8, 0x34, 0x56, 0x78];
            frame.extend_from_slice(&mask);
            frame.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
            self.stream.write_all(&frame).await.unwrap();
        }

        /// The next message; None once the server closed the socket.
        async fn next(&mut self) -> Option<Value> {
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
                        8 => return None,
                        _ => {}
                    }
                }
            })
            .await
            .expect("a message from the server")
        }

        /// The next message, which must have this type.
        async fn expect(&mut self, kind: &str) -> Value {
            let message = self
                .next()
                .await
                .unwrap_or_else(|| panic!("closed, wanted {kind}"));
            assert_eq!(message["type"], kind, "{message}");
            assert_eq!(message["version"], 2);
            message
        }

        async fn closed(&mut self) {
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
            let f = Fixture::new();
            let running = start_on(f.host.clone(), [127, 0, 0, 1].into(), 0, 0).expect("starts");
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
        ws.send(json!({"version":2,"type":"request-pairing","pin":"4321","clientName":"Living room TV"}))
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
        ws.send(json!({"version":2,"type":"request-pairing","pin":"4321","clientName":"Living room TV"}))
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
    async fn the_ninth_unauthenticated_connection_pushes_out_the_oldest() {
        let server = Server::start();
        let mut sockets = Vec::new();
        for _ in 0..8 {
            sockets.push(server.connect().await.0);
        }
        let mut ninth = server.connect().await.0;
        sockets[0].closed().await;
        // The others stay open: a probe connection may idle.
        ninth.send(json!({"version":2,"type":"get-apps"})).await;
        assert_eq!(ninth.expect("error").await["code"], "not-authenticated");
        sockets[1]
            .send(json!({"version":2,"type":"get-apps"}))
            .await;
        assert_eq!(
            sockets[1].expect("error").await["code"],
            "not-authenticated"
        );
    }

    #[tokio::test]
    async fn a_second_connection_from_the_same_device_replaces_the_first() {
        let server = Server::start();
        let (client_id, secret) = server.pair("TV").await;
        let mut first = sign_in(&server, &client_id, &secret).await;
        let mut second = sign_in(&server, &client_id, &secret).await;
        first.closed().await;
        second.send(json!({"version":2,"type":"get-apps"})).await;
        second.expect("apps").await;
        assert_eq!(server.f.host.webrtc.connections(), 1);
    }

    #[tokio::test]
    async fn streaming_requests_say_not_yet_and_bad_messages_get_errors() {
        let server = Server::start();
        let (client_id, secret) = server.pair("TV").await;
        let mut ws = sign_in(&server, &client_id, &secret).await;
        ws.send(json!({"version":2,"type":"start-session","appId":"0","video":{"width":1920,"height":1080,"fps":60,"codec":"hevc","bitrateKbps":20000,"hdr":false},"audio":{"channels":2}}))
            .await;
        let error = ws.expect("error").await;
        assert_eq!(error["code"], "unsupported-settings");
        assert_eq!(error["requestType"], "start-session");
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
        assert!(start_on(f.host.clone(), [127, 0, 0, 1].into(), port, 0).is_none());
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
        assert!(start(f.host.clone(), [127, 0, 0, 1].into()).is_none());
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
}
