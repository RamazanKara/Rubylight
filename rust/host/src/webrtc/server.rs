//! The WebRTC signalling server: a WebSocket on `webrtc_port`, on its own
//! listener, for TVs and browsers that speak the Moonlight WebRTC protocol
//! (version 2). It pairs devices, authenticates them, lists apps and answers
//! `capabilities`. `start-session`, `switch-session`, `stop-session`,
//! `stop-host-session` and the SDP answer go to the connection's session actor
//! (`session.rs`), which streams over the str0m media transport.
//!
//! The WebSocket is plain `ws://`. Authentication keeps other devices on the
//! network from using or interrupting a stream; it hides nothing. Peers
//! outside the local network are closed before the first message.

use super::{
    clients::{Close, Registered, Status},
    discovery,
    session::{self, Command, Commands, Request},
};
use crate::{
    network::{Reach, reach},
    state::{Host, Shared},
    stream::Media,
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
    rt::{TokioExecutor, TokioIo, TokioTimer},
    server::conn::auto::Builder,
    service::TowerToHyperService,
};
use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    sync::{
        mpsc::{UnboundedSender, unbounded_channel},
        oneshot,
    },
    task::JoinHandle,
};

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

/// How long a connection has to finish its HTTP request and upgrade.
const HANDSHAKE: Duration = Duration::from_secs(10);
/// How long a connection may stay unauthenticated, or until its pairing
/// request runs out if that is later.
const UNAUTHENTICATED: Duration = Duration::from_secs(60);

/// The time limits of a connection; tests shorten them.
#[derive(Clone, Copy)]
struct Limits {
    handshake: Duration,
    unauthenticated: Duration,
    pairing: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            handshake: HANDSHAKE,
            unauthenticated: UNAUTHENTICATED,
            pairing: PAIRING_LIFETIME,
        }
    }
}

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
/// the log, never moved. Without `media` the server pairs and lists apps but
/// cannot stream.
pub fn start(h: Shared, bind: IpAddr, media: Option<Arc<Media>>) -> Option<Running> {
    let config = h.config.read().unwrap().clone();
    if !config.webrtc_server() {
        h.webrtc.set_status(Status::default());
        h.warnings.clear(WARNING);
        return None;
    }
    start_on(h, bind, config.webrtc_port(), discovery::PORT, media)
}

/// [`start`] on chosen ports (tests use ephemeral ones; `0` picks one).
pub fn start_on(
    h: Shared,
    bind: IpAddr,
    port: u16,
    discovery_port: u16,
    media: Option<Arc<Media>>,
) -> Option<Running> {
    start_with(h, bind, port, discovery_port, media, Limits::default())
}

fn start_with(
    h: Shared,
    bind: IpAddr,
    port: u16,
    discovery_port: u16,
    media: Option<Arc<Media>>,
    limits: Limits,
) -> Option<Running> {
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
    tasks.push(tokio::spawn(accept(h, listener, media, limits)));
    Some(Running { tasks })
}

#[derive(Clone)]
struct Context {
    h: Shared,
    limits: Limits,
    media: Option<Arc<Media>>,
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

async fn accept(
    h: Shared,
    listener: tokio::net::TcpListener,
    media: Option<Arc<Media>>,
    limits: Limits,
) {
    let router = Router::new()
        .fallback(get(upgrade))
        .with_state(Context { h, limits, media });
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
            // HTTP/1 only: the WebSocket upgrade needs it, and the version
            // sniffing of the auto builder would wait on a silent peer.
            let mut builder = Builder::new(TokioExecutor::new()).http1_only();
            builder
                .http1()
                .timer(TokioTimer::new())
                .header_read_timeout(limits.handshake);
            // The connection future ends when the upgrade is handed over, so
            // this bounds a peer that never sends its request, sends it
            // slowly, or never completes the upgrade.
            let served = builder.serve_connection_with_upgrades(
                TokioIo::new(socket),
                TowerToHyperService::new(router),
            );
            if tokio::time::timeout(limits.handshake, served)
                .await
                .is_err()
            {
                tracing::debug!(%address, "WebRTC connection did not finish its upgrade in time");
            }
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
        .on_upgrade(move |socket| {
            connection(context.h, context.limits, context.media, peer, socket)
        })
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
    limits: Limits,
    /// When a connection that has not authenticated is closed.
    unauthenticated_until: tokio::time::Instant,
    /// The stream pipeline, and where the session actor's replies go.
    streaming: Option<Streaming>,
    /// The session actor, started with the first session request.
    session: Option<Commands>,
}

struct Streaming {
    media: Arc<Media>,
    replies: UnboundedSender<ServerMessage>,
}

impl Connection {
    fn new(h: Shared, limits: Limits, peer: Peer, id: u64) -> Self {
        Self {
            challenge: h.webrtc.challenge(),
            h,
            id,
            peer,
            client: None,
            pairing: None,
            probe: None,
            limits,
            unauthenticated_until: tokio::time::Instant::now() + limits.unauthenticated,
            streaming: None,
            session: None,
        }
    }
}

async fn connection(
    h: Shared,
    limits: Limits,
    media: Option<Arc<Media>>,
    peer: Peer,
    mut socket: WebSocket,
) {
    let Some(Registered { id, mut closed }) = h.webrtc.open(peer.address.ip().to_canonical())
    else {
        tracing::debug!(peer = %peer.address, "WebRTC connection refused: this peer has all the unauthenticated connections the host allows");
        let _ = socket.send(Message::Close(None)).await;
        return;
    };
    // Dropping the state frees the connection's slots and pairing request,
    // however the task ends: a normal exit, a panic or an abort.
    let mut state = Connection::new(h.clone(), limits, peer, id);
    let (replies, mut replied) = unbounded_channel();
    state.streaming = media.map(|media| Streaming { media, replies });
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
        let idle_until = state
            .client
            .is_none()
            .then_some(state.unauthenticated_until);
        // Biased: a host-side close, a pairing result and the idle limit are
        // answered before a busy peer's next message.
        tokio::select! {
            biased;
            Some(reason) = closed.recv() => {
                tracing::debug!(peer = %peer.address, id, ?reason, "WebRTC connection closed by the host");
                match reason {
                    Close::Replaced | Close::TooManyUnauthenticated => flow = Flow::Close,
                }
            }
            Some(message) = replied.recv() => out.push(message),
            outcome = state.pairing_outcome() => flow = state.finish_pairing(outcome, &mut out),
            _ = idle(idle_until) => {
                tracing::debug!(peer = %peer.address, id, "WebRTC connection closed: not authenticated in time");
                flow = Flow::Close;
            }
            Ok(()) = changed.changed() => flow = state.recheck(&mut out),
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
            _ = tick.tick() => state.refresh_capabilities(&mut out),
        }
    }
    drop(state);
    tracing::debug!(peer = %peer.address, id, "WebRTC connection closed");
}

/// Resolves at `until`; never for a connection that has authenticated.
async fn idle(until: Option<tokio::time::Instant>) {
    match until {
        Some(until) => tokio::time::sleep_until(until).await,
        None => std::future::pending().await,
    }
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
            ClientMessage::StartSession {
                app_id,
                video,
                audio,
            } => self.command(
                kind,
                Command::Start(Request {
                    app_id,
                    video,
                    audio,
                }),
                out,
            ),
            ClientMessage::SwitchSession {
                app_id,
                video,
                audio,
            } => self.command(
                kind,
                Command::Switch(Request {
                    app_id,
                    video,
                    audio,
                }),
                out,
            ),
            ClientMessage::StopSession => match &self.session {
                Some(session) => {
                    session.send(Command::Stop);
                }
                None => out.push(ServerMessage::session_status(SessionState::Idle, None)),
            },
            ClientMessage::StopHostSession => self.command(kind, Command::StopHost, out),
            ClientMessage::Answer { session_id, sdp } => {
                // An answer with no session behind it is late signalling.
                match &self.session {
                    Some(session) => {
                        session.send(Command::Answer { session_id, sdp });
                    }
                    None => tracing::debug!(
                        id = self.id,
                        kind,
                        "WebRTC signalling without a session ignored"
                    ),
                }
            }
            // The host is ICE-lite: it never needs the client's candidates, which it learns from
            // the checks it receives.
            ClientMessage::Candidate { .. } => {
                tracing::debug!(id = self.id, kind, "WebRTC client candidate ignored");
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

    /// Hand a session request to the connection's actor, starting it on first use.
    fn command(&mut self, kind: &str, command: Command, out: &mut Vec<ServerMessage>) {
        let (Some(streaming), Some(client)) = (&self.streaming, &self.client) else {
            out.push(Self::error(
                ErrorCode::InternalError,
                kind,
                "Streaming is not available on this host",
            ));
            return;
        };
        let session = self.session.get_or_insert_with(|| {
            session::spawn(
                self.h.clone(),
                streaming.media.clone(),
                client.client_id.clone(),
                self.peer.address,
                self.peer.local.ip(),
                streaming.replies.clone(),
            )
        });
        if !session.send(command) {
            out.push(Self::error(
                ErrorCode::InternalError,
                kind,
                "The session manager stopped",
            ));
        }
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
        if !self
            .h
            .config
            .read()
            .unwrap()
            .boolean("enable_pairing", true)
        {
            tracing::info!(peer = %self.peer.address, "WebRTC pairing refused: pairing is turned off");
            out.push(Self::error(
                ErrorCode::PairingFailed,
                "request-pairing",
                "Pairing is turned off on this host",
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
        // A new request replaces the connection's old one, and does not
        // raise another notification.
        let again = self.pairing.is_some();
        self.cancel_pairing();
        let (request_id, entered) = self.h.webrtc.begin_pairing(&self.h, pin, name);
        let deadline = tokio::time::Instant::now() + self.limits.pairing;
        self.pairing = Some(Pairing {
            request_id,
            entered,
            deadline,
        });
        // The socket may wait for the answer for as long as the request lives.
        self.unauthenticated_until = self.unauthenticated_until.max(deadline);
        tracing::info!(client = name, peer = %self.peer.address, "WebRTC pairing PIN required in the web interface");
        if !again
            && self
                .h
                .webrtc
                .may_notify(self.peer.address.ip().to_canonical())
        {
            butterpollo_windows::tray::notify(
                "Pairing request",
                &format!("{name} wants to pair. Enter the PIN it shows in the Rubylight console."),
            );
        }
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
}

impl Drop for Connection {
    fn drop(&mut self) {
        // Dropped while unwinding, a second panic would abort the host.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.cancel_pairing();
            self.h.webrtc.close(self.id);
        }));
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

/// The encoder probe from the host's codec probe: AV1, AV1 10-bit and HEVC Main10, and the
/// throughput of the H.264, HEVC and AV1 encoders.
pub(super) fn encoder_probe(codecs: u32, mb_rates: [u64; 3]) -> EncoderProbe {
    EncoderProbe {
        av1: codecs & 0x10000 != 0,
        av1_10bit: codecs & 0x20000 != 0,
        hevc_main10: codecs & 0x200 != 0,
        h264_mb_rate: mb_rates[0],
        hevc_mb_rate: mb_rates[1],
        av1_mb_rate: mb_rates[2],
    }
}

/// `encoder_probe` for what the host has probed so far.
pub(super) fn host_probe(h: &Host) -> EncoderProbe {
    use std::sync::atomic::Ordering::Acquire;
    encoder_probe(
        h.codecs.load(Acquire),
        std::array::from_fn(|i| h.encoder_mb_rates[i].load(Acquire)),
    )
}

/// The frame rates offered per mode. `webrtc_frame_rates = 30,60,90,120` replaces them for every
/// mode, so a test host can stream rates that are not offered by default.
pub(super) fn frame_rate_offer(config: &butterpollo_core::config::Config) -> FrameRateOffer {
    let rates: Vec<u32> = config
        .get("webrtc_frame_rates", "")
        .split(',')
        .filter_map(|rate| rate.trim().parse().ok())
        .collect();
    if rates.is_empty() {
        FrameRateOffer::default()
    } else {
        FrameRateOffer::uniform(&rates)
    }
}

fn capabilities(h: &Host) -> (ServerMessage, EncoderProbe) {
    let probe = host_probe(h);
    let offer = frame_rate_offer(&h.config.read().unwrap());
    let body = HostCapabilities::new(probe, offer).capabilities();
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
pub(crate) mod tests {
    use super::*;
    use crate::state::test_support::Fixture;
    use butterpollo_core::webrtc::auth::proof_hex;
    use serde_json::{Value, json};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpStream,
    };

    /// A minimal WebSocket client: enough to talk to the server in tests.
    pub(crate) struct Ws {
        stream: TcpStream,
    }

    impl Ws {
        pub(crate) async fn connect(address: SocketAddr) -> Self {
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
                        8 => return None,
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
            Self::start_with(Limits::default())
        }

        fn start_with(limits: Limits) -> Self {
            let f = Fixture::new();
            let running = start_with(f.host.clone(), [127, 0, 0, 1].into(), 0, 0, None, limits)
                .expect("starts");
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
    async fn a_connection_task_that_panics_or_is_aborted_frees_its_slots() {
        let f = Fixture::new();
        let h = f.host.clone();
        let peer = Peer {
            address: ([192, 168, 1, 9], 5000).into(),
            local: ([192, 168, 1, 2], 8000).into(),
        };
        let ip = peer.address.ip();
        // A connection with a pairing request, as a TV waiting for its PIN has.
        let claim = |h: &Shared| {
            let Registered { id, closed } = h.webrtc.open(ip).expect("a slot");
            let mut state = Connection::new(h.clone(), Limits::default(), peer, id);
            let (request_id, entered) = h.webrtc.begin_pairing(h, "4321", "TV");
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
        second.send(json!({"version":2,"type":"get-apps"})).await;
        second.expect("apps").await;
        assert_eq!(server.f.host.webrtc.connections(), 1);
    }

    #[tokio::test]
    async fn a_server_without_a_stream_pipeline_refuses_session_requests_and_bad_messages_get_errors()
     {
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
            let body =
                HostCapabilities::new(encoder_probe(bits, [0; 3]), FrameRateOffer::default())
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
}
