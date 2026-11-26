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

#[cfg(test)]
use super::clients::Registered;
use super::{
    clients::{Registry, Status},
    discovery,
};
use axum::{
    Extension, Router,
    extract::{State, WebSocketUpgrade},
    response::{IntoResponse, Response},
    routing::get,
};
use butterpollo_session::{
    catalog,
    webrtc::{
        auth::PAIRING_LIFETIME,
        capabilities::{EncoderProbe, FrameRateOffer, HostCapabilities},
        protocol::{AppEntry, ServerMessage},
    },
};
use butterpollo_stream::{
    network::{self, Throttle, local_network},
    state::{Host, Shared},
    stream::Media,
    webrtc_bridge::{CONSOLE_PROTOCOL, ConsoleSignIn, PLAY_PROTOCOL},
};
use hyper_util::{
    rt::{TokioExecutor, TokioIo, TokioTimer},
    server::conn::auto::Builder,
    service::TowerToHyperService,
};
use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::task::JoinHandle;

mod connection;
mod origin;
mod pairing;

use connection::connection;
#[cfg(test)]
use connection::send_within;
#[cfg(test)]
use connection::{Authenticated, Connection, client_line_is_loud};
#[cfg(test)]
use pairing::{Pairing, name_problem};

/// The console warning code for a server that could not start.
pub const WARNING: &str = "webrtc_server";

/// The most one client message may hold. `client-log` and an SDP answer are
/// the largest messages a client sends.
#[cfg(any(test, feature = "test-support"))]
pub const MAX_MESSAGE: usize = 256 * 1024;
#[cfg(not(any(test, feature = "test-support")))]
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
pub struct Limits {
    pub handshake: Duration,
    pub unauthenticated: Duration,
    pub pairing: Duration,
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
    stop: tokio::sync::watch::Sender<bool>,
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
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
        crate::WebRtcRuntime::for_host(&h)
            .clients
            .set_status(Status::default());
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

pub fn start_with(
    h: Shared,
    bind: IpAddr,
    port: u16,
    discovery_port: u16,
    media: Option<Arc<Media>>,
    limits: Limits,
) -> Option<Running> {
    start_with_cap(h, bind, port, discovery_port, media, limits, MAX_SOCKETS)
}

fn start_with_cap(
    h: Shared,
    bind: IpAddr,
    port: u16,
    discovery_port: u16,
    media: Option<Arc<Media>>,
    limits: Limits,
    max_sockets: usize,
) -> Option<Running> {
    let runtime = crate::WebRtcRuntime::install(&h, media.clone());
    let clients = runtime.clients;
    let mut status = Status {
        enabled: true,
        ..Status::default()
    };
    let listener = match network::tcp((bind, port).into()) {
        Ok(listener) => listener,
        Err(error) => {
            let message = format!(
                "The WebRTC server could not listen on {bind} port {port}: {error:#}. Another program may be using the port; choose another in Settings, Network."
            );
            tracing::error!("{message}");
            h.warnings.set(WARNING, &message);
            status.error = Some(message);
            clients.set_status(status);
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
    clients.set_status(status);
    tracing::info!(%signalling, discovery = ?discovery_address, "WebRTC server listening (unencrypted ws://, local network only)");
    let (stop, stopped) = tokio::sync::watch::channel(false);
    tasks.push(tokio::spawn(accept(
        h,
        clients,
        listener,
        media,
        limits,
        max_sockets,
        stopped,
    )));
    Some(Running { tasks, stop })
}

#[derive(Clone)]
struct Context {
    h: Shared,
    clients: Arc<Registry>,
    limits: Limits,
    media: Option<Arc<Media>>,
    stopped: tokio::sync::watch::Receiver<bool>,
}

/// Where a connection came from and arrived at.
#[derive(Clone, Copy)]
struct Peer {
    address: SocketAddr,
    local: SocketAddr,
    /// The browser runs on this PC ([`network::browser_on_this_pc`]).
    this_pc: bool,
    /// Which socket it opened, and what opened it.
    opened: Opened,
}

/// The socket a connection opened, and for `webrtc_port` what opened it ([`origin::opener`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Opened {
    /// `webrtc_port` (plain `ws://`) from a TV app or a program: it learns the Wake-on-LAN address
    /// in `auth-required`, which the TV's reachability probe uses without signing in.
    PlainApp,
    /// `webrtc_port` from a web page the host trusts: the address only after signing in.
    PlainPage,
    /// The console's or the guest port's TLS socket (`wss://`).
    Tls,
}

impl Opened {
    /// Whether the connection is plain `ws://`, so a secret sent on it crosses the network in
    /// the clear.
    fn plain(self) -> bool {
        self != Self::Tls
    }
}

/// The `csrf_allowed_origins` setting, a JSON list kept as a string.
fn trusted_origins(h: &Host) -> Vec<String> {
    let configured = h
        .config
        .read()
        .unwrap()
        .get("csrf_allowed_origins", "[]")
        .to_owned();
    butterpollo_config::config::Config::parse(&format!("csrf_allowed_origins = {configured}\n"))
        .map(|c| c.list("csrf_allowed_origins"))
        .unwrap_or_default()
}

/// The host-wide limit on the log lines refused pages write.
static REFUSED_PAGES: std::sync::Mutex<Throttle> = std::sync::Mutex::new(Throttle::new());

fn refused_peer(throttle: &mut Throttle, peer: SocketAddr) {
    if let Some(more) = throttle.allow() {
        tracing::warn!(
            %peer,
            more,
            "WebRTC signalling closed a connection from outside the local network"
        );
    }
}

async fn accept(
    h: Shared,
    clients: Arc<Registry>,
    listener: tokio::net::TcpListener,
    media: Option<Arc<Media>>,
    limits: Limits,
    max_sockets: usize,
    stopped: tokio::sync::watch::Receiver<bool>,
) {
    let router = Router::new().fallback(get(upgrade)).with_state(Context {
        h,
        clients,
        limits,
        media,
        stopped,
    });
    let sockets = Arc::new(tokio::sync::Semaphore::new(max_sockets));
    let mut refused = Throttle::new();
    loop {
        let (socket, address) = network::accept(&listener).await;
        if !local_network(address.ip()) {
            refused_peer(&mut refused, address);
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
        // Keep the connection slot permit in request extensions so it passes into `on_upgrade`
        // and spans the WebSocket lifetime rather than dropping at HTTP 101 completion.
        let router = router
            .clone()
            .layer(Extension(Arc::new(permit)))
            .layer(Extension(Peer {
                address,
                local,
                this_pc: false,
                opened: Opened::PlainApp,
            }));
        tokio::spawn(async move {
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
    Extension(permit): Extension<Arc<tokio::sync::OwnedSemaphorePermit>>,
    headers: axum::http::HeaderMap,
    socket: WebSocketUpgrade,
) -> Response {
    let origins: Vec<&str> = headers
        .get_all(axum::http::header::ORIGIN)
        .iter()
        .map(|origin| origin.to_str().unwrap_or(""))
        .collect();
    let pc_name = butterpollo_win_io::net::host_name();
    let opened = match origin::opener(
        origins.iter().copied(),
        &trusted_origins(&context.h),
        pc_name.as_deref(),
    ) {
        origin::Opener::App => Opened::PlainApp,
        origin::Opener::Page => Opened::PlainPage,
        origin::Opener::Refused => {
            let refusal = origin::refusal(&origins.join(", "));
            if let Some(more) = REFUSED_PAGES.lock().unwrap().allow() {
                tracing::warn!(peer = %peer.address, more, "{refusal}");
            }
            return (axum::http::StatusCode::FORBIDDEN, refusal).into_response();
        }
    };
    let peer = Peer {
        this_pc: network::browser_on_this_pc(peer.address, peer.local, &headers),
        opened,
        ..peer
    };
    socket
        .max_message_size(MAX_MESSAGE)
        .max_frame_size(MAX_MESSAGE)
        .on_upgrade(move |socket| async move {
            let _permit = permit;
            connection(
                context.h,
                context.clients,
                context.limits,
                context.media,
                peer,
                None,
                None,
                Some(context.stopped),
                socket,
            )
            .await;
        })
}

/// The close code of a connection a newer connection of the same device replaced (4000-4999 are
/// the application's own).
pub const REPLACED: u16 = 4000;
/// How often a console connection checks that its console sign-in still holds and the WebRTC
/// server is on.
const SIGNED_IN_CHECK: Duration = Duration::from_secs(5);

/// Whether a console sign-in still holds.
type SignedIn = Box<dyn Fn() -> bool + Send + Sync>;

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

impl Console {
    pub fn new(media: Option<Arc<Media>>) -> Self {
        Self {
            media,
            signed_in_check: SIGNED_IN_CHECK,
        }
    }

    /// [`Self::new`] with a shorter sign-in check, for tests.
    #[cfg(any(test, feature = "test-support"))]
    pub fn checking_every(mut self, every: Duration) -> Self {
        self.signed_in_check = every;
        self
    }

    /// Answer the upgrade with [`CONSOLE_PROTOCOL`] and run the connection as already
    /// authenticated: it starts with `authenticated`, `gateway-status` and `capabilities`, and
    /// everything after that is protocol v2 and its extensions, as on `webrtc_port`.
    pub fn upgrade(&self, h: Shared, socket: WebSocketUpgrade, sign_in: ConsoleSignIn) -> Response {
        let media = self.media.clone();
        let clients = crate::WebRtcRuntime::for_host(&h).clients;
        let every = self.signed_in_check;
        let peer = Peer {
            address: SocketAddr::new(sign_in.peer.ip().to_canonical(), sign_in.peer.port()),
            local: SocketAddr::new(sign_in.local.ip().to_canonical(), sign_in.local.port()),
            this_pc: sign_in.this_pc,
            opened: Opened::Tls,
        };
        socket
            .protocols([CONSOLE_PROTOCOL])
            .max_message_size(MAX_MESSAGE)
            .max_frame_size(MAX_MESSAGE)
            .on_upgrade(move |socket| {
                connection(
                    h,
                    clients,
                    Limits::default(),
                    media,
                    peer,
                    Some((sign_in, every)),
                    None,
                    None,
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
        if let Some(reason) = network::play_refused(peer, local) {
            tracing::warn!(%peer, %local, reason, "play socket refused");
            return axum::http::StatusCode::NOT_FOUND.into_response();
        }
        let media = self.media.clone();
        let clients = crate::WebRtcRuntime::for_host(&h).clients;
        let every = self.signed_in_check;
        let peer = Peer {
            address: SocketAddr::new(peer.ip().to_canonical(), peer.port()),
            local: SocketAddr::new(local.ip().to_canonical(), local.port()),
            this_pc: network::browser_on_this_pc(peer, local, headers),
            opened: Opened::Tls,
        };
        socket
            .protocols([PLAY_PROTOCOL])
            .max_message_size(MAX_MESSAGE)
            .max_frame_size(MAX_MESSAGE)
            .on_upgrade(move |socket| {
                connection(
                    h,
                    clients,
                    Limits::default(),
                    media,
                    peer,
                    None,
                    Some(every),
                    None,
                    socket,
                )
            })
    }
}

fn gateway_status(h: &Host, local: IpAddr) -> ServerMessage {
    let name = network::host_name(&h.config.read().unwrap());
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
pub(super) fn frame_rate_offer(config: &butterpollo_config::config::Config) -> FrameRateOffer {
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
    let apps: Vec<_> = h
        .apps
        .read()
        .unwrap()
        .iter()
        .map(|app| (app.id(), app.name.clone(), catalog::artwork(app, &assets)))
        .collect();
    apps.into_iter()
        .map(|(id, title, path)| AppEntry {
            id: id.to_string(),
            title,
            artwork_available: std::fs::metadata(path).is_ok_and(|m| m.len() <= MAX_ARTWORK),
            running: running == Some(id),
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
