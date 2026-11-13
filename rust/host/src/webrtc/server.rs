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
        ws::{CloseFrame, Message, WebSocket, close_code},
    },
    response::{IntoResponse, Response},
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
    /// The browser runs on this PC ([`crate::network::browser_on_this_pc`]).
    this_pc: bool,
}

/// Keeps a repeating warning, such as refused peers of a scan, to one line in ten seconds.
pub(crate) struct Throttle {
    last: Option<Instant>,
    skipped: u32,
}

impl Throttle {
    pub(crate) const fn new() -> Self {
        Self {
            last: None,
            skipped: 0,
        }
    }

    /// Whether to log now, at most once in ten seconds; with how many were skipped since the last
    /// one logged.
    pub(crate) fn allow(&mut self) -> Option<u32> {
        let now = Instant::now();
        if self
            .last
            .is_some_and(|at| now.duration_since(at) < Duration::from_secs(10))
        {
            self.skipped += 1;
            return None;
        }
        self.last = Some(now);
        Some(std::mem::take(&mut self.skipped))
    }

    fn refused(&mut self, peer: SocketAddr) {
        if let Some(more) = self.allow() {
            tracing::warn!(
                %peer,
                more,
                "WebRTC signalling closed a connection from outside the local network"
            );
        }
    }
}

/// Loopback, private, link-local, ULA and carrier-grade NAT addresses; nothing the internet can reach.
pub(crate) fn local_network(peer: IpAddr) -> bool {
    reach(peer.to_canonical()) <= Reach::Lan
}

/// Unicode's format characters (Cf, as of Unicode 15.1) and the line and paragraph separators
/// (Zl, Zp): bidi overrides and isolates reorder the text around them, zero-width ones hide
/// in it, and a separator starts a line that can pass for the notice's own. Refusing U+200D
/// also refuses emoji joined with it.
fn invisible(c: char) -> bool {
    matches!(
        c,
        '\u{00ad}'
            | '\u{0600}'..='\u{0605}'
            | '\u{061c}'
            | '\u{06dd}'
            | '\u{070f}'
            | '\u{0890}'..='\u{0891}'
            | '\u{08e2}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
            | '\u{110bd}'
            | '\u{110cd}'
            | '\u{13430}'..='\u{1343f}'
            | '\u{1bca0}'..='\u{1bca3}'
            | '\u{1d173}'..='\u{1d17a}'
            | '\u{e0001}'
            | '\u{e0020}'..='\u{e007f}'
    )
}

/// What is wrong with a pairing device's name, if anything: a format or separator character
/// ([`invisible`]), a digit other than ASCII 0-9
/// (fullwidth, circled, superscript, math bold, Arabic-Indic and the like, which can spell a PIN
/// the ASCII check misses), or ASCII digits that, read in order with anything between them
/// dropped, contain `pin`.
fn name_problem(pin: &str, name: &str) -> Option<&'static str> {
    if name.chars().any(invisible) {
        return Some("clientName may not contain format or line separator characters");
    }
    if name.chars().any(|c| c.is_numeric() && !c.is_ascii_digit()) {
        return Some("clientName may use only the digits 0-9");
    }
    let digits: String = name.chars().filter(char::is_ascii_digit).collect();
    (!pin.is_empty() && digits.contains(pin)).then_some("clientName must not contain the PIN")
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
    let mut refused = Throttle::new();
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
        // Whether the browser is on this PC is read from its request, in `upgrade`.
        let router = router.clone().layer(Extension(Peer {
            address,
            local,
            this_pc: false,
        }));
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
    headers: axum::http::HeaderMap,
    socket: WebSocketUpgrade,
) -> Response {
    let peer = Peer {
        this_pc: crate::network::browser_on_this_pc(peer.address, peer.local, &headers),
        ..peer
    };
    socket
        .max_message_size(MAX_MESSAGE)
        .max_frame_size(MAX_MESSAGE)
        .on_upgrade(move |socket| {
            connection(context.h, context.limits, context.media, peer, None, socket)
        })
}

/// The close code of a connection a newer connection of the same device replaced (4000-4999 are
/// the application's own).
pub const REPLACED: u16 = 4000;
/// The WebSocket subprotocol of the console's signalling route.
pub const CONSOLE_PROTOCOL: &str = "rubylight.signal.v2";
/// The WebSocket subprotocol of the play page's signalling route on the guest port.
pub const PLAY_PROTOCOL: &str = "rubylight.play.v2";
/// How often a console connection checks that its console sign-in still holds and the WebRTC
/// server is on.
const SIGNED_IN_CHECK: Duration = Duration::from_secs(5);

/// What the console's HTTPS listener needs to hand a signed-in browser to this server: the
/// stream pipeline (None in tests, which then cannot stream) and how often the sign-in is checked.
#[derive(Clone)]
pub struct Console {
    media: Option<Arc<Media>>,
    signed_in_check: Duration,
}

impl Default for Console {
    fn default() -> Self {
        Self::new(None)
    }
}

/// A console browser the console's own sign-in, origin and CSRF checks have let through, as the
/// WebRTC device `client_id`.
pub struct ConsoleSignIn {
    pub client_id: String,
    pub name: String,
    pub peer: SocketAddr,
    pub local: SocketAddr,
    /// The browser runs on this PC ([`crate::network::browser_on_this_pc`]).
    pub this_pc: bool,
    /// Whether the console sign-in that opened the socket still holds (not signed out, not
    /// expired). The connection closes with "signed out" once it does not.
    pub signed_in: SignedIn,
}

/// Whether a console sign-in still holds.
pub type SignedIn = Box<dyn Fn() -> bool + Send>;

impl Console {
    pub fn new(media: Option<Arc<Media>>) -> Self {
        Self {
            media,
            signed_in_check: SIGNED_IN_CHECK,
        }
    }

    /// [`Self::new`] with a shorter sign-in check, for tests.
    #[cfg(test)]
    pub fn checking_every(mut self, every: Duration) -> Self {
        self.signed_in_check = every;
        self
    }

    /// Answer the upgrade with [`CONSOLE_PROTOCOL`] and run the connection as already
    /// authenticated: it starts with `authenticated`, `gateway-status` and `capabilities`, and
    /// everything after that is protocol v2 and its extensions, as on `webrtc_port`.
    pub fn upgrade(&self, h: Shared, socket: WebSocketUpgrade, sign_in: ConsoleSignIn) -> Response {
        let media = self.media.clone();
        let every = self.signed_in_check;
        let peer = Peer {
            address: SocketAddr::new(sign_in.peer.ip().to_canonical(), sign_in.peer.port()),
            local: SocketAddr::new(sign_in.local.ip().to_canonical(), sign_in.local.port()),
            this_pc: sign_in.this_pc,
        };
        socket
            .protocols([CONSOLE_PROTOCOL])
            .max_message_size(MAX_MESSAGE)
            .max_frame_size(MAX_MESSAGE)
            .on_upgrade(move |socket| {
                connection(
                    h,
                    Limits::default(),
                    media,
                    peer,
                    Some((sign_in, every)),
                    socket,
                )
            })
    }

    /// Answer the play page's upgrade on the guest port with [`PLAY_PROTOCOL`] and run the
    /// connection exactly as one on `webrtc_port`: it starts with `auth-required`, and pairs or
    /// authenticates before anything else. A peer outside the local network is refused here too.
    pub fn play(
        &self,
        h: Shared,
        socket: WebSocketUpgrade,
        peer: SocketAddr,
        local: SocketAddr,
        headers: &axum::http::HeaderMap,
    ) -> Response {
        if let Some(reason) = crate::network::play_refused(peer, local) {
            tracing::warn!(%peer, %local, reason, "play socket refused");
            return axum::http::StatusCode::NOT_FOUND.into_response();
        }
        let media = self.media.clone();
        let peer = Peer {
            address: SocketAddr::new(peer.ip().to_canonical(), peer.port()),
            local: SocketAddr::new(local.ip().to_canonical(), local.port()),
            this_pc: crate::network::browser_on_this_pc(peer, local, headers),
        };
        socket
            .protocols([PLAY_PROTOCOL])
            .max_message_size(MAX_MESSAGE)
            .max_frame_size(MAX_MESSAGE)
            .on_upgrade(move |socket| connection(h, Limits::default(), media, peer, None, socket))
    }
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
    entered: oneshot::Receiver<crate::state::PinEntry>,
    deadline: tokio::time::Instant,
}

/// What came of a pairing request.
enum Outcome {
    Entered(Result<crate::state::PinEntry, oneshot::error::RecvError>),
    Expired,
}

struct Connection {
    h: Shared,
    id: u64,
    peer: Peer,
    challenge: Challenge,
    client: Option<Authenticated>,
    pairing: Option<Pairing>,
    /// What the last `capabilities` was built from: the encoder probe and the frame rate offer.
    offered: Option<(EncoderProbe, FrameRateOffer)>,
    limits: Limits,
    /// When a connection that has not authenticated is closed.
    unauthenticated_until: tokio::time::Instant,
    /// The stream pipeline, and where the session actor's replies go.
    streaming: Option<Streaming>,
    /// The session actor, started with the first session request.
    session: Option<Commands>,
    /// This connection's page has said why this session's peer connection failed
    /// ([`client_line_is_loud`]).
    reported_failure: bool,
}

/// Whether a `client-log` line goes in a default log, with how many were kept out of it since the
/// last that went in; the others are for debugging. A page's report that its peer connection failed
/// has what the host can't see, the reason and the browser. A connection gets one such line per
/// session, and a paired page reconnecting in a loop gets no more than `throttle` lets through.
fn client_line_is_loud(line: &str, reported: &mut bool, throttle: &mut Throttle) -> Option<u32> {
    if !line.starts_with("connection failed:") || std::mem::replace(reported, true) {
        return None;
    }
    throttle.allow()
}

/// The host-wide limit on the failures pages report ([`client_line_is_loud`]).
static CLIENT_FAILURES: std::sync::Mutex<Throttle> = std::sync::Mutex::new(Throttle::new());

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
            offered: None,
            limits,
            unauthenticated_until: tokio::time::Instant::now() + limits.unauthenticated,
            streaming: None,
            session: None,
            reported_failure: false,
        }
    }
}

async fn connection(
    h: Shared,
    limits: Limits,
    media: Option<Arc<Media>>,
    peer: Peer,
    console: Option<(ConsoleSignIn, Duration)>,
    mut socket: WebSocket,
) {
    let ip = peer.address.ip().to_canonical();
    let registered = match &console {
        Some((sign_in, _)) => Some(h.webrtc.open_authenticated(ip, &sign_in.client_id)),
        None => h.webrtc.open(ip),
    };
    let Some(Registered { id, mut closed }) = registered else {
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
    let mut out = Vec::new();
    // A console connection is signed in already; its sign-in and the WebRTC server setting are
    // checked every few seconds.
    let mut signed_in = None;
    match console {
        Some((sign_in, every)) => {
            tracing::info!(device = %sign_in.name, peer = %peer.address, "WebRTC console connection opened");
            state.client = Some(Authenticated {
                client_id: sign_in.client_id,
                name: sign_in.name,
            });
            out.push(ServerMessage::Authenticated);
            state.startup(&mut out);
            let mut check = tokio::time::interval(every);
            check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            signed_in = Some((sign_in.signed_in, check));
        }
        None => out.push(state.auth_required()),
    }
    let mut flow = Flow::Continue;
    // The close frame a host-side close sends; none unless it has a reason to give.
    let mut close = None;
    loop {
        if !send(&mut socket, &mut out).await {
            break;
        }
        if flow == Flow::Close {
            let _ = socket.send(Message::Close(close.take())).await;
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
                    // The page tells its user why the stream ended.
                    Close::Replaced => {
                        close = Some(CloseFrame {
                            code: REPLACED,
                            reason: "Opened in another tab or browser".into(),
                        });
                        flow = Flow::Close;
                    }
                    Close::TooManyUnauthenticated => flow = Flow::Close,
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
            reason = console_revoked(&h, &mut signed_in) => {
                tracing::info!(peer = %peer.address, id, reason, "WebRTC console connection closed");
                let _ = send(&mut socket, &mut out).await;
                let _ = socket
                    .send(Message::Close(Some(CloseFrame {
                        code: close_code::POLICY,
                        reason: reason.into(),
                    })))
                    .await;
                break;
            }
        }
    }
    drop(state);
    tracing::debug!(peer = %peer.address, id, "WebRTC connection closed");
}

/// Why a console connection must end, at the first check that finds it: its console sign-in no
/// longer holds, or the WebRTC server was turned off (the console's listener outlives it). Never
/// for a connection that came through `webrtc_port`.
async fn console_revoked(
    h: &Shared,
    check: &mut Option<(SignedIn, tokio::time::Interval)>,
) -> &'static str {
    let Some((signed_in, every)) = check else {
        return std::future::pending().await;
    };
    loop {
        every.tick().await;
        if !signed_in() {
            return "signed out";
        }
        if !h.config.read().unwrap().webrtc_server() {
            return "the WebRTC server is off";
        }
    }
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
            } => {
                // The Stream page tries again on the same socket.
                self.reported_failure = false;
                self.command(
                    kind,
                    Command::Start(Request {
                        app_id,
                        video,
                        audio,
                    }),
                    out,
                )
            }
            ClientMessage::SwitchSession {
                app_id,
                video,
                audio,
            } => {
                self.reported_failure = false;
                self.command(
                    kind,
                    Command::Switch(Request {
                        app_id,
                        video,
                        audio,
                    }),
                    out,
                )
            }
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
                    let loud = client_line_is_loud(
                        &line,
                        &mut self.reported_failure,
                        &mut CLIENT_FAILURES.lock().unwrap(),
                    );
                    match loud {
                        Some(more) => tracing::warn!(device = name, more, "WebRTC client: {line}"),
                        None => tracing::debug!(device = name, "WebRTC client: {line}"),
                    }
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
                self.peer.this_pc,
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
        // The console and the tray show the name: a name carrying the PIN would hand the owner
        // the PIN to type for whoever asks. Refused before the rate limit, so a name to fix
        // costs none of the device's requests.
        if let Some(problem) = name_problem(pin, name) {
            tracing::info!(peer = %self.peer.address, problem, "WebRTC pairing refused: the device name");
            out.push(Self::error(
                ErrorCode::InvalidMessage,
                "request-pairing",
                problem,
            ));
            return;
        }
        if !self
            .h
            .webrtc
            .may_pair(self.peer.address.ip().to_canonical())
        {
            tracing::warn!(peer = %self.peer.address, "WebRTC pairing refused: too many pairing requests from this device in the last minute");
            out.push(Self::error(
                ErrorCode::PairingFailed,
                "request-pairing",
                "Too many pairing requests from this device; wait a minute and try again",
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
        // A new request replaces the connection's old one, and does not
        // raise another notification.
        let again = self.pairing.is_some();
        self.cancel_pairing();
        let address = self.peer.address.ip().to_canonical();
        let (request_id, entered) = self.h.webrtc.begin_pairing(&self.h, pin, name, address);
        let deadline = tokio::time::Instant::now() + self.limits.pairing;
        let notice_id = request_id.clone();
        self.pairing = Some(Pairing {
            request_id,
            entered,
            deadline,
        });
        // The socket may wait for the answer for as long as the request lives.
        self.unauthenticated_until = self.unauthenticated_until.max(deadline);
        tracing::info!(client = name, peer = %self.peer.address, "WebRTC pairing PIN required in the web interface");
        if !again && self.h.webrtc.may_notify(address) {
            let (title, text) = crate::state::pairing_notice(name, address);
            butterpollo_windows::tray::notify_pairing(&notice_id, &title, &text);
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
            Outcome::Entered(Ok((entered, name, _, approver))) => {
                match self.h.webrtc.finish_pairing(
                    &self.h,
                    &pairing.request_id,
                    &entered,
                    &name,
                    approver.as_deref(),
                    self.peer.address.ip(),
                ) {
                    Err(code) => {
                        let text = match code {
                            ErrorCode::IncorrectPin => "The PIN does not match the one shown",
                            ErrorCode::PairingExpired => "The pairing request expired",
                            _ => "Pairing failed",
                        };
                        if code == ErrorCode::IncorrectPin {
                            tracing::warn!(peer = %self.peer.address, "WebRTC pairing failed: the PIN entered in the console is not the one the device shows");
                        } else {
                            tracing::info!(peer = %self.peer.address, %code, "WebRTC pairing did not complete");
                        }
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
        let (capabilities, offered) = capabilities(&self.h);
        self.offered = Some(offered);
        out.push(capabilities);
    }

    /// `capabilities` again when the encoder probe finished or changed, or `webrtc_frame_rates`
    /// changed the rates on offer.
    fn refresh_capabilities(&mut self, out: &mut Vec<ServerMessage>) {
        if self.client.is_none() {
            return;
        }
        let (capabilities, offered) = capabilities(&self.h);
        if self.offered.as_ref() != Some(&offered) {
            self.offered = Some(offered);
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

/// The `capabilities` message, and the probe and offer it was built from.
fn capabilities(h: &Host) -> (ServerMessage, (EncoderProbe, FrameRateOffer)) {
    let probe = host_probe(h);
    let offer = frame_rate_offer(&h.config.read().unwrap());
    let body = HostCapabilities::new(probe, offer.clone()).capabilities();
    (ServerMessage::Capabilities(body), (probe, offer))
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
pub(crate) mod tests;
